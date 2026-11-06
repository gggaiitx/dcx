//! COM / WinRT ABI plumbing: GUIDs, HRESULTs, HSTRINGs, activation factories,
//! a raw vtable-slot `ComPtr`, and the SEH guard.
//!
//! No `windows`/`windows-sys` dependency: the injected DLL has to stay small and
//! the WinRT bindings in those crates do not cover `Windows.UI.Xaml` at all.
//!
//! IIDs/slots live in [`crate::ids`]; this module only knows IUnknown and
//! IInspectable, whose layouts are fixed by the WinRT ABI spec.

#![allow(non_snake_case)]

use core::ffi::c_void;
use core::mem;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

// ---------------------------------------------------------------------------
// Scalars
// ---------------------------------------------------------------------------

pub type HRESULT = i32;
pub type ULONG = u32;
/// Opaque `HSTRING` handle. A null value is the empty string.
///
/// `HSTRING` points at the first UTF-16 code unit; the length lives in a header
/// in front of it, which is why it is always manipulated through the combase
/// entry points and never as a plain pointer.
pub type HSTRING = *mut c_void;

pub const S_OK: HRESULT = 0;
pub const S_FALSE: HRESULT = 1;
pub const E_INVALIDARG: HRESULT = 0x8007_0057u32 as i32;
pub const E_NOINTERFACE: HRESULT = 0x8000_4002u32 as i32;
pub const E_POINTER: HRESULT = 0x8000_4003u32 as i32;
pub const E_FAIL: HRESULT = 0x8000_4005u32 as i32;
pub const E_OUTOFMEMORY: HRESULT = 0x8007_000Eu32 as i32;
pub const CLASS_E_NOAGGREGATION: HRESULT = 0x8004_0110u32 as i32;
pub const CLASS_E_CLASSNOTAVAILABLE: HRESULT = 0x8004_0111u32 as i32;
pub const RO_E_CLOSED: HRESULT = 0x8000_000Bu32 as i32;
/// `RPC_E_WRONG_THREAD` — the call reached XAML from a thread that does not own
/// the tree.
pub const RPC_E_WRONG_THREAD: HRESULT = 0x8001_0110u32 as i32;

pub fn succeeded(hr: HRESULT) -> bool {
    hr >= 0
}

pub fn failed(hr: HRESULT) -> bool {
    hr < 0
}

/// `HRESULT` rendered as `0x........`, for diagnostics.
pub fn hr_hex(hr: HRESULT) -> String {
    format!("0x{:08X}", hr as u32)
}

// ---------------------------------------------------------------------------
// GUID
// ---------------------------------------------------------------------------

/// Mirrors the Windows `GUID`/`IID`/`CLSID` struct (16 bytes, passed by
/// reference into `QueryInterface`/`RoGetActivationFactory`).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    pub const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self {
            data1,
            data2,
            data3,
            data4,
        }
    }

    /// Byte-wise comparison (the ABI compares the 16 struct bytes, not fields).
    pub fn is(&self, other: &Guid) -> bool {
        self.data4 == other.data4
            && self.data1 == other.data1
            && self.data2 == other.data2
            && self.data3 == other.data3
    }

    /// `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}`, the form C++/WinRT prints.
    pub fn to_braced(&self) -> String {
        format!(
            "{{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
            self.data1,
            self.data2,
            self.data3,
            self.data4[0],
            self.data4[1],
            self.data4[2],
            self.data4[3],
            self.data4[4],
            self.data4[5],
            self.data4[6],
            self.data4[7]
        )
    }
}

impl core::fmt::Debug for Guid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.to_braced())
    }
}

impl core::fmt::Display for Guid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.to_braced())
    }
}

pub const IID_IUNKNOWN: Guid = Guid::new(0x0000_0000, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const IID_IINSPECTABLE: Guid = Guid::new(
    0xAF86_E2E0,
    0xB12D,
    0x4C6A,
    [0x9C, 0x5A, 0xD7, 0xAA, 0x65, 0x10, 0x1E, 0x90],
);
/// `Windows.Foundation.IActivationFactory` (from C++/WinRT `base.h`, line 1994).
pub const IID_IACTIVATIONFACTORY: Guid = Guid::new(
    0x0000_0035,
    0x0000,
    0x0000,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);

// ---------------------------------------------------------------------------
// Absolute vtable slots every WinRT object has
// ---------------------------------------------------------------------------

pub const SLOT_QUERY_INTERFACE: usize = 0;
pub const SLOT_ADD_REF: usize = 1;
pub const SLOT_RELEASE: usize = 2;
/// `IInspectable::GetIids(ULONG* count, IID** iids)` — caller frees `iids` with
/// `CoTaskMemFree`.
pub const SLOT_GET_IIDS: usize = 3;
/// `IInspectable::GetRuntimeClassName(HSTRING* name)` — caller frees `name` with
/// `WindowsDeleteString`.
pub const SLOT_GET_RUNTIME_CLASS_NAME: usize = 4;
/// `IInspectable::GetTrustLevel(TrustLevel* level)`.
pub const SLOT_GET_TRUST_LEVEL: usize = 5;

/// `IActivationFactory::ActivateInstance(IInspectable** instance)`.
pub const SLOT_ACTIVATE_INSTANCE: usize = 6;

// ---------------------------------------------------------------------------
// kernel32: combase is resolved at runtime
// ---------------------------------------------------------------------------

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

#[link(name = "ole32")]
extern "system" {
    fn CoTaskMemFree(p: *mut c_void);
}

type CreateStringFn = unsafe extern "system" fn(*const u16, u32, *mut HSTRING) -> HRESULT;
type DeleteStringFn = unsafe extern "system" fn(HSTRING) -> HRESULT;
type GetStringRawBufferFn = unsafe extern "system" fn(HSTRING, *mut u32) -> *const u16;
type GetActivationFactoryFn =
    unsafe extern "system" fn(HSTRING, *const Guid, *mut *mut c_void) -> HRESULT;

/// combase.dll entry points, resolved once per process.
pub struct Combase {
    pub create_string: CreateStringFn,
    pub delete_string: DeleteStringFn,
    pub get_string_raw_buffer: GetStringRawBufferFn,
    pub get_activation_factory: GetActivationFactoryFn,
}

static COMBASE: AtomicPtr<Combase> = AtomicPtr::new(ptr::null_mut());

fn wide_nul(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Resolve combase.dll once. Returns `None` if anything is missing (the engine
/// then has no way to talk to XAML and reports that instead of crashing).
pub fn combase() -> Option<&'static Combase> {
    let cached = COMBASE.load(Ordering::Acquire);
    if !cached.is_null() {
        return Some(unsafe { &*cached });
    }

    unsafe {
        let module = LoadLibraryW(wide_nul("combase.dll").as_ptr());
        if module.is_null() {
            return None;
        }

        let create = GetProcAddress(module, b"WindowsCreateString\0".as_ptr());
        let delete = GetProcAddress(module, b"WindowsDeleteString\0".as_ptr());
        let raw = GetProcAddress(module, b"WindowsGetStringRawBuffer\0".as_ptr());
        let factory = GetProcAddress(module, b"RoGetActivationFactory\0".as_ptr());
        if create.is_null() || delete.is_null() || raw.is_null() || factory.is_null() {
            return None;
        }

        let boxed = Box::new(Combase {
            create_string: mem::transmute(create),
            delete_string: mem::transmute(delete),
            get_string_raw_buffer: mem::transmute(raw),
            get_activation_factory: mem::transmute(factory),
        });
        let ptr = Box::into_raw(boxed);
        // Racing threads may both build the table; the loser leaks one small
        // allocation rather than risking a use-after-free in an injected DLL.
        match COMBASE.compare_exchange(ptr::null_mut(), ptr, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Some(&*ptr),
            Err(winner) => {
                drop(Box::from_raw(ptr));
                Some(&*winner)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// HSTRING
// ---------------------------------------------------------------------------

/// Owned `HSTRING`. Dropping it calls `WindowsDeleteString`.
pub struct HString(HSTRING);

impl HString {
    /// `WindowsCreateString`. The string is copied into the HSTRING, so the
    /// temporary UTF-16 buffer can go away immediately.
    pub fn new(s: &str) -> Option<Self> {
        let api = combase()?;
        let wide: Vec<u16> = s.encode_utf16().collect();
        // A null `sourceString` is rejected by some combase versions even for
        // length 0; hand it a NUL-terminated empty string instead.
        static EMPTY: [u16; 1] = [0];
        let src = if wide.is_empty() {
            EMPTY.as_ptr()
        } else {
            wide.as_ptr()
        };
        let mut out: HSTRING = ptr::null_mut();
        let hr = unsafe { (api.create_string)(src, wide.len() as u32, &mut out) };
        if failed(hr) {
            return None;
        }
        // WinRT represents the empty string as a null HSTRING handle (that is
        // exactly what C++/WinRT's `hstring` does); it is legal to pass to APIs
        // and `WindowsDeleteString(null)` is a no-op.
        Some(HString(out))
    }

    pub fn as_raw(&self) -> HSTRING {
        self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_null()
    }

    /// Length of the string in UTF-16 code units, as the HSTRING header reports
    /// it (`WindowsGetStringRawBuffer`'s out parameter).
    pub fn len(&self) -> usize {
        if self.0.is_null() {
            return 0;
        }
        let Some(api) = combase() else {
            return 0;
        };
        // SAFETY: `self.0` is a live HSTRING owned by this value.
        guarded(|| unsafe {
            let mut len: u32 = 0;
            (api.get_string_raw_buffer)(self.0, &mut len);
            len as usize
        })
        .unwrap_or(0)
    }

    pub fn to_string_lossy(&self) -> String {
        // SAFETY: `self.0` is either null or a live HSTRING this value owns.
        unsafe { raw_hstring_to_string(self.0, false) }
    }
}

impl Drop for HString {
    fn drop(&mut self) {
        if let (Some(api), false) = (combase(), self.0.is_null()) {
            unsafe {
                (api.delete_string)(self.0);
            }
        }
        self.0 = ptr::null_mut();
    }
}

/// Copy the characters of a borrowed `HSTRING` into a `String`.
///
/// `take_ownership` also calls `WindowsDeleteString` (used for `HSTRING`s an API
/// handed back to us through an out parameter).
///
/// # Safety
/// `h` must be null or a valid `HSTRING`. Every read is SEH-guarded so a bogus
/// handle yields `""` instead of faulting, but ownership of a non-null handle is
/// transferred by `take_ownership`, which the caller must only set for handles it
/// owns.
pub unsafe fn raw_hstring_to_string(h: HSTRING, take_ownership: bool) -> String {
    if h.is_null() {
        return String::new();
    }
    let Some(api) = combase() else {
        return String::new();
    };

    // Reading an HSTRING is a plain memory read, but the handle comes from XAML;
    // guard it so a bogus pointer is reported instead of taken down the process.
    let text = guarded(|| {
        let mut len: u32 = 0;
        let buf = (api.get_string_raw_buffer)(h, &mut len);
        if buf.is_null() || len == 0 {
            String::new()
        } else {
            String::from_utf16_lossy(core::slice::from_raw_parts(buf, len as usize))
        }
    })
    .unwrap_or_default();

    if take_ownership {
        delete_hstring(h);
    }
    text
}

/// Delete an `HSTRING` we own (e.g. one returned by `GetRuntimeClassName`).
pub unsafe fn delete_hstring(h: HSTRING) {
    if let (Some(api), false) = (combase(), h.is_null()) {
        (api.delete_string)(h);
    }
}

/// `RoGetActivationFactory(className, iid)`. Returns the requested interface of
/// the activation factory / statics object.
pub unsafe fn ro_get_activation_factory(class_name: &str, iid: &Guid) -> Option<ComPtr> {
    let api = combase()?;
    let name = HString::new(class_name)?;
    let mut out: *mut c_void = ptr::null_mut();
    let hr = (api.get_activation_factory)(name.as_raw(), iid as *const Guid, &mut out);
    if failed(hr) || out.is_null() {
        None
    } else {
        Some(ComPtr(out))
    }
}

/// Create an instance of an activatable runtime class through its
/// `IActivationFactory` (`ActivateInstance`, absolute slot 6).
pub unsafe fn activate_instance(class_name: &str) -> Option<ComPtr> {
    let factory = ro_get_activation_factory(class_name, &IID_IACTIVATIONFACTORY)?;
    type ActivateFn = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT;
    let f: ActivateFn = mem::transmute(factory.slot(SLOT_ACTIVATE_INSTANCE));
    let mut out: *mut c_void = ptr::null_mut();
    let hr = f(factory.0, &mut out);
    factory.release();
    if failed(hr) || out.is_null() {
        None
    } else {
        Some(ComPtr(out))
    }
}

// ---------------------------------------------------------------------------
// SEH guard
// ---------------------------------------------------------------------------

#[link(name = "xamlguard")]
extern "C" {
    /// `int dcx_xaml_guard(void (*fn)(void*), void* arg)` — returns 0 when the
    /// body ran to completion, otherwise the SEH exception code.
    fn dcx_xaml_guard(f: unsafe extern "C" fn(*mut c_void), arg: *mut c_void) -> i32;
}

struct GuardCtx<'a> {
    f: &'a mut dyn FnMut(),
}

unsafe extern "C" fn guard_trampoline(p: *mut c_void) {
    let ctx = &mut *(p as *mut GuardCtx);
    (ctx.f)();
}

/// Run `f` inside a `__try/__except` frame.
///
/// Returns `Err(exception_code)` when a structured exception was raised (e.g.
/// `0xC000_0005` access violation) and `Ok(value)` otherwise. `f` must not
/// panic: a Rust unwind cannot cross the C frame.
pub fn guarded<R>(f: impl FnOnce() -> R) -> Result<R, u32> {
    let mut out: Option<R> = None;
    let mut f = Some(f);
    {
        let out_ref = &mut out;
        let mut body = || {
            if let Some(f) = f.take() {
                *out_ref = Some(f());
            }
        };
        let mut ctx = GuardCtx { f: &mut body };
        let code =
            unsafe { dcx_xaml_guard(guard_trampoline, &mut ctx as *mut GuardCtx as *mut c_void) };
        if code != 0 {
            return Err(code as u32);
        }
    }
    match out {
        Some(v) => Ok(v),
        // The body never ran and no exception was raised: cannot happen, but
        // return a distinguishable code instead of panicking.
        None => Err(0xE000_0000),
    }
}

// ---------------------------------------------------------------------------
// ComPtr
// ---------------------------------------------------------------------------

/// A raw interface pointer with the vtable operations every COM object supports.
///
/// The wrapper is `Copy` and does **not** manage the reference count: this
/// mirrors the engine's own `ComPtr` and keeps slot-level code readable. Release
/// what you own explicitly with [`ComPtr::release`].
#[derive(Clone, Copy)]
pub struct ComPtr(pub *mut c_void);

// COM objects handed to us live on the XAML UI thread; the engine only ever
// passes them between its own locks.
unsafe impl Send for ComPtr {}

impl ComPtr {
    pub const NULL: ComPtr = ComPtr(ptr::null_mut());

    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }

    pub fn as_ptr(&self) -> *mut c_void {
        self.0
    }

    /// # Safety
    /// `p` must be a valid interface pointer whose reference is being donated to
    /// the returned wrapper (it will not be released by the wrapper).
    pub unsafe fn from_raw(p: *mut c_void) -> Self {
        ComPtr(p)
    }

    /// Read vtable slot `index` (absolute, counting IUnknown's three methods).
    ///
    /// # Safety
    /// `self` must point at a live COM object whose vtable has at least
    /// `index + 1` entries.
    pub unsafe fn slot(&self, index: usize) -> *const c_void {
        let vtbl = *(self.0 as *const *const *const c_void);
        *vtbl.add(index)
    }

    /// `IUnknown::QueryInterface`.
    pub unsafe fn query_interface(&self, iid: &Guid) -> Option<ComPtr> {
        match self.query_interface_ex(iid) {
            Ok((hr, ptr)) if hr == S_OK && !ptr.is_null() => Some(ptr),
            _ => None,
        }
    }

    /// `QueryInterface` with the `HRESULT` and any caught fault preserved, for
    /// diagnostics.
    pub unsafe fn query_interface_ex(&self, iid: &Guid) -> Result<(HRESULT, ComPtr), u32> {
        if self.0.is_null() {
            return Ok((E_POINTER, ComPtr::NULL));
        }
        type Fn = unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT;
        guarded(|| {
            let f: Fn = mem::transmute(self.slot(SLOT_QUERY_INTERFACE));
            let mut out: *mut c_void = ptr::null_mut();
            let hr = f(self.0, iid as *const Guid, &mut out);
            (hr, ComPtr(out))
        })
    }

    pub unsafe fn add_ref(&self) -> ULONG {
        if self.0.is_null() {
            return 0;
        }
        type Fn = unsafe extern "system" fn(*mut c_void) -> ULONG;
        let f: Fn = mem::transmute(self.slot(SLOT_ADD_REF));
        f(self.0)
    }

    /// `IUnknown::Release`. Safe to call on null (no-op).
    pub unsafe fn release(&self) -> ULONG {
        if self.0.is_null() {
            return 0;
        }
        type Fn = unsafe extern "system" fn(*mut c_void) -> ULONG;
        let f: Fn = mem::transmute(self.slot(SLOT_RELEASE));
        f(self.0)
    }

    /// `IInspectable::GetRuntimeClassName` — `""` on failure.
    ///
    /// The single most useful diagnostic when a slot or an IID is wrong: it
    /// names the class of whatever object you actually got.
    pub unsafe fn runtime_class_name(&self) -> String {
        if self.0.is_null() {
            return String::new();
        }
        let read = guarded(|| {
            type Fn = unsafe extern "system" fn(*mut c_void, *mut HSTRING) -> HRESULT;
            let f: Fn = mem::transmute(self.slot(SLOT_GET_RUNTIME_CLASS_NAME));
            let mut name: HSTRING = ptr::null_mut();
            let hr = f(self.0, &mut name);
            (hr, name)
        });
        match read {
            Ok((hr, name)) if hr == S_OK && !name.is_null() => {
                // GetRuntimeClassName hands ownership of the HSTRING to us.
                raw_hstring_to_string(name, true)
            }
            _ => String::new(),
        }
    }

    /// `IInspectable::GetIids`: every interface IID the object reports.
    ///
    /// Used by the probe to *prove* which IID a parameterized interface must
    /// have (and that our computed IIDs are among them) instead of guessing.
    pub unsafe fn get_iids(&self) -> Vec<Guid> {
        if self.0.is_null() {
            return Vec::new();
        }
        let read = guarded(|| {
            type Fn = unsafe extern "system" fn(*mut c_void, *mut u32, *mut *mut Guid) -> HRESULT;
            let f: Fn = mem::transmute(self.slot(SLOT_GET_IIDS));
            let mut count: u32 = 0;
            let mut iids: *mut Guid = ptr::null_mut();
            let hr = f(self.0, &mut count, &mut iids);
            (hr, count, iids)
        });
        match read {
            Ok((hr, count, iids)) if hr == S_OK && !iids.is_null() => {
                let mut out = Vec::with_capacity(count as usize);
                for i in 0..count as usize {
                    out.push(*iids.add(i));
                }
                CoTaskMemFree(iids as *mut c_void);
                out
            }
            _ => Vec::new(),
        }
    }
}

impl core::fmt::Debug for ComPtr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "ComPtr({:p})", self.0)
    }
}

// ---------------------------------------------------------------------------
// Tests: pure ABI, no XAML, no explorer
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_is_16_bytes() {
        assert_eq!(mem::size_of::<Guid>(), 16);
    }

    #[test]
    fn guid_braced_form() {
        // IID_IXamlReaderStatics from docs/winrt-abi.md.
        let g = Guid::new(
            0x9891_C6BD,
            0x534F,
            0x4955,
            [0xB8, 0x5A, 0x8A, 0x8D, 0xC0, 0xDC, 0xA6, 0x02],
        );
        assert_eq!(g.to_braced(), "{9891c6bd-534f-4955-b85a-8a8dc0dca602}");
        assert!(IID_IUNKNOWN.is(&IID_IUNKNOWN));
        assert!(!IID_IUNKNOWN.is(&IID_IINSPECTABLE));
    }

    #[test]
    fn hstring_roundtrip() {
        let s = HString::new("Windows.UI.Xaml.Style \u{2713}").expect("WindowsCreateString");
        assert_eq!(s.to_string_lossy(), "Windows.UI.Xaml.Style \u{2713}");
        assert_eq!(s.len(), "Windows.UI.Xaml.Style \u{2713}".chars().count());
        drop(s);

        let empty = HString::new("").expect("empty HSTRING");
        assert_eq!(empty.to_string_lossy(), "");
        assert!(empty.is_empty());
    }

    #[test]
    fn combase_table_resolves() {
        // The runtime-resolved combase entry points are what every XAML call
        // sits on. `RoGetActivationFactory` itself is only exercised inside the
        // host (Windows Runtime must be initialised, which a test process is
        // not), so this only proves the table resolves.
        let api = combase().expect("combase.dll entry points");
        let mut out: HSTRING = ptr::null_mut();
        let wide: Vec<u16> = "combase".encode_utf16().collect();
        let hr = unsafe { (api.create_string)(wide.as_ptr(), wide.len() as u32, &mut out) };
        assert_eq!(hr, S_OK);
        assert!(!out.is_null());
        unsafe { delete_hstring(out) };
    }

    #[test]
    fn guard_catches_access_violation() {
        let bad = 1usize as *const u8;
        let r = guarded(|| unsafe { ptr::read_volatile(bad) });
        match r {
            Err(code) => assert_eq!(code, 0xC000_0005, "expected AV code, got 0x{code:08X}"),
            Ok(v) => panic!("guard did not catch the fault, read {v}"),
        }
    }

    #[test]
    fn guard_passes_through_value() {
        assert_eq!(guarded(|| 7u32), Ok(7));
        assert_eq!(guarded(|| "x".to_string()), Ok("x".to_string()));
    }
}
