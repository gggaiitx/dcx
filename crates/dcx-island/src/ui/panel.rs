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

    /// Load a thumbnail once, decoded no larger than the row can show.
    ///
    /// The row is 48×~42 pt (≈65×52 px at 125 %); a full-size screenshot
    /// (2180×908) used to become a 7.9 MB RGBA texture *per entry* — nine of
    /// those were ~70 MB of the island's memory. Decoding down to a 160 px box
    /// keeps the aspect ratio, covers 200 % scaling, and costs ~40 KB each.
    fn get(&mut self, ctx: &egui::Context, id: u64, path: &std::path::Path) -> Option<egui::TextureHandle> {
        if let Some(t) = self.map.get(&id) {
            return Some(t.clone());
        }
        const MAX_EDGE: u32 = 160;
        let decoded = image::open(path).ok()?;
        let decoded = if decoded.width() > MAX_EDGE || decoded.height() > MAX_EDGE {
            decoded.thumbnail(MAX_EDGE, MAX_EDGE)
        } else {
            decoded
        };
        let img = decoded.to_rgba8();
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
    pub perf_scroll: &'a mut f32,
    pub live_scroll: &'a mut f32,
    /// Cards from the local push API; read-only here, the UI thread owns them.
    pub cards: &'a crate::api::PushCards,
    /// Now-playing snapshot, its locally interpolated position, and the cover
    /// texture built by the UI thread. All three appear and vanish together.
    pub media: Option<&'a crate::media::Snapshot>,
    pub media_pos: f64,
    pub media_cover: Option<&'a egui::TextureHandle>,
    /// Desktop/library counts from the organizer; absent until the first scan.
    pub library: Option<crate::library::Snapshot>,
    /// Start-menu apps, unfiltered; the tab filters by its own query.
    pub apps: &'a [crate::launcher::AppEntry],
    pub search_query: &'a mut String,
    pub search_scroll: &'a mut f32,
    /// File results; meaningful only when `search_ready`.
    pub search_hits: &'a [crate::search::FileHit],
    pub search_ready: bool,
    pub search_error: Option<&'a str>,
    /// Age of the snapshot behind `search_hits`, seconds.
    pub search_index_age: Option<u64>,
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
    /// Run a push card's whitelisted action (open url / folder / panel tab).
    RunCard(String),
    /// Dismiss a push card before its TTL expires.
    DismissCard(String),
    /// Transport key for the now-playing row.
    MediaCmd(crate::media::Cmd),
    /// Copy this entry into the organizer's collection ("收进盒子").
    ClipToBox(u64),
    /// Open the organizer's library root in Explorer.
    OpenLibrary,
    /// Open a file or shortcut from the search tab.
    OpenPath(String),
    /// Show a search result in Explorer.
    RevealPath(String),
    /// The search field changed; the app debounces the query.
    SearchEdited,
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
    let mut content = Rect::from_min_max(
        Pos2::new(rect.left() + pad, tabbar.bottom() + 6.0),
        Pos2::new(rect.right() - pad, footer.top() - 4.0),
    );
    // The now-playing row sits above everything the tabs draw, on every tab:
    // it belongs to the desktop, not to a list. Only while a session exists.
    if let Some(snap) = st.media {
        let row = Rect::from_min_size(content.min, Vec2::new(content.width(), MEDIA_ROW_H));
        draw_media_row(ui, row, p, snap, st.media_pos, st.media_cover, actions);
        content = Rect::from_min_max(
            Pos2::new(content.left(), row.bottom() + 8.0),
            content.max,
        );
    }
    if content.height() <= 8.0 {
        return;
    }
    match st.tab {
        Tab::Tasks => tasks_tab(ui, content, p, st, actions),
        Tab::Clip => clip_tab(ui, content, p, st, actions),
        Tab::Inbox => inbox_tab(ui, content, p, st, actions),
        Tab::Perf => perf_tab(ui, content, p, st, actions),
        Tab::Live => live_tab(ui, content, p, st, actions),
        Tab::Search => search_tab(ui, content, p, st, actions),
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

// ---------------------------------------------------------------------------
// Now playing
// ---------------------------------------------------------------------------

/// 34 pt cover + 2 × 6 pt pad.
const MEDIA_ROW_H: f32 = 46.0;

fn draw_media_row(
    ui: &mut egui::Ui,
    rect: Rect,
    p: &Palette,
    snap: &crate::media::Snapshot,
    pos: f64,
    cover: Option<&egui::TextureHandle>,
    actions: &mut Vec<Action>,
) {
    widgets::round_rect(ui, rect, 9.0, with_alpha(p.surface_alt, 0.55), Stroke::NONE);
    let pad = 6.0;
    let inner = rect.shrink2(Vec2::new(pad, pad));

    let cov = Rect::from_min_size(inner.min, Vec2::splat(inner.height()));
    match cover {
        Some(tex) => {
            ui.put(
                cov,
                egui::Image::new(tex)
                    .fit_to_exact_size(cov.size())
                    .rounding(6.0),
            );
        }
        None => {
            widgets::round_rect(ui, cov, 6.0, with_alpha(p.surface, 0.9), Stroke::NONE);
            widgets::glyph(ui, cov.center(), 14.0, Glyph::Note, p.text_faint);
        }
    }

    // Transport keys, right-aligned. Disabled keys still occupy their slot:
    // a row that reshuffles per player would move the buttons under the
    // pointer.
    let btn = 24.0;
    let gap = 2.0;
    let mut bx = inner.right() - btn * 3.0 - gap * 2.0;
    let mut button = |ui: &mut egui::Ui, glyph: Glyph, enabled: bool| -> bool {
        let r = Rect::from_center_size(Pos2::new(bx + btn * 0.5, rect.center().y), Vec2::splat(btn));
        bx += btn + gap;
        if enabled {
            widgets::icon_button(ui, r, glyph, p, false).clicked()
        } else {
            widgets::glyph(ui, r.center(), 12.0, glyph, with_alpha(p.text_faint, 0.45));
            false
        }
    };
    if button(ui, Glyph::Prev, snap.can_prev) {
        actions.push(Action::MediaCmd(crate::media::Cmd::Prev));
    }
    let toggle = if snap.playing { Glyph::Pause } else { Glyph::Play };
    if button(ui, toggle, snap.can_toggle) {
        actions.push(Action::MediaCmd(crate::media::Cmd::Toggle));
    }
    if button(ui, Glyph::Next, snap.can_next) {
        actions.push(Action::MediaCmd(crate::media::Cmd::Next));
    }

    let lh_title = ui.fonts(|f| f.row_height(&FontId::proportional(12.0))).max(15.0);
    let lh_meta = ui.fonts(|f| f.row_height(&FontId::proportional(9.5))).max(12.0);
    let text_left = cov.right() + 8.0;
    let text_right = inner.right() - btn * 3.0 - gap * 2.0 - 8.0;
    let title_w = (text_right - text_left).max(20.0);
    let text_top = rect.center().y - (lh_title + lh_meta) * 0.5 + 0.5;

    let title = if snap.title.is_empty() {
        "未知曲目"
    } else {
        snap.title.as_str()
    };
    widgets::text(
        ui,
        Pos2::new(text_left, text_top),
        Align2::LEFT_TOP,
        &widgets::fit_one_line(ui, title, 12.0, title_w),
        12.0,
        p.text,
    );

    let mut meta = snap.artist.clone();
    if !snap.album.is_empty() && snap.album != snap.title {
        if !meta.is_empty() {
            meta.push_str(" · ");
        }
        meta.push_str(&snap.album);
    }
    let time = if snap.duration > 0.0 {
        format!("{} / {}", fmt_time(pos), fmt_time(snap.duration))
    } else {
        String::new()
    };
    let time_w = if time.is_empty() {
        0.0
    } else {
        ui.fonts(|f| {
            f.layout_no_wrap(time.clone(), FontId::proportional(9.5), Color32::WHITE)
                .size()
                .x
        })
    };
    widgets::text(
        ui,
        Pos2::new(text_left, text_top + lh_title),
        Align2::LEFT_TOP,
        &widgets::fit_one_line(ui, &meta, 9.5, (title_w - time_w - 8.0).max(10.0)),
        9.5,
        p.text_faint,
    );
    if !time.is_empty() {
        widgets::text(
            ui,
            Pos2::new(text_right, text_top + lh_title + lh_meta * 0.5),
            Align2::RIGHT_CENTER,
            &time,
            9.5,
            p.text_faint,
        );
    }

    // Progress hairline along the plate's bottom edge — the one place the
    // interpolated position shows without competing with the title.
    if snap.duration > 0.0 {
        let track = Rect::from_min_max(
            Pos2::new(inner.left(), rect.bottom() - 3.5),
            Pos2::new(inner.right(), rect.bottom() - 1.5),
        );
        widgets::round_rect(ui, track, 1.0, with_alpha(p.text_faint, 0.3), Stroke::NONE);
        let frac = (pos / snap.duration).clamp(0.0, 1.0) as f32;
        if frac > 0.001 {
            widgets::round_rect(
                ui,
                Rect::from_min_size(track.min, Vec2::new(track.width() * frac, track.height())),
                1.0,
                p.accent,
                Stroke::NONE,
            );
        }
    }
}

fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Snapshot age in words: "刚刚" / "12 分钟前" / "3 小时前" / "2 天前".
fn age_label(secs: u64) -> String {
    if secs < 60 {
        "刚刚".to_string()
    } else if secs < 3600 {
        format!("{} 分钟前", secs / 60)
    } else if secs < 86_400 {
        format!("{} 小时前", secs / 3600)
    } else {
        format!("{} 天前", secs / 86_400)
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
        if *tab == Tab::Live {
            let n = st.cards.len();
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
        Tab::Live => "脚本推送的临时卡片 · 到期自动消失",
        Tab::Search => "Enter 打开首项 · Ctrl+Enter 定位",
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
    let time_resp = ui.put(
        time_rect,
        egui::TextEdit::singleline(st.task_time)
            // A format hint, not a label: "09:00" tells the user what to type in
            // the space where they type it, and stays short enough to fit.
            .hint_text("09:00")
            .frame(false)
            .text_color(time_color)
            .margin(egui::Margin::symmetric(2.0, 4.0)),
    );
    hover_hint(time_resp, "09:00 单次；09:00-18:00 表示区间（配合右侧间隔）");

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
    if hover_hint(
        cycle_chip(ui, every_rect, p, every_label(*st.task_every), *st.task_every > 0),
        "每半小时提醒一次，例如工作日 09:00-18:00 每 30 分钟",
    )
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
    if hover_hint(
        widgets::icon_button(ui, doc_rect, Glyph::Doc, p, false),
        "把今天的任务导出为 Markdown",
    )
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
    if hover_hint(
        widgets::icon_button(ui, clear, Glyph::Trash, p, true),
        "清空未固定的条目",
    )
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
    let text_w = (list.width() - 8.0 - 22.0 - 22.0 - 22.0 - 4.0).max(60.0);
    let heights: Vec<f32> = items.iter().map(|i| clip_row_h(ui, i, text_w)).collect();
    let content_h: f32 = heights.iter().sum();
    scroll(ui, list, content_h, st.clip_scroll);
    scroll_bar(ui, list, p, content_h, st.clip_scroll);
    let offset = *st.clip_scroll;
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
        // The delete slot is reserved even when the pointer is elsewhere:
        // allocating it only on hover shifted pin and collect 22 pt left the
        // moment the pointer entered the row, so aiming at collect landed on
        // the pin (and at the pin landed on the trash).
        let del = Rect::from_center_size(Pos2::new(icons_right - 8.0, row.center().y), Vec2::splat(20.0));
        if hovered && widgets::icon_button_sized(ui, del, Glyph::Trash, p, true, 10.5, 1.2).clicked() {
            actions.push(Action::RemoveClip(item.id));
        }
        icons_right -= 22.0;
        let pin = Rect::from_center_size(Pos2::new(icons_right - 8.0, row.center().y), Vec2::splat(20.0));
        if widgets::icon_button(ui, pin, Glyph::Pin, p, false).clicked() {
            actions.push(Action::PinClip(item.id));
        }
        if item.pinned {
            widgets::glyph(ui, pin.center(), 12.0, Glyph::Pin, p.accent);
        }
        icons_right -= 22.0;

        let collect = Rect::from_center_size(
            Pos2::new(icons_right - 8.0, row.center().y),
            Vec2::splat(20.0),
        );
        if hover_hint(
            widgets::icon_button(ui, collect, Glyph::Collect, p, false),
            "存入收纳盒（复制一份，剪贴板保留）",
        )
        .clicked()
        {
            actions.push(Action::ClipToBox(item.id));
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
    }    ui.set_clip_rect(prev_clip);
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
    let channels: Vec<(Source, ChannelState, Option<u64>, &'static str)> = st
        .inbox
        .channels
        .iter()
        .map(|(s, c)| (*s, c.state.clone(), c.last_at, c.note))
        .collect();

    // One tool row does both jobs: the channels as dot+icon chips on the left,
    // the two list actions on the right.
    //
    // The channels used to be four full-width rows above the list. They cost
    // ~110 pt of height on every visit to report something that changes twice a
    // day, and they pushed "全部已读 / 清空" into a band of their own — a line
    // away from the only thing those buttons act on. The list is what this tab
    // is for, so it gets the height back.
    let bar_h = 22.0;
    let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width(), bar_h));

    // Buttons are as wide as their labels, not a fixed 64 pt: a 64 pt box for
    // "清空" is a third of the row spent on air.
    let btn_w = |label: &str| {
        ui.painter()
            .layout_no_wrap(
                label.to_string(),
                FontId::proportional(10.0),
                Color32::WHITE,
            )
            .size()
            .x
            + 18.0
    };
    let clear_w = btn_w("清空").max(42.0);
    let read_w = btn_w("全部已读").max(56.0);
    let clear = Rect::from_min_size(
        Pos2::new(bar.right() - clear_w, bar.top()),
        Vec2::new(clear_w, bar_h),
    );
    let read_all = Rect::from_min_size(
        Pos2::new(clear.left() - 4.0 - read_w, bar.top()),
        Vec2::new(read_w, bar_h),
    );
    if small_button(ui, read_all, p, "全部已读").clicked() {
        actions.push(Action::MarkAllRead);
    }
    if small_button(ui, clear, p, "清空").clicked() {
        actions.push(Action::ClearInbox);
    }

    // Channel chips. The **dot** is the only thing that carries state — source
    // colour = live, grey ring = not live, orange ring = needs attention — and
    // the icon carries identity and stays neutral. Colouring the icons by brand
    // is what made four of them read as "four channels working" while one of
    // them had no account configured at all.
    //
    // 29 pt per chip with 8 pt between them: at 2 pt apart the four marks fused
    // into one strip of ink, and a 12 pt outline at 15 px has no room to breathe
    // — the silhouettes live at the edges of the shape.
    const CHIP_W: f32 = 29.0;
    const CHIP_GAP: f32 = 8.0;
    let mut x = bar.left();
    for (source, state, last_at, note) in &channels {
        let chip = Rect::from_min_size(Pos2::new(x, bar.top()), Vec2::new(CHIP_W, bar_h));
        if chip.right() > read_all.left() - 6.0 {
            break;
        }
        let live = state.is_ok();
        let hovered = ui.rect_contains_pointer(chip);
        // Opaque, and the same colour the two-bubble mark is filled with: a
        // translucent plate would leave a paler square inside the front bubble.
        let plate = if hovered { p.surface_alt } else { p.surface };
        if hovered {
            widgets::round_rect(ui, chip, 6.0, p.surface_alt, Stroke::NONE);
        }
        let dot = Pos2::new(chip.left() + 6.0, chip.center().y);
        if live {
            let (r, g, b) = source.color();
            ui.painter()
                .circle_filled(dot, 3.0, Color32::from_rgb(r, g, b));
        } else {
            // Hollow ring: filled means "listening", and nothing else does.
            let ring = if state.is_problem() { p.warn } else { p.text_faint };
            ui.painter()
                .circle_stroke(dot, 2.6, Stroke::new(1.1_f32, ring));
        }
        widgets::glyph_on(
            ui,
            Pos2::new(dot.x + 11.0, chip.center().y),
            13.5,
            channel_glyph(*source),
            if live { p.text_dim } else { p.text_faint },
            1.1,
            plate,
        );
        // The status text and the listening note both live in the tooltip now:
        // the row has room for state-as-colour, not for "已接入 · 尚未收到信号"
        // written four times over.
        let (status, _) = channel_status(state, *last_at, st.today, p);
        hover_hint(
            ui.interact(
                chip,
                ui.id().with(("ch", source.key())),
                egui::Sense::hover(),
            ),
            format!("{} · {}\n{}", source.label(), status, note),
        );
        x += CHIP_W + CHIP_GAP;
    }

    // A count, not a row: an orange ring says *which* chip, this says how many
    // so the problem is visible without hovering each one. Dropped entirely when
    // the row is too narrow to hold it — the ring is still there.
    let offline = channels
        .iter()
        .filter(|(_, c, _, _)| !c.is_ok() && !matches!(c, ChannelState::Idle))
        .count();
    if offline > 0 {
        let label = format!("{offline} 个渠道未接入");
        let w = ui
            .painter()
            .layout_no_wrap(label.clone(), FontId::proportional(9.5), p.warn)
            .size()
            .x;
        if x + 6.0 + w < read_all.left() - 8.0 {
            widgets::text(
                ui,
                Pos2::new(read_all.left() - 10.0, bar.center().y),
                Align2::RIGHT_CENTER,
                &label,
                9.5,
                p.warn,
            );
        }
    }

    let mut y = bar.bottom() + 5.0;
    ui.painter().line_segment(
        [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
        Stroke::new(1.0_f32, p.divider),
    );
    y += 7.0;

    let items: Vec<_> = st
        .inbox
        .items()
        .map(|i| (i.id, i.source, i.app.clone(), i.title.clone(), i.detail.clone(), i.at, i.read))
        .collect();
    if items.is_empty() {
        empty_state(
            ui,
            Rect::from_min_max(Pos2::new(rect.left(), y), rect.max),
            p,
            "没有新消息提醒",
            "开启的渠道（QQ / 微信 / 邮箱 / 系统通知…）会汇总到这里",
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
    let content_h: f32 = items.iter().map(|(_, _, _, _, d, _, _)| row_h_for(d)).sum();
    scroll(ui, list, content_h, st.inbox_scroll);
    scroll_bar(ui, list, p, content_h, st.inbox_scroll);
    let offset = *st.inbox_scroll;
    let prev_clip = ui.clip_rect();
    ui.set_clip_rect(list);
    y = list.top() - offset;
    for (id, source, app, title, detail, at, read) in items {
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
        // measured baseline so neither can climb into the headline. A System
        // row names the raising app instead — "系统" on every row would say
        // nothing about which of the twenty apps toasted.
        let line1_y = row.top() + PAD_TOP;
        let caption = if source == Source::System && !app.is_empty() {
            app.as_str()
        } else {
            source.label()
        };
        widgets::text(
            ui,
            Pos2::new(row.left() + 12.0, line1_y),
            Align2::LEFT_TOP,
            caption,
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
            &widgets::fit_one_line(ui, &title, 12.0, title_w),
            12.0,
            title_color,
        );
        if !detail.is_empty() {
            widgets::text(
                ui,
                Pos2::new(row.left() + 12.0, title_y + lh_title),
                Align2::LEFT_TOP,
                &widgets::fit_one_line(ui, &detail, 9.5, title_w),
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
    }    ui.set_clip_rect(prev_clip);
}

// ---------------------------------------------------------------------------
// Live (push API cards)
// ---------------------------------------------------------------------------

/// Wrapped body line count, capped at three. Free functions rather than
/// closures capturing `ui`: such a closure would hold an immutable borrow of
/// the ui across every mutable widget call in the row.
fn card_body_lines(ui: &egui::Ui, body: &str, inner_w: f32, lh_body: f32) -> f32 {
    if body.is_empty() {
        return 0.0;
    }
    let galley = ui
        .painter()
        .layout(body.to_string(), FontId::proportional(10.0), Color32::WHITE, inner_w);
    (galley.size().y / lh_body).round().clamp(1.0, 3.0)
}

/// Card plate height. The same measurement feeds the scroll range and the
/// paint, so the two can never disagree.
fn card_h(ui: &egui::Ui, c: &crate::api::PushCard, inner_w: f32, lh_title: f32, lh_body: f32) -> f32 {
    const PAD_Y: f32 = 8.0;
    let mut h = PAD_Y + lh_title + PAD_Y;
    if !c.body.is_empty() {
        h += 3.0 + card_body_lines(ui, &c.body, inner_w, lh_body) * lh_body;
    }
    if c.progress.is_some() {
        h += 5.0 + 12.0;
    }
    if c.action.is_some() {
        h += 5.0 + 20.0;
    }
    h
}

fn live_tab(
    ui: &mut egui::Ui,
    rect: Rect,
    p: &Palette,
    st: &mut PanelState<'_>,
    actions: &mut Vec<Action>,
) {
    // Cloned for the same reason as `perf_tab`: the scroll helper borrows
    // `st.live_scroll` mutably while the rows read the store.
    let cards: Vec<crate::api::PushCard> = st.cards.iter().cloned().collect();
    if cards.is_empty() {
        empty_state(
            ui,
            rect,
            p,
            "没有动态卡片",
            "脚本通过本地推送 API 发来的进度与提醒会显示在这里",
        );
        return;
    }
    let now = crate::inbox::now_secs();

    // Measured line heights, same rule as the inbox rows: fixed offsets were
    // tuned for 100 % DPI and smeared at 125 %.
    let lh_title = ui.fonts(|f| f.row_height(&FontId::proportional(12.5))).max(16.0);
    let lh_body = ui.fonts(|f| f.row_height(&FontId::proportional(10.0))).max(13.0);
    const PAD_X: f32 = 10.0;
    const PAD_Y: f32 = 8.0;
    const GAP: f32 = 8.0;
    let inner_w = (rect.width() - PAD_X * 2.0).max(40.0);

    let content_h: f32 = cards
        .iter()
        .map(|c| card_h(ui, c, inner_w, lh_title, lh_body))
        .sum::<f32>()
        + GAP * (cards.len().saturating_sub(1)) as f32;
    scroll(ui, rect, content_h, st.live_scroll);
    scroll_bar(ui, rect, p, content_h, st.live_scroll);
    let offset = *st.live_scroll;
    let prev_clip = ui.clip_rect();
    ui.set_clip_rect(rect);
    let mut y = rect.top() - offset;
    for card in &cards {
        let h = card_h(ui, card, inner_w, lh_title, lh_body);
        if y + h < rect.top() {
            y += h + GAP;
            continue;
        }
        if y > rect.bottom() {
            break;
        }
        let plate = Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(rect.width(), h));
        widgets::round_rect(ui, plate, 9.0, with_alpha(p.surface_alt, 0.6), Stroke::NONE);
        let hovered = ui.rect_contains_pointer(plate);
        let inner = Rect::from_min_max(
            plate.min + Vec2::new(PAD_X, PAD_Y),
            plate.max - Vec2::new(PAD_X, PAD_Y),
        );

        // Line 1: title left, remaining lifetime right. The reserve is constant
        // so the title does not jump when the hover-only dismiss button appears.
        let remain = card.expires_at().saturating_sub(now);
        let remain_label = if remain >= 60 {
            format!("{}分", remain / 60)
        } else {
            format!("{remain}秒")
        };
        widgets::text(
            ui,
            Pos2::new(inner.left(), inner.top()),
            Align2::LEFT_TOP,
            &widgets::fit_one_line(ui, &card.title, 12.5, inner.width() - 52.0),
            12.5,
            p.text,
        );
        widgets::text(
            ui,
            Pos2::new(inner.right() - 22.0, inner.top() + lh_title * 0.5),
            Align2::RIGHT_CENTER,
            &remain_label,
            9.5,
            p.text_faint,
        );
        if hovered {
            // A 13 pt glyph on an 18 pt plate (what `icon_button` draws) read
            // as a title-bar close button bolted onto a 46 pt card. The
            // dismiss affordance here is secondary to the card's own action,
            // so it gets a smaller mark, a thinner stem and a tighter plate.
            let x = Rect::from_center_size(
                Pos2::new(inner.right() - 7.0, inner.top() + lh_title * 0.5),
                Vec2::splat(16.0),
            );
            let resp = ui.allocate_rect(x, egui::Sense::click());
            if resp.hovered() {
                widgets::round_rect(ui, x, 8.0, p.accent_soft, Stroke::NONE);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let color = if resp.hovered() { p.accent } else { p.text_dim };
            widgets::glyph_weighted(ui, x.center(), 8.5, Glyph::Close, color, 1.15);
            if hover_hint(resp, "移除卡片").clicked() {
                actions.push(Action::DismissCard(card.id.clone()));
            }
        }
        let mut cy = inner.top() + lh_title;

        if !card.body.is_empty() {
            cy += 3.0;
            let lines = card_body_lines(ui, &card.body, inner_w, lh_body);
            let body_rect = Rect::from_min_size(
                Pos2::new(inner.left(), cy),
                Vec2::new(inner.width(), lines * lh_body),
            );
            let galley = ui.painter().layout(
                card.body.clone(),
                FontId::proportional(10.0),
                p.text_dim,
                inner.width(),
            );
            ui.painter()
                .with_clip_rect(body_rect)
                .galley(body_rect.min, galley, p.text_dim);
            cy += lines * lh_body;
        }

        if let Some(progress) = card.progress {
            cy += 5.0;
            let row = Rect::from_min_size(Pos2::new(inner.left(), cy), Vec2::new(inner.width(), 12.0));
            let track = Rect::from_min_max(
                Pos2::new(row.left(), row.center().y - 2.0),
                Pos2::new(row.right() - 40.0, row.center().y + 2.0),
            );
            widgets::round_rect(ui, track, 2.0, with_alpha(p.text_faint, 0.35), Stroke::NONE);
            let fill_w = track.width() * progress.clamp(0.0, 1.0);
            if fill_w > 0.5 {
                widgets::round_rect(
                    ui,
                    Rect::from_min_size(track.min, Vec2::new(fill_w, track.height())),
                    2.0,
                    p.accent,
                    Stroke::NONE,
                );
            }
            widgets::text(
                ui,
                Pos2::new(row.right(), row.center().y),
                Align2::RIGHT_CENTER,
                &format!("{}%", (progress * 100.0).round() as u32),
                9.5,
                p.text_dim,
            );
            cy += 12.0;
        }

        if let Some(action) = &card.action {
            cy += 5.0;
            let label = action.label();
            let w = ui
                .painter()
                .layout_no_wrap(label.to_string(), FontId::proportional(10.0), Color32::WHITE)
                .size()
                .x
                + 18.0;
            let btn = Rect::from_min_size(Pos2::new(inner.left(), cy), Vec2::new(w, 20.0));
            if small_button(ui, btn, p, label).clicked() {
                actions.push(Action::RunCard(card.id.clone()));
            }
        }
        y += h + GAP;
    }    ui.set_clip_rect(prev_clip);
}

// ---------------------------------------------------------------------------
// Search / launcher
// ---------------------------------------------------------------------------

fn search_tab(
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
    let resp = ui.put(
        field,
        egui::TextEdit::singleline(st.search_query)
            .hint_text("搜索应用与文件")
            .frame(false)
            .text_color(p.text)
            .margin(egui::Margin::symmetric(2.0, 3.0)),
    );
    if resp.changed() {
        actions.push(Action::SearchEdited);
    }

    let query = st.search_query.trim().to_string();
    let list = Rect::from_min_max(Pos2::new(rect.left(), search.bottom() + 8.0), rect.max);
    if query.chars().count() < 2 {
        empty_state(
            ui,
            list,
            p,
            "输入至少 2 个字符",
            "搜应用与文件名 · 内容检索请用文件检索（Alt+S）",
        );
        return;
    }

    let apps: Vec<&crate::launcher::AppEntry> = crate::launcher::matches(st.apps, &query, 5);
    let files: Vec<&crate::search::FileHit> = if st.search_ready {
        // The headless query returns up to 50; the list shows the first 20 and
        // the caption points at the full app for anything deeper.
        st.search_hits.iter().take(20).collect()
    } else {
        Vec::new()
    };
    // Name-only + snapshot age belong in the caption: this quick search is not
    // the full story, and the full story is one hotkey away.
    let index_note = match st.search_index_age {
        Some(age) if age > 3600 => {
            format!("索引 {} · 可能不含新文件 · Alt+S 刷新", age_label(age))
        }
        Some(age) => format!("索引 {} · 仅文件名 · 内容检索用 Alt+S", age_label(age)),
        None => "仅文件名 · 内容检索用 Alt+S".to_string(),
    };

    // Enter opens the first row; Ctrl+Enter reveals it. The two are detected
    // differently: egui's TextEdit only surrenders focus for a *plain* Enter
    // (`return_key` matches `Modifiers::NONE`), so with Ctrl held `lost_focus`
    // never fires — the reveal branch has to watch the key on its own. Focus
    // is handed straight back so the user can keep narrowing the query.
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let reveal = ui.input(|i| i.modifiers.command);
    if enter && (reveal || resp.lost_focus()) {
        let first = apps
            .first()
            .map(|a| a.path.to_string_lossy().to_string())
            .or_else(|| files.first().map(|f| f.path.clone()));
        if let Some(path) = first {
            actions.push(if reveal {
                Action::RevealPath(path)
            } else {
                Action::OpenPath(path)
            });
        }
        if !reveal {
            resp.request_focus();
        }
    }

    enum Row<'a> {
        Caption(&'static str),
        /// The file-result header: label left, index age / scope note right.
        Files { note: &'a str },
        App(&'a crate::launcher::AppEntry),
        File(&'a crate::search::FileHit),
        Note(&'a str),
    }
    let mut rows: Vec<Row> = Vec::new();
    if !apps.is_empty() {
        rows.push(Row::Caption("应用"));
        rows.extend(apps.iter().map(|a| Row::App(a)));
    }
    match (st.search_ready, st.search_error) {
        (false, _) => rows.push(Row::Note("搜索文件…")),
        (true, Some(err)) => rows.push(Row::Note(err)),
        (true, None) if files.is_empty() && apps.is_empty() => {
            rows.push(Row::Note("没有匹配的应用或文件 · 内容检索用 Alt+S"))
        }
        (true, None) if files.is_empty() => {}
        (true, None) => {
            rows.push(Row::Files { note: &index_note });
            rows.extend(files.iter().map(|f| Row::File(f)));
        }
    }

    let height_of = |row: &Row| -> f32 {
        match row {
            Row::Caption(_) => 16.0,
            Row::Files { .. } => 16.0,
            Row::App(_) => 26.0,
            Row::File(_) => 34.0,
            Row::Note(_) => 20.0,
        }
    };
    let content_h: f32 = rows.iter().map(height_of).sum();
    scroll(ui, list, content_h, st.search_scroll);
    scroll_bar(ui, list, p, content_h, st.search_scroll);
    let offset = *st.search_scroll;
    let prev_clip = ui.clip_rect();
    ui.set_clip_rect(list);
    let mut y = list.top() - offset;
    for row in &rows {
        let h = height_of(row);
        if y + h < list.top() {
            y += h;
            continue;
        }
        if y > list.bottom() {
            break;
        }
        let r = Rect::from_min_size(Pos2::new(list.left(), y), Vec2::new(list.width(), h));
        let mid = r.center().y;
        match row {
            Row::Caption(text) => {
                widgets::text(
                    ui,
                    Pos2::new(r.left() + 1.0, mid),
                    Align2::LEFT_CENTER,
                    text,
                    9.0,
                    p.text_faint,
                );
            }
            Row::Note(text) => {
                widgets::text(
                    ui,
                    Pos2::new(r.left() + 1.0, mid),
                    Align2::LEFT_CENTER,
                    text,
                    10.5,
                    p.text_faint,
                );
            }            Row::Files { note } => {
                widgets::text(
                    ui,
                    Pos2::new(r.left() + 1.0, mid),
                    Align2::LEFT_CENTER,
                    "文件",
                    9.0,
                    p.text_faint,
                );
                widgets::text(
                    ui,
                    Pos2::new(r.right(), mid),
                    Align2::RIGHT_CENTER,
                    note,
                    9.0,
                    p.text_faint,
                );
            }
            Row::App(app) => {
                let hovered = ui.rect_contains_pointer(r);
                if hovered {
                    widgets::round_rect(
                        ui,
                        r.shrink2(Vec2::new(0.0, 1.0)),
                        7.0,
                        with_alpha(p.surface_alt, 0.7),
                        Stroke::NONE,
                    );
                }
                widgets::text(
                    ui,
                    Pos2::new(r.left() + 1.0, mid),
                    Align2::LEFT_CENTER,
                    &widgets::fit_one_line(ui, &app.name, 12.0, r.width() - 6.0),
                    12.0,
                    p.text,
                );
                let resp = ui.interact(
                    r,
                    ui.id().with(("search-app", &app.path)),
                    egui::Sense::click(),
                );
                if resp.clicked() {
                    actions.push(Action::OpenPath(app.path.to_string_lossy().to_string()));
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                hover_hint(resp, app.path.display().to_string());
            }
            Row::File(file) => {
                let hovered = ui.rect_contains_pointer(r);
                if hovered {
                    widgets::round_rect(
                        ui,
                        r.shrink2(Vec2::new(0.0, 1.0)),
                        7.0,
                        with_alpha(p.surface_alt, 0.7),
                        Stroke::NONE,
                    );
                }
                // Ellipsised by measurement; the full path is a tooltip only
                // when one of the two lines did not fit — a hint on a row that
                // already shows everything is noise.
                let name = widgets::fit_one_line(ui, &file.name, 12.0, r.width() - 6.0);
                let mut meta = String::new();
                if let Some(size) = file.size {
                    meta.push_str(&crate::metrics::human_bytes(size));
                }
                if let Some(mtime) = file.mtime {
                    let stamp = dcx_win32::local_time_from_unix(mtime);
                    if !stamp.is_empty() {
                        if !meta.is_empty() {
                            meta.push_str(" · ");
                        }
                        meta.push_str(&stamp);
                    }
                }
                let meta_w = if meta.is_empty() {
                    0.0
                } else {
                    ui.fonts(|f| {
                        f.layout_no_wrap(meta.clone(), FontId::proportional(9.0), Color32::WHITE)
                            .size()
                            .x
                    })
                };
                let dir = widgets::fit_one_line(
                    ui,
                    &file.dir,
                    9.0,
                    (r.width() - 8.0 - meta_w).max(20.0),
                );
                widgets::text(
                    ui,
                    Pos2::new(r.left() + 1.0, r.top() + 4.0),
                    Align2::LEFT_TOP,
                    &name,
                    12.0,
                    p.text,
                );
                widgets::text(
                    ui,
                    Pos2::new(r.left() + 1.0, r.top() + 19.0),
                    Align2::LEFT_TOP,
                    &dir,
                    9.0,
                    p.text_faint,
                );
                if !meta.is_empty() {
                    widgets::text(
                        ui,
                        Pos2::new(r.right() - 1.0, r.top() + 19.0),
                        Align2::RIGHT_TOP,
                        &meta,
                        9.0,
                        with_alpha(p.text_faint, 0.85),
                    );
                }
                let resp = ui.interact(
                    r,
                    ui.id().with(("search-file", &file.path)),
                    egui::Sense::click(),
                );
                if resp.clicked() {
                    actions.push(Action::OpenPath(file.path.clone()));
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if name.ends_with('…') || dir.ends_with('…') {
                    hover_hint(resp, file.path.clone());
                }
            }
        }
        y += h;
    }    ui.set_clip_rect(prev_clip);
}

// ---------------------------------------------------------------------------
// Performance
// ---------------------------------------------------------------------------

fn perf_tab(
    ui: &mut egui::Ui,
    rect: Rect,
    p: &Palette,
    st: &mut PanelState<'_>,
    actions: &mut Vec<Action>,
) {
    // Cloned because the rows below borrow `st.perf_scroll` mutably while they
    // read the snapshot; it is a handful of numbers and a small `Vec`.
    let snap = st.snap.clone();
    let disk_on = st.cfg.metrics.disk;
    let ring_d = 66.0;
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

    // Everything under the two cards is a single scrolling column. It has to
    // scroll because the number of rows is no longer fixed: one row per mounted
    // volume means a machine with four disks needs ~60 pt more than a laptop
    // with one, and silently dropping the last volumes is worse than a wheel
    // turn.
    enum Row {
        Line { label: String, value: String },
        Caption(String),
        Volume { name: String, value: String, frac: f32, level: u8 },
        Error(String),
        /// Desktop backlog + collection size, with the "open library" button.
        Library { desktop: usize, library: usize },
    }
    let mut rows: Vec<Row> = Vec::new();
    rows.push(Row::Line {
        label: "内存".to_string(),
        value: format!(
            "{} / {}",
            crate::metrics::human_bytes(snap.mem_used),
            crate::metrics::human_bytes(snap.mem_total)
        ),
    });
    rows.push(Row::Line {
        label: "网络".to_string(),
        value: format!(
            "↓ {} · ↑ {}",
            crate::metrics::human_rate(snap.net_down_bps),
            crate::metrics::human_rate(snap.net_up_bps)
        ),
    });
    if disk_on {
        rows.push(Row::Line {
            label: "磁盘读写".to_string(),
            value: format!(
                "↓ {} · ↑ {}",
                crate::metrics::human_rate(snap.disk_read_bps),
                crate::metrics::human_rate(snap.disk_write_bps)
            ),
        });
    }
    rows.push(Row::Line {
        label: "开机".to_string(),
        value: format!("{} 小时", crate::sys::uptime_secs() / 3600),
    });
    if disk_on {
        rows.push(Row::Caption("磁盘容量".to_string()));
        if snap.volumes.is_empty() {
            rows.push(Row::Line {
                label: "未找到可读的磁盘".to_string(),
                value: String::new(),
            });
        }
        for v in &snap.volumes {
            let name = format!(
                "{}: {}",
                v.letter,
                if v.label.is_empty() {
                    "本地磁盘"
                } else {
                    v.label.as_str()
                }
            );
            rows.push(Row::Volume {
                name,
                value: format!(
                    "{} / {}",
                    crate::metrics::human_bytes(v.used()),
                    crate::metrics::human_bytes(v.total)
                ),
                frac: v.percent() / 100.0,
                level: crate::metrics::level(v.percent()),
            });
        }
    }
    if let Some(err) = &st.metrics_error {
        rows.push(Row::Error(err.clone()));
    }
    if let Some(lib) = st.library {
        rows.push(Row::Library {
            desktop: lib.desktop,
            library: lib.library,
        });
    }

    // 20 pt a row, 16 pt for a caption: with the cards at 66 pt this leaves room
    // for three volumes in the default 292 pt panel, and the column scrolls when
    // the machine has more. Rows were 24 pt when the list had a fixed length;
    // a variable-length list cannot afford that.
    let height_of = |row: &Row| match row {
        Row::Caption(_) => 16.0,
        Row::Library { .. } => 24.0,
        _ => 20.0,
    };
    let content_h: f32 = rows.iter().map(height_of).sum();
    let view = Rect::from_min_max(Pos2::new(rect.left(), top.bottom() + 10.0), rect.max);
    scroll(ui, view, content_h, st.perf_scroll);
    scroll_bar(ui, view, p, content_h, st.perf_scroll);
    let offset = *st.perf_scroll;

    let prev_clip = ui.clip_rect();
    ui.set_clip_rect(view);
    let mut y = view.top() - offset;
    for row in &rows {
        let h = height_of(row);
        if y + h > view.top() && y < view.bottom() {
            paint(ui, view, y, h, row, p, actions);
        }
        y += h;
    }    ui.set_clip_rect(prev_clip);

    /// One line of the performance column. Labels are proportional, every
    /// number is monospace so a changing value never shifts the ones beside it,
    /// and a volume puts a bar in the gap between the two: the same order as
    /// the metric rows, so the whole column reads as one table instead of three
    /// widget styles.
    fn paint(
        ui: &mut egui::Ui,
        view: Rect,
        y: f32,
        h: f32,
        row: &Row,
        p: &Palette,
        actions: &mut Vec<Action>,
    ) {
    let mid = y + h * 0.5;
    match row {
        Row::Caption(text) => {
            widgets::text(
                ui,
                Pos2::new(view.left() + 1.0, mid),
                Align2::LEFT_CENTER,
                text,
                9.0,
                p.text_faint,
            );
        }
        Row::Line { label, value } => {
            widgets::text(
                ui,
                Pos2::new(view.left() + 1.0, mid),
                Align2::LEFT_CENTER,
                label,
                11.0,
                p.text_dim,
            );
            if !value.is_empty() {
                widgets::mono_text(
                    ui,
                    Pos2::new(view.right() - 1.0, mid),
                    Align2::RIGHT_CENTER,
                    value,
                    10.5,
                    p.text,
                );
            }
        }
        Row::Error(text) => {
            widgets::text(
                ui,
                Pos2::new(view.left() + 1.0, mid),
                Align2::LEFT_CENTER,
                text,
                10.0,
                p.danger,
            );
        }
        Row::Library { desktop, library } => {
            widgets::text(
                ui,
                Pos2::new(view.left() + 1.0, mid),
                Align2::LEFT_CENTER,
                "桌面待整理",
                11.0,
                p.text_dim,
            );
            let label = "打开收纳库";
            let btn_w = ui
                .painter()
                .layout_no_wrap(label.to_string(), FontId::proportional(10.0), Color32::WHITE)
                .size()
                .x
                + 16.0;
            let btn = Rect::from_min_size(
                Pos2::new(view.right() - btn_w, mid - 9.0),
                Vec2::new(btn_w, 18.0),
            );
            widgets::mono_text(
                ui,
                Pos2::new(btn.left() - 8.0, mid),
                Align2::RIGHT_CENTER,
                &format!("{desktop} 项 · 收纳库 {library} 项"),
                10.0,
                p.text,
            );
            if small_button(ui, btn, p, label).clicked() {
                actions.push(Action::OpenLibrary);
            }
        }
        Row::Volume {
            name,
            value,
            frac,
            level,
        } => {
            let name_galley =
                ui.painter()
                    .layout_no_wrap(name.clone(), FontId::proportional(10.5), p.text_dim);
            let name_w = name_galley.size().x;
            ui.painter().galley(
                Pos2::new(view.left() + 1.0, mid - name_galley.size().y * 0.5),
                name_galley,
                p.text_dim,
            );
            let value_galley =
                ui.painter()
                    .layout_no_wrap(value.clone(), FontId::monospace(10.0), p.text);
            let value_w = value_galley.size().x;
            ui.painter().galley(
                Pos2::new(view.right() - 1.0 - value_w, mid - value_galley.size().y * 0.5),
                value_galley,
                p.text,
            );
            let bar_left = view.left() + 1.0 + name_w + 10.0;
            let bar_right = view.right() - 1.0 - value_w - 10.0;
            if bar_right - bar_left > 18.0 {
                widgets::bar(
                    ui,
                    Rect::from_min_max(
                        Pos2::new(bar_left, mid - 2.0),
                        Pos2::new(bar_right, mid + 2.0),
                    ),
                    *frac,
                    p.level(*level),
                    p.track,
                );
            }
        }
    }
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
    if max > 0.0 && ui.rect_contains_pointer(view.expand2(Vec2::new(12.0, 0.0))) {
        let delta = ui.input(|i| i.raw_scroll_delta.y);
        if delta.abs() > 0.01 {
            *offset = (*offset - delta).clamp(0.0, max);
            ui.ctx().request_repaint();
        }
    }
    *offset
}

/// Thin scrollbar hugging the panel's right edge — draggable, with
/// click-on-track jump.
///
/// The list's own rectangle stops at the content padding (14 pt in from the
/// panel edge); the bar lives in that gutter instead of at `view.right()`, so
/// it reads as part of the panel frame rather than as a line drawn over the
/// rows. It must be called **before** the rows are painted (and before the
/// list clip is set): a drag then moves the thumb on the same frame that
/// received it, instead of one frame behind the pointer.
fn scroll_bar(ui: &mut egui::Ui, view: Rect, p: &Palette, content_h: f32, offset: &mut f32) {
    let max = (content_h - view.height()).max(0.0);
    if max <= 0.5 {
        return;
    }
    let track = Rect::from_min_max(
        Pos2::new(view.right() + 8.0, view.top()),
        Pos2::new(view.right() + 11.0, view.bottom()),
    );
    let frac = (view.height() / content_h).clamp(0.08, 1.0);
    let h = (track.height() * frac).max(18.0);
    let travel = (track.height() - h).max(1.0);
    let thumb = Rect::from_min_size(
        Pos2::new(track.left(), track.top() + travel * (*offset / max).clamp(0.0, 1.0)),
        Vec2::new(track.width(), h),
    );

    // A 3 pt sliver cannot be grabbed; the hit zone is 4 pt wider on each
    // side, and a click anywhere on the track jumps there.
    let hit = Rect::from_min_max(
        Pos2::new(track.left() - 4.0, track.top()),
        Pos2::new(track.right() + 4.0, track.bottom()),
    );
    let resp = ui.interact(
        hit,
        ui.id().with(("scroll-bar", view.top().to_bits(), view.left().to_bits())),
        egui::Sense::click_and_drag(),
    );
    if resp.dragged() {
        let dy = ui.input(|i| i.pointer.delta().y);
        if dy != 0.0 {
            *offset = (*offset + dy * max / travel).clamp(0.0, max);
            ui.ctx().request_repaint();
        }
    } else if resp.clicked() {
        if let Some(pos) = ui.ctx().pointer_interact_pos() {
            // Clicking the thumb itself must not nudge the offset; only a
            // click on bare track jumps.
            if !thumb.contains(pos) {
                let target = ((pos.y - track.top() - h * 0.5) / travel).clamp(0.0, 1.0);
                *offset = target * max;
                ui.ctx().request_repaint();
            }
        }
    }
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }

    let color = if resp.dragged() {
        with_alpha(p.accent, 0.9)
    } else if resp.hovered() {
        with_alpha(p.text_dim, 0.85)
    } else {
        with_alpha(p.text_faint, 0.55)
    };
    widgets::round_rect(ui, thumb, 1.5, color, Stroke::NONE);
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

/// Outline that identifies a channel at 13.5 pt. Silhouette only — see the
/// channel marks in [`Glyph`]: at this size any inner detail turns to mush, so
/// the four shapes differ by outline (two bubbles / one round bubble /
/// briefcase / envelope) and never by colour.
fn channel_glyph(source: Source) -> Glyph {
    match source {
        Source::Qq => Glyph::Bubble,
        Source::WeChat => Glyph::BubbleTwo,
        // A briefcase, not a second chat bubble: 企业微信 is the *work* channel,
        // and two bubble outlines side by side were indistinguishable at 15 px.
        Source::WeCom => Glyph::Briefcase,
        Source::Mail => Glyph::Mail,
        Source::System => Glyph::Bell,
    }
}

/// Right-hand status of a channel, with the colour it is written in.
///
/// Now that the status is no longer a row of its own this string only appears
/// in the chip's tooltip; it is kept in one place so the tooltip cannot drift
/// from the dot, which says the same thing in colour.
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

/// Tooltip sized to its content, capped to the island window.
///
/// egui's default tooltip width is 600 pt — wider than the whole panel — so a
/// long path was laid out 600 pt wide and then cut off at the window edge.
/// Inside this window a hint must wrap at the panel width instead.
fn hover_hint(resp: egui::Response, text: impl Into<egui::WidgetText>) -> egui::Response {
    let max = (resp.ctx.screen_rect().width() - 24.0).max(140.0);
    resp.on_hover_ui(|ui| {
        ui.set_max_width(max);
        ui.label(text);
    })
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
