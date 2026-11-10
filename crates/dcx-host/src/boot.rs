//! Process entry plumbing: console routing and the single-instance guard.
//!
//! The host ships as a Windows GUI binary (`windows_subsystem = "windows"`), so
//! double-clicking `dcx-host.exe` gives a tray application instead of a console
//! window that would otherwise stay on screen for the whole session. Two
//! consequences are handled here:
//!
//!   * Launched from cmd / pwsh, `--help` and `--selftest` must still print, so
//!     the process attaches to the parent console when one exists.
//!   * With no console at all, `println!` writes nowhere; stdout and stderr are
//!     pointed at `%LOCALAPPDATA%\dcx\host.log` instead (rotated at 512 KB).

use std::fs::OpenOptions;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use dcx_win32::{
    wide, AttachConsole, CreateFileW, CreateMutexW, FindWindowW, GetLastError, GetLocalTime,
    GetStdHandle, IsIconic, SetForegroundWindow, SetStdHandle, ShowWindow, ATTACH_PARENT_PROCESS,
    ERROR_ALREADY_EXISTS, FILE_SHARE_READ, FILE_SHARE_WRITE, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    OPEN_EXISTING, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SW_RESTORE, SYSTEMTIME,
};

/// Session-local mutex name. `Local\` scopes it to the logon session, so two
/// users may each run their own host (and their own Explorer).
const MUTEX_NAME: &str = "Local\\DCX.Host.Singleton";

/// Title of the settings window, used to bring it back when the user starts a
/// second copy of the exe.
const SETTINGS_TITLE: &str = "DCX 设置";

/// Rotate the log once it exceeds this size; a single run costs a few KB.
const LOG_MAX_BYTES: u64 = 512 * 1024;

/// Route stdout/stderr to a console if one is reachable, else to the log file.
///
/// Must run before the first `println!`: Rust caches the std handles on first
/// use, so a later `SetStdHandle` would be ignored.
pub fn init_output() {
    // A handle that already exists is the caller's, not ours to replace: cmd
    // redirection (`> file`), pipes and IDE capture all arrive this way. Only a
    // double-clicked process — no strip, no parent shell — has nothing here.
    if std_handle_valid(STD_OUTPUT_HANDLE) {
        return;
    }
    // Nothing was handed to us. A parent console may still exist (launched from
    // a shell that closed its handles), so attach and reuse its buffer.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0 && reopen_console_streams() {
        return;
    }
    redirect_to_log();
}

fn std_handle_valid(slot: u32) -> bool {
    let handle = unsafe { GetStdHandle(slot) };
    handle != 0 && handle != INVALID_HANDLE_VALUE
}

/// Point the std handles at the (now attached) parent console's screen buffer.
/// Returns false when no usable handle could be opened.
fn reopen_console_streams() -> bool {
    let name = wide("CONOUT$");
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle == 0 {
        return false;
    }
    unsafe {
        SetStdHandle(STD_OUTPUT_HANDLE, handle);
        SetStdHandle(STD_ERROR_HANDLE, handle);
    }
    true
}

/// `%LOCALAPPDATA%\dcx\host.log` — next to `settings.toml`.
pub fn log_path() -> PathBuf {
    let mut p = dcx_settings::Settings::path();
    p.set_file_name("host.log");
    p
}

/// Local wall-clock stamp, `YYYY-MM-DD HH:MM:SS`.
pub fn stamp_now() -> String {
    let mut now = SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut now) };
    now.stamp()
}

fn redirect_to_log() {
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    rotate(&path);
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        // Nothing to report to and nowhere to report it — carry on silently.
        return;
    };
    let handle = file.as_raw_handle() as isize;
    unsafe {
        SetStdHandle(STD_OUTPUT_HANDLE, handle);
        SetStdHandle(STD_ERROR_HANDLE, handle);
    }
    // `SetStdHandle` does not take ownership, and dropping `file` would close
    // the handle the process now writes through.
    std::mem::forget(file);

    let mut now = SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut now) };
    println!(
        "\n===== dcx-host {} pid={} {} =====",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        now.stamp()
    );
}

/// Keep the log bounded: rename the previous run aside once it gets large.
fn rotate(path: &Path) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() <= LOG_MAX_BYTES {
        return;
    }
    let mut old = path.to_path_buf();
    old.set_file_name("host.log.old");
    let _ = std::fs::remove_file(&old);
    let _ = std::fs::rename(path, &old);
}

/// Is another host already running in this session?
///
/// Two hosts would each place a tray icon and each keep its own snapshot of
/// `settings.toml`, silently overwriting the other's edits, so the second
/// instance exits instead. When the duplicate was launched by hand — the usual
/// "double-clicked the exe twice" case — the existing settings window is
/// brought forward so the click still does something visible.
pub fn already_running() -> bool {
    let name = wide(MUTEX_NAME);
    let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
    if handle == 0 {
        // Creating the mutex failed for a reason other than a duplicate; not a
        // good enough reason to refuse service.
        return false;
    }
    if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        // The handle is deliberately leaked: the kernel closes it on exit, and
        // the name has to stay owned for the whole process lifetime.
        return false;
    }
    focus_settings();
    true
}

/// Bring the settings window to the front if it is already open.
///
/// The window has its own process now, so "open settings" twice must not create
/// two of them: the second launch raises the first and exits.
pub fn focus_settings_window() -> bool {
    let title = wide(SETTINGS_TITLE);
    let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
    if hwnd == 0 {
        return false;
    }
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd);
    }
    true
}

/// Paint the native title bar in the app's own surface colour.
///
/// The settings window keeps system decorations on purpose — snap layouts,
/// double-click to maximise, the standard drag region and the close button all
/// come for free and behave exactly like every other window. What it does not
/// keep is the default caption strip: Windows 11 lets a window recolour it
/// (`DWMWA_CAPTION_COLOR`, build 22000+), so the title bar is painted in the
/// same colour as the top of the page and the frame reads as one surface
/// instead of a grey band bolted onto the content.
///
/// All four calls are best-effort: on Windows 10 the attributes do not exist
/// and `DwmSetWindowAttribute` returns an error, which is why the return value
/// is ignored.
pub fn tint_caption(caption: (u8, u8, u8), text: (u8, u8, u8), dark: bool) {
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWA_CAPTION_COLOR: u32 = 35;
    const DWMWA_TEXT_COLOR: u32 = 36;

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            attr: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }

    let title = wide(SETTINGS_TITLE);
    let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
    if hwnd == 0 {
        return;
    }
    // COLORREF is 0x00BBGGRR, not RGB.
    let cref =
        |(r, g, b): (u8, u8, u8)| -> u32 { (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) };
    let cap = cref(caption);
    let txt = cref(text);
    let flag: i32 = i32::from(dark);
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, &cap as *const u32 as *const _, 4);
        DwmSetWindowAttribute(hwnd, DWMWA_TEXT_COLOR, &txt as *const u32 as *const _, 4);
        // Same as the caption: no contrasting outline around the frame.
        DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &cap as *const u32 as *const _, 4);
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &flag as *const i32 as *const _,
            4,
        );
    }
}

/// Bring the running instance's settings window to the front, if it is open.
fn focus_settings() {
    let title = wide(SETTINGS_TITLE);
    let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
    if hwnd == 0 {
        return;
    }
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd);
    }
}
