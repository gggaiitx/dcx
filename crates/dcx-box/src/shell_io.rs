//! Shell actions behind the 桌面收纳盒 panel.
//!
//! Inbound drag-and-drop does **not** live here: winit registers its own OLE
//! drop target on every window it creates, which is what turns a Windows drag
//! into egui's `dropped_files` / `hovered_files`. A second `RegisterDragDrop`
//! on the same HWND fails with `DRAGDROP_E_ALREADYREGISTERED` (0x80040101).
//!
//! Everything here is an *explicit user action*. Classification itself never
//! calls into this module — collecting a file moves it into the library, and no
//! file is moved, renamed or deleted as a side effect of filing it.

use std::ffi::c_void;
use std::path::Path;

#[link(name = "shell32")]
extern "system" {
    fn SHFileOperationW(op: *const SHFileOpStructW) -> i32;
    /// The dedicated Properties-dialog API. `ShellExecuteW("properties", …)`
    /// returns 31 (SE_ERR_NOASSOC) on Windows 11 — the verb is not exposed
    /// through ShellExecute — so the property sheet never appeared.
    fn SHObjectProperties(
        hwnd: isize,
        object_type: u32,
        name: *const u16,
        page: *const u16,
    ) -> i32;
    fn SHParseDisplayName(
        name: *const u16,
        bind_ctx: *mut c_void,
        pidl: *mut *mut c_void,
        sfgao_in: u32,
        sfgao_out: *mut u32,
    ) -> i32;
    fn SHOpenFolderAndSelectItems(
        pidl_folder: *const c_void,
        cidl: u32,
        apidl: *const *const c_void,
        flags: u32,
    ) -> i32;
    fn ILFree(pidl: *mut c_void);
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
}

#[repr(C)]
struct SHFileOpStructW {
    hwnd: isize,
    func: u32,
    from: *const u16,
    to: *const u16,
    flags: u16,
    aborted: i32,
    name_mappings: *mut c_void,
    progress_title: *const u16,
}

const FO_DELETE: u32 = 0x0003;
const FOF_SILENT: u16 = 0x0004;
const FOF_NOCONFIRMATION: u16 = 0x0010;
const FOF_ALLOWUNDO: u16 = 0x0040;
const FOF_NOERRORUI: u16 = 0x0400;

const SW_SHOWNORMAL: i32 = 1;
const SHOP_FILEPATH: u32 = 0x0000_0002;
const COINIT_APARTMENTTHREADED: u32 = 0x2;

/// Open a file with its default handler.
pub fn open_path(path: &Path) -> Result<(), String> {
    let file = dcx_win32::wide(&path.to_string_lossy());
    let op = dcx_win32::wide("open");
    let code = unsafe {
        dcx_win32::ShellExecuteW(
            0,
            op.as_ptr(),
            file.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    shell_execute_result(code)
}

/// Open the shell's property sheet for a file or folder.
pub fn open_properties(path: &Path) -> Result<(), String> {
    let file = dcx_win32::wide(&path.to_string_lossy());
    let ok = unsafe {
        SHObjectProperties(
            0,
            SHOP_FILEPATH,
            file.as_ptr(),
            core::ptr::null(),
        )
    };
    if ok != 0 {
        Ok(())
    } else {
        Err(error_text("SHObjectProperties"))
    }
}

/// Open Explorer with the file selected.
///
/// `explorer.exe /select,"…"` cannot be passed through `Command::arg`: Rust
/// quotes the whole argument and escapes the inner quotes, and Explorer's own
/// parser then exits with code 1. The shell API is both quote-safe and tells
/// us whether it worked.
pub fn reveal(path: &Path) -> Result<(), String> {
    unsafe {
        CoInitializeEx(core::ptr::null_mut(), COINIT_APARTMENTTHREADED);
    }
    let file = dcx_win32::wide(&path.to_string_lossy());
    let mut pidl: *mut c_void = core::ptr::null_mut();
    let mut attrs = 0u32;
    let hr = unsafe {
        SHParseDisplayName(
            file.as_ptr(),
            core::ptr::null_mut(),
            &mut pidl,
            0,
            &mut attrs,
        )
    };
    if hr < 0 || pidl.is_null() {
        return Err(format!(
            "无法解析路径（0x{:08X}）",
            hr as u32
        ));
    }
    let hr = unsafe { SHOpenFolderAndSelectItems(pidl, 0, core::ptr::null(), 0) };
    unsafe { ILFree(pidl) };
    if hr < 0 {
        Err(format!("无法在资源管理器中定位（0x{:08X}）", hr as u32))
    } else {
        Ok(())
    }
}

/// Open a folder in Explorer.
pub fn open_dir(path: &Path) -> Result<(), String> {
    std::process::Command::new("explorer")
        .arg(path.as_os_str())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法启动资源管理器：{e}"))
}

/// `ShellExecuteW` returns a value > 32 on success; everything else is one of
/// the legacy `SE_ERR_*` codes (31 = no association).
fn shell_execute_result(code: isize) -> Result<(), String> {
    if code > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW 返回 {code}"))
    }
}

fn error_text(what: &str) -> String {
    let e = std::io::Error::last_os_error();
    match e.raw_os_error() {
        Some(0) | None => format!("{what} 失败"),
        Some(_) => format!("{what} 失败：{e}"),
    }
}

/// Move paths to the Recycle Bin (`FOF_ALLOWUNDO`), i.e. **undoable**.
///
/// `SHFileOperationW` needs a double-NUL-terminated list of wide paths. We
/// deliberately do not add `FOF_NOCONFIRMATION` without a UI confirmation of
/// our own: the panel asks first when `Config::confirm_delete` is on.
pub fn recycle(paths: &[impl AsRef<Path>]) -> Result<Vec<std::path::PathBuf>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut buf: Vec<u16> = Vec::new();
    for p in paths {
        let s = p.as_ref().to_string_lossy();
        buf.extend(s.encode_utf16());
        buf.push(0);
    }
    buf.push(0); // second NUL terminates the list

    let op = SHFileOpStructW {
        hwnd: 0,
        func: FO_DELETE,
        from: buf.as_ptr(),
        to: core::ptr::null(),
        flags: FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI,
        aborted: 0,
        name_mappings: core::ptr::null_mut(),
        progress_title: core::ptr::null(),
    };
    // SAFETY: `op.from` points at `buf`, alive for the whole call; all other
    // pointers are null and the struct is the documented layout.
    let code = unsafe { SHFileOperationW(&op) };
    if code != 0 {
        return Err(format!("SHFileOperationW 返回 {code}"));
    }
    // The API reports one code for the whole batch; a locked file can be
    // skipped while the call still succeeds. Only the paths that actually
    // vanished may be dropped from the classification manifest.
    Ok(paths
        .iter()
        .filter(|p| !p.as_ref().exists())
        .map(|p| p.as_ref().to_path_buf())
        .collect())
}
