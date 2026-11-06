//! `dcx-search.exe` — DCX 的「文件检索」独立应用。
//!
//! 进程模型（与收纳盒一致：一个进程、一个 winit 事件循环、一个崩溃域）：
//!
//! ```text
//! dcx-host.exe   常驻平台：注册全局热键，按 apps.toml 启停应用
//!     │  热键 / apps.toml
//!     ▼
//! dcx-search.exe 本进程：egui 窗口 + 后台索引线程 + 查询线程
//!     │  读 %LOCALAPPDATA%\dcx\search.toml
//!     │  轮询 search_show 标记文件（host 写）实现「再次按下前置窗口」
//!     ▼
//! 文件系统（只读）：ignore 遍历 / grep-searcher 内容检索
//! ```
//!
//! 默认关闭：host 不会随平台拉起本进程；全局热键由 host 常驻监听，
//! 按下才 `launch_on_demand("search")`。设置集中在 host 唯一设置窗口，
//! 本进程不带设置 UI。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod file_icon;
mod index;
mod model;
mod query;
mod ui;
mod view;
mod win;

#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(attrs: *mut core::ffi::c_void, initial_owner: i32, name: *const u16) -> isize;
    fn GetLastError() -> u32;
}

/// Per-monitor-v2 DPI awareness must be set before any window exists; a
/// DPI-unaware process reports 96 forever and the 125% desktop would place
/// the window in one coordinate space while egui paints in another.
fn enable_dpi_awareness() {
    const PER_MONITOR_AWARE_V2: isize = -4;
    unsafe {
        dcx_win32::SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2);
    }
}

/// A named mutex, not `FindWindowW`: the winit window appears a few frames
/// after process start, so a window lookup races with a second launch.
fn already_running() -> bool {
    const ERROR_ALREADY_EXISTS: u32 = 183;
    let name: Vec<u16> = "DCX_SEARCH_SINGLETON\0".encode_utf16().collect();
    let handle = unsafe { CreateMutexW(core::ptr::null_mut(), 0, name.as_ptr()) };
    handle != 0 && unsafe { GetLastError() } == ERROR_ALREADY_EXISTS
}

/// Release builds have no console and the workspace profile is
/// `panic = "abort"`; a file is the only trace a silent exit leaves behind.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let text = format!("[dcx-search] PANIC: {info}");
        let mut path = win::local_dir();
        path.push("search_panic.log");
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
    win::mark_process_start();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("--help") | Some("-h")) {
        println!("dcx-search [--help]");
        println!("  (no option) : search window (normally started by dcx-host)");
        println!("  settings are edited in dcx-host.exe --settings (桌面整理)");
        return;
    }

    if already_running() {
        // A second launch still means "the user wants the window": touch the
        // show marker so the running instance raises itself.
        win::touch_show_marker();
        return;
    }

    match app::spawn() {
        Some(handle) => {
            let _ = handle.join();
        }
        None => return,
    }
}
