//! The one and only event loop of `dcx-box.exe`.
//!
//! # What changed and why
//!
//! Before the rewrite the tile lived in the main window and the panel in a
//! deferred viewport whose position was pushed through asynchronous viewport
//! commands every frame. Two problems followed directly from that:
//!
//! * **拖动失效** — the panel's egui coordinates fed back into a window that
//!   was moving under the pointer, and the fallback OS-cursor state machine ran
//!   in a frame loop winit was free to throttle.
//! * **按钮漂移** — the tile's x came from `slot.left`, the right edge of the
//!   *last app button*. Every program start/close moved that number, and the
//!   engine's own 52 DIP reservation changes the pill width too, so the anchor
//!   chased its own tail.
//!
//! Now there is one window, positioned with a synchronous `SetWindowPos`, and
//! the default anchor is the tray's left edge — a number that does **not**
//! depend on how many app buttons are open. A `TileMode::Fixed` option locks an
//! absolute pixel position, and a short stability filter absorbs the residual
//! tray-width jitter. See [`App::compute_geo`].

use crate::desk_place::Placer;
use crate::file_icon::IconCache;
use crate::model::{Cmd, Dialog, Model, Tab};
use crate::{shell_io, tile, ui, view, win};
use dcx_organizer::ViewMode;
use egui::{Color32, Pos2, Rect, TextureHandle, Vec2};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::platform::windows::EventLoopBuilderExtWindows;

pub const WIN_TITLE: &str = "桌面收纳盒";

/// Outward padding kept around the panel for its soft shadow. Zero while we
/// check whether the translucent ring is what makes DWM flash the window on a
/// click: with no ring the window rect *is* the panel.
pub(crate) const SHADOW: f32 = 0.0;
const GAP: f32 = 8.0;
const HOVER_OPEN: Duration = Duration::from_millis(110);
const HOVER_CLOSE: Duration = Duration::from_millis(320);
/// Movement before a press on the header turns into a window move. At 4 pt a
/// click with a little hand shake dragged the panel a few points and detached
/// it from the tile — the other half of "the window jumps on an ambiguous
/// click". It is also the tolerance for the header double-click.
const DRAG_THRESHOLD: f32 = 6.0;
/// Movement needed before an edge press becomes a resize. The band along each
/// border is 5 pt wide and sits right where a click is "inside or outside?"
/// — at 2.5 pt a slightly shaky click resized the panel by a few points (and
/// persisted it), which read as the window jumping on its own.
const RESIZE_THRESHOLD: f32 = 5.0;
const GHOST_W: f32 = 316.0;
const GHOST_H: f32 = 44.0;
const MIN_PANEL_W: f32 = 320.0;
const MIN_PANEL_H: f32 = 260.0;
const MAX_PANEL_W: f32 = 1100.0;

static STARTED: AtomicBool = AtomicBool::new(false);

/// Start the one eframe loop. `main` blocks on the handle so a WM_CLOSE from
/// the host's app manager ends the process cleanly.
pub fn spawn() -> Option<std::thread::JoinHandle<()>> {
    if STARTED.swap(true, Ordering::SeqCst) {
        return None;
    }
    Some(std::thread::spawn(|| {
        let result = eframe::run_native(
            WIN_TITLE,
            native_options(),
            Box::new(|cc| {
                ui::setup_fonts(&cc.egui_ctx);
                Ok(Box::new(App::new(&cc.egui_ctx)))
            }),
        );
        if let Err(e) = result {
            eprintln!("[dcx-box] window failed: {e}");
            win::trace(&format!("eframe failed: {e}"));
            STARTED.store(false, Ordering::SeqCst);
        }
    }))
}

/// Visible dock height in DIPs, as the engine applies it to the taskbar's XAML
/// root. Missing settings fall back to the same default the engine uses.
fn dock_height() -> f32 {
    dcx_settings::Settings::load().effective_height() as f32
}

fn native_options() -> eframe::NativeOptions {
    let scale = win::scale_for(0, win::slot_dpi_cached());
    let screen = win::work_area(scale);
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([view::BTN_W + 8.0, view::BTN_H + 8.0])
            .with_position([screen.right() - 320.0, screen.bottom() - 72.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(true)
            .with_taskbar(false)
            .with_always_on_top()
            .with_min_inner_size([view::BTN_W, view::BTN_H])
            .with_icon(ui::app_icon()),
        event_loop_builder: Some(Box::new(|b| {
            b.with_any_thread(true);
        })),
        ..Default::default()
    }
}

#[derive(Clone, Copy, Debug)]
struct Geo {
    tile: Rect,
    panel: Rect,
    header: Rect,
    /// Auto-hide has the bar parked off-screen; the tile keeps a valid
    /// position (off-screen with the bar) but must not be shown at it.
    bar_parked: bool,
}

struct PanelDrag {
    start_cursor: Pos2,
    start_panel: Pos2,
}

struct ResizeDrag {
    start_cursor: Pos2,
    start_panel: Rect,
    edge: view::Edge,
}

struct RowDrag {
    path: std::path::PathBuf,
    from: String,
    started: Instant,
}

/// Where a drop would land. Resolved from the rects painted last frame, so a
/// drop delivered before the current frame's draw still routes correctly.
enum DropRoute {
    Folder(std::path::PathBuf),
    Category(String),
    Auto,
}

struct Ghost {
    label: String,
    icon: Option<TextureHandle>,
    frames: u32,
}

struct ViewOutputs {
    row_drag_start: Option<(std::path::PathBuf, String)>,
    tab_rects: Vec<(Tab, Rect)>,
    folder_rects: Vec<(std::path::PathBuf, Rect)>,
    drop_folder: Option<std::path::PathBuf>,
    popup_rect: Option<Rect>,
    /// Scrollbar in screen points (from the previous frame), so a drag that
    /// starts on the thumb is not mistaken for a resize of the east edge.
    scrollbar_rect: Option<Rect>,
    search_focused: bool,
}

pub struct App {
    ctx: egui::Context,
    win: win::Window,
    tile: tile::Tile,
    model: Model,
    icons: IconCache,
    placer: Placer,
    scale: f32,
    /// Visible dock height in DIPs, as applied to the XAML root. The shell
    /// window stays 48 DIP, so this — not `Shell_TrayWnd` — is the height the
    /// drawer button and its hover plate must fit.
    dock_h: f32,

    slot: Option<win::Slot>,
    app_right: Option<f32>,
    app_probe_at: Instant,

    geo: Option<Geo>,
    open: bool,
    pinned: bool,
    shown: bool,
    hover_since: Option<Instant>,
    leave_since: Option<Instant>,
    lbtn_prev: bool,
    esc_prev: bool,
    press_over_ui: bool,
    press_was_gesture: bool,

    panel_pos: Option<Pos2>,
    panel_drag: Option<PanelDrag>,
    panel_press: Option<Pos2>,
    resize_drag: Option<ResizeDrag>,
    resize_press: Option<(Pos2, view::Edge)>,
    last_header_click: Option<Instant>,
    row_drag: Option<RowDrag>,
    ghost: Option<Arc<Mutex<Ghost>>>,

    anchor_x: Option<f32>,
    bar_sig: [i32; 4],

    tab_rects: Vec<(Tab, Rect)>,
    drop_tab: Option<String>,
    folder_rects: Vec<(std::path::PathBuf, Rect)>,
    drop_folder: Option<std::path::PathBuf>,
    origin: Pos2,
    popup_rect: Option<Rect>,
    /// Scrollbar in screen points (from the previous frame), so a drag that
    /// starts on the thumb is not mistaken for a resize of the east edge.
    scrollbar_rect: Option<Rect>,
    search_focused: bool,

    last_house: Instant,
    heartbeat_at: Instant,
    state_at: Instant,
    /// What DWM backdrop is currently applied to the panel window.
    backdrop_on: Option<bool>,
    /// Flight recorder for "the window twitched": every mouse edge and every
    /// change of (open, shown, geo-present) goes to `box_trace.log`.
    last_state: Option<(bool, bool, bool, bool)>,
    lbtn_dbg: bool,
    /// `box_show` marker (host writes it on the global hotkey). The first
    /// change after process start means "show", later ones toggle.
    show_stamp: u128,
    marker_seen: bool,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        // The collection model keeps Explorer's desktop native. An older build
        // may have hidden the icon layer and left its ownership marker behind;
        // give those icons back once.
        restore_owned_desktop_icons();
        let model = Model::new();
        let state = win::read_ui_state();
        let panel_pos = match (state.get("panel_x"), state.get("panel_y")) {
            (Some(x), Some(y)) => Some(Pos2::new(*x, *y)),
            _ => None,
        };
        // Launched by the global hotkey: the host touches the marker *before*
        // this process finishes starting, so a stale-looking stamp is really
        // the show request for this very launch.
        let launched_by_hotkey = win::show_marker_time()
            .map(|t| t >= win::process_started())
            .unwrap_or(false);
        // The taskbar button lives in its own window; hand it the egui context
        // so a click can wake this loop instead of waiting for the idle tick.
        tile::bind_ctx(ctx);
        App {
            ctx: ctx.clone(),
            win: win::Window::new(),
            tile: tile::Tile::new(),
            model,
            icons: IconCache::new(),
            placer: Placer::spawn(),
            scale: 1.0,
            dock_h: dock_height(),
            slot: None,
            app_right: None,
            app_probe_at: Instant::now() - Duration::from_secs(10),
            geo: None,
            open: false,
            pinned: false,
            shown: true,
            hover_since: None,
            leave_since: None,
            lbtn_prev: false,
            esc_prev: false,
            press_over_ui: false,
            press_was_gesture: false,
            panel_pos,
            panel_drag: None,
            panel_press: None,
            resize_drag: None,
            resize_press: None,
            last_header_click: None,
            row_drag: None,
            ghost: None,
            anchor_x: None,
            bar_sig: [0; 4],
            tab_rects: Vec::new(),
            drop_tab: None,
            folder_rects: Vec::new(),
            drop_folder: None,
            origin: Pos2::ZERO,
            popup_rect: None,
            scrollbar_rect: None,
            search_focused: false,
            last_house: Instant::now(),
            heartbeat_at: Instant::now(),
            state_at: Instant::now(),
            backdrop_on: None,
            last_state: None,
            lbtn_dbg: false,
            show_stamp: if launched_by_hotkey {
                0
            } else {
                win::show_marker_stamp()
            },
            marker_seen: false,
        }
    }

    fn cursor(&self) -> Option<Pos2> {
        win::cursor_px().map(|p| Pos2::new(p.x / self.scale, p.y / self.scale))
    }

    /// Push the configured backdrop to DWM when it differs from what the window
    /// already has. Safe to call every frame: it is a comparison plus, on a
    /// change, a couple of attribute writes.
    ///
    /// The corner preference travels with it: the acrylic backdrop is a full
    /// rectangle, so with our own 12 pt rounding the four corners showed a
    /// translucent wedge *outside* the painted panel. Letting DWM round the
    /// window clips the backdrop and the tint with the same curve.
    fn apply_backdrop(&mut self) {
        let want = self.model.cfg.acrylic;
        if self.backdrop_on == Some(want) || self.win.hwnd == 0 {
            return;
        }
        // Nothing to push to DWM; kept as the hook where a future blur pass
        // would start.
        self.backdrop_on = Some(want);
    }

    // -- geometry -----------------------------------------------------------

    /// Resolve the tile anchor: **right after the last program icon**, inside
    /// the band the engine reserves.
    ///
    /// There is one rule, not three modes. `TileMode` / `tile_offset` /
    /// `tile_fixed_x` are still carried in `organizer.toml` because
    /// `persist_prefs` round-trips them, but nothing here reads them and the
    /// settings centre has no control for them — the reservation model and a
    /// user-locked x cannot coexist, since the slot moves with the app row.
    ///
    /// Both ends of the reserved band drift with the number of app buttons:
    /// `slot.left` (last icon's right edge) obviously so, and `slot.limit`
    /// (the tray edge) because the pill is right-aligned against the tray.
    /// Anchoring on `limit` therefore never fixed the drift the old `Tray`
    /// mode complained about; it only moved it. The button's own z-order is
    /// re-asserted separately by the z-order guard in [`crate::tile`].
    fn compute_geo(&mut self) -> Option<Geo> {
        let scale = self.scale;
        let tb = win::taskbar(scale)?;
        let screen = win::work_area(scale);
        // Follow the visible dock, not the fixed 48 DIP shell window: the
        // engine applies `taskbar_height` to the XAML row. `min` also keeps the
        // button inside the task list if a shell build reports a shorter row
        // than the setting. A fixed 42/40 pt button kept its size when the
        // taskbar was shrunk, so its hover/selected plate spilled above and
        // below the dock.
        let row_h = if tb.row_h_px > 0 {
            tb.row_h_px as f32 / scale
        } else {
            self.dock_h
        };
        let btn_h = self.dock_h.min(row_h).clamp(28.0, 60.0);
        let btn_w = view::BTN_W.min(btn_h + 2.0);

        let limit = self
            .slot
            .map(|s| s.limit / scale)
            .filter(|v| *v > tb.rect.left() + 40.0)
            .unwrap_or(tb.tray_left);
        // One rule, no modes: sit **right after the last program icon**. The
        // engine reserves a 52 DIP band at the end of the app row, so `left` is
        // the last icon's right edge and the tile belongs at `left + 4` — 4 DIP
        // of air on both sides of the plate. `limit` (the pill's right edge) is
        // only a safety clamp for the case where the reservation could not be
        // written. The old `Tray` mode anchored on `limit`, which is derived from
        // the icon row anyway and therefore drifted with every program that
        // opened — the exact complaint it was meant to fix.
        let anchor = match self.slot {
            Some(s) if s.right > 0.0 => (s.right / scale + 4.0)
                .min(limit - btn_w - 2.0)
                .max(tb.rect.left() + 4.0),
            _ => (limit - btn_w - 4.0).max(tb.rect.left() + 4.0),
        };
        if self
            .anchor_x
            .map(|prev| (prev - anchor).abs() > 0.5)
            .unwrap_or(false)
        {
            win::trace(&format!(
                "anchor -> {anchor:.1} (limit={limit:.1} slot={:?} scale={scale})",
                self.slot.map(|s| (s.right, s.limit, s.dpi))
            ));
        }
        self.anchor_x = Some(anchor);

        let sig = [
            (tb.rect.left() * scale).round() as i32,
            (tb.rect.top() * scale).round() as i32,
            (tb.rect.right() * scale).round() as i32,
            (tb.rect.bottom() * scale).round() as i32,
        ];
        self.bar_sig = sig;

        let tile = Rect::from_min_size(
            Pos2::new(anchor, tb.rect.center().y - btn_h / 2.0),
            Vec2::new(btn_w, btn_h),
        );

        let panel_w = (self.model.cfg.panel_width as f32)
            .clamp(MIN_PANEL_W, MAX_PANEL_W.min(screen.width() - 16.0));
        let panel_h = (self.model.cfg.panel_height as f32)
            .clamp(MIN_PANEL_H, (screen.height() - 40.0).max(MIN_PANEL_H));
        let panel = match self.panel_pos {
            Some(p) => Rect::from_min_size(
                Pos2::new(
                    p.x.clamp(4.0, (screen.right() - panel_w - 4.0).max(4.0)),
                    p.y.clamp(4.0, (screen.bottom() - panel_h - 4.0).max(4.0)),
                ),
                Vec2::new(panel_w, panel_h),
            ),
            None => {
                // While the panel is open, keep the rectangle it opened with.
                // The engine re-measures the taskbar band for about a second
                // after the box starts, and a follow position that tracked that
                // moved the whole window under the cursor — read as "the window
                // jumps when I click". Closing releases it, so the next open
                // lands on the current anchor.
                if self.open {
                    if let Some(prev) = self.geo {
                        prev.panel
                    } else {
                        follow_panel(tile, panel_w, panel_h, screen)
                    }
                } else {
                    follow_panel(tile, panel_w, panel_h, screen)
                }
            }
        };
        let header = Rect::from_min_max(
            panel.min + Vec2::new(8.0, 0.0),
            Pos2::new(
                panel.right() - view::header_actions_w(panel.width()) - 8.0,
                panel.top() + view::HEADER_H,
            ),
        );
        Some(Geo {
            tile,
            panel,
            header,
            bar_parked: tb.parked,
        })
    }

    // -- timers -------------------------------------------------------------

    fn tick(&mut self) {
        let now = Instant::now();
        if self.heartbeat_at.elapsed() >= Duration::from_secs(1) {
            self.heartbeat_at = now;
            win::heartbeat();
        }
        if let Some(t) = self.model.alpha_dirty {
            if t.elapsed() >= Duration::from_millis(300) {
                self.model.alpha_dirty = None;
                self.model.persist_prefs();
            }
        }
        if self.last_house.elapsed() >= Duration::from_secs(1) {
            self.last_house = now;
            self.model.check_config();
            // Same cadence as the engine's own settings poll: the tile follows
            // a taskbar-height change within about a second.
            self.dock_h = dock_height();
            self.win.housekeeping();
            // The bar's own rectangle cannot tell "visible" from "auto-hidden
            // but out", so the shell is asked once a second and the slide
            // watcher is armed only while auto-hide really is on.
            tile::set_auto_hide(self.model.cfg.enabled && win::taskbar_auto_hide());
            if self.open && self.model.last_refresh.elapsed() >= Duration::from_millis(1200) {
                self.model.refresh();
            }
        }
        if self.state_at.elapsed() >= Duration::from_secs(1) {
            self.state_at = now;
            if let Some(g) = self.geo {
                // Publish the *clamped* on-screen origin rather than the raw
                // drag position: a drag can end outside the work area, and
                // restoring that raw value on the next launch would pin the
                // panel somewhere the user never saw it.
                let panel = self.panel_pos.map(|_| g.panel.min);
                win::write_ui_state(panel, g.tile.min, self.scale);
            }
        }
        // Published by the slot watcher (timer rate): the row's end has to
        // reach the frame that the watcher just woke, not the next 400 ms poll.
        if let Some(s) = win::latest_slot() {
            self.slot = Some(s);
        }
        if self.slot.is_none() && self.app_probe_at.elapsed() >= Duration::from_millis(1500) {
            self.app_probe_at = now;
            self.app_right = app_area_right(self.scale);
        }
    }

    /// Put collection items back on the real desktop. The files are moved and
    /// Explorer's own icons appear; `at` positions the first icon at the drop.
    fn release_to_desktop(&mut self, paths: &[std::path::PathBuf], at: Option<Pos2>) -> Vec<std::path::PathBuf> {
        let out = self.model.move_to_desktop(paths);
        if let Some(cur) = at {
            let px = (
                (cur.x * self.scale).round() as i32,
                (cur.y * self.scale).round() as i32,
            );
            for p in &out {
                self.placer.release(p, Some(px));
            }
        }
        out
    }

    /// Global hotkey (`box_show`): the first change after launch shows the
    /// panel, later presses toggle it, mirroring the search window.
    fn poll_show_marker(&mut self) {
        let stamp = win::show_marker_stamp();
        if stamp == self.show_stamp {
            return;
        }
        self.show_stamp = stamp;
        if self.marker_seen {
            if self.open {
                self.close_panel();
            } else {
                self.open_panel(true);
            }
        } else {
            self.marker_seen = true;
            self.open_panel(true);
        }
    }

    // -- pointer ------------------------------------------------------------

    fn open_panel(&mut self, pinned: bool) {
        win::trace(&format!("open_panel pinned={pinned} already={}", self.open));
        if !self.open {
            self.open = true;
            self.hover_since = None;
            // Files saved while the panel was closed were not scanned (and
            // therefore not absorbed). Capture them, then land on 「最近」 so
            // the just-saved file is the first thing in view.
            let new_files = self.model.refresh_collect_new();
            if !new_files.is_empty() {
                self.model.show_recent(new_files.len());
            }
        }
        if pinned {
            self.pinned = true;
        }
        self.leave_since = None;
    }

    fn close_panel(&mut self) {
        win::trace("close_panel");
        self.open = false;
        self.pinned = false;
        self.row_drag = None;
        self.ghost = None;
        self.model.popup = None;
        self.model.dialog = None;
        self.leave_since = None;
    }

    fn pump_pointer(&mut self, geo: Geo) {
        let cur = self.cursor();
        // The button is its own window; its hover state comes from its own
        // WM_MOUSEMOVE / WM_MOUSELEAVE, not from a global cursor guess.
        let over_tile = tile::hovered();
        let over_panel = cur.map(|c| geo.panel.contains(c)).unwrap_or(false);
        let pressed = win::key_down(win::VK_LBUTTON);
        // Global Escape: the light-dismiss flyout may not own keyboard focus
        // (the user can click the desktop first), so egui's Key event alone is
        // not enough to close it.
        let esc = win::key_down(win::VK_ESCAPE);
        let esc_edge = esc && !self.esc_prev;
        self.esc_prev = esc;
        let clicked = pressed && !self.lbtn_prev;
        let released = !pressed && self.lbtn_prev;
        self.lbtn_prev = pressed;

        if clicked {
            self.press_over_ui = over_tile
                || over_panel
                || self
                    .popup_rect
                    .map(|r| cur.map(|c| r.contains(c)).unwrap_or(false))
                    .unwrap_or(false);
            self.press_was_gesture = false;
        }
        if pressed
            && (self.panel_drag.is_some()
                || self.resize_drag.is_some()
                || self.row_drag.is_some()
                || self.model.tab_drag.is_some()
                || self.model.popup.is_some())
        {
            self.press_was_gesture = true;
        }

        // -- panel move / resize, driven from this one frame loop ------------
        if self.open {
            let header_hit = cur.map(|c| geo.header.contains(c)).unwrap_or(false);
            let edge_hit = cur
                .and_then(|c| view::edge_at(geo.panel, c))
                .filter(|_| !self.blocked_by_popup())
                // The scrollbar sits inside the east band; a press there belongs
                // to the list, not to the window edge.
                .filter(|_| {
                    !cur.map(|c| self.scrollbar_rect.map(|r| r.contains(c)).unwrap_or(false))
                        .unwrap_or(false)
                });
            // Press **edges** are unreliable: the idle frame loop runs at
            // 200 ms, so a quick click can start and end between two frames and
            // `clicked` never fires. Recording "a press is in progress" instead
            // is edge-free: it stays true for as long as the button is down.
            //
            // Move and resize are mutually exclusive: the resize band lies
            // *inside* the panel, so a press near the top edge would otherwise
            // start both gestures and a simple header drag would silently
            // change the height.
            if pressed
                && self.panel_press.is_none()
                && self.resize_press.is_none()
                && self.model.dialog.is_none()
            {
                match (cur, edge_hit) {
                    (Some(c), Some(edge)) => self.resize_press = Some((c, edge)),
                    (Some(c), None) if header_hit => self.panel_press = Some(c),
                    _ => {}
                }
            }
            if let (Some(start), Some(c)) = (self.panel_press, cur) {
                if pressed && self.panel_drag.is_none() && (c - start).length() > DRAG_THRESHOLD {
                    let start_panel = self.panel_pos.unwrap_or(geo.panel.min);
                    self.panel_drag = Some(PanelDrag {
                        start_cursor: start,
                        start_panel,
                    });
                    self.press_was_gesture = true;
                    win::trace("panel drag start");
                }
            }
            if let (Some((start, edge)), Some(c)) = (self.resize_press, cur) {
                if pressed && self.resize_drag.is_none() && (c - start).length() > RESIZE_THRESHOLD {
                    self.resize_drag = Some(ResizeDrag {
                        start_cursor: start,
                        start_panel: geo.panel,
                        edge,
                    });
                    self.press_was_gesture = true;
                }
            }
            if let (Some(d), Some(c)) = (&self.panel_drag, cur) {
                if pressed {
                    self.panel_pos = Some(d.start_panel + (c - d.start_cursor));
                }
            }
            if let (Some(d), Some(c)) = (&self.resize_drag, cur) {
                if pressed {
                    let r = resize_rect(d, c, self.scale);
                    self.panel_pos = Some(r.min);
                    self.model.cfg.panel_width = r.width().round() as u32;
                    self.model.cfg.panel_height = r.height().round() as u32;
                }
            }
            if released {
                if self.panel_drag.take().is_some() {
                    win::trace(&format!("panel drag end at {:?}", self.panel_pos));
                }
                if self.resize_drag.take().is_some() {
                    self.model.persist_prefs();
                }
                // Double-click the header returns the panel above the tile.
                if let Some(start) = self.panel_press.take() {
                    let moved = cur
                        .map(|c| (c - start).length() < DRAG_THRESHOLD)
                        .unwrap_or(false);
                    if moved && header_hit {
                        let now = Instant::now();
                        if self
                            .last_header_click
                            .map(|t| now.duration_since(t) < Duration::from_millis(350))
                            .unwrap_or(false)
                        {
                            self.panel_pos = None;
                            self.last_header_click = None;
                            // Drop the freeze so the next frame recomputes the
                            // follow position instead of keeping the old one.
                            self.geo = None;
                            self.model.toast("已回到任务栏按钮上方");
                        } else {
                            self.last_header_click = Some(now);
                        }
                    }
                }
                self.resize_press = None;
            }
            // Clicking outside a popup closes it.
            if clicked && self.model.popup.is_some() {
                let in_popup = self
                    .popup_rect
                    .map(|r| cur.map(|c| r.contains(c)).unwrap_or(false))
                    .unwrap_or(false);
                if !in_popup {
                    self.model.close_popup();
                }
            }
        } else {
            self.panel_press = None;
            self.resize_press = None;
            self.panel_drag = None;
            self.resize_drag = None;
        }

        // -- open / dismiss --------------------------------------------------
        if !self.open {
            if over_tile {
                let since = *self.hover_since.get_or_insert_with(Instant::now);
                let hover_opens = self.model.cfg.open_mode == dcx_organizer::OpenMode::Hover;
                if hover_opens && since.elapsed() >= HOVER_OPEN {
                    self.open_panel(false);
                }
            } else {
                self.hover_since = None;
            }
            return;
        }
        if esc_edge {
            if self.model.dialog.is_some() {
                self.model.dialog = None;
                return;
            }
            if self.model.popup.is_some() {
                self.model.close_popup();
                return;
            }
            self.close_panel();
            return;
        }
        // `!over_tile`: a click that opened the panel is also a release outside
        // it. Without this the same gesture would dismiss what it just opened.
        if released
            && self.model.cfg.close_on_outside
            && !self.press_over_ui
            && !self.press_was_gesture
            && !over_tile
            && self.model.dialog.is_none()
        {
            self.close_panel();
            return;
        }
        let inside = over_panel
            || over_tile
            || self.row_drag.is_some()
            || self.model.tab_drag.is_some()
            || self.panel_drag.is_some()
            || self.model.popup.is_some()
            || self.model.dialog.is_some();
        if inside || self.pinned {
            self.leave_since = None;
            if self.row_drag.is_some() {
                self.pinned = true;
            }
        } else {
            let since = *self.leave_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= HOVER_CLOSE {
                self.close_panel();
            }
        }
    }

    // -- keyboard -----------------------------------------------------------

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let events: Vec<egui::Event> = ctx.input(|i| i.events.clone());
        for ev in events {
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = ev
            else {
                continue;
            };
            if key == egui::Key::Escape {
                if self.model.dialog.is_some() {
                    self.model.dialog = None;
                } else if self.model.popup.is_some() {
                    self.model.close_popup();
                } else if !self.model.query.is_empty() {
                    self.model.query.clear();
                    self.model.on_query_changed();
                } else {
                    self.close_panel();
                }
                continue;
            }
            // A dialog or an open menu owns the keyboard. Without this, Enter
            // in the rename box also opened the selected file (and Delete
            // replaced the box with the delete confirmation).
            if self.model.dialog.is_some() || self.model.popup.is_some() {
                continue;
            }
            if self.search_focused {
                continue;
            }
            if modifiers.ctrl && key == egui::Key::F {
                self.model.search_focus = true;
                continue;
            }
            if modifiers.ctrl && key == egui::Key::C {
                let text = self
                    .model
                    .selected_paths()
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    ctx.copy_text(text);
                    self.model.toast("已复制路径");
                }
                continue;
            }
            if modifiers.ctrl && key == egui::Key::A {
                self.model.select_all();
                continue;
            }
            if key == egui::Key::Delete {
                let paths = self.model.selected_paths();
                if !paths.is_empty() {
                    self.push(Cmd::AskDelete(paths));
                }
                continue;
            }
            if key == egui::Key::F2 {
                if let Some(p) = self.model.selected_paths().into_iter().next() {
                    self.push(Cmd::RenameDialog(p));
                }
                continue;
            }
            if key == egui::Key::Enter {
                if let Some(p) = self.model.selected_paths().into_iter().next() {
                    self.push(Cmd::Open(p));
                }
                continue;
            }
            if key == egui::Key::ArrowDown || key == egui::Key::ArrowUp {
                let step = if key == egui::Key::ArrowDown { 1 } else { -1 };
                let rows = self.model.rows.clone();
                if !rows.is_empty() {
                    let cur = self
                        .model
                        .selection
                        .iter()
                        .next()
                        .and_then(|p| rows.iter().position(|r| r.path() == p))
                        .unwrap_or(0);
                    let next = (cur as i32 + step).clamp(0, rows.len() as i32 - 1) as usize;
                    self.model
                        .clicked_row(rows[next].path(), next, false, false);
                    let row_h = if self.model.cfg.view_mode == ViewMode::List {
                        view::LIST_ROW
                    } else {
                        view::CARD_H
                    };
                    self.model.list_scroll = (next as f32 * row_h - 100.0).max(0.0);
                }
            }
        }
    }

    fn push(&mut self, cmd: Cmd) {
        self.model.push(cmd);
    }

    /// A popup or modal owns the interaction: edge drags must not start under it.
    fn blocked_by_popup(&self) -> bool {
        self.model.popup.is_some() || self.model.dialog.is_some()
    }

    // -- external drop ------------------------------------------------------

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let (dropped, hovering) =
            ctx.input(|i| (i.raw.dropped_files.clone(), !i.raw.hovered_files.is_empty()));
        self.model.drag_in = hovering;
        if hovering && !self.open {
            self.open_panel(false);
        }
        if dropped.is_empty() {
            return;
        }
        let paths: Vec<std::path::PathBuf> = dropped.iter().filter_map(|d| d.path.clone()).collect();
        if paths.is_empty() {
            return;
        }
        // Route by what is under the pointer *now*: `drop_tab` is a preview
        // computed from the previous frame and a drop can arrive without one.
        let route = self
            .cursor()
            .map(|c| self.drop_route_at(c))
            .unwrap_or(DropRoute::Auto);
        match route {
            // A folder row on the desktop tab is a real desktop folder; an
            // external drop there is still a collect, not a move into that
            // folder, because the box's job is to take it off the desktop.
            DropRoute::Folder(_) if self.model.active == Tab::Desktop => {
                self.model.absorb_auto(&paths)
            }
            DropRoute::Folder(dir) => self.model.move_to_folder(&paths, &dir),
            DropRoute::Category(key) => self.model.absorb(&paths, &key),
            DropRoute::Auto => self.model.absorb_auto(&paths),
        }
        self.drop_tab = None;
        self.drop_folder = None;
    }

    fn drop_route_at(&self, cur: Pos2) -> DropRoute {
        let local = cur - self.origin.to_vec2();
        if let Some(folder) = self
            .folder_rects
            .iter()
            .find(|(_, r)| r.contains(local))
            .map(|(p, _)| p.clone())
        {
            return DropRoute::Folder(folder);
        }
        if let Some(tab) = self
            .tab_rects
            .iter()
            .find(|(_, r)| r.contains(local))
            .map(|(t, _)| t.clone())
        {
            return match tab {
                Tab::Key(k) => DropRoute::Category(k),
                Tab::Desktop | Tab::All | Tab::Recent => DropRoute::Auto,
            };
        }
        DropRoute::Auto
    }

    fn category_tab_at(&self, local: Pos2) -> Option<String> {
        self.tab_rects
            .iter()
            .find(|(_, r)| r.contains(local))
            .and_then(|(t, _)| t.key().map(|s| s.to_string()))
    }

    // -- commands -----------------------------------------------------------

    fn exec(&mut self, cmd: Cmd, ctx: &egui::Context) {
        match cmd {
            Cmd::Open(p) => match shell_io::open_path(&p) {
                // Launcher behaviour: the drawer has done its job once the file
                // is on its way. Off keeps it open for opening several files.
                Ok(()) => {
                    if self.model.cfg.close_after_open {
                        self.close_panel();
                    }
                }
                // Keep the panel open so the toast is visible instead of the
                // click looking like it did nothing.
                Err(e) => self.model.toast(format!("无法打开：{e}")),
            },
            Cmd::Collect(paths, key) => self.model.collect(&paths, key.as_deref()),
            Cmd::AskCollectDesktop => {
                let paths = self.model.desktop_paths();
                if paths.is_empty() {
                    self.model.toast("桌面上没有可收纳的项目");
                } else {
                    self.model.dialog = Some(Dialog::ConfirmCollectDesktop { paths });
                }
            }
            Cmd::CollectDesktop(paths) => self.model.collect_desktop(&paths),
            Cmd::OpenLibrary => match shell_io::open_dir(&self.model.library_folder()) {
                Ok(()) => self.close_panel(),
                Err(e) => self.model.toast(format!("无法打开收纳库：{e}")),
            },
            Cmd::CopyLibraryPath => {
                let path = self.model.library_folder();
                ctx.copy_text(path.to_string_lossy().to_string());
                self.model.toast("已复制收纳库路径");
            }
            // Anything that opens a window of its own has to dismiss the panel
            // first: it is topmost, so Explorer windows, the properties sheet
            // and the settings centre all ended up behind it.
            Cmd::Reveal(p) => match shell_io::reveal(&p) {
                Ok(()) => self.close_panel(),
                Err(e) => self.model.toast(format!("无法在资源管理器中显示：{e}")),
            },
            Cmd::Properties(p) => match shell_io::open_properties(&p) {
                Ok(()) => self.close_panel(),
                Err(e) => self.model.toast(format!("无法打开属性：{e}")),
            },
            Cmd::CopyText(s) => {
                ctx.copy_text(s);
                self.model.toast("已复制");
            }
            Cmd::OpenDesktop => match shell_io::open_dir(self.model.org.desktop()) {
                Ok(()) => self.close_panel(),
                Err(e) => self.model.toast(format!("无法打开桌面目录：{e}")),
            },
            Cmd::Refresh => self.model.refresh(),
            Cmd::ClosePanel => self.close_panel(),
            Cmd::FocusSearch => self.model.search_focus = true,
            Cmd::AskDelete(paths) => {
                if self.model.cfg.confirm_delete {
                    self.model.dialog = Some(Dialog::ConfirmDelete { paths });
                } else {
                    self.push(Cmd::DeleteNow(paths));
                }
            }
            Cmd::DeleteNow(paths) => match shell_io::recycle(&paths) {
                Ok(removed) => {
                    let failed = paths.len() - removed.len();
                    if !removed.is_empty() {
                        // Only forget paths that really left: a file skipped
                        // because it is open keeps its manual category.
                        self.model.forget(&removed);
                    }
                    // Recycle Bin's icon turns from empty to full.
                    self.icons.clear();
                    self.model.toast(if failed == 0 {
                        format!("已将 {} 项移入回收站", removed.len())
                    } else if removed.is_empty() {
                        "删除失败：文件可能正在使用中".to_string()
                    } else {
                        format!(
                            "已将 {} 项移入回收站，{} 项失败",
                            removed.len(),
                            failed
                        )
                    });
                }
                Err(e) => self.model.toast(format!("删除失败：{e}")),
            },
            Cmd::ReleaseToDesktop(paths, at) => {
                self.release_to_desktop(&paths, at);
            }
            Cmd::Assign(paths, key) => self.model.assign(&paths, &key),
            Cmd::ClearOverrides(paths) => self.model.clear_overrides(&paths),
            Cmd::ReclassifyAll => self.model.reclassify_all(),
            Cmd::RenameDialog(path) => {
                let text = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.model.dialog = Some(Dialog::Rename { path, text });
            }
            Cmd::RenameApply(path, name) => self.model.rename(&path, &name),
            Cmd::NewCategoryDialog => {
                self.model.dialog = Some(Dialog::NewCategory {
                    label: String::new(),
                    exts: String::new(),
                });
            }
            Cmd::NewCategory(label, exts) => self.model.new_category(&label, &exts),
            Cmd::AboutDialog => self.model.dialog = Some(Dialog::About),
            Cmd::SetSort(k) => self.model.set_sort(k),
            Cmd::ToggleView => {
                let next = if self.model.cfg.view_mode == ViewMode::List {
                    ViewMode::Grid
                } else {
                    ViewMode::List
                };
                self.model.set_view_mode(next);
            }
            Cmd::ToggleHidden => {
                self.model.cfg.show_hidden = !self.model.cfg.show_hidden;
                self.model.persist_prefs();
                self.model.refresh();
            }
            Cmd::ToggleSingleClick => {
                self.model.cfg.single_click_open = !self.model.cfg.single_click_open;
                self.model.persist_prefs();
            }
            Cmd::ToggleTime => {
                self.model.cfg.show_time = !self.model.cfg.show_time;
                self.model.persist_prefs();
            }
            Cmd::ToggleSize => {
                self.model.cfg.show_size = !self.model.cfg.show_size;
                self.model.persist_prefs();
            }
            Cmd::ToggleFolders => {
                self.model.cfg.show_folders = !self.model.cfg.show_folders;
                self.model.persist_prefs();
                self.model.rebuild_rows();
            }
            Cmd::ToggleExt => {
                self.model.cfg.hide_ext = !self.model.cfg.hide_ext;
                self.model.persist_prefs();
            }
            Cmd::ToggleCloseAfterOpen => {
                self.model.cfg.close_after_open = !self.model.cfg.close_after_open;
                self.model.persist_prefs();
            }
            Cmd::ToggleCloseOnOutside => {
                self.model.cfg.close_on_outside = !self.model.cfg.close_on_outside;
                self.model.persist_prefs();
            }
            Cmd::ToggleAcrylic => {
                self.model.cfg.acrylic = !self.model.cfg.acrylic;
                self.model.persist_prefs();
                // The backdrop itself is applied at the top of the next frame.
                ctx.request_repaint();
            }
            Cmd::ClearSelection => self.model.selection.clear(),
            Cmd::SelectAll => self.model.select_all(),
            Cmd::Absorb(paths, key) => self.model.absorb(&paths, &key),
            Cmd::ToggleExpand(p) => self.model.toggle_expand(&p),
            Cmd::MoveToFolder(paths, dir) => self.model.move_to_folder(&paths, &dir),
            Cmd::ChooseMoveFolder(paths) => {
                if let Some(dir) = win::pick_folder(self.win.hwnd) {
                    let dir = std::path::PathBuf::from(dir);
                    if dir.is_dir() {
                        self.model.move_to_folder(&paths, &dir);
                    }
                }
            }
            Cmd::MoveToDesktop(paths) => {
                let at = self.cursor();
                self.release_to_desktop(&paths, at);
            }
            Cmd::NewFolderDialog(parent) => {
                let parent = parent.unwrap_or_else(|| {
                    if self.model.active == Tab::Desktop {
                        self.model.org.desktop().to_path_buf()
                    } else {
                        self.model.library_folder()
                    }
                });
                self.model.dialog = Some(Dialog::NewFolder {
                    parent,
                    text: "新建文件夹".to_string(),
                });
            }
            Cmd::NewFolderApply(parent, name) => self.model.create_folder(&parent, &name),
            Cmd::OpenSettings => {
                // Same reason: a topmost flyout would cover the settings window.
                self.close_panel();
                if let Ok(mut exe) = std::env::current_exe() {
                    exe.set_file_name("dcx-host.exe");
                    let _ = std::process::Command::new(exe)
                        .arg("--settings")
                        .env("DCX_SETTINGS_PAGE", "2")
                        .spawn();
                }
            }
            Cmd::SavePrefs => self.model.persist_prefs(),
            // -- taskbar tile position -------------------------------------
            Cmd::TileMenu => {
                let items = self.model.tile_menu_items();
                // draw_popup clamps `at` inside the panel; passing its bottom
                // puts the menu near the button instead of up in the header.
                let at = self
                    .geo
                    .map(|g| g.panel.max - self.origin.to_vec2())
                    .unwrap_or(Pos2::ZERO);
                self.open_panel(true);
                self.model
                    .open_popup(at, Some("收纳盒".into()), items, 226.0);
                self.ctx.request_repaint();
            }
        }
    }

    // -- row drag -----------------------------------------------------------

    fn start_row_drag(&mut self, path: std::path::PathBuf, from: String) {
        self.placer.warm();
        let (ext, is_dir) = {
            // Nested rows are not part of the desktop snapshot; the row list
            // is the only place that knows what they are.
            let e = self
                .model
                .rows
                .iter()
                .find(|r| r.entry.path == path)
                .map(|r| &r.entry)
                .or_else(|| self.model.snap.items.iter().find(|e| e.path == path));
            (
                e.map(|e| e.ext.clone()).unwrap_or_default(),
                e.map(|e| e.is_dir).unwrap_or(false),
            )
        };
        let icon = self.icons.get(&self.ctx, &path, &ext, is_dir);
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.row_drag = Some(RowDrag {
            path,
            from,
            started: Instant::now(),
        });
        self.ghost = Some(Arc::new(Mutex::new(Ghost {
            label,
            icon,
            frames: 0,
        })));
        self.model.popup = None;
    }

    fn finish_row_drag(&mut self) {
        let Some(drag) = self.row_drag.as_ref() else {
            return;
        };
        let released =
            !win::key_down(win::VK_LBUTTON) && drag.started.elapsed() >= Duration::from_millis(90);
        if !released {
            return;
        }
        let drag = self.row_drag.take().unwrap();
        self.ghost = None;
        let Some(cur) = self.cursor() else { return };
        let Some(geo) = self.geo else { return };
        let paths = self.model.drag_targets(&drag.path);
        let library = self.model.library_folder();
        let from_desktop = paths.iter().any(|p| !self.model.org.contains(p));

        if !geo.panel.expand(6.0).contains(cur) {
            if from_desktop {
                self.model.toast("这些项目已经在桌面");
            } else {
                self.release_to_desktop(&paths, Some(cur));
            }
            self.drop_tab = None;
            self.drop_folder = None;
            return;
        }
        let local = cur - self.origin.to_vec2();
        // Folder rows win over tabs: they are inside the list, never overlap.
        if let Some(folder) = self
            .folder_rects
            .iter()
            .find(|(_, r)| r.contains(local))
            .map(|(p, _)| p.clone())
        {
            let already = paths.iter().all(|p| {
                p.parent()
                    .map(|par| dcx_organizer::same_path(par, &folder))
                    .unwrap_or(false)
            });
            if !already && self.model.drop_allowed(&paths, &folder) {
                self.model.move_to_folder(&paths, &folder);
            }
            self.drop_tab = None;
            self.drop_folder = None;
            return;
        }
        let tab = self
            .tab_rects
            .iter()
            .find(|(_, r)| r.contains(local))
            .map(|(t, _)| t.clone());
        let key = tab.as_ref().and_then(|t| t.key().map(|s| s.to_string()));
        if from_desktop {
            match tab {
                Some(Tab::Key(k)) => self.model.collect(&paths, Some(&k)),
                Some(Tab::All | Tab::Recent) => self.model.collect(&paths, None),
                Some(Tab::Desktop) | None => {}
            }
        } else if matches!(tab, Some(Tab::Desktop)) {
            // Dropping a collection item on the desktop tab puts it back on
            // the real desktop.
            self.release_to_desktop(&paths, None);
        } else if let Some(key) = key {
            // A row inside a collection folder has no category tab of its own.
            let all_top = paths.iter().all(|p| {
                p.parent()
                    .map(|par| dcx_organizer::same_path(par, &library))
                    .unwrap_or(false)
            });
            if !all_top {
                self.model.toast("文件夹内的项目不能直接归入分类");
            } else if key != drag.from {
                self.model.assign(&paths, &key);
            }
        }
        self.drop_tab = None;
        self.drop_folder = None;
    }

    /// Resolve what is under the pointer while a drag is in flight. Tabs take
    /// metadata drops; folder rows take physical moves. External drags (from
    /// Explorer) can also land on a folder row.
    fn update_drop_targets(&mut self) {
        let Some(cur) = self.cursor() else {
            self.drop_tab = None;
            self.drop_folder = None;
            return;
        };
        let local = cur - self.origin.to_vec2();
        let folder = self
            .folder_rects
            .iter()
            .find(|(_, r)| r.contains(local))
            .map(|(p, _)| p.clone());
        if let Some(drag) = &self.row_drag {
            let paths = self.model.drag_targets(&drag.path);
            self.drop_folder = folder.filter(|f| self.model.drop_allowed(&paths, f));
            self.drop_tab = self
                .tab_rects
                .iter()
                .find(|(_, r)| r.contains(local))
                .and_then(|(t, _)| t.key().map(|s| s.to_string()));
        } else if self.model.drag_in {
            // External hover: collection folder rows take a physical move,
            // category chips collect into that category, the body collects and
            // lets the rules classify. A folder row on the desktop tab is a
            // real desktop folder, not a collection target.
            self.drop_folder = if self.model.active == Tab::Desktop {
                None
            } else {
                folder
            };
            self.drop_tab = self.category_tab_at(local);
        } else {
            self.drop_folder = None;
            self.drop_tab = None;
        }
    }

    fn hide_window(&mut self) {
        // Same rule as the normal close path: move, never resize, so the next
        // open does not have to rebuild the surface.
        let size = self
            .geo
            .map(|g| win::snap(g.panel.expand(SHADOW), self.scale).size())
            .unwrap_or(Vec2::splat(1.0));
        let off = Rect::from_min_size(Pos2::new(-32000.0, -32000.0), size);
        self.win.place(off, self.scale, false);
        self.win.clear_region();
        self.win.disable_hit();
    }
}

/// The collection model never hides Explorer's icons. An older build may have
/// done so and left `box_icons_owned`; restore exactly what *we* hid, without
/// touching a user-hidden layer.
fn restore_owned_desktop_icons() {
    if !dcx_win32::desktop_icons_owned() {
        return;
    }
    if dcx_win32::desktop_icons_hidden() == Some(true) {
        let _ = dcx_win32::set_desktop_icons_hidden(false);
        win::trace("desktop icons restored by box (collection mode)");
    }
    dcx_win32::set_desktop_icons_owned(false);
}

// ---------------------------------------------------------------------------
// eframe::App
// ---------------------------------------------------------------------------

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.win.ensure(WIN_TITLE) {
            ctx.request_repaint_after(Duration::from_millis(60));
            return;
        }
        self.poll_show_marker();
        // Taskbar-style acrylic: DWM paints it behind our transparent surface,
        // so the painted panel background has to stay translucent (see
        // `view::draw_panel`). Re-checked every frame — the switch can be
        // flipped from the 「更多」 menu, and `DwmSetWindowAttribute` is only
        // called when the value really changes.
        // No DWM system backdrop: its rectangle cannot be clipped by a window
        // region or by DWM rounding on this borderless window, so the four
        // corners always showed a translucent wedge and it covered the desktop
        // instead of letting it through. The panel is a plain per-pixel-alpha
        // surface now, mixed with the desktop by `acrylic_alpha`.
        self.apply_backdrop();
        self.scale = win::scale_for(
            self.win.hwnd,
            self.slot.and_then(|s| s.dpi).or_else(win::slot_dpi_cached),
        );
        // Every Rect in this crate is a **Win32 logical pixel**: physical
        // pixels from GetWindowRect / box_slot.txt divided by `scale`.
        // `place()` puts the window at `rect * scale` device pixels, so the
        // client area is `rect.size() * scale` pixels wide. egui must therefore
        // use the very same DPI scale for one point, otherwise the CentralPanel
        // is painted in device pixels while the layout is in points: the panel
        // renders at 80 % of its size and the tile lands ~190 px above the
        // taskbar on a 125 % display (hit tests and painting disagree, which
        // also makes dragging look broken).
        if (ctx.pixels_per_point() - self.scale).abs() > 0.001 {
            ctx.set_pixels_per_point(self.scale);
        }
        self.tick();

        // Flight recorder (see `last_state`): the clicks and the open/shown
        // transitions are the only things that can make the window twitch.
        let lbtn_now = win::key_down(win::VK_LBUTTON);
        if lbtn_now != self.lbtn_dbg {
            self.lbtn_dbg = lbtn_now;
            win::trace(if lbtn_now { "mouse down" } else { "mouse up" });
        }
        let st = (self.open, self.shown, self.geo.is_some(), self.pinned);
        if self.last_state != Some(st) {
            self.last_state = Some(st);
            win::trace(&format!(
                "state open={} shown={} geo={} pinned={}",
                st.0, st.1, st.2, st.3
            ));
        }

        let geo = match self.compute_geo() {
            Some(g) if self.model.cfg.enabled => g,
            _ => {
                if !self.model.cfg.enabled {
                    win::touch_no_reserve(true);
                }
                // No taskbar window at all (Explorer restarting) or the box is
                // switched off. An open panel keeps the rectangle it had
                // instead of closing under the cursor.
                if self.open && self.model.cfg.enabled {
                    if let Some(last) = self.geo {
                        self.tile.sync(last.tile, self.scale, self.open, false);
                        last
                    } else {
                        self.shown = false;
                        self.tile.hide();
                        self.hide_window();
                        ctx.request_repaint_after(Duration::from_millis(400));
                        return;
                    }
                } else {
                    self.shown = false;
                    self.tile.hide();
                    self.hide_window();
                    ctx.request_repaint_after(Duration::from_millis(400));
                    return;
                }
            }
        };
        win::touch_no_reserve(false);
        self.shown = true;

        // Content that owns the screen. The button is topmost and its guard
        // re-raises it whenever another process covers its pixels, so it would
        // float above a full-screen app; during a snip it is worse than that —
        // the frozen frame already shows the button, and the anchor may have
        // moved since the freeze, so the live copy lands next to the frozen one
        // ("the icon sticks out and is misaligned"). Hide instead, the way the
        // shell hides its own bars. A raw foreground window only displaces the
        // button while the panel is closed: an open panel is what the user is
        // working in.
        let away = win::snip_overlay_open() || win::shell_busy();
        let covered = !self.open && win::foreground_covers_bar(self.win.hwnd);

        // Auto-hide has the bar parked off-screen: the button goes with it.
        // Nothing else can happen until the bar slides back, and the bar
        // watcher wakes this loop the moment it starts to move, so idle at the
        // old slow tick instead of running a full frame twice a second.
        if geo.bar_parked && !self.open {
            self.shown = false;
            self.tile.sync(geo.tile, self.scale, false, false);
            self.hide_window();
            ctx.request_repaint_after(Duration::from_millis(400));
            return;
        }

        let popup_open = self.model.popup.is_some();
        self.pump_pointer(geo);
        self.handle_keyboard(ctx);
        self.handle_drops(ctx);

        // The taskbar button is its own window now: position it from the
        // measured slot and let it own its clicks. It is never part of the
        // panel window, so dragging the panel cannot move or blink it.
        self.tile.sync(
            geo.tile,
            self.scale,
            self.open,
            self.shown && self.model.cfg.enabled && !geo.bar_parked && !away && !covered,
        );
        if tile::take_click() {
            if self.open {
                self.close_panel();
            } else {
                self.open_panel(true);
            }
            win::trace("tile click");
        }
        if tile::take_secondary() {
            self.exec(Cmd::TileMenu, ctx);
        }

        // The panel window is **only** the panel (plus its 6 pt shadow ring).
        // While a drag is in flight the size is frozen and the window merely
        // translates — resizing a per-pixel-alpha surface every frame is what
        // made the drag flash.
        // While a capture overlay or a full-screen app is up the panel window
        // is parked too: it is topmost as well and would sit above the frozen
        // frame / the app. `open` itself is untouched, so it comes back.
        let open = self.open && self.shown && self.model.cfg.enabled && !away;
        let on_screen = win::snap(geo.panel.expand(SHADOW), self.scale);
        // Park the window at the **panel's own size**, not 1x1. Opening is then
        // a pure move: resizing a per-pixel-alpha surface on the same frame it
        // is drawn shows an empty frame first, which is the flash seen when the
        // drawer button is clicked.
        let off = Rect::from_min_size(Pos2::new(-32000.0, -32000.0), on_screen.size());
        let target = if open { on_screen } else { off };
        let move_only = self.panel_drag.is_some();
        self.win.place(target, self.scale, move_only);
        self.origin = self.win.origin(self.scale).unwrap_or(target.min);
        // `place` never touches the z-order; anything else topmost (the shell's
        // bar, a tray flyout, a third-party desktop overlay) can be raised above
        // the panel, which then stays buried. Put it back the frame after.
        self.win.ensure_raised();

        // Shape = the panel's rounded rectangle. Outside it the window does not
        // exist, so desktop / taskbar / settings clicks are never intercepted.
        // Skipped while resizing: the shape would be rebuilt every frame, which
        // is another source of flicker. It is restored on release.
        // Shape = the panel's rounded rectangle. Only needed while the window is
        // bigger than the panel (shadow ring). With no ring the window *is* the
        // panel, so the region would only shave the four corner triangles — and
        // a window region on a DWM-composited transparent window is what left a
        // stale strip at the top when the window was clicked.
        let origin = self.origin;
        let local = |r: Rect| r.translate(-origin.to_vec2());
        // No window region: with no backdrop there is nothing to clip, and a
        // region on this composited window was what left a stale strip behind.
        if open && self.resize_drag.is_some() {
            self.win.clear_region();
        }
        self.geo = Some(geo);
        self.update_drop_targets();

        let skin = view::Skin::system();
        let ctx2 = ctx.clone();
        let cursor = self.cursor().map(|c| c - self.origin.to_vec2());
        let drag_in = self.model.drag_in;
        let enabled = self.model.cfg.enabled;
        let dialog_open = self.model.dialog.is_some();
        let open = self.open;
        let shown = self.shown;
        let pinned = self.pinned;
        let drop_tab = self.drop_tab.clone();
        let drop_folder = self.drop_folder.clone();
        let mut d = view::Draw {
            model: &mut self.model,
            skin: &skin,
            icons: &mut self.icons,
            ctx: &ctx2,
            panel: local(geo.panel),
            open,
            drag_in,
            panel_pinned: pinned,
            cursor,
            row_drag_start: None,
            tab_rects: Vec::new(),
            drop_tab,
            folder_rects: Vec::new(),
            drop_folder,
            header_rect: local(geo.header),
            list_rect: Rect::NOTHING,
            popup_rect: None,
            scrollbar_rect: None,
            search_focused: false,
            blocked: popup_open || dialog_open,
        };
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::TRANSPARENT))
            .show(ctx, |ui| {
                if open && shown && enabled {
                    view::draw_panel(&mut d, ui);
                }
            });

        let out = ViewOutputs {
            row_drag_start: d.row_drag_start.take(),
            scrollbar_rect: d.scrollbar_rect,
            tab_rects: d.tab_rects.clone(),
            folder_rects: d.folder_rects.clone(),
            drop_folder: d.drop_folder.clone(),
            popup_rect: d.popup_rect,
            search_focused: d.search_focused,
        };
        drop(d);
        self.tab_rects = out.tab_rects;
        self.folder_rects = out.folder_rects;
        self.drop_folder = out.drop_folder;
        self.scrollbar_rect = out
            .scrollbar_rect
            .map(|r| r.translate(self.origin.to_vec2()));
        self.popup_rect = out.popup_rect.map(|r| r.translate(self.origin.to_vec2()));
        self.search_focused = out.search_focused;
        if let Some((path, from)) = out.row_drag_start {
            self.start_row_drag(path, from);
        }
        // The tile is a *window* click, delivered by egui: a quick tap must not
        // depend on a polling frame happening to fall inside the press.

        let cmds = self.model.take_cmds();
        for cmd in cmds {
            self.exec(cmd, ctx);
        }
        self.finish_row_drag();

        if let Some(ghost) = self.ghost.clone() {
            let s = skin;
            ctx.show_viewport_deferred(
                egui::ViewportId::from_hash_of("dcx-drag-ghost"),
                ghost_builder(),
                move |vctx, _class| ghost_frame(vctx, &ghost, s),
            );
        }

        let cur = self.cursor();
        let near = cur
            .map(|c| geo.tile.expand(24.0).contains(c))
            .unwrap_or(false);
        let wait = if self.open || self.row_drag.is_some() {
            16
        } else if near {
            45
        } else {
            200
        };
        ctx.request_repaint_after(Duration::from_millis(wait));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // A clean box exit must never leave the user with an iconless desktop.
        // Crashes leave the ownership marker in place; the watchdog restarts
        // the box, which keeps the icons hidden as intended.
        if dcx_win32::desktop_icons_owned() {
            let _ = dcx_win32::set_desktop_icons_hidden(false);
            dcx_win32::set_desktop_icons_owned(false);
            win::trace("desktop icons restored on exit");
        }
    }
}

// ---------------------------------------------------------------------------
// Drag ghost viewport (click-through, follows the OS cursor)
// ---------------------------------------------------------------------------

fn ghost_builder() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_inner_size([GHOST_W, GHOST_H])
        .with_position([-32000.0, -32000.0])
        .with_visible(false)
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_taskbar(false)
        .with_always_on_top()
        .with_active(false)
        .with_mouse_passthrough(true)
}

fn ghost_frame(ctx: &egui::Context, ghost: &Arc<Mutex<Ghost>>, s: view::Skin) {
    let Some(cur_px) = win::cursor_px() else {
        return;
    };
    let scale = win::scale_for(0, win::slot_dpi_cached());
    let cur = Pos2::new(cur_px.x / scale, cur_px.y / scale);
    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(
        cur.x - GHOST_W * 0.5,
        cur.y - GHOST_H * 0.5,
    )));
    let mut g = ghost.lock().unwrap_or_else(|e| e.into_inner());
    g.frames += 1;
    let show = g.frames > 1;
    let icon = g.icon.clone();
    let label = g.label.clone();
    drop(g);
    if show {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    }
    egui::CentralPanel::default()
        .frame(egui::Frame::none().fill(Color32::TRANSPARENT))
        .show(ctx, |ui| {
            view::draw_ghost_pill(ui, s, ui.max_rect().center(), &label, icon);
        });
    ctx.request_repaint_after(Duration::from_millis(16));
}

/// Win32 estimate of the app-icon row's right edge. A **fallback only**: on
/// Win11 `ReBarWindow32` keeps a stale width.
fn app_area_right(scale: f32) -> Option<f32> {
    unsafe {
        let bar_cls = dcx_win32::wide("Shell_TrayWnd");
        let bar = dcx_win32::FindWindowW(bar_cls.as_ptr(), core::ptr::null());
        if bar == 0 {
            return None;
        }
        let cls = dcx_win32::wide("ReBarWindow32");
        let rebar = dcx_win32::FindWindowExW(bar, 0, cls.as_ptr(), core::ptr::null());
        if rebar == 0 {
            return None;
        }
        let mut rc = dcx_win32::RECT::default();
        if dcx_win32::GetWindowRect(rebar, &mut rc) == 0 || rc.width() <= 0 {
            return None;
        }
        Some(rc.right as f32 / scale)
    }
}

/// New panel rectangle for a drag of one edge or corner.
///
/// All four sides are live: the west/north edges move the origin *and* the
/// size, which is why this returns a full rect instead of a `Vec2`.
/// Default panel rectangle: centred over the tile and sitting just above it.
fn follow_panel(tile: Rect, w: f32, h: f32, screen: Rect) -> Rect {
    let px = (tile.center().x - w / 2.0).clamp(4.0, (screen.right() - w - 4.0).max(4.0));
    let py = (tile.top() - GAP - h).max(4.0);
    Rect::from_min_size(Pos2::new(px, py), Vec2::new(w, h))
}

fn resize_rect(d: &ResizeDrag, cur: Pos2, scale: f32) -> Rect {
    let mut r = d.start_panel;
    let dx = cur.x - d.start_cursor.x;
    let dy = cur.y - d.start_cursor.y;
    let screen = win::screen_rect(scale);
    if d.edge.touches_east() {
        r.max.x += dx;
    }
    if d.edge.touches_west() {
        r.min.x += dx;
    }
    if d.edge.touches_south() {
        r.max.y += dy;
    }
    if d.edge.touches_north() {
        r.min.y += dy;
    }
    // Minimum size is enforced on the side being dragged, so the opposite edge
    // stays glued to where the user put it.
    if r.width() < MIN_PANEL_W {
        if d.edge.touches_west() {
            r.min.x = r.max.x - MIN_PANEL_W;
        } else {
            r.max.x = r.min.x + MIN_PANEL_W;
        }
    }
    if r.height() < MIN_PANEL_H {
        if d.edge.touches_north() {
            r.min.y = r.max.y - MIN_PANEL_H;
        } else {
            r.max.y = r.min.y + MIN_PANEL_H;
        }
    }
    r.max.x = r.max.x.min(screen.right() - 2.0);
    r.max.y = r.max.y.min(screen.bottom() - 2.0);
    r.min.x = r.min.x.max(2.0);
    r.min.y = r.min.y.max(2.0);
    r
}
