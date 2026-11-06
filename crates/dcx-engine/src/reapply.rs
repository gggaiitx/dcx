//! Re-assert our value when explorer writes over it.
//!
//! Port of two pieces of the reference:
//! * `ApplyCustomizationsForVisualStateGroup` 17608-17638 — after `SetValue`,
//!   `RegisterPropertyChangedCallback(property, …)`, whose body re-applies
//!   `propertyCustomizationState.customValue`;
//! * `g_elementPropertyModifying` (12166) — a flag set around every write *we*
//!   make, so our own `SetValue` does not re-enter the callback.
//!
//! Without this the theme silently decays. Measured on the tray clock: the
//! theme's `TextBlock#DateInnerTextBlock Visibility=Collapsed` reads back as
//! Collapsed in the pass that writes it and as Visible in the next one — the
//! clock control re-shows its own date part, and a write-once rule never puts it
//! back.
//!
//! # Lifetime
//!
//! The delegate is created with one reference, handed to
//! `RegisterPropertyChangedCallback` (which adds its own), then released, so XAML
//! holds the only reference and frees it with the element. The property and the
//! value are add-ref'd because the callback needs them after `apply_rules` has
//! dropped its own.

#![allow(non_snake_case)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use dcx_xaml::com::{guarded, ComPtr, Guid, E_NOINTERFACE, E_POINTER, HRESULT, IID_IUNKNOWN, S_OK};
use dcx_xaml::ids::IID_DEPENDENCY_PROPERTY_CHANGED_CALLBACK;
use dcx_xaml::xaml;

/// `g_elementPropertyModifying`: set while a write originates from us.
static MODIFYING: AtomicBool = AtomicBool::new(false);

/// Set for the duration of a write we initiate, so the callback it provokes does
/// not re-apply. Nesting is restored, like the reference's `wasModifying`.
pub struct Suppress(Option<bool>);

impl Suppress {
    pub fn begin() -> Self {
        Suppress(Some(MODIFYING.swap(true, Ordering::SeqCst)))
    }
}

impl Drop for Suppress {
    fn drop(&mut self) {
        if let Some(previous) = self.0.take() {
            MODIFYING.store(previous, Ordering::SeqCst);
        }
    }
}

/// `DependencyPropertyChangedCallback`'s ABI object. WinRT delegates derive from
/// `IUnknown`, so `Invoke` is vtable slot 3.
#[repr(C)]
struct Vtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    invoke: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> HRESULT,
}

static VTBL: Vtbl = Vtbl {
    query_interface: cb_qi,
    add_ref: cb_add_ref,
    release: cb_release,
    invoke: cb_invoke,
};

/// One registration: what to write back.
#[repr(C)]
struct Callback {
    vtbl: *const Vtbl,
    refs: AtomicU32,
    /// `DependencyProperty*` this callback watches.
    dp: *mut c_void,
    /// `IInspectable*` holding the value the theme asked for.
    value: *mut c_void,
}

unsafe extern "system" fn cb_qi(
    this: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    *out = core::ptr::null_mut();
    let want = &*iid;
    if want.is(&IID_DEPENDENCY_PROPERTY_CHANGED_CALLBACK) {
        cb_add_ref(this);
        *out = this;
        return S_OK;
    }
    // This delegate derives from `IUnknown` only (see
    // `IID_DEPENDENCY_PROPERTY_CHANGED_CALLBACK`), so it has no IInspectable
    // methods to answer.
    if want.is(&IID_IUNKNOWN) {
        cb_add_ref(this);
        *out = this;
        return S_OK;
    }
    E_NOINTERFACE
}

unsafe extern "system" fn cb_add_ref(this: *mut c_void) -> u32 {
    let cb = this as *mut Callback;
    (*cb).refs.fetch_add(1, Ordering::SeqCst) + 1
}

unsafe extern "system" fn cb_release(this: *mut c_void) -> u32 {
    let cb = this as *mut Callback;
    let remaining = (*cb).refs.fetch_sub(1, Ordering::SeqCst) - 1;
    if remaining == 0 {
        ComPtr((*cb).dp).release();
        ComPtr((*cb).value).release();
        drop(Box::from_raw(cb));
    }
    remaining
}

/// `Invoke(DependencyObject sender, DependencyProperty args)`.
///
/// `sender` is the element that changed, which is why no element reference is
/// kept here and why a stale registration cannot dangle.
unsafe extern "system" fn cb_invoke(
    this: *mut c_void,
    sender: *mut c_void,
    args: *mut c_void,
) -> HRESULT {
    let _ = args;
    let cb = this as *mut Callback;
    // The callback is registered for one DependencyProperty, so a notification
    // on this delegate already identifies the property. The reference's
    // callback reacts to every invocation and does not compare the `args`
    // interface identity; doing so here would skip a valid notification if
    // XAML hands over an equivalent but different `DependencyProperty` wrapper.
    if !sender.is_null() && !MODIFYING.load(Ordering::SeqCst) {
        let _guard = Suppress::begin();
        let _ = xaml::dependency_object_set_value(
            ComPtr(sender),
            ComPtr((*cb).dp),
            ComPtr((*cb).value),
        );
    }
    S_OK
}

/// Watch `dp` on `element` and keep writing `value` back if anything else
/// changes it. Returns false when the registration failed.
///
/// # Safety
/// UI thread; `element`, `dp` and `value` must be live COM pointers.
pub unsafe fn watch(element: *mut c_void, dp: *mut c_void, value: *mut c_void) -> bool {
    let cb = Box::into_raw(Box::new(Callback {
        vtbl: &VTBL,
        refs: AtomicU32::new(1),
        dp,
        value,
    }));
    let cb_ptr = cb as *mut c_void;

    // The callback outlives the pass that registered it.
    ComPtr(dp).add_ref();
    ComPtr(value).add_ref();

    let registered = match guarded(|| {
        xaml::register_property_changed_callback(ComPtr(element), ComPtr(dp), ComPtr(cb_ptr))
    }) {
        Ok(token) => token.is_some(),
        Err(_) => false,
    };

    // XAML takes its own reference on success; drop the one we created with.
    //
    // On failure `refs` is still 1, so this release drops the callback to zero
    // and `cb_release` frees the box **and** releases `dp`/`value`. The old code
    // then released `dp` and `value` a second time, underflowing their COM
    // reference counts inside Explorer — objects freed while XAML still used
    // them. Which is exactly the path taken when
    // `RegisterPropertyChangedCallback` fails after a Windows update changes the
    // visual tree.
    ComPtr(cb_ptr).release();
    registered
}
