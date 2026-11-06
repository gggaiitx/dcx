//! Raw vtable access for the three XAML diagnostics interfaces we depend on.
//!
//! Layouts and slot order are taken from `xamlom.h` (Windhawk 1.6.1 compiler
//! headers), not from memory. Slot numbers below are absolute, counting
//! IUnknown's three methods:
//!
//! ```text
//! IXamlDiagnostics        3 GetDispatcher  4 GetUiLayer  5 GetApplication
//!                         6 GetIInspectableFromHandle  7 GetHandleFromIInspectable
//!                         8 HitTest  9 RegisterInstance  10 GetInitializationData
//! IVisualTreeService      3 AdviseVisualTreeChange  4 UnadviseVisualTreeChange
//! IVisualTreeServiceCallback   3 OnVisualTreeChange
//! IVisualTreeServiceCallback2  4 OnElementStateChanged
//! ```

#![allow(non_snake_case)]

use crate::com::*;
use core::ffi::c_void;

/// `MIDL_uhyper` in xamlom.h.
pub type InstanceHandle = u64;

/// Mirrors `SourceInfo`. BSTR(8) + 3*u32 + pad(4) + BSTR(8) = 32 bytes.
#[repr(C)]
pub struct SourceInfo {
    pub FileName: BSTR,
    pub LineNumber: u32,
    pub ColumnNumber: u32,
    pub CharPosition: u32,
    pub Hash: BSTR,
}

/// Mirrors `VisualElement`: 8 + 32 + 8 + 8 + 4 + pad(4) = 64 bytes.
#[repr(C)]
pub struct VisualElement {
    pub Handle: InstanceHandle,
    pub SrcInfo: SourceInfo,
    pub Type: BSTR,
    pub Name: BSTR,
    pub NumChildren: u32,
}

#[repr(C)]
pub struct ParentChildRelation {
    pub Parent: InstanceHandle,
    pub Child: InstanceHandle,
    pub ChildIndex: u32,
}

pub const VISUAL_MUTATION_ADD: i32 = 0;

// --- abstract COM pointer ---------------------------------------------------

/// A borrowed interface pointer. Never released by this type; callers manage
/// lifetime explicitly (see [`release`]).
#[derive(Clone, Copy)]
pub struct ComPtr(pub *mut c_void);

// The pointer is only ever used behind the engine's own locks; COM objects
// created here are apartment-agnostic as far as we are concerned.
unsafe impl Send for ComPtr {}

impl ComPtr {
    pub const NULL: ComPtr = ComPtr(core::ptr::null_mut());

    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }

    /// Read vtable slot `index` as a function pointer.
    ///
    /// # Safety
    /// `self` must point at a live COM object whose vtable has at least
    /// `index + 1` entries.
    pub unsafe fn slot(&self, index: usize) -> *const c_void {
        let vtbl = *(self.0 as *const *const *const c_void);
        *vtbl.add(index)
    }

    pub unsafe fn query_interface(&self, iid: &Guid) -> Option<ComPtr> {
        type Fn = unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT;
        let f: Fn = core::mem::transmute(self.slot(0));
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = f(self.0, iid as *const Guid, &mut out);
        if hr == S_OK && !out.is_null() {
            Some(ComPtr(out))
        } else {
            None
        }
    }

    pub unsafe fn add_ref(&self) -> ULONG {
        type Fn = unsafe extern "system" fn(*mut c_void) -> ULONG;
        let f: Fn = core::mem::transmute(self.slot(1));
        f(self.0)
    }

    pub unsafe fn release(&self) -> ULONG {
        type Fn = unsafe extern "system" fn(*mut c_void) -> ULONG;
        let f: Fn = core::mem::transmute(self.slot(2));
        f(self.0)
    }

    /// `IXamlDiagnostics::GetIInspectableFromHandle` (slot 6).
    pub unsafe fn get_inspectable(&self, handle: InstanceHandle) -> Option<ComPtr> {
        type Fn =
            unsafe extern "system" fn(*mut c_void, InstanceHandle, *mut *mut c_void) -> HRESULT;
        let f: Fn = core::mem::transmute(self.slot(6));
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = f(self.0, handle, &mut out);
        if hr == S_OK && !out.is_null() {
            Some(ComPtr(out))
        } else {
            None
        }
    }

    /// `IVisualTreeService::AdviseVisualTreeChange` (slot 3).
    pub unsafe fn advise_visual_tree_change(&self, callback: *mut c_void) -> HRESULT {
        type Fn = unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT;
        let f: Fn = core::mem::transmute(self.slot(3));
        f(self.0, callback)
    }

    /// `IVisualTreeService::UnadviseVisualTreeChange` (slot 4).
    pub unsafe fn unadvise_visual_tree_change(&self, callback: *mut c_void) -> HRESULT {
        type Fn = unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT;
        let f: Fn = core::mem::transmute(self.slot(4));
        f(self.0, callback)
    }
}

/// The callback object we hand to `AdviseVisualTreeChange`.
///
/// vtable: IUnknown(3) + OnVisualTreeChange(1) + OnElementStateChanged(1)
#[repr(C)]
pub struct VisualTreeCallbackVtbl {
    pub QueryInterface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
    pub AddRef: unsafe extern "system" fn(*mut c_void) -> ULONG,
    pub Release: unsafe extern "system" fn(*mut c_void) -> ULONG,
    pub OnVisualTreeChange:
        unsafe extern "system" fn(*mut c_void, ParentChildRelation, VisualElement, i32) -> HRESULT,
    pub OnElementStateChanged:
        unsafe extern "system" fn(*mut c_void, InstanceHandle, i32, *const u16) -> HRESULT,
}

#[repr(C)]
pub struct VisualTreeCallback {
    pub vtbl: *const VisualTreeCallbackVtbl,
    pub refs: u32,
}

impl VisualTreeCallback {
    pub fn new() -> Box<Self> {
        Box::new(Self {
            vtbl: &VTBL,
            refs: 1,
        })
    }

    /// Consume a boxed callback into a raw pointer for `AdviseVisualTreeChange`.
    pub fn into_raw(b: Box<Self>) -> *mut c_void {
        Box::into_raw(b) as *mut c_void
    }
}

static VTBL: VisualTreeCallbackVtbl = VisualTreeCallbackVtbl {
    QueryInterface: cb_qi,
    AddRef: cb_addref,
    Release: cb_release,
    OnVisualTreeChange: cb_on_change,
    OnElementStateChanged: cb_on_state,
};

unsafe extern "system" fn cb_qi(
    this: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    let want = &*iid;
    if want.is(&IID_IUNKNOWN) || want.is(&IID_IVISUALTREESERVICECALLBACK2) {
        *out = this;
        cb_addref(this);
        return S_OK;
    }
    *out = core::ptr::null_mut();
    E_NOINTERFACE
}

unsafe extern "system" fn cb_addref(this: *mut c_void) -> ULONG {
    let obj = &mut *(this as *mut VisualTreeCallback);
    obj.refs += 1;
    obj.refs
}

unsafe extern "system" fn cb_release(this: *mut c_void) -> ULONG {
    let obj = &mut *(this as *mut VisualTreeCallback);
    obj.refs -= 1;
    if obj.refs == 0 {
        drop(Box::from_raw(this as *mut VisualTreeCallback));
        0
    } else {
        obj.refs
    }
}

unsafe extern "system" fn cb_on_change(
    _this: *mut c_void,
    _relation: ParentChildRelation,
    element: VisualElement,
    mutation: i32,
) -> HRESULT {
    if mutation != VISUAL_MUTATION_ADD {
        return S_OK;
    }
    let ty = bstr_to_string(element.Type);
    let name = bstr_to_string(element.Name);
    // Record + match + apply, the way `VisualTreeWatcher::OnVisualTreeChange`
    // does: the callback is where the reference styles each new element.
    crate::record_element(_relation.Parent, element.Handle, &ty, &name);
    S_OK
}

unsafe extern "system" fn cb_on_state(
    _this: *mut c_void,
    _handle: InstanceHandle,
    _state: i32,
    _value: *const u16,
) -> HRESULT {
    S_OK
}
