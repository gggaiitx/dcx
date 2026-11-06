//! The Test Automation Provider object XAML loads into explorer.
//!
//! Flow (verified against the installed styler at
//! `windows-11-taskbar-styler.wh.cpp:11393`):
//!
//! 1. we call `InitializeXamlDiagnosticsEx("VisualDiagConnectionN", pid, "",
//!    <our dll path>, CLSID_DCXTAP, null)`
//! 2. XAML `LoadLibrary`s our DLL and calls `DllGetClassObject(CLSID_DCXTAP, ...)`
//! 3. XAML creates our object and calls `SetSite(<IXamlDiagnostics>)`
//! 4. from that site we `QueryInterface(IVisualTreeService3)` and
//!    `AdviseVisualTreeChange`, which is the point where elements start
//!    arriving in [`crate::xaml::cb_on_change`].

#![allow(non_snake_case)]

use crate::com::*;
use crate::xaml::{ComPtr, VisualTreeCallback};
use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, Ordering};

/// The `IXamlDiagnostics*` site. Atomic, not a Mutex: `AdviseVisualTreeChange`
/// invokes our callback synchronously, and the callback needs to read this.
/// A non-reentrant lock here deadlocks the XAML UI thread immediately.
pub static SITE: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
pub static CALLBACK: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
pub static CONNECTED: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

// ---------------------------------------------------------------------------
// TAP object
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct TapVtbl {
    pub QueryInterface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
    pub AddRef: unsafe extern "system" fn(*mut c_void) -> ULONG,
    pub Release: unsafe extern "system" fn(*mut c_void) -> ULONG,
    pub SetSite: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT,
    pub GetSite: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
}

#[repr(C)]
pub struct TapObj {
    pub vtbl: *const TapVtbl,
    pub refs: u32,
    pub site: *mut c_void,
}

static TAP_VTBL: TapVtbl = TapVtbl {
    QueryInterface: tap_qi,
    AddRef: tap_addref,
    Release: tap_release,
    SetSite: tap_set_site,
    GetSite: tap_get_site,
};

fn new_tap() -> *mut c_void {
    let obj = Box::new(TapObj {
        vtbl: &TAP_VTBL,
        refs: 1,
        site: core::ptr::null_mut(),
    });
    Box::into_raw(obj) as *mut c_void
}

unsafe extern "system" fn tap_qi(
    this: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    let want = &*iid;
    if want.is(&IID_IUNKNOWN) || want.is(&IID_IOBJECTWITHSITE) {
        *out = this;
        tap_addref(this);
        S_OK
    } else {
        *out = core::ptr::null_mut();
        E_NOINTERFACE
    }
}

unsafe extern "system" fn tap_addref(this: *mut c_void) -> ULONG {
    let obj = &mut *(this as *mut TapObj);
    obj.refs += 1;
    obj.refs
}

unsafe extern "system" fn tap_release(this: *mut c_void) -> ULONG {
    let obj = &mut *(this as *mut TapObj);
    obj.refs -= 1;
    if obj.refs == 0 {
        drop(Box::from_raw(this as *mut TapObj));
        0
    } else {
        obj.refs
    }
}

unsafe extern "system" fn tap_set_site(this: *mut c_void, site: *mut c_void) -> HRESULT {
    let obj = &mut *(this as *mut TapObj);

    // Detach any previous connection before swapping the pointer.
    let prev = SITE.swap(site, Ordering::SeqCst);
    if !prev.is_null() {
        let cb = CALLBACK.load(Ordering::SeqCst);
        if !cb.is_null() {
            if let Some(vts) = ComPtr(prev).query_interface(&IID_IVISUALTREESERVICE3) {
                vts.unadvise_visual_tree_change(cb);
            }
        }
    }

    obj.site = site;
    CONNECTED.store(site, Ordering::SeqCst);

    if site.is_null() {
        crate::log("TAP: site cleared");
        return S_OK;
    }

    let site_ptr = ComPtr(site);
    if let Some(vts) = site_ptr.query_interface(&IID_IVISUALTREESERVICE3) {
        let cb_box = VisualTreeCallback::new();
        let cb_raw = VisualTreeCallback::into_raw(cb_box);
        CALLBACK.store(cb_raw, Ordering::SeqCst);
        let hr = vts.advise_visual_tree_change(cb_raw);
        crate::log(&format!(
            "TAP: sited, IVisualTreeService3 ok, AdviseVisualTreeChange hr=0x{:08X}",
            hr as u32
        ));
        vts.release();
    } else {
        crate::log("TAP: site has no IVisualTreeService3");
    }

    S_OK
}

unsafe extern "system" fn tap_get_site(
    this: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HRESULT {
    let obj = &*(this as *mut TapObj);
    if out.is_null() {
        return E_POINTER;
    }
    if obj.site.is_null() {
        *out = core::ptr::null_mut();
        return E_FAIL;
    }
    match ComPtr(obj.site).query_interface(&*riid) {
        Some(p) => {
            *out = p.0;
            S_OK
        }
        None => {
            *out = core::ptr::null_mut();
            E_NOINTERFACE
        }
    }
}

// ---------------------------------------------------------------------------
// Class factory
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct FactoryVtbl {
    pub QueryInterface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
    pub AddRef: unsafe extern "system" fn(*mut c_void) -> ULONG,
    pub Release: unsafe extern "system" fn(*mut c_void) -> ULONG,
    pub CreateInstance: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const Guid,
        *mut *mut c_void,
    ) -> HRESULT,
    pub LockServer: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
}

#[repr(C)]
pub struct FactoryObj {
    pub vtbl: *const FactoryVtbl,
    pub refs: u32,
}

static FACTORY_VTBL: FactoryVtbl = FactoryVtbl {
    QueryInterface: fac_qi,
    AddRef: fac_addref,
    Release: fac_release,
    CreateInstance: fac_create,
    LockServer: fac_lock,
};

static mut FACTORY: FactoryObj = FactoryObj {
    vtbl: &FACTORY_VTBL,
    refs: 1,
};

unsafe extern "system" fn fac_qi(
    this: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    let want = &*iid;
    if want.is(&IID_IUNKNOWN) || want.is(&IID_ICLASSFACTORY) {
        *out = this;
        S_OK
    } else {
        *out = core::ptr::null_mut();
        E_NOINTERFACE
    }
}

unsafe extern "system" fn fac_addref(this: *mut c_void) -> ULONG {
    let obj = &mut *(this as *mut FactoryObj);
    obj.refs += 1;
    obj.refs
}

unsafe extern "system" fn fac_release(this: *mut c_void) -> ULONG {
    // Static instance: never freed.
    let obj = &mut *(this as *mut FactoryObj);
    if obj.refs > 1 {
        obj.refs -= 1;
    }
    obj.refs
}

unsafe extern "system" fn fac_create(
    _this: *mut c_void,
    outer: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HRESULT {
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    if out.is_null() {
        return E_POINTER;
    }
    let obj = new_tap();
    let hr = tap_qi(obj, iid, out);
    if hr != S_OK {
        tap_release(obj);
    } else {
        // Drop the creation reference; the caller holds one now.
        tap_release(obj);
    }
    hr
}

unsafe extern "system" fn fac_lock(_this: *mut c_void, _lock: i32) -> HRESULT {
    S_OK
}

// ---------------------------------------------------------------------------
// DllGetClassObject — the export XAML calls after LoadLibrary
// ---------------------------------------------------------------------------

/// # Safety
/// Called by XAML. `out` must be a valid out-pointer.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const Guid,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HRESULT {
    crate::log("DllGetClassObject called");
    if rclsid.is_null() || riid.is_null() || out.is_null() {
        return E_POINTER;
    }
    if !(*rclsid).is(&CLSID_DCXTAP) {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory = core::ptr::addr_of_mut!(FACTORY) as *mut c_void;
    fac_qi(factory, riid, out)
}
