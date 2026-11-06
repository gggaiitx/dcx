//! Port of the `WindhawkBlur` brush from the Windows 11 Taskbar Styler mod.
//!
//! Ground truth: `windows-11-taskbar-styler.wh.cpp`
//! * `XamlBlurBrush` (class 12572, ctor 13423, OnConnected 13735, OnDisconnected 13876)
//! * `CreateEffectBrush` (13752), `CreateFallbackBrush` (13747)
//! * effect classes `CompositeEffect` (12714), `FloodEffect` (12840),
//!   `GaussianBlurEffect` (13105), `ColorMatrixEffect` (13259), `BorderEffect` (12965)
//! * parameter parser `ParseNonXamlPropertyOverrideValue` (15214)
//!
//! The brush is a composable WinRT runtime class derived from
//! `Windows.UI.Xaml.Media.XamlCompositionBrushBase`: the outer object aggregates
//! an inner object created by
//! `IXamlCompositionBrushBaseFactory::CreateInstance`, implements
//! `IXamlCompositionBrushBaseOverrides` (OnConnected/OnDisconnected), forwards
//! `QueryInterface` for every other interface to the inner object, and hands a
//! `Windows.UI.Composition.CompositionBrush` back through
//! `IXamlCompositionBrushBaseProtected::put_CompositionBrush`.
//!
//! # Engine API
//!
//! ```no_run
//! use dcx_blur::{parse_windhawk_blur, create, BrushHost, ComPtrLike};
//!
//! # fn f(host: &dyn BrushHost, element_raw: *mut core::ffi::c_void) {
//! let params = parse_windhawk_blur(
//!     r#"<WindhawkBlur BlurAmount="5" TintColor="{ThemeResource SystemChromeMediumColor}" TintOpacity="0.7" />"#,
//! )
//! .expect("Glass constant");
//! let brush = unsafe {
//!     create(
//!         host,
//!         ComPtrLike::from_raw_borrowed(element_raw), // UIElement/IInspectable
//!         params,
//!     )
//! };
//! // `brush` is the IInspectable to pass to DependencyObject::SetValue for
//! // e.g. `Panel.BackgroundProperty`.
//! # }
//! ```
//!
//! `ComPtrLike` is a `#[repr(transparent)]` wrapper over `*mut c_void`:
//!
//! * `ComPtrLike::from_raw_owned(raw)` — take over one reference (move a
//!   `ComPtr.0` out of the engine).
//! * `ComPtrLike::from_raw_borrowed(raw)` — AddRef; use when the engine keeps
//!   its own reference.
//! * `as_raw()` — the ABI pointer (`IInspectable*`, `IUIElement*`, ...).
//! * `query(iid)` — owned QI result; `into_raw()` — give the reference away.
//! * `Drop` calls `Release` through the pointer's own vtable.
//!
//! # Live probe sequence (for the Lead)
//!
//! 0. Engine startup: `dcx_blur::set_log_hook(|msg| dcx_engine::log(msg));`
//!
//! 1. **Effects + composition graph only** (isolates the five D2D effect COM
//!    objects, `CreateEffectFactory`, `SetSourceParameter`):
//!    ```ignore
//!    let compositor = dcx_blur::probe_compositor_for_element(element_raw)?;
//!    let tint = dcx_blur::probe_xaml_theme_color("SystemChromeMediumColor")?;
//!    let brush = dcx_blur::probe_effect_graph(compositor, 5.0, tint, None, None)?;
//!    // success: log says "dcx-blur probe: effect graph built"; brush is a
//!    // CompositionEffectBrush (ICompositionEffectBrush) worth releasing.
//!    ```
//!
//! 2. **Composable class only** (isolates the aggregation/vtable work):
//!    ```ignore
//!    let params = dcx_blur::parse_windhawk_blur(glass).unwrap();
//!    let blur = dcx_blur::XamlBlurBrush::create(host, element_borrowed, params)?;
//!    blur.invoke_on_connected();            // runs CreateEffectBrush + put_CompositionBrush
//!    let b = blur.composition_brush();      // Some => the protected put worked
//!    ```
//!
//! 3. **Full path**: `SetValue(BackgroundProperty, brush)`; XAML calls
//!    `OnConnected` itself. Watch the log for
//!    `"dcx-blur: OnConnected: ..."`, `"dcx-blur: XamlBlurBrush created and
//!    aggregated..."` and `put_CompositionBrush`.
//!
//! # Known risks that must be verified live
//!
//! * Aggregation/vtable correctness is the main risk. The identity pointer is
//!   the `IXamlCompositionBrushBaseOverrides` facade (as in C++/WinRT);
//!   unknown IIDs are forwarded to the inner object's non-delegating QI. What
//!   must be confirmed live: XAML accepting the object as a `Brush` (it QIs for
//!   `IBrush`/`IDependencyObject`, which we forward), `OnConnected` being
//!   invoked, and `put_CompositionBrush` succeeding.
//! * `IWeakReferenceSource`/`IMarshal` are not implemented on the outer object
//!   (C++/WinRT provides them). If XAML turns out to need a weak reference to
//!   the brush, QI will fail gracefully; that is the first thing to add if the
//!   brush is accepted but never renders.
//! * The reference's proxy-`SolidColorBrush` resource insertion + change
//!   notifications are deferred; the `{ThemeResource ...}` tint is resolved
//!   once during `create` (same `XamlReader` call as the reference ctor).
//! * The `NoiseOpacity`/`NoiseDensity` overlay branch is not ported (stage 1);
//!   the theme that is being ported never sets it.

pub mod brush;
pub mod effects;
pub mod ffi;

pub use brush::{
    connect_now, create, parse_windhawk_blur, parse_windhawk_blur_strict,
    probe_compositor_for_element, probe_effect_graph, probe_xaml_theme_color, set_log_hook,
    BlurParams, BrushHost, XamlBlurBrush,
};
pub use ffi::{
    add_ref, query_interface, release, Color, ComPtrLike, Guid, HString, HRESULT, ULONG,
};
