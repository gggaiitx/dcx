//! `WindhawkBlur` ->`XamlBlurBrush`: the hand-rolled composable WinRT runtime
//! class, the `<WindhawkBlur .../>` parameter parser, and `create()`.
//!
//! # Reference mapping
//!
//! | this module | reference |
//! |---|---|
//! | `XamlBlurBrush` outer object | `class XamlBlurBrush : Media::XamlCompositionBrushBaseT<XamlBlurBrush>` (12572) |
//! | `parse_windhawk_blur_strict` | `ParseNonXamlPropertyOverrideValue` (15214) |
//! | `create` | `SetOrClearValue`'s `winrt::make<XamlBlurBrush>(...)` branch (14994) |
//! | `create_effect_brush` | `CreateEffectBrush` (13752) |
//! | `create_fallback_brush` | `CreateFallbackBrush` (13747) |
//! | `brush_on_connected` / `brush_on_disconnected` | `OnConnected` (13735) / `OnDisconnected` (13876) |
//! | `should_use_fallback` | `ShouldUseFallback` (13909) |
//!
//! # What C++/WinRT's `XamlCompositionBrushBaseT<D>` gives the reference
//! (and how it is reproduced here)
//!
//! * The outer object implements `IXamlCompositionBrushBaseOverrides`
//!   (method table: `OnConnected`, `OnDisconnected`), `IBrushOverrides2`, and
//!   `IInspectable`; its identity pointer is the `IXamlCompositionBrushBaseOverrides`
//!   facade (the first interface in `implements<...>`), like C++/WinRT.
//! * The ctor calls
//!   `IXamlCompositionBrushBaseFactory::CreateInstance(this, m_inner)` (slot 6)
//!   and stores the returned non-delegating inner object.
//! * `QueryInterface`: the two implemented interfaces are answered locally,
//!   every other IID is forwarded to the inner object's non-delegating QI
//!   (`root_implements::QueryInterface`).
//! * `IBrushOverrides2::PopulatePropertyInfoOverride` is not overridden by the
//!   reference: its `IBrushOverrides2T<D>` default implementation forwards to
//!   the inner object, so this object does too.
//! * `IXamlCompositionBrushBaseProtected` is also not implemented by the outer;
//!   `this->CompositionBrush()` / `CompositionBrush(brush)` in the reference
//!   compile to `try_as<IXamlCompositionBrushBaseProtected>()` = QI on the inner
//!   object, which is what `get_composition_brush` / `put_composition_brush` do.
//! * `GetRuntimeClassName` returns the default interface's name
//!   (`impl::runtime_class_name<implements_default_interface<D>>`), i.e.
//!   `"Windows.UI.Xaml.Media.IXamlCompositionBrushBaseOverrides"`.
//! * `GetIids` reports the outer's local IIDs followed by the inner object's.
//!
//! # Deferred (documented deviations, stage 1)
//!
//! * The reference inserts a proxy `SolidColorBrush` into the element's
//!   `ResourceDictionary` and re-creates the brush from the
//!   `Color`-changed callback / energy-saver / `UISettings.AdvancedEffectsEnabled`
//!   notifications. This port resolves the theme colour once, in [`create`],
//!   using the same `XamlReader.Load` call the reference makes in its ctor; the
//!   proxy/notification plumbing is not ported.
//! * The `NoiseOpacity`/`NoiseDensity` branch of `CreateEffectBrush` needs
//!   `RandomAccessStream` + `LoadedImageSurface`; it is not ported. If a rule
//!   asks for noise this port logs and skips the overlay. The stage-1 theme
//!   (`Glass`, reference lines 3574/6388/3769) never sets it. The correct
//!   no-noise path is what matters here and it *is* the reference path.
//! * `UISettings::AdvancedEffectsEnabled` (only consulted by
//!   `ShouldUseFallback` when a `FallbackColor` is configured) is not ported;
//!   see `ffi::energy_saver_active`.
//! * `IWeakReferenceSource` / `IMarshal` are not implemented on the outer
//!   object (C++/WinRT provides them). See the hand-over notes.

#![allow(non_snake_case)]

use crate::effects;
use crate::ffi;
use crate::ffi::*;
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::ptr;

// ---------------------------------------------------------------------------
// BlurParams
// ---------------------------------------------------------------------------

/// Parsed `<WindhawkBlur .../>` parameters, one-to-one with
/// `XamlBlurBrushParams` (reference 11589-11600).
///
/// `tint` is `0xAARRGGBB`; it already contains the alpha derived from
/// `TintOpacity` (like the reference applies it during parsing). The separate
/// [`BlurParams::tint_opacity`] is kept because the reference re-applies it
/// after resolving a `{ThemeResource}` tint (`RefreshThemeTint`, 13885-13897).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlurParams {
    /// `BlurAmount` (reference default: 0).
    pub blur_amount: f32,
    /// `TintColor` as `0xAARRGGBB`; alpha = 0 when neither `TintColor` nor
    /// `TintOpacity` was given.
    pub tint: u32,
    /// `TintOpacity` after clamping to `[0,1]`, stored as the resulting byte
    /// (`std::optional<uint8_t>` in the reference).
    pub tint_opacity: Option<u8>,
    /// `TintColor="{ThemeResource KEY}"`; empty when the tint is a literal.
    pub tint_theme_resource_key: String,
    /// `TintLuminosityOpacity`.
    pub tint_luminosity_opacity: Option<f32>,
    /// `TintSaturation`.
    pub tint_saturation: Option<f32>,
    /// `NoiseOpacity`.
    pub noise_opacity: Option<f32>,
    /// `NoiseDensity`.
    pub noise_density: Option<f32>,
    /// `FallbackColor` as `0xAARRGGBB`.
    pub fallback_color: Option<u32>,
    /// `FallbackColor="{ThemeResource KEY}"`; empty when not given.
    pub fallback_theme_resource_key: String,
}

impl BlurParams {
    /// Parsed tint as a WinRT colour.
    pub fn tint_color(&self) -> Color {
        Color::from_argb(self.tint)
    }
}

/// `<WindhawkBlur .../>` parser.
///
/// Returns `None` both when `value` is not a `WindhawkBlur` fragment and when
/// it is a malformed one; use [`parse_windhawk_blur_strict`] when the caller
/// wants to log the reference's `std::runtime_error` text.
pub fn parse_windhawk_blur(value: &str) -> Option<BlurParams> {
    parse_windhawk_blur_strict(value).ok().flatten()
}

/// `ParseNonXamlPropertyOverrideValue` (reference 15214-15464).
///
/// `Ok(None)` = the string is not a `WindhawkBlur` fragment (the reference
/// returns `std::nullopt`); `Err(msg)` = the reference would have thrown
/// `std::runtime_error(msg)`.
pub fn parse_windhawk_blur_strict(value: &str) -> Result<Option<BlurParams>, String> {
    let trimmed = value.trim();

    // `constexpr auto kWindhawkBlurPrefix = L"<WindhawkBlur "sv;`
    let Some(rest) = trimmed.strip_prefix("<WindhawkBlur ") else {
        return Ok(None);
    };
    // `constexpr auto kWindhawkBlurSuffix = L"/>"sv;`
    let Some(body) = rest.strip_suffix("/>") else {
        return Err("WindhawkBlur: Bad suffix".into());
    };

    let mut pending_tint_color_theme_resource = false;
    let mut pending_fallback_color_theme_resource = false;
    let mut tint_theme_resource_key = String::new();
    let mut fallback_theme_resource_key = String::new();
    let mut tint = Color::default();
    let mut fallback_color: Option<Color> = None;
    let mut tint_opacity = f32::NAN;
    let mut tint_luminosity_opacity = f32::NAN;
    let mut tint_saturation = f32::NAN;
    let mut noise_opacity = f32::NAN;
    let mut noise_density = f32::NAN;
    let mut blur_amount = 0.0f32;

    // The reference splits the body on single spaces and trims each token.
    for prop in body.split(' ') {
        let prop = prop.trim();
        if prop.is_empty() {
            continue;
        }

        if pending_tint_color_theme_resource {
            let Some(key) = prop.strip_suffix("}\"") else {
                return Err("WindhawkBlur: Invalid TintColor theme resource syntax".into());
            };
            pending_tint_color_theme_resource = false;
            tint_theme_resource_key = key.to_string();
            continue;
        }

        if pending_fallback_color_theme_resource {
            let Some(key) = prop.strip_suffix("}\"") else {
                return Err("WindhawkBlur: Invalid FallbackColor theme resource syntax".into());
            };
            pending_fallback_color_theme_resource = false;
            fallback_theme_resource_key = key.to_string();
            continue;
        }

        if prop == "TintColor=\"{ThemeResource" {
            pending_tint_color_theme_resource = true;
            continue;
        }

        if prop == "FallbackColor=\"{ThemeResource" {
            pending_fallback_color_theme_resource = true;
            continue;
        }

        // Colour literals: `TintColor="#AARRGGBB"` / `"#RRGGBB"`.
        if prop.starts_with("TintColor=\"#") && prop.ends_with('"') {
            tint = parse_color(prop.trim_start_matches("TintColor=\"#"), "TintColor")?;
            continue;
        }

        if prop.starts_with("FallbackColor=\"#") && prop.ends_with('"') {
            fallback_color = Some(parse_color(
                prop.trim_start_matches("FallbackColor=\"#"),
                "FallbackColor",
            )?);
            continue;
        }

        if let Some(raw) = quoted_property(prop, "TintOpacity") {
            tint_opacity = parse_f32(raw, "TintOpacity")?;
            continue;
        }

        if let Some(raw) = quoted_property(prop, "TintLuminosityOpacity") {
            tint_luminosity_opacity = parse_f32(raw, "TintLuminosityOpacity")?;
            continue;
        }

        if let Some(raw) = quoted_property(prop, "TintSaturation") {
            tint_saturation = parse_f32(raw, "TintSaturation")?;
            continue;
        }

        if let Some(raw) = quoted_property(prop, "NoiseOpacity") {
            noise_opacity = parse_f32(raw, "NoiseOpacity")?;
            continue;
        }

        if let Some(raw) = quoted_property(prop, "NoiseDensity") {
            noise_density = parse_f32(raw, "NoiseDensity")?;
            continue;
        }

        if let Some(raw) = quoted_property(prop, "BlurAmount") {
            blur_amount = parse_f32(raw, "BlurAmount")?;
            continue;
        }

        return Err("WindhawkBlur: Bad property".into());
    }

    if pending_tint_color_theme_resource {
        return Err("WindhawkBlur: Unterminated TintColor theme resource".into());
    }
    if pending_fallback_color_theme_resource {
        return Err("WindhawkBlur: Unterminated FallbackColor theme resource".into());
    }

    // if (!std::isnan(tintOpacity)) { clamp; tint.A = tintOpacity * 255.0f; }
    if !tint_opacity.is_nan() {
        let clamped = tint_opacity.clamp(0.0, 1.0);
        tint.a = (clamped * 255.0f32) as u8;
    }

    Ok(Some(BlurParams {
        blur_amount,
        tint: tint.to_argb(),
        tint_opacity: (!tint_opacity.is_nan()).then_some(tint.a),
        tint_theme_resource_key,
        tint_luminosity_opacity: (!tint_luminosity_opacity.is_nan())
            .then_some(tint_luminosity_opacity),
        tint_saturation: (!tint_saturation.is_nan()).then_some(tint_saturation),
        noise_opacity: (!noise_opacity.is_nan()).then_some(noise_opacity),
        noise_density: (!noise_density.is_nan()).then_some(noise_density),
        fallback_color: fallback_color.map(|c| c.to_argb()),
        fallback_theme_resource_key,
    }))
}

fn quoted_property<'a>(prop: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("{name}=\"");
    prop.strip_prefix(&prefix)?.strip_suffix('"')
}

fn parse_f32(raw: &str, name: &str) -> Result<f32, String> {
    raw.trim()
        .parse::<f32>()
        .map_err(|_| format!("WindhawkBlur: Unsupported {name} value"))
}

fn parse_color(hex: &str, name: &str) -> Result<Color, String> {
    // The reference keeps the trailing quote in the token; callers strip it.
    let hex = hex.trim_end_matches('"');
    let has_alpha = match hex.len() {
        6 => false,
        8 => true,
        _ => return Err(format!("WindhawkBlur: Unsupported {name} value")),
    };
    let value = u32::from_str_radix(hex, 16)
        .map_err(|_| format!("WindhawkBlur: Unsupported {name} value"))?;
    // Reference: A = hasAlpha ? HIBYTE(HIWORD(v)) : 255,
    //            R = LOBYTE(HIWORD(v)), G = HIBYTE(LOWORD(v)),
    //            B = LOBYTE(LOWORD(v)).
    let a = if has_alpha {
        ((value >> 24) & 0xFF) as u8
    } else {
        0xFF
    };
    Ok(Color::new(
        a,
        ((value >> 16) & 0xFF) as u8,
        ((value >> 8) & 0xFF) as u8,
        (value & 0xFF) as u8,
    ))
}

// ---------------------------------------------------------------------------
// BrushHost
// ---------------------------------------------------------------------------

/// Services the engine supplies to [`create`]; keeps this crate free of any
/// dependency on `dcx-xaml` / `dcx-engine`.
pub trait BrushHost {
    /// `ElementCompositionPreview.GetElementVisual(element).Compositor()`.
    ///
    /// `element` is the `Windows.UI.Xaml.UIElement*` ABI pointer (as `u64`)
    /// that `create` obtained by QI-ing the element it was given; valid for the
    /// duration of the `create` call. Return a `ComPtrLike` holding the
    /// **Compositor's default interface** (`ICompositor`); `create` QIs to
    /// `ICompositor` anyway, so another face of the same object also works.
    fn compositor_for(&self, element: u64) -> Option<ComPtrLike>;

    /// Resolve `{ThemeResource key}` to an ARGB colour (`0xAARRGGBB`).
    ///
    /// The reference resolves it by loading
    /// `<SolidColorBrush Color="{ThemeResource KEY}"/>` through `XamlReader`
    /// (ctor 13449-13472); this crate does that itself first and only asks the
    /// host when that fails, so a host may return `None`.
    fn resolve_theme_color(&self, element: u64, key: &str) -> Option<u32>;

    /// Diagnostic sink used while `create` runs. `OnConnected`/`OnDisconnected`
    /// run later and use [`set_log_hook`].
    fn log(&self, msg: &str);
}

/// Install the sink used by `OnConnected`/`OnDisconnected` and the probe
/// helpers (the engine passes a wrapper around `dcx_engine::log`).
pub fn set_log_hook(f: fn(&str)) {
    ffi::set_log_hook(f);
}

// ---------------------------------------------------------------------------
// Resolved state kept per brush instance
// ---------------------------------------------------------------------------

/// `XamlBlurBrush`'s member fields (reference 12599-12621), minus the deferred
/// notification plumbing.
struct ResolvedParams {
    blur_amount: f32,
    tint: Color,
    tint_opacity: Option<u8>,
    tint_luminosity_opacity: Option<f32>,
    tint_saturation: Option<f32>,
    noise_opacity: Option<f32>,
    noise_density: Option<f32>,
    fallback_color: Option<Color>,
    fallback_theme_resource_key: String,
}

struct BrushState {
    /// Set while the outer object is being torn down, so `Release` calls that
    /// the inner object delegates back to the controlling outer cannot recurse.
    destroying: bool,
    /// Non-delegating `IInspectable*` of the inner object; one reference owned
    /// by the outer (C++/WinRT's `m_inner`).
    inner: Option<ComPtrLike>,
    /// `m_compositor` (reference ctor 13434).
    compositor: Option<ComPtrLike>,
    params: ResolvedParams,
}

#[repr(C)]
pub(crate) struct OverridesVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> ULONG,
    release: unsafe extern "system" fn(*mut c_void) -> ULONG,
    get_iids: unsafe extern "system" fn(*mut c_void, *mut u32, *mut *mut Guid) -> HRESULT,
    get_runtime_class_name: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    get_trust_level: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT,
    on_connected: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    on_disconnected: unsafe extern "system" fn(*mut c_void) -> HRESULT,
}

#[repr(C)]
pub(crate) struct BrushOverrides2Vtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> ULONG,
    release: unsafe extern "system" fn(*mut c_void) -> ULONG,
    get_iids: unsafe extern "system" fn(*mut c_void, *mut u32, *mut *mut Guid) -> HRESULT,
    get_runtime_class_name: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    get_trust_level: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT,
    populate_property_info_override:
        unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> HRESULT,
}

#[repr(C, align(32))]
pub(crate) struct BrushOuter {
    /// Identity / `IXamlCompositionBrushBaseOverrides` facade, offset 0.
    v_overrides: *const OverridesVtbl,
    /// `IBrushOverrides2` facade, offset 8.
    v_brush_overrides: *const BrushOverrides2Vtbl,
    refs: Count,
    state: UnsafeCell<BrushState>,
}

impl BrushOuter {
    #[inline]
    fn overrides_facade(&self) -> *mut c_void {
        self as *const Self as *mut c_void
    }

    #[inline]
    fn brush_overrides_facade(&self) -> *mut c_void {
        unsafe { (self as *const Self as *mut u8).add(8) as *mut c_void }
    }
}

#[inline]
unsafe fn outer(this: *mut c_void) -> *mut BrushOuter {
    object_from_facade::<BrushOuter>(this)
}

#[inline]
unsafe fn state<'a>(o: *mut BrushOuter) -> &'a mut BrushState {
    &mut *(*o).state.get()
}

#[inline]
unsafe fn resolved<'a>(o: *mut BrushOuter) -> &'a ResolvedParams {
    &(*o).state.get().as_ref().unwrap().params
}

// ---------------------------------------------------------------------------
// IUnknown / IInspectable
// ---------------------------------------------------------------------------

unsafe extern "system" fn brush_qi(
    this: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    *out = ptr::null_mut();
    let iid = &*iid;
    let o = outer(this);

    // `implemented_interfaces<D>` = the interfaces passed to `implements<>`:
    // IXamlCompositionBrushBaseOverrides (default/identity) + IBrushOverrides2.
    if *iid == IID_IUNKNOWN
        || *iid == IID_IINSPECTABLE
        || *iid == IID_IAGILEOBJECT
        || *iid == IID_IXAMLCOMPOSITIONBRUSHBAASEOVERRIDES
    {
        let facade = (*o).overrides_facade();
        add_ref(facade);
        *out = facade;
        return S_OK;
    }
    if *iid == IID_IBRUSHOVERRIDES2 {
        let facade = (*o).brush_overrides_facade();
        add_ref(facade);
        *out = facade;
        return S_OK;
    }

    // `root_implements::QueryInterface`: anything else is forwarded to the
    // inner object's non-delegating QI.
    let inner = state(o).inner.as_ref().map(|c| c.as_raw());
    match inner {
        Some(inner) if !inner.is_null() => {
            let f: unsafe extern "system" fn(
                *mut c_void,
                *const Guid,
                *mut *mut c_void,
            ) -> HRESULT = method(inner, 0);
            f(inner, iid, out)
        }
        _ => E_NOINTERFACE,
    }
}

unsafe extern "system" fn brush_add_ref(this: *mut c_void) -> ULONG {
    let o = outer(this);
    if state(o).destroying {
        // Being destroyed: `Release` already ran to zero. Do not resurrect.
        return 1;
    }
    (*o).refs.add()
}

unsafe extern "system" fn brush_release(this: *mut c_void) -> ULONG {
    let o = outer(this);
    if state(o).destroying {
        return 0;
    }
    let remaining = (*o).refs.release();
    if remaining == 0 {
        // Destroy the inner/compositor references *before* freeing the
        // allocation; `destroying` keeps a delegating inner Release from
        // recursing into a freed object.
        state(o).destroying = true;
        drop(Box::from_raw(o));
    }
    remaining
}

unsafe extern "system" fn brush_get_iids(
    this: *mut c_void,
    count: *mut u32,
    array: *mut *mut Guid,
) -> HRESULT {
    if count.is_null() || array.is_null() {
        return E_POINTER;
    }
    let o = outer(this);
    let local = [
        IID_IXAMLCOMPOSITIONBRUSHBAASEOVERRIDES,
        IID_IBRUSHOVERRIDES2,
    ];
    let inner = state(o).inner.as_ref().map(|c| c.as_raw());
    let Some(inner) = inner.filter(|p| !p.is_null()) else {
        return match com_alloc_iids(&local) {
            Some((mem, n)) => {
                *count = n;
                *array = mem;
                S_OK
            }
            None => E_OUTOFMEMORY,
        };
    };

    // C++/WinRT composes: local iids followed by the inner object's iids.
    let mut inner_count: u32 = 0;
    let mut inner_array: *mut Guid = ptr::null_mut();
    let f: unsafe extern "system" fn(*mut c_void, *mut u32, *mut *mut Guid) -> HRESULT =
        method(inner, 3);
    let hr = f(inner, &mut inner_count, &mut inner_array);
    let mut all: Vec<Guid> = Vec::with_capacity(local.len() + inner_count as usize);
    all.extend_from_slice(&local);
    if succeeded(hr) && !inner_array.is_null() && inner_count > 0 {
        all.extend_from_slice(core::slice::from_raw_parts(
            inner_array,
            inner_count as usize,
        ));
    }
    com_free(inner_array as *mut c_void);
    match com_alloc_iids(&all) {
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

unsafe extern "system" fn brush_get_runtime_class_name(
    _this: *mut c_void,
    out: *mut *mut c_void,
) -> HRESULT {
    // C++/WinRT: impl::runtime_class_name<implements_default_interface<D>>.
    // For a composable outer the default interface is the first interface in
    // `implements<>`, i.e. IXamlCompositionBrushBaseOverrides.
    write_runtime_class_name(
        "Windows.UI.Xaml.Media.IXamlCompositionBrushBaseOverrides",
        out,
    )
}

unsafe extern "system" fn brush_get_trust_level(_this: *mut c_void, out: *mut i32) -> HRESULT {
    write_trust_level(out)
}

// ---------------------------------------------------------------------------
// IBrushOverrides2 (forwarded to the inner object, as C++/WinRT's default
// IBrushOverrides2T<D> does through `try_as<IBrushOverrides2>()`)
// ---------------------------------------------------------------------------

unsafe fn forward_populate_property_info(
    o: *mut BrushOuter,
    property_name: *mut c_void,
    animation_property_info: *mut c_void,
) -> HRESULT {
    let inner = state(o).inner.as_ref().map(|c| c.as_raw());
    let Some(inner) = inner.filter(|p| !p.is_null()) else {
        return E_FAIL;
    };
    let brush_overrides = match query_interface(inner, &IID_IBRUSHOVERRIDES2) {
        Some(p) => p,
        None => return E_NOINTERFACE,
    };
    method2_hr(
        brush_overrides.as_raw(),
        SLOT_BRUSH_POPULATE_PROPERTY_INFO,
        property_name,
        animation_property_info,
    )
}

unsafe extern "system" fn brush_populate_property_info_override(
    this: *mut c_void,
    property_name: *mut c_void,
    animation_property_info: *mut c_void,
) -> HRESULT {
    let o = outer(this);
    match guarded(|| forward_populate_property_info(o, property_name, animation_property_info)) {
        Ok(hr) => hr,
        Err(code) => {
            log(&format!(
                "dcx-blur: SEH {} in PopulatePropertyInfoOverride",
                hr_hex(code)
            ));
            E_FAIL
        }
    }
}

// ---------------------------------------------------------------------------
// IXamlCompositionBrushBaseOverrides
// ---------------------------------------------------------------------------

unsafe extern "system" fn brush_on_connected(this: *mut c_void) -> HRESULT {
    let o = outer(this);
    match guarded(|| on_connected_inner(o)) {
        Ok(hr) => hr,
        Err(code) => {
            log(&format!("dcx-blur: SEH {} in OnConnected", hr_hex(code)));
            E_FAIL
        }
    }
}

unsafe fn on_connected_inner(o: *mut BrushOuter) -> HRESULT {
    // if (!CompositionBrush()) { ... }
    if let Some(existing) = get_composition_brush(o) {
        drop(existing);
        log("dcx-blur: OnConnected: CompositionBrush already set, skipping");
        return S_OK;
    }

    // RefreshThemeTint()/RefreshFallbackColor() already ran in `create` (this
    // port resolves them eagerly; the deferred proxy-brush notifications are
    // documented in the module header).
    let p = resolved(o);
    log(&format!(
        "dcx-blur: OnConnected: blur={} tint=#{:08X} tintOpacity={:?} lum={:?} sat={:?} noise={:?}/{:?} fallback={:?}",
        p.blur_amount,
        p.tint.to_argb(),
        p.tint_opacity,
        p.tint_luminosity_opacity,
        p.tint_saturation,
        p.noise_opacity,
        p.noise_density,
        p.fallback_color.map(|c| c.to_argb())
    ));
    let use_fallback = should_use_fallback(o);
    let brush = if use_fallback {
        log("dcx-blur: OnConnected: energy saver/advanced effects -> fallback brush");
        create_fallback_brush(o)
    } else {
        create_effect_brush(o)
    };
    let Some(brush) = brush else {
        log("dcx-blur: OnConnected: failed to create the composition brush");
        return E_FAIL;
    };
    let composition_brush = match brush.query(&IID_ICOMPOSITIONBRUSH) {
        Some(b) => b,
        None => {
            log("dcx-blur: OnConnected: composition brush has no ICompositionBrush");
            return E_FAIL;
        }
    };
    put_composition_brush(o, Some(&composition_brush))
}

unsafe extern "system" fn brush_on_disconnected(this: *mut c_void) -> HRESULT {
    let o = outer(this);
    match guarded(|| on_disconnected_inner(o)) {
        Ok(hr) => hr,
        Err(code) => {
            log(&format!("dcx-blur: SEH {} in OnDisconnected", hr_hex(code)));
            E_FAIL
        }
    }
}

unsafe fn on_disconnected_inner(o: *mut BrushOuter) -> HRESULT {
    // Reference 13876-13883:
    //   if (auto brush = CompositionBrush()) { brush.Close(); CompositionBrush(nullptr); }
    if let Some(brush) = get_composition_brush(o) {
        if let Some(closable) = brush.query(&IID_ICLOSABLE) {
            let hr = method0_hr(closable.as_raw(), SLOT_CLOSABLE_CLOSE);
            if failed(hr) {
                log(&format!(
                    "dcx-blur: OnDisconnected: Close() -> {}",
                    hr_hex(hr)
                ));
            }
        }
        let hr = put_composition_brush(o, None);
        if failed(hr) {
            return hr;
        }
        log("dcx-blur: OnDisconnected: composition brush closed");
    }
    S_OK
}

// ---------------------------------------------------------------------------
// IXamlCompositionBrushBaseProtected helpers (reference: `CompositionBrush()`)
// ---------------------------------------------------------------------------

unsafe fn protected_interface(o: *mut BrushOuter) -> Option<ComPtrLike> {
    let inner = state(o).inner.as_ref()?.as_raw();
    query_interface(inner, &IID_IXAMLCOMPOSITIONBRUSHBAASEPROTECTED)
}

unsafe fn get_composition_brush(o: *mut BrushOuter) -> Option<ComPtrLike> {
    let prot = protected_interface(o)?;
    out0_object(prot.as_raw(), SLOT_BRUSH_PROTECTED_GET)
}

unsafe fn put_composition_brush(o: *mut BrushOuter, brush: Option<&ComPtrLike>) -> HRESULT {
    let Some(prot) = protected_interface(o) else {
        log("dcx-blur: inner object has no IXamlCompositionBrushBaseProtected");
        return E_FAIL;
    };
    let raw = brush.map(|b| b.as_raw()).unwrap_or(ptr::null_mut());
    let hr = method1_hr(prot.as_raw(), SLOT_BRUSH_PROTECTED_PUT, raw);
    if failed(hr) {
        log(&format!("dcx-blur: put_CompositionBrush -> {}", hr_hex(hr)));
    }
    hr
}

// ---------------------------------------------------------------------------
// ShouldUseFallback (reference 13909-13965) -stage-1 reachable part
// ---------------------------------------------------------------------------

unsafe fn should_use_fallback(o: *mut BrushOuter) -> bool {
    let p = resolved(o);
    // if (!m_fallbackColor && m_fallbackThemeResourceKey.empty()) return false;
    if p.fallback_color.is_none() && p.fallback_theme_resource_key.is_empty() {
        return false;
    }
    // energySaverActive || advancedEffectsOff
    energy_saver_active()
}

// ---------------------------------------------------------------------------
// CreateFallbackBrush (reference 13747)
// ---------------------------------------------------------------------------

unsafe fn create_fallback_brush(o: *mut BrushOuter) -> Option<ComPtrLike> {
    let p = resolved(o);
    let color = p.fallback_color.unwrap_or(p.tint);
    // `m_compositor.CreateColorBrush(m_fallbackColor.value_or(m_tint))`
    let compositor = state(o).compositor.as_ref()?.as_raw();
    let mut out: *mut c_void = ptr::null_mut();
    let f: unsafe extern "system" fn(*mut c_void, Color, *mut *mut c_void) -> HRESULT =
        method(compositor, SLOT_COMPOSITOR_CREATE_COLOR_BRUSH);
    let hr = f(compositor, color, &mut out);
    if failed(hr) || out.is_null() {
        log(&format!("dcx-blur: CreateColorBrush -> {}", hr_hex(hr)));
        return None;
    }
    Some(ComPtrLike::from_raw_owned(out))
}

// ---------------------------------------------------------------------------
// CreateEffectBrush (reference 13752-13874)
// ---------------------------------------------------------------------------

/// Luma coefficients, reference 13757-13759.
const LUMA_R: f32 = 0.2126;
const LUMA_G: f32 = 0.7152;
const LUMA_B: f32 = 0.0722;

unsafe fn create_effect_brush(o: *mut BrushOuter) -> Option<ComPtrLike> {
    let p = resolved(o);
    let compositor = state(o).compositor.as_ref()?.query(&IID_ICOMPOSITOR)?;
    create_effect_brush_with(&compositor, p)
}

/// The graph itself, independent of the brush object so the engine probe can
/// exercise it with a compositor of its own.
unsafe fn create_effect_brush_with(
    compositor: &ComPtrLike,
    p: &ResolvedParams,
) -> Option<ComPtrLike> {
    // auto backdropBrush = m_compositor.CreateBackdropBrush();
    let compositor2 = compositor.query(&IID_ICOMPOSITOR2)?;
    let backdrop = out0_object(compositor2.as_raw(), SLOT_COMPOSITOR2_CREATE_BACKDROP_BRUSH)?;

    // 1. Blur: GaussianBlurEffect with Source = CompositionEffectSourceParameter("backdrop").
    let backdrop_param = effects::source_parameter("backdrop")?;
    let blur = effects::gaussian_blur();
    effects::set_source_from_object(&blur, backdrop_param.as_raw())?;
    effects::set_gaussian_blur_amount(&blur, p.blur_amount);
    effects::set_name(&blur, "BlurEffect");
    drop(backdrop_param);

    let mut top_of_stack = effects::as_source(&blur)?;

    // 2. Saturation (optional), reference 13770-13787.
    if let Some(saturation) = p.tint_saturation {
        if saturation != 1.0 {
            let s = saturation.max(0.0);
            let inv_s = 1.0 - s;
            let mut matrix = [0.0f32; 20];
            matrix[0] = inv_s * LUMA_R + s;
            matrix[1] = inv_s * LUMA_R;
            matrix[2] = inv_s * LUMA_R;
            matrix[3] = 0.0;
            matrix[4] = inv_s * LUMA_G;
            matrix[5] = inv_s * LUMA_G + s;
            matrix[6] = inv_s * LUMA_G;
            matrix[7] = 0.0;
            matrix[8] = inv_s * LUMA_B;
            matrix[9] = inv_s * LUMA_B;
            matrix[10] = inv_s * LUMA_B + s;
            matrix[11] = 0.0;
            matrix[12] = 0.0;
            matrix[13] = 0.0;
            matrix[14] = 0.0;
            matrix[15] = 1.0;

            let sat_matrix = effects::color_matrix();
            effects::set_source_interface(&sat_matrix, &top_of_stack);
            effects::set_color_matrix(&sat_matrix, &matrix);
            effects::set_name(&sat_matrix, "SaturationEffect");
            top_of_stack = effects::as_source(&sat_matrix)?;
        }
    }

    // 3. Luminosity (optional), reference 13791-13811.
    if let Some(luminosity_opacity) = p.tint_luminosity_opacity {
        if luminosity_opacity > 0.0 {
            let op = luminosity_opacity.clamp(0.0, 1.0);
            let tint_lum = (p.tint.r as f32 / 255.0) * LUMA_R
                + (p.tint.g as f32 / 255.0) * LUMA_G
                + (p.tint.b as f32 / 255.0) * LUMA_B;

            let mut matrix = [0.0f32; 20];
            matrix[0] = 1.0 - (LUMA_R * op);
            matrix[1] = -(LUMA_R * op);
            matrix[2] = -(LUMA_R * op);
            matrix[3] = 0.0;
            matrix[4] = -(LUMA_G * op);
            matrix[5] = 1.0 - (LUMA_G * op);
            matrix[6] = -(LUMA_G * op);
            matrix[7] = 0.0;
            matrix[8] = -(LUMA_B * op);
            matrix[9] = -(LUMA_B * op);
            matrix[10] = 1.0 - (LUMA_B * op);
            matrix[11] = 0.0;
            matrix[12] = 0.0;
            matrix[13] = 0.0;
            matrix[14] = 0.0;
            matrix[15] = 1.0;
            matrix[16] = tint_lum * op;
            matrix[17] = tint_lum * op;
            matrix[18] = tint_lum * op;
            matrix[19] = 0.0;

            let lum_matrix = effects::color_matrix();
            effects::set_source_interface(&lum_matrix, &top_of_stack);
            effects::set_color_matrix(&lum_matrix, &matrix);
            effects::set_name(&lum_matrix, "LuminosityBlend");
            top_of_stack = effects::as_source(&lum_matrix)?;
        }
    }

    // 4. Noise overlay (optional): NOT PORTED for stage 1.
    //    Reference 13813-13850 builds a tiled noise surface via
    //    RandomAccessStream + LoadedImageSurface + BorderEffect + ColorMatrix
    //    + CompositeEffect. Keep the branch point, log, and continue with the
    //    exact no-noise path.
    if let Some(noise_opacity) = p.noise_opacity {
        if noise_opacity > 0.0 {
            log(&format!(
                "dcx-blur: NoiseOpacity={noise_opacity} NoiseDensity={:?} requested but the noise overlay is not ported (stage 1); continuing without it",
                p.noise_density
            ));
        }
    }

    // 5. Tint: FloodEffect(tint) composited source-over over the stack,
    //    reference 13852-13860.
    let flood = effects::flood(p.tint);
    effects::set_name(&flood, "FloodEffect");
    let flood_source = effects::as_source(&flood)?;

    let composite = effects::composite(effects::D2D1_COMPOSITE_MODE_SOURCE_OVER);
    effects::push_composite_source(&composite, &top_of_stack);
    effects::push_composite_source(&composite, &flood_source);

    // auto factory = m_compositor.CreateEffectFactory(*compositeEffect);
    let factory = out1_object(
        compositor.as_raw(),
        SLOT_COMPOSITOR_CREATE_EFFECT_FACTORY,
        composite.as_raw(),
    )?;
    if let Some(status) = get_out::<i32>(factory.as_raw(), SLOT_EFFECT_FACTORY_GET_LOAD_STATUS) {
        // 0 = Success, 1 = EffectTooComplex, 2 = Pending, -1 = Other.
        if status != 0 {
            log(&format!(
                "dcx-blur: CompositionEffectFactory LoadStatus = {status}"
            ));
        }
    }

    // auto brush = factory.CreateBrush();
    let brush = out0_object(factory.as_raw(), SLOT_EFFECT_FACTORY_CREATE_BRUSH)?;

    // brush.SetSourceParameter(L"backdrop", backdropBrush);
    let parameter_name = HString::create("backdrop")?;
    let backdrop_brush = backdrop.query(&IID_ICOMPOSITIONBRUSH)?;
    let hr = method2_hr(
        brush.as_raw(),
        SLOT_EFFECT_BRUSH_SET_SOURCE_PARAMETER,
        parameter_name.as_raw(),
        backdrop_brush.as_raw(),
    );
    if failed(hr) {
        log(&format!(
            "dcx-blur: SetSourceParameter(backdrop) -> {}",
            hr_hex(hr)
        ));
        return None;
    }

    Some(brush)
}

// ---------------------------------------------------------------------------
// Vtables
// ---------------------------------------------------------------------------

static OVERRIDES_VTBL: OverridesVtbl = OverridesVtbl {
    query_interface: brush_qi,
    add_ref: brush_add_ref,
    release: brush_release,
    get_iids: brush_get_iids,
    get_runtime_class_name: brush_get_runtime_class_name,
    get_trust_level: brush_get_trust_level,
    on_connected: brush_on_connected,
    on_disconnected: brush_on_disconnected,
};

static BRUSH_OVERRIDES_VTBL: BrushOverrides2Vtbl = BrushOverrides2Vtbl {
    query_interface: brush_qi,
    add_ref: brush_add_ref,
    release: brush_release,
    get_iids: brush_get_iids,
    get_runtime_class_name: brush_get_runtime_class_name,
    get_trust_level: brush_get_trust_level,
    populate_property_info_override: brush_populate_property_info_override,
};

// ---------------------------------------------------------------------------
// create()
// ---------------------------------------------------------------------------

/// Construct a `XamlBlurBrush` for `element`.
///
/// Returns the brush's `IInspectable` (identity pointer), ready to hand to
/// `DependencyObject::SetValue` as the value of a `Brush` dependency property.
///
/// Ownership: one reference is returned. `element` is consumed (the caller
/// should pass `ComPtrLike::from_raw_borrowed(ptr)` when it keeps its own
/// reference to the element).
///
/// # Safety
/// Must be called on the XAML UI thread inside the target process. Wraps the
/// fallible native calls in the SEH guard.
pub unsafe fn create(
    host: &dyn BrushHost,
    element: ComPtrLike,
    params: BlurParams,
) -> Option<ComPtrLike> {
    match guarded(|| create_inner(host, element, params)) {
        Ok(result) => result,
        Err(code) => {
            host.log(&format!("dcx-blur: SEH {} during create", hr_hex(code)));
            None
        }
    }
}

unsafe fn create_inner(
    host: &dyn BrushHost,
    element: ComPtrLike,
    params: BlurParams,
) -> Option<ComPtrLike> {
    // Reference `SetOrClearValue`: `if (auto uiElement = elementDo.try_as<UIElement>())`
    let ui_element = match element.query(&IID_IUIELEMENT) {
        Some(e) => e,
        None => {
            host.log("dcx-blur: element is not a UIElement (try_as<UIElement> failed)");
            return None;
        }
    };
    let element_raw = ui_element.as_raw() as u64;
    let is_framework_element = element.query(&IID_IFRAMEWORKELEMENT).is_some();

    // m_compositor = ElementCompositionPreview::GetElementVisual(element).Compositor()
    let compositor = match host.compositor_for(element_raw) {
        Some(c) => c,
        None => {
            host.log(
                "dcx-blur: host supplied no compositor; falling back to ElementCompositionPreview",
            );
            match ffi::element_compositor(ui_element.as_raw()) {
                Some(c) => c,
                None => {
                    host.log("dcx-blur: ElementCompositionPreview failed; cannot create brush");
                    return None;
                }
            }
        }
    };
    // Normalise to the Compositor's default interface (ICompositor).
    let compositor = match guarded(|| compositor.query(&IID_ICOMPOSITOR)) {
        Ok(Some(c)) => c,
        _ => {
            host.log("dcx-blur: QI(ICompositor) on the supplied compositor failed; using it as-is");
            compositor
        }
    };

    // --- ctor: createProxy(tint theme resource) + RefreshThemeTint ----------
    let mut tint = Color::from_argb(params.tint);
    let mut fallback_color = params.fallback_color.map(Color::from_argb);

    if !params.tint_theme_resource_key.is_empty() {
        if !is_framework_element {
            // Reference: `auto fe = element.try_as<FrameworkElement>(); if (!fe) return nullptr;`
            host.log(
                "dcx-blur: try_as<FrameworkElement>() failed; tint ThemeResource not resolved",
            );
        } else if let Some(color) =
            resolve_theme_color(host, element_raw, &params.tint_theme_resource_key)
        {
            // RefreshThemeTint(): m_tint = proxyBrush.Color(); if (m_tintOpacity) m_tint.A = ...
            tint = color;
            if let Some(opacity) = params.tint_opacity {
                tint.a = opacity;
            }
            host.log(&format!(
                "dcx-blur: tint ThemeResource {} -> #{:08X}",
                params.tint_theme_resource_key,
                tint.to_argb()
            ));
        } else {
            host.log(&format!(
                "dcx-blur: failed to resolve tint ThemeResource {}; keeping parsed tint #{:08X}",
                params.tint_theme_resource_key,
                tint.to_argb()
            ));
        }
    }

    if !params.fallback_theme_resource_key.is_empty() && is_framework_element {
        // RefreshFallbackColor(): m_fallbackColor = fallbackProxyBrush.Color().
        if let Some(color) =
            resolve_theme_color(host, element_raw, &params.fallback_theme_resource_key)
        {
            fallback_color = Some(color);
            host.log(&format!(
                "dcx-blur: fallback ThemeResource {} -> #{:08X}",
                params.fallback_theme_resource_key,
                color.to_argb()
            ));
        }
    }

    let resolved = ResolvedParams {
        blur_amount: params.blur_amount,
        tint,
        tint_opacity: params.tint_opacity,
        tint_luminosity_opacity: params.tint_luminosity_opacity,
        tint_saturation: params.tint_saturation,
        noise_opacity: params.noise_opacity,
        noise_density: params.noise_density,
        fallback_color,
        fallback_theme_resource_key: params.fallback_theme_resource_key,
    };
    host.log(&format!(
        "dcx-blur: XamlBlurBrush ctor: blur={} tint=#{:08X} tintOpacity={:?} fallback={:?}",
        resolved.blur_amount,
        resolved.tint.to_argb(),
        resolved.tint_opacity,
        resolved.fallback_color.map(|c| c.to_argb())
    ));

    // --- outer object -------------------------------------------------------
    let boxed = Box::new(BrushOuter {
        v_overrides: &OVERRIDES_VTBL,
        v_brush_overrides: &BRUSH_OVERRIDES_VTBL,
        refs: Count::new(1),
        state: UnsafeCell::new(BrushState {
            destroying: false,
            inner: None,
            compositor: Some(compositor),
            params: resolved,
        }),
    });
    let outer_raw = Box::into_raw(boxed);
    let identity = (*outer_raw).overrides_facade();

    // XamlCompositionBrushBase factory: `IXamlCompositionBrushBaseFactory::CreateInstance`
    let factory = match activation_factory(
        "Windows.UI.Xaml.Media.XamlCompositionBrushBase",
        &IID_IXAMLCOMPOSITIONBRUSHBAASEFACTORY,
    ) {
        Some(f) => f,
        None => {
            host.log("dcx-blur: XamlCompositionBrushBase activation factory unavailable");
            release(identity);
            return None;
        }
    };

    let created = finish_create(host, outer_raw, identity, &factory);
    drop(factory);
    if !created {
        host.log("dcx-blur: IXamlCompositionBrushBaseFactory::CreateInstance failed");
        release(identity);
        return None;
    }

    host.log("dcx-blur: XamlBlurBrush created and aggregated with its inner object");

    Some(ComPtrLike::from_raw_owned(identity))
}

unsafe fn resolve_theme_color(host: &dyn BrushHost, element_raw: u64, key: &str) -> Option<Color> {
    // Primary: the reference's own XamlReader probe (ctor 13449-13472).
    if let Ok(Some(color)) = guarded(|| ffi::xaml_theme_color(key)) {
        return Some(color);
    }
    // Fallback: let the engine resolve it with its own bindings.
    host.resolve_theme_color(element_raw, key)
        .map(Color::from_argb)
}

unsafe fn finish_create(
    host: &dyn BrushHost,
    outer_raw: *mut BrushOuter,
    identity: *mut c_void,
    factory: &ComPtrLike,
) -> bool {
    let mut inner_out: *mut c_void = ptr::null_mut();
    let mut value_out: *mut c_void = ptr::null_mut();
    let inner_slot: *mut *mut c_void = &mut inner_out;
    let value_slot: *mut *mut c_void = &mut value_out;
    let factory_raw = factory.as_raw();
    let result = guarded(move || {
        method3_hr::<*mut c_void, *mut *mut c_void, *mut *mut c_void>(
            factory_raw,
            SLOT_BRUSH_FACTORY_CREATE_INSTANCE,
            identity,
            inner_slot,
            value_slot,
        )
    });
    let hr = match result {
        Ok(hr) => hr,
        Err(code) => {
            host.log(&format!("dcx-blur: SEH {} in CreateInstance", hr_hex(code)));
            return false;
        }
    };

    // Keep the inner object's reference *before* releasing the factory's
    // `value` retval (which is a separate reference to the new instance).
    if !inner_out.is_null() {
        state(outer_raw).inner = Some(ComPtrLike::from_raw_owned(inner_out));
    }
    if !value_out.is_null() {
        release(value_out);
    }

    if failed(hr) {
        host.log(&format!("dcx-blur: CreateInstance -> {}", hr_hex(hr)));
        return false;
    }
    if state(outer_raw).inner.is_none() {
        host.log("dcx-blur: CreateInstance returned no inner object");
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// XamlBlurBrush handle
// ---------------------------------------------------------------------------

/// Handle to a created brush instance (the composable outer object).
///
/// `create` hands the raw `IInspectable` to the engine for `SetValue`; this
/// wrapper is the same object with typed probe helpers.
pub struct XamlBlurBrush {
    ptr: ComPtrLike,
}

impl XamlBlurBrush {
    /// See [`create`].
    ///
    /// # Safety
    /// Same contract as [`create`].
    pub unsafe fn create(
        host: &dyn BrushHost,
        element: ComPtrLike,
        params: BlurParams,
    ) -> Option<Self> {
        create(host, element, params).map(|ptr| Self { ptr })
    }

    /// The `IInspectable` to pass to `SetValue` (borrowed).
    pub fn as_inspectable(&self) -> *mut c_void {
        self.ptr.as_raw()
    }

    /// Take ownership of the reference (for `SetValue`).
    pub fn into_inspectable(self) -> ComPtrLike {
        self.ptr
    }

    /// Probe only: read the published composition brush (null before
    /// `OnConnected` ran).
    ///
    /// # Safety
    /// The object must still be alive (it is a COM object held by `self`).
    pub unsafe fn composition_brush(&self) -> Option<ComPtrLike> {
        let o = outer(self.ptr.as_raw());
        get_composition_brush(o)
    }

    /// Probe only: run `OnConnected`'s body directly, without waiting for XAML
    /// to connect the brush. XAML's later `OnConnected` sees the published
    /// brush and is a no-op, so this is safe for a live diagnostic.
    ///
    /// # Safety
    /// UI thread; brush must be alive.
    pub unsafe fn invoke_on_connected(&self) -> HRESULT {
        brush_on_connected(self.ptr.as_raw())
    }
}

// ---------------------------------------------------------------------------
// Probe helpers for the live engine test
// ---------------------------------------------------------------------------

/// Run the brush's own `OnConnected` (overrides vtable slot 6) without waiting
/// for XAML to call it, and report whether a composition brush got published.
///
/// Used to separate "our effect graph is broken" from "XAML never connected the
/// brush". XAML's later `OnConnected` sees the published brush and is a no-op.
///
/// # Safety
/// `identity` must be a live brush identity from [`create`], on the UI thread.
pub unsafe fn connect_now(identity: *mut c_void) -> (HRESULT, bool) {
    let hr = brush_on_connected(identity);
    let published = match get_composition_brush(outer(identity)) {
        Some(brush) => {
            drop(brush);
            true
        }
        None => false,
    };
    (hr, published)
}

/// Resolve `<SolidColorBrush Color="{ThemeResource KEY}"/>` through
/// `XamlReader` (the reference's `createProxy` path), returning `0xAARRGGBB`.
///
/// # Safety
/// UI thread inside the XAML process.
pub unsafe fn probe_xaml_theme_color(key: &str) -> Option<u32> {
    match guarded(|| ffi::xaml_theme_color(key)) {
        Ok(color) => color.map(|c| c.to_argb()),
        Err(code) => {
            log(&format!(
                "dcx-blur probe: SEH {} in XamlReader tint resolution",
                hr_hex(code)
            ));
            None
        }
    }
}

/// Probe helper: obtain the `Compositor` for an element through
/// `ElementCompositionPreview.GetElementVisual(element)`, without needing a
/// `BrushHost` implementation.
///
/// `element` is any interface pointer of the element; this crate QIs it to
/// `IUIElement` first. The result is the Compositor's default interface
/// (`ICompositor`), ready for [`probe_effect_graph`].
///
/// # Safety
/// UI thread inside the XAML process; `element` must be a live element.
pub unsafe fn probe_compositor_for_element(element: *mut c_void) -> Option<ComPtrLike> {
    let ui_element = query_interface(element, &IID_IUIELEMENT)?;
    let result = guarded(|| ffi::element_compositor(ui_element.as_raw()));
    match result {
        Ok(compositor) => compositor.map(|c| c.query(&IID_ICOMPOSITOR).unwrap_or(c)),
        Err(code) => {
            log(&format!(
                "dcx-blur probe: SEH {} in ElementCompositionPreview::GetElementVisual",
                hr_hex(code)
            ));
            None
        }
    }
}

/// Build the `CreateEffectBrush` graph standalone against a compositor the
/// engine supplies, returning the `CompositionEffectBrush` (default interface
/// `ICompositionEffectBrush`).
///
/// This isolates the effect COM objects + `CreateEffectFactory` +
/// `SetSourceParameter` from the composable-class aggregation, which is the
/// other half of the risk. `tint` must already be resolved (`0xAARRGGBB`).
///
/// # Safety
/// UI thread; `compositor` must be the element's compositor.
pub unsafe fn probe_effect_graph(
    compositor: ComPtrLike,
    blur_amount: f32,
    tint: u32,
    tint_luminosity_opacity: Option<f32>,
    tint_saturation: Option<f32>,
) -> Option<ComPtrLike> {
    let compositor = compositor.query(&IID_ICOMPOSITOR).unwrap_or(compositor);
    let params = ResolvedParams {
        blur_amount,
        tint: Color::from_argb(tint),
        tint_opacity: None,
        tint_luminosity_opacity,
        tint_saturation,
        noise_opacity: None,
        noise_density: None,
        fallback_color: None,
        fallback_theme_resource_key: String::new(),
    };
    match guarded(|| create_effect_brush_with(&compositor, &params)) {
        Ok(brush) => {
            log(if brush.is_some() {
                "dcx-blur probe: effect graph built"
            } else {
                "dcx-blur probe: effect graph build FAILED"
            });
            brush
        }
        Err(code) => {
            log(&format!(
                "dcx-blur probe: SEH {} building effect graph",
                hr_hex(code)
            ));
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Tests: ParseNonXamlPropertyOverrideValue parity (pure parsing, no COM)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_glass_exactly() {
        // Literal theme string, reference line 3574.
        let p = parse_windhawk_blur(
            r##"<WindhawkBlur BlurAmount="5" TintColor="{ThemeResource SystemChromeMediumColor}" TintOpacity="0.7" />"##,
        )
        .expect("theme glass must parse");
        assert_eq!(p.blur_amount, 5.0);
        assert_eq!(p.tint_theme_resource_key, "SystemChromeMediumColor");
        assert_eq!(p.tint_opacity, Some(178)); // 0.7 * 255 = 178.5 -> 178
        assert_eq!(p.tint, 0xB200_0000); // alpha applied, RGB still zero until resolved
        assert_eq!(p.tint_luminosity_opacity, None);
        assert_eq!(p.tint_saturation, None);
        assert_eq!(p.noise_opacity, None);
        assert_eq!(p.noise_density, None);
        assert_eq!(p.fallback_color, None);
        assert!(p.fallback_theme_resource_key.is_empty());
    }

    #[test]
    fn parses_literal_hex_colours() {
        let p = parse_windhawk_blur(r##"<WindhawkBlur BlurAmount="10" TintColor="#FFFF0000"/>"##)
            .unwrap();
        assert_eq!(p.tint, 0xFFFF_0000);
        assert_eq!(p.tint_opacity, None);

        // 6 digits -> implicit opaque alpha.
        let p = parse_windhawk_blur(r##"<WindhawkBlur BlurAmount="1" TintColor="#2a2e32" />"##)
            .unwrap();
        assert_eq!(p.tint, 0xFF2A_2E32);
    }

    #[test]
    fn parses_all_optional_properties() {
        let p = parse_windhawk_blur(
            r##"<WindhawkBlur BlurAmount="30" TintColor="{ThemeResource CardStrokeColorDefaultSolid}" TintOpacity="0.0" TintLuminosityOpacity="1.0" TintSaturation="1.0" NoiseDensity="1.0" NoiseOpacity="0.1" FallbackColor="#80000000" />"##,
        )
        .unwrap();
        assert_eq!(p.blur_amount, 30.0);
        assert_eq!(p.tint_theme_resource_key, "CardStrokeColorDefaultSolid");
        assert_eq!(p.tint_opacity, Some(0));
        assert_eq!(p.tint_luminosity_opacity, Some(1.0));
        assert_eq!(p.tint_saturation, Some(1.0));
        assert_eq!(p.noise_density, Some(1.0));
        assert_eq!(p.noise_opacity, Some(0.1));
        assert_eq!(p.fallback_color, Some(0x8000_0000));

        // Both a literal and a theme fallback colour may be present.
        let p = parse_windhawk_blur(
            r##"<WindhawkBlur BlurAmount="1" FallbackColor="#11223344" FallbackColor="{ThemeResource SystemAltLowColor}" />"##,
        )
        .unwrap();
        assert_eq!(p.fallback_color, Some(0x1122_3344));
        assert_eq!(p.fallback_theme_resource_key, "SystemAltLowColor");
    }

    #[test]
    fn clamps_tint_opacity_like_the_reference() {
        let p = parse_windhawk_blur(r##"<WindhawkBlur TintOpacity="2.0" />"##).unwrap();
        assert_eq!(p.tint_opacity, Some(255));
        assert_eq!(p.tint, 0xFF00_0000);

        let p = parse_windhawk_blur(r##"<WindhawkBlur TintOpacity="-1" />"##).unwrap();
        assert_eq!(p.tint_opacity, Some(0));
        assert_eq!(p.tint, 0x0000_0000);
    }

    #[test]
    fn not_a_blur_fragment_is_none() {
        assert!(parse_windhawk_blur("").is_none());
        assert!(parse_windhawk_blur(r##"<LinearGradientBrush />"##).is_none());
        assert_eq!(
            parse_windhawk_blur_strict(r##"<LinearGradientBrush />"##),
            Ok(None)
        );
    }

    #[test]
    fn malformed_fragments_report_reference_errors() {
        assert_eq!(
            parse_windhawk_blur_strict(r##"<WindhawkBlur BlurAmount="5""##),
            Err("WindhawkBlur: Bad suffix".to_string())
        );
        assert_eq!(
            parse_windhawk_blur_strict(r##"<WindhawkBlur BlurAmount="5" Nope="1"/>"##),
            Err("WindhawkBlur: Bad property".to_string())
        );
        assert_eq!(
            parse_windhawk_blur_strict(r##"<WindhawkBlur TintColor="#12"/>"##),
            Err("WindhawkBlur: Unsupported TintColor value".to_string())
        );
        assert_eq!(
            parse_windhawk_blur_strict(r##"<WindhawkBlur TintColor="{ThemeResource Foo"/>"##),
            Err("WindhawkBlur: Invalid TintColor theme resource syntax".to_string())
        );
        assert_eq!(
            parse_windhawk_blur_strict(r##"<WindhawkBlur TintColor="{ThemeResource />"##),
            Err("WindhawkBlur: Unterminated TintColor theme resource".to_string())
        );
        assert_eq!(
            parse_windhawk_blur_strict(r##"<WindhawkBlur FallbackColor="{ThemeResource />"##),
            Err("WindhawkBlur: Unterminated FallbackColor theme resource".to_string())
        );
        assert_eq!(
            parse_windhawk_blur_strict(r##"<WindhawkBlur BlurAmount="not-a-number"/>"##),
            Err("WindhawkBlur: Unsupported BlurAmount value".to_string())
        );

        // Errors must not be visible through the lenient entry point.
        assert!(parse_windhawk_blur(r##"<WindhawkBlur Nope="1"/>"##).is_none());
    }

    #[test]
    fn tolerates_extra_whitespace_like_split_string_view() {
        let p =
            parse_windhawk_blur(r##"  <WindhawkBlur  BlurAmount="5"   TintColor="#00FF00"  />  "##)
                .unwrap();
        assert_eq!(p.blur_amount, 5.0);
        assert_eq!(p.tint, 0xFF00_FF00);
    }

    #[test]
    fn default_params_are_reference_defaults() {
        let p = parse_windhawk_blur(r##"<WindhawkBlur />"##).unwrap();
        assert_eq!(p.blur_amount, 0.0);
        assert_eq!(p.tint, 0);
        assert_eq!(p.tint_opacity, None);
        assert!(p.tint_theme_resource_key.is_empty());
    }

    // -----------------------------------------------------------------
    // Outer-object ABI, without the OS-created inner (that needs a XAML
    // host; the live probe covers the aggregation itself).
    // -----------------------------------------------------------------

    unsafe fn bare_outer() -> *mut BrushOuter {
        let boxed = Box::new(BrushOuter {
            v_overrides: &OVERRIDES_VTBL,
            v_brush_overrides: &BRUSH_OVERRIDES_VTBL,
            refs: Count::new(1),
            state: UnsafeCell::new(BrushState {
                destroying: false,
                inner: None,
                compositor: None,
                params: ResolvedParams {
                    blur_amount: 5.0,
                    tint: Color::new(0xB2, 0x1F, 0x1F, 0x1F),
                    tint_opacity: Some(0xB2),
                    tint_luminosity_opacity: None,
                    tint_saturation: None,
                    noise_opacity: None,
                    noise_density: None,
                    fallback_color: None,
                    fallback_theme_resource_key: String::new(),
                },
            }),
        });
        Box::into_raw(boxed)
    }

    #[test]
    fn outer_qi_serves_its_own_interfaces_and_forwards_the_rest() {
        unsafe {
            let o = bare_outer();
            let identity = (*o).overrides_facade();
            let brush_overrides = (*o).brush_overrides_facade();
            assert_eq!(brush_overrides as usize, identity as usize + 8);

            // Identity + IXamlCompositionBrushBaseOverrides + IInspectable all
            // resolve to the same facade (C++/WinRT default interface).
            for iid in [
                IID_IUNKNOWN,
                IID_IINSPECTABLE,
                IID_IXAMLCOMPOSITIONBRUSHBAASEOVERRIDES,
            ] {
                let mut out: *mut c_void = ptr::null_mut();
                let f: unsafe extern "system" fn(
                    *mut c_void,
                    *const Guid,
                    *mut *mut c_void,
                ) -> HRESULT = method(identity, 0);
                assert_eq!(f(identity, &iid as *const Guid, &mut out), S_OK);
                assert_eq!(out, identity);
                release(out);
            }
            let mut out: *mut c_void = ptr::null_mut();
            let f: unsafe extern "system" fn(
                *mut c_void,
                *const Guid,
                *mut *mut c_void,
            ) -> HRESULT = method(identity, 0);
            assert_eq!(
                f(identity, &IID_IBRUSHOVERRIDES2 as *const Guid, &mut out),
                S_OK
            );
            assert_eq!(out, brush_overrides);
            release(out);

            // No inner yet: forwarded QIs fail cleanly (this is the state the
            // factory sees while it constructs the inner object).
            assert_eq!(
                f(identity, &IID_ICOMPOSITIONBRUSH as *const Guid, &mut out),
                E_NOINTERFACE
            );
            assert!(out.is_null());

            // Runtime class name = default interface name (C++/WinRT).
            let name = get_out::<*mut c_void>(identity, 4).unwrap();
            assert_eq!(
                hstring_to_string(name),
                "Windows.UI.Xaml.Media.IXamlCompositionBrushBaseOverrides"
            );
            hstring_free(name);

            // GetIids: the two local interfaces.
            let mut count = 0u32;
            let mut array: *mut Guid = ptr::null_mut();
            let g: unsafe extern "system" fn(*mut c_void, *mut u32, *mut *mut Guid) -> HRESULT =
                method(identity, 3);
            assert_eq!(g(identity, &mut count, &mut array), S_OK);
            assert_eq!(count, 2);
            let iids = core::slice::from_raw_parts(array, count as usize);
            assert_eq!(iids[0], IID_IXAMLCOMPOSITIONBRUSHBAASEOVERRIDES);
            assert_eq!(iids[1], IID_IBRUSHOVERRIDES2);
            com_free(array as *mut c_void);

            // Refcount: one owned reference; dropping it destroys the object.
            assert_eq!(add_ref(identity), 2);
            assert_eq!(release(identity), 1);
            release(identity);
        }
    }

    #[test]
    fn on_connected_without_inner_fails_gracefully() {
        unsafe {
            let o = bare_outer();
            let identity = (*o).overrides_facade();
            // No inner object -> no IXamlCompositionBrushBaseProtected -> the
            // reference's `CompositionBrush()` getter yields null, so
            // OnConnected tries to build a brush and fails (no compositor):
            // it must return an HRESULT, not crash.
            let on_connected: unsafe extern "system" fn(*mut c_void) -> HRESULT =
                method(identity, 6);
            assert!(failed(on_connected(identity)));
            let on_disconnected: unsafe extern "system" fn(*mut c_void) -> HRESULT =
                method(identity, 7);
            assert_eq!(on_disconnected(identity), S_OK);
            release(identity);
        }
    }
}
