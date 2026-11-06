//! The shell's own "automatically hide the taskbar" switch.
//!
//! Read and written through the AppBar messages the shell uses for itself, not
//! through `settings.toml`: Windows persists this state on its own, so a mirror
//! in our file would start drifting the moment the user flips the toggle in the
//! Windows Settings app.
//!
//! This is the one piece of taskbar *behaviour* an outside process may change.
//! `SetWindowPos` on `Shell_TrayWnd` is vetoed inside Explorer (see
//! `dcx_settings::MAX_TASKBAR_HEIGHT`), but `ABM_SETSTATE` moves the window and
//! recomputes the work area in the same call — verified on 10.0.26200, where the
//! tray rect goes from `(0,1020)-(1920,1080)` to `(0,1078)-(1920,1138)` and back
//! with each write.

use dcx_win32::{
    wide, FindWindowW, SHAppBarMessage, ABM_GETSTATE, ABM_SETSTATE, ABS_AUTOHIDE, APPBARDATA, HWND,
    RECT,
};

/// Whether the taskbar is currently hidden until the pointer reaches the edge.
/// `false` when the tray window cannot be found (Explorer restarting).
pub fn is_enabled() -> bool {
    let tray = tray_window();
    tray != 0 && (read_state(tray) & ABS_AUTOHIDE as isize) != 0
}

/// Flip the auto-hide bit, leaving the rest of the mask — notably
/// `ABS_ALWAYSONTOP` — exactly as the shell reported it.
pub fn set_enabled(on: bool) -> Result<(), String> {
    let tray = tray_window();
    if tray == 0 {
        return Err("找不到任务栏窗口 Shell_TrayWnd".to_string());
    }
    let current = read_state(tray);
    let wanted = if on {
        current | ABS_AUTOHIDE as isize
    } else {
        current & !(ABS_AUTOHIDE as isize)
    };
    let mut data = appbar(ABM_SETSTATE, tray, wanted);
    if unsafe { SHAppBarMessage(ABM_SETSTATE, &mut data) } == 0 {
        return Err(format!(
            "任务栏自动隐藏未能写入：{}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn tray_window() -> HWND {
    let class = wide("Shell_TrayWnd");
    unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) }
}

/// `ABM_GETSTATE` answers through the return value; `lParam` is an input only.
fn read_state(tray: HWND) -> isize {
    let mut data = appbar(ABM_GETSTATE, tray, 0);
    unsafe { SHAppBarMessage(ABM_GETSTATE, &mut data) }
}

fn appbar(msg: u32, tray: HWND, param: isize) -> APPBARDATA {
    APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        hWnd: tray,
        uMsg: msg,
        rc: RECT::default(),
        lParam: param,
    }
}
