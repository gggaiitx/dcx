//! The XAML level the engine talks to.
//!
//! Everything here is a transcription of a C++/WinRT call site in
//! `windows-11-taskbar-styler.wh.cpp`, not a new design:
//!
//! | function | reference |
//! |---|---|
//! | [`build_style_document`] / [`load_style`] | `GetStyleFromXamlSetters` 15466 |
//! | [`parse_xaml_value`] | `ParseXamlValue` 19419 |
//! | [`resolve_style`] | `GetResolvedPropertyOverrides` 15543 |
//! | [`dependency_object_set_value`] / [`dependency_object_clear_value`] | `SetOrClearValue` 14981 |
//! | [`read_local_value_with_workaround`] | `ReadLocalValueWithWorkaround` 12441 |
//! | [`unbox_property_value`] | `TryUnboxPropertyValue` 15848 |
//! | [`has_visual_state_group`] | `GetVisualStateGroup` 15755 |
//! | [`clone_column_definitions`] | `SetOrClearValue` 15114-15148 |
//!
//! Every call goes through [`crate::com::guarded`], so a structured exception is
//! returned as `Err(code)` instead of unwinding into Rust and killing explorer.
//!
//! Slots are absolute and cited from `docs/winrt-abi.md` (see [`crate::ids`] for
//! the IIDs). Reminder of the layout, because it is the bug this crate was born
//! fixing: `0 QueryInterface 1 AddRef 2 Release 3 GetIids 4 GetRuntimeClassName
//! 5 GetTrustLevel 6.. interface methods`.

#![allow(non_snake_case)]

use crate::com::{
    activate_instance, guarded, hr_hex, raw_hstring_to_string, ro_get_activation_factory, ComPtr,
    Guid, HString, E_FAIL, E_POINTER, HRESULT, S_OK,
};
use crate::ids::*;
use core::ffi::c_void;
use core::mem;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

// ---------------------------------------------------------------------------
// Absolute vtable slots, all from docs/winrt-abi.md
// ---------------------------------------------------------------------------

/// `Windows.UI.Xaml.Markup.IXamlReaderStatics` slot 6: `Load`.
const SLOT_XAMLREADER_LOAD: usize = 6;

/// `Windows.UI.Xaml.IStyle` slot 7: `get_Setters`.
const SLOT_STYLE_GET_SETTERS: usize = 7;

/// `Windows.UI.Xaml.ISetter` slot 6: `get_Property`; slot 8: `get_Value`.
const SLOT_SETTER_GET_PROPERTY: usize = 6;
const SLOT_SETTER_GET_VALUE: usize = 8;

/// `Windows.UI.Xaml.IDependencyObject`: 7 `SetValue`, 8 `ClearValue`,
/// 9 `ReadLocalValue`, 10 `GetAnimationBaseValue`.
const SLOT_DO_SET_VALUE: usize = 7;
const SLOT_DO_CLEAR_VALUE: usize = 8;
const SLOT_DO_READ_LOCAL_VALUE: usize = 9;
const SLOT_DO_GET_ANIMATION_BASE_VALUE: usize = 10;

/// `Windows.UI.Xaml.IDependencyObject2`: 6 `RegisterPropertyChangedCallback`,
/// 7 `UnregisterPropertyChangedCallback`.
const SLOT_DO2_REGISTER_CALLBACK: usize = 6;
const SLOT_DO2_UNREGISTER_CALLBACK: usize = 7;

/// `Windows.UI.Xaml.IFrameworkElement`: 9 `get_Tag`, 13 `get_ActualWidth`,
/// 33 `get_Name`, 38 `get_Style`, 39 `put_Style`.
const SLOT_FE_GET_TAG: usize = 9;
const SLOT_FE_GET_ACTUAL_WIDTH: usize = 13;
const SLOT_FE_GET_NAME: usize = 33;
const SLOT_FE_GET_STYLE: usize = 38;
const SLOT_FE_PUT_STYLE: usize = 39;

/// `Windows.UI.Xaml.IUIElement` slot 22: `put_Visibility`.
const SLOT_UIE_PUT_VISIBILITY: usize = 22;

/// `Windows.UI.Xaml.Controls.IGridFactory` slot 6: `CreateInstance`.
const SLOT_IGRIDFACTORY_CREATE_INSTANCE: usize = 6;

/// `Windows.UI.Xaml.Controls.IGrid` slot 7: `get_ColumnDefinitions`.
const SLOT_GRID_GET_COLUMN_DEFINITIONS: usize = 7;

/// `Windows.UI.Xaml.Controls.IColumnDefinition`: 6 `get_Width`, 7 `put_Width`,
/// 8 `get_MaxWidth`, 9 `put_MaxWidth`, 10 `get_MinWidth`, 11 `put_MinWidth`.
const SLOT_COLDEF_GET_WIDTH: usize = 6;
const SLOT_COLDEF_PUT_WIDTH: usize = 7;
const SLOT_COLDEF_GET_MAX_WIDTH: usize = 8;
const SLOT_COLDEF_PUT_MAX_WIDTH: usize = 9;
const SLOT_COLDEF_GET_MIN_WIDTH: usize = 10;
const SLOT_COLDEF_PUT_MIN_WIDTH: usize = 11;

/// `Windows.UI.Xaml.IVisualStateManagerStatics` slot 6: `GetVisualStateGroups`.
const SLOT_VSM_GET_VISUAL_STATE_GROUPS: usize = 6;

/// `Windows.UI.Xaml.IVisualStateGroup`: 6 `get_Name`, 8 `get_States`,
/// 9 `get_CurrentState`.
const SLOT_VSG_GET_NAME: usize = 6;
const SLOT_VSG_GET_STATES: usize = 8;
const SLOT_VSG_GET_CURRENT_STATE: usize = 9;

/// `Windows.UI.Xaml.IVisualState` slot 6: `get_Name`.
const SLOT_VS_GET_NAME: usize = 6;

/// `Windows.UI.Xaml.Media.IVisualTreeHelperStatics`: 10 `GetChild`,
/// 11 `GetChildrenCount`, 12 `GetParent`.
const SLOT_VTH_GET_CHILD: usize = 10;
const SLOT_VTH_GET_CHILDREN_COUNT: usize = 11;
const SLOT_VTH_GET_PARENT: usize = 12;

/// `Windows.UI.Xaml.IDependencyPropertyStatics` slot 6: `get_UnsetValue`.
const SLOT_DPS_GET_UNSET_VALUE: usize = 6;

/// `Windows.Foundation.Collections.IVector`1`: 6 `GetAt`, 7 `get_Size`,
/// 13 `Append`.
const SLOT_IVECTOR_GET_AT: usize = 6;
const SLOT_IVECTOR_GET_SIZE: usize = 7;
const SLOT_IVECTOR_APPEND: usize = 13;

/// `Windows.Foundation.Collections.IIterable`1` slot 6: `First`.
const SLOT_IITERABLE_FIRST: usize = 6;

/// `Windows.Foundation.Collections.IIterator`1`: 6 `get_Current`,
/// 8 `MoveNext`.
const SLOT_IITERATOR_GET_CURRENT: usize = 6;
const SLOT_IITERATOR_MOVE_NEXT: usize = 8;

/// `Windows.Foundation.Collections.IKeyValuePair`2` slot 7: `get_Value`.
const SLOT_KVP_GET_VALUE: usize = 7;

/// `Windows.Foundation.Collections.IReference`1` slot 6: `get_Value`.
const SLOT_IREFERENCE_GET_VALUE: usize = 6;

/// `Windows.Foundation.IPropertyValue` — `get_Type` is slot 6, the typed getters
/// follow in declaration order (docs/winrt-abi.md).
const SLOT_PV_GET_TYPE: usize = 6;
const SLOT_PV_GET_UINT8: usize = 8;
const SLOT_PV_GET_INT16: usize = 9;
const SLOT_PV_GET_UINT16: usize = 10;
const SLOT_PV_GET_INT32: usize = 11;
const SLOT_PV_GET_UINT32: usize = 12;
const SLOT_PV_GET_INT64: usize = 13;
const SLOT_PV_GET_UINT64: usize = 14;
const SLOT_PV_GET_SINGLE: usize = 15;
const SLOT_PV_GET_DOUBLE: usize = 16;
const SLOT_PV_GET_CHAR16: usize = 17;
const SLOT_PV_GET_BOOLEAN: usize = 18;
const SLOT_PV_GET_STRING: usize = 19;

/// `Windows.Foundation.PropertyType` values (`metadump --enum
/// Windows.Foundation.PropertyType` on Windows.Foundation.winmd).
const PT_UINT8: i32 = 1;
const PT_INT16: i32 = 2;
const PT_UINT16: i32 = 3;
const PT_INT32: i32 = 4;
const PT_UINT32: i32 = 5;
const PT_INT64: i32 = 6;
const PT_UINT64: i32 = 7;
const PT_SINGLE: i32 = 8;
const PT_DOUBLE: i32 = 9;
const PT_CHAR16: i32 = 10;
const PT_BOOLEAN: i32 = 11;
const PT_STRING: i32 = 12;
const PT_OTHER_TYPE: i32 = 20;

/// `Windows.UI.Xaml.Visibility`.
pub const VISIBILITY_VISIBLE: i32 = 0;
pub const VISIBILITY_COLLAPSED: i32 = 1;

/// `Windows.UI.Xaml.GridUnitType`.
pub const GRID_UNIT_AUTO: i32 = 0;
pub const GRID_UNIT_PIXEL: i32 = 1;
pub const GRID_UNIT_STAR: i32 = 2;

/// Presentation XAML namespace, as in the reference document template.
pub const XAML_NS: &str = "http://schemas.microsoft.com/winfx/2006/xaml/presentation";

// ---------------------------------------------------------------------------
// Value types crossing the ABI
// ---------------------------------------------------------------------------

/// `struct_Windows_UI_Xaml_GridLength` (winrt/impl/Windows.UI.Xaml.0.h line
/// 5898): `{ double Value; int32_t GridUnitType; }`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GridLength {
    pub Value: f64,
    pub GridUnitType: i32,
}

/// A boxed primitive, as produced by [`unbox_property_value`].
///
/// Mirrors `UnboxedPropertyValue` (reference 15832); the engine maps this onto
/// `dcx-theme`'s `PropValue`.
#[derive(Clone, Debug, PartialEq)]
pub enum UnboxedValue {
    /// `nullptr` property value.
    Null,
    String(String),
    Bool(bool),
    Char16(u16),
    Int(i64),
    UInt(u64),
    Single(f32),
    Double(f64),
    /// Opaque value (brush, thickness, …) or a boxed type we do not unbox.
    Other,
}

impl UnboxedValue {
    /// Numeric view, for the `[Prop=Value]` matcher (all integer/float ranks
    /// compare as one, like the reference's `std::variant` comparison does not —
    /// but the values it meets in practice are same-typed).
    pub fn numeric(&self) -> Option<f64> {
        match self {
            UnboxedValue::Char16(v) => Some(*v as f64),
            UnboxedValue::Int(v) => Some(*v as f64),
            UnboxedValue::UInt(v) => Some(*v as f64),
            UnboxedValue::Single(v) => Some(*v as f64),
            UnboxedValue::Double(v) => Some(*v),
            _ => None,
        }
    }
}

/// One `<Setter>` for the synthetic `<Style>`.
///
/// `Property="X" Value="text"` vs `Property="X"><Setter.Value>…</Setter.Value>`
/// mirrors the `=`/`:=` split of the reference's rule parser (`ParseRule`
/// 19075): [`StyleValue::Literal`] is an attribute, [`StyleValue::Xaml`] is a
/// XAML fragment, [`StyleValue::Null`] is the `{x:Null}` placeholder used for
/// dynamic and capture rules.
#[derive(Clone, Copy, Debug)]
pub enum StyleValue<'a> {
    Literal(&'a str),
    Xaml(&'a str),
    Null,
}

/// One setter of a style document.
#[derive(Clone, Copy, Debug)]
pub struct StyleSetter<'a> {
    pub property: &'a str,
    pub value: StyleValue<'a>,
}

/// A `(DependencyProperty, value)` pair read back out of a loaded `<Style>`.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedSetter {
    /// `DependencyProperty` interface pointer (owned).
    pub dp: ComPtr,
    /// `ISetter::get_Value` boxed value (owned).
    pub value: ComPtr,
}

/// Outcome of one ABI call: the `HRESULT` plus any structured exception the SEH
/// guard caught (`seh == 0` when none was raised).
#[derive(Clone, Copy, Debug, Default)]
pub struct Step {
    pub hr: HRESULT,
    pub seh: u32,
}

impl Step {
    pub fn ok(&self) -> bool {
        self.seh == 0 && self.hr == S_OK
    }

    pub fn describe(&self) -> String {
        if self.seh != 0 {
            format!("SEH 0x{:08X}", self.seh)
        } else {
            format!("hr={}", hr_hex(self.hr))
        }
    }
}

fn step(hr: HRESULT) -> Step {
    Step { hr, seh: 0 }
}

fn seh_step(code: u32) -> Step {
    Step {
        hr: E_FAIL,
        seh: code,
    }
}

// ---------------------------------------------------------------------------
// Guarded vtable call helpers
// ---------------------------------------------------------------------------

/// `HRESULT f(this, Out*)` — WinRT returns *every* value through an out
/// parameter, scalars and enums included.
unsafe fn call_out<T: Copy>(this: ComPtr, slot: usize) -> (Step, T) {
    if this.is_null() {
        return (step(E_POINTER), mem::zeroed());
    }
    match guarded(|| {
        type Fn = unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT;
        let f: Fn = mem::transmute(this.slot(slot));
        let mut out: T = mem::zeroed();
        let hr = f(this.0, &mut out as *mut T as *mut c_void);
        (step(hr), out)
    }) {
        Ok(v) => v,
        Err(code) => (seh_step(code), mem::zeroed()),
    }
}

/// `HRESULT f(this, interface**)`.
unsafe fn get_ptr(this: ComPtr, slot: usize) -> (Step, ComPtr) {
    let (s, p) = call_out::<*mut c_void>(this, slot);
    (s, ComPtr(p))
}

/// `HRESULT f(this, HSTRING*)`; the returned string is copied then released.
unsafe fn get_hstring(this: ComPtr, slot: usize) -> (Step, String) {
    let (s, h) = call_out::<*mut c_void>(this, slot);
    if !s.ok() {
        return (s, String::new());
    }
    (s, raw_hstring_to_string(h, true))
}

/// `HRESULT f(this, arg)` where `arg` is an interface pointer.
unsafe fn call_ptr_arg(this: ComPtr, slot: usize, arg: ComPtr) -> Step {
    if this.is_null() {
        return step(E_POINTER);
    }
    match guarded(|| {
        type Fn = unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT;
        let f: Fn = mem::transmute(this.slot(slot));
        step(f(this.0, arg.0))
    }) {
        Ok(s) => s,
        Err(code) => seh_step(code),
    }
}

/// `HRESULT f(this, arg)` where `arg` is an `int32` (enum values included).
unsafe fn call_i32_arg(this: ComPtr, slot: usize, arg: i32) -> Step {
    if this.is_null() {
        return step(E_POINTER);
    }
    match guarded(|| {
        type Fn = unsafe extern "system" fn(*mut c_void, i32) -> HRESULT;
        let f: Fn = mem::transmute(this.slot(slot));
        step(f(this.0, arg))
    }) {
        Ok(s) => s,
        Err(code) => seh_step(code),
    }
}

/// `HRESULT f(this, arg)` for any by-value argument (`f64`, `GridLength`, …),
/// matching the C++/WinRT ABI (structs > 8 bytes travel by reference on x64, as
/// Rust's `extern "system"` does too).
unsafe fn call_by_value_arg<T: Copy>(this: ComPtr, slot: usize, arg: T) -> Step {
    if this.is_null() {
        return step(E_POINTER);
    }
    match guarded(|| {
        let f: unsafe extern "system" fn(*mut c_void, T) -> HRESULT =
            mem::transmute(this.slot(slot));
        step(f(this.0, arg))
    }) {
        Ok(s) => s,
        Err(code) => seh_step(code),
    }
}

/// `HRESULT f(this, u32, Out*)` — `IVector<T>::GetAt`.
unsafe fn call_index_out_ptr(this: ComPtr, slot: usize, index: u32) -> (Step, ComPtr) {
    if this.is_null() {
        return (step(E_POINTER), ComPtr::NULL);
    }
    match guarded(|| {
        type Fn = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT;
        let f: Fn = mem::transmute(this.slot(slot));
        let mut out: *mut c_void = ptr::null_mut();
        let hr = f(this.0, index, &mut out);
        (step(hr), ComPtr(out))
    }) {
        Ok(v) => v,
        Err(code) => (seh_step(code), ComPtr::NULL),
    }
}

/// `HRESULT f(this, arg, Out*)` — instance methods with one interface argument
/// (`ReadLocalValue(dp, value*)`) and statics methods (`GetParent(e, parent*)`)
/// share this shape.
unsafe fn call_ptr_arg_out_ptr(this: ComPtr, slot: usize, arg: ComPtr) -> (Step, ComPtr) {
    if this.is_null() {
        return (step(E_POINTER), ComPtr::NULL);
    }
    match guarded(|| {
        type Fn = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> HRESULT;
        let f: Fn = mem::transmute(this.slot(slot));
        let mut out: *mut c_void = ptr::null_mut();
        let hr = f(this.0, arg.0, &mut out);
        (step(hr), ComPtr(out))
    }) {
        Ok(v) => v,
        Err(code) => (seh_step(code), ComPtr::NULL),
    }
}

/// `HRESULT f(this, arg, u32*)`.
unsafe fn statics_call_ptr_arg_out_u32(stils: ComPtr, slot: usize, arg: ComPtr) -> (Step, u32) {
    if stils.is_null() {
        return (step(E_POINTER), 0);
    }
    match guarded(|| {
        type Fn = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut u32) -> HRESULT;
        let f: Fn = mem::transmute(stils.slot(slot));
        let mut out: u32 = 0;
        let hr = f(stils.0, arg.0, &mut out);
        (step(hr), out)
    }) {
        Ok(v) => v,
        Err(code) => (seh_step(code), 0),
    }
}

/// `QueryInterface` with the `HRESULT` preserved for diagnostics.
unsafe fn qi(this: ComPtr, iid: &Guid) -> (Step, ComPtr) {
    if this.is_null() {
        return (step(E_POINTER), ComPtr::NULL);
    }
    match this.query_interface_ex(iid) {
        Ok((hr, p)) => (step(hr), p),
        Err(code) => (seh_step(code), ComPtr::NULL),
    }
}

/// `(Step, ComPtr)` -> `Option<ComPtr>` when the call succeeded and returned an
/// object. The caller then owns the reference.
fn ok_ptr(result: (Step, ComPtr)) -> Option<ComPtr> {
    let (s, p) = result;
    if s.ok() && !p.is_null() {
        Some(p)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Cached activation factories / statics (process lifetime, as in C++/WinRT's
// `get_activation_factory`)
// ---------------------------------------------------------------------------

static XAML_READER_STATICS: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static VISUAL_TREE_HELPER_STATICS: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static VISUAL_STATE_MANAGER_STATICS: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static DEPENDENCY_PROPERTY_STATICS: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static UNSET_VALUE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// `RoGetActivationFactory` + one-time cache. The cached reference lives for the
/// process lifetime; the returned pointer is *borrowed* — never release it.
unsafe fn cached_statics(cell: &AtomicPtr<c_void>, class_name: &str, iid: &Guid) -> Option<ComPtr> {
    let cached = cell.load(Ordering::Acquire);
    if !cached.is_null() {
        return Some(ComPtr(cached));
    }
    let factory = ro_get_activation_factory(class_name, iid)?;
    match cell.compare_exchange(
        ptr::null_mut(),
        factory.0,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        // The reference the factory handed us becomes the cache entry.
        Ok(_) => Some(factory),
        Err(winner) => {
            factory.release();
            Some(ComPtr(winner))
        }
    }
}

unsafe fn xaml_reader_statics() -> Option<ComPtr> {
    cached_statics(
        &XAML_READER_STATICS,
        "Windows.UI.Xaml.Markup.XamlReader",
        &IID_IXAMLREADERSTATICS,
    )
}

/// `Windows.UI.Xaml.Media.VisualTreeHelper` statics (borrowed).
pub unsafe fn visual_tree_helper_statics() -> Option<ComPtr> {
    cached_statics(
        &VISUAL_TREE_HELPER_STATICS,
        "Windows.UI.Xaml.Media.VisualTreeHelper",
        &IID_IVISUALTREEHELPERSTATICS,
    )
}

/// `Windows.UI.Xaml.VisualStateManager` statics (borrowed). The class lives in
/// `Windows.UI.Xaml`, not `.Controls`.
pub unsafe fn visual_state_manager_statics() -> Option<ComPtr> {
    cached_statics(
        &VISUAL_STATE_MANAGER_STATICS,
        "Windows.UI.Xaml.VisualStateManager",
        &IID_IVISUALSTATEMANAGERSTATICS,
    )
}

unsafe fn dependency_property_statics() -> Option<ComPtr> {
    cached_statics(
        &DEPENDENCY_PROPERTY_STATICS,
        "Windows.UI.Xaml.DependencyProperty",
        &IID_IDEPENDENCYPROPERTYSTATICS,
    )
}

// ---------------------------------------------------------------------------
// XamlReader
// ---------------------------------------------------------------------------

/// `XamlReader::Load` (absolute slot 6), `HRESULT` preserved.
pub unsafe fn xaml_reader_load_ex(xaml: &str) -> (Step, ComPtr) {
    let Some(reader) = xaml_reader_statics() else {
        // CLASS_E_CLASSNOTAVAILABLE
        return (step(0x8004_0111u32 as i32), ComPtr::NULL);
    };
    let Some(text) = HString::new(xaml) else {
        return (step(E_FAIL), ComPtr::NULL);
    };
    match guarded(|| {
        type LoadFn =
            unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> HRESULT;
        let f: LoadFn = mem::transmute(reader.slot(SLOT_XAMLREADER_LOAD));
        let mut out: *mut c_void = ptr::null_mut();
        let hr = f(reader.0, text.as_raw(), &mut out);
        (step(hr), ComPtr(out))
    }) {
        Ok(v) => v,
        Err(code) => (seh_step(code), ComPtr::NULL),
    }
}

/// `XamlReader.Load` — `Some` on success. The caller owns the reference.
pub unsafe fn xaml_reader_load(xaml: &str) -> Option<ComPtr> {
    ok_ptr(xaml_reader_load_ex(xaml))
}

/// `winrt::get_class_name` for an object we already hold.
pub unsafe fn class_name(obj: ComPtr) -> String {
    obj.runtime_class_name()
}

// ---------------------------------------------------------------------------
// Style documents — GetStyleFromXamlSetters (reference 15466)
// ---------------------------------------------------------------------------

/// `EscapeXmlAttribute` (reference 15159): escapes `&`, `"`, `<`, `>`; the
/// apostrophe is deliberately left alone, exactly like the reference.
pub fn escape_xml_attribute(data: &str) -> String {
    let mut out = String::with_capacity(data.len());
    for c in data.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// The exact document `GetStyleFromXamlSetters` (15466) hands to
/// `XamlReader::Load`: a `<ResourceDictionary>` carrying five namespace
/// declarations with a `<Style>` inside. When the target type contains a dot the
/// namespace part becomes a `using:` prefix and only the short name is used for
/// `TargetType`; the split is at the *last* dot (`rfind('.')`).
pub fn build_style_document(target_type: &str, setters_xaml: &str) -> String {
    let mut xaml = String::from(
        "<ResourceDictionary\n    \
         xmlns=\"http://schemas.microsoft.com/winfx/2006/xaml/presentation\"\n    \
         xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\"\n    \
         xmlns:d=\"http://schemas.microsoft.com/expression/blend/2008\"\n    \
         xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\"\n    \
         xmlns:muxc=\"using:Microsoft.UI.Xaml.Controls\"",
    );

    match target_type.rfind('.') {
        Some(pos) => {
            let type_namespace = &target_type[..pos];
            let type_name = &target_type[pos + 1..];
            xaml.push_str("\n    xmlns:windhawkstyler=\"using:");
            xaml.push_str(&escape_xml_attribute(type_namespace));
            xaml.push_str("\">\n    <Style TargetType=\"windhawkstyler:");
            xaml.push_str(&escape_xml_attribute(type_name));
            xaml.push_str("\">\n");
        }
        None => {
            xaml.push_str(">\n    <Style TargetType=\"");
            xaml.push_str(&escape_xml_attribute(target_type));
            xaml.push_str("\">\n");
        }
    }

    xaml.push_str(setters_xaml);
    xaml.push_str("    </Style>\n</ResourceDictionary>");
    xaml
}

/// The same document without the `muxc:` namespace declaration.
///
/// Only [`abi_probe`] uses this: `xmlns:muxc="using:Microsoft.UI.Xaml.Controls"`
/// fails if the host has no WinUI2 metadata provider, and the probe must tell
/// that apart from an ABI bug. The production path keeps the reference document.
pub fn build_style_document_without_muxc(target_type: &str, setters_xaml: &str) -> String {
    build_style_document(target_type, setters_xaml)
        .replace("\n    xmlns:muxc=\"using:Microsoft.UI.Xaml.Controls\"", "")
}

/// `      <Setter Property="P" Value="V"/>` lines in reference form.
pub fn build_setters_xaml(setters: &[StyleSetter<'_>]) -> String {
    let mut out = String::new();
    for s in setters {
        out.push_str("        <Setter Property=\"");
        out.push_str(&escape_xml_attribute(s.property));
        out.push('"');
        match s.value {
            StyleValue::Literal(v) => {
                out.push_str(" Value=\"");
                out.push_str(&escape_xml_attribute(v));
                out.push_str("\" />\n");
            }
            StyleValue::Null => out.push_str(" Value=\"{x:Null}\" />\n"),
            StyleValue::Xaml(v) => {
                out.push_str(">\n            <Setter.Value>\n");
                out.push_str(v);
                out.push_str("\n            </Setter.Value>\n        </Setter>\n");
            }
        }
    }
    out
}

/// Load a style document and extract the `Style` exactly like the reference
/// (15509-15513):
///
/// ```text
/// resourceDictionary = XamlReader::Load(xaml).as<ResourceDictionary>();
/// auto [styleKey, styleInspectable] = resourceDictionary.First().Current();
/// return styleInspectable.as<Style>();
/// ```
///
/// The dictionary's default interface is `IResourceDictionary`; `First()` needs a
/// `QueryInterface` to the *parameterized*
/// `IIterable<IKeyValuePair<Object,Object>>` (see
/// [`IID_IITERABLE_KVP_OBJECT_OBJECT`]).
pub unsafe fn load_style_document(xaml: &str) -> Option<ComPtr> {
    let obj = xaml_reader_load(xaml)?;
    let style = style_from_resource_dictionary(obj);
    obj.release();
    style
}

/// Extract the first `Style` out of a loaded `ResourceDictionary`.
unsafe fn style_from_resource_dictionary(dictionary: ComPtr) -> Option<ComPtr> {
    // `.as<ResourceDictionary>()`
    let dict = ok_ptr(qi(dictionary, &IID_IRESOURCEDICTIONARY))?;

    // `resourceDictionary.First()`
    let iterable = match ok_ptr(qi(dict, &IID_IITERABLE_KVP_OBJECT_OBJECT)) {
        Some(i) => i,
        None => {
            dict.release();
            return None;
        }
    };
    dict.release();

    let iterator = ok_ptr(get_ptr(iterable, SLOT_IITERABLE_FIRST));
    iterable.release();
    let iterator = iterator?;

    // `.Current()` -> IKeyValuePair<Object,Object>
    let kvp = ok_ptr(get_ptr(iterator, SLOT_IITERATOR_GET_CURRENT));
    iterator.release();
    let kvp = kvp?;

    // `.Value()` -> the Style object
    let value = ok_ptr(get_ptr(kvp, SLOT_KVP_GET_VALUE));
    kvp.release();
    let value = value?;

    // `.as<Style>()`
    let style = ok_ptr(qi(value, &IID_ISTYLE));
    value.release();
    style
}

/// `GetStyleFromXamlSetters(type, xamlStyleSetters)` (reference 15466). The
/// caller owns the returned `IStyle` reference.
pub unsafe fn load_style_from_xaml_setters(
    target_type: &str,
    setters_xaml: &str,
) -> Option<ComPtr> {
    let document = build_style_document(target_type, setters_xaml);
    load_style_document(&document)
}

/// Like [`load_style_from_xaml_setters`] but takes typed setters.
pub unsafe fn load_style(target_type: &str, setters: &[StyleSetter<'_>]) -> Option<ComPtr> {
    let setters_xaml = build_setters_xaml(setters);
    load_style_from_xaml_setters(target_type, &setters_xaml)
}

/// `ParseXamlValue` (reference 19419): resolve a XAML fragment (`:=` value) by
/// loading it as `<Setter Property="Tag"><Setter.Value>…</Setter.Value></Setter>`
/// inside a `FrameworkElement` style, returning `setter.Value()`. The caller owns
/// the returned `IInspectable`.
pub unsafe fn parse_xaml_value(xaml_value: &str) -> Option<ComPtr> {
    let mut setters = String::new();
    setters.push_str("        <Setter Property=\"Tag\">\n");
    setters.push_str("            <Setter.Value>\n");
    setters.push_str(xaml_value);
    setters.push('\n');
    setters.push_str("            </Setter.Value>\n");
    setters.push_str("        </Setter>\n");

    let style = load_style_from_xaml_setters("FrameworkElement", &setters)?;
    let value = match setter_at(style, 0) {
        Some(setter) => {
            let v = setter_value(setter);
            setter.release();
            v
        }
        None => None,
    };
    style.release();
    value
}

// ---------------------------------------------------------------------------
// Style.Setters -> IList<SetterBase> -> ISetter (reference 15612-15636)
// ---------------------------------------------------------------------------

/// `Style.Setters` (absolute slot 7) then `QueryInterface` for the parameterized
/// `IVector<SetterBase>`.
///
/// The pointer `get_Setters` returns is `SetterBaseCollection`'s *identity*
/// pointer, whose default interface is `ISetterBaseCollection`
/// (`winrt/Windows.UI.Xaml.h` line 1538:
/// `default_interface<SetterBaseCollection> = ISetterBaseCollection`) — **not**
/// `IList<SetterBase>`. The reference's `style.Setters().GetAt(i)` goes through
/// `IList` via a `QueryInterface` using the computed IID
/// [`IID_IVECTOR_SETTERBASE`]; this function does the same. Caller owns the
/// returned `IList`.
pub unsafe fn style_setters(style: ComPtr) -> Option<ComPtr> {
    let collection = ok_ptr(get_ptr(style, SLOT_STYLE_GET_SETTERS))?;
    let list = ok_ptr(qi(collection, &IID_IVECTOR_SETTERBASE));
    collection.release();
    list
}

/// `Style.Setters().Size()`.
pub unsafe fn style_setter_count(style: ComPtr) -> Option<u32> {
    let list = style_setters(style)?;
    let count = vector_size(list);
    list.release();
    count
}

/// `Style.Setters().GetAt(index).as<Setter>()` — the `ISetter` interface of one
/// setter. Caller owns it.
pub unsafe fn setter_at(style: ComPtr, index: u32) -> Option<ComPtr> {
    let list = style_setters(style)?;
    let base = vector_get(list, index);
    list.release();
    let base = base?;
    // GetAt returns SetterBase* (default interface ISetterBase); `.as<Setter>()`
    // is a QueryInterface for ISetter.
    let setter = ok_ptr(qi(base, &IID_ISETTER));
    base.release();
    setter
}

/// `ISetter::get_Property` (slot 6) — the `DependencyProperty`. Caller owns it.
pub unsafe fn setter_property(setter: ComPtr) -> Option<ComPtr> {
    ok_ptr(get_ptr(setter, SLOT_SETTER_GET_PROPERTY))
}

/// `ISetter::get_Value` (slot 8) — the boxed value. Caller owns it.
pub unsafe fn setter_value(setter: ComPtr) -> Option<ComPtr> {
    ok_ptr(get_ptr(setter, SLOT_SETTER_GET_VALUE))
}

/// `GetResolvedPropertyOverrides` (reference 15543): load one style for
/// `target_type`, then read every setter's `(Property, Value)` back out in
/// document order (value rules first, captures last — same order as passed).
pub unsafe fn resolve_style(
    target_type: &str,
    setters: &[StyleSetter<'_>],
) -> Option<Vec<ResolvedSetter>> {
    if setters.is_empty() {
        return Some(Vec::new());
    }
    let style = load_style(target_type, setters)?;
    let resolved = resolve_loaded_style(style, setters.len() as u32);
    style.release();
    resolved
}

/// Same as [`resolve_style`] but for a hand-built setters fragment.
pub unsafe fn resolve_style_from_xaml_setters(
    target_type: &str,
    setters_xaml: &str,
) -> Option<Vec<ResolvedSetter>> {
    let style = load_style_from_xaml_setters(target_type, setters_xaml)?;
    let count = style_setter_count(style)?;
    let resolved = resolve_loaded_style(style, count);
    style.release();
    resolved
}

unsafe fn resolve_loaded_style(style: ComPtr, count: u32) -> Option<Vec<ResolvedSetter>> {
    let list = style_setters(style)?;
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let base = match vector_get(list, i) {
            Some(b) => b,
            None => break,
        };
        let setter = ok_ptr(qi(base, &IID_ISETTER));
        base.release();
        let Some(setter) = setter else {
            break;
        };
        let dp = setter_property(setter);
        let value = setter_value(setter);
        setter.release();
        match (dp, value) {
            (Some(dp), Some(value)) => out.push(ResolvedSetter { dp, value }),
            _ => break,
        }
    }
    list.release();
    Some(out)
}

/// Release a resolved setter list.
pub unsafe fn release_resolved_setters(setters: Vec<ResolvedSetter>) {
    for s in setters {
        s.dp.release();
        s.value.release();
    }
}

// ---------------------------------------------------------------------------
// Generic collection helpers
// ---------------------------------------------------------------------------

/// `IVector<T>::get_Size` (absolute slot 7).
pub unsafe fn vector_size(list: ComPtr) -> Option<u32> {
    let (s, size) = call_out::<u32>(list, SLOT_IVECTOR_GET_SIZE);
    if s.ok() {
        Some(size)
    } else {
        None
    }
}

/// `IVector<T>::GetAt` (absolute slot 6). Caller owns the element.
pub unsafe fn vector_get(list: ComPtr, index: u32) -> Option<ComPtr> {
    ok_ptr(call_index_out_ptr(list, SLOT_IVECTOR_GET_AT, index))
}

/// `IVector<T>::Append` (absolute slot 13).
pub unsafe fn vector_append(list: ComPtr, item: ComPtr) -> bool {
    call_ptr_arg(list, SLOT_IVECTOR_APPEND, item).ok()
}

/// `IIterable<T>::First` (absolute slot 6). Caller owns the iterator.
pub unsafe fn iterable_first(iterable: ComPtr) -> Option<ComPtr> {
    ok_ptr(get_ptr(iterable, SLOT_IITERABLE_FIRST))
}

/// `IIterator<T>::get_Current` (absolute slot 6). Caller owns the element.
pub unsafe fn iterator_current(iterator: ComPtr) -> Option<ComPtr> {
    ok_ptr(get_ptr(iterator, SLOT_IITERATOR_GET_CURRENT))
}

/// `IIterator<T>::MoveNext` (absolute slot 8).
pub unsafe fn iterator_move_next(iterator: ComPtr) -> Option<bool> {
    let (s, moved) = call_out::<u32>(iterator, SLOT_IITERATOR_MOVE_NEXT);
    if s.ok() {
        Some(moved != 0)
    } else {
        None
    }
}

/// `IKeyValuePair<K,V>::get_Value` (absolute slot 7). Caller owns the value.
pub unsafe fn kvp_value(kvp: ComPtr) -> Option<ComPtr> {
    ok_ptr(get_ptr(kvp, SLOT_KVP_GET_VALUE))
}

// ---------------------------------------------------------------------------
// DependencyObject: SetValue / ClearValue / ReadLocalValue (reference 14981)
// ---------------------------------------------------------------------------

/// `IDependencyObject::SetValue` (absolute slot 7).
pub unsafe fn dependency_object_set_value(obj: ComPtr, dp: ComPtr, value: ComPtr) -> HRESULT {
    let (s_q, dep) = qi(obj, &IID_IDEPENDENCYOBJECT);
    if !s_q.ok() || dep.is_null() {
        return if s_q.seh != 0 { E_FAIL } else { s_q.hr };
    }
    let result = call_set_value(dep, dp, value);
    dep.release();
    result.hr
}

/// `HRESULT SetValue(DependencyProperty*, IInspectable*)`.
unsafe fn call_set_value(dep: ComPtr, dp: ComPtr, value: ComPtr) -> Step {
    match guarded(|| {
        type Fn = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> HRESULT;
        let f: Fn = mem::transmute(dep.slot(SLOT_DO_SET_VALUE));
        step(f(dep.0, dp.0, value.0))
    }) {
        Ok(s) => s,
        Err(code) => seh_step(code),
    }
}

/// `IDependencyObject::ClearValue` (absolute slot 8).
pub unsafe fn dependency_object_clear_value(obj: ComPtr, dp: ComPtr) -> HRESULT {
    let (s_q, dep) = qi(obj, &IID_IDEPENDENCYOBJECT);
    if !s_q.ok() || dep.is_null() {
        return if s_q.seh != 0 { E_FAIL } else { s_q.hr };
    }
    let result = call_ptr_arg(dep, SLOT_DO_CLEAR_VALUE, dp);
    dep.release();
    result.hr
}

/// `IDependencyObject::ReadLocalValue` (absolute slot 9). Caller owns the value.
pub unsafe fn dependency_object_read_local_value(obj: ComPtr, dp: ComPtr) -> Option<ComPtr> {
    let (s_q, dep) = qi(obj, &IID_IDEPENDENCYOBJECT);
    if !s_q.ok() || dep.is_null() {
        return None;
    }
    let (_, value) = call_ptr_arg_out_ptr(dep, SLOT_DO_READ_LOCAL_VALUE, dp);
    dep.release();
    if value.is_null() {
        None
    } else {
        Some(value)
    }
}

/// `IDependencyObject::GetAnimationBaseValue` (absolute slot 10).
pub unsafe fn dependency_object_get_animation_base_value(
    obj: ComPtr,
    dp: ComPtr,
) -> Option<ComPtr> {
    let (s_q, dep) = qi(obj, &IID_IDEPENDENCYOBJECT);
    if !s_q.ok() || dep.is_null() {
        return None;
    }
    let (_, value) = call_ptr_arg_out_ptr(dep, SLOT_DO_GET_ANIMATION_BASE_VALUE, dp);
    dep.release();
    if value.is_null() {
        None
    } else {
        Some(value)
    }
}

/// `HRESULT f(this, ptrA, ptrB, int64*)`.
unsafe fn call_two_ptr_arg_out_i64(this: ComPtr, slot: usize, a: ComPtr, b: ComPtr) -> (Step, i64) {
    if this.is_null() {
        return (step(E_POINTER), 0);
    }
    match guarded(|| {
        type Fn =
            unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *mut i64) -> HRESULT;
        let f: Fn = mem::transmute(this.slot(slot));
        let mut out: i64 = 0;
        let hr = f(this.0, a.0, b.0, &mut out);
        (step(hr), out)
    }) {
        Ok(v) => v,
        Err(code) => (seh_step(code), 0),
    }
}

/// `IDependencyObject2::RegisterPropertyChangedCallback` (absolute slot 6).
///
/// Returns the token for [`unregister_property_changed_callback`]. `callback` is
/// a `DependencyPropertyChangedCallback` delegate — an `IUnknown`-derived object
/// whose `Invoke` sits at vtable slot 3.
pub unsafe fn register_property_changed_callback(
    obj: ComPtr,
    dp: ComPtr,
    callback: ComPtr,
) -> Option<i64> {
    let (s_q, dep) = qi(obj, &IID_IDEPENDENCYOBJECT2);
    if !s_q.ok() || dep.is_null() {
        return None;
    }
    let (s, token) = call_two_ptr_arg_out_i64(dep, SLOT_DO2_REGISTER_CALLBACK, dp, callback);
    dep.release();
    if s.ok() {
        Some(token)
    } else {
        None
    }
}

/// `IDependencyObject2::UnregisterPropertyChangedCallback` (absolute slot 7).
pub unsafe fn unregister_property_changed_callback(obj: ComPtr, dp: ComPtr, token: i64) -> HRESULT {
    let (s_q, dep) = qi(obj, &IID_IDEPENDENCYOBJECT2);
    if !s_q.ok() || dep.is_null() {
        return if s_q.seh != 0 { E_FAIL } else { s_q.hr };
    }
    let result = guarded(|| {
        type Fn = unsafe extern "system" fn(*mut c_void, *mut c_void, i64) -> HRESULT;
        let f: Fn = mem::transmute(dep.slot(SLOT_DO2_UNREGISTER_CALLBACK));
        f(dep.0, dp.0, token)
    });
    dep.release();
    result.unwrap_or(E_FAIL)
}

/// `DependencyProperty.UnsetValue` (cached singleton). Borrowed — do not release.
pub unsafe fn dependency_property_unset_value() -> Option<ComPtr> {
    let cached = UNSET_VALUE.load(Ordering::Acquire);
    if !cached.is_null() {
        return Some(ComPtr(cached));
    }
    let statics = dependency_property_statics()?;
    let value = ok_ptr(get_ptr(statics, SLOT_DPS_GET_UNSET_VALUE))?;
    match UNSET_VALUE.compare_exchange(
        ptr::null_mut(),
        value.0,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => Some(value),
        Err(winner) => {
            value.release();
            Some(ComPtr(winner))
        }
    }
}

/// `value == DependencyProperty::UnsetValue()` (reference 15062, 15975).
///
/// WinRT objects QI to their identity pointer, so the `IInspectable` pointer is
/// the object identity; XAML returns the one `UnsetValue` singleton for every
/// "no local value" read, which is what the reference's `==` compares.
pub unsafe fn is_unset_value(value: ComPtr) -> bool {
    if value.is_null() {
        return false;
    }
    match dependency_property_unset_value() {
        Some(unset) => unset.0 == value.0,
        None => false,
    }
}

/// `ReadLocalValueWithWorkaround` (reference 12441).
///
/// `ReadLocalValue` can hand back a `BindingExpression( Base)` for properties set
/// through a `{TemplateBinding}` and `SetValue` rejects those, so the reference
/// falls back to `GetAnimationBaseValue`.
pub unsafe fn read_local_value_with_workaround(obj: ComPtr, dp: ComPtr) -> Option<ComPtr> {
    let value = dependency_object_read_local_value(obj, dp)?;
    let class = value.runtime_class_name();
    if class == "Windows.UI.Xaml.Data.BindingExpressionBase"
        || class == "Windows.UI.Xaml.Data.BindingExpression"
    {
        value.release();
        return dependency_object_get_animation_base_value(obj, dp);
    }
    Some(value)
}

// ---------------------------------------------------------------------------
// FrameworkElement / UIElement accessors
// ---------------------------------------------------------------------------

/// `FrameworkElement.Name` (absolute slot 33).
pub unsafe fn framework_element_name(obj: ComPtr) -> String {
    let (s_q, fe) = qi(obj, &IID_IFRAMEWORKEL);
    if !s_q.ok() || fe.is_null() {
        return String::new();
    }
    let (_, name) = get_hstring(fe, SLOT_FE_GET_NAME);
    fe.release();
    name
}

/// `FrameworkElement.ActualWidth` (absolute slot 13).
pub unsafe fn framework_element_actual_width(obj: ComPtr) -> f64 {
    let (s_q, fe) = qi(obj, &IID_IFRAMEWORKEL);
    if !s_q.ok() || fe.is_null() {
        return 0.0;
    }
    let (_, width) = call_out::<f64>(fe, SLOT_FE_GET_ACTUAL_WIDTH);
    fe.release();
    width
}

/// `FrameworkElement.ActualHeight` (absolute slot 14).
pub unsafe fn framework_element_actual_height(obj: ComPtr) -> f64 {
    let (s_q, fe) = qi(obj, &IID_IFRAMEWORKEL);
    if !s_q.ok() || fe.is_null() {
        return 0.0;
    }
    let (_, height) = call_out::<f64>(fe, 14);
    fe.release();
    height
}

/// `Windows.Foundation.Numerics.Vector3`, ABI layout (three `f32`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Vector3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// `UIElement.ActualOffset` — where XAML actually put the element, in the
/// coordinate space of the XAML root (DIPs, not pixels).
///
/// `IUIElement10.get_ActualOffset` is absolute slot 6 and returns a 12-byte
/// struct: under the x64 MS ABI that is a hidden `Vector3*` out-parameter, so the
/// ABI is `HRESULT f(this, Vector3* retval)`.
pub unsafe fn ui_element_actual_offset(obj: ComPtr) -> Option<Vector3> {
    let (s_q, el) = qi(obj, &IID_IUIELEMENT10);
    if !s_q.ok() || el.is_null() {
        return None;
    }
    let result = guarded(|| {
        type Fn = unsafe extern "system" fn(*mut c_void, *mut Vector3) -> HRESULT;
        let f: Fn = mem::transmute(el.slot(6));
        let mut out = Vector3::default();
        let hr = f(el.0, &mut out);
        (hr, out)
    });
    el.release();
    match result {
        Ok((hr, v)) if hr == S_OK => Some(v),
        _ => None,
    }
}

/// `ICompositeTransform` setters. Slot numbers come from
/// `tools/metadump --iface Windows.UI.Xaml.Media.ICompositeTransform`.
const SLOT_CT_PUT_SCALE_X: usize = 11;
const SLOT_CT_PUT_SCALE_Y: usize = 13;
const SLOT_CT_PUT_TRANSLATE_X: usize = 21;
const SLOT_CT_PUT_TRANSLATE_Y: usize = 23;
const SLOT_CT_GET_SCALE_X: usize = 10;
const SLOT_CT_PUT_CENTER_X: usize = 7;
const SLOT_CT_PUT_CENTER_Y: usize = 9;

/// `IUIElement.get_RenderTransform` (slot 13) / `put_RenderTransform` (slot 14).
const SLOT_UI_GET_RENDER_TRANSFORM: usize = 13;
const SLOT_UI_PUT_RENDER_TRANSFORM: usize = 14;

/// Create a **live** `CompositeTransform`.
///
/// A transform resolved out of a `<Style>` setter is sealed by XAML: its
/// `put_ScaleX` returns `S_OK` and changes nothing. That is the one trap in this
/// whole effect, so the object is created the only way that yields a mutable
/// instance — `XamlReader.Load` on a throwaway element that owns it.
pub unsafe fn composite_transform_new() -> Option<ComPtr> {
    let doc = concat!(
        r#"<Grid xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation""#,
        r#" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">"#,
        r#"<Grid.RenderTransform><CompositeTransform/></Grid.RenderTransform></Grid>"#
    );
    let grid = xaml_reader_load(doc)?;
    let transform = ui_element_get_render_transform(grid);
    grid.release();
    let transform = transform?;
    let xf = ok_ptr(qi(transform, &IID_ICOMPOSITETRANSFORM));
    transform.release();
    xf
}

/// `UIElement.RenderTransform` (slot 13). Caller owns the returned object.
pub unsafe fn ui_element_get_render_transform(obj: ComPtr) -> Option<ComPtr> {
    let el = ok_ptr(qi(obj, &IID_IUIELEMENT))?;
    let t = ok_ptr(get_ptr(el, SLOT_UI_GET_RENDER_TRANSFORM));
    el.release();
    t
}

/// `UIElement.RenderTransform = t` (slot 14).
pub unsafe fn ui_element_put_render_transform(obj: ComPtr, t: ComPtr) -> HRESULT {
    let (s_q, el) = qi(obj, &IID_IUIELEMENT);
    if !s_q.ok() || el.is_null() {
        return if s_q.seh != 0 { E_FAIL } else { s_q.hr };
    }
    let result = call_ptr_arg(el, SLOT_UI_PUT_RENDER_TRANSFORM, t);
    el.release();
    result.hr
}

/// `CompositeTransform.ScaleX` — readback used to prove a transform is mutable
/// (a sealed one still reports the value it was created with).
pub unsafe fn composite_transform_scale_x(xf: ComPtr) -> f64 {
    let (_, v) = call_out::<f64>(xf, SLOT_CT_GET_SCALE_X);
    v
}

/// Pin the transform's rotation/scale centre to the element's bottom-centre, in
/// the element's own coordinate space.
///
/// `RenderTransformOrigin` would be the natural knob, but it takes a
/// `Windows.Foundation.Point` by value — a 2×f32 struct that lands in an SSE
/// register under the x64 ABI and does not survive the plain `fn(this, u64)`
/// transmute this crate uses everywhere. `CompositeTransform.CenterX/CenterY`
/// are ordinary `put_Double` calls and express the same thing: scale about the
/// bottom edge, so a magnified icon grows *upward out of* the dock instead of
/// down through the floor.
pub unsafe fn composite_transform_center(xf: ComPtr, cx: f64, cy: f64) {
    put_double(xf, SLOT_CT_PUT_CENTER_X, cx);
    put_double(xf, SLOT_CT_PUT_CENTER_Y, cy);
}

unsafe fn put_double(this: ComPtr, slot: usize, value: f64) -> bool {
    if this.is_null() {
        return false;
    }
    match guarded(|| {
        type Fn = unsafe extern "system" fn(*mut c_void, f64) -> HRESULT;
        let f: Fn = mem::transmute(this.slot(slot));
        f(this.0, value)
    }) {
        Ok(hr) => hr == S_OK,
        Err(_) => false,
    }
}

/// Write one frame of a `CompositeTransform`: uniform scale plus a translation.
///
/// Mutating the cached object (rather than replacing the element's
/// `RenderTransform`) is what makes a per-frame animation affordable — rebuilding
/// a transform through the XAML parser costs milliseconds, four `put_` calls cost
/// microseconds. `RenderTransform` changes invalidate only the render pass, never
/// layout, so the taskbar's buttons keep their real hit-test geometry.
///
/// `scale`/`tx`/`ty` are skipped when already within `eps` so a settled dock
/// writes nothing at all.
pub unsafe fn composite_transform_frame(
    xf: ComPtr,
    scale: f64,
    tx: f64,
    ty: f64,
    last: &mut (f64, f64, f64),
    eps: f64,
) -> u32 {
    let mut writes = 0u32;
    if (scale - last.0).abs() > eps {
        if put_double(xf, SLOT_CT_PUT_SCALE_X, scale)
            & put_double(xf, SLOT_CT_PUT_SCALE_Y, scale)
        {
            last.0 = scale;
            writes += 2;
        }
    }
    if (tx - last.1).abs() > eps {
        if put_double(xf, SLOT_CT_PUT_TRANSLATE_X, tx) {
            last.1 = tx;
            writes += 1;
        }
    }
    if (ty - last.2).abs() > eps && put_double(xf, SLOT_CT_PUT_TRANSLATE_Y, ty) {
        last.2 = ty;
        writes += 1;
    }
    writes
}

/// `FrameworkElement.Tag` (absolute slot 9) — a boxed value. Caller owns it.
pub unsafe fn framework_element_tag(obj: ComPtr) -> Option<ComPtr> {
    let fe = ok_ptr(qi(obj, &IID_IFRAMEWORKEL))?;
    let tag = ok_ptr(get_ptr(fe, SLOT_FE_GET_TAG));
    fe.release();
    tag
}

/// `FrameworkElement.Style` (absolute slot 38). Caller owns it.
pub unsafe fn framework_element_style(obj: ComPtr) -> Option<ComPtr> {
    let fe = ok_ptr(qi(obj, &IID_IFRAMEWORKEL))?;
    let style = ok_ptr(get_ptr(fe, SLOT_FE_GET_STYLE));
    fe.release();
    style
}

/// `FrameworkElement.Style = style` (absolute slot 39).
pub unsafe fn framework_element_put_style(obj: ComPtr, style: ComPtr) -> HRESULT {
    let (s_q, fe) = qi(obj, &IID_IFRAMEWORKEL);
    if !s_q.ok() || fe.is_null() {
        return if s_q.seh != 0 { E_FAIL } else { s_q.hr };
    }
    let result = call_ptr_arg(fe, SLOT_FE_PUT_STYLE, style);
    fe.release();
    result.hr
}

/// `UIElement.Visibility = visibility` (absolute slot 22).
pub unsafe fn ui_element_visibility_set(obj: ComPtr, visibility: i32) -> HRESULT {
    let (s_q, uie) = qi(obj, &IID_IUIELEMENT);
    if !s_q.ok() || uie.is_null() {
        return if s_q.seh != 0 { E_FAIL } else { s_q.hr };
    }
    let result = call_i32_arg(uie, SLOT_UIE_PUT_VISIBILITY, visibility);
    uie.release();
    result.hr
}

// ---------------------------------------------------------------------------
// VisualTreeHelper
// ---------------------------------------------------------------------------

/// `VisualTreeHelper.GetParent(element)` (absolute slot 12). The reference uses
/// this instead of `FrameworkElement.Parent` (see 16027). Caller owns the result.
pub unsafe fn visual_parent(obj: ComPtr) -> Option<ComPtr> {
    let statics = visual_tree_helper_statics()?;
    let dep = ok_ptr(qi(obj, &IID_IDEPENDENCYOBJECT))?;
    let (_, parent) = call_ptr_arg_out_ptr(statics, SLOT_VTH_GET_PARENT, dep);
    dep.release();
    if parent.is_null() {
        None
    } else {
        Some(parent)
    }
}

/// `VisualTreeHelper.GetChildrenCount` (absolute slot 11).
pub unsafe fn visual_children_count(obj: ComPtr) -> u32 {
    let Some(statics) = visual_tree_helper_statics() else {
        return 0;
    };
    let Some(dep) = ok_ptr(qi(obj, &IID_IDEPENDENCYOBJECT)) else {
        return 0;
    };
    let (_, count) = statics_call_ptr_arg_out_u32(statics, SLOT_VTH_GET_CHILDREN_COUNT, dep);
    dep.release();
    count
}

/// `VisualTreeHelper.GetChild(parent, index)` (absolute slot 10). Caller owns it.
pub unsafe fn visual_child(parent: ComPtr, index: i32) -> Option<ComPtr> {
    let statics = visual_tree_helper_statics()?;
    let dep = ok_ptr(qi(parent, &IID_IDEPENDENCYOBJECT))?;
    let result = guarded(|| {
        type Fn =
            unsafe extern "system" fn(*mut c_void, *mut c_void, i32, *mut *mut c_void) -> HRESULT;
        let f: Fn = mem::transmute(statics.slot(SLOT_VTH_GET_CHILD));
        let mut out: *mut c_void = ptr::null_mut();
        let hr = f(statics.0, dep.0, index, &mut out);
        (step(hr), ComPtr(out))
    });
    dep.release();
    match result {
        Ok((s, p)) if s.ok() && !p.is_null() => Some(p),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// VisualStateManager (reference 15755)
// ---------------------------------------------------------------------------

/// `VisualStateManager.GetVisualStateGroups(element)` (absolute slot 6).
///
/// The declared return type is `IList<VisualStateGroup>`, so the returned pointer
/// is that interface's vtable and `GetAt`/`get_Size` (slots 6/7) work on it
/// directly. Caller owns the list.
pub unsafe fn visual_state_groups(obj: ComPtr) -> Option<ComPtr> {
    let statics = visual_state_manager_statics()?;
    let fe = ok_ptr(qi(obj, &IID_IFRAMEWORKEL))?;
    let (s, list) = call_ptr_arg_out_ptr(statics, SLOT_VSM_GET_VISUAL_STATE_GROUPS, fe);
    fe.release();
    if s.ok() && !list.is_null() {
        Some(list)
    } else {
        None
    }
}

/// `VisualStateGroup.Name` (absolute slot 6).
pub unsafe fn visual_state_group_name(group: ComPtr) -> String {
    let (s_q, vsg) = qi(group, &IID_IVISUALSTATEGROUP);
    if !s_q.ok() || vsg.is_null() {
        return String::new();
    }
    let (_, name) = get_hstring(vsg, SLOT_VSG_GET_NAME);
    vsg.release();
    name
}

/// `VisualStateGroup.States()` (absolute slot 8) — `IList<VisualState>`.
/// Caller owns the list.
pub unsafe fn visual_state_group_states(group: ComPtr) -> Option<ComPtr> {
    let (s_q, vsg) = qi(group, &IID_IVISUALSTATEGROUP);
    if !s_q.ok() || vsg.is_null() {
        return None;
    }
    let (s, states) = get_ptr(vsg, SLOT_VSG_GET_STATES);
    vsg.release();
    if s.ok() && !states.is_null() {
        Some(states)
    } else {
        None
    }
}

/// `VisualState.Name` (absolute slot 6).
pub unsafe fn visual_state_name(state: ComPtr) -> String {
    let (s_q, vs) = qi(state, &IID_IVISUALSTATE);
    if !s_q.ok() || vs.is_null() {
        return String::new();
    }
    let (_, name) = get_hstring(vs, SLOT_VS_GET_NAME);
    vs.release();
    name
}

/// `VisualStateGroup.CurrentState().Name()` (slots 9 then 6); `""` when the
/// group has no current state.
pub unsafe fn visual_state_group_current_state_name(group: ComPtr) -> String {
    let (s_q, vsg) = qi(group, &IID_IVISUALSTATEGROUP);
    if !s_q.ok() || vsg.is_null() {
        return String::new();
    }
    let (_, state) = get_ptr(vsg, SLOT_VSG_GET_CURRENT_STATE);
    vsg.release();
    if state.is_null() {
        return String::new();
    }
    let name = visual_state_name(state);
    state.release();
    name
}

/// `GetVisualStateGroup(element, name) != nullptr` (reference 15755). The
/// `@VisualStateGroup` matcher only asks whether the group exists.
pub unsafe fn has_visual_state_group(obj: ComPtr, group_name: &str) -> bool {
    visual_state_group_by_name(obj, group_name).is_some()
}

/// `GetVisualStateGroup(element, name)`. Caller owns the returned group.
pub unsafe fn visual_state_group_by_name(obj: ComPtr, group_name: &str) -> Option<ComPtr> {
    let list = visual_state_groups(obj)?;
    let count = vector_size(list).unwrap_or(0);
    let mut found = None;
    for i in 0..count {
        let Some(group) = vector_get(list, i) else {
            continue;
        };
        if visual_state_group_name(group) == group_name {
            found = Some(group);
            break;
        }
        group.release();
    }
    list.release();
    found
}

// ---------------------------------------------------------------------------
// Grid / ColumnDefinitionCollection (reference 15114-15148)
// ---------------------------------------------------------------------------

/// `Controls::Grid{}` — a scratch Grid, used by the reference as the owner of a
/// cloned `ColumnDefinitionCollection` (reference 15126).
///
/// `Grid` is a *composable* class, so C++/WinRT's default constructor goes
/// through `IGridFactory::CreateInstance(nullptr, &inner)`, not through
/// `IActivationFactory`. If that factory is unavailable we fall back to
/// `XamlReader.Load("<Grid/>")`, which yields the same fully-initialised object
/// type and serves the same purpose.
pub unsafe fn create_grid() -> Option<ComPtr> {
    if let Some(factory) =
        ro_get_activation_factory("Windows.UI.Xaml.Controls.Grid", &IID_IGRIDFACTORY)
    {
        let created = guarded(|| {
            type Fn = unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                *mut *mut c_void,
                *mut *mut c_void,
            ) -> HRESULT;
            let f: Fn = mem::transmute(factory.slot(SLOT_IGRIDFACTORY_CREATE_INSTANCE));
            let mut inner: *mut c_void = ptr::null_mut();
            let mut out: *mut c_void = ptr::null_mut();
            let hr = f(factory.0, ptr::null_mut(), &mut inner, &mut out);
            (step(hr), ComPtr(out))
        });
        factory.release();
        if let Ok((s, grid)) = created {
            if s.ok() && !grid.is_null() {
                return Some(grid);
            }
        }
    }
    let xaml = format!("<Grid xmlns=\"{XAML_NS}\"/>");
    xaml_reader_load(&xaml)
}

/// `Controls::ColumnDefinition{}` — one fresh `ColumnDefinition`.
pub unsafe fn create_column_definition() -> Option<ComPtr> {
    activate_instance("Windows.UI.Xaml.Controls.ColumnDefinition")
}

/// `Grid.ColumnDefinitions` (absolute slot 7).
///
/// Not a dependency property: it is the instance getter on `IGrid` (hard
/// requirement 4 of the plan). `ColumnDefinitionCollection`'s default interface is
/// `IList<ColumnDefinition>`, so the returned pointer is already an
/// `IVector<ColumnDefinition>` and `GetAt`/`get_Size`/`Append` work on it
/// directly. Caller owns the collection.
pub unsafe fn grid_column_definitions(grid: ComPtr) -> Option<ComPtr> {
    let g = ok_ptr(qi(grid, &IID_IGRID))?;
    let (s, columns) = get_ptr(g, SLOT_GRID_GET_COLUMN_DEFINITIONS);
    g.release();
    if s.ok() && !columns.is_null() {
        Some(columns)
    } else {
        None
    }
}

/// `ColumnDefinition.Width` (absolute slot 6; structure return).
pub unsafe fn column_definition_width(column: ComPtr) -> GridLength {
    match ok_ptr(qi(column, &IID_ICOLUMNDEFINITION)) {
        Some(cd) => {
            let (_, width) = call_out::<GridLength>(cd, SLOT_COLDEF_GET_WIDTH);
            cd.release();
            width
        }
        None => GridLength::default(),
    }
}

/// `ColumnDefinition.Width = width` (absolute slot 7; structure passed by value).
pub unsafe fn put_column_definition_width(column: ComPtr, width: GridLength) -> HRESULT {
    let (s_q, cd) = qi(column, &IID_ICOLUMNDEFINITION);
    if !s_q.ok() || cd.is_null() {
        return if s_q.seh != 0 { E_FAIL } else { s_q.hr };
    }
    let result = call_by_value_arg(cd, SLOT_COLDEF_PUT_WIDTH, width);
    cd.release();
    result.hr
}

/// `ColumnDefinition.MaxWidth` (absolute slot 8).
pub unsafe fn column_definition_max_width(column: ComPtr) -> f64 {
    match ok_ptr(qi(column, &IID_ICOLUMNDEFINITION)) {
        Some(cd) => {
            let (_, v) = call_out::<f64>(cd, SLOT_COLDEF_GET_MAX_WIDTH);
            cd.release();
            v
        }
        None => f64::INFINITY,
    }
}

/// `ColumnDefinition.MaxWidth = value` (absolute slot 9).
pub unsafe fn put_column_definition_max_width(column: ComPtr, width: f64) -> HRESULT {
    let (s_q, cd) = qi(column, &IID_ICOLUMNDEFINITION);
    if !s_q.ok() || cd.is_null() {
        return if s_q.seh != 0 { E_FAIL } else { s_q.hr };
    }
    let result = call_by_value_arg(cd, SLOT_COLDEF_PUT_MAX_WIDTH, width);
    cd.release();
    result.hr
}

/// `ColumnDefinition.MinWidth` (absolute slot 10).
pub unsafe fn column_definition_min_width(column: ComPtr) -> f64 {
    match ok_ptr(qi(column, &IID_ICOLUMNDEFINITION)) {
        Some(cd) => {
            let (_, v) = call_out::<f64>(cd, SLOT_COLDEF_GET_MIN_WIDTH);
            cd.release();
            v
        }
        None => 0.0,
    }
}

/// `ColumnDefinition.MinWidth = value` (absolute slot 11).
pub unsafe fn put_column_definition_min_width(column: ComPtr, width: f64) -> HRESULT {
    let (s_q, cd) = qi(column, &IID_ICOLUMNDEFINITION);
    if !s_q.ok() || cd.is_null() {
        return if s_q.seh != 0 { E_FAIL } else { s_q.hr };
    }
    let result = call_by_value_arg(cd, SLOT_COLDEF_PUT_MIN_WIDTH, width);
    cd.release();
    result.hr
}

/// `SetOrClearValue`'s ColumnDefinitions clone (reference 15124-15135).
///
/// A resolved `ColumnDefinitionCollection` is one object shared by every element
/// the rule applies to (e.g. one taskbar per monitor, all on the same UI thread)
/// and the layout engine writes `ActualWidth` back into each `ColumnDefinition`;
/// sharing would leak one monitor's column sizes onto another. The reference
/// gives every element a private copy: a scratch `Grid` owns the fresh
/// collection, `Width`/`MinWidth`/`MaxWidth` are copied, and the collection is
/// appended to. Returns `(cloned collection, scratch owner)`; the caller must keep
/// the owner alive until `SetValue` has taken the collection.
pub unsafe fn clone_column_definitions(source: ComPtr) -> Option<(ComPtr, ComPtr)> {
    let owner_grid = create_grid()?;
    let Some(columns) = grid_column_definitions(owner_grid) else {
        owner_grid.release();
        return None;
    };

    // A partial clone would be worse than no clone (the grid would silently end
    // up with fewer columns), so any failure aborts and hands the problem back.
    let count = vector_size(source)?;
    for i in 0..count {
        let Some(source_column) = vector_get(source, i) else {
            columns.release();
            owner_grid.release();
            return None;
        };
        let Some(cloned) = create_column_definition() else {
            source_column.release();
            columns.release();
            owner_grid.release();
            return None;
        };
        put_column_definition_width(cloned, column_definition_width(source_column));
        put_column_definition_min_width(cloned, column_definition_min_width(source_column));
        put_column_definition_max_width(cloned, column_definition_max_width(source_column));
        let appended = vector_append(columns, cloned);
        cloned.release();
        source_column.release();
        if !appended {
            columns.release();
            owner_grid.release();
            return None;
        }
    }

    Some((columns, owner_grid))
}

// ---------------------------------------------------------------------------
// Property value unboxing — TryUnboxPropertyValue (reference 15848)
// ---------------------------------------------------------------------------

/// Unwrap a boxed primitive into [`UnboxedValue`] by dispatching on
/// `IPropertyValue::Type()`; `Other` for opaque values (brushes, thicknesses, …),
/// exactly like the reference returns `std::nullopt` for them.
pub unsafe fn unbox_property_value(value: ComPtr) -> UnboxedValue {
    if value.is_null() {
        return UnboxedValue::Null;
    }
    let (s_q, pv) = qi(value, &IID_IPROPERTYVALUE);
    if !s_q.ok() || pv.is_null() {
        return UnboxedValue::Other;
    }
    let (s_ty, ty) = call_out::<i32>(pv, SLOT_PV_GET_TYPE);
    if !s_ty.ok() {
        pv.release();
        return UnboxedValue::Other;
    }
    let out = match ty {
        PT_STRING => UnboxedValue::String(get_hstring(pv, SLOT_PV_GET_STRING).1),
        PT_BOOLEAN => UnboxedValue::Bool(call_out::<i32>(pv, SLOT_PV_GET_BOOLEAN).1 != 0),
        PT_CHAR16 => UnboxedValue::Char16(call_out::<u16>(pv, SLOT_PV_GET_CHAR16).1),
        PT_INT32 => UnboxedValue::Int(call_out::<i32>(pv, SLOT_PV_GET_INT32).1 as i64),
        PT_UINT32 => UnboxedValue::UInt(call_out::<u32>(pv, SLOT_PV_GET_UINT32).1 as u64),
        PT_INT16 => UnboxedValue::Int(call_out::<i16>(pv, SLOT_PV_GET_INT16).1 as i64),
        PT_UINT16 => UnboxedValue::UInt(call_out::<u16>(pv, SLOT_PV_GET_UINT16).1 as u64),
        PT_UINT8 => UnboxedValue::UInt(call_out::<u8>(pv, SLOT_PV_GET_UINT8).1 as u64),
        PT_INT64 => UnboxedValue::Int(call_out::<i64>(pv, SLOT_PV_GET_INT64).1),
        PT_UINT64 => UnboxedValue::UInt(call_out::<u64>(pv, SLOT_PV_GET_UINT64).1),
        PT_SINGLE => UnboxedValue::Single(call_out::<f32>(pv, SLOT_PV_GET_SINGLE).1),
        PT_DOUBLE => UnboxedValue::Double(call_out::<f64>(pv, SLOT_PV_GET_DOUBLE).1),
        PT_OTHER_TYPE => {
            // Common for enums. The reference does `value.try_as<int32_t>()`,
            // i.e. QI for IReference<Int32> and read Value.
            match ok_ptr(qi(value, &IID_IREFERENCE_INT32)) {
                Some(reference) => {
                    let (_, v) = call_out::<i32>(reference, SLOT_IREFERENCE_GET_VALUE);
                    reference.release();
                    UnboxedValue::Int(v as i64)
                }
                None => UnboxedValue::Other,
            }
        }
        _ => UnboxedValue::Other,
    };
    pv.release();
    out
}

// ---------------------------------------------------------------------------
// Live probe — the Lead calls this from dcx-engine's TAP callback
// ---------------------------------------------------------------------------

fn fmt_iids(iids: &[Guid]) -> String {
    if iids.is_empty() {
        return "(none)".to_string();
    }
    iids.iter()
        .map(|g| g.to_braced())
        .collect::<Vec<_>>()
        .join(" ")
}

/// One-shot diagnostic sweep over every ABI fact this crate depends on.
///
/// Runs on the XAML UI thread (the TAP callback), creates only its own objects —
/// the live visual tree is never modified — and returns one line per observation
/// so a fault shows up as data instead of a dead explorer. Call it from
/// `dcx-engine`'s probe and log every line.
///
/// Sections: (1) combase/HSTRING/activation plumbing, (2) `XamlReader.Load` +
/// class name + `GetIids` + `IFrameworkElement` (settles the slot-base question
/// empirically), (3) `GetStyleFromXamlSetters` document → ResourceDictionary →
/// `IIterable<KVP>` (computed parameterized IID) → Style, (4) the production
/// `resolve_style` path, (5) `ParseXamlValue`, (6) `SetValue`/`ReadLocalValue`/
/// `ClearValue`/`UnsetValue`, (7) VisualTreeHelper / VisualStateManager /
/// ColumnDefinitions, (8) the theme's `ColumnDefinitions:=<…>` rule.
pub unsafe fn abi_probe() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    macro_rules! say {
        ($($arg:tt)*) => { out.push(format!($($arg)*)) };
    }

    say!("== dcx-xaml abi_probe ==");

    // --- 1. combase / HSTRING / activation factory -------------------------
    say!("[1] combase resolved: {}", crate::com::combase().is_some());
    match HString::new("dcx-xaml") {
        Some(s) => say!(
            "[1] WindowsCreateString ok, readback=\"{}\"",
            s.to_string_lossy()
        ),
        None => say!("[1] WindowsCreateString FAILED"),
    }
    match ro_get_activation_factory("Windows.UI.Xaml.Markup.XamlReader", &IID_IXAMLREADERSTATICS) {
        Some(f) => {
            say!("[1] RoGetActivationFactory(XamlReader) ok ptr={:p}", f.0);
            f.release();
        }
        None => say!("[1] RoGetActivationFactory(XamlReader) FAILED"),
    }

    // --- 2. XamlReader.Load(<Grid/>) + slot-base verification --------------
    let grid_xaml = format!("<Grid xmlns=\"{XAML_NS}\"/>");
    let (s, grid) = xaml_reader_load_ex(&grid_xaml);
    say!(
        "[2] XamlReader.Load(<Grid/>) {} ptr={:p}",
        s.describe(),
        grid.0
    );
    if !grid.is_null() {
        // Slot 4 is GetRuntimeClassName iff the base is 6 — the exact fact the
        // previous probe got wrong.
        say!(
            "[2] GetRuntimeClassName (slot 4) = \"{}\"",
            grid.runtime_class_name()
        );
        let iids = grid.get_iids();
        say!(
            "[2] GetIids (slot 3): {} iid(s): {}",
            iids.len(),
            fmt_iids(&iids)
        );
        say!(
            "[2] IID_IFrameworkElement present in GetIids: {}",
            iids.iter().any(|g| g.is(&IID_IFRAMEWORKEL))
        );
        let (s_q, fe) = qi(grid, &IID_IFRAMEWORKEL);
        say!("[2] QI IFrameworkElement {} ptr={:p}", s_q.describe(), fe.0);
        if !fe.is_null() {
            fe.release();
            say!(
                "[2] FrameworkElement.Name = \"{}\"",
                framework_element_name(grid)
            );
            say!(
                "[2] FrameworkElement.ActualWidth = {}",
                framework_element_actual_width(grid)
            );
        }
        let (s_uie, uie) = qi(grid, &IID_IUIELEMENT);
        say!("[2] QI IUIElement {} ptr={:p}", s_uie.describe(), uie.0);
        if !uie.is_null() {
            say!(
                "[2] put_Visibility(Collapsed) {}",
                call_i32_arg(uie, SLOT_UIE_PUT_VISIBILITY, VISIBILITY_COLLAPSED).describe()
            );
            uie.release();
        }
        // Grid is composable (IGridFactory), ColumnDefinition is activatable;
        // the clone path needs both constructors to work.
        match create_grid() {
            Some(g) => {
                say!(
                    "[2] create_grid() ok ptr={:p} class=\"{}\"",
                    g.0,
                    g.runtime_class_name()
                );
                g.release();
            }
            None => say!("[2] create_grid() FAILED"),
        }
        match activate_instance("Windows.UI.Xaml.Controls.ColumnDefinition") {
            Some(c) => {
                say!("[2] ActivateInstance(ColumnDefinition) ok ptr={:p}", c.0);
                c.release();
            }
            None => say!("[2] ActivateInstance(ColumnDefinition) FAILED"),
        }
    }

    // --- 3. GetStyleFromXamlSetters document -------------------------------
    let setters_for_grid = "        <Setter Property=\"Margin\" Value=\"10,2,3,2\"/>\n        <Setter Property=\"Tag\" Value=\"probe\"/>\n";
    let document = build_style_document("Windows.UI.Xaml.Controls.Grid", setters_for_grid);
    let (s_doc, doc) = xaml_reader_load_ex(&document);
    say!(
        "[3] Load(style document) {} ptr={:p}",
        s_doc.describe(),
        doc.0
    );
    if !doc.is_null() {
        say!("[3] loaded class = \"{}\"", doc.runtime_class_name());
        let (s_dict, dict) = qi(doc, &IID_IRESOURCEDICTIONARY);
        say!(
            "[3] QI IResourceDictionary {} ptr={:p}",
            s_dict.describe(),
            dict.0
        );
        if !dict.is_null() {
            let diids = dict.get_iids();
            say!("[3] dictionary GetIids: {}", fmt_iids(&diids));
            say!(
                "[3] computed IID_IIterable<KVP<Object,Object>> = {} present={}",
                IID_IITERABLE_KVP_OBJECT_OBJECT,
                diids.iter().any(|g| g.is(&IID_IITERABLE_KVP_OBJECT_OBJECT))
            );
            say!(
                "[3] computed IID_IMap<Object,Object>> = {} present={}",
                IID_IMAP_OBJECT_OBJECT,
                diids.iter().any(|g| g.is(&IID_IMAP_OBJECT_OBJECT))
            );
            let (s_it, iterable) = qi(dict, &IID_IITERABLE_KVP_OBJECT_OBJECT);
            say!(
                "[3] QI IIterable<KVP<Object,Object>> {} ptr={:p}",
                s_it.describe(),
                iterable.0
            );
            if !iterable.is_null() {
                let (s_first, iterator) = get_ptr(iterable, SLOT_IITERABLE_FIRST);
                say!("[3] First() {} ptr={:p}", s_first.describe(), iterator.0);
                if !iterator.is_null() {
                    let (s_cur, kvp) = get_ptr(iterator, SLOT_IITERATOR_GET_CURRENT);
                    say!(
                        "[3] Current() {} ptr={:p} class=\"{}\"",
                        s_cur.describe(),
                        kvp.0,
                        kvp.runtime_class_name()
                    );
                    if !kvp.is_null() {
                        let (s_val, value) = get_ptr(kvp, SLOT_KVP_GET_VALUE);
                        say!(
                            "[3] KVP.Value() {} ptr={:p} class=\"{}\"",
                            s_val.describe(),
                            value.0,
                            value.runtime_class_name()
                        );
                        if !value.is_null() {
                            let (s_style, style) = qi(value, &IID_ISTYLE);
                            say!("[3] QI IStyle {} ptr={:p}", s_style.describe(), style.0);
                            if !style.is_null() {
                                let (s_set, collection) = get_ptr(style, SLOT_STYLE_GET_SETTERS);
                                say!(
                                    "[3] IStyle.get_Setters (slot 7) {} ptr={:p} class=\"{}\"",
                                    s_set.describe(),
                                    collection.0,
                                    collection.runtime_class_name()
                                );
                                if !collection.is_null() {
                                    let (s_l, list) = qi(collection, &IID_IVECTOR_SETTERBASE);
                                    say!(
                                        "[3] QI IVector<SetterBase> {} ptr={:p}",
                                        s_l.describe(),
                                        list.0
                                    );
                                    if !list.is_null() {
                                        let (s_sz, size) =
                                            call_out::<u32>(list, SLOT_IVECTOR_GET_SIZE);
                                        say!(
                                            "[3] IVector<SetterBase>::get_Size {} size={}",
                                            s_sz.describe(),
                                            size
                                        );
                                        list.release();
                                    }
                                    collection.release();
                                }
                                style.release();
                            }
                            value.release();
                        }
                        kvp.release();
                    }
                    iterator.release();
                }
                iterable.release();
            }
            dict.release();
        }
        doc.release();
    }

    // Same document without the `muxc:` namespace, to separate "no WinUI2
    // metadata provider in this host" from "ABI is wrong".
    let document_no_muxc =
        build_style_document_without_muxc("Windows.UI.Xaml.Controls.Grid", setters_for_grid);
    let (s_nm, obj_nm) = xaml_reader_load_ex(&document_no_muxc);
    say!(
        "[3b] Load(style document without xmlns:muxc) {} ptr={:p}",
        s_nm.describe(),
        obj_nm.0
    );
    if !obj_nm.is_null() {
        obj_nm.release();
    }

    // --- 4. Production extraction path + 6. SetValue/ReadLocalValue --------
    let probe_setters = [
        StyleSetter {
            property: "Margin",
            value: StyleValue::Literal("10,2,3,2"),
        },
        StyleSetter {
            property: "Tag",
            value: StyleValue::Literal("probe"),
        },
    ];
    match resolve_style("Windows.UI.Xaml.Controls.Grid", &probe_setters) {
        Some(resolved) => {
            say!("[4] resolve_style ok, {} setter(s)", resolved.len());
            for (i, r) in resolved.iter().enumerate() {
                say!(
                    "[4]  setter[{i}] dp class=\"{}\" value class=\"{}\"",
                    r.dp.runtime_class_name(),
                    r.value.runtime_class_name()
                );
            }
            if !grid.is_null() && !resolved.is_empty() {
                let margin_dp = resolved[0].dp;
                let margin_value = resolved[0].value;
                let hr = dependency_object_set_value(grid, margin_dp, margin_value);
                say!("[6] SetValue(Grid, Margin, 10,2,3,2) {}", hr_hex(hr));
                match dependency_object_read_local_value(grid, margin_dp) {
                    Some(v) => {
                        say!(
                            "[6] ReadLocalValue(Margin) class=\"{}\" unset={} unboxed={:?}",
                            v.runtime_class_name(),
                            is_unset_value(v),
                            unbox_property_value(v)
                        );
                        v.release();
                    }
                    None => say!("[6] ReadLocalValue(Margin) returned null"),
                }
                say!(
                    "[6] ClearValue(Margin) {}",
                    hr_hex(dependency_object_clear_value(grid, margin_dp))
                );
                match dependency_object_read_local_value(grid, margin_dp) {
                    Some(v) => {
                        say!(
                            "[6] after Clear: ReadLocalValue(Margin) class=\"{}\" unset={}",
                            v.runtime_class_name(),
                            is_unset_value(v)
                        );
                        v.release();
                    }
                    None => say!("[6] after Clear: ReadLocalValue(Margin) null"),
                }
                match dependency_property_unset_value() {
                    Some(u) => say!(
                        "[6] DependencyProperty.UnsetValue class=\"{}\"",
                        u.runtime_class_name()
                    ),
                    None => say!("[6] DependencyProperty.UnsetValue FAILED"),
                }
                if resolved.len() > 1 {
                    let hr = dependency_object_set_value(grid, resolved[1].dp, resolved[1].value);
                    say!("[6] SetValue(Grid, Tag, \"probe\") {}", hr_hex(hr));
                    match read_local_value_with_workaround(grid, resolved[1].dp) {
                        Some(v) => {
                            say!(
                                "[6] ReadLocalValue(Tag) unboxed={:?}",
                                unbox_property_value(v)
                            );
                            v.release();
                        }
                        None => say!("[6] ReadLocalValue(Tag) null"),
                    }
                }
            }
            release_resolved_setters(resolved);
        }
        None => say!("[4] resolve_style FAILED (production extraction path)"),
    }

    // --- 5. ParseXamlValue --------------------------------------------------
    let fragment = "<LinearGradientBrush StartPoint=\"0,0\" EndPoint=\"0,1\"><GradientStop Color=\"#50808080\" Offset=\"0.0\" /></LinearGradientBrush>";
    match parse_xaml_value(fragment) {
        Some(v) => {
            say!(
                "[5] ParseXamlValue(<LinearGradientBrush/>) ok class=\"{}\"",
                v.runtime_class_name()
            );
            v.release();
        }
        None => say!("[5] ParseXamlValue(<LinearGradientBrush/>) FAILED"),
    }

    // --- 7. VisualTreeHelper / VisualStateManager / ColumnDefinitions ------
    if !grid.is_null() {
        say!(
            "[7] VisualTreeHelper.GetParent(Grid) = {:?}",
            visual_parent(grid)
        );
        say!(
            "[7] VisualTreeHelper.GetChildrenCount(Grid) = {}",
            visual_children_count(grid)
        );
        match visual_state_groups(grid) {
            Some(list) => {
                let n = vector_size(list).unwrap_or(0);
                say!("[7] GetVisualStateGroups(Grid) ok, {} group(s)", n);
                list.release();
            }
            None => say!("[7] GetVisualStateGroups(Grid) FAILED"),
        }

        match grid_column_definitions(grid) {
            Some(columns) => {
                let source_count = vector_size(columns).unwrap_or(0);
                say!(
                    "[7] Grid.ColumnDefinitions ok, {} column(s) class=\"{}\"",
                    source_count,
                    columns.runtime_class_name()
                );
                match create_column_definition() {
                    Some(column) => {
                        let width = GridLength {
                            Value: 1.0,
                            GridUnitType: GRID_UNIT_STAR,
                        };
                        say!(
                            "[7] put_Width(1*) {}",
                            hr_hex(put_column_definition_width(column, width))
                        );
                        say!("[7] get_Width = {:?}", column_definition_width(column));
                        say!("[7] Append(column) = {}", vector_append(columns, column));
                        column.release();
                        say!(
                            "[7] ColumnDefinitions size after Append = {}",
                            vector_size(columns).unwrap_or(0)
                        );
                    }
                    None => say!("[7] ActivateInstance(ColumnDefinition) FAILED"),
                }
                columns.release();
            }
            None => say!("[7] Grid.ColumnDefinitions FAILED"),
        }
    }

    // --- 8. The theme's own `ColumnDefinitions:=<…>` rule (ref line 3765) ---
    let columns_setter = [StyleSetter {
        property: "ColumnDefinitions",
        value: StyleValue::Xaml(
            "<ColumnDefinitionCollection><ColumnDefinition Width=\"*\"/><ColumnDefinition Width=\"Auto\"/></ColumnDefinitionCollection>",
        ),
    }];
    match resolve_style("Windows.UI.Xaml.Controls.Grid", &columns_setter) {
        Some(resolved) => {
            say!(
                "[8] resolve_style(ColumnDefinitions:=<ColumnDefinitionCollection>) ok, {} setter(s)",
                resolved.len()
            );
            for r in resolved.iter() {
                say!("[8]  value class=\"{}\"", r.value.runtime_class_name());
            }
            release_resolved_setters(resolved);
        }
        None => say!("[8] resolve_style(ColumnDefinitions:=<ColumnDefinitionCollection>) FAILED"),
    }

    if !grid.is_null() {
        grid.release();
    }

    say!("== dcx-xaml abi_probe end ==");
    out
}

/// Read-only diagnostic sweep over a **live** element from the TAP tree.
///
/// Complements [`abi_probe`]: that one proves the ABI against detached objects,
/// this one reports what the engine's element accessors actually see on a real
/// taskbar element, without mutating anything (no `SetValue`, no `ClearValue`).
/// Call it from the engine probe with the `ComPtr` of any recorded element.
pub unsafe fn abi_probe_element(element: ComPtr) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    macro_rules! say {
        ($($arg:tt)*) => { out.push(format!($($arg)*)) };
    }

    say!("== dcx-xaml abi_probe_element ==");
    if element.is_null() {
        say!("[e] element is null");
        return out;
    }

    say!(
        "[e] class=\"{}\" ptr={:p}",
        element.runtime_class_name(),
        element.0
    );
    let iids = element.get_iids();
    say!("[e] GetIids ({}): {}", iids.len(), fmt_iids(&iids));
    say!(
        "[e] implements IFrameworkElement={} IUIElement={} IDependencyObject={} IGrid={}",
        iids.iter().any(|g| g.is(&IID_IFRAMEWORKEL)),
        iids.iter().any(|g| g.is(&IID_IUIELEMENT)),
        iids.iter().any(|g| g.is(&IID_IDEPENDENCYOBJECT)),
        iids.iter().any(|g| g.is(&IID_IGRID)),
    );

    say!(
        "[e] Name=\"{}\" ActualWidth={}",
        framework_element_name(element),
        framework_element_actual_width(element)
    );

    match framework_element_tag(element) {
        Some(tag) => {
            say!(
                "[e] Tag class=\"{}\" unboxed={:?}",
                tag.runtime_class_name(),
                unbox_property_value(tag)
            );
            tag.release();
        }
        None => say!("[e] Tag null"),
    }

    match visual_parent(element) {
        Some(parent) => {
            say!(
                "[e] parent class=\"{}\" name=\"{}\"",
                parent.runtime_class_name(),
                framework_element_name(parent)
            );
            parent.release();
        }
        None => say!("[e] parent null"),
    }

    let child_count = visual_children_count(element);
    say!("[e] VisualTreeHelper.GetChildrenCount = {}", child_count);

    match visual_state_groups(element) {
        Some(list) => {
            let group_count = vector_size(list).unwrap_or(0);
            say!("[e] VisualStateGroups count = {}", group_count);
            for gi in 0..group_count {
                let Some(group) = vector_get(list, gi) else {
                    continue;
                };
                let name = visual_state_group_name(group);
                let current = visual_state_group_current_state_name(group);
                let mut state_names: Vec<String> = Vec::new();
                if let Some(states) = visual_state_group_states(group) {
                    let n = vector_size(states).unwrap_or(0);
                    for si in 0..n {
                        if let Some(state) = vector_get(states, si) {
                            state_names.push(visual_state_name(state));
                            state.release();
                        }
                    }
                    states.release();
                }
                say!(
                    "[e]  group \"{}\" current=\"{}\" states=[{}]",
                    name,
                    current,
                    state_names.join(", ")
                );
                group.release();
            }
            list.release();
        }
        None => say!("[e] VisualStateGroups FAILED"),
    }

    if iids.iter().any(|g| g.is(&IID_IGRID)) {
        match grid_column_definitions(element) {
            Some(columns) => {
                say!(
                    "[e] Grid.ColumnDefinitions count = {}",
                    vector_size(columns).unwrap_or(0)
                );
                columns.release();
            }
            None => say!("[e] Grid.ColumnDefinitions FAILED"),
        }
    }

    say!("== dcx-xaml abi_probe_element end ==");
    out
}

// ---------------------------------------------------------------------------
// Tests — document building only (pure text, no COM/XAML)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_matches_reference_escape_xml_attribute() {
        // Reference 15159: & " < > only; the apostrophe is left alone.
        assert_eq!(
            escape_xml_attribute("a&b\"c<d>e'f"),
            "a&amp;b&quot;c&lt;d&gt;e'f"
        );
    }

    #[test]
    fn style_document_uses_using_prefix_for_dotted_types() {
        let doc = build_style_document("Windows.UI.Xaml.Controls.Grid", "");
        let expected = "<ResourceDictionary\n    \
             xmlns=\"http://schemas.microsoft.com/winfx/2006/xaml/presentation\"\n    \
             xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\"\n    \
             xmlns:d=\"http://schemas.microsoft.com/expression/blend/2008\"\n    \
             xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\"\n    \
             xmlns:muxc=\"using:Microsoft.UI.Xaml.Controls\"\n    \
             xmlns:windhawkstyler=\"using:Windows.UI.Xaml.Controls\">\n    \
             <Style TargetType=\"windhawkstyler:Grid\">\n    \
             </Style>\n</ResourceDictionary>";
        assert_eq!(doc, expected);
    }

    #[test]
    fn style_document_uses_bare_type_without_dot() {
        let doc = build_style_document("FrameworkElement", "");
        assert!(doc.contains("<Style TargetType=\"FrameworkElement\">"));
        assert!(!doc.contains("windhawkstyler"));
    }

    #[test]
    fn style_document_splits_at_last_dot() {
        let doc = build_style_document("Taskbar.TaskbarFrame", "");
        assert!(doc.contains("xmlns:windhawkstyler=\"using:Taskbar\""));
        assert!(doc.contains("<Style TargetType=\"windhawkstyler:TaskbarFrame\">"));
    }

    #[test]
    fn setters_xaml_matches_rule_forms() {
        let setters = [
            StyleSetter {
                property: "Margin",
                value: StyleValue::Literal("10,2,3,2"),
            },
            StyleSetter {
                property: "Grid.Column",
                value: StyleValue::Literal("1"),
            },
            StyleSetter {
                property: "Tag",
                value: StyleValue::Null,
            },
            StyleSetter {
                property: "BorderBrush",
                value: StyleValue::Xaml("<LinearGradientBrush />"),
            },
        ];
        let xaml = build_setters_xaml(&setters);
        assert!(xaml.contains("<Setter Property=\"Margin\" Value=\"10,2,3,2\" />"));
        assert!(xaml.contains("<Setter Property=\"Grid.Column\" Value=\"1\" />"));
        assert!(xaml.contains("<Setter Property=\"Tag\" Value=\"{x:Null}\" />"));
        assert!(xaml.contains(
            "<Setter Property=\"BorderBrush\">\n            <Setter.Value>\n<LinearGradientBrush />\n            </Setter.Value>\n        </Setter>"
        ));
        // Values must be attribute-escaped like the reference does.
        let escaped = build_setters_xaml(&[StyleSetter {
            property: "Tag",
            value: StyleValue::Literal("a&b<c>d\"e"),
        }]);
        assert!(escaped.contains("Value=\"a&amp;b&lt;c&gt;d&quot;e\""));
    }
}
