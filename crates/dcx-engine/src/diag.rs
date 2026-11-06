//! Crash diagnostics for the injected engine.
//!
//! XAML faults are opaque: WER only reports "AV in dcx_engine.dll +0x462b",
//! which changes meaning on every rebuild. A vectored exception handler gives
//! us the faulting address *and* a stack trace with module-relative offsets,
//! written to the same log as everything else, on the next injection.
//!
//! It only logs; it never handles the exception (`EXCEPTION_CONTINUE_SEARCH`).

#![allow(non_snake_case)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

#[link(name = "kernel32")]
extern "system" {
    fn AddVectoredExceptionHandler(first: u32, handler: *mut c_void) -> *mut c_void;
    fn RtlCaptureStackBackTrace(
        frames_to_skip: u32,
        frames_to_capture: u32,
        back_trace: *mut *mut c_void,
        back_trace_hash: *mut u32,
    ) -> u16;
    fn GetModuleHandleExW(flags: u32, addr: *const u16, out: *mut isize) -> i32;
    fn GetModuleFileNameW(module: isize, buf: *mut u16, size: u32) -> u32;
}

#[repr(C)]
struct ExceptionRecord {
    code: u32,
    flags: u32,
    record: *mut ExceptionRecord,
    address: *mut c_void,
    number_parameters: u32,
    information: [usize; 15],
}

#[repr(C)]
struct ExceptionPointers {
    record: *mut ExceptionRecord,
    context: *mut c_void,
}

static EVENTS: AtomicU32 = AtomicU32::new(0);

/// `EXCEPTION_DISPOSITION::ExceptionContinueSearch` — keep unwinding and let the
/// process's real handler deal with it. We only observe.
const EXCEPTION_CONTINUE_SEARCH: i32 = 1;

/// Codes worth a stack walk. Everything else returns immediately.
///
/// This started as a deny-list ("log anything except the noise XAML raises")
/// and that inverted the cost: CoreMessaging/XAML raise first-chance exceptions
/// continuously on the taskbar UI thread, so the handler ran `OpenOptions` +
/// `RtlCaptureStackBackTrace` on that thread over and over. Measured: with the
/// handler installed the taskbar thread stopped answering `WM_NULL` within a
/// second of injection and explorer sat at ~100% CPU. Only real faults are
/// interesting, and they are rare, so the filter is a small allow-list.
fn is_reportable(code: u32) -> bool {
    matches!(
        code,
        0xC000_0005 // STATUS_ACCESS_VIOLATION
            | 0xC000_00FD // STATUS_STACK_OVERFLOW
            | 0xC000_0374 // STATUS_HEAP_CORRUPTION
            | 0xC000_0409 // STATUS_STACK_BUFFER_OVERRUN (__fastfail)
            | 0xC000_0094 // STATUS_INTEGER_DIVIDE_BY_ZERO
            | 0xC000_0096 // STATUS_PRIVILEGED_INSTRUCTION
            | 0x8000_0002 // STATUS_DATATYPE_MISALIGNMENT
    )
}

unsafe extern "system" fn handler(info: *mut ExceptionPointers) -> i32 {
    let ep = &*info;
    let rec = &*ep.record;
    let code = rec.code;
    if !is_reportable(code) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    if EVENTS.fetch_add(1, Ordering::SeqCst) >= 12 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    crate::log(&format!(
        "!!! exception code=0x{code:08X} at {}",
        describe(rec.address)
    ));
    let mut frames: [*mut c_void; 24] = [core::ptr::null_mut(); 24];
    let n = RtlCaptureStackBackTrace(
        1,
        frames.len() as u32,
        frames.as_mut_ptr(),
        core::ptr::null_mut(),
    );
    for (i, f) in frames[..n as usize].iter().enumerate() {
        crate::log(&format!("    #{i} {}", describe(*f)));
    }
    EXCEPTION_CONTINUE_SEARCH
}

/// "module.dll+0x1234" for an address.
unsafe fn describe(addr: *mut c_void) -> String {
    if addr.is_null() {
        return "null".into();
    }
    let mut hmod: isize = 0;
    let ok = GetModuleHandleExW(0x0000_0004 | 0x0000_0002, addr as *const u16, &mut hmod);
    if ok == 0 || hmod == 0 {
        return format!("{addr:?} (no module)");
    }
    let mut buf = vec![0u16; 512];
    let n = GetModuleFileNameW(hmod, buf.as_mut_ptr(), buf.len() as u32);
    let name = String::from_utf16_lossy(&buf[..n as usize]);
    let short = name.rsplit(['\\', '/']).next().unwrap_or(&name).to_string();
    format!("{short}+0x{:X} ({addr:?})", addr as usize - hmod as usize)
}

pub fn install() {
    unsafe {
        AddVectoredExceptionHandler(1, handler as *mut c_void);
    }
}
