//! `dcx-snip.exe` — DCX 的「桌面快照」独立应用。
//!
//! ```text
//! dcx-host.exe   常驻：注册全局热键，按 apps.toml 启停，唯一设置中心
//!     │  Alt+J → launch_on_demand("snip") + touch snip_show
//!     ▼
//! dcx-snip.exe   本进程：Win32 覆盖层 + tiny-skia 自绘，不用 eframe/winit
//!     │  读 %LOCALAPPDATA%\dcx\snip.toml
//!     ▼
//! 虚拟屏幕位图 → 选区 → 标注 → 剪贴板 / PNG / JPEG
//! ```
//!
//! 刻意不链接 `eframe`：一个进程只能有一个 winit 事件循环，而覆盖层需要
//! 完全掌控自己的窗口过程（鼠标捕获、键盘焦点、逐帧合成）。这里用裸 Win32，
//! 与宿主的热键窗口是同一套写法。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod capture;
mod config;
mod edit_text;
mod ffi;
mod font_menu;
mod fonts;
mod icons;
mod layout;
mod longshot;
mod model;
mod output;
mod paint;
mod picker;
mod radius;
mod session;
mod stitch;
mod surface;
mod text;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Per-monitor-v2 DPI awareness must be set before any window exists. Without
/// it the virtual-screen metrics come back in virtualised units and every
/// captured coordinate is scaled behind our back.
fn enable_dpi_awareness() {
    const PER_MONITOR_AWARE_V2: isize = -4;
    unsafe {
        dcx_win32::SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2);
    }
}

/// Release builds have no console and the workspace profile is `panic = "abort"`,
/// so a file is the only trace a silent exit leaves behind.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        app::log(&format!("PANIC: {info}"));
    }));
}

fn main() {
    enable_dpi_awareness();
    install_panic_hook();
    let code = app::run();
    std::process::exit(code);
}
