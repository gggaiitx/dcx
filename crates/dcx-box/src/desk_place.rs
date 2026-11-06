//! Put a released item back on the desktop *where it was dropped*.
//!
//! Un-hiding a file is not enough: the shell drops the new icon into the first
//! free grid slot, which looks nothing like "I dragged it there". Explorer
//! positions desktop icons through `IFolderView::SelectAndPositionItems`, and
//! that is the only API that works here — the desktop list view is
//! `LVS_OWNERDATA`, so `LVM_SETITEMPOSITION` returns success and is silently
//! ignored (verified on 26200).
//!
//! Getting the view means walking `IShellWindows` → `IServiceProvider` →
//! `IShellBrowser` → `IShellView` → `IFolderView`. Every one of those is a raw
//! vtable call: the project has no COM bindings and pulling in a whole crate for
//! five interfaces would cost more than this file.
//!
//! The whole release chain — clear the hidden attribute, flush the shell
//! notification, find the item, position it — lives on one resident STA worker
//! thread ([`Placer`]). The UI thread only mutates the manifest. Two things make
//! the drop feel instant:
//!
//! * `warm()` is sent when a drag *starts*, so the COM walk to `IFolderView`
//!   happens while the user is still moving the mouse. By the time the button
//!   comes up the only work left is the attribute flip and the position call.
//!   The old code walked the whole chain on release, after a synchronous
//!   `SHCNF_FLUSH` that blocked the UI thread — the visible stall.
//! * The item only shows up in the view a moment *after* the hidden attribute is
//!   cleared, so `position` polls briefly. `SHCNF_FLUSH` in this thread returns
//!   once the shell has re-enumerated, so the first poll normally hits.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use dcx_win32 as w;

// ---------------------------------------------------------------------------
// Minimal COM plumbing
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid {
    d1: u32,
    d2: u16,
    d3: u16,
    d4: [u8; 8],
}

impl Guid {
    const fn new(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Self {
        Guid { d1, d2, d3, d4 }
    }
}

const CLSID_SHELL_WINDOWS: Guid = Guid::new(
    0x9BA0_5972,
    0xF6A8,
    0x11CF,
    [0xA4, 0x42, 0x00, 0xA0, 0xC9, 0x0A, 0x8F, 0x39],
);
const IID_ISHELL_WINDOWS: Guid = Guid::new(
    0x85CB_6900,
    0x4D95,
    0x11CF,
    [0x96, 0x0C, 0x00, 0x80, 0xC7, 0xF4, 0xEE, 0x85],
);
const IID_ISERVICE_PROVIDER: Guid = Guid::new(
    0x6D51_40C1,
    0x7436,
    0x11CE,
    [0x80, 0x34, 0x00, 0xAA, 0x00, 0x60, 0x09, 0xFA],
);
const SID_STOP_LEVEL_BROWSER: Guid = Guid::new(
    0x4C96_BE40,
    0x915C,
    0x11CF,
    [0x99, 0xD3, 0x00, 0xAA, 0x00, 0x4A, 0xE8, 0x37],
);
const IID_ISHELL_BROWSER: Guid = Guid::new(
    0x0002_14E2,
    0x0000,
    0x0000,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);
const IID_IFOLDER_VIEW: Guid = Guid::new(
    0xCDE7_25B0,
    0xCCC9,
    0x4519,
    [0x91, 0x7E, 0x32, 0x5D, 0x72, 0xFA, 0xB4, 0xCE],
);

/// `SWC_DESKTOP` — the shell window class of the desktop.
const SWC_DESKTOP: i32 = 0x08;
/// `SWFO_NEEDDISPATCH` — hand back an `IDispatch`, which is what can be QI'd
/// for `IServiceProvider`.
const SWFO_NEED_DISPATCH: i32 = 0x01;
const CLSCTX_ALL: u32 = 0x17;
/// `COINIT_APARTMENTTHREADED`: the shell hands out its proxies most reliably to
/// an STA, and this thread does nothing else.
const COINIT_APARTMENTTHREADED: u32 = 0x02;

/// `SVGIO_ALLVIEW` — files plus namespace icons.
const SVGIO_ALLVIEW: u32 = 0x02;
/// `SVSI_POSITIONITEM` — "set this item's position", the flag Explorer itself
/// uses when a drag ends on the desktop.
const SVSI_POSITIONITEM: u32 = 0x0008_0000;
/// Full path, e.g. `D:\Users\…\Desktop\a.docx`; namespace icons come back as
/// `::{CLSID}` instead, which simply never matches a file path.
const SIGDN_DESKTOPABSOLUTEPARSING: i32 = 0x8002_8000u32 as i32;

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
    fn CoUninitialize();
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: *mut c_void,
        clsctx: u32,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> i32;
    fn CoTaskMemFree(p: *mut c_void);
}

#[link(name = "shell32")]
extern "system" {
    fn SHGetNameFromIDList(pidl: *const c_void, sigdn: i32, name: *mut *mut u16) -> i32;
}

/// Address of one vtable entry.
unsafe fn method(p: *mut c_void, index: usize) -> *const c_void {
    let vtable = *(p as *const *const *const c_void);
    *vtable.add(index)
}

unsafe fn query_interface(p: *mut c_void, iid: &Guid) -> Option<*mut c_void> {
    type Fn = unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32;
    let f: Fn = std::mem::transmute(method(p, 0));
    let mut out: *mut c_void = std::ptr::null_mut();
    if f(p, iid, &mut out) >= 0 && !out.is_null() {
        Some(out)
    } else {
        None
    }
}

unsafe fn release(p: *mut c_void) {
    if p.is_null() {
        return;
    }
    type Fn = unsafe extern "system" fn(*mut c_void) -> u32;
    let f: Fn = std::mem::transmute(method(p, 2));
    f(p);
}

// ---------------------------------------------------------------------------
// The walk to the desktop's folder view
// ---------------------------------------------------------------------------

/// Live `IFolderView` of the desktop, or `None` when the shell is not answering
/// (no Explorer, session locking, …). Returns `(IShellView, IFolderView)`.
unsafe fn desktop_view() -> Option<(*mut c_void, *mut c_void)> {
    // IShellWindows is an IDispatch, so FindWindowSW sits at 3 + 7 + 8.
    const FIND_WINDOW_SW: usize = 15;
    const QUERY_SERVICE: usize = 3;
    const QUERY_ACTIVE_SHELL_VIEW: usize = 15;

    let mut windows: *mut c_void = std::ptr::null_mut();
    if CoCreateInstance(
        &CLSID_SHELL_WINDOWS,
        std::ptr::null_mut(),
        CLSCTX_ALL,
        &IID_ISHELL_WINDOWS,
        &mut windows,
    ) < 0
    {
        return None;
    }

    // `VARIANT` is 24 bytes on x64; two empty ones mean "the desktop itself".
    let mut loc = [0u8; 24];
    let mut root = [0u8; 24];
    let mut hwnd: i32 = 0;
    let mut dispatch: *mut c_void = std::ptr::null_mut();
    type FindWindowSw = unsafe extern "system" fn(
        *mut c_void,
        *mut u8,
        *mut u8,
        i32,
        *mut i32,
        i32,
        *mut *mut c_void,
    ) -> i32;
    let find: FindWindowSw = std::mem::transmute(method(windows, FIND_WINDOW_SW));
    let hr = find(
        windows,
        loc.as_mut_ptr(),
        root.as_mut_ptr(),
        SWC_DESKTOP,
        &mut hwnd,
        SWFO_NEED_DISPATCH,
        &mut dispatch,
    );
    let result = (|| {
        if hr < 0 || dispatch.is_null() {
            return None;
        }
        let provider = query_interface(dispatch, &IID_ISERVICE_PROVIDER)?;
        type QueryService = unsafe extern "system" fn(
            *mut c_void,
            *const Guid,
            *const Guid,
            *mut *mut c_void,
        ) -> i32;
        let qs: QueryService = std::mem::transmute(method(provider, QUERY_SERVICE));
        let mut browser: *mut c_void = std::ptr::null_mut();
        let hr = qs(
            provider,
            &SID_STOP_LEVEL_BROWSER,
            &IID_ISHELL_BROWSER,
            &mut browser,
        );
        release(provider);
        if hr < 0 || browser.is_null() {
            return None;
        }
        type QueryView = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32;
        let qv: QueryView = std::mem::transmute(method(browser, QUERY_ACTIVE_SHELL_VIEW));
        let mut view: *mut c_void = std::ptr::null_mut();
        let hr = qv(browser, &mut view);
        release(browser);
        if hr < 0 || view.is_null() {
            return None;
        }
        let folder_view = match query_interface(view, &IID_IFOLDER_VIEW) {
            Some(fv) => fv,
            None => {
                release(view);
                return None;
            }
        };
        Some((view, folder_view))
    })();
    release(dispatch);
    release(windows);
    result
}

/// Item's full path, as the view spells it, or `None` for namespace icons.
unsafe fn item_path(pidl: *const c_void) -> Option<String> {
    let mut raw: *mut u16 = std::ptr::null_mut();
    if SHGetNameFromIDList(pidl, SIGDN_DESKTOPABSOLUTEPARSING, &mut raw) < 0 || raw.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *raw.add(len) != 0 {
        len += 1;
    }
    let name = String::from_utf16_lossy(std::slice::from_raw_parts(raw, len));
    CoTaskMemFree(raw as *mut c_void);
    Some(name)
}

/// Find the view's child pidl for `path`. The pidl belongs to the view and must
/// not be freed.
unsafe fn find_item(folder_view: *mut c_void, path: &Path) -> Option<*mut c_void> {
    const ITEM_COUNT: usize = 7;
    const ITEM: usize = 6;

    let want = path.to_string_lossy().to_lowercase();
    type Count = unsafe extern "system" fn(*mut c_void, u32, *mut i32) -> i32;
    let count: Count = std::mem::transmute(method(folder_view, ITEM_COUNT));
    let mut n = 0i32;
    if count(folder_view, SVGIO_ALLVIEW, &mut n) < 0 {
        return None;
    }
    type Item = unsafe extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> i32;
    let item: Item = std::mem::transmute(method(folder_view, ITEM));
    for i in 0..n {
        let mut pidl: *mut c_void = std::ptr::null_mut();
        if item(folder_view, i, &mut pidl) < 0 || pidl.is_null() {
            continue;
        }
        if let Some(found) = item_path(pidl) {
            if found.to_lowercase() == want {
                return Some(pidl);
            }
        }
    }
    None
}

/// Icon spacing, used to centre the icon block on the pointer instead of
/// hanging it off the bottom-right like the shell's own drop does.
unsafe fn icon_spacing(folder_view: *mut c_void) -> (i32, i32) {
    const GET_SPACING: usize = 12;
    type Spacing = unsafe extern "system" fn(*mut c_void, *mut Point) -> i32;
    let f: Spacing = std::mem::transmute(method(folder_view, GET_SPACING));
    let mut pt = Point { x: 0, y: 0 };
    if f(folder_view, &mut pt) >= 0 && pt.x > 0 && pt.y > 0 {
        (pt.x, pt.y)
    } else {
        (100, 100)
    }
}

/// The desktop view's top-left corner on screen; icon coordinates are relative
/// to it, and on a multi-monitor desktop it is not always (0, 0).
unsafe fn view_origin(view: *mut c_void) -> (i32, i32) {
    const GET_WINDOW: usize = 3;
    type GetWindow = unsafe extern "system" fn(*mut c_void, *mut isize) -> i32;
    let gw: GetWindow = std::mem::transmute(method(view, GET_WINDOW));
    let mut hwnd: isize = 0;
    if gw(view, &mut hwnd) < 0 || hwnd == 0 {
        return (0, 0);
    }
    let mut rc = w::RECT::default();
    if w::GetWindowRect(hwnd, &mut rc) == 0 {
        return (0, 0);
    }
    (rc.left, rc.top)
}

// ---------------------------------------------------------------------------
// Resident placement worker
// ---------------------------------------------------------------------------

enum Cmd {
    /// A drag started: pre-build the COM chain to `IFolderView` so the release
    /// does not have to pay for it. Cheap no-op when the view is already live.
    Warm,
    /// The button came up: un-hide `path`, tell the shell to re-read the
    /// folder, and drop the icon at `cursor` (physical screen px, `None` =
    /// just un-hide, wherever the shell wants it).
    Release {
        path: PathBuf,
        cursor: Option<(i32, i32)>,
    },
}

/// The desktop-placement worker.
///
/// One thread, one STA, one channel. Cheap to hold: the thread parks on the
/// channel between commands, and the `IFolderView` proxies it keeps warm are
/// released when the last `Placer` clone is dropped (the panel owns the only
/// one, for the process's lifetime).
pub struct Placer {
    tx: Sender<Cmd>,
}

impl Placer {
    pub fn spawn() -> Placer {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("dcx-desk-place".to_string())
            .spawn(move || {
                unsafe {
                    if CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) < 0 {
                        return;
                    }
                }
                let mut view: Option<(*mut c_void, *mut c_void)> = None;
                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        Cmd::Warm => {
                            if view.is_none() {
                                view = unsafe { desktop_view() };
                            }
                        }
                        Cmd::Release { path, cursor } => unsafe {
                            release_item(&mut view, &path, cursor);
                        },
                    }
                }
                if let Some((v, fv)) = view {
                    unsafe {
                        release(fv);
                        release(v);
                    }
                }
                unsafe { CoUninitialize() };
            })
            .expect("dcx-desk-place thread");
        Placer { tx }
    }

    /// Pre-walk the COM chain to `IFolderView` (call when a drag starts).
    pub fn warm(&self) {
        let _ = self.tx.send(Cmd::Warm);
    }

    /// Put `path` back on the desktop, centred on `cursor` when given.
    pub fn release(&self, path: &Path, cursor: Option<(i32, i32)>) {
        let _ = self.tx.send(Cmd::Release {
            path: path.to_path_buf(),
            cursor,
        });
    }
}

/// One release, on the worker thread.
///
/// The cached view can go stale behind our back — Explorer restarts, the shell
/// window is rebuilt — and a dead proxy answers with `hr < 0` instead of
/// crashing, so any failure drops the cache and retries once with a fresh walk.
unsafe fn release_item(
    view: &mut Option<(*mut c_void, *mut c_void)>,
    path: &Path,
    cursor: Option<(i32, i32)>,
) {
    // 1. Make the file visible again. Pure filesystem work, no notification
    //    yet — the one UPDATEDIR below is what makes Explorer pick it up.
    set_hidden_attr(path, false);
    // 2. Synchronous UPDATEDIR on the desktop: returns once the shell has
    //    re-enumerated, so the item is normally in the view on the first poll.
    //    FLUSH used to run on the UI thread (the release stall); here it only
    //    delays this worker.
    notify_flush(path);

    let Some(cursor) = cursor else {
        return;
    };
    if view.is_none() {
        *view = desktop_view();
    }
    if let Some((v, fv)) = *view {
        if position(v, fv, path, cursor) {
            return;
        }
        // Either the view is a dead proxy from a previous shell, or the item
        // never showed up. A fresh walk covers the first case; the second one
        // just fails again and is left to the periodic refresh to reconcile.
        release(fv);
        release(v);
    }
    *view = None;
    if let Some((v, fv)) = desktop_view() {
        position(v, fv, path, cursor);
        *view = Some((v, fv));
    }
}

/// `IFolderView::SelectAndPositionItems` for `path` at `cursor`.
unsafe fn position(
    view: *mut c_void,
    folder_view: *mut c_void,
    path: &Path,
    cursor: (i32, i32),
) -> bool {
    const SELECT_AND_POSITION: usize = 16;

    // The notification was flushed, so the item is normally already there; the
    // poll only rides out a busy shell. Short and tightening — the old fixed
    // 3.6 s worst case is what "nothing happens" looked like.
    let mut pidl = None;
    for attempt in 0..10 {
        pidl = find_item(folder_view, path);
        if pidl.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(if attempt < 4 { 10 } else { 25 }));
    }
    let Some(pidl) = pidl else {
        return false;
    };

    let (ox, oy) = view_origin(view);
    let (cx, cy) = icon_spacing(folder_view);
    let mut pt = Point {
        x: (cursor.0 - ox - cx / 2).max(0),
        y: (cursor.1 - oy - cy / 2).max(0),
    };
    type SelectAndPosition =
        unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void, *mut Point, u32) -> i32;
    let place: SelectAndPosition = std::mem::transmute(method(folder_view, SELECT_AND_POSITION));
    let mut list = [pidl];
    place(
        folder_view,
        1,
        list.as_mut_ptr(),
        &mut pt as *mut Point,
        SVSI_POSITIONITEM,
    ) >= 0
}

/// Toggle `FILE_ATTRIBUTE_HIDDEN` on `path` (worker-side twin of the
/// organizer's `set_hidden`; returns true when the attribute actually changed).
unsafe fn set_hidden_attr(path: &Path, hidden: bool) -> bool {
    let wide = w::wide(&path.to_string_lossy());
    let attrs = w::GetFileAttributesW(wide.as_ptr());
    if attrs == w::INVALID_FILE_ATTRIBUTES {
        return false;
    }
    let has = attrs & w::FILE_ATTRIBUTE_HIDDEN != 0;
    if has == hidden {
        return false;
    }
    let next = if hidden {
        attrs | w::FILE_ATTRIBUTE_HIDDEN
    } else {
        attrs & !w::FILE_ATTRIBUTE_HIDDEN
    };
    w::SetFileAttributesW(wide.as_ptr(), next) != 0
}

/// Synchronous `SHCNE_UPDATEDIR` on `path`'s folder — the desktop — so the
/// shell re-reads it before we go looking for the new icon.
unsafe fn notify_flush(path: &Path) {
    let Some(dir) = path.parent() else {
        return;
    };
    let wide = w::wide(&dir.to_string_lossy());
    w::SHChangeNotify(
        w::SHCNE_UPDATEDIR,
        w::SHCNF_PATHW | w::SHCNF_FLUSH,
        wide.as_ptr() as *const core::ffi::c_void,
        core::ptr::null(),
    );
}
