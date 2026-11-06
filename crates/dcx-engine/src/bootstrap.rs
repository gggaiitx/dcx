//! Bootstrap: ask XAML to load us as a Test Automation Provider.
//!
//! Signature taken verbatim from `xamlom.h:93`:
//! `HRESULT WINAPI InitializeXamlDiagnosticsEx(PCWSTR, DWORD, PCWSTR, PCWSTR, CLSID, PCWSTR)`
//!
//! `CLSID` is passed by value. On the MS x64 ABI a 16-byte struct travels via a
//! hidden pointer, which Rust's `extern "system"` with `#[repr(C)]` reproduces.

#![allow(non_snake_case)]

use crate::com::*;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};

type FnInitializeXamlDiagnosticsEx = unsafe extern "system" fn(
    end_point_name: *const u16,
    pid: u32,
    dll_xaml_diagnostics: *const u16,
    tap_dll_name: *const u16,
    tap_clsid: Guid,
    site_name: *const u16,
) -> HRESULT;

const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> isize;
    fn GetProcAddress(module: isize, name: *const u8) -> *mut c_void;
    fn GetModuleFileNameW(module: isize, buf: *mut u16, size: u32) -> u32;
    fn GetModuleHandleExW(flags: u32, addr: *const u16, out: *mut isize) -> i32;
}

const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x0000_0002;
const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x0000_0004;

#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(hwnd: isize, attr: u32, value: *const c_void, size: u32) -> i32;
}

/// Windows draws a 1px DWM border/shadow around the taskbar window even when
/// the XAML background is transparent. `DWMWA_BORDER_COLOR = DWMWA_COLOR_NONE`
/// removes that full-width gray line; it does not affect the XAML capsule.
unsafe fn remove_taskbar_dwm_border(hwnd: isize) {
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_BORDER_COLOR,
        &DWMWA_COLOR_NONE as *const u32 as *const c_void,
        core::mem::size_of::<u32>() as u32,
    );
}

#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class: *const u16, name: *const u16) -> isize;
    fn FindWindowExW(parent: isize, after: isize, class: *const u16, name: *const u16) -> isize;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
    fn RegisterWindowMessageW(name: *const u16) -> u32;
    fn SendMessageTimeoutW(
        hwnd: isize,
        msg: u32,
        wp: usize,
        lp: isize,
        flags: u32,
        timeout_ms: u32,
        result: *mut usize,
    ) -> isize;
    fn SetWindowsHookExW(
        id_hook: i32,
        proc: Option<unsafe extern "system" fn(i32, usize, isize) -> isize>,
        hmod: isize,
        thread_id: u32,
    ) -> isize;
    fn UnhookWindowsHookEx(hook: isize) -> i32;
    fn CallNextHookEx(hook: isize, code: i32, wp: usize, lp: isize) -> isize;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLastError() -> u32;
}

const WH_CALLWNDPROC: i32 = 4;
const HC_ACTION: i32 = 0;
const SMTO_ABORTIFHUNG: u32 = 0x0002;
const SMTO_BLOCK: u32 = 0x0001;

static FIRED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Taskbar window handle + the registered reload message, published by
/// `connect()` so the settings-watch thread can marshal reloads onto the
/// taskbar UI thread (the only thread allowed to touch XAML).
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
static RUN_MSG: AtomicU32 = AtomicU32::new(0);

/// `CWPSTRUCT` layout: lParam, wParam, message, hwnd.
#[repr(C)]
struct CwpStruct {
    l_param: isize,
    w_param: usize,
    message: u32,
    hwnd: isize,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Path of *this* DLL.
///
/// `GetModuleFileNameW(0, ..)` returns the host executable (Explorer.EXE), which
/// makes XAML look for `DllGetClassObject` in explorer instead of in us — the
/// TAP then never gets sited. Resolve our own module by address instead.
unsafe fn current_module_path() -> Option<Vec<u16>> {
    let mut hmod: isize = 0;
    let ok = GetModuleHandleExW(
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        current_module_path as *const u16,
        &mut hmod,
    );
    if ok == 0 || hmod == 0 {
        return None;
    }
    let mut buf = vec![0u16; 1024];
    let n = GetModuleFileNameW(hmod, buf.as_mut_ptr(), buf.len() as u32);
    if n == 0 || n as usize >= buf.len() {
        return None;
    }
    buf.truncate(n as usize);
    buf.push(0);
    Some(buf)
}

pub fn do_attach() -> HRESULT {
    crate::log("do_attach: start");
    unsafe {
        let dll_path = match current_module_path() {
            Some(p) => p,
            None => {
                crate::log("do_attach: GetModuleFileName failed");
                return E_FAIL;
            }
        };
        let shown = String::from_utf16_lossy(&dll_path[..dll_path.len() - 1]);
        crate::log(&format!("do_attach: module = {shown}"));

        let wux = LoadLibraryExW(
            wide("Windows.UI.Xaml.dll").as_ptr(),
            core::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        );
        if wux == 0 {
            crate::log("do_attach: Windows.UI.Xaml.dll absent");
            return E_FAIL;
        }

        let proc = GetProcAddress(wux, b"InitializeXamlDiagnosticsEx\0".as_ptr());
        if proc.is_null() {
            crate::log("do_attach: no InitializeXamlDiagnosticsEx export");
            return E_FAIL;
        }
        let init: FnInitializeXamlDiagnosticsEx = core::mem::transmute(proc);

        let empty = wide("");
        let pid = dcx_win32::GetCurrentProcessId();

        // The XAML core answers only on one specific connection index, and that
        // index is not 1 in general. The reference walks up to 10000.
        for i in 1..=10000u32 {
            let name = wide(&format!("VisualDiagConnection{i}"));
            let hr = init(
                name.as_ptr(),
                pid,
                empty.as_ptr(),
                dll_path.as_ptr(),
                CLSID_DCXTAP,
                core::ptr::null(),
            );
            if hr != ERROR_NOT_FOUND_HR {
                crate::log(&format!(
                    "do_attach: connection {i} -> hr=0x{:08X}",
                    hr as u32
                ));
                // ABI-probe build: the visual tree is delivered to
                // `xaml::cb_on_change`, which runs `probe::run` once. Theme
                // application stays disabled until the probe results are in.
                return hr;
            }
        }
        crate::log("do_attach: all connection names taken");
        E_FAIL
    }
}

static ATTACHED: AtomicBool = AtomicBool::new(false);
static HOOK_MSG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

unsafe extern "system" fn cbt_proc(code: i32, wp: usize, lp: isize) -> isize {
    if code == HC_ACTION {
        let cwp = &*(lp as *const CwpStruct);
        // Tells "the hook never ran" apart from "it ran but our message never
        // reached this thread".
        let fired = FIRED.fetch_add(1, Ordering::Relaxed);
        if fired < 3 {
            crate::log(&format!(
                "cbt fired #{fired} msg=0x{:X} want=0x{:X} hwnd=0x{:X}",
                cwp.message,
                HOOK_MSG.load(Ordering::SeqCst),
                cwp.hwnd
            ));
        }
        if cwp.message == HOOK_MSG.load(Ordering::SeqCst) && cwp.message != 0 {
            if cwp.w_param == 0 {
                crate::log("cbt_proc: connect on taskbar UI thread");
                if !ATTACHED.swap(true, Ordering::SeqCst) {
                    let hr = do_attach();
                    crate::log(&format!("cbt_proc: attach hr=0x{:08X}", hr as u32));
                    if hr != S_OK {
                        // Let a later attempt try again.
                        ATTACHED.store(false, Ordering::SeqCst);
                    } else {
                        // Build the rule engine here, not only on a settings
                        // change: on a freshly started shell no settings write
                        // has happened yet, so an empty ENGINE left the taskbar
                        // unthemed until the user touched a setting. This also
                        // applies the window geometry (dock gap) once.
                        crate::build_engine_from_settings();
                    }
                }
            } else if cwp.w_param == 1 {
                // Posted from `schedule_apply`: enumeration has settled by now.
                crate::log("cbt_proc: apply on taskbar UI thread");
                crate::apply_all();
            } else if cwp.w_param == 2 {
                // Posted from the settings watch: rebuild + re-apply.
                crate::log("cbt_proc: reload settings on taskbar UI thread");
                crate::build_engine_from_settings();
                crate::reload_and_apply();
                // The hover effect is not a theme rule: it owns `RenderTransform`
                // and the button plate by itself, so it has to re-read the
                // settings and rebuild its geometry table on its own.
                crate::hover::on_settings_changed();
            } else {
                crate::log(&format!("cbt_proc: unknown wparam {}", cwp.w_param));
            }
            return 0;
        }
    }
    CallNextHookEx(0, code, wp, lp)
}

/// Everything that touches XAML runs on the taskbar's UI thread.
///
/// `SetValue` and the diagnostics connection both belong to that thread's XAML
/// core; the reference keeps them together too. For the ABI-probe build the hook
/// only carries the connect request.
pub fn connect() -> HRESULT {
    unsafe {
        if ATTACHED.load(Ordering::SeqCst) {
            return S_OK;
        }
        // Exactly what the reference targets (`GetTaskbarUiWnd`): the
        // DesktopWindowContentBridge child of the taskbar, not Shell_TrayWnd
        // itself. That window is the XAML host; only its thread owns the
        // taskbar's XAML core, and only there does the diagnostics endpoint
        // answer.
        let tray = FindWindowW(wide("Shell_TrayWnd").as_ptr(), core::ptr::null());
        if tray == 0 {
            crate::log("connect: Shell_TrayWnd not found");
            return E_FAIL;
        }
        remove_taskbar_dwm_border(tray);
        // `GetTaskbarUiWnd` (reference 20459-20467) verbatim: the island is a
        // *direct* child of Shell_TrayWnd. Searching the whole subtree is wrong -
        // other DesktopWindowContentBridge windows (Start menu, search) hang off
        // the taskbar too and belong to a different thread, and attaching to one
        // of those never delivers the message.
        let bridge = FindWindowExW(
            tray,
            0,
            wide("Windows.UI.Composition.DesktopWindowContentBridge").as_ptr(),
            core::ptr::null(),
        );
        if bridge == 0 {
            // Shell_TrayWnd itself does not own the taskbar XAML core, so a
            // fallback there just burns the connection-name loop. Retry later.
            crate::log("connect: DesktopWindowContentBridge not ready, retrying later");
            return E_FAIL;
        }
        let target = bridge;
        crate::log(&format!("connect: target hwnd=0x{target:X}"));
        let tid = GetWindowThreadProcessId(target, core::ptr::null_mut());
        if tid == 0 {
            crate::log("connect: no thread owns the bridge window");
            return E_FAIL;
        }
        // Measured on a healthy shell: the taskbar island lives on explorer's
        // main thread, the same one that owns Shell_TrayWnd. Anything else means
        // we are about to attach to the wrong thread.
        let tray_tid = GetWindowThreadProcessId(tray, core::ptr::null_mut());
        if tid != tray_tid {
            crate::log(&format!(
                "connect: bridge thread {tid} is not the taskbar thread {tray_tid}, skipping"
            ));
            return E_FAIL;
        }
        let msg = RegisterWindowMessageW(wide("DCX_RunFromWindowThread").as_ptr());
        HOOK_MSG.store(msg, Ordering::SeqCst);
        // Publish the handle + message so the settings-watch thread can marshal
        // reloads onto the taskbar UI thread.
        TRAY_HWND.store(target, Ordering::SeqCst);
        RUN_MSG.store(msg, Ordering::SeqCst);

        // Pre-flight, all measured in this process so the numbers are
        // consistent: is the target thread even pumping before we touch it?
        let mut probe: usize = 0;
        let alive = SendMessageTimeoutW(target, 0, 0, 0, SMTO_ABORTIFHUNG, 2000, &mut probe);
        crate::log(&format!(
            "connect: tid={tid} tray_tid={} wm_null_ok={alive} msg=0x{msg:X}, installing hook",
            GetWindowThreadProcessId(tray, core::ptr::null_mut())
        ));

        let hook = SetWindowsHookExW(WH_CALLWNDPROC, Some(cbt_proc), 0, tid);
        if hook == 0 {
            crate::log(&format!(
                "connect: SetWindowsHookEx failed, last error {}",
                GetLastError()
            ));
            return E_FAIL;
        }
        crate::log("connect: hook installed, sending");
        // Never block the attach thread on the taskbar's thread: a plain
        // `SendMessageW` here waits forever if that thread is busy or hung.
        let mut result: usize = 0;
        let ok = SendMessageTimeoutW(
            target,
            msg,
            0,
            0,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            5000,
            &mut result,
        );
        UnhookWindowsHookEx(hook);
        crate::log(&format!(
            "connect: send ok={ok} fired={} attached={}",
            FIRED.load(Ordering::Relaxed),
            ATTACHED.load(Ordering::SeqCst)
        ));

        if ATTACHED.load(Ordering::SeqCst) {
            // XAML delivers the visual tree asynchronously after Advise, so the
            // first pass has to run a little later. All XAML work stays on the
            // taskbar UI thread; a posted message is how we get back there.
            spawn_settings_watch();
            schedule_apply(target, msg);
            S_OK
        } else {
            E_FAIL
        }
    }
}

fn schedule_apply(tray: isize, msg: u32) {
    // Several passes: the first enumeration has no layout yet, so `ActualWidth`
    // captures and the `{{...}}` rules that consume them only become correct
    // once XAML has measured the taskbar. The reference reacts to the variable
    // change instead; re-applying is the same effect with less bookkeeping.
    std::thread::spawn(move || {
        for delay in [1200u64, 2400, 4200, 7000, 20_000, 45_000, 75_000] {
            std::thread::sleep(std::time::Duration::from_millis(delay));
            post_to_taskbar(tray, msg, 1);
        }
    });
}

/// Marshal a call onto the taskbar UI thread (the only thread allowed to touch
/// XAML) by installing a brief `WH_CALLWNDPROC` hook, exactly like `connect`.
pub(crate) fn post_to_taskbar(tray: isize, msg: u32, wparam: usize) {
    unsafe {
        let tid = GetWindowThreadProcessId(tray, core::ptr::null_mut());
        if tid == 0 {
            return;
        }
        let hook = SetWindowsHookExW(WH_CALLWNDPROC, Some(cbt_proc), 0, tid);
        if hook == 0 {
            return;
        }
        let mut result: usize = 0;
        SendMessageTimeoutW(
            tray,
            msg,
            wparam,
            0,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            5000,
            &mut result,
        );
        UnhookWindowsHookEx(hook);
    }
}

static SETTINGS_WATCH_STARTED: AtomicBool = AtomicBool::new(false);

/// Watch the settings file for writes; when it changes, marshal a reload onto
/// the taskbar UI thread. Runs until the process exits.
pub(crate) fn spawn_settings_watch() {
    if !SETTINGS_WATCH_STARTED.swap(true, Ordering::SeqCst) {
        std::thread::spawn(|| {
            let mut last = settings_mtime();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(1000));
                let now = settings_mtime();
                if now != 0 && now != last {
                    last = now;
                    let tray = TRAY_HWND.load(Ordering::SeqCst);
                    let msg = RUN_MSG.load(Ordering::SeqCst);
                    if tray != 0 && msg != 0 {
                        crate::log("settings watch: file changed, posting reload");
                        post_to_taskbar(tray, msg, 2);
                    }
                }
            }
        });
    }
}

fn settings_mtime() -> u64 {
    use std::os::windows::fs::MetadataExt;
    let p = dcx_settings::Settings::path();
    match std::fs::metadata(&p) {
        Ok(m) => m.last_write_time(),
        Err(_) => 0,
    }
}
