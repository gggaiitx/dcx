//! Raw Win32 / GDI / DWM bindings.
//!
//! Deliberately hand-rolled instead of pulling the `windows` crate: the surface
//! the platform needs is small, builds stay instant, and the produced binary
//! carries no extra runtime. Swap this crate for `windows` later if you want
//! typed WinRT access — nothing above this layer depends on how it is bound.

#![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms)]

use core::ffi::c_void;

/// Cross-process locked, crash-safe persistence for the TOML documents that
/// double as IPC between the DCX processes.
pub mod config_file;

pub type BOOL = i32;
pub type DWORD = u32;
pub type UINT = u32;
pub type LONG = i32;
pub type HRESULT = i32;
pub type WPARAM = usize;
pub type LPARAM = isize;
pub type LRESULT = isize;

pub type HWND = isize;
pub type HDC = isize;
pub type HGDIOBJ = isize;
pub type HBITMAP = isize;
pub type HINSTANCE = isize;
pub type HMONITOR = isize;
pub type HWINEVENTHOOK = isize;
pub type HANDLE = isize;

pub const TRUE: BOOL = 1;
pub const FALSE: BOOL = 0;

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RECT {
    pub left: LONG,
    pub top: LONG,
    pub right: LONG,
    pub bottom: LONG,
}

impl RECT {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct POINT {
    pub x: LONG,
    pub y: LONG,
}

/// Local wall-clock time, for log headers (`GetLocalTime`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SYSTEMTIME {
    pub year: u16,
    pub month: u16,
    pub day_of_week: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub milliseconds: u16,
}

/// 100 ns ticks since 1601-01-01 — what `FileTimeToSystemTime` eats.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FILETIME {
    pub low: u32,
    pub high: u32,
}

impl SYSTEMTIME {
    /// `YYYY-MM-DD HH:MM:SS` — the shape log readers expect.
    pub fn stamp(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct WINDOWPLACEMENT {
    pub length: UINT,
    pub flags: UINT,
    pub showCmd: UINT,
    pub ptMinPosition: POINT,
    pub ptMaxPosition: POINT,
    pub rcNormalPosition: RECT,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SIZE {
    pub cx: LONG,
    pub cy: LONG,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: UINT,
    pub wParam: WPARAM,
    pub lParam: LPARAM,
    pub time: DWORD,
    pub pt: POINT,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BLENDFUNCTION {
    pub BlendOp: u8,
    pub BlendFlags: u8,
    pub SourceConstantAlpha: u8,
    pub AlphaFormat: u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct BITMAPINFOHEADER {
    pub biSize: DWORD,
    pub biWidth: LONG,
    pub biHeight: LONG,
    pub biPlanes: u16,
    pub biBitCount: u16,
    pub biCompression: DWORD,
    pub biSizeImage: DWORD,
    pub biXPelsPerMeter: LONG,
    pub biYPelsPerMeter: LONG,
    pub biClrUsed: DWORD,
    pub biClrImportant: DWORD,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct BITMAPINFO {
    pub bmiHeader: BITMAPINFOHEADER,
    pub bmiColors: [DWORD; 3],
}

// ---------------------------------------------------------------------------
// Shell icons + file attributes
// ---------------------------------------------------------------------------

/// `SHFILEINFOW` — only `hIcon` is used, but the struct has to be the full size
/// or `SHGetFileInfoW` refuses to fill it.
#[repr(C)]
pub struct SHFILEINFOW {
    pub hIcon: isize,
    pub iIcon: i32,
    pub dwAttributes: DWORD,
    pub szDisplayName: [u16; 260],
    pub szTypeName: [u16; 80],
}

impl Default for SHFILEINFOW {
    fn default() -> Self {
        SHFILEINFOW {
            hIcon: 0,
            iIcon: 0,
            dwAttributes: 0,
            szDisplayName: [0; 260],
            szTypeName: [0; 80],
        }
    }
}

#[repr(C)]
#[derive(Default)]
pub struct ICONINFO {
    pub fIcon: BOOL,
    pub xHotspot: DWORD,
    pub yHotspot: DWORD,
    pub hbmMask: HBITMAP,
    pub hbmColor: HBITMAP,
}

#[repr(C)]
#[derive(Default)]
pub struct BITMAP {
    pub bmType: LONG,
    pub bmWidth: LONG,
    pub bmHeight: LONG,
    pub bmWidthBytes: LONG,
    pub bmPlanes: u16,
    pub bmBitsPixel: u16,
    pub bmBits: *mut c_void,
}

pub const SHGFI_ICON: UINT = 0x0000_0100;
pub const SHGFI_LARGEICON: UINT = 0x0000_0000;
pub const SHGFI_SMALLICON: UINT = 0x0000_0001;
pub const SHGFI_PIDL: UINT = 0x0000_0008;
pub const SHGFI_USEFILEATTRIBUTES: UINT = 0x0000_0010;

pub const FILE_ATTRIBUTE_READONLY: DWORD = 0x0000_0001;
pub const FILE_ATTRIBUTE_HIDDEN: DWORD = 0x0000_0002;
pub const FILE_ATTRIBUTE_SYSTEM: DWORD = 0x0000_0004;
pub const FILE_ATTRIBUTE_NORMAL: DWORD = 0x0000_0080;
pub const INVALID_FILE_ATTRIBUTES: DWORD = 0xFFFF_FFFF;

pub const SHCNE_CREATE: LONG = 0x0000_0002;
pub const SHCNE_DELETE: LONG = 0x0000_0004;
pub const SHCNE_UPDATEDIR: LONG = 0x0000_1000;
pub const SHCNE_UPDATEITEM: LONG = 0x0000_2000;
pub const SHCNF_PATHW: UINT = 0x0005;
/// Process the notification queue before returning (synchronous).
pub const SHCNF_FLUSH: UINT = 0x1000;
/// Start processing but return immediately — the caller must not block on the
/// shell's relayout, which is what a UI thread sees as a stutter.
pub const SHCNF_FLUSHNOWAIT: UINT = 0x2000;

#[link(name = "shell32")]
extern "system" {
    pub fn SHGetFileInfoW(
        path: *const u16,
        attrs: DWORD,
        info: *mut SHFILEINFOW,
        size: UINT,
        flags: UINT,
    ) -> usize;
    pub fn SHChangeNotify(event: LONG, flags: UINT, item1: *const c_void, item2: *const c_void);
    pub fn SHGetFolderLocation(
        hwnd: isize,
        csidl: i32,
        token: isize,
        flags: DWORD,
        pidl: *mut *mut c_void,
    ) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    pub fn CoTaskMemFree(p: *mut c_void);
}

#[link(name = "user32")]
extern "system" {
    pub fn GetIconInfo(icon: isize, info: *mut ICONINFO) -> BOOL;
    pub fn DestroyIcon(icon: isize) -> BOOL;
}

#[link(name = "gdi32")]
extern "system" {
    pub fn GetDIBits(
        dc: HDC,
        bmp: HBITMAP,
        start: UINT,
        lines: UINT,
        bits: *mut c_void,
        info: *mut BITMAPINFO,
        usage: UINT,
    ) -> i32;
    pub fn GetObjectW(obj: HGDIOBJ, size: i32, out: *mut c_void) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GetFileAttributesW(path: *const u16) -> DWORD;
    pub fn SetFileAttributesW(path: *const u16, attrs: DWORD) -> BOOL;
}

#[repr(C)]
pub struct WNDCLASSW {
    pub style: UINT,
    pub lpfnWndProc: Option<unsafe extern "system" fn(HWND, UINT, WPARAM, LPARAM) -> LRESULT>,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: HINSTANCE,
    pub hIcon: isize,
    pub hCursor: isize,
    pub hbrBackground: isize,
    pub lpszMenuName: *const u16,
    pub lpszClassName: *const u16,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const WM_DESTROY: UINT = 0x0002;
pub const WM_TIMER: UINT = 0x0113;
pub const WM_QUIT: UINT = 0x0012;

pub const WS_POPUP: DWORD = 0x8000_0000;
pub const WS_CAPTION: DWORD = 0x00C0_0000;
pub const WS_EX_LAYERED: DWORD = 0x0008_0000;
pub const WS_EX_TRANSPARENT: DWORD = 0x0000_0020;
pub const WS_EX_TOPMOST: DWORD = 0x0000_0008;
pub const WS_EX_TOOLWINDOW: DWORD = 0x0000_0080;
pub const WS_EX_NOACTIVATE: DWORD = 0x0800_0000;
/// Set on every WinUI 3 / XAML island top level window (Start, Search,
/// Settings, the input host). Their content is composited independently of the
/// window surface, so DWM frame attributes around them stay visible as an empty
/// outline.
pub const WS_EX_NOREDIRECTIONBITMAP: DWORD = 0x0020_0000;

pub const GWL_STYLE: i32 = -16;
pub const GWL_EXSTYLE: i32 = -20;
/// Owner window for a top-level window (the `hwndParent` of `CreateWindowEx`).
pub const GWLP_HWNDPARENT: i32 = -8;

pub const SW_HIDE: i32 = 0;
pub const SW_SHOWNOACTIVATE: i32 = 4;
pub const SW_RESTORE: i32 = 9;

pub const DIB_RGB_COLORS: UINT = 0;
pub const BI_RGB: DWORD = 0;

pub const SRCCOPY: DWORD = 0x00CC_0020;
pub const HALFTONE: i32 = 4;

pub const ULW_ALPHA: DWORD = 0x0000_0002;
pub const AC_SRC_OVER: u8 = 0x00;
pub const AC_SRC_ALPHA: u8 = 0x01;

pub const PW_RENDERFULLCONTENT: UINT = 0x0000_0002;

pub const DWMWA_CLOAK: DWORD = 13;
pub const DWMWA_TRANSITIONS_FORCEDISABLED: DWORD = 3;
/// `DWMWA_WINDOW_CORNER_PREFERENCE`. Setting it on a window that has no frame
/// of its own makes DWM draw one (rounded outline + 1 px border), so it is only
/// meaningful for regular app windows.
pub const DWMWA_WINDOW_CORNER_PREFERENCE: DWORD = 33;
/// `DWMWA_BORDER_COLOR`
pub const DWMWA_BORDER_COLOR: DWORD = 34;
/// `DWMWCP_*` values for `DWMWA_WINDOW_CORNER_PREFERENCE`.
pub const DWMWCP_DEFAULT: i32 = 0;
pub const DWMWCP_DONOTROUND: i32 = 1;
pub const DWMWCP_ROUND: i32 = 2;
pub const DWMWCP_ROUNDSMALL: i32 = 3;
/// `DWMWA_COLOR_NONE` — "do not draw a border".
pub const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;

pub const EVENT_SYSTEM_MINIMIZESTART: DWORD = 0x0016;
pub const EVENT_SYSTEM_MINIMIZEEND: DWORD = 0x0017;

pub const WINEVENT_OUTOFCONTEXT: UINT = 0x0000;
pub const WINEVENT_SKIPOWNPROCESS: UINT = 0x0002;

pub const HWND_TOPMOST: HWND = -1;
pub const SWP_NOMOVE: UINT = 0x0002;
pub const SWP_NOSIZE: UINT = 0x0001;
pub const SWP_NOACTIVATE: UINT = 0x0010;
pub const SWP_SHOWWINDOW: UINT = 0x0040;
pub const SWP_NOZORDER: UINT = 0x0004;
pub const SWP_NOOWNERZORDER: UINT = 0x0200;
pub const SWP_NOSENDCHANGING: UINT = 0x0400;

// --- process / stdio plumbing (single-instance guard, console routing) ------

/// `SetStdHandle` slot for stdout.
pub const STD_OUTPUT_HANDLE: DWORD = -11i32 as DWORD;
/// `SetStdHandle` slot for stderr.
pub const STD_ERROR_HANDLE: DWORD = -12i32 as DWORD;
/// `AttachConsole` argument: use the parent process' console.
pub const ATTACH_PARENT_PROCESS: DWORD = u32::MAX;
/// `GetLastError` value reported by a mutex that already existed.
pub const ERROR_ALREADY_EXISTS: DWORD = 183;
pub const GENERIC_WRITE: DWORD = 0x4000_0000;
pub const FILE_SHARE_READ: DWORD = 0x0000_0001;
pub const FILE_SHARE_WRITE: DWORD = 0x0000_0002;
pub const OPEN_EXISTING: DWORD = 3;
pub const INVALID_HANDLE_VALUE: HANDLE = -1;

pub const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
pub const MONITOR_DEFAULTTONEAREST: DWORD = 2;

pub const GA_ROOT: UINT = 2;

// --- AppBar state (the shell's own taskbar switches) ------------------------

/// Ask the shell for its `ABS_*` mask. The mask comes back as the return value,
/// not through the struct.
pub const ABM_GETSTATE: DWORD = 0x0004;
/// Hand the shell a new `ABS_*` mask in `lParam`. Win10 1703 and later.
pub const ABM_SETSTATE: DWORD = 0x000A;
/// Hide the appbar until the pointer reaches the screen edge.
pub const ABS_AUTOHIDE: DWORD = 0x0001;
/// Keep the appbar above ordinary windows.
pub const ABS_ALWAYSONTOP: DWORD = 0x0002;

// ---------------------------------------------------------------------------
// user32
// ---------------------------------------------------------------------------

#[link(name = "user32")]
extern "system" {
    pub fn SetProcessDpiAwarenessContext(value: isize) -> BOOL;

    pub fn GetMessageW(msg: *mut MSG, hwnd: HWND, min: UINT, max: UINT) -> BOOL;
    pub fn TranslateMessage(msg: *const MSG) -> BOOL;
    pub fn DispatchMessageW(msg: *const MSG) -> LRESULT;
    pub fn PostQuitMessage(code: i32);
    pub fn PeekMessageW(msg: *mut MSG, hwnd: HWND, min: UINT, max: UINT, remove: UINT) -> BOOL;
    pub fn PostThreadMessageW(thread_id: DWORD, msg: UINT, wparam: WPARAM, lparam: LPARAM) -> BOOL;
    pub fn GetCurrentThreadId() -> DWORD;

    pub fn SetTimer(hwnd: HWND, id: usize, elapse: UINT, proc: usize) -> usize;
    pub fn KillTimer(hwnd: HWND, id: usize) -> BOOL;

    pub fn RegisterClassW(class: *const WNDCLASSW) -> u16;
    pub fn CreateWindowExW(
        ex_style: DWORD,
        class_name: *const u16,
        window_name: *const u16,
        style: DWORD,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: HWND,
        menu: isize,
        instance: HINSTANCE,
        param: *mut c_void,
    ) -> HWND;
    pub fn DestroyWindow(hwnd: HWND) -> BOOL;
    pub fn DefWindowProcW(hwnd: HWND, msg: UINT, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
    pub fn ShowWindow(hwnd: HWND, cmd: i32) -> BOOL;
    pub fn ShowWindowAsync(hwnd: HWND, cmd: i32) -> BOOL;
    pub fn CloseWindow(hwnd: HWND) -> BOOL;
    pub fn SetWindowPos(
        hwnd: HWND,
        after: HWND,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: UINT,
    ) -> BOOL;
    /// Topmost window under a screen point, or 0. Used to tell "our tile is on
    /// top" from "the taskbar was raised over it".
    pub fn WindowFromPoint(pt: POINT) -> HWND;
    /// Is `child` a descendant of `parent` (or `parent` itself)?
    pub fn IsChild(parent: HWND, child: HWND) -> BOOL;

    pub fn UpdateLayeredWindow(
        hwnd: HWND,
        dst_dc: HDC,
        dst_pt: *const POINT,
        size: *const SIZE,
        src_dc: HDC,
        src_pt: *const POINT,
        color_key: DWORD,
        blend: *const BLENDFUNCTION,
        flags: DWORD,
    ) -> BOOL;

    pub fn SetWinEventHook(
        event_min: DWORD,
        event_max: DWORD,
        hmod: HINSTANCE,
        proc: Option<
            unsafe extern "system" fn(HWINEVENTHOOK, DWORD, HWND, LONG, LONG, DWORD, DWORD),
        >,
        process_id: DWORD,
        thread_id: DWORD,
        flags: UINT,
    ) -> HWINEVENTHOOK;
    pub fn UnhookWinEvent(hook: HWINEVENTHOOK) -> BOOL;

    pub fn GetWindowRect(hwnd: HWND, rect: *mut RECT) -> BOOL;
    pub fn GetClientRect(hwnd: HWND, rect: *mut RECT) -> BOOL;
    pub fn GetWindowPlacement(hwnd: HWND, placement: *mut WINDOWPLACEMENT) -> BOOL;
    pub fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
    pub fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
    pub fn FindWindowExW(parent: HWND, after: HWND, class: *const u16, name: *const u16) -> HWND;
    pub fn GetAsyncKeyState(vk: i32) -> i16;
    pub fn ReleaseCapture() -> BOOL;
    pub fn IsWindow(hwnd: HWND) -> BOOL;
    pub fn IsWindowVisible(hwnd: HWND) -> BOOL;
    pub fn IsIconic(hwnd: HWND) -> BOOL;
    pub fn GetForegroundWindow() -> HWND;
    pub fn GetShellWindow() -> HWND;
    pub fn GetAncestor(hwnd: HWND, flags: UINT) -> HWND;
    pub fn GetClassNameW(hwnd: HWND, buf: *mut u16, max: i32) -> i32;
    pub fn FindWindowW(class: *const u16, name: *const u16) -> HWND;
    pub fn SetForegroundWindow(hwnd: HWND) -> BOOL;
    pub fn EnumWindows(
        proc: Option<unsafe extern "system" fn(HWND, LPARAM) -> BOOL>,
        lparam: LPARAM,
    ) -> BOOL;
    pub fn EnumChildWindows(
        parent: HWND,
        proc: Option<unsafe extern "system" fn(HWND, LPARAM) -> BOOL>,
        lparam: LPARAM,
    ) -> BOOL;
    pub fn GetWindowThreadProcessId(hwnd: HWND, pid: *mut DWORD) -> DWORD;
    pub fn GetSystemMetrics(index: i32) -> i32;
    pub fn PostMessageW(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> BOOL;
    pub fn SendNotifyMessageW(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> BOOL;

    pub fn GetCursorPos(pt: *mut POINT) -> BOOL;
    pub fn GetDpiForWindow(hwnd: HWND) -> UINT;
    pub fn GetDpiForSystem() -> UINT;

    pub fn GetDC(hwnd: HWND) -> HDC;
    pub fn ReleaseDC(hwnd: HWND, dc: HDC) -> i32;
    pub fn GetMonitorInfoW(monitor: HMONITOR, info: *mut MONITORINFO) -> BOOL;
    pub fn MonitorFromWindow(hwnd: HWND, flags: DWORD) -> HMONITOR;
    pub fn MonitorFromRect(rect: *const RECT, flags: DWORD) -> HMONITOR;
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MONITORINFO {
    pub cbSize: DWORD,
    pub rcMonitor: RECT,
    pub rcWork: RECT,
    pub dwFlags: DWORD,
}

/// `APPBARDATA`, the payload of `SHAppBarMessage`.
///
/// The `RECT` is four `LONG`s, not four pointers: that keeps the state/lParam
/// slot at offset 40 and the size at 48 on x64. Widen those edge coordinates and
/// `cbSize` no longer matches what the shell expects, which makes every AppBar
/// call fail without an obvious reason.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct APPBARDATA {
    pub cbSize: DWORD,
    pub hWnd: HWND,
    pub uMsg: UINT,
    pub rc: RECT,
    pub lParam: LPARAM,
}

// ---------------------------------------------------------------------------
// gdi32
// ---------------------------------------------------------------------------

#[link(name = "gdi32")]
extern "system" {
    pub fn CreateCompatibleDC(dc: HDC) -> HDC;
    pub fn CreateCompatibleBitmap(dc: HDC, w: i32, h: i32) -> HBITMAP;
    pub fn CreateDIBSection(
        dc: HDC,
        info: *const BITMAPINFO,
        usage: UINT,
        bits: *mut *mut c_void,
        section: HANDLE,
        offset: DWORD,
    ) -> HBITMAP;
    pub fn SelectObject(dc: HDC, obj: HGDIOBJ) -> HGDIOBJ;
    pub fn DeleteObject(obj: HGDIOBJ) -> BOOL;
    pub fn DeleteDC(dc: HDC) -> BOOL;
    pub fn BitBlt(
        dst: HDC,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        src: HDC,
        sx: i32,
        sy: i32,
        rop: DWORD,
    ) -> BOOL;
    pub fn StretchBlt(
        dst: HDC,
        xd: i32,
        yd: i32,
        wd: i32,
        hd: i32,
        src: HDC,
        xs: i32,
        ys: i32,
        ws: i32,
        hs: i32,
        rop: DWORD,
    ) -> BOOL;
    pub fn SetStretchBltMode(dc: HDC, mode: i32) -> i32;
    pub fn GdiFlush() -> BOOL;
}

// ---------------------------------------------------------------------------
// dwmapi
// ---------------------------------------------------------------------------

#[link(name = "dwmapi")]
extern "system" {
    pub fn DwmSetWindowAttribute(
        hwnd: HWND,
        attr: DWORD,
        value: *const c_void,
        size: DWORD,
    ) -> HRESULT;
    pub fn DwmGetWindowAttribute(
        hwnd: HWND,
        attr: DWORD,
        value: *mut c_void,
        size: DWORD,
    ) -> HRESULT;
    pub fn DwmFlush() -> HRESULT;
}

// ---------------------------------------------------------------------------
// shell32 / kernel32
// ---------------------------------------------------------------------------

#[link(name = "shell32")]
extern "system" {
    pub fn ShellExecuteW(
        hwnd: HWND,
        op: *const u16,
        file: *const u16,
        params: *const u16,
        dir: *const u16,
        show: i32,
    ) -> isize;

    /// Talk to a registered AppBar — including `Shell_TrayWnd` — about its
    /// geometry and state. Non-zero means the message was accepted.
    pub fn SHAppBarMessage(dwMessage: DWORD, pData: *mut APPBARDATA) -> LRESULT;
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
    pub fn GetCurrentProcessId() -> DWORD;
    pub fn Sleep(ms: DWORD);
    pub fn GetTickCount64() -> u64;
    pub fn QueryPerformanceCounter(out: *mut i64) -> BOOL;
    pub fn QueryPerformanceFrequency(out: *mut i64) -> BOOL;

    /// Named mutex used as the single-instance lock: the second caller gets a
    /// valid handle but `GetLastError() == ERROR_ALREADY_EXISTS`.
    pub fn CreateMutexW(attrs: *mut c_void, owner: BOOL, name: *const u16) -> HANDLE;
    pub fn GetLastError() -> DWORD;

    pub fn AttachConsole(pid: DWORD) -> BOOL;
    pub fn GetConsoleWindow() -> HWND;
    pub fn GetStdHandle(which: DWORD) -> HANDLE;
    pub fn SetStdHandle(which: DWORD, handle: HANDLE) -> BOOL;
    pub fn CreateFileW(
        name: *const u16,
        access: DWORD,
        share: DWORD,
        attrs: *mut c_void,
        disposition: DWORD,
        flags: DWORD,
        template: HANDLE,
    ) -> HANDLE;

    pub fn GetLocalTime(out: *mut SYSTEMTIME);
    /// UTC → local, for the "when did this arrive" stamp in the file box.
    pub fn FileTimeToLocalFileTime(utc: *const FILETIME, local: *mut FILETIME) -> BOOL;
    pub fn FileTimeToSystemTime(ft: *const FILETIME, out: *mut SYSTEMTIME) -> BOOL;
}

// ---------------------------------------------------------------------------
// advapi32 — 注册表：读写小工具（主题色 + 开机启动项）
// ---------------------------------------------------------------------------

#[link(name = "advapi32")]
extern "system" {
    fn RegOpenKeyExW(
        key: isize,
        sub_key: *const u16,
        options: u32,
        desired: u32,
        result: *mut isize,
    ) -> i32;
    fn RegCreateKeyExW(
        key: isize,
        sub_key: *const u16,
        reserved: u32,
        class: *mut u16,
        options: u32,
        desired: u32,
        security: *mut core::ffi::c_void,
        result: *mut isize,
        disposition: *mut u32,
    ) -> i32;
    fn RegQueryValueExW(
        key: isize,
        value_name: *const u16,
        reserved: *mut u32,
        ty: *mut u32,
        data: *mut u8,
        data_size: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        key: isize,
        value_name: *const u16,
        reserved: u32,
        ty: u32,
        data: *const u8,
        data_size: u32,
    ) -> i32;
    fn RegDeleteValueW(key: isize, value_name: *const u16) -> i32;
    /// 只有注册表自测（`registry_tests`）收尾时用得到，正式代码不删键。
    #[allow(dead_code)]
    fn RegDeleteKeyW(key: isize, sub_key: *const u16) -> i32;
    fn RegCloseKey(key: isize) -> i32;
}

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const KEY_QUERY_VALUE: u32 = 0x0001;
const KEY_SET_VALUE: u32 = 0x0002;
const REG_SZ: u32 = 1;
const REG_DWORD: u32 = 4;
const ERROR_FILE_NOT_FOUND: i32 = 2;

/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme`.
///
/// This is the same flag Explorer uses to pick light or dark chrome, so the
/// tray glyph can be white on a dark bar and dark on a light one. A missing
/// value (older builds) counts as light, which is the Windows default.
pub fn apps_use_light_theme() -> bool {
    unsafe {
        let path = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
        let mut key: isize = 0;
        if RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_QUERY_VALUE, &mut key) != 0 {
            return true;
        }
        let mut value: u32 = 1;
        let mut size = core::mem::size_of::<u32>() as u32;
        let mut ty: u32 = 0;
        let name = wide("AppsUseLightTheme");
        let ok = RegQueryValueExW(
            key,
            name.as_ptr(),
            core::ptr::null_mut(),
            &mut ty,
            &mut value as *mut u32 as *mut u8,
            &mut size,
        ) == 0
            && ty == REG_DWORD;
        RegCloseKey(key);
        !ok || value != 0
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Reads a string value under `HKCU\<path>`. `None` when the key, the value or
/// the type is missing — callers treat that as "not configured".
pub fn reg_read_string(path: &str, name: &str) -> Option<String> {
    unsafe {
        let sub = wide(path);
        let mut key: isize = 0;
        if RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_QUERY_VALUE, &mut key) != 0 {
            return None;
        }

        let value = wide(name);
        let mut ty: u32 = 0;
        let mut size: u32 = 0;
        let mut out = None;

        // First call with a null buffer only asks for the size (in bytes).
        let sized = RegQueryValueExW(
            key,
            value.as_ptr(),
            core::ptr::null_mut(),
            &mut ty,
            core::ptr::null_mut(),
            &mut size,
        ) == 0;

        if sized && ty == REG_SZ && size >= 2 {
            let mut buf = vec![0u8; size as usize];
            let read = RegQueryValueExW(
                key,
                value.as_ptr(),
                core::ptr::null_mut(),
                &mut ty,
                buf.as_mut_ptr(),
                &mut size,
            ) == 0;
            if read {
                let mut units: Vec<u16> = buf
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                while units.last() == Some(&0) {
                    units.pop();
                }
                out = Some(String::from_utf16_lossy(&units));
            }
        }

        RegCloseKey(key);
        out
    }
}

/// Writes a string value under `HKCU\<path>`, creating the key if needed.
/// Returns the raw `LSTATUS` so callers can show it verbatim.
pub fn reg_write_string(path: &str, name: &str, value: &str) -> Result<(), i32> {
    unsafe {
        let sub = wide(path);
        let mut key: isize = 0;
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            0,
            core::ptr::null_mut(),
            0,
            KEY_SET_VALUE,
            core::ptr::null_mut(),
            &mut key,
            core::ptr::null_mut(),
        );
        if rc != 0 {
            return Err(rc);
        }

        let name_w = wide(name);
        let mut data: Vec<u16> = value.encode_utf16().collect();
        data.push(0);
        let bytes = (data.len() * 2) as u32;
        let rc = RegSetValueExW(
            key,
            name_w.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr() as *const u8,
            bytes,
        );
        RegCloseKey(key);
        if rc == 0 { Ok(()) } else { Err(rc) }
    }
}

/// Reads a `REG_DWORD` under `HKCU\<path>`.
pub fn reg_read_u32(path: &str, name: &str) -> Option<u32> {
    unsafe {
        let sub = wide(path);
        let mut key: isize = 0;
        if RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_QUERY_VALUE, &mut key) != 0 {
            return None;
        }
        let value = wide(name);
        let mut ty: u32 = 0;
        let mut data: u32 = 0;
        let mut size = core::mem::size_of::<u32>() as u32;
        let ok = RegQueryValueExW(
            key,
            value.as_ptr(),
            core::ptr::null_mut(),
            &mut ty,
            &mut data as *mut u32 as *mut u8,
            &mut size,
        ) == 0
            && ty == REG_DWORD
            && size >= core::mem::size_of::<u32>() as u32;
        RegCloseKey(key);
        if ok {
            Some(data)
        } else {
            None
        }
    }
}

/// Writes a `REG_DWORD` under `HKCU\<path>`, creating the key if needed.
pub fn reg_write_u32(path: &str, name: &str, value: u32) -> Result<(), i32> {
    unsafe {
        let sub = wide(path);
        let mut key: isize = 0;
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            0,
            core::ptr::null_mut(),
            0,
            KEY_SET_VALUE,
            core::ptr::null_mut(),
            &mut key,
            core::ptr::null_mut(),
        );
        if rc != 0 {
            return Err(rc);
        }
        let name_w = wide(name);
        let rc = RegSetValueExW(
            key,
            name_w.as_ptr(),
            0,
            REG_DWORD,
            &value as *const u32 as *const u8,
            core::mem::size_of::<u32>() as u32,
        );
        RegCloseKey(key);
        if rc == 0 {
            Ok(())
        } else {
            Err(rc)
        }
    }
}

// ---------------------------------------------------------------------------
// Desktop icon layer
//
// Windows has no per-item "hide from the desktop but keep visible to file
// dialogs" flag. The only clean mechanism is Explorer's own
// 「查看 → 显示桌面图标」 toggle: it hides the whole icon layer without
// touching any file attribute, so 「另存为」 can still navigate the desktop.
// The command id is the same one Explorer's context menu posts (0x7402); the
// registry value keeps Explorer's own menu in sync.
// ---------------------------------------------------------------------------

const EXPLORER_ADVANCED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const WM_COMMAND: u32 = 0x0111;
const TOGGLE_DESKTOP_ICONS: usize = 0x7402;
const WM_SETTINGCHANGE: u32 = 0x001A;
const HWND_BROADCAST: isize = 0xFFFF;
const SW_SHOW: i32 = 5;
const GA_PARENT: UINT = 1;

struct DesktopShell {
    host: isize,
    defview: isize,
    listview: isize,
}

struct ClassSearch {
    class: &'static str,
    found: isize,
}

unsafe extern "system" fn find_class_cb(hwnd: isize, lparam: isize) -> BOOL {
    let search = &mut *(lparam as *mut ClassSearch);
    let mut buf = [0u16; 128];
    let n = GetClassNameW(hwnd, buf.as_mut_ptr(), 128);
    if n > 0 {
        let name = String::from_utf16_lossy(&buf[..n as usize]);
        if name.eq_ignore_ascii_case(search.class) {
            search.found = hwnd;
            return 0;
        }
    }
    1
}

/// `FindWindowExW` only checks direct children, and on Windows 11 26xxx the
/// desktop's `SysListView32` sits below a different nesting level than on
/// older builds. A recursive class search is the portable way in.
fn find_descendant(parent: isize, class: &'static str) -> isize {
    if parent == 0 {
        return 0;
    }
    let mut search = ClassSearch { class, found: 0 };
    unsafe {
        EnumChildWindows(
            parent,
            Some(find_class_cb),
            &mut search as *mut ClassSearch as isize,
        );
    }
    search.found
}

fn find_desktop_shell() -> DesktopShell {
    let shell = unsafe { GetShellWindow() };
    let shell = if shell != 0 {
        shell
    } else {
        unsafe { FindWindowW(wide("Progman").as_ptr(), core::ptr::null()) }
    };
    let mut defview = 0isize;
    if shell != 0 {
        defview = unsafe {
            FindWindowExW(
                shell,
                0,
                wide("SHELLDLL_DefView").as_ptr(),
                core::ptr::null(),
            )
        };
    }
    if defview == 0 {
        defview = find_descendant(shell, "SHELLDLL_DefView");
    }
    let host = if defview != 0 {
        let parent = unsafe { GetAncestor(defview, GA_PARENT) };
        if parent != 0 {
            parent
        } else {
            shell
        }
    } else {
        shell
    };
    let mut listview = find_descendant(defview, "SysListView32");
    if listview == 0 {
        listview = find_descendant(host, "SysListView32");
    }
    DesktopShell {
        host,
        defview,
        listview,
    }
}

/// True when the desktop icon layer is hidden. `None` when Explorer cannot be
/// observed and the registry says nothing either.
pub fn desktop_icons_hidden() -> Option<bool> {
    let shell = find_desktop_shell();
    if shell.listview != 0 {
        return Some(unsafe { IsWindowVisible(shell.listview) } == 0);
    }
    reg_read_u32(EXPLORER_ADVANCED_KEY, "HideIcons").map(|v| v != 0)
}

fn write_hide_icons(hidden: bool) -> Result<(), String> {
    reg_write_u32(EXPLORER_ADVANCED_KEY, "HideIcons", if hidden { 1 } else { 0 })
        .map_err(|e| format!("写入 HideIcons 失败：{e}"))
}

fn notify_shell_icons_changed() {
    unsafe {
        SHChangeNotify(
            0x0800_0000, // SHCNE_ASSOCCHANGED
            0x1000,      // SHCNF_FLUSH
            core::ptr::null(),
            core::ptr::null(),
        );
        SendNotifyMessageW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, 0);
    }
}

/// Hide or show the whole desktop icon layer. Returns `true` when the visual
/// state changed. Uses Explorer's own toggle first (keeps its menu, registry
/// and view in sync) and only falls back to `ShowWindow` when the command
/// could not be posted at all.
pub fn set_desktop_icons_hidden(hidden: bool) -> Result<bool, String> {
    let shell = find_desktop_shell();
    if shell.listview == 0 {
        let currently_hidden = reg_read_u32(EXPLORER_ADVANCED_KEY, "HideIcons")
            .map(|v| v != 0)
            .unwrap_or(false);
        if currently_hidden == hidden {
            return Ok(false);
        }
        write_hide_icons(hidden)?;
        notify_shell_icons_changed();
        return Ok(true);
    }

    let visible = unsafe { IsWindowVisible(shell.listview) } != 0;
    if visible == !hidden {
        // State is already what the caller wants; still keep the registry in
        // step so Explorer's context menu agrees.
        write_hide_icons(hidden)?;
        return Ok(false);
    }

    let mut posted = false;
    for target in [shell.defview, shell.host] {
        if target == 0 {
            continue;
        }
        if unsafe { PostMessageW(target, WM_COMMAND, TOGGLE_DESKTOP_ICONS, 0) } != 0 {
            posted = true;
            break;
        }
    }
    if posted {
        // A queued command is not safe to retry: it may still run. Wait for
        // the real visibility, then write the registry only after it landed.
        for _ in 0..80 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            if (unsafe { IsWindowVisible(shell.listview) } != 0) == !hidden {
                write_hide_icons(hidden)?;
                return Ok(true);
            }
        }
        return Err("资源管理器没有完成桌面图标切换".to_string());
    }

    // The desktop command is unavailable (some Explorer replacements); hide
    // the list view directly and wake the shell.
    unsafe {
        ShowWindow(shell.listview, if hidden { SW_HIDE } else { SW_SHOW });
    }
    write_hide_icons(hidden)?;
    notify_shell_icons_changed();
    if (unsafe { IsWindowVisible(shell.listview) } != 0) == !hidden {
        Ok(true)
    } else {
        Err("无法切换桌面图标层".to_string())
    }
}

/// `%LOCALAPPDATA%\dcx\<name>` — shared runtime state between the host, the
/// settings window and the box process.
pub fn dcx_state_file(name: &str) -> std::path::PathBuf {
    let mut p = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push("dcx");
    p.push(name);
    p
}

/// Whether the desktop icon layer is hidden **because this app hid it**. The
/// marker matters: if the user had already hidden desktop icons, disabling the
/// box must not reveal them.
pub fn desktop_icons_owned() -> bool {
    std::fs::read_to_string(dcx_state_file("box_icons_owned"))
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

pub fn set_desktop_icons_owned(owned: bool) {
    let p = dcx_state_file("box_icons_owned");
    if owned {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&p, b"1");
    } else {
        let _ = std::fs::remove_file(&p);
    }
}

/// Deletes a value under `HKCU\<path>`. A value that is already gone counts as
/// success, so toggling twice never surfaces a phantom error.
pub fn reg_delete_value(path: &str, name: &str) -> Result<(), i32> {
    unsafe {
        let sub = wide(path);
        let mut key: isize = 0;
        let rc = RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_SET_VALUE, &mut key);
        if rc != 0 {
            return Err(rc);
        }
        let name_w = wide(name);
        let rc = RegDeleteValueW(key, name_w.as_ptr());
        RegCloseKey(key);
        if rc == 0 || rc == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(rc)
        }
    }
}

/// UTF-16 conversion with a trailing NUL, for the `*W` APIs.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Same, straight from an OS string.
///
/// `to_string_lossy()` replaces unpaired surrogates with U+FFFD, so a path that
/// is legal for NTFS but not valid UTF-8 would make the API act on a *different*
/// path than the one the user selected.
pub fn wide_path(p: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(core::iter::once(0)).collect()
}

pub fn rect_w(rc: &RECT) -> i32 {
    rc.width()
}
pub fn rect_h(rc: &RECT) -> i32 {
    rc.height()
}

/// `MM-DD HH:MM` local time from unix seconds. Empty when the API refuses
/// (which only happens for absurd inputs).
///
/// FILETIME counts 100 ns ticks from 1601-01-01 and the unix epoch sits
/// 11 644 473 600 s later; everything past that is two conversions so the
/// result follows the machine's time zone and DST instead of guessing an offset.
pub fn local_time_from_unix(secs: u64) -> String {
    let ticks = (secs.saturating_add(11_644_473_600)).saturating_mul(10_000_000);
    let utc = FILETIME {
        low: (ticks & 0xFFFF_FFFF) as u32,
        high: (ticks >> 32) as u32,
    };
    unsafe {
        let mut local = FILETIME::default();
        if FileTimeToLocalFileTime(&utc, &mut local) == FALSE {
            return String::new();
        }
        let mut st = SYSTEMTIME::default();
        if FileTimeToSystemTime(&local, &mut st) == FALSE {
            return String::new();
        }
        format!(
            "{:02}-{:02} {:02}:{:02}",
            st.month, st.day, st.hour, st.minute
        )
    }
}

/// `PrintWindow` with full content, into a caller-provided DC.
pub unsafe fn print_window(hwnd: HWND, dc: HDC) -> bool {
    // PrintWindow is in user32 but shares the signature we declared there.
    print_window_raw(hwnd, dc, PW_RENDERFULLCONTENT) != FALSE
}

#[link(name = "user32")]
extern "system" {
    #[link_name = "PrintWindow"]
    fn print_window_raw(hwnd: HWND, dc: HDC, flags: UINT) -> BOOL;
}

pub unsafe fn cloak(hwnd: HWND, on: bool) -> bool {
    let v: BOOL = if on { TRUE } else { FALSE };
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_CLOAK,
        &v as *const BOOL as *const c_void,
        core::mem::size_of::<BOOL>() as DWORD,
    ) == 0
}

pub unsafe fn disable_transitions(hwnd: HWND) {
    let v: BOOL = TRUE;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_TRANSITIONS_FORCEDISABLED,
        &v as *const BOOL as *const c_void,
        core::mem::size_of::<BOOL>() as DWORD,
    );
}

pub unsafe fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

/// `GWL_STYLE` for `hwnd`, zero when the handle is gone.
pub unsafe fn style(hwnd: HWND) -> DWORD {
    if IsWindow(hwnd) == FALSE {
        return 0;
    }
    GetWindowLongPtrW(hwnd, GWL_STYLE) as DWORD
}

/// `GWL_EXSTYLE` for `hwnd`, zero when the handle is gone.
pub unsafe fn ex_style(hwnd: HWND) -> DWORD {
    if IsWindow(hwnd) == FALSE {
        return 0;
    }
    GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as DWORD
}

/// DWM-hidden window (other virtual desktop, or a shell surface that is not on
/// screen). It is `IsWindowVisible`, yet nothing of it is drawn.
pub unsafe fn is_cloaked(hwnd: HWND) -> bool {
    let mut v: BOOL = FALSE;
    DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAK,
        &mut v as *mut BOOL as *mut c_void,
        core::mem::size_of::<BOOL>() as DWORD,
    ) == 0
        && v != FALSE
}

pub unsafe fn corner_preference(hwnd: HWND) -> Option<i32> {
    let mut v: i32 = DWMWCP_DEFAULT;
    let hr = DwmGetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &mut v as *mut i32 as *mut c_void,
        core::mem::size_of::<i32>() as DWORD,
    );
    (hr == 0).then_some(v)
}

pub unsafe fn set_corner_preference(hwnd: HWND, value: i32) -> bool {
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &value as *const i32 as *const c_void,
        core::mem::size_of::<i32>() as DWORD,
    ) == 0
}

/// `DWMWA_SYSTEMBACKDROP_TYPE` (Windows 11 22H2+).
pub const DWMWA_SYSTEMBACKDROP_TYPE: DWORD = 38;
/// `DWMSBT_*` — what DWM paints behind a transparent client area.
pub const DWMSBT_NONE: i32 = 1;
pub const DWMSBT_TRANSIENTWINDOW: i32 = 3;

/// Ask DWM for a system backdrop (acrylic). False on systems that predate the
/// attribute — the caller keeps its opaque background in that case.
pub unsafe fn set_system_backdrop(hwnd: HWND, kind: i32) -> bool {
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_SYSTEMBACKDROP_TYPE,
        &kind as *const i32 as *const c_void,
        core::mem::size_of::<i32>() as DWORD,
    ) == 0
}

pub unsafe fn set_border_color(hwnd: HWND, bgr: u32) -> bool {
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_BORDER_COLOR,
        &bgr as *const u32 as *const c_void,
        core::mem::size_of::<u32>() as DWORD,
    ) == 0
}

#[cfg(test)]
#[cfg(target_pointer_width = "64")]
mod tests {
    use super::APPBARDATA;
    use core::ptr::addr_of;

    /// The shell matches `cbSize` against its own `APPBARDATA`, and reads the
    /// `ABM_SETSTATE` mask at offset 40. Both break silently if the `RECT` is
    /// declared with pointer-sized members.
    #[test]
    fn appbar_data_matches_the_shell_layout() {
        let d = APPBARDATA::default();
        let base = &d as *const APPBARDATA as usize;
        assert_eq!(core::mem::size_of::<APPBARDATA>(), 48);
        assert_eq!(addr_of!(d.cbSize) as usize - base, 0);
        assert_eq!(addr_of!(d.lParam) as usize - base, 40);
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    /// 一次性暂存键，跑完就删，不碰用户的真实配置。
    const SCRATCH: &str = r"Software\DCX-Selftest";

    #[test]
    fn string_write_read_delete_roundtrip() {
        // 干净起点：键里那条值即使上次跑测试残留也先清掉。
        let _ = reg_delete_value(SCRATCH, "v");
        assert!(reg_read_string(SCRATCH, "v").is_none());

        // 启动项里真实会出现的那种命令行：带空格、带引号、带参数。
        let cmd = "\"C:\\Program Files\\DCX\\dcx-host.exe\" --no-settings";
        reg_write_string(SCRATCH, "v", cmd).expect("write");
        assert_eq!(reg_read_string(SCRATCH, "v").as_deref(), Some(cmd));

        // 非 ASCII 要走 UTF-16，别被 UTF-8 假设坑到。
        reg_write_string(SCRATCH, "v", "开机自启 ✓").expect("overwrite");
        assert_eq!(reg_read_string(SCRATCH, "v").as_deref(), Some("开机自启 ✓"));

        reg_delete_value(SCRATCH, "v").expect("delete");
        assert!(reg_read_string(SCRATCH, "v").is_none());
        // 删两次不该报错（ui 上连点两下是常态）。
        reg_delete_value(SCRATCH, "v").expect("delete twice");

        // 收尾：把暂存键也移除，不在注册表里留垃圾。
        unsafe {
            let sub = wide(SCRATCH);
            RegDeleteKeyW(HKEY_CURRENT_USER, sub.as_ptr());
        }
    }
}
