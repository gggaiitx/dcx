//! Process shell: the hidden control window, its timer, and the "file is IPC"
//! handshake with `dcx-host.exe`.
//!
//! The control window is **hidden but not message-only**. `apps.rs` in the host
//! finds a running instance with `FindWindowW(title)` and stops it with
//! `WM_CLOSE`; message-only (`HWND_MESSAGE`) windows are explicitly excluded
//! from window enumeration, so the host would believe the app is dead and spawn
//! a second copy on every hotkey press.

use std::time::Duration;

use crate::config;
use crate::ffi::*;
use dcx_win32::HWND;

const TIMER_TICK: usize = 1;
/// Hotkey presses arrive as a file marker. 150 ms of polling added a visible
/// half-frame delay to every repeat shot; 60 ms keeps the wake-up cost trivial
/// and makes the overlay feel immediate.
const TICK_MS: u32 = 60;
const WM_APP_TICK: u32 = 0x8000 + 20;

pub const WIN_TITLE: &str = "桌面快照";

pub struct App {
    last_marker: u64,
    last_reshoot: u64,
    /// A manual second launch (double-click) asks for a fresh frame; the old
    /// overlay is cancelled first and the shot is taken once its window is
    /// really gone.
    pending_reshoot: bool,
    apps_on: bool,
    /// `apps.toml` is a 2 s cache: the old code parsed it 16 times a second for
    /// the whole resident lifetime.
    apps_checked: u64,
    idle_since: u64,
    idle_exit_secs: u64,
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn run() -> i32 {
    let started_ms = epoch_ms();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        log("dcx-snip: --probe | --bench [frames] | --demo-synth | --no-shot | --shot (default)");
        return 0;
    }
    if args.iter().any(|a| a == "--probe") {
        return probe();
    }
    if let Some(i) = args.iter().position(|a| a == "--demo-synth") {
        let out = args
            .get(i + 1)
            .cloned()
            .unwrap_or_else(|| "out/snip-synth.png".to_string());
        return match crate::session::demo_synth(std::path::Path::new(&out)) {
            Ok(()) => 0,
            Err(e) => {
                log(&format!("demo-synth failed: {e}"));
                1
            }
        };
    }
    if let Some(i) = args.iter().position(|a| a == "--bench") {
        let frames = args
            .get(i + 1)
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(60);
        return crate::session::bench(frames);
    }
    if let Some(i) = args.iter().position(|a| a == "--demo") {
        let out = args
            .get(i + 1)
            .cloned()
            .unwrap_or_else(|| "out/snip-demo.png".to_string());
        return match crate::capture::grab() {
            Ok(shot) => match crate::session::demo(shot, config::load(), std::path::Path::new(&out)) {
                Ok(()) => 0,
                Err(e) => {
                    log(&format!("demo failed: {e}"));
                    1
                }
            },
            Err(e) => {
                log(&format!("demo capture failed: {e}"));
                1
            }
        };
    }
    let no_shot = args.iter().any(|a| a == "--no-shot");

    // A second launch means "take another shot" **only** when a human started
    // it. A host-initiated start (`--no-shot`) that raced with the watchdog
    // must not touch any marker: that would cancel the live overlay it was
    // supposed to leave alone.
    if already_running() {
        if !no_shot {
            touch_marker("snip_reshoot");
        }
        return 0;
    }

    unsafe {
        let class = crate::wide("DCX.Snip.Control");
        let title = crate::wide(WIN_TITLE);
        let cls = dcx_win32::WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(core::ptr::null()),
            hIcon: 0,
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: core::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        if dcx_win32::RegisterClassW(&cls) == 0 {
            return 2;
        }
        let hwnd = dcx_win32::CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            0,
            0,
            GetModuleHandleW(core::ptr::null()),
            core::ptr::null_mut(),
        );
        if hwnd == 0 {
            return 2;
        }
        // The host launches this process with `--no-shot` and then touches the
        // marker. Depending on scheduling we may read either side of that
        // write: if the marker is fresh it *is* the request, if it arrives
        // after us the first timer tick sees the change. Without this the
        // hotkey either cancelled its own shot or did nothing.
        let marker_ms = read_ms("snip_show");
        let fresh = marker_ms > 0 && marker_ms + 2_000 >= started_ms;
        let cfg = config::load();
        let app = Box::new(App {
            last_marker: marker_ms,
            last_reshoot: read_ms("snip_reshoot"),
            pending_reshoot: false,
            apps_on: apps_enabled(),
            apps_checked: GetTickCount64(),
            idle_since: 0,
            idle_exit_secs: cfg.activation.idle_exit_seconds.max(5) as u64,
        });
        let ptr = Box::into_raw(app);
        dcx_win32::SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        SetTimer(hwnd, TIMER_TICK, TICK_MS, core::ptr::null());
        if !no_shot || fresh {
            (*ptr).shot();
        }

        let mut msg = dcx_win32::MSG::default();
        loop {
            let r = GetMessageW(&mut msg, 0, 0, 0);
            if r <= 0 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        KillTimer(hwnd, TIMER_TICK);
        drop(Box::from_raw(ptr));
        0
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_CLOSE => {
            // The host asks us to stop by title; finish the session first so a
            // half-drawn annotation is not left on screen.
            crate::session::cancel();
            PostQuitMessage(0);
            0
        }
        WM_TIMER if wp == TIMER_TICK => {
            let ptr = dcx_win32::GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
            if !ptr.is_null() {
                unsafe { (*ptr).tick() };
            }
            0
        }
        WM_APP_TICK => {
            let ptr = dcx_win32::GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
            if !ptr.is_null() {
                unsafe { (*ptr).tick() };
            }
            0
        }
        _ => unsafe { dcx_win32::DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

impl App {
    fn tick(&mut self) {
        crate::session::reap();
        let now = unsafe { GetTickCount64() };

        if now.saturating_sub(self.apps_checked) > 2_000 {
            self.apps_on = apps_enabled();
            self.apps_checked = now;
        }

        // Hotkey / tray: a new `snip_show` value means "shoot, or cancel the
        // overlay that is already up".
        let show = read_ms("snip_show");
        if show != 0 && show != self.last_marker {
            self.last_marker = show;
            if crate::session::is_open() {
                // A hotkey while the overlay is up cancels it. During a long
                // capture that means back to the selection, not silently
                // dropping the stitched frames off the screen.
                crate::session::hotkey_toggle();
            } else {
                self.shot();
            }
        }

        // A human launching the exe a second time asks for a new frame, not a
        // cancel. It gets its own marker so it is never confused with the
        // hotkey's toggle semantics.
        let reshoot = read_ms("snip_reshoot");
        if reshoot != 0 && reshoot != self.last_reshoot {
            self.last_reshoot = reshoot;
            if crate::session::is_open() {
                crate::session::cancel();
            }
            self.pending_reshoot = true;
        }
        if self.pending_reshoot && !crate::session::is_open() {
            self.pending_reshoot = false;
            self.shot();
            return;
        }

        if crate::session::is_open() {
            self.idle_since = 0;
            return;
        }
        if self.apps_on {
            self.idle_since = 0;
            return;
        }
        if self.idle_since == 0 {
            self.idle_since = now;
        } else if now.saturating_sub(self.idle_since) > self.idle_exit_secs * 1000 {
            log("idle timeout, exiting");
            unsafe { PostQuitMessage(0) };
        }
    }

    fn shot(&mut self) {
        match crate::capture::grab() {
            Ok(shot) => {
                let cfg = config::load();
                if let Err(e) = crate::session::open(shot, cfg) {
                    log(&format!("open overlay failed: {e}"));
                }
            }
            Err(e) => log(&format!("capture failed: {e}")),
        }
    }
}

fn already_running() -> bool {
    const ERROR_ALREADY_EXISTS: u32 = 183;
    unsafe {
        let name = crate::wide("DCX_SNIP_SINGLETON");
        let handle = CreateMutexW(core::ptr::null_mut(), 0, name.as_ptr());
        handle != 0 && GetLastError() == ERROR_ALREADY_EXISTS
    }
}

fn marker_path(name: &str) -> std::path::PathBuf {
    config::local_dir().join(name)
}

fn read_ms(name: &str) -> u64 {
    std::fs::read_to_string(marker_path(name))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

/// Atomic: the app polls these files every 60 ms, and a truncate-then-write
/// exposed an empty value that used to look like a brand-new request.
fn touch_marker(name: &str) {
    let stamp = epoch_ms();
    let dir = config::local_dir();
    let path = dir.join(name);
    let tmp = dir.join(format!("{name}.tmp{}", std::process::id()));
    if std::fs::write(&tmp, stamp.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn apps_enabled() -> bool {
    let path = config::local_dir().join("apps.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(doc) = text.parse::<toml::Value>() else {
        return false;
    };
    doc.get("apps")
        .and_then(|a| a.get("snip"))
        .and_then(|s| s.get("enabled"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

pub fn log(msg: &str) {
    use std::io::Write;
    let mut path = config::local_dir();
    path.push("snip.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{now}] {msg}");
    }
}

/// `--probe`: capture once, report the geometry, never open a window. This is
/// the only way to verify the capture path without a human looking at a screen.
fn probe() -> i32 {
    unsafe {
        let ok = dcx_win32::SetProcessDpiAwarenessContext(-4);
        log(&format!(
            "dpi: set_pmv2={ok} err={} system={} screen={}x{}",
            GetLastError(),
            GetDpiForSystem(),
            dcx_win32::GetSystemMetrics(SM_XVIRTUALSCREEN),
            dcx_win32::GetSystemMetrics(SM_CXVIRTUALSCREEN)
        ));
    }
    match crate::capture::grab() {
        Ok(shot) => {
            let sample = shot.bgra.iter().step_by(4).take(16).copied().collect::<Vec<_>>();
            log(&format!(
                "probe ok: origin=({}, {}) size={}x{} bytes={} sample={:?}",
                shot.origin.0,
                shot.origin.1,
                shot.w,
                shot.h,
                shot.bgra.len(),
                sample
            ));
            0
        }
        Err(e) => {
            log(&format!("probe failed: {e}"));
            1
        }
    }
}

#[allow(dead_code)]
fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}
