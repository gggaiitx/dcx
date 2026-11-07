//! WeChat / WeCom "there is something new" signal via the shell hook.
//!
//! # What this can and cannot see
//!
//! `RegisterShellHookWindow` + `HSHELL_FLASH` reports that a window asked the
//! taskbar to flash its button. That event carries a window handle and nothing
//! else: there is no sender, no message text and no unread count in it. The
//! module therefore reports *"微信：有新消息"* and deliberately never calls
//! `GetWindowTextW` — a flashed chat window's title is the contact's name, and
//! reading it would turn a taskbar signal into chat metadata collection.
//!
//! Personal WeChat and WeCom have no official outbound API either; this file is
//! the whole of the inbound story for them.
//!
//! # Known limits
//!
//! * The flag only fires while the app is set to show its taskbar button and
//!   the user has not disabled flashing. If the app is minimised to the tray,
//!   there is no event at all.
//! * Some WeCom builds raise their own toast instead of flashing; the settings
//!   card reports "未收到信号" rather than pretending the channel works.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::inbox::Source;

const HSHELL_WINDOWCREATED: usize = 1;
const HSHELL_REDRAW: usize = 2;
const HSHELL_FLASH: usize = 0x8006;
const HSHELL_RUDEAPPACTIVATED: usize = 0x8004;

const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

#[link(name = "user32")]
extern "system" {
    fn RegisterShellHookWindow(hwnd: isize) -> i32;
    fn DeregisterShellHookWindow(hwnd: isize) -> i32;
    fn RegisterWindowMessageW(name: *const u16) -> u32;
}

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
    fn CloseHandle(handle: isize) -> i32;
    fn QueryFullProcessImageNameW(
        process: isize,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> i32;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Flash {
    pub source: Source,
    pub exe: String,
}

pub struct ShellHook {
    pub rx: Receiver<Flash>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    hwnd: Arc<Mutex<isize>>,
}

impl Drop for ShellHook {
    fn drop(&mut self) {
        self.stop
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Ok(h) = self.hwnd.lock() {
            if *h != 0 {
                unsafe { dcx_win32::PostMessageW(*h, 0x0010, 0, 0) };
            }
        }
    }
}

static SINK: Mutex<Option<Sender<Flash>>> = Mutex::new(None);
static SHELLHOOK_MSG: Mutex<u32> = Mutex::new(0);
/// Per-source debounce: a single unread message can flash the button several
/// times, and WeChat re-flashes on every new message batch.
static LAST: Mutex<Option<(Source, Instant)>> = Mutex::new(None);

const DEBOUNCE: Duration = Duration::from_secs(6);

pub fn start() -> ShellHook {
    let (tx, rx) = channel();
    if let Ok(mut slot) = SINK.lock() {
        *slot = Some(tx);
    }
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hwnd = Arc::new(Mutex::new(0isize));
    let hwnd2 = Arc::clone(&hwnd);
    let stop2 = Arc::clone(&stop);

    std::thread::Builder::new()
        .name("dcx-island-shellhook".to_string())
        .spawn(move || unsafe {
            let class = dcx_win32::wide("DCX.Island.ShellHook");
            let cls = dcx_win32::WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(hook_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: dcx_win32::GetModuleHandleW(core::ptr::null()),
                hIcon: 0,
                hCursor: 0,
                hbrBackground: 0,
                lpszMenuName: core::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            dcx_win32::RegisterClassW(&cls);
            let title = dcx_win32::wide("DCX shell hook");
            let h = dcx_win32::CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                -3, // HWND_MESSAGE
                0,
                0,
                core::ptr::null_mut(),
            );
            if h == 0 {
                crate::win::trace("shell hook window could not be created");
                return;
            }
            if let Ok(mut slot) = hwnd2.lock() {
                *slot = h;
            }
            let msg_name = dcx_win32::wide("SHELLHOOK");
            let msg = RegisterWindowMessageW(msg_name.as_ptr());
            if let Ok(mut slot) = SHELLHOOK_MSG.lock() {
                *slot = msg;
            }
            if RegisterShellHookWindow(h) == 0 {
                crate::win::trace("RegisterShellHookWindow failed");
            }

            let mut msg_buf = dcx_win32::MSG::default();
            while dcx_win32::GetMessageW(&mut msg_buf, 0, 0, 0) > 0 {
                dcx_win32::TranslateMessage(&msg_buf);
                dcx_win32::DispatchMessageW(&msg_buf);
                if stop2.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
            }
            DeregisterShellHookWindow(h);
            if let Ok(mut slot) = hwnd2.lock() {
                *slot = 0;
            }
        })
        .ok();

    ShellHook { rx, stop, hwnd }
}

/// Process image name → inbox source. Matching is on the file name only: the
/// install path differs between WeChat 3 (`WeChat.exe`), WeChat 4
/// (`Weixin.exe`) and WeCom (`WXWork.exe`).
pub fn classify(exe: &str) -> Option<Source> {
    let name = exe
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(exe)
        .to_ascii_lowercase();
    match name.as_str() {
        "weixin.exe" | "wechat.exe" => Some(Source::WeChat),
        "wxwork.exe" => Some(Source::WeCom),
        _ => None,
    }
}

unsafe extern "system" fn hook_proc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let shell_msg = *SHELLHOOK_MSG.lock().unwrap_or_else(|e| e.into_inner());
    if msg != 0 && msg == shell_msg {
        // `HSHELL_FLASH` is the documented "attention requested" code. `REDRAW`
        // fires for ordinary repaints and would produce constant false alarms.
        if wparam == HSHELL_FLASH || wparam == HSHELL_RUDEAPPACTIVATED {
            let target = lparam;
            std::thread::spawn(move || note_flash(target));
        }
        let _ = (HSHELL_WINDOWCREATED, HSHELL_REDRAW);
        return 1;
    }
    unsafe { dcx_win32::DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn note_flash(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let mut pid = 0u32;
    unsafe { dcx_win32::GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 {
        return;
    }
    let Some(exe) = process_image(pid) else {
        return;
    };
    let Some(source) = classify(&exe) else {
        return;
    };
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((prev_source, at)) = last.as_ref() {
            if *prev_source == source && at.elapsed() < DEBOUNCE {
                return;
            }
        }
        *last = Some((source, Instant::now()));
    }
    if let Ok(slot) = SINK.lock() {
        if let Some(tx) = slot.as_ref() {
            let _ = tx.send(Flash { source, exe });
        }
    }
}

fn process_image(pid: u32) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_names_map_to_the_right_channel() {
        assert_eq!(
            classify(r"C:\Program Files\Tencent\WeChat\WeChat.exe"),
            Some(Source::WeChat)
        );
        assert_eq!(
            classify(r"D:\Apps\Weixin\Weixin.exe"),
            Some(Source::WeChat)
        );
        assert_eq!(
            classify(r"C:\Program Files\WXWork\WXWork.exe"),
            Some(Source::WeCom)
        );
        assert_eq!(classify(r"C:\Windows\explorer.exe"), None);
        assert_eq!(classify(""), None);
    }
}
