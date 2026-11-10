//! Win32 / GDI entry points that `dcx-win32` does not already declare.
//!
//! `dcx-win32` is the shared, deliberately small binding surface for the
//! platform; the snapshot overlay additionally needs painting, capture and a
//! handful of child-window calls. Declaring them here keeps the shared crate
//! from growing for one consumer. Everything is `extern "system"` and linked by
//! name, exactly like `dcx-host/src/hotkey.rs` does for its hotkey window.

#![allow(non_snake_case, dead_code)]

use core::ffi::c_void;
use dcx_win32::{BOOL, DWORD, HANDLE, HDC, HGDIOBJ, HINSTANCE, HMONITOR, HWND, LONG, LRESULT, POINT, RECT, UINT, WPARAM};

// -- messages ---------------------------------------------------------------

pub const WM_DESTROY: UINT = 0x0002;
pub const WM_CLOSE: UINT = 0x0010;
pub const WM_PAINT: UINT = 0x000F;
pub const WM_ERASEBKGND: UINT = 0x0014;
pub const WM_ACTIVATE: UINT = 0x0006;
pub const WM_SETFOCUS: UINT = 0x0007;
pub const WM_KILLFOCUS: UINT = 0x0008;
pub const WM_ENABLE: UINT = 0x000A;
pub const WM_SETCURSOR: UINT = 0x0020;
pub const WM_MOUSEACTIVATE: UINT = 0x0021;
pub const WM_GETMINMAXINFO: UINT = 0x0024;
pub const WM_NCHITTEST: UINT = 0x0084;
pub const WM_KEYDOWN: UINT = 0x0100;
pub const WM_KEYUP: UINT = 0x0101;
pub const WM_CHAR: UINT = 0x0102;
pub const WM_SYSKEYDOWN: UINT = 0x0104;
pub const WM_SYSKEYUP: UINT = 0x0105;
pub const WM_TIMER: UINT = 0x0113;
pub const WM_COMMAND: UINT = 0x0111;
pub const WM_MOUSEMOVE: UINT = 0x0200;
pub const WM_LBUTTONDOWN: UINT = 0x0201;
pub const WM_LBUTTONUP: UINT = 0x0202;
/// Without this constant the match arm became a *binding* and swallowed every
/// later message in the window procedure (keyboard, timer, WM_CLOSE).
pub const WM_LBUTTONDBLCLK: UINT = 0x0203;
pub const WM_RBUTTONDOWN: UINT = 0x0204;
pub const WM_RBUTTONUP: UINT = 0x0205;
pub const WM_MBUTTONDOWN: UINT = 0x0207;
pub const WM_MBUTTONUP: UINT = 0x0208;
pub const WM_INPUT: UINT = 0x00FF;
pub const WM_MOUSEWHEEL: UINT = 0x020A;
pub const WM_CAPTURECHANGED: UINT = 0x0215;
pub const WM_DPICHANGED: UINT = 0x02E0;

// -- window styles ----------------------------------------------------------

pub const WS_POPUP: DWORD = 0x8000_0000;
pub const WS_VISIBLE: DWORD = 0x1000_0000;
pub const WS_CHILD: DWORD = 0x4000_0000;
pub const WS_BORDER: DWORD = 0x0080_0000;
pub const WS_TABSTOP: DWORD = 0x0001_0000;
pub const WS_CLIPCHILDREN: DWORD = 0x0200_0000;
pub const ES_AUTOHSCROLL: DWORD = 0x0080;
pub const ES_LEFT: DWORD = 0x0000;

pub const WS_EX_TOPMOST: DWORD = 0x0000_0008;
pub const WS_EX_TOOLWINDOW: DWORD = 0x0000_0080;
pub const WS_EX_LAYERED: DWORD = 0x0008_0000;
pub const WS_EX_NOACTIVATE: DWORD = 0x0800_0000;
pub const WS_EX_TRANSPARENT: DWORD = 0x0000_0020;

pub const SW_HIDE: i32 = 0;
pub const SW_SHOW: i32 = 5;
pub const SW_SHOWNA: i32 = 8;

pub const HWND_TOPMOST: HWND = -1;
pub const SWP_NOSIZE: UINT = 0x0001;
pub const SWP_NOMOVE: UINT = 0x0002;
pub const SWP_NOZORDER: UINT = 0x0004;
pub const SWP_NOACTIVATE: UINT = 0x0010;
pub const SWP_SHOWWINDOW: UINT = 0x0040;
pub const SWP_FRAMECHANGED: UINT = 0x0020;

pub const GWLP_USERDATA: i32 = -21;
pub const GWLP_WNDPROC: i32 = -4;
pub const GWL_STYLE: i32 = -16;

/// `GetStockObject` indices.
pub const WHITE_BRUSH: i32 = 0;

// -- GDI --------------------------------------------------------------------

pub const SRCCOPY: DWORD = 0x00CC_0020;
pub const CAPTUREBLT: DWORD = 0x4000_0000;
pub const DIB_RGB_COLORS: UINT = 0;

pub const TRANSPARENT: i32 = 1;
pub const FW_NORMAL: i32 = 400;
pub const FW_MEDIUM: i32 = 500;
pub const FW_SEMIBOLD: i32 = 600;
pub const FW_BOLD: i32 = 700;
pub const DEFAULT_CHARSET: DWORD = 1;
pub const CLEARTYPE_QUALITY: DWORD = 5;

pub const DT_LEFT: UINT = 0x0000;
pub const DT_TOP: UINT = 0x0000;
pub const DT_CENTER: UINT = 0x0001;
pub const DT_SINGLELINE: UINT = 0x0020;
pub const DT_NOPREFIX: UINT = 0x0800;

// -- system metrics / cursors ----------------------------------------------

pub const SM_XVIRTUALSCREEN: i32 = 76;
pub const SM_YVIRTUALSCREEN: i32 = 77;
pub const SM_CXVIRTUALSCREEN: i32 = 78;
pub const SM_CYVIRTUALSCREEN: i32 = 79;

pub const IDC_ARROW: usize = 32512;
pub const IDC_IBEAM: usize = 32513;
pub const IDC_CROSS: usize = 32515;
pub const IDC_SIZEALL: usize = 32646;
pub const IDC_SIZENS: usize = 32645;
pub const IDC_SIZEWE: usize = 32644;
pub const IDC_SIZENWSE: usize = 32642;
pub const IDC_SIZENESW: usize = 32643;
pub const IDC_HAND: usize = 32649;

/// `GetAncestor` flags.
pub const GA_ROOT: UINT = 2;
/// `GetWindow` z-order commands.
pub const GW_HWNDNEXT: UINT = 2;

pub const VK_ESCAPE: i32 = 0x1B;pub const VK_RETURN: i32 = 0x0D;
pub const VK_DELETE: i32 = 0x2E;
pub const VK_BACK: i32 = 0x08;
pub const VK_SPACE: i32 = 0x20;
pub const VK_TAB: i32 = 0x09;
pub const VK_SHIFT: i32 = 0x10;
pub const VK_CONTROL: i32 = 0x11;
pub const VK_ALT: i32 = 0x12;
pub const VK_LEFT: i32 = 0x25;
pub const VK_UP: i32 = 0x26;
pub const VK_RIGHT: i32 = 0x27;
pub const VK_DOWN: i32 = 0x28;

/// `PAINTSTRUCT` — only `hdc` and `rcPaint` are read, but the size must match
/// or `EndPaint` walks past the end of our stack slot.

// -- layered windows (long-capture bar / preview) ---------------------------

pub const ULW_ALPHA: u32 = 0x0000_0002;
pub const LWA_COLORKEY: DWORD = 0x0000_0001;
pub const AC_SRC_OVER: u8 = 0x00;
pub const AC_SRC_ALPHA: u8 = 0x01;
/// Clicking a `WS_EX_NOACTIVATE` bar must not pull focus away from the page
/// being scrolled.
pub const MA_NOACTIVATE: LRESULT = 3;

// -- global hotkeys (long-capture finish / cancel) --------------------------

pub const WM_HOTKEY: UINT = 0x0312;
pub const MOD_CONTROL: UINT = 0x0002;
pub const MOD_NOREPEAT: UINT = 0x4000;

/// Layered-window blend descriptor for `UpdateLayeredWindow`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct BLENDFUNCTION {
    pub BlendOp: u8,
    pub BlendFlags: u8,
    pub SourceConstantAlpha: u8,
    pub AlphaFormat: u8,
}

#[link(name = "user32")]
extern "system" {
    pub fn UpdateLayeredWindow(
        hwnd: HWND,
        dst_dc: HDC,
        dst_pt: *const POINT,
        size: *const dcx_win32::SIZE,
        src_dc: HDC,
        src_pt: *const POINT,
        color_key: u32,
        blend: *const BLENDFUNCTION,
        flags: u32,
    ) -> BOOL;
    pub fn SetLayeredWindowAttributes(hwnd: HWND, key: u32, alpha: u8, flags: DWORD) -> BOOL;
    pub fn RegisterHotKey(hwnd: HWND, id: i32, modifiers: UINT, vk: UINT) -> BOOL;
    pub fn UnregisterHotKey(hwnd: HWND, id: i32) -> BOOL;
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PAINTSTRUCT {
    pub hdc: HDC,
    pub f_erase: BOOL,
    pub rc_paint: RECT,
    pub f_restore: BOOL,
    pub f_inc_update: BOOL,
    pub reserved: [u8; 32],
}

impl Default for PAINTSTRUCT {
    fn default() -> Self {
        PAINTSTRUCT {
            hdc: 0,
            f_erase: 0,
            rc_paint: RECT::default(),
            f_restore: 0,
            f_inc_update: 0,
            reserved: [0; 32],
        }
    }
}

#[link(name = "user32")]
extern "system" {
    pub fn SetCapture(hwnd: HWND) -> HWND;
    pub fn GetCapture() -> HWND;
    pub fn ReleaseCapture() -> BOOL;
    pub fn ScreenToClient(hwnd: HWND, pt: *mut POINT) -> BOOL;
    pub fn ClientToScreen(hwnd: HWND, pt: *mut POINT) -> BOOL;
    pub fn InvalidateRect(hwnd: HWND, rect: *const RECT, erase: BOOL) -> BOOL;
    pub fn UpdateWindow(hwnd: HWND) -> BOOL;
    pub fn BeginPaint(hwnd: HWND, ps: *mut PAINTSTRUCT) -> HDC;
    pub fn EndPaint(hwnd: HWND, ps: *const PAINTSTRUCT) -> BOOL;
    pub fn SetFocus(hwnd: HWND) -> HWND;
    pub fn GetFocus() -> HWND;
    pub fn GetKeyState(vk: i32) -> i16;
    pub fn GetAsyncKeyState(vk: i32) -> i16;
    pub fn LoadCursorW(instance: HINSTANCE, name: *const u16) -> HANDLE;
    pub fn SetCursor(cursor: HANDLE) -> HANDLE;
    pub fn SendMessageW(hwnd: HWND, msg: UINT, wparam: WPARAM, lparam: LRESULT) -> LRESULT;
    pub fn GetParent(hwnd: HWND) -> HWND;
    pub fn SetWindowTextW(hwnd: HWND, text: *const u16) -> BOOL;
    pub fn GetWindowTextW(hwnd: HWND, buf: *mut u16, max: i32) -> i32;
    pub fn GetWindowTextLengthW(hwnd: HWND) -> i32;
    pub fn KillTimer(hwnd: HWND, id: usize) -> BOOL;
    pub fn SetTimer(hwnd: HWND, id: usize, elapse: UINT, proc: *const c_void) -> usize;
    pub fn PostMessageW(hwnd: HWND, msg: UINT, wparam: WPARAM, lparam: LRESULT) -> BOOL;
    pub fn PostQuitMessage(code: i32);
    pub fn DestroyWindow(hwnd: HWND) -> BOOL;
    pub fn SetProcessDPIAware() -> BOOL;
    pub fn SystemParametersInfoW(action: UINT, param: UINT, data: *mut c_void, win_ini: UINT) -> BOOL;
    pub fn SetThreadDpiAwarenessContext(ctx: isize) -> isize;
    pub fn ShowWindow(hwnd: HWND, cmd: i32) -> BOOL;
    pub fn GetMessageW(msg: *mut dcx_win32::MSG, hwnd: HWND, min: UINT, max: UINT) -> BOOL;
    pub fn TranslateMessage(msg: *const dcx_win32::MSG) -> BOOL;
    pub fn DispatchMessageW(msg: *const dcx_win32::MSG) -> LRESULT;
    pub fn GetDC(hwnd: HWND) -> HDC;
    pub fn ReleaseDC(hwnd: HWND, dc: HDC) -> i32;
    pub fn GetCursorPos(pt: *mut POINT) -> BOOL;
    pub fn SetForegroundWindow(hwnd: HWND) -> BOOL;
    pub fn WindowFromPoint(pt: POINT) -> HWND;
    pub fn GetAncestor(hwnd: HWND, flags: UINT) -> HWND;
    pub fn GetWindow(hwnd: HWND, cmd: UINT) -> HWND;
    pub fn IsWindowVisible(hwnd: HWND) -> BOOL;
    pub fn IsIconic(hwnd: HWND) -> BOOL;
    pub fn GetCurrentProcessId() -> DWORD;
    pub fn MonitorFromPoint(pt: POINT, flags: DWORD) -> HMONITOR;
    pub fn GetDpiForSystem() -> UINT;
}

#[link(name = "gdi32")]
extern "system" {
    pub fn SetTextColor(dc: HDC, color: u32) -> u32;
    pub fn SetBkMode(dc: HDC, mode: i32) -> i32;
    pub fn SetBkColor(dc: HDC, color: u32) -> u32;
    pub fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: DWORD,
        underline: DWORD,
        strike_out: DWORD,
        charset: DWORD,
        out_precision: DWORD,
        clip_precision: DWORD,
        quality: DWORD,
        pitch_and_family: DWORD,
        face: *const u16,
    ) -> HGDIOBJ;
    pub fn DrawTextW(dc: HDC, text: *const u16, len: i32, rect: *mut RECT, flags: UINT) -> i32;
    pub fn GetTextExtentPoint32W(dc: HDC, text: *const u16, len: i32, size: *mut dcx_win32::SIZE) -> BOOL;
    pub fn CreateSolidBrush(color: u32) -> HGDIOBJ;
    pub fn GetStockObject(index: i32) -> HGDIOBJ;
    pub fn CallWindowProcW(old: isize, hwnd: HWND, msg: UINT, wp: WPARAM, lp: LRESULT) -> LRESULT;
    pub fn FillRect(dc: HDC, rect: *const RECT, brush: HGDIOBJ) -> i32;
    pub fn FrameRect(dc: HDC, rect: *const RECT, brush: HGDIOBJ) -> i32;
    pub fn GdiFlush() -> BOOL;
}

#[link(name = "kernel32")]
extern "system" {
    pub fn LoadLibraryW(name: *const u16) -> HINSTANCE;
    pub fn GetProcAddress(module: HINSTANCE, name: *const u8) -> *const c_void;
    pub fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
    pub fn CloseHandle(handle: HANDLE) -> BOOL;
    pub fn GetTickCount64() -> u64;
    pub fn CreateMutexW(attrs: *mut c_void, owner: BOOL, name: *const u16) -> HANDLE;
    pub fn GetLastError() -> DWORD;
}

#[link(name = "dwmapi")]
extern "system" {
    pub fn DwmFlush() -> LONG;
}

// -- clipboard --------------------------------------------------------------

pub const GMEM_MOVEABLE: UINT = 0x0002;
pub const CF_DIB: UINT = 8;

#[link(name = "user32")]
extern "system" {
    pub fn OpenClipboard(hwnd: HWND) -> BOOL;
    pub fn EmptyClipboard() -> BOOL;
    pub fn CloseClipboard() -> BOOL;
    pub fn SetClipboardData(format: UINT, mem: HANDLE) -> HANDLE;
    pub fn RegisterClipboardFormatW(name: *const u16) -> UINT;
    pub fn AttachThreadInput(attach: DWORD, to: DWORD, on: BOOL) -> BOOL;
    pub fn GetForegroundWindow() -> HWND;
    pub fn GetWindowThreadProcessId(hwnd: HWND, pid: *mut DWORD) -> DWORD;
    pub fn GetCurrentThreadId() -> DWORD;
    pub fn SetActiveWindow(hwnd: HWND) -> HWND;
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GlobalAlloc(flags: UINT, bytes: usize) -> HANDLE;
    pub fn GlobalLock(mem: HANDLE) -> *mut c_void;
    pub fn GlobalUnlock(mem: HANDLE) -> BOOL;
    pub fn GlobalFree(mem: HANDLE) -> HANDLE;
    pub fn GetLocalTime(out: *mut dcx_win32::SYSTEMTIME);
}

// -- installed font families ------------------------------------------------

/// `SYMBOL_CHARSET`: Wingdings / Webdings and friends. Excluded from the font
/// menu — they cannot render the text a user is annotating.
pub const SYMBOL_CHARSET: u8 = 2;

/// `LOGFONTW` (92 bytes). Only `lfCharSet` is written and `lfFaceName` read;
/// the rest exists so the size and alignment match what GDI expects to walk.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LOGFONTW {
    pub lfHeight: i32,
    pub lfWidth: i32,
    pub lfEscapement: i32,
    pub lfOrientation: i32,
    pub lfWeight: i32,
    pub lfItalic: u8,
    pub lfUnderline: u8,
    pub lfStrikeOut: u8,
    pub lfCharSet: u8,
    pub lfOutPrecision: u8,
    pub lfClipPrecision: u8,
    pub lfQuality: u8,
    pub lfPitchAndFamily: u8,
    pub lfFaceName: [u16; 32],
}

impl Default for LOGFONTW {
    fn default() -> Self {
        LOGFONTW {
            lfHeight: 0,
            lfWidth: 0,
            lfEscapement: 0,
            lfOrientation: 0,
            lfWeight: 0,
            lfItalic: 0,
            lfUnderline: 0,
            lfStrikeOut: 0,
            lfCharSet: 0,
            lfOutPrecision: 0,
            lfClipPrecision: 0,
            lfQuality: 0,
            lfPitchAndFamily: 0,
            lfFaceName: [0; 32],
        }
    }
}

/// `ENUMLOGFONTEXW`: the logical font plus the full name, style and script.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ENUMLOGFONTEXW {
    pub elfLogFont: LOGFONTW,
    pub elfFullName: [u16; 64],
    pub elfStyle: [u16; 32],
    pub elfScript: [u16; 32],
}

pub type FontEnumProcW =
    unsafe extern "system" fn(*const ENUMLOGFONTEXW, *const c_void, u32, isize) -> i32;

#[link(name = "gdi32")]
extern "system" {
    /// Walking every installed family: `lfCharSet = DEFAULT_CHARSET` asks for
    /// all character sets, and the callback filters from there.
    pub fn EnumFontFamiliesExW(
        dc: HDC,
        lf: *const LOGFONTW,
        proc: FontEnumProcW,
        lparam: isize,
        flags: u32,
    ) -> i32;
}
