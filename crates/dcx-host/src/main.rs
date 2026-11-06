//! DCX host — the **platform**, not an application.
//!
//! Owns exactly three things:
//!   1. taskbar injection (the `dcx_engine` DLL into Explorer, with a watchdog);
//!   2. the ONE central settings window, which edits the platform and every
//!      registered standalone application (see `apps.rs`);
//!   3. the system tray plus the platform's own effects (window corners,
//!      border cleaning, auto-hide, autostart).
//!
//! Standalone applications (currently `dcx-box.exe` — the desktop file box)
//! are separate processes with their own icon and UI; this host only launches
//! and stops them. No application window, event loop or UI code lives here.
//!
//! Process model:
//!   * main thread  : builds the tray icon and runs the corner-effect WinEvent
//!                    loop.
//!   * watchdog     : keeps Explorer injected with `dcx_engine` (re-injects on
//!                    Explorer restart), and keeps enabled apps running.
//!   * settings UI  : `dcx-host.exe --settings` — a separate process, because
//!                    winit allows one event loop per process.
//!
//! Host / engine / apps talk through files under `%LOCALAPPDATA%\dcx`:
//! `settings.toml`, `organizer.toml`, `apps.toml`, `box_slot.txt`.

// GUI subsystem so double-clicking the exe gives a tray app instead of a
// console window that stays open for the session. Debug builds keep the
// console: `cargo run` output is worth more than tidiness while iterating.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod apps;
mod auto_hide;
mod autostart;
mod boot;
mod chrome;
mod hotkey;
mod injector;
mod settings_app;
mod shutdown;
mod tray;

extern "system" {
    fn SetProcessDpiAwarenessContext(value: isize) -> i32;
}

/// Per-monitor-v2 DPI awareness must be on before the settings window measures
/// the screen: `GetDpiForSystem` reports 96 while the process is DPI-unaware,
/// which makes the window believe a 1080p display is 1080 logical pixels tall
/// and place itself partly below the screen. eframe/winit would set this later
/// anyway; doing it first only makes the measurement correct.
fn enable_dpi_awareness() {
    const PER_MONITOR_AWARE_V2: isize = -4;
    unsafe {
        SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2);
    }
}

/// Set by the tray "Quit" handler so any loop can observe shutdown intent.
pub(crate) static QUIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A panic is fatal here — the release profile is `panic = "abort"` — and the
/// process may be running without any console attached (started by the logon
/// Run key), in which case the default hook's message goes nowhere. Mirror it
/// into `%LOCALAPPDATA%\dcx\panic.log` so a silent exit is still explainable.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let text = format!(
            "{} [dcx-host] PANIC: {}",
            boot::stamp_now(),
            info
        );
        eprintln!("{text}");
        let mut path = boot::log_path();
        path.set_file_name("panic.log");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
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
    // Console routing before the first print: double-clicked builds have no
    // console, and their output belongs in `%LOCALAPPDATA%\dcx\host.log`.
    boot::init_output();
    install_panic_hook();

    let args: Vec<String> = std::env::args().skip(1).collect();
    // No argument is the normal entry point: tray + injection + settings
    // window. A double-click should show the window, so that is the default and
    // `--no-settings` opts out. `--with-settings` stays accepted as a no-op
    // alias so the packaged scripts keep working.
    let no_settings = args.iter().any(|a| a == "--no-settings");
    let selftest_ms: Option<u64> = match args.first().map(String::as_str) {
        Some("--selftest") => Some(1500),
        Some("--settings") => {
            // Settings window only, on the main thread: no tray, no corner
            // effect, no injection. The host spawns this as its own process so
            // the window gets a normal top-level input path without competing
            // with any application's event loop (winit: one loop per process).
            if let Ok(page) = std::env::var("DCX_SETTINGS_PAGE") {
                if let Ok(n) = page.parse::<u8>() {
                    settings_app::set_start_page(n);
                }
            }
            // Second launch = raise the window that is already there.
            if boot::focus_settings_window() {
                return;
            }
            crate::injector::spawn_status_monitor();
            if let Err(e) = settings_app::run_standalone() {
                eprintln!("[dcx-host] settings window failed: {e}");
                std::process::exit(1);
            }
            return;
        }
        Some("--help") | Some("-h") => {
            println!("dcx-host [--no-settings | --selftest | --settings]");
            println!("  (no option)     : tray + injection + app launch + settings — normal entry");
            println!("  --no-settings   : tray + injection + app launch, no window (logon)");
            println!("  --selftest      : run effects for 1.5s then exit (no tray / no injection)");
            println!("  --settings      : central settings window only (also used at runtime)");
            println!("  standalone apps : dcx-box.exe (desktop file box)");
            println!("                    dcx-search.exe (file search, default off, Alt+s)");
            return;
        }
        _ => None,
    };

    println!("DCX desktop enhancement host");
    println!("  corners  : uniform window corners");
    println!("  settings : taskbar shape / icon size / spacing (tray UI)");
    println!("  engine   : auto-injected into Explorer.exe");

    match selftest_ms {
        Some(ms) => {
            println!("  selftest: running {ms} ms then exiting\n");
            platform_smoke(ms);
            println!("selftest finished cleanly");
        }
        None => {
            // One host per session: a second copy would add a second tray icon
            // and a second writer for the same settings.toml.
            if boot::already_running() {
                println!(
                    "[dcx-host] already running in this session — exiting (log: {})",
                    boot::log_path().display()
                );
                return;
            }

            // 便携目录被搬动后，修掉启动项里的旧路径，别让"已开启"变成空转。
            autostart::refresh_path();

            let mut platform = dcx_core::Platform::new(8);
            platform.with_high_res_timer(true).register(Box::new(
                dcx_effect_corners::CornerEffect::new(dcx_effect_corners::CornerConfig::default()),
            ));
            // Keep Explorer styled even across shell restarts.
            injector::spawn_watchdog();
            // Sample whether Explorer actually hosts the engine, for the
            // "is this live?" indicator in the settings window.
            injector::spawn_status_monitor();
            // Popups (Start, tray overflow, context menus) are borderless
            // windows that Windows still frames with a 1 px DWM line; clear it
            // wherever one appears.
            chrome::spawn_border_cleaner();
            // Tray + menu (callbacks fire on tray's own thread). The returned
            // icon is kept alive for the process lifetime.
            let _tray = tray::build_tray();
            // Registered standalone applications are separate executables. The
            // host only starts them and watches that they stay up; their windows,
            // icons and event loops belong to them.
            apps::spawn_enabled();
            // The search app is default-off, so the global hotkey lives here
            // in the resident platform rather than in the app process.
            hotkey::start();
            println!("  tray ready — right-click the icon for settings / quit\n");
            if !no_settings {
                // The settings window lives in its own process: winit allows one
                // event loop per process and the panel owns this one.
                settings_app::open_settings();
                println!("  settings window opened\n");
            }
            // Corner-effect loop owns the main thread until the process exits.
            platform.run();
            apps::stop_all();
            println!("DCX stopped");
        }
    }
}

/// Self-test body: build the effect list, run it for `ms`, then return.
fn platform_smoke(ms: u64) {
    let mut platform = dcx_core::Platform::new(8);
    platform
        .with_high_res_timer(true)
        .register(Box::new(dcx_effect_corners::CornerEffect::new(
            dcx_effect_corners::CornerConfig::default(),
        )));
    platform.run_for(ms);
}
