//! Toolbar geometry — the single source of truth for both painting and hit
//! testing.
//!
//! The toolbar is one row docked to the selection's bottom edge (flipping
//! above when there is no room), with the attributes the active tool actually
//! uses inlined in the same panel. That keeps every control within a short
//! pointer trip from the selection and removes the detached attribute pill the
//! first version had.
//!
//! **The painter never decides where a control is.** [`Layout::build`] produces
//! the rectangles and the action list, the painter iterates them, the mouse
//! handler asks the same list. Drawing and clicking cannot drift apart.

use crate::model::Tool;
use crate::surface::{Rect, Rgba};

// -- palette ---------------------------------------------------------------

/// The snapshot page's own hue. Distinct from every existing DCX page
/// (blue / amber / violet / terracotta / green).
pub const ACCENT: Rgba = Rgba::opaque(0x1E, 0x9C, 0x8B);
pub const PANEL: Rgba = Rgba::new(0x16, 0x1B, 0x21, 0xF2);
pub const PANEL_DEEP: Rgba = Rgba::new(0x0E, 0x12, 0x16, 0xF2);
pub const BORDER: Rgba = Rgba::new(0xFF, 0xFF, 0xFF, 0x1C);
pub const HOVER: Rgba = Rgba::new(0xFF, 0xFF, 0xFF, 0x14);
pub const PRESSED: Rgba = Rgba::new(0xFF, 0xFF, 0xFF, 0x28);
pub const ACTIVE: Rgba = Rgba::new(0x1E, 0x9C, 0x8B, 0x38);
pub const TEXT: Rgba = Rgba::opaque(0xED, 0xF2, 0xF5);
pub const MUTED: Rgba = Rgba::new(0x9A, 0xA7, 0xB1, 0xFF);
pub const DISABLED: Rgba = Rgba::new(0x6A, 0x74, 0x7C, 0xFF);
pub const DANGER: Rgba = Rgba::opaque(0xE5, 0x48, 0x4D);
/// Ink used on top of the accent fill: white fails contrast on teal at 13 pt.
pub const INK: Rgba = Rgba::opaque(0x08, 0x13, 0x10);
pub const MASK: [u8; 3] = [0x0A, 0x0E, 0x12];

/// One annotation colour. The name doubles as the tooltip; keeping the two in
/// one row means adding a colour cannot leave the hint stale.
pub struct Swatch {
    pub rgb: [u8; 3],
    pub name: &'static str,
}

/// Annotation colours. Ten, not five: the attribute row has its own line now,
/// and picking red-orange-teal by eye is the whole point of a palette.
pub const SWATCHES: [Swatch; 10] = [
    Swatch { rgb: [0xE5, 0x48, 0x4D], name: "红色" },
    Swatch { rgb: [0xF7, 0x6B, 0x15], name: "橙色" },
    Swatch { rgb: [0xF2, 0xB0, 0x2C], name: "琥珀" },
    Swatch { rgb: [0x3E, 0xC1, 0x76], name: "绿色" },
    Swatch { rgb: [0x12, 0xA5, 0x94], name: "青色" },
    Swatch { rgb: [0x2B, 0x9C, 0xE9], name: "蓝色" },
    Swatch { rgb: [0x8E, 0x4E, 0xC6], name: "紫色" },
    Swatch { rgb: [0xF3, 0xF5, 0xF7], name: "白色" },
    Swatch { rgb: [0x9A, 0xA7, 0xB1], name: "灰色" },
    Swatch { rgb: [0x16, 0x18, 0x1D], name: "黑色" },
];

pub const WIDTHS: [f32; 3] = [1.5, 3.0, 6.0];

// -- actions ---------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    Tool(Tool),
    Undo,
    Redo,
    Copy,
    Save,
    Close,
    Color(usize),
    Width(usize),
    FontUp,
    FontDown,
    /// Font family; opens the family popover.
    FontFace,
    /// One entry of that popover.
    Font(usize),
    Bold,
    Italic,
    Shadow,
    ToggleFill,
    ToggleCurve,
    /// Select the whole monitor the selection lives on.
    FullScreen,
    /// Enter scrolling-capture stitching mode.
    LongShot,
    /// Cycle the selection/output corner radius.
    Radius,
    /// Open the free colour picker.
    CustomColor,
    /// Not clickable: a hairline between two groups.
    Divider,
}

/// Hover hint: a label and the key that does the same thing. The toolbar is
/// icon-only for the tools, so the hints carry the names and shortcuts.
pub fn tip(a: Act) -> (&'static str, &'static str) {
    match a {
        Act::Tool(Tool::Hand) => ("移动 / 选择", "1"),
        Act::Tool(Tool::Rect) => ("矩形", "2"),
        Act::Tool(Tool::Arrow) => ("箭头", "3"),
        Act::Tool(Tool::Text) => ("文本", "4"),
        Act::Tool(Tool::Marker) => ("荧光笔", "5"),
        Act::Tool(Tool::Blur) => ("模糊", "6"),
        Act::Tool(Tool::Spot) => ("聚光", "7"),
        Act::Tool(Tool::Serial) => ("序号标记", "8"),
        Act::FullScreen => ("当前屏幕选区", "Ctrl+A"),
        Act::LongShot => ("长截图（滚动自动拼接）", "Ctrl+L"),
        Act::Radius => ("截图圆角", ""),
        Act::CustomColor => ("自定义颜色", ""),
        Act::Undo => ("撤销", "Ctrl+Z"),
        Act::Redo => ("重做", "Ctrl+Shift+Z"),
        Act::Copy => ("复制并关闭", "Enter"),
        Act::Save => ("保存到默认目录", "Ctrl+S"),
        Act::Close => ("取消", "Esc"),
        Act::Color(i) => (SWATCHES.get(i).map(|s| s.name).unwrap_or("颜色"), ""),
        Act::Width(0) => ("细线", "滚轮"),
        Act::Width(1) => ("中线", "滚轮"),
        Act::Width(2) => ("粗线", "滚轮"),
        Act::Width(_) => ("线宽", "滚轮"),
        Act::FontUp => ("字号 +", ""),
        Act::FontDown => ("字号 −", ""),
        Act::FontFace => ("字体", ""),
        Act::Font(i) => (
            crate::fonts::all()
                .get(i)
                .map(|f| f.label.as_str())
                .unwrap_or("字体"),
            "",
        ),
        Act::Bold => ("粗体", "Ctrl+B"),
        Act::Italic => ("斜体", "Ctrl+I"),
        Act::Shadow => ("文字阴影", ""),
        Act::ToggleFill => ("填充", ""),
        Act::ToggleCurve => ("直线 / 曲线", ""),
        Act::Divider => ("", ""),
    }
}

// -- metrics (logical points) ---------------------------------------------

/// Tool row: tools, capture actions, undo/redo, output.
const ROW1_H: f32 = 46.0;
const ROW1_BTN: f32 = 36.0;
/// Attribute row: colour, width/type, font and size for the active tool.
/// Shorter than the tool row — it holds swatches and toggles, not the primary
/// actions, and 40 pt keeps the two-row bar from covering the document.
const ROW2_H: f32 = 40.0;
const ROW2_BTN: f32 = 30.0;
const BTN: f32 = 36.0;
const PAD: f32 = 5.0;
const GAP: f32 = 2.0;
const GROUP: f32 = 11.0;
const SWATCH: f32 = 22.0;
/// Colour-wheel cell: a little wider than a swatch so the ring reads.
const CUSTOM_W: f32 = 26.0;
const WCELL: f32 = 30.0;
/// Font-family box: wide enough for a 4-glyph CJK name plus the caret; longer
/// names are ellipsised by `paint::bar`.
const FACE_W: f32 = 88.0;
/// Bold / italic / shadow toggles.
const STYLE_W: f32 = 26.0;
const ACTION: f32 = 64.0;
const EDGE: f32 = 8.0;
const DOCK: f32 = 10.0;

pub struct Layout {
    pub scale: f32,
    pub bar: Rect,
    /// Every cell including dividers. [`Layout::hit`] skips dividers.
    pub hits: Vec<(Rect, Act)>,
    /// Where a tooltip goes: below the bar when the bar sits under the
    /// selection (i.e. away from the pixels being annotated), above otherwise.
    pub tip_below: bool,
    /// 1 = tools only, 2 = tools + attribute row.
    pub rows: u8,
    /// Y of the hairline between the two rows, when there are two.
    pub separator: Option<f32>,
}

impl Layout {
    pub fn hit(&self, x: f32, y: f32) -> Option<Act> {
        self.hits
            .iter()
            .find(|(r, a)| *a != Act::Divider && r.contains(x, y))
            .map(|(_, a)| *a)
    }

    pub fn cell(&self, act: Act) -> Option<Rect> {
        self.hits.iter().find(|(_, a)| *a == act).map(|(r, _)| *r)
    }

    /// `attr` is the tool whose attributes should be shown: the active drawing
    /// tool, or the kind of the selected object under the Hand tool. `None`
    /// keeps the bar down to one row of tools + actions.
    ///
    /// Two rows, not one: the attribute set for text grew to font, weight,
    /// slant and shadow, and a single row pushed the whole bar past the right
    /// edge on narrow screens. Attributes live on their own line so the tool
    /// row keeps its width (and its muscle memory) whatever the tool is.
    pub fn build(sel: Option<Rect>, monitor: Rect, scale: f32, attr: Option<Tool>) -> Layout {
        let s = scale.max(0.6);
        let mut layout = Layout {
            scale: s,
            bar: Rect::default(),
            hits: Vec::new(),
            tip_below: true,
            rows: 1,
            separator: None,
        };
        let Some(sel) = sel else { return layout };

        // -- tool row -------------------------------------------------------
        let mut tools: Vec<(Act, f32)> = Vec::new();
        for t in Tool::ALL {
            tools.push((Act::Tool(t), BTN));
        }
        // Capture actions: whole screen, scrolling stitch, rounded corners.
        tools.push((Act::FullScreen, BTN));
        tools.push((Act::LongShot, BTN));
        tools.push((Act::Radius, BTN));
        tools.push((Act::Divider, GROUP));
        tools.push((Act::Undo, BTN));
        tools.push((Act::Redo, BTN));
        tools.push((Act::Divider, GROUP));
        tools.push((Act::Copy, ACTION));
        tools.push((Act::Save, ACTION));
        tools.push((Act::Close, BTN));

        // -- attribute row --------------------------------------------------
        let group = |cells: &mut Vec<(Act, f32)>| cells.push((Act::Divider, GROUP));
        let mut attrs: Vec<(Act, f32)> = Vec::new();
        if let Some(attr) = attr {
            for i in 0..SWATCHES.len() {
                attrs.push((Act::Color(i), SWATCH));
            }
            attrs.push((Act::CustomColor, CUSTOM_W));
            // Text and serial badges have no useful stroke width; hiding the
            // width group keeps the row honest.
            if !matches!(attr, Tool::Text | Tool::Serial) {
                group(&mut attrs);
                for i in 0..WIDTHS.len() {
                    attrs.push((Act::Width(i), WCELL));
                }
            }
            match attr {
                Tool::Text => {
                    group(&mut attrs);
                    attrs.push((Act::FontDown, WCELL));
                    attrs.push((Act::FontUp, WCELL));
                    // Font family, then the three style toggles: the row reads
                    // 字号 → 字体 → 粗体/斜体/阴影.
                    group(&mut attrs);
                    attrs.push((Act::FontFace, FACE_W));
                    attrs.push((Act::Bold, STYLE_W));
                    attrs.push((Act::Italic, STYLE_W));
                    attrs.push((Act::Shadow, STYLE_W));
                }
                Tool::Serial => {
                    group(&mut attrs);
                    attrs.push((Act::FontDown, WCELL));
                    attrs.push((Act::FontUp, WCELL));
                }
                Tool::Rect => {
                    group(&mut attrs);
                    attrs.push((Act::ToggleFill, WCELL));
                }
                Tool::Arrow => {
                    group(&mut attrs);
                    attrs.push((Act::ToggleCurve, WCELL));
                }
                _ => {}
            }
        }

        let width_of = |cells: &[(Act, f32)]| {
            let gaps = cells.len().saturating_sub(1) as f32 * GAP;
            cells.iter().map(|(_, w)| *w).sum::<f32>() + gaps
        };
        let inner = width_of(&tools).max(width_of(&attrs));
        let bw = (inner + 2.0 * PAD) * s;
        let bh = (ROW1_H + if attrs.is_empty() { 0.0 } else { ROW2_H }) * s;

        // -- placement ------------------------------------------------------
        let min_x = monitor.x0 + EDGE * s;
        let max_x = (monitor.x1 - EDGE * s - bw).max(min_x);
        let min_y = monitor.y0 + EDGE * s;
        let max_y = (monitor.y1 - EDGE * s - bh).max(min_y);
        let gap = DOCK * s;
        // Left-anchored to the selection: the tool buttons stay under the same
        // pixel when the attributes appear/disappear, so a second click cannot
        // land on a control that just slid sideways.
        let bx = sel.x0.clamp(min_x, max_x);
        let below = sel.y1 + gap;
        let above = sel.y0 - gap - bh;
        let (by, tip_below) = if below <= max_y {
            (below, true)
        } else if above >= min_y {
            (above, false)
        } else {
            // Full-height selection: tuck it inside the bottom edge.
            ((sel.y1 - bh - gap).clamp(min_y, max_y), false)
        };
        let bar = Rect::from_xywh(bx, by, bw, bh);
        layout.bar = bar;
        layout.tip_below = tip_below;

        // Tool row first, attributes under it: the tool row's anchor (bar.y0)
        // never moves when the attribute row appears, which is what makes the
        // first click after arming a tool land on the same pixel as the second.
        let mut x = bar.x0 + PAD * s;
        let y1 = bar.y0 + (ROW1_H - ROW1_BTN) * 0.5 * s;
        for (act, w) in tools {
            let cw = w * s;
            layout
                .hits
                .push((Rect::from_xywh(x, y1, cw, ROW1_BTN * s), act));
            x += cw + GAP * s;
        }
        if !attrs.is_empty() {
            layout.rows = 2;
            layout.separator = Some(bar.y0 + ROW1_H * s);
            // Centre the attribute row in the panel: it is almost always
            // narrower than the tool row, and a left-aligned stub under a long
            // toolbar reads like a rendering mistake.
            let mut x = bar.x0 + (bar.width() - width_of(&attrs) * s) * 0.5;
            let y2 = bar.y0 + (ROW1_H + (ROW2_H - ROW2_BTN) * 0.5) * s;
            for (act, w) in attrs {
                let cw = w * s;
                layout
                    .hits
                    .push((Rect::from_xywh(x, y2, cw, ROW2_BTN * s), act));
                x += cw + GAP * s;
            }
        }
        layout
    }
}
