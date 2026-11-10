//! Auto-injection of the taskbar styling engine (`dcx_engine.dll`) into
//! Explorer.exe.
//!
//! Why a watchdog instead of a one-shot: Explorer can restart (crash, `taskkill`,
//! logoff/on), and a restarted Explorer is a fresh process with no DLL. So we
//! poll every few seconds, inject any Explorer that does not yet host the DLL,
//! and forget PIDs that have gone away.
//!
//! Injection itself is delegated to the existing `wh_injector.exe` (classic
//! `VirtualAllocEx → WriteProcessMemory → CreateRemoteThread(LoadLibraryW)`),
//! which we shell out to with `<pid> <dll>` arguments. Paths are resolved from
//! the host's own directory, overridable via `DCX_INJECTOR` / `DCX_ENGINE_DLL`
//! for development.
//!
//! **Capability gate.** The engine only understands the Windows 11 XAML taskbar.
//! On Windows 10 (and older) `DesktopWindowContentBridge` does not exist, so an
//! injected engine would retry `FindWindowExW` forever inside Explorer while
//! never applying anything. Every entry point below therefore checks
//! `dcx_win32::platform::taskbar_style_supported()` and becomes a no-op / an
//! explicit error when it is false. Standalone apps do not use this module.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Set while a shell restart is in progress (`shutdown::restore_taskbar_and_quit`).
///
/// The watchdog keeps running until the process exits, so without this flag it
/// would re-inject the engine into the fresh Explorer during that window and the
/// "restore" would silently undo itself.
static PAUSED: AtomicBool = AtomicBool::new(false);

// --- Win32 toolhelp snapshots (kernel32, linked by default on windows-msvc) ---
extern "system" {
    fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> isize;
    fn CloseHandle(handle: isize) -> i32;
    fn Process32FirstW(snapshot: isize, entry: *mut PROCESSENTRY32W) -> i32;
    fn Process32NextW(snapshot: isize, entry: *mut PROCESSENTRY32W) -> i32;
    fn Module32FirstW(snapshot: isize, entry: *mut MODULEENTRY32W) -> i32;
    fn Module32NextW(snapshot: isize, entry: *mut MODULEENTRY32W) -> i32;
}

const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
const TH32CS_SNAPMODULE: u32 = 0x0000_0008;
const TH32CS_SNAPMODULE32: u32 = 0x0000_0010;
const INVALID_HANDLE: isize = -1;

#[repr(C)]
struct PROCESSENTRY32W {
    dw_size: u32,
    cnt_usage: u32,
    th32_process_id: u32,
    th32_default_heap_id: usize,
    th32_module_id: u32,
    cnt_threads: u32,
    th32_parent_process_id: u32,
    pc_pri_class_base: i32,
    dw_flags: u32,
    sz_exe_file: [u16; 260],
}

#[repr(C)]
struct MODULEENTRY32W {
    dw_size: u32,
    th32_module_id: u32,
    th32_process_id: u32,
    glbl_cnt_usage: u32,
    proc_cnt_usage: u32,
    mod_base_addr: usize,
    mod_base_size: u32,
    h_module: usize,
    sz_module: [u16; 256],
    sz_exe_path: [u16; 260],
}

/// Read a NUL-terminated UTF-16 buffer into a `String`.
fn read_wstr(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// All currently-running Explorer.exe process ids.
fn explorer_pids() -> Vec<u32> {
    let mut pids = Vec::new();
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE {
        return pids;
    }
    let mut pe: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    pe.dw_size = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    if unsafe { Process32FirstW(snap, &mut pe) } != 0 {
        loop {
            let name = read_wstr(&pe.sz_exe_file);
            if name.eq_ignore_ascii_case("explorer.exe") {
                pids.push(pe.th32_process_id);
            }
            if unsafe { Process32NextW(snap, &mut pe) } == 0 {
                break;
            }
        }
    }
    unsafe { CloseHandle(snap) };
    pids
}

/// Whether `pid` already hosts a module whose name (or path) matches `name`.
/// Returns `false` on any failure so the caller falls through to injection.
fn has_module(pid: u32, name: &str) -> bool {
    let wanted = name.to_ascii_lowercase();
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) };
    if snap == INVALID_HANDLE {
        return false;
    }
    let mut me: MODULEENTRY32W = unsafe { std::mem::zeroed() };
    me.dw_size = std::mem::size_of::<MODULEENTRY32W>() as u32;
    let mut found = false;
    if unsafe { Module32FirstW(snap, &mut me) } != 0 {
        loop {
            let base = read_wstr(&me.sz_module).to_ascii_lowercase();
            let path = read_wstr(&me.sz_exe_path).to_ascii_lowercase();
            if base == wanted || path.ends_with(&wanted) {
                found = true;
                break;
            }
            if unsafe { Module32NextW(snap, &mut me) } == 0 {
                break;
            }
        }
    }
    unsafe { CloseHandle(snap) };
    found
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
}

fn injector_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("DCX_INJECTOR") {
        return Some(PathBuf::from(p));
    }
    exe_dir().map(|d| d.join("wh_injector.exe"))
}

fn dll_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("DCX_ENGINE_DLL") {
        return Some(PathBuf::from(p));
    }
    exe_dir().map(|d| d.join("dcx_engine.dll"))
}

/// Inject `dcx_engine.dll` into `pid` using `wh_injector.exe`.
fn inject(pid: u32) -> bool {
    let injector = match injector_path() {
        Some(p) => p,
        None => {
            eprintln!("[dcx-host] DCX_INJECTOR / wh_injector.exe not found next to host");
            return false;
        }
    };
    let dll = match dll_path() {
        Some(p) => p,
        None => {
            eprintln!("[dcx-host] DCX_ENGINE_DLL / dcx_engine.dll not found next to host");
            return false;
        }
    };
    match Command::new(&injector)
        .arg(pid.to_string())
        .arg(&dll)
        .status()
    {
        Ok(status) => {
            if !status.success() {
                eprintln!("[dcx-host] wh_injector exited {status} for pid {pid}");
            }
            status.success()
        }
        Err(e) => {
            eprintln!("[dcx-host] run wh_injector failed: {e}");
            false
        }
    }
}

/// Whether any `explorer.exe` is running — i.e. the shell is up.
pub fn shell_present() -> bool {
    !explorer_pids().is_empty()
}

/// Stop re-injecting. Used by the restore path; there is no automatic resume.
pub fn pause() {
    PAUSED.store(true, Ordering::SeqCst);
}

/// Undo [`pause`] — only for a restore attempt that could not go through.
pub fn resume() {
    PAUSED.store(false, Ordering::SeqCst);
}

pub fn is_paused() -> bool {
    PAUSED.load(Ordering::SeqCst)
}

/// The one place the injection watchdog asks "does this shell have a XAML
/// taskbar?". Cached per process in `dcx_win32::platform`, so calling it every
/// frame in the settings window is free.
fn supported() -> bool {
    dcx_win32::platform::taskbar_style_supported()
}

/// Spawn the background thread that keeps Explorer injected.
///
/// Unsupported platforms skip the thread entirely: enumerating Explorer PIDs
/// every 5 s and injecting a DLL that can never connect is exactly the silent
/// busy-loop the capability gate exists to prevent.
pub fn spawn_watchdog() {
    if !supported() {
        println!(
            "[dcx-host] taskbar styling disabled: {} (reason: {})",
            dcx_win32::platform::describe(),
            dcx_win32::platform::reason()
        );
        return;
    }
    thread::spawn(|| {
        let mut injected: HashSet<u32> = HashSet::new();
        // pid → (consecutive failures, next attempt). A permanently failing
        // injection used to respawn `wh_injector.exe` every 5 s for the whole
        // session, filling `host.log` and spawning processes forever.
        let mut retry: HashMap<u32, (u32, Instant)> = HashMap::new();
        loop {
            if PAUSED.load(Ordering::SeqCst) {
                // A restore is running and the host is about to die; keep the
                // thread alive but hands-off until then.
                thread::sleep(Duration::from_millis(250));
                continue;
            }
            let pids = explorer_pids();
            for &pid in &pids {
                if injected.contains(&pid) {
                    continue;
                }
                if !has_module(pid, "dcx_engine.dll") {
                    let now = Instant::now();
                    if let Some((_, due)) = retry.get(&pid) {
                        if now < *due {
                            continue;
                        }
                    }
                    if inject(pid) {
                        println!("[dcx-host] injected dcx_engine into explorer pid {pid}");
                        injected.insert(pid);
                        retry.remove(&pid);
                    } else {
                        let failures = retry.get(&pid).map(|(n, _)| n + 1).unwrap_or(1);
                        let delay = if failures >= 6 {
                            Duration::from_secs(300)
                        } else {
                            Duration::from_secs(5u64 << failures.min(5))
                        };
                        retry.insert(pid, (failures, now + delay));
                        eprintln!(
                            "[dcx-host] failed to inject explorer pid {pid} (attempt {failures}; \
                             retry in {} s — is wh_injector.exe / dcx_engine.dll present?)",
                            delay.as_secs()
                        );
                    }
                } else {
                    injected.insert(pid);
                    retry.remove(&pid);
                }
            }
            // Forget PIDs that have exited (e.g. Explorer restarted).
            injected.retain(|pid| pids.contains(pid));
            retry.retain(|pid, _| pids.contains(pid));
            thread::sleep(Duration::from_secs(5));
        }
    });
}

// ---------------------------------------------------------------------------
// Engine presence, for the settings UI
//
// The settings window needs to answer "did my change land?" without reading
// engine logs. It therefore needs to know whether Explorer actually hosts the
// DLL. That answer is sampled in the background (toolhelp snapshots are cheap
// but not free) and read lock-free from the UI thread.
// ---------------------------------------------------------------------------

/// One watchdog pass on demand: inject every Explorer that lost the engine.
///
/// The settings window's 「重新应用」 calls this right after rewriting the file,
/// so a shell that restarted — or a host started without the watchdog, like
/// `--settings` — gets the engine back without waiting out the 5 s poll. Unlike
/// the background pass it reports *why* it failed, because the user is standing
/// in front of the window waiting for the taskbar to change.
///
/// Runs on the caller's thread on purpose: `wh_injector` finishes in a few
/// hundred milliseconds and the answer belongs in this click.
pub fn inject_now() -> Result<(), String> {
    if !supported() {
        return Err(format!(
            "任务栏样式不可用：{}，{}",
            dcx_win32::platform::display_name(),
            dcx_win32::platform::reason()
        ));
    }
    if is_paused() {
        return Err("正在恢复任务栏，注入已暂停".to_string());
    }
    let pids = explorer_pids();
    if pids.is_empty() {
        return Err("没有发现 explorer.exe".to_string());
    }
    let missing: Vec<u32> = pids
        .into_iter()
        .filter(|&pid| !has_module(pid, "dcx_engine.dll"))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    for (what, path) in [
        ("wh_injector.exe", injector_path()),
        ("dcx_engine.dll", dll_path()),
    ] {
        if !path.is_some_and(|p| p.exists()) {
            return Err(format!(
                "找不到 {what}：把它放到宿主同目录，或用 DCX_INJECTOR / DCX_ENGINE_DLL 指定"
            ));
        }
    }
    let failed: Vec<u32> = missing.into_iter().filter(|&pid| !inject(pid)).collect();
    if failed.is_empty() {
        ENGINE_STATE.store(ENGINE_ATTACHED, Ordering::Relaxed);
        Ok(())
    } else {
        let list = failed
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!("explorer pid {list} 注入失败，详见宿主控制台"))
    }
}

/// No Explorer process found — usually a transient state during a shell restart.
pub const ENGINE_UNKNOWN: u8 = 0;
/// Explorer is running but does not host `dcx_engine.dll`.
pub const ENGINE_ABSENT: u8 = 1;
/// At least one Explorer process hosts the engine.
pub const ENGINE_ATTACHED: u8 = 2;
/// This OS has no XAML taskbar; no DLL was injected and no polling is running.
pub const ENGINE_UNSUPPORTED: u8 = 3;

static ENGINE_STATE: AtomicU8 = AtomicU8::new(ENGINE_UNKNOWN);

/// Latest sampled engine state (one of the `ENGINE_*` constants).
pub fn engine_state() -> u8 {
    if !supported() {
        return ENGINE_UNSUPPORTED;
    }
    ENGINE_STATE.load(Ordering::Relaxed)
}

/// Sample [`engine_state`] every 3 s. Cheap enough to keep running for the
/// lifetime of the host; safe to call once. On unsupported systems no thread is
/// started at all — `engine_state()` answers `ENGINE_UNSUPPORTED` directly.
pub fn spawn_status_monitor() {
    if !supported() {
        ENGINE_STATE.store(ENGINE_UNSUPPORTED, Ordering::Relaxed);
        return;
    }
    thread::spawn(|| loop {
        let pids = explorer_pids();
        let state = if pids.is_empty() {
            ENGINE_UNKNOWN
        } else if pids.iter().any(|&pid| has_module(pid, "dcx_engine.dll")) {
            ENGINE_ATTACHED
        } else {
            ENGINE_ABSENT
        };
        ENGINE_STATE.store(state, Ordering::Relaxed);
        thread::sleep(Duration::from_secs(3));
    });
}
