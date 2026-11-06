//! Theme data: `g_themeWindowGlass_variant_Split`, transcribed verbatim from
//! `D:\SoftWare\Windhawk\AppData\ModsSource\windows-11-taskbar-styler.wh.cpp`
//! lines 3595-3788. Do not edit by hand; the strings are character-for-character copies
//! (only the C++ `L"..."` wrapping and `\uXXXX` escapes are Rust syntax now).

/// One `ThemeTargetStyles` entry, transcribed verbatim.
pub struct TargetStyles {
    /// XAML selector, e.g. `"Taskbar.TaskbarFrame > Grid#RootGrid"`.
    pub target: &'static str,
    /// Property rules, e.g. `&["Margin=10,2,3,2", "Background:=$Glass"]`.
    pub styles: &'static [&'static str],
}

/// A theme: target styles plus the style constants (`$Name`) they may reference.
pub struct Theme {
    pub target_styles: &'static [TargetStyles],
    pub style_constants: &'static [&'static str],
}

/// `g_themeWindowGlass_variant_Split` (reference lines 3595-3788).
pub static WINDOW_GLASS_SPLIT: Theme = Theme {
    target_styles: &[
        TargetStyles {
            target: "Taskbar.TaskbarFrame > Grid#RootGrid@DockingStates",
            styles: &[
                "Tag=horizontal",
                "Tag@DockedLeft=vertical",
                "Tag@DockedRight=vertical",
                "Tag=>taskbarDock",
            ],
        },
        TargetStyles {
            target: "Taskbar.TaskbarFrame",
            styles: &[
                "Grid.Column=1",
                "MaxWidth={{containerGridWidth>0?min($TaskbarFrameMaxWidth,containerGridWidth):$TaskbarFrameMaxWidth}}",
                "Width={{taskbarDock==`vertical`?skip():`Auto`}}",
                "MinWidth:=100",
            ],
        },
        TargetStyles {
            target: "Taskbar.TaskbarFrame > Grid#RootGrid",
            styles: &[
                "Margin=10,2,3,2",
                // The reference's hairline renders as a gray line around the
                // floating dock. The glass surface alone is the intended look.
                "BorderThickness=0",
                "CornerRadius=25,5,5,25",
                "Background:=$Background",
                "Padding=10,0,0,0",
            ],
        },
        TargetStyles {
            // The default full-width taskbar background is also the source of
            // the gray line above the floating dock. Collapse the control
            // itself; its fill/stroke children are handled below for versions
            // whose visual tree doesn't expose the parent selector.
            target: "Taskbar.TaskbarBackground#BackgroundControl, Taskbar.TaskbarBackground",
            styles: &[
                "Visibility=Collapsed",
            ],
        },
        TargetStyles {
            target: "Taskbar.TaskbarBackground#BackgroundControl > Windows.UI.Xaml.Controls.Grid > Windows.UI.Xaml.Shapes.Rectangle#BackgroundFill",
            styles: &[
                "Visibility=Collapsed",
            ],
        },
        TargetStyles {
            target: "Taskbar.TaskbarBackground#BackgroundControl > Windows.UI.Xaml.Controls.Grid > Windows.UI.Xaml.Shapes.Rectangle#BackgroundStroke",
            styles: &[
                "Visibility=Collapsed",
            ],
        },
        TargetStyles {
            target: "Taskbar.AugmentedEntryPointButton#AugmentedEntryPointButton > Taskbar.TaskListButtonPanel#ExperienceToggleButtonRootPanel",
            styles: &[
                "Margin=0,8,0,8",
                "Background:=Transparent",
                "CornerRadius=0",
                "BorderThickness=0,0,1,0",
                "BorderBrush:=#20808080",
                "Padding=2,0,5,0",
                "MaxWidth:=200",
            ],
        },
        TargetStyles {
            target: "StackPanel#SystemTrayFrameGrid, Grid#SystemTrayFrameGrid",
            styles: &[
                "Margin=0,2,3,2",
                "Background:=$Background",
                "BorderBrush:=$BorderBrush",
                "BorderThickness=$BorderThickness",
                "CornerRadius=5,25,25,5",
                "Padding=0,0,10,0",
            ],
        },
        TargetStyles {
            target: "SystemTray.ChevronIconView",
            styles: &[
                "Padding=$TrayPadding",
                "CornerRadius=10",
                "Margin=5,0,0,0",
            ],
        },
        TargetStyles {
            target: "SystemTray.NotifyIconView#NotifyItemIcon",
            styles: &[
                "Padding=$TrayPadding",
                "CornerRadius=10",
                "Margin=5,0,0,0",
            ],
        },
        TargetStyles {
            target: "SystemTray.OmniButton",
            styles: &[
                "Padding=$TrayPadding",
                "CornerRadius=10",
            ],
        },
        TargetStyles {
            target: "SystemTray.CopilotIcon",
            styles: &[
                "Padding=$TrayPadding",
            ],
        },
        TargetStyles {
            target: "SystemTray.OmniButton#NotificationCenterButton > Grid > ContentPresenter > ItemsPresenter > StackPanel > ContentPresenter > SystemTray.IconView#SystemTrayIcon > Grid",
            styles: &[
                "Padding=$TrayPadding",
            ],
        },
        TargetStyles {
            target: "SystemTray.IconView#SystemTrayIcon > Grid#ContainerGrid > ContentPresenter#ContentPresenter > Grid#ContentGrid > SystemTray.TextIconContent > Grid#ContainerGrid",
            styles: &[
                "Padding=10",
                "CornerRadius=10",
            ],
        },
        TargetStyles {
            target: "SystemTray.StackListView#IconStack > ItemsPresenter > StackPanel > ContentPresenter > SystemTray.IconView#SystemTrayIcon",
            styles: &[
                "Padding=0",
            ],
        },
        TargetStyles {
            target: "SystemTray.Stack#ShowDesktopStack",
            styles: &[
                "Visibility=Visible",
            ],
        },
        TargetStyles {
            target: "Taskbar.Gripper#GripperControl",
            styles: &[
                "Width=Auto",
                "MinWidth=24",
            ],
        },
        TargetStyles {
            target: "Windows.UI.Xaml.Controls.Grid#AugmentedEntryPointContentGrid",
            styles: &[
                "Margin=4,0,0,0",
            ],
        },
        TargetStyles {
            target: "TextBlock#TimeInnerTextBlock",
            styles: &[
                "FontSize=13",
                "FontFamily=vivo Sans EN VF",
                "Margin=0",
                "Padding=0",
                "RenderTransform:=<TranslateTransform X=\"0\" Y=\"0\" />",
            ],
        },
        TargetStyles {
            target: "TextBlock#DateInnerTextBlock",
            styles: &[
                "Visibility=Collapsed",
                "RenderTransform:=<TranslateTransform X=\"0\" Y=\"-9\" />",
                "FontSize=11",
                "FontFamily=vivo Sans EN VF",
            ],
        },
        TargetStyles {
            target: "TextBlock#InnerTextBlock[Text=\u{E971}]",
            styles: &[
                "Text=\u{ED14}",
            ],
        },
        TargetStyles {
            target: "Windows.UI.Xaml.Controls.Grid#ConfirmatorMainGrid",
            styles: &[
                "CornerRadius=22",
                "BorderThickness=$BorderThickness",
                "BorderBrush:=$BorderBrush",
                "Background:=$Background",
            ],
        },
        TargetStyles {
            target: "TextBlock#SearchBoxTextBlock",
            styles: &[
                "Text=Search",
                "FontSize=10",
                "FontFamily=vivo Sans EN VF",
            ],
        },
        TargetStyles {
            target: "SystemTray.OmniButton#NotificationCenterButton > Grid > ContentPresenter > ItemsPresenter > StackPanel > ContentPresenter > SystemTray.IconView#SystemTrayIcon > Grid > Grid > SystemTray.TextIconContent",
            styles: &[
                "Visibility=Collapsed",
            ],
        },
        TargetStyles {
            target: "Windows.UI.Xaml.Controls.Button",
            styles: &[
                "BorderThickness=$BorderThickness",
            ],
        },
        TargetStyles {
            target: "WindowsInternal.ComposableShell.Experiences.Switcher.AltTab > Windows.UI.Xaml.Controls.Grid#ModalRootGrid > Windows.UI.Xaml.Controls.Border#BackgroundElement",
            styles: &[
                "BorderThickness=0",
                "Background=Transparent",
                "CornerRadius=$CornerRadius",
            ],
        },
        TargetStyles {
            target: "WindowsInternal.ComposableShell.Experiences.Switcher.AltTab > Windows.UI.Xaml.Controls.Grid#ModalRootGrid > Windows.UI.Xaml.Controls.Border#BackgroundElement > WindowsInternal.ComposableShell.Experiences.Switcher.SwitchItemList",
            styles: &[
                "Background:=$Background",
                "BorderThickness=$BorderThickness",
                "BorderBrush:=$BorderBrush",
                "CornerRadius=$CornerRadius",
            ],
        },
        TargetStyles {
            target: "WindowsInternal.ComposableShell.Experiences.Switcher.VirtualDesktopBarElement#VirtualDesktopBar > Grid > Border",
            styles: &[
                "BorderThickness=$BorderThickness",
                "BorderBrush:=$BorderBrush",
                "Background:=$Background",
                "CornerRadius=$CornerRadius",
            ],
        },
        TargetStyles {
            target: "WindowsInternal.ComposableShell.Experiences.Switcher.VirtualDesktopBarElement#VirtualDesktopBar",
            styles: &[
                "Width=Auto",
                "Visibility=Visible",
                "HorizontalAlignment=1",
            ],
        },
        TargetStyles {
            target: "Windows.UI.Xaml.Controls.Border#BackgroundDimmingLayer",
            styles: &[
                "Background:=Transparent",
            ],
        },
        TargetStyles {
            target: "Taskbar.TaskListButtonPanel#ExperienceToggleButtonRootPanel > Windows.UI.Xaml.Controls.Grid > Windows.UI.Xaml.Controls.Border#BackgroundElement, Taskbar.TaskListButtonPanel#ExperienceToggleButtonRootPanel > Windows.UI.Xaml.Controls.Border#BackgroundElement, SearchUx.SearchUI.SearchButtonRootGrid#SearchBoxButtonRootPanel > Windows.UI.Xaml.Controls.Grid > Windows.UI.Xaml.Controls.Border#BackgroundElement, SearchUx.SearchUI.SearchButtonRootGrid#SearchBoxButtonRootPanel > Windows.UI.Xaml.Controls.Border#BackgroundElement",
            styles: &[
                "CornerRadius=10",
            ],
        },
        TargetStyles {
            target: "Taskbar.TaskListButton#TaskListButton",
            styles: &[
                "CornerRadius=10",
            ],
        },
        TargetStyles {
            target: "Windows.UI.Xaml.Controls.Border#SnapBarBorder",
            styles: &[
                "Background:=$Background",
                "BorderBrush:=$BorderBrush",
                "CornerRadius=$CornerRadius",
                "BorderThickness=$BorderThickness",
                "RenderTransform:=<TranslateTransform X=\"0\" Y=\"10\" />",
                "Margin=0,0,0,-10",
            ],
        },
        TargetStyles {
            target: "Windows.UI.Xaml.Controls.Border#SnapPickerBorder",
            styles: &[
                "Background:=$Background",
                "BorderBrush:=$BorderBrush",
                "CornerRadius=$CornerRadius",
                "BorderThickness=$BorderThickness",
            ],
        },
        TargetStyles {
            target: "Windows.UI.Xaml.Controls.Border#SearchPillBackgroundElement",
            styles: &[
                "BorderBrush:=$ElementBorderBrush",
                "CornerRadius=$ElementCornerRadius",
                "BorderThickness=$ElementBorderThickness",
                "MaxWidth:=100",
                "Width=Auto",
            ],
        },
        TargetStyles {
            target: "Taskbar.TaskbarExtensionElement",
            styles: &[
                "RenderTransform:=<TranslateTransform X=\"0\" Y=\"0\" />",
            ],
        },
        TargetStyles {
            target: "Taskbar.TaskListButtonPanel#ExperienceToggleButtonRootPanel",
            styles: &[
                "RenderTransform:=<TranslateTransform X=\"0\" Y=\"0\" />",
            ],
        },
        TargetStyles {
            target: "Windows.UI.Xaml.Controls.ToolTip > Windows.UI.Xaml.Controls.ContentPresenter#LayoutRoot",
            styles: &[
                "Background:=$Background",
                "BorderBrush:=$BorderBrush",
                "BorderThickness:=$BorderThickness",
                "CornerRadius=12",
            ],
        },
        TargetStyles {
            target: "SearchUx.SearchUI.SearchButtonControl",
            styles: &[
                "MaxWidth:=300",
                "MinWidth:=10",
                "Width=Auto",
                "Margin=0",
            ],
        },
        TargetStyles {
            target: "WindowsInternal.ComposableShell.Experiences.Switcher.VirtualDesktopBarElement > Windows.UI.Xaml.Controls.Grid#GridElement > Windows.UI.Xaml.Controls.Border#VirtualDesktopSwitcherBackground",
            styles: &[
                "Background:=$Background",
                "BorderBrush:=$BorderBrush",
                "BorderThickness=$BorderThickness",
                "CornerRadius=$CornerRadius",
            ],
        },
        TargetStyles {
            target: "Windows.UI.Xaml.Shapes.Rectangle#BackgroundFill",
            styles: &[
                "Fill:=$Background",
            ],
        },
        TargetStyles {
            target: "SystemTray.SystemTrayFrame",
            styles: &[
                "Grid.Column=2",
            ],
        },
        TargetStyles {
            // No border: flyouts are translucent, so a 1 px stroke reads as a
            // stray rectangle floating over the desktop instead of an edge.
            target: "Windows.UI.Xaml.Controls.Border#OverflowFlyoutBackgroundBorder",
            styles: &[
                "Background:=$Background",
                "BorderThickness=0",
                "CornerRadius=$CornerRadius",
            ],
        },
        TargetStyles {
            target: "Windows.UI.Xaml.Controls.MenuFlyoutPresenter > Windows.UI.Xaml.Controls.Border",
            styles: &[
                "Background:=$Background",
                "BorderThickness=0",
                "CornerRadius=$CornerRadius",
            ],
        },
        TargetStyles {
            target: "Windows.UI.Xaml.Controls.Grid#HoverFlyoutGrid > Windows.UI.Xaml.Controls.Border#HoverFlyoutBackground",
            styles: &[
                "Background:=Transparent",
                "BorderThickness=0",
                "CornerRadius=$CornerRadius",
            ],
        },
        TargetStyles {
            target: ":root > ScrollViewer > ScrollContentPresenter > Border > Grid",
            styles: &[
                "ColumnDefinitions:=<ColumnDefinitionCollection><ColumnDefinition Width=\"*\"/><ColumnDefinition Width=\"Auto\"/><ColumnDefinition Width=\"Auto\"/><ColumnDefinition Width=\"*\"/></ColumnDefinitionCollection>",
                "ActualWidth=>containerGridWidth",
            ],
        },
    ],
    style_constants: &[
        "Translucent=<WindhawkBlur BlurAmount=\"15\" TintColor=\"#10808080\"/>",
        "Glass=<WindhawkBlur BlurAmount=\"5\" TintColor=\"{ThemeResource SystemChromeMediumColor}\" TintOpacity=\"0.7\" />",
        "Frosted=<WindhawkBlur BlurAmount=\"20\" TintColor=\"{ThemeResource SystemChromeMediumColor}\" TintOpacity=\"0.7\" />",
        "Acrylic=<WindhawkBlur BlurAmount=\"30\" TintColor=\"{ThemeResource SystemChromeMediumColor}\" TintOpacity=\"0.8\" />",
        "Background=$Glass",
        "BorderBrush=<LinearGradientBrush StartPoint=\"0,0\" EndPoint=\"0,1\"><GradientStop Color=\"#50808080\" Offset=\"0.0\" /><GradientStop Color=\"#30404040\" Offset=\"0.25\" /><GradientStop Color=\"#40808080\" Offset=\"1\" /></LinearGradientBrush>",
        "BorderBrush2=<WindhawkBlur BlurAmount=\"10\" TintColor=\"#909090\" TintOpacity=\"0.2\"/>",
        "ElementBG=<SolidColorBrush Color=\"{ThemeResource SystemChromeLowColor}\" Opacity=\"0.3\" />",
        "ElementBorderThickness=0.3,0.3,0.3,1",
        "ElementBorderBrush=<LinearGradientBrush StartPoint=\"0,0\" EndPoint=\"0,1\"><GradientStop Color=\"#50808080\" Offset=\"1\" /><GradientStop Color=\"#50606060\" Offset=\"0.15\" /></LinearGradientBrush>",
        "ElementCornerRadius=12",
        "ElementSysColor=<SolidColorBrush Color=\"{ThemeResource SystemAccentColorLight1}\" Opacity=\"1\" />",
        "ElementSysColor2=<SolidColorBrush Color=\"{ThemeResource SystemAccentColorLight2}\" Opacity=\"1\" />",
        "ElementSysColor3=<SolidColorBrush Color=\"{ThemeResource SystemAccentColorLight3}\" Opacity=\"1\" />",
        "ElementSysColor4=<SolidColorBrush Color=\"{ThemeResource SystemAccentColorDark1}\" Opacity=\"1\" />",
        "BorderThickness=0.5,1,0.5,1",
        "CornerRadius=25",
        "TrayPadding=2,4,2,4",
        "TaskbarFrameMaxWidth=1895",
    ],
};

// ---------------------------------------------------------------------------
// Runtime (settings-driven) theme.
//
// The static `WINDOW_GLASS_SPLIT` above is `&'static` and immutable. To let user
// settings change the taskbar look without rebuilding the binary, we clone it
// into an owned form and append/override a handful of selectors. `RuleEngine`
// already stores everything it parses as owned data, so `from_runtime` feeds it
// the owned clone and is otherwise identical to `new`.
// ---------------------------------------------------------------------------

/// Resolved, settings-derived parameters consumed by [`build_runtime_theme`].
///
/// Kept free of any settings-enum types (`TaskbarShape`/`IconSize`/`Spacing`)
/// so `dcx-theme` does not depend on `dcx-settings`; the engine maps
/// `Settings` -> `ThemeParams`.
#[derive(Debug, Clone, Copy)]
pub struct ThemeParams {
    /// `true` => capsule (rounded ends, no border), `false` => card.
    pub capsule: bool,
    /// Render scale applied to task list buttons.
    pub icon_scale: f32,
    /// Extra horizontal margin between taskbar buttons, in px. Negative pulls
    /// the buttons closer together.
    pub gap_between: i32,
    /// Extra vertical padding at the taskbar ends, in px.
    pub gap_edge: u32,
    /// Taskbar maximum width override; 0 means "use the base value".
    pub max_width: u32,
    /// Gap between the dock and the bottom screen edge, in DIPs. Implemented as
    /// extra bottom margin: the shell window cannot grow, so the space comes
    /// out of the dock's own height.
    pub dock_gap: u32,
    /// Visible dock height in DIPs (0 = leave the theme default). Must satisfy
    /// `height + dock_gap <= 44`, which the settings clamp enforces.
    pub height: u32,
}

/// Owned counterpart of [`TargetStyles`] so a theme can be built at runtime.
pub struct OwnedTargetStyles {
    pub target: String,
    pub styles: Vec<String>,
}

/// A fully owned theme: the input to [`RuleEngine::from_runtime`].
pub struct RuntimeTheme {
    pub target_styles: Vec<OwnedTargetStyles>,
    pub style_constants: Vec<String>,
}

impl RuntimeTheme {
    /// Clone the static `WINDOW_GLASS_SPLIT` theme into an owned form.
    pub fn from_static(base: &Theme) -> Self {
        RuntimeTheme {
            target_styles: base
                .target_styles
                .iter()
                .map(|ts| OwnedTargetStyles {
                    target: ts.target.to_string(),
                    styles: ts.styles.iter().map(|s| s.to_string()).collect(),
                })
                .collect(),
            style_constants: base.style_constants.iter().map(|c| c.to_string()).collect(),
        }
    }
}

/// Override (or append) a `$Name=Value` style constant by name.
fn set_constant(constants: &mut Vec<String>, name: &str, value: &str) {
    let prefix = format!("{name}=");
    match constants.iter_mut().find(|c| c.starts_with(&prefix)) {
        Some(slot) => *slot = format!("{name}={value}"),
        None => constants.push(format!("{name}={value}")),
    }
}

/// Build a runtime theme from resolved settings parameters.
///
/// Strategy (matches the engine's eager constant expansion): constants are set
/// *first*, then the target styles that reference them are parsed, so overrides
/// win. Runtime selectors are appended *after* the base ones, and because
/// `match_element` lets a later rule win a property, they override the base.
pub fn build_runtime_theme(p: &ThemeParams) -> RuntimeTheme {
    let mut theme = RuntimeTheme::from_static(&WINDOW_GLASS_SPLIT);

    // Settings-driven constants referenced by the runtime selectors below.
    set_constant(
        &mut theme.style_constants,
        "IconScale",
        &format!("{}", p.icon_scale),
    );
    set_constant(
        &mut theme.style_constants,
        "IconGapBetween",
        &p.gap_between.to_string(),
    );
    set_constant(
        &mut theme.style_constants,
        "IconGapEdge",
        &p.gap_edge.to_string(),
    );
    if p.max_width > 0 {
        set_constant(
            &mut theme.style_constants,
            "TaskbarFrameMaxWidth",
            &p.max_width.to_string(),
        );
    }

    // --- Shape: capsule vs card -------------------------------------------
    // RootGrid is the floating dock's rounded rectangle, so the shape is
    // expressed purely by its corner radius: capsule rounds the outer ends,
    // card uses one uniform 8 px radius.
    //
    // Card mode deliberately does NOT paint a frame. An earlier version added
    // `BorderThickness=1` + `BorderBrush` on
    // `Windows.UI.Xaml.Controls.Border#BackgroundBorder`. That element name is
    // shared by every tray icon, the clock and the overflow flyout — the
    // recorded tree (`docs/evidence/taskbar-tree.log`) shows six of them — so
    // the "card frame" was drawn around each tray button instead of the dock.
    let corner = if p.capsule { "25,5,5,25" } else { "8" };
    theme.target_styles.push(OwnedTargetStyles {
        target: "Taskbar.TaskbarFrame > Grid#RootGrid".into(),
        styles: vec![format!("CornerRadius={corner}")],
    });
    // Explicit dock height: the shell window is a fixed 48 DIP, so the XAML is
    // the only place the visible bar can grow or shrink.
    if p.height > 0 {
        theme.target_styles.push(OwnedTargetStyles {
            target: "Taskbar.TaskbarFrame > Grid#RootGrid".into(),
            styles: vec![format!("Height={}", p.height)],
        });
        theme.target_styles.push(OwnedTargetStyles {
            target: "StackPanel#SystemTrayFrameGrid, Grid#SystemTrayFrameGrid".into(),
            styles: vec![format!("Height={}", p.height)],
        });
    }
    // Lift the dock off the bottom edge. The base theme's margin is 10,2,3,2;
    // only the bottom side changes, and the engine grows the shell window by
    // the same amount so the dock itself does not shrink.
    if p.dock_gap > 0 {
        theme.target_styles.push(OwnedTargetStyles {
            target: "Taskbar.TaskbarFrame > Grid#RootGrid".into(),
            styles: vec![format!("Margin=10,2,3,{}", 2 + p.dock_gap)],
        });
        // The tray is a *sibling* segment of RootGrid inside TaskbarFrame, with
        // its own `Margin=0,2,3,2`. Without the same bottom margin it stays
        // glued to the window's bottom edge while the task list lifts, so the
        // two halves end up at different heights.
        theme.target_styles.push(OwnedTargetStyles {
            target: "StackPanel#SystemTrayFrameGrid, Grid#SystemTrayFrameGrid".into(),
            styles: vec![format!("Margin=0,2,3,{}", 2 + p.dock_gap)],
        });
    }
    // Keep the system tray area visually consistent with the chosen shape.
    theme.target_styles.push(OwnedTargetStyles {
        target: "StackPanel#SystemTrayFrameGrid, Grid#SystemTrayFrameGrid".into(),
        styles: vec![format!(
            "CornerRadius={}",
            if p.capsule { "5,25,25,5" } else { "8" }
        )],
    });

    // --- Icon size -------------------------------------------------------
    // No rule here: `dcx_engine::hover` owns `RenderTransform` on every task
    // button. It installs one `CompositeTransform` per button and mutates it
    // each frame (base scale = the `icon_scale` setting, times the hover factor),
    // so a static rule would be overwritten on the first hover frame and would
    // then fight the animation on every settings reload.

    // --- Icon spacing: horizontal gap between buttons ----------------------
    theme.target_styles.push(OwnedTargetStyles {
        target: "Taskbar.TaskListButton#TaskListButton".into(),
        styles: vec![format!("Margin=$IconGapBetween,0,$IconGapBetween,0")],
    });

    // --- Edge gap: vertical padding at the dock ends -----------------------
    theme.target_styles.push(OwnedTargetStyles {
        target: "Taskbar.TaskbarFrame > Grid#RootGrid".into(),
        styles: vec![format!("Padding=10,$IconGapEdge,3,$IconGapEdge")],
    });

    theme
}
