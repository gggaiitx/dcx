//! Strip the 1 px frame Windows paints around shell popup windows.
//!
//! Start, the tray overflow and XAML flyouts are borderless, translucent
//! windows. Windows still decorates them with a frame — either `DWMWA_BORDER_COLOR`
//! on the window itself, or the frame glued to the XAML root by the hosting
//! process. The DWM attribute can be cleared from here even when the popup lives
//! in another process, so this sweeps every shell popup window on a timer.
//!
//! Both halves of the frame have to be addressed, because they come from two
//! different attributes:
//!   * `DWMWA_BORDER_COLOR` — a colour somebody assigned to the border;
//!   * `DWMWA_WINDOW_CORNER_PREFERENCE` — asking DWM for rounded corners on a
//!     window that owns no frame makes it synthesise one, border included. This
//!     is the one that produced a hollow outlined rectangle behind Start (the
//!     search flyout's 1000×938 island), and it outlives the process that set
//!     it: DWM keeps the attribute on the window until the window dies.
//!
//! Matching is deliberately broad: the popup class names differ between builds
//! (`Windows.UI.Core.CoreWindow`, `Xaml_WindowedPopupClass`, ...), so a window is
//! also accepted when its process is one of the XAML shell hosts. Explorer's own
//! folder windows are excluded by class, because their 1 px frame is the
//! expected Windows look.

use std::time::Duration;

/// Windows 11's XAML popup classes. Folder windows (`CabinetWClass`) and other
/// normal windows are deliberately absent: their frame is the expected look.
const POPUP_CLASSES: [&str; 6] = [
    // Start, search, quick settings, UWP-style XAML hosts.
    "Windows.UI.Core.CoreWindow",
    // Context menus, jump lists, date/volume flyouts.
    "Xaml_WindowedPopupClass",
    // Tray overflow flyout.
    "TopLevelWindowForOverflowXamlIsland",
    // Taskbar, primary and secondary.
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    // Flyout host used by some builds for the tray panels.
    "Windows.UI.Composition.DesktopWindowContentBridge",
];

/// Processes that only ever own shell popups, so every visible window they
/// create can be de-framed regardless of class name.
const SHELL_HOSTS: [&str; 3] = [
    "StartMenuExperienceHost.exe",
    "ShellExperienceHost.exe",
    "SearchHost.exe",
];

const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWA_BORDER_COLOR: u32 = 34;
const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
const DWMWCP_DONOTROUND: i32 = 1;

#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[link(name = "user32")]
extern "system" {
    fn EnumWindows(
        proc: Option<unsafe extern "system" fn(isize, isize) -> i32>,
        lparam: isize,
    ) -> i32;
    fn IsWindowVisible(hwnd: isize) -> i32;
    fn GetClassNameW(hwnd: isize, buf: *mut u16, len: i32) -> i32;
    fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
}

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
    fn QueryFullProcessImageNameW(process: isize, flags: u32, buf: *mut u16, size: *mut u32)
        -> i32;
    fn CloseHandle(handle: isize) -> i32;
}

#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(
        hwnd: isize,
        attr: u32,
        value: *const core::ffi::c_void,
        size: u32,
    ) -> i32;
    fn DwmGetWindowAttribute(
        hwnd: isize,
        attr: u32,
        value: *mut core::ffi::c_void,
        size: u32,
    ) -> i32;
}

/// File name (no directory) of the process that owns `pid`.
fn exe_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle == 0 {
            return None;
        }
        let mut buf = [0u16; 260];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..size as usize]);
        Some(
            path.rsplit(['\\', '/'])
                .next()
                .unwrap_or(path.as_str())
                .to_string(),
        )
    }
}

unsafe extern "system" fn enum_proc(hwnd: isize, lparam: isize) -> i32 {
    if IsWindowVisible(hwnd) == 0 {
        return 1;
    }

    let mut buf = [0u16; 128];
    let len = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if len <= 0 {
        return 1;
    }
    let class = String::from_utf16_lossy(&buf[..len as usize]);

    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    let by_process = exe_name(pid)
        .map(|name| SHELL_HOSTS.iter().any(|h| h.eq_ignore_ascii_case(&name)))
        .unwrap_or(false);

    if !by_process && !POPUP_CLASSES.contains(&class.as_str()) {
        return 1;
    }

    // `DesktopWindowXamlSource` helper windows are 1×1 and never visible as a
    // frame; skipping them keeps the sweep focused on real surfaces.
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if GetWindowRect(hwnd, &mut rect) == 0 {
        return 1;
    }
    if rect.right - rect.left <= 8 || rect.bottom - rect.top <= 8 {
        return 1;
    }

    let state = &mut *(lparam as *mut SweepState);
    let value = DWMWA_COLOR_NONE;
    let hr = DwmSetWindowAttribute(
        hwnd,
        DWMWA_BORDER_COLOR,
        &value as *const u32 as *const core::ffi::c_void,
        core::mem::size_of::<u32>() as u32,
    );
    if hr >= 0 {
        state.cleared += 1;
        if state.detail.len() < 6 {
            state.detail.push(format!(
                "{class} {}x{}",
                rect.right - rect.left,
                rect.bottom - rect.top
            ));
        }
    }

    // Corner preference is the other half of the same frame: DWM synthesises a
    // 1 px border for a window that asks for round corners but owns no frame.
    // Reset it here so a window poisoned by an older build is repaired without
    // waiting for its process to exit.
    let mut corner: i32 = DWMWA_COLOR_NONE as i32;
    if DwmGetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &mut corner as *mut i32 as *mut core::ffi::c_void,
        core::mem::size_of::<i32>() as u32,
    ) == 0
        && corner != DWMWCP_DONOTROUND
        && dwm_set_i32(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND)
    {
        state.unrounded += 1;
    }
    1
}

fn dwm_set_i32(hwnd: isize, attr: u32, value: i32) -> bool {
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attr,
            &value as *const i32 as *const core::ffi::c_void,
            core::mem::size_of::<i32>() as u32,
        ) >= 0
    }
}

struct SweepState {
    cleared: u32,
    unrounded: u32,
    detail: Vec<String>,
}

/// Clear the DWM border on every shell popup that currently exists.
fn sweep() -> SweepState {
    let mut state = SweepState {
        cleared: 0,
        unrounded: 0,
        detail: Vec::new(),
    };
    unsafe {
        EnumWindows(Some(enum_proc), &mut state as *mut SweepState as isize);
    }
    state
}

/// Keep popup frames cleared for the lifetime of the process.
pub fn spawn_border_cleaner() {
    std::thread::spawn(|| {
        // First pass right away: the tray already exists when the host starts.
        let first = sweep();
        if first.cleared > 0 || first.unrounded > 0 {
            println!(
                "  chrome  : cleared DWM border on {} window(s), reset corners on {}: {}",
                first.cleared,
                first.unrounded,
                first.detail.join(", ")
            );
        }
        loop {
            std::thread::sleep(Duration::from_millis(1500));
            let _ = sweep();
        }
    });
}
