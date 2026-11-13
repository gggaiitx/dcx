//! Panel rendering.
//!
//! Everything is painted on **one** egui context, in window-local points. The
//! frame loop converts screen rectangles into these local rectangles; the view
//! never talks to Win32 and never positions a window. Input is plain egui
//! hit-testing, which is exactly what the old deferred-viewport design could
//! not deliver.
//!
//! The list uses a hand-rolled virtual scroll instead of `ScrollArea`: the
//! latter treats a press-drag on a row as "scroll by dragging" and swallowed
//! the file-drag gesture. Here every row is a plain `allocate_rect`, so drag,
//! right-click and multi-select all behave normally.

use crate::file_icon::IconCache;
use crate::model::{Cmd, Dialog, MenuIcon, Model, PopupItem, Tab};
use dcx_organizer::{fmt_size, fmt_time, ViewMode, CATCH_ALL};
use egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, Rounding, Sense, Stroke, TextureHandle, Vec2,
};

pub const HEADER_H: f32 = 50.0;
pub const SEARCH_H: f32 = 40.0;
pub const TABS_H: f32 = 42.0;
pub const FOOTER_H: f32 = 32.0;
pub const LIST_ROW: f32 = 34.0;
pub const CARD_W: f32 = 96.0;
pub const CARD_H: f32 = 94.0;
pub const TILE_SIZE: f32 = 40.0;
pub const BTN_W: f32 = 44.0;
pub const BTN_H: f32 = 42.0;
/// Pointer travel before a chip press turns into a drag (points).
const DRAG_TAB: f32 = 4.0;
/// Thickness of the resize band along each panel edge. Kept small: the top
/// band sits on top of the header's drag area, and 7 pt made a normal header
/// drag turn into a height change.
pub const RESIZE: f32 = 5.0;
/// Width the header keeps free on the right for the function area. The same
/// number is used by the frame loop to decide where the drag handle ends, so
/// both must come from here.
pub const HEADER_ACTIONS_W: f32 = 214.0;
pub const HEADER_ACTIONS_COMPACT: f32 = 158.0;

/// Below this width the header drops its labels and the tabs get tighter.
pub const NARROW: f32 = 560.0;

pub fn header_actions_w(panel_w: f32) -> f32 {
    if panel_w >= NARROW {
        HEADER_ACTIONS_W
    } else {
        HEADER_ACTIONS_COMPACT
    }
}

// ---------------------------------------------------------------------------
// Resize edges
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    N,
    S,
    E,
    W,
    Ne,
    Nw,
    Se,
    Sw,
}

impl Edge {
    pub fn touches_north(self) -> bool {
        matches!(self, Edge::N | Edge::Ne | Edge::Nw)
    }
    pub fn touches_south(self) -> bool {
        matches!(self, Edge::S | Edge::Se | Edge::Sw)
    }
    pub fn touches_west(self) -> bool {
        matches!(self, Edge::W | Edge::Nw | Edge::Sw)
    }
    pub fn touches_east(self) -> bool {
        matches!(self, Edge::E | Edge::Ne | Edge::Se)
    }
    fn cursor(self) -> egui::CursorIcon {
        match self {
            Edge::N | Edge::S => egui::CursorIcon::ResizeVertical,
            Edge::E | Edge::W => egui::CursorIcon::ResizeHorizontal,
            Edge::Ne | Edge::Sw => egui::CursorIcon::ResizeNeSw,
            Edge::Nw | Edge::Se => egui::CursorIcon::ResizeNwSe,
        }
    }
}

/// The eight hot zones of a panel, in the panel's own coordinate space.
pub fn edge_zones(panel: Rect) -> [(Edge, Rect); 8] {
    let t = RESIZE;
    let (l, tp, r, b) = (panel.left(), panel.top(), panel.right(), panel.bottom());
    [
        (
            Edge::N,
            Rect::from_min_max(Pos2::new(l + t, tp), Pos2::new(r - t, tp + t)),
        ),
        (
            Edge::S,
            Rect::from_min_max(Pos2::new(l + t, b - t), Pos2::new(r - t, b)),
        ),
        (
            Edge::W,
            Rect::from_min_max(Pos2::new(l, tp + t), Pos2::new(l + t, b - t)),
        ),
        (
            Edge::E,
            Rect::from_min_max(Pos2::new(r - t, tp + t), Pos2::new(r, b - t)),
        ),
        (
            Edge::Nw,
            Rect::from_min_max(Pos2::new(l, tp), Pos2::new(l + t, tp + t)),
        ),
        (
            Edge::Ne,
            Rect::from_min_max(Pos2::new(r - t, tp), Pos2::new(r, tp + t)),
        ),
        (
            Edge::Sw,
            Rect::from_min_max(Pos2::new(l, b - t), Pos2::new(l + t, b)),
        ),
        (
            Edge::Se,
            Rect::from_min_max(Pos2::new(r - t, b - t), Pos2::new(r, b)),
        ),
    ]
}

pub fn edge_at(panel: Rect, p: Pos2) -> Option<Edge> {
    edge_zones(panel)
        .iter()
        .find(|(_, r)| r.contains(p))
        .map(|(e, _)| *e)
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct Skin {
    pub dark: bool,
    pub bg: Color32,
    pub surface: Color32,
    pub edge: Color32,
    pub hairline: Color32,
    pub text: Color32,
    pub secondary: Color32,
    pub tertiary: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub divider: Color32,
    pub tile_fill: Color32,
    pub tile_hover: Color32,
    pub popup: Color32,
    pub danger: Color32,
    pub ok: Color32,
    pub shadow: Color32,
}

impl Skin {
    pub fn system() -> Self {
        if dcx_win32::apps_use_light_theme() {
            Skin::light()
        } else {
            Skin::dark()
        }
    }

    fn light() -> Self {
        Skin {
            dark: false,
            bg: Color32::from_rgb(0xFB, 0xFB, 0xFC),
            surface: Color32::from_rgb(0xF3, 0xF4, 0xF6),
            edge: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            hairline: Color32::from_rgba_unmultiplied(0x00, 0x00, 0x00, 0x18),
            text: Color32::from_rgb(0x1B, 0x1C, 0x1E),
            secondary: Color32::from_rgb(0x5B, 0x5F, 0x66),
            tertiary: Color32::from_rgb(0x8B, 0x90, 0x98),
            accent: Color32::from_rgb(0x0E, 0x7C, 0x7E),
            accent_soft: Color32::from_rgba_unmultiplied(0x0E, 0x7C, 0x7E, 0x1C),
            hover: Color32::from_rgba_unmultiplied(0x00, 0x00, 0x00, 0x0A),
            pressed: Color32::from_rgba_unmultiplied(0x00, 0x00, 0x00, 0x16),
            divider: Color32::from_rgba_unmultiplied(0x00, 0x00, 0x00, 0x10),
            tile_fill: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x60),
            tile_hover: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x80),
            popup: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            danger: Color32::from_rgb(0xC4, 0x2B, 0x1C),
            ok: Color32::from_rgb(0x1F, 0x8A, 0x55),
            shadow: Color32::from_black_alpha(18),
        }
    }

    fn dark() -> Self {
        Skin {
            dark: true,
            bg: Color32::from_rgb(0x20, 0x21, 0x24),
            surface: Color32::from_rgb(0x2B, 0x2D, 0x31),
            edge: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x22),
            hairline: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x18),
            text: Color32::from_rgb(0xF3, 0xF4, 0xF6),
            secondary: Color32::from_rgb(0xC2, 0xC6, 0xCC),
            tertiary: Color32::from_rgb(0x8E, 0x94, 0x9C),
            accent: Color32::from_rgb(0x4F, 0xD8, 0xCC),
            accent_soft: Color32::from_rgba_unmultiplied(0x4F, 0xD8, 0xCC, 0x28),
            hover: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x10),
            pressed: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x1C),
            divider: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x14),
            tile_fill: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x18),
            tile_hover: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x18),
            popup: Color32::from_rgb(0x2C, 0x2E, 0x32),
            danger: Color32::from_rgb(0xFF, 0x99, 0xA4),
            ok: Color32::from_rgb(0x6C, 0xCF, 0x91),
            shadow: Color32::from_black_alpha(48),
        }
    }
}

fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Stable accent per category key, including user-created ones.
pub fn cat_color(key: &str, s: &Skin) -> Color32 {
    match key {
        "doc" => Color32::from_rgb(0x2F, 0x6F, 0xD0),
        "image" => Color32::from_rgb(0x1F, 0x9D, 0x67),
        "archive" => Color32::from_rgb(0xC0, 0x7A, 0x12),
        "media" => Color32::from_rgb(0x8B, 0x5C, 0xD6),
        "app" => s.accent,
        "folder" => Color32::from_rgb(0xD9, 0xA5, 0x1F),
        _ => {
            let palette = [
                (0xD1, 0x4B, 0x5A),
                (0x2C, 0x88, 0x9E),
                (0x8A, 0x6D, 0x3B),
                (0x6A, 0x63, 0xC7),
                (0x4F, 0x8A, 0x4B),
            ];
            let h: usize = key
                .bytes()
                .fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize));
            let (r, g, b) = palette[h % palette.len()];
            Color32::from_rgb(r, g, b)
        }
    }
}

fn text(p: &Painter, at: Pos2, align: Align2, s: &str, size: f32, color: Color32) {
    p.text(at, align, s, font(size), color);
}

fn elide(p: &Painter, s: &str, at: Pos2, max_w: f32, size: f32, color: Color32) {
    let mut job = egui::text::LayoutJob::single_section(
        s.to_string(),
        egui::TextFormat {
            font_id: font(size),
            color,
            ..Default::default()
        },
    );
    job.wrap.max_width = max_w.max(8.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let g = p.fonts(|f| f.layout_job(job));
    p.galley(Pos2::new(at.x, at.y - g.size().y * 0.5), g, color);
}

// ---------------------------------------------------------------------------
// Per-frame draw context
// ---------------------------------------------------------------------------

/// Inputs from the frame loop and outputs the frame loop needs back.
pub struct Draw<'a> {
    pub model: &'a mut Model,
    pub skin: &'a Skin,
    pub icons: &'a mut IconCache,
    pub ctx: &'a egui::Context,
    /// Window-local rectangles.
    pub panel: Rect,
    pub open: bool,
    pub drag_in: bool,
    pub panel_pinned: bool,
    /// Cursor in window-local points (from egui, for popup placement).
    pub cursor: Option<Pos2>,
    /// A row drag started on this frame.
    pub row_drag_start: Option<(std::path::PathBuf, String)>,
    /// Tab rectangles for drop routing (window-local).
    pub tab_rects: Vec<(Tab, Rect)>,
    /// Tab currently highlighted as a drop target.
    pub drop_tab: Option<String>,
    /// Folder rows on screen (window-local), for drop routing.
    pub folder_rects: Vec<(std::path::PathBuf, Rect)>,
    /// Folder currently highlighted as a drop target.
    pub drop_folder: Option<std::path::PathBuf>,
    pub header_rect: Rect,
    pub list_rect: Rect,
    pub popup_rect: Option<Rect>,
    /// The list scrollbar, in window-local points. The frame loop uses it to
    /// keep the east resize band from stealing a scrollbar drag.
    pub scrollbar_rect: Option<Rect>,
    /// The search field owns the keyboard this frame.
    pub search_focused: bool,
    /// A popup or modal dialog owns this frame's clicks.
    pub blocked: bool,
}

impl<'a> Draw<'a> {
    pub fn push(&mut self, cmd: Cmd) {
        self.model.push(cmd);
    }

    /// True when a popup or modal dialog should swallow clicks outside it.
    pub fn blocked(&self) -> bool {
        self.blocked
    }
}

// ---------------------------------------------------------------------------
// Taskbar tile
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------

pub fn draw_panel(d: &mut Draw<'_>, ui: &mut egui::Ui) {
    let s = *d.skin;
    let panel = d.panel;
    let p = ui.painter().clone();

    // The window is only `app::SHADOW` larger than the panel, so the shadow
    // lives in that ring — nothing transparent is left to swallow a click, and
    // the panel keeps its soft edge. Skipped entirely at zero (no ring to draw
    // into).
    if crate::app::SHADOW > 0.0 {
        for i in (1..=3).rev() {
            let k = i as f32;
            p.rect_filled(
                panel.expand(k * 0.9).translate(Vec2::new(0.0, k * 1.1)),
                Rounding::same(12.0 + k * 0.9),
                s.shadow,
            );
        }
    }
    // With the acrylic backdrop the panel tint has to stay translucent, or it
    // would paint over the blur DWM puts behind the window. The four corners
    // outside this rounded rectangle are clipped by the window region (see
    // `app::update`), which clips the backdrop with them — painting our own
    // shape alone would leave a translucent wedge at each corner.
    let acrylic = d.model.cfg.acrylic;
    let bg = if acrylic {
        let a = d.model.cfg.acrylic_alpha.clamp(120, 255) as u8;
        Color32::from_rgba_unmultiplied(s.bg.r(), s.bg.g(), s.bg.b(), a)
    } else {
        s.bg
    };
    p.rect_filled(panel, Rounding::same(12.0), bg);
    p.rect_stroke(
        panel.shrink(0.5),
        Rounding::same(11.5),
        Stroke::new(1.0_f32, s.edge),
    );
    if d.drag_in {
        p.rect_stroke(
            panel.shrink(2.0),
            Rounding::same(10.0),
            Stroke::new(2.0_f32, s.accent),
        );
    }

    let header = Rect::from_min_max(panel.min, Pos2::new(panel.right(), panel.top() + HEADER_H));
    let search = Rect::from_min_max(
        Pos2::new(panel.left(), header.bottom()),
        Pos2::new(panel.right(), header.bottom() + SEARCH_H),
    );
    let tabs = Rect::from_min_max(
        Pos2::new(panel.left(), search.bottom()),
        Pos2::new(panel.right(), search.bottom() + TABS_H),
    );
    let footer = Rect::from_min_max(
        Pos2::new(panel.left(), panel.bottom() - FOOTER_H),
        panel.max,
    );
    let list = Rect::from_min_max(
        Pos2::new(panel.left(), tabs.bottom()),
        Pos2::new(panel.right(), footer.top()),
    );
    d.header_rect = Rect::from_min_max(
        Pos2::new(header.left() + 8.0, header.top()),
        Pos2::new(
            header.right() - header_actions_w(panel.width()) - 8.0,
            header.bottom(),
        ),
    );
    d.list_rect = list;

    p.hline(
        header.left() + 12.0..=header.right() - 12.0,
        header.bottom(),
        Stroke::new(1.0_f32, s.divider),
    );
    p.hline(
        footer.left() + 12.0..=footer.right() - 12.0,
        footer.top(),
        Stroke::new(1.0_f32, s.divider),
    );

    draw_header(d, ui, header);
    draw_search(d, ui, search);
    draw_tabs(d, ui, tabs);
    draw_list(d, ui, list);
    draw_footer(d, ui, footer);
    draw_edge_hover(d, ui, panel);
    draw_popup(d, ui, panel);
    draw_dialog(d, ui, panel);
    draw_toast(d, ui, panel);
}

/// Cursor feedback for the resize bands. The bands themselves are invisible —
/// only the cursor changes, which is how a Win11 flyout behaves.
fn draw_edge_hover(d: &Draw<'_>, ui: &mut egui::Ui, panel: Rect) {
    let Some(c) = d.cursor else { return };
    if let Some(edge) = edge_at(panel, c) {
        ui.ctx().set_cursor_icon(edge.cursor());
    }
}

fn icon_button(ui: &mut egui::Ui, r: Rect, s: &Skin, kind: IconKind) -> egui::Response {
    icon_button_id(ui, r, s, kind, None)
}

/// The same button with an explicit widget id.
///
/// In this hand-painted UI `allocate_rect` derives its id from the allocation
/// order, and the number of widgets allocated before it changes with the number
/// of visible rows. A moving id makes egui treat the press and the release as
/// two different widgets, so the click is dropped — which is why category
/// chips and the list scrollbar felt "sticky".
fn icon_button_id(
    ui: &mut egui::Ui,
    r: Rect,
    s: &Skin,
    kind: IconKind,
    id: Option<egui::Id>,
) -> egui::Response {
    let resp = match id {
        Some(id) => ui.interact(r, id, Sense::click()),
        None => ui.allocate_rect(r, Sense::click()),
    };
    if resp.hovered() {
        ui.painter().rect_filled(r, Rounding::same(6.0), s.hover);
    }
    let c = r.center();
    let col = if resp.hovered() { s.text } else { s.secondary };
    let st = Stroke::new(1.4_f32, col);
    let p = ui.painter().clone();
    match kind {
        IconKind::More => {
            for dx in [-5.0, 0.0, 5.0] {
                p.circle_filled(Pos2::new(c.x + dx, c.y), 1.4, col);
            }
        }
        IconKind::Close => {
            p.line_segment(
                [
                    Pos2::new(c.x - 4.0, c.y - 4.0),
                    Pos2::new(c.x + 4.0, c.y + 4.0),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + 4.0, c.y - 4.0),
                    Pos2::new(c.x - 4.0, c.y + 4.0),
                ],
                st,
            );
        }
        IconKind::List => {
            for i in 0..3 {
                let y = c.y - 5.0 + i as f32 * 5.0;
                p.circle_filled(Pos2::new(c.x - 4.5, y), 1.2, col);
                p.line_segment([Pos2::new(c.x - 1.5, y), Pos2::new(c.x + 5.0, y)], st);
            }
        }
        IconKind::Grid => {
            for (dx, dy) in [(-4.5f32, -4.5f32), (1.5, -4.5), (-4.5, 1.5), (1.5, 1.5)] {
                p.rect_filled(
                    Rect::from_min_size(Pos2::new(c.x + dx, c.y + dy), Vec2::splat(3.2)),
                    Rounding::same(1.0),
                    col,
                );
            }
        }
        IconKind::Display => {
            // Three rails with a knob on each — the conventional "view
            // options" glyph, and distinct from List/Grid beside it.
            for (i, dy) in [-4.0f32, 0.0, 4.0].iter().enumerate() {
                let y = c.y + *dy;
                let knob = if i % 2 == 0 { 2.6 } else { -2.6 };
                p.line_segment([Pos2::new(c.x - 5.2, y), Pos2::new(c.x + 5.2, y)], st);
                p.circle_filled(Pos2::new(c.x + knob, y), 1.7, col);
            }
        }
        // `chevron(dir)` draws the point towards `dir`, so a *left* chevron
        // needs dir = +1 (the tip sits on the left).
        IconKind::ChevronLeft => chevron(&p, c, st, 1.0),
        IconKind::ChevronRight => chevron(&p, c, st, -1.0),
    }
    resp
}

fn chevron(p: &Painter, c: Pos2, st: Stroke, dir: f32) {
    p.line_segment(
        [
            Pos2::new(c.x + dir * 2.0, c.y - 4.0),
            Pos2::new(c.x - dir * 2.0, c.y),
        ],
        st,
    );
    p.line_segment(
        [
            Pos2::new(c.x - dir * 2.0, c.y),
            Pos2::new(c.x + dir * 2.0, c.y + 4.0),
        ],
        st,
    );
}

#[derive(Clone, Copy)]
enum IconKind {
    More,
    Close,
    List,
    Grid,
    /// Header 「显示」 dropdown (list display switches).
    Display,
    ChevronLeft,
    ChevronRight,
}

fn draw_header(d: &mut Draw<'_>, ui: &mut egui::Ui, r: Rect) {
    let s = *d.skin;
    let p = ui.painter().clone();
    let wide = r.width() >= NARROW;
    // Small brand mark.
    let logo = Rect::from_center_size(Pos2::new(r.left() + 26.0, r.center().y), Vec2::splat(24.0));
    p.rect_filled(logo, Rounding::same(7.0), s.accent);
    p.rect_stroke(
        logo.shrink(6.0),
        Rounding::same(2.0),
        Stroke::new(1.3_f32, Color32::WHITE),
    );
    p.hline(
        logo.left() + 6.0..=logo.right() - 6.0,
        logo.center().y,
        Stroke::new(1.3_f32, Color32::WHITE),
    );

    let text_x = logo.right() + 10.0;
    let text_w = (r.right() - header_actions_w(r.width()) - 8.0 - text_x).max(24.0);
    // One line only: the old subtitle repeated the folder name ("Desktop") plus
    // a count the footer already reports.
    elide(
        &p,
        "桌面收纳盒",
        Pos2::new(text_x, r.center().y),
        text_w,
        13.5,
        s.text,
    );

    // ---- function area, top-right -----------------------------------------
    // [排序 ▾] [列表/网格] [显示 ▾] ⋯ ✕ — everything that used to be a second
    // toolbar row lives here now, so the list gets the height back.
    let total = header_actions_w(r.width());
    let mut x = r.right() - 8.0;
    let cy = r.center().y;
    let cell =
        |x0: f32, w: f32| Rect::from_center_size(Pos2::new(x0 - w / 2.0, cy), Vec2::new(w, 28.0));
    let close_r = cell(x, 26.0);
    x -= 30.0;
    let more_r = cell(x, 26.0);
    x -= 30.0;
    let sep_x = x + 2.0;
    let disp_r = cell(x, 26.0);
    x -= 30.0;
    let view_r = cell(x, 26.0);
    x -= 30.0;
    let sort_w = (total - 8.0 - 26.0 * 4.0 - 4.0 * 4.0 - 4.0).max(26.0);
    let sort_r = cell(x, sort_w);

    let close = icon_button(ui, close_r, &s, IconKind::Close);
    let more = icon_button(ui, more_r, &s, IconKind::More);
    let disp = icon_button(ui, disp_r, &s, IconKind::Display);
    let view = icon_button(
        ui,
        view_r,
        &s,
        if d.model.cfg.view_mode == ViewMode::List {
            IconKind::List
        } else {
            IconKind::Grid
        },
    );
    // Sort is a chip with the current key when there is room for the label.
    if wide {
        chip(ui, sort_r, &s, "", false);
        text(
            ui.painter(),
            Pos2::new(sort_r.left() + 10.0, sort_r.center().y),
            Align2::LEFT_CENTER,
            "排序",
            11.0,
            s.secondary,
        );
        // Drawn, not "▾": the CJK font has no glyph for U+25BE and it showed
        // up as a tofu box in the header.
        let c = Pos2::new(sort_r.right() - 11.0, sort_r.center().y + 1.0);
        ui.painter().line_segment(
            [Pos2::new(c.x - 3.2, c.y - 1.6), Pos2::new(c.x, c.y + 1.6)],
            Stroke::new(1.2_f32, s.tertiary),
        );
        ui.painter().line_segment(
            [Pos2::new(c.x, c.y + 1.6), Pos2::new(c.x + 3.2, c.y - 1.6)],
            Stroke::new(1.2_f32, s.tertiary),
        );
    } else {
        glyph_button(ui, sort_r, &s, MenuIcon::ArrowUpDown);
    }
    ui.painter().vline(
        sep_x,
        r.top() + 15.0..=r.bottom() - 15.0,
        Stroke::new(1.0_f32, s.divider),
    );

    if close.clicked() && !d.blocked() {
        d.push(Cmd::ClosePanel);
    }
    if more.clicked() && !d.blocked() {
        let items = d.model.more_items();
        // Right edge on the button's right edge: the menu opens inwards, so it
        // can never poke out of the panel on a narrow panel.
        let w = popup_width(ui, &items, 170.0, 320.0);
        d.model.open_popup(
            Pos2::new(more_r.right() - w, more_r.bottom() + 2.0),
            Some("更多操作".into()),
            items,
            w,
        );
    }
    if disp.clicked() && !d.blocked() {
        let items = d.model.display_items();
        let w = popup_width(ui, &items, 150.0, 320.0);
        d.model.open_popup(
            Pos2::new(disp_r.right() - w, disp_r.bottom() + 2.0),
            Some("显示".into()),
            items,
            w,
        );
    }
    if view.clicked() && !d.blocked() {
        d.push(Cmd::ToggleView);
    }
    let sort_resp = ui.allocate_rect(sort_r, Sense::click());
    if sort_resp.clicked() && !d.blocked() {
        let items = d.model.sort_items();
        d.model.open_popup(
            Pos2::new(sort_r.left(), sort_r.bottom() + 3.0),
            Some("排序方式".into()),
            items,
            188.0,
        );
    }
    if sort_resp.hovered() {
        ui.painter()
            .rect_stroke(sort_r, Rounding::same(5.0), Stroke::new(1.0_f32, s.accent));
    }
    sort_resp.on_hover_text("排序方式");

    if ui.allocate_rect(d.header_rect, Sense::hover()).hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
}

/// Icon button that uses the shared menu glyph set, so the header and the
/// dropdown menus cannot drift apart.
fn glyph_button(ui: &mut egui::Ui, r: Rect, s: &Skin, icon: MenuIcon) -> egui::Response {
    let resp = ui.allocate_rect(r, Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, Rounding::same(6.0), s.hover);
    }
    let col = if resp.hovered() { s.text } else { s.secondary };
    menu_icon(
        ui.painter(),
        Rect::from_center_size(r.center(), Vec2::splat(15.0)),
        icon,
        col,
    );
    resp
}

fn draw_search(d: &mut Draw<'_>, ui: &mut egui::Ui, r: Rect) {
    let s = *d.skin;
    let box_r = Rect::from_min_max(
        Pos2::new(r.left() + 12.0, r.top() + 5.0),
        Pos2::new(r.right() - 12.0, r.bottom() - 5.0),
    );
    ui.painter()
        .rect_filled(box_r, Rounding::same(7.0), s.surface);
    ui.painter()
        .rect_stroke(box_r, Rounding::same(7.0), Stroke::new(1.0_f32, s.hairline));
    // magnifier
    let c = Pos2::new(box_r.left() + 16.0, box_r.center().y - 1.0);
    ui.painter()
        .circle_stroke(c, 4.5, Stroke::new(1.3_f32, s.tertiary));
    ui.painter().line_segment(
        [
            Pos2::new(c.x + 3.2, c.y + 3.2),
            Pos2::new(c.x + 6.5, c.y + 6.5),
        ],
        Stroke::new(1.3_f32, s.tertiary),
    );
    let edit_r = Rect::from_min_max(
        Pos2::new(box_r.left() + 30.0, box_r.top() + 2.0),
        Pos2::new(box_r.right() - 58.0, box_r.bottom() - 2.0),
    );
    let resp = ui.put(
        edit_r,
        egui::TextEdit::singleline(&mut d.model.query)
            .hint_text("搜索文件名或扩展名…")
            .font(font(12.5))
            .text_color(s.text)
            .frame(false)
            .desired_width(edit_r.width()),
    );
    if resp.changed() {
        d.model.on_query_changed();
    }
    d.search_focused = resp.has_focus();
    if d.model.search_focus {
        resp.request_focus();
        d.model.search_focus = false;
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(
            box_r,
            Rounding::same(7.0),
            Stroke::new(1.0_f32, s.accent.gamma_multiply(0.65)),
        );
    }

    // Right side of the field: an inline hint when empty, the clear button once
    // there is something to clear. A shortcut printed as a key cap reads better
    // than a paragraph of placeholder text.
    if d.model.query.is_empty() {
        if !resp.has_focus() {
            let key = Rect::from_center_size(
                Pos2::new(box_r.right() - 26.0, box_r.center().y),
                Vec2::new(38.0, 18.0),
            );
            ui.painter().rect_filled(key, Rounding::same(4.0), s.bg);
            ui.painter()
                .rect_stroke(key, Rounding::same(4.0), Stroke::new(1.0_f32, s.hairline));
            text(
                ui.painter(),
                key.center(),
                Align2::CENTER_CENTER,
                "Ctrl F",
                9.0,
                s.tertiary,
            );
        }
    } else {
        let clear = Rect::from_center_size(
            Pos2::new(box_r.right() - 16.0, box_r.center().y),
            Vec2::splat(18.0),
        );
        let cr = icon_button(ui, clear, &s, IconKind::Close);
        if cr.clicked() && !d.blocked() {
            d.model.query.clear();
            d.model.on_query_changed();
        }
    }
}

/// Stable identity for a tab. `Tab::key()` is `None` for the two virtual tabs,
/// so it cannot be used as an id on its own.
fn tab_id(tab: &Tab) -> String {
    match tab {
        Tab::Desktop => "desktop".to_string(),
        Tab::All => "all".to_string(),
        Tab::Recent => "recent".to_string(),
        Tab::Key(k) => format!("k:{k}"),
    }
}

fn draw_tabs(d: &mut Draw<'_>, ui: &mut egui::Ui, r: Rect) {
    let s = *d.skin;
    let infos = d.model.tab_infos();
    let arrow_w = 18.0;
    let pad = 10.0;

    // Measure every chip first: the strip only shows arrows when it really
    // overflows, and the active chip can then be scrolled into view.
    let measure = ui.painter().clone();
    let mut chips: Vec<(Tab, String, f32, bool)> = Vec::new();
    for (tab, label, _count) in &infos {
        let label_w = measure.fonts(|f| {
            f.layout_no_wrap(label.clone(), font(12.0), Color32::WHITE)
                .size()
                .x
        });
        // Counts are deliberately **not** shown: the tabs are a navigation
        // control, and a per-tab number that changes on every file operation
        // made the strip twitch and stole width from the labels.
        let w = 12.0 + label_w + 12.0;
        let active = *tab == d.model.active && d.model.query.trim().is_empty();
        chips.push((tab.clone(), label.clone(), w, active));
    }
    let gap = 5.0;
    let total_w: f32 = chips.iter().map(|c| c.2 + gap).sum::<f32>() + 28.0 + gap;
    let avail = r.width() - pad * 2.0;
    let overflow = total_w > avail + 0.5;

    let (strip, left_arrow, right_arrow) = if overflow {
        let l = Rect::from_center_size(
            Pos2::new(r.left() + pad + arrow_w / 2.0, r.center().y),
            Vec2::new(arrow_w, 26.0),
        );
        let rr = Rect::from_center_size(
            Pos2::new(r.right() - pad - arrow_w / 2.0, r.center().y),
            Vec2::new(arrow_w, 26.0),
        );
        (
            Rect::from_min_max(
                Pos2::new(l.right() + 3.0, r.top()),
                Pos2::new(rr.left() - 3.0, r.bottom()),
            ),
            Some(l),
            Some(rr),
        )
    } else {
        (
            Rect::from_min_max(
                Pos2::new(r.left() + pad, r.top()),
                Pos2::new(r.right() - pad, r.bottom()),
            ),
            None,
            None,
        )
    };

    // Air before the first chip, so the active pill's 1 px outline is not cut by
    // the clip rect (it used to sit flush against it and look clipped).
    const INSET: f32 = 4.0;
    let max_scroll = (INSET + total_w - strip.width()).max(0.0);

    // A chip drag is in flight (set by the previous frame's `drag_started`).
    // While it is, the scroll position must be **frozen**: the strip is being
    // re-laid-out around a moving gap, and easing the scroll underneath that
    // would shift every chip mid-gesture, making the drop slot jump around.
    let dragging = d.model.tab_drag.is_some();

    // -- scrolling ---------------------------------------------------------
    // `raw_scroll_delta`, not `smooth_scroll_delta`: egui spreads a smoothed
    // wheel event over ~150 ms of *its own* frames, and this panel is drawn by
    // a hand-written loop, so the leftover delta arrived in bursts — exactly
    // the stutter that was reported. Take the event at once and ease the
    // position ourselves below.
    if !dragging && !d.blocked() && d.cursor.map(|c| r.contains(c)).unwrap_or(false) {
        let (dx, dy) = ui.input(|i| (i.raw_scroll_delta.x, i.raw_scroll_delta.y));
        // A one-line tab bar has no vertical overflow, so the vertical wheel
        // drives it sideways; a horizontal wheel/trackpad wins when larger.
        let step = if dy.abs() >= dx.abs() { dy } else { dx };
        if step.abs() > 0.01 {
            d.model.tab_scroll_target -= step;
        }
    }

    // Auto-reveal, but **only** when the selection changed. Running it every
    // frame pinned the strip to the active chip: any attempt to scroll it out
    // of view was undone on the next frame, so the strip felt glued in place.
    let active_key = chips
        .iter()
        .find(|c| c.3)
        .map(|c| tab_id(&c.0))
        .unwrap_or_default();
    let reveal = !dragging && d.model.tab_synced != active_key;
    if reveal {
        if let Some((idx, c)) = chips.iter().enumerate().find(|(_, c)| c.3) {
            let before: f32 = INSET + chips[..idx].iter().map(|c| c.2 + gap).sum::<f32>();
            let w = c.2;
            if before < d.model.tab_scroll_target {
                d.model.tab_scroll_target = before;
            } else if before + w > d.model.tab_scroll_target + strip.width() {
                d.model.tab_scroll_target = before + w - strip.width();
            }
        }
        d.model.tab_synced = active_key;
    }

    d.model.tab_scroll_target = d.model.tab_scroll_target.clamp(0.0, max_scroll);

    if !dragging {
        if let Some(l) = left_arrow {
            if icon_button_id(
                ui,
                l,
                &s,
                IconKind::ChevronLeft,
                Some(egui::Id::new("dcx-tab-prev")),
            )
            .clicked()
            {
                d.model.tab_scroll_target =
                    (d.model.tab_scroll_target - 90.0).clamp(0.0, max_scroll);
            }
        }
        if let Some(rr) = right_arrow {
            if icon_button_id(
                ui,
                rr,
                &s,
                IconKind::ChevronRight,
                Some(egui::Id::new("dcx-tab-next")),
            )
            .clicked()
            {
                d.model.tab_scroll_target =
                    (d.model.tab_scroll_target + 90.0).clamp(0.0, max_scroll);
            }
        }
    }

    if reveal {
        // A category switch must land immediately; the easing is for the wheel.
        d.model.tab_scroll = d.model.tab_scroll_target;
    } else if !dragging {
        let diff = d.model.tab_scroll_target - d.model.tab_scroll;
        if diff.abs() > 0.4 {
            d.model.tab_scroll += diff * 0.42;
            // Keep the loop awake for the handful of frames this takes;
            // otherwise the motion advances one step per idle tick and looks
            // like a slideshow.
            ui.ctx().request_repaint();
        } else {
            d.model.tab_scroll = d.model.tab_scroll_target;
        }
    }
    let scroll = d.model.tab_scroll;

    // -- drag to reorder ---------------------------------------------------
    // Split the row into "everything except the chip being dragged". Laying the
    // others out first means the drop slot can be rendered as a live gap.
    let mut drag = d.model.tab_drag.clone();
    let drag_key = drag.as_ref().map(|t| t.key.clone());
    let cursor_x = d.cursor.map(|c| c.x).unwrap_or(-1e4);
    let fixed_head = chips.iter().take_while(|c| c.0.key().is_none()).count();

    let mut base: Vec<(Tab, String, f32, bool)> = Vec::new();
    let mut drag_w = 0.0f32;
    let mut drag_label = String::new();
    for c in chips.iter().cloned() {
        if Some(tab_id(&c.0)) == drag_key {
            drag_w = c.2;
            drag_label = c.1.clone();
            continue;
        }
        base.push(c);
    }
    if drag.is_some() && drag_w <= 0.0 {
        // The category vanished under the drag (deleted in the settings centre).
        drag = None;
        d.model.tab_drag = None;
    }
    // The catch-all is pinned last by `Config::repair`; the drop slot must not
    // offer a position after it.
    let tail = base
        .last()
        .map(|c| c.0.key() == Some(CATCH_ALL))
        .unwrap_or(false);

    let mut slot = 0usize;
    if drag.is_some() {
        // Midpoint rule over the **final** positions (no placeholder in this
        // pass). Using the placeholder's own layout would move the midpoints
        // and make the target oscillate.
        let mut cx = strip.left() + INSET - scroll;
        let mut hit = base.len();
        for (i, c) in base.iter().enumerate() {
            if cursor_x < cx + c.2 / 2.0 {
                hit = i;
                break;
            }
            cx += c.2 + gap;
        }
        let lo = fixed_head.min(base.len());
        let hi = (base.len() - if tail { 1 } else { 0 }).max(lo);
        slot = hit.clamp(lo, hi);
    }

    let painter = ui.painter().with_clip_rect(strip);
    let mut x = strip.left() + INSET - scroll;
    let mut rects: Vec<(Tab, Rect, String, bool)> = Vec::new();
    let mut slot_x: Option<f32> = None;
    let slot_w = if drag.is_some() { drag_w + gap } else { 0.0 };
    for (i, (tab, label, w, active)) in base.iter().cloned().enumerate() {
        if drag.is_some() && slot_x.is_none() && i == slot {
            slot_x = Some(x);
            x += slot_w;
        }
        let tab_r = Rect::from_min_size(Pos2::new(x, r.top() + 7.0), Vec2::new(w, 28.0));
        rects.push((tab, tab_r, label, active));
        x += w + gap;
    }
    if drag.is_some() && slot_x.is_none() {
        slot_x = Some(x);
        x += slot_w;
    }
    let add_r = Rect::from_min_size(Pos2::new(x, r.top() + 7.0), Vec2::new(28.0, 28.0));

    let hover = d.cursor.unwrap_or(Pos2::new(-1e4, -1e4));
    for (key, tab_r, label, active) in &rects {
        let drop = d
            .drop_tab
            .as_deref()
            .map(|k| key.key() == Some(k))
            .unwrap_or(false);
        let hovered = hover.x >= tab_r.left()
            && hover.x <= tab_r.right()
            && hover.y >= tab_r.top()
            && hover.y <= tab_r.bottom();
        if drop {
            painter.rect_filled(*tab_r, Rounding::same(8.0), s.accent);
        } else if *active {
            painter.rect_filled(*tab_r, Rounding::same(8.0), s.accent_soft);
            painter.rect_stroke(
                tab_r.shrink(0.5),
                Rounding::same(8.0),
                Stroke::new(1.0_f32, s.accent.gamma_multiply(0.45)),
            );
        } else if hovered {
            painter.rect_filled(*tab_r, Rounding::same(8.0), s.hover);
        }
        let col = if drop {
            Color32::WHITE
        } else if *active {
            s.accent
        } else {
            s.secondary
        };
        painter.text(
            tab_r.center(),
            Align2::CENTER_CENTER,
            label,
            font(12.0),
            col,
        );
    }

    // Drop indicator: a short accent bar where the chip would land.
    if let Some(sx) = slot_x {
        painter.rect_filled(
            Rect::from_center_size(
                Pos2::new(sx - gap / 2.0, r.top() + 21.0),
                Vec2::new(2.5, 22.0),
            ),
            Rounding::same(1.2),
            s.accent,
        );
    }

    // "New category" chip, same rhythm as the others but dashed-looking.
    painter.rect_stroke(
        add_r.shrink(0.5),
        Rounding::same(8.0),
        Stroke::new(1.0_f32, s.hairline),
    );
    painter.text(
        add_r.center(),
        Align2::CENTER_CENTER,
        "+",
        font(15.0),
        s.tertiary,
    );
    drop(painter);

    // The dragged chip follows the pointer, lifted out of the row.
    if drag.is_some() {
        let gx = cursor_x.clamp(strip.left() + drag_w / 2.0, strip.right() - drag_w / 2.0);
        let gr = Rect::from_center_size(Pos2::new(gx, r.top() + 21.0), Vec2::new(drag_w, 28.0));
        let gp = ui.painter();
        gp.rect_filled(
            gr.translate(Vec2::new(0.0, 2.0)),
            Rounding::same(8.0),
            s.shadow,
        );
        gp.rect_filled(gr, Rounding::same(8.0), s.accent_soft);
        gp.rect_stroke(gr, Rounding::same(8.0), Stroke::new(1.2_f32, s.accent));
        gp.text(
            gr.center(),
            Align2::CENTER_CENTER,
            &drag_label,
            font(12.0),
            s.accent,
        );
    }

    d.tab_rects.clear();
    for (key, tab_r, _, _) in &rects {
        if !tab_r.intersects(strip) {
            continue;
        }
        d.tab_rects.push((key.clone(), *tab_r));
        // Clip the *interactive* rect to the strip: a half-scrolled-out chip
        // must not steal clicks from the arrow drawn beside it.
        let hit = tab_r.intersect(strip);
        if hit.width() <= 0.0 || hit.height() <= 0.0 {
            continue;
        }
        // Clicks and hover still come from egui (a click needs no movement, so
        // the frozen pointer position does not matter). The drag is driven by
        // hand below from the live global cursor.
        let resp = ui.interact(hit, egui::Id::new(("dcx-tab", tab_id(key))), Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        // A drag that ends is not a click. egui keeps the pointer frozen at the
        // press point (no `CursorMoved` reaches it), so the release after a
        // 200 px drag still reports `clicked()` on the chip that was grabbed —
        // which switched the tab as a side effect of reordering it.
        if resp.clicked() && !d.blocked() && d.model.tab_drag.is_none() {
            d.model.set_active(key.clone());
            // Force the reveal pass next frame so the new chip is brought into
            // view without waiting for a second click.
            d.model.tab_synced.clear();
        }
    }

    // -- drag-to-reorder, driven from the live global cursor -----------------
    // egui's `drag_started()` never fires here: the panel's winit loop does not
    // deliver `CursorMoved` to egui, so the pointer stays frozen at the press
    // point. Track the press and the threshold by hand instead, mirroring the
    // panel-move / resize gestures in the frame loop.
    let lbtn = crate::win::key_down(crate::win::VK_LBUTTON);
    let cur = d.cursor;

    // 1) Press: any chip under the pointer starts a candidate. Pinned chips are
    //    recorded too, so the gesture can be *explained* instead of dropped.
    if lbtn && d.model.tab_press.is_none() && d.model.tab_drag.is_none() {
        if let Some(c) = cur {
            for (key, tab_r, _, _) in &rects {
                if tab_r.contains(c) {
                    d.model.tab_press = Some(crate::model::TabPress {
                        key: tab_id(key),
                        origin: c,
                        movable: key.key().map(|k| k != CATCH_ALL).unwrap_or(false),
                        spoken: false,
                    });
                    break;
                }
            }
        }
    }

    // 2) Drag: once the pointer travels far enough, lift it into a drag.
    if lbtn {
        if let Some(press) = d.model.tab_press.clone() {
            if let Some(c) = cur {
                if (c - press.origin).length() > DRAG_TAB {
                    if press.movable {
                        d.model.tab_press = None;
                        d.model.tab_drag = Some(crate::model::TabDrag {
                            key: press.key,
                            target: slot,
                        });
                    } else if !press.spoken {
                        // The press stays alive so this runs once per gesture.
                        d.model.tab_press = Some(crate::model::TabPress {
                            key: press.key.clone(),
                            origin: press.origin,
                            movable: false,
                            spoken: true,
                        });
                        let why = if press.key == "k:other" {
                            "「其它」固定排在最后，不参与排序".to_string()
                        } else {
                            "「全部」和「最近」是虚拟页，不参与排序".to_string()
                        };
                        d.model.toast(why);
                        ui.ctx().request_repaint();
                    }
                }
            }
        }
    } else {
        // 3) Release without moving = a plain click (already handled by egui
        //    above); release after a drag commits the reorder below.
        d.model.tab_press = None;
    }

    // The dragged chip has left the row, so its own response no longer exists.
    // Commit on release — including a release outside the panel, which would
    // otherwise leave the strip stuck in "dragging" forever.
    if let Some(t) = d.model.tab_drag.clone() {
        if !lbtn {
            let mut order: Vec<String> = base
                .iter()
                .filter_map(|c| c.0.key().map(|k| k.to_string()))
                .collect();
            let n = order.len();
            let keep_last = if tail { 1 } else { 0 };
            let at = slot
                .saturating_sub(fixed_head)
                .min(n.saturating_sub(keep_last));
            // `TabDrag::key` is the widget id ("k:image"); the config stores
            // the bare category key ("image"). Inserting the id made the
            // reorder skip that category entirely and append it at the end.
            let cfg_key = t.key.strip_prefix("k:").unwrap_or(&t.key).to_string();
            order.insert(at, cfg_key);
            d.model.tab_drag = None;
            d.model.reorder_categories(&order);
            // The active chip may have moved: bring it back into view.
            d.model.tab_synced.clear();
        } else {
            if let Some(td) = &mut d.model.tab_drag {
                td.target = slot;
            }
            ui.ctx().request_repaint();
        }
    }

    // Strip cursor vocabulary: a hand while hovering, the four-way move cursor
    // only once the button is down and the gesture is armed — and it stays that
    // way when the pointer has already travelled outside the strip. Running
    // after the hover pass is what makes it win for the frame.
    if d.model.tab_press.is_some() || d.model.tab_drag.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
    }

    let add_hit = add_r.intersect(strip);
    if add_hit.width() > 0.0
        && ui
            .interact(add_hit, egui::Id::new("dcx-tab-add"), Sense::click())
            .clicked()
        && !d.blocked()
    {
        d.push(Cmd::NewCategoryDialog);
    }
}

fn chip(ui: &mut egui::Ui, r: Rect, s: &Skin, label: &str, active: bool) {
    ui.painter().rect_filled(
        r,
        Rounding::same(5.0),
        if active { s.accent_soft } else { s.surface },
    );
    ui.painter()
        .rect_stroke(r, Rounding::same(5.0), Stroke::new(1.0_f32, s.hairline));
    text(
        ui.painter(),
        r.center(),
        Align2::CENTER_CENTER,
        label,
        11.0,
        if active { s.accent } else { s.secondary },
    );
}

fn draw_list(d: &mut Draw<'_>, ui: &mut egui::Ui, r: Rect) {
    let s = *d.skin;
    let grid = d.model.cfg.view_mode == ViewMode::Grid;
    let inner = r.shrink2(Vec2::new(4.0, 3.0));
    // Round, don't floor: at the default panel width (400–420 pt) the floor of
    // `(width - 10) / 96` was three columns, which left a wide dead band on the
    // right. Rounding gives four, and the cells then stretch to fill the row so
    // no gap remains at any width.
    let cols = if grid {
        (((inner.width() - 10.0) / CARD_W).round() as usize).max(1)
    } else {
        1
    };
    let cell_w = if grid {
        (inner.width() - 10.0) / cols as f32
    } else {
        inner.width()
    };
    let row_h = if grid { CARD_H } else { LIST_ROW };
    let rows_count = if grid {
        (d.model.rows.len() + cols - 1) / cols
    } else {
        d.model.rows.len()
    };
    let content_h = rows_count as f32 * row_h + 8.0;
    let max_scroll = (content_h - inner.height()).max(0.0);

    // Wheel + scrollbar drag, only while the pointer is over the list. A popup
    // or dialog on top owns the wheel: without this check a wheel over the
    // context menu also scrolled the list behind it.
    if !d.blocked() && d.cursor.map(|c| r.contains(c)).unwrap_or(false) {
        let delta = ui.input(|i| i.smooth_scroll_delta.y);
        if delta.abs() > 0.01 {
            d.model.list_scroll = (d.model.list_scroll - delta).clamp(0.0, max_scroll);
        }
    }
    d.model.list_scroll = d.model.list_scroll.clamp(0.0, max_scroll);

    if d.model.rows.is_empty() {
        let c = r.center();
        let (title, hint) = if d.drag_in {
            ("松手即可收纳到盒子", "文件会移入收纳库；拖到分类标签可指定分类")
        } else if !d.model.query.trim().is_empty() {
            ("没有匹配的文件", "试试更短的关键词，或清空搜索")
        } else if d.model.active == Tab::Desktop {
            (
                "桌面上没有项目",
                "新放到桌面的文件会显示在这里，拖进来即可收纳",
            )
        } else if d.model.active == Tab::All && d.model.snap.items.is_empty() {
            (
                "收纳库还是空的",
                "切到「桌面」把文件拖进来，或用 ⋯ 菜单的「自动收纳桌面」",
            )
        } else {
            (
                "这个分类下暂时没有文件",
                "把文件拖进来，或拖到其它分类标签上",
            )
        };
        text(
            ui.painter(),
            Pos2::new(c.x, c.y - 11.0),
            Align2::CENTER_CENTER,
            title,
            13.0,
            s.secondary,
        );
        text(
            ui.painter(),
            Pos2::new(c.x, c.y + 11.0),
            Align2::CENTER_CENTER,
            hint,
            11.0,
            s.tertiary,
        );
        blank_list_click(d, ui, inner, &[]);
        return;
    }

    let mut hit_rects: Vec<Rect> = Vec::new();
    let painter = ui.painter().with_clip_rect(inner);
    let first = (d.model.list_scroll / row_h).floor() as usize;
    let last = ((d.model.list_scroll + inner.height()) / row_h).ceil() as usize + 1;
    for ri in first..last.min(rows_count) {
        let y = inner.top() + ri as f32 * row_h - d.model.list_scroll;
        if grid {
            for ci in 0..cols {
                let idx = ri * cols + ci;
                let Some(row) = d.model.rows.get(idx).cloned() else {
                    break;
                };
                let e = row.entry.clone();
                let card = Rect::from_min_size(
                    Pos2::new(inner.left() + 5.0 + ci as f32 * cell_w, y),
                    Vec2::new(cell_w - 4.0, CARD_H - 4.0),
                );
                // The painter is clipped to `inner`, but `ui.interact` is not:
                // the row poking out above the viewport used to keep a clickable
                // rectangle over the tab strip, and being registered after the
                // tabs it won every hit test there — the nav bar went dead once
                // the grid was scrolled.
                let hit = card.intersect(inner);
                if hit.width() < 1.0 || hit.height() < 1.0 {
                    continue;
                }
                hit_rects.push(hit);
                if e.is_dir {
                    d.folder_rects.push((e.path.clone(), hit));
                }
                // Stable id: an auto id depends on how many widgets were
                // allocated before it, and that count changes as the list
                // scrolls — which silently cancels drags.
                let resp = ui.interact(
                    hit,
                    egui::Id::new(("dcx-row", &e.path)),
                    Sense::click_and_drag(),
                );
                let target = d.drop_folder.as_ref() == Some(&e.path);
                draw_card(d, ui, &painter, card, e, &resp, target);
            }
        } else {
            let row_r = Rect::from_min_max(
                Pos2::new(inner.left() + 4.0, y),
                Pos2::new(inner.right() - 12.0, y + LIST_ROW),
            );
            let row = d.model.rows[ri].clone();
            let e = row.entry.clone();
            // Same clip as the grid branch: a row scrolled half out of view must
            // not keep a click target over the tabs or the footer.
            let hit = row_r.intersect(inner);
            if hit.width() < 1.0 || hit.height() < 1.0 {
                continue;
            }
            hit_rects.push(hit);
            if e.is_dir {
                d.folder_rects.push((e.path.clone(), hit));
            }
            let resp = ui.interact(
                hit,
                egui::Id::new(("dcx-row", &e.path)),
                Sense::click_and_drag(),
            );
            let target = d.drop_folder.as_ref() == Some(&e.path);
            draw_row(d, ui, &painter, row_r, row, ri, &resp, target);
        }
    }
    drop(painter);
    blank_list_click(d, ui, inner, &hit_rects);

    // Scrollbar. It hugs the right edge (the Win11 look), but the frame loop
    // knows its rectangle and refuses to start an edge resize there — that is
    // what used to make the thumb fight the resize band.
    if max_scroll > 0.5 {
        let bar_r = Rect::from_min_max(
            Pos2::new(r.right() - 9.0, r.top() + 4.0),
            Pos2::new(r.right() - 4.0, r.bottom() - 4.0),
        );
        d.scrollbar_rect = Some(bar_r);
        let visible = (inner.height() / content_h).clamp(0.0, 1.0);
        let thumb_h = (bar_r.height() * visible).max(26.0);
        let travel = (bar_r.height() - thumb_h).max(1.0);
        // Win11 uses a translucent grey, not a solid mid-grey: a solid thumb
        // reads as part of the content and was far too heavy at this width.
        let (track, thumb_col, thumb_hot) = if s.dark {
            (
                Color32::from_white_alpha(10),
                Color32::from_white_alpha(45),
                Color32::from_white_alpha(90),
            )
        } else {
            (
                Color32::from_black_alpha(8),
                Color32::from_black_alpha(40),
                Color32::from_black_alpha(75),
            )
        };
        ui.painter().rect_filled(bar_r, Rounding::same(2.5), track);
        let t = (d.model.list_scroll / max_scroll).clamp(0.0, 1.0);
        let thumb = Rect::from_min_size(
            Pos2::new(bar_r.left(), bar_r.top() + t * travel),
            Vec2::new(bar_r.width(), thumb_h),
        );
        // A fixed id: `allocate_rect` derives its id from the allocation order,
        // and the number of visible rows changes while scrolling, so the id
        // changed mid-gesture and egui dropped the drag every few frames.
        let resp = ui.interact(
            bar_r,
            egui::Id::new("dcx-scrollbar"),
            Sense::click_and_drag(),
        );
        let hot = resp.hovered() || resp.dragged();
        ui.painter().rect_filled(
            thumb,
            Rounding::same(2.5),
            if hot { thumb_hot } else { thumb_col },
        );

        // Drag the thumb by its **grab offset**, not by its centre. The old
        // "put the thumb centre under the cursor" mapping jumped on the first
        // frame and moved faster than the pointer for the rest of the gesture,
        // which is what felt non-linear.
        if resp.drag_started() {
            if let Some(pos) = ui.ctx().pointer_interact_pos() {
                let grab = (pos.y - thumb.top()).clamp(0.0, thumb_h);
                d.model.scroll_grab = Some(grab);
            }
        }
        if resp.clicked() {
            // A click on the track jumps so the thumb is centred under the
            // cursor (standard behaviour), then the next drag keeps that.
            if let Some(pos) = ui.ctx().pointer_interact_pos() {
                let grab = thumb_h / 2.0;
                d.model.scroll_grab = Some(grab);
                let f = ((pos.y - grab - bar_r.top()) / travel).clamp(0.0, 1.0);
                d.model.list_scroll = f * max_scroll;
            }
        }
        if resp.dragged() {
            if let (Some(pos), Some(grab)) = (ui.ctx().pointer_interact_pos(), d.model.scroll_grab)
            {
                let f = ((pos.y - grab - bar_r.top()) / travel).clamp(0.0, 1.0);
                d.model.list_scroll = f * max_scroll;
            }
        }
        if resp.drag_stopped() {
            d.model.scroll_grab = None;
        }
    }
}

/// Clicks that land on the list background (not on any row). egui's row
/// responses cannot see them, so this reads the raw pointer buttons and uses
/// the same hit rectangles the rows were drawn with.
fn blank_list_click(d: &mut Draw<'_>, ui: &egui::Ui, inner: Rect, hits: &[Rect]) {
    if d.blocked() {
        return;
    }
    let Some(c) = d.cursor else {
        return;
    };
    if !inner.contains(c) || hits.iter().any(|r| r.contains(c)) {
        return;
    }
    let (primary, secondary) = ui.input(|i| {
        (
            i.pointer.primary_clicked(),
            i.pointer.secondary_clicked(),
        )
    });
    if secondary {
        d.model.selection.clear();
        let items = d.model.blank_items();
        d.model.open_popup(c, None, items, 192.0);
    } else if primary {
        d.model.selection.clear();
    }
}

fn row_bg(p: &Painter, r: Rect, s: &Skin, selected: bool, hovered: bool) {
    if selected {
        p.rect_filled(r, Rounding::same(6.0), s.accent_soft);
        p.rect_stroke(
            r.shrink(0.5),
            Rounding::same(6.0),
            Stroke::new(1.0_f32, s.accent),
        );
    } else if hovered {
        p.rect_filled(r, Rounding::same(6.0), s.hover);
    }
}

/// Name shown in a row or card. `hide_ext` mirrors Explorer's「隐藏已知文件类型
/// 的扩展名」: only the final extension goes, and a dotfile keeps its name.
fn display_name(e: &dcx_organizer::Entry, hide_ext: bool) -> String {
    if !hide_ext || e.is_dir {
        return e.name.clone();
    }
    match e.name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => e.name.clone(),
    }
}

fn draw_row(
    d: &mut Draw<'_>,
    ui: &mut egui::Ui,
    p: &Painter,
    r: Rect,
    row: crate::model::RowItem,
    idx: usize,
    resp: &egui::Response,
    drop_target: bool,
) {
    let s = *d.skin;
    let e = &row.entry;
    let selected = d.model.selection.contains(&e.path);
    row_bg(p, r, &s, selected, resp.hovered());
    if drop_target {
        p.rect_filled(r, Rounding::same(6.0), s.accent_soft);
        p.rect_stroke(
            r.shrink(0.5),
            Rounding::same(6.0),
            Stroke::new(1.5_f32, s.accent),
        );
    }

    let indent = row.depth as f32 * 16.0;
    let color = if e.is_dir {
        s.accent
    } else {
        cat_color(&e.category, &s)
    };
    // Tree rows reserve a disclosure column before the icon, so folder and
    // file names line up at the same depth.
    // Search results are flat: a disclosure arrow there would promise
    // children that the query is deliberately not showing.
    let chevron_r = if e.is_dir
        && d.model.active != Tab::Desktop
        && d.model.query.trim().is_empty()
    {
        let cr = Rect::from_center_size(
            Pos2::new(r.left() + 9.0 + indent, r.center().y),
            Vec2::splat(14.0),
        );
        let st = Stroke::new(1.3_f32, s.secondary);
        let c = cr.center();
        if row.expanded {
            p.line_segment(
                [Pos2::new(c.x - 3.5, c.y - 2.0), Pos2::new(c.x, c.y + 2.0)],
                st,
            );
            p.line_segment(
                [Pos2::new(c.x, c.y + 2.0), Pos2::new(c.x + 3.5, c.y - 2.0)],
                st,
            );
        } else {
            p.line_segment(
                [Pos2::new(c.x - 2.0, c.y - 3.5), Pos2::new(c.x + 2.0, c.y)],
                st,
            );
            p.line_segment(
                [Pos2::new(c.x + 2.0, c.y), Pos2::new(c.x - 2.0, c.y + 3.5)],
                st,
            );
        }
        Some(cr)
    } else {
        None
    };
    let icon_r = Rect::from_center_size(
        Pos2::new(r.left() + 25.0 + indent, r.center().y),
        Vec2::splat(21.0),
    );
    paint_icon(d, ui.ctx(), p, icon_r, e, color);

    let meta = {
        let mut parts: Vec<String> = Vec::new();
        if d.model.cfg.show_size {
            parts.push(if e.is_dir {
                "文件夹".to_string()
            } else {
                fmt_size(e.size)
            });
        }
        if d.model.cfg.show_time {
            let t = fmt_time(e.modified);
            if !t.is_empty() {
                parts.push(t);
            }
        }
        parts.join(" · ")
    };
    // No meta column when both switches are off: the name gets the width back
    // instead of leaving a 118 pt hole on the right.
    let meta_w = if meta.is_empty() { 12.0 } else { 118.0 };
    let name_r = Rect::from_min_max(
        Pos2::new(r.left() + 41.0 + indent, r.top()),
        Pos2::new(r.right() - meta_w - 8.0, r.bottom()),
    );
    let name = display_name(e, d.model.cfg.hide_ext);
    elide(
        &p,
        &name,
        Pos2::new(name_r.left(), r.center().y),
        name_r.width(),
        12.5,
        s.text,
    );
    text(
        &p,
        Pos2::new(r.right() - 4.0, r.center().y),
        Align2::RIGHT_CENTER,
        &meta,
        10.5,
        s.tertiary,
    );
    // Manual-classification marker.
    if e.manual {
        p.circle_filled(
            Pos2::new(r.left() + 3.0 + indent, r.center().y),
            1.6,
            s.accent,
        );
    }

    handle_row_input(d, ui, e, idx, resp, r, chevron_r);
}

fn draw_card(
    d: &mut Draw<'_>,
    ui: &mut egui::Ui,
    p: &Painter,
    r: Rect,
    e: dcx_organizer::Entry,
    resp: &egui::Response,
    drop_target: bool,
) {
    let s = *d.skin;
    let selected = d.model.selection.contains(&e.path);
    row_bg(p, r, &s, selected, resp.hovered());
    if drop_target {
        p.rect_filled(r, Rounding::same(6.0), s.accent_soft);
        p.rect_stroke(
            r.shrink(0.5),
            Rounding::same(6.0),
            Stroke::new(1.5_f32, s.accent),
        );
    }
    let color = if e.is_dir {
        s.accent
    } else {
        cat_color(&e.category, &s)
    };
    let icon_r = Rect::from_center_size(Pos2::new(r.center().x, r.top() + 30.0), Vec2::splat(34.0));
    paint_icon(d, ui.ctx(), p, icon_r, &e, color);
    // Cards centre the label when it fits and elide when it does not — a short
    // name pinned to the left edge looked detached from its icon.
    let label_w = r.width() - 12.0;
    let name = display_name(&e, d.model.cfg.hide_ext);
    let name_w = p.fonts(|f| {
        f.layout_no_wrap(name.clone(), font(11.0), Color32::WHITE)
            .size()
            .x
    });
    let name_x = if name_w <= label_w {
        r.center().x - name_w / 2.0
    } else {
        r.center().x - label_w / 2.0
    };
    elide(
        &p,
        &name,
        Pos2::new(name_x, r.top() + 62.0),
        label_w,
        11.0,
        s.text,
    );
    if d.model.cfg.show_size {
        let size_label = if e.is_dir {
            "文件夹".to_string()
        } else {
            fmt_size(e.size)
        };
        text(
            &p,
            Pos2::new(r.center().x, r.top() + 79.0),
            Align2::CENTER_CENTER,
            &size_label,
            9.5,
            s.tertiary,
        );
    }
    handle_row_input(d, ui, &e, 0, resp, r, None);
}

fn paint_icon(
    d: &mut Draw<'_>,
    ctx: &egui::Context,
    p: &Painter,
    r: Rect,
    e: &dcx_organizer::Entry,
    color: Color32,
) {
    let tex = d.icons.get(ctx, &e.path, &e.ext, e.is_dir);
    match tex {
        Some(t) => {
            p.image(
                t.id(),
                r,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => {
            p.rect_filled(r, Rounding::same(5.0), color.gamma_multiply(0.18));
            let label = if e.is_dir {
                "▣".to_string()
            } else if e.ext.is_empty() {
                "—".to_string()
            } else {
                e.ext.to_uppercase().chars().take(3).collect()
            };
            text(&p, r.center(), Align2::CENTER_CENTER, &label, 8.5, color);
        }
    }
}

fn handle_row_input(
    d: &mut Draw<'_>,
    ui: &mut egui::Ui,
    e: &dcx_organizer::Entry,
    _idx: usize,
    resp: &egui::Response,
    r: Rect,
    chevron: Option<Rect>,
) {
    if d.blocked() {
        return;
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.clicked() {
        // The disclosure triangle is part of the row's hit rectangle. Without
        // consuming it here, clicking it would just select the folder — the
        // first version of the tree looked like it did nothing.
        let on_chevron = chevron
            .zip(d.cursor)
            .map(|(cr, c)| cr.contains(c))
            .unwrap_or(false);
        if on_chevron {
            d.push(Cmd::ToggleExpand(e.path.clone()));
            return;
        }
        d.model.clicked_row(
            &e.path,
            0,
            crate::win::ctrl_down(),
            crate::win::shift_down(),
        );
        if d.model.cfg.single_click_open {
            d.push(Cmd::Open(e.path.clone()));
        }
    }
    if resp.double_clicked() && !d.model.cfg.single_click_open {
        d.push(Cmd::Open(e.path.clone()));
    }
    if resp.secondary_clicked() {
        if !d.model.selection.contains(&e.path) {
            d.model.clicked_row(&e.path, 0, false, false);
        }
        let items = d.model.context_items(&e.path);
        let at = d.cursor.unwrap_or(r.center());
        d.model.open_popup(at, None, items, 212.0);
    }
    if resp.drag_started() {
        d.row_drag_start = Some((e.path.clone(), e.category.clone()));
    }
}

fn draw_footer(d: &mut Draw<'_>, ui: &mut egui::Ui, r: Rect) {
    let s = *d.skin;
    let p = ui.painter().clone();
    let sel = d.model.selection.len();
    let left = if d.drag_in {
        "松手收纳到盒子 · 拖到分类标签可指定分类".to_string()
    } else if sel > 0 {
        format!("已选中 {} 项", sel)
    } else {
        format!(
            "{} · {} 项 · {}",
            d.model.active_label(),
            d.model.top_count(),
            fmt_size(d.model.total_bytes())
        )
    };
    text(
        &p,
        Pos2::new(r.left() + 14.0, r.center().y),
        Align2::LEFT_CENTER,
        &left,
        11.0,
        if d.drag_in { s.accent } else { s.secondary },
    );

    let mut x = r.right() - 12.0;
    let mk = |w: f32, x: &mut f32| {
        let rr = Rect::from_center_size(Pos2::new(*x - w / 2.0, r.center().y), Vec2::new(w, 24.0));
        *x -= w + 6.0;
        rr
    };
    let library_r = mk(58.0, &mut x);
    let reset_r = mk(76.0, &mut x);
    chip(ui, library_r, &s, "收纳", false);
    chip(ui, reset_r, &s, "重新分类", false);
    let library = ui
        .allocate_rect(library_r, Sense::click())
        .on_hover_text("单击复制收纳库路径，双击打开");
    if !d.blocked() {
        // The first click of a double click also reports `clicked()`: the path
        // is copied, then the second click wins and opens the folder.
        if library.double_clicked() {
            d.push(Cmd::OpenLibrary);
        } else if library.clicked() {
            d.push(Cmd::CopyLibraryPath);
        }
    }
    if ui.allocate_rect(reset_r, Sense::click()).clicked() && !d.blocked() {
        d.push(Cmd::ReclassifyAll);
    }
    let overrides = d.model.org.override_count();
    if overrides > 0 && sel == 0 {
        text(
            &p,
            Pos2::new(reset_r.left() - 10.0, r.center().y),
            Align2::RIGHT_CENTER,
            &format!("手动分类 {overrides} 项"),
            10.0,
            s.tertiary,
        );
    }
    let _ = x;
}

// ---------------------------------------------------------------------------
// Popup / dialog / toast
// ---------------------------------------------------------------------------

/// One glyph per menu row. Everything is drawn from strokes so the menu keeps
/// the same weight as the rest of the panel at any DPI.
fn menu_icon(p: &Painter, r: Rect, icon: MenuIcon, color: Color32) {
    let c = r.center();
    let st = Stroke::new(1.25_f32, color);
    let thin = Stroke::new(1.1_f32, color);
    let half = r.width() * 0.5;
    match icon {
        MenuIcon::None => {}
        // A pointer leaving a box: "clicks outside it".
        MenuIcon::Outside => {
            let box_r = Rect::from_min_max(
                Pos2::new(c.x - half * 0.95, c.y - half * 0.95),
                Pos2::new(c.x + half * 0.05, c.y + half * 0.05),
            );
            p.rect_stroke(box_r, Rounding::same(1.5), thin);
            let tip = Pos2::new(c.x + half * 0.95, c.y + half * 0.95);
            p.line_segment([Pos2::new(c.x + half * 0.15, c.y + half * 0.15), tip], st);
            p.line_segment([tip, Pos2::new(c.x + half * 0.25, c.y + half * 0.9)], st);
            p.line_segment([tip, Pos2::new(c.x + half * 0.9, c.y + half * 0.25)], st);
        }
        // Two overlapping translucent panes: the frosted-glass idea.
        MenuIcon::Acrylic => {
            let a = Rect::from_min_max(
                Pos2::new(c.x - half * 0.95, c.y - half * 0.95),
                Pos2::new(c.x + half * 0.2, c.y + half * 0.2),
            );
            let b = Rect::from_min_max(
                Pos2::new(c.x - half * 0.2, c.y - half * 0.2),
                Pos2::new(c.x + half * 0.95, c.y + half * 0.95),
            );
            p.rect_stroke(a, Rounding::same(1.5), thin);
            p.rect_filled(b, Rounding::same(1.5), color.gamma_multiply(0.28));
            p.rect_stroke(b, Rounding::same(1.5), st);
        }
        MenuIcon::Refresh => {
            p.circle_stroke(c, half * 0.82, st);
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.45, c.y - half * 0.95),
                    Pos2::new(c.x + half * 0.95, c.y - half * 0.55),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.95, c.y - half * 0.55),
                    Pos2::new(c.x + half * 0.42, c.y - half * 0.35),
                ],
                st,
            );
        }
        MenuIcon::Folder => {
            let body = Rect::from_min_max(
                Pos2::new(c.x - half * 0.95, c.y - half * 0.45),
                Pos2::new(c.x + half * 0.95, c.y + half * 0.75),
            );
            p.rect_stroke(body, Rounding::same(1.5), st);
            p.line_segment(
                [
                    Pos2::new(body.left(), c.y - half * 0.45),
                    Pos2::new(body.left() + half * 0.6, c.y - half * 0.45),
                ],
                thin,
            );
        }
        MenuIcon::Reclassify => {
            for dx in [-half * 0.45, half * 0.45] {
                p.line_segment(
                    [
                        Pos2::new(c.x + dx, c.y - half * 0.8),
                        Pos2::new(c.x + dx, c.y + half * 0.8),
                    ],
                    thin,
                );
            }
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.75, c.y - half * 0.35),
                    Pos2::new(c.x - half * 0.45, c.y - half * 0.75),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.15, c.y - half * 0.35),
                    Pos2::new(c.x - half * 0.45, c.y - half * 0.75),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.15, c.y + half * 0.35),
                    Pos2::new(c.x + half * 0.45, c.y + half * 0.75),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.75, c.y + half * 0.35),
                    Pos2::new(c.x + half * 0.45, c.y + half * 0.75),
                ],
                st,
            );
        }
        MenuIcon::List => {
            for i in 0..3 {
                let y = c.y - half * 0.62 + i as f32 * half * 0.62;
                p.circle_filled(Pos2::new(c.x - half * 0.72, y), 1.0, color);
                p.line_segment(
                    [
                        Pos2::new(c.x - half * 0.35, y),
                        Pos2::new(c.x + half * 0.85, y),
                    ],
                    thin,
                );
            }
        }
        MenuIcon::Grid => {
            for (dx, dy) in [(-1.0f32, -1.0f32), (0.15, -1.0), (-1.0, 0.15), (0.15, 0.15)] {
                p.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(c.x + dx * half * 0.8, c.y + dy * half * 0.8),
                        Vec2::splat(half * 0.72),
                    ),
                    Rounding::same(1.0),
                    color,
                );
            }
        }
        MenuIcon::Eye | MenuIcon::EyeOff => {
            p.circle_stroke(c, half * 0.85, thin);
            p.circle_filled(c, half * 0.32, color);
            if icon == MenuIcon::EyeOff {
                p.line_segment(
                    [
                        Pos2::new(c.x - half * 0.9, c.y + half * 0.9),
                        Pos2::new(c.x + half * 0.9, c.y - half * 0.9),
                    ],
                    st,
                );
            }
        }
        MenuIcon::Cursor => {
            let pts = vec![
                Pos2::new(c.x - half * 0.45, c.y - half * 0.85),
                Pos2::new(c.x + half * 0.35, c.y + half * 0.1),
                Pos2::new(c.x - half * 0.05, c.y + half * 0.18),
                Pos2::new(c.x - half * 0.12, c.y + half * 0.85),
            ];
            p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        MenuIcon::Plus => {
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.7, c.y),
                    Pos2::new(c.x + half * 0.7, c.y),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x, c.y - half * 0.7),
                    Pos2::new(c.x, c.y + half * 0.7),
                ],
                st,
            );
        }
        MenuIcon::Gear => {
            // A tight ring with short teeth: long spokes at this size read as a
            // snowflake instead of a gear.
            p.circle_stroke(c, half * 0.52, Stroke::new(1.3_f32, color));
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::PI / 4.0;
                p.line_segment(
                    [
                        Pos2::new(c.x + a.cos() * half * 0.66, c.y + a.sin() * half * 0.66),
                        Pos2::new(c.x + a.cos() * half * 0.92, c.y + a.sin() * half * 0.92),
                    ],
                    Stroke::new(1.5_f32, color),
                );
            }
            p.circle_filled(c, 1.1, color);
        }
        MenuIcon::Info => {
            p.circle_stroke(c, half * 0.85, st);
            p.circle_filled(Pos2::new(c.x, c.y - half * 0.35), 1.0, color);
            p.line_segment(
                [
                    Pos2::new(c.x, c.y - half * 0.02),
                    Pos2::new(c.x, c.y + half * 0.5),
                ],
                st,
            );
        }
        MenuIcon::Open => {
            let box_r = Rect::from_min_max(
                Pos2::new(c.x - half * 0.85, c.y - half * 0.35),
                Pos2::new(c.x + half * 0.35, c.y + half * 0.85),
            );
            p.rect_stroke(box_r, Rounding::same(1.5), thin);
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.1, c.y + half * 0.1),
                    Pos2::new(c.x + half * 0.85, c.y - half * 0.8),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.35, c.y - half * 0.8),
                    Pos2::new(c.x + half * 0.85, c.y - half * 0.8),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.85, c.y - half * 0.8),
                    Pos2::new(c.x + half * 0.85, c.y - half * 0.3),
                ],
                st,
            );
        }
        MenuIcon::Copy => {
            let back = Rect::from_min_size(
                Pos2::new(c.x - half * 0.85, c.y - half * 0.75),
                Vec2::splat(half * 1.15),
            );
            let front = Rect::from_min_size(
                Pos2::new(c.x - half * 0.25, c.y - half * 0.15),
                Vec2::splat(half * 1.15),
            );
            p.rect_stroke(back, Rounding::same(1.5), thin);
            p.rect_filled(front, Rounding::same(1.5), Color32::TRANSPARENT);
            p.rect_stroke(front, Rounding::same(1.5), st);
        }
        MenuIcon::Rename => {
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.75, c.y + half * 0.65),
                    Pos2::new(c.x + half * 0.55, c.y - half * 0.65),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.75, c.y + half * 0.65),
                    Pos2::new(c.x - half * 0.35, c.y + half * 0.75),
                ],
                thin,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.55, c.y - half * 0.65),
                    Pos2::new(c.x + half * 0.8, c.y - half * 0.35),
                ],
                thin,
            );
        }
        MenuIcon::Tag => {
            let pts = vec![
                Pos2::new(c.x - half * 0.8, c.y),
                Pos2::new(c.x, c.y - half * 0.8),
                Pos2::new(c.x + half * 0.8, c.y),
                Pos2::new(c.x, c.y + half * 0.8),
            ];
            p.add(egui::Shape::convex_polygon(pts, Color32::TRANSPARENT, thin));
            p.circle_filled(c, 1.3, color);
        }
        MenuIcon::TextName => {
            // Letter "T" — the closest vector shorthand for "name".
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.75, c.y - half * 0.6),
                    Pos2::new(c.x + half * 0.75, c.y - half * 0.6),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x, c.y - half * 0.6),
                    Pos2::new(c.x, c.y + half * 0.7),
                ],
                st,
            );
        }
        MenuIcon::Clock => {
            p.circle_stroke(c, half * 0.85, st);
            p.line_segment([c, Pos2::new(c.x, c.y - half * 0.45)], st);
            p.line_segment([c, Pos2::new(c.x + half * 0.4, c.y + half * 0.1)], st);
        }
        MenuIcon::SortSize => {
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.55, c.y - half * 0.75),
                    Pos2::new(c.x - half * 0.55, c.y + half * 0.75),
                ],
                thin,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.85, c.y - half * 0.45),
                    Pos2::new(c.x - half * 0.55, c.y - half * 0.75),
                ],
                thin,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.25, c.y - half * 0.45),
                    Pos2::new(c.x - half * 0.55, c.y - half * 0.75),
                ],
                thin,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.55, c.y + half * 0.75),
                    Pos2::new(c.x + half * 0.55, c.y - half * 0.75),
                ],
                thin,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.25, c.y + half * 0.45),
                    Pos2::new(c.x + half * 0.55, c.y + half * 0.75),
                ],
                thin,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.85, c.y + half * 0.45),
                    Pos2::new(c.x + half * 0.55, c.y + half * 0.75),
                ],
                thin,
            );
        }
        MenuIcon::FileType => {
            let body = Rect::from_min_max(
                Pos2::new(c.x - half * 0.55, c.y - half * 0.8),
                Pos2::new(c.x + half * 0.6, c.y + half * 0.8),
            );
            p.rect_stroke(body, Rounding::same(1.5), st);
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.2, c.y + half * 0.05),
                    Pos2::new(c.x + half * 0.25, c.y + half * 0.05),
                ],
                thin,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.2, c.y + half * 0.38),
                    Pos2::new(c.x + half * 0.1, c.y + half * 0.38),
                ],
                thin,
            );
        }
        MenuIcon::ArrowUpDown => {
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.45, c.y + half * 0.7),
                    Pos2::new(c.x - half * 0.45, c.y - half * 0.7),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.75, c.y - half * 0.35),
                    Pos2::new(c.x - half * 0.45, c.y - half * 0.7),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.15, c.y - half * 0.35),
                    Pos2::new(c.x - half * 0.45, c.y - half * 0.7),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.45, c.y - half * 0.7),
                    Pos2::new(c.x + half * 0.45, c.y + half * 0.7),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.15, c.y + half * 0.35),
                    Pos2::new(c.x + half * 0.45, c.y + half * 0.7),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.75, c.y + half * 0.35),
                    Pos2::new(c.x + half * 0.45, c.y + half * 0.7),
                ],
                st,
            );
        }
        MenuIcon::Release => {
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.55, c.y - half * 0.75),
                    Pos2::new(c.x - half * 0.65, c.y),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.65, c.y),
                    Pos2::new(c.x + half * 0.55, c.y + half * 0.75),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.65, c.y),
                    Pos2::new(c.x + half * 0.9, c.y),
                ],
                thin,
            );
        }
        MenuIcon::Trash => {
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.85, c.y - half * 0.55),
                    Pos2::new(c.x + half * 0.85, c.y - half * 0.55),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.2, c.y - half * 0.8),
                    Pos2::new(c.x + half * 0.2, c.y - half * 0.8),
                ],
                thin,
            );
            let body = Rect::from_min_max(
                Pos2::new(c.x - half * 0.62, c.y - half * 0.35),
                Pos2::new(c.x + half * 0.62, c.y + half * 0.8),
            );
            p.rect_stroke(body, Rounding::same(1.5), st);
            for dx in [-half * 0.2, half * 0.2] {
                p.line_segment(
                    [
                        Pos2::new(c.x + dx, body.top() + 2.0),
                        Pos2::new(c.x + dx, body.bottom() - 2.0),
                    ],
                    thin,
                );
            }
        }
        MenuIcon::Check => {
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.7, c.y),
                    Pos2::new(c.x - half * 0.15, c.y + half * 0.55),
                ],
                Stroke::new(1.5_f32, color),
            );
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.15, c.y + half * 0.55),
                    Pos2::new(c.x + half * 0.75, c.y - half * 0.6),
                ],
                Stroke::new(1.5_f32, color),
            );
        }
        // Tree disclosure glyphs, same directions as the row chevrons.
        MenuIcon::Expand => {
            p.line_segment(
                [Pos2::new(c.x - 4.0, c.y - 2.0), Pos2::new(c.x, c.y + 2.0)],
                st,
            );
            p.line_segment(
                [Pos2::new(c.x, c.y + 2.0), Pos2::new(c.x + 4.0, c.y - 2.0)],
                st,
            );
        }
        MenuIcon::Collapse => {
            p.line_segment(
                [Pos2::new(c.x - 4.0, c.y + 2.0), Pos2::new(c.x, c.y - 2.0)],
                st,
            );
            p.line_segment(
                [Pos2::new(c.x, c.y - 2.0), Pos2::new(c.x + 4.0, c.y + 2.0)],
                st,
            );
        }
        // An arrow entering a folder body.
        MenuIcon::MoveIn => {
            let body = Rect::from_min_max(
                Pos2::new(c.x - half * 0.95, c.y - half * 0.35),
                Pos2::new(c.x + half * 0.95, c.y + half * 0.8),
            );
            p.rect_stroke(body, Rounding::same(1.5), st);
            p.line_segment(
                [
                    Pos2::new(c.x - half * 0.55, c.y - half * 0.65),
                    Pos2::new(c.x + half * 0.2, c.y - half * 0.65),
                ],
                thin,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.75, c.y - half * 1.0),
                    Pos2::new(c.x + half * 0.75, c.y + half * 0.15),
                ],
                thin,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.45, c.y - half * 0.15),
                    Pos2::new(c.x + half * 0.75, c.y + half * 0.15),
                ],
                thin,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + half * 0.75, c.y + half * 0.15),
                    Pos2::new(c.x + half * 1.05, c.y - half * 0.15),
                ],
                thin,
            );
        }
    }
}

/// Width for a dropdown: the widest label plus the icon column and the right
/// gutter, clamped. A fixed width made 「更多」 and 「显示」 look padded out while
/// a long label like 「当前：降序 · 点此改为升序」 clipped.
fn popup_width(ui: &egui::Ui, items: &[PopupItem], min: f32, max: f32) -> f32 {
    let mut widest = 0.0f32;
    for it in items {
        if it.separator {
            continue;
        }
        let w = ui.fonts(|f| {
            f.layout_no_wrap(it.label.clone(), font(11.5), Color32::WHITE)
                .size()
                .x
        });
        widest = widest.max(w);
    }
    // 30 pt icon column + 22 pt right gutter for the check mark.
    (widest + 30.0 + 22.0).clamp(min, max)
}

fn draw_popup(d: &mut Draw<'_>, ui: &mut egui::Ui, panel: Rect) {
    let Some(pop) = d.model.popup.clone() else {
        d.popup_rect = None;
        return;
    };
    let s = *d.skin;
    let item_h = 25.0;
    let head_h = if pop.title.is_some() { 26.0 } else { 6.0 };
    let seps = pop.items.iter().filter(|i| i.separator).count() as f32;
    let rows_h = (pop.items.len() - seps as usize) as f32 * item_h + seps * 7.0;
    let h = (head_h + rows_h + 8.0)
        .min(panel.height() - 24.0)
        .min(460.0);
    let mut min = pop.at;
    min.x = min.x.clamp(
        panel.left() + 6.0,
        (panel.right() - pop.width - 6.0).max(panel.left() + 6.0),
    );
    min.y = min.y.clamp(
        panel.top() + 6.0,
        (panel.bottom() - h - 6.0).max(panel.top() + 6.0),
    );
    let r = Rect::from_min_size(min, Vec2::new(pop.width, h));
    d.popup_rect = Some(r);

    let p = ui.painter().clone();
    for i in (1..=3).rev() {
        let k = i as f32;
        p.rect_filled(
            r.expand(k * 1.5).translate(Vec2::new(0.0, k)),
            Rounding::same(10.0 + k),
            s.shadow,
        );
    }
    p.rect_filled(r, Rounding::same(9.0), s.popup);
    p.rect_stroke(r, Rounding::same(9.0), Stroke::new(1.0_f32, s.hairline));
    let mut y = r.top() + 4.0;
    if let Some(title) = &pop.title {
        text(
            &p,
            Pos2::new(r.left() + 12.0, y + 10.0),
            Align2::LEFT_CENTER,
            title,
            10.5,
            s.tertiary,
        );
        y += head_h;
        p.hline(
            r.left() + 8.0..=r.right() - 8.0,
            y,
            Stroke::new(1.0_f32, s.divider),
        );
    }
    let clip = p.with_clip_rect(Rect::from_min_max(
        Pos2::new(r.left(), y),
        Pos2::new(r.right(), r.bottom() - 4.0),
    ));
    let mut cy = y + 2.0 - pop.scroll;
    let mut click: Option<Cmd> = None;
    for item in &pop.items {
        if item.separator {
            clip.hline(
                r.left() + 10.0..=r.right() - 10.0,
                cy + 3.0,
                Stroke::new(1.0_f32, s.divider),
            );
            cy += 7.0;
            continue;
        }
        let ir = Rect::from_min_size(
            Pos2::new(r.left() + 4.0, cy),
            Vec2::new(pop.width - 8.0, item_h - 1.0),
        );
        if cy + item_h > y && cy < r.bottom() - 4.0 {
            // A slider row: label left, track right. The knob is dragged from
            // the live global cursor — egui's pointer position is frozen here
            // (no `CursorMoved` reaches it), same reason the tab strip does its
            // gestures by hand.
            if let Some(v0) = item.slider {
                let track = Rect::from_min_max(
                    Pos2::new(ir.left() + 66.0, ir.center().y - 1.5),
                    Pos2::new(ir.right() - 14.0, ir.center().y + 1.5),
                );
                // Input **before** drawing: routing the new value through a
                // `Cmd` meant the knob was always one frame behind the pointer,
                // which read as "not following the drag".
                let mut v = v0;
                if crate::win::key_down(crate::win::VK_LBUTTON) {
                    if let Some(c) = d.cursor {
                        // A generous vertical band: the track is only 3 pt tall.
                        let band = Rect::from_min_max(
                            Pos2::new(ir.left(), ir.center().y - 11.0),
                            Pos2::new(ir.right(), ir.center().y + 11.0),
                        );
                        if band.contains(c) {
                            let t = ((c.x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0);
                            let alpha = (120.0 + t * 135.0).round() as u16;
                            if alpha != d.model.cfg.acrylic_alpha {
                                d.model.cfg.acrylic_alpha = alpha;
                                d.model.alpha_dirty = Some(std::time::Instant::now());
                            }
                            v = t;
                            // The popup keeps its own copy of the row (built
                            // when the menu opened). Without writing the new
                            // position back the knob fell back to that stale
                            // value the frame after the button came up.
                            if let Some(pop) = &mut d.model.popup {
                                if let Some(it) = pop.items.iter_mut().find(|i| i.slider.is_some()) {
                                    it.slider = Some(t);
                                }
                            }
                        }
                    }
                }
                text(
                    &clip,
                    Pos2::new(ir.left() + 12.0, ir.center().y),
                    Align2::LEFT_CENTER,
                    &item.label,
                    11.5,
                    s.secondary,
                );
                p.rect_filled(track, Rounding::same(1.5), s.hairline);
                let kx = track.left() + track.width() * v;
                p.rect_filled(
                    Rect::from_min_max(track.left_top(), Pos2::new(kx, track.bottom())),
                    Rounding::same(1.5),
                    s.accent,
                );
                p.circle_filled(Pos2::new(kx, ir.center().y), 4.5, s.accent);
                cy += item_h;
                continue;
            }
            let enabled = !item.disabled && item.cmd.is_some();
            let resp = ui.allocate_rect(
                ir,
                if enabled {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            );
            if resp.hovered() && enabled {
                ui.painter().rect_filled(ir, Rounding::same(5.0), s.hover);
            }
            let col = if item.disabled {
                s.tertiary
            } else if item.danger {
                s.danger
            } else {
                s.text
            };
            // Every row keeps the same indent, so labels line up whether or not
            // the row has a glyph.
            let icon_col = if item.disabled {
                s.tertiary.gamma_multiply(0.7)
            } else if item.danger {
                s.danger
            } else {
                s.secondary
            };
            menu_icon(
                &clip,
                Rect::from_center_size(
                    Pos2::new(ir.left() + 17.0, ir.center().y),
                    Vec2::splat(13.0),
                ),
                item.icon,
                icon_col,
            );
            elide(
                &clip,
                &item.label,
                Pos2::new(ir.left() + 30.0, ir.center().y),
                ir.width() - 30.0 - 22.0,
                11.5,
                col,
            );
            if item.checked {
                // Drawn, not "✓": that codepoint has no glyph in Microsoft
                // YaHei and rendered as a hollow box.
                menu_icon(
                    &clip,
                    Rect::from_center_size(
                        Pos2::new(ir.right() - 15.0, ir.center().y),
                        Vec2::splat(12.0),
                    ),
                    MenuIcon::Check,
                    s.accent,
                );
            }
            if resp.clicked() {
                click = item.cmd.clone();
            }
        }
        cy += item_h;
    }
    drop(clip);
    if let Some(cmd) = click {
        d.model.close_popup();
        d.push(cmd);
    }
    if d.cursor.map(|c| r.contains(c)).unwrap_or(false) {
        let delta = ui.input(|i| i.smooth_scroll_delta.y);
        if delta.abs() > 0.01 {
            if let Some(pop) = &mut d.model.popup {
                pop.scroll =
                    (pop.scroll - delta).clamp(0.0, (cy + pop.scroll - r.bottom()).max(0.0));
            }
        }
    }
}

fn draw_dialog(d: &mut Draw<'_>, ui: &mut egui::Ui, panel: Rect) {
    let Some(dialog) = d.model.dialog.clone() else {
        return;
    };
    let s = *d.skin;
    let (w, h) = match &dialog {
        Dialog::About => (360.0, 304.0),
        Dialog::ConfirmDelete { .. } => (320.0, 168.0),
        Dialog::Rename { .. } => (320.0, 176.0),
        Dialog::NewFolder { .. } => (320.0, 182.0),
        Dialog::NewCategory { .. } => (320.0, 214.0),
        Dialog::ConfirmCollectDesktop { .. } => (360.0, 196.0),
    };
    let r = Rect::from_center_size(panel.center(), Vec2::new(w, h)).intersect(panel.shrink(8.0));
    ui.painter()
        .rect_filled(panel, Rounding::same(14.0), Color32::from_black_alpha(60));
    ui.painter().rect_filled(r, Rounding::same(12.0), s.popup);
    ui.painter()
        .rect_stroke(r, Rounding::same(12.0), Stroke::new(1.0_f32, s.hairline));

    let mut confirm: Option<Cmd> = None;
    match &dialog {
        Dialog::ConfirmDelete { paths } => {
            text(
                ui.painter(),
                Pos2::new(r.left() + 20.0, r.top() + 30.0),
                Align2::LEFT_CENTER,
                "移到回收站？",
                15.0,
                s.text,
            );
            text(
                ui.painter(),
                Pos2::new(r.left() + 20.0, r.top() + 62.0),
                Align2::LEFT_CENTER,
                &format!("将 {} 项移入回收站，可在回收站还原。", paths.len()),
                11.5,
                s.secondary,
            );
            text(
                ui.painter(),
                Pos2::new(r.left() + 20.0, r.top() + 84.0),
                Align2::LEFT_CENTER,
                "清空回收站后将无法恢复。",
                11.0,
                s.tertiary,
            );
            let (ok, cancel) = dialog_buttons(r, ui, &s, "删除", true);
            if ok {
                confirm = Some(Cmd::DeleteNow(paths.clone()));
            }
            if cancel {
                d.model.dialog = None;
            }
        }
        Dialog::ConfirmCollectDesktop { paths } => {
            let lib_name = d
                .model
                .library_folder()
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "收纳库".to_string());
            text(
                ui.painter(),
                Pos2::new(r.left() + 20.0, r.top() + 30.0),
                Align2::LEFT_CENTER,
                "收纳桌面文件？",
                15.0,
                s.text,
            );
            text(
                ui.painter(),
                Pos2::new(r.left() + 20.0, r.top() + 64.0),
                Align2::LEFT_CENTER,
                &format!("将桌面上的 {} 项移入收纳库「{lib_name}」。", paths.len()),
                11.5,
                s.secondary,
            );
            text(
                ui.painter(),
                Pos2::new(r.left() + 20.0, r.top() + 88.0),
                Align2::LEFT_CENTER,
                "桌面图标会消失，文件不会被删除；",
                11.0,
                s.secondary,
            );
            text(
                ui.painter(),
                Pos2::new(r.left() + 20.0, r.top() + 108.0),
                Align2::LEFT_CENTER,
                "之后可在盒子里把它们拖回桌面。",
                11.0,
                s.tertiary,
            );
            let (ok, cancel) = dialog_buttons(r, ui, &s, "收纳", false);
            if ok {
                confirm = Some(Cmd::CollectDesktop(paths.clone()));
            }
            if cancel {
                d.model.dialog = None;
            }
        }
        Dialog::Rename { path, .. } => {
            text(
                ui.painter(),
                Pos2::new(r.left() + 20.0, r.top() + 28.0),
                Align2::LEFT_CENTER,
                "重命名",
                15.0,
                s.text,
            );
            let edit_r = Rect::from_min_max(
                Pos2::new(r.left() + 20.0, r.top() + 48.0),
                Pos2::new(r.right() - 20.0, r.top() + 76.0),
            );
            ui.painter()
                .rect_filled(edit_r, Rounding::same(6.0), s.surface);
            let mut buf = match &d.model.dialog {
                Some(Dialog::Rename { text, .. }) => text.clone(),
                _ => String::new(),
            };
            let resp = ui.put(
                edit_r.shrink2(Vec2::new(8.0, 2.0)),
                egui::TextEdit::singleline(&mut buf)
                    .font(font(12.5))
                    .frame(false)
                    .text_color(s.text),
            );
            resp.request_focus();
            if let Some(Dialog::Rename { text, .. }) = &mut d.model.dialog {
                *text = buf.clone();
            }
            let (ok, cancel) = dialog_buttons(r, ui, &s, "确定", false);
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ok || enter) && !buf.trim().is_empty() {
                confirm = Some(Cmd::RenameApply(path.clone(), buf));
            }
            if cancel {
                d.model.dialog = None;
            }
        }
        Dialog::NewFolder { parent, .. } => {
            text(
                ui.painter(),
                Pos2::new(r.left() + 20.0, r.top() + 28.0),
                Align2::LEFT_CENTER,
                "新建文件夹",
                15.0,
                s.text,
            );
            let edit_r = Rect::from_min_max(
                Pos2::new(r.left() + 20.0, r.top() + 48.0),
                Pos2::new(r.right() - 20.0, r.top() + 76.0),
            );
            ui.painter()
                .rect_filled(edit_r, Rounding::same(6.0), s.surface);
            let mut buf = match &d.model.dialog {
                Some(Dialog::NewFolder { text, .. }) => text.clone(),
                _ => String::new(),
            };
            let resp = ui.put(
                edit_r.shrink2(Vec2::new(8.0, 2.0)),
                egui::TextEdit::singleline(&mut buf)
                    .font(font(12.5))
                    .frame(false)
                    .text_color(s.text),
            );
            resp.request_focus();
            if let Some(Dialog::NewFolder { text, .. }) = &mut d.model.dialog {
                *text = buf.clone();
            }
            let where_text = if dcx_organizer::same_path(parent, d.model.org.desktop()) {
                "将在桌面创建".to_string()
            } else {
                parent
                    .file_name()
                    .map(|n| format!("将在「{}」中创建", n.to_string_lossy()))
                    .unwrap_or_else(|| "将在桌面创建".to_string())
            };
            text(
                ui.painter(),
                Pos2::new(r.left() + 20.0, r.top() + 100.0),
                Align2::LEFT_CENTER,
                &where_text,
                10.5,
                s.tertiary,
            );
            text(
                ui.painter(),
                Pos2::new(r.left() + 20.0, r.top() + 119.0),
                Align2::LEFT_CENTER,
                "同名时自动编号，不会覆盖已有文件夹。",
                10.5,
                s.tertiary,
            );
            let (ok, cancel) = dialog_buttons(r, ui, &s, "创建", false);
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ok || enter) && !buf.trim().is_empty() {
                confirm = Some(Cmd::NewFolderApply(parent.clone(), buf));
            }
            if cancel {
                d.model.dialog = None;
            }
        }
        Dialog::NewCategory { .. } => {
            text(
                ui.painter(),
                Pos2::new(r.left() + 20.0, r.top() + 26.0),
                Align2::LEFT_CENTER,
                "新建分类",
                15.0,
                s.text,
            );
            let (label_r, exts_r) = (
                Rect::from_min_max(
                    Pos2::new(r.left() + 20.0, r.top() + 44.0),
                    Pos2::new(r.right() - 20.0, r.top() + 72.0),
                ),
                Rect::from_min_max(
                    Pos2::new(r.left() + 20.0, r.top() + 82.0),
                    Pos2::new(r.right() - 20.0, r.top() + 110.0),
                ),
            );
            let mut label = match &d.model.dialog {
                Some(Dialog::NewCategory { label, .. }) => label.clone(),
                _ => String::new(),
            };
            let mut exts = match &d.model.dialog {
                Some(Dialog::NewCategory { exts, .. }) => exts.clone(),
                _ => String::new(),
            };
            ui.painter()
                .rect_filled(label_r, Rounding::same(6.0), s.surface);
            ui.painter()
                .rect_filled(exts_r, Rounding::same(6.0), s.surface);
            let label_resp = ui.put(
                label_r.shrink2(Vec2::new(8.0, 2.0)),
                egui::TextEdit::singleline(&mut label)
                    .hint_text("分类名称，如「设计稿」")
                    .font(font(12.0))
                    .frame(false)
                    .text_color(s.text),
            );
            let exts_resp = ui.put(
                exts_r.shrink2(Vec2::new(8.0, 2.0)),
                egui::TextEdit::singleline(&mut exts)
                    .hint_text("扩展名，空格分隔，如 psd ai sketch")
                    .font(font(12.0))
                    .frame(false)
                    .text_color(s.text),
            );
            // Focus the name field when the dialog opens, but never steal it
            // back from the extension field the user just clicked.
            if !exts_resp.has_focus() {
                label_resp.request_focus();
            }
            if let Some(Dialog::NewCategory { label: l, exts: e }) = &mut d.model.dialog {
                *l = label.clone();
                *e = exts.clone();
            }
            text(
                ui.painter(),
                Pos2::new(r.left() + 20.0, r.top() + 128.0),
                Align2::LEFT_CENTER,
                "只改分类记录，不会移动任何文件。",
                10.5,
                s.tertiary,
            );
            let (ok, cancel) = dialog_buttons(r, ui, &s, "创建", false);
            // Enter works in the other two text dialogs; this one silently
            // ignored it, which read as "the keyboard is broken here".
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ok || enter) && !label.trim().is_empty() {
                confirm = Some(Cmd::NewCategory(label, exts));
            }
            if cancel {
                d.model.dialog = None;
            }
        }
        Dialog::About => {
            // Brand mark, same glyph as the header, so the dialog reads as part
            // of the same product instead of a stray text box.
            let logo = Rect::from_min_size(Pos2::new(r.left() + 20.0, r.top() + 22.0), Vec2::splat(36.0));
            ui.painter().rect_filled(logo, Rounding::same(10.0), s.accent);
            ui.painter().rect_stroke(
                logo.shrink(9.0),
                Rounding::same(3.0),
                Stroke::new(1.6_f32, Color32::WHITE),
            );
            ui.painter().hline(
                logo.left() + 9.0..=logo.right() - 9.0,
                logo.center().y,
                Stroke::new(1.6_f32, Color32::WHITE),
            );
            text(
                ui.painter(),
                Pos2::new(logo.right() + 14.0, r.top() + 33.0),
                Align2::LEFT_CENTER,
                "桌面收纳盒",
                16.0,
                s.text,
            );
            text(
                ui.painter(),
                Pos2::new(logo.right() + 14.0, r.top() + 54.0),
                Align2::LEFT_CENTER,
                &format!(
                    "v{} · 文件收纳到独立库目录，拖出即放回桌面",
                    env!("CARGO_PKG_VERSION")
                ),
                10.5,
                s.tertiary,
            );
            ui.painter().hline(
                r.left() + 20.0..=r.right() - 20.0,
                r.top() + 78.0,
                Stroke::new(1.0_f32, s.divider),
            );

            // What is actually in the box right now — more useful than a slogan.
            let items = d.model.snap.items.len();
            let cats = d.model.cfg.categories.len();
            let bytes: u64 = d
                .model
                .snap
                .items
                .iter()
                .filter(|e| !e.is_dir)
                .map(|e| e.size)
                .sum();
            text(
                ui.painter(),
                Pos2::new(r.left() + 20.0, r.top() + 98.0),
                Align2::LEFT_CENTER,
                &format!("已收纳 {items} 项 · {cats} 个分类 · {}", fmt_size(bytes)),
                11.5,
                s.secondary,
            );

            let rows = [
                ("桌面 → 分类标签", "收纳进库；文件从桌面移走"),
                ("库内拖到标签", "只改分类，文件不动"),
                ("拖出面板", "放回原生桌面"),
                ("拖入文件", "收纳到库，可拖到标签指定分类"),
                ("Ctrl+点击 / Ctrl+F", "多选 / 搜索"),
            ];
            for (i, (k, v)) in rows.iter().enumerate() {
                let y = r.top() + 126.0 + i as f32 * 24.0;
                text(
                    ui.painter(),
                    Pos2::new(r.left() + 20.0, y),
                    Align2::LEFT_CENTER,
                    k,
                    11.0,
                    s.text,
                );
                text(
                    ui.painter(),
                    Pos2::new(r.left() + 132.0, y),
                    Align2::LEFT_CENTER,
                    v,
                    11.0,
                    s.tertiary,
                );
            }

            // One button only: the shared helper always draws a cancel next to
            // it, and "取消" on an about box is noise.
            let ok = Rect::from_min_size(
                Pos2::new(r.right() - 92.0 - 16.0, r.bottom() - 44.0),
                Vec2::new(92.0, 28.0),
            );
            let hot = ui
                .allocate_rect(ok, Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .hovered();
            ui.painter().rect_filled(
                ok,
                Rounding::same(6.0),
                if hot { s.accent.gamma_multiply(0.9) } else { s.accent },
            );
            text(
                ui.painter(),
                ok.center(),
                Align2::CENTER_CENTER,
                "知道了",
                12.0,
                Color32::WHITE,
            );
            if ui
                .interact(ok, egui::Id::new("dcx-about-ok"), Sense::click())
                .clicked()
            {
                d.model.dialog = None;
            }
        }
    }
    if let Some(cmd) = confirm {
        d.model.dialog = None;
        d.push(cmd);
    }
}

fn dialog_buttons(
    r: Rect,
    ui: &mut egui::Ui,
    s: &Skin,
    ok_label: &str,
    danger: bool,
) -> (bool, bool) {
    let ok = Rect::from_min_size(
        Pos2::new(r.right() - 92.0 - 12.0, r.bottom() - 42.0),
        Vec2::new(92.0, 28.0),
    );
    let cancel = Rect::from_min_size(Pos2::new(ok.left() - 78.0, ok.top()), Vec2::new(72.0, 28.0));
    ui.painter()
        .rect_filled(cancel, Rounding::same(6.0), s.surface);
    ui.painter().rect_stroke(
        cancel,
        Rounding::same(6.0),
        Stroke::new(1.0_f32, s.hairline),
    );
    ui.painter().rect_filled(
        ok,
        Rounding::same(6.0),
        if danger { s.danger } else { s.accent },
    );
    text(
        ui.painter(),
        cancel.center(),
        Align2::CENTER_CENTER,
        "取消",
        12.0,
        s.secondary,
    );
    text(
        ui.painter(),
        ok.center(),
        Align2::CENTER_CENTER,
        ok_label,
        12.0,
        Color32::WHITE,
    );
    (
        ui.allocate_rect(ok, Sense::click()).clicked(),
        ui.allocate_rect(cancel, Sense::click()).clicked(),
    )
}

fn draw_toast(d: &mut Draw<'_>, ui: &mut egui::Ui, panel: Rect) {
    if !d.model.toast_alive() {
        return;
    }
    let Some((text_value, at)) = d.model.toast.clone() else {
        return;
    };
    let alpha = ((2600.0 - at.elapsed().as_millis() as f32) / 380.0).clamp(0.0, 1.0);
    let g = ui.fonts(|f| f.layout_no_wrap(text_value.clone(), font(12.0), Color32::WHITE));
    let w = (g.size().x + 28.0).min(panel.width() - 24.0);
    let r = Rect::from_min_size(
        Pos2::new(panel.center().x - w / 2.0, panel.bottom() - FOOTER_H - 42.0),
        Vec2::new(w, 30.0),
    );
    ui.painter().rect_filled(
        r,
        Rounding::same(15.0),
        Color32::from_rgba_unmultiplied(0x1C, 0x1E, 0x20, (228.0 * alpha) as u8),
    );
    elide(
        ui.painter(),
        &text_value,
        Pos2::new(r.center().x - (w - 20.0) / 2.0, r.center().y),
        w - 20.0,
        12.0,
        Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, (255.0 * alpha) as u8),
    );
}

// ---------------------------------------------------------------------------
// Drag ghost (separate, click-through viewport)
// ---------------------------------------------------------------------------

pub fn draw_ghost_pill(
    ui: &mut egui::Ui,
    s: Skin,
    center: Pos2,
    name: &str,
    icon: Option<TextureHandle>,
) {
    let mut job = egui::text::LayoutJob::single_section(
        name.to_string(),
        egui::TextFormat {
            font_id: font(12.0),
            color: s.text,
            ..Default::default()
        },
    );
    job.wrap.max_width = 220.0;
    job.wrap.max_rows = 1;
    job.wrap.overflow_character = Some('…');
    let g = ui.fonts(|f| f.layout_job(job));
    let w = (20.0 + 7.0 + g.size().x + 22.0).min(300.0);
    let pill = Rect::from_center_size(center, Vec2::new(w, 32.0));
    let p = ui.painter().clone();
    p.rect_filled(
        pill.translate(Vec2::new(0.0, 2.0)).expand(1.0),
        Rounding::same(16.0),
        s.shadow,
    );
    p.rect_filled(pill, Rounding::same(16.0), s.popup);
    p.rect_stroke(pill, Rounding::same(16.0), Stroke::new(1.0_f32, s.accent));
    let ir = Rect::from_center_size(
        Pos2::new(pill.left() + 16.0, pill.center().y),
        Vec2::splat(20.0),
    );
    match icon {
        Some(t) => {
            p.image(
                t.id(),
                ir,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => {
            p.rect_filled(ir, Rounding::same(5.0), s.accent_soft);
            text(&p, ir.center(), Align2::CENTER_CENTER, "•", 12.0, s.accent);
        }
    }
    p.galley(
        Pos2::new(ir.right() + 7.0, pill.center().y - g.size().y / 2.0),
        g,
        s.text,
    );
}
