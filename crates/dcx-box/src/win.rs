//! Platform window layer for `dcx-box.exe`.
//!
//! # Why this file exists
//!
//! The previous implementation drew the tile in the main eframe window and the
//! panel in a **deferred viewport**. That split is the root cause of "界面不能
//! 拖动": the panel's `update()` ran in a second window whose position was
//! moved every frame through an asynchronous `ViewportCommand`, so egui's local
//! pointer positions fed back into themselves, `drag_started()` was unreliable,
//! and the whole gesture had to be re-implemented from raw OS cursor polling in
//! a frame loop that winit was free to throttle. It also inherited the known
//! "`clicked()` never arrives in a deferred viewport" problem documented in
//! the platform README.
//!
//! The rewrite uses **one** window and **one** egui context:
//!
//! * the OS window is resized/moved synchronously with `SetWindowPos` (no
//!   async viewport commands, no one-frame lag, no rubber-banding while a drag
//!   is in flight);
//! * the window rectangle is the union of the tile and the panel, and a
//!   `WM_NCHITTEST` subclass returns `HTTRANSPARENT` for every pixel outside
//!   those two rectangles, so the empty transparent area never swallows a
//!   click meant for the desktop;
//! * all pointer gestures are read from one place, in the one frame loop.
//!
//! # Units
//!
//! Everything in this module that is called a "point" is an egui point. Under
//! per-monitor-v2 DPI awareness with `pixels_per_point() == 1.0` (forced in
//! `app.rs`), one point is one Win32 *logical* pixel, i.e.
//! `physical_px / scale`. Win32 APIs and `box_slot.txt` speak physical pixels;
//! every crossing is explicit. Conflating the two was a historical bug that
//! made the tile disappear at 125 %.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use egui::{Pos2, Rect, Vec2};

pub const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
const GWLP_WNDPROC: i32 = -4;
const WM_NCHITTEST: u32 = 0x0084;
const HTTRANSPARENT: isize = -1;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_FRAMECHANGED: u32 = 0x0020;

pub const VK_LBUTTON: i32 = 0x01;
pub const VK_SHIFT: i32 = 0x10;
pub const VK_CONTROL: i32 = 0x11;
pub const VK_ESCAPE: i32 = 0x1B;

#[link(name = "user32")]
extern "system" {
    fn GetPhysicalCursorPos(pt: *mut dcx_win32::POINT) -> i32;
    fn CallWindowProcW(prev: isize, hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn SetWindowRgn(hwnd: isize, region: isize, redraw: i32) -> i32;
}

// `SetWindowRgn` needs GDI regions. A window **region** is the only
// click-through mechanism that works across processes: the pixels outside it
// are not part of the window at all, so the window manager hands the click to
// whatever is underneath without asking us. `HTTRANSPARENT` from
// `WM_NCHITTEST` is documented to forward "to windows in the same thread" and
// measurably fails for our always-on-top window — it was swallowing every
// click over the settings window and the notification area.
#[link(name = "gdi32")]
extern "system" {
    fn CreateRectRgn(x1: i32, y1: i32, x2: i32, y2: i32) -> isize;
    fn CreateRoundRectRgn(x1: i32, y1: i32, x2: i32, y2: i32, w: i32, h: i32) -> isize;
    fn CombineRgn(dst: isize, src1: isize, src2: isize, mode: i32) -> i32;
    fn DeleteObject(obj: isize) -> i32;
}

const RGN_OR: i32 = 2;

/// Previous window procedure, installed once on the main window.
static OLD_PROC: AtomicIsize = AtomicIsize::new(0);
/// Rectangles that accept the mouse, in **physical** pixels. Everything else
/// in the (larger) transparent window is click-through.
static HIT: Mutex<HitRects> = Mutex::new(HitRects {
    panel: [0, 0, 0, 0],
    tile: [0, 0, 0, 0],
    active: false,
});

#[derive(Clone, Copy)]
struct HitRects {
    panel: [i32; 4],
    tile: [i32; 4],
    active: bool,
}

fn in_rect(r: [i32; 4], x: i32, y: i32) -> bool {
    x >= r[0] && y >= r[1] && x < r[2] && y < r[3]
}

/// `WM_NCHITTEST` is the only reliable way to make *part* of a layered,
/// borderless, per-pixel-alpha window transparent to the mouse. Letting winit
/// handle it means the whole union rectangle owns every click — which, once
/// the panel has been dragged away from the tile, is most of the screen.
unsafe extern "system" fn hit_test_proc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if msg == WM_NCHITTEST {
        // GET_X_LPARAM / GET_Y_LPARAM: signed 16-bit screen coordinates.
        let x = (lparam & 0xFFFF) as u16 as i16 as i32;
        let y = ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32;
        if let Ok(h) = HIT.lock() {
            if h.active && !in_rect(h.panel, x, y) && !in_rect(h.tile, x, y) {
                return HTTRANSPARENT;
            }
        }
    }
    let prev = OLD_PROC.load(Ordering::Relaxed);
    if prev != 0 {
        CallWindowProcW(prev, hwnd, msg, wparam, lparam)
    } else {
        dcx_win32::DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

/// Cursor in **physical** pixels. `GetPhysicalCursorPos` is immune to the
/// process's DPI-awareness mode and to multi-monitor virtual origins.
pub fn cursor_px() -> Option<Pos2> {
    let mut pt = dcx_win32::POINT::default();
    if unsafe { GetPhysicalCursorPos(&mut pt) } == 0
        && unsafe { dcx_win32::GetCursorPos(&mut pt) } == 0
    {
        return None;
    }
    Some(Pos2::new(pt.x as f32, pt.y as f32))
}

pub fn key_down(vk: i32) -> bool {
    unsafe { dcx_win32::GetAsyncKeyState(vk) & 0x8000u16 as i16 != 0 }
}

pub fn shift_down() -> bool {
    key_down(VK_SHIFT)
}
pub fn ctrl_down() -> bool {
    key_down(VK_CONTROL)
}

/// System DPI scale (1.0 at 100 %, 1.25 at 125 %).
/// The engine's measured DPI, cached for a couple of seconds.
///
/// `scale_for` is called from the frame loop *and* from the drag-ghost
/// viewport; both want the same answer without re-reading a file every frame.
pub fn slot_dpi_cached() -> Option<f32> {
    use std::sync::Mutex;
    static CACHE: Mutex<Option<(std::time::Instant, Option<f32>)>> = Mutex::new(None);
    if let Ok(mut g) = CACHE.lock() {
        if let Some((at, v)) = *g {
            if at.elapsed() < Duration::from_secs(2) {
                return v;
            }
        }
        let v = read_slot().and_then(|(s, _)| s.dpi);
        *g = Some((std::time::Instant::now(), v));
        return v;
    }
    None
}

pub fn scale_for(hwnd: isize, slot_dpi: Option<f32>) -> f32 {
    let window = if hwnd != 0 {
        unsafe { dcx_win32::GetDpiForWindow(hwnd) }
    } else {
        0
    } as f32;
    let system = unsafe { dcx_win32::GetDpiForSystem() } as f32;
    // A window parked off-screen (the closed panel lives at -32000) has no
    // monitor association and reports 96 even on a 125 % display. Taking the
    // larger keeps the first frame after opening at the right scale instead of
    // drawing one frame at 80 % and then snapping to full size.
    let mut dpi = window.max(system).max(96.0);
    // Both queries return exactly 96 in a DPI-unaware process, which is
    // indistinguishable from a genuine 100 % display. The engine measures the
    // taskbar in device pixels and records the scale it saw; trust it **only**
    // as an upward correction, so a real 100 % monitor is never forced to the
    // taskbar's 125 %.
    if let Some(s) = slot_dpi {
        let measured = s * 96.0;
        if measured > dpi && dpi <= 96.5 {
            dpi = measured;
        }
    }
    (dpi / 96.0).max(1.0)
}

/// Work area in points (excludes the taskbar).
pub fn work_area(scale: f32) -> Rect {
    let w = unsafe { dcx_win32::GetSystemMetrics(0) } as f32 / scale;
    let h = unsafe { dcx_win32::GetSystemMetrics(1) } as f32 / scale;
    Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h))
}

/// Physical screen size, for the auto-hide test.
fn screen_h_px() -> i32 {
    unsafe { dcx_win32::GetSystemMetrics(1) }
}

/// Whole primary screen in points (includes the taskbar band), used to clamp
/// a resize so a panel edge cannot be dragged off-screen.
pub fn screen_rect(scale: f32) -> Rect {
    work_area(scale)
}

pub fn pt_rect(rc: &dcx_win32::RECT, scale: f32) -> Rect {
    Rect::from_min_max(
        Pos2::new(rc.left as f32 / scale, rc.top as f32 / scale),
        Pos2::new(rc.right as f32 / scale, rc.bottom as f32 / scale),
    )
}

fn to_px(r: Rect, scale: f32) -> [i32; 4] {
    [
        (r.min.x * scale).round() as i32,
        (r.min.y * scale).round() as i32,
        (r.max.x * scale).round() as i32,
        (r.max.y * scale).round() as i32,
    ]
}

/// Taskbar geometry for one frame. All rectangles are in points.
#[derive(Clone, Copy, Debug)]
pub struct Taskbar {
    pub rect: Rect,
    /// Left edge of the notification area, in points.
    pub tray_left: f32,
    /// Bar window height in physical pixels (to sanity-check the engine's slot
    /// file). Not the visible row: the shell window stays 48 DIP even when the
    /// engine shrinks the dock inside it.
    pub bar_h_px: i32,
    /// Height of the task list row in physical pixels. The engine sets the XAML
    /// `RootGrid` height to `taskbar_height`, so this is the visible dock/button
    /// row and is the right reference for the drawer button and its plate.
    pub row_h_px: i32,
    /// Auto-hide parks the bar off-screen with only a ~2 px sliver left on the
    /// desktop edge. The rectangle is still the bar's real one (it keeps its
    /// full height), so the button can ride the slide; `parked` only says the
    /// bar is at its resting place and nothing of it is really visible.
    pub parked: bool,
}

pub fn taskbar(scale: f32) -> Option<Taskbar> {
    let cls = dcx_win32::wide("Shell_TrayWnd");
    let bar = unsafe { dcx_win32::FindWindowW(cls.as_ptr(), core::ptr::null()) };
    if bar == 0 {
        return None;
    }
    let mut rc = dcx_win32::RECT::default();
    if unsafe { dcx_win32::GetWindowRect(bar, &mut rc) } == 0 || rc.right <= rc.left {
        return None;
    }
    // Auto-hide slides the whole window down (top = screen bottom - 2 px), it
    // does not resize it: the visible sliver is what tells the two resting
    // positions apart, and during the slide it is anything in between.
    let sliver_px = screen_h_px() - rc.top;
    let parked = sliver_px <= 3;
    let ncls = dcx_win32::wide("TrayNotifyWnd");
    let notify = unsafe { dcx_win32::FindWindowExW(bar, 0, ncls.as_ptr(), core::ptr::null()) };
    let mut nr = dcx_win32::RECT::default();
    let tray_left = if notify != 0 && unsafe { dcx_win32::GetWindowRect(notify, &mut nr) } != 0 {
        nr.left as f32 / scale
    } else {
        rc.right as f32 / scale - 240.0
    };
    // The task list is where the engine applies `taskbar_height`; the shell
    // window around it is fixed. `ReBarWindow32` is the same row on builds
    // where the task list is nested differently.
    let row_h_px = ["MSTaskListWClass", "ReBarWindow32"]
        .iter()
        .find_map(|name| {
            let class = dcx_win32::wide(name);
            let h = unsafe { dcx_win32::FindWindowExW(bar, 0, class.as_ptr(), core::ptr::null()) };
            if h == 0 {
                return None;
            }
            let mut r = dcx_win32::RECT::default();
            (unsafe { dcx_win32::GetWindowRect(h, &mut r) } != 0 && r.height() > 0)
                .then_some(r.height())
        })
        .unwrap_or(rc.height());
    Some(Taskbar {
        rect: pt_rect(&rc, scale),
        tray_left,
        bar_h_px: rc.height(),
        row_h_px,
        parked,
    })
}

/// Is the shell's taskbar set to auto-hide right now?
///
/// The bar's rectangle alone cannot answer this: while an auto-hidden bar is
/// out, its rect is identical to a permanently visible one.
pub fn taskbar_auto_hide() -> bool {
    let mut data = dcx_win32::APPBARDATA::default();
    data.cbSize = core::mem::size_of::<dcx_win32::APPBARDATA>() as u32;
    let state = unsafe { dcx_win32::SHAppBarMessage(dcx_win32::ABM_GETSTATE, &mut data) };
    (state as u32 & dcx_win32::ABS_AUTOHIDE) != 0
}

// --- what is in front of the bar -------------------------------------------

#[link(name = "shell32")]
extern "system" {
    fn SHQueryUserNotificationState(state: *mut i32) -> i32;
}

/// Classes the "covers the monitor" test must ignore: the desktop, the bar and
/// the shell's own full-screen hosts (Task View). Hiding the button on the
/// desktop or in Task View would hide it for no reason — the bar is still there.
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

fn class_of(hwnd: isize) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { dcx_win32::GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

/// The shell says "nothing of yours belongs on top": a full-screen app (D3D or
/// borderless) or presentation mode.
///
/// `QUNS_BUSY` (2), `QUNS_RUNNING_D3D_FULL_SCREEN` (3) and
/// `QUNS_PRESENTATION_MODE` (4) qualify. `QUNS_APP` (7) does **not**: it only
/// means a Store app owns the foreground, and the Start menu reports it while
/// open — the button lives on the bar and has to stay put. A failed query reads
/// as "not busy": the button must not disappear because one HRESULT came back
/// negative.
pub fn shell_busy() -> bool {
    let mut state: i32 = 0;
    let hr = unsafe { SHQueryUserNotificationState(&mut state) };
    hr >= 0 && matches!(state, 2 | 3 | 4)
}

/// A **topmost** window covers the bar's monitor and owns the screen.
///
/// Only a topmost window can get above the bar — the bar is topmost itself — so
/// this is exactly the topmost full-screen content (video players, games,
/// borderless player modes) where the button would otherwise be the only thing
/// left floating over the picture. A plain full-screen window keeps the bar and
/// the button visible, and with an auto-hidden taskbar a **maximised** window
/// covers the monitor exactly: while the bar slides out the button has to come
/// back with it, not stay hidden.
pub fn foreground_covers_bar(own: isize) -> bool {
    let fg = unsafe { dcx_win32::GetForegroundWindow() };
    if fg == 0 || fg == own || is_shell_class(&class_of(fg)) {
        return false;
    }
    if unsafe { dcx_win32::ex_style(fg) } & dcx_win32::WS_EX_TOPMOST == 0 {
        return false;
    }
    let cls = dcx_win32::wide("Shell_TrayWnd");
    let bar = unsafe { dcx_win32::FindWindowW(cls.as_ptr(), core::ptr::null()) };
    let anchor = if bar != 0 { bar } else { fg };
    let mon = unsafe { dcx_win32::MonitorFromWindow(anchor, dcx_win32::MONITOR_DEFAULTTONEAREST) };
    let mut info = dcx_win32::MONITORINFO::default();
    info.cbSize = core::mem::size_of::<dcx_win32::MONITORINFO>() as u32;
    if mon == 0 || unsafe { dcx_win32::GetMonitorInfoW(mon, &mut info) } == 0 {
        return false;
    }
    let mut rc = dcx_win32::RECT::default();
    if unsafe { dcx_win32::GetWindowRect(fg, &mut rc) } == 0 {
        return false;
    }
    // 2 px tolerance: a maximised window is one border-pixel smaller than the
    // monitor on some drivers.
    let m = info.rcMonitor;
    rc.left <= m.left + 2
        && rc.top <= m.top + 2
        && rc.right >= m.right - 2
        && rc.bottom >= m.bottom - 2
}

/// dcx-snip's capture overlay is up.
///
/// Its frozen frame was grabbed **before** the overlay appeared and already
/// contains the button. The tile is topmost with a guard that re-raises it when
/// another process covers its pixels, so it would be painted a second time
/// above the dimmed frame — and the anchor can have moved since the freeze, so
/// the live copy lands next to the frozen one: "the icon sticks out and is
/// misaligned".
pub fn snip_overlay_open() -> bool {
    let cls = dcx_win32::wide("DCX.Snip.Overlay");
    let h = unsafe { dcx_win32::FindWindowW(cls.as_ptr(), core::ptr::null()) };
    h != 0 && unsafe { dcx_win32::IsWindowVisible(h) } != 0
}

/// The engine's measurement of the taskbar row, in **physical** pixels.
#[derive(Clone, Copy, Debug)]
pub struct Slot {
    /// Right edge of the last app button.
    pub right: f32,
    /// Left edge of the tray.
    pub limit: f32,
    pub bar_h: f32,
    pub n: usize,
    pub reserved: bool,
    /// DPI scale the engine measured on the taskbar's monitor (1.25 = 125 %).
    /// `None` for engine builds that predate the field.
    pub dpi: Option<f32>,
}

pub fn slot_file() -> std::path::PathBuf {
    let mut p = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push("dcx");
    p.push("box_slot.txt");
    p
}

/// Latest reading published by the slot watcher. The frame loop reads this
/// instead of the file: the watcher polls at timer rate, the loop must see a
/// new value on the very frame it is woken for.
static LATEST_SLOT: Mutex<Option<Slot>> = Mutex::new(None);

pub fn latest_slot() -> Option<Slot> {
    LATEST_SLOT.lock().ok().and_then(|g| *g)
}

pub fn set_latest_slot(s: Slot) {
    if let Ok(mut g) = LATEST_SLOT.lock() {
        *g = Some(s);
    }
}

/// Read the engine's slot file. Age is returned, not enforced: when the bar is
/// auto-hidden the engine cannot measure, so a minutes-old file is normal and
/// still perfectly valid. The caller keeps the last good reading.
pub fn read_slot() -> Option<(Slot, Duration)> {
    let path = slot_file();
    let meta = std::fs::metadata(&path).ok()?;
    let age = meta.modified().ok()?.elapsed().ok()?;
    if age > Duration::from_secs(3600) {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let (mut left, mut limit, mut bar_h, mut n, mut reserved, mut dpi) =
        (None, None, None, None, false, None);
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "left" => left = v.parse::<f32>().ok(),
            "limit" => limit = v.parse::<f32>().ok(),
            "bar_h" => bar_h = v.parse::<f32>().ok(),
            "n" => n = v.parse::<usize>().ok(),
            "reserved" => reserved = v == "1",
            "dpi" => dpi = v.parse::<f32>().ok(),
            _ => {}
        }
    }
    let (right, limit, bar_h) = (left?, limit?, bar_h?);
    if bar_h < 20.0 {
        return None;
    }
    Some((
        Slot {
            right,
            limit,
            bar_h,
            n: n.unwrap_or(0),
            reserved,
            dpi,
        },
        age,
    ))
}

/// Owned OS window handle plus the small amount of Win32 state the box needs.
pub struct Window {
    pub hwnd: isize,
    installed: bool,
    applied: Option<[i32; 4]>,
    place_frames: u32,
    frame_fix_at: std::time::Instant,
    /// Last region written, in window-local physical pixels. Re-writing a region
    /// makes the window manager rebuild its shape, so it is only touched when the
    /// rectangles actually change (once per drag frame at most).
    region_key: Option<Vec<([i32; 4], i32)>>,
}

impl Window {
    pub fn new() -> Self {
        Window {
            hwnd: 0,
            installed: false,
            applied: None,
            place_frames: 0,
            frame_fix_at: std::time::Instant::now() - Duration::from_secs(10),
            region_key: None,
        }
    }

    /// winit creates the window a few frames after `run_native` starts. Find it
    /// by title (eframe 0.29 does not expose the HWND), then install everything
    /// that needs a real HWND exactly once.
    pub fn ensure(&mut self, title: &str) -> bool {
        if self.hwnd != 0 && self.installed {
            return true;
        }
        if self.hwnd == 0 {
            let t = dcx_win32::wide(title);
            let hwnd = unsafe { dcx_win32::FindWindowW(core::ptr::null(), t.as_ptr()) };
            if hwnd == 0 {
                return false;
            }
            self.hwnd = hwnd;
        }
        let hwnd = self.hwnd;
        unsafe {
            let ex = dcx_win32::GetWindowLongPtrW(hwnd, dcx_win32::GWL_EXSTYLE);
            if ex & WS_EX_TOOLWINDOW == 0 {
                dcx_win32::SetWindowLongPtrW(hwnd, dcx_win32::GWL_EXSTYLE, ex | WS_EX_TOOLWINDOW);
                dcx_win32::SetWindowPos(
                    hwnd,
                    0,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
            let prev = dcx_win32::SetWindowLongPtrW(
                hwnd,
                GWLP_WNDPROC,
                hit_test_proc as *const () as isize,
            );
            if prev == 0 {
                // Subclassing failed. Do NOT leave the window pointing at a
                // proc that falls through to DefWindowProcW — that would kill
                // every winit message. Retry on the next frame.
                return false;
            }
            OLD_PROC.store(prev, Ordering::Relaxed);
            self.fix_dwm(hwnd);
        }
        self.installed = true;
        true
    }

    unsafe fn fix_dwm(&self, hwnd: isize) {
        // DWM otherwise composites a 1 px *black* border on borderless windows
        // with its default corner preference — the four black corners in the
        // old screenshots. Also stops Win11 from rounding our rounded panel.
        dcx_win32::set_border_color(hwnd, dcx_win32::DWMWA_COLOR_NONE);
        dcx_win32::set_corner_preference(hwnd, dcx_win32::DWMWCP_DONOTROUND);
    }

    /// Re-apply DWM attributes to every top-level window this process owns
    /// (the deferred drag ghost is recreated on demand). Once a second.
    pub fn housekeeping(&mut self) {
        if self.frame_fix_at.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.frame_fix_at = std::time::Instant::now();
        unsafe { dcx_win32::EnumWindows(Some(clear_frame_cb), 0) };
    }

    /// Move and resize the OS window **synchronously**. This is the single most
    /// important difference from the old deferred-viewport design: egui starts
    /// the next frame with the window already where the layout expects it.
    ///
    /// `move_only` keeps the previous width/height. Dragging the panel changes
    /// the union rectangle every frame; resizing a per-pixel-alpha surface that
    /// often reallocates it and flashes. During a drag the window keeps one
    /// size and only translates, and the tile is not painted — zero resize for
    /// the whole gesture.
    pub fn place(&mut self, rect: Rect, scale: f32, move_only: bool) {
        if self.hwnd == 0 {
            return;
        }
        let px = to_px(rect, scale);
        let final_px = if move_only {
            match self.applied {
                Some(prev) => [
                    px[0],
                    px[1],
                    px[0] + (prev[2] - prev[0]),
                    px[1] + (prev[3] - prev[1]),
                ],
                None => px,
            }
        } else {
            px
        };
        self.place_frames = self.place_frames.wrapping_add(1);
        if self.applied == Some(final_px) {
            // Once a second, verify the OS actually kept what we asked for.
            // DPI changes, `set_pixels_per_point` and shell animations can all
            // resize the window behind our back; without this check the
            // `applied` cache would leave it at the wrong size forever.
            let periodic = self.place_frames % 60 == 0;
            if !periodic {
                return;
            }
            let mut rc = dcx_win32::RECT::default();
            let ok = unsafe { dcx_win32::GetWindowRect(self.hwnd, &mut rc) } != 0;
            if ok
                && (rc.left - final_px[0]).abs() <= 2
                && (rc.top - final_px[1]).abs() <= 2
                && (rc.right - final_px[2]).abs() <= 2
                && (rc.bottom - final_px[3]).abs() <= 2
            {
                return;
            }
        }
        self.applied = Some(final_px);
        let (w, h) = (final_px[2] - final_px[0], final_px[3] - final_px[1]);
        unsafe {
            dcx_win32::SetWindowPos(
                self.hwnd,
                0,
                final_px[0],
                final_px[1],
                w.max(1),
                h.max(1),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    /// Re-assert topmost when another process owns the panel's pixels.
    ///
    /// The panel is topmost, but so are the shell's bar, flyouts and whatever
    /// else installs an always-on-top overlay; any of them can be raised above
    /// us. `place()` deliberately never touches the z-order (it would fight the
    /// shell), so nothing put the panel back — it was simply buried until it
    /// closed. Same rule as the taskbar button: raise only on a real conflict.
    pub fn ensure_raised(&self) {
        if self.hwnd == 0 {
            return;
        }
        let mut rc = dcx_win32::RECT::default();
        if unsafe { dcx_win32::GetWindowRect(self.hwnd, &mut rc) } == 0 {
            return;
        }
        // Parked at -32000 while the panel is closed.
        if rc.left < -30000 {
            return;
        }
        let pt = dcx_win32::POINT {
            x: (rc.left + rc.right) / 2,
            y: (rc.top + rc.bottom) / 2,
        };
        let hit = unsafe { dcx_win32::WindowFromPoint(pt) };
        if hit == 0 || hit == self.hwnd {
            return;
        }
        let root = unsafe { dcx_win32::GetAncestor(hit, dcx_win32::GA_ROOT) };
        if root == self.hwnd {
            return;
        }
        // Our own windows (the drag ghost, a popup spawned as its own window)
        // may legitimately sit on top.
        let mut pid: u32 = 0;
        unsafe { dcx_win32::GetWindowThreadProcessId(root, &mut pid) };
        if pid == unsafe { dcx_win32::GetCurrentProcessId() } {
            return;
        }
        unsafe {
            dcx_win32::SetWindowPos(
                self.hwnd,
                dcx_win32::HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// Window's true on-screen origin in points. `SetWindowPos` is synchronous,
    /// so this equals the requested origin — reading it back keeps the drawing
    /// correct even if DWM or the shell adjusted the rectangle.
    pub fn origin(&self, scale: f32) -> Option<Pos2> {
        if self.hwnd == 0 {
            return None;
        }
        let mut rc = dcx_win32::RECT::default();
        if unsafe { dcx_win32::GetWindowRect(self.hwnd, &mut rc) } == 0 {
            return None;
        }
        Some(Pos2::new(rc.left as f32 / scale, rc.top as f32 / scale))
    }

    /// Publish the two clickable rectangles, in physical pixels.
    pub fn set_hit(&self, panel: Rect, tile: Rect, scale: f32) {
        if let Ok(mut h) = HIT.lock() {
            h.panel = to_px(panel, scale);
            h.tile = to_px(tile, scale);
            h.active = true;
        }
    }

    pub fn disable_hit(&self) {
        if let Ok(mut h) = HIT.lock() {
            h.active = false;
        }
    }

    /// Shape the window to the rectangles that actually draw something.
    ///
    /// Everything outside the region stops being part of the window: clicks,
    /// wheel messages and hit tests go to whatever is below (desktop, taskbar,
    /// the settings window). This is what makes a panel dragged away from the
    /// tile stop blocking half the screen, and it replaces the old
    /// `HTTRANSPARENT` trick, which only forwards to windows in the *same*
    /// thread.
    ///
    /// Rectangles are window-local points; the radius is the corner radius the
    /// surface is painted with, so the region matches the visible shape instead
    /// of its bounding box.
    pub fn set_region(&mut self, rects: &[(Rect, f32)], scale: f32) {
        if self.hwnd == 0 {
            return;
        }
        let mut px: Vec<([i32; 4], i32)> = rects
            .iter()
            .map(|(r, radius)| (to_px(*r, scale), (radius * scale).round().max(0.0) as i32))
            .filter(|(r, _)| r[2] > r[0] && r[3] > r[1])
            .collect();
        if px.is_empty() {
            return;
        }
        px.sort_unstable();
        if self.region_key.as_deref() == Some(px.as_slice()) {
            return;
        }
        unsafe {
            let acc = CreateRectRgn(0, 0, 0, 0);
            if acc == 0 {
                return;
            }
            for (r, rad) in &px {
                let one = if *rad > 0 {
                    CreateRoundRectRgn(r[0], r[1], r[2], r[3], rad * 2, rad * 2)
                } else {
                    CreateRectRgn(r[0], r[1], r[2], r[3])
                };
                if one != 0 {
                    CombineRgn(acc, acc, one, RGN_OR);
                    DeleteObject(one);
                }
            }
            // On success the system owns the region; deleting it would be a
            // use-after-free inside the window manager.
            if SetWindowRgn(self.hwnd, acc, 1) == 0 {
                DeleteObject(acc);
            } else {
                self.region_key = Some(px);
            }
        }
    }

    /// Drop the shape (used while the window is parked off-screen).
    pub fn clear_region(&mut self) {
        if self.hwnd != 0 && self.region_key.is_some() {
            unsafe { SetWindowRgn(self.hwnd, 0, 1) };
        }
        self.region_key = None;
    }
}

unsafe extern "system" fn clear_frame_cb(hwnd: isize, _: isize) -> i32 {
    // Each window gets these attributes **once**. `housekeeping` runs every
    // second to catch windows that appear later (the drag ghost), but
    // re-applying them to windows that already have them made DWM re-composite
    // the panel's rounded corners for a frame — visible as the window
    // flickering while it was being clicked.
    static SEEN: Mutex<Vec<isize>> = Mutex::new(Vec::new());
    let mut pid: u32 = 0;
    dcx_win32::GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == unsafe { dcx_win32::GetCurrentProcessId() } {
        let fresh = match SEEN.lock() {
            Ok(mut seen) => {
                if seen.contains(&hwnd) {
                    false
                } else {
                    // Hwnds are reused after a window dies; a bounded list is
                    // enough to avoid unbounded growth without missing a live
                    // window for long.
                    if seen.len() >= 32 {
                        seen.clear();
                    }
                    seen.push(hwnd);
                    true
                }
            }
            Err(_) => false,
        };
        if !fresh {
            return 1;
        }
        dcx_win32::set_border_color(hwnd, dcx_win32::DWMWA_COLOR_NONE);
        dcx_win32::set_corner_preference(hwnd, dcx_win32::DWMWCP_DONOTROUND);
        let ex = dcx_win32::GetWindowLongPtrW(hwnd, dcx_win32::GWL_EXSTYLE);
        if ex & WS_EX_TOOLWINDOW == 0 {
            dcx_win32::SetWindowLongPtrW(hwnd, dcx_win32::GWL_EXSTYLE, ex | WS_EX_TOOLWINDOW);
        }
    }
    1
}

/// Snap a rectangle to whole device pixels so borders do not smear at 125 %.
pub fn snap(r: Rect, scale: f32) -> Rect {
    let q = |v: f32| (v * scale).round() / scale;
    Rect::from_min_max(
        Pos2::new(q(r.min.x), q(r.min.y)),
        Pos2::new(q(r.max.x), q(r.max.y)),
    )
}

/// The runtime state file: panel position/size and the tile's last screen
/// position. The settings centre reads `tile_x`/`tile_y` to implement
/// 「锁定当前位置」 without any new IPC channel.
pub fn ui_state_file() -> std::path::PathBuf {
    let mut p = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push("dcx");
    p.push("box_ui.txt");
    p
}

pub fn read_ui_state() -> std::collections::HashMap<String, f32> {
    let mut out = std::collections::HashMap::new();
    if let Ok(text) = std::fs::read_to_string(ui_state_file()) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                if let Ok(n) = v.trim().parse::<f32>() {
                    out.insert(k.trim().to_string(), n);
                }
            }
        }
    }
    out
}

pub fn write_ui_state(panel: Option<Pos2>, tile: Pos2, scale: f32) {
    let path = ui_state_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut text =
        String::from("# dcx — 收纳盒界面状态（屏幕坐标：点为逻辑像素，tile 为物理像素）\n");
    match panel {
        Some(p) => text.push_str(&format!("panel_x={:.1}\npanel_y={:.1}\n", p.x, p.y)),
        None => text.push_str("# 面板跟随任务栏按钮\n"),
    }
    text.push_str(&format!(
        "tile_x={:.0}\ntile_y={:.0}\n",
        tile.x * scale,
        tile.y * scale
    ));
    let tmp = path.with_extension("txt.tmp");
    if std::fs::write(&tmp, text.as_bytes()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Escape hatch consumed by the injected engine: while this file exists the
/// engine stops reserving the taskbar band. Used when the feature is off.
pub fn touch_no_reserve(set: bool) {
    let mut p = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push("dcx");
    p.push("no_reserve");
    if set {
        let _ = std::fs::write(&p, b"1");
    } else {
        let _ = std::fs::remove_file(&p);
    }
}

pub fn heartbeat() {
    let Some(mut p) = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from) else {
        return;
    };
    p.push("dcx");
    p.push("box_host_alive");
    let _ = std::fs::write(&p, b"1");
}

/// Diagnostic trace. A GUI-subsystem build has no console, so `println!` is
/// lost; this file is the only way to see the drag/anchor state machine.
pub fn trace(line: &str) {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    let ms = START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis();
    let mut p = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push("dcx");
    p.push("box_trace.log");
    let truncate = p.metadata().map(|m| m.len() >= 128 * 1024).unwrap_or(false);
    // `truncate` needs write access; the old `.append(!truncate)` form opened
    // read-only once the log crossed 128 KiB, so tracing died silently.
    let opened = if truncate {
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&p)
    } else {
        std::fs::OpenOptions::new().create(true).append(true).open(&p)
    };
    if let Ok(mut f) = opened {
        use std::io::Write;
        let _ = writeln!(f, "[dcx-box +{ms}ms] {line}");
    }
}

static PROCESS_START: OnceLock<SystemTime> = OnceLock::new();

/// Call once in `main`, before the window exists: the host can touch
/// `box_show` before this process finishes starting, and the comparison
/// against this timestamp is how the app tells "start closed" from "the
/// hotkey just launched me".
pub fn mark_process_start() {
    let _ = PROCESS_START.set(SystemTime::now());
}

pub fn process_started() -> SystemTime {
    *PROCESS_START.get_or_init(SystemTime::now)
}

pub fn show_marker_stamp() -> u128 {
    std::fs::metadata(dcx_win32::dcx_state_file("box_show"))
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

pub fn show_marker_time() -> Option<SystemTime> {
    std::fs::metadata(dcx_win32::dcx_state_file("box_show"))
        .ok()
        .and_then(|m| m.modified().ok())
}

#[repr(C)]
struct BrowseInfoW {
    hwnd_owner: isize,
    pidl_root: *const core::ffi::c_void,
    display_name: *mut u16,
    title: *const u16,
    flags: u32,
    callback: *const core::ffi::c_void,
    l_param: isize,
    image: i32,
}

#[link(name = "shell32")]
extern "system" {
    fn SHBrowseForFolderW(info: *mut BrowseInfoW) -> *mut core::ffi::c_void;
    fn SHGetPathFromIDListW(pidl: *const core::ffi::c_void, path: *mut u16) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut core::ffi::c_void, coinit: u32) -> i32;
    fn CoTaskMemFree(p: *mut core::ffi::c_void);
}

/// Native folder picker for「移入文件夹 → 选择文件夹…」. Owned by the panel so
/// the topmost flyout cannot bury it.
pub fn pick_folder(owner: isize) -> Option<String> {
    const BIF_RETURNONLYFSDIRS: u32 = 0x0001;
    const BIF_EDITBOX: u32 = 0x0010;
    const BIF_NEWDIALOGSTYLE: u32 = 0x0040;
    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    unsafe {
        CoInitializeEx(core::ptr::null_mut(), COINIT_APARTMENTTHREADED);
    }
    let title = dcx_win32::wide("选择要移入的文件夹");
    let mut display = [0u16; 260];
    let mut info = BrowseInfoW {
        hwnd_owner: owner,
        pidl_root: core::ptr::null(),
        display_name: display.as_mut_ptr(),
        title: title.as_ptr(),
        flags: BIF_RETURNONLYFSDIRS | BIF_EDITBOX | BIF_NEWDIALOGSTYLE,
        callback: core::ptr::null(),
        l_param: 0,
        image: 0,
    };
    let pidl = unsafe { SHBrowseForFolderW(&mut info) };
    if pidl.is_null() {
        return None;
    }
    let mut buf = [0u16; 32_768];
    let ok = unsafe { SHGetPathFromIDListW(pidl, buf.as_mut_ptr()) };
    unsafe { CoTaskMemFree(pidl) };
    if ok == 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(0);
    let path = String::from_utf16_lossy(&buf[..end]);
    (!path.is_empty()).then_some(path)
}
