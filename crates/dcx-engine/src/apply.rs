//! Applying theme rules to live XAML elements.
//!
//! Port of the reference's `ApplyCustomizations` value path:
//! * `GetStyleFromXamlSetters` 15466 — one `<Style>` per element lets XAML's own
//!   parser convert every value;
//! * `GetResolvedPropertyOverrides` 15543 — the resolved `(DependencyProperty,
//!   IInspectable)` pairs are read back through `Style.Setters`;
//! * `SetOrClearValue` 14981 — `DependencyObject::SetValue`;
//! * `ParseNonXamlPropertyOverrideValue` 15214 + `XamlBlurBrush` 12572 — values
//!   that are not XAML types (`<WindhawkBlur .../>`) are built into a custom
//!   composition brush by `dcx-blur`.
//!
//! The COM plumbing lives in `dcx-xaml` (hand-written WinRT bindings, absolute
//! vtable slots); this module only decides *what* to set, like the reference.

#![allow(non_snake_case)]

use core::ffi::c_void;
use std::sync::Mutex;

use dcx_xaml::com::{ComPtr, HRESULT};
use dcx_xaml::ids::IID_IVECTOR_COLUMNDEFINITION;
use dcx_xaml::xaml::{self, StyleSetter, StyleValue};

/// One property rule as handed over by `dcx-theme`.
pub struct Rule<'a> {
    pub property: &'a str,
    pub value: &'a str,
    /// `Property:=<XAML/>` — value is a XAML fragment, not an attribute string.
    pub is_xaml: bool,
}

#[derive(Default)]
pub struct ApplyStats {
    pub applied: usize,
    pub failed: usize,
    pub blurred: usize,
    pub blurred_failed: usize,
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Resolved `(DependencyProperty, value)` pairs, cached per
/// `(class_name, rule set)`. Resolving a rule set means running the XAML parser,
/// which costs milliseconds; the reference resolves once per element type and
/// keeps the result (`ResolvedRules`). We keep the COM references alive in the
/// cache and hand `add_ref` copies to each caller.
static RESOLVED: Mutex<Vec<(String, Vec<(usize, usize)>)>> = Mutex::new(Vec::new());

unsafe fn resolve_cached(
    class_name: &str,
    plain: &[StyleSetter<'_>],
) -> Option<Vec<(usize, usize)>> {
    let mut key = String::with_capacity(64);
    key.push_str(class_name);
    for setter in plain {
        key.push('\u{1}');
        key.push_str(setter.property);
        key.push('\u{2}');
        match setter.value {
            StyleValue::Literal(v) | StyleValue::Xaml(v) => key.push_str(v),
            StyleValue::Null => key.push_str("{x:Null}"),
        }
    }
    if let Ok(cache) = RESOLVED.lock() {
        if let Some((_, pairs)) = cache.iter().find(|(k, _)| *k == key) {
            let mut out = Vec::with_capacity(pairs.len());
            for (dp, value) in pairs {
                add_ref_raw(*dp as *mut c_void);
                add_ref_raw(*value as *mut c_void);
                out.push((*dp, *value));
            }
            return Some(out);
        }
    }
    let setters = xaml::resolve_style(class_name, plain)?;
    let mut pairs = Vec::with_capacity(setters.len());
    for setter in &setters {
        pairs.push((setter.dp.as_ptr() as usize, setter.value.as_ptr() as usize));
    }
    // The cache owns the resolved references; the caller gets its own.
    for (dp, value) in &pairs {
        add_ref_raw(*dp as *mut c_void);
        add_ref_raw(*value as *mut c_void);
    }
    if let Ok(mut cache) = RESOLVED.lock() {
        cache.push((key, pairs.clone()));
    }
    Some(pairs)
}

/// Apply one element's rules. `element` is the borrowed `IInspectable*` of the
/// element, `class_name` its concrete runtime class (used as `TargetType`).
pub unsafe fn apply_rules(
    element: *mut c_void,
    class_name: &str,
    rules: &[Rule<'_>],
) -> ApplyStats {
    let mut stats = ApplyStats::default();
    // `g_elementPropertyModifying`: writes from us must not re-enter the
    // property-changed callbacks we register below.
    let _modifying = crate::reapply::Suppress::begin();

    let mut plain: Vec<StyleSetter<'_>> = Vec::new();
    let mut blurs: Vec<(&str, &str)> = Vec::new();
    for rule in rules {
        if rule.value.contains("WindhawkBlur") {
            blurs.push((rule.property, rule.value));
        } else {
            plain.push(StyleSetter {
                property: rule.property,
                value: if rule.is_xaml {
                    StyleValue::Xaml(rule.value)
                } else {
                    StyleValue::Literal(rule.value)
                },
            });
        }
    }

    if !plain.is_empty() {
        match resolve_cached(class_name, &plain) {
            Some(pairs) => {
                for (dp, value) in pairs {
                    let hr = set_value(
                        ComPtr(element),
                        ComPtr(dp as *mut c_void),
                        ComPtr(value as *mut c_void),
                    );
                    // Drop the extra reference handed out by `resolve_cached`.
                    ComPtr(value as *mut c_void).release();
                    ComPtr(dp as *mut c_void).release();
                    if hr >= 0 {
                        stats.applied += 1;
                    } else {
                        stats.failed += 1;
                        crate::log(&format!(
                            "apply: {}: SetValue hr=0x{:08X}",
                            class_name, hr as u32
                        ));
                    }
                }
            }
            None => {
                stats.failed += rules.len();
                crate::log(&format!("apply: {}: style resolution failed", class_name));
            }
        }
    }

    for (property, value) in blurs {
        match build_blur_brush(element, value) {
            Some(brush) => {
                // What XAML thinks this object is, before and after the write.
                crate::log(&format!(
                    "apply: {class_name}.{property}: brush class={}",
                    xaml::class_name(brush)
                ));
                if let Some(dp) = resolve_dependency_property(class_name, property) {
                    let hr = xaml::dependency_object_set_value(ComPtr(element), dp, brush);
                    let stored = xaml::read_local_value_with_workaround(ComPtr(element), dp);
                    let stored_text = match stored {
                        Some(v) => {
                            let t = xaml::class_name(v);
                            v.release();
                            t
                        }
                        None => "<unset>".to_string(),
                    };
                    crate::log(&format!(
                        "apply: {class_name}.{property}: SetValue hr=0x{:08X} stored={stored_text}",
                        hr as u32
                    ));
                    // Did XAML ever connect the brush? Forcing our own
                    // OnConnected separates a broken effect graph from a brush
                    // XAML never wired up.
                    let (ch, published) = dcx_blur::connect_now(brush.0);
                    crate::log(&format!(
                        "apply: {class_name}.{property}: forced OnConnected hr=0x{:08X} published={published}",
                        ch as u32
                    ));
                    dp.release();
                    if hr >= 0 {
                        stats.applied += 1;
                        stats.blurred += 1;
                    } else {
                        stats.blurred_failed += 1;
                        crate::log(&format!(
                            "apply: {class_name}.{property}: blur SetValue hr=0x{:08X}",
                            hr as u32
                        ));
                    }
                } else {
                    stats.blurred_failed += 1;
                    crate::log(&format!(
                        "apply: {class_name}.{property}: no DependencyProperty for blur"
                    ));
                }
                brush.release();
            }
            None => {
                stats.blurred_failed += 1;
                crate::log(&format!(
                    "apply: {class_name}.{property}: WindhawkBlur create failed"
                ));
            }
        }
    }

    stats
}

/// `SetOrClearValue`'s definition-collection special case (reference 15124-15135).
///
/// Resolved values are cached and shared between passes and between elements,
/// but a `ColumnDefinitionCollection` owns `ColumnDefinition` DependencyObjects
/// into which the layout engine writes `ActualWidth`. Handing the same object to
/// two grids — one taskbar per monitor — leaks one monitor's column sizes onto
/// the other, and re-setting it on the *same* grid every pass re-enters measure
/// with a collection that is already its own. Give every `SetValue` a private
/// copy, exactly like the reference.
unsafe fn set_value(element: ComPtr, dp: ComPtr, value: ComPtr) -> HRESULT {
    // The reference's `value.try_as<ColumnDefinitionCollection>()`: that class's
    // default interface is `IVector<ColumnDefinition>`.
    let Some(columns) = value.query_interface(&IID_IVECTOR_COLUMNDEFINITION) else {
        let hr = xaml::dependency_object_set_value(element, dp, value);
        if hr >= 0 && !crate::reapply::watch(element.0, dp.0, value.0) {
            crate::log("apply: RegisterPropertyChangedCallback failed");
        }
        return hr;
    };
    let cloned = xaml::clone_column_definitions(columns);
    columns.release();
    let Some((collection, owner)) = cloned else {
        crate::log("apply: ColumnDefinitions clone failed; setting the shared collection");
        let hr = xaml::dependency_object_set_value(element, dp, value);
        if hr >= 0 && !crate::reapply::watch(element.0, dp.0, value.0) {
            crate::log("apply: RegisterPropertyChangedCallback failed");
        }
        return hr;
    };
    let hr = xaml::dependency_object_set_value(element, dp, collection);
    if hr >= 0 && !crate::reapply::watch(element.0, dp.0, collection.0) {
        // Watch the clone, since that is the collection the grid now owns.
        crate::log("apply: RegisterPropertyChangedCallback failed (column definitions)");
    }
    // The scratch grid owns the collection only until SetValue takes its own
    // reference, which has happened by now.
    collection.release();
    owner.release();
    hr
}

/// `GetStyleFromXamlSetters` with a single `{x:Null}` setter, used only to turn a
/// property name into its `DependencyProperty` — the reference does the same for
/// dynamic/capture rules (15564-15606).
unsafe fn resolve_dependency_property(class_name: &str, property: &str) -> Option<ComPtr> {
    let setters_xaml = format!(
        "<Setter Property=\"{}\" Value=\"{{x:Null}}\"/>",
        escape_xml(property)
    );
    let style = xaml::load_style_from_xaml_setters(class_name, &setters_xaml)?;
    let setter = xaml::setter_at(style, 0);
    let dp = setter.and_then(|s| {
        let dp = xaml::setter_property(s);
        s.release();
        dp
    });
    style.release();
    dp
}

struct BrushHost;

impl dcx_blur::BrushHost for BrushHost {
    fn compositor_for(&self, _element: u64) -> Option<dcx_blur::ComPtrLike> {
        // `dcx-blur` falls back to ElementCompositionPreview itself; nothing to
        // add here yet (the engine does not keep a Compositor around).
        None
    }

    fn resolve_theme_color(&self, _element: u64, _key: &str) -> Option<u32> {
        // `dcx-blur` resolves `{ThemeResource ...}` itself through the reference
        // proxy-SolidColorBrush path.
        None
    }

    fn log(&self, msg: &str) {
        crate::log(msg);
    }
}

/// Brush cache: one brush per (element, value). Re-creating a composable brush on
/// every apply pass would leak XAML objects and is a needless risk; the
/// reference keeps the brush alive in its per-element customization state.
static BRUSHES: Mutex<Vec<((usize, String), usize)>> = Mutex::new(Vec::new());
/// `#[link(name = "kernel32")]` style import of AddRef, to hand out a second ref.
unsafe fn add_ref_raw(ptr: *mut c_void) -> u32 {
    type Fn = unsafe extern "system" fn(*mut c_void) -> u32;
    let vtbl = *(ptr as *const *const *const c_void);
    let add_ref: Fn = core::mem::transmute(*vtbl.add(1));
    add_ref(ptr)
}

/// Build the reference `XamlBlurBrush` for one `<WindhawkBlur .../>` value.
unsafe fn build_blur_brush(element: *mut c_void, value: &str) -> Option<ComPtr> {
    let params = match dcx_blur::parse_windhawk_blur_strict(value) {
        Ok(Some(p)) => p,
        Ok(None) => {
            crate::log("apply: value is not a WindhawkBlur fragment");
            return None;
        }
        Err(e) => {
            crate::log(&format!("apply: WindhawkBlur parse error: {e}"));
            return None;
        }
    };
    let key = (element as usize, value.to_string());
    if let Ok(cache) = BRUSHES.lock() {
        if let Some((_, raw)) = cache.iter().find(|(k, _)| *k == key) {
            add_ref_raw(*raw as *mut c_void);
            return Some(ComPtr(*raw as *mut c_void));
        }
    }
    let brush = dcx_blur::create(
        &BrushHost,
        dcx_blur::ComPtrLike::from_raw_borrowed(element),
        params,
    )?;
    let raw = brush.into_raw();
    if let Ok(mut cache) = BRUSHES.lock() {
        cache.push((key, raw as usize));
    }
    add_ref_raw(raw);
    Some(ComPtr(raw))
}

// ---------------------------------------------------------------------------
// Live property reads used by captures and element matches
// ---------------------------------------------------------------------------

/// `FrameworkElement.Tag` as text (the theme captures `Tag=>taskbarDock`).
pub unsafe fn tag_string(element: *mut c_void) -> Option<String> {
    let tag = xaml::framework_element_tag(ComPtr(element))?;
    let text = match xaml::unbox_property_value(tag) {
        xaml::UnboxedValue::String(s) => Some(s),
        xaml::UnboxedValue::Bool(b) => Some(b.to_string()),
        xaml::UnboxedValue::Int(i) => Some(i.to_string()),
        xaml::UnboxedValue::UInt(u) => Some(u.to_string()),
        xaml::UnboxedValue::Double(d) => Some(format_double(d)),
        _ => None,
    };
    tag.release();
    text
}

/// `FrameworkElement.ActualWidth` (the theme captures `ActualWidth=>containerGridWidth`).
pub unsafe fn actual_width(element: *mut c_void) -> f64 {
    xaml::framework_element_actual_width(ComPtr(element))
}

/// `VisualStateManager.GetVisualStateGroups(element)` contains `group_name`.
pub unsafe fn has_visual_state_group(element: *mut c_void, group_name: &str) -> bool {
    xaml::has_visual_state_group(ComPtr(element), group_name)
}

/// Name of the group's current state, if any.
pub unsafe fn current_state_name(element: *mut c_void, group_name: &str) -> Option<String> {
    let group = xaml::visual_state_group_by_name(ComPtr(element), group_name)?;
    let name = xaml::visual_state_group_current_state_name(group);
    group.release();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Locale-independent double text, matching `FormatDoubleInvariant` (15795):
/// shortest round-trip representation, `NaN`/`Infinity` spellings.
pub fn format_double(d: f64) -> String {
    if d.is_nan() {
        return "NaN".to_string();
    }
    if d.is_infinite() {
        return if d < 0.0 { "-Infinity" } else { "Infinity" }.to_string();
    }
    let mut s = format!("{d}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

// ---------------------------------------------------------------------------
// Verification read-backs (diagnostics only)
// ---------------------------------------------------------------------------

/// Read a property's local value back as text, to prove a `SetValue` stuck.
pub unsafe fn read_back(
    element: *mut c_void,
    class_name: &str,
    properties: &[&str],
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for property in properties {
        let Some(dp) = resolve_dependency_property(class_name, property) else {
            out.push((property.to_string(), "<no dp>".to_string()));
            continue;
        };
        let value = xaml::read_local_value_with_workaround(ComPtr(element), dp);
        dp.release();
        let text = match value {
            Some(v) => {
                let unboxed = xaml::unbox_property_value(v);
                v.release();
                match unboxed {
                    xaml::UnboxedValue::String(s) => s,
                    xaml::UnboxedValue::Bool(b) => b.to_string(),
                    xaml::UnboxedValue::Int(i) => i.to_string(),
                    xaml::UnboxedValue::UInt(u) => u.to_string(),
                    xaml::UnboxedValue::Single(f) => format_double(f as f64),
                    xaml::UnboxedValue::Double(f) => format_double(f),
                    xaml::UnboxedValue::Char16(c) => format!("U+{c:04X}"),
                    other => format!("{other:?}"),
                }
            }
            None => "<unset>".to_string(),
        };
        out.push((property.to_string(), text));
    }
    out
}

/// `Grid.ColumnDefinitions` count (the split-layout rule).
pub unsafe fn column_definitions_count(element: *mut c_void) -> Option<u32> {
    let columns = xaml::grid_column_definitions(ComPtr(element))?;
    let count = xaml::vector_size(columns);
    columns.release();
    count
}
