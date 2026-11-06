//! Hand-rolled WinRT/COM plumbing for `dcx-blur`.
//!
//! Everything here is transcribed from the ABI the reference mod compiles
//! against (C++/WinRT headers under
//! `D:\SoftWare\Windhawk\Compiler\include\winrt`) and the absolute vtable slots
//! in `dcx/docs/winrt-abi.md`. The slot convention is:
//!
//! * WinMD interfaces derive from `IInspectable`, so their vtable is
//!   `0 QueryInterface, 1 AddRef, 2 Release, 3 GetIids, 4 GetRuntimeClassName,
//!    5 GetTrustLevel, 6.. own methods`.
//! * `IGraphicsEffectD2D1Interop` is *not* a WinMD interface: it is declared as
//!   `DECLARE_INTERFACE_IID_(IGraphicsEffectD2D1Interop, IUnknown, ...)` in
//!   `windows.graphics.effects.interop.h` (Windows SDK 10.0.19041 winrt
//!   include), so its methods start at slot 3.
//!
//! The crate is self-contained on purpose: it may depend only on `dcx-win32`,
//! and the engine passes live objects in through the [`ComPtrLike`] wrapper.

#![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms)]

use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// HRESULT / GUID
// ---------------------------------------------------------------------------

pub type HRESULT = i32;
pub type ULONG = u32;

pub const S_OK: HRESULT = 0;
pub const S_FALSE: HRESULT = 1;
pub const E_NOTIMPL: HRESULT = 0x8000_4001u32 as i32;
pub const E_NOINTERFACE: HRESULT = 0x8000_4002u32 as i32;
pub const E_POINTER: HRESULT = 0x8000_4003u32 as i32;
pub const E_FAIL: HRESULT = 0x8000_4005u32 as i32;
pub const E_INVALIDARG: HRESULT = 0x8007_0057u32 as i32;
pub const E_OUTOFMEMORY: HRESULT = 0x8007_000Eu32 as i32;
pub const E_BOUNDS: HRESULT = 0x8000_000Bu32 as i32;

#[inline]
pub fn succeeded(hr: HRESULT) -> bool {
    hr >= 0
}

#[inline]
pub fn failed(hr: HRESULT) -> bool {
    hr < 0
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    pub const fn new(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Self {
        Self {
            data1: d1,
            data2: d2,
            data3: d3,
            data4: d4,
        }
    }
}

// Standard COM identities (not in the WinMD files).
pub const IID_IUNKNOWN: Guid = Guid::new(
    0x0000_0000,
    0,
    0,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);
pub const IID_IINSPECTABLE: Guid = Guid::new(
    0xAF86_E2E0,
    0xB12D,
    0x4C6A,
    [0x9C, 0x5A, 0xD7, 0xAA, 0x65, 0x10, 0x1E, 0x90],
);
pub const IID_IAGILEOBJECT: Guid = Guid::new(
    0x94EA_2B94,
    0xE9CC,
    0x49E0,
    [0xC0, 0xFF, 0xEE, 0x64, 0xCA, 0x8F, 0x5B, 0x90],
);
pub const IID_ICLOSABLE: Guid = Guid::new(
    0x30D5_A829,
    0x7FA4,
    0x4026,
    [0x83, 0xBB, 0xD7, 0x5B, 0xAE, 0x4E, 0xA9, 0x9E],
);

// --- Windows.UI.Xaml.Media --------------------------------------------------
// docs/winrt-abi.md, regenerated from C:\Windows\System32\WinMetadata.

pub const IID_ISOLIDCOLORBRUSH: Guid = Guid::new(
    0x9D85_0850,
    0x66F3,
    0x48DF,
    [0x9A, 0x8F, 0x82, 0x4B, 0xD5, 0xE0, 0x70, 0xAF],
);
pub const IID_IXAMLCOMPOSITIONBRUSHBAASEFACTORY: Guid = Guid::new(
    0x394F_0823,
    0x2451,
    0x4ED8,
    [0xBD, 0x24, 0x48, 0x81, 0x49, 0xB3, 0x42, 0x8D],
);
pub const IID_IXAMLCOMPOSITIONBRUSHBAASEOVERRIDES: Guid = Guid::new(
    0xD191_27F1,
    0x38B4,
    0x4EA1,
    [0x8F, 0x33, 0x84, 0x96, 0x29, 0xA4, 0xC9, 0xC1],
);
pub const IID_IXAMLCOMPOSITIONBRUSHBAASEPROTECTED: Guid = Guid::new(
    0x1513_F3D8,
    0x0457,
    0x4E1C,
    [0xAD, 0x77, 0x11, 0xC1, 0xD9, 0x87, 0x97, 0x43],
);
pub const IID_IBRUSHOVERRIDES2: Guid = Guid::new(
    0xD092_B151,
    0xD83B,
    0x5A81,
    [0xA7, 0x1E, 0xA1, 0xC7, 0xF8, 0xAD, 0x69, 0x63],
);

// --- Windows.UI.Xaml --------------------------------------------------------

pub const IID_IXAMLREADERSTATICS: Guid = Guid::new(
    0x9891_C6BD,
    0x534F,
    0x4955,
    [0xB8, 0x5A, 0x8A, 0x8D, 0xC0, 0xDC, 0xA6, 0x02],
);
pub const IID_IUIELEMENT: Guid = Guid::new(
    0x676D_0BE9,
    0xB65C,
    0x41C6,
    [0xBA, 0x40, 0x58, 0xCF, 0x87, 0xF2, 0x01, 0xC1],
);
pub const IID_IFRAMEWORKELEMENT: Guid = Guid::new(
    0xA391_D09B,
    0x4A99,
    0x4B7C,
    [0x9D, 0x8D, 0x6F, 0xA5, 0xD0, 0x1F, 0x6F, 0xBF],
);

// --- Windows.UI.Xaml.Hosting ------------------------------------------------

pub const IID_IELEMENTCOMPOSITIONPREVIEWSTATICS: Guid = Guid::new(
    0x08C9_2B38,
    0xEC99,
    0x4C55,
    [0xBC, 0x85, 0xA1, 0xC1, 0x80, 0xB2, 0x76, 0x46],
);

// --- Windows.UI.Composition -------------------------------------------------

pub const IID_ICOMPOSITIONOBJECT: Guid = Guid::new(
    0xBCB4_AD45,
    0x7609,
    0x4550,
    [0x93, 0x4F, 0x16, 0x00, 0x2A, 0x68, 0xFD, 0xED],
);
pub const IID_ICOMPOSITOR: Guid = Guid::new(
    0xB403_CA50,
    0x7F8C,
    0x4E83,
    [0x98, 0x5F, 0xCC, 0x45, 0x06, 0x00, 0x36, 0xD8],
);
pub const IID_ICOMPOSITOR2: Guid = Guid::new(
    0x7350_81DC,
    0x5E24,
    0x45DA,
    [0xA3, 0x8F, 0xE3, 0x2C, 0xC3, 0x49, 0xA9, 0xA0],
);
pub const IID_ICOMPOSITIONEFFECTFACTORY: Guid = Guid::new(
    0xBE56_24AF,
    0xBA7E,
    0x4510,
    [0x98, 0x50, 0x41, 0xC0, 0xB4, 0xFF, 0x74, 0xDF],
);
pub const IID_ICOMPOSITIONEFFECTBRUSH: Guid = Guid::new(
    0xBF7F_795E,
    0x83CC,
    0x44BF,
    [0xA4, 0x47, 0x3E, 0x3C, 0x07, 0x17, 0x89, 0xEC],
);
pub const IID_ICOMPOSITIONBRUSH: Guid = Guid::new(
    0xAB0D_7608,
    0x30C0,
    0x40E9,
    [0xB5, 0x68, 0xB6, 0x0A, 0x6B, 0xD1, 0xFB, 0x46],
);
pub const IID_ICOMPOSITIONCOLORBRUSH: Guid = Guid::new(
    0x2B26_4C5E,
    0xBF35,
    0x4831,
    [0x86, 0x42, 0xCF, 0x70, 0xC2, 0x0F, 0xFF, 0x2F],
);
pub const IID_ICOMPOSITIONEFFECTSOURCEPARAMETERFACTORY: Guid = Guid::new(
    0xB3D9_F276,
    0xABA3,
    0x4724,
    [0xAC, 0xF3, 0xD0, 0x39, 0x74, 0x64, 0xDB, 0x1C],
);
pub const IID_ICOMPOSITIONEFFECTSOURCEPARAMETER: Guid = Guid::new(
    0x858A_B13A,
    0x3292,
    0x4E4E,
    [0xB3, 0xBB, 0x2B, 0x6C, 0x65, 0x44, 0xA6, 0xEE],
);

// --- Windows.Foundation -----------------------------------------------------

pub const IID_IPROPERTYVALUESTATICS: Guid = Guid::new(
    0x629B_DBC8,
    0xD932,
    0x4FF4,
    [0x96, 0xB9, 0x8D, 0x96, 0xC5, 0xC1, 0xE8, 0x58],
);
pub const IID_IPROPERTYVALUE: Guid = Guid::new(
    0x4BD6_82DD,
    0x7554,
    0x40E9,
    [0x9A, 0x9B, 0x82, 0x65, 0x4E, 0xDE, 0x7E, 0x62],
);

// --- Windows.Graphics.Effects ----------------------------------------------
// Windows.Graphics.winmd (docs/winrt-abi.md does not list them; dumped with
// `metadump.exe Windows.Graphics.winmd --iface <name>`).

pub const IID_IGRAPHICSEFFECT: Guid = Guid::new(
    0xCB51_C0CE,
    0x8FE6,
    0x4636,
    [0xB2, 0x02, 0x86, 0x1F, 0xAA, 0x07, 0xD8, 0xF3],
);
pub const IID_IGRAPHICSEFFECTSOURCE: Guid = Guid::new(
    0x2D8F_9DDC,
    0x4339,
    0x4EB9,
    [0x92, 0x16, 0xF9, 0xDE, 0xB7, 0x56, 0x58, 0xA2],
);
pub const IID_IGRAPHICSEFFECTD2D1INTEROP: Guid = Guid::new(
    0x2FC5_7384,
    0xA068,
    0x44D7,
    [0xA3, 0x31, 0x30, 0x98, 0x2F, 0xCF, 0x71, 0x77],
);

// ---------------------------------------------------------------------------
// Absolute vtable slots (docs/winrt-abi.md)
// ---------------------------------------------------------------------------

/// `IXamlCompositionBrushBaseFactory::CreateInstance` (slot 6).
pub const SLOT_BRUSH_FACTORY_CREATE_INSTANCE: usize = 6;
/// `IXamlCompositionBrushBaseOverrides::OnConnected` (6), `OnDisconnected` (7).
pub const SLOT_BRUSH_ON_CONNECTED: usize = 6;
pub const SLOT_BRUSH_ON_DISCONNECTED: usize = 7;
/// `IXamlCompositionBrushBaseProtected::get_CompositionBrush` (6), `put_...` (7).
pub const SLOT_BRUSH_PROTECTED_GET: usize = 6;
pub const SLOT_BRUSH_PROTECTED_PUT: usize = 7;
/// `IBrushOverrides2::PopulatePropertyInfoOverride` (6).
pub const SLOT_BRUSH_POPULATE_PROPERTY_INFO: usize = 6;

/// `IXamlReaderStatics::Load` (6).
pub const SLOT_XAMLREADER_LOAD: usize = 6;
/// `ISolidColorBrush::get_Color` (6).
pub const SLOT_SOLIDCOLORBRUSH_GET_COLOR: usize = 6;
/// `IElementCompositionPreviewStatics::GetElementVisual` (6).
pub const SLOT_ELEMENT_COMPOSITION_PREVIEW_GET_ELEMENT_VISUAL: usize = 6;
/// `ICompositionObject::get_Compositor` (6).
pub const SLOT_COMPOSITION_OBJECT_GET_COMPOSITOR: usize = 6;

/// `ICompositor::CreateColorBrush(Color)` (8), `CreateEffectFactory` (11).
pub const SLOT_COMPOSITOR_CREATE_COLOR_BRUSH: usize = 8;
pub const SLOT_COMPOSITOR_CREATE_EFFECT_FACTORY: usize = 11;
/// `ICompositor2::CreateBackdropBrush` (8).
pub const SLOT_COMPOSITOR2_CREATE_BACKDROP_BRUSH: usize = 8;
/// `ICompositionEffectFactory::CreateBrush` (6), `get_LoadStatus` (8).
pub const SLOT_EFFECT_FACTORY_CREATE_BRUSH: usize = 6;
pub const SLOT_EFFECT_FACTORY_GET_LOAD_STATUS: usize = 8;
/// `ICompositionEffectBrush::SetSourceParameter` (7).
pub const SLOT_EFFECT_BRUSH_SET_SOURCE_PARAMETER: usize = 7;
/// `ICompositionEffectSourceParameterFactory::Create` (6).
pub const SLOT_SOURCE_PARAMETER_FACTORY_CREATE: usize = 6;
/// `IClosable::Close` (6).
pub const SLOT_CLOSABLE_CLOSE: usize = 6;

/// `IPropertyValueStatics`: CreateUInt32 (11), CreateSingle (14),
/// CreateBoolean (17), CreateSingleArray (33).
pub const SLOT_PV_CREATE_UINT32: usize = 11;
pub const SLOT_PV_CREATE_SINGLE: usize = 14;
pub const SLOT_PV_CREATE_BOOLEAN: usize = 17;
pub const SLOT_PV_CREATE_SINGLE_ARRAY: usize = 33;

/// `IGraphicsEffect::get_Name` (6), `put_Name` (7).
pub const SLOT_GRAPHICS_EFFECT_GET_NAME: usize = 6;
pub const SLOT_GRAPHICS_EFFECT_PUT_NAME: usize = 7;

/// `IGraphicsEffectD2D1Interop` is IUnknown-derived: methods start at slot 3.
pub const SLOT_INTEROP_GET_EFFECT_ID: usize = 3;
pub const SLOT_INTEROP_GET_NAMED_PROPERTY_MAPPING: usize = 4;
pub const SLOT_INTEROP_GET_PROPERTY_COUNT: usize = 5;
pub const SLOT_INTEROP_GET_PROPERTY: usize = 6;
pub const SLOT_INTEROP_GET_SOURCE: usize = 7;
pub const SLOT_INTEROP_GET_SOURCE_COUNT: usize = 8;

// ---------------------------------------------------------------------------
// Windows.UI.Color  (Windows.UI.h: { A, R, G, B }, 4 bytes)
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Color {
    pub a: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(a: u8, r: u8, g: u8, b: u8) -> Self {
        Self { a, r, g, b }
    }

    /// `0xAARRGGBB`.
    pub const fn from_argb(value: u32) -> Self {
        Self {
            a: (value >> 24) as u8,
            r: (value >> 16) as u8,
            g: (value >> 8) as u8,
            b: value as u8,
        }
    }

    pub const fn to_argb(self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }
}

// ---------------------------------------------------------------------------
// SEH guard (`src/seh_guard.c`)
// ---------------------------------------------------------------------------

#[link(name = "dcxblurguard")]
extern "C" {
    fn dcx_blur_guard(f: unsafe extern "C" fn(*mut c_void), arg: *mut c_void) -> i32;
}

struct GuardCtx<F, R> {
    f: Option<F>,
    out: Option<R>,
}

unsafe extern "C" fn guard_trampoline<F: FnOnce() -> R, R>(p: *mut c_void) {
    let ctx = &mut *(p as *mut GuardCtx<F, R>);
    if let Some(f) = ctx.f.take() {
        ctx.out = Some(f());
    }
}

/// Run `f` under a `__try/__except` frame.
///
/// Native exceptions do not unwind Rust frames here: if the guard trips, `f`
/// never returns and its captured state is abandoned, so callers must treat
/// `Err(code)` as "the operation failed, no result". `code` is the SEH
/// exception code (e.g. `0xC0000005` for an access violation).
pub fn guarded<F: FnOnce() -> R, R>(f: F) -> Result<R, i32> {
    let mut ctx: GuardCtx<F, R> = GuardCtx {
        f: Some(f),
        out: None,
    };
    let code = unsafe {
        dcx_blur_guard(
            guard_trampoline::<F, R>,
            &mut ctx as *mut GuardCtx<F, R> as *mut c_void,
        )
    };
    if code != 0 {
        Err(code)
    } else {
        match ctx.out {
            Some(v) => Ok(v),
            // Defensive: the trampoline always sets `out` on normal return.
            None => Err(-1),
        }
    }
}

// ---------------------------------------------------------------------------
// Logging hook
// ---------------------------------------------------------------------------

static LOG_HOOK: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// Install the crate's log sink (the engine passes a wrapper around
/// `dcx_engine::log`). Called from `OnConnected`/`OnDisconnected`, which run
/// long after `create` returned, hence a process-global hook instead of the
/// `BrushHost` borrow.
pub fn set_log_hook(f: fn(&str)) {
    LOG_HOOK.store(f as usize as *mut c_void, Ordering::SeqCst);
}

pub(crate) fn log(msg: &str) {
    let p = LOG_HOOK.load(Ordering::Relaxed);
    if !p.is_null() {
        let f: fn(&str) = unsafe { core::mem::transmute::<*mut c_void, fn(&str)>(p) };
        f(msg);
    }
}

/// Format an HRESULT as `0x%08X` for logging.
pub(crate) fn hr_hex(hr: HRESULT) -> String {
    format!("0x{:08X}", hr as u32)
}

// ---------------------------------------------------------------------------
// HSTRING
// ---------------------------------------------------------------------------

#[link(name = "runtimeobject")]
extern "system" {
    fn WindowsCreateString(source: *const u16, length: u32, out: *mut *mut c_void) -> HRESULT;
    fn WindowsDeleteString(value: *mut c_void) -> HRESULT;
    fn WindowsGetStringRawBuffer(value: *mut c_void, length: *mut u32) -> *const u16;
}

/// Copy an HSTRING's contents into a Rust `String` (does not take ownership).
pub unsafe fn hstring_to_string(value: *mut c_void) -> String {
    if value.is_null() {
        return String::new();
    }
    let mut len: u32 = 0;
    let buf = WindowsGetStringRawBuffer(value, &mut len);
    if buf.is_null() || len == 0 {
        return String::new();
    }
    let slice = core::slice::from_raw_parts(buf, len as usize);
    String::from_utf16_lossy(slice)
}

/// Release an HSTRING we received from an ABI out-parameter.
pub unsafe fn hstring_free(value: *mut c_void) {
    if !value.is_null() {
        WindowsDeleteString(value);
    }
}

/// Decode a NUL-terminated UTF-16 string (classic COM `LPCWSTR` parameters).
pub unsafe fn wide_to_string(value: *const u16) -> String {
    if value.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    // Classic COM parameters are NUL-terminated; cap defensively so a bogus
    // pointer cannot spin forever.
    while len < 4096 && *value.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(core::slice::from_raw_parts(value, len))
}

#[repr(transparent)]
pub struct HString(*mut c_void);

impl HString {
    pub fn create(value: &str) -> Option<Self> {
        if value.is_empty() {
            let mut out: *mut c_void = ptr::null_mut();
            let hr = unsafe { WindowsCreateString(ptr::null(), 0, &mut out) };
            return if failed(hr) || out.is_null() {
                None
            } else {
                Some(Self(out))
            };
        }
        let wide: Vec<u16> = value.encode_utf16().collect();
        let mut out: *mut c_void = ptr::null_mut();
        let hr = unsafe { WindowsCreateString(wide.as_ptr(), wide.len() as u32, &mut out) };
        if failed(hr) || out.is_null() {
            None
        } else {
            Some(Self(out))
        }
    }

    #[inline]
    pub fn as_raw(&self) -> *mut c_void {
        self.0
    }

    /// Transfer ownership to the caller (an out-HSTRING parameter, e.g.
    /// `GetRuntimeClassName`).
    #[inline]
    pub fn into_raw(self) -> *mut c_void {
        let raw = self.0;
        core::mem::forget(self);
        raw
    }
}

impl Drop for HString {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                WindowsDeleteString(self.0);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RoGetActivationFactory
// ---------------------------------------------------------------------------

#[link(name = "runtimeobject")]
extern "system" {
    fn RoGetActivationFactory(
        class_id: *mut c_void,
        iid: *const Guid,
        factory: *mut *mut c_void,
    ) -> HRESULT;
}

/// `RoGetActivationFactory(class, iid)`; returns an owned interface pointer.
pub unsafe fn activation_factory(class: &str, iid: &Guid) -> Option<ComPtrLike> {
    let name = HString::create(class)?;
    let mut out: *mut c_void = ptr::null_mut();
    let hr = RoGetActivationFactory(name.as_raw(), iid, &mut out);
    if failed(hr) || out.is_null() {
        log(&format!(
            "dcx-blur: RoGetActivationFactory({class}) -> {}",
            hr_hex(hr)
        ));
        return None;
    }
    Some(ComPtrLike::from_raw_owned(out))
}

// ---------------------------------------------------------------------------
// Raw vtable calls
// ---------------------------------------------------------------------------

/// Read entry `slot` of the vtable of `this` and transmute it to `F`.
///
/// # Safety
/// `this` must be a live COM interface pointer whose vtable has at least
/// `slot + 1` entries, and `F` must be the exact signature at that slot.
pub unsafe fn method<F: Copy>(this: *mut c_void, slot: usize) -> F {
    let vtable = *(this as *const *const *const c_void);
    let raw = *vtable.add(slot);
    debug_assert_eq!(
        core::mem::size_of::<F>(),
        core::mem::size_of::<*const c_void>()
    );
    core::mem::transmute_copy::<*const c_void, F>(&raw)
}

/// `HRESULT Method()`.
pub unsafe fn method0_hr(this: *mut c_void, slot: usize) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void) -> HRESULT = method(this, slot);
    f(this)
}

/// `HRESULT Method(A)`.
pub unsafe fn method1_hr<A>(this: *mut c_void, slot: usize, a: A) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, A) -> HRESULT = method(this, slot);
    f(this, a)
}

/// `HRESULT Method(A, B)`.
pub unsafe fn method2_hr<A, B>(this: *mut c_void, slot: usize, a: A, b: B) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, A, B) -> HRESULT = method(this, slot);
    f(this, a, b)
}

/// `HRESULT Method(A, B, C)`.
pub unsafe fn method3_hr<A, B, C>(this: *mut c_void, slot: usize, a: A, b: B, c: C) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, A, B, C) -> HRESULT = method(this, slot);
    f(this, a, b, c)
}

/// `HRESULT Method(out T)`, where `T` is an ABI scalar/struct/pointer.
pub unsafe fn get_out<T>(this: *mut c_void, slot: usize) -> Option<T> {
    let mut out: T = core::mem::zeroed();
    let f: unsafe extern "system" fn(*mut c_void, *mut T) -> HRESULT = method(this, slot);
    let hr = f(this, &mut out);
    if failed(hr) {
        None
    } else {
        Some(out)
    }
}

/// `HRESULT Method(out IInspectable-ish*)`, returning an owned object pointer.
pub unsafe fn out0_object(this: *mut c_void, slot: usize) -> Option<ComPtrLike> {
    let mut out: *mut c_void = ptr::null_mut();
    let f: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT = method(this, slot);
    let hr = f(this, &mut out);
    if failed(hr) || out.is_null() {
        None
    } else {
        Some(ComPtrLike::from_raw_owned(out))
    }
}

/// `HRESULT Method(A, out IInspectable-ish*)`, returning an owned object pointer.
pub unsafe fn out1_object<A>(this: *mut c_void, slot: usize, a: A) -> Option<ComPtrLike> {
    let mut out: *mut c_void = ptr::null_mut();
    let f: unsafe extern "system" fn(*mut c_void, A, *mut *mut c_void) -> HRESULT =
        method(this, slot);
    let hr = f(this, a, &mut out);
    if failed(hr) || out.is_null() {
        None
    } else {
        Some(ComPtrLike::from_raw_owned(out))
    }
}

// ---------------------------------------------------------------------------
// IUnknown helpers
// ---------------------------------------------------------------------------

pub unsafe fn add_ref(ptr: *mut c_void) -> ULONG {
    if ptr.is_null() {
        return 0;
    }
    let f: unsafe extern "system" fn(*mut c_void) -> ULONG = method(ptr, 1);
    f(ptr)
}

pub unsafe fn release(ptr: *mut c_void) -> ULONG {
    if ptr.is_null() {
        return 0;
    }
    let f: unsafe extern "system" fn(*mut c_void) -> ULONG = method(ptr, 2);
    f(ptr)
}

pub unsafe fn query_interface(ptr: *mut c_void, iid: &Guid) -> Option<ComPtrLike> {
    if ptr.is_null() {
        return None;
    }
    let mut out: *mut c_void = ptr::null_mut();
    let f: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT =
        method(ptr, 0);
    let hr = f(ptr, iid, &mut out);
    if failed(hr) || out.is_null() {
        None
    } else {
        Some(ComPtrLike::from_raw_owned(out))
    }
}

// ---------------------------------------------------------------------------
// ComPtrLike
// ---------------------------------------------------------------------------

/// Owned COM interface pointer (one reference).
///
/// * `ComPtrLike::from_raw_owned(raw)` takes over one reference.
/// * `ComPtrLike::from_raw_borrowed(raw)` AddRefs; use it when the caller keeps
///   ownership.
/// * `as_raw()` is the ABI pointer (`IInspectable*` / `IUIElement*` / ...),
///   exactly what the engine's own `ComPtr` holds.
/// * `query(iid)` returns another owned pointer.
/// * Dropping calls `Release` through the pointer's own vtable.
#[repr(transparent)]
pub struct ComPtrLike {
    ptr: *mut c_void,
}

impl ComPtrLike {
    pub const fn null() -> Self {
        Self {
            ptr: ptr::null_mut(),
        }
    }

    #[inline]
    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    #[inline]
    pub fn as_raw(&self) -> *mut c_void {
        self.ptr
    }

    /// Take ownership of one reference without AddRef.
    ///
    /// # Safety
    /// `ptr` must be null or a valid interface pointer owning exactly one
    /// reference that the caller is giving up.
    pub unsafe fn from_raw_owned(ptr: *mut c_void) -> Self {
        Self { ptr }
    }

    /// AddRef `ptr` and take the new reference.
    ///
    /// # Safety
    /// `ptr` must be null or a live COM interface pointer.
    pub unsafe fn from_raw_borrowed(ptr: *mut c_void) -> Self {
        if !ptr.is_null() {
            add_ref(ptr);
        }
        Self { ptr }
    }

    /// Give up ownership, returning the raw pointer (no Release).
    pub fn into_raw(self) -> *mut c_void {
        let raw = self.ptr;
        core::mem::forget(self);
        raw
    }

    /// QI to `iid`; returns a new owned reference (or `None`).
    ///
    /// # Safety
    /// The pointer must be live.
    pub unsafe fn query(&self, iid: &Guid) -> Option<ComPtrLike> {
        query_interface(self.ptr, iid)
    }

    /// `out0_object` on this pointer.
    pub unsafe fn out0_object(&self, slot: usize) -> Option<ComPtrLike> {
        out0_object(self.ptr, slot)
    }

    /// `out1_object` on this pointer.
    pub unsafe fn out1_object<A>(&self, slot: usize, a: A) -> Option<ComPtrLike> {
        out1_object(self.ptr, slot, a)
    }

    /// `method0_hr` on this pointer.
    pub unsafe fn method0_hr(&self, slot: usize) -> HRESULT {
        method0_hr(self.ptr, slot)
    }

    /// `method1_hr` on this pointer.
    pub unsafe fn method1_hr<A>(&self, slot: usize, a: A) -> HRESULT {
        method1_hr(self.ptr, slot, a)
    }

    /// `method2_hr` on this pointer.
    pub unsafe fn method2_hr<A, B>(&self, slot: usize, a: A, b: B) -> HRESULT {
        method2_hr(self.ptr, slot, a, b)
    }

    /// `get_out` on this pointer.
    pub unsafe fn get_out<T>(&self, slot: usize) -> Option<T> {
        get_out(self.ptr, slot)
    }
}

impl Drop for ComPtrLike {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                release(self.ptr);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared reference count for the COM objects this crate implements.
// ---------------------------------------------------------------------------

pub(crate) struct Count(AtomicU32);

impl Count {
    pub const fn new(value: u32) -> Self {
        Self(AtomicU32::new(value))
    }

    pub fn add(&self) -> u32 {
        self.0.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Returns the remaining count after the decrement.
    pub fn release(&self) -> u32 {
        self.0.fetch_sub(1, Ordering::AcqRel).wrapping_sub(1)
    }
}

// ---------------------------------------------------------------------------
// XAML `{ThemeResource KEY}` colour resolution (reference ctor 13449-13472)
// ---------------------------------------------------------------------------

const XAML_PRESENTATION_NS: &str = "http://schemas.microsoft.com/winfx/2006/xaml/presentation";

/// `XamlReader.Load("<SolidColorBrush Color=\"{ThemeResource KEY}\"/>")`
/// followed by `ISolidColorBrush.Color`, exactly like `createProxy` in the
/// reference ctor.
///
/// The reference then inserts this brush into the element's
/// `ResourceDictionary` and hooks `Color` changes; that part is deferred (see
/// `brush.rs`), the immediate colour read is what stage 1 needs.
///
/// # Safety
/// Must run on the UI thread. The call can raise (bad markup / unknown theme
/// resource): the caller wraps it in [`guarded`].
pub unsafe fn xaml_theme_color(key: &str) -> Option<Color> {
    let factory = activation_factory("Windows.UI.Xaml.Markup.XamlReader", &IID_IXAMLREADERSTATICS)?;
    let xaml = format!(
        r#"<SolidColorBrush xmlns="{XAML_PRESENTATION_NS}" Color="{{ThemeResource {key}}}"/>"#
    );
    let source = HString::create(&xaml)?;
    let loaded = out1_object(factory.as_raw(), SLOT_XAMLREADER_LOAD, source.as_raw())?;
    let brush = loaded.query(&IID_ISOLIDCOLORBRUSH)?;
    get_out::<Color>(brush.as_raw(), SLOT_SOLIDCOLORBRUSH_GET_COLOR)
}

/// `ElementCompositionPreview.GetElementVisual(element).Compositor()`.
///
/// The engine normally supplies the compositor through
/// [`crate::brush::BrushHost::compositor_for`]; this is the self-contained
/// fallback used when the host does not have one.
pub unsafe fn element_compositor(ui_element: *mut c_void) -> Option<ComPtrLike> {
    let statics = activation_factory(
        "Windows.UI.Xaml.Hosting.ElementCompositionPreview",
        &IID_IELEMENTCOMPOSITIONPREVIEWSTATICS,
    )?;
    let visual = out1_object(
        statics.as_raw(),
        SLOT_ELEMENT_COMPOSITION_PREVIEW_GET_ELEMENT_VISUAL,
        ui_element,
    )?;
    let composition_object = visual.query(&IID_ICOMPOSITIONOBJECT)?;
    out0_object(
        composition_object.as_raw(),
        SLOT_COMPOSITION_OBJECT_GET_COMPOSITOR,
    )
}

// ---------------------------------------------------------------------------
// Energy saver / advanced effects (reference ShouldUseFallback, 13909-13965)
// ---------------------------------------------------------------------------

#[repr(C)]
struct SystemPowerStatus {
    ac_line_status: u8,
    battery_flag: u8,
    battery_life_percent: u8,
    system_status_flag: u8,
    battery_life_time: u32,
    battery_full_life_time: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
}

#[link(name = "advapi32")]
extern "system" {
    fn RegOpenKeyExW(
        key: isize,
        sub_key: *const u16,
        options: u32,
        desired: u32,
        result: *mut isize,
    ) -> i32;
    fn RegQueryValueExW(
        key: isize,
        value_name: *const u16,
        reserved: *mut u32,
        ty: *mut u32,
        data: *mut u8,
        data_size: *mut u32,
    ) -> i32;
    fn RegCloseKey(key: isize) -> i32;
}

const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
const KEY_QUERY_VALUE: u32 = 0x0001;
const REG_DWORD: u32 = 4;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// `HKLM\SYSTEM\CurrentControlSet\Control\Power\EnergySaverState == 1`
/// (reference: the only reliable signal on Windows 11 24H2+).
unsafe fn energy_saver_registry() -> bool {
    let mut key: isize = 0;
    let path = wide("SYSTEM\\CurrentControlSet\\Control\\Power");
    if RegOpenKeyExW(
        HKEY_LOCAL_MACHINE,
        path.as_ptr(),
        0,
        KEY_QUERY_VALUE,
        &mut key,
    ) != 0
    {
        return false;
    }
    let mut value: u32 = 0;
    let mut ty: u32 = 0;
    let mut size: u32 = core::mem::size_of::<u32>() as u32;
    let name = wide("EnergySaverState");
    let ok = RegQueryValueExW(
        key,
        name.as_ptr(),
        ptr::null_mut(),
        &mut ty,
        &mut value as *mut u32 as *mut u8,
        &mut size,
    ) == 0
        && ty == REG_DWORD;
    RegCloseKey(key);
    ok && value == 1
}

/// Backup signal for older Windows (reference fallback).
unsafe fn energy_saver_power_status() -> bool {
    let mut status = SystemPowerStatus {
        ac_line_status: 0,
        battery_flag: 0,
        battery_life_percent: 0,
        system_status_flag: 0,
        battery_life_time: 0,
        battery_full_life_time: 0,
    };
    GetSystemPowerStatus(&mut status) != 0 && status.system_status_flag != 0
}

/// Reference `ShouldUseFallback`'s energy-saver half.
///
/// The `UISettings::AdvancedEffectsEnabled` half is **not ported for stage 1**:
/// it can only make this function return `true` when a `FallbackColor` /
/// `FallbackColor="{ThemeResource ...}"` is configured, and the ported theme
/// (`Glass`, reference lines 3574/6388/3769) has no fallback colour, so the
/// decision point short-circuits to `false` before this is ever reached.
/// Documented deviation, not a substitute: the struct fields and the branch are
/// kept so a later stage can fill this in.
pub(crate) unsafe fn energy_saver_active() -> bool {
    energy_saver_registry() || matches!(guarded(|| energy_saver_power_status()), Ok(true))
}

// ---------------------------------------------------------------------------
// CoTaskMem helpers (GetIids arrays)
// ---------------------------------------------------------------------------

#[link(name = "ole32")]
extern "system" {
    fn CoTaskMemAlloc(size: usize) -> *mut c_void;
    fn CoTaskMemFree(ptr: *mut c_void);
}

pub(crate) unsafe fn com_alloc_iids(iids: &[Guid]) -> Option<(*mut Guid, u32)> {
    let size = core::mem::size_of::<Guid>() * iids.len();
    if size == 0 {
        return Some((ptr::null_mut(), 0));
    }
    let mem = CoTaskMemAlloc(size) as *mut Guid;
    if mem.is_null() {
        return None;
    }
    ptr::copy_nonoverlapping(iids.as_ptr(), mem, iids.len());
    Some((mem, iids.len() as u32))
}

pub(crate) unsafe fn com_free(ptr: *mut c_void) {
    if !ptr.is_null() {
        CoTaskMemFree(ptr);
    }
}

// ---------------------------------------------------------------------------
// Small COM-object support shared by effects.rs / brush.rs
// ---------------------------------------------------------------------------

/// Facade vtables live at offsets 0/8/16 of an `align(32)` allocation.
pub(crate) const FACADE_ALIGN: usize = 32;

/// Recover the object pointer from one of its facade pointers.
///
/// Facades live at offsets 0, 8 and 16 inside an `align(32)` allocation, so
/// masking the low 5 bits always lands on the object base.
#[inline]
pub(crate) unsafe fn object_from_facade<T>(this: *mut c_void) -> *mut T {
    debug_assert!(!this.is_null());
    ((this as usize) & !(FACADE_ALIGN - 1)) as *mut T
}

/// BaseTrust.
pub(crate) const BASE_TRUST: i32 = 0;

/// Shared `GetRuntimeClassName` body: returns `class_name` as an HSTRING.
pub(crate) unsafe fn write_runtime_class_name(class_name: &str, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    *out = ptr::null_mut();
    match HString::create(class_name) {
        Some(h) => {
            *out = h.into_raw();
            S_OK
        }
        None => E_OUTOFMEMORY,
    }
}

/// Shared `GetTrustLevel` body.
pub(crate) unsafe fn write_trust_level(out: *mut i32) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    *out = BASE_TRUST;
    S_OK
}
