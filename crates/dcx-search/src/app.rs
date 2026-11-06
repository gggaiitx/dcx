//! The one eframe event loop for `dcx-search.exe`.
//!
//! Owns the UI state, the background engine, the query generation counter and
//! the frame-side consumption of `Ev` batches. No Win32/winit window procedure
//! is subclassed: the host raises this window by touching `search_show`, and
//! this loop polls that marker (the platform's "file is IPC" convention).

use crate::config::Config;
use crate::index::{self, Index, Volume};
use crate::model::{Ev, Mode, Query, ScopeSel};
use crate::view::{self, Action, Skin, ViewState};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use winit::platform::windows::EventLoopBuilderExtWindows;

/// Must equal `AppEntry.window_title` in `dcx-host/src/apps.rs` byte for byte:
/// the host uses `FindWindowW` to stop a manually started instance.
pub const WIN_TITLE: &str = "文件检索";

static STARTED: AtomicBool = AtomicBool::new(false);

const DEBOUNCE_MS: u64 = 80;
const CONFIG_DEBOUNCE_MS: u64 = 220;
const MAX_BATCHES_PER_FRAME: usize = 4;

pub fn spawn() -> Option<std::thread::JoinHandle<()>> {
    if STARTED.swap(true, Ordering::SeqCst) {
        return None;
    }
    Some(std::thread::spawn(|| {
        let cfg = Config::load();
        let result = eframe::run_native(
            WIN_TITLE,
            native_options(&cfg),
            Box::new(|cc| {
                crate::ui::setup_fonts(&cc.egui_ctx);
                Ok(Box::new(App::new()))
            }),
        );
        if let Err(e) = result {
            crate::win::trace(&format!("eframe failed: {e}"));
            STARTED.store(false, Ordering::SeqCst);
        }
    }))
}

fn native_options(cfg: &Config) -> eframe::NativeOptions {
    // Resident start = wait off-screen for the hotkey. Creating the window
    // there avoids a one-frame flash at the default centre position before
    // the first `update` gets a chance to move it.
    let launched_recently = crate::win::show_marker_time()
        .map(|t| t >= crate::win::process_started())
        .unwrap_or(false);
    let start_hidden = cfg.activation.enabled && !launched_recently;
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([cfg.ui.window_w, cfg.ui.window_h])
        .with_min_inner_size([640.0, 480.0])
        .with_resizable(true)
        // P0 uses the system title bar (PRD §4.2 allows it); P1 switches
        // to a borderless, self-drawn caption strip like the reference UI.
        .with_decorations(true)
        .with_icon(crate::ui::app_icon());
    if start_hidden {
        viewport = viewport.with_position(egui::pos2(-32_000.0, -32_000.0));
    }
    eframe::NativeOptions {
        viewport,
        event_loop_builder: Some(Box::new(|b| {
            b.with_any_thread(true);
        })),
        ..Default::default()
    }
}

struct Engine {
    index: Arc<RwLock<Index>>,
    ev_tx: Sender<Ev>,
    indexing: Arc<AtomicBool>,
    scanned: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    /// Shared by every watcher; a rebuild bumps it so older watcher threads
    /// stop writing into an index the UI no longer reads.
    watch_epoch: Arc<AtomicU64>,
    next_epoch: u64,
    volumes: Vec<Volume>,
    signature: u64,
    /// Watcher changes not yet written to the snapshot.
    dirty: Arc<AtomicBool>,
    saving: Arc<AtomicBool>,
    refresh_secs: u64,
    last_save_tick: Instant,
    last_save_attempt: Instant,
}

fn refresh_secs(cfg: &Config) -> u64 {
    cfg.index.refresh_minutes.max(1) as u64 * 60
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Engine {
    fn new(cfg: &Config) -> (Self, Receiver<Ev>) {
        let (ev_tx, ev_rx) = bounded::<Ev>(4096);
        let volumes = index::volumes();
        let signature = index::snapshot_signature(cfg, &volumes);
        let mut engine = Engine {
            index: Arc::new(RwLock::new(Index::new())),
            ev_tx,
            indexing: Arc::new(AtomicBool::new(false)),
            scanned: Arc::new(AtomicU64::new(0)),
            stop: Arc::new(AtomicBool::new(false)),
            watch_epoch: Arc::new(AtomicU64::new(0)),
            next_epoch: 1,
            volumes,
            signature,
            dirty: Arc::new(AtomicBool::new(false)),
            saving: Arc::new(AtomicBool::new(false)),
            refresh_secs: refresh_secs(cfg),
            last_save_tick: Instant::now(),
            // Treat "no snapshot yet" as very old so an exit after a failed
            // build save still retries.
            last_save_attempt: Instant::now() - Duration::from_secs(3600),
        };

        // Load the previous snapshot before starting any walk. A snapshot saved
        // minutes ago is as good as a fresh walk for everyday search and turns
        // a ~90 s cold start into a file read.
        let snapshot = index::load_snapshot(&index::snapshot_path(), engine.signature);
        match snapshot {
            Some((cached, saved_at, entries)) => {
                engine.index = Arc::new(RwLock::new(cached));
                let epoch = engine.next_epoch;
                engine.next_epoch += 1;
                engine.watch_epoch.store(epoch, Ordering::SeqCst);
                engine.scanned.store(entries as u64, Ordering::SeqCst);
                let _ = engine.ev_tx.send(Ev::IndexReady {
                    epoch,
                    entries,
                    errors: 0,
                    elapsed_ms: 0,
                    cached: true,
                });
                index::spawn_watch(
                    engine.index.clone(),
                    ScopeSel::Computer,
                    engine.volumes.clone(),
                    cfg.clone(),
                    engine.ev_tx.clone(),
                    engine.watch_epoch.clone(),
                    epoch,
                    engine.dirty.clone(),
                );
                let age = now_secs().saturating_sub(saved_at);
                // If the snapshot is seconds old, an exit right after launch
                // should not rewrite 159 MB for nothing.
                engine.last_save_attempt = Instant::now()
                    .checked_sub(Duration::from_secs(age.min(3600)))
                    .unwrap_or_else(Instant::now);
                engine.last_save_tick = Instant::now();
                if age > engine.refresh_secs {
                    crate::win::trace(&format!(
                        "index snapshot is {age}s old (limit {}s); rebuilding",
                        engine.refresh_secs
                    ));
                    engine.start(cfg);
                } else {
                    crate::win::trace(&format!(
                        "index snapshot loaded: {entries} entries, {age}s old"
                    ));
                }
            }
            None => {
                if index::snapshot_path().exists() {
                    crate::win::trace("index snapshot unusable (format/scope changed); rebuilding");
                }
                engine.start(cfg);
            }
        }
        (engine, ev_rx)
    }

    /// Fresh index + walk + watcher. Old workers keep writing into their own
    /// `Arc` (discarded here) instead of the new one, so a rebuild needs no
    /// destructive cleanup and cannot corrupt in-flight results.
    fn start(&mut self, cfg: &Config) {
        self.stop.store(true, Ordering::SeqCst);
        let epoch = self.next_epoch;
        self.next_epoch += 1;
        self.watch_epoch.store(epoch, Ordering::SeqCst);
        self.index = Arc::new(RwLock::new(Index::new()));
        self.indexing = Arc::new(AtomicBool::new(false));
        self.scanned = Arc::new(AtomicU64::new(0));
        self.stop = Arc::new(AtomicBool::new(false));
        self.volumes = index::volumes();
        self.signature = index::snapshot_signature(cfg, &self.volumes);
        self.refresh_secs = refresh_secs(cfg);
        self.dirty.store(false, Ordering::SeqCst);
        index::spawn_build(
            self.index.clone(),
            cfg.clone(),
            ScopeSel::Computer,
            self.volumes.clone(),
            self.ev_tx.clone(),
            self.indexing.clone(),
            self.scanned.clone(),
            self.stop.clone(),
            epoch,
        );
        index::spawn_watch(
            self.index.clone(),
            ScopeSel::Computer,
            self.volumes.clone(),
            cfg.clone(),
            self.ev_tx.clone(),
            self.watch_epoch.clone(),
            epoch,
            self.dirty.clone(),
        );
    }

    fn rebuild(&mut self, cfg: &Config) {
        self.start(cfg);
    }

    fn epoch(&self) -> u64 {
        self.watch_epoch.load(Ordering::SeqCst)
    }

    /// Write the snapshot in the background. Called after a build finishes and
    /// every few minutes while the watcher has changed something.
    pub fn request_save(&mut self, force: bool) {
        if !force && !self.dirty.swap(false, Ordering::SeqCst) {
            return;
        }
        if self.saving.swap(true, Ordering::SeqCst) {
            if force {
                self.dirty.store(true, Ordering::SeqCst);
            }
            return;
        }
        self.last_save_attempt = Instant::now();
        let index = self.index.clone();
        let path = index::snapshot_path();
        let signature = self.signature;
        let saving = self.saving.clone();
        let dirty = self.dirty.clone();
        std::thread::spawn(move || {
            let result = match index.read() {
                Ok(guard) => index::save_snapshot(&guard, &path, signature),
                Err(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "index lock poisoned",
                )),
            };
            match &result {
                Ok(()) => crate::win::trace(&format!(
                    "index snapshot saved: {} bytes",
                    std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
                )),
                Err(e) => crate::win::trace(&format!("index snapshot save failed: {e}")),
            }
            if result.is_err() {
                dirty.store(true, Ordering::SeqCst);
            }
            saving.store(false, Ordering::SeqCst);
        });
    }

    pub fn tick_save(&mut self) {
        // A 159 MB snapshot is not something to rewrite every few minutes; the
        // watcher keeps the in-memory index current, this only refreshes the
        // copy that the next cold start will use.
        if self.last_save_tick.elapsed() < Duration::from_secs(3600) {
            return;
        }
        self.last_save_tick = Instant::now();
        if !self.indexing.load(Ordering::SeqCst) {
            self.request_save(false);
        }
    }

    /// Exit-time write. Blocking on purpose: the process is about to end and
    /// the thread that would normally do this would die with it. Skipped when a
    /// snapshot was written recently — the few seconds of session changes are
    /// not worth rewriting the whole file on every close.
    pub fn save_blocking(&mut self) {
        // Throttle *before* consuming the dirty flag. The old order cleared it
        // and returned, so a snapshot skipped by the 10-minute throttle was
        // never written in this session at all.
        if self.last_save_attempt.elapsed() < Duration::from_secs(600) {
            return;
        }
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return;
        }
        self.last_save_attempt = Instant::now();
        if let Ok(guard) = self.index.read() {
            let path = index::snapshot_path();
            match index::save_snapshot(&guard, &path, self.signature) {
                Ok(()) => crate::win::trace(&format!(
                    "index snapshot saved on exit: {} bytes",
                    std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
                )),
                Err(e) => {
                    crate::win::trace(&format!("index snapshot save on exit failed: {e}"));
                    self.dirty.store(true, Ordering::SeqCst);
                }
            }
        }
    }
}

pub struct App {
    cfg: Config,
    cfg_stamp: (u64, u128),
    cfg_dirty: Option<Instant>,
    view: ViewState,
    engine: Engine,
    ev_rx: Receiver<Ev>,
    generation: Arc<AtomicU64>,
    last_signature: String,
    pending_query: Option<Instant>,
    show_stamp: u128,
    positioned: bool,
    /// The first `search_show` change after process start always means "show".
    /// Treating it as a toggle can hide a window the host launched for the very
    /// hotkey that produced the marker (a launch/touch race).
    marker_seen: bool,
    /// `activation.enabled = true` means "resident index": start with the
    /// window off-screen and wait for the hotkey instead of popping it up.
    start_hidden: bool,
    hidden: bool,
    hidden_at: Option<Instant>,
    hidden_by_toggle: bool,
    /// Set before we ask the window to close, so `close_requested` does not
    /// turn our own exit (timer / release) into another hide.
    allow_exit: bool,
    geometry_dirty: Option<Instant>,
    last_outer: Option<(i32, i32, i32, i32)>,
    scope_sig: String,
    /// A query must run again after the index finishes (or is rebuilt): the
    /// first pass may have been answered by a partially built index.
    rerun: bool,
    last_rebuild: Option<Instant>,
}

fn scope_from_key(key: &str, vols: &[Volume], custom: &[String]) -> ScopeSel {
    match key {
        "desktop" => ScopeSel::Desktop,
        "computer" => ScopeSel::Computer,
        other => {
            if let Some(p) = custom.iter().find(|p| p.as_str() == other) {
                ScopeSel::Path(p.clone())
            } else if let Some(v) = vols.iter().find(|v| v.root == other) {
                ScopeSel::Path(v.root.clone())
            } else {
                ScopeSel::Computer
            }
        }
    }
}

fn scope_key(s: &ScopeSel) -> String {
    match s {
        ScopeSel::Desktop => "desktop".into(),
        ScopeSel::Computer => "computer".into(),
        ScopeSel::Path(p) => p.clone(),
    }
}

/// Everything that changes what the index has to contain. Editing these while
/// the app runs triggers a rebuild; `content.*` only changes how a query reads
/// the already-built candidate set.
fn scope_signature(cfg: &Config) -> String {
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{}",
        cfg.scope.include_hidden,
        cfg.scope.follow_links,
        cfg.scope.custom_roots.join("\u{2}"),
        cfg.scope.exclude.join("\u{2}")
    )
}

fn scope_items(vols: &[Volume], custom: &[String]) -> Vec<(ScopeSel, String)> {
    let mut out = vec![
        (ScopeSel::Desktop, "桌面".into()),
        (ScopeSel::Computer, "此电脑".into()),
    ];
    for v in vols {
        let label = if v.removable {
            format!("{} · 可移动", v.label)
        } else {
            v.label.clone()
        };
        out.push((ScopeSel::Path(v.root.clone()), label));
    }
    for p in custom {
        out.push((
            ScopeSel::Path(p.clone()),
            p.trim_end_matches(['\\', '/'])
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or(p)
                .to_string(),
        ));
    }
    out
}

impl App {
    fn new() -> Self {
        let cfg = Config::load();
        let vols = index::volumes();
        let items = scope_items(&vols, &cfg.scope.custom_roots);
        let mode = Mode::from_key(&cfg.ui.last_mode);
        let scope = scope_from_key(&cfg.ui.last_scope, &vols, &cfg.scope.custom_roots);
        let view = ViewState::new(
            items,
            mode,
            scope,
            [cfg.ui.col_name_w, cfg.ui.col_path_w, cfg.ui.col_size_w],
            (
                crate::view::SortCol::from_key(&cfg.ui.sort_by),
                cfg.ui.sort_desc,
            ),
        );
        let (engine, ev_rx) = Engine::new(&cfg);
        let generation = Arc::new(AtomicU64::new(1));

        // The host launches us and touches `search_show` around the same time.
        // If the marker is not older than this process, it was a hotkey
        // launch — honour it even when `enabled` says "start resident".
        let mut start_hidden = cfg.activation.enabled;
        if let Some(t) = crate::win::show_marker_time() {
            if t >= crate::win::process_started() {
                start_hidden = false;
            }
        }

        App {
            cfg: cfg.clone(),
            cfg_stamp: Config::stamp(),
            cfg_dirty: None,
            view,
            engine,
            ev_rx,
            generation,
            last_signature: String::new(),
            pending_query: None,
            show_stamp: crate::win::show_marker_stamp(),
            positioned: false,
            marker_seen: false,
            start_hidden,
            hidden: start_hidden,
            hidden_at: None,
            hidden_by_toggle: false,
            allow_exit: false,
            geometry_dirty: None,
            last_outer: None,
            scope_sig: scope_signature(&cfg),
            rerun: false,
            last_rebuild: None,
        }
    }

    fn bring_front(&mut self) {
        let scale = crate::win::scale_for(WIN_TITLE).max(0.5);
        let w = (self.cfg.ui.window_w * scale).round().max(320.0) as i32;
        let h = (self.cfg.ui.window_h * scale).round().max(240.0) as i32;
        let (cx, cy) = crate::win::cursor_pos();
        // Position on the monitor the pointer is on, not the primary one.
        let (wx, wy, ww, wh) = crate::win::work_area_at(cx, cy);
        let max_x = (wx + ww - w).max(wx);
        let max_y = (wy + wh - h).max(wy);
        let centered = (wx + (ww - w) / 2, wy + (wh - h) / 2);
        let (x, y) = match self.cfg.activation.show_on.as_str() {
            "last" => match (self.cfg.ui.window_x, self.cfg.ui.window_y) {
                (Some(lx), Some(ly)) if crate::win::rect_on_any_monitor(lx, ly, w, h) => {
                    // Clamp to the monitor the window was last on, which is not
                    // necessarily the one the pointer is on now.
                    let (ax, ay, aw, ah) = crate::win::work_area_at(lx + w / 2, ly + h / 2);
                    (
                        lx.clamp(ax, (ax + aw - w).max(ax)),
                        ly.clamp(ay, (ay + ah - h).max(ay)),
                    )
                }
                _ => centered,
            },
            "center" => centered,
            // "mouse": upper-middle of the cursor's monitor, the launcher look.
            _ => (wx + (ww - w) / 2, wy + wh / 8),
        };
        crate::win::show_at(
            WIN_TITLE,
            x.clamp(wx, max_x),
            y.clamp(wy, max_y),
            w,
            h,
        );
        self.last_outer = crate::win::window_rect(WIN_TITLE);
        self.hidden = false;
        self.hidden_at = None;
        self.hidden_by_toggle = false;
    }

    fn hide(&mut self, by_toggle: bool) {
        crate::win::hide_offscreen(WIN_TITLE);
        self.hidden = true;
        self.hidden_at = Some(Instant::now());
        self.hidden_by_toggle = by_toggle;
    }

    /// Handle the platform's close button / Esc. `release` really exits;
    /// `keep_5m` and `resident` keep the index alive and only hide the window.
    fn request_close(&mut self) {
        if self.cfg.activation.residency == "release" {
            self.save_geometry();
            self.engine.save_blocking();
            self.allow_exit = true;
            crate::win::close_window(WIN_TITLE);
        } else {
            self.save_geometry();
            self.hide(false);
        }
    }

    fn exit_deadline(&self) -> Option<Instant> {
        if !self.hidden {
            return None;
        }
        let at = self.hidden_at?;
        match self.cfg.activation.residency.as_str() {
            "resident" => None,
            "keep_5m" => Some(at + Duration::from_secs(300)),
            _ if self.hidden_by_toggle => Some(at + Duration::from_millis(1500)),
            _ => None,
        }
    }

    fn toggle_window(&mut self) {
        if crate::win::is_foreground(WIN_TITLE) {
            self.save_geometry();
            self.hide(true);
        } else {
            self.bring_front();
        }
    }

    fn refresh_scope_items(&mut self) {
        self.engine.volumes = index::volumes();
        let items = scope_items(&self.engine.volumes, &self.cfg.scope.custom_roots);
        if !items.iter().any(|(s, _)| s == &self.view.scope) {
            self.view.scope = ScopeSel::Computer;
        }
        self.view.scope_items = items;
    }

    fn watch_geometry(&mut self) {
        if !self.positioned {
            return;
        }
        let Some(rect) = crate::win::window_rect(WIN_TITLE) else {
            return;
        };
        // Windows clamps "park off-screen" to about -25600, so an x test alone
        // misses the hidden state. No monitor intersection is unambiguous.
        if !crate::win::rect_on_any_monitor(rect.0, rect.1, rect.2, rect.3) {
            return;
        }
        if self.last_outer != Some(rect) {
            self.last_outer = Some(rect);
            self.geometry_dirty = Some(Instant::now());
        }
    }

    fn submit(&mut self) {
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.view.busy = true;
        self.view.hits.clear();
        self.view.file_paths.clear();
        self.view.selected = None;
        self.view.partial = false;
        self.view.status_note = None;
        let q = Query {
            gen,
            mode: self.view.mode,
            raw: self.view.text.clone(),
            scope: self.view.scope.clone(),
            category: self.view.category.clone(),
            case_sensitive: false,
            max_results: self.cfg.content.max_results,
            max_file_size: self.cfg.max_file_size(),
            encoding: self.cfg.content.encoding.clone(),
            time_budget_ms: self.cfg.content.time_budget_ms,
        };
        crate::query::run(
            q,
            self.engine.index.clone(),
            self.generation.clone(),
            self.engine.ev_tx.clone(),
        );
    }

    fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.view.busy = false;
    }

    fn persist_ui(&mut self) {
        // Read-modify-write: the host edits `activation`/`scope`/`content` in
        // the same document, and a blind save would roll those back.
        let mut disk = Config::load();
        disk.ui.last_mode = self.view.mode.key().into();
        disk.ui.last_scope = scope_key(&self.view.scope);
        disk.ui.last_category = self.view.category.clone();
        disk.ui.col_name_w = self.view.col_name_w;
        disk.ui.col_path_w = self.view.col_path_w;
        disk.ui.col_size_w = self.view.col_size_w;
        disk.ui.sort_by = self
            .view
            .sort_col
            .map(|c| c.key().to_string())
            .unwrap_or_default();
        disk.ui.sort_desc = self.view.sort_desc;
        if let Some((x, y, w, h)) = crate::win::window_rect(WIN_TITLE) {
            if crate::win::rect_on_any_monitor(x, y, w, h) {
                let scale = crate::win::scale_for(WIN_TITLE).max(0.5);
                disk.ui.window_w = (w as f32 / scale).max(640.0);
                disk.ui.window_h = (h as f32 / scale).max(480.0);
                disk.ui.window_x = Some(x);
                disk.ui.window_y = Some(y);
            }
        }
        if let Err(e) = disk.save() {
            crate::win::trace(&format!("cannot save search.toml: {e}"));
            return;
        }
        self.cfg = disk;
        self.cfg_stamp = Config::stamp();
        self.cfg_dirty = None;
        self.geometry_dirty = None;
    }

    fn save_geometry(&mut self) {
        self.persist_ui();
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let (esc, enter, ctrl_enter, ctrl_c, ctrl_f, ctrl_d, ctrl_1, ctrl_2, up, down, pgup, pgdn) =
            ctx.input(|i| {
                let m = i.modifiers;
                (
                    i.key_pressed(egui::Key::Escape),
                    i.key_pressed(egui::Key::Enter),
                    m.ctrl && i.key_pressed(egui::Key::Enter),
                    m.ctrl && i.key_pressed(egui::Key::C),
                    m.ctrl && i.key_pressed(egui::Key::F),
                    m.ctrl && i.key_pressed(egui::Key::D),
                    m.ctrl && i.key_pressed(egui::Key::Num1),
                    m.ctrl && i.key_pressed(egui::Key::Num2),
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::PageUp),
                    i.key_pressed(egui::Key::PageDown),
                )
            });
        // A focused text edit owns Ctrl+C / Ctrl+F; do not hijack clipboard.
        let focused = ctx.memory(|m| m.focused().is_some());

        if esc {
            if self.view.show_scope {
                self.view.show_scope = false;
            } else if self.view.busy {
                self.cancel();
            } else {
                self.request_close();
            }
            return;
        }
        if ctrl_1 {
            self.set_mode(Mode::Name);
        }
        if ctrl_2 {
            self.set_mode(Mode::Content);
        }
        if ctrl_f {
            self.view.focus_request = true;
        }
        if ctrl_d {
            self.view.show_scope = !self.view.show_scope;
            if self.view.show_scope {
                // Ctrl+D has no button rect to anchor to; let the view pin the
                // popup under the scope button on its next frame.
                self.view.scope_anchor = egui::Pos2::ZERO;
                self.refresh_scope_items();
            }
        }
        // Enter inside the search box means "run the query", not "open the
        // previous first result"; the click/caret owns the field then.
        if enter && focused {
            self.rerun = true;
            return;
        }
        if ctrl_enter {
            if let Some(i) = self.selected_or_first() {
                if let Some(h) = self.view.hits.get(i) {
                    crate::win::reveal_file(std::path::Path::new(&h.path));
                }
            }
        } else if enter {
            if let Some(i) = self.selected_or_first() {
                if let Some(h) = self.view.hits.get(i) {
                    crate::win::open_file(std::path::Path::new(&h.path));
                }
            }
        }
        if ctrl_c && !focused {
            if let Some(i) = self.view.selected {
                if let Some(h) = self.view.hits.get(i) {
                    ctx.output_mut(|o| o.copied_text = h.path.clone());
                    self.view.status_note = Some("已复制路径".into());
                }
            }
        }
        let len = self.view.hits.len();
        if len > 0 {
            // With no selection yet, any arrow key selects the first row.
            // The old `unwrap_or(0)` made Down jump straight to row 2.
            let next = match self.view.selected {
                None => 0,
                Some(cur) if up => cur.saturating_sub(1),
                Some(cur) if down => (cur + 1).min(len - 1),
                Some(cur) if pgup => cur.saturating_sub(10),
                Some(cur) if pgdn => (cur + 10).min(len - 1),
                Some(cur) => cur,
            };
            if up || down || pgup || pgdn {
                self.view.selected = Some(next);
                self.view.scroll_to_selected = true;
            }
        }
    }

    fn selected_or_first(&self) -> Option<usize> {
        self.view
            .selected
            .or(if self.view.hits.is_empty() { None } else { Some(0) })
    }

    fn set_mode(&mut self, mode: Mode) {
        if self.view.mode == mode {
            return;
        }
        self.view.mode = mode;
        self.view.hits.clear();
        self.view.file_paths.clear();
        self.view.selected = None;
        self.cfg_dirty = Some(Instant::now());
    }

    fn drain_events(&mut self) {
        let gen = self.generation.load(Ordering::SeqCst);
        let epoch = self.engine.epoch();
        let mut changed = false;
        for _ in 0..MAX_BATCHES_PER_FRAME {
            let Ok(ev) = self.ev_rx.try_recv() else { break };
            match ev {
                Ev::Batch { gen: g, hits } if g == gen => {
                    if self.view.selected.is_none() && !hits.is_empty() {
                        self.view.selected = Some(0);
                    }
                    for h in &hits {
                        self.view.file_paths.insert(h.path.clone());
                    }
                    self.view.hits.extend(hits);
                    changed = true;
                }
                Ev::Batch { .. } => {}
                Ev::Progress {
                    gen: g,
                    epoch: e,
                    scanned,
                    indexing,
                } => {
                    // gen 0 is index-build progress; a stale epoch means the
                    // walk belongs to an index that has already been replaced.
                    let relevant = if g == 0 {
                        e == epoch
                    } else {
                        g == gen
                    };
                    if relevant {
                        self.view.scanned = scanned;
                        if !indexing {
                            self.view.indexing = false;
                        }
                    }
                }
                Ev::Done {
                    gen: g,
                    partial,
                    stats,
                } if g == gen => {
                    self.view.busy = false;
                    // Keep a UI-side truncation (more rows than the cap): the
                    // worker's "complete" must not clear it.
                    self.view.partial |= partial;
                    self.view.elapsed_ms = stats.elapsed_ms;
                    self.view.scanned = stats.scanned.max(self.view.scanned);
                    // Score order is only the default; a clicked column wins.
                    if self.view.sort_col.is_some() {
                        self.view.sort_hits();
                    } else {
                        self.view.hits.sort_by(|a, b| b.score.cmp(&a.score));
                    }
                    changed = true;
                    if self.view.selected.is_none() && !self.view.hits.is_empty() {
                        self.view.selected = Some(0);
                    }
                }
                Ev::Done { .. } => {}
                Ev::IndexReady {
                    epoch: e,
                    entries,
                    errors,
                    elapsed_ms,
                    cached,
                } if e == epoch => {
                    self.view.indexing = false;
                    self.view.index_entries = entries;
                    if errors > 0 {
                        self.view.status_note =
                            Some(format!("索引阶段跳过 {errors} 个不可读目录"));
                    } else if !cached && elapsed_ms >= 3000 && self.view.text.trim().is_empty() {
                        self.view.status_note =
                            Some(format!("索引就绪：{entries} 项，用时 {elapsed_ms} ms"));
                    }
                    if !cached {
                        // Persist the fresh walk so the next launch is instant.
                        self.engine.request_save(true);
                    }
                    // The first query may have run against a partial index (or
                    // the window was raised while a rebuild was in progress).
                    if !self.view.text.trim().is_empty() {
                        self.rerun = true;
                    }
                }
                Ev::IndexReady { .. } => {}
                Ev::Rebuild { epoch: e, reason } if e == epoch => {
                    let now = Instant::now();
                    let due = self
                        .last_rebuild
                        .map(|t| now.duration_since(t) >= Duration::from_secs(10))
                        .unwrap_or(true);
                    if due {
                        self.last_rebuild = Some(now);
                        self.engine.rebuild(&self.cfg);
                        self.generation.fetch_add(1, Ordering::SeqCst);
                        self.view.busy = false;
                        self.view.hits.clear();
                        self.view.file_paths.clear();
                        self.view.selected = None;
                        self.view.status_note = Some(format!("{reason}，已重建索引"));
                        self.rerun = !self.view.text.trim().is_empty();
                    }
                }
                Ev::Rebuild { .. } => {}
            }
        }
        // Keep at most max_results rows even if several batches arrive together.
        let cap = self.cfg.content.max_results;
        if self.view.hits.len() > cap {
            self.view.hits.truncate(cap);
            self.view.partial = true;
            changed = true;
            if self.view.mode == Mode::Content {
                // Dropping rows also drops their files from the status count.
                self.view.file_paths = self
                    .view
                    .hits
                    .iter()
                    .map(|h| h.path.clone())
                    .collect();
            }
        }
        if changed && self.view.sort_col.is_some() {
            self.view.sort_hits();
        }
        if self.view.selected.is_none() && !self.view.hits.is_empty() {
            self.view.selected = Some(0);
        }
    }

    fn apply_actions(&mut self, actions: Vec<Action>, ctx: &egui::Context) {
        for a in actions {
            match a {
                Action::Open(i) => {
                    if let Some(h) = self.view.hits.get(i) {
                        crate::win::open_file(std::path::Path::new(&h.path));
                    }
                }
                Action::Reveal(i) => {
                    if let Some(h) = self.view.hits.get(i) {
                        crate::win::reveal_file(std::path::Path::new(&h.path));
                    }
                }
                Action::Copy(i) => {
                    if let Some(h) = self.view.hits.get(i) {
                        ctx.output_mut(|o| o.copied_text = h.path.clone());
                    }
                }
                Action::CopyName(i) => {
                    if let Some(h) = self.view.hits.get(i) {
                        let name = std::path::Path::new(&h.path)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| h.path.clone());
                        ctx.output_mut(|o| o.copied_text = name);
                    }
                }
                Action::CopyDir(i) => {
                    if let Some(h) = self.view.hits.get(i) {
                        let dir = std::path::Path::new(&h.path)
                            .parent()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        ctx.output_mut(|o| o.copied_text = dir);
                    }
                }
                Action::Cancel => self.cancel(),
                Action::Submit => {
                    self.pending_query = None;
                    self.submit();
                }
                Action::ConfigChanged => self.cfg_dirty = Some(Instant::now()),
                Action::RefreshScopes => self.refresh_scope_items(),
                Action::PickFolder => self.add_custom_root(),
            }
        }
    }

    /// "选择文件夹…" in the scope popup: remember the folder in `search.toml`,
    /// switch to it and rebuild, because the index did not necessarily cover it
    /// (a UNC share is not part of any local volume).
    fn add_custom_root(&mut self) {
        let Some(path) = crate::win::pick_folder() else {
            return;
        };
        let mut disk = Config::load();
        if !disk
            .scope
            .custom_roots
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&path))
        {
            disk.scope.custom_roots.push(path.clone());
            if let Err(e) = disk.save() {
                self.view.status_note = Some(format!("无法保存检索范围：{e}"));
            }
        }
        self.scope_sig = scope_signature(&disk);
        self.cfg = disk;
        self.cfg_stamp = Config::stamp();
        self.refresh_scope_items();
        self.view.scope = ScopeSel::Path(path);
        self.view.hits.clear();
        self.view.file_paths.clear();
        self.view.selected = None;
        self.view.busy = false;
        self.engine.rebuild(&self.cfg);
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.rerun = !self.view.text.trim().is_empty();
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // egui points must equal the DPI scale; Win32 rects are physical px.
        ctx.set_pixels_per_point(crate::win::scale_for(WIN_TITLE));

        if !self.positioned {
            self.positioned = true;
            if self.start_hidden {
                self.hidden = true;
                self.hidden_at = Some(Instant::now());
                crate::win::hide_offscreen(WIN_TITLE);
            } else {
                self.bring_front();
            }
        }

        // Native close button. `release` lets the window die; the other two
        // residency modes keep the index and hide instead.
        if ctx.input(|i| i.viewport().close_requested()) {
            if !self.allow_exit && self.cfg.activation.residency != "release" {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.save_geometry();
                self.hide(false);
            } else {
                self.save_geometry();
                self.engine.save_blocking();
            }
        }

        let stamp = crate::win::show_marker_stamp();
        if stamp != self.show_stamp {
            self.show_stamp = stamp;
            if self.marker_seen {
                self.toggle_window();
            } else {
                self.marker_seen = true;
                self.bring_front();
            }
        }

        // Hot-reload settings only when we are not mid-edit ourselves.
        if self.cfg_dirty.is_none() {
            let st = Config::stamp();
            if st != self.cfg_stamp {
                let old = self.cfg.clone();
                self.cfg = Config::load();
                self.cfg_stamp = st;
                let new_sig = scope_signature(&self.cfg);
                self.refresh_scope_items();
                if new_sig != self.scope_sig {
                    // Scope/exclude/hidden changes alter what the index must
                    // contain: rebuild instead of serving an incomplete set.
                    self.scope_sig = new_sig;
                    self.engine.rebuild(&self.cfg);
                    self.generation.fetch_add(1, Ordering::SeqCst);
                    self.view.busy = false;
                    self.view.hits.clear();
                    self.view.file_paths.clear();
                    self.view.selected = None;
                    self.view.status_note = Some("检索范围已更新，正在重建索引…".into());
                    self.rerun = !self.view.text.trim().is_empty();
                } else if old.content != self.cfg.content && !self.view.text.trim().is_empty() {
                    // Size/encoding/budget/cap changes only affect the next
                    // query; re-run it so the visible rows match the new rule.
                    self.rerun = true;
                }
            }
        }

        self.handle_keys(ctx);
        self.drain_events();
        self.engine.tick_save();

        if self.rerun {
            self.rerun = false;
            if !self.view.text.trim().is_empty() {
                self.pending_query = None;
                self.submit();
            }
        }

        // 80 ms input debounce → new generation → old workers wind down.
        let sig = self.view.signature();
        if sig != self.last_signature {
            self.last_signature = sig;
            self.pending_query = Some(Instant::now());
        }
        if let Some(t) = self.pending_query {
            if t.elapsed().as_millis() >= DEBOUNCE_MS as u128 {
                self.pending_query = None;
                self.submit();
            }
        }
        if let Some(t) = self.cfg_dirty {
            if t.elapsed().as_millis() >= CONFIG_DEBOUNCE_MS as u128 {
                self.persist_ui();
            }
        }

        self.view.indexing = self.engine.indexing.load(Ordering::SeqCst);
        let skin = Skin::system();
        let actions = egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(skin.bg))
            .show(ctx, |ui| view::draw(ui, &mut self.view, &skin))
            .inner;
        self.apply_actions(actions, ctx);

        self.watch_geometry();
        if let Some(t) = self.geometry_dirty {
            if t.elapsed() >= Duration::from_millis(700) {
                self.save_geometry();
            } else {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }

        // A hidden window still polls the show marker and the exit deadline.
        if let Some(deadline) = self.exit_deadline() {
            if Instant::now() >= deadline {
                self.save_geometry();
                self.engine.save_blocking();
                self.allow_exit = true;
                crate::win::close_window(WIN_TITLE);
            } else {
                ctx.request_repaint_after(Duration::from_millis(200));
            }
        }

        let interval = if self.hidden {
            200
        } else if self.view.busy || self.view.indexing || self.pending_query.is_some() {
            33
        } else {
            120
        };
        ctx.request_repaint_after(Duration::from_millis(interval));
    }
}
