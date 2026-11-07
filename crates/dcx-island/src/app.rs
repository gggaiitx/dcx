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
    scale: f32,
    pal: theme::Palette,
    pal_at: Option<Instant>,

    open: bool,
    anim: f32,
    hovered: bool,
    parked: bool,
    focus_lost: Option<Instant>,
    last_input: Instant,
    last_fs: Instant,
    last_marker: Instant,
    last_stamp: u128,

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
    clip_query: String,

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
            scale: 1.0,
            pal: theme::Palette::system(),
            pal_at: None,
            open: false,
            anim: 0.0,
            hovered: false,
            parked: false,
            focus_lost: None,
            last_input: Instant::now(),
            last_fs: Instant::now(),
            last_marker: Instant::now(),
            last_stamp: 0,
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
            clip_query: String::new(),
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
            if !self.open {
                self.set_open(true, ctx);
            }
            self.dirty_at = Some(Instant::now());
        }
        if self.reminder.as_ref().map(|(_, t)| t.elapsed().as_secs() > 12).unwrap_or(false) {
            self.reminder = None;
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
        let mut next = dcx_island_config::load();
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
            if let Some(seq) = self.clip.self_write {
                if seq == sys::clipboard_sequence() {
                    // Our own write-back; do not record it again.
                    self.clip.self_write = None;
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
        let mon = self.win.monitor();
        let full = win::foreground_is_fullscreen(&mon, self.win.hwnd);
        if full != self.parked {
            win::trace(&format!("fullscreen={full} parked={}", !full));
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
        if self.open == open {
            return;
        }
        self.open = open;
        self.last_input = Instant::now();
        if open {
            self.focus_lost = None;
            // Focus is required for the task input; the window is created
            // without WS_EX_NOACTIVATE for exactly this reason.
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        } else {
            self.save_all();
            if let Some(tab) = Some(self.tab.key()) {
                if self.cfg.ui.last_tab != tab {
                    self.cfg.ui.last_tab = tab.to_string();
                    let cfg = self.cfg.clone();
                    let _ = dcx_island_config::save(&cfg);
                }
            }
        }
        self.dirty_at = Some(Instant::now());
        ctx.request_repaint();
    }

    // -- geometry ----------------------------------------------------------

    fn compute_geo(&self) -> Geo {
        let scale = self.scale.max(1.0);
        let mon = self.win.monitor();
        let mon_w = (mon.rect[2] - mon.rect[0]) as f32;
        let mon_h = (mon.rect[3] - mon.rect[1]) as f32;

        let capsule_w = (self.cfg.behavior.capsule_width * scale).min(mon_w - 24.0 * scale);
        let capsule_h = self.cfg.behavior.capsule_height * scale;
        let panel_w = (self.cfg.ui.panel_width * scale).min(mon_w - 16.0 * scale);
        let panel_h = (self.cfg.ui.panel_height * scale).min(mon_h - 120.0 * scale);

        let open = self.open && !self.parked;
        let win_w = if open { capsule_w.max(panel_w) } else { capsule_w };
        let win_h = if open { capsule_h + (GAP * scale) + panel_h } else { capsule_h };

        let center_x = (mon.rect[0] + mon.rect[2]) as f32 * 0.5;
        let x = (center_x - win_w * 0.5).round() as i32;
        let y = mon.work[1] + (EDGE_INSET * scale).round() as i32;

        let w_pt = win_w / scale;
        let capsule = Rect::from_min_size(
            Pos2::new((w_pt - capsule_w / scale) * 0.5, 0.0),
            Vec2::new(capsule_w / scale, capsule_h / scale),
        );
        let panel = if open {
            Some(Rect::from_min_size(
                Pos2::new((w_pt - panel_w / scale) * 0.5, capsule_h / scale + GAP),
                Vec2::new(panel_w / scale, panel_h / scale),
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

    fn draw(&mut self, ctx: &egui::Context, geo: &Geo) {
        let pal = self.pal;
        let caps = self.capsule_state();
        let open = self.open;
        let target = if open { 1.0 } else { 0.0 };
        // 0.18 per frame at 60 Hz ≈ 90 ms: fast enough to feel instant, slow
        // enough that the panel does not pop.
        self.anim += (target - self.anim) * 0.32;
        if (target - self.anim).abs() < 0.01 {
            self.anim = target;
        }
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
                let full = Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(
                        geo.window_px[2] as f32 / geo.scale,
                        geo.window_px[3] as f32 / geo.scale,
                    ),
                );
                ui.set_min_size(full.size());

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
                                clip_query: &mut self.clip_query,
                                textures: &mut self.textures,
                                toast: self.toast.as_ref().map(|(t, _)| t.as_str()),
                                dispatch_note: self.send_note.as_ref().map(|(t, _)| t.as_str()),
                            };
                            crate::ui::panel::draw(&mut child, faded, &mut st, &pal, &mut actions);
                        });
                    }
                }

                let hit = crate::ui::compact::draw(
                    ui,
                    cap_rect,
                    &caps,
                    &pal,
                    self.cfg.behavior.opacity,
                    (cap_rect.height() * 0.46).clamp(13.0, 19.0),
                );
                if hit.clicked {
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
        crate::ui::compact::CapsuleState {
            time: c.time_text(self.cfg.clock.hour24, self.cfg.clock.show_seconds),
            weekday: {
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
            task: self.tasks.current().map(|t| t.title.clone()),
            unread: if self.cfg.inbox.badge {
                self.inbox.unread()
            } else {
                0
            },
            hovered: self.hovered,
            open: self.open,
        }
    }

    fn handle_resize(&mut self, ctx: &egui::Context, _geo: &Geo, resp: &egui::Response) {
        if resp.hovered() || resp.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeNwSe);
        }
        if resp.dragged() {
            let d = resp.drag_delta();
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
        }
    }

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        self.last_input = Instant::now();
        match action {
            Action::Close => self.set_open(false, ctx),
            Action::Tab(tab) => {
                self.tab = tab;
                self.cfg.ui.last_tab = tab.key().to_string();
                self.dirty_at = Some(Instant::now());
            }
            Action::AddTask => {
                let title = self.task_input.trim().to_string();
                if title.is_empty() {
                    return;
                }
                let at = if self.task_time.trim().is_empty() {
                    None
                } else {
                    match clock::parse_hhmm(&self.task_time) {
                        Some(secs) => Some(secs),
                        None => {
                            self.toast = Some(("时间格式应为 15:30".to_string(), Instant::now()));
                            return;
                        }
                    }
                };
                let id = self.tasks.add(&title, &self.today, at, self.task_repeat);
                if self.tasks.current().is_none() {
                    self.tasks.set_current(id);
                }
                self.task_input.clear();
                self.task_time.clear();
                self.task_repeat = tasks::Repeat::Once;
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
        }
        ctx.request_repaint();
    }

    fn schedule_repaint(&self, ctx: &egui::Context) {
        if self.open {
            // While open, keep the panel responsive (hover, drag, animations).
            ctx.request_repaint_after(Duration::from_millis(120));
        } else {
            let c = clock::Clock::now();
            let mut wait = c.to_next_second();
            if self.cfg.clock.show_seconds {
                wait = wait.min(Duration::from_millis(250));
            }
            if self.cfg.metrics.show {
                wait = wait.min(Duration::from_millis(1000));
            }
            ctx.request_repaint_after(wait);
        }
    }
}

impl eframe::App for Island {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ensure();
        ctx.set_pixels_per_point(self.scale);
        self.housekeeping(ctx);

        if self.parked {
            // Full-screen content in front: park off-screen and stop drawing.
            // A hidden eframe window still runs `update`, so this is cheap.
            self.win.place([-32_000, -32_000, -32_000 + 120, -32_000 + 24]);
            ctx.request_repaint_after(Duration::from_millis(700));
            return;
        }

        let geo = self.compute_geo();
        self.draw(ctx, &geo);
        self.win.place(geo.window_px);
        self.win.housekeeping();

        // Focus loss closes the panel. The grace period covers the frame where
        // the window is still becoming active right after a click.
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if self.open && !focused {
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
