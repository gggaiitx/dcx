//! Taskbar slot for the 收纳盒 tile — measured from the live XAML tree.
//!
//! # Why the engine has to measure this
//!
//! The tile is drawn by `dcx-box.exe` — a standalone application with its own
//! process and icon, launched by `dcx-host.exe`. It is a transparent
//! always-on-top window that has to sit right after the last app icon and
//! before the tray. Win32 cannot answer that on Windows 11:
//!
//! * `ReBarWindow32` / `MSTaskSwWClass` still exist, but keep the width they had
//!   when the shell moved its drawing into XAML. Measured on 10.0.26200: rebar
//!   right edge 951 px while the last icon ends at 1170 px — anchoring on it
//!   dropped the tile on top of four app icons.
//! * `MSTaskListWClass` still exists too, but is empty (`TB_BUTTONCOUNT = 0`), so
//!   the classic toolbar messages have nothing to report.
//!
//! The live XAML tree is the only place the real geometry exists, and the engine
//! is already sitting inside it. This module samples the tree four times a
//! second and publishes the numbers as text — the same file-IPC pattern the
//! settings use:
//!
//! ```text
//! %LOCALAPPDATA%\dcx\box_slot.txt
//! left=1170       right edge of the last app button, physical px
//! limit=1223      left edge of the tray, physical px
//! bar_top=1020    taskbar rect, physical px
//! bar_h=60
//! dpi=1.25
//! n=13            how many app buttons the row has
//! seq=482         tick counter
//! ```
//!
//! Positions come out of XAML in DIP (`ActualOffset` summed along the ancestor
//! chain, `ActualWidth`); they are scaled by the *taskbar's* DPI here so the box
//! app never has to guess which monitor's scale applies.
//!
//! The file is written with the same temp-file + rename dance as
//! `settings.toml`: `dcx-box.exe` polls it, and a half-written document would
//! make the tile jump.
//!
//! The file names below (`box_slot.txt`, `box_host_alive`, `organizer.toml`,
//! `no_reserve`) are a **wire protocol** with the box app and the settings
//! centre. They were kept unchanged when the box was split out of the host, so
//! an already-injected engine DLL keeps working without being replaced.
//!
//! # Reserving the slot
//!
//! Measuring alone is not enough: the tile is a 40 DIP window, and a full row of
//! app icons runs right up to the tray (measured: last icon at 1170 px, tray at
//! 1223 px — one more window and the tile would sit on an icon). So the engine
//! also *reserves* the space, by giving the repeater that holds every taskbar
//! item a right margin:
//!
//! ```text
//! Grid#RootGrid
//!   Taskbar.TaskbarBackground#BackgroundControl
//!   ItemsRepeater#TaskbarFrameRepeater   ← margin-right = 52 DIP
//!       LaunchListButton ×2, TaskListButton ×n
//!   Taskbar.Gripper#GripperControl
//! ```
//!
//! The repeater is content-sized (measured 660.8 DIP for 15 items of 44), and the
//! pill behind it (`TaskbarFrame` → `RootGrid`) is content-sized around *it*. A
//! right margin therefore widens the pill by exactly that much and leaves an
//! empty slot at its right end, without touching a single icon: no compression,
//! no clipping, and the row keeps its own alignment. The tray is a separate
//! island (`SystemTrayFrameGrid`), so it does not move either.

#![allow(non_snake_case)]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use dcx_xaml::com::ComPtr as XamlPtr;
use dcx_xaml::xaml;

use crate::xaml::ComPtr;

/// Own timer. `hover` runs at 6 ms while the pointer is on the dock; the slot has
/// no business at that rate.
const TIMER_ID: usize = 0x0DC0_0002;
const TICK_MS: u32 = 250;
/// Rewrite the file even when nothing changed, so the box app can tell "the
/// shell is idle" (file still fresh) from "the engine is gone" (file went stale).
const HEARTBEAT_TICKS: u32 = 8;
/// How long without a box-app heartbeat before the reserved band is released.
/// `dcx-box.exe` touches `box_host_alive` once a second, so five seconds covers a
/// couple of missed frames without leaving an empty gap for long after a real
/// exit.
const HEARTBEAT_MS: u128 = 5000;
/// Width held free for the tile, in DIP. A Win11 app button is 44 wide, the tile
/// is 40, and the rest is the air on either side — same rhythm as the row.
pub const SLOT_DIP: f64 = 52.0;

static STARTED: AtomicBool = AtomicBool::new(false);
static TICKS: AtomicU32 = AtomicU32::new(0);
/// Epoch ms of the last time [`tick`] actually ran — proof the timer is alive.
static LAST_TICK: AtomicU64 = AtomicU64::new(0);
static REARMED: AtomicBool = AtomicBool::new(false);
/// A live sampler ticks every 250 ms; three seconds of silence means the timer is
/// gone, not that it is merely slow.
const QUIET_MS: u128 = 3000;
static DUMPED: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<Option<Published>> = Mutex::new(None);
/// `Some((repeater handle, reserved now))` — the margin is written once per
/// element, not every tick: a `Margin` write re-runs measure/arrange on the whole
/// taskbar, and at 4 Hz that would be a permanent layout storm.
static RESERVED: Mutex<Option<(u64, bool)>> = Mutex::new(None);
static BOX_ENABLED: Mutex<Option<(bool, u128)>> = Mutex::new(None);
/// Last seen per-button layout signature: `(element handle, rounded x in DIP)`
/// for every on-screen `TaskListButton`, sorted by handle. Compared each tick
/// to detect reorders that the TAP callback misses.
///
/// `record_element` (lib.rs) only fires on `VisualMutationType::Add`, i.e. when
/// `ItemsRepeater` creates or recycles an element. A pure drag-and-drop reorder
/// of already-realised buttons moves them in layout without recycling, so the
/// callback never runs and `hover::mark_dirty()` is never called — leaving
/// `hover::State::buttons[i].x` stale. The next hover then packs scaled widths
/// from the old `x` values and writes translations onto elements that are now
/// elsewhere on screen, which reads as two icons squeezed onto one slot.
///
/// This signature is built from `absolute_offset`, which sums `ActualOffset`
/// up to (but excluding) `DesktopWindowXamlSource`: it is a coordinate inside
/// the XAML island, so the taskbar window sliding on auto-hide does not change
/// it, and `RenderTransform` (used by the hover magnifier) does not touch
/// `ActualOffset` either — the signature is stable while the dock is animating.
static LAST_SIG: Mutex<Option<Vec<(u64, i64)>>> = Mutex::new(None);

#[derive(Clone, Copy, PartialEq, Eq)]
struct Published {
    /// Right edge of the last app button, physical px from the screen's left edge.
    left: i32,
    /// Left edge of the tray, physical px. The tile never crosses it.
    limit: i32,
    bar_top: i32,
    bar_h: i32,
    /// Taskbar DPI × 100 (125 = 125 %).
    dpi_x100: i32,
    n: u32,
}

#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class: *const u16, name: *const u16) -> isize;
}

/// `%LOCALAPPDATA%\dcx\box_slot.txt` — the file `dcx-box.exe` polls.
pub fn slot_path() -> std::path::PathBuf {
    let mut p = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push("dcx");
    p.push("box_slot.txt");
    p
}

/// Start sampling. Must run on the thread that hosts the taskbar XAML — the
/// timer delivers to that thread's message queue, and XAML may only be read there.
pub fn ensure_started() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        dcx_win32::SetTimer(0, TIMER_ID, TICK_MS, timer_proc as *const () as usize);
    }
    crate::log("slot: sampler started");
}

/// A task button appeared or vanished: the row changed, so sample on the next
/// tick instead of waiting for the heartbeat.
pub fn note_change() {
    ensure_started();
}

/// Is the tile's band currently held open?
///
/// The dock effect uses this: the tile is a window of its own, parked in the gap
/// after the last app button, and it cannot be pushed aside — so the magnified
/// row has to stop before it instead of growing into it.
pub fn tile_reserved() -> bool {
    match RESERVED.lock() {
        Ok(g) => matches!(*g, Some((_, true))),
        Err(_) => false,
    }
}

/// Revive the sampler if its timer stopped firing.
///
/// `SetTimer(0, …)` binds the timer to the *calling thread*, and XAML recycles the
/// dispatcher threads it hands to third-party callbacks: measured on 10.0.26200,
/// the sampler ran for exactly ~8 minutes after attach and then went quiet for
/// good while the shell itself stayed healthy. `hover`'s timer survives because it
/// is re-armed on every retune, so it is the natural place to notice and re-arm —
/// `SetTimer` replaces the entry, and the call now happens on a thread that is
/// demonstrably pumping.
pub fn keep_alive() {
    if !STARTED.load(Ordering::SeqCst) {
        ensure_started();
        return;
    }
    let last = LAST_TICK.load(Ordering::SeqCst);
    if last == 0 {
        // Still waiting for the first tick; the bar has to settle first, and a
        // timer created a moment ago is not "silent".
        return;
    }
    let now = now_ms() as u64;
    if (now as u128).saturating_sub(last as u128) < QUIET_MS {
        return;
    }
    LAST_TICK.store(now, Ordering::SeqCst);
    unsafe {
        dcx_win32::SetTimer(0, TIMER_ID, TICK_MS, timer_proc as *const () as usize);
    }
    if !REARMED.swap(true, Ordering::SeqCst) {
        crate::log("slot: sampler re-armed (timer had gone silent)");
    }
}

unsafe extern "system" fn timer_proc(_hwnd: isize, _msg: u32, _id: usize, _time: u32) {
    tick();
}

fn tick() {
    LAST_TICK.store(now_ms() as u64, Ordering::SeqCst);
    let seq = TICKS.fetch_add(1, Ordering::SeqCst) + 1;
    let Some(m) = measure() else {
        return;
    };
    if !DUMPED.swap(true, Ordering::SeqCst) {
        dump_tree();
    }
    let wanted = box_enabled();
    // Not before the bar has settled: at 250 ms the ItemsRepeater is often still
    // in its first measure, and a margin written into that pass is the one place
    // this module can plausibly hurt the shell. Two seconds of quiet is cheap.
    if seq >= 8 {
        reserve(wanted);
    }
    let changed = match STATE.lock() {
        Ok(g) => g.map(|p| p != m).unwrap_or(true),
        Err(_) => true,
    };
    if changed || seq % HEARTBEAT_TICKS == 0 {
        publish(m, seq, wanted);
        if let Ok(mut g) = STATE.lock() {
            *g = Some(m);
        }
    }
}

/// Give the task list a right margin the width of the tile, or take it back.
///
/// Written once per `(element, state)`: `Margin` invalidates measure/arrange for
/// the whole taskbar, so re-writing it on every tick would keep the shell in a
/// permanent layout pass.
fn reserve(held: bool) {
    let site = crate::tap::SITE.load(Ordering::SeqCst);
    if site.is_null() {
        return;
    }
    // Escape hatch: `%LOCALAPPDATA%\dcx\no_reserve` turns the reservation off
    // without a rebuild. If a Windows build ever reacts badly to a margin on the
    // frame repeater, the tile still works — it just sits in whatever gap is left
    // before the tray instead of owning a slot.
    let mut marker = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    marker.push("dcx");
    marker.push("no_reserve");
    let held = held && !marker.exists();
    let snapshot = crate::elements_snapshot();
    let Some(record) = snapshot
        .iter()
        .find(|r| r.name == "TaskbarFrameRepeater" && r.ty.contains("ItemsRepeater"))
    else {
        return;
    };
    if let Ok(g) = RESERVED.lock() {
        if *g == Some((record.handle, held)) {
            return;
        }
    }
    let Some(element) = (unsafe { ComPtr(site).get_inspectable(record.handle) }) else {
        return;
    };
    let margin = if held {
        format!("0,0,{SLOT_DIP},0")
    } else {
        "0,0,0,0".to_string()
    };
    let rules = [crate::apply::Rule {
        property: "Margin",
        value: &margin,
        is_xaml: false,
    }];
    let stats = unsafe { crate::apply::apply_rules(element.0, &record.ty, &rules) };
    unsafe { element.release() };
    if stats.applied > 0 {
        crate::log(&format!("slot: reserve={held} margin={margin}"));
        if let Ok(mut g) = RESERVED.lock() {
            *g = Some((record.handle, held));
        }
    }
}

/// `enabled` out of `organizer.toml`, i.e. "does the user want the box at all".
///
/// Read here rather than shipped in the settings: the two files already are the
/// IPC boundary between the box app, the settings centre and the engine, and a
/// third copy of one boolean is one more thing that can disagree. Cached for two
/// seconds — the tick is 4 Hz.
fn box_enabled() -> bool {
    let now = now_ms();
    // `enabled` from organizer.toml, cached for 2 s. The heartbeat below is
    // checked every tick instead, because a dying box process must release the
    // band promptly rather than up to two seconds later.
    let on = match BOX_ENABLED.lock() {
        Ok(g) => match *g {
            Some((on, at)) if now.saturating_sub(at) < 2000 => on,
            _ => read_organizer_enabled(),
        },
        Err(_) => read_organizer_enabled(),
    };
    if let Ok(mut g) = BOX_ENABLED.lock() {
        *g = Some((on, now));
    }
    if !on {
        return false;
    }
    // Box heartbeat: the reserved band is a visible empty gap on the taskbar
    // when `dcx-box.exe` (the process that paints the tile) is not running —
    // the engine keeps the margin but nothing fills it. The box app touches
    // `%LOCALAPPDATA%\dcx\box_host_alive` every second; a stale or missing
    // file means no tile is being painted, so release the band until the box
    // reappears, even though `enabled` is still true. The file name still says
    // "host" for wire compatibility with already-injected engine builds.
    host_alive(now)
}

/// Read the `enabled` key out of `organizer.toml`. Missing file = defaults =
/// the feature is on (`Config::default()`).
fn read_organizer_enabled() -> bool {
    let mut path = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    path.push("dcx");
    path.push("organizer.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let mut on = true;
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    break; // top-level keys only
                }
                if let Some(rest) = line.strip_prefix("enabled") {
                    on = !rest.trim_start().starts_with("= false");
                    break;
                }
            }
            on
        }
        Err(_) => true,
    }
}

/// Is the box app (the process that paints the tile) alive and producing a
/// heartbeat? `dcx-box.exe` writes `%LOCALAPPDATA%\dcx\box_host_alive` once a
/// second; a file older than `HEARTBEAT_MS` — or missing entirely — means no
/// tile is on screen, so the reserved band would be an empty gap.
fn host_alive(now: u128) -> bool {
    let mut p = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push("dcx");
    p.push("box_host_alive");
    let Ok(meta) = std::fs::metadata(&p) else {
        return false;
    };
    let Ok(mtime) = meta.modified() else {
        return false;
    };
    let m = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    now.saturating_sub(m) < HEARTBEAT_MS
}

/// Sample one geometry frame. `None` while the taskbar XAML is not reachable
/// (before the shell settles) — the box app keeps using its own fallback then.
fn measure() -> Option<Published> {
    let site = crate::tap::SITE.load(Ordering::SeqCst);
    if site.is_null() {
        return None;
    }
    let cls: Vec<u16> = "Shell_TrayWnd\0".encode_utf16().collect();
    let bar = unsafe { FindWindowW(cls.as_ptr(), core::ptr::null()) };
    if bar == 0 {
        return None;
    }
    let mut rc = dcx_win32::RECT::default();
    if unsafe { dcx_win32::GetWindowRect(bar, &mut rc) } == 0 || rc.right <= rc.left {
        return None;
    }
    let dpi = unsafe { dcx_win32::GetDpiForWindow(bar) }.max(96) as f64 / 96.0;
    let bar_w_dip = (rc.right - rc.left) as f64 / dpi;

    let snapshot = crate::elements_snapshot();
    let mut last_right = f64::MIN;
    let mut n = 0u32;
    // Per-button layout signature — see `LAST_SIG`. Rounded to 1 DIP so sub-pixel
    // layout jitter does not flap the dirty flag every tick.
    let mut sig: Vec<(u64, i64)> = Vec::new();
    for record in snapshot
        .iter()
        .filter(|r| r.name == "TaskListButton" && r.ty.starts_with("Taskbar."))
    {
        // `ActualOffset` is parent-relative: sum the chain, or a centred taskbar
        // reports x=0 for every button.
        let Some((x, _y)) =
            (unsafe { crate::hover::absolute_offset(&snapshot, site, record.handle) })
        else {
            continue;
        };
        let Some(element) = (unsafe { ComPtr(site).get_inspectable(record.handle) }) else {
            continue;
        };
        let w = unsafe { xaml::framework_element_actual_width(XamlPtr(element.0)) };
        unsafe { element.release() };
        if w < 8.0 {
            continue;
        }
        // `ItemsRepeater` parks recycled buttons far off-screen (measured x ≈
        // -9700 DIP); they carry a plausible width and would win the `max`.
        if x < -1.0 || x + w > bar_w_dip + 1.0 {
            continue;
        }
        n += 1;
        last_right = last_right.max(x + w);
        sig.push((record.handle, x.round() as i64));
    }
    // Detect a layout change the TAP callback would have missed (pure reorder
    // without recycle) and invalidate hover's cached geometry table. `mark_dirty`
    // is idempotent; hover rebuilds on its next idle frame (`amp < 0.01`), so a
    // change noticed mid-hover is applied cleanly on the way out rather than
    // mid-animation.
    sig.sort_by_key(|(h, _)| *h);
    let sig_changed = match LAST_SIG.lock() {
        Ok(g) => g.as_ref() != Some(&sig),
        Err(_) => false,
    };
    if sig_changed {
        crate::log(&format!(
            "slot: layout signature changed (n={}) — marking hover dirty",
            sig.len()
        ));
        crate::hover::mark_dirty();
    }
    if let Ok(mut g) = LAST_SIG.lock() {
        if g.is_none() || sig_changed {
            *g = Some(sig);
        }
    }
    if n == 0 {
        return None;
    }

    // Tray left edge: the same anchor `hover` uses to stop the dock. Falling back
    // to the window edge keeps the tile on the bar even if the tray element is
    // not in the snapshot yet. The 2 DIP inset (was 6) is just enough air to
    // keep the tile from touching the tray chevron without going *behind* the
    // last app button — on a centred taskbar the real gap between the row and
    // the tray can be under 4 DIP, so 6 made `limit < left` and the box threw
    // the measurement away.
    let mut limit_dip = bar_w_dip;
    for record in snapshot.iter().filter(|r| r.ty.ends_with("SystemTrayFrame")) {
        if let Some((x, _)) = unsafe { crate::hover::absolute_offset(&snapshot, site, record.handle) } {
            limit_dip = x - 2.0;
            break;
        }
    }

    let ox = rc.left as f64;
    Some(Published {
        left: (ox + last_right * dpi).round() as i32,
        limit: (ox + limit_dip * dpi).round() as i32,
        bar_top: rc.top,
        bar_h: rc.height(),
        dpi_x100: (dpi * 100.0).round() as i32,
        n,
    })
}

fn publish(m: Published, seq: u32, held: bool) {
    let path = slot_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = format!(
        "# dcx-engine — taskbar slot for the file box (physical px)\n\
         left={}\nlimit={}\nbar_top={}\nbar_h={}\ndpi={:.2}\nn={}\nreserved={}\nseq={}\n",
        m.left,
        m.limit,
        m.bar_top,
        m.bar_h,
        m.dpi_x100 as f64 / 100.0,
        m.n,
        if held { 1 } else { 0 },
        seq
    );
    let tmp = path.with_extension("txt.tmp");
    if std::fs::write(&tmp, text.as_bytes()).is_ok() && std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::write(&path, text.as_bytes());
    }
}

/// Milliseconds since the epoch, for the `organizer.toml` read cache.
fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// One-shot: log the taskbar's XAML skeleton, so the element that has to give way
/// to the tile can be identified from the log instead of guessed.
///
/// Kept in the shipped build: it costs one pass two seconds after attach and is
/// the only way to re-derive the paths after a Windows build changes the tree.
fn dump_tree() {
    let site = crate::tap::SITE.load(Ordering::SeqCst);
    if site.is_null() {
        return;
    }
    let snapshot = crate::elements_snapshot();
    crate::log(&format!("slot: tree dump, {} elements", snapshot.len()));
    for r in &snapshot {
        let interesting = r.ty.starts_with("Taskbar.")
            || !r.name.is_empty()
            || r.ty.contains("ItemsRepeater")
            || r.ty.contains("StackPanel")
            || r.ty.ends_with("Grid");
        if !interesting {
            continue;
        }
        crate::log(&format!("TREE h={} p={} {}#{}", r.handle, r.parent, r.ty, r.name));
    }

    // Where does a button sit, and what can be stretched around it?
    let Some(btn) = snapshot
        .iter()
        .find(|r| r.name == "TaskListButton" && r.ty.starts_with("Taskbar."))
    else {
        return;
    };
    let mut cur = btn.handle;
    for level in 0..10 {
        let Some(record) = snapshot.iter().find(|r| r.handle == cur) else {
            break;
        };
        let Some(element) = (unsafe { ComPtr(site).get_inspectable(cur) }) else {
            break;
        };
        let w = unsafe { xaml::framework_element_actual_width(XamlPtr(element.0)) };
        let h = unsafe { xaml::framework_element_actual_height(XamlPtr(element.0)) };
        let props = unsafe {
            crate::apply::read_back(
                element.0,
                &record.ty,
                &["Margin", "Width", "HorizontalAlignment", "Padding"],
            )
        };
        unsafe { element.release() };
        let text: Vec<String> = props.iter().map(|(k, v)| format!("{k}={v}")).collect();
        crate::log(&format!(
            "SLOT-CHAIN {level} {}#{} w={w:.1} h={h:.1} {}",
            record.ty,
            record.name,
            text.join(" ")
        ));
        if record.parent == 0 {
            break;
        }
        cur = record.parent;
    }
    // Children of each chain element, so "what sits next to the task list" is
    // visible without a second injection round.
    let mut parents: Vec<u64> = Vec::new();
    let mut cur = btn.parent;
    for _ in 0..4 {
        parents.push(cur);
        match snapshot.iter().find(|r| r.handle == cur) {
            Some(r) if r.parent != 0 => cur = r.parent,
            _ => break,
        }
    }
    for parent in parents {
        let kids: Vec<String> = snapshot
            .iter()
            .filter(|r| r.parent == parent)
            .map(|r| format!("{}#{}", r.ty.rsplit('.').next().unwrap_or(&r.ty), r.name))
            .collect();
        crate::log(&format!(
            "SLOT-KIDS h={parent}: {}",
            kids.join(" | ")
        ));
    }
}

/// Diagnostic entry point (unused otherwise): the published file's path as text.
pub fn describe() -> String {
    slot_path().display().to_string()
}
