//! Port of the Windows 11 Taskbar Styler theme engine.
//!
//! Ground truth: `D:\SoftWare\Windhawk\AppData\ModsSource\windows-11-taskbar-styler.wh.cpp`.
//! For the current target theme (`g_themeWindowGlass_variant_Split`, lines
//! 3595-3788) the functions to port are:
//!
//! * `ApplyStyleConstants` (18884) / `ParseStyleConstant` (18917) / `LoadStyleConstants` (18939)
//! * `ElementMatcherFromString` (18976) / `ParseRule` (19075) / `AdjustTypeName` (19143)
//! * `SplitTargetString` (19173) / `AddElementCustomizationRulesForSingleTarget` (19202)
//! * `TestElementMatcher` (15935) / `FindElementPropertyOverrides` (16010)
//! * `StyleVariableExpressionEvaluator` (16319) / `ExpandStyleVariables` (16804)
//!
//! Everything here is pure logic: no Win32, no COM, no XAML. The engine feeds
//! it element facts through [`ElementLookup`] and gets back rules to apply.

pub mod expr;
pub mod rules;
pub mod theme;

// Flat re-exports of the contract names, so the engine may use either
// `dcx_theme::RuleEngine` or `dcx_theme::rules::RuleEngine`.
pub use expr::VarLookup;
pub use rules::{Capture, ElementLookup, MatchedRules, Override, PropValue, RuleEngine, ValueSpec};
pub use theme::{
    build_runtime_theme, OwnedTargetStyles, RuntimeTheme, TargetStyles, Theme, ThemeParams,
    WINDOW_GLASS_SPLIT,
};
