//! The expanded panel: four tabs sharing one header, one tab bar and one
//! footer.
//!
//! Every widget is placed from explicit rectangles rather than egui's layout,
//! for the same reason the capsule is: the panel is resized by dragging its
//! bottom-right corner, and a layout pass would re-flow every row on each frame
//! of that drag. Rows are painted and hit-tested by the same numbers, so a click
//! can never land on a row the user is not looking at.

use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, Vec2};

use crate::clipboard::{ClipItem, Kind};
use crate::inbox::{ChannelState, Inbox, Source};
use crate::tasks::Task;
use crate::ui::theme::Palette;
use crate::ui::widgets::{self, Glyph};
use crate::ui::Tab;

pub struct TextureCache {
    map: std::collections::HashMap<u64, egui::TextureHandle>,
}

impl TextureCache {
    pub fn new() -> TextureCache {
        TextureCache {
            map: std::collections::HashMap::new(),
        }
    }

    /// Load a thumbnail once. Images are decoded at full size and handed to egui
    /// as `ColorImage`; the list shows at most a dozen, and the cache is capped
    /// so a long session cannot grow without bound.
    fn get(&mut self, ctx: &egui::Context, id: u64, path: &std::path::Path) -> Option<egui::TextureHandle> {
        if let Some(t) = self.map.get(&id) {
            return Some(t.clone());
        }
        let img = image::open(path).ok()?.to_rgba8();
        let size = [img.width() as usize, img.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
        let tex = ctx.load_texture(format!("clip-{id}"), color, Default::default());
        if self.map.len() > 24 {
            self.map.clear();
        }
        self.map.insert(id, tex.clone());
        Some(tex)
    }
}

pub struct PanelState<'a> {
    pub tab: Tab,
    pub tasks: &'a mut crate::tasks::Store,
    pub clip: &'a mut crate::clipboard::Store,
    pub inbox: &'a mut Inbox,
    pub snap: crate::metrics::Snapshot,
    pub metrics_error: Option<String>,
    pub cfg: &'a dcx_island_config::Config,
    pub today: &'a str,
    pub task_input: &'a mut String,
    pub task_time: &'a mut String,
    pub task_repeat: &'a mut crate::tasks::Repeat,
    pub clip_query: &'a mut String,
    pub textures: &'a mut TextureCache,
    pub toast: Option<&'a str>,
    pub dispatch_note: Option<&'a str>,
}

#[derive(Clone, Debug)]
pub enum Action {
    Close,
    Tab(Tab),
    AddTask,
    ToggleDone(u64),
    SetCurrent(u64),
    RemoveTask(u64),
    Archive,
    PinClip(u64),
    CopyClip(u64),
    RemoveClip(u64),
    ClearClips,
    MarkAllRead,
    ClearInbox,
    RemoveInbox(u64),
}

pub fn draw(
    ui: &mut egui::Ui,
    rect: Rect,
    st: &mut PanelState<'_>,
    p: &Palette,
    actions: &mut Vec<Action>,
) {
    let radius = 14.0;
    widgets::shadow(ui, rect, radius, p.shadow);
    widgets::round_rect(
        ui,
        rect,
        radius,
        p.surface,
        Stroke::new(1.0_f32, p.stroke),
    );

    let pad = 14.0;
    let header_h = 38.0;
    let tab_h = 34.0;
    let footer_h = 26.0;

    // -- header ------------------------------------------------------------
    let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), header_h));
    draw_header(ui, header, p, st, actions);

    // -- tab bar -----------------------------------------------------------
    let tabbar = Rect::from_min_size(
        Pos2::new(rect.left(), header.bottom() + 2.0),
        Vec2::new(rect.width(), tab_h),
    );
    draw_tabs(ui, tabbar, p, st, actions);

    // -- footer ------------------------------------------------------------
    let footer = Rect::from_min_size(
        Pos2::new(rect.left(), rect.bottom() - footer_h),
        Vec2::new(rect.width(), footer_h),
    );
    draw_footer(ui, footer, p, st);

    // -- content -----------------------------------------------------------
    let content = Rect::from_min_max(
        Pos2::new(rect.left() + pad, tabbar.bottom() + 6.0),
        Pos2::new(rect.right() - pad, footer.top() - 4.0),
    );
    if content.height() <= 8.0 {
        return;
    }
    match st.tab {
        Tab::Tasks => tasks_tab(ui, content, p, st, actions),
        Tab::Clip => clip_tab(ui, content, p, st, actions),
        Tab::Inbox => inbox_tab(ui, content, p, st, actions),
        Tab::Perf => perf_tab(ui, content, p, st),
    }

    // Resize grip, bottom-right, outside the content rectangle.
    crate::ui::compact::resize_grip(
        ui,
        Rect::from_min_max(
            Pos2::new(rect.right() - 16.0, rect.bottom() - 16.0),
            rect.max,
        ),
        p,
    );
}

fn draw_header(
    ui: &mut egui::Ui,
    rect: Rect,
    p: &Palette,
    st: &PanelState<'_>,
    actions: &mut Vec<Action>,
) {
    let clock = crate::clock::Clock::now();
    let left = rect.left() + 16.0;
    let y = rect.center().y;

    let title = format!("{} {}", clock.date_cn(), clock.weekday_cn());
    widgets::text(ui, Pos2::new(left, y), Align2::LEFT_CENTER, &title, 13.0, p.text);

    let open = st.tasks.for_day(st.today).iter().filter(|t| !t.done).count();
    let detail = format!("{open} 项待办");
    widgets::text(
        ui,
        Pos2::new(left + 118.0, y + 0.5),
        Align2::LEFT_CENTER,
        &detail,
        11.0,
        p.text_faint,
    );

    let btn = Rect::from_center_size(
        Pos2::new(rect.right() - 20.0, y),
        Vec2::splat(22.0),
    );
    if widgets::icon_button(ui, btn, Glyph::Close, p, false).clicked() {
        actions.push(Action::Close);
    }
    let archive = Rect::from_center_size(
        Pos2::new(rect.right() - 46.0, y),
        Vec2::splat(22.0),
    );
    if widgets::icon_button(ui, archive, Glyph::Doc, p, false)
        .on_hover_text("把今天的任务导出为 Markdown")
        .clicked()
    {
        actions.push(Action::Archive);
    }
}

fn draw_tabs(
    ui: &mut egui::Ui,
    rect: Rect,
    p: &Palette,
    st: &PanelState<'_>,
    actions: &mut Vec<Action>,
) {
    let tabs = Tab::all();
    let w = rect.width() / tabs.len() as f32;
    for (i, tab) in tabs.iter().enumerate() {
        let r = Rect::from_min_size(
            Pos2::new(rect.left() + i as f32 * w, rect.top()),
            Vec2::new(w, rect.height()),
        );
        let active = st.tab == *tab;
        let resp = ui.allocate_rect(r, egui::Sense::click());
        if resp.hovered() && !active {
            widgets::round_rect(
                ui,
                r.shrink2(Vec2::new(6.0, 5.0)),
                7.0,
                p.surface_alt,
                Stroke::NONE,
            );
        }
        let color = if active {
            p.accent
        } else if resp.hovered() {
            p.text
        } else {
            p.text_dim
        };
        let mut label = tab.label().to_string();
        if *tab == Tab::Inbox {
            let n = st.inbox.unread();
            if n > 0 {
                label = format!("{} {}", label, n.min(99));
            }
        }
        let galley = ui
            .painter()
            .layout_no_wrap(label, FontId::proportional(12.0), color);
        ui.painter().galley(
            Pos2::new(r.center().x - galley.size().x * 0.5, r.center().y - galley.size().y * 0.5),
            galley,
            color,
        );
        if active {
            let underline = Rect::from_center_size(
                Pos2::new(r.center().x, r.bottom() - 3.0),
                Vec2::new((w * 0.34).min(34.0), 2.0),
            );
            widgets::round_rect(ui, underline, 1.0, p.accent, Stroke::NONE);
        }
        if resp.clicked() {
            actions.push(Action::Tab(*tab));
        }
    }
    // Hairline under the whole bar.
    ui.painter().line_segment(
        [
            Pos2::new(rect.left() + 12.0, rect.bottom()),
            Pos2::new(rect.right() - 12.0, rect.bottom()),
        ],
        Stroke::new(1.0_f32, p.divider),
    );
}

fn draw_footer(ui: &mut egui::Ui, rect: Rect, p: &Palette, st: &PanelState<'_>) {
    let hint = match st.tab {
        Tab::Tasks => "Enter 添加 · 点标题设为当前 · Esc 收起",
        Tab::Clip => "点条目回写剪贴板 · 图钉保留",
        Tab::Inbox => "只显示提醒，不读取聊天内容",
        Tab::Perf => "1 秒采样 · 平滑显示",
    };
    if let Some(toast) = st.toast {
        widgets::text(
            ui,
            Pos2::new(rect.left() + 16.0, rect.center().y),
            Align2::LEFT_CENTER,
            toast,
            10.5,
            p.accent,
        );
    } else {
        widgets::text(
            ui,
            Pos2::new(rect.left() + 16.0, rect.center().y),
            Align2::LEFT_CENTER,
            hint,
            10.5,
            p.text_faint,
        );
    }
    if let Some(note) = st.dispatch_note {
        widgets::text(
            ui,
            Pos2::new(rect.right() - 16.0, rect.center().y),
            Align2::RIGHT_CENTER,
            note,
            10.0,
            p.text_faint,
        );
    }
}

// ---------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------

fn tasks_tab(
    ui: &mut egui::Ui,
    rect: Rect,
    p: &Palette,
    st: &mut PanelState<'_>,
    actions: &mut Vec<Action>,
) {
    // -- composer ----------------------------------------------------------
    let composer_h = 34.0;
    let composer = Rect::from_min_size(rect.min, Vec2::new(rect.width(), composer_h));
    widgets::round_rect(ui, composer, 9.0, p.surface_alt, Stroke::new(1.0_f32, p.divider));

    let add_w = 30.0;
    let time_w = 50.0;
    let repeat_w = 52.0;
    let field = Rect::from_min_max(
        composer.min + Vec2::new(8.0, 4.0),
        Pos2::new(
            composer.right() - (add_w + time_w + repeat_w + 14.0),
            composer.bottom() - 4.0,
        ),
    );
    let resp = ui.put(
        field,
        egui::TextEdit::singleline(st.task_input)
            .hint_text("添加任务，回车确认")
            .frame(false)
            .text_color(p.text)
            .margin(egui::Margin::symmetric(2.0, 4.0)),
    );
    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        actions.push(Action::AddTask);
        resp.request_focus();
    }

    // Reminder time field: accepts `15:30`, empty means no reminder.
    let time_rect = Rect::from_min_size(
        Pos2::new(composer.right() - (add_w + time_w + repeat_w + 8.0), composer.top() + 4.0),
        Vec2::new(time_w, composer.height() - 8.0),
    );
    // Colour is computed before the widget takes its mutable borrow of the
    // text buffer; reading the same String inside the builder chain is a
    // second borrow and does not compile.
    let time_color = if crate::clock::parse_hhmm(st.task_time).is_some()
        || st.task_time.trim().is_empty()
    {
        p.text_dim
    } else {
        p.danger
    };
    ui.put(
        time_rect,
        egui::TextEdit::singleline(st.task_time)
            .hint_text("时间")
            .frame(false)
            .text_color(time_color)
            .margin(egui::Margin::symmetric(2.0, 4.0)),
    );

    let repeat_rect = Rect::from_min_size(
        Pos2::new(time_rect.right() + 2.0, composer.top() + 4.0),
        Vec2::new(repeat_w, composer.height() - 8.0),
    );
    let rresp = ui.allocate_rect(repeat_rect, egui::Sense::click());
    if rresp.hovered() {
        widgets::round_rect(ui, repeat_rect.shrink(2.0), 6.0, p.accent_soft, Stroke::NONE);
    }
    widgets::text(
        ui,
        repeat_rect.center(),
        Align2::CENTER_CENTER,
        st.task_repeat.label(),
        10.0,
        p.text_dim,
    );
    if rresp.clicked() {
        *st.task_repeat = match *st.task_repeat {
            crate::tasks::Repeat::Once => crate::tasks::Repeat::Daily,
            crate::tasks::Repeat::Daily => crate::tasks::Repeat::Workdays,
            crate::tasks::Repeat::Workdays => crate::tasks::Repeat::Weekly,
            crate::tasks::Repeat::Weekly => crate::tasks::Repeat::Once,
        };
    }

    let add_rect = Rect::from_min_size(
        Pos2::new(composer.right() - add_w - 2.0, composer.top() + 3.0),
        Vec2::new(add_w - 2.0, composer.height() - 6.0),
    );
    if widgets::icon_button(ui, add_rect, Glyph::Plus, p, false).clicked() {
        actions.push(Action::AddTask);
    }

    // -- list --------------------------------------------------------------
    let list = Rect::from_min_max(Pos2::new(rect.left(), composer.bottom() + 8.0), rect.max);
    let items: Vec<Task> = st.tasks.for_day(st.today).into_iter().cloned().collect();
    if items.is_empty() {
        empty_state(ui, list, p, "今天还没有任务", "在上方输入内容后回车");
        return;
    }
    let row_h = 38.0;
    let mut y = list.top();
    for task in &items {
        if y + row_h > list.bottom() + 1.0 {
            break;
        }
        let row = Rect::from_min_size(Pos2::new(list.left(), y), Vec2::new(list.width(), row_h));
        draw_task_row(ui, row, p, task, actions);
        y += row_h;
    }
}

fn draw_task_row(ui: &mut egui::Ui, row: Rect, p: &Palette, task: &Task, actions: &mut Vec<Action>) {
    let hovered = ui.rect_contains_pointer(row);

    if task.current {
        widgets::round_rect(
            ui,
            row.shrink2(Vec2::new(0.0, 1.0)),
            8.0,
            p.accent_soft,
            Stroke::NONE,
        );
    } else if hovered {
        widgets::round_rect(
            ui,
            row.shrink2(Vec2::new(0.0, 1.0)),
            8.0,
            with_alpha(p.surface_alt, 0.7),
            Stroke::NONE,
        );
    }

    // Check circle.
    let circle = Rect::from_center_size(
        Pos2::new(row.left() + 16.0, row.center().y),
        Vec2::splat(17.0),
    );
    let cresp = ui.allocate_rect(circle, egui::Sense::click());
    let ring = if task.done { p.ok } else if cresp.hovered() { p.accent } else { p.text_faint };
    ui.painter()
        .circle_stroke(circle.center(), 7.5, Stroke::new(1.4_f32, ring));
    if task.done {
        widgets::glyph(ui, circle.center(), 9.0, Glyph::Check, p.ok);
    }
    if cresp.clicked() {
        actions.push(Action::ToggleDone(task.id));
    }

    // Title (click = make current).
    let title_color = if task.done { p.text_faint } else { p.text };
    let title_right = row.right() - 92.0;
    let title_rect = Rect::from_min_max(
        Pos2::new(circle.right() + 8.0, row.top()),
        Pos2::new(title_right.max(circle.right() + 30.0), row.bottom()),
    );
    let tresp = ui.allocate_rect(title_rect, egui::Sense::click());
    let galley = ui.painter().layout(
        task.title.clone(),
        FontId::proportional(12.5),
        title_color,
        title_rect.width(),
    );
    let ty = title_rect.center().y - galley.size().y * 0.5;
    ui.painter()
        .with_clip_rect(title_rect)
        .galley(Pos2::new(title_rect.left(), ty), galley.clone(), title_color);
    if task.done {
        let strike_y = ty + galley.size().y * 0.55;
        ui.painter().line_segment(
            [
                Pos2::new(title_rect.left(), strike_y),
                Pos2::new(title_rect.left() + galley.size().x.min(title_rect.width()), strike_y),
            ],
            Stroke::new(1.0_f32, p.text_faint),
        );
    }
    if tresp.clicked() {
        actions.push(Action::SetCurrent(task.id));
    }
    if tresp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // Right side: time, repeat mark, delete.
    let mut right = row.right() - 12.0;
    if hovered {
        let del = Rect::from_center_size(Pos2::new(right - 8.0, row.center().y), Vec2::splat(20.0));
        if widgets::icon_button(ui, del, Glyph::Trash, p, true).clicked() {
            actions.push(Action::RemoveTask(task.id));
        }
        right -= 24.0;
    }
    if task.repeat != crate::tasks::Repeat::Once {
        widgets::text(
            ui,
            Pos2::new(right, row.center().y),
            Align2::RIGHT_CENTER,
            task.repeat.label(),
            9.5,
            p.text_faint,
        );
        right -= 30.0;
    }
    if let Some(at) = task.at {
        let text = format!("{:02}:{:02}", at / 3600, (at % 3600) / 60);
        let overdue = !task.done && task.day == crate::clock::Clock::now().iso_date() && {
            let now = crate::clock::Clock::now().secs_of_day();
            now >= at && now <= at + 1800
        };
        widgets::mono_text(
            ui,
            Pos2::new(right, row.center().y),
            Align2::RIGHT_CENTER,
            &text,
            10.5,
            if overdue { p.danger } else { p.text_dim },
        );
    }
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

fn clip_tab(
    ui: &mut egui::Ui,
    rect: Rect,
    p: &Palette,
    st: &mut PanelState<'_>,
    actions: &mut Vec<Action>,
) {
    let search_h = 30.0;
    let search = Rect::from_min_size(rect.min, Vec2::new(rect.width(), search_h));
    widgets::round_rect(ui, search, 8.0, p.surface_alt, Stroke::new(1.0_f32, p.divider));
    let field = search.shrink2(Vec2::new(8.0, 3.0));
    ui.put(
        field,
        egui::TextEdit::singleline(st.clip_query)
            .hint_text("搜索剪贴板历史")
            .frame(false)
            .text_color(p.text)
            .margin(egui::Margin::symmetric(2.0, 3.0)),
    );
    let clear = Rect::from_center_size(
        Pos2::new(search.right() - 14.0, search.center().y),
        Vec2::splat(18.0),
    );
    if widgets::icon_button(ui, clear, Glyph::Trash, p, true)
        .on_hover_text("清空未固定的条目")
        .clicked()
    {
        actions.push(Action::ClearClips);
    }

    let list = Rect::from_min_max(Pos2::new(rect.left(), search.bottom() + 8.0), rect.max);
    let items: Vec<ClipItem> = st
        .clip
        .filtered(st.clip_query)
        .into_iter()
        .cloned()
        .collect();
    if items.is_empty() {
        let (title, sub) = if st.clip_query.trim().is_empty() {
            ("剪贴板是空的", "复制文本或图片后会出现在这里")
        } else {
            ("没有匹配的条目", "换个关键词试试")
        };
        empty_state(ui, list, p, title, sub);
        return;
    }

    let ctx = ui.ctx().clone();
    let mut y = list.top();
    for item in &items {
        let row_h = if item.kind == Kind::Image { 46.0 } else { 40.0 };
        if y + row_h > list.bottom() + 1.0 {
            break;
        }
        let row = Rect::from_min_size(Pos2::new(list.left(), y), Vec2::new(list.width(), row_h));
        let hovered = ui.rect_contains_pointer(row);
        if hovered {
            widgets::round_rect(ui, row.shrink2(Vec2::new(0.0, 1.0)), 8.0, with_alpha(p.surface_alt, 0.7), Stroke::NONE);
        }

        let mut text_left = row.left() + 8.0;
        if item.kind == Kind::Image {
            let thumb = Rect::from_min_size(
                Pos2::new(row.left() + 6.0, row.top() + 5.0),
                Vec2::new(48.0, row_h - 10.0),
            );
            widgets::round_rect(ui, thumb, 5.0, p.track, Stroke::NONE);
            if let Some(name) = &item.image {
                if let Some(tex) = st.textures.get(&ctx, item.id, &st.clip.dir.join(name)) {
                    let size = tex.size_vec2();
                    let scale = (thumb.width() / size.x).min(thumb.height() / size.y);
                    let draw = Rect::from_center_size(thumb.center(), size * scale);
                    ui.painter().image(
                        tex.id(),
                        draw,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            }
            text_left = thumb.right() + 8.0;
        }

        let mut icons_right = row.right() - 8.0;
        if hovered {
            let del = Rect::from_center_size(Pos2::new(icons_right - 8.0, row.center().y), Vec2::splat(20.0));
            if widgets::icon_button(ui, del, Glyph::Trash, p, true).clicked() {
                actions.push(Action::RemoveClip(item.id));
            }
            icons_right -= 22.0;
        }
        let pin = Rect::from_center_size(Pos2::new(icons_right - 8.0, row.center().y), Vec2::splat(20.0));
        if widgets::icon_button(ui, pin, Glyph::Pin, p, false).clicked() {
            actions.push(Action::PinClip(item.id));
        }
        if item.pinned {
            widgets::glyph(ui, pin.center(), 12.0, Glyph::Pin, p.accent);
        }
        icons_right -= 22.0;

        let text_right = icons_right - 4.0;
        let text_rect = Rect::from_min_max(
            Pos2::new(text_left, row.top() + 4.0),
            Pos2::new(text_right.max(text_left + 20.0), row.bottom() - 4.0),
        );
        let title = if item.kind == Kind::Text {
            item.preview()
        } else {
            format!("图片 · {}", item.size_label())
        };
        let galley = ui.painter().layout(
            title,
            FontId::proportional(12.0),
            if item.pinned { p.text } else { p.text_dim },
            text_rect.width(),
        );
        let ty = if item.kind == Kind::Text {
            text_rect.center().y - galley.size().y * 0.5
        } else {
            text_rect.center().y - galley.size().y * 0.5
        };
        ui.painter()
            .with_clip_rect(text_rect)
            .galley(Pos2::new(text_rect.left(), ty), galley, p.text_dim);

        let resp = ui.interact(
            Rect::from_min_max(
                Pos2::new(text_left, row.top()),
                Pos2::new(text_right, row.bottom()),
            ),
            ui.id().with(("clip-row", item.id)),
            egui::Sense::click(),
        );
        if resp.clicked() {
            actions.push(Action::CopyClip(item.id));
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        y += row_h;
    }
}

// ---------------------------------------------------------------------------
// Inbox
// ---------------------------------------------------------------------------

fn inbox_tab(
    ui: &mut egui::Ui,
    rect: Rect,
    p: &Palette,
    st: &mut PanelState<'_>,
    actions: &mut Vec<Action>,
) {
    let card_h = 26.0;
    let channels: Vec<(Source, ChannelState, Option<u64>, &'static str)> = st
        .inbox
        .channels
        .iter()
        .map(|(s, c)| (*s, c.state.clone(), c.last_at, c.note))
        .collect();

    let mut y = rect.top();
    for (source, state, last_at, note) in &channels {
        let row = Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(rect.width(), card_h));
        let (r, g, b) = source.color();
        let dot = Pos2::new(row.left() + 6.0, row.center().y);
        ui.painter().circle_filled(dot, 3.5, Color32::from_rgb(r, g, b));
        widgets::text(
            ui,
            Pos2::new(dot.x + 10.0, row.center().y),
            Align2::LEFT_CENTER,
            source.label(),
            11.0,
            p.text,
        );
        let state_color = match state {
            ChannelState::Ok => p.ok,
            ChannelState::Off => p.text_faint,
            ChannelState::Idle => p.text_dim,
            _ => p.warn,
        };
        let mut label = state.label();
        if state.is_ok() {
            match last_at {
                Some(at) => {
                    label = format!("已就绪 · 上次{}", crate::inbox::short_time(*at, st.today));
                }
                None => label = "已就绪 · 尚未收到信号".to_string(),
            }
        }
        let galley = ui
            .painter()
            .layout_no_wrap(label.clone(), FontId::proportional(10.0), state_color);
        let max_w = row.width() - 150.0;
        let clipped = if galley.size().x > max_w {
            ui.painter()
                .layout(label.clone(), FontId::proportional(10.0), state_color, max_w)
        } else {
            galley
        };
        ui.painter().with_clip_rect(row).galley(
            Pos2::new(row.right() - clipped.size().x - 4.0, row.center().y - clipped.size().y * 0.5),
            clipped,
            state_color,
        );
        if state.is_problem() {
            let _ = ui.interact(row, ui.id().with(("ch", source.key())), egui::Sense::hover())
                .on_hover_text(*note);
        }
        y += card_h;
    }

    y += 4.0;
    ui.painter().line_segment(
        [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
        Stroke::new(1.0_f32, p.divider),
    );
    y += 6.0;

    let toolbar = Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(rect.width(), 22.0));
    let read_all = Rect::from_min_size(
        Pos2::new(toolbar.right() - 140.0, toolbar.top()),
        Vec2::new(64.0, 20.0),
    );
    let clear = Rect::from_min_size(
        Pos2::new(toolbar.right() - 68.0, toolbar.top()),
        Vec2::new(64.0, 20.0),
    );
    if small_button(ui, read_all, p, "全部已读").clicked() {
        actions.push(Action::MarkAllRead);
    }
    if small_button(ui, clear, p, "清空").clicked() {
        actions.push(Action::ClearInbox);
    }
    y += 26.0;

    let items: Vec<_> = st
        .inbox
        .items()
        .map(|i| (i.id, i.source, i.title.clone(), i.detail.clone(), i.at, i.read))
        .collect();
    if items.is_empty() {
        empty_state(
            ui,
            Rect::from_min_max(Pos2::new(rect.left(), y), rect.max),
            p,
            "没有新消息提醒",
            "QQ / 微信 / 企业微信 / 邮箱的提醒会汇总到这里",
        );
        return;
    }
    let row_h = 40.0;
    for (id, source, title, detail, at, read) in items {
        if y + row_h > rect.bottom() + 1.0 {
            break;
        }
        let row = Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(rect.width(), row_h));
        let hovered = ui.rect_contains_pointer(row);
        if hovered {
            widgets::round_rect(ui, row.shrink2(Vec2::new(0.0, 1.0)), 8.0, with_alpha(p.surface_alt, 0.7), Stroke::NONE);
        }
        if !read {
            ui.painter().circle_filled(
                Pos2::new(row.left() + 4.0, row.center().y),
                2.0,
                p.accent,
            );
        }
        let name = source.label();
        widgets::text(
            ui,
            Pos2::new(row.left() + 12.0, row.top() + 12.0),
            Align2::LEFT_TOP,
            name,
            9.5,
            p.text_faint,
        );
        let title_rect = Rect::from_min_max(
            Pos2::new(row.left() + 12.0, row.top() + 20.0),
            Pos2::new(row.right() - 56.0, row.bottom()),
        );
        let galley = ui.painter().layout(
            title.clone(),
            FontId::proportional(12.0),
            if read { p.text_dim } else { p.text },
            title_rect.width(),
        );
        ui.painter().with_clip_rect(title_rect).galley(
            Pos2::new(title_rect.left(), title_rect.top()),
            galley,
            if read { p.text_dim } else { p.text },
        );
        let time = crate::inbox::short_time(at, st.today);
        widgets::text(
            ui,
            Pos2::new(row.right() - 34.0, row.top() + 12.0),
            Align2::RIGHT_TOP,
            &time,
            9.5,
            p.text_faint,
        );
        if hovered {
            let del = Rect::from_center_size(Pos2::new(row.right() - 14.0, row.bottom() - 12.0), Vec2::splat(18.0));
            if widgets::icon_button(ui, del, Glyph::Trash, p, true).clicked() {
                actions.push(Action::RemoveInbox(id));
            }
        }
        if !detail.is_empty() {
            widgets::text(
                ui,
                Pos2::new(row.left() + 12.0, row.bottom() - 6.0),
                Align2::LEFT_BOTTOM,
                &truncate(&detail, 40),
                9.5,
                p.text_faint,
            );
        }
        y += row_h;
    }
}

// ---------------------------------------------------------------------------
// Performance
// ---------------------------------------------------------------------------

fn perf_tab(ui: &mut egui::Ui, rect: Rect, p: &Palette, st: &PanelState<'_>) {
    let snap = st.snap;
    let ring_d = 78.0;
    let gap = 12.0;
    let top = Rect::from_min_size(rect.min, Vec2::new(rect.width(), ring_d));
    let card_w = (top.width() - gap) * 0.5;

    ring_card(
        ui,
        Rect::from_min_size(top.min, Vec2::new(card_w, ring_d)),
        p,
        "CPU",
        snap.cpu,
    );
    ring_card(
        ui,
        Rect::from_min_size(
            Pos2::new(top.left() + card_w + gap, top.top()),
            Vec2::new(card_w, ring_d),
        ),
        p,
        "内存",
        snap.mem_percent,
    );

    let mut y = top.bottom() + 12.0;
    let rows: Vec<(String, String, Option<f32>)> = vec![
        (
            "内存".to_string(),
            format!(
                "{} / {}",
                crate::metrics::human_bytes(snap.mem_used),
                crate::metrics::human_bytes(snap.mem_total)
            ),
            None,
        ),
        (
            "系统盘".to_string(),
            if snap.disk_total > 0 {
                format!(
                    "{} / {}",
                    crate::metrics::human_bytes(snap.disk_used),
                    crate::metrics::human_bytes(snap.disk_total)
                )
            } else {
                "不可用".to_string()
            },
            None,
        ),
        (
            "网络".to_string(),
            format!(
                "↓ {} · ↑ {}",
                crate::metrics::human_rate(snap.net_down_bps),
                crate::metrics::human_rate(snap.net_up_bps)
            ),
            None,
        ),
        (
            "开机".to_string(),
            format!("{} 小时", crate::sys::uptime_secs() / 3600),
            None,
        ),
    ];
    for (label, value, _) in rows {
        if y + 24.0 > rect.bottom() {
            break;
        }
        widgets::text(
            ui,
            Pos2::new(rect.left() + 2.0, y + 12.0),
            Align2::LEFT_CENTER,
            &label,
            11.0,
            p.text_dim,
        );
        widgets::mono_text(
            ui,
            Pos2::new(rect.right() - 2.0, y + 12.0),
            Align2::RIGHT_CENTER,
            &value,
            10.5,
            p.text,
        );
        y += 24.0;
    }

    if let Some(err) = &st.metrics_error {
        widgets::text(
            ui,
            Pos2::new(rect.left() + 2.0, (y + 14.0).min(rect.bottom() - 6.0)),
            Align2::LEFT_CENTER,
            err,
            10.0,
            p.danger,
        );
    }
}

fn ring_card(ui: &mut egui::Ui, rect: Rect, p: &Palette, label: &str, percent: f32) {
    widgets::round_rect(ui, rect, 10.0, p.surface_alt, Stroke::new(1.0_f32, p.divider));
    let center = Pos2::new(rect.center().x, rect.center().y);
    let radius = (rect.height() * 0.5 - 10.0).max(14.0);
    widgets::ring(
        ui,
        center,
        radius,
        5.0,
        percent / 100.0,
        p.level(crate::metrics::level(percent)),
        p.track,
    );
    widgets::mono_text(
        ui,
        center,
        Align2::CENTER_CENTER,
        &format!("{percent:.0}%"),
        13.0,
        p.text,
    );
    widgets::text(
        ui,
        Pos2::new(rect.left() + 10.0, rect.top() + 9.0),
        Align2::LEFT_TOP,
        label,
        9.5,
        p.text_faint,
    );
}

// ---------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------

fn empty_state(ui: &mut egui::Ui, rect: Rect, p: &Palette, title: &str, sub: &str) {
    if rect.height() < 40.0 {
        return;
    }
    widgets::text(
        ui,
        Pos2::new(rect.center().x, rect.center().y - 9.0),
        Align2::CENTER_CENTER,
        title,
        12.0,
        p.text_dim,
    );
    widgets::text(
        ui,
        Pos2::new(rect.center().x, rect.center().y + 11.0),
        Align2::CENTER_CENTER,
        sub,
        10.0,
        p.text_faint,
    );
}

fn small_button(
    ui: &mut egui::Ui,
    rect: Rect,
    p: &Palette,
    label: &str,
) -> egui::Response {
    let resp = ui.allocate_rect(rect, egui::Sense::click());
    let fill = if resp.hovered() { p.accent_soft } else { Color32::TRANSPARENT };
    widgets::round_rect(ui, rect, 6.0, fill, Stroke::NONE);
    widgets::text(
        ui,
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        10.0,
        if resp.hovered() { p.accent } else { p.text_dim },
    );
    resp
}

fn with_alpha(color: Color32, factor: f32) -> Color32 {
    let a = (color.a() as f32 * factor.clamp(0.0, 1.0)) as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}…")
}
