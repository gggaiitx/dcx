//! Search window layout, skin and interactions.
//!
//! Top-to-bottom: search row (scope picker + keyword + mode + magnifier),
//! category tiles, a four-column result table (name / path / size / modified)
//! with draggable column edges, and a status bar. There is deliberately no
//! tab strip: the app has exactly one page.
//!
//! `ScrollArea::show_rows` is used for the result table: it is a real virtual
//! list and avoids per-frame allocation for 2000 rows.

use crate::model::{categories, Hit, Mode, ScopeSel};
use egui::{
    Align2, Color32, CursorIcon, FontId, Id, Margin, Pos2, Rect, RichText, Rounding, Sense, Stroke,
    Vec2,
};
use std::cmp::Ordering;

#[derive(Clone, Copy)]
pub struct Skin {
    pub dark: bool,
    pub bg: Color32,
    pub surface: Color32,
    pub popup: Color32,
    pub hairline: Color32,
    pub text: Color32,
    pub secondary: Color32,
    pub tertiary: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub hover: Color32,
    pub divider: Color32,
    pub danger: Color32,
    pub ok: Color32,
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
            bg: Color32::WHITE,
            surface: Color32::from_rgb(0xF5, 0xF6, 0xF8),
            popup: Color32::WHITE,
            hairline: Color32::from_rgba_unmultiplied(0, 0, 0, 0x16),
            text: Color32::from_rgb(0x1B, 0x1C, 0x1E),
            secondary: Color32::from_rgb(0x5B, 0x5F, 0x66),
            tertiary: Color32::from_rgb(0x7C, 0x82, 0x8B),
            accent: Color32::from_rgb(0x0E, 0x7C, 0x7E),
            accent_soft: Color32::from_rgba_unmultiplied(0x0E, 0x7C, 0x7E, 0x18),
            hover: Color32::from_rgba_unmultiplied(0, 0, 0, 0x08),
            divider: Color32::from_rgba_unmultiplied(0, 0, 0, 0x0D),
            danger: Color32::from_rgb(0xC4, 0x2B, 0x1C),
            ok: Color32::from_rgb(0x1F, 0x8A, 0x55),
        }
    }

    fn dark() -> Self {
        Skin {
            dark: true,
            bg: Color32::from_rgb(0x1E, 0x1F, 0x22),
            surface: Color32::from_rgb(0x2A, 0x2C, 0x30),
            popup: Color32::from_rgb(0x2C, 0x2E, 0x33),
            hairline: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x16),
            text: Color32::from_rgb(0xF2, 0xF3, 0xF5),
            secondary: Color32::from_rgb(0xB8, 0xBC, 0xC2),
            tertiary: Color32::from_rgb(0x9A, 0xA0, 0xA8),
            accent: Color32::from_rgb(0x4F, 0xD8, 0xCC),
            accent_soft: Color32::from_rgba_unmultiplied(0x4F, 0xD8, 0xCC, 0x26),
            hover: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x0C),
            divider: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x12),
            danger: Color32::from_rgb(0xF1, 0x70, 0x63),
            ok: Color32::from_rgb(0x63, 0xD0, 0x93),
        }
    }
}

pub struct ViewState {
    pub text: String,
    pub mode: Mode,
    pub scope: ScopeSel,
    pub scope_items: Vec<(ScopeSel, String)>,
    pub category: String,
    pub show_scope: bool,
    pub scope_anchor: Pos2,
    pub hits: Vec<Hit>,
    /// Distinct files behind `hits`. Content mode reports several lines from
    /// the same file; "共匹配 N 个文件" must not count those as N files.
    pub file_paths: std::collections::HashSet<String>,
    pub selected: Option<usize>,
    pub busy: bool,
    pub partial: bool,
    pub indexing: bool,
    pub index_entries: usize,
    pub scanned: u64,
    pub elapsed_ms: u64,
    pub status_note: Option<String>,
    /// Set by `Ctrl+F` in `app.rs`; consumed by the search box on its next frame.
    pub focus_request: bool,
    /// Keyboard navigation moved the selection; scroll it into view once.
    pub scroll_to_selected: bool,
    // Table columns. The modified-time column takes the remaining width.
    pub col_name_w: f32,
    pub col_path_w: f32,
    pub col_size_w: f32,
    /// None = the query's score order.
    pub sort_col: Option<SortCol>,
    pub sort_desc: bool,
    /// Shell icons, keyed by extension (or full path for per-file artwork).
    icon_cache: crate::file_icon::IconCache,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortCol {
    Name,
    Path,
    Size,
    Mtime,
}

impl SortCol {
    pub fn key(self) -> &'static str {
        match self {
            SortCol::Name => "name",
            SortCol::Path => "path",
            SortCol::Size => "size",
            SortCol::Mtime => "mtime",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "name" => Some(SortCol::Name),
            "path" => Some(SortCol::Path),
            "size" => Some(SortCol::Size),
            "mtime" => Some(SortCol::Mtime),
            _ => None,
        }
    }
}

impl ViewState {
    pub fn new(
        scope_items: Vec<(ScopeSel, String)>,
        mode: Mode,
        scope: ScopeSel,
        cols: [f32; 3],
        sort: (Option<SortCol>, bool),
    ) -> Self {
        ViewState {
            text: String::new(),
            mode,
            scope,
            scope_items,
            category: "all".into(),
            show_scope: false,
            scope_anchor: Pos2::ZERO,
            hits: Vec::new(),
            file_paths: std::collections::HashSet::new(),
            selected: None,
            busy: false,
            partial: false,
            indexing: true,
            index_entries: 0,
            scanned: 0,
            elapsed_ms: 0,
            status_note: None,
            focus_request: false,
            scroll_to_selected: false,
            col_name_w: cols[0],
            col_path_w: cols[1],
            col_size_w: cols[2],
            sort_col: sort.0,
            sort_desc: sort.1,
            icon_cache: crate::file_icon::IconCache::new(),
        }
    }

    fn icon_for(&mut self, ctx: &egui::Context, path: &std::path::Path) -> Option<egui::TextureHandle> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        self.icon_cache.get(ctx, path, &ext, false)
    }

    /// Re-order rows for the active column. Streaming batches call this again,
    /// so the visible order stays stable while results arrive.
    pub fn sort_hits(&mut self) {
        let Some(col) = self.sort_col else {
            return;
        };
        let desc = self.sort_desc;
        let selected = self
            .selected
            .and_then(|i| self.hits.get(i))
            .map(|h| (h.path.clone(), h.line));
        self.hits.sort_by(|a, b| {
            let primary = match col {
                SortCol::Name => a.name_lower.cmp(&b.name_lower),
                SortCol::Path => a.dir_lower.cmp(&b.dir_lower),
                SortCol::Size => cmp_optional(a.size, b.size, desc),
                SortCol::Mtime => cmp_optional(a.mtime, b.mtime, desc),
            };
            let primary = if desc && matches!(col, SortCol::Name | SortCol::Path) {
                primary.reverse()
            } else {
                primary
            };
            primary
                .then_with(|| a.name_lower.cmp(&b.name_lower))
                .then_with(|| a.dir_lower.cmp(&b.dir_lower))
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.line.cmp(&b.line))
        });
        // Keep the same row selected after the indices move.
        if let Some((path, line)) = selected {
            self.selected = self
                .hits
                .iter()
                .position(|h| h.path == path && h.line == line);
        }
    }

    pub fn signature(&self) -> String {
        let scope = match &self.scope {
            ScopeSel::Desktop => "desktop".to_string(),
            ScopeSel::Computer => "computer".to_string(),
            ScopeSel::Path(p) => format!("path:{p}"),
        };
        format!(
            "{}\u{1}{}\u{1}{}\u{1}{}",
            self.text,
            self.mode.key(),
            scope,
            self.category
        )
    }
}

#[derive(Clone, Debug)]
pub enum Action {
    Open(usize),
    Reveal(usize),
    Copy(usize),
    CopyName(usize),
    CopyDir(usize),
    Cancel,
    ConfigChanged,
    /// The magnifier: query now, even when the input signature is unchanged.
    Submit,
    /// The scope popup just opened: re-enumerate volumes so a USB stick
    /// plugged in after launch shows up without restarting the app.
    RefreshScopes,
    /// "选择文件夹…" in the scope popup.
    PickFolder,
}

const SEARCH_H: f32 = 56.0;
const CAT_H: f32 = 66.0;
const STATUS_H: f32 = 30.0;
const HEADER_H: f32 = 30.0;
const ROW_NAME_H: f32 = 36.0;
const ROW_CONTENT_H: f32 = 50.0;
const SCROLL_GUTTER: f32 = 12.0;

const MIN_NAME_W: f32 = 150.0;
const MIN_PATH_W: f32 = 120.0;
const MIN_SIZE_W: f32 = 72.0;
const MIN_TIME_W: f32 = 110.0;

pub fn draw(ui: &mut egui::Ui, s: &mut ViewState, p: &Skin) -> Vec<Action> {
    let mut actions = Vec::new();
    ui.style_mut().visuals.override_text_color = Some(p.text);

    ui.add_space(10.0);
    search_row(ui, s, p, &mut actions);
    ui.add_space(4.0);
    category_row(ui, s, p, &mut actions);
    ui.add_space(4.0);

    let list_h = (ui.available_height() - STATUS_H).max(80.0);
    let (list_rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), list_h), Sense::hover());
    let content_w = (list_rect.width() - SCROLL_GUTTER).max(220.0);
    let header = Rect::from_min_max(
        list_rect.left_top(),
        egui::pos2(list_rect.right(), list_rect.top() + HEADER_H),
    );
    let body = Rect::from_min_max(
        egui::pos2(list_rect.left(), header.bottom()),
        list_rect.right_bottom(),
    );

    table_header(ui, s, p, header, content_w, &mut actions);
    table_rows(ui, s, p, body, content_w, &mut actions);
    if s.hits.is_empty() {
        empty_state(ui, s, p, body);
    }

    status_bar(ui, s, p);
    actions
}

// ---------------------------------------------------------------------------
// Search row
// ---------------------------------------------------------------------------

fn search_row(ui: &mut egui::Ui, s: &mut ViewState, p: &Skin, actions: &mut Vec<Action>) {
    let (row_rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), SEARCH_H), Sense::hover());
    let inner = Rect::from_min_max(
        egui::pos2(row_rect.left() + 12.0, row_rect.top() + 5.0),
        egui::pos2(row_rect.right() - 12.0, row_rect.bottom() - 7.0),
    );
    let rp = ui.painter_at(row_rect);
    rp.rect(
        inner,
        Rounding::same(10.0),
        p.popup,
        Stroke::new(1.0_f32, p.hairline),
    );

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner.shrink2(Vec2::new(10.0, 5.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 6.0;

    let (scope_rect, scope_resp) = scope_button(&mut child, s, p);
    // A hairline between the scope picker and the keyword box, not a second
    // box: the whole search row has exactly one border.
    rp.vline(
        scope_rect.right() + 4.0,
        inner.center().y - 9.0..=inner.center().y + 9.0,
        Stroke::new(1.0_f32, p.divider),
    );
    if scope_resp.clicked() {
        s.show_scope = !s.show_scope;
        s.scope_anchor = scope_rect.left_bottom() + Vec2::new(0.0, 6.0);
        if s.show_scope {
            actions.push(Action::RefreshScopes);
        }
    }
    if s.show_scope && s.scope_anchor == Pos2::ZERO {
        s.scope_anchor = scope_rect.left_bottom() + Vec2::new(0.0, 6.0);
    }

    let tail_w = 108.0 + 34.0 + 6.0 * 4.0 + 10.0;
    let edit_w = (child.available_width() - tail_w).max(140.0);
    let edit = child.add(
        egui::TextEdit::singleline(&mut s.text)
            .hint_text(RichText::new(s.mode.placeholder()).color(p.tertiary))
            .desired_width(edit_w)
            .frame(false)
            .font(FontId::proportional(14.0)),
    );
    if edit.changed() {
        s.selected = None;
    }
    if s.focus_request {
        edit.request_focus();
        // Ctrl+F means "focus and select all", not just focus: the first
        // keystroke must replace the previous query instead of appending.
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), edit.id) {
            state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(s.text.chars().count()),
            )));
            state.store(ui.ctx(), edit.id);
        }
        s.focus_request = false;
    }

    mode_switch(&mut child, s, p, actions);
    search_button(&mut child, s, p, actions);

    if edit.has_focus() {
        rp.rect_stroke(
            inner,
            Rounding::same(10.0),
            Stroke::new(1.2_f32, p.accent),
        );
    }

    scope_popup(ui, s, p, scope_rect, actions);
}

fn scope_button(ui: &mut egui::Ui, s: &ViewState, p: &Skin) -> (Rect, egui::Response) {
    let label = s.scope.label();
    let font = FontId::proportional(13.0);
    let text_w = ui
        .painter()
        .layout_no_wrap(label.clone(), font.clone(), p.text)
        .size()
        .x;
    let w = (text_w + 54.0).clamp(96.0, 200.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 32.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_filled(rect, Rounding::same(7.0), p.hover);
    }
    let icon = Rect::from_center_size(
        egui::pos2(rect.left() + 15.0, rect.center().y),
        Vec2::splat(17.0),
    );
    scope_icon(painter, icon, ScopeIcon::for_scope(&s.scope), p.secondary);
    let text = elide(painter, &label, &font, rect.width() - 52.0);
    painter.text(
        egui::pos2(rect.left() + 28.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        font,
        p.text,
    );
    chevron(
        painter,
        egui::pos2(rect.right() - 12.0, rect.center().y + 1.0),
        p.secondary,
    );
    (rect, resp)
}

fn mode_switch(ui: &mut egui::Ui, s: &mut ViewState, p: &Skin, actions: &mut Vec<Action>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(108.0, 30.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, Rounding::same(7.0), p.surface);
    let half = rect.width() / 2.0;
    for (i, mode) in [Mode::Name, Mode::Content].into_iter().enumerate() {
        let cell = Rect::from_min_max(
            egui::pos2(rect.left() + i as f32 * half + 2.0, rect.top() + 2.0),
            egui::pos2(rect.left() + (i + 1) as f32 * half - 2.0, rect.bottom() - 2.0),
        );
        let resp = ui
            .interact(cell, ui.id().with(("mode", i)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        let active = s.mode == mode;
        if active {
            ui.painter()
                .rect_filled(cell, Rounding::same(5.0), p.accent_soft);
        } else if resp.hovered() {
            ui.painter()
                .rect_filled(cell, Rounding::same(5.0), p.hover);
        }
        ui.painter().text(
            cell.center(),
            Align2::CENTER_CENTER,
            mode.label(),
            FontId::proportional(12.0),
            if active { p.accent } else { p.secondary },
        );
        if resp.clicked() && !active {
            s.mode = mode;
            s.hits.clear();
            s.file_paths.clear();
            s.selected = None;
            s.partial = false;
            actions.push(Action::ConfigChanged);
        }
    }
}

fn search_button(ui: &mut egui::Ui, s: &ViewState, p: &Skin, actions: &mut Vec<Action>) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(34.0, 30.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, Rounding::same(6.0), p.hover);
    }
    let c = rect.center();
    if s.busy {
        let stop = Rect::from_center_size(c, Vec2::splat(9.0));
        ui.painter().rect_filled(stop, Rounding::same(2.0), p.accent);
    } else {
        let stroke = Stroke::new(1.6_f32, p.accent);
        let lens = egui::pos2(c.x - 1.5, c.y - 1.5);
        ui.painter().circle_stroke(lens, 5.2, stroke);
        ui.painter().line_segment(
            [
                egui::pos2(lens.x + 3.7, lens.y + 3.7),
                egui::pos2(lens.x + 7.2, lens.y + 7.2),
            ],
            stroke,
        );
    }
    if resp.clicked() {
        if s.busy {
            actions.push(Action::Cancel);
        } else {
            actions.push(Action::Submit);
        }
    }
}

fn scope_popup(
    ui: &mut egui::Ui,
    s: &mut ViewState,
    p: &Skin,
    scope_rect: Rect,
    actions: &mut Vec<Action>,
) {
    if !s.show_scope {
        return;
    }
    let items = s.scope_items.clone();
    let width = 286.0;
    let mut picked: Option<ScopeSel> = None;
    let mut pick_folder = false;

    let area = egui::Window::new("dcx_search_scope")
        .id(Id::new("dcx_search_scope_popup"))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .fixed_pos(s.scope_anchor)
        .frame(
            egui::Frame::none()
                .fill(p.popup)
                .stroke(Stroke::new(1.0_f32, p.hairline))
                .rounding(Rounding::same(10.0))
                .shadow(egui::epaint::Shadow {
                    offset: Vec2::new(0.0, 6.0),
                    blur: 18.0,
                    spread: 0.0,
                    color: if p.dark {
                        Color32::from_black_alpha(140)
                    } else {
                        Color32::from_black_alpha(34)
                    },
                })
                .inner_margin(Margin::same(6.0)),
        )
        .show(ui.ctx(), |fui| {
            fui.set_width(width);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, true])
                        .max_height(310.0)
                        .show(fui, |lui| {
                            for (sel, label) in &items {
                                let chosen = *sel == s.scope;
                                let (rect, resp) = lui
                                    .allocate_exact_size(Vec2::new(width, 34.0), Sense::click());
                                let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                                let painter = lui.painter();
                                if chosen || resp.hovered() {
                                    painter.rect_filled(
                                        rect,
                                        Rounding::same(7.0),
                                        if chosen { p.accent_soft } else { p.hover },
                                    );
                                }
                                let icon = Rect::from_center_size(
                                    egui::pos2(rect.left() + 21.0, rect.center().y),
                                    Vec2::splat(17.0),
                                );
                                scope_icon(
                                    painter,
                                    icon,
                                    ScopeIcon::for_scope(sel),
                                    if chosen { p.accent } else { p.secondary },
                                );
                                let font = FontId::proportional(13.0);
                                let text = elide(painter, label, &font, width - 76.0);
                                painter.text(
                                    egui::pos2(rect.left() + 38.0, rect.center().y),
                                    Align2::LEFT_CENTER,
                                    text,
                                    font,
                                    if chosen { p.accent } else { p.text },
                                );
                                if chosen {
                                    check_mark(
                                        painter,
                                        egui::pos2(rect.right() - 19.0, rect.center().y),
                                        p.accent,
                                    );
                                }
                                if resp.clicked() {
                                    picked = Some(sel.clone());
                                }
                            }
                        });

                    let (divider, _) = fui.allocate_exact_size(Vec2::new(width, 9.0), Sense::hover());
                    fui.painter().hline(
                        divider.left() + 6.0..=divider.right() - 6.0,
                        divider.center().y,
                        Stroke::new(1.0_f32, p.divider),
                    );

                    let (rect, resp) =
                        fui.allocate_exact_size(Vec2::new(width, 34.0), Sense::click());
                    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                    if resp.hovered() {
                        fui.painter()
                            .rect_filled(rect, Rounding::same(7.0), p.hover);
                    }
                    let icon = Rect::from_center_size(
                        egui::pos2(rect.left() + 21.0, rect.center().y),
                        Vec2::splat(17.0),
                    );
                    scope_icon(fui.painter(), icon, ScopeIcon::Folder, p.secondary);
                    fui.painter().text(
                        egui::pos2(rect.left() + 38.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        "选择文件夹…",
                        FontId::proportional(13.0),
                        p.secondary,
                    );
                    if resp.clicked() {
                        pick_folder = true;
                    }
        });

    if let Some(sel) = picked {
        s.scope = sel;
        s.show_scope = false;
        s.hits.clear();
        s.file_paths.clear();
        actions.push(Action::ConfigChanged);
    } else if pick_folder {
        s.show_scope = false;
        actions.push(Action::PickFolder);
    } else if ui.input(|i| i.pointer.any_click()) {
        let inside = ui
            .input(|i| i.pointer.interact_pos())
            .map(|pos| {
                area.as_ref()
                    .map(|win| win.response.rect.contains(pos))
                    .unwrap_or(false)
                    || scope_rect.contains(pos)
            })
            .unwrap_or(false);
        if !inside {
            s.show_scope = false;
        }
    }
}

// ---------------------------------------------------------------------------
// Category tiles
// ---------------------------------------------------------------------------

fn category_row(ui: &mut egui::Ui, s: &mut ViewState, p: &Skin, actions: &mut Vec<Action>) {
    let (cat_rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), CAT_H), Sense::hover());
    let cats = categories();
    let n = cats.len();
    let left = cat_rect.left() + 14.0;
    let cw = (cat_rect.width() - 28.0) / n as f32;
    let cp = ui.painter_at(cat_rect);
    for (i, cat) in cats.iter().enumerate() {
        let cell = Rect::from_min_max(
            egui::pos2(left + i as f32 * cw + 2.0, cat_rect.top() + 2.0),
            egui::pos2(left + (i + 1) as f32 * cw - 2.0, cat_rect.bottom() - 4.0),
        );
        let resp = ui
            .interact(cell, ui.id().with(("cat", cat.key)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        if resp.hovered() && s.category != cat.key {
            cp.rect_filled(cell, Rounding::same(9.0), p.hover);
        }
        let active = s.category == cat.key;
        let tile = Rect::from_center_size(
            egui::pos2(cell.center().x, cell.top() + 17.0),
            Vec2::splat(30.0),
        );
        cp.rect_filled(
            tile,
            Rounding::same(8.0),
            if active { p.accent_soft } else { p.surface },
        );
        cat_glyph(
            &cp,
            tile.center(),
            cat.key,
            if active { p.accent } else { p.secondary },
        );
        cp.text(
            egui::pos2(cell.center().x, cell.bottom() - 12.0),
            Align2::CENTER_CENTER,
            cat.label,
            FontId::proportional(11.5),
            if active { p.accent } else { p.secondary },
        );
        if resp.clicked() {
            s.category = cat.key.to_string();
            s.hits.clear();
            s.file_paths.clear();
            s.selected = None;
            actions.push(Action::ConfigChanged);
        }
    }
}

// ---------------------------------------------------------------------------
// Result table
// ---------------------------------------------------------------------------

fn table_columns(s: &ViewState, avail: f32) -> [f32; 4] {
    let mut name = s.col_name_w.max(MIN_NAME_W);
    let mut path = s.col_path_w.max(MIN_PATH_W);
    let mut size = s.col_size_w.max(MIN_SIZE_W);
    let mut time = avail - name - path - size;
    if time < MIN_TIME_W {
        let need = MIN_TIME_W - time;
        let take = (path - MIN_PATH_W).max(0.0).min(need);
        path -= take;
        let need = need - take;
        let take = (name - MIN_NAME_W).max(0.0).min(need);
        name -= take;
        let need = need - take;
        let take = (size - MIN_SIZE_W).max(0.0).min(need);
        size -= take;
        time = (avail - name - path - size).max(0.0);
    }
    if time < 40.0 {
        // Window narrower than the sum of minima: scale everything down so the
        // header and rows still agree instead of overflowing.
        let total = name + path + size + MIN_TIME_W;
        let scale = (avail / total.max(1.0)).clamp(0.0, 1.0);
        name *= scale;
        path *= scale;
        size *= scale;
        time = (avail - name - path - size).max(0.0);
    }
    [name, path, size, time]
}

fn sort_header(
    ui: &mut egui::Ui,
    s: &mut ViewState,
    p: &Skin,
    hit: Rect,
    text_x: f32,
    label: &str,
    col: SortCol,
    font: &FontId,
    actions: &mut Vec<Action>,
) {
    let resp = ui
        .interact(hit, ui.id().with(("sort", col.key())), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let active = s.sort_col == Some(col);
    let color = if active || resp.hovered() {
        p.text
    } else {
        p.tertiary
    };
    let painter = ui.painter();
    let width = painter
        .layout_no_wrap(label.to_owned(), font.clone(), color)
        .size()
        .x;
    painter.text(
        egui::pos2(text_x, hit.center().y),
        Align2::LEFT_CENTER,
        label,
        font.clone(),
        color,
    );
    if active {
        sort_arrow(
            painter,
            egui::pos2(text_x + width + 6.0, hit.center().y),
            s.sort_desc,
            p.accent,
        );
    }
    if resp.clicked() {
        if active {
            s.sort_desc = !s.sort_desc;
        } else {
            s.sort_col = Some(col);
            s.sort_desc = false;
        }
        s.sort_hits();
        actions.push(Action::ConfigChanged);
    }
}

fn sort_arrow(painter: &egui::Painter, c: Pos2, desc: bool, color: Color32) {
    let pts = if desc {
        vec![
            egui::pos2(c.x - 4.0, c.y - 2.0),
            egui::pos2(c.x + 4.0, c.y - 2.0),
            egui::pos2(c.x, c.y + 3.0),
        ]
    } else {
        vec![
            egui::pos2(c.x - 4.0, c.y + 2.0),
            egui::pos2(c.x + 4.0, c.y + 2.0),
            egui::pos2(c.x, c.y - 3.0),
        ]
    };
    painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

fn table_header(
    ui: &mut egui::Ui,
    s: &mut ViewState,
    p: &Skin,
    rect: Rect,
    content_w: f32,
    actions: &mut Vec<Action>,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, Rounding::ZERO, p.surface);
    painter.hline(
        rect.left()..=rect.right(),
        rect.bottom() - 0.5,
        Stroke::new(1.0_f32, p.divider),
    );

    let cols = table_columns(s, content_w);
    let x0 = rect.left();
    let x1 = x0 + cols[0];
    let x2 = x1 + cols[1];
    let x3 = x2 + cols[2];
    let font = FontId::proportional(11.5);

    let name_hit = Rect::from_min_max(
        egui::pos2(x0 + 4.0, rect.top()),
        egui::pos2(x1 - 8.0, rect.bottom()),
    );
    let path_hit = Rect::from_min_max(
        egui::pos2(x1 + 8.0, rect.top()),
        egui::pos2(x2 - 8.0, rect.bottom()),
    );
    let size_hit = Rect::from_min_max(
        egui::pos2(x2 + 8.0, rect.top()),
        egui::pos2(x3 - 8.0, rect.bottom()),
    );
    let time_hit = Rect::from_min_max(
        egui::pos2(x3 + 8.0, rect.top()),
        egui::pos2(rect.right() - 4.0, rect.bottom()),
    );
    sort_header(ui, s, p, name_hit, x0 + 12.0, "名称", SortCol::Name, &font, actions);
    sort_header(ui, s, p, path_hit, x1 + 12.0, "路径", SortCol::Path, &font, actions);
    sort_header(ui, s, p, size_hit, x2 + 10.0, "大小", SortCol::Size, &font, actions);
    sort_header(ui, s, p, time_hit, x3 + 12.0, "修改时间", SortCol::Mtime, &font, actions);

    for (i, bound) in [x1, x2, x3].into_iter().enumerate() {
        let handle = Rect::from_center_size(
            egui::pos2(bound, rect.center().y),
            Vec2::new(9.0, rect.height()),
        );
        let resp = ui
            .interact(handle, ui.id().with(("col_drag", i)), Sense::drag())
            .on_hover_cursor(CursorIcon::ResizeHorizontal);
        if resp.hovered() || resp.dragged() {
            ui.painter().vline(
                bound,
                rect.top()..=rect.bottom(),
                Stroke::new(1.0_f32, p.accent),
            );
        }
        if resp.dragged() {
            let dx = ui.input(|input| input.pointer.delta().x);
            if dx.abs() > 0.01 {
                match i {
                    0 => {
                        let max = (content_w - s.col_path_w - s.col_size_w - MIN_TIME_W)
                            .max(MIN_NAME_W);
                        s.col_name_w = (s.col_name_w + dx).clamp(MIN_NAME_W, max);
                    }
                    1 => {
                        let max = (content_w - s.col_name_w - s.col_size_w - MIN_TIME_W)
                            .max(MIN_PATH_W);
                        s.col_path_w = (s.col_path_w + dx).clamp(MIN_PATH_W, max);
                    }
                    _ => {
                        let max = (content_w - s.col_name_w - s.col_path_w - MIN_TIME_W)
                            .max(MIN_SIZE_W);
                        s.col_size_w = (s.col_size_w + dx).clamp(MIN_SIZE_W, max);
                    }
                }
                actions.push(Action::ConfigChanged);
            }
        }
    }
}

fn table_rows(
    ui: &mut egui::Ui,
    s: &mut ViewState,
    p: &Skin,
    rect: Rect,
    content_w: f32,
    actions: &mut Vec<Action>,
) {
    let row_h = if s.mode == Mode::Content {
        ROW_CONTENT_H
    } else {
        ROW_NAME_H
    };
    let cols = table_columns(s, content_w);
    let mut list = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .scroll_bar_visibility(if s.hits.is_empty() {
            egui::scroll_area::ScrollBarVisibility::AlwaysHidden
        } else {
            egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded
        })
        .show_rows(&mut list, row_h, s.hits.len(), |lui, range| {
            for i in range {
                let Some(hit) = s.hits.get(i).cloned() else {
                    continue;
                };
                let (row, resp) = lui.allocate_exact_size(
                    Vec2::new(lui.available_width(), row_h),
                    Sense::click(),
                );
                let selected = s.selected == Some(i);
                let painter = lui.painter_at(row);
                if selected || resp.hovered() {
                    painter.rect_filled(
                        row,
                        Rounding::ZERO,
                        if selected { p.accent_soft } else { p.hover },
                    );
                }
                painter.hline(
                    row.left()..=row.right(),
                    row.bottom() - 0.5,
                    Stroke::new(1.0_f32, p.divider),
                );
                if selected {
                    painter.rect_filled(
                        Rect::from_min_size(
                            row.left_top(),
                            Vec2::new(2.5, row.height()),
                        ),
                        Rounding::ZERO,
                        p.accent,
                    );
                }

                let content = hit.kind == crate::model::HitKind::Content;
                // Real shell icon, cached by extension (exe/lnk/ico by path).
                let icon = s.icon_for(lui.ctx(), std::path::Path::new(&hit.path));
                let name = std::path::Path::new(&hit.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| hit.path.clone());
                let path = std::path::Path::new(&hit.path)
                    .parent()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_default();

                // -- name ----------------------------------------------------
                let name_cell = Rect::from_min_max(
                    egui::pos2(row.left() + 10.0, row.top() + 2.0),
                    egui::pos2(row.left() + cols[0] - 4.0, row.bottom() - 2.0),
                );
                let np = painter.with_clip_rect(name_cell);
                let icon_y = if content {
                    name_cell.center().y - 9.0
                } else {
                    name_cell.center().y
                };
                let tile = Rect::from_center_size(
                    egui::pos2(name_cell.left() + 10.0, icon_y),
                    Vec2::splat(20.0),
                );
                if let Some(tex) = icon {
                    np.image(
                        tex.id(),
                        tile,
                        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                } else {
                    let fallback = Rect::from_center_size(tile.center(), Vec2::splat(18.0));
                    np.rect(
                        fallback,
                        Rounding::same(4.0),
                        if content { p.accent_soft } else { p.surface },
                        Stroke::new(1.0_f32, p.hairline),
                    );
                    row_glyph(
                        &np,
                        fallback.center(),
                        content,
                        if content { p.ok } else { p.accent },
                    );
                }
                let name_font = FontId::proportional(13.0);
                let name_text = elide(
                    &np,
                    &name,
                    &name_font,
                    name_cell.width() - 34.0,
                );
                let name_cut = name_text != name;
                np.text(
                    egui::pos2(name_cell.left() + 24.0, icon_y),
                    Align2::LEFT_CENTER,
                    name_text,
                    name_font,
                    p.text,
                );
                if content {
                    let preview = content_preview(&hit);
                    let preview_font = FontId::proportional(11.0);
                    np.text(
                        egui::pos2(name_cell.left() + 24.0, name_cell.center().y + 10.0),
                        Align2::LEFT_CENTER,
                        elide(&np, &preview, &preview_font, name_cell.width() - 30.0),
                        preview_font,
                        p.tertiary,
                    );
                }

                // -- path ----------------------------------------------------
                let path_cell = Rect::from_min_max(
                    egui::pos2(row.left() + cols[0] + 12.0, row.top()),
                    egui::pos2(row.left() + cols[0] + cols[1] - 6.0, row.bottom()),
                );
                let path_font = FontId::proportional(11.5);
                let path_text = elide(&painter, &path, &path_font, path_cell.width());
                let path_cut = path_text != path;
                painter.text(
                    egui::pos2(path_cell.left(), row.center().y),
                    Align2::LEFT_CENTER,
                    path_text,
                    path_font,
                    p.secondary,
                );

                // A truncated cell reveals its full text on hover. Doing this
                // only when elided keeps full names free of noisy tooltips.
                if resp.hovered() {
                    if let Some(pos) = lui.ctx().pointer_hover_pos() {
                        if name_cut && name_cell.contains(pos) {
                            resp.clone().on_hover_text_at_pointer(name.clone());
                        } else if path_cut && path_cell.contains(pos) {
                            resp.clone().on_hover_text_at_pointer(path.clone());
                        }
                    }
                }

                // -- size ----------------------------------------------------
                let size_cell = Rect::from_min_max(
                    egui::pos2(row.left() + cols[0] + cols[1], row.top()),
                    egui::pos2(row.left() + cols[0] + cols[1] + cols[2], row.bottom()),
                );
                let size_font = FontId::proportional(11.5);
                painter.text(
                    egui::pos2(size_cell.right() - 10.0, row.center().y),
                    Align2::RIGHT_CENTER,
                    fmt_size(hit.size),
                    size_font,
                    p.tertiary,
                );

                // -- modified time -------------------------------------------
                let time_cell = Rect::from_min_max(
                    egui::pos2(row.left() + cols[0] + cols[1] + cols[2] + 12.0, row.top()),
                    egui::pos2(row.right() - 6.0, row.bottom()),
                );
                let time_font = FontId::proportional(11.5);
                painter.text(
                    egui::pos2(time_cell.left(), row.center().y),
                    Align2::LEFT_CENTER,
                    fmt_time(hit.mtime),
                    time_font,
                    p.tertiary,
                );

                if resp.clicked() {
                    s.selected = Some(i);
                }
                if selected && s.scroll_to_selected {
                    resp.scroll_to_me(Some(egui::Align::Center));
                }
                if resp.double_clicked() {
                    actions.push(Action::Open(i));
                }
                if let Some(menu) = resp.context_menu(|mui| {
                    if mui.button("打开").clicked() {
                        actions.push(Action::Open(i));
                        mui.close_menu();
                    }
                    if mui.button("打开所在文件夹").clicked() {
                        actions.push(Action::Reveal(i));
                        mui.close_menu();
                    }
                    if mui.button("复制文件名").clicked() {
                        actions.push(Action::CopyName(i));
                        mui.close_menu();
                    }
                    if mui.button("复制完整路径").clicked() {
                        actions.push(Action::Copy(i));
                        mui.close_menu();
                    }
                    if mui.button("复制所在目录").clicked() {
                        actions.push(Action::CopyDir(i));
                        mui.close_menu();
                    }
                }) {
                    let _ = menu;
                }
            }
        });
    s.scroll_to_selected = false;
}

// ---------------------------------------------------------------------------
// Empty state / status bar
// ---------------------------------------------------------------------------

fn empty_state(ui: &egui::Ui, s: &ViewState, p: &Skin, rect: Rect) {
    let (title, sub) = if s.indexing && s.text.trim().is_empty() {
        (
            "正在建立文件索引…".to_string(),
            format!("已扫描 {} 项，索引就绪后结果会自动刷新", s.scanned),
        )
    } else if s.text.trim().is_empty() {
        (
            "输入关键词开始检索".to_string(),
            "支持 * ? 通配符、空格 AND，以及 ext: path: regex: 前缀".to_string(),
        )
    } else if s.busy {
        ("正在检索…".to_string(), "按 Esc 可取消本次检索".to_string())
    } else {
        (
            "没有找到匹配的文件".to_string(),
            "试试更短的关键词、去掉通配符，或切换检索范围与分类".to_string(),
        )
    };
    let center = rect.center();
    let painter = ui.painter_at(rect);
    painter.text(
        egui::pos2(center.x, center.y - 12.0),
        Align2::CENTER_CENTER,
        title,
        FontId::proportional(15.0),
        p.secondary,
    );
    painter.text(
        egui::pos2(center.x, center.y + 14.0),
        Align2::CENTER_CENTER,
        sub,
        FontId::proportional(12.0),
        p.tertiary,
    );
}

fn status_bar(ui: &mut egui::Ui, s: &ViewState, p: &Skin) {
    let (st_rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), STATUS_H), Sense::hover());
    let sp = ui.painter_at(st_rect);
    sp.hline(
        st_rect.left()..=st_rect.right(),
        st_rect.top() + 0.5,
        Stroke::new(1.0_f32, p.divider),
    );
    let count = if s.mode == Mode::Content {
        s.file_paths.len()
    } else {
        s.hits.len()
    };
    let left = format!(
        "[{}] 共匹配 {} 个文件{}",
        s.scope.label(),
        count,
        if s.partial {
            "（结果已截断，请细化关键词）"
        } else {
            ""
        }
    );
    sp.text(
        egui::pos2(st_rect.left() + 14.0, st_rect.center().y),
        Align2::LEFT_CENTER,
        left,
        FontId::proportional(12.0),
        p.secondary,
    );
    let right = if s.indexing {
        format!("正在建立索引… 已扫描 {} 项", s.scanned)
    } else if s.busy {
        "检索中…".to_string()
    } else {
        format!(
            "{} · 索引 {} 项 · 用时 {} ms",
            if s.mode == Mode::Content {
                "内容检索"
            } else {
                "文件名检索"
            },
            s.index_entries,
            s.elapsed_ms
        )
    };
    sp.text(
        egui::pos2(st_rect.right() - 14.0, st_rect.center().y),
        Align2::RIGHT_CENTER,
        right,
        FontId::proportional(11.0),
        p.tertiary,
    );
    if let Some(note) = &s.status_note {
        sp.text(
            egui::pos2(st_rect.center().x, st_rect.center().y),
            Align2::CENTER_CENTER,
            note,
            FontId::proportional(11.0),
            p.danger,
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn content_preview(hit: &Hit) -> String {
    let loc = match (hit.line, hit.col) {
        (Some(l), Some(c)) => format!("第 {l} 行 {c} 列"),
        (Some(l), None) => format!("第 {l} 行"),
        _ => "命中".to_string(),
    };
    // `highlight` is a byte range, so slice through `get` — a UTF-8 boundary
    // split must not panic (panic = abort here).
    let preview = match hit.highlight {
        Some((a, b)) => match (hit.preview.get(..a), hit.preview.get(a..b)) {
            (Some(pre), Some(mid)) => {
                format!("{}〖{}〗{}", pre, mid, hit.preview.get(b..).unwrap_or(""))
            }
            _ => hit.preview.clone(),
        },
        None => hit.preview.clone(),
    };
    format!("{loc} · {preview}")
}

fn elide(painter: &egui::Painter, text: &str, font: &FontId, max_w: f32) -> String {
    if max_w <= 0.0 {
        return String::new();
    }
    let probe = |candidate: &str| {
        painter
            .layout_no_wrap(candidate.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x
    };
    if probe(text) <= max_w {
        return text.to_owned();
    }
    let total = probe(text).max(1.0);
    let chars = text.chars().count();
    let mut take = (((max_w / total) * chars as f32).floor() as usize).max(1);
    take = take.min(chars.saturating_sub(1));
    loop {
        let mut candidate: String = text.chars().take(take).collect();
        candidate.push('…');
        if probe(&candidate) <= max_w || take == 0 {
            return candidate;
        }
        take -= 1;
    }
}

fn cmp_optional(a: Option<u64>, b: Option<u64>, desc: bool) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => {
            if desc {
                y.cmp(&x)
            } else {
                x.cmp(&y)
            }
        }
        // Unknown size/time always sinks to the bottom.
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn fmt_size(bytes: Option<u64>) -> String {
    let Some(bytes) = bytes else {
        return "—".to_string();
    };
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    // The first division already lands on KB: starting the unit index at 0
    // *before* dividing counted that step twice, so 9.6 MB rendered as 9.60 GB.
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0usize;
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn fmt_time(secs: Option<u64>) -> String {
    secs.map(crate::win::format_time)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "—".to_string())
}

fn chevron(painter: &egui::Painter, c: Pos2, color: Color32) {
    let st = Stroke::new(1.3_f32, color);
    painter.line_segment([egui::pos2(c.x - 3.0, c.y - 1.8), egui::pos2(c.x, c.y + 1.4)], st);
    painter.line_segment([egui::pos2(c.x, c.y + 1.4), egui::pos2(c.x + 3.0, c.y - 1.8)], st);
}

fn check_mark(painter: &egui::Painter, c: Pos2, color: Color32) {
    let st = Stroke::new(1.6_f32, color);
    painter.line_segment([egui::pos2(c.x - 4.0, c.y), egui::pos2(c.x - 1.0, c.y + 3.0)], st);
    painter.line_segment([egui::pos2(c.x - 1.0, c.y + 3.0), egui::pos2(c.x + 4.5, c.y - 3.5)], st);
}

#[derive(Clone, Copy)]
enum ScopeIcon {
    Desktop,
    Computer,
    Drive,
    Folder,
}

impl ScopeIcon {
    fn for_scope(scope: &ScopeSel) -> Self {
        match scope {
            ScopeSel::Desktop => ScopeIcon::Desktop,
            ScopeSel::Computer => ScopeIcon::Computer,
            ScopeSel::Path(path) => {
                let trimmed = path.trim_end_matches(['\\', '/']);
                if trimmed.len() == 2 && trimmed.as_bytes()[1] == b':' {
                    ScopeIcon::Drive
                } else {
                    ScopeIcon::Folder
                }
            }
        }
    }
}

fn scope_icon(painter: &egui::Painter, area: Rect, icon: ScopeIcon, color: Color32) {
    let c = area.center();
    let st = Stroke::new(1.2_f32, color);
    match icon {
        ScopeIcon::Desktop => {
            let screen = Rect::from_center_size(
                egui::pos2(c.x, c.y - 1.5),
                Vec2::new(14.0, 9.5),
            );
            painter.rect_stroke(screen, Rounding::same(1.5), st);
            painter.hline(c.x - 4.0..=c.x + 4.0, c.y + 5.5, st);
            painter.vline(c.x, c.y + 3.0..=c.y + 5.5, st);
        }
        ScopeIcon::Computer => {
            let tower = Rect::from_center_size(c, Vec2::new(10.0, 13.0));
            painter.rect_stroke(tower, Rounding::same(1.5), st);
            painter.hline(tower.left() + 2.0..=tower.right() - 2.0, tower.top() + 4.0, st);
            painter.circle_filled(egui::pos2(tower.center().x, tower.bottom() - 2.5), 1.0, color);
        }
        ScopeIcon::Drive => {
            let body = Rect::from_center_size(c, Vec2::new(15.0, 8.0));
            painter.rect_stroke(body, Rounding::same(2.0), st);
            painter.hline(body.left() + 2.5..=body.right() - 2.5, body.center().y, st);
            painter.circle_filled(egui::pos2(body.right() - 3.0, body.center().y), 1.0, color);
        }
        ScopeIcon::Folder => {
            let body = Rect::from_min_max(
                egui::pos2(c.x - 7.0, c.y - 3.5),
                egui::pos2(c.x + 7.0, c.y + 5.5),
            );
            painter.rect_stroke(body, Rounding::same(1.5), st);
            painter.hline(c.x - 7.0..=c.x - 2.0, c.y - 5.0, st);
        }
    }
}

fn row_glyph(painter: &egui::Painter, c: Pos2, content: bool, color: Color32) {
    let st = Stroke::new(1.0_f32, color);
    if content {
        painter.hline(c.x - 4.5..=c.x + 4.5, c.y - 2.2, st);
        painter.hline(c.x - 4.5..=c.x + 1.5, c.y + 1.6, st);
    } else {
        let r = Rect::from_center_size(c, Vec2::new(7.5, 9.5));
        painter.rect_stroke(r, Rounding::same(1.0), st);
        painter.hline(r.left() + 1.5..=r.right() - 1.5, c.y - 1.4, st);
        painter.hline(r.left() + 1.5..=r.right() - 2.5, c.y + 1.6, st);
    }
}

fn cat_glyph(painter: &egui::Painter, c: Pos2, key: &str, color: Color32) {
    let st = Stroke::new(1.25_f32, color);
    match key {
        "image" => {
            let r = Rect::from_center_size(c, Vec2::new(13.0, 10.5));
            painter.rect_stroke(r, Rounding::same(1.5), st);
            painter.circle_filled(egui::pos2(r.left() + 3.6, r.top() + 3.0), 1.2, color);
            painter.line_segment(
                [
                    egui::pos2(r.left() + 1.5, r.bottom() - 1.5),
                    egui::pos2(r.center().x, r.top() + 4.8),
                ],
                st,
            );
            painter.line_segment(
                [
                    egui::pos2(r.center().x, r.top() + 4.8),
                    egui::pos2(r.right() - 1.5, r.bottom() - 1.5),
                ],
                st,
            );
        }
        "archive" => {
            let r = Rect::from_center_size(c, Vec2::new(13.0, 10.5));
            painter.rect_stroke(r, Rounding::same(1.5), st);
            painter.hline(r.left()..=r.right(), r.top() + 3.2, st);
            painter.vline(c.x, r.top() + 5.6..=r.bottom() - 2.0, st);
        }
        "app" => {
            for dx in [-4.0_f32, 1.0] {
                for dy in [-4.5_f32, 0.5] {
                    let r = Rect::from_min_size(
                        egui::pos2(c.x + dx, c.y + dy),
                        Vec2::splat(3.8),
                    );
                    painter.rect_filled(r, Rounding::same(1.0), color);
                }
            }
        }
        "video" => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(c.x - 3.2, c.y - 5.2),
                    egui::pos2(c.x + 5.2, c.y),
                    egui::pos2(c.x - 3.2, c.y + 5.2),
                ],
                color,
                Stroke::NONE,
            ));
        }
        "audio" => {
            painter.vline(c.x + 2.5, c.y - 5.0..=c.y + 2.5, st);
            painter.hline(c.x + 2.5..=c.x + 5.2, c.y - 5.0, st);
            painter.circle_filled(egui::pos2(c.x + 1.0, c.y + 3.5), 2.0, color);
        }
        "other" => {
            for dx in [-4.2_f32, 0.0, 4.2] {
                painter.circle_filled(egui::pos2(c.x + dx, c.y), 1.25, color);
            }
        }
        _ => {
            let r = Rect::from_center_size(c, Vec2::new(10.0, 11.5));
            painter.rect_stroke(r, Rounding::same(1.2), st);
            painter.hline(r.left() + 2.0..=r.right() - 2.0, c.y - 2.2, st);
            painter.hline(r.left() + 2.0..=r.right() - 2.0, c.y + 1.6, st);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fmt_size;

    #[test]
    fn size_units_are_not_shifted_by_one_step() {
        assert_eq!(fmt_size(Some(0)), "0 B");
        assert_eq!(fmt_size(Some(512)), "512 B");
        assert_eq!(fmt_size(Some(1024)), "1.00 KB");
        // 9829 KB is the 9.6 MB file from the bug report, not 9.60 GB.
        assert_eq!(fmt_size(Some(9_829 * 1024)), "9.60 MB");
        assert_eq!(fmt_size(Some(1024_u64.pow(3))), "1.00 GB");
        assert_eq!(fmt_size(Some(9_829 * 1024_u64.pow(2))), "9.60 GB");
        assert_eq!(fmt_size(None), "—");
    }
}
