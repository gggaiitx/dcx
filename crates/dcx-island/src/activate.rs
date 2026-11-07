//! Bring the app behind an inbox row back to the foreground.
//!
//! The inbox only ever knows **which app** raised a signal — no chat name, no
//! sender, no body (see `notify::shell_hook`). Opening the exact conversation
//! would require reading message metadata, which this app refuses to do, so
//! "focus that app's main window" is the honest ceiling and the only thing the
//! click promises.
//!
//! Matching is by process image name, walked with `EnumWindows`:
//!
//! * WeChat 3 `WeChat.exe`, WeChat 4 `Weixin.exe`
//! * WeCom `WXWork.exe`
//! * QQ / QQ NT / TIM `QQ.exe`, `QQNT.exe`, `TIM.exe`
//!
//! Mail has no app of its own here (the channel speaks IMAP directly), so a
//! mail row never activates anything and the caller shows a toast instead.

use crate::inbox::Source;

const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const SW_RESTORE: i32 = 9;
const GW_OWNER: u32 = 4;

#[link(name = "user32")]
extern "system" {
    fn GetWindowTextW(hwnd: isize, buf: *mut u16, max: i32) -> i32;
    fn GetWindow(hwnd: isize, cmd: u32) -> isize;
    fn BringWindowToTop(hwnd: isize) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
    fn CloseHandle(handle: isize) -> i32;
    fn QueryFullProcessImageNameW(process: isize, flags: u32, name: *mut u16, size: *mut u32) -> i32;
}

/// File name (lower case) → does it belong to `source`?
fn belongs(exe: &str, source: Source) -> bool {
    let name = exe
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(exe)
        .to_ascii_lowercase();
    match source {
        Source::WeChat => matches!(name.as_str(), "weixin.exe" | "wechat.exe"),
        Source::WeCom => name == "wxwork.exe",
        Source::Qq => matches!(name.as_str(), "qq.exe" | "qqnt.exe" | "tim.exe"),
        // No local mail client is owned by this channel.
        Source::Mail => false,
    }
}

fn exe_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle == 0 {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..size as usize]))
    }
}

struct Search {
    source: Source,
    best: isize,
    area: i64,
}

unsafe extern "system" fn enum_proc(hwnd: isize, lparam: isize) -> i32 {
    let search = unsafe { &mut *(lparam as *mut Search) };
    // Only a visible, top-level, titled window is "the app": tray helpers and
    // owned popups (WeChat's mini-program host, QQ's chat windows) would
    // otherwise win the area comparison on a bad day.
    if unsafe { dcx_win32::IsWindowVisible(hwnd) } == 0
        || unsafe { GetWindow(hwnd, GW_OWNER) } != 0
    {
        return 1;
    }
    let mut title = [0u16; 512];
    if unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) } <= 0 {
        return 1;
    }
    let mut pid = 0u32;
    unsafe { dcx_win32::GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 {
        return 1;
    }
    let Some(exe) = exe_name(pid) else { return 1 };
    if !belongs(&exe, search.source) {
        return 1;
    }
    let mut rc = dcx_win32::RECT::default();
    if unsafe { dcx_win32::GetWindowRect(hwnd, &mut rc) } == 0 {
        return 1;
    }
    let area = (rc.right - rc.left) as i64 * (rc.bottom - rc.top) as i64;
    if area > search.area {
        search.area = area;
        search.best = hwnd;
    }
    1
}

/// Bring `source`'s main window to the front. Returns false when the app is not
/// running (or, for mail, has no window at all).
pub fn activate(source: Source) -> bool {
    let mut search = Search {
        source,
        best: 0,
        area: -1,
    };
    unsafe {
        dcx_win32::EnumWindows(Some(enum_proc), &mut search as *mut Search as isize);
    }
    if search.best == 0 {
        return false;
    }
    let hwnd = search.best;
    unsafe {
        if dcx_win32::IsIconic(hwnd) != 0 {
            dcx_win32::ShowWindowAsync(hwnd, SW_RESTORE);
        }
        // The island received the click, so this process holds the foreground
        // right that `SetForegroundWindow` requires.
        BringWindowToTop(hwnd);
        dcx_win32::SetForegroundWindow(hwnd);
    }
    crate::win::trace(&format!(
        "activate {} → hwnd={hwnd} class={}",
        source.key(),
        crate::win::class_name(hwnd)
    ));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_names_map_to_the_right_source() {
        assert!(belongs(r"C:\Program Files\Tencent\WeChat\WeChat.exe", Source::WeChat));
        assert!(belongs(r"D:\Apps\Weixin\Weixin.exe", Source::WeChat));
        assert!(belongs(r"C:\Program Files\WXWork\WXWork.exe", Source::WeCom));
        assert!(belongs(r"D:\Tencent\QQNT\QQ.exe", Source::Qq));
        assert!(!belongs(r"C:\Windows\explorer.exe", Source::WeChat));
        assert!(!belongs("", Source::Qq));
        // Mail never activates a window.
        assert!(!belongs(r"C:\Program Files\Outlook\OUTLOOK.EXE", Source::Mail));
    }
}
