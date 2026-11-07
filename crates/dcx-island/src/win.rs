//! Platform window layer for `dcx-island.exe`.
//!
//! # Window model (one window, no viewports)
//!
//! The capsule and the panel live in **one** eframe window with **one** egui
//! context. `dcx-box` was rewritten to this model for a documented reason
//! (`crates/dcx-box/src/win.rs`): a deferred viewport gets asynchronous
//! position commands, pointer coordinates feed back into a window that is
//! moving under the cursor, and `clicked()` never arrives. The island needs
//! text input (task titles), so it must be the *active* window while expanded —
//! the exact case a deferred viewport breaks.
//!
//! The OS window rectangle is therefore always the union of what is painted:
//!
//! * collapsed — exactly the capsule, so no transparent pixel can swallow a
//!   desktop click;
//! * expanded — capsule on top, panel below it, centred on the same axis.
//!
//! The capsule sits at the window's top-centre in both states, so toggling only
//! changes the window's **size** (grows down and symmetrically sideways) and
//! the capsule never moves on screen. No region, no `HTTRANSPARENT` — neither
//! is needed when the window is only as large as its content.
//!
//! # Units
//!
//! Every "point" here is an egui logical point; Win32 speaks physical pixels.
//! The crossing is explicit through `scale` (`dpi / 96`), never implicit. Under
//! per-monitor-v2 awareness at 125 % one point is 1.25 physical pixels, and
//! mixing the two is what made the box button vanish in an earlier version.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use egui::{Pos2, Rect};

pub const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
pub const WS_EX_NOACTIVATE: isize = 0x0800_0000;

const GWLP_WNDPROC: i32 = -4;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_FRAMECHANGED: u32 = 0x0020;
const SWP_SHOWWINDOW: u32 = 0x0040;
const HWND_TOPMOST: isize = -1;

const MONITOR_DEFAULTTOPRIMARY: u32 = 1;
const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;

#[link(name = "user32")]
extern "system" {
    fn CallWindowProcW(prev: isize, hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
    fn SetWindowPos(
        hwnd: isize,
        after: isize,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
    fn GetWindowRect(hwnd: isize, rect: *mut dcx_win32::RECT) -> i32;
    fn FindWindowW(class: *const u16, name: *const u16) -> isize;
    fn GetForegroundWindow() -> isize;
    fn GetClassNameW(hwnd: isize, buf: *mut u16, max: i32) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn MonitorFromWindow(hwnd: isize, flags: u32) -> isize;
    fn GetMonitorInfoW(monitor: isize, info: *mut dcx_win32::MONITORINFO) -> i32;
    fn GetDpiForWindow(hwnd: isize) -> u32;
    fn IsWindow(hwnd: isize) -> i32;
    fn GetDpiForSystem() -> u32;
    fn WindowFromPoint(point: dcx_win32::POINT) -> isize;
}

#[link(name = "shell32")]
extern "system" {
    fn SHQueryUserNotificationState(state: *mut i32) -> i32;
}

/// How often the z-order hit test runs. Four times a second is well under the
/// cost of a repaint and fast enough that a slide-out taskbar is corrected
/// before the eye notices.
const RAISE_POLL: Duration = Duration::from_millis(250);

/// Previous window procedure, installed once on the eframe window.
static OLD_PROC: AtomicIsize = AtomicIsize::new(0);
/// The window exists and the DWM attributes have been applied.
static INSTALLED: Mutex<bool> = Mutex::new(false);

pub fn local_dir() -> std::path::PathBuf {
    dcx_island_config::local_dir()
}

/// Append one diagnostic line. GUI builds have no stdout, so a file is the only
/// trace an unattended failure leaves behind. Rotated by truncation at 256 KB.
pub fn trace(line: &str) {
    let path = local_dir().join("island.log");
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 256 * 1024 {
            let _ = std::fs::remove_file(&path);
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{} {line}", stamp());
    }
}

fn stamp() -> String {
    let mut st = dcx_win32::SYSTEMTIME::default();
    unsafe { dcx_win32::GetLocalTime(&mut st) };
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        st.hour, st.minute, st.second, st.milliseconds
    )
}

/// `%LOCALAPPDATA%\dcx\island_show` — written by the host's hotkey thread.
pub fn touch_show_marker() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default();
    let _ = std::fs::write(local_dir().join("island_show"), now);
}

pub fn show_marker_time() -> Option<SystemTime> {
    let text = std::fs::read_to_string(local_dir().join("island_show")).ok()?;
    let ms: u64 = text.trim().parse().ok()?;
    Some(UNIX_EPOCH + Duration::from_millis(ms))
}

/// Remove the marker so the same stamp cannot raise the window twice.
pub fn clear_show_marker() {
    let _ = std::fs::remove_file(local_dir().join("island_show"));
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Monitor {
    /// Full monitor rectangle in physical pixels.
    pub rect: [i32; 4],
    /// Work area (screen minus an un-hidden taskbar), physical pixels.
    pub work: [i32; 4],
    pub dpi: u32,
}

fn rect_from(rc: &dcx_win32::RECT) -> [i32; 4] {
    [rc.left, rc.top, rc.right, rc.bottom]
}

/// Monitor that owns the window (or the primary one before the window exists).
pub fn monitor(hwnd: isize) -> Monitor {
    let mut info = dcx_win32::MONITORINFO::default();
    info.cbSize = core::mem::size_of::<dcx_win32::MONITORINFO>() as u32;
    let mon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY) };
    let ok = if mon != 0 {
        (unsafe { GetMonitorInfoW(mon, &mut info) }) != 0
    } else {
        false
    };
    if !ok {
        // No monitor information: fall back to the primary screen metrics so a
        // caller never has to deal with an empty rectangle.
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        return Monitor {
            rect: [0, 0, w, h],
            work: [0, 0, w, h],
            dpi: unsafe { GetDpiForSystem() }.max(96),
        };
    }
    let dpi = if hwnd != 0 {
        unsafe { GetDpiForWindow(hwnd) }
    } else {
        unsafe { GetDpiForSystem() }
    }
    .max(96) as u32;
    Monitor {
        rect: rect_from(&info.rcMonitor),
        work: rect_from(&info.rcWork),
        dpi,
    }
}

pub fn window_rect(hwnd: isize) -> Option<[i32; 4]> {
    if hwnd == 0 {
        return None;
    }
    let mut rc = dcx_win32::RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rc) } != 0 {
        Some(rect_from(&rc))
    } else {
        None
    }
}

pub fn is_window(hwnd: isize) -> bool {
    hwnd != 0 && unsafe { IsWindow(hwnd) } != 0
}

pub fn class_name(hwnd: isize) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

/// Classes the "is a full-screen app covering this monitor" test must ignore:
/// the desktop itself, the taskbar, and the shell's full-screen host. Hiding
/// the island on the desktop would hide it forever.
fn is_shell_class(class: &str) -> bool {
    matches!(
        class,
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "Windows.UI.Core.CoreWindow"
            | "ApplicationFrameWindow"
            | "MultitaskingViewFrame"
            | "ForegroundStaging"
            | "XamlExplorerHostIslandWindow"
    )
}

/// Is a full-screen window (video, game, slideshow, RDP session) in front?
///
/// The capsule is topmost, so without this test it would sit on top of a
/// full-screen video with no way to dismiss it.
///
/// The primary signal is the shell's own answer (`SHQueryUserNotificationState`,
/// the same call that decides whether a toast may appear). The rectangle
/// heuristic below is only a fallback: a **maximized** window has exactly the
/// monitor's rectangle when the taskbar auto-hides, so the heuristic alone
/// parked the island — invisible, topmost, nowhere — on a perfectly normal
/// desktop with one maximized window.
pub fn foreground_is_fullscreen(mon: &Monitor, own: isize) -> bool {
    if let Some(park) = shell_says_no_popups() {
        return park;
    }
    let fg = unsafe { GetForegroundWindow() };
    if fg == 0 || fg == own {
        return false;
    }
    let class = class_name(fg);
    if is_shell_class(&class) {
        return false;
    }
    let Some(rc) = window_rect(fg) else {
        return false;
    };
    let (mw, mh) = (
        mon.rect[2] - mon.rect[0],
        mon.rect[3] - mon.rect[1],
    );
    let (ww, wh) = (rc[2] - rc[0], rc[3] - rc[1]);
    // 2 px tolerance: a maximised window is one border-pixel smaller than the
    // monitor on some drivers.
    ww >= mw - 2 && wh >= mh - 2 && rc[0] <= mon.rect[0] + 2 && rc[1] <= mon.rect[1] + 2
}

/// `SHQueryUserNotificationState` → is a full-screen app in the way?
///
/// `QUNS_BUSY` (2), `QUNS_RUNNING_D3D_FULL_SCREEN` (3),
/// `QUNS_PRESENTATION_MODE` (4) and `QUNS_APP` (7) all mean "do not put
/// anything on top". `None` means the query itself failed.
fn shell_says_no_popups() -> Option<bool> {
    let mut state: i32 = 0;
    let hr = unsafe { SHQueryUserNotificationState(&mut state) };
    if hr < 0 {
        return None;
    }
    Some(matches!(state, 2 | 3 | 4 | 7))
}

/// The island's only OS window handle, discovered once.
pub struct Window {
    pub hwnd: isize,
    applied: Option<[i32; 4]>,
    frame_fix_at: Option<std::time::Instant>,
    raise_at: Option<std::time::Instant>,
}

impl Window {
    pub fn new() -> Self {
        Window {
            hwnd: 0,
            applied: None,
            frame_fix_at: None,
            raise_at: None,
        }
    }

    /// eframe 0.29 does not expose the HWND, so it is found by title — the same
    /// approach `dcx-box` uses, and the reason `WIN_TITLE` must stay stable.
    pub fn ensure(&mut self, title: &str) -> bool {
        if self.hwnd != 0 && is_window(self.hwnd) {
            return true;
        }
        let t = dcx_win32::wide(title);
        let hwnd = unsafe { FindWindowW(core::ptr::null(), t.as_ptr()) };
        if hwnd == 0 {
            return false;
        }
        self.hwnd = hwnd;
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, dcx_win32::GWL_EXSTYLE);
            // Tool window: no taskbar button, no Alt-Tab entry. Topmost: above
            // the taskbar even when auto-hide raises the bar.
            let want = (ex | WS_EX_TOOLWINDOW) & !WS_EX_NOACTIVATE as isize;
            if want != ex {
                SetWindowLongPtrW(hwnd, dcx_win32::GWL_EXSTYLE, want);
                SetWindowPos(
                    hwnd,
                    0,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
            install_proc(hwnd);
            fix_dwm(hwnd);
        }
        if let Ok(mut done) = INSTALLED.lock() {
            *done = true;
        }
        trace(&format!("window ready hwnd={hwnd}"));
        true
    }

    /// Place the window at `rect` (physical pixels). Only writes when the
    /// rectangle actually changed: a redundant `SetWindowPos` on a layered
    /// surface is a visible flash.
    pub fn place(&mut self, rect: [i32; 4]) {
        if self.hwnd == 0 {
            return;
        }
        if self.applied == Some(rect) {
            return;
        }
        self.applied = Some(rect);
        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                rect[0],
                rect[1],
                rect[2] - rect[0],
                rect[3] - rect[1],
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
    }

    /// Put the window back on top after something raised itself over it.
    ///
    /// Being topmost is not enough: the auto-hidden taskbar re-raises itself on
    /// every slide-out, and the same thing was true for `dcx-box`. Re-asserting
    /// topmost unconditionally every frame would be a permanent z-order fight,
    /// so this first asks the OS what is actually at the capsule's centre.
    /// `WindowFromPoint` skips hidden windows, which is exactly right: the bar
    /// that just slid away must not keep us re-raising.
    pub fn ensure_raised(&mut self) {
        if self.hwnd == 0 {
            return;
        }
        let Some(rc) = window_rect(self.hwnd) else {
            return;
        };
        let point = dcx_win32::POINT {
            x: (rc[0] + rc[2]) / 2,
            y: (rc[1] + rc[3]) / 2,
        };
        let above = unsafe { WindowFromPoint(point) };
        if above == 0 || above == self.hwnd {
            return;
        }
        trace(&format!(
            "z-order: covered by {} (hwnd={above}); re-raising",
            class_name(above)
        ));
        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// Once a second, re-apply the DWM attributes to every top-level window this
    /// process owns. DWM recomputes the frame on some theme/dpi changes and the
    /// 1 px black border comes back.
    pub fn housekeeping(&mut self) {
        let now = std::time::Instant::now();
        // Cheap hit test, four times a second: `place()` early-returns once the
        // rectangle is stable, so without this the window never re-asserts
        // topmost after the taskbar slides over it.
        if self
            .raise_at
            .map(|t| now.duration_since(t) >= RAISE_POLL)
            .unwrap_or(true)
        {
            self.raise_at = Some(now);
            self.ensure_raised();
        }
        let due = self
            .frame_fix_at
            .map(|t| now.duration_since(t) >= Duration::from_secs(2))
            .unwrap_or(true);
        if !due {
            return;
        }
        self.frame_fix_at = Some(now);
        if self.hwnd != 0 {
            unsafe { fix_dwm(self.hwnd) };
        }
    }

    pub fn scale(&self) -> f32 {
        let dpi = if self.hwnd != 0 {
            unsafe { GetDpiForWindow(self.hwnd) }
        } else {
            unsafe { GetDpiForSystem() }
        };
        (dpi.max(96) as f32) / 96.0
    }

    pub fn monitor(&self) -> Monitor {
        monitor(self.hwnd)
    }
}

unsafe fn fix_dwm(hwnd: isize) {
    // DWM draws a 1 px black frame around a borderless window with the default
    // corner preference; that is what produced the four black corners. It also
    // rounds Win11 windows on its own, which fights a capsule.
    dcx_win32::set_border_color(hwnd, dcx_win32::DWMWA_COLOR_NONE);
    dcx_win32::set_corner_preference(hwnd, dcx_win32::DWMWCP_DONOTROUND);
}

/// Subclass once so the window can (a) forward `WM_DPICHANGED` bookkeeping and
/// (b) keep `WM_NCHITTEST` untouched. The proc intentionally falls through to
/// winit for every message: the island has no click-through band to carve out,
/// and swallowing messages here broke input in an earlier iteration.
fn install_proc(hwnd: isize) {
    unsafe {
        let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, pass_through_proc as *const () as isize);
        if prev != 0 {
            OLD_PROC.store(prev, Ordering::SeqCst);
        }
    }
}

unsafe extern "system" fn pass_through_proc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let prev = OLD_PROC.load(Ordering::SeqCst);
    if prev != 0 {
        unsafe { CallWindowProcW(prev, hwnd, msg, wparam, lparam) }
    } else {
        unsafe { dcx_win32::DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
}

/// Convert a point-space rectangle into physical pixels, snapped outwards.
pub fn to_px(rect: Rect, scale: f32) -> [i32; 4] {
    [
        (rect.left() * scale).round() as i32,
        (rect.top() * scale).round() as i32,
        (rect.right() * scale).round() as i32,
        (rect.bottom() * scale).round() as i32,
    ]
}

pub fn px_rect(rect: [i32; 4], scale: f32) -> Rect {
    Rect::from_min_max(
        Pos2::new(rect[0] as f32 / scale, rect[1] as f32 / scale),
        Pos2::new(rect[2] as f32 / scale, rect[3] as f32 / scale),
    )
}

/// Size a point rectangle so it lands on whole physical pixels: an off-by-half
/// window edge blurs every glyph along it.
pub fn snap(rect: Rect, scale: f32) -> Rect {
    let px = to_px(rect, scale);
    px_rect(px, scale)
}
