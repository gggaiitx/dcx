//! The single eframe loop of `dcx-island.exe`.
//!
//! # Window model
//!
//! One window, one egui context. When collapsed the window **is** the capsule
//! (`dcx_island::ui::compact`); expanding grows it downwards and symmetrically
//! sideways, keeping the capsule at the same screen position. The window is
//! therefore never larger than what it draws, which is the only reliable way to
//! avoid a transparent band swallowing desktop clicks — `HTTRANSPARENT` does not
//! forward to other processes, and a window region on a DWM-composited surface
//! leaves stale pixels (both were tried in `dcx-box`).
//!
//! # Who writes what
//!
//! The UI thread owns `island.toml`, `island_tasks.json` and
//! `island_clipboard.json`. Background threads (sampling, clipboard capture,
//! QQ, mail) only push values into channels. That is deliberate: the config
//! lock blocks for up to 3 s, and a second writer would eventually drop a frame.
//!
//! # Frame budget
//!
//! Per frame: geometry, painting, at most one `SetWindowPos`. Network calls are
//! always on a thread; `GetIfTable2` runs in the sampler. A repaint is requested
//! no faster than the next clock second (or 250 ms while the panel is open), so
//! an idle island costs almost no CPU.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant, SystemTime};

use egui::{Pos2, Rect, Vec2};
use winit::platform::windows::EventLoopBuilderExtWindows;

use dcx_island_config::Config;

use crate::ui::panel::{Action, PanelState, TextureCache};
use crate::ui::{theme, Tab};
use crate::{clipboard, clock, dispatch, inbox, metrics, notify, sys, tasks, win};

/// Must equal `AppEntry.window_title` in `dcx-host/src/apps.rs` byte for byte:
/// the host uses `FindWindowW` to stop a manually started instance and to
/// decide whether the app is running.
pub const WIN_TITLE: &str = "桌面信息岛";

const GAP: f32 = 8.0;
const EDGE_INSET: f32 = 8.0;
/// How long a reminder pulses the pill. Long enough to notice while working in
/// another window, short enough that a topmost overlay stops drawing attention.
const REMINDER_SECS: f32 = 12.0;
const CONFIG_POLL: Duration = Duration::from_millis(900);
const FS_POLL: Duration = Duration::from_millis(700);
const SAVE_DEBOUNCE: Duration = Duration::from_millis(900);
const FOCUS_GRACE: Duration = Duration::from_millis(260);
const MARKER_POLL: Duration = Duration::from_millis(220);

static STARTED: AtomicBool = AtomicBool::new(false);

pub fn spawn() -> Option<std::thread::JoinHandle<()>> {
    if STARTED.swap(true, Ordering::SeqCst) {
        return None;
    }
    Some(std::thread::spawn(|| {
        // A disabled island must not create a window at all: the host may have
        // started us from a stale `apps.toml`, and "enabled = false" is the
        // switch the settings centre writes first.
        let cfg = dcx_island_config::load();
        if !cfg.enabled {
            win::trace("disabled in island.toml; not creating a window");
            STARTED.store(false, Ordering::SeqCst);
            return;
        }
        let result = eframe::run_native(
            WIN_TITLE,
            native_options(),
            Box::new(|cc| {
                theme::setup_fonts(&cc.egui_ctx);
                Ok(Box::new(Island::new(cfg)))
            }),
        );
        if let Err(e) = result {
            win::trace(&format!("eframe failed: {e}"));
            STARTED.store(false, Ordering::SeqCst);
        }
    }))
}

fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([268.0, 34.0])
            .with_min_inner_size([120.0, 24.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_taskbar(false)
            .with_always_on_top()
            // Created off-screen and moved on the first frame: showing at the
            // default `centered` position first would flash a capsule in the
            // middle of the screen on every start.
            .with_position(egui::pos2(-32_000.0, -32_000.0))
            .with_icon(theme::app_icon()),
        event_loop_builder: Some(Box::new(|b| {
            b.with_any_thread(true);
        })),
        centered: false,
        ..Default::default()
    }
}

struct Geo {
    /// Window rectangle in physical pixels.
    window_px: [i32; 4],
    /// Capsule in window-local points.
    capsule: Rect,
    /// Panel in window-local points; `None` while collapsed.
    panel: Option<Rect>,
    scale: f32,
}

enum MailMsg {
    Sent(Vec<&'static str>),
    Failed(String),
}

struct Island {
    cfg: Config,
    cfg_stamp: Option<SystemTime>,
    cfg_at: Instant,

    win: win::Window,
    /// Monitor the island lives on, cached while it is visible.
    ///
    /// `MonitorFromWindow` on the parked (off-screen at -32000) window returns
    /// the *nearest* monitor, which is the other screen here — so recomputing it
    /// while parked flip-flopped park state every poll: hidden, visible,
    /// hidden…
    mon: win::Monitor,
    scale: f32,
    pal: theme::Palette,
    pal_at: Option<Instant>,

    open: bool,
    /// The panel was opened without taking focus (a reminder). It must not be
    /// closed by the focus-loss rule and does not steal the keyboard.
    passive: bool,
    anim: f32,
    hovered: bool,
    parked: bool,
    focus_lost: Option<Instant>,
    last_input: Instant,
    last_fs: Instant,
    last_marker: Instant,
    last_stamp: u128,
    /// What was last written to the `ui` table, so a close does not rewrite the
    /// document with identical values.
    ui_saved: (String, f32, f32),

    sampler: metrics::Sampler,
    dispatcher: dispatch::Dispatcher,
    send_at: Option<Instant>,
    send_rx: Option<Receiver<MailMsg>>,
    send_note: Option<(String, Instant)>,

    tasks: tasks::Store,
    clip: clipboard::Store,
    clip_listener: Option<clipboard::Listener>,
    inbox: inbox::Inbox,
    notifier: notify::Notifier,

    textures: TextureCache,
    tab: Tab,
    task_input: String,
    task_time: String,
    task_repeat: tasks::Repeat,
    /// Minutes between reminders for the task being composed. 0 = a single
    /// reminder. Purely composer state; the value is copied into the task on
    /// submit and then reset, so it never drifts from what was actually saved.
    task_every: u32,
    clip_query: String,
    /// Wheel offset of the two scrollable lists, in points. Kept per tab so
    /// switching tabs does not lose the position.
    clip_scroll: f32,
    inbox_scroll: f32,

    today: String,
    last_secs: u32,
    last_tick: Instant,
    dirty_at: Option<Instant>,

    toast: Option<(String, Instant)>,
    reminder: Option<(String, Instant)>,
}

impl Island {
    fn new(cfg: Config) -> Island {
        let today = clock::Clock::now().iso_date();
        let mut tasks = tasks::Store::load(dcx_island_config::tasks_path());
        if cfg.tasks.carry_over {
            let moved = tasks.carry_over(&today);
            if moved > 0 {
                win::trace(&format!("carried over {moved} tasks"));
            }
        }
        let mut clip = clipboard::Store::load(
            dcx_island_config::clipboard_path(),
            dcx_island_config::image_dir(),
        );
        clip.trim(&cfg.clipboard);
        let listener = if cfg.clipboard.enabled {
            clipboard::start(cfg.clipboard.capture_images)
        } else {
            None
        };
        clipboard::set_capture_images(cfg.clipboard.capture_images);

        let mut boxed = Island {
            cfg_stamp: file_stamp(&dcx_island_config::config_path()),
            cfg_at: Instant::now(),
            win: win::Window::new(),
            mon: win::monitor(0),
            scale: 1.0,
            pal: theme::Palette::system(),
            pal_at: None,
            open: false,
            passive: false,
            anim: 0.0,
            hovered: false,
            parked: false,
            focus_lost: None,
            last_input: Instant::now(),
            last_fs: Instant::now(),
            last_marker: Instant::now(),
            last_stamp: 0,
            ui_saved: (
                cfg.ui.last_tab.clone(),
                cfg.ui.panel_width,
                cfg.ui.panel_height,
            ),
            sampler: metrics::Sampler::start(cfg.metrics.interval_secs),
            dispatcher: dispatch::Dispatcher::default(),
            send_at: None,
            send_rx: None,
            send_note: None,
            tasks,
            clip,
            clip_listener: listener,
            inbox: inbox::Inbox::new(cfg.inbox.keep),
            notifier: notify::Notifier::start(&cfg, sys::cred_read(dispatch::CRED_IMAP)),
            textures: TextureCache::new(),
            tab: Tab::from_key(&cfg.ui.last_tab),
            task_input: String::new(),
            task_time: String::new(),
            task_repeat: tasks::Repeat::Once,
            task_every: 0,
            clip_query: String::new(),
            clip_scroll: 0.0,
            inbox_scroll: 0.0,
            today,
            last_secs: 0,
            last_tick: Instant::now(),
            dirty_at: None,
            toast: None,
            reminder: None,
            cfg,
        };
        boxed.notifier.seed_states(&mut boxed.inbox, &boxed.cfg);
        boxed
    }

    fn ensure(&mut self) {
        if self.win.ensure(WIN_TITLE) {
            let scale = self.win.scale();
            if (scale - self.scale).abs() > 0.01 {
                win::trace(&format!("scale {scale}"));
                self.scale = scale;
            }
            // Only while on screen: see the `mon` field comment.
            if !self.parked {
                self.mon = self.win.monitor();
            }
        }
    }

    // -- periodic work -----------------------------------------------------

    fn housekeeping(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if now.duration_since(self.last_tick) >= Duration::from_millis(200) {
            self.last_tick = now;
            self.poll_config(ctx);
            self.poll_marker(ctx);
            self.drain_clipboard(ctx);
            if self.notifier.poll(&mut self.inbox) {
                ctx.request_repaint();
            }
            self.poll_send();
            if self.dirty_at.map(|t| t.elapsed() >= SAVE_DEBOUNCE).unwrap_or(false) {
                self.save_all();
            }
        }

        let clock_now = clock::Clock::now();
        if clock_now.secs_of_day() != self.last_secs || self.today != clock_now.iso_date() {
            self.last_secs = clock_now.secs_of_day();
            self.on_tick(ctx, &clock_now);
        }

        if now.duration_since(self.last_fs) >= FS_POLL {
            self.last_fs = now;
            self.update_park_state();
        }

        if self.pal_at.map(|t| t.elapsed() >= Duration::from_secs(3)).unwrap_or(true) {
            self.pal_at = Some(now);
            self.pal = theme::Palette::system();
        }

        let auto = self.cfg.behavior.auto_collapse_secs;
        if self.open && auto > 0 {
            if now.duration_since(self.last_input) >= Duration::from_secs(auto) {
                self.set_open(false, ctx);
            }
        }
    }

    fn on_tick(&mut self, ctx: &egui::Context, c: &clock::Clock) {
        if self.today != c.iso_date() {
            self.today = c.iso_date();
            if self.cfg.tasks.carry_over {
                self.tasks.carry_over(&self.today);
            }
        }
        // Reminders: the store marks them fired, so this is safe to call every
        // second and survives a restart without a second alert.
        let due = self.tasks.take_due(&self.today, c.secs_of_day());
        if let Some(first) = due.first() {
            let text = dispatch::reminder_text(&first.title, first.at);
            self.reminder = Some((first.title.clone(), Instant::now()));
            if self.cfg.tasks.sound {
                sys::beep();
            }
            self.toast = Some((format!("提醒：{}", first.title), Instant::now()));
            if self.cfg.tasks.dispatch_due {
                self.queue_dispatch(text);
            }
            // The island flashes; it does not open. Raising a topmost panel on
            // every reminder stole the desktop for twelve seconds and made the
            // reminder compete with whatever the user was doing. The pill shows
            // the task name, pulses its border and beeps — the panel is one
            // click away if the details are wanted.
            self.dirty_at = Some(Instant::now());
        }
        if self
            .reminder
            .as_ref()
            .map(|(_, t)| t.elapsed().as_secs_f32() > REMINDER_SECS)
            .unwrap_or(false)
        {
            self.reminder = None;
            // The panel was raised by a reminder and never touched; take it
            // back down instead of leaving a topmost window on screen.
            if self.open && self.passive {
                self.set_open(false, ctx);
            }
        }
        if self.toast.as_ref().map(|(_, t)| t.elapsed().as_secs() > 4).unwrap_or(false) {
            self.toast = None;
        }
        ctx.request_repaint();
    }

    fn poll_config(&mut self, ctx: &egui::Context) {
        if self.cfg_at.elapsed() < CONFIG_POLL {
            return;
        }
        self.cfg_at = Instant::now();
        let stamp = file_stamp(&dcx_island_config::config_path());
        if stamp == self.cfg_stamp {
            return;
        }
        self.cfg_stamp = stamp;
        let old = self.cfg.clone();
        let next = dcx_island_config::load();
        // The settings centre writes `enabled` into both documents; if it is
        // off here, the user turned the island off while we were running.
        if !next.enabled {
            win::trace("disabled by config; closing");
            self.save_all();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if next.metrics.interval_secs != old.metrics.interval_secs {
            self.sampler.set_interval(next.metrics.interval_secs);
        }
        if next.clipboard.enabled != old.clipboard.enabled {
            if next.clipboard.enabled {
                self.clip_listener = clipboard::start(next.clipboard.capture_images);
            } else {
                self.clip_listener = None;
            }
        }
        clipboard::set_capture_images(next.clipboard.capture_images);
        if next.inbox.keep != old.inbox.keep {
            self.inbox.set_keep(next.inbox.keep);
        }
        if next.inbox != old.inbox || next.mail != old.mail {
            // Channels own threads with their own permission state; rebuilding
            // the notifier is cheaper and more predictable than restarting each
            // probe in place.
            self.notifier = notify::Notifier::start(&next, sys::cred_read(dispatch::CRED_IMAP));
            self.notifier.seed_states(&mut self.inbox, &next);
        }
        if next.ui.last_tab != old.ui.last_tab {
            self.tab = Tab::from_key(&next.ui.last_tab);
        }
        if next.clipboard.max_items != old.clipboard.max_items
            || next.clipboard.max_image_mb != old.clipboard.max_image_mb
        {
            self.clip.trim(&next.clipboard);
        }
        self.cfg = next;
        ctx.request_repaint();
    }

    fn poll_marker(&mut self, ctx: &egui::Context) {
        if self.last_marker.elapsed() < MARKER_POLL {
            return;
        }
        self.last_marker = Instant::now();
        let Some(stamp) = win::show_marker_time() else {
            return;
        };
        let ms = stamp
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        if ms <= self.last_stamp {
            return;
        }
        self.last_stamp = ms;
        // The host's hotkey touches the marker; treat it as a toggle so the same
        // key opens and closes the panel.
        let target = !self.open;
        self.set_open(target, ctx);
        win::clear_show_marker();
    }

    fn drain_clipboard(&mut self, ctx: &egui::Context) {
        let Some(listener) = &self.clip_listener else {
            return;
        };
        let mut added = 0;
        while let Ok(raw) = listener.rx.try_recv() {
            // `take`, not a plain read: the marker must never outlive the event
            // it was set for, or the next genuine copy would be mistaken for
            // our own write-back and silently dropped.
            if let Some(seq) = self.clip.self_write.take() {
                if seq == sys::clipboard_sequence() {
                    // Our own write-back; do not record it again.
                    continue;
                }
            }
            if self.clip.push(raw, &self.cfg.clipboard).is_some() {
                added += 1;
            }
        }
        if added > 0 {
            self.dirty_at = Some(Instant::now());
            ctx.request_repaint();
        }
    }

    fn poll_send(&mut self) {
        let Some(rx) = &self.send_rx else { return };
        match rx.try_recv() {
            Ok(MailMsg::Sent(channels)) => {
                self.send_note = Some((format!("已推送：{}", channels.join("、")), Instant::now()));
                self.send_rx = None;
            }
            Ok(MailMsg::Failed(e)) => {
                self.send_note = Some((format!("推送失败：{e}"), Instant::now()));
                self.send_rx = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.send_rx = None,
        }
    }

    fn queue_dispatch(&mut self, text: String) {
        if self.send_rx.is_some() {
            return;
        }
        let gap = Duration::from_secs(self.cfg.dispatch.min_gap_secs);
        if let Some(at) = self.send_at {
            if at.elapsed() < gap {
                win::trace("dispatch skipped: rate limited");
                return;
            }
        }
        self.send_at = Some(Instant::now());
        let cfg = self.cfg.clone();
        let (tx, rx) = channel();
        self.send_rx = Some(rx);
        std::thread::spawn(move || {
            let msg = match dispatch::push_once(&cfg, &text) {
                Ok(channels) => MailMsg::Sent(channels),
                Err(e) => MailMsg::Failed(e),
            };
            let _ = tx.send(msg);
        });
    }

    fn update_park_state(&mut self) {
        if !self.cfg.behavior.hide_fullscreen {
            if self.parked {
                self.parked = false;
            }
            return;
        }
        let mon = self.mon;
        // The strip the capsule occupies, in physical pixels. Computed from the
        // monitor rather than from the window rectangle: while parked the window
        // sits off-screen and its rectangle says nothing about where the island
        // would be.
        //
        // `None` while the panel is open: the island is topmost and the user is
        // looking at it, so "some window is behind the capsule" is not a reason
        // to vanish. A real full-screen app is still caught by the shell state.
        let strip = if self.open {
            None
        } else {
            let scale = self.scale.max(1.0);
            let mon_w = (mon.rect[2] - mon.rect[0]) as f32;
            let cap_w = (self.cfg.behavior.capsule_width * scale).min(mon_w - 24.0 * scale);
            let cap_h = self.cfg.behavior.capsule_height * scale;
            let cx = (mon.rect[0] + mon.rect[2]) as f32 * 0.5;
            let top = mon.work[1] + (EDGE_INSET * scale).round() as i32;
            Some([
                (cx - cap_w * 0.5).round() as i32,
                top,
                (cx + cap_w * 0.5).round() as i32,
                top + cap_h.round() as i32,
            ])
        };
        let full = win::foreground_is_fullscreen(&mon, self.win.hwnd, strip);
        if full != self.parked {
            win::trace(&format!(
                "fullscreen={full} → {}",
                if full { "parked" } else { "shown" }
            ));
            self.parked = full;
        }
    }

    fn save_all(&mut self) {
        self.tasks.save_if_dirty();
        self.clip.save_if_dirty();
        self.dirty_at = None;
        if let Some(e) = &self.tasks.error {
            win::trace(&format!("tasks save failed: {e}"));
        }
        if let Some(e) = &self.clip.error {
            win::trace(&format!("clipboard save failed: {e}"));
        }
    }

    fn set_open(&mut self, open: bool, ctx: &egui::Context) {
        self.set_open_with(open, ctx, true);
    }

    /// `activate = false` shows the panel without taking the keyboard: a
    /// reminder must not pull focus out of whatever the user is typing in.
    fn set_open_with(&mut self, open: bool, ctx: &egui::Context, activate: bool) {
        if self.open == open {
            if open && activate {
                self.passive = false;
            }
            return;
        }
        self.open = open;
        self.last_input = Instant::now();
        if open {
            self.focus_lost = None;
            self.passive = !activate;
            // Focus is required for the task input; the window is created
            // without WS_EX_NOACTIVATE for exactly this reason.
            if activate {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        } else {
            self.passive = false;
            self.save_all();
            self.persist_ui();
        }
        self.dirty_at = Some(Instant::now());
        ctx.request_repaint();
    }

    /// Write the island-owned half of `island.toml` (last tab, panel size).
    ///
    /// Only the `ui` table, and only when it actually changed: the settings
    /// centre owns every other table, and a full save here used to roll back a
    /// switch the user had changed less than `CONFIG_POLL` ago.
    fn persist_ui(&mut self) {
        let tab = self.tab.key().to_string();
        let (w, h) = (self.cfg.ui.panel_width, self.cfg.ui.panel_height);
        let clean = self.ui_saved.0 == tab
            && (self.ui_saved.1 - w).abs() < 0.5
            && (self.ui_saved.2 - h).abs() < 0.5;
        if clean {
            return;
        }
        match dcx_island_config::patch_ui(|ui| {
            ui.last_tab = tab.clone();
            ui.panel_width = w;
            ui.panel_height = h;
        }) {
            Ok(()) => {
                self.ui_saved = (tab, w, h);
                self.cfg.ui.last_tab = self.ui_saved.0.clone();
                // Our own write must not look like a change made elsewhere,
                // otherwise the next poll reloads the document for nothing.
                self.cfg_stamp = file_stamp(&dcx_island_config::config_path());
            }
            Err(e) => {
                win::trace(&format!("ui save failed: {e}"));
                // Force the next call to try again instead of believing the
                // values are on disk.
                self.ui_saved.0.clear();
            }
        }
    }

    // -- geometry ----------------------------------------------------------

    fn compute_geo(&self) -> Geo {
        let scale = self.scale.max(1.0);
        let mon = self.mon;
        let mon_w = (mon.rect[2] - mon.rect[0]) as f32;
        let mon_h = (mon.rect[3] - mon.rect[1]) as f32;

        let capsule_w = (self.cfg.behavior.capsule_width * scale).min(mon_w - 24.0 * scale);
        let capsule_h = self.cfg.behavior.capsule_height * scale;
        let panel_w = (self.cfg.ui.panel_width * scale).min(mon_w - 16.0 * scale);
        let panel_h = (self.cfg.ui.panel_height * scale).min(mon_h - 120.0 * scale);

        // The window follows the same easing as the panel fade. Growing it by
        // 500 px in a single `SetWindowPos` makes DWM recompose the whole
        // layered surface at once — that is the flash seen when clicking the
        // capsule. Growing it over the same ~100 ms as the fade reads as one
        // deliberate unfold.
        let e = self.anim.clamp(0.0, 1.0);
        let win_w = (capsule_w + (panel_w - capsule_w) * e).max(capsule_w);
        // The 8 pt gap between capsule and panel closes as the panel unfolds:
        // at the end the capsule row *is* the top of the card, not a pill
        // floating above it. The window height ignores the gap entirely — it
        // grows by the body only — so collapsed it is exactly the capsule.
        let gap = GAP * (1.0 - e);
        let win_h = capsule_h + panel_h * e;

        let center_x = (mon.rect[0] + mon.rect[2]) as f32 * 0.5;
        let x = (center_x - win_w * 0.5).round() as i32;
        let y = mon.work[1] + (EDGE_INSET * scale).round() as i32;

        let w_pt = win_w / scale;
        // Snapped to whole physical pixels: an off-by-half window edge blurs
        // every glyph along it, and the capsule sits directly on the top edge.
        let capsule = win::snap(
            Rect::from_min_size(
                Pos2::new((w_pt - capsule_w / scale) * 0.5, 0.0),
                Vec2::new(capsule_w / scale, capsule_h / scale),
            ),
            scale,
        );
        // The panel keeps its final geometry and is clipped by the growing
        // window, so the content does not re-flow on every animation frame.
        let panel = if (self.open && !self.parked) || e > 0.02 {
            Some(win::snap(
                Rect::from_min_size(
                    Pos2::new((w_pt - panel_w / scale) * 0.5, capsule_h / scale + gap),
                    Vec2::new(panel_w / scale, panel_h / scale),
                ),
                scale,
            ))
        } else {
            None
        };

        Geo {
            window_px: [x, y, x + win_w.round() as i32, y + win_h.round() as i32],
            capsule,
            panel,
            scale,
        }
    }

    /// Advance the open/close easing. Driven from `update` (before geometry) so
    /// the window rectangle and the panel fade always use the same value.
    fn advance_anim(&mut self) {
        let target = if self.open && !self.parked { 1.0 } else { 0.0 };
        self.anim += (target - self.anim) * 0.32;
        if (target - self.anim).abs() < 0.008 {
            self.anim = target;
        }
    }

    fn draw(&mut self, ctx: &egui::Context, geo: &Geo) {
        let pal = self.pal;
        let caps = self.capsule_state();
        let anim = self.anim;

        let mut actions: Vec<Action> = Vec::new();
        // `Response` is produced by `Ui::interact`, which needs a live `Ui`.
        // The panel closure already borrows `self`, so the resize grip is
        // interacted with there and only consumed after the closure returns.
        let mut resize_resp: Option<egui::Response> = None;
        let cap_rect = geo.capsule;
        let panel_rect = geo.panel;

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(egui::Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::same(0.0)),
            )
            .show(ctx, |ui| {
                // Size of the window in points. `window_px` is a rectangle
                // [left, top, right, bottom], not a size: on a monitor placed
                // to the left of the primary one `right` is even negative.
                let full = Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(
                        (geo.window_px[2] - geo.window_px[0]) as f32 / geo.scale,
                        (geo.window_px[3] - geo.window_px[1]) as f32 / geo.scale,
                    ),
                );
                ui.set_min_size(full.size());

                // One surface for the whole window, from the first frame of the
                // unfold onwards.
                //
                // The capsule used to be its own pill with its own border and a
                // see-through 8 pt gap under it, so the two halves of the app
                // looked like two objects stacked on each other. Now the card is
                // the window: it starts life as the pill (same radius, same
                // colour, the pill drawn on top of it is indistinguishable) and
                // ends as the panel, with no seam in between.
                //
                // Painting the card over the whole window also leaves the
                // desktop visible only outside the rounded corners, which is
                // unavoidable for a rounded window and invisible in practice.
                let radius = 14.0 + (cap_rect.height() * 0.5 - 14.0) * (1.0 - anim);
                if anim > 0.005 {
                    let card = Rect::from_min_max(Pos2::ZERO, full.max);
                    // No `shadow()` call: the card is the whole window, so every
                    // ring of it fell outside the window and was clipped. The
                    // only place a shadow ever showed was the collapsed pill,
                    // where it was cut off mid-gradient — the "transparent box".
                    crate::ui::widgets::round_rect(
                        ui,
                        card,
                        radius,
                        pal.surface,
                        egui::Stroke::new(1.0_f32, pal.stroke),
                    );
                }

                if let Some(rect) = panel_rect {
                    if anim > 0.01 {
                        // Unfold: the panel slides up 6 pt and fades in while
                        // the window is already at its final size.
                        let offset = (1.0 - anim) * 6.0;
                        let faded = Rect::from_min_size(
                            rect.min + Vec2::new(0.0, offset),
                            rect.size(),
                        );
                        ui.scope(|ui| {
                            let mut child = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(faded)
                                    .layout(egui::Layout::top_down(egui::Align::Min)),
                            );
                            child.set_opacity(anim);
                            let mut st = PanelState {
                                tab: self.tab,
                                tasks: &mut self.tasks,
                                clip: &mut self.clip,
                                inbox: &mut self.inbox,
                                snap: self.sampler.read(),
                                metrics_error: self.sampler.error(),
                                cfg: &self.cfg,
                                today: &self.today,
                                task_input: &mut self.task_input,
                                task_time: &mut self.task_time,
                                task_repeat: &mut self.task_repeat,
                                task_every: &mut self.task_every,
                                clip_query: &mut self.clip_query,
                                clip_scroll: &mut self.clip_scroll,
                                inbox_scroll: &mut self.inbox_scroll,
                                textures: &mut self.textures,
                                toast: self.toast.as_ref().map(|(t, _)| t.as_str()),
                                dispatch_note: self.send_note.as_ref().map(|(t, _)| t.as_str()),
                            };
                            crate::ui::panel::draw(&mut child, faded, &mut st, &pal, &mut actions, true);
                        });
                    }
                }

                // The island's own content becomes the panel's title row: the
                // row's left and right edges travel from the pill's edges to
                // the card's padding, so the clock slides left and the metrics
                // slide right as it opens instead of the whole row jumping to a
                // new width. That travel is what "向面板铺开" is.
                //
                // Collapsed, the row **is** the window: the pill used to be
                // inset by its own padding, which left a 14 px strip of empty
                // window on each side where the clipped shadow showed as a
                // rectangular band. The inset is now padding instead of
                // geometry — same content position, no visible frame.
                let cap_pad = (cap_rect.height() * 0.34).clamp(9.0, 14.0);
                let lerp = |a: f32, b: f32| a + (b - a) * anim;
                let row = Rect::from_min_max(
                    Pos2::new(lerp(cap_rect.left(), full.left() + 16.0), cap_rect.top()),
                    Pos2::new(lerp(cap_rect.right(), full.right() - 16.0), cap_rect.bottom()),
                );
                let hit = crate::ui::compact::draw(
                    ui,
                    row,
                    &caps,
                    &pal,
                    self.cfg.behavior.opacity,
                    (cap_rect.height() * 0.46).clamp(13.0, 19.0),
                    crate::ui::compact::CapsuleOpts {
                        // The card above already covers the pill's area with the
                        // same colour; drawing the pill too would only add a
                        // second border inside the card.
                        shell: anim < 0.02,
                        pad: lerp(cap_pad, 0.0),
                        corner: anim,
                    },
                );
                if hit.close {
                    self.set_open(false, ctx);
                }
                // Collapsed, the whole pill is the open button. Open, that row
                // belongs to the panel and only the close button brings it
                // back down — clicking the title bar of a window does not
                // close it.
                if hit.clicked && anim < 0.5 {
                    if self.open {
                        self.set_open(false, ctx);
                    } else {
                        self.set_open(true, ctx);
                    }
                    self.parked = false;
                }

                if let Some(panel) = panel_rect {
                    let grip = Rect::from_min_max(
                        Pos2::new(panel.right() - 16.0, panel.bottom() - 16.0),
                        panel.max,
                    );
                    resize_resp = Some(ui.interact(
                        grip,
                        egui::Id::new("island-resize-hit"),
                        egui::Sense::drag(),
                    ));
                }
            });

        // Panel interactions arrive after painting; applying them now means the
        // click and its effect land in the same frame.
        for action in actions {
            self.apply(action, ctx);
        }

        let hover = ctx.pointer_hover_pos().is_some();
        if hover != self.hovered {
            self.hovered = hover;
            if hover {
                self.last_input = Instant::now();
            }
        }
        // Panel resize by dragging the bottom-right grip.
        if let Some(resp) = resize_resp {
            self.handle_resize(ctx, geo, &resp);
        }
    }

    fn capsule_state(&self) -> crate::ui::compact::CapsuleState {
        let c = clock::Clock::now();
        let snap = self.sampler.read();
        let metrics_on = self.cfg.metrics.show;
        // The capsule carries a task name only while its reminder runs. Before
        // this it showed whatever task was marked "current", all day: a pill
        // permanently displaying "喝水" reads as an unread alert, and the task
        // list is one click away.
        let (task, flash) = match &self.reminder {
            Some((title, at)) => (Some(title.clone()), flash_envelope(at.elapsed().as_secs_f32())),
            None => (None, 0.0),
        };
        crate::ui::compact::CapsuleState {
            // `clock.show = false` hides the whole clock module, weekday
            // included: the setting is called "显示时钟", not "显示时分".
            time: if self.cfg.clock.show {
                c.time_text(self.cfg.clock.hour24, self.cfg.clock.show_seconds)
            } else {
                String::new()
            },
            weekday: if !self.cfg.clock.show {
                String::new()
            } else {
                let mut w = if self.cfg.clock.show_weekday {
                    c.weekday_cn().to_string()
                } else {
                    String::new()
                };
                if self.cfg.clock.show_date {
                    w = format!("{} {}", c.date_cn(), w);
                }
                w
            },
            cpu: if metrics_on && self.cfg.metrics.cpu {
                Some(snap.cpu)
            } else {
                None
            },
            mem: if metrics_on && self.cfg.metrics.memory {
                Some(snap.mem_percent)
            } else {
                None
            },
            net: if metrics_on && self.cfg.metrics.network {
                Some((snap.net_down_bps, snap.net_up_bps))
            } else {
                None
            },
            task,
            unread: if self.cfg.inbox.badge {
                self.inbox.unread()
            } else {
                0
            },
            hovered: self.hovered,
            open: self.open,
            flash,
        }
    }

    fn handle_resize(&mut self, ctx: &egui::Context, _geo: &Geo, resp: &egui::Response) {
        // Only while the panel is fully open and settled.
        //
        // The grip rectangle is derived from the panel geometry, but the window
        // is still growing for ~200 ms after opening. A press that started
        // somewhere else (a tab, the capsule) then produced one enormous
        // `drag_delta` here and clamped the panel to its minimum size — which is
        // how a 351×307 panel became 320×260 without anyone dragging it.
        if !self.open || self.anim < 0.99 {
            if resp.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::ResizeNwSe);
            }
            return;
        }
        if resp.hovered() || resp.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeNwSe);
        }
        if resp.dragged_by(egui::PointerButton::Primary) {
            let d = resp.drag_delta();
            // A single frame never moves more than 60 pt: a real drag is bounded
            // by how fast the hand moves, a bogus delta is not.
            let d = Vec2::new(d.x.clamp(-60.0, 60.0), d.y.clamp(-60.0, 60.0));
            if d != Vec2::ZERO {
                self.cfg.ui.panel_width =
                    (self.cfg.ui.panel_width + d.x).clamp(320.0, 720.0);
                self.cfg.ui.panel_height =
                    (self.cfg.ui.panel_height + d.y).clamp(260.0, 900.0);
                self.last_input = Instant::now();
                self.dirty_at = Some(Instant::now());
                ctx.request_repaint();
            }
        }
        if resp.drag_stopped() {
            self.save_all();
            self.persist_ui();
        }
    }

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        self.last_input = Instant::now();
        // Any click in the panel means the user took it over: the reminder's
        // 12 s auto-close no longer applies.
        self.passive = false;
        match action {
            Action::Close => self.set_open(false, ctx),
            Action::Tab(tab) => {
                self.tab = tab;
                self.cfg.ui.last_tab = tab.key().to_string();
                self.dirty_at = Some(Instant::now());
                self.persist_ui();
            }
            Action::AddTask => {
                let title = self.task_input.trim().to_string();
                if title.is_empty() {
                    return;
                }
                // The time field accepts both `09:00` and `09:00-18:00`. With an
                // interval the end is the last slot; without one the end is
                // meaningless and is dropped inside `Store::add`.
                let mut at = None;
                let mut until = None;
                if !self.task_time.trim().is_empty() {
                    match clock::parse_hhmm_span(&self.task_time) {
                        Some((start, end)) => {
                            at = Some(start);
                            until = end;
                        }
                        None => {
                            self.toast =
                                Some(("时间格式应为 09:00 或 09:00-18:00".to_string(), Instant::now()));
                            return;
                        }
                    }
                }
                let every = (self.task_every > 0).then_some(self.task_every);
                // Typing a range without an interval obviously means "spread it
                // out": the range has no other reading. Default to 30 minutes
                // rather than saving a window that does nothing.
                let every = every.or_else(|| until.map(|_| 30));
                let id = self.tasks.add(tasks::NewTask {
                    title: &title,
                    day: &self.today,
                    at,
                    until,
                    every,
                    repeat: self.task_repeat,
                });
                if self.tasks.current().is_none() {
                    self.tasks.set_current(id);
                }
                // Say what was actually created. "每半小时" silently becoming a
                // nine-to-five window is exactly the kind of assumption the user
                // should be able to see and correct.
                if let Some(t) = self.tasks.get(id) {
                    if t.is_interval() {
                        let mut parts = vec![t.repeat.label().to_string()];
                        if let Some(w) = t.window_label() {
                            parts.push(w);
                        }
                        if let Some(e) = t.every_label() {
                            parts.push(e);
                        }
                        self.toast = Some((format!("已添加：{}", parts.join(" ")), Instant::now()));
                    }
                }
                self.task_input.clear();
                self.task_time.clear();
                self.task_repeat = tasks::Repeat::Once;
                self.task_every = 0;
                self.dirty_at = Some(Instant::now());
            }
            Action::ToggleDone(id) => {
                self.tasks.toggle_done(id, &self.today);
                self.dirty_at = Some(Instant::now());
            }
            Action::SetCurrent(id) => {
                self.tasks.set_current(id);
                self.dirty_at = Some(Instant::now());
            }
            Action::RemoveTask(id) => {
                self.tasks.remove(id);
                self.dirty_at = Some(Instant::now());
            }
            Action::Archive => match self.tasks.archive(&self.cfg.tasks.archive_dir, &self.today) {
                Ok(path) => {
                    self.toast = Some((format!("已归档到 {}", path.display()), Instant::now()))
                }
                Err(e) => self.toast = Some((e, Instant::now())),
            },
            Action::PinClip(id) => {
                self.clip.toggle_pin(id);
                self.dirty_at = Some(Instant::now());
            }
            Action::CopyClip(id) => {
                if self.clip.write_back(id) {
                    self.toast = Some(("已复制到剪贴板".to_string(), Instant::now()));
                } else {
                    self.toast = Some(("条目文件已不存在".to_string(), Instant::now()));
                }
            }
            Action::RemoveClip(id) => {
                self.clip.remove(id);
                self.dirty_at = Some(Instant::now());
            }
            Action::ClearClips => {
                self.clip.clear_unpinned();
                self.dirty_at = Some(Instant::now());
            }
            Action::MarkAllRead => self.inbox.mark_all_read(),
            Action::ClearInbox => self.inbox.clear(),
            Action::RemoveInbox(id) => self.inbox.remove(id),
            Action::OpenInbox(id, source) => {
                self.inbox.mark_read(id);
                if crate::activate::activate(source) {
                    // The other app has the foreground now; a topmost island
                    // left open would sit over it for no reason.
                    self.set_open(false, ctx);
                } else {
                    self.toast = Some((
                        format!("没有找到正在运行的「{}」窗口", source.label()),
                        Instant::now(),
                    ));
                }
            }
        }
        ctx.request_repaint();
    }

    fn schedule_repaint(&self, ctx: &egui::Context) {
        let target = if self.open && !self.parked { 1.0 } else { 0.0 };
        if (target - self.anim).abs() > 0.001 {
            // Mid-unfold: the window grows every frame, so the frames have to
            // keep coming or the resize would visibly step.
            ctx.request_repaint_after(Duration::from_millis(16));
            return;
        }
        if self.open {
            // While open, keep the panel responsive (hover, drag, animations).
            ctx.request_repaint_after(Duration::from_millis(120));
        } else if self.reminder.is_some() {
            // The pulse is the alert: it has to run at a real frame rate, and
            // only for the twelve seconds it lasts.
            ctx.request_repaint_after(Duration::from_millis(50));
        } else {
            let c = clock::Clock::now();
            let mut wait = c.to_next_second();
            if self.cfg.clock.show_seconds {
                wait = wait.min(Duration::from_millis(250));
            }
            if self.cfg.metrics.show {
                wait = wait.min(Duration::from_millis(1000));
            }
            // The hotkey marker is only read from `housekeeping`, i.e. once per
            // frame. Waiting for the next whole second made `Alt+I` take up to
            // a second to react; MARKER_POLL caps that at the poll interval.
            wait = wait.min(MARKER_POLL);
            ctx.request_repaint_after(wait);
        }
    }
}

impl eframe::App for Island {
    /// The window is transparent by design: only the capsule and the panel are
    /// painted, so the window corners, the `GAP` band between them and the
    /// strips beside the capsule (the panel is wider) must show the desktop.
    ///
    /// eframe's default is `rgba(12, 12, 12, 180)` — a 70 % black veil over the
    /// whole window rectangle. That is the "black frame" around the capsule and
    /// the panel: it is not a DWM border and not a shadow, it is the clear
    /// colour showing through every unpainted pixel.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ensure();
        ctx.set_pixels_per_point(self.scale);
        self.housekeeping(ctx);
        // Before geometry: the window rectangle is derived from the easing.
        self.advance_anim();

        if self.parked {
            // Full-screen content in front: park off-screen and stop drawing.
            // A hidden eframe window still runs `update`, so this is cheap.
            self.win.place([-32_000, -32_000, -32_000 + 120, -32_000 + 24]);
            ctx.request_repaint_after(Duration::from_millis(700));
            return;
        }

        let geo = self.compute_geo();
        // Resize *before* painting. egui clips to the current window rectangle,
        // so a window that is still collapsed would drop the first expanded
        // frame — the frame in which every panel button first appears.
        self.win.place(geo.window_px);
        self.draw(ctx, &geo);
        self.win.housekeeping();

        // Focus loss closes the panel. The grace period covers the frame where
        // the window is still becoming active right after a click. A panel the
        // island raised itself (reminder) never had focus to begin with, so it
        // must not be closed by this rule.
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if self.open && !focused && !self.passive {
            let since = *self.focus_lost.get_or_insert_with(Instant::now);
            if since.elapsed() >= FOCUS_GRACE {
                self.set_open(false, ctx);
            }
        } else {
            self.focus_lost = None;
        }
        if self.open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.set_open(false, ctx);
        }

        self.schedule_repaint(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_all();
    }
}

fn file_stamp(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Pulse envelope for a reminder, 0..1: rises in 0.25 s, breathes at ~0.85 s,
/// fades out over the last 1.6 s. It never reaches zero inside the window, so
/// the pill reads as "alerting" rather than as a blinking cursor.
fn flash_envelope(t: f32) -> f32 {
    if t < 0.0 || t > REMINDER_SECS {
        return 0.0;
    }
    let rise = (t / 0.25).min(1.0);
    let fall = ((REMINDER_SECS - t) / 1.6).clamp(0.0, 1.0);
    let breathe = 0.5 - 0.5 * (t * std::f32::consts::TAU * 1.18).cos();
    rise * fall * (0.35 + 0.65 * breathe)
}
