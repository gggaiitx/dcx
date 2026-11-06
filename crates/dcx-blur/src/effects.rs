//! Hand-rolled D2D effect COM objects.
//!
//! Faithful port of the five effect structs in the reference mod:
//!
//! | struct | reference line | effect GUID |
//! |---|---|---|
//! | `CompositeEffect`   | 12714 | `CLSID_D2D1Composite`    |
//! | `FloodEffect`       | 12840 | `CLSID_D2D1Flood`        |
//! | `BorderEffect`      | 12965 | `CLSID_D2D1Border`       |
//! | `GaussianBlurEffect`| 13105 | `CLSID_D2D1GaussianBlur` |
//! | `ColorMatrixEffect` | 13259 | `CLSID_D2D1ColorMatrix`  |
//!
//! Each object implements `Windows.Graphics.Effects.IGraphicsEffect`,
//! `Windows.Graphics.Effects.IGraphicsEffectSource` and
//! `IGraphicsEffectD2D1Interop` (`2FC57384-A068-44D7-A331-30982FCF7177`).
//! The named-property mappings, property counts, source counts and default
//! values below are copied one-to-one from the reference; the compositor uses
//! them to build the D2D graph, so they are the part that makes the brush
//! composite correctly.
//!
//! C++/WinRT's `implements<D, IGraphicsEffect, IGraphicsEffectSource,
//! IGraphicsEffectD2D1Interop>` gives the object one *facade* per interface:
//! the identity/default interface is `IGraphicsEffect` (offset 0), the others
//! follow at offsets 8 and 16. Every facade's vtable starts with the same six
//! `IInspectable` entries (the two WinMD interfaces) or the three `IUnknown`
//! entries (`IGraphicsEffectD2D1Interop`, not a WinMD interface).

#![allow(non_snake_case)]

use crate::ffi::*;
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::ptr;

// ---------------------------------------------------------------------------
// D2D constants (d2d1effects.h, reference 12737-13419)
// ---------------------------------------------------------------------------

pub const CLSID_D2D1COMPOSITE: Guid = Guid::new(
    0x48FC_9F51,
    0xF6AC,
    0x48F1,
    [0x8B, 0x58, 0x3B, 0x28, 0xAC, 0x46, 0xF7, 0x6D],
);
pub const CLSID_D2D1FLOOD: Guid = Guid::new(
    0x61C2_3C20,
    0xAE69,
    0x4D8E,
    [0x94, 0xCF, 0x50, 0x07, 0x8D, 0xF6, 0x38, 0xF2],
);
pub const CLSID_D2D1BORDER: Guid = Guid::new(
    0x2A2D_49C0,
    0x4ACF,
    0x43C7,
    [0x8C, 0x6A, 0x7C, 0x4A, 0x27, 0x87, 0x4D, 0x27],
);
pub const CLSID_D2D1GAUSSIANBLUR: Guid = Guid::new(
    0x1FEB_6D69,
    0x2FE6,
    0x4AC9,
    [0x8C, 0x58, 0x1D, 0x7F, 0x93, 0xE7, 0xA6, 0xA5],
);
pub const CLSID_D2D1COLORMATRIX: Guid = Guid::new(
    0x921F_03D6,
    0x641C,
    0x47DF,
    [0x85, 0x2D, 0xB4, 0xBB, 0x61, 0x53, 0xAE, 0x11],
);

// Property indices.
pub const D2D1_COMPOSITE_PROP_MODE: u32 = 0;
pub const D2D1_FLOOD_PROP_COLOR: u32 = 0;
pub const D2D1_BORDER_PROP_EDGE_MODE_X: u32 = 0;
pub const D2D1_BORDER_PROP_EDGE_MODE_Y: u32 = 1;
pub const D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION: u32 = 0;
pub const D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION: u32 = 1;
pub const D2D1_GAUSSIANBLUR_PROP_BORDER_MODE: u32 = 2;
pub const D2D1_COLORMATRIX_PROP_COLOR_MATRIX: u32 = 0;
pub const D2D1_COLORMATRIX_PROP_ALPHA_MODE: u32 = 1;
pub const D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT: u32 = 2;

// Enum values the reference uses.
/// `D2D1_COMPOSITE_MODE_SOURCE_OVER` (d2d1_1.h).
pub const D2D1_COMPOSITE_MODE_SOURCE_OVER: u32 = 0;
/// `D2D1_BORDER_EDGE_MODE_WRAP` (d2d1effects.h).
pub const D2D1_BORDER_EDGE_MODE_WRAP: u32 = 1;
/// `D2D1_BORDER_MODE_SOFT` (d2d1effects.h).
pub const D2D1_BORDER_MODE_SOFT: u32 = 0;
/// `MY_D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED` (reference 12563-12568).
pub const GAUSSIANBLUR_OPTIMIZATION_BALANCED: u32 = 1;
/// `D2D1_COLORMATRIX_ALPHA_MODE_PREMULTIPLIED` (d2d1effects.h).
pub const COLORMATRIX_ALPHA_MODE_PREMULTIPLIED: u32 = 1;
/// `GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT`
/// (windows.graphics.effects.interop.h, reference 12640-12653).
pub const GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT: i32 = 1;

/// Identity colour matrix (`D2D1_MATRIX_5X4_F`), reference 13277-13283.
pub const IDENTITY_MATRIX: [f32; 20] = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0, //
    0.0, 0.0, 0.0, 0.0,
];

// ---------------------------------------------------------------------------
// Object layout
// ---------------------------------------------------------------------------

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectKind {
    Composite = 0,
    Flood = 1,
    Border = 2,
    GaussianBlur = 3,
    ColorMatrix = 4,
}

enum EffectData {
    Composite {
        mode: u32,
        sources: Vec<ComPtrLike>,
    },
    Flood {
        color: Color,
    },
    Border {
        source: Option<ComPtrLike>,
        extend_x: u32,
        extend_y: u32,
    },
    GaussianBlur {
        source: Option<ComPtrLike>,
        blur_amount: f32,
        optimization: u32,
        border_mode: u32,
    },
    ColorMatrix {
        source: Option<ComPtrLike>,
        matrix: [f32; 20],
        alpha_mode: u32,
        clamp_output: bool,
    },
}

#[repr(C)]
pub(crate) struct GfxEffectVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> ULONG,
    release: unsafe extern "system" fn(*mut c_void) -> ULONG,
    get_iids: unsafe extern "system" fn(*mut c_void, *mut u32, *mut *mut Guid) -> HRESULT,
    get_runtime_class_name: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    get_trust_level: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT,
    get_name: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    put_name: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT,
}

#[repr(C)]
pub(crate) struct GfxSourceVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> ULONG,
    release: unsafe extern "system" fn(*mut c_void) -> ULONG,
    get_iids: unsafe extern "system" fn(*mut c_void, *mut u32, *mut *mut Guid) -> HRESULT,
    get_runtime_class_name: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    get_trust_level: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT,
}

/// `IGraphicsEffectD2D1Interop` (IUnknown-derived: no IInspectable methods).
#[repr(C)]
pub(crate) struct InteropVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> ULONG,
    release: unsafe extern "system" fn(*mut c_void) -> ULONG,
    get_effect_id: unsafe extern "system" fn(*mut c_void, *mut Guid) -> HRESULT,
    get_named_property_mapping:
        unsafe extern "system" fn(*mut c_void, *const u16, *mut u32, *mut i32) -> HRESULT,
    get_property_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    get_property: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT,
    get_source: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT,
    get_source_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
}

#[repr(C, align(32))]
pub(crate) struct EffectObject {
    /// Identity / default interface (`IGraphicsEffect`) facade, offset 0.
    v_effect: *const GfxEffectVtbl,
    /// `IGraphicsEffectSource` facade, offset 8.
    v_source: *const GfxSourceVtbl,
    /// `IGraphicsEffectD2D1Interop` facade, offset 16.
    v_interop: *const InteropVtbl,
    refs: Count,
    kind: EffectKind,
    name: UnsafeCell<String>,
    data: UnsafeCell<EffectData>,
}

impl EffectObject {
    #[inline]
    fn effect_facade(&self) -> *mut c_void {
        self as *const Self as *mut c_void
    }

    #[inline]
    fn source_facade(&self) -> *mut c_void {
        unsafe { (self as *const Self as *mut u8).add(8) as *mut c_void }
    }

    #[inline]
    fn interop_facade(&self) -> *mut c_void {
        unsafe { (self as *const Self as *mut u8).add(16) as *mut c_void }
    }
}

#[inline]
unsafe fn obj(this: *mut c_void) -> *mut EffectObject {
    object_from_facade::<EffectObject>(this)
}

#[inline]
unsafe fn data<'a>(o: *mut EffectObject) -> &'a mut EffectData {
    &mut *(*o).data.get()
}

// ---------------------------------------------------------------------------
// IUnknown / IInspectable
// ---------------------------------------------------------------------------

unsafe extern "system" fn effect_qi(
    this: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    *out = ptr::null_mut();
    let iid = &*iid;
    let o = obj(this);
    let facade = if *iid == IID_IUNKNOWN
        || *iid == IID_IINSPECTABLE
        || *iid == IID_IAGILEOBJECT
        || *iid == IID_IGRAPHICSEFFECT
    {
        (*o).effect_facade()
    } else if *iid == IID_IGRAPHICSEFFECTSOURCE {
        (*o).source_facade()
    } else if *iid == IID_IGRAPHICSEFFECTD2D1INTEROP {
        (*o).interop_facade()
    } else {
        return E_NOINTERFACE;
    };
    add_ref(facade);
    *out = facade;
    S_OK
}

unsafe extern "system" fn effect_add_ref(this: *mut c_void) -> ULONG {
    (*obj(this)).refs.add()
}

unsafe extern "system" fn effect_release(this: *mut c_void) -> ULONG {
    let o = obj(this);
    let remaining = (*o).refs.release();
    if remaining == 0 {
        drop(Box::from_raw(o));
    }
    remaining
}

unsafe extern "system" fn effect_get_iids(
    _this: *mut c_void,
    count: *mut u32,
    array: *mut *mut Guid,
) -> HRESULT {
    if count.is_null() || array.is_null() {
        return E_POINTER;
    }
    match com_alloc_iids(&[IID_IGRAPHICSEFFECT, IID_IGRAPHICSEFFECTSOURCE]) {
        Some((mem, n)) => {
            *count = n;
            *array = mem;
            S_OK
        }
        None => {
            *count = 0;
            *array = ptr::null_mut();
            E_OUTOFMEMORY
        }
    }
}

unsafe extern "system" fn effect_get_runtime_class_name(
    _this: *mut c_void,
    out: *mut *mut c_void,
) -> HRESULT {
    // C++/WinRT returns the name of the default interface of the
    // implementation type; for these structs that is IGraphicsEffect.
    write_runtime_class_name("Windows.Graphics.Effects.IGraphicsEffect", out)
}

unsafe extern "system" fn effect_get_trust_level(_this: *mut c_void, out: *mut i32) -> HRESULT {
    write_trust_level(out)
}

// ---------------------------------------------------------------------------
// IGraphicsEffect::Name
// ---------------------------------------------------------------------------

unsafe extern "system" fn effect_get_name(this: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    *out = ptr::null_mut();
    let o = obj(this);
    let name = &*(*o).name.get();
    match HString::create(name) {
        Some(h) => {
            *out = h.into_raw();
            S_OK
        }
        None => E_OUTOFMEMORY,
    }
}

unsafe extern "system" fn effect_put_name(this: *mut c_void, name: *mut c_void) -> HRESULT {
    let o = obj(this);
    let value = if name.is_null() {
        String::new()
    } else {
        hstring_to_string(name)
    };
    *(*o).name.get() = value;
    S_OK
}

// ---------------------------------------------------------------------------
// IGraphicsEffectD2D1Interop
// ---------------------------------------------------------------------------

unsafe extern "system" fn interop_get_effect_id(this: *mut c_void, id: *mut Guid) -> HRESULT {
    if id.is_null() {
        return E_INVALIDARG;
    }
    let o = obj(this);
    *id = match (*o).kind {
        EffectKind::Composite => CLSID_D2D1COMPOSITE,
        EffectKind::Flood => CLSID_D2D1FLOOD,
        EffectKind::Border => CLSID_D2D1BORDER,
        EffectKind::GaussianBlur => CLSID_D2D1GAUSSIANBLUR,
        EffectKind::ColorMatrix => CLSID_D2D1COLORMATRIX,
    };
    S_OK
}

unsafe extern "system" fn interop_get_named_property_mapping(
    this: *mut c_void,
    name: *const u16,
    index: *mut u32,
    mapping: *mut i32,
) -> HRESULT {
    if name.is_null() || index.is_null() || mapping.is_null() {
        return E_INVALIDARG;
    }
    let o = obj(this);
    let name = wide_to_string(name);
    let prop_index: Option<u32> = match (*o).kind {
        EffectKind::Composite => (name == "Mode").then_some(D2D1_COMPOSITE_PROP_MODE),
        EffectKind::Flood => (name == "Color").then_some(D2D1_FLOOD_PROP_COLOR),
        EffectKind::Border => {
            if name == "ExtendX" {
                Some(D2D1_BORDER_PROP_EDGE_MODE_X)
            } else if name == "ExtendY" {
                Some(D2D1_BORDER_PROP_EDGE_MODE_Y)
            } else {
                None
            }
        }
        EffectKind::GaussianBlur => {
            if name == "BlurAmount" {
                Some(D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION)
            } else if name == "Optimization" {
                Some(D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION)
            } else if name == "BorderMode" {
                Some(D2D1_GAUSSIANBLUR_PROP_BORDER_MODE)
            } else {
                None
            }
        }
        EffectKind::ColorMatrix => {
            if name == "ColorMatrix" {
                Some(D2D1_COLORMATRIX_PROP_COLOR_MATRIX)
            } else if name == "AlphaMode" {
                Some(D2D1_COLORMATRIX_PROP_ALPHA_MODE)
            } else if name == "ClampOutput" {
                Some(D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT)
            } else {
                None
            }
        }
    };
    match prop_index {
        Some(i) => {
            *index = i;
            *mapping = GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT;
            S_OK
        }
        None => E_INVALIDARG,
    }
}

unsafe extern "system" fn interop_get_property_count(
    this: *mut c_void,
    count: *mut u32,
) -> HRESULT {
    if count.is_null() {
        return E_INVALIDARG;
    }
    let o = obj(this);
    *count = match (*o).kind {
        EffectKind::Composite => 1,
        EffectKind::Flood => 1,
        EffectKind::Border => 2,
        EffectKind::GaussianBlur => 3,
        EffectKind::ColorMatrix => 3,
    };
    S_OK
}

enum PropValueArg<'a> {
    UInt32(u32),
    Single(f32),
    Boolean(bool),
    SingleArray(&'a [f32]),
}

/// Create a `Windows.Foundation.IPropertyValue` through the
/// `Windows.Foundation.PropertyValue` statics, then QI it to `IPropertyValue`
/// (the ABI out-parameter type; the raw `IInspectable*` the statics return is a
/// different vtable).
unsafe fn property_value(
    stats: *mut c_void,
    slot: usize,
    kind: PropValueArg<'_>,
) -> Option<ComPtrLike> {
    let mut out: *mut c_void = ptr::null_mut();
    let hr = match kind {
        PropValueArg::UInt32(value) => {
            let f: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT =
                method(stats, slot);
            f(stats, value, &mut out)
        }
        PropValueArg::Single(value) => {
            let f: unsafe extern "system" fn(*mut c_void, f32, *mut *mut c_void) -> HRESULT =
                method(stats, slot);
            f(stats, value, &mut out)
        }
        PropValueArg::Boolean(value) => {
            let f: unsafe extern "system" fn(*mut c_void, bool, *mut *mut c_void) -> HRESULT =
                method(stats, slot);
            f(stats, value, &mut out)
        }
        PropValueArg::SingleArray(values) => {
            let f: unsafe extern "system" fn(
                *mut c_void,
                u32,
                *mut f32,
                *mut *mut c_void,
            ) -> HRESULT = method(stats, slot);
            f(
                stats,
                values.len() as u32,
                values.as_ptr() as *mut f32,
                &mut out,
            )
        }
    };
    if failed(hr) || out.is_null() {
        return None;
    }
    let pv = query_interface(out, &IID_IPROPERTYVALUE);
    release(out);
    pv
}

unsafe fn property_value_statics() -> Option<ComPtrLike> {
    activation_factory(
        "Windows.Foundation.PropertyValue",
        &IID_IPROPERTYVALUESTATICS,
    )
}

unsafe fn pv_uint32(value: u32) -> Option<ComPtrLike> {
    let stats = property_value_statics()?;
    property_value(
        stats.as_raw(),
        SLOT_PV_CREATE_UINT32,
        PropValueArg::UInt32(value),
    )
}

unsafe fn pv_single(value: f32) -> Option<ComPtrLike> {
    let stats = property_value_statics()?;
    property_value(
        stats.as_raw(),
        SLOT_PV_CREATE_SINGLE,
        PropValueArg::Single(value),
    )
}

unsafe fn pv_boolean(value: bool) -> Option<ComPtrLike> {
    let stats = property_value_statics()?;
    property_value(
        stats.as_raw(),
        SLOT_PV_CREATE_BOOLEAN,
        PropValueArg::Boolean(value),
    )
}

unsafe fn pv_single_array(values: &[f32]) -> Option<ComPtrLike> {
    let stats = property_value_statics()?;
    property_value(
        stats.as_raw(),
        SLOT_PV_CREATE_SINGLE_ARRAY,
        PropValueArg::SingleArray(values),
    )
}

unsafe extern "system" fn interop_get_property(
    this: *mut c_void,
    index: u32,
    value: *mut *mut c_void,
) -> HRESULT {
    if value.is_null() {
        return E_INVALIDARG;
    }
    *value = ptr::null_mut();
    let o = obj(this);
    let pv = match &mut *(*o).data.get() {
        EffectData::Composite { mode, .. } => {
            if index != D2D1_COMPOSITE_PROP_MODE {
                return E_BOUNDS;
            }
            pv_uint32(*mode)
        }
        EffectData::Flood { color } => {
            if index != D2D1_FLOOD_PROP_COLOR {
                return E_BOUNDS;
            }
            let c = *color;
            pv_single_array(&[
                c.r as f32 / 255.0,
                c.g as f32 / 255.0,
                c.b as f32 / 255.0,
                c.a as f32 / 255.0,
            ])
        }
        EffectData::Border {
            extend_x, extend_y, ..
        } => match index {
            D2D1_BORDER_PROP_EDGE_MODE_X => pv_uint32(*extend_x),
            D2D1_BORDER_PROP_EDGE_MODE_Y => pv_uint32(*extend_y),
            _ => return E_BOUNDS,
        },
        EffectData::GaussianBlur {
            blur_amount,
            optimization,
            border_mode,
            ..
        } => match index {
            D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION => pv_single(*blur_amount),
            D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION => pv_uint32(*optimization),
            D2D1_GAUSSIANBLUR_PROP_BORDER_MODE => pv_uint32(*border_mode),
            _ => return E_BOUNDS,
        },
        EffectData::ColorMatrix {
            matrix,
            alpha_mode,
            clamp_output,
            ..
        } => match index {
            D2D1_COLORMATRIX_PROP_COLOR_MATRIX => pv_single_array(matrix),
            D2D1_COLORMATRIX_PROP_ALPHA_MODE => pv_uint32(*alpha_mode),
            D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT => pv_boolean(*clamp_output),
            _ => return E_BOUNDS,
        },
    };
    match pv {
        Some(p) => {
            *value = p.into_raw();
            S_OK
        }
        None => E_FAIL,
    }
}

unsafe extern "system" fn interop_get_source(
    this: *mut c_void,
    index: u32,
    source: *mut *mut c_void,
) -> HRESULT {
    if source.is_null() {
        return E_INVALIDARG;
    }
    *source = ptr::null_mut();
    let o = obj(this);
    match &mut *(*o).data.get() {
        EffectData::Composite { sources, .. } => match sources.get(index as usize) {
            Some(s) => {
                add_ref(s.as_raw());
                *source = s.as_raw();
                S_OK
            }
            // `std::vector::at` throws -> E_BOUNDS in the reference.
            None => E_BOUNDS,
        },
        EffectData::Flood { .. } => E_BOUNDS,
        // Reference: `if (index == 0 && Source) { copy } return E_BOUNDS;`
        EffectData::Border { source: s, .. } | EffectData::ColorMatrix { source: s, .. } => match s
        {
            Some(s) if index == 0 => {
                add_ref(s.as_raw());
                *source = s.as_raw();
                S_OK
            }
            _ => E_BOUNDS,
        },
        // Reference GaussianBlur only checks `index == 0`; Source is non-null by
        // construction in CreateEffectBrush, so both paths agree.
        EffectData::GaussianBlur { source: s, .. } => {
            if index == 0 {
                if let Some(s) = s {
                    add_ref(s.as_raw());
                    *source = s.as_raw();
                }
                S_OK
            } else {
                E_BOUNDS
            }
        }
    }
}

unsafe extern "system" fn interop_get_source_count(this: *mut c_void, count: *mut u32) -> HRESULT {
    if count.is_null() {
        return E_INVALIDARG;
    }
    let o = obj(this);
    *count = match &mut *(*o).data.get() {
        EffectData::Composite { sources, .. } => sources.len() as u32,
        EffectData::Flood { .. } => 0,
        EffectData::Border { .. }
        | EffectData::GaussianBlur { .. }
        | EffectData::ColorMatrix { .. } => 1,
    };
    S_OK
}

// ---------------------------------------------------------------------------
// Vtables
// ---------------------------------------------------------------------------

static GFX_EFFECT_VTBL: GfxEffectVtbl = GfxEffectVtbl {
    query_interface: effect_qi,
    add_ref: effect_add_ref,
    release: effect_release,
    get_iids: effect_get_iids,
    get_runtime_class_name: effect_get_runtime_class_name,
    get_trust_level: effect_get_trust_level,
    get_name: effect_get_name,
    put_name: effect_put_name,
};

static GFX_SOURCE_VTBL: GfxSourceVtbl = GfxSourceVtbl {
    query_interface: effect_qi,
    add_ref: effect_add_ref,
    release: effect_release,
    get_iids: effect_get_iids,
    get_runtime_class_name: effect_get_runtime_class_name,
    get_trust_level: effect_get_trust_level,
};

static INTEROP_VTBL: InteropVtbl = InteropVtbl {
    query_interface: effect_qi,
    add_ref: effect_add_ref,
    release: effect_release,
    get_effect_id: interop_get_effect_id,
    get_named_property_mapping: interop_get_named_property_mapping,
    get_property_count: interop_get_property_count,
    get_property: interop_get_property,
    get_source: interop_get_source,
    get_source_count: interop_get_source_count,
};

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

fn default_name(kind: EffectKind) -> &'static str {
    match kind {
        EffectKind::Composite => "CompositeEffect",
        EffectKind::Flood => "FloodEffect",
        EffectKind::Border => "BorderEffect",
        EffectKind::GaussianBlur => "GaussianBlurEffect",
        EffectKind::ColorMatrix => "ColorMatrixEffect",
    }
}

fn allocate(kind: EffectKind, data: EffectData) -> ComPtrLike {
    let boxed = Box::new(EffectObject {
        v_effect: &GFX_EFFECT_VTBL,
        v_source: &GFX_SOURCE_VTBL,
        v_interop: &INTEROP_VTBL,
        refs: Count::new(1),
        kind,
        name: UnsafeCell::new(default_name(kind).to_string()),
        data: UnsafeCell::new(data),
    });
    let raw = Box::into_raw(boxed);
    unsafe { ComPtrLike::from_raw_owned((*raw).effect_facade()) }
}

/// `CompositeEffect` (reference 12714): default name "CompositeEffect",
/// default mode `D2D1_COMPOSITE_MODE_SOURCE_OVER`.
pub fn composite(mode: u32) -> ComPtrLike {
    allocate(
        EffectKind::Composite,
        EffectData::Composite {
            mode,
            sources: Vec::new(),
        },
    )
}

/// `FloodEffect` (reference 12840): default `Color{}` (all zero).
pub fn flood(color: Color) -> ComPtrLike {
    allocate(EffectKind::Flood, EffectData::Flood { color })
}

/// `BorderEffect` (reference 12965): null source, `ExtendX/Y = WRAP`.
pub fn border() -> ComPtrLike {
    allocate(
        EffectKind::Border,
        EffectData::Border {
            source: None,
            extend_x: D2D1_BORDER_EDGE_MODE_WRAP,
            extend_y: D2D1_BORDER_EDGE_MODE_WRAP,
        },
    )
}

/// `GaussianBlurEffect` (reference 13105): `BlurAmount = 3.0`,
/// `Optimization = BALANCED`, `BorderMode = SOFT`.
pub fn gaussian_blur() -> ComPtrLike {
    allocate(
        EffectKind::GaussianBlur,
        EffectData::GaussianBlur {
            source: None,
            blur_amount: 3.0,
            optimization: GAUSSIANBLUR_OPTIMIZATION_BALANCED,
            border_mode: D2D1_BORDER_MODE_SOFT,
        },
    )
}

/// `ColorMatrixEffect` (reference 13259): identity matrix,
/// `AlphaMode = PREMULTIPLIED`, `ClampOutput = false`.
pub fn color_matrix() -> ComPtrLike {
    allocate(
        EffectKind::ColorMatrix,
        EffectData::ColorMatrix {
            source: None,
            matrix: IDENTITY_MATRIX,
            alpha_mode: COLORMATRIX_ALPHA_MODE_PREMULTIPLIED,
            clamp_output: false,
        },
    )
}

// ---------------------------------------------------------------------------
// Typed field setters (the reference writes members directly, not via COM)
// ---------------------------------------------------------------------------

#[inline]
unsafe fn this_obj(effect: &ComPtrLike) -> *mut EffectObject {
    object_from_facade::<EffectObject>(effect.as_raw())
}

pub unsafe fn set_name(effect: &ComPtrLike, name: &str) {
    let o = this_obj(effect);
    *(*o).name.get() = name.to_string();
}

pub unsafe fn set_composite_mode(effect: &ComPtrLike, mode: u32) {
    let o = this_obj(effect);
    debug_assert_eq!((*o).kind, EffectKind::Composite);
    if let EffectData::Composite { mode: m, .. } = data(o) {
        *m = mode;
    }
}

pub unsafe fn set_flood_color(effect: &ComPtrLike, color: Color) {
    let o = this_obj(effect);
    debug_assert_eq!((*o).kind, EffectKind::Flood);
    if let EffectData::Flood { color: c } = data(o) {
        *c = color;
    }
}

pub unsafe fn set_gaussian_blur_amount(effect: &ComPtrLike, amount: f32) {
    let o = this_obj(effect);
    debug_assert_eq!((*o).kind, EffectKind::GaussianBlur);
    if let EffectData::GaussianBlur { blur_amount, .. } = data(o) {
        *blur_amount = amount;
    }
}

pub unsafe fn set_border_extend(effect: &ComPtrLike, x: u32, y: u32) {
    let o = this_obj(effect);
    debug_assert_eq!((*o).kind, EffectKind::Border);
    if let EffectData::Border {
        extend_x, extend_y, ..
    } = data(o)
    {
        *extend_x = x;
        *extend_y = y;
    }
}

pub unsafe fn set_color_matrix(effect: &ComPtrLike, matrix: &[f32; 20]) {
    let o = this_obj(effect);
    debug_assert_eq!((*o).kind, EffectKind::ColorMatrix);
    if let EffectData::ColorMatrix { matrix: m, .. } = data(o) {
        *m = *matrix;
    }
}

pub unsafe fn set_color_matrix_alpha_mode(effect: &ComPtrLike, alpha_mode: u32) {
    let o = this_obj(effect);
    debug_assert_eq!((*o).kind, EffectKind::ColorMatrix);
    if let EffectData::ColorMatrix { alpha_mode: a, .. } = data(o) {
        *a = alpha_mode;
    }
}

pub unsafe fn set_color_matrix_clamp_output(effect: &ComPtrLike, clamp: bool) {
    let o = this_obj(effect);
    debug_assert_eq!((*o).kind, EffectKind::ColorMatrix);
    if let EffectData::ColorMatrix { clamp_output, .. } = data(o) {
        *clamp_output = clamp;
    }
}

// ---------------------------------------------------------------------------
// Source wiring
// ---------------------------------------------------------------------------

/// QI `effect` (any of its facades) to `IGraphicsEffectSource`; the compositor
/// needs that interface when the object is used as another effect's source
/// (`wge::IGraphicsEffectSource topOfStack = *blurEffect;`).
pub unsafe fn as_source(effect: &ComPtrLike) -> Option<ComPtrLike> {
    effect.query(&IID_IGRAPHICSEFFECTSOURCE)
}

unsafe fn store_source(slot: &mut Option<ComPtrLike>, source: &ComPtrLike) {
    *slot = Some(ComPtrLike::from_raw_borrowed(source.as_raw()));
}

/// QI `object` to `IGraphicsEffectSource` and store it as `effect`'s source.
///
/// Used for `blurEffect->Source = wuc::CompositionEffectSourceParameter(L"backdrop")`:
/// the argument is any interface of the source-parameter object.
pub unsafe fn set_source_from_object(
    effect: &ComPtrLike,
    object: *mut c_void,
) -> Option<ComPtrLike> {
    let source = query_interface(object, &IID_IGRAPHICSEFFECTSOURCE)?;
    match data(this_obj(effect)) {
        EffectData::Border { source: s, .. } => store_source(s, &source),
        EffectData::GaussianBlur { source: s, .. } => store_source(s, &source),
        EffectData::ColorMatrix { source: s, .. } => store_source(s, &source),
        _ => return None,
    }
    Some(source)
}

/// Store an already-QI'd `IGraphicsEffectSource` as `effect`'s source.
/// (`satMatrix->Source = topOfStack;`)
pub unsafe fn set_source_interface(effect: &ComPtrLike, source: &ComPtrLike) -> bool {
    match data(this_obj(effect)) {
        EffectData::Border { source: s, .. } => store_source(s, source),
        EffectData::GaussianBlur { source: s, .. } => store_source(s, source),
        EffectData::ColorMatrix { source: s, .. } => store_source(s, source),
        _ => return false,
    }
    true
}

/// `compositeEffect->Sources.push_back(topOfStack);`
pub unsafe fn push_composite_source(effect: &ComPtrLike, source: &ComPtrLike) -> bool {
    match data(this_obj(effect)) {
        EffectData::Composite { sources, .. } => {
            sources.push(ComPtrLike::from_raw_borrowed(source.as_raw()));
            true
        }
        _ => false,
    }
}

/// `wuc::CompositionEffectSourceParameter(L"backdrop")`.
///
/// Returns the `CompositionEffectSourceParameter` object (default interface
/// `ICompositionEffectSourceParameter`); pass it to
/// [`set_source_from_object`] to store the `IGraphicsEffectSource` face.
pub unsafe fn source_parameter(name: &str) -> Option<ComPtrLike> {
    let factory = activation_factory(
        "Windows.UI.Composition.CompositionEffectSourceParameter",
        &IID_ICOMPOSITIONEFFECTSOURCEPARAMETERFACTORY,
    )?;
    let hname = HString::create(name)?;
    let mut out: *mut c_void = ptr::null_mut();
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> HRESULT =
        method(factory.as_raw(), SLOT_SOURCE_PARAMETER_FACTORY_CREATE);
    let hr = f(factory.as_raw(), hname.as_raw(), &mut out);
    if failed(hr) || out.is_null() {
        log(&format!(
            "dcx-blur: CompositionEffectSourceParameter.Create({name}) -> {}",
            hr_hex(hr)
        ));
        return None;
    }
    Some(ComPtrLike::from_raw_owned(out))
}

// ---------------------------------------------------------------------------
// Tests: exercise our own vtables directly (no XAML / no compositor needed).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi;

    #[link(name = "runtimeobject")]
    extern "system" {
        fn RoInitialize(init_type: i32) -> HRESULT;
    }

    static WINRT_INIT: std::sync::Once = std::sync::Once::new();

    /// Bare test processes are not WinRT-initialized (explorer is). Initialize
    /// MTA once so RoGetActivationFactory works in the ABI tests.
    unsafe fn init_winrt() {
        WINRT_INIT.call_once(|| {
            let _ = RoInitialize(1); // RO_INIT_MULTITHREADED; ignore HRESULT
        });
    }

    unsafe fn call_effect_id(effect: &ComPtrLike) -> Option<Guid> {
        let interop = effect.query(&IID_IGRAPHICSEFFECTD2D1INTEROP)?;
        let f: unsafe extern "system" fn(*mut c_void, *mut Guid) -> HRESULT =
            ffi::method(interop.as_raw(), SLOT_INTEROP_GET_EFFECT_ID);
        let mut id = Guid::new(0, 0, 0, [0; 8]);
        let hr = f(interop.as_raw(), &mut id);
        if failed(hr) {
            None
        } else {
            Some(id)
        }
    }

    fn count(effect: &ComPtrLike) -> u32 {
        unsafe {
            let interop = effect.query(&IID_IGRAPHICSEFFECTD2D1INTEROP).unwrap();
            let f: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT =
                ffi::method(interop.as_raw(), SLOT_INTEROP_GET_PROPERTY_COUNT);
            let mut n = 0u32;
            assert_eq!(f(interop.as_raw(), &mut n), S_OK);
            n
        }
    }

    fn mapping(effect: &ComPtrLike, name: &str) -> Option<(u32, i32)> {
        unsafe {
            let interop = effect.query(&IID_IGRAPHICSEFFECTD2D1INTEROP).unwrap();
            let wide: Vec<u16> = name.encode_utf16().chain(core::iter::once(0)).collect();
            let f: unsafe extern "system" fn(
                *mut c_void,
                *const u16,
                *mut u32,
                *mut i32,
            ) -> HRESULT = ffi::method(interop.as_raw(), SLOT_INTEROP_GET_NAMED_PROPERTY_MAPPING);
            let mut index = 0u32;
            let mut map = 0i32;
            let hr = f(interop.as_raw(), wide.as_ptr(), &mut index, &mut map);
            if failed(hr) {
                None
            } else {
                Some((index, map))
            }
        }
    }

    fn source_count(effect: &ComPtrLike) -> u32 {
        unsafe {
            let interop = effect.query(&IID_IGRAPHICSEFFECTD2D1INTEROP).unwrap();
            let f: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT =
                ffi::method(interop.as_raw(), SLOT_INTEROP_GET_SOURCE_COUNT);
            let mut n = 0u32;
            assert_eq!(f(interop.as_raw(), &mut n), S_OK);
            n
        }
    }

    #[test]
    fn qi_facades_are_distinct_and_stable() {
        unsafe {
            let effect = composite(D2D1_COMPOSITE_MODE_SOURCE_OVER);
            let raw = effect.as_raw();

            let identity = effect.query(&IID_IUNKNOWN).unwrap();
            assert_eq!(identity.as_raw(), raw);
            let inspectable = effect.query(&IID_IINSPECTABLE).unwrap();
            assert_eq!(inspectable.as_raw(), raw);

            let source = effect.query(&IID_IGRAPHICSEFFECTSOURCE).unwrap();
            assert_eq!(source.as_raw() as usize, raw as usize + 8);

            let interop = effect.query(&IID_IGRAPHICSEFFECTD2D1INTEROP).unwrap();
            assert_eq!(interop.as_raw() as usize, raw as usize + 16);

            assert!(effect.query(&IID_ISOLIDCOLORBRUSH).is_none());
        }
    }

    #[test]
    fn effect_ids_match_d2d_clsids() {
        unsafe {
            assert_eq!(call_effect_id(&composite(0)).unwrap(), CLSID_D2D1COMPOSITE);
            assert_eq!(
                call_effect_id(&flood(Color::new(0, 0, 0, 0))).unwrap(),
                CLSID_D2D1FLOOD
            );
            assert_eq!(call_effect_id(&border()).unwrap(), CLSID_D2D1BORDER);
            assert_eq!(
                call_effect_id(&gaussian_blur()).unwrap(),
                CLSID_D2D1GAUSSIANBLUR
            );
            assert_eq!(
                call_effect_id(&color_matrix()).unwrap(),
                CLSID_D2D1COLORMATRIX
            );
        }
    }

    #[test]
    fn property_counts_and_mappings_match_reference() {
        let composite_effect = composite(D2D1_COMPOSITE_MODE_SOURCE_OVER);
        assert_eq!(count(&composite_effect), 1);
        assert_eq!(
            mapping(&composite_effect, "Mode"),
            Some((
                D2D1_COMPOSITE_PROP_MODE,
                GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT
            ))
        );
        assert_eq!(mapping(&composite_effect, "Nope"), None);
        assert_eq!(source_count(&composite_effect), 0);

        let flood_effect = flood(Color::new(1, 2, 3, 4));
        assert_eq!(count(&flood_effect), 1);
        assert_eq!(
            mapping(&flood_effect, "Color"),
            Some((
                D2D1_FLOOD_PROP_COLOR,
                GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT
            ))
        );
        assert_eq!(source_count(&flood_effect), 0);

        let border_effect = border();
        assert_eq!(count(&border_effect), 2);
        assert_eq!(
            mapping(&border_effect, "ExtendX"),
            Some((D2D1_BORDER_PROP_EDGE_MODE_X, 1))
        );
        assert_eq!(
            mapping(&border_effect, "ExtendY"),
            Some((D2D1_BORDER_PROP_EDGE_MODE_Y, 1))
        );
        assert_eq!(source_count(&border_effect), 1);

        let blur = gaussian_blur();
        assert_eq!(count(&blur), 3);
        assert_eq!(
            mapping(&blur, "BlurAmount"),
            Some((D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION, 1))
        );
        assert_eq!(
            mapping(&blur, "Optimization"),
            Some((D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION, 1))
        );
        assert_eq!(
            mapping(&blur, "BorderMode"),
            Some((D2D1_GAUSSIANBLUR_PROP_BORDER_MODE, 1))
        );
        assert_eq!(source_count(&blur), 1);

        let matrix = color_matrix();
        assert_eq!(count(&matrix), 3);
        assert_eq!(
            mapping(&matrix, "ColorMatrix"),
            Some((D2D1_COLORMATRIX_PROP_COLOR_MATRIX, 1))
        );
        assert_eq!(
            mapping(&matrix, "AlphaMode"),
            Some((D2D1_COLORMATRIX_PROP_ALPHA_MODE, 1))
        );
        assert_eq!(
            mapping(&matrix, "ClampOutput"),
            Some((D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT, 1))
        );
    }

    #[test]
    fn name_round_trips_through_the_igfx_vtable() {
        unsafe {
            let effect = gaussian_blur();
            let name = effect.get_out::<*mut c_void>(SLOT_GRAPHICS_EFFECT_GET_NAME);
            let name = name.unwrap();
            assert_eq!(hstring_to_string(name), "GaussianBlurEffect");
            hstring_free(name);
        }
    }

    #[test]
    fn get_iids_and_runtime_class_name_match_cppwinrt() {
        unsafe {
            let effect = composite(0);
            let mut count = 0u32;
            let mut array: *mut Guid = ptr::null_mut();
            let f: unsafe extern "system" fn(*mut c_void, *mut u32, *mut *mut Guid) -> HRESULT =
                ffi::method(effect.as_raw(), 3);
            assert_eq!(f(effect.as_raw(), &mut count, &mut array), S_OK);
            assert_eq!(count, 2);
            let iids = core::slice::from_raw_parts(array, count as usize);
            assert_eq!(iids[0], IID_IGRAPHICSEFFECT);
            assert_eq!(iids[1], IID_IGRAPHICSEFFECTSOURCE);
            ffi::com_free(array as *mut c_void);

            let name = effect.get_out::<*mut c_void>(4).unwrap();
            assert_eq!(
                hstring_to_string(name),
                "Windows.Graphics.Effects.IGraphicsEffect"
            );
            hstring_free(name);
            let mut trust = -1i32;
            let g: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT =
                ffi::method(effect.as_raw(), 5);
            assert_eq!(g(effect.as_raw(), &mut trust), S_OK);
            assert_eq!(trust, 0);
        }
    }

    #[test]
    fn get_property_returns_ipropertyvalue() {
        unsafe {
            init_winrt();
            // Flood colour must be exposed as a 4-element Single array
            // (R, G, B, A in 0..1), reference 12912-12919.
            let effect = flood(Color::new(255, 51, 102, 153));
            let interop = effect.query(&IID_IGRAPHICSEFFECTD2D1INTEROP).unwrap();
            let f: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT =
                ffi::method(interop.as_raw(), SLOT_INTEROP_GET_PROPERTY);
            let mut value: *mut c_void = ptr::null_mut();
            let hr = f(interop.as_raw(), D2D1_FLOOD_PROP_COLOR, &mut value);
            assert_eq!(hr, S_OK);
            assert!(!value.is_null());
            // The ABI type is IPropertyValue, a different vtable from the
            // IInspectable* PropertyValue::Create* returns; getting a valid
            // pointer that QIs back to itself proves the QI step.
            let as_pv = query_interface(value, &IID_IPROPERTYVALUE).unwrap();
            assert_eq!(as_pv.as_raw(), value);
            let as_inspectable = query_interface(value, &IID_IINSPECTABLE).unwrap();
            assert!(!as_inspectable.is_null());
            release(value);
        }
    }

    #[test]
    fn source_parameter_graph_wiring() {
        unsafe {
            init_winrt();
            // `blurEffect->Source = CompositionEffectSourceParameter(L"backdrop")`
            let blur = gaussian_blur();
            let param = match source_parameter("backdrop") {
                Some(p) => p,
                None => {
                    // RoGetActivationFactory for composition classes can fail in
                    // a bare test process (no compositor); the live probe covers
                    // this path. Do not fail the suite for the environment.
                    eprintln!("source_parameter: activation unavailable in this process; skipped");
                    return;
                }
            };
            assert!(set_source_from_object(&blur, param.as_raw()).is_some());

            let interop = blur.query(&IID_IGRAPHICSEFFECTD2D1INTEROP).unwrap();
            let f: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT =
                ffi::method(interop.as_raw(), SLOT_INTEROP_GET_SOURCE);
            let mut source: *mut c_void = ptr::null_mut();
            assert_eq!(f(interop.as_raw(), 0, &mut source), S_OK);
            assert!(!source.is_null());
            // The stored source is the IGraphicsEffectSource face.
            let as_source = query_interface(source, &IID_IGRAPHICSEFFECTSOURCE).unwrap();
            assert_eq!(as_source.as_raw(), source);
            release(source);

            // Out-of-range and the Flood (no sources) case are E_BOUNDS.
            let mut none: *mut c_void = ptr::null_mut();
            assert_eq!(f(interop.as_raw(), 1, &mut none), E_BOUNDS);
            let flood_effect = flood(Color::new(0, 0, 0, 0));
            let flood_interop = flood_effect.query(&IID_IGRAPHICSEFFECTD2D1INTEROP).unwrap();
            let g: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT =
                ffi::method(flood_interop.as_raw(), SLOT_INTEROP_GET_SOURCE);
            assert_eq!(g(flood_interop.as_raw(), 0, &mut none), E_BOUNDS);
        }
    }

    #[test]
    fn composite_holds_source_references() {
        unsafe {
            let effect = gaussian_blur();
            let source = as_source(&effect).unwrap();
            let composite = composite(D2D1_COMPOSITE_MODE_SOURCE_OVER);
            assert!(push_composite_source(&composite, &source));
            assert_eq!(source_count(&composite), 1);
            let interop = composite.query(&IID_IGRAPHICSEFFECTD2D1INTEROP).unwrap();
            let f: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT =
                ffi::method(interop.as_raw(), SLOT_INTEROP_GET_SOURCE);
            let mut out: *mut c_void = ptr::null_mut();
            assert_eq!(f(interop.as_raw(), 0, &mut out), S_OK);
            // The composite must hand back the nested effect as an
            // IGraphicsEffect (the compositor recurses through it).
            let nested = query_interface(out, &IID_IGRAPHICSEFFECT).unwrap();
            assert!(!nested.is_null());
            release(out);
        }
    }
    #[test]
    fn refcount_balance() {
        unsafe {
            let effect = composite(0);
            let raw = effect.as_raw();
            let other = effect.query(&IID_IGRAPHICSEFFECT).unwrap();
            assert_eq!(other.as_raw(), raw);
            drop(other);
            drop(effect);
        }
    }
}
