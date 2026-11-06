//! Win32 helpers for the search window.
//!
//! GUI-subsystem builds have no console, so diagnostics go to
//! `%LOCALAPPDATA%\dcx\search_trace.log`. Window show/hide is done with
//! `SetWindowPos` rather than `ViewportCommand::Visible(false)` — eframe stops
//! calling `update` on a hidden viewport, which would kill the frame loop.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub const SHOW_MARKER: &str = "search_show";

static PROCESS_START: OnceLock<SystemTime> = OnceLock::new();

/// Call once in `main`, before any window exists. The show marker can be
/// touched by the host *before* this process finishes creating its window; a
/// comparison against this timestamp is how the app tells "start hidden" apart
/// from "the hotkey just launched me".
pub fn mark_process_start() {
    let _ = PROCESS_START.set(SystemTime::now());
}

pub fn process_started() -> SystemTime {
    *PROCESS_START.get_or_init(SystemTime::now)
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct MonitorInfo {
    cb_size: u32,
    rc_monitor: Rect,
    rc_work: Rect,
    flags: u32,
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

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FileTime {
    low: u32,
    high: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct SystemTimeW {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

const MONITOR_DEFAULTTONULL: u32 = 0;
const MONITOR_DEFAULTTONEAREST: u32 = 2;

#[link(name = "user32")]
extern "system" {
    fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: isize) -> i32;
    fn GetForegroundWindow() -> isize;
    fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
    fn GetDpiForWindow(hwnd: isize) -> u32;
    fn MonitorFromRect(rect: *const Rect, flags: u32) -> isize;
    fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
    fn SetWindowPos(
        hwnd: isize,
        after: isize,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
    fn GetCursorPos(out: *mut Point) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: isize,
        op: *const u16,
        file: *const u16,
        params: *const u16,
        dir: *const u16,
        show: i32,
    ) -> isize;
    fn SHBrowseForFolderW(info: *mut BrowseInfoW) -> *mut core::ffi::c_void;
    fn SHGetPathFromIDListW(pidl: *const core::ffi::c_void, path: *mut u16) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut core::ffi::c_void, coinit: u32) -> i32;
    fn CoTaskMemFree(p: *mut core::ffi::c_void);
}

#[link(name = "kernel32")]
extern "system" {
    fn FileTimeToLocalFileTime(ft: *const FileTime, out: *mut FileTime) -> i32;
    fn FileTimeToSystemTime(ft: *const FileTime, out: *mut SystemTimeW) -> i32;
}

/// `YYYY-MM-DD HH:MM` in local time. The table only needs minute precision;
/// avoiding chrono keeps the standalone app's dependency set small.
pub fn format_time(epoch_secs: u64) -> String {
    const EPOCH_DIFF: u64 = 11_644_473_600;
    let ticks = epoch_secs.saturating_add(EPOCH_DIFF).saturating_mul(10_000_000);
    let utc = FileTime {
        low: ticks as u32,
        high: (ticks >> 32) as u32,
    };
    let mut local = FileTime::default();
    if unsafe { FileTimeToLocalFileTime(&utc, &mut local) } == 0 {
        return String::new();
    }
    let mut st = SystemTimeW::default();
    if unsafe { FileTimeToSystemTime(&local, &mut st) } == 0 {
        return String::new();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        st.year, st.month, st.day, st.hour, st.minute
    )
}

const SW_RESTORE: i32 = 9;
const SW_SHOWNORMAL: i32 = 1;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_SHOWWINDOW: u32 = 0x0040;

pub fn local_dir() -> PathBuf {
    let mut p = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    p.push("dcx");
    let _ = std::fs::create_dir_all(&p);
    p
}

/// Append-only trace, truncated when it grows past 128 KiB.
pub fn trace(line: &str) {
    let path = local_dir().join("search_trace.log");
    if path.metadata().map(|m| m.len()).unwrap_or(0) >= 128 * 1024 {
        let _ = std::fs::write(&path, "");
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(f, "[dcx-search] {line}");
    }
}

/// egui points per physical pixel for a window. `GetDpiForSystem` is only
/// reliable after `SetProcessDpiAwarenessContext(-4)` ran in `main`; once the
/// window exists, its own DPI is what matters (a 100 % primary plus a 125 %
/// secondary must not share one scale).
pub fn scale_for(title: &str) -> f32 {
    let hwnd = find_window(title);
    let dpi = if hwnd != 0 {
        unsafe { GetDpiForWindow(hwnd) }
    } else {
        unsafe { dcx_win32::GetDpiForSystem() }
    };
    (dpi.max(96) as f32 / 96.0).max(1.0)
}

pub fn screen_size() -> (i32, i32) {
    unsafe {
        (
            dcx_win32::GetSystemMetrics(0),
            dcx_win32::GetSystemMetrics(1),
        )
    }
}

pub fn cursor_pos() -> (i32, i32) {
    let mut p = Point::default();
    if unsafe { GetCursorPos(&mut p) } != 0 {
        (p.x, p.y)
    } else {
        let (w, h) = screen_size();
        (w / 2, h / 2)
    }
}

/// Work area (taskbar excluded) of the monitor containing a point. Falls back
/// to the primary screen if the monitor query fails.
pub fn work_area_at(x: i32, y: i32) -> (i32, i32, i32, i32) {
    let probe = Rect {
        left: x,
        top: y,
        right: x + 1,
        bottom: y + 1,
    };
    let monitor = unsafe { MonitorFromRect(&probe, MONITOR_DEFAULTTONEAREST) };
    work_area_of(monitor)
}

fn work_area_of(monitor: isize) -> (i32, i32, i32, i32) {
    if monitor != 0 {
        let mut info = MonitorInfo {
            cb_size: std::mem::size_of::<MonitorInfo>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            let r = info.rc_work;
            return (r.left, r.top, r.right - r.left, r.bottom - r.top);
        }
    }
    let (w, h) = screen_size();
    (0, 0, w, h)
}

/// Does a rectangle intersect any monitor? Used to reject a remembered
/// position after the user unplugged the screen it lived on.
pub fn rect_on_any_monitor(x: i32, y: i32, w: i32, h: i32) -> bool {
    let rect = Rect {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    };
    unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONULL) != 0 }
}

pub fn window_rect(title: &str) -> Option<(i32, i32, i32, i32)> {
    let hwnd = find_window(title);
    if hwnd == 0 {
        return None;
    }
    let mut rect = Rect::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return None;
    }
    Some((
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
    ))
}

pub fn is_foreground(title: &str) -> bool {
    let hwnd = find_window(title);
    hwnd != 0 && unsafe { GetForegroundWindow() } == hwnd
}

/// Native folder picker for a custom search root.
pub fn pick_folder() -> Option<String> {
    const BIF_RETURNONLYFSDIRS: u32 = 0x0001;
    const BIF_EDITBOX: u32 = 0x0010;
    const BIF_NEWDIALOGSTYLE: u32 = 0x0040;
    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    unsafe {
        CoInitializeEx(core::ptr::null_mut(), COINIT_APARTMENTTHREADED);
    }
    let title = dcx_win32::wide("选择要加入检索范围的文件夹");
    let mut display = [0u16; 260];
    let mut info = BrowseInfoW {
        hwnd_owner: 0,
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

/// The winit window title is exactly `app::WIN_TITLE` (also what the host's
/// `AppEntry.window_title` and `FindWindowW` agree on).
pub fn find_window(title: &str) -> isize {
    let wide = dcx_win32::wide(title);
    unsafe { dcx_win32::FindWindowW(core::ptr::null(), wide.as_ptr()) }
}

/// Move the window to a physical-pixel rectangle and make it foreground.
pub fn show_at(title: &str, x: i32, y: i32, w: i32, h: i32) {
    let hwnd = find_window(title);
    if hwnd == 0 {
        return;
    }
    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
        SetWindowPos(hwnd, 0, x, y, w, h, SWP_NOZORDER | SWP_SHOWWINDOW);
        SetForegroundWindow(hwnd);
    }
}

/// Hidden state = move off-screen. Never `Visible(false)`.
pub fn hide_offscreen(title: &str) {
    let hwnd = find_window(title);
    if hwnd == 0 {
        return;
    }
    unsafe {
        SetWindowPos(
            hwnd,
            0,
            -32000,
            -32000,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Ask the window to close normally (winit ends the event loop, `main` joins
/// the handle and the process exits). Used for `residency = "release"`.
pub fn close_window(title: &str) {
    const WM_CLOSE: u32 = 0x0010;
    let hwnd = find_window(title);
    if hwnd != 0 {
        unsafe {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
    }
}

fn marker_path() -> PathBuf {
    local_dir().join(SHOW_MARKER)
}

/// Cheap change token for the "raise the window" signal. The host writes this
/// file on every hotkey press; the app polls it once per frame batch. File IPC
/// is the platform's existing convention (see `box_ui.txt` / `box_slot.txt`),
/// and it avoids subclassing winit's window procedure.
pub fn show_marker_stamp() -> u128 {
    std::fs::metadata(marker_path())
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

pub fn show_marker_time() -> Option<SystemTime> {
    std::fs::metadata(marker_path())
        .ok()
        .and_then(|m| m.modified().ok())
}

pub fn touch_show_marker() {
    let path = marker_path();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default();
    let _ = std::fs::write(&path, now);
}

/// `ShellExecuteW("open")` — the shell decides which app owns the extension.
pub fn open_file(path: &Path) {
    let file = dcx_win32::wide(&path.to_string_lossy());
    let op = dcx_win32::wide("open");
    unsafe {
        ShellExecuteW(
            0,
            op.as_ptr(),
            file.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Open the containing folder with the file selected.
pub fn reveal_file(path: &Path) {
    let exe = dcx_win32::wide("explorer.exe");
    let params = dcx_win32::wide(&format!("/select,\"{}\"", path.to_string_lossy()));
    unsafe {
        ShellExecuteW(
            0,
            core::ptr::null(),
            exe.as_ptr(),
            params.as_ptr(),
            core::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}
