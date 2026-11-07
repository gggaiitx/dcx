//! `dcx-island.exe` — DCX 的第三个独立应用：桌面信息岛。
//!
//! 进程模型与其他应用一致：**一个进程、一个 winit 事件循环、一个崩溃域**。
//!
//! ```text
//! dcx-host.exe    常驻平台：托盘、注入看门狗、统一设置中心、全局热键
//!     │  热键 / apps.toml / island_show
//!     ▼
//! dcx-island.exe  本进程：胶囊 + 面板（同一窗口）+ 后台采样/监听线程
//!     │  %LOCALAPPDATA%\dcx\island.toml
//!     │  island_tasks.json / island_clipboard.json / island_clipboard\*.png
//!     ▼
//! Win32：GetSystemTimes / GetIfTable2 / clipboard / shell hook / WinRT / IMAP
//! ```
//!
//! 设置全部在 host 的统一设置窗口里编辑，本进程不带设置 UI。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod clipboard;
mod clock;
mod dispatch;
mod inbox;
mod metrics;
mod notify;
mod sys;
mod tasks;
mod ui;
mod win;

#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(attrs: *mut core::ffi::c_void, initial_owner: i32, name: *const u16) -> isize;
    fn GetLastError() -> u32;
}

/// Per-monitor-v2 DPI awareness before any window exists: otherwise Windows
/// reports 96 DPI forever and the capsule is placed in one coordinate space
/// while egui paints in another (the box had exactly this bug at 125 %).
fn enable_dpi_awareness() {
    const PER_MONITOR_AWARE_V2: isize = -4;
    unsafe {
        dcx_win32::SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2);
    }
}

/// A named mutex, not `FindWindowW`: the winit window appears a few frames
/// after start, so a window lookup races with a second launch.
fn already_running() -> bool {
    const ERROR_ALREADY_EXISTS: u32 = 183;
    let name: Vec<u16> = "DCX_ISLAND_SINGLETON\0".encode_utf16().collect();
    let handle = unsafe { CreateMutexW(core::ptr::null_mut(), 0, name.as_ptr()) };
    handle != 0 && unsafe { GetLastError() } == ERROR_ALREADY_EXISTS
}

/// Release builds have no console and the workspace profile is `panic = "abort"`,
/// so a file is the only trace a silent exit leaves behind.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let text = format!("[dcx-island] PANIC: {info}");
        let mut path = win::local_dir();
        path.push("island_panic.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let _ = writeln!(f, "{text}");
        }
    }));
}

fn main() {
    enable_dpi_awareness();
    install_panic_hook();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("--help") | Some("-h")) {
        println!("dcx-island [--help]");
        println!("  (no option) : the island (normally started by dcx-host)");
        println!("  settings     : edit in dcx-host.exe --settings → 桌面信息岛");
        return;
    }

    if already_running() {
        // A second launch means "the user wants to see it": touch the marker so
        // the running instance expands instead of silently doing nothing.
        win::touch_show_marker();
        return;
    }

    for problem in sys::sizes_are_sane() {
        win::trace(&format!("ABI check failed: {problem}"));
    }

    if let Some(handle) = app::spawn() {
        let _ = handle.join();
    }
}
