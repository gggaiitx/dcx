//! DCX engine — the optional injected module for taskbar-level effects.
//!
//! Goal: reproduce the Windhawk `windows-11-taskbar-styler` theme
//! `WindowGlass_variant_Split` by porting the mod's own logic (theme data,
//! selector matching, style variables, `<Style>`-based value resolution and
//! `DependencyObject::SetValue`), not by inventing a different approach.
//!
//! Flow, mirroring `VisualTreeWatcher::OnVisualTreeChange` (11151) and
//! `ApplyCustomizations` (18523):
//!
//! ```text
//! InitializeXamlDiagnosticsEx   -> XAML sited us as its TAP
//! AdviseVisualTreeChange        -> XAML reports every element (Add)
//! cb_on_change                  -> record, match rules, apply
//! posted apply passes           -> re-apply dynamic rules once layout settled
//! ```
//!
//! Everything runs on the taskbar's XAML UI thread; nothing executes unless the
//! host injects this DLL into a XAML process.

pub mod apply;
pub mod bootstrap;
pub mod com;
pub mod diag;
pub mod hover;
pub mod reapply;
pub mod slot;
pub mod tap;
pub mod xaml;

use std::cell::RefCell;
use std::io::Write;
use std::sync::Mutex;

/// One visual-tree element as reported by the TAP.
#[derive(Clone)]
pub struct ElementRecord {
    pub handle: u64,
    pub parent: u64,
    pub ty: String,
    pub name: String,
}

/// Mod-global style variables (`StyleVariableState.variables` in the reference,
/// set by `Property=>VarName` captures).
static VARS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static ELEMENTS: Mutex<Vec<ElementRecord>> = Mutex::new(Vec::new());
static LOG_LOCK: Mutex<()> = Mutex::new(());
/// One `(element, property)` pair already written.
///
/// `ApplyCustomizationsForVisualStateGroup` (reference 17565-17570) inserts the
/// property into the element's customization state and `continue`s when the
/// insert fails: a property is written once per element, and afterwards only a
/// change event re-writes it. Re-applying every rule on every pass is not a
/// harmless optimisation miss — re-setting e.g. `ColumnDefinitions` realizes the
/// subtree again, which reports new `Add` mutations, which applies again.
static WRITTEN: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
/// Set when a capture changed a style variable, so the next pass re-resolves the
/// `{{...}}` rules. `SetStyleVariableIfChangedAndPropagate` (reference 17372).
static VARS_DIRTY: AtomicBool = AtomicBool::new(false);

/// Marks `(handle, property)` written; false when it already was.
fn mark_written(handle: u64, property: &str) -> bool {
    let mut list = match WRITTEN.lock() {
        Ok(l) => l,
        Err(_) => return true,
    };
    if list.iter().any(|(h, p)| *h == handle && p == property) {
        return false;
    }
    if list.len() < 16384 {
        list.push((handle, property.to_string()));
    }
    true
}

/// Append a line to `%TEMP%\dcx-engine.log`. A file, not the console: this runs
/// inside explorer.exe where there is no stdout.
pub fn log(msg: &str) {
    let _guard = LOG_LOCK.lock();
    let path = std::env::temp_dir().join("dcx-engine.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let pid = unsafe { dcx_win32::GetCurrentProcessId() };
        let _ = writeln!(f, "[{pid}] {msg}");
    }
}

fn var_get(name: &str) -> Option<String> {
    VARS.lock()
        .ok()?
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
}

/// Returns true when the value changed.
fn var_set(name: &str, value: &str) -> bool {
    let mut vars = match VARS.lock() {
        Ok(v) => v,
        Err(_) => return false,
    };
    match vars.iter_mut().find(|(n, _)| n == name) {
        Some(slot) => {
            if slot.1 == value {
                false
            } else {
                slot.1 = value.to_string();
                true
            }
        }
        None => {
            vars.push((name.to_string(), value.to_string()));
            true
        }
    }
}

struct VarsLookup;

impl dcx_theme::VarLookup for VarsLookup {
    fn get(&self, name: &str) -> Option<String> {
        var_get(name)
    }
}

/// Runtime rule engine, built from settings. `None` until `build_engine_from_settings`
/// runs (on attach, and again on every settings change).
static ENGINE: Mutex<Option<dcx_theme::RuleEngine>> = Mutex::new(None);

/// A reload pass is currently walking the element tree.
static RELOAD_BUSY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// A reload was requested while one was running: run one more pass afterwards.
static RELOAD_PENDING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Lock the runtime engine. Returns `None` before it has been built.
pub(crate) fn with_engine<R>(f: impl FnOnce(&dcx_theme::RuleEngine) -> R) -> Option<R> {
    ENGINE.lock().ok().and_then(|g| g.as_ref().map(f))
}

// ---------------------------------------------------------------------------
// Element lookup for the rule engine
// ---------------------------------------------------------------------------

/// `ElementLookup` over the recorded tree, with the live-XAML queries the
/// reference does (`winrt::get_class_name`, `VisualStateManager`).
struct RecordedLookup<'a> {
    elements: &'a [ElementRecord],
    site: *mut core::ffi::c_void,
    /// Group name last queried by the matcher, so `apply_element` can read the
    /// group's current state to pick the right `Property@State` bucket.
    last_group: RefCell<Option<String>>,
}

impl<'a> RecordedLookup<'a> {
    fn find(&self, handle: u64) -> Option<&'a ElementRecord> {
        self.elements.iter().find(|e| e.handle == handle)
    }

    unsafe fn inspectable(&self, handle: u64) -> Option<crate::xaml::ComPtr> {
        if self.site.is_null() {
            return None;
        }
        crate::xaml::ComPtr(self.site).get_inspectable(handle)
    }
}

impl<'a> dcx_theme::ElementLookup for RecordedLookup<'a> {
    fn class_name(&self, handle: u64) -> Option<String> {
        // `winrt::get_class_name`: the live runtime class. The TAP-reported type
        // differs for some internal elements (the taskbar root reports
        // `Windows.UI.Xaml.Internal.RootScrollViewer` while selectors say
        // `ScrollViewer`), so the live name is authoritative.
        unsafe {
            if let Some(obj) = self.inspectable(handle) {
                let name = dcx_xaml::xaml::class_name(dcx_xaml::com::ComPtr(obj.0));
                obj.release();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
        self.find(handle).map(|e| e.ty.clone())
    }

    fn name(&self, handle: u64) -> Option<String> {
        self.find(handle)
            .map(|e| e.name.clone())
            .filter(|n| !n.is_empty())
    }

    fn parent(&self, handle: u64) -> Option<u64> {
        // `VisualTreeHelper.GetParent`. The TAP relation carries the same visual
        // parent except that the taskbar subtree hangs off the XAML host
        // (`DesktopWindowXamlSource`), which is not a visual parent: treat that
        // as the root so `:root` matches the ScrollViewer, like the reference.
        let e = self.find(handle)?;
        if e.parent == 0 {
            return None;
        }
        let parent = self.find(e.parent)?;
        if parent.ty.ends_with("DesktopWindowXamlSource") {
            None
        } else {
            Some(e.parent)
        }
    }

    fn child_index_in_parent(&self, handle: u64) -> Option<u32> {
        let e = self.find(handle)?;
        if e.parent == 0 {
            return None;
        }
        let mut index = 0u32;
        for other in self.elements.iter().filter(|o| o.parent == e.parent) {
            if other.handle == handle {
                return Some(index);
            }
            index += 1;
        }
        None
    }

    fn local_value(&self, handle: u64, property: &str) -> Option<dcx_theme::PropValue> {
        // `[Property=Value]` matchers: resolve the DP through a throwaway
        // `<Style>` setter, read the local value, unbox it.
        unsafe {
            let record = self.find(handle)?;
            let obj = self.inspectable(handle)?;
            let dp = resolve_dp(&record.ty, property);
            let value = dp.and_then(|dp| {
                let v = dcx_xaml::xaml::read_local_value_with_workaround(
                    dcx_xaml::com::ComPtr(obj.0),
                    dp,
                );
                dp.release();
                v
            });
            obj.release();
            let value = value?;
            let unboxed = dcx_xaml::xaml::unbox_property_value(value);
            value.release();
            Some(match unboxed {
                dcx_xaml::xaml::UnboxedValue::String(s) => dcx_theme::PropValue::Text(s),
                dcx_xaml::xaml::UnboxedValue::Bool(b) => dcx_theme::PropValue::Bool(b),
                dcx_xaml::xaml::UnboxedValue::Int(i) => dcx_theme::PropValue::Number(i as f64),
                dcx_xaml::xaml::UnboxedValue::UInt(u) => dcx_theme::PropValue::Number(u as f64),
                dcx_xaml::xaml::UnboxedValue::Single(f) => dcx_theme::PropValue::Number(f as f64),
                dcx_xaml::xaml::UnboxedValue::Double(f) => dcx_theme::PropValue::Number(f),
                _ => dcx_theme::PropValue::Other,
            })
        }
    }

    fn visual_state_group(&self, handle: u64, group: &str) -> bool {
        *self.last_group.borrow_mut() = Some(group.to_string());
        unsafe {
            match self.inspectable(handle) {
                Some(obj) => {
                    let has =
                        dcx_xaml::xaml::has_visual_state_group(dcx_xaml::com::ComPtr(obj.0), group);
                    obj.release();
                    has
                }
                None => false,
            }
        }
    }
}

/// Resolve a property name on a class into its `DependencyProperty`, using the
/// reference's `{x:Null}` throwaway setter trick.
pub(crate) fn resolve_dp(class_name: &str, property: &str) -> Option<dcx_xaml::com::ComPtr> {
    let escaped = property.replace('"', "&quot;");
    let setters = format!("<Setter Property=\"{escaped}\" Value=\"{{x:Null}}\"/>");
    unsafe {
        let style = dcx_xaml::xaml::load_style_from_xaml_setters(class_name, &setters)?;
        let setter = dcx_xaml::xaml::setter_at(style, 0);
        let dp = setter.and_then(|s| {
            let dp = dcx_xaml::xaml::setter_property(s);
            s.release();
            dp
        });
        style.release();
        dp
    }
}

// ---------------------------------------------------------------------------
// Applying one element
// ---------------------------------------------------------------------------

/// Apply one element's rules.
///
/// `force` re-resolves the `{{...}}` rules even if they were written before; the
/// caller sets it only when a capture actually changed a style variable, which is
/// the reference's `SetStyleVariableIfChangedAndPropagate` behaviour.
fn apply_element(record: &ElementRecord, force: bool) {
    let snapshot = match ELEMENTS.lock() {
        Ok(g) => g.clone(),
        Err(_) => return,
    };
    let site = tap::SITE.load(std::sync::atomic::Ordering::SeqCst);
    let lookup = RecordedLookup {
        elements: &snapshot,
        site,
        last_group: RefCell::new(None),
    };
    let matched = match with_engine(|e| e.match_element(record.handle, &lookup)) {
        Some(m) => m,
        None => return,
    };
    if matched.overrides.is_empty() && matched.captures.is_empty() {
        return;
    }
    if site.is_null() {
        return;
    }

    unsafe {
        let Some(element) = crate::xaml::ComPtr(site).get_inspectable(record.handle) else {
            return;
        };

        // Which `Property@State` bucket applies: the group's current state wins,
        // otherwise the state-less (`""`) rules, exactly like the reference's
        // `ApplyCustomizationsForVisualStateGroup` (17552).
        let group = lookup.last_group.borrow().clone();
        let current_state = group
            .as_deref()
            .and_then(|g| apply::current_state_name(element.0, g));

        let mut rules: Vec<apply::Rule<'_>> = Vec::new();
        let mut skipped_dynamic = 0usize;
        for ov in &matched.overrides {
            // Reference delays `BackgroundFill.Fill` by one dispatcher turn
            // because setting it before `TaskbarBackground::OnApplyTemplate`
            // crashes or gets overwritten (15011-15031). The theme also hides
            // that rectangle, so the fill is skipped instead.
            if record.name == "BackgroundFill" && ov.property == "Fill" {
                continue;
            }
            let wanted = ov.visual_state.is_empty()
                || Some(ov.visual_state.as_str()) == current_state.as_deref();
            if !wanted {
                continue;
            }
            let dynamic = matches!(ov.value, dcx_theme::ValueSpec::Dynamic { .. });
            let fresh = mark_written(record.handle, &ov.property);
            if !fresh && !(dynamic && force) {
                continue;
            }
            match &ov.value {
                dcx_theme::ValueSpec::Literal(v) => rules.push(apply::Rule {
                    property: &ov.property,
                    value: v,
                    is_xaml: false,
                }),
                dcx_theme::ValueSpec::Xaml(v) => rules.push(apply::Rule {
                    property: &ov.property,
                    value: v,
                    is_xaml: true,
                }),
                dcx_theme::ValueSpec::Dynamic { raw, is_xaml, .. } => {
                    // `ExpandStyleVariables` (16804) with the captured variables.
                    // `with_engine` wraps the closure result in `Option`, and
                    // `expand_expression` itself returns `Option<String>`, so
                    // we must unwrap the inner `Option` here.
                    match with_engine(|e| e.expand_expression(raw, &VarsLookup)) {
                        Some(Some(expanded)) => rules.push(apply::Rule {
                            property: &ov.property,
                            value: Box::leak(expanded.into_boxed_str()),
                            is_xaml: *is_xaml,
                        }),
                        Some(None) | None => skipped_dynamic += 1,
                    }
                }
            }
        }

        if !rules.is_empty() {
            let stats = apply::apply_rules(element.0, &record.ty, &rules);
            if stats.applied > 0 || stats.blurred > 0 {
                log(&format!(
                    "APPLIED {}#{:<24} ok={} blur={} fail={}/{}",
                    record.ty,
                    record.name,
                    stats.applied - stats.blurred,
                    stats.blurred,
                    stats.failed + stats.blurred_failed,
                    rules.len()
                ));
            }
        }

        // Does the value we wrote survive? The tray clock re-writes its own
        // parts as it ticks.
        if matches!(
            record.name.as_str(),
            "DateInnerTextBlock" | "TimeInnerTextBlock"
        ) {
            let props = apply::read_back(
                element.0,
                &record.ty,
                &["Visibility", "FontSize", "Margin", "Padding", "Text"],
            );
            let text: Vec<String> = props.iter().map(|(k, v)| format!("{k}={v}")).collect();
            log(&format!(
                "READBACK {}#{}: {}",
                record.ty,
                record.name,
                text.join(" ")
            ));
        }

        // Captures run after the rules, so `Tag=horizontal` is visible to
        // `Tag=>taskbarDock` in the same pass (the reference wires captures
        // before values and then listens for changes; a delayed pass covers it).
        for capture in &matched.captures {
            let value = match capture.property.as_str() {
                "ActualWidth" => Some(apply::format_double(apply::actual_width(element.0))),
                "Tag" => apply::tag_string(element.0),
                other => {
                    log(&format!(
                        "apply: capture of unsupported property {other} (=> {})",
                        capture.variable
                    ));
                    None
                }
            };
            if let Some(value) = value {
                if var_set(&capture.variable, &value) {
                    // Only a changed capture re-triggers the `{{...}}` rules, like
                    // SetStyleVariableIfChangedAndPropagate (reference 17372).
                    VARS_DIRTY.store(true, Ordering::SeqCst);
                    log(&format!(
                        "capture: {} => {} = {}",
                        capture.property, capture.variable, value
                    ));
                }
            }
        }

        // Diagnostic (reload only): read back the actual values we just set on
        // the two elements the settings feature controls, so we can confirm the
        // write landed on the *live* taskbar element and not a stale handle.
        if force && (record.name == "RootGrid" || record.name == "TaskListButton") {
            let props: &[&str] = if record.name == "RootGrid" {
                &["CornerRadius", "Padding", "Height", "BorderThickness"]
            } else {
                &["Margin", "RenderTransform", "RenderTransformOrigin"]
            };
            let rb = apply::read_back(element.0, &record.ty, props);
            let text: Vec<String> = rb.iter().map(|(k, v)| format!("{k}={v}")).collect();
            log(&format!(
                "READBACK {}#{}: {}",
                record.ty,
                record.name,
                text.join(" ")
            ));
        }

        if skipped_dynamic > 0 {
            log(&format!(
                "apply: {}#{}: {} dynamic rule(s) not expandable yet",
                record.ty, record.name, skipped_dynamic
            ));
        }
        element.release();
    }
}

// ---------------------------------------------------------------------------
// TAP callback bookkeeping
// ---------------------------------------------------------------------------

/// Called for every `VisualMutationType::Add` element.
pub fn record_element(parent: u64, handle: u64, ty: &str, name: &str) {
    let record = ElementRecord {
        handle,
        parent,
        ty: ty.to_string(),
        name: name.to_string(),
    };
    {
        let mut store = match ELEMENTS.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        // One live record per handle. The TAP re-reports elements on every
        // re-layout, and a stale duplicate keeps its *old* parent — consumers
        // that walk the ancestor chain (`hover` does, to turn the parent-relative
        // `ActualOffset` into a screen coordinate) would then sum a chain that no
        // longer exists and end up with a coordinate tens of thousands of pixels
        // off screen.
        if let Some(slot) = store.iter_mut().find(|r| r.handle == handle) {
            *slot = record.clone();
        } else {
            if store.len() >= 8192 {
                return;
            }
            store.push(record.clone());
        }
    }
    if matches!(name, "DateInnerTextBlock" | "TimeInnerTextBlock") {
        log(&format!(
            "RECORD {ty}#{name} handle={handle} parent={parent}"
        ));
    }
    // A new (or recycled) task button invalidates the hover geometry table.
    if name == "TaskListButton" {
        hover::mark_dirty();
        // …and moves the end of the app-icon row, which is where the file box
        // tile is pinned. Sampling starts with the first button; later changes
        // only need the sampler to be alive.
        slot::note_change();
    }
    // The reference applies inside the callback, so new taskbar buttons get
    // styled as they appear.
    apply_element(&record, false);
}

/// Re-resolve the `{{...}}` rules over every recorded element. Only needed after
/// a capture changed a style variable: `ActualWidth` is meaningless until XAML
/// has measured the taskbar, so the first passes seed it and this pass consumes
/// it.
pub fn apply_all() {
    let force = VARS_DIRTY.swap(false, Ordering::SeqCst);
    let snapshot: Vec<ElementRecord> = match ELEMENTS.lock() {
        Ok(g) => g.clone(),
        Err(_) => return,
    };
    log(&format!(
        "apply_all: {} elements (dynamic re-resolve: {})",
        snapshot.len(),
        force
    ));
    for record in &snapshot {
        apply_element(record, force);
    }
    // An engine that attaches to a shell which is *already* running may never see
    // a new `TaskListButton`, and the slot sampler is started from exactly that
    // event. `apply_all` runs on the taskbar UI thread right after attach, so it
    // is the one place guaranteed to run in that case.
    slot::ensure_started();
    log("apply_all: done");
}

/// Full re-apply after a settings change: forget every written property so the
/// new values actually land, then re-resolve dynamic rules and rewrite all.
///
/// Re-entrancy guard: setting a XAML property can pump messages, and the
/// settings watchdog is a timer message — so a reload can be requested while
/// another reload is still walking the tree. Nested walks used to recurse into
/// `Windows.UI.Xaml.dll` and killed Explorer (0xc000027b stowed exception).
/// A nested request now only sets `RELOAD_PENDING`, and the outer loop repeats
/// once the current pass is finished.
pub(crate) fn reload_and_apply() {
    if RELOAD_BUSY.swap(true, Ordering::SeqCst) {
        RELOAD_PENDING.store(true, Ordering::SeqCst);
        log("reload_and_apply: busy — coalesced");
        return;
    }
    loop {
        RELOAD_PENDING.store(false, Ordering::SeqCst);
        reload_once();
        if !RELOAD_PENDING.load(Ordering::SeqCst) {
            break;
        }
    }
    RELOAD_BUSY.store(false, Ordering::SeqCst);
}

/// One full rebuild of every element from the current rule engine.
fn reload_once() {
    if settings_corrupt() {
        // A half-written or hand-broken document used to reload the defaults and
        // force `apply_element(record, true)` over the whole taskbar, piling up
        // another batch of property-changed callbacks that never unregister.
        // Keep what is on screen until a parseable revision arrives.
        log("reload skipped: settings.toml did not parse; keeping the applied theme");
        return;
    }
    reset_caches();
    let snapshot: Vec<ElementRecord> = match ELEMENTS.lock() {
        Ok(g) => g.clone(),
        Err(_) => return,
    };
    log(&format!("reload_and_apply: {} elements", snapshot.len()));
    for record in &snapshot {
        apply_element(record, true);
    }
    log("reload_and_apply: done");
}

/// Drop the per-element write cache, captured variables and dynamic-dirty flag
/// so a subsequent apply rewrites everything from scratch.
fn reset_caches() {
    if let Ok(mut w) = WRITTEN.lock() {
        w.clear();
    }
    if let Ok(mut v) = VARS.lock() {
        v.clear();
    }
    VARS_DIRTY.store(true, Ordering::SeqCst);
}

/// True when the last read of `settings.toml` failed to parse.
fn settings_corrupt() -> bool {
    dcx_win32::config_file::is_poisoned(&dcx_settings::Settings::path())
}

/// (Re)build the rule engine from the on-disk settings file.
pub(crate) fn build_engine_from_settings() {
    // `load` records a parse failure in the poison registry; check after it so
    // the very first call already sees the verdict.
    let settings = dcx_settings::Settings::load();
    if settings_corrupt() {
        log("engine rebuild skipped: settings.toml did not parse");
        return;
    }
    let params = theme_params_from_settings(&settings);
    let theme = dcx_theme::build_runtime_theme(&params);
    let engine = dcx_theme::RuleEngine::from_runtime(&theme);
    if let Ok(mut g) = ENGINE.lock() {
        *g = Some(engine);
    }
    log(&format!(
        "engine built: shape={:?} scale={} gap_between={} max_width={}",
        settings.taskbar_shape,
        params.icon_scale,
        params.gap_between,
        params.max_width
    ));
}

fn theme_params_from_settings(s: &dcx_settings::Settings) -> dcx_theme::ThemeParams {
    dcx_theme::ThemeParams {
        capsule: matches!(s.taskbar_shape, dcx_settings::TaskbarShape::Capsule),
        icon_scale: s.icon_scale_ratio(),
        gap_between: s.icon_spacing.between,
        gap_edge: 0,
        max_width: s.max_width.unwrap_or(1895),
        dock_gap: 0,
        height: s.effective_height(),
    }
}

/// Element count seen so far.
pub fn element_count() -> usize {
    ELEMENTS.lock().map(|s| s.len()).unwrap_or(0)
}

/// Copy of the recorded visual tree, for modules that need the whole list
/// (`hover` builds its geometry table from it).
pub(crate) fn elements_snapshot() -> Vec<ElementRecord> {
    ELEMENTS.lock().map(|g| g.clone()).unwrap_or_default()
}

/// Entry point for the injector.
///
/// # Safety
/// Must be called from a thread inside a process that hosts XAML.
#[no_mangle]
pub unsafe extern "system" fn DcxAttach() -> i32 {
    log("=== DcxAttach ===");
    dcx_blur::set_log_hook(|m| log(m));
    build_engine_from_settings();
    let hr = bootstrap::connect();
    if hr != com::S_OK {
        log(&format!("schedule failed: 0x{:08X}", hr as u32));
    }
    hr
}

// ---------------------------------------------------------------------------
// Self-attach on load
// ---------------------------------------------------------------------------

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};

static STARTED: AtomicBool = AtomicBool::new(false);

/// Set from `DllMain(DLL_PROCESS_DETACH)`.
///
/// Every long-lived thread checks this: the attach loop (up to 10 minutes), the
/// seven delayed apply passes and the 1 s settings watch. Without it, unloading
/// the image left threads that kept calling into freed code pages.
static STOPPING: AtomicBool = AtomicBool::new(false);

/// True once the loader is detaching this image.
pub(crate) fn stopping() -> bool {
    STOPPING.load(Ordering::SeqCst)
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateThread(
        attr: *mut c_void,
        stack: usize,
        start: Option<unsafe extern "system" fn(*mut c_void) -> u32>,
        param: *mut c_void,
        flags: u32,
        tid: *mut u32,
    ) -> isize;
    fn Sleep(ms: u32);
}

unsafe extern "system" fn attach_thread(_param: *mut c_void) -> u32 {
    // The taskbar's XAML island (`DesktopWindowContentBridge`) is not there the
    // moment explorer starts — measured on this machine it can take a couple of
    // minutes — and until it exists there is no XAML core to attach to. The
    // reference catches the window with a `CreateWindowEx` hook (20097-20104);
    // we are injected after the fact, so wait for it instead.
    for attempt in 1..=600u32 {
        if stopping() {
            log("attach cancelled: image is detaching");
            return 0;
        }
        Sleep(1000);
        if attempt % 20 == 1 {
            log(&format!("attach attempt {attempt}"));
        }
        let hr = bootstrap::connect();
        if hr == crate::com::S_OK {
            log(&format!("attached on attempt {attempt}"));
            return 0;
        }
    }
    log("attach gave up");
    0
}

/// # Safety
/// Called by the loader. Must not block or touch COM here (loader lock).
///
/// # Hard constraint: this image must not be `FreeLibrary`ed
///
/// XAML keeps the TAP object registered through `InitializeXamlDiagnosticsEx`
/// and the element handles cached in this crate point into that tree; the
/// shutdown path (host/tray 「恢复任务栏」) therefore restarts Explorer instead of
/// unloading the DLL — see `dcx-host/src/shutdown.rs:9-13` and
/// `docs/engineering-notes.md`. That is a deliberate architectural choice, and
/// it is the only supported way to unload: `DLL_PROCESS_DETACH` below stops the
/// threads and removes the vectored handler, but it cannot undo references XAML
/// holds into this code.
#[no_mangle]
pub unsafe extern "system" fn DllMain(_inst: isize, reason: u32, _reserved: *mut c_void) -> i32 {
    const DLL_PROCESS_ATTACH: u32 = 1;
    const DLL_PROCESS_DETACH: u32 = 0;
    if reason == DLL_PROCESS_ATTACH && !STARTED.swap(true, Ordering::SeqCst) {
        diag::install();
        CreateThread(
            core::ptr::null_mut(),
            0,
            Some(attach_thread),
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
        );
    } else if reason == DLL_PROCESS_DETACH {
        // Everything here must be lock-free and non-blocking: DETACH also runs
        // during process termination, under the loader lock. In particular it
        // must not take `hover::STATE` — the UI thread may hold it — so the
        // effects are torn down from the timer callback instead.
        STOPPING.store(true, Ordering::SeqCst);
        diag::uninstall();
    }
    1
}

#[no_mangle]
pub unsafe extern "system" fn DcxElementCount() -> u32 {
    element_count() as u32
}
