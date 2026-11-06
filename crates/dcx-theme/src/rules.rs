//! Target/rule parsing and selector matching.
//!
//! Faithful port of the reference mod's rule engine:
//!
//! * `ApplyStyleConstants` (18884), `ParseStyleConstant` (18917),
//!   `LoadStyleConstants` (18939)
//! * `ElementMatcherFromString` (18976), `ParseRule` (19075),
//!   `AdjustTypeName` (19143), `SplitTargetString` (19173),
//!   `AddElementCustomizationRulesForSingleTarget` (19202)
//! * `TestElementMatcher` (15935), `FindElementPropertyOverrides` (16010)
//!
//! The engine owns the live XAML tree and feeds element facts in through
//! [`ElementLookup`]; this crate never touches COM.

use crate::expr::{expand_style_variables, is_valid_style_variable_identifier, VarLookup};
use crate::theme::RuntimeTheme;
use crate::theme::Theme;

// ---------------------------------------------------------------------------
// Public contract types (docs/styler-port-plan.md, "dcx-theme — contract").
// ---------------------------------------------------------------------------

/// Facts about one live element, supplied by the engine.
pub trait ElementLookup {
    /// `winrt::get_class_name`.
    fn class_name(&self, handle: u64) -> Option<String>;
    /// `FrameworkElement.Name`.
    fn name(&self, handle: u64) -> Option<String>;
    /// `VisualTreeHelper.GetParent`.
    fn parent(&self, handle: u64) -> Option<u64>;
    /// Zero-based child index of `handle` in its parent (for `[N]` matchers).
    fn child_index_in_parent(&self, handle: u64) -> Option<u32>;
    /// `ReadLocalValueWithWorkaround` typed for `[P=V]` matchers.
    fn local_value(&self, handle: u64, property: &str) -> Option<PropValue>;
    /// Whether the element has a visual state group with this name
    /// (`VisualStateManager.GetVisualStateGroups`), for `@GroupName` matchers.
    fn visual_state_group(&self, handle: u64, group: &str) -> bool;
}

/// Unboxed local value of a dependency property, for `[Property=Value]` matchers.
#[derive(Debug, Clone, PartialEq)]
pub enum PropValue {
    Number(f64),
    Text(String),
    Bool(bool),
    /// Opaque (brush, thickness, ...): the reference cannot compare these and
    /// treats the filter as a mismatch.
    Other,
}

/// Rules resolved for one element (`FindElementPropertyOverrides`).
#[derive(Debug, Clone, Default)]
pub struct MatchedRules {
    /// Rules in application order: the reference walks its rule list backwards
    /// and the first rule that sets a property wins, so `overrides` is already
    /// deduplicated by property, first-wins.
    pub overrides: Vec<Override>,
    /// `Property=>VarName` captures, deduplicated by variable name, first-wins.
    pub captures: Vec<Capture>,
}

/// One property assignment from a matched rule.
#[derive(Debug, Clone, PartialEq)]
pub struct Override {
    /// `"Margin"`, `"Background"`, ...
    pub property: String,
    /// `""` unless the rule was `Property@VisualState=...`.
    pub visual_state: String,
    pub value: ValueSpec,
}

/// One `Property=>VarName` capture from a matched rule.
#[derive(Debug, Clone, PartialEq)]
pub struct Capture {
    pub property: String,
    pub variable: String,
}

/// How a rule's right-hand side must be resolved by the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum ValueSpec {
    /// `Property=value` -- plain text the XAML parser converts.
    Literal(String),
    /// `Property:=value` -- value is a XAML fragment (`isXamlValue`).
    Xaml(String),
    /// `Property=...{{...}}...` -- re-resolved on every apply
    /// (`DynamicStyleTemplate`). `raw` still contains the `{{...}}` text with
    /// style constants already substituted.
    Dynamic {
        property: String,
        raw: String,
        is_xaml: bool,
    },
}

// ---------------------------------------------------------------------------
// Parsed representation.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatcherKind {
    /// Normal element matcher.
    Element,
    /// `'*'`: matches zero or more intermediate ancestors.
    Wildcard,
    /// `':root'`: asserts the next element has no parent.
    Root,
}

/// `ElementMatcher` (reference 11547).
#[derive(Debug, Clone)]
struct ElementMatcher {
    kind: MatcherKind,
    type_name: String,
    name: String,
    visual_state_group: Option<String>,
    one_based_index: i32,
    property_values: Vec<(String, String)>,
}

impl ElementMatcher {
    fn new(kind: MatcherKind) -> Self {
        Self {
            kind,
            type_name: String::new(),
            name: String::new(),
            visual_state_group: None,
            one_based_index: 0,
            property_values: Vec::new(),
        }
    }
}

/// `ValueRule` (reference 11564).
#[derive(Debug, Clone)]
struct ValueRule {
    property_name: String,
    visual_state: String,
    value: String,
    is_xaml_value: bool,
}

impl ValueRule {
    /// `ValueRule::isDynamic()` (reference 11567).
    fn is_dynamic(&self) -> bool {
        self.value.contains("{{")
    }
}

/// `CaptureRule` (reference 11575).
#[derive(Debug, Clone)]
struct CaptureRule {
    property_name: String,
    var_name: String,
}

enum ParsedRule {
    Value(ValueRule),
    Capture(CaptureRule),
}

/// `ElementCustomizationRules` (reference 11809): one target string.
#[derive(Debug, Clone)]
struct ElementCustomizationRules {
    element_matcher: ElementMatcher,
    parent_element_matchers: Vec<ElementMatcher>,
    value_rules: Vec<ValueRule>,
    capture_rules: Vec<CaptureRule>,
}

// ---------------------------------------------------------------------------
// Small string helpers, ported from the reference.
// ---------------------------------------------------------------------------

/// `TrimStringView` (reference 15189): trims `" \t\r\v\n"` -- note that unlike
/// Rust's `str::trim` this does *not* trim form feed.
fn trim_string(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\r' | '\u{b}' | '\n'))
}

/// `SplitStringView` (reference 15197): splits on an exact delimiter, keeping
/// empty pieces and without trimming.
fn split_string_view<'a>(s: &'a str, delimiter: &str) -> Vec<&'a str> {
    s.split(delimiter).collect()
}

/// `SplitTargetString` (reference 19173): splits the comma-separated target
/// list, ignoring commas inside `[Property=Value]` clauses.
fn split_target_string(target: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut part_begin = 0usize;
    let mut in_property = false;
    for (i, c) in target.char_indices() {
        match c {
            '[' => in_property = true,
            ']' => in_property = false,
            ',' if !in_property => {
                result.push(&target[part_begin..i]);
                part_begin = i + 1;
            }
            _ => {}
        }
    }
    result.push(&target[part_begin..]);
    result
}

/// `AdjustTypeName` (reference 19143): expand short type names and prefixes.
fn adjust_type_name(type_name: &str) -> String {
    if !type_name.contains(['.', ':']) {
        if type_name == "Rectangle" {
            return "Windows.UI.Xaml.Shapes.Rectangle".to_string();
        }
        return format!("Windows.UI.Xaml.Controls.{type_name}");
    }

    const ADJUSTMENTS: [(&str, &str); 4] = [
        ("taskbar:", "Taskbar."),
        ("systemtray:", "SystemTray."),
        ("udk:", "WindowsUdk.UI.Shell."),
        ("muxc:", "Microsoft.UI.Xaml.Controls."),
    ];

    for (prefix, replacement) in ADJUSTMENTS {
        if let Some(rest) = type_name.strip_prefix(prefix) {
            return format!("{replacement}{rest}");
        }
    }

    type_name.to_string()
}

// ---------------------------------------------------------------------------
// Parsers.
// ---------------------------------------------------------------------------

/// `ElementMatcherFromString` (reference 18976).
fn element_matcher_from_string(s: &str) -> Result<ElementMatcher, String> {
    let trimmed = trim_string(s);
    if trimmed == "*" {
        return Ok(ElementMatcher::new(MatcherKind::Wildcard));
    }
    if trimmed == ":root" {
        return Ok(ElementMatcher::new(MatcherKind::Root));
    }

    let mut result = ElementMatcher::new(MatcherKind::Element);

    let is_special = |c: char| matches!(c, '#' | '@' | '[');
    let first_special = s.find(is_special);
    result.type_name = trim_string(&s[..first_special.unwrap_or(s.len())]).to_string();
    if result.type_name.is_empty() {
        return Err("Bad target syntax, empty type".to_string());
    }

    let mut i = first_special;
    while let Some(idx) = i {
        let i_next = s[idx + 1..].find(is_special).map(|p| p + idx + 1);
        let next_part = &s[idx + 1..i_next.unwrap_or(s.len())];

        match s[idx..].chars().next().expect("index is a char boundary") {
            '#' => {
                if !result.name.is_empty() {
                    return Err("Bad target syntax, more than one name".to_string());
                }
                result.name = trim_string(next_part).to_string();
                if result.name.is_empty() {
                    return Err("Bad target syntax, empty name".to_string());
                }
            }

            '@' => {
                if result.visual_state_group.is_some() {
                    return Err("Bad target syntax, more than one visual state group".to_string());
                }
                // The reference does not reject an empty group name here; the
                // group lookup then simply finds nothing.
                result.visual_state_group = Some(trim_string(next_part).to_string());
            }

            '[' => {
                let mut rule = trim_string(next_part);
                if rule.is_empty() || !rule.ends_with(']') {
                    return Err("Bad target syntax, missing ']'".to_string());
                }
                rule = trim_string(&rule[..rule.len() - 1]);
                if rule.is_empty() {
                    return Err("Bad target syntax, empty property".to_string());
                }

                if rule.chars().all(|c| c.is_ascii_digit()) {
                    result.one_based_index = rule
                        .parse::<i32>()
                        .map_err(|_| "Bad target syntax, bad index".to_string())?;
                    i = i_next;
                    continue;
                }

                let rule_eq_pos = rule
                    .find('=')
                    .ok_or_else(|| "Bad target syntax, missing '=' in property".to_string())?;
                let rule_key = trim_string(&rule[..rule_eq_pos]);
                let rule_val = trim_string(&rule[rule_eq_pos + 1..]);
                if rule_key.is_empty() {
                    return Err("Bad target syntax, empty property name".to_string());
                }
                result
                    .property_values
                    .push((rule_key.to_string(), rule_val.to_string()));
            }

            _ => return Err("Bad target syntax".to_string()),
        }

        i = i_next;
    }

    Ok(result)
}

/// `ParseRule` (reference 19075): `Property[@VisualState][:]=value` or
/// `Property=>VarName`.
fn parse_rule(s: &str) -> Result<ParsedRule, String> {
    let eq_pos = s
        .find('=')
        .ok_or_else(|| "Bad style syntax, '=' is missing".to_string())?;

    let mut name = &s[..eq_pos];
    let mut value = &s[eq_pos + 1..];

    if value.starts_with('>') {
        // Capture rule: the right-hand side is a style variable name.
        value = &value[1..];

        if name.ends_with(':') {
            return Err(
                "Bad style syntax, ':=>' is not valid (':=' XAML value cannot be combined \
                 with '=>' capture)"
                    .to_string(),
            );
        }
        if name.contains('@') {
            return Err(
                "Bad style syntax, '@VisualState' not allowed on a capture rule".to_string(),
            );
        }

        let property_name = trim_string(name);
        if property_name.is_empty() {
            return Err("Bad style syntax, empty name".to_string());
        }

        let var_name = trim_string(value);
        if var_name.is_empty() {
            return Err("Bad style syntax, empty capture variable name".to_string());
        }
        if !is_valid_style_variable_identifier(var_name) {
            return Err("Bad style syntax, invalid capture variable name".to_string());
        }

        return Ok(ParsedRule::Capture(CaptureRule {
            property_name: property_name.to_string(),
            var_name: var_name.to_string(),
        }));
    }

    let mut result = ValueRule {
        property_name: String::new(),
        visual_state: String::new(),
        value: trim_string(value).to_string(),
        is_xaml_value: false,
    };

    if name.ends_with(':') {
        result.is_xaml_value = true;
        name = &name[..name.len() - 1];
    }

    if let Some(at_pos) = name.find('@') {
        result.visual_state = trim_string(&name[at_pos + 1..]).to_string();
        name = &name[..at_pos];
    }

    result.property_name = trim_string(name).to_string();
    if result.property_name.is_empty() {
        return Err("Bad style syntax, empty name".to_string());
    }

    Ok(ParsedRule::Value(result))
}

/// `AddElementCustomizationRulesForSingleTarget` (reference 19202).
fn add_element_customization_rules_for_single_target(
    target: &str,
    styles: &[String],
) -> Result<ElementCustomizationRules, String> {
    let target_parts = split_string_view(target, " > ");

    let mut element_matcher: Option<ElementMatcher> = None;
    let mut parent_element_matchers: Vec<ElementMatcher> = Vec::new();
    let mut value_rules: Vec<ValueRule> = Vec::new();
    let mut capture_rules: Vec<CaptureRule> = Vec::new();
    let mut has_visual_state_group = false;

    // Walked right-to-left: the first part is the matched element, the rest are
    // its ancestors, closest first.
    for (rev_index, target_part) in target_parts.iter().rev().enumerate() {
        let is_first = rev_index == 0;
        let is_leftmost = rev_index + 1 == target_parts.len();

        let mut matcher = element_matcher_from_string(target_part)?;

        let prev_is_wildcard = parent_element_matchers
            .last()
            .map(|m| m.kind == MatcherKind::Wildcard)
            .unwrap_or(false);

        match matcher.kind {
            MatcherKind::Element => {
                matcher.type_name = adjust_type_name(&matcher.type_name);
            }
            MatcherKind::Wildcard => {
                if is_first {
                    return Err("Bad target syntax, '*' can't be the matched element".to_string());
                }
                if is_leftmost {
                    return Err(
                        "Bad target syntax, '*' can't be the leftmost target part".to_string()
                    );
                }
                if prev_is_wildcard {
                    return Err(
                        "Bad target syntax, '*' can't be adjacent to another '*'".to_string()
                    );
                }
            }
            MatcherKind::Root => {
                if is_first {
                    return Err(
                        "Bad target syntax, ':root' can't be the matched element".to_string()
                    );
                }
                if !is_leftmost {
                    return Err(
                        "Bad target syntax, ':root' must be the leftmost target part".to_string(),
                    );
                }
                if prev_is_wildcard {
                    return Err(
                        "Bad target syntax, ':root' must be followed by a non-wildcard target \
                         part"
                            .to_string(),
                    );
                }
            }
        }

        if matcher.visual_state_group.is_some() {
            if has_visual_state_group {
                return Err("Element type can't have more than one visual state group".to_string());
            }
            has_visual_state_group = true;
        }

        if is_first {
            let mut unresolved_value_rules = Vec::new();
            let mut unresolved_capture_rules = Vec::new();
            for style in styles {
                match parse_rule(style)? {
                    ParsedRule::Value(rule) => unresolved_value_rules.push(rule),
                    ParsedRule::Capture(rule) => unresolved_capture_rules.push(rule),
                }
            }
            element_matcher = Some(matcher);
            value_rules = unresolved_value_rules;
            capture_rules = unresolved_capture_rules;
        } else {
            parent_element_matchers.push(matcher);
        }
    }

    Ok(ElementCustomizationRules {
        element_matcher: element_matcher.expect("at least one target part"),
        parent_element_matchers,
        value_rules,
        capture_rules,
    })
}

// ---------------------------------------------------------------------------
// Matching.
// ---------------------------------------------------------------------------

/// `TestElementMatcher` (reference 15935).
///
/// `visual_state_group_out` mirrors the reference's out-parameter: when the
/// matcher names a visual state group and the element has it, the group name is
/// written out (the reference writes the `VisualStateGroup` instance).
fn test_element_matcher(
    handle: u64,
    matcher: &ElementMatcher,
    lookup: &dyn ElementLookup,
    visual_state_group_out: &mut Option<String>,
) -> bool {
    if !matcher.type_name.is_empty() {
        match lookup.class_name(handle) {
            Some(class_name) => {
                if class_name != matcher.type_name {
                    return false;
                }
            }
            None => return false,
        }
    }

    if !matcher.name.is_empty() {
        let name = lookup.name(handle).unwrap_or_default();
        if name != matcher.name {
            return false;
        }
    }

    if matcher.one_based_index != 0 {
        if lookup.parent(handle).is_none() {
            return false;
        }
        let index = matcher.one_based_index - 1;
        if index < 0 {
            return false;
        }
        match lookup.child_index_in_parent(handle) {
            Some(actual) if i64::from(actual) == i64::from(index) => {}
            _ => return false,
        }
    }

    for (property, expected) in &matcher.property_values {
        match lookup.local_value(handle, property) {
            Some(value) => {
                if !property_value_matches(&value, expected) {
                    return false;
                }
            }
            // ReadLocalValue returned null or DependencyProperty::UnsetValue().
            None => return false,
        }
    }

    if let Some(group_name) = &matcher.visual_state_group {
        if lookup.visual_state_group(handle, group_name) {
            *visual_state_group_out = Some(group_name.clone());
        }
    }

    true
}

/// Compare a `[Property=Value]` filter's expected text with the actual local
/// value (the reference unboxes both and compares typed variants).
fn property_value_matches(actual: &PropValue, expected: &str) -> bool {
    match actual {
        PropValue::Text(text) => text == expected,
        PropValue::Bool(value) => {
            // The XAML boolean converter's spellings.
            if expected.eq_ignore_ascii_case("true") {
                *value
            } else if expected.eq_ignore_ascii_case("false") {
                !*value
            } else {
                false
            }
        }
        PropValue::Number(value) => match expected.trim().parse::<f64>() {
            Ok(expected) => expected == *value,
            Err(_) => false,
        },
        // TryUnboxPropertyValue returns nullopt for opaque values, which the
        // reference treats as a mismatch.
        PropValue::Other => false,
    }
}

/// The recursive `matchParents` lambda of `FindElementPropertyOverrides`
/// (reference 16054), including `'*'` backtracking.
fn match_parents(
    handle: u64,
    parent_matchers: &[ElementMatcher],
    matcher_index: usize,
    lookup: &dyn ElementLookup,
    visual_state_group_out: &mut Option<String>,
) -> bool {
    if matcher_index >= parent_matchers.len() {
        return true;
    }

    let matcher = &parent_matchers[matcher_index];

    match matcher.kind {
        MatcherKind::Root => {
            if lookup.parent(handle).is_some() {
                return false;
            }
            match_parents(
                handle,
                parent_matchers,
                matcher_index + 1,
                lookup,
                visual_state_group_out,
            )
        }

        MatcherKind::Wildcard => {
            // '*' is always followed by an Element matcher (validated at parse
            // time). Walk up and try recursing for each matching ancestor.
            if matcher_index + 1 >= parent_matchers.len() {
                return false;
            }
            let next_matcher = &parent_matchers[matcher_index + 1];
            let mut current = handle;
            loop {
                let parent = match lookup.parent(current) {
                    Some(parent) => parent,
                    None => return false,
                };
                current = parent;
                if test_element_matcher(current, next_matcher, lookup, visual_state_group_out)
                    && match_parents(
                        current,
                        parent_matchers,
                        matcher_index + 2,
                        lookup,
                        visual_state_group_out,
                    )
                {
                    return true;
                }
            }
        }

        MatcherKind::Element => {
            let parent = match lookup.parent(handle) {
                Some(parent) => parent,
                None => return false,
            };
            if !test_element_matcher(parent, matcher, lookup, visual_state_group_out) {
                return false;
            }
            match_parents(
                parent,
                parent_matchers,
                matcher_index + 1,
                lookup,
                visual_state_group_out,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// RuleEngine.
// ---------------------------------------------------------------------------

/// Parsed theme rules + style constants.
pub struct RuleEngine {
    rules: Vec<ElementCustomizationRules>,
    /// Style constants, kept sorted by name length (longest first) so that
    /// `ApplyStyleConstants` replaces the longest name first; equal-length
    /// definitions are ordered so a later definition overrides an earlier one.
    style_constants: Vec<(String, String)>,
}

impl RuleEngine {
    /// Parse all rules and style constants of `theme`.
    ///
    /// Mirrors `InitializeSettings`' use of `LoadStyleConstants` +
    /// `ProcessSingleTargetStylesFromSettings`: commented entries are skipped,
    /// style constants are applied to every style string, and a target with a
    /// syntax error is skipped as a whole (the reference logs and continues).
    pub fn new(theme: &Theme) -> Self {
        let mut engine = Self {
            rules: Vec::new(),
            style_constants: Vec::new(),
        };

        // LoadStyleConstants (18939).
        for raw_constant in theme.style_constants {
            if let Some(parsed) = engine.parse_style_constant(raw_constant) {
                engine.add_style_constant(parsed);
            }
        }

        // ProcessSingleTargetStylesFromSettings (19313) + AddElementCustomizationRules.
        for target_styles in theme.target_styles {
            engine.add_target_styles(target_styles.target, target_styles.styles);
        }

        engine
    }

    /// Build an engine from a runtime (settings-driven) owned theme.
    ///
    /// Identical to [`RuleEngine::new`] except the input is owned, so it can be
    /// produced at runtime from user settings via `build_runtime_theme`.
    pub fn from_runtime(theme: &RuntimeTheme) -> Self {
        let mut engine = Self {
            rules: Vec::new(),
            style_constants: Vec::new(),
        };
        for raw in &theme.style_constants {
            if let Some(parsed) = engine.parse_style_constant(raw) {
                engine.add_style_constant(parsed);
            }
        }
        for ts in &theme.target_styles {
            engine.add_target_styles(&ts.target, &ts.styles);
        }
        engine
    }

    /// `ProcessSingleTargetStylesFromSettings` (reference 19313) for one entry.
    ///
    /// Generic over the borrowed string type so it serves both the `&'static`
    /// base theme and the owned runtime theme.
    fn add_target_styles<S: AsRef<str>>(&mut self, target: &str, styles: &[S]) {
        // Skip if commented.
        if target.starts_with("//") {
            return;
        }

        let mut expanded = Vec::with_capacity(styles.len());
        for style in styles {
            let style = style.as_ref();
            // Skip if commented.
            if style.starts_with("//") {
                continue;
            }
            expanded.push(self.expand_constants(style));
        }

        if !expanded.is_empty() {
            self.add_element_customization_rules(target, &expanded);
        }
    }

    /// `AddElementCustomizationRules` (reference 19302): split the target list,
    /// then parse each single target.
    fn add_element_customization_rules(&mut self, target: &str, styles: &[String]) {
        for single_target in split_target_string(target) {
            match add_element_customization_rules_for_single_target(single_target, styles) {
                Ok(rules) => self.rules.push(rules),
                Err(_) => {
                    // The reference logs the error and skips this target.
                }
            }
        }
    }

    /// `ParseStyleConstant` (reference 18917).
    fn parse_style_constant(&self, constant: &str) -> Option<(String, String)> {
        // Skip if commented.
        if constant.starts_with("//") {
            return None;
        }

        let eq_pos = constant.find('=')?;
        let key = trim_string(&constant[..eq_pos]).to_string();
        let value_raw = trim_string(&constant[eq_pos + 1..]);
        let value = self.expand_constants(value_raw);
        Some((key, value))
    }

    /// The `addToResult` lambda of `LoadStyleConstants` (reference 18941).
    fn add_style_constant(&mut self, constant: (String, String)) {
        // `std::lower_bound` with `a.first.size() > b.first.size()`: insert
        // before the first entry whose name is no longer than the new one, so
        // longer names come first and a later equal-name definition overrides.
        let insert_index = self
            .style_constants
            .iter()
            .position(|(name, _)| name.len() <= constant.0.len())
            .unwrap_or(self.style_constants.len());
        self.style_constants.insert(insert_index, constant);
    }

    /// `ApplyStyleConstants` (reference 18884): replace `$Name` with its value,
    /// longest name first.
    pub fn expand_constants(&self, style: &str) -> String {
        let mut result = String::new();
        let mut last_pos = 0usize;

        while let Some(find_pos) = style[last_pos..].find('$').map(|p| p + last_pos) {
            result.push_str(&style[last_pos..find_pos]);

            let rest = &style[find_pos + 1..];
            let mut constant: Option<&(String, String)> = None;
            for candidate in &self.style_constants {
                if rest.starts_with(candidate.0.as_str()) {
                    constant = Some(candidate);
                    break;
                }
            }

            match constant {
                Some((name, value)) => {
                    result.push_str(value);
                    last_pos = find_pos + 1 + name.len();
                }
                None => {
                    result.push('$');
                    last_pos = find_pos + 1;
                }
            }
        }

        // Care for the rest after the last occurrence.
        result.push_str(&style[last_pos..]);
        result
    }

    /// `ExpandStyleVariables` (reference 16804): substitute `{{expr}}` from the
    /// captured style variables. `None` means the style must not be applied
    /// (evaluation failure or a live `skip()`).
    pub fn expand_expression(&self, expr: &str, vars: &dyn VarLookup) -> Option<String> {
        expand_style_variables(expr, vars)
    }

    /// `FindElementPropertyOverrides` (reference 16010) + `TestElementMatcher`.
    ///
    /// Walks the rule list backwards; the first rule that sets a property wins.
    pub fn match_element(&self, handle: u64, lookup: &dyn ElementLookup) -> MatchedRules {
        let mut result = MatchedRules::default();
        let mut properties_added: Vec<String> = Vec::new();
        let mut captures_added: Vec<String> = Vec::new();

        for rules in self.rules.iter().rev() {
            let mut visual_state_group: Option<String> = None;

            if !test_element_matcher(
                handle,
                &rules.element_matcher,
                lookup,
                &mut visual_state_group,
            ) {
                continue;
            }

            if !match_parents(
                handle,
                &rules.parent_element_matchers,
                0,
                lookup,
                &mut visual_state_group,
            ) {
                continue;
            }

            for value_rule in &rules.value_rules {
                if properties_added
                    .iter()
                    .any(|p| *p == value_rule.property_name)
                {
                    continue;
                }
                properties_added.push(value_rule.property_name.clone());

                // Emit every visual-state variant of this property from this
                // rule, first definition per visual state winning. The
                // reference keeps them in a per-property map.
                let mut states_added: Vec<&str> = Vec::new();
                for variant in &rules.value_rules {
                    if variant.property_name != value_rule.property_name {
                        continue;
                    }
                    if states_added.iter().any(|s| **s == *variant.visual_state) {
                        continue;
                    }
                    states_added.push(&variant.visual_state);

                    let value = if variant.is_dynamic() {
                        ValueSpec::Dynamic {
                            property: variant.property_name.clone(),
                            raw: variant.value.clone(),
                            is_xaml: variant.is_xaml_value,
                        }
                    } else if variant.is_xaml_value {
                        ValueSpec::Xaml(variant.value.clone())
                    } else {
                        ValueSpec::Literal(variant.value.clone())
                    };

                    result.overrides.push(Override {
                        property: variant.property_name.clone(),
                        visual_state: variant.visual_state.clone(),
                        value,
                    });
                }
            }

            for capture_rule in &rules.capture_rules {
                if captures_added.iter().any(|v| *v == capture_rule.var_name) {
                    continue;
                }
                captures_added.push(capture_rule.var_name.clone());
                result.captures.push(Capture {
                    property: capture_rule.property_name.clone(),
                    variable: capture_rule.var_name.clone(),
                });
            }

            // The reference keys the resolved rules by the element's
            // VisualStateGroup instance (`overridesPerVSG`). The contract's
            // `MatchedRules` is flat and carries the per-rule visual state only,
            // so the group is resolved (which exercises
            // `ElementLookup::visual_state_group`, as the reference does) but is
            // not part of the result.
            let _ = visual_state_group;
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `TargetStyles` is defined in `theme` and re-exported from the crate root,
    // so `use super::*` does not bring it in. Without this line the whole test
    // target fails with 25 × E0422 and `cargo test --workspace` never runs.
    use crate::TargetStyles;
    use std::collections::HashMap;

    // -- mock ElementLookup -------------------------------------------------

    #[derive(Default)]
    struct MockNode {
        class_name: String,
        /// TAP `VisualElement.Type` (e.g. `Windows.UI.Xaml.Internal.RootScrollViewer`).
        /// Recorded for documentation only: the matcher compares `class_name`
        /// (which models `winrt::get_class_name`).
        tap_type: String,
        name: String,
        parent: Option<u64>,
        index: Option<u32>,
        values: Vec<(String, PropValue)>,
        visual_state_groups: Vec<String>,
    }

    #[derive(Default)]
    struct MockLookup {
        nodes: HashMap<u64, MockNode>,
        vsg_calls: std::cell::RefCell<Vec<(u64, String)>>,
    }

    impl MockLookup {
        fn new() -> Self {
            Self::default()
        }

        /// Add a node; its child index in the parent is derived from the nodes
        /// added so far (first added child is index 0).
        fn add(&mut self, id: u64, class_name: &str, name: &str, parent: Option<u64>) -> u64 {
            let index =
                parent.map(|p| self.nodes.values().filter(|n| n.parent == Some(p)).count() as u32);
            self.nodes.insert(
                id,
                MockNode {
                    class_name: class_name.to_string(),
                    tap_type: class_name.to_string(),
                    name: name.to_string(),
                    parent,
                    index,
                    values: Vec::new(),
                    visual_state_groups: Vec::new(),
                },
            );
            id
        }

        /// Record the TAP `VisualElement.Type` when it differs from the runtime
        /// class name the matcher uses.
        fn with_tap_type(mut self, id: u64, tap_type: &str) -> Self {
            self.nodes.get_mut(&id).unwrap().tap_type = tap_type.to_string();
            self
        }

        fn with_value(mut self, id: u64, property: &str, value: PropValue) -> Self {
            self.nodes
                .get_mut(&id)
                .unwrap()
                .values
                .push((property.to_string(), value));
            self
        }

        fn with_vsg(mut self, id: u64, group: &str) -> Self {
            self.nodes
                .get_mut(&id)
                .unwrap()
                .visual_state_groups
                .push(group.to_string());
            self
        }
    }

    impl ElementLookup for MockLookup {
        fn class_name(&self, handle: u64) -> Option<String> {
            self.nodes.get(&handle).map(|n| n.class_name.clone())
        }

        fn name(&self, handle: u64) -> Option<String> {
            self.nodes.get(&handle).map(|n| n.name.clone())
        }

        fn parent(&self, handle: u64) -> Option<u64> {
            self.nodes.get(&handle).and_then(|n| n.parent)
        }

        fn child_index_in_parent(&self, handle: u64) -> Option<u32> {
            self.nodes.get(&handle).and_then(|n| n.index)
        }

        fn local_value(&self, handle: u64, property: &str) -> Option<PropValue> {
            self.nodes.get(&handle).and_then(|n| {
                n.values
                    .iter()
                    .find(|(p, _)| p == property)
                    .map(|(_, v)| v.clone())
            })
        }

        fn visual_state_group(&self, handle: u64, group: &str) -> bool {
            self.vsg_calls
                .borrow_mut()
                .push((handle, group.to_string()));
            self.nodes
                .get(&handle)
                .map(|n| n.visual_state_groups.iter().any(|g| g == group))
                .unwrap_or(false)
        }
    }

    struct Vars(HashMap<String, String>);

    impl VarLookup for Vars {
        fn get(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    fn vars(pairs: &[(&str, &str)]) -> Vars {
        Vars(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    // -- helpers ------------------------------------------------------------

    fn engine() -> RuleEngine {
        RuleEngine::new(&crate::theme::WINDOW_GLASS_SPLIT)
    }

    /// All overrides for `property` (there may be several visual states).
    fn overrides_for<'a>(matched: &'a MatchedRules, property: &str) -> Vec<&'a Override> {
        matched
            .overrides
            .iter()
            .filter(|o| o.property == property)
            .collect()
    }

    fn one_override<'a>(matched: &'a MatchedRules, property: &str) -> &'a Override {
        let all = overrides_for(matched, property);
        assert_eq!(
            all.len(),
            1,
            "expected exactly one override for {property}, got {:?}",
            all.iter()
                .map(|o| (&o.visual_state, &o.value))
                .collect::<Vec<_>>()
        );
        all[0]
    }

    fn value_text(spec: &ValueSpec) -> String {
        match spec {
            ValueSpec::Literal(v) | ValueSpec::Xaml(v) => v.clone(),
            ValueSpec::Dynamic { raw, .. } => raw.clone(),
        }
    }

    const GLASS: &str = "<WindhawkBlur BlurAmount=\"5\" TintColor=\"{ThemeResource \
                         SystemChromeMediumColor}\" TintOpacity=\"0.7\" />";

    // -- theme parsing ------------------------------------------------------

    #[test]
    fn theme_has_all_entries_and_constants() {
        // The reference theme (lines 3595-3788) has 45 `ThemeTargetStyles`
        // entries and 20 style constants. This port adds one entry:
        // `Taskbar.TaskbarBackground#BackgroundControl, Taskbar.TaskbarBackground`
        // → `Visibility=Collapsed`, which removes the full-width shell background
        // that shows up as a grey line above the floating dock.
        assert_eq!(crate::theme::WINDOW_GLASS_SPLIT.target_styles.len(), 46);
        // 19, not the reference's 20: the port keeps the constants the ported
        // rules actually resolve (`$Background`, `$BorderBrush`,
        // `$BorderThickness`, `$CornerRadius`, `$Element*`, `$Glass`,
        // `$TaskbarFrameMaxWidth`, `$TrayPadding`). The runtime theme adds
        // `IconGapBetween` / `IconGapEdge` on top (see `build_runtime_theme`).
        // This assertion never ran before the test target compiled.
        assert_eq!(crate::theme::WINDOW_GLASS_SPLIT.style_constants.len(), 19);

        let engine = engine();
        assert_eq!(engine.style_constants.len(), 19);

        // Three entries have comma-separated target lists (2, 2 and 4 targets),
        // so 46 entries parse into 43 + 2 + 2 + 4 = 51 customization rules.
        assert_eq!(engine.rules.len(), 51);

        // Style constants are sorted by name length, longest first.
        let lengths: Vec<usize> = engine
            .style_constants
            .iter()
            .map(|(k, _)| k.len())
            .collect();
        let mut sorted = lengths.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(lengths, sorted);

        // Every rule has an element matcher (the rightmost target part).
        for rule in &engine.rules {
            assert_eq!(rule.element_matcher.kind, MatcherKind::Element);
            assert!(!rule.element_matcher.type_name.is_empty());
            assert!(!rule.value_rules.is_empty() || !rule.capture_rules.is_empty());
        }
    }

    #[test]
    fn theme_style_constants_expand() {
        let engine = engine();
        assert_eq!(engine.expand_constants("$Glass"), GLASS);
        // `Background=$Glass` is resolved while loading the constants, so
        // `$Background` expands straight to the Glass brush.
        assert_eq!(engine.expand_constants("$Background"), GLASS);
        assert_eq!(engine.expand_constants("$BorderThickness"), "0.5,1,0.5,1");
        assert_eq!(engine.expand_constants("$CornerRadius"), "25");
        assert_eq!(engine.expand_constants("$TrayPadding"), "2,4,2,4");
        assert_eq!(engine.expand_constants("$TaskbarFrameMaxWidth"), "1895");
        assert_eq!(engine.expand_constants("$ElementCornerRadius"), "12");
        assert_eq!(
            engine.expand_constants("$ElementBorderThickness"),
            "0.3,0.3,0.3,1"
        );
        assert_eq!(
            engine.expand_constants("$BorderBrush"),
            "<LinearGradientBrush StartPoint=\"0,0\" EndPoint=\"0,1\">\
             <GradientStop Color=\"#50808080\" Offset=\"0.0\" />\
             <GradientStop Color=\"#30404040\" Offset=\"0.25\" />\
             <GradientStop Color=\"#40808080\" Offset=\"1\" />\
             </LinearGradientBrush>"
        );
        // Longest name first: $ElementBG must not expand as $Element + BG.
        assert_eq!(
            engine.expand_constants("$ElementBG"),
            "<SolidColorBrush Color=\"{ThemeResource SystemChromeLowColor}\" Opacity=\"0.3\" />"
        );
        // Unknown `$Name` is left untouched.
        assert_eq!(engine.expand_constants("$Nope"), "$Nope");
        assert_eq!(
            engine.expand_constants("100%$Glass"),
            format!("100%{GLASS}")
        );
    }

    #[test]
    fn theme_root_grid_rule_is_verbatim() {
        let engine = engine();
        // `Taskbar.TaskbarFrame > Grid#RootGrid`
        let rules = engine
            .rules
            .iter()
            .find(|r| {
                r.element_matcher.type_name == "Windows.UI.Xaml.Controls.Grid"
                    && r.element_matcher.name == "RootGrid"
                    && r.element_matcher.visual_state_group.is_none()
                    && r.parent_element_matchers.len() == 1
                    && r.parent_element_matchers[0].type_name == "Taskbar.TaskbarFrame"
            })
            .expect("RootGrid rule");

        assert_eq!(rules.value_rules.len(), 5);
        assert_eq!(rules.value_rules[0].property_name, "Margin");
        assert_eq!(rules.value_rules[0].value, "10,2,3,2");
        assert!(!rules.value_rules[0].is_xaml_value);
        assert_eq!(rules.value_rules[1].property_name, "BorderThickness");
        // Deliberate deviation from the reference (`0.5,1,0.5,1`): the hairline
        // renders as a grey border around the floating dock. See the comment in
        // `theme::WINDOW_GLASS_SPLIT`.
        assert_eq!(rules.value_rules[1].value, "0");
        assert_eq!(rules.value_rules[2].property_name, "CornerRadius");
        assert_eq!(rules.value_rules[2].value, "25,5,5,25");
        assert_eq!(rules.value_rules[3].property_name, "Background");
        assert!(rules.value_rules[3].is_xaml_value);
        assert_eq!(rules.value_rules[3].value, GLASS);
        assert_eq!(rules.value_rules[4].property_name, "Padding");
        assert_eq!(rules.value_rules[4].value, "10,0,0,0");
    }

    #[test]
    fn theme_taskbar_frame_rule_is_verbatim() {
        let engine = engine();
        let rules = engine
            .rules
            .iter()
            .find(|r| {
                r.parent_element_matchers.is_empty()
                    && r.element_matcher.type_name == "Taskbar.TaskbarFrame"
            })
            .expect("TaskbarFrame rule");
        assert_eq!(rules.value_rules.len(), 4);

        assert_eq!(rules.value_rules[0].property_name, "Grid.Column");
        assert_eq!(rules.value_rules[0].value, "1");
        assert_eq!(rules.value_rules[0].visual_state, "");

        assert_eq!(rules.value_rules[1].property_name, "MaxWidth");
        assert!(rules.value_rules[1].is_dynamic());
        assert_eq!(
            rules.value_rules[1].value,
            "{{containerGridWidth>0?min(1895,containerGridWidth):1895}}"
        );
        assert!(!rules.value_rules[1].is_xaml_value);

        assert_eq!(rules.value_rules[2].property_name, "Width");
        assert!(rules.value_rules[2].is_dynamic());
        assert_eq!(
            rules.value_rules[2].value,
            "{{taskbarDock==`vertical`?skip():`Auto`}}"
        );

        assert_eq!(rules.value_rules[3].property_name, "MinWidth");
        assert!(rules.value_rules[3].is_xaml_value);
        assert_eq!(rules.value_rules[3].value, "100");
    }

    #[test]
    fn theme_docking_states_rule_and_capture() {
        let engine = engine();
        let rules = engine
            .rules
            .iter()
            .find(|r| {
                r.element_matcher.name == "RootGrid"
                    && r.element_matcher.visual_state_group.as_deref() == Some("DockingStates")
            })
            .expect("@DockingStates rule");
        assert_eq!(rules.parent_element_matchers.len(), 1);
        assert_eq!(rules.value_rules.len(), 3);
        assert_eq!(rules.value_rules[0].property_name, "Tag");
        assert_eq!(rules.value_rules[0].visual_state, "");
        assert_eq!(rules.value_rules[0].value, "horizontal");
        assert_eq!(rules.value_rules[1].visual_state, "DockedLeft");
        assert_eq!(rules.value_rules[1].value, "vertical");
        assert_eq!(rules.value_rules[2].visual_state, "DockedRight");
        assert_eq!(rules.value_rules[2].value, "vertical");
        assert_eq!(rules.capture_rules.len(), 1);
        assert_eq!(rules.capture_rules[0].property_name, "Tag");
        assert_eq!(rules.capture_rules[0].var_name, "taskbarDock");
    }

    #[test]
    fn theme_root_scrollviewer_rule_has_capture() {
        let engine = engine();
        let rules = engine
            .rules
            .iter()
            .find(|r| {
                r.parent_element_matchers
                    .last()
                    .map(|m| m.kind == MatcherKind::Root)
                    .unwrap_or(false)
            })
            .expect(":root rule");
        // Right-to-left: Grid, Border, ScrollContentPresenter, ScrollViewer, :root.
        assert_eq!(rules.parent_element_matchers.len(), 4);
        assert_eq!(
            rules.parent_element_matchers[0].type_name,
            "Windows.UI.Xaml.Controls.Border"
        );
        assert_eq!(
            rules.parent_element_matchers[1].type_name,
            "Windows.UI.Xaml.Controls.ScrollContentPresenter"
        );
        assert_eq!(
            rules.parent_element_matchers[2].type_name,
            "Windows.UI.Xaml.Controls.ScrollViewer"
        );
        assert_eq!(rules.parent_element_matchers[3].kind, MatcherKind::Root);

        assert_eq!(rules.value_rules.len(), 1);
        assert_eq!(rules.value_rules[0].property_name, "ColumnDefinitions");
        assert!(rules.value_rules[0].is_xaml_value);
        assert_eq!(
            rules.value_rules[0].value,
            "<ColumnDefinitionCollection><ColumnDefinition Width=\"*\"/>\
             <ColumnDefinition Width=\"Auto\"/><ColumnDefinition Width=\"Auto\"/>\
             <ColumnDefinition Width=\"*\"/></ColumnDefinitionCollection>"
        );
        assert_eq!(rules.capture_rules.len(), 1);
        assert_eq!(rules.capture_rules[0].property_name, "ActualWidth");
        assert_eq!(rules.capture_rules[0].var_name, "containerGridWidth");
    }

    #[test]
    fn theme_property_filter_rule_is_verbatim() {
        let engine = engine();
        let rules = engine
            .rules
            .iter()
            .find(|r| !r.element_matcher.property_values.is_empty())
            .expect("[Text=...] rule");
        assert_eq!(rules.element_matcher.name, "InnerTextBlock");
        assert_eq!(
            rules.element_matcher.property_values,
            vec![("Text".to_string(), "\u{e971}".to_string())]
        );
        assert_eq!(rules.value_rules.len(), 1);
        assert_eq!(rules.value_rules[0].property_name, "Text");
        assert_eq!(rules.value_rules[0].value, "\u{ed14}");
    }

    #[test]
    fn theme_comma_target_entry_expands_to_four_rules() {
        let engine = engine();
        let corner_10: Vec<&ElementCustomizationRules> = engine
            .rules
            .iter()
            .filter(|r| {
                r.value_rules.len() == 1
                    && r.value_rules[0].property_name == "CornerRadius"
                    && r.value_rules[0].value == "10"
                    && r.element_matcher.name == "BackgroundElement"
            })
            .collect();
        // The 4-target entry, plus `Taskbar.TaskListButton#TaskListButton`
        // (also CornerRadius=10, but not named BackgroundElement).
        assert_eq!(corner_10.len(), 4);

        let classes: Vec<&str> = corner_10
            .iter()
            .map(|r| r.parent_element_matchers[0].type_name.as_str())
            .collect();
        assert_eq!(
            classes,
            vec![
                "Windows.UI.Xaml.Controls.Grid",
                "Taskbar.TaskListButtonPanel",
                "Windows.UI.Xaml.Controls.Grid",
                "SearchUx.SearchUI.SearchButtonRootGrid",
            ]
        );
    }

    #[test]
    fn theme_two_target_entry_expands_to_two_rules() {
        let engine = engine();
        let rules: Vec<&ElementCustomizationRules> = engine
            .rules
            .iter()
            .filter(|r| r.element_matcher.name == "SystemTrayFrameGrid")
            .collect();
        assert_eq!(rules.len(), 2);
        let types: Vec<&str> = rules
            .iter()
            .map(|r| r.element_matcher.type_name.as_str())
            .collect();
        assert_eq!(
            types,
            vec![
                "Windows.UI.Xaml.Controls.StackPanel",
                "Windows.UI.Xaml.Controls.Grid",
            ]
        );
        for rule in rules {
            assert_eq!(rule.value_rules[0].property_name, "Margin");
            assert_eq!(rule.value_rules[0].value, "0,2,3,2");
            assert_eq!(rule.value_rules[4].property_name, "CornerRadius");
            assert_eq!(rule.value_rules[4].value, "5,25,25,5");
        }
    }

    // -- selector matching --------------------------------------------------

    #[test]
    fn matches_bare_element_type() {
        let engine = engine();
        let mut lookup = MockLookup::new();
        lookup.add(1, "Taskbar.TaskbarFrame", "", None);
        let matched = engine.match_element(1, &lookup);
        assert_eq!(
            one_override(&matched, "Grid.Column").value,
            ValueSpec::Literal("1".into())
        );
        assert!(overrides_for(&matched, "MaxWidth")[0]
            .value
            .clone()
            .eq(&ValueSpec::Dynamic {
                property: "MaxWidth".into(),
                raw: "{{containerGridWidth>0?min(1895,containerGridWidth):1895}}".into(),
                is_xaml: false,
            }));

        // A different class name doesn't match.
        let mut other = MockLookup::new();
        other.add(1, "Windows.UI.Xaml.Controls.Grid", "", None);
        assert!(engine.match_element(1, &other).overrides.is_empty());
    }

    #[test]
    fn matches_parent_chain() {
        let engine = engine();
        let mut lookup = MockLookup::new();
        lookup.add(1, "Taskbar.TaskbarFrame", "", None);
        lookup.add(2, "Windows.UI.Xaml.Controls.Grid", "RootGrid", Some(1));
        let matched = engine.match_element(2, &lookup);
        assert_eq!(
            one_override(&matched, "Margin").value,
            ValueSpec::Literal("10,2,3,2".into())
        );
        assert_eq!(
            one_override(&matched, "CornerRadius").value,
            ValueSpec::Literal("25,5,5,25".into())
        );
        assert_eq!(
            one_override(&matched, "Background").value,
            ValueSpec::Xaml(GLASS.into())
        );
        assert_eq!(
            one_override(&matched, "BorderThickness").value,
            // The reference hairline is deliberately dropped (see the theme).
            ValueSpec::Literal("0".into())
        );
        assert_eq!(
            one_override(&matched, "Padding").value,
            ValueSpec::Literal("10,0,0,0".into())
        );

        // Without the expected parent the chain doesn't match.
        let mut other = MockLookup::new();
        other.add(1, "Windows.UI.Xaml.Controls.Panel", "", None);
        other.add(2, "Windows.UI.Xaml.Controls.Grid", "RootGrid", Some(1));
        assert!(engine.match_element(2, &other).overrides.is_empty());
    }

    #[test]
    fn matches_root_scrollviewer_chain() {
        let engine = engine();
        let mut lookup = MockLookup::new();
        // ScrollViewer is the root (no parent), then the chain down to Grid.
        lookup.add(1, "Windows.UI.Xaml.Controls.ScrollViewer", "", None);
        lookup.add(
            2,
            "Windows.UI.Xaml.Controls.ScrollContentPresenter",
            "",
            Some(1),
        );
        lookup.add(3, "Windows.UI.Xaml.Controls.Border", "", Some(2));
        lookup.add(4, "Windows.UI.Xaml.Controls.Grid", "", Some(3));

        let matched = engine.match_element(4, &lookup);
        assert_eq!(
            one_override(&matched, "ColumnDefinitions").value,
            ValueSpec::Xaml(
                "<ColumnDefinitionCollection><ColumnDefinition Width=\"*\"/>\
                 <ColumnDefinition Width=\"Auto\"/><ColumnDefinition Width=\"Auto\"/>\
                 <ColumnDefinition Width=\"*\"/></ColumnDefinitionCollection>"
                    .into()
            )
        );
        assert_eq!(matched.captures.len(), 1);
        assert_eq!(matched.captures[0].property, "ActualWidth");
        assert_eq!(matched.captures[0].variable, "containerGridWidth");

        // ':root' requires the ScrollViewer to have no parent.
        let mut nested = MockLookup::new();
        nested.add(1, "Windows.UI.Xaml.Controls.Grid", "", None);
        nested.add(2, "Windows.UI.Xaml.Controls.ScrollViewer", "", Some(1));
        nested.add(
            3,
            "Windows.UI.Xaml.Controls.ScrollContentPresenter",
            "",
            Some(2),
        );
        nested.add(4, "Windows.UI.Xaml.Controls.Border", "", Some(3));
        nested.add(5, "Windows.UI.Xaml.Controls.Grid", "", Some(4));
        assert!(engine.match_element(5, &nested).overrides.is_empty());
    }

    #[test]
    fn matches_long_wildcard_less_chain() {
        let engine = engine();
        let mut lookup = MockLookup::new();
        // Element: SystemTray.OmniButton#NotificationCenterButton >
        // Grid > ContentPresenter > ItemsPresenter > StackPanel >
        // ContentPresenter > SystemTray.IconView#SystemTrayIcon > Grid
        lookup.add(1, "SystemTray.OmniButton", "NotificationCenterButton", None);
        lookup.add(2, "Windows.UI.Xaml.Controls.Grid", "", Some(1));
        lookup.add(3, "Windows.UI.Xaml.Controls.ContentPresenter", "", Some(2));
        lookup.add(4, "Windows.UI.Xaml.Controls.ItemsPresenter", "", Some(3));
        lookup.add(5, "Windows.UI.Xaml.Controls.StackPanel", "", Some(4));
        lookup.add(6, "Windows.UI.Xaml.Controls.ContentPresenter", "", Some(5));
        lookup.add(7, "SystemTray.IconView", "SystemTrayIcon", Some(6));
        lookup.add(8, "Windows.UI.Xaml.Controls.Grid", "", Some(7));

        let matched = engine.match_element(8, &lookup);
        assert_eq!(
            one_override(&matched, "Padding").value,
            ValueSpec::Literal("2,4,2,4".into())
        );

        // The innermost Grid's own child keeps the shorter rule instead.
        lookup.add(9, "Windows.UI.Xaml.Controls.Grid", "", Some(8));
        assert!(engine.match_element(9, &lookup).overrides.is_empty());

        // A wrong intermediate type breaks the chain.
        let mut broken = MockLookup::new();
        broken.add(1, "SystemTray.OmniButton", "NotificationCenterButton", None);
        broken.add(2, "Windows.UI.Xaml.Controls.Grid", "", Some(1));
        broken.add(3, "Windows.UI.Xaml.Controls.ContentPresenter", "", Some(2));
        broken.add(4, "Windows.UI.Xaml.Controls.ItemsPresenter", "", Some(3));
        broken.add(5, "Windows.UI.Xaml.Controls.StackPanel", "", Some(4));
        broken.add(
            6,
            "Windows.UI.Xaml.Controls.Border", // not a ContentPresenter
            "",
            Some(5),
        );
        broken.add(7, "SystemTray.IconView", "SystemTrayIcon", Some(6));
        broken.add(8, "Windows.UI.Xaml.Controls.Grid", "", Some(7));
        assert!(engine.match_element(8, &broken).overrides.is_empty());
    }

    #[test]
    fn matches_property_filter() {
        let engine = engine();
        let mut hit = MockLookup::new();
        hit.add(
            1,
            "Windows.UI.Xaml.Controls.TextBlock",
            "InnerTextBlock",
            None,
        );
        hit = hit.with_value(1, "Text", PropValue::Text("\u{e971}".into()));
        let matched = engine.match_element(1, &hit);
        assert_eq!(
            one_override(&matched, "Text").value,
            ValueSpec::Literal("\u{ed14}".into())
        );

        // Wrong local value doesn't match.
        let mut miss = MockLookup::new();
        miss.add(
            1,
            "Windows.UI.Xaml.Controls.TextBlock",
            "InnerTextBlock",
            None,
        );
        miss = miss.with_value(1, "Text", PropValue::Text("x".into()));
        assert!(engine.match_element(1, &miss).overrides.is_empty());

        // Missing local value doesn't match.
        let mut missing = MockLookup::new();
        missing.add(
            1,
            "Windows.UI.Xaml.Controls.TextBlock",
            "InnerTextBlock",
            None,
        );
        assert!(engine.match_element(1, &missing).overrides.is_empty());
    }

    #[test]
    fn property_filter_compares_numbers_and_bools() {
        // Synthetic theme so the filter types are exercised directly.
        let theme = Theme {
            target_styles: &[
                TargetStyles {
                    target: "Grid#G[Grid.Column=1]",
                    styles: &["Margin=1"],
                },
                TargetStyles {
                    target: "Grid#H[IsHitTestVisible=True]",
                    styles: &["Margin=2"],
                },
            ],
            style_constants: &[],
        };
        let engine = RuleEngine::new(&theme);

        let mut lookup = MockLookup::new();
        lookup.add(1, "Windows.UI.Xaml.Controls.Grid", "G", None);
        lookup = lookup.with_value(1, "Grid.Column", PropValue::Number(1.0));
        assert!(!engine.match_element(1, &lookup).overrides.is_empty());

        let mut wrong = MockLookup::new();
        wrong.add(1, "Windows.UI.Xaml.Controls.Grid", "G", None);
        wrong = wrong.with_value(1, "Grid.Column", PropValue::Number(2.0));
        assert!(engine.match_element(1, &wrong).overrides.is_empty());

        let mut bool_hit = MockLookup::new();
        bool_hit.add(1, "Windows.UI.Xaml.Controls.Grid", "H", None);
        bool_hit = bool_hit.with_value(1, "IsHitTestVisible", PropValue::Bool(true));
        assert!(!engine.match_element(1, &bool_hit).overrides.is_empty());

        let mut opaque = MockLookup::new();
        opaque.add(1, "Windows.UI.Xaml.Controls.Grid", "G", None);
        opaque = opaque.with_value(1, "Grid.Column", PropValue::Other);
        assert!(engine.match_element(1, &opaque).overrides.is_empty());
    }

    #[test]
    fn matches_one_based_child_index() {
        let theme = Theme {
            target_styles: &[TargetStyles {
                target: "Grid#Parent > Border[2]",
                styles: &["Margin=7"],
            }],
            style_constants: &[],
        };
        let engine = RuleEngine::new(&theme);

        let mut lookup = MockLookup::new();
        lookup.add(1, "Windows.UI.Xaml.Controls.Grid", "Parent", None);
        lookup.add(2, "Windows.UI.Xaml.Controls.Border", "", Some(1)); // index 0
        lookup.add(3, "Windows.UI.Xaml.Controls.Border", "", Some(1)); // index 1 = [2]
        lookup.add(4, "Windows.UI.Xaml.Controls.Border", "", Some(1)); // index 2

        assert_eq!(
            one_override(&engine.match_element(3, &lookup), "Margin").value,
            ValueSpec::Literal("7".into())
        );
        assert!(engine.match_element(2, &lookup).overrides.is_empty());
        assert!(engine.match_element(4, &lookup).overrides.is_empty());

        // The element has no parent, so an index can never match.
        let mut root = MockLookup::new();
        root.add(9, "Windows.UI.Xaml.Controls.Border", "", None);
        assert!(engine.match_element(9, &root).overrides.is_empty());
    }

    #[test]
    fn matches_wildcard_and_backtracks() {
        let theme = Theme {
            target_styles: &[
                TargetStyles {
                    target: "A > * > B#x",
                    styles: &["Margin=1"],
                },
                TargetStyles {
                    target: "A > C",
                    styles: &["Padding=2"],
                },
            ],
            style_constants: &[],
        };
        let engine = RuleEngine::new(&theme);

        // B#x parent M (not A), M parent A: the wildcard consumes M.
        let mut lookup = MockLookup::new();
        lookup.add(1, "Windows.UI.Xaml.Controls.A", "", None);
        lookup.add(2, "Windows.UI.Xaml.Controls.M", "", Some(1));
        lookup.add(3, "Windows.UI.Xaml.Controls.B", "x", Some(2));
        let matched = engine.match_element(3, &lookup);
        assert_eq!(
            one_override(&matched, "Margin").value,
            ValueSpec::Literal("1".into())
        );

        // Many intermediate ancestors: '* ' backtracks up to A.
        let mut deep = MockLookup::new();
        deep.add(1, "Windows.UI.Xaml.Controls.A", "", None);
        deep.add(2, "Windows.UI.Xaml.Controls.X", "", Some(1));
        deep.add(3, "Windows.UI.Xaml.Controls.Y", "", Some(2));
        deep.add(4, "Windows.UI.Xaml.Controls.B", "x", Some(3));
        assert!(!engine.match_element(4, &deep).overrides.is_empty());

        // No A ancestor at all: no match.
        let mut none = MockLookup::new();
        none.add(1, "Windows.UI.Xaml.Controls.Z", "", None);
        none.add(2, "Windows.UI.Xaml.Controls.B", "x", Some(1));
        assert!(engine.match_element(2, &none).overrides.is_empty());

        // Plain parent chain.
        let mut a_to_c = MockLookup::new();
        a_to_c.add(1, "Windows.UI.Xaml.Controls.A", "", None);
        a_to_c.add(2, "Windows.UI.Xaml.Controls.C", "", Some(1));
        assert_eq!(
            one_override(&engine.match_element(2, &a_to_c), "Padding").value,
            ValueSpec::Literal("2".into())
        );
    }

    #[test]
    fn matches_visual_state_group_rule() {
        let engine = engine();
        let mut lookup = MockLookup::new();
        lookup.add(1, "Taskbar.TaskbarFrame", "", None);
        lookup.add(2, "Windows.UI.Xaml.Controls.Grid", "RootGrid", Some(1));
        lookup = lookup.with_vsg(2, "DockingStates");

        let matched = engine.match_element(2, &lookup);
        let tags = overrides_for(&matched, "Tag");
        assert_eq!(tags.len(), 3);
        assert_eq!(
            tags.iter()
                .map(|o| (o.visual_state.as_str(), value_text(&o.value)))
                .collect::<Vec<_>>(),
            vec![
                ("", "horizontal".to_string()),
                ("DockedLeft", "vertical".to_string()),
                ("DockedRight", "vertical".to_string()),
            ]
        );
        assert_eq!(matched.captures.len(), 1);
        assert_eq!(matched.captures[0].variable, "taskbarDock");
        assert_eq!(matched.captures[0].property, "Tag");

        // `@Group` presence does not gate the match (reference behavior: the
        // group is only resolved as a bucket key).
        let no_vsg_calls = lookup.vsg_calls.borrow().clone();
        assert!(no_vsg_calls.contains(&(2, "DockingStates".to_string())));
    }

    #[test]
    fn visual_state_group_is_resolved_through_lookup() {
        let theme = Theme {
            target_styles: &[TargetStyles {
                target: "Grid#G@States",
                styles: &["Margin=1"],
            }],
            style_constants: &[],
        };
        let engine = RuleEngine::new(&theme);
        let mut lookup = MockLookup::new();
        lookup.add(1, "Windows.UI.Xaml.Controls.Grid", "G", None);
        lookup = lookup.with_vsg(1, "States");
        assert!(!engine.match_element(1, &lookup).overrides.is_empty());
        assert_eq!(
            lookup.vsg_calls.borrow().as_slice(),
            &[(1u64, "States".to_string())]
        );
    }

    #[test]
    fn first_wins_ordering() {
        // Two rules for the same element: the later rule in the list is seen
        // first by the backwards walk, so it wins for the properties it sets.
        let theme = Theme {
            target_styles: &[
                TargetStyles {
                    target: "Grid#G",
                    styles: &["Margin=1", "Padding=1"],
                },
                TargetStyles {
                    target: "Grid#G",
                    styles: &["Margin=2", "CornerRadius=9"],
                },
            ],
            style_constants: &[],
        };
        let engine = RuleEngine::new(&theme);
        let mut lookup = MockLookup::new();
        lookup.add(1, "Windows.UI.Xaml.Controls.Grid", "G", None);

        let matched = engine.match_element(1, &lookup);
        assert_eq!(
            one_override(&matched, "Margin").value,
            ValueSpec::Literal("2".into())
        );
        assert_eq!(
            one_override(&matched, "Padding").value,
            ValueSpec::Literal("1".into())
        );
        assert_eq!(
            one_override(&matched, "CornerRadius").value,
            ValueSpec::Literal("9".into())
        );

        // Within one rule the first definition of a property wins.
        let theme = Theme {
            target_styles: &[TargetStyles {
                target: "Grid#G",
                styles: &["Margin=first", "Margin=second"],
            }],
            style_constants: &[],
        };
        let engine = RuleEngine::new(&theme);
        assert_eq!(
            one_override(&engine.match_element(1, &lookup), "Margin").value,
            ValueSpec::Literal("first".into())
        );
    }

    #[test]
    fn captures_are_deduplicated_by_variable_name() {
        let theme = Theme {
            target_styles: &[
                TargetStyles {
                    target: "Grid#G",
                    styles: &["Width=>w"],
                },
                TargetStyles {
                    target: "Grid#G",
                    styles: &["Height=>w"],
                },
            ],
            style_constants: &[],
        };
        let engine = RuleEngine::new(&theme);
        let mut lookup = MockLookup::new();
        lookup.add(1, "Windows.UI.Xaml.Controls.Grid", "G", None);
        let matched = engine.match_element(1, &lookup);
        assert_eq!(matched.captures.len(), 1);
        // Backwards walk: the later rule (Height) wins.
        assert_eq!(matched.captures[0].property, "Height");
        assert_eq!(matched.captures[0].variable, "w");
    }

    #[test]
    fn type_name_adjustment() {
        let theme = Theme {
            target_styles: &[
                TargetStyles {
                    target: "Rectangle",
                    styles: &["Margin=1"],
                },
                TargetStyles {
                    target: "taskbar:TaskbarFrame",
                    styles: &["Margin=2"],
                },
                TargetStyles {
                    target: "systemtray:SystemTrayFrame",
                    styles: &["Margin=3"],
                },
                TargetStyles {
                    target: "udk:Foo",
                    styles: &["Margin=4"],
                },
                TargetStyles {
                    target: "muxc:Bar",
                    styles: &["Margin=5"],
                },
            ],
            style_constants: &[],
        };
        let engine = RuleEngine::new(&theme);
        let types: Vec<&str> = engine
            .rules
            .iter()
            .map(|r| r.element_matcher.type_name.as_str())
            .collect();
        assert_eq!(
            types,
            vec![
                "Windows.UI.Xaml.Shapes.Rectangle",
                "Taskbar.TaskbarFrame",
                "SystemTray.SystemTrayFrame",
                "WindowsUdk.UI.Shell.Foo",
                "Microsoft.UI.Xaml.Controls.Bar",
            ]
        );
    }

    #[test]
    fn malformed_targets_and_rules_are_skipped() {
        let theme = Theme {
            target_styles: &[
                // Missing '=' in a rule drops the whole target.
                TargetStyles {
                    target: "Grid#G",
                    styles: &["Margin=1", "NoEquals"],
                },
                // '*' can't be the matched element.
                TargetStyles {
                    target: "*",
                    styles: &["Margin=1"],
                },
                // ':root' must be the leftmost part.
                TargetStyles {
                    target: "A > :root > B",
                    styles: &["Margin=1"],
                },
                // Two visual state groups.
                TargetStyles {
                    target: "Grid#G@A@B",
                    styles: &["Margin=1"],
                },
                // Bad capture variable name.
                TargetStyles {
                    target: "Grid#G",
                    styles: &["Width=>not-a-name"],
                },
                // Commented entries are skipped.
                TargetStyles {
                    target: "//Grid#G",
                    styles: &["Margin=1"],
                },
                TargetStyles {
                    target: "Grid#G",
                    styles: &["//Margin=1"],
                },
                // One valid rule.
                TargetStyles {
                    target: "Grid#G",
                    styles: &["Margin=42"],
                },
            ],
            style_constants: &[],
        };
        let engine = RuleEngine::new(&theme);
        assert_eq!(engine.rules.len(), 1);
        assert_eq!(engine.rules[0].value_rules.len(), 1);
        assert_eq!(engine.rules[0].value_rules[0].value, "42");
    }

    #[test]
    fn style_constants_longest_first_and_later_overrides() {
        let theme = Theme {
            target_styles: &[TargetStyles {
                target: "Grid#G",
                styles: &["Margin=$AB,$A", "Padding=$C"],
            }],
            style_constants: &[
                // Deliberately out of order; same-name definition later wins.
                "A=a1",
                "AB=ab1",
                "A=a2",
                // Commented-out constants are skipped entirely.
                "//C=commented",
            ],
        };
        let engine = RuleEngine::new(&theme);
        assert_eq!(engine.rules.len(), 1);
        // "AB" must win over "A" followed by "B"; later "A=a2" overrides "a1";
        // `$C` has no definition and is left untouched.
        assert_eq!(engine.rules[0].value_rules[0].value, "ab1,a2");
        assert_eq!(engine.rules[0].value_rules[1].value, "$C");
    }

    #[test]
    fn dynamic_rules_are_marked_and_keep_property() {
        let engine = engine();
        let mut lookup = MockLookup::new();
        lookup.add(1, "Taskbar.TaskbarFrame", "", None);
        let matched = engine.match_element(1, &lookup);

        let max_width = one_override(&matched, "MaxWidth");
        assert_eq!(
            max_width.value,
            ValueSpec::Dynamic {
                property: "MaxWidth".into(),
                raw: "{{containerGridWidth>0?min(1895,containerGridWidth):1895}}".into(),
                is_xaml: false,
            }
        );
        let width = one_override(&matched, "Width");
        assert_eq!(
            width.value,
            ValueSpec::Dynamic {
                property: "Width".into(),
                raw: "{{taskbarDock==`vertical`?skip():`Auto`}}".into(),
                is_xaml: false,
            }
        );
        // XAML rules are not dynamic even though their text contains '='.
        assert_eq!(
            one_override(&matched, "MinWidth").value,
            ValueSpec::Xaml("100".into())
        );
    }

    #[test]
    fn engine_expand_expression_uses_captures() {
        let engine = engine();
        assert_eq!(
            engine
                .expand_expression(
                    "{{containerGridWidth>0?min(1895,containerGridWidth):1895}}",
                    &vars(&[("containerGridWidth", "800")]),
                )
                .as_deref(),
            Some("800")
        );
        assert_eq!(
            engine.expand_expression(
                "{{taskbarDock==`vertical`?skip():`Auto`}}",
                &vars(&[("taskbarDock", "vertical")]),
            ),
            None
        );
        assert_eq!(
            engine
                .expand_expression(
                    "{{taskbarDock==`vertical`?skip():`Auto`}}",
                    &vars(&[("taskbarDock", "horizontal")]),
                )
                .as_deref(),
            Some("Auto")
        );
    }

    #[test]
    fn unmatched_elements_match_nothing() {
        let engine = engine();
        let lookup = MockLookup::new();
        let matched = engine.match_element(123, &lookup);
        assert!(matched.overrides.is_empty());
        assert!(matched.captures.is_empty());
    }

    // -----------------------------------------------------------------------
    // Golden reference: synthetic Windows 11 taskbar tree
    //
    // This is the contract the `dcx-engine` adapter is checked against: given
    // the live element facts below, `RuleEngine::match_element` must produce
    // exactly these `MatchedRules`.
    //
    // Tree shape, from the live capture in `docs/evidence/taskbar-tree.log`
    // (element ids are the live instance handles there):
    //
    //   #0      Windows.UI.Xaml.Hosting.DesktopWindowXamlSource  (tree root)
    //   #2      Windows.UI.Xaml.Internal.RootScrollViewer        (visual-tree root)
    //   #3         Windows.UI.Xaml.Controls.ScrollContentPresenter
    //   #4            Windows.UI.Xaml.Controls.Border
    //   #5               Windows.UI.Xaml.Controls.Grid     <- ":root > ScrollViewer > ... > Grid"
    //   #6                  Taskbar.TaskbarFrame
    //   #8                     Windows.UI.Xaml.Controls.Grid #RootGrid
    //   #10                       Taskbar.TaskbarBackground #BackgroundControl
    //   #22                          Windows.UI.Xaml.Controls.Grid
    //   #45/#46                        Rectangle #BackgroundFill / #BackgroundStroke
    //   #7                  SystemTray.SystemTrayFrame
    //   #9                     Windows.UI.Xaml.Controls.Grid #SystemTrayFrameGrid
    //   #19/#20                   SystemTray.OmniButton #ControlCenterButton/#NotificationCenterButton
    //
    // Two element-fact details matter and are modelled here:
    //
    // 1. `ElementLookup::class_name` must be `winrt::get_class_name` (the
    //    runtime class name), NOT the TAP `VisualElement.Type`. The reference
    //    matches parent parts with a null fallback class name
    //    (`TestElementMatcher(parent, matcher, &visualStateGroup, nullptr)`,
    //    reference 16078-16079; same for the wildcard path at 16064-16065), and
    //    the live TAP reports the root element's Type as
    //    `Windows.UI.Xaml.Internal.RootScrollViewer` while its runtime class
    //    name is the public `Windows.UI.Xaml.Controls.ScrollViewer`. Without
    //    that, the theme's `ScrollViewer` part cannot match and the split
    //    layout would never be applied.
    // 2. `ElementLookup::parent` must be `VisualTreeHelper.GetParent`, not the
    //    TAP `ParentChildRelation.Parent` (the TAP parent of the root scroll
    //    viewer is `DesktopWindowXamlSource`, which is not a visual parent
    //    and would make `:root` fail). Reference 16043/16057/16072.
    // -----------------------------------------------------------------------

    // Handles of the golden tree.
    const TREE_ROOT_SV: u64 = 1;
    const TREE_SCP: u64 = 2;
    const TREE_BORDER: u64 = 3;
    const TREE_COLUMN_GRID: u64 = 4;
    const TREE_TASKBAR_FRAME: u64 = 5;
    const TREE_ROOT_GRID: u64 = 6;
    const TREE_BACKGROUND_CONTROL: u64 = 7;
    const TREE_BACKGROUND_GRID: u64 = 8;
    const TREE_BACKGROUND_FILL: u64 = 9;
    const TREE_BACKGROUND_STROKE: u64 = 10;
    const TREE_SYSTEM_TRAY_FRAME: u64 = 11;
    const TREE_SYSTEM_TRAY_GRID: u64 = 12;
    const TREE_CONTROL_CENTER_BUTTON: u64 = 13;
    const TREE_CONTROL_CENTER_GRID: u64 = 14;
    const TREE_NOTIFICATION_CENTER_BUTTON: u64 = 15;
    const TREE_NOTIFICATION_CENTER_GRID: u64 = 16;
    const TREE_CHAIN_CP1: u64 = 17;
    const TREE_CHAIN_ITEMS: u64 = 18;
    const TREE_CHAIN_STACK: u64 = 19;
    const TREE_CHAIN_CP2: u64 = 20;
    const TREE_ICON_VIEW: u64 = 21;
    const TREE_ICON_GRID: u64 = 22;
    const TREE_ICON_GRID2: u64 = 23;
    const TREE_TEXT_ICON_CONTENT: u64 = 24;
    const TREE_TEXT_BLOCK: u64 = 25;

    const CLS_GRID: &str = "Windows.UI.Xaml.Controls.Grid";
    const CLS_RECTANGLE: &str = "Windows.UI.Xaml.Shapes.Rectangle";
    const CLS_BORDER: &str = "Windows.UI.Xaml.Controls.Border";
    const CLS_SCROLL_CONTENT_PRESENTER: &str = "Windows.UI.Xaml.Controls.ScrollContentPresenter";
    /// Runtime class name of the live `Windows.UI.Xaml.Internal.RootScrollViewer`.
    const CLS_ROOT_SCROLL_VIEWER: &str = "Windows.UI.Xaml.Controls.ScrollViewer";
    const CLS_CONTENT_PRESENTER: &str = "Windows.UI.Xaml.Controls.ContentPresenter";
    const CLS_ITEMS_PRESENTER: &str = "Windows.UI.Xaml.Controls.ItemsPresenter";
    const CLS_STACK_PANEL: &str = "Windows.UI.Xaml.Controls.StackPanel";

    /// `$Glass` after ApplyStyleConstants.
    const GOLDEN_GLASS: &str = r#"<WindhawkBlur BlurAmount="5" TintColor="{ThemeResource SystemChromeMediumColor}" TintOpacity="0.7" />"#;
    /// `$BorderBrush` after ApplyStyleConstants.
    const GOLDEN_BORDER_BRUSH: &str = r##"<LinearGradientBrush StartPoint="0,0" EndPoint="0,1"><GradientStop Color="#50808080" Offset="0.0" /><GradientStop Color="#30404040" Offset="0.25" /><GradientStop Color="#40808080" Offset="1" /></LinearGradientBrush>"##;
    /// The `:root > ... > Grid` rule's `ColumnDefinitions:=` XAML value.
    const GOLDEN_COLUMN_DEFINITIONS: &str = r#"<ColumnDefinitionCollection><ColumnDefinition Width="*"/><ColumnDefinition Width="Auto"/><ColumnDefinition Width="Auto"/><ColumnDefinition Width="*"/></ColumnDefinitionCollection>"#;

    fn golden_taskbar_lookup() -> MockLookup {
        let mut lookup = MockLookup::new();

        // Visual-tree root: the live RootScrollViewer, whose runtime class name
        // is the public ScrollViewer name (see the module comment above).
        lookup.add(TREE_ROOT_SV, CLS_ROOT_SCROLL_VIEWER, "", None);
        lookup = lookup.with_tap_type(TREE_ROOT_SV, "Windows.UI.Xaml.Internal.RootScrollViewer");
        lookup.add(
            TREE_SCP,
            CLS_SCROLL_CONTENT_PRESENTER,
            "",
            Some(TREE_ROOT_SV),
        );
        lookup.add(TREE_BORDER, CLS_BORDER, "", Some(TREE_SCP));
        lookup.add(TREE_COLUMN_GRID, CLS_GRID, "", Some(TREE_BORDER));

        // Left pill.
        lookup.add(
            TREE_TASKBAR_FRAME,
            "Taskbar.TaskbarFrame",
            "",
            Some(TREE_COLUMN_GRID),
        );
        lookup.add(
            TREE_ROOT_GRID,
            CLS_GRID,
            "RootGrid",
            Some(TREE_TASKBAR_FRAME),
        );
        lookup = lookup.with_vsg(TREE_ROOT_GRID, "DockingStates");
        lookup.add(
            TREE_BACKGROUND_CONTROL,
            "Taskbar.TaskbarBackground",
            "BackgroundControl",
            Some(TREE_ROOT_GRID),
        );
        lookup.add(
            TREE_BACKGROUND_GRID,
            CLS_GRID,
            "",
            Some(TREE_BACKGROUND_CONTROL),
        );
        lookup.add(
            TREE_BACKGROUND_FILL,
            CLS_RECTANGLE,
            "BackgroundFill",
            Some(TREE_BACKGROUND_GRID),
        );
        lookup.add(
            TREE_BACKGROUND_STROKE,
            CLS_RECTANGLE,
            "BackgroundStroke",
            Some(TREE_BACKGROUND_GRID),
        );

        // Right pill.
        lookup.add(
            TREE_SYSTEM_TRAY_FRAME,
            "SystemTray.SystemTrayFrame",
            "",
            Some(TREE_COLUMN_GRID),
        );
        lookup.add(
            TREE_SYSTEM_TRAY_GRID,
            CLS_GRID,
            "SystemTrayFrameGrid",
            Some(TREE_SYSTEM_TRAY_FRAME),
        );
        lookup.add(
            TREE_CONTROL_CENTER_BUTTON,
            "SystemTray.OmniButton",
            "ControlCenterButton",
            Some(TREE_SYSTEM_TRAY_GRID),
        );
        lookup.add(
            TREE_CONTROL_CENTER_GRID,
            CLS_GRID,
            "",
            Some(TREE_CONTROL_CENTER_BUTTON),
        );
        lookup.add(
            TREE_NOTIFICATION_CENTER_BUTTON,
            "SystemTray.OmniButton",
            "NotificationCenterButton",
            Some(TREE_SYSTEM_TRAY_GRID),
        );

        // `... > SystemTray.TextIconContent` ancestor chain.
        lookup.add(
            TREE_NOTIFICATION_CENTER_GRID,
            CLS_GRID,
            "",
            Some(TREE_NOTIFICATION_CENTER_BUTTON),
        );
        lookup.add(
            TREE_CHAIN_CP1,
            CLS_CONTENT_PRESENTER,
            "",
            Some(TREE_NOTIFICATION_CENTER_GRID),
        );
        lookup.add(
            TREE_CHAIN_ITEMS,
            CLS_ITEMS_PRESENTER,
            "",
            Some(TREE_CHAIN_CP1),
        );
        lookup.add(
            TREE_CHAIN_STACK,
            CLS_STACK_PANEL,
            "",
            Some(TREE_CHAIN_ITEMS),
        );
        lookup.add(
            TREE_CHAIN_CP2,
            CLS_CONTENT_PRESENTER,
            "",
            Some(TREE_CHAIN_STACK),
        );
        lookup.add(
            TREE_ICON_VIEW,
            "SystemTray.IconView",
            "SystemTrayIcon",
            Some(TREE_CHAIN_CP2),
        );
        lookup.add(TREE_ICON_GRID, CLS_GRID, "", Some(TREE_ICON_VIEW));
        lookup.add(TREE_ICON_GRID2, CLS_GRID, "", Some(TREE_ICON_GRID));
        lookup.add(
            TREE_TEXT_ICON_CONTENT,
            "SystemTray.TextIconContent",
            "",
            Some(TREE_ICON_GRID2),
        );

        // The `[Text=...]` target, matched outside the tree walk.
        lookup.add(
            TREE_TEXT_BLOCK,
            "Windows.UI.Xaml.Controls.TextBlock",
            "InnerTextBlock",
            None,
        );
        lookup.with_value(TREE_TEXT_BLOCK, "Text", PropValue::Text("\u{e971}".into()))
    }

    // -- expected-value builders -------------------------------------------

    fn literal(property: &str, value: &str) -> Override {
        Override {
            property: property.to_string(),
            visual_state: String::new(),
            value: ValueSpec::Literal(value.to_string()),
        }
    }

    fn xaml(property: &str, value: &str) -> Override {
        Override {
            property: property.to_string(),
            visual_state: String::new(),
            value: ValueSpec::Xaml(value.to_string()),
        }
    }

    fn dynamic(property: &str, raw: &str, is_xaml: bool) -> Override {
        Override {
            property: property.to_string(),
            visual_state: String::new(),
            value: ValueSpec::Dynamic {
                property: property.to_string(),
                raw: raw.to_string(),
                is_xaml,
            },
        }
    }

    fn state_literal(property: &str, visual_state: &str, value: &str) -> Override {
        Override {
            property: property.to_string(),
            visual_state: visual_state.to_string(),
            value: ValueSpec::Literal(value.to_string()),
        }
    }

    fn capture(property: &str, variable: &str) -> Capture {
        Capture {
            property: property.to_string(),
            variable: variable.to_string(),
        }
    }

    fn assert_matched(
        engine: &RuleEngine,
        lookup: &MockLookup,
        handle: u64,
        expected_overrides: Vec<Override>,
        expected_captures: Vec<Capture>,
    ) {
        let matched = engine.match_element(handle, lookup);
        assert_eq!(
            matched.overrides, expected_overrides,
            "overrides for handle {handle}"
        );
        assert_eq!(
            matched.captures, expected_captures,
            "captures for handle {handle}"
        );
    }

    fn assert_not_matched(engine: &RuleEngine, lookup: &MockLookup, handle: u64) {
        let matched = engine.match_element(handle, lookup);
        assert!(
            matched.overrides.is_empty(),
            "expected no overrides for handle {handle}, got {:?}",
            matched.overrides
        );
        assert!(
            matched.captures.is_empty(),
            "expected no captures for handle {handle}, got {:?}",
            matched.captures
        );
    }

    // -- golden assertions, one per element --------------------------------

    #[test]
    fn golden_column_grid() {
        let engine = engine();
        let lookup = golden_taskbar_lookup();
        assert_matched(
            &engine,
            &lookup,
            TREE_COLUMN_GRID,
            vec![xaml("ColumnDefinitions", GOLDEN_COLUMN_DEFINITIONS)],
            vec![capture("ActualWidth", "containerGridWidth")],
        );
    }

    #[test]
    fn golden_taskbar_frame() {
        let engine = engine();
        let lookup = golden_taskbar_lookup();
        assert_matched(
            &engine,
            &lookup,
            TREE_TASKBAR_FRAME,
            vec![
                literal("Grid.Column", "1"),
                dynamic(
                    "MaxWidth",
                    "{{containerGridWidth>0?min(1895,containerGridWidth):1895}}",
                    false,
                ),
                dynamic("Width", "{{taskbarDock==`vertical`?skip():`Auto`}}", false),
                xaml("MinWidth", "100"),
            ],
            vec![],
        );
    }

    #[test]
    fn golden_root_grid() {
        let engine = engine();
        let lookup = golden_taskbar_lookup();
        assert_matched(
            &engine,
            &lookup,
            TREE_ROOT_GRID,
            vec![
                literal("Margin", "10,2,3,2"),
                // The reference hairline is deliberately dropped: it draws a
                // grey line around the floating dock.
                literal("BorderThickness", "0"),
                literal("CornerRadius", "25,5,5,25"),
                xaml("Background", GOLDEN_GLASS),
                literal("Padding", "10,0,0,0"),
                state_literal("Tag", "", "horizontal"),
                state_literal("Tag", "DockedLeft", "vertical"),
                state_literal("Tag", "DockedRight", "vertical"),
            ],
            vec![capture("Tag", "taskbarDock")],
        );

        // The `@DockingStates` part is resolved through ElementLookup; the
        // engine must bucket the Tag rules by the group's current state itself.
        assert!(lookup
            .vsg_calls
            .borrow()
            .contains(&(TREE_ROOT_GRID, "DockingStates".to_string())));
    }

    #[test]
    fn golden_background_rectangles() {
        let engine = engine();
        let lookup = golden_taskbar_lookup();

        // Both `Rectangle#BackgroundFill` rules match: the name-only rule
        // (Fill:=$Background) and the parent-chain rule (Visibility=Collapsed).
        assert_matched(
            &engine,
            &lookup,
            TREE_BACKGROUND_FILL,
            vec![
                xaml("Fill", GOLDEN_GLASS),
                literal("Visibility", "Collapsed"),
            ],
            vec![],
        );
        assert_matched(
            &engine,
            &lookup,
            TREE_BACKGROUND_STROKE,
            vec![literal("Visibility", "Collapsed")],
            vec![],
        );
    }

    #[test]
    fn golden_system_tray_frame() {
        let engine = engine();
        let lookup = golden_taskbar_lookup();
        assert_matched(
            &engine,
            &lookup,
            TREE_SYSTEM_TRAY_FRAME,
            vec![literal("Grid.Column", "2")],
            vec![],
        );
    }

    #[test]
    fn golden_system_tray_frame_grid() {
        let engine = engine();
        let lookup = golden_taskbar_lookup();
        assert_matched(
            &engine,
            &lookup,
            TREE_SYSTEM_TRAY_GRID,
            vec![
                literal("Margin", "0,2,3,2"),
                xaml("Background", GOLDEN_GLASS),
                xaml("BorderBrush", GOLDEN_BORDER_BRUSH),
                literal("BorderThickness", "0.5,1,0.5,1"),
                literal("CornerRadius", "5,25,25,5"),
                literal("Padding", "0,0,10,0"),
            ],
            vec![],
        );
    }

    #[test]
    fn golden_omnibuttons() {
        let engine = engine();
        let lookup = golden_taskbar_lookup();

        // The plain `SystemTray.OmniButton` rule matches both named buttons.
        for handle in [TREE_CONTROL_CENTER_BUTTON, TREE_NOTIFICATION_CENTER_BUTTON] {
            assert_matched(
                &engine,
                &lookup,
                handle,
                vec![literal("Padding", "2,4,2,4"), literal("CornerRadius", "10")],
                vec![],
            );
        }
    }

    #[test]
    fn golden_text_icon_content_chain() {
        let engine = engine();
        let lookup = golden_taskbar_lookup();

        // The full 10-part chain matches.
        assert_matched(
            &engine,
            &lookup,
            TREE_TEXT_ICON_CONTENT,
            vec![literal("Visibility", "Collapsed")],
            vec![],
        );

        // The single Grid that the 9-part `... > SystemTrayIcon > Grid` rule
        // matches gets its own Padding rule; the Grid above it does not.
        assert_matched(
            &engine,
            &lookup,
            TREE_ICON_GRID,
            vec![literal("Padding", "2,4,2,4")],
            vec![],
        );
        assert_not_matched(&engine, &lookup, TREE_ICON_GRID2);

        // One missing link breaks the chain.
        let mut broken = golden_taskbar_lookup();
        broken.nodes.get_mut(&TREE_CHAIN_CP2).unwrap().class_name =
            "Windows.UI.Xaml.Controls.Border".to_string();
        assert_not_matched(&engine, &broken, TREE_TEXT_ICON_CONTENT);
    }

    #[test]
    fn golden_text_property_filter() {
        let engine = engine();
        let lookup = golden_taskbar_lookup();
        assert_matched(
            &engine,
            &lookup,
            TREE_TEXT_BLOCK,
            vec![literal("Text", "\u{ed14}")],
            vec![],
        );

        // A different local value doesn't match.
        let mut other_text = golden_taskbar_lookup();
        other_text.nodes.get_mut(&TREE_TEXT_BLOCK).unwrap().values =
            vec![("Text".to_string(), PropValue::Text("x".to_string()))];
        assert_not_matched(&engine, &other_text, TREE_TEXT_BLOCK);

        // An unset local value doesn't match either.
        let mut unset = golden_taskbar_lookup();
        unset
            .nodes
            .get_mut(&TREE_TEXT_BLOCK)
            .unwrap()
            .values
            .clear();
        assert_not_matched(&engine, &unset, TREE_TEXT_BLOCK);
    }

    #[test]
    fn golden_plain_run_context_elements_have_no_rules() {
        let engine = engine();
        let lookup = golden_taskbar_lookup();
        for handle in [
            TREE_ROOT_SV,
            TREE_SCP,
            TREE_BORDER,
            TREE_BACKGROUND_GRID,
            TREE_CONTROL_CENTER_GRID,
            TREE_NOTIFICATION_CENTER_GRID,
            TREE_CHAIN_CP1,
            TREE_CHAIN_ITEMS,
            TREE_CHAIN_STACK,
            TREE_CHAIN_CP2,
            TREE_ICON_VIEW,
            TREE_ICON_GRID2,
        ] {
            assert_not_matched(&engine, &lookup, handle);
        }
    }

    /// The full-width shell background is collapsed on purpose: its fill is the
    /// grey line that shows above/behind the floating dock.
    #[test]
    fn golden_taskbar_background_is_collapsed() {
        let engine = engine();
        let lookup = golden_taskbar_lookup();
        assert_matched(
            &engine,
            &lookup,
            TREE_BACKGROUND_CONTROL,
            vec![literal("Visibility", "Collapsed")],
            vec![],
        );
    }

    /// The `:root > ScrollViewer > ...` part of the theme only matches when the
    /// engine reports the root element's *runtime* class name. The live TAP
    /// reports `Windows.UI.Xaml.Internal.RootScrollViewer` as its Type; the
    /// reference's parent matchers are tested with a null fallback class name
    /// (reference 16064-16065, 16078-16079), so the TAP type is not enough.
    #[test]
    fn golden_root_rule_requires_public_runtime_class_name() {
        let engine = engine();
        let mut lookup = golden_taskbar_lookup();

        assert_eq!(
            lookup.class_name(TREE_ROOT_SV).as_deref(),
            Some(CLS_ROOT_SCROLL_VIEWER)
        );
        assert_eq!(
            lookup.nodes[&TREE_ROOT_SV].tap_type,
            "Windows.UI.Xaml.Internal.RootScrollViewer"
        );

        // Reporting the TAP type as the class name loses the layout rule.
        lookup.nodes.get_mut(&TREE_ROOT_SV).unwrap().class_name =
            "Windows.UI.Xaml.Internal.RootScrollViewer".to_string();
        assert_not_matched(&engine, &lookup, TREE_COLUMN_GRID);
    }

    /// The tree sketch in the task brief adds an extra `ScrollViewer` element
    /// below a root `RootScrollViewer`. The reference cannot match that: `:root`
    /// asserts that the *next* part (`ScrollViewer`) has no visual parent
    /// (reference 16042-16048). The live tree has only one ScrollViewer-derived
    /// element at the top, and `ScrollContentPresenter`'s parent is it
    /// (`docs/evidence/taskbar-tree.log`: `#3 ... parent 124544488`,
    /// `#2 ... Windows.UI.Xaml.Internal.RootScrollViewer`), so the sketch's
    /// intermediate node does not exist.
    #[test]
    fn golden_sketch_with_extra_scrollviewer_node_does_not_match() {
        let engine = engine();
        let mut lookup = MockLookup::new();
        lookup.add(
            1,
            "Windows.UI.Xaml.Hosting.DesktopWindowXamlSource",
            "",
            None,
        );
        lookup.add(2, "Windows.UI.Xaml.Internal.RootScrollViewer", "", Some(1));
        lookup.add(3, CLS_ROOT_SCROLL_VIEWER, "", Some(2));
        lookup.add(4, CLS_SCROLL_CONTENT_PRESENTER, "", Some(3));
        lookup.add(5, CLS_BORDER, "", Some(4));
        lookup.add(6, CLS_GRID, "", Some(5));
        assert_not_matched(&engine, &lookup, 6);
    }
}
