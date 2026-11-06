//! `dcx-box.exe` — the standalone desktop file box application.
//!
//! Process model (after the split):
//!
//! ```text
//! dcx-host.exe   platform: tray + Explorer injection + settings centre
//!      │  starts / stops this executable (apps.toml)
//!      ▼
//! dcx-box.exe    this process: taskbar tile + panel viewports + own icon
//!      │  reads organizer.toml, writes box_host_alive
//!      ▼
//! dcx_engine.dll measures the taskbar slot, writes box_slot.txt
//! ```
//!
//! The box has **no settings UI**; all application settings are edited in the
//! one central settings window owned by `dcx-host.exe`. Configuration reaches
//! this process through `%LOCALAPPDATA%\dcx\organizer.toml`, exactly as before.

// GUI subsystem so double-clicking the exe gives a taskbar tile instead of a
// console window that stays open for the session. Debug builds keep the console.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod desk_place;
mod file_icon;
mod model;
mod shell_io;
mod tile;
mod ui;
mod view;
mod win;

#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(attrs: *mut core::ffi::c_void, initial_owner: i32, name: *const u16) -> isize;
    fn GetLastError() -> u32;
}

/// Per-monitor-v2 DPI awareness must be on before any window exists: a
/// DPI-unaware process makes `GetDpiForSystem` report 96 forever, so every
/// rectangle is measured at 1.0 while the desktop runs at 125 % — the window
/// ends up placed in one place and painted in another.
fn enable_dpi_awareness() {
    const PER_MONITOR_AWARE_V2: isize = -4;
    unsafe {
        dcx_win32::SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2);
    }
}

/// One box per session. A named mutex (held for the process lifetime) is used
/// instead of `FindWindowW(title)`: the winit window appears a few frames after
/// startup, so a window lookup races with a second launch and lets two tiles
/// through. The host normally starts us once, but a manual double-click must
/// not add a second tile either.
fn already_running() -> bool {
    const ERROR_ALREADY_EXISTS: u32 = 183;
    let name: Vec<u16> = "DCX_BOX_SINGLETON\0".encode_utf16().collect();
    let handle = unsafe { CreateMutexW(core::ptr::null_mut(), 0, name.as_ptr()) };
    handle != 0 && unsafe { GetLastError() } == ERROR_ALREADY_EXISTS
}

/// Release builds have no console; mirror a panic into a file so a silent exit
/// is still explainable. The release profile is `panic = "abort"`, so this is
/// the only trace left behind.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let text = format!("[dcx-box] PANIC: {info}");
        eprintln!("{text}");
        let mut path = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        path.push("dcx");
        let _ = std::fs::create_dir_all(&path);
        path.push("box_panic.log");
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
    if matches!(
        args.first().map(String::as_str),
        Some("--help") | Some("-h")
    ) {
        println!("dcx-box [--help]");
        println!("  (no option) : taskbar tile + file box panel");
        println!("  settings are edited in dcx-host.exe --settings (application page)");
        return;
    }

    if already_running() {
        println!("[dcx-box] already running in this session — exiting");
        return;
    }

    // `spawn` starts the eframe event loop on its own thread (it needs
    // `with_any_thread`, exactly as when it lived inside the host). Blocking on
    // the handle means WM_CLOSE from the host's app manager ends the loop and
    // then ends this process cleanly, instead of leaving a sleeping main behind.
    match app::spawn() {
        Some(handle) => {
            let _ = handle.join();
        }
        None => return,
    }
}
