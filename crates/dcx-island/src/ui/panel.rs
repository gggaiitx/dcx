//! The expanded panel: four tabs sharing one header, one tab bar and one
//! footer.
//!
//! Every widget is placed from explicit rectangles rather than egui's layout,
//! for the same reason the capsule is: the panel is resized by dragging its
//! bottom-right corner, and a layout pass would re-flow every row on each frame
//! of that drag. Rows are painted and hit-tested by the same numbers, so a click
//! can never land on a row the user is not looking at.

use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, Ui, Vec2};

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
    /// Minutes between reminders while composing; 0 = one reminder a day.
    pub task_every: &'a mut u32,
    pub clip_query: &'a mut String,
    /// Wheel offset of the two scrollable lists, in points (see [`scroll`]).
    pub clip_scroll: &'a mut f32,
    pub inbox_scroll: &'a mut f32,
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
    /// Open the app that produced this row (and mark the row read).
    OpenInbox(u64, Source),
}

/// `merged` means the caller already painted one card over the whole window
/// (capsule row included), so this function must not paint a second, rounded
/// background of its own — doing so would draw the panel's top corners and
/// border in the middle of that card.
pub fn draw(
    ui: &mut egui::Ui,
    rect: Rect,
    st: &mut PanelState<'_>,
    p: &Palette,
    actions: &mut Vec<Action>,
    merged: bool,
) {
    let radius = 14.0;
    if !merged {
        widgets::shadow(ui, rect, radius, p.shadow);
        widgets::round_rect(
            ui,
            rect,
            radius,
            p.surface,
            Stroke::new(1.0_f32, p.stroke),
        );
    }

    let pad = 14.0;
    let tab_h = 34.0;
    let footer_h = 26.0;

    // No header of its own. The island's capsule row is the panel's title row
    // now: it carries the clock, the date, the metrics and the close button, and
    // it is drawn by `app.rs` above this rectangle. A second header row here
    // repeated the date and pushed the tab bar down for nothing — that duplicate
    // row was most of why the two halves read as separate panels.

    // -- tab bar -----------------------------------------------------------
    let tabbar = Rect::from_min_size(
        Pos2::new(rect.left(), rect.top() + 2.0),
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
        // Counts live on the tab that owns them. The panel header used to carry
        // "N 项待办"; with the island row taking over as the title bar there is
        // no room for it up there, and next to the word "任务" is where a user
        // looks for it anyway.
        if *tab == Tab::Tasks {
            let open = st.tasks.for_day(st.today).iter().filter(|t| !t.done).count();
            if open > 0 {
                label = format!("{} {}", label, open.min(99));
            }
        }
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
    // The export action lives in this tab's tool row, at the same slot the
    // clipboard tab puts its "clear" button in: tab-scoped actions stay on the
    // tab that owns the data. It used to sit in the panel's title row, where it
    // meant nothing until you guessed which list it exported.
    let doc_w = 24.0;
    let time_w = 62.0;
    let repeat_w = 48.0;
    let every_w = 46.0;
    let tail = add_w + doc_w + time_w + repeat_w + every_w;
    let field = Rect::from_min_max(
        composer.min + Vec2::new(8.0, 4.0),
        Pos2::new(
            composer.right() - (tail + 14.0),
            composer.bottom() - 4.0,
        ),
    );
    let resp = ui.put(
        field,
        egui::TextEdit::singleline(st.task_input)
            // The hint has to fit the field it is drawn in: at the panel's
            // minimum width the long hint ran into the time field's own hint and
            // the row read "添加任务，时间" as if it were one string.
            .hint_text(if field.width() > 118.0 {
                "添加任务，回车确认"
            } else {
                "新任务"
            })
            .frame(false)
            .text_color(p.text)
            .margin(egui::Margin::symmetric(2.0, 4.0)),
    );
    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        actions.push(Action::AddTask);
        resp.request_focus();
    }

    // Reminder time field: accepts `15:30`, or `09:00-18:00` for an interval.
    // Empty means no reminder.
    let mut x = composer.right() - (tail + 8.0);
    let time_rect = Rect::from_min_size(
        Pos2::new(x, composer.top() + 4.0),
        Vec2::new(time_w, composer.height() - 8.0),
    );
    // Colour is computed before the widget takes its mutable borrow of the
    // text buffer; reading the same String inside the builder chain is a
    // second borrow and does not compile.
    let time_ok = crate::clock::parse_hhmm_span(st.task_time).is_some()
        || st.task_time.trim().is_empty();
    let time_color = if time_ok { p.text_dim } else { p.danger };
    ui.put(
        time_rect,
        egui::TextEdit::singleline(st.task_time)
            // A format hint, not a label: "09:00" tells the user what to type in
            // the space where they type it, and stays short enough to fit.
            .hint_text("09:00")
            .frame(false)
            .text_color(time_color)
            .margin(egui::Margin::symmetric(2.0, 4.0)),
    )
    .on_hover_text("09:00 单次；09:00-18:00 表示区间（配合右侧间隔）");

    x = time_rect.right() + 2.0;
    let repeat_rect = Rect::from_min_size(
        Pos2::new(x, composer.top() + 4.0),
        Vec2::new(repeat_w, composer.height() - 8.0),
    );
    if cycle_chip(ui, repeat_rect, p, st.task_repeat.label(), false).clicked() {
        *st.task_repeat = match *st.task_repeat {
            crate::tasks::Repeat::Once => crate::tasks::Repeat::Daily,
            crate::tasks::Repeat::Daily => crate::tasks::Repeat::Workdays,
            crate::tasks::Repeat::Workdays => crate::tasks::Repeat::Weekly,
            crate::tasks::Repeat::Weekly => crate::tasks::Repeat::Once,
        };
    }

    // Interval chip: how often inside the day. Deliberately a separate control
    // from the repeat chip, because they answer different questions — "which
    // days" and "how often within a day". Merging them into one five-value
    // cycle would make "工作日每半小时" reachable only through an obscure order.
    x = repeat_rect.right() + 2.0;
    let every_rect = Rect::from_min_size(
        Pos2::new(x, composer.top() + 4.0),
        Vec2::new(every_w, composer.height() - 8.0),
    );
    if cycle_chip(ui, every_rect, p, every_label(*st.task_every), *st.task_every > 0)
        .on_hover_text("每半小时提醒一次，例如工作日 09:00-18:00 每 30 分钟")
        .clicked()
    {
        *st.task_every = match *st.task_every {
            0 => 15,
            15 => 30,
            30 => 60,
            60 => 120,
            _ => 0,
        };
    }

    let add_rect = Rect::from_min_size(
        Pos2::new(composer.right() - add_w - 2.0, composer.top() + 3.0),
        Vec2::new(add_w - 2.0, composer.height() - 6.0),
    );
    if widgets::icon_button(ui, add_rect, Glyph::Plus, p, false).clicked() {
        actions.push(Action::AddTask);
    }

    let doc_rect = Rect::from_min_size(
        Pos2::new(add_rect.left() - doc_w + 2.0, composer.top() + 3.0),
        Vec2::new(doc_w - 4.0, composer.height() - 6.0),
    );
    if widgets::icon_button(ui, doc_rect, Glyph::Doc, p, false)
        .on_hover_text("把今天的任务导出为 Markdown")
        .clicked()
    {
        actions.push(Action::Archive);
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
    // An interval row shows two lines on the right (repeat + interval above,
    // the window below), so it needs more room than a plain row's one time.
    let interval = task.is_interval();
    let right_reserve = if interval { 118.0 } else { 92.0 };
    let title_right = row.right() - right_reserve;
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
    let clock = crate::clock::Clock::now();
    let overdue = task.due_now(&clock.iso_date(), clock.secs_of_day());
    let time_colour = if overdue { p.danger } else { p.text_dim };
    if interval {
        // Two lines instead of one: `工作日 · 每30分` over `09:00–18:00`. Squeezed
        // into a single line the row would either truncate the window or push
        // the title down to a few characters.
        let every = task.every.unwrap_or(0);
        let meta = if task.repeat == crate::tasks::Repeat::Once {
            short_every(every)
        } else {
            format!("{} · {}", task.repeat.label(), short_every(every))
        };
        widgets::text(
            ui,
            Pos2::new(right, row.center().y - 7.0),
            Align2::RIGHT_CENTER,
            &meta,
            9.0,
            p.text_faint,
        );
        if let Some(window) = task.window_label() {
            widgets::mono_text(
                ui,
                Pos2::new(right, row.center().y + 7.0),
                Align2::RIGHT_CENTER,
                &window,
                10.0,
                time_colour,
            );
        }
    } else {
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
            widgets::mono_text(
                ui,
                Pos2::new(right, row.center().y),
                Align2::RIGHT_CENTER,
                &text,
                10.5,
                time_colour,
            );
        }
    }
}

/// `每30分` / `每2时` — the short form used inside a task row.
fn short_every(minutes: u32) -> String {
    if minutes == 0 {
        String::new()
    } else if minutes % 60 == 0 {
        format!("每{}时", minutes / 60)
    } else {
        format!("每{}分", minutes)
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
    // Row height follows the text: a fixed 40/46 pt row clipped the third line
    // of every longer entry in half, which is what "内容看不完整" was. The width
    // here is what the text actually gets (left padding + pin/delete column).
    let text_w = (list.width() - 8.0 - 22.0 - 22.0 - 4.0).max(60.0);
    let heights: Vec<f32> = items.iter().map(|i| clip_row_h(ui, i, text_w)).collect();
    let content_h: f32 = heights.iter().sum();
    let offset = scroll(ui, list, content_h, st.clip_scroll);
    let mut y = list.top() - offset;
    // Rows are painted from an explicit rectangle, so the painter has to be
    // clipped to the list or a scrolled row would draw over the search field.
    // The previous clip is restored at the end: the resize grip is drawn after
    // this tab and would otherwise be clipped away too.
    let prev_clip = ui.clip_rect();
    ui.set_clip_rect(list);
    for (item, row_h) in items.iter().zip(heights.iter().copied()) {
        if y + row_h < list.top() {
            y += row_h;
            continue;
        }
        if y > list.bottom() {
            break;
        }
        let row = Rect::from_min_size(Pos2::new(list.left(), y), Vec2::new(list.width(), row_h));
        // A partially scrolled row is painted (the list should not blink empty
        // bands) and stays clickable on its visible part: `set_clip_rect` above
        // also clips every interaction rectangle to the list, so it cannot steal
        // a click from the search box.
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
        // Top-aligned: a three-line entry used to be centred inside a two-line
        // box, so its first line was clipped in half and the text read as cut
        // off. Now the overflow goes below the clip, which looks like a list row
        // that continues rather than a broken glyph row.
        ui.painter()
            .with_clip_rect(text_rect)
            .galley(Pos2::new(text_rect.left(), text_rect.top()), galley, p.text_dim);

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
    scroll_bar(ui, list, p, content_h, offset);
    ui.set_clip_rect(prev_clip);
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

    // Order on this page: channel health, then the two list actions, then the
    // list. The actions used to sit above the health rows, which meant "全部已读
    // / 清空" hung a line away from the only thing they act on and the page
    // opened with a toolbar for an empty list.
    let mut y = rect.top();
    for (source, state, last_at, note) in &channels {
        let row = Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(rect.width(), card_h));
        let resp = ui.interact(row, ui.id().with(("ch", source.key())), egui::Sense::hover());
        if resp.hovered() {
            widgets::round_rect(ui, row, 7.0, with_alpha(p.surface_alt, 0.7), Stroke::NONE);
        }
        // The dot reports the connection, not the brand. Four brand-coloured
        // dots read as four working channels, and one of them was an account
        // that is not even filled in; now colour means "live" and grey means
        // "not live", which is the only reading that survives a glance.
        let connected = state.is_ok();
        let dot_color = if connected {
            let (r, g, b) = source.color();
            Color32::from_rgb(r, g, b)
        } else {
            p.text_faint
        };
        let dot = Pos2::new(row.left() + 6.0, row.center().y);
        ui.painter().circle_filled(dot, 3.5, dot_color);
        widgets::text(
            ui,
            Pos2::new(dot.x + 10.0, row.center().y),
            Align2::LEFT_CENTER,
            source.label(),
            11.0,
            if connected { p.text } else { p.text_dim },
        );
        let (label, state_color) = channel_status(state, *last_at, st.today, p);
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
        // The row explains itself on hover: the right-hand label has room for
        // a state, not for how the channel listens, and "已接入" alone does not
        // tell anyone what the island can see from that app.
        resp.on_hover_text(format!("{}\n{}", label, note));
        y += card_h;
    }

    y += 4.0;
    let toolbar = Rect::from_min_size(
        Pos2::new(rect.left(), y),
        Vec2::new(rect.width(), 20.0),
    );
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

    y = toolbar.bottom() + 4.0;
    ui.painter().line_segment(
        [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
        Stroke::new(1.0_f32, p.divider),
    );
    y += 6.0;

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
    // Line heights are measured, not guessed. The row used to be a fixed 40/48
    // with hand-picked offsets, and those offsets were computed for 100 % DPI:
    // at 125 % the 12 pt headline needs ~17 pt while the detail line was being
    // drawn at `bottom - 5`, so the two boxes overlapped and "来源 / 标题 /
    // 详情" read as one smeared clump. Everything below is derived from these
    // three numbers, so the row is correct at any scale.
    let lh_small = ui.fonts(|f| f.row_height(&FontId::proportional(9.5))).max(12.0);
    let lh_title = ui.fonts(|f| f.row_height(&FontId::proportional(12.0))).max(15.0);
    const PAD_TOP: f32 = 3.0;
    const PAD_BOT: f32 = 5.0;
    // Only the message list scrolls; the per-channel health cards above it stay
    // pinned, which is what makes a long list usable.
    let list = Rect::from_min_max(Pos2::new(rect.left(), y), rect.max);
    // `content_h` has to agree with the per-row height below or the scrollbar
    // runs out of travel early.
    let row_h_for = |detail: &str| {
        let h = PAD_TOP + lh_small + lh_title + PAD_BOT
            + if detail.is_empty() { 0.0 } else { lh_small };
        h.max(40.0)
    };
    let content_h: f32 = items.iter().map(|(_, _, _, d, _, _)| row_h_for(d)).sum();
    let offset = scroll(ui, list, content_h, st.inbox_scroll);
    let prev_clip = ui.clip_rect();
    ui.set_clip_rect(list);
    y = list.top() - offset;
    for (id, source, title, detail, at, read) in items {
        let row_h = row_h_for(&detail);
        if y + row_h < list.top() {
            y += row_h;
            continue;
        }
        if y > list.bottom() {
            break;
        }
        let row = Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(rect.width(), row_h));
        // See `clip_tab`: the clip rect also bounds every interaction, so a
        // half-visible row cannot take clicks from the cards above the list.
        let hovered = ui.rect_contains_pointer(row);
        if hovered {
            widgets::round_rect(ui, row.shrink2(Vec2::new(0.0, 1.0)), 8.0, with_alpha(p.surface_alt, 0.7), Stroke::NONE);
        }
        if !read {
            ui.painter().circle_filled(
                Pos2::new(row.left() + 4.0, row.top() + PAD_TOP + lh_small * 0.5),
                2.0,
                p.accent,
            );
        }
        // Line 1: source on the left, age on the right, both on the same
        // measured baseline so neither can climb into the headline.
        let line1_y = row.top() + PAD_TOP;
        widgets::text(
            ui,
            Pos2::new(row.left() + 12.0, line1_y),
            Align2::LEFT_TOP,
            source.label(),
            9.5,
            p.text_faint,
        );
        let time = crate::inbox::short_time(at, st.today);
        widgets::text(
            ui,
            Pos2::new(row.right() - 8.0, line1_y + lh_small * 0.5),
            Align2::RIGHT_CENTER,
            &time,
            9.5,
            p.text_faint,
        );
        // Line 2: the headline, on one line and ellipsised to the width it has.
        // Wrapping it into the detail line is the bug this row is recovering
        // from, so the fit is done by measurement rather than by character count.
        let title_y = line1_y + lh_small;
        // 24 pt on the right is where the hover-only delete button sits.
        let title_w = row.width() - 12.0 - 26.0;
        let title_color = if read { p.text_dim } else { p.text };
        widgets::text(
            ui,
            Pos2::new(row.left() + 12.0, title_y),
            Align2::LEFT_TOP,
            &fit_one_line(ui, &title, 12.0, title_w),
            12.0,
            title_color,
        );
        if !detail.is_empty() {
            widgets::text(
                ui,
                Pos2::new(row.left() + 12.0, title_y + lh_title),
                Align2::LEFT_TOP,
                &fit_one_line(ui, &detail, 9.5, title_w),
                9.5,
                p.text_faint,
            );
        }
        if hovered {
            let del = Rect::from_center_size(Pos2::new(row.right() - 14.0, row.bottom() - 13.0), Vec2::splat(18.0));
            if widgets::icon_button(ui, del, Glyph::Trash, p, true).clicked() {
                actions.push(Action::RemoveInbox(id));
            }
        }
        // Clicking the row opens the app that raised it. The delete button owns
        // the bottom-right corner, so the click target stops short of it.
        let hit = Rect::from_min_max(
            Pos2::new(row.left(), row.top()),
            Pos2::new(row.right() - 30.0, row.bottom()),
        );
        let resp = ui.interact(
            hit,
            ui.id().with(("inbox-row", id)),
            egui::Sense::click(),
        );
        if resp.clicked() {
            actions.push(Action::OpenInbox(id, source));
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        y += row_h;
    }
    scroll_bar(ui, list, p, content_h, offset);
    ui.set_clip_rect(prev_clip);
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

    // `metrics.style` is a real switch in the settings centre ("条形 / 环形");
    // the panel used to draw rings whatever it said. The capsule keeps bars —
    // there is no room for a ring in 34 pt.
    let rings = st.cfg.metrics.style == "ring";
    for (i, (label, value)) in [("CPU", snap.cpu), ("内存", snap.mem_percent)]
        .into_iter()
        .enumerate()
    {
        let card = Rect::from_min_size(
            Pos2::new(top.left() + (card_w + gap) * i as f32, top.top()),
            Vec2::new(card_w, ring_d),
        );
        if rings {
            ring_card(ui, card, p, label, value);
        } else {
            bar_card(ui, card, p, label, value);
        }
    }

    let mut y = top.bottom() + 12.0;
    let mut rows: Vec<(String, String, Option<f32>)> = vec![
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
    // `metrics.disk` is a switch in the settings centre; the row was always
    // shown regardless of it.
    if !st.cfg.metrics.disk {
        rows.retain(|(label, _, _)| label != "系统盘");
    }
    for (label, value, _) in rows.drain(..) {
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

/// The `bar` alternative to [`ring_card`], selected by `metrics.style`.
fn bar_card(ui: &mut egui::Ui, rect: Rect, p: &Palette, label: &str, percent: f32) {
    widgets::round_rect(ui, rect, 10.0, p.surface_alt, Stroke::new(1.0_f32, p.divider));
    widgets::text(
        ui,
        Pos2::new(rect.left() + 10.0, rect.top() + 9.0),
        Align2::LEFT_TOP,
        label,
        9.5,
        p.text_faint,
    );
    widgets::mono_text(
        ui,
        Pos2::new(rect.left() + 10.0, rect.center().y - 2.0),
        Align2::LEFT_CENTER,
        &format!("{percent:.0}%"),
        20.0,
        p.text,
    );
    let bar = Rect::from_min_max(
        Pos2::new(rect.left() + 10.0, rect.bottom() - 16.0),
        Pos2::new(rect.right() - 10.0, rect.bottom() - 10.0),
    );
    widgets::bar(
        ui,
        bar,
        percent / 100.0,
        p.level(crate::metrics::level(percent)),
        p.track,
    );
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

// ---------------------------------------------------------------------------
// Scrolling for the hand-placed lists
// ---------------------------------------------------------------------------

/// Advance the wheel offset of a hand-placed list and return it.
///
/// `ScrollArea` cannot be used here: every row is painted and hit-tested from
/// explicit rectangles (see the module comment), and the lists used to simply
/// stop at `list.bottom()`, which hid all but the first ~7 of the 200 clipboard
/// entries. This keeps the model and only adds an offset.
///
/// `raw_scroll_delta` rather than `smooth_scroll_delta`: egui spreads a smooth
/// delta over its own frames, and this panel repaints at 120 ms while open, so
/// the leftover arrived in bursts (the same reason `dcx-box` switched).
fn scroll(ui: &egui::Ui, view: Rect, content_h: f32, offset: &mut f32) -> f32 {
    let max = (content_h - view.height()).max(0.0);
    *offset = offset.clamp(0.0, max);
    if max > 0.0 && ui.rect_contains_pointer(view) {
        let delta = ui.input(|i| i.raw_scroll_delta.y);
        if delta.abs() > 0.01 {
            *offset = (*offset - delta).clamp(0.0, max);
            ui.ctx().request_repaint();
        }
    }
    *offset
}

/// Thin scrollbar on the right edge of `view`, drawn only when it overflows.
fn scroll_bar(ui: &egui::Ui, view: Rect, p: &Palette, content_h: f32, offset: f32) {
    if content_h <= view.height() + 0.5 {
        return;
    }
    let track = Rect::from_min_max(
        Pos2::new(view.right() - 3.0, view.top()),
        Pos2::new(view.right(), view.bottom()),
    );
    let frac = (view.height() / content_h).clamp(0.08, 1.0);
    let h = (track.height() * frac).max(18.0);
    let t = offset / (content_h - view.height()).max(1.0);
    let y = track.top() + (track.height() - h) * t;
    widgets::round_rect(
        ui,
        Rect::from_min_size(Pos2::new(track.left(), y), Vec2::new(track.width(), h)),
        1.5,
        with_alpha(p.text_faint, 0.55),
        Stroke::NONE,
    );
}

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

/// Right-hand status of a channel row, with the colour it is written in.
///
/// "已接入 / 未接入" rather than the state machine's own vocabulary: the dot on
/// the left says the same thing in colour, and both have to agree.
fn channel_status(
    state: &ChannelState,
    last_at: Option<u64>,
    today: &str,
    p: &Palette,
) -> (String, Color32) {
    match state {
        ChannelState::Ok => (
            match last_at {
                Some(at) => format!("已接入 · {}", crate::inbox::short_time(at, today)),
                None => "已接入 · 尚未收到信号".to_string(),
            },
            p.ok,
        ),
        ChannelState::Idle => ("接入中".to_string(), p.text_dim),
        ChannelState::Off => ("未接入".to_string(), p.text_faint),
        other => (other.label(), p.warn),
    }
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

/// Compact cycling chip used by the task composer for 重复 and 间隔. The caller
/// owns the value and the cycle order — the chip only draws and reports clicks.
///
/// It carries a faint plate even when idle: as bare grey text the two chips read
/// as labels, and a label is not something a user tries to click. `active` (an
/// interval is set) tints both the plate and the text so the row shows at a
/// glance that this task reminds more than once.
fn cycle_chip(ui: &mut egui::Ui, rect: Rect, p: &Palette, label: &str, active: bool) -> egui::Response {
    let resp = ui.allocate_rect(rect, egui::Sense::click());
    let plate = rect.shrink2(Vec2::new(1.0, 3.0));
    let fill = if resp.hovered() {
        p.accent_soft
    } else if active {
        with_alpha(p.accent, 0.10)
    } else {
        with_alpha(p.surface_alt, 0.9)
    };
    widgets::round_rect(ui, plate, 6.0, fill, Stroke::NONE);
    let colour = if resp.hovered() || active {
        p.accent
    } else {
        p.text_dim
    };
    widgets::text(ui, rect.center(), Align2::CENTER_CENTER, label, 9.5, colour);
    resp
}

/// Chip label for the composer's interval state. `0` is "once a day", which is
/// the default and has to read as a non-choice rather than as a value.
fn every_label(minutes: u32) -> &'static str {
    match minutes {
        15 => "15分",
        30 => "30分",
        60 => "1时",
        120 => "2时",
        _ => "单次",
    }
}

fn with_alpha(color: Color32, factor: f32) -> Color32 {
    let a = (color.a() as f32 * factor.clamp(0.0, 1.0)) as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

/// Clipboard row height, measured from the wrapped preview.
///
/// Fixed heights were the bug: 40 pt fitted one line, 46 pt fitted two, and a
/// three-line entry was cut in the middle of the third line. The row now grows
/// with its text, capped at three lines — and there is no hover tooltip to read
/// the rest in: a tooltip that covers the list it belongs to made the whole tab
/// flicker while the pointer moved over it.
fn clip_row_h(ui: &egui::Ui, item: &ClipItem, text_w: f32) -> f32 {
    match item.kind {
        Kind::Image => 52.0,
        Kind::Text => {
            const PAD: f32 = 8.0;
            const MAX_LINES: f32 = 3.0;
            let galley = ui.painter().layout(
                item.preview(),
                FontId::proportional(12.0),
                Color32::WHITE,
                text_w,
            );
            let line = ui
                .fonts(|f| f.row_height(&FontId::proportional(12.0)))
                .max(12.0);
            let lines = (galley.size().y / line).round().clamp(1.0, MAX_LINES);
            (lines * line + PAD).max(34.0)
        }
    }
}

/// Trims `text` to a single line of `max_w` at `size`, ending in an ellipsis.
///
/// Measured, not counted: a 12 pt CJK glyph is about twice as wide as a 12 pt
/// Latin one, so "40 characters" is either a wasted line or two inches of
/// overflow depending on the string. Binary search over the prefix length keeps
/// this to a handful of layout calls per row.
fn fit_one_line(ui: &Ui, text: &str, size: f32, max_w: f32) -> String {
    if max_w <= 0.0 {
        return String::new();
    }
    let font = FontId::proportional(size);
    let width = |s: &str| {
        ui.fonts(|f| {
            f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
    };
    if width(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let cand: String = chars[..mid].iter().collect::<String>() + "…";
        if width(&cand) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo == 0 {
        return "…".to_string();
    }
    chars[..lo].iter().collect::<String>() + "…"
}
