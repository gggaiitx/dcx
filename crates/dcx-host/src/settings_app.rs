//! Settings window (egui/eframe), styled after the Windows 11 Settings app.
//!
//! This is the **one** settings surface for the whole platform: taskbar theme,
//! platform behaviour, and the settings of every registered standalone app
//! (see `apps.rs`). Applications do not own a settings window; they only read
//! their own TOML document. Each standalone app gets its own page — 「桌面整理」
//! for the file box and 「文件检索」 for search — so enabling, status and every
//! option of one app stay on that app's page.
//!
//! Visual rules borrowed from Win11 Settings:
//!   * **one surface**: the native title bar, the navigation column and the
//!     page share a single backdrop — a flat top band exactly matching the DWM
//!     caption colour, then a slow Mica-like gradient below it, painted once
//!     into the background layer. No panel fills, so there is no seam, no
//!     divider and no tonal step between title bar, left and right;
//!   * coloured navigation glyphs with no tile or badge behind them; selection
//!     is carried by Win11's solid selected pill, an indicator bar and the
//!     primary text colour — the label keeps its regular weight;
//!   * rail colours sampled from the Win11 Settings app: #ECF6F8 caption /
//!     #E9F7F9 rail, #E3EDEF selected pill — see `Palette::light`; the row
//!     pitch is 46 pt (≈58 physical px at 125 % DPI), a little taller than
//!     Win11's 40 pt so the list does not feel cramped; selection changes the
//!     pill, the indicator and the text colour, never the font weight;
//!   * one column of cards floating on that surface: white fill, hairline
//!     border, 6 px radius, 1 px soft shadow, 16 px row padding, 1 px
//!     separators inset from the text column;
//!   * each setting is a row: optional 20 px glyph, title (13.5 px) with a
//!     12 px secondary line, control right-aligned;
//!   * Win11 control shapes: 40×20 pill switch, 20 px ring radio, 4 px slider
//!     track with a round accent thumb, 32 px standard buttons;
//!   * accent is the DCX brand teal: #0E7C7E (light) / #4FD8CC (dark);
//!     everything else stays on Win11's neutral greys so the shell still
//!     looks native.
//!
//! Behavioural rules (why the first version felt wrong):
//!   * edits apply live (debounced ~220 ms) — no "save" button to forget;
//!   * the footer states honestly whether the file was written and whether the
//!     engine is actually injected, so "I changed it and nothing happened" has
//!     an answer on screen;
//!   * optional values (`height`, `max width`) are switches, not detached
//!     sliders: turning one off restores the derived value instead of leaving a
//!     stale override behind;
//!   * the shell's own state (taskbar auto-hide) is read live and written
//!     through `ABM_SETSTATE` on click — it bypasses the debounce and the file,
//!     because Windows is the one that stores it.

use dcx_organizer::{Config as BoxConfig, OpenMode, Organizer as BoxOrganizer};
use dcx_settings::{
    HoverEffect, Settings, Spacing, TaskbarShape, UiTheme, DEFAULT_MAX_WIDTH, MAX_ICON_SCALE,
    MAX_ICON_SPACING, MAX_TASKBAR_HEIGHT, MIN_ICON_SCALE, MIN_ICON_SPACING, MIN_TASKBAR_HEIGHT,
    TASKBAR_BUDGET,
};
use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, FontId, Layout, Margin, Rect, Response, RichText,
    Rounding, Sense, Stroke,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};// winit is the exact same 0.30.13 instance eframe links against (see Cargo.lock),
// so the `EventLoopBuilder` types match and `with_any_thread` applies cleanly.
use winit::platform::windows::EventLoopBuilderExtWindows;

/// Guards against opening more than one settings window at a time.
static OPEN: AtomicBool = AtomicBool::new(false);
/// Page the next window should open on (the tray's 「桌面整理」 jumps to the
/// combined app + box page). Read once in `new`, reset after.
static START_PAGE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// Set when the tray is clicked while the window already exists, so the running
/// window can raise itself instead of silently ignoring the click.
static FOCUS_REQUEST: AtomicBool = AtomicBool::new(false);
/// Result of a background 「重新应用」 injection, picked up by the next frame.
static REAPPLY_RESULT: std::sync::Mutex<Option<Result<(), String>>> =
    std::sync::Mutex::new(None);
/// Result of a background library migration: `(new root, outcome)`.
static MIGRATE_RESULT: std::sync::Mutex<
    Option<(std::path::PathBuf, std::io::Result<dcx_organizer::MigrateReport>)>,
> = std::sync::Mutex::new(None);

/// Default window size. Win11 Settings opens wider than 900 px; the wider
/// default gives the (also wider) navigation column and the card column room
/// to breathe at 125 % DPI instead of squeezing the cards into a narrow strip.
const WIN_W: f32 = 1080.0;
const WIN_H: f32 = 700.0;
const CARD_RADIUS: f32 = 6.0;
const SIDEBAR_W: f32 = 256.0;
/// Horizontal gutter between the window edge and the content column.
const PAGE_GUTTER: f32 = 34.0;
/// How long the 「恢复并退出」 button stays armed after the first click.
const CONFIRM_WINDOW: Duration = Duration::from_secs(5);
/// Width kept clear on the right of a [`row`] for its action widget. Text is
/// clamped to what is left so a long subtitle truncates instead of drawing over
/// the button.
const ROW_ACTION_W: f32 = 132.0;
/// Space a 40 px switch needs (see [`row_tight`]).
const ROW_SWITCH_W: f32 = 52.0;
/// Space a status dot / short word needs (see [`row_info`]).
const ROW_INFO_W: f32 = 16.0;
/// Project page shown on the「关于」page. Keep it in sync with `repository` in
/// the workspace `Cargo.toml` / the README badge.
const GITHUB_URL: &str = "https://github.com/gggaiitx/dcx";

extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
    fn GetDpiForSystem() -> u32;
}

/// Fit the window to the primary monitor: a fixed 760 px logical height is
/// taller than the usable area on a 1080p screen at 125–150 % scaling, which
/// used to push the footer (status + buttons) off-screen.
fn fitted_window_size() -> ([f32; 2], [f32; 2]) {
    const SM_CXSCREEN: i32 = 0;
    const SM_CYSCREEN: i32 = 1;
    let dpi = unsafe { GetDpiForSystem() }.max(96) as f32 / 96.0;
    let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) } as f32 / dpi;
    let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) } as f32 / dpi;
    let w = WIN_W.min((screen_w - 80.0).max(640.0));
    let h = WIN_H.min((screen_h - 160.0).max(520.0));
    // Centred horizontally, slightly above centre vertically (taskbar + title
    // bar). Without an explicit position Win11's default cascade can put the
    // window partly below the screen on a 125 % 1080p display.
    let x = ((screen_w - w) / 2.0).max(0.0);
    let y = ((screen_h - h) / 2.0 - 20.0).max(0.0);
    ([w, h], [x, y])
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Page codes for [`START_PAGE`] (tray menu and the `--settings` env var).
pub const PAGE_LOOK: u8 = 0;
pub const PAGE_ABOUT: u8 = 1;
/// Desktop file box: its enable switch, running state and its own options.
pub const PAGE_BOX: u8 = 2;
/// File search: its enable switch, running state and its own options.
pub const PAGE_SEARCH: u8 = 3;
/// Desktop info island: its one master switch and all module options.
pub const PAGE_ISLAND: u8 = 4;

/// Which page a freshly spawned settings process should open on.
pub fn set_start_page(page: u8) {
    START_PAGE.store(page, Ordering::SeqCst);
}

/// Spawn the central settings window.
///
/// winit allows exactly **one** event loop per process (`EventLoop can't be
/// recreated`). The platform host owns the WinEvent/message loop and each
/// standalone app owns its own eframe loop, so this window always runs in a
/// separate process via `--settings`. Reusing one binary keeps registry access,
/// engine status and app lifecycle in the same privileged place.
pub fn open_settings() {
    open_settings_at(PAGE_LOOK);
}

/// Same, but landing on a specific page (the tray's 「桌面整理」 uses `PAGE_BOX`).
pub fn open_settings_at(page: u8) {
    START_PAGE.store(page, Ordering::SeqCst);
    // A *separate process*: winit allows one event loop per process, and the
    // host and every app already have theirs. `--settings` runs only this
    // window, so it gets a normal top-level input path without competing.
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    match spawn_settings(&exe, page) {
        Ok(child) => {
            if let Ok(mut slot) = SETTINGS_CHILD.lock() {
                // Reap the previous window if it has already exited; replacing
                // the handle without `try_wait` leaked one process handle per
                // open (a second launch raises the existing window and exits).
                if let Some(mut old) = slot.take() {
                    let _ = old.try_wait();
                }
                *slot = Some(child);
            }
        }
        Err(e) => eprintln!("[dcx-host] cannot start the settings window: {e}"),
    }
}

/// The one settings process this host owns. Kept so quitting the platform can
/// take it down: it used to keep running (and keep writing the same TOML and
/// `host.log`) after the tray's 「退出」.
static SETTINGS_CHILD: std::sync::Mutex<Option<std::process::Child>> =
    std::sync::Mutex::new(None);

/// Terminate the spawned settings window, if it is still alive.
///
/// `WM_CLOSE` first: eframe then runs `on_exit` → `flush`, so an edit still
/// inside the 220 ms debounce is not thrown away. `kill` is only the fallback
/// for a window that does not answer.
pub fn close_settings_window() {
    let mut child = match SETTINGS_CHILD.lock() {
        Ok(mut slot) => match slot.take() {
            Some(c) => c,
            None => return,
        },
        Err(_) => return,
    };
    post_close("DCX 设置");
    let deadline = Instant::now() + Duration::from_millis(1500);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[link(name = "user32")]
extern "system" {
    fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
}

fn post_close(title: &str) {
    const WM_CLOSE: u32 = 0x0010;
    let t = dcx_win32::wide(title);
    let hwnd = unsafe { dcx_win32::FindWindowW(core::ptr::null(), t.as_ptr()) };
    if hwnd != 0 {
        unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    }
}

// -- settings process in a kill-on-close job --------------------------------

const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;

#[repr(C)]
#[derive(Default)]
struct BasicLimitInformation {
    per_process_user_time: i64,
    per_job_user_time: i64,
    limit_flags: u32,
    minimum_working_set: usize,
    maximum_working_set: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[repr(C)]
#[derive(Default)]
struct IoCounters {
    read_operation_count: u64,
    write_operation_count: u64,
    other_operation_count: u64,
    read_transfer_count: u64,
    write_transfer_count: u64,
    other_transfer_count: u64,
}

#[repr(C)]
#[derive(Default)]
struct ExtendedLimitInformation {
    basic: BasicLimitInformation,
    io: IoCounters,
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateJobObjectW(attrs: *mut core::ffi::c_void, name: *const u16) -> isize;
    fn SetInformationJobObject(
        job: isize,
        class: u32,
        info: *mut core::ffi::c_void,
        len: u32,
    ) -> i32;
    fn AssignProcessToJobObject(job: isize, process: isize) -> i32;
    fn CloseHandle(handle: isize) -> i32;
}

/// One kill-on-close job for every settings child. Created once; the handle is
/// intentionally leaked for the process lifetime (the OS closes it on exit,
/// including an abnormal one, which is what makes the child go away with us).
static SETTINGS_JOB: std::sync::Mutex<isize> = std::sync::Mutex::new(0);

fn settings_job() -> isize {
    let mut slot = SETTINGS_JOB.lock().unwrap_or_else(|e| e.into_inner());
    if *slot == 0 {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job == 0 {
                return 0;
            }
            let mut info = ExtendedLimitInformation::default();
            info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                &mut info as *mut _ as *mut core::ffi::c_void,
                core::mem::size_of::<ExtendedLimitInformation>() as u32,
            );
            if ok == 0 {
                CloseHandle(job);
                return 0;
            }
            *slot = job;
        }
    }
    *slot
}

/// Spawn the settings process inside a job that kills it when the host dies.
///
/// `std::process::exit` now also calls [`close_settings_window`], but that only
/// covers an orderly quit; a crash or a task-manager kill leaves the child
/// running with no owner (two writers for every TOML, two `host.log`
/// appenders). The job handle is deliberately leaked for the process lifetime:
/// the OS closes it on exit — including an abnormal one — and the child goes
/// with it.
fn spawn_settings(exe: &std::path::Path, page: u8) -> std::io::Result<std::process::Child> {
    use std::os::windows::io::AsRawHandle;
    let mut command = std::process::Command::new(exe);
    command
        .arg("--settings")
        .env("DCX_SETTINGS_PAGE", page.to_string());
    let child = command.spawn()?;
    let job = settings_job();
    if job != 0 {
        unsafe {
            AssignProcessToJobObject(job, child.as_raw_handle() as isize);
        }
    }
    Ok(child)
}



/// Run the settings window on the calling (main) thread. Used by
/// `dcx-host --settings` for development: no tray, no injection, just the UI.
pub fn run_standalone() -> eframe::Result<()> {
    eframe::run_native(
        "DCX 设置",
        native_options(),
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            Ok(Box::new(SettingsApp::new()))
        }),
    )
}

fn native_options() -> eframe::NativeOptions {
    let (size, pos) = fitted_window_size();
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_position(pos)
            .with_min_inner_size([640.0, 520.0])
            .with_resizable(true)
            .with_icon(app_icon()),
        // eframe/winit refuse to build an EventLoop off the main thread by
        // default; the settings window intentionally runs on its own thread so
        // it never blocks the host's corner-effect message loop. Allow it.
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    }
}

/// Application icon, embedded as raw RGBA at build time by
/// `tools/gen_brand.py`. 64px is what the title bar and the Alt-Tab thumbnail
/// sample from; eframe takes a single frame, so no size ladder here.
const APP_ICON: &[u8] = include_bytes!("../../../brand/embed/icon-64.rgba");
const APP_ICON_PX: u32 = 64;

pub(crate) fn app_icon() -> egui::IconData {
    egui::IconData {
        rgba: APP_ICON.to_vec(),
        width: APP_ICON_PX,
        height: APP_ICON_PX,
    }
}

// ---------------------------------------------------------------------------
// Fonts (CJK)
// ---------------------------------------------------------------------------

/// Registered font family for semibold text, when a bold face was found.
const BOLD_FAMILY: &str = "dcx-bold";
/// Registered family for the shell's own icon font (Segoe Fluent Icons).
const ICON_FAMILY: &str = "dcx-icons";
/// Set by [`setup_fonts`] once the bold family is registered. egui panics when
/// a `FontFamily::Name` is not bound to any font, so every caller has to know.
static HAS_BOLD: AtomicBool = AtomicBool::new(false);
/// Set by [`setup_fonts`] once the icon font is registered.
static HAS_ICONS: AtomicBool = AtomicBool::new(false);

/// Load a system CJK font into egui so Chinese labels render. egui's bundled
/// font only covers Latin, so without this every Chinese glyph shows as a tofu
/// box. We prepend the CJK font to the families; egui falls through to the next
/// font per missing glyph, so Latin text still renders from the default.
///
/// The bold face is loaded as its own family so headings can carry real weight
/// contrast — Win11's page titles and group captions are semibold, and faking
/// that by enlarging the regular weight reads as "big body text", not a title.
pub(crate) fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // Candidate system fonts in priority order. `msyh.ttc` is the collection of
    // Microsoft YaHei on every Win10/11 box; index 0 is the regular weight.
    let candidates = [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
        r"C:\Windows\Fonts\malgun.ttf",
    ];
    let loaded = candidates.iter().find_map(|p| {
        std::fs::read(p).ok().map(|bytes| {
            fonts
                .font_data
                .insert("cjk".to_string(), egui::FontData::from_owned(bytes));
            "cjk"
        })
    });
    if let Some(name) = loaded {
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            if let Some(list) = fonts.families.get_mut(&family) {
                list.insert(0, name.to_string());
            }
        }
    } else {
        eprintln!("[dcx-host] no system CJK font found; Chinese labels will show as boxes");
    }

    // Bold: same glyph coverage as the regular face, so the two never disagree
    // about which CJK characters exist.
    let bold_sources = [
        r"C:\Windows\Fonts\msyhbd.ttc",
        r"C:\Windows\Fonts\segoeuib.ttf",
        r"C:\Windows\Fonts\arialbd.ttf",
    ];
    if let Some(bytes) = bold_sources.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert(
            "cjk-bold".to_string(),
            egui::FontData::from_owned(bytes),
        );
        let mut list = fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        list.insert(0, "cjk-bold".to_string());
        fonts
            .families
            .insert(egui::FontFamily::Name(BOLD_FAMILY.into()), list);
        HAS_BOLD.store(true, Ordering::Relaxed);
    }
    setup_icon_font(&mut fonts);
    ctx.set_fonts(fonts);
}

/// Semibold face at `size`, falling back to the regular weight if the system
/// has no bold font (the layout still works, only the weight contrast is lost).
fn bold(size: f32) -> FontId {
    if HAS_BOLD.load(Ordering::Relaxed) {
        FontId::new(size, egui::FontFamily::Name(BOLD_FAMILY.into()))
    } else {
        font(size)
    }
}

/// Load the shell's icon font as its own family.
///
/// Hand-drawing ten icons with the painter produced glyphs that read as
/// "sketched", not "system": inconsistent stroke weights, off-pixel curves, and
/// three of them ambiguous. The shell already ships a matching set — Segoe
/// Fluent Icons on Win11, its MDL2 predecessor on Win10 — so borrow those and
/// only fall back to the painter when neither file exists.
fn setup_icon_font(fonts: &mut egui::FontDefinitions) {
    let sources = [
        r"C:\Windows\Fonts\SegoeIcons.ttf",
        r"C:\Windows\Fonts\segmdl2.ttf",
    ];
    if let Some(bytes) = sources.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert(
            "segoe-icons".to_string(),
            egui::FontData::from_owned(bytes),
        );
        fonts.families.insert(
            egui::FontFamily::Name(ICON_FAMILY.into()),
            vec!["segoe-icons".to_string()],
        );
        HAS_ICONS.store(true, Ordering::Relaxed);
    }
}

/// Icon-font face. Only valid while [`HAS_ICONS`] is set.
fn icon_font(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name(ICON_FAMILY.into()))
}

fn icons_available() -> bool {
    HAS_ICONS.load(Ordering::Relaxed)
}

/// Codepoints in Segoe Fluent Icons (identical in Segoe MDL2 Assets for these).
mod glyph {
    pub const MONITOR: char = '\u{E7F4}';
    pub const FOLDER: char = '\u{E8B7}';
    pub const SEARCH: char = '\u{E721}';
    /// Segoe MDL2 Assets 「Clock」— the island's compact capsule is a clock.
    pub const CLOCK: char = '\u{E823}';
    pub const INFO: char = '\u{E946}';
    pub const TRASH: char = '\u{E74D}';
    pub const PLUS: char = '\u{E710}';
    pub const SUN: char = '\u{E706}';
    pub const MOON: char = '\u{E708}';
}

// ---------------------------------------------------------------------------
// Palette — Win11 Settings light / dark
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Palette {
    dark: bool,
    /// Window backdrop, top and bottom stops. Painted once across the whole
    /// window, behind both columns: that single surface is what makes the
    /// navigation and the page read as one piece instead of two panels.
    /// `bg_top` is a **flat** colour for the whole width: the native caption is
    /// a solid DWM fill, so any horizontal variation at the top of the client
    /// area reads as a seam under the title bar. The warm variation lives at
    /// the bottom only.
    bg_top: Color32,
    bg_bottom: Color32,
    bg_bottom_warm: Color32,
    /// Translucent cool wash over the navigation column. It fades out inside
    /// the page, so the columns get a tonal difference without a divider line.
    nav_tint: Color32,
    /// Translucent light spot near the top-right of the page.
    glow: Color32,
    card: Color32,
    /// Drop shadow under a card; a barely-there lift, like Win11's card layer.
    card_shadow: Color32,
    border: Color32,
    separator: Color32,
    text: Color32,
    secondary: Color32,
    tertiary: Color32,
    accent: Color32,
    accent_soft: Color32,
    track: Color32,
    hover: Color32,
    switch_off: Color32,
    switch_off_border: Color32,
    ok: Color32,
    warn: Color32,
    /// Destructive confirmation (confirming a shell restart).
    danger: Color32,
    nav_selected: Color32,
    nav_hover: Color32,
    /// Short vertical stroke at the left edge of the selected navigation item.
    /// Win11 uses the shell accent here, not the app's own brand colour.
    nav_indicator: Color32,
    /// Scrollbar thumb (Win11: thin grey overlay).
    scroll_thumb: Color32,
    scroll_thumb_hover: Color32,
}

impl Palette {
    fn light() -> Self {
        Self {
            dark: false,
            // Sampled from the Win11 Settings navigation column: #ECF6F8 at
            // the title bar / top of the page, #E9F7F9 further down. A pale
            // cyan Mica surface, not neutral white. The caption band uses
            // exactly `bg_top`, so the DWM title bar stays seamless.
            bg_top: Color32::from_rgb(0xEC, 0xF6, 0xF8),
            bg_bottom: Color32::from_rgb(0xE9, 0xF7, 0xF9),
            bg_bottom_warm: Color32::from_rgb(0xEB, 0xF5, 0xF6),
            // Only a whisper of extra colour: Win11's rail and page are the
            // same Mica surface, so no separate panel fill or warm glow.
            nav_tint: Color32::from_rgba_unmultiplied(0x4A, 0xA8, 0xC0, 6),
            glow: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 5),
            card: Color32::WHITE,
            card_shadow: Color32::from_black_alpha(12),
            border: Color32::from_rgb(0xDC, 0xE9, 0xEC),
            separator: Color32::from_rgb(0xE2, 0xED, 0xEF),
            text: Color32::from_rgb(0x1A, 0x1A, 0x1A),
            secondary: Color32::from_rgb(0x55, 0x5A, 0x63),
            tertiary: Color32::from_rgb(0x86, 0x8C, 0x96),
            accent: Color32::from_rgb(0x0E, 0x7C, 0x7E),
            accent_soft: Color32::from_rgb(0xE4, 0xF2, 0xF1),
            track: Color32::from_rgb(0xDD, 0xE9, 0xEC),
            hover: Color32::from_rgb(0xED, 0xF5, 0xF6),
            switch_off: Color32::from_rgb(0xDC, 0xE8, 0xEA),
            switch_off_border: Color32::from_rgb(0xCF, 0xDF, 0xE2),
            ok: Color32::from_rgb(0x0F, 0x7B, 0x0F),
            warn: Color32::from_rgb(0x9D, 0x5D, 0x00),
            danger: Color32::from_rgb(0xC4, 0x2B, 0x1C),
            // Sampled Win11 values: selected pill #E3EDEF on a #E9F7F9 rail
            // (≈7–10 levels deeper, plainly visible), hover one step lighter,
            // indicator bar #1D6978. Opaque, not the near-invisible alpha
            // washes the previous build used.
            nav_selected: Color32::from_rgb(0xE3, 0xED, 0xEF),
            nav_hover: Color32::from_rgb(0xE8, 0xF3, 0xF4),
            nav_indicator: Color32::from_rgb(0x1D, 0x69, 0x78),
            scroll_thumb: Color32::from_rgb(0x9A, 0xA6, 0xA8),
            scroll_thumb_hover: Color32::from_rgb(0x7A, 0x86, 0x88),
        }
    }

    fn dark() -> Self {
        Self {
            dark: true,
            bg_top: Color32::from_rgb(0x28, 0x2B, 0x30),
            bg_bottom: Color32::from_rgb(0x1D, 0x20, 0x24),
            bg_bottom_warm: Color32::from_rgb(0x23, 0x20, 0x24),
            nav_tint: Color32::from_rgba_unmultiplied(0x54, 0x8C, 0xD8, 16),
            glow: Color32::from_rgba_unmultiplied(0x4F, 0xD8, 0xCC, 8),
            card: Color32::from_rgb(0x2C, 0x2F, 0x35),
            card_shadow: Color32::from_black_alpha(80),
            border: Color32::from_rgb(0x3B, 0x40, 0x48),
            separator: Color32::from_rgb(0x36, 0x3B, 0x43),
            text: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            secondary: Color32::from_rgb(0xC5, 0xC5, 0xC5),
            tertiary: Color32::from_rgb(0x9A, 0x9A, 0x9A),
            accent: Color32::from_rgb(0x4F, 0xD8, 0xCC),
            accent_soft: Color32::from_rgb(0x10, 0x39, 0x3A),
            track: Color32::from_rgb(0x3A, 0x3A, 0x3A),
            hover: Color32::from_rgb(0x2F, 0x33, 0x3A),
            switch_off: Color32::from_rgb(0x3A, 0x3A, 0x3A),
            switch_off_border: Color32::from_rgb(0x46, 0x46, 0x46),
            ok: Color32::from_rgb(0x6C, 0xCB, 0x5F),
            warn: Color32::from_rgb(0xF0, 0xC6, 0x74),
            danger: Color32::from_rgb(0xFF, 0x99, 0xA4),
            // Dark counterpart of the same three sampled states: a visible
            // but quiet pill, a lighter hover, a bright accent indicator.
            nav_selected: Color32::from_rgb(0x33, 0x3B, 0x42),
            nav_hover: Color32::from_rgb(0x2D, 0x34, 0x3A),
            nav_indicator: Color32::from_rgb(0x4F, 0xC3, 0xD8),
            scroll_thumb: Color32::from_rgb(0x6A, 0x6A, 0x6A),
            scroll_thumb_hover: Color32::from_rgb(0x8A, 0x8A, 0x8A),
        }
    }
}

fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

fn apply_style(ctx: &egui::Context, p: &Palette) {
    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (egui::TextStyle::Small, font(12.5)),
        (egui::TextStyle::Body, font(14.5)),
        (egui::TextStyle::Button, font(14.0)),
        (egui::TextStyle::Heading, bold(24.0)),
        (egui::TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(13.0, 7.0);
    style.spacing.interact_size = egui::vec2(26.0, 26.0);
    // Win11 scrollbars are thin overlays that sit on the window edge instead of
    // taking a column of their own.
    style.spacing.scroll = egui::style::ScrollStyle {
        floating: true,
        bar_width: 4.0,
        handle_min_length: 32.0,
        bar_inner_margin: 2.0,
        bar_outer_margin: 0.0,
        floating_width: 4.0,
        floating_allocated_width: 0.0,
        foreground_color: false,
        ..style.spacing.scroll.clone()
    };

    let v = &mut style.visuals;
    v.dark_mode = p.dark;
    v.override_text_color = Some(p.text);
    // Panels are transparent on purpose: the single backdrop painted in
    // [`paint_backdrop`] must show through both the navigation column and the
    // page, otherwise their fills reintroduce the seam.
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = p.card;
    v.extreme_bg_color = p.track;
    v.faint_bg_color = p.hover;
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0_f32, p.accent);
    v.hyperlink_color = p.accent;
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;

    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = Rounding::same(CARD_RADIUS);
    }
    v.widgets.noninteractive.bg_fill = p.card;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);

    v.widgets.inactive.bg_fill = p.card;
    v.widgets.inactive.weak_bg_fill = p.card;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);

    v.widgets.hovered.bg_fill = p.hover;
    v.widgets.hovered.weak_bg_fill = p.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, p.text);

    v.widgets.active.bg_fill = p.accent_soft;
    v.widgets.active.weak_bg_fill = p.accent_soft;
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, p.text);

    ctx.set_style(style);
}

/// Height of the flat band at the top of the client area, in points. The
/// native caption is a **solid** DWM fill (`DWMWA_CAPTION_COLOR`), so the first
/// band of the page has to be exactly that colour: a gradient starting at
/// y = 0 shows up as a seam immediately under the title bar.
const CAPTION_BAND_H: f32 = 44.0;
/// Vertical distance over which the navigation wash fades in below that band.
const WASH_FADE_H: f32 = 96.0;

/// Paint the one window surface: a flat caption-matching band, a slow gradient
/// below it, and two translucent washes that only begin under the band.
///
/// This replaces the per-panel fills. A `SidePanel` filled with one grey and a
/// `CentralPanel` filled with another always leaves a tonal step at the seam,
/// which reads as a divider line even when no line is drawn. Painting once in
/// the background layer keeps the columns — and now the native title bar — on
/// literally the same surface.
fn paint_backdrop(ctx: &egui::Context, p: &Palette) {
    let rect = ctx.screen_rect();
    let painter = ctx.layer_painter(egui::LayerId::background());
    let band_y = (rect.top() + CAPTION_BAND_H).min(rect.bottom());
    let clear = Color32::TRANSPARENT;

    // 1. Base. The top band is one flat colour across the full width, matching
    //    the DWM caption exactly. Only below it does the Mica-like gradient
    //    begin, so no horizontal step exists anywhere along the title edge.
    let mut top = egui::Mesh::default();
    top.colored_vertex(rect.left_top(), p.bg_top);
    top.colored_vertex(rect.right_top(), p.bg_top);
    top.colored_vertex(egui::pos2(rect.right(), band_y), p.bg_top);
    top.colored_vertex(egui::pos2(rect.left(), band_y), p.bg_top);
    top.add_triangle(0, 1, 2);
    top.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(top));

    if band_y < rect.bottom() {
        let mut base = egui::Mesh::default();
        base.colored_vertex(egui::pos2(rect.left(), band_y), p.bg_top);
        base.colored_vertex(egui::pos2(rect.right(), band_y), p.bg_top);
        base.colored_vertex(rect.right_bottom(), p.bg_bottom_warm);
        base.colored_vertex(rect.left_bottom(), p.bg_bottom);
        base.add_triangle(0, 1, 2);
        base.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(base));
    }

    // 2. Cool wash behind the navigation column: fades in vertically from the
    //    bottom of the flat band, and out horizontally well inside the page.
    //    Neither edge is visible, so the columns still read as one surface.
    let fade_x = (rect.left() + SIDEBAR_W + 64.0).min(rect.right());
    let wash_y = (band_y + WASH_FADE_H).min(rect.bottom());
    let mut rise = egui::Mesh::default();
    rise.colored_vertex(egui::pos2(rect.left(), band_y), clear);
    rise.colored_vertex(egui::pos2(fade_x, band_y), clear);
    rise.colored_vertex(egui::pos2(fade_x, wash_y), clear);
    rise.colored_vertex(egui::pos2(rect.left(), wash_y), p.nav_tint);
    rise.add_triangle(0, 1, 2);
    rise.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(rise));

    if wash_y < rect.bottom() {
        let mut hold = egui::Mesh::default();
        hold.colored_vertex(egui::pos2(rect.left(), wash_y), p.nav_tint);
        hold.colored_vertex(egui::pos2(fade_x, wash_y), clear);
        hold.colored_vertex(egui::pos2(fade_x, rect.bottom()), clear);
        hold.colored_vertex(egui::pos2(rect.left(), rect.bottom()), p.nav_tint);
        hold.add_triangle(0, 1, 2);
        hold.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(hold));
    }

    // 3. Soft light spot, confined to the area below the caption band. Four
    //    fully transparent corners mean its top edge cannot read as a line.
    let gx0 = (rect.right() - rect.width() * 0.62).max(fade_x - 40.0);
    let gx1 = rect.right();
    let gy0 = band_y;
    let gy1 = (band_y + (rect.bottom() - band_y) * 0.62).min(rect.bottom());
    let corners = [
        egui::pos2(gx0, gy0),
        egui::pos2(gx1, gy0),
        egui::pos2(gx1, gy1),
        egui::pos2(gx0, gy1),
    ];
    let mut glow = egui::Mesh::default();
    glow.colored_vertex(
        egui::pos2((gx0 + gx1) / 2.0, (gy0 + gy1) / 2.0),
        p.glow,
    );
    for &q in &corners {
        glow.colored_vertex(q, clear);
    }
    for i in 0u32..4 {
        glow.add_triangle(0, 1 + i, 1 + ((i + 1) % 4));
    }
    painter.add(egui::Shape::mesh(glow));
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    /// Shape, icon scale and every geometry value, on a single page.
    Look,
    /// Desktop file box: app enable/state, switches, desktop location and the
    /// extension → tab rules.
    Box,
    /// File search: app enable/state, hotkey, scope and content limits.
    Search,
    /// Desktop info island: app enable/state, clock, metrics, tasks,
    /// clipboard, inbox and outbound channels.
    Island,
    About,
}

impl Page {
    /// Route a `PAGE_*` code from the tray or the start-page env var.
    fn from_code(code: u8) -> Self {
        match code {
            PAGE_BOX => Page::Box,
            PAGE_SEARCH => Page::Search,
            PAGE_ISLAND => Page::Island,
            PAGE_ABOUT => Page::About,
            _ => Page::Look,
        }
    }
}

/// One editable row of the rule card. The strings are what the text fields
/// bind to; the `BoxConfig` is only rebuilt from them when something changed,
/// so typing a half-finished extension never reaches the file (and never
/// re-files anything).
struct RuleRow {
    key: String,
    label: String,
    exts: String,
}

impl RuleRow {
    fn from_cfg(cfg: &BoxConfig) -> Vec<RuleRow> {
        cfg.categories
            .iter()
            .map(|c| RuleRow {
                key: c.key.clone(),
                label: c.label.clone(),
                exts: c.exts.join(" "),
            })
            .collect()
    }

    /// Rebuild the whole document from the buffers. Order is the tab order; the
    /// organiser's own `repair()` still forces the catch-all to the end.
    fn build(rows: &[RuleRow], base: &BoxConfig) -> BoxConfig {
        let mut cfg = base.clone();
        cfg.categories = rows
            .iter()
            .map(|r| dcx_organizer::Category {
                key: r.key.clone(),
                label: r.label.clone(),
                exts: r
                    .exts
                    .split(|c: char| c.is_whitespace() || c == ',' || c == '，')
                    .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
                    .filter(|e| !e.is_empty())
                    .collect(),
            })
            .collect();
        cfg
    }
}

/// Everything `search.toml` exposes to the user. The app reads the same file;
/// a key this window does not write is left untouched by the read-modify-write
/// below (`ui.*` belongs to the app, `activation.enabled` is mirrored into
/// `apps.toml`).
#[derive(Clone, PartialEq)]
struct SearchValues {
    enabled: bool,
    hotkey: String,
    hotkey_enabled: bool,
    /// "release" | "keep_5m" | "resident"
    residency: String,
    /// "mouse" | "center" | "last"
    show_on: String,
    max_mb: u32,
    encoding: String,
    max_results: u32,
    time_budget_s: u32,
    include_hidden: bool,
    follow_links: bool,
    custom_roots: Vec<String>,
    /// One rule per line; kept as text while editing so a half-typed rule is
    /// never written into the index configuration.
    exclude_text: String,
    /// Hours between full index rebuilds. The on-disk snapshot is loaded on
    /// every launch; this only controls when it is considered stale.
    refresh_hours: u32,
}

fn default_excludes() -> Vec<String> {
    [
        "$Recycle.Bin",
        "System Volume Information",
        "WinSxS",
        "node_modules",
        "AppData\\Local\\Temp",
        "pagefile.sys",
        "hiberfil.sys",
        "swapfile.sys",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn string_array(value: Option<&toml::Value>) -> Option<Vec<String>> {
    value
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect()
        })
}

fn search_doc_path() -> std::path::PathBuf {
    let mut p = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push("dcx");
    p.push("search.toml");
    p
}

fn read_search_doc() -> toml::Value {
    let path = search_doc_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => match s.parse::<toml::Value>() {
            Ok(v) => {
                dcx_win32::config_file::note_parse_ok(&path);
                v
            }
            Err(e) => {
                // The search app reads the same file with its own schema. An
                // unparseable document must not be rewritten from these defaults
                // — that reset the other side's scope and exclusion rules.
                eprintln!("[dcx-host] parse {path:?} failed ({e}); keeping it read-only");
                dcx_win32::config_file::note_parse_error(&path);
                toml::Value::Table(Default::default())
            }
        },
        Err(_) => toml::Value::Table(Default::default()),
    }
}

fn search_set(doc: &mut toml::Value, section: &str, key: &str, value: toml::Value) {
    if !doc.is_table() {
        *doc = toml::Value::Table(Default::default());
    }
    let root = match doc.as_table_mut() {
        Some(t) => t,
        None => return,
    };
    if !root.contains_key(section) {
        root.insert(section.to_string(), toml::Value::Table(Default::default()));
    }
    if let Some(entry) = root.get_mut(section) {
        if !entry.is_table() {
            *entry = toml::Value::Table(Default::default());
        }
        if let Some(t) = entry.as_table_mut() {
            t.insert(key.to_string(), value);
        }
    }
}

fn load_search_values() -> SearchValues {
    let doc = read_search_doc();
    let a = doc.get("activation");
    let c = doc.get("content");
    let sc = doc.get("scope");
    let ix = doc.get("index");

    let pick = |section: Option<&toml::Value>, key: &str, default: &str| -> String {
        section
            .and_then(|v| v.get(key))
            .and_then(toml::Value::as_str)
            .unwrap_or(default)
            .to_string()
    };
    let flag = |section: Option<&toml::Value>, key: &str, default: bool| -> bool {
        section
            .and_then(|v| v.get(key))
            .and_then(toml::Value::as_bool)
            .unwrap_or(default)
    };
    let number = |section: Option<&toml::Value>, key: &str, default: i64| -> i64 {
        section
            .and_then(|v| v.get(key))
            .and_then(toml::Value::as_integer)
            .unwrap_or(default)
    };

    let residency = match pick(a, "residency", "release").as_str() {
        "keep_5m" => "keep_5m",
        "resident" => "resident",
        _ => "release",
    };
    let show_on = match pick(a, "show_on", "mouse").as_str() {
        "center" => "center",
        "last" => "last",
        _ => "mouse",
    };
    let encoding = match pick(c, "encoding", "auto").as_str() {
        "utf8" => "utf8",
        "gb18030" => "gb18030",
        _ => "auto",
    };
    let excludes = string_array(sc.and_then(|v| v.get("exclude"))).unwrap_or_else(default_excludes);

    SearchValues {
        enabled: crate::apps::is_enabled("search"),
        hotkey: pick(a, "hotkey", "Alt+s"),
        hotkey_enabled: flag(a, "hotkey_enabled", true),
        residency: residency.to_string(),
        show_on: show_on.to_string(),
        max_mb: number(c, "max_file_size_mb", 2).clamp(1, 64) as u32,
        encoding: encoding.to_string(),
        max_results: number(c, "max_results", 2000).clamp(100, 20_000) as u32,
        time_budget_s: (number(c, "time_budget_ms", 8000).clamp(1000, 60_000) / 1000) as u32,
        include_hidden: flag(sc, "include_hidden", false),
        follow_links: flag(sc, "follow_links", false),
        custom_roots: string_array(sc.and_then(|v| v.get("custom_roots"))).unwrap_or_default(),
        exclude_text: excludes.join("\n"),
        refresh_hours: ((number(ix, "refresh_minutes", 360).clamp(60, 10_080) + 59) / 60) as u32,
    }
}

/// Atomic replace + keep `apps.toml` in step. Both files are read by another
/// process every few seconds, so a half-write would be observed as a reset.
fn save_search_values(v: &SearchValues, prev_enabled: bool) -> Result<(), String> {
    let mut doc = read_search_doc();
    search_set(&mut doc, "activation", "enabled", toml::Value::Boolean(v.enabled));
    search_set(
        &mut doc,
        "activation",
        "hotkey",
        toml::Value::String(v.hotkey.clone()),
    );
    search_set(
        &mut doc,
        "activation",
        "hotkey_enabled",
        toml::Value::Boolean(v.hotkey_enabled),
    );
    search_set(
        &mut doc,
        "activation",
        "residency",
        toml::Value::String(v.residency.clone()),
    );
    search_set(
        &mut doc,
        "activation",
        "show_on",
        toml::Value::String(v.show_on.clone()),
    );
    search_set(
        &mut doc,
        "content",
        "max_file_size_mb",
        toml::Value::Integer(v.max_mb as i64),
    );
    search_set(
        &mut doc,
        "content",
        "encoding",
        toml::Value::String(v.encoding.clone()),
    );
    search_set(
        &mut doc,
        "content",
        "max_results",
        toml::Value::Integer(v.max_results as i64),
    );
    search_set(
        &mut doc,
        "content",
        "time_budget_ms",
        toml::Value::Integer(v.time_budget_s as i64 * 1000),
    );
    search_set(
        &mut doc,
        "scope",
        "include_hidden",
        toml::Value::Boolean(v.include_hidden),
    );
    search_set(
        &mut doc,
        "scope",
        "follow_links",
        toml::Value::Boolean(v.follow_links),
    );
    search_set(
        &mut doc,
        "scope",
        "custom_roots",
        toml::Value::Array(
            v.custom_roots
                .iter()
                .map(|p| toml::Value::String(p.clone()))
                .collect(),
        ),
    );
    search_set(
        &mut doc,
        "scope",
        "exclude",
        toml::Value::Array(
            v.exclude_text
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| toml::Value::String(s.to_string()))
                .collect(),
        ),
    );
    search_set(
        &mut doc,
        "index",
        "refresh_minutes",
        toml::Value::Integer(v.refresh_hours.max(1) as i64 * 60),
    );
    let text = toml::to_string_pretty(&doc).map_err(|e| format!("序列化失败：{e}"))?;
    let path = search_doc_path();
    {
        let _guard = dcx_win32::config_file::FileLock::acquire(
            &path,
            dcx_win32::config_file::LOCK_TIMEOUT,
        )
        .map_err(|e| format!("配置文件被占用：{e}"))?;
        if dcx_win32::config_file::is_poisoned(&path) {
            return Err(dcx_win32::config_file::poisoned_error(&path).to_string());
        }
        dcx_win32::config_file::write_atomic(&path, text.as_bytes())
            .map_err(|e| format!("写入失败：{e}"))?;
    }
    if v.enabled != prev_enabled {
        crate::apps::set_enabled("search", v.enabled)?;
    }
    Ok(())
}

struct SettingsApp {
    /// Selected navigation page.
    page: Page,
    /// Live document being edited.
    s: Settings,
    /// Last document successfully written to disk.
    saved: Settings,
    /// The shell's taskbar auto-hide bit, sampled when the window opens. Not
    /// part of `s`: Windows owns that state, we only mirror it for display.
    auto_hide: bool,
    /// Whether the current user's Run key points at this exe. Also outside `s`:
    /// the value is machine state (see `autostart`), not a DCX setting.
    autostart: bool,
    /// When `auto_hide` / `autostart` were last re-read from the shell.
    /// `SHAppBarMessage` is a synchronous shell call; doing it per frame froze
    /// the whole window while Explorer was busy or restarting.
    shell_state_at: Instant,
    /// 「重新应用」 is injecting on a worker thread.
    reapply_busy: bool,
    /// A library migration is running on a worker thread.
    migrate_busy: bool,
    /// Per-category counts shown on the rule card. `counts()` scans the whole
    /// library; running it on every frame made this page the slowest one.
    bx_counts: Vec<(String, usize, u64)>,
    /// `None` = refresh on the next frame.
    bx_counts_at: Option<Instant>,
    /// When the current unsaved edit started (debounce clock).
    dirty_since: Option<Instant>,
    /// Earliest time we will retry after a failed write.
    retry_after: Instant,
    /// Last write error, cleared on success.
    error: Option<String>,
    /// When the 「恢复并退出」 button was armed. A second click inside
    /// [`CONFIRM_WINDOW`] restarts the shell; the timer disarms on its own so a
    /// stray click cannot leave a primed destructive button behind.
    confirm_quit_at: Option<Instant>,

    // -- desktop file box ---------------------------------------------------
    /// Box document being edited (its own file, see `dcx-organizer`).
    bx: BoxConfig,
    /// Last box document written to disk.
    bx_saved: BoxConfig,
    /// Edit buffers for the rule card.
    bx_rows: Vec<RuleRow>,
    /// Debounce clock for the box document — separate from the taskbar one so a
    /// slider drag and a rule edit do not fight over the same timer.
    bx_dirty: Option<Instant>,
    /// Set by the three actions that edit the path-keyed manifest
    /// (`overrides` / `hidden` / `excluded`): 「清除手动分类」, deleting a category
    /// and migrating the library. Those are the only edits this window makes to
    /// keys `dcx-box.exe` also writes, so `persist_box` refreshes the manifest
    /// from disk unless this is set. Without it the stale copy loaded in `new()`
    /// was written back and wiped every manual classification made in the panel.
    bx_manifest_dirty: bool,
    /// Result of the last「立即整理」/「立即归档」, shown for a few seconds.
    bx_result: Option<(String, Instant)>,
    /// Theme the native caption has been tinted for. `None` until the first
    /// frame, when the window finally exists and can be found by title.
    caption_tint: Option<bool>,

    // -- file search --------------------------------------------------------
    search: SearchValues,
    search_saved: SearchValues,
    search_dirty: Option<Instant>,
    search_error: Option<String>,
    /// Half-typed exclusion rule; kept out of `SearchValues` so typing does not
    /// mark the document dirty before「添加」is pressed.
    exclude_new: String,

    // -- desktop info island ------------------------------------------------
    /// `island.toml` being edited. The island process also writes this document
    /// (tab, panel size), so `persist_island` merges the `ui` table from disk.
    island: dcx_island_config::Config,
    island_saved: dcx_island_config::Config,
    island_dirty: Option<Instant>,
    island_error: Option<String>,
    /// Credential edit buffers. They are deliberately **not** part of the
    /// document: secrets go to the Windows credential manager on an explicit
    /// button press, never into a TOML or a debounced auto-save.
    island_imap_secret: String,
    island_mail_secret: String,
    island_wecom_secret: String,
}

impl SettingsApp {
    fn new() -> Self {
        let s = Settings::load();
        let mut bx = BoxConfig::load();
        // Migrate old per-file hiding even when the box process is disabled.
        // This is what makes desktop folders visible in file dialogs again.
        if !bx.hidden.is_empty() {
            let mut org = BoxOrganizer::new(bx.clone());
            org.restore_hidden();
            bx.hidden = org.cfg.hidden.clone();
        }
        // The collection model keeps Explorer's desktop native; give back
        // icons an earlier build hid and left an ownership marker behind.
        restore_desktop_icons_if_owned();
        let search = load_search_values();
        let mut island = dcx_island_config::load();
        // `apps.toml` is the host's registry; mirror it into the document so
        // the single master switch always shows one consistent state.
        island.enabled = crate::apps::is_enabled("island");
        SettingsApp {
            saved: s.clone(),
            auto_hide: crate::auto_hide::is_enabled(),
            autostart: crate::autostart::is_enabled(),
            shell_state_at: Instant::now(),
            reapply_busy: false,
            migrate_busy: false,
            bx_counts: Vec::new(),
            bx_counts_at: None,
            s,
            dirty_since: None,
            retry_after: Instant::now(),
            error: None,
            confirm_quit_at: None,
            bx_saved: bx.clone(),
            bx_rows: RuleRow::from_cfg(&bx),
            bx_dirty: None,
            bx_manifest_dirty: false,
            bx_result: None,
            caption_tint: None,
            search_saved: search.clone(),
            search,
            search_dirty: None,
            search_error: None,
            exclude_new: String::new(),
            island_saved: island.clone(),
            island,
            island_dirty: None,
            island_error: None,
            island_imap_secret: String::new(),
            island_mail_secret: String::new(),
            island_wecom_secret: String::new(),
            page: Page::from_code(START_PAGE.swap(PAGE_LOOK, Ordering::SeqCst)),
            bx,
        }
    }

    fn persist(&mut self) {
        match self.s.save() {
            Ok(()) => {
                self.saved = self.s.clone();
                self.dirty_since = None;
                self.error = None;
                println!("[dcx-host] settings written — engine reloads within ~1s");
            }
            Err(e) => {
                self.error = Some(format!("保存失败：{e}"));
                self.retry_after = Instant::now() + Duration::from_secs(2);
                self.dirty_since = Some(Instant::now());
                eprintln!("[dcx-host] save settings failed: {e}");
            }
        }
    }

    /// Reset look-and-feel but keep the window theme the user is currently in.
    fn reset(&mut self) {
        let theme = self.s.ui_theme;
        self.s = Settings::default();
        self.s.ui_theme = theme;
    }

    // -- page ---------------------------------------------------------------

    fn sidebar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        // No inter-item gap: egui's default 7 pt item spacing plus the old
        // 3 pt add_space stretched the pitch to 56 pt (70 physical px at
        // 125 %). Win11 measures 50 px, but a 40 pt row felt too cramped, so
        // the row is 46 pt with zero gap — a 46 pt pitch (≈58 physical px).
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.add_space(14.0);
        for (page, icon, label) in [
            (Page::Look, NavIcon::Appearance, "任务栏样式"),
            (Page::Box, NavIcon::Box, "桌面整理"),
            (Page::Search, NavIcon::Search, "文件检索"),
            (Page::Island, NavIcon::Island, "桌面信息岛"),
            (Page::About, NavIcon::About, "关于"),
        ] {
            if nav_item(ui, p, self.page == page, icon, label).clicked() {
                self.page = page;
            }
        }

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            let dirty = self.s != self.saved;
            status_block(ui, p, dirty, self.error.as_deref());
        });
    }

    fn page_head(&mut self, ui: &mut egui::Ui, p: &Palette, title: &str, subtitle: &str) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(title).font(bold(30.0)).color(p.text));
                ui.add_space(5.0);
                ui.label(RichText::new(subtitle).size(13.5).color(p.secondary));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let is_dark = self.s.ui_theme == UiTheme::Dark;
                if let Some(dark) = theme_switch(ui, p, is_dark) {
                    self.s.ui_theme = if dark { UiTheme::Dark } else { UiTheme::Light };
                }
            });
        });
    }

    fn about_card(&self, ui: &mut egui::Ui, p: &Palette) {
        card(ui, p, |ui| {
            row(ui, p, RowIcon::Doc, "配置文件", "settings.toml", |ui| {
                if standard_button(ui, p, "打开目录", true).clicked() {
                    reveal_settings();
                }
            });
            row_separator(ui, p);
            let (engine, color) = engine_state_text(p);
            row_info(ui, p, RowIcon::Info, "引擎状态", &engine, |ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, color);
            });
            row_separator(ui, p);
            row_info(
                ui,
                p,
                RowIcon::Spacing,
                "当前配置",
                &summary(&self.s),
                |ui| {
                    ui.label(RichText::new("").size(12.0).color(p.secondary));
                },
            );
            row_separator(ui, p);
            row_info(
                ui,
                p,
                RowIcon::Doc,
                "版本",
                env!("CARGO_PKG_VERSION"),
                |ui| {
                    ui.label(RichText::new("").size(12.0).color(p.secondary));
                },
            );
            row_separator(ui, p);
            row(ui, p, RowIcon::Link, "项目主页", GITHUB_URL, |ui| {
                if standard_button(ui, p, "打开", true).clicked() {
                    open_url(GITHUB_URL);
                }
            });
        });
    }

    /// Handing the taskbar back is the one action in here that touches the
    /// shell, so it gets its own card with an inline confirmation instead of a
    /// button in the footer row next to 「恢复默认」.
    fn shutdown_card(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let busy = crate::shutdown::is_busy();
        // Drop a stale arm: the user clicked once and walked away.
        if let Some(at) = self.confirm_quit_at {
            if at.elapsed() >= CONFIRM_WINDOW {
                self.confirm_quit_at = None;
            }
        }
        let armed = self.confirm_quit_at.is_some();

        let (title, subtitle) = if busy {
            ("正在恢复任务栏", "正在重启资源管理器，随后退出")
        } else if armed {
            ("确认恢复任务栏？", "再点一次确认，桌面会闪一下")
        } else {
            (
                "退出并恢复任务栏",
                "重启资源管理器并退出 DCX，任务栏恢复系统默认",
            )
        };

        card(ui, p, |ui| {
            row(ui, p, RowIcon::Exit, title, subtitle, |ui| {
                let label = if busy {
                    "正在恢复…"
                } else if armed {
                    "确认恢复"
                } else {
                    "恢复并退出"
                };
                let clicked = if armed {
                    danger_button(ui, p, label, !busy)
                } else {
                    standard_button(ui, p, label, !busy)
                };
                if clicked.clicked() && !busy {
                    if armed {
                        // Persist first: the debounce may not have flushed the
                        // last edit, and this process is about to disappear.
                        self.persist();
                        self.confirm_quit_at = None;
                        println!("[dcx-host] restoring taskbar (shell restart) and quitting");
                        crate::shutdown::restore_taskbar_and_quit();
                    } else {
                        self.confirm_quit_at = Some(Instant::now());
                    }
                }
            });
        });

        // The restart runs on its own thread, so the click alone does not
        // schedule another frame — without this the "正在恢复" state would never
        // paint before the window goes away.
        if crate::shutdown::is_busy() {
            ui.ctx().request_repaint_after(Duration::from_millis(120));
        }
    }

    fn appearance_card(&mut self, ui: &mut egui::Ui, p: &Palette) {
        card(ui, p, |ui| {
            row(
                ui,
                p,
                RowIcon::Shape,
                "形状",
                "胶囊：两端 25 px 大圆角；卡片：四角统一 8 px 圆角",
                |ui| {
                    let mut shape = self.s.taskbar_shape;
                    if radios(
                        ui,
                        p,
                        &mut shape,
                        &[
                            (TaskbarShape::Capsule, "胶囊"),
                            (TaskbarShape::Card, "卡片"),
                        ],
                    ) {
                        self.s.taskbar_shape = shape;
                    }
                },
            );
            row_separator(ui, p);
            row(
                ui,
                p,
                RowIcon::Hover,
                "悬停效果",
                "背景：只高亮底板；放大：图标按距离放大、推开相邻图标，并隐藏底板",
                |ui| {
                    let mut mode = self.s.hover_effect;
                    if radios(
                        ui,
                        p,
                        &mut mode,
                        &[(HoverEffect::Background, "背景"), (HoverEffect::Dock, "放大")],
                    ) {
                        self.s.hover_effect = mode;
                    }
                },
            );
            row_separator(ui, p);
            slider_row(
                ui,
                p,
                RowIcon::Size,
                "图标大小",
                "按百分比缩放任务按钮图标（50–125%），不影响任务栏高度",
                &mut self.s.icon_scale,
                MIN_ICON_SCALE,
                MAX_ICON_SCALE,
                "%",
                true,
            );
            row_separator(ui, p);
            row(
                ui,
                p,
                RowIcon::Preset,
                "快速预设",
                "一键套用常用组合",
                |ui| {
                    // Added in reverse: the parent layout places controls right-to-left.
                    if small_button(ui, p, "宽松", is_roomy(&self.s)).clicked() {
                        apply_roomy(&mut self.s);
                    }
                    ui.add_space(2.0);
                    if small_button(ui, p, "标准", is_standard(&self.s)).clicked() {
                        apply_standard(&mut self.s);
                    }
                    ui.add_space(2.0);
                    if small_button(ui, p, "紧凑", is_compact(&self.s)).clicked() {
                        apply_compact(&mut self.s);
                    }
                },
            );
        });
    }

    fn metrics_card(&mut self, ui: &mut egui::Ui, p: &Palette) {
        card(ui, p, |ui| {
            let height_max = MAX_TASKBAR_HEIGHT.min(TASKBAR_BUDGET).max(MIN_TASKBAR_HEIGHT);
            let mut h = self.s.effective_height();
            if slider_row(
                ui,
                p,
                RowIcon::Tall,
                "任务栏高度",
                "可见高度 30–44 px（托盘同高）；上限由任务栏窗口决定",
                &mut h,
                MIN_TASKBAR_HEIGHT,
                height_max,
                " px",
                true,
            ) {
                self.s.taskbar_height = Some(h);
            }
            row_separator(ui, p);
            slider_row(
                ui,
                p,
                RowIcon::Spacing,
                "图标水平间距",
                "任务按钮左右额外 Margin，负数收紧（−6 到 +6）",
                &mut self.s.icon_spacing.between,
                MIN_ICON_SPACING,
                MAX_ICON_SPACING,
                " px",
                true,
            );

            row_separator(ui, p);

            // Height is intentionally NOT exposed.
            //
            // The bar height is owned by `Shell_TrayWnd`; writing a larger
            // `RootGrid Height` into its XAML only makes the dock overflow the
            // window and get cut off at the screen edge — there is no setting
            // here that can safely do it. `taskbar_height` therefore stays a
            // hand-edit-only key (see README) and defaults to "leave it alone".
            let mut auto_w = self.s.max_width.is_none();
            let hint = format!("{DEFAULT_MAX_WIDTH} px");
            row_tight(
                ui,
                p,
                RowIcon::Width,
                "最大宽度跟随默认",
                &hint,
                |ui| {
                    if switch(ui, p, &mut auto_w) {
                        self.s.max_width = if auto_w {
                            None
                        } else {
                            Some(DEFAULT_MAX_WIDTH)
                        };
                    }
                },
            );
            if self.s.max_width.is_some() {
                row_separator(ui, p);
                let mut v = self.s.max_width.unwrap_or(DEFAULT_MAX_WIDTH);
                if slider_row(
                    ui,
                    p,
                    RowIcon::Width,
                    "任务栏最大宽度",
                    "窄于屏幕时居中悬浮",
                    &mut v,
                    1024,
                    2048,
                    " px",
                    true,
                ) {
                    self.s.max_width = Some(v);
                }
            }
        });
    }

    fn behavior_card(&mut self, ui: &mut egui::Ui, p: &Palette) {
        card(ui, p, |ui| {
            let mut on = self.auto_hide;
            row_tight(
                ui,
                p,
                RowIcon::Hide,
                "自动隐藏任务栏",
                "鼠标移到屏幕底边才滑出；系统自己保存，不写进 settings.toml",
                |ui| {
                    if switch(ui, p, &mut on) {
                        self.apply_auto_hide(on);
                    }
                },
            );
            let mut boot = self.autostart;
            row_tight(
                ui,
                p,
                RowIcon::Power,
                "开机自启",
                "登录后只进托盘、不弹设置窗口；写当前用户启动项，关闭即删除",
                |ui| {
                    if switch(ui, p, &mut boot) {
                        self.apply_autostart(boot);
                    }
                },
            );
        });
    }

    /// Writes the shell's auto-hide bit. The next repaint re-reads it, so a
    /// rejected write puts the switch back where the taskbar actually is.
    fn apply_auto_hide(&mut self, on: bool) {
        match crate::auto_hide::set_enabled(on) {
            Ok(()) => self.error = None,
            Err(e) => self.error = Some(e),
        }
    }

    /// Writes (or removes) the Run-key entry. Unlike auto-hide, nothing else
    /// owns this value, so a failure is ours to report — the switch snaps back
    /// on the next repaint because `update` re-reads the registry.
    fn apply_autostart(&mut self, on: bool) {
        match crate::autostart::set_enabled(on) {
            Ok(()) => self.error = None,
            Err(e) => self.error = Some(e),
        }
    }

    // -- 独立应用：状态与平台行为 -------------------------------------------

    /// Read-only running state of one standalone app. The enable switch stays
    /// on the app's own page (and, for the box, is the same master switch that
    /// keeps `organizer.toml` and `apps.toml` in step).
    fn app_status_row(&self, ui: &mut egui::Ui, p: &Palette, id: &str, icon: RowIcon) {
        let entry = crate::apps::entries().iter().find(|e| e.id == id);
        let title = entry.map(|e| e.name).unwrap_or("运行状态");
        let subtitle = entry
            .map(|e| format!("{} · {}", e.exe, e.description))
            .unwrap_or_default();
        let running = crate::apps::is_running(id);
        let (text, color) = if running {
            ("运行中", p.ok)
        } else {
            ("已停止", p.tertiary)
        };
        row_tight(ui, p, icon, title, &subtitle, |ui| {
            ui.label(RichText::new(text).size(12.0).color(color));
        });
    }

    /// Platform-wide lifecycle card, shown on 「关于」. Deliberately contains no
    /// per-app switch: each app is enabled on its own settings page.
    fn platform_card(&mut self, ui: &mut egui::Ui, p: &Palette) {
        caption(ui, p, "平台行为");
        card(ui, p, |ui| {
            row(
                ui,
                p,
                RowIcon::Info,
                "统一启动与看门狗",
                "应用由 dcx-host 按 apps.toml 启停，开关在各应用自己的设置页",
                |ui| {
                    if standard_button(ui, p, "打开目录", true).clicked() {
                        // apps.toml lives beside settings.toml.
                        reveal_settings();
                    }
                },
            );
        });
    }

    // -- 文件检索 -----------------------------------------------------------

    fn persist_search(&mut self) {
        match save_search_values(&self.search, self.search_saved.enabled) {
            Ok(()) => {
                self.search_saved = self.search.clone();
                self.search_dirty = None;
                self.search_error = None;
            }
            Err(e) => {
                self.search_error = Some(e);
                self.search_dirty = None;
            }
        }
    }

    /// Search edits are flushed even after the user leaves the page: the timer
    /// lives here (called every frame by `ui`) instead of inside the page
    /// render, so switching to another page cannot defer the write until close.
    fn search_tick(&mut self, ctx: &egui::Context) {
        if let Some(t) = self.search_dirty {
            if t.elapsed() >= Duration::from_millis(220) {
                self.persist_search();
            }
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    /// File search settings. Everything here lives in `search.toml`; the app
    /// polls the file's `(len, mtime)` stamp every frame and applies a change
    /// to the next query, so nothing here needs a restart.
    fn search_cards(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let mut sv = self.search.clone();
        let mut want_folder = false;
        let mut remove_root: Option<usize> = None;
        let defaults = default_excludes();
        let mut exclude_rules: Vec<String> = sv
            .exclude_text
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let mut exclude_remove: Option<usize> = None;
        let mut exclude_restore = false;
        let mut exclude_add: Option<String> = None;

        caption(ui, p, "常规");
        card(ui, p, |ui| {
            let mut enabled = sv.enabled;
            row_tight(
                ui,
                p,
                RowIcon::Folder,
                "启用文件检索（常驻）",
                "随平台启动并预建索引；关闭后不占后台磁盘，快捷键仍可按需唤起（首次唤起即时建索引）",
                |ui| {
                    switch(ui, p, &mut enabled);
                },
            );
            sv.enabled = enabled;
            row_separator(ui, p);
            self.app_status_row(ui, p, "search", RowIcon::Search);
            row_separator(ui, p);

            let mut hk_on = sv.hotkey_enabled;
            row_tight(
                ui,
                p,
                RowIcon::Power,
                "启用全局快捷键",
                "由 dcx-host 常驻监听，应用未运行时也能唤起；再次按下可隐藏窗口",
                |ui| {
                    switch(ui, p, &mut hk_on);
                },
            );
            sv.hotkey_enabled = hk_on;
            row_separator(ui, p);

            row(
                ui,
                p,
                RowIcon::Info,
                "全局快捷键",
                "示例 Alt+s、Ctrl+Shift+F；必须带 Alt / Ctrl / Shift / Win 之一，改键约 1 秒后生效",
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut sv.hotkey)
                            .desired_width(130.0),
                    );
                    if let Err(why) = crate::hotkey::validate(&sv.hotkey) {
                        ui.label(RichText::new(why).size(11.0).color(p.danger));
                    }
                },
            );
            row_separator(ui, p);

            let mut residency = match sv.residency.as_str() {
                "keep_5m" => 1u8,
                "resident" => 2,
                _ => 0,
            };
            row_reserve(
                ui,
                p,
                RowIcon::Power,
                "关闭窗口后",
                "退出释放索引；保留 5 分钟内可再唤起；常驻仅隐藏窗口",
                300.0,
                |ui| {
                    radios(
                        ui,
                        p,
                        &mut residency,
                        &[(0u8, "退出"), (1u8, "保留 5 分钟"), (2u8, "后台常驻")],
                    );
                },
            );
            sv.residency = match residency {
                1 => "keep_5m",
                2 => "resident",
                _ => "release",
            }
            .to_string();
            row_separator(ui, p);

            let mut show_on = match sv.show_on.as_str() {
                "center" => 1u8,
                "last" => 2,
                _ => 0,
            };
            row_reserve(
                ui,
                p,
                RowIcon::Desktop,
                "唤起位置",
                "默认出现在指针所在的显示器；上次位置会记住关闭时停靠点",
                300.0,
                |ui| {
                    radios(
                        ui,
                        p,
                        &mut show_on,
                        &[(0u8, "鼠标处"), (1u8, "屏幕中央"), (2u8, "上次位置")],
                    );
                },
            );
            sv.show_on = match show_on {
                1 => "center",
                2 => "last",
                _ => "mouse",
            }
            .to_string();
            row_separator(ui, p);

            let mut hours = sv.refresh_hours;
            row(
                ui,
                p,
                RowIcon::Info,
                "索引缓存刷新",
                "启动先载入上次索引；超过该间隔才全盘重建。运行中的文件变化实时更新",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut hours)
                            .range(1..=168)
                            .suffix(" 小时"),
                    );
                },
            );
            sv.refresh_hours = hours;
        });

        ui.add_space(18.0);
        caption(ui, p, "检索范围");
        card(ui, p, |ui| {
            let mut hidden = sv.include_hidden;
            row_tight(
                ui,
                p,
                RowIcon::Hide,
                "包含隐藏文件",
                "默认跳过隐藏与系统属性文件；开启后索引会明显变大",
                |ui| {
                    switch(ui, p, &mut hidden);
                },
            );
            sv.include_hidden = hidden;
            row_separator(ui, p);

            let mut links = sv.follow_links;
            row_tight(
                ui,
                p,
                RowIcon::Folder,
                "跟随符号链接",
                "默认不跟随，避免目录环与跨盘重复索引",
                |ui| {
                    switch(ui, p, &mut links);
                },
            );
            sv.follow_links = links;
            row_separator(ui, p);

            let roots = sv.custom_roots.clone();
            if roots.is_empty() {
                row_tight(
                    ui,
                    p,
                    RowIcon::Folder,
                    "自定义检索文件夹",
                    "添加后可在检索窗口的路径浮层里直接选择；范围变化会触发重建索引",
                    |ui| {
                        if standard_button(ui, p, "添加文件夹", true).clicked() {
                            want_folder = true;
                        }
                    },
                );
            } else {
                for (i, root) in roots.iter().enumerate() {
                    if i > 0 {
                        row_separator(ui, p);
                    }
                    let name = std::path::Path::new(root)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| root.clone());
                    row(ui, p, RowIcon::Folder, &name, root, |ui| {
                        if delete_button(ui, p, "移除这个检索文件夹") {
                            remove_root = Some(i);
                        }
                    });
                }
                row_separator(ui, p);
                row_tight(
                    ui,
                    p,
                    RowIcon::Folder,
                    "添加文件夹",
                    "范围变化会触发重建索引；索引建立期间仍可检索已收录的文件",
                    |ui| {
                        if standard_button(ui, p, "浏览…", true).clicked() {
                            want_folder = true;
                        }
                    },
                );
            }
            row_separator(ui, p);
            row_reserve(
                ui,
                p,
                RowIcon::Hide,
                "排除规则",
                "按路径子串匹配；范围变化会触发重建索引",
                96.0,
                |ui| {
                    if standard_button(ui, p, "恢复默认", exclude_rules != defaults).clicked() {
                        exclude_restore = true;
                    }
                },
            );
            // ONE setting item: the whole editor is enclosed in a single inset
            // well, so the rules never read as a stack of separate settings.
            egui::Frame::none()
                .inner_margin(Margin {
                    left: 16.0,
                    right: 16.0,
                    top: 0.0,
                    bottom: 13.0,
                })
                .show(ui, |ui| {
                    egui::Frame::none()
                        .fill(p.hover)
                        .stroke(Stroke::new(1.0_f32, p.border))
                        .rounding(Rounding::same(6.0))
                        .inner_margin(Margin::symmetric(12.0, 12.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if exclude_rules.is_empty() {
                                ui.label(
                                    RichText::new("当前不排除任何路径，所有可读文件都会进入索引")
                                        .size(12.0)
                                        .color(p.secondary),
                                );
                            } else {
                                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                                ui.horizontal_wrapped(|ui| {
                                    for (i, rule) in exclude_rules.iter().enumerate() {
                                        if exclude_chip(ui, p, rule) {
                                            exclude_remove = Some(i);
                                        }
                                    }
                                });
                            }
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                // Same 32 pt height as `standard_button`; the
                                // default TextEdit was shorter and the pair read
                                // as two unrelated controls.
                                let edit = ui.add_sized(
                                    egui::vec2(260.0, 32.0),
                                    egui::TextEdit::singleline(&mut self.exclude_new)
                                        .hint_text(
                                            RichText::new("输入路径或名称，回车添加")
                                                .size(11.5)
                                                .color(p.tertiary),
                                        )
                                        .font(font(12.5))
                                        .text_color(p.text)
                                        .vertical_align(Align::Center)
                                        .margin(Margin::symmetric(8.0, 4.0)),
                                );
                                let submitted = edit.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                let ready = !self.exclude_new.trim().is_empty();
                                if standard_button(ui, p, "添加", ready).clicked()
                                    || (submitted && ready)
                                {
                                    exclude_add = Some(self.exclude_new.trim().to_string());
                                }
                            });
                        });
                });
        });
        if let Some(rule) = exclude_add {
            self.exclude_new.clear();
            exclude_rules.push(rule);
        }
        if let Some(i) = exclude_remove {
            if i < exclude_rules.len() {
                exclude_rules.remove(i);
            }
        }
        if exclude_restore {
            exclude_rules = defaults;
        }
        let exclude_text = exclude_rules.join("\n");
        if exclude_text != sv.exclude_text {
            sv.exclude_text = exclude_text;
        }

        ui.add_space(18.0);
        caption(ui, p, "内容检索");
        card(ui, p, |ui| {
            row(
                ui,
                p,
                RowIcon::Size,
                "单文件上限",
                "超过此大小的文本文件跳过，避免长时间占用磁盘",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut sv.max_mb)
                            .range(1..=64)
                            .suffix(" MB"),
                    );
                },
            );
            row_separator(ui, p);

            let mut encoding = match sv.encoding.as_str() {
                "utf8" => 1u8,
                "gb18030" => 2,
                _ => 0,
            };
            row_reserve(
                ui,
                p,
                RowIcon::Doc,
                "文本编码",
                "自动：BOM 嗅探；中文无命中时回退 GB18030",
                330.0,
                |ui| {
                    radios(
                        ui,
                        p,
                        &mut encoding,
                        &[(0u8, "自动"), (1u8, "UTF-8"), (2u8, "GB18030")],
                    );
                },
            );
            sv.encoding = match encoding {
                1 => "utf8",
                2 => "gb18030",
                _ => "auto",
            }
            .to_string();
            row_separator(ui, p);

            slider_row(
                ui,
                p,
                RowIcon::Sort,
                "结果上限",
                "超出后标记「结果已截断」，提示细化关键词；文件名与内容检索共用",
                &mut sv.max_results,
                100,
                20_000,
                " 条",
                true,
            );
            row_separator(ui, p);
            slider_row(
                ui,
                p,
                RowIcon::Info,
                "时间预算",
                "单次检索的最长耗时，到点返回已有结果并标记为不完整",
                &mut sv.time_budget_s,
                1,
                60,
                " 秒",
                true,
            );
        });

        if want_folder {
            if let Some(path) = pick_folder("选择要加入检索范围的文件夹") {
                if !sv
                    .custom_roots
                    .iter()
                    .any(|p| p.eq_ignore_ascii_case(&path))
                {
                    sv.custom_roots.push(path);
                }
            }
        }
        if let Some(i) = remove_root {
            if i < sv.custom_roots.len() {
                sv.custom_roots.remove(i);
            }
        }

        if let Some(e) = crate::hotkey::last_error() {
            ui.add_space(6.0);
            ui.label(RichText::new(e).size(12.0).color(p.danger));
        }
        if let Some(e) = &self.search_error {
            ui.add_space(4.0);
            ui.label(RichText::new(e).size(12.0).color(p.danger));
        }
        if sv != self.search {
            self.search = sv;
            self.search_dirty = Some(Instant::now());
            ui.ctx().request_repaint_after(Duration::from_millis(120));
        }
        ui.add_space(18.0);
    }

    // -- 桌面信息岛 ---------------------------------------------------------

    /// Write `island.toml`. Both processes write this document, so the `ui`
    /// table (last tab, panel size) is re-read from disk and kept; without the
    /// merge a slider edit here would roll back the island's own geometry.
    fn persist_island(&mut self) {
        let prev_enabled = self.island_saved.enabled;
        let disk = dcx_island_config::load();
        let merged = dcx_island_config::Config {
            ui: disk.ui.clone(),
            ..self.island.clone()
        };
        match dcx_island_config::save(&merged) {
            Ok(()) => {
                // One master switch, two documents: the island process reads
                // `island.toml`, the host's watchdog reads `apps.toml`.
                if merged.enabled != prev_enabled {
                    if let Err(e) = crate::apps::set_enabled("island", merged.enabled) {
                        self.island_error = Some(e);
                        self.island_dirty = Some(Instant::now());
                        return;
                    }
                }
                self.island_saved = self.island.clone();
                self.island_dirty = None;
                self.island_error = None;
            }
            Err(e) => {
                self.island_error = Some(e);
                self.island_dirty = Some(Instant::now());
            }
        }
    }

    /// Island edits are flushed even after the user leaves the page.
    fn island_tick(&mut self, ctx: &egui::Context) {
        if let Some(t) = self.island_dirty {
            if t.elapsed() >= Duration::from_millis(220) {
                self.persist_island();
            }
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    fn island_cards(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let mut iv = self.island.clone();

        caption(ui, p, "常规");
        card(ui, p, |ui| {
            let mut enabled = iv.enabled;
            row_tight(
                ui,
                p,
                RowIcon::Power,
                "启用桌面信息岛",
                "唯一启停开关：同时写入 island.toml 与 apps.toml；默认关闭，开启后顶部常驻悬浮胶囊",
                |ui| {
                    switch(ui, p, &mut enabled);
                },
            );
            iv.enabled = enabled;
            row_separator(ui, p);
            self.app_status_row(ui, p, "island", RowIcon::Info);
            row_separator(ui, p);

            let mut hk_on = iv.hotkey_enabled;
            row_tight(
                ui,
                p,
                RowIcon::Power,
                "启用全局快捷键",
                "由 dcx-host 常驻监听；仅在信息岛已启用时有效，按下展开/收起面板",
                |ui| {
                    switch(ui, p, &mut hk_on);
                },
            );
            iv.hotkey_enabled = hk_on;
            row_separator(ui, p);

            row(
                ui,
                p,
                RowIcon::Info,
                "全局快捷键",
                "默认 Alt+i；必须带 Alt / Ctrl / Shift / Win 之一，改键约 1 秒后生效",
                |ui| {
                    ui.add(egui::TextEdit::singleline(&mut iv.hotkey).desired_width(130.0));
                    if let Err(why) = crate::hotkey::validate(&iv.hotkey) {
                        ui.label(RichText::new(why).size(11.0).color(p.danger));
                    }
                },
            );
            row_separator(ui, p);

            let mut hour24 = iv.clock.hour24;
            row_tight(ui, p, RowIcon::Info, "24 小时制", "关闭后显示 AM / PM", |ui| {
                switch(ui, p, &mut hour24);
            });
            iv.clock.hour24 = hour24;
            row_separator(ui, p);

            let mut show_seconds = iv.clock.show_seconds;
            row_tight(ui, p, RowIcon::Info, "显示秒", "紧凑态显示 HH:MM:SS", |ui| {
                switch(ui, p, &mut show_seconds);
            });
            iv.clock.show_seconds = show_seconds;
            row_separator(ui, p);

            let mut show_weekday = iv.clock.show_weekday;
            let mut show_date = iv.clock.show_date;
            row_tight(ui, p, RowIcon::Info, "显示星期", "紧凑态显示中文星期", |ui| {
                switch(ui, p, &mut show_weekday);
            });
            row_separator(ui, p);
            row_tight(ui, p, RowIcon::Info, "显示日期", "紧凑态同时显示年月日", |ui| {
                switch(ui, p, &mut show_date);
            });
            iv.clock.show_weekday = show_weekday;
            iv.clock.show_date = show_date;
            row_separator(ui, p);

            let mut hide_fs = iv.behavior.hide_fullscreen;
            row_tight(
                ui,
                p,
                RowIcon::Desktop,
                "全屏时自动隐藏",
                "视频、游戏、演示文稿处于前台时收起胶囊，避免遮挡",
                |ui| {
                    switch(ui, p, &mut hide_fs);
                },
            );
            iv.behavior.hide_fullscreen = hide_fs;
            row_separator(ui, p);

            slider_row(
                ui,
                p,
                RowIcon::Width,
                "胶囊宽度",
                "逻辑点，125% DPI 下乘以 1.25 为物理像素",
                &mut iv.behavior.capsule_width,
                150.0_f32,
                420.0_f32,
                " pt",
                true,
            );
            row_separator(ui, p);
            slider_row(
                ui,
                p,
                RowIcon::Tall,
                "胶囊高度",
                "逻辑点；过小会让文字被裁切",
                &mut iv.behavior.capsule_height,
                26.0_f32,
                64.0_f32,
                " pt",
                true,
            );
            row_separator(ui, p);
            slider_row(
                ui,
                p,
                RowIcon::Shape,
                "背景不透明度",
                "只作用于胶囊背景，文字保持不透明",
                &mut iv.behavior.opacity,
                0.6_f32,
                1.0_f32,
                "",
                true,
            );
        });

        ui.add_space(18.0);
        caption(ui, p, "系统性能");
        card(ui, p, |ui| {
            let mut show = iv.metrics.show;
            row_tight(ui, p, RowIcon::Power, "显示性能指标", "紧凑态显示 CPU / 内存等读数", |ui| {
                switch(ui, p, &mut show);
            });
            iv.metrics.show = show;
            row_separator(ui, p);
            for (flag, title, subtitle) in [
                (&mut iv.metrics.cpu, "CPU 占用", "GetSystemTimes 两次采样差分"),
                (&mut iv.metrics.memory, "内存占用", "GlobalMemoryStatusEx"),
                (&mut iv.metrics.disk, "磁盘已用", "GetDiskFreeSpaceExW（系统盘）"),
                (&mut iv.metrics.network, "网络速率", "GetIfTable2 差分上下行"),
            ] {
                let mut on = *flag;
                row_tight(ui, p, RowIcon::Size, title, subtitle, |ui| {
                    switch(ui, p, &mut on);
                });
                *flag = on;
                row_separator(ui, p);
            }

            row(
                ui,
                p,
                RowIcon::Info,
                "采样间隔",
                "后台线程采样周期，1–10 秒；越长越省电",
                |ui| {
                    ui.add(egui::DragValue::new(&mut iv.metrics.interval_secs).range(1..=10).suffix(" 秒"));
                },
            );
            row_separator(ui, p);
            let mut style = if iv.metrics.style == "ring" { 1u8 } else { 0u8 };
            row_reserve(ui, p, RowIcon::Shape, "指标样式", "条形或环形，超过 75% 变色", 240.0, |ui| {
                radios(ui, p, &mut style, &[(0u8, "条形"), (1u8, "环形")]);
            });
            iv.metrics.style = if style == 1 { "ring" } else { "bar" }.to_string();
        });

        ui.add_space(18.0);
        caption(ui, p, "任务与剪贴板");
        card(ui, p, |ui| {
            let mut carry = iv.tasks.carry_over;
            let mut sound = iv.tasks.sound;
            let mut dispatch_due = iv.tasks.dispatch_due;
            row_tight(ui, p, RowIcon::Sort, "未完成任务自动结转", "每日首次启动时把昨天未完成的任务移到今天", |ui| {
                switch(ui, p, &mut carry);
            });
            row_separator(ui, p);
            row_tight(ui, p, RowIcon::Info, "提醒声音", "到点时提示音", |ui| {
                switch(ui, p, &mut sound);
            });
            row_separator(ui, p);
            row_tight(
                ui,
                p,
                RowIcon::Link,
                "到点推送到出站渠道",
                "同时按下方「出站推送」的开关发送；未配置任何渠道时只做岛内弹层",
                |ui| {
                    switch(ui, p, &mut dispatch_due);
                },
            );
            iv.tasks.carry_over = carry;
            iv.tasks.sound = sound;
            iv.tasks.dispatch_due = dispatch_due;
            row_separator(ui, p);

            row_reserve(
                ui,
                p,
                RowIcon::Folder,
                "Markdown 归档目录",
                "留空表示不归档；任务按天导出",
                320.0,
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut iv.tasks.archive_dir)
                            .desired_width(230.0)
                            .hint_text("例如 D:\\文档\\岛归档"),
                    );
                },
            );
            row_separator(ui, p);

            let mut clip_on = iv.clipboard.enabled;
            let mut images = iv.clipboard.capture_images;
            let mut skip_pwd = iv.clipboard.skip_password_fields;
            row_tight(
                ui,
                p,
                RowIcon::Doc,
                "记录剪贴板历史",
                "文本与图片只存本地；首次开启请确认本机无合规限制",
                |ui| {
                    switch(ui, p, &mut clip_on);
                },
            );
            row_separator(ui, p);
            row_tight(ui, p, RowIcon::Shape, "记录图片", "关闭后只保留文本", |ui| {
                switch(ui, p, &mut images);
            });
            row_separator(ui, p);
            row_tight(
                ui,
                p,
                RowIcon::Hide,
                "跳过密码框内容",
                "焦点位于密码输入控件时不记录；这是尽力而为，不是安全保证",
                |ui| {
                    switch(ui, p, &mut skip_pwd);
                },
            );
            iv.clipboard.enabled = clip_on;
            iv.clipboard.capture_images = images;
            iv.clipboard.skip_password_fields = skip_pwd;
            row_separator(ui, p);

            row(
                ui,
                p,
                RowIcon::Size,
                "历史上限 / 单条上限",
                "条数 20–2000；单条文本字符数 200–200000，超出截断",
                |ui| {
                    ui.add(egui::DragValue::new(&mut iv.clipboard.max_items).range(20..=2000));
                    ui.add(egui::DragValue::new(&mut iv.clipboard.max_text_chars).range(200..=200_000));
                },
            );
            row_separator(ui, p);
            row(
                ui,
                p,
                RowIcon::Size,
                "图片目录预算",
                "超过后按时间清理最旧图片，单位 MB",
                |ui| {
                    ui.add(egui::DragValue::new(&mut iv.clipboard.max_image_mb).range(0..=4096).suffix(" MB"));
                },
            );
            row_separator(ui, p);
            let mut excludes = iv.clipboard.exclude_patterns.join("\n");
            ui.add_space(4.0);
            ui.label(RichText::new("不记录的文本规则（每行一条正则）").size(12.5).color(p.secondary));
            ui.add_space(4.0);
            ui.add(
                egui::TextEdit::multiline(&mut excludes)
                    .desired_rows(3)
                    .desired_width(ui.available_width())
                    .hint_text(r"(?i)\b(password|passwd|pwd)\s*[:=]"),
            );
            iv.clipboard.exclude_patterns = excludes
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        });

        ui.add_space(18.0);
        caption(ui, p, "通知渠道 · 入站");
        card(ui, p, |ui| {
            let mut wechat = iv.inbox.wechat;
            let mut wecom = iv.inbox.wecom;
            let mut qq = iv.inbox.qq;
            let mut mail = iv.inbox.mail;
            row_tight(
                ui,
                p,
                RowIcon::Info,
                "微信",
                "SHELLHOOK 任务栏关注信号；不读正文、发送人或准确未读数",
                |ui| {
                    switch(ui, p, &mut wechat);
                },
            );
            row_separator(ui, p);
            row_tight(
                ui,
                p,
                RowIcon::Info,
                "企业微信",
                "复用同一 SHELLHOOK 关注信号；是否触发 HSHELL_FLASH 以实机为准",
                |ui| {
                    switch(ui, p, &mut wecom);
                },
            );
            row_separator(ui, p);
            row_tight(
                ui,
                p,
                RowIcon::Info,
                "QQ",
                "WinRT UserNotificationListener 按 AUMID 过滤；未打包桌面程序常被系统拒绝授权",
                |ui| {
                    switch(ui, p, &mut qq);
                },
            );
            row_separator(ui, p);
            row_tight(
                ui,
                p,
                RowIcon::Info,
                "邮箱（IMAP）",
                "只同步未读邮件头，首次同步不提醒历史邮件；需要授权码",
                |ui| {
                    switch(ui, p, &mut mail);
                },
            );
            iv.inbox.wechat = wechat;
            iv.inbox.wecom = wecom;
            iv.inbox.qq = qq;
            iv.inbox.mail = mail;
            row_separator(ui, p);
            row(
                ui,
                p,
                RowIcon::Info,
                "每项保留条数",
                "消息条目只在本次运行的内存中保存，不落盘聊天内容",
                |ui| {
                    ui.add(egui::DragValue::new(&mut iv.inbox.keep).range(5..=200));
                },
            );
            row_separator(ui, p);
            let mut badge = iv.inbox.badge;
            row_tight(ui, p, RowIcon::Info, "紧凑态未读角标", "有新提醒时胶囊上显示计数", |ui| {
                switch(ui, p, &mut badge);
            });
            iv.inbox.badge = badge;
            row_separator(ui, p);

            row_reserve(
                ui,
                p,
                RowIcon::Info,
                "邮箱账号 / IMAP / SMTP",
                "授权码在下方凭据区保存，不写进配置文件",
                330.0,
                |ui| {
                    ui.add(egui::TextEdit::singleline(&mut iv.mail.user).desired_width(150.0).hint_text("账号"));
                    ui.add(egui::TextEdit::singleline(&mut iv.mail.imap_host).desired_width(120.0));
                    ui.add(egui::DragValue::new(&mut iv.mail.imap_port).range(1..=65535));
                },
            );
            row_separator(ui, p);
            row_reserve(
                ui,
                p,
                RowIcon::Info,
                "发件地址 / SMTP",
                "收件人留空表示发给自己；SMTP 走隐式 TLS（465）",
                330.0,
                |ui| {
                    ui.add(egui::TextEdit::singleline(&mut iv.mail.to).desired_width(140.0).hint_text("收件人（可选）"));
                    ui.add(egui::TextEdit::singleline(&mut iv.mail.smtp_host).desired_width(120.0));
                    ui.add(egui::DragValue::new(&mut iv.mail.smtp_port).range(1..=65535));
                },
            );
            row_separator(ui, p);
            row(
                ui,
                p,
                RowIcon::Info,
                "邮箱轮询间隔",
                "分钟；1–240，关闭渠道不会空转轮询",
                |ui| {
                    ui.add(egui::DragValue::new(&mut iv.mail.poll_minutes).range(1..=240).suffix(" 分钟"));
                },
            );
            row_separator(ui, p);
            let mut skip_backlog = iv.mail.skip_backlog;
            row_tight(ui, p, RowIcon::Info, "首次同步忽略历史邮件", "避免开启时补报一周的旧邮件", |ui| {
                switch(ui, p, &mut skip_backlog);
            });
            iv.mail.skip_backlog = skip_backlog;
        });

        ui.add_space(18.0);
        caption(ui, p, "通知渠道 · 出站与凭据");
        card(ui, p, |ui| {
            let mut dw = iv.dispatch.wecom;
            let mut dm = iv.dispatch.mail;
            row_tight(
                ui,
                p,
                RowIcon::Link,
                "企业微信群机器人",
                "官方 webhook，20 条/分钟上限；需要 Webhook Key",
                |ui| {
                    switch(ui, p, &mut dw);
                },
            );
            row_separator(ui, p);
            row_tight(ui, p, RowIcon::Link, "邮件 SMTP", "用上方发件配置推送提醒", |ui| {
                switch(ui, p, &mut dm);
            });
            iv.dispatch.wecom = dw;
            iv.dispatch.mail = dm;
            row_separator(ui, p);
            row(
                ui,
                p,
                RowIcon::Info,
                "最小发送间隔",
                "两次推送之间的最短秒数，默认 5 秒，远低于 webhook 限流",
                |ui| {
                    ui.add(egui::DragValue::new(&mut iv.dispatch.min_gap_secs).range(0..=60).suffix(" 秒"));
                },
            );
            row_separator(ui, p);

            island_cred_row(
                ui,
                p,
                "IMAP 授权码",
                "收件用；不是 QQ / 邮箱登录密码",
                dcx_island_config::credential::KEY_IMAP,
                &mut self.island_imap_secret,
                &mut self.island_error,
            );
            row_separator(ui, p);
            island_cred_row(
                ui,
                p,
                "SMTP 授权码",
                "留空则复用 IMAP 授权码",
                dcx_island_config::credential::KEY_MAIL,
                &mut self.island_mail_secret,
                &mut self.island_error,
            );
            row_separator(ui, p);
            island_cred_row(
                ui,
                p,
                "企业微信 Webhook Key",
                "群机器人 URL 中 key= 后面的值",
                dcx_island_config::credential::KEY_WECOM,
                &mut self.island_wecom_secret,
                &mut self.island_error,
            );
        });

        ui.add_space(18.0);
        caption(ui, p, "能力边界（避免误解）");
        card(ui, p, |ui| {
            row_info(
                ui,
                p,
                RowIcon::Info,
                "QQ / 微信个人号",
                "无官方出站发送接口；不做协议逆向、不做 UI 自动化模拟发送，设置页不提供「发消息」开关",
                |ui| {
                    ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                },
            );
            row_separator(ui, p);
            row_info(
                ui,
                p,
                RowIcon::Info,
                "企业微信入站",
                "官方智能机器人「长连接」API 模式无需公网回调即可收消息；当前版本未实现，只做本地关注信号",
                |ui| {
                    ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                },
            );
            row_separator(ui, p);
            row_info(
                ui,
                p,
                RowIcon::Info,
                "凭据与日志",
                "授权码只存 Windows 凭据管理器，不进 island.toml、不写日志；剪贴板与通知内容不上传",
                |ui| {
                    ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                },
            );
        });

        if let Some(e) = crate::hotkey::last_island_error() {
            ui.add_space(6.0);
            ui.label(RichText::new(e).size(12.0).color(p.danger));
        }
        if let Some(e) = &self.island_error {
            ui.add_space(4.0);
            ui.label(RichText::new(e).size(12.0).color(p.danger));
        }
        if iv != self.island {
            self.island = iv;
            self.island_dirty = Some(Instant::now());
            ui.ctx().request_repaint_after(Duration::from_millis(120));
        }
        ui.add_space(18.0);
    }

    // -- 桌面整理 -----------------------------------------------------------

    /// Writes the box document. Same atomic-replace trick as the taskbar file:
    /// the panel polls it, and a half-written TOML would reset every category.
    fn persist_box(&mut self) {
        // `dcx-box.exe` also writes this document. Re-read the keys this window
        // does not own — the view preferences *and* the path-keyed manifest —
        // so an edit made here cannot roll back what the panel just did.
        let disk = BoxConfig::load();
        self.bx.sort_by = disk.sort_by;
        self.bx.sort_desc = disk.sort_desc;
        self.bx.view_mode = disk.view_mode;
        self.bx.panel_width = disk.panel_width;
        self.bx.panel_height = disk.panel_height;
        self.bx.show_hidden = disk.show_hidden;
        self.bx.ignore = disk.ignore.clone();
        self.bx.single_click_open = disk.single_click_open;
        self.bx.remember_tab = disk.remember_tab;
        self.bx.last_tab = disk.last_tab.clone();
        self.bx.confirm_delete = disk.confirm_delete;
        if !self.bx_manifest_dirty {
            // The panel files documents into `overrides` on every drop; the copy
            // loaded in `new()` is minutes old by the time a slider is moved.
            self.bx.overrides = disk.overrides.clone();
            self.bx.hidden = disk.hidden.clone();
            self.bx.excluded = disk.excluded.clone();
        }
        match self.bx.save() {
            Ok(()) => {
                self.bx_saved = self.bx.clone();
                self.bx_dirty = None;
                self.bx_manifest_dirty = false;
            }
            Err(e) => {
                self.error = Some(format!("收纳盒配置保存失败：{e}"));
                self.bx_dirty = Some(Instant::now());
            }
        }
    }

    fn box_tick(&mut self, ctx: &egui::Context) {
        if self.bx != self.bx_saved {
            let since = *self.bx_dirty.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_millis(220) {
                self.persist_box();
            }
            ctx.request_repaint_after(Duration::from_millis(120));
        } else {
            self.bx_dirty = None;
        }
    }

    /// 「清除手动分类」: forget every drag choice; everything falls back to the
    /// extension rules. Reports how many choices were dropped — the only honest
    /// way to answer "did that do anything?".
    fn run_reset(&mut self) {
        let org = BoxOrganizer::new(self.bx.clone());
        let n = org.override_count();
        if n == 0 {
            self.bx_result = Some(("没有手动分类记录".to_string(), Instant::now()));
            return;
        }
        let _ = self.bx.overrides.clear();
        self.bx_manifest_dirty = true;
        self.bx_result = Some((format!("已清除 {n} 项手动分类"), Instant::now()));
    }

    /// Deleting a category must not leave choices pointing at it: those files
    /// would fall back to the rules on the next load anyway, so prune now and
    /// keep the manifest clean.
    fn prune_overrides_of(&mut self, key: &str) {
        self.bx.overrides.retain(|_, k| k != key);
        self.bx_manifest_dirty = true;
    }

    /// Point the collection at a new folder and move the existing files there.
    ///
    /// A failure leaves the document untouched — the box keeps reading the old
    /// root — and `migrate_library` puts back whatever it had already moved.
    fn set_library_dir(&mut self, picked: &str) {
        if self.migrate_busy {
            return;
        }
        let new = std::path::PathBuf::from(picked.trim());
        let old = self.bx.library_dir();
        if dcx_organizer::same_path(&old, &new) {
            self.bx_result = Some(("收纳库目录没有变化".to_string(), Instant::now()));
            return;
        }
        let desktop = dcx_organizer::desktop_dir();
        if dcx_organizer::is_same_or_under(&new, &desktop)
            || dcx_organizer::is_same_or_under(&desktop, &new)
        {
            alert(
                "无法使用该目录",
                "收纳库不能放在桌面内，也不能包含桌面目录。请选择其他文件夹。",
            );
            return;
        }
        if dcx_organizer::is_same_or_under(&new, &old) || dcx_organizer::is_same_or_under(&old, &new)
        {
            alert(
                "无法使用该目录",
                "新目录与原收纳库不能互相包含。请选择其他文件夹。",
            );
            return;
        }
        let count = std::fs::read_dir(&old)
            .map(|rd| rd.filter_map(|e| e.ok()).count())
            .unwrap_or(0);
        if count > 0 {
            let text = format!(
                "将收纳库从\n{}\n迁移到\n{}\n\n将移动 {} 个顶层项目，不会丢失文件；\
                 目标目录已有同名项目时会自动加序号。",
                old.display(),
                new.display(),
                count
            );
            if !confirm("迁移收纳库", &text) {
                return;
            }
        }
        // The walk can take minutes on a network drive or a large library, and it
        // used to run inside the frame: the window stopped painting and the move
        // could not be cancelled. Do it on a worker and apply the result on the
        // UI thread.
        self.migrate_busy = true;
        self.bx_result = Some(("正在迁移收纳库…".to_string(), Instant::now()));
        if let Ok(mut slot) = MIGRATE_RESULT.lock() {
            *slot = None;
        }
        let target = new.clone();
        std::thread::spawn(move || {
            let r = dcx_organizer::migrate_library(&old, &target);
            if let Ok(mut slot) = MIGRATE_RESULT.lock() {
                *slot = Some((target, r));
            }
        });
    }

    /// Apply a finished `migrate_library` run. Runs on the UI thread because it
    /// rewrites this window's copy of the manifest and writes the document.
    fn finish_migration(
        &mut self,
        new: std::path::PathBuf,
        result: std::io::Result<dcx_organizer::MigrateReport>,
    ) {
        self.migrate_busy = false;
        match result {
            Ok(report) => {
                // Manual categories and legacy manifests are keyed by absolute
                // path: follow the files, or every hand-sorted document would
                // fall back to the extension rules after the move.
                let pairs = &report.pairs;
                let overrides = std::mem::take(&mut self.bx.overrides);
                self.bx.overrides = overrides
                    .into_iter()
                    .map(|(k, v)| (dcx_organizer::remap_moved_key(&k, pairs), v))
                    .collect();
                let hidden = std::mem::take(&mut self.bx.hidden);
                self.bx.hidden = hidden
                    .into_iter()
                    .map(|k| dcx_organizer::remap_moved_key(&k, pairs))
                    .collect();
                let excluded = std::mem::take(&mut self.bx.excluded);
                self.bx.excluded = excluded
                    .into_iter()
                    .map(|k| dcx_organizer::remap_moved_key(&k, pairs))
                    .collect();
                let default = BoxConfig::default().library_dir();
                self.bx.library_root = if dcx_organizer::same_path(&new, &default) {
                    None
                } else {
                    Some(new.display().to_string())
                };
                // Commit now, not on the 220 ms debounce: the files already
                // moved, so the box must switch roots before it scans again.
                self.bx_manifest_dirty = true;
                self.persist_box();
                let mut msg = if report.moved == 0 {
                    "收纳库目录已更新".to_string()
                } else {
                    format!("已迁移 {} 项到新收纳库", report.moved)
                };
                if report.renamed > 0 {
                    msg.push_str(&format!("，{} 项重名已自动改名", report.renamed));
                }
                if report.leftovers > 0 {
                    msg.push_str(&format!(
                        "；{} 项仍被占用未能从旧目录删除，请关闭相关程序后手动清理",
                        report.leftovers
                    ));
                }
                self.bx_result = Some((msg, Instant::now()));
            }
            Err(e) => alert("收纳库迁移失败", &format!("收纳库未改动：{e}")),
        }
    }

    fn box_page(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let mut enabled = self.bx.enabled;
        let mut auto_classify = self.bx.auto_classify;
        let mut hover_open = self.bx.open_mode == OpenMode::Hover;
        let mut hotkey_on = self.bx.hotkey_enabled;
        let mut hotkey = self.bx.hotkey.clone();

        caption(ui, p, "常规");
        card(ui, p, |ui| {
            row_tight(
                ui,
                p,
                RowIcon::BoxGlyph,
                "启用收纳盒",
                "本页唯一主开关：同时更新应用启停与引擎槽位；按钮固定于任务栏图标右侧",
                |ui| {
                    switch(ui, p, &mut enabled);
                },
            );
            row_separator(ui, p);
            self.app_status_row(ui, p, "box", RowIcon::BoxGlyph);
            row_separator(ui, p);
            row_tight(
                ui,
                p,
                RowIcon::Sort,
                "按扩展名自动分类",
                "没有手动调整过的文件，按下面的扩展名规则自动归入分类",
                |ui| {
                    switch(ui, p, &mut auto_classify);
                },
            );
            row_separator(ui, p);
            row_tight(
                ui,
                p,
                RowIcon::BoxGlyph,
                "悬停即展开",
                "关闭后只有点击按钮才展开，指针扫过任务栏不会误触",
                |ui| {
                    switch(ui, p, &mut hover_open);
                },
            );
            row_separator(ui, p);
            row_tight(
                ui,
                p,
                RowIcon::Power,
                "启用全局快捷键",
                "由 dcx-host 常驻监听；面板打开时再按一次收起，关闭时唤出",
                |ui| {
                    switch(ui, p, &mut hotkey_on);
                },
            );
            row_separator(ui, p);
            row(
                ui,
                p,
                RowIcon::Info,
                "全局快捷键",
                "示例 Alt+d、Ctrl+Alt+B；改键约 1 秒后生效，被占用时下方会提示",
                |ui| {
                    ui.add(egui::TextEdit::singleline(&mut hotkey).desired_width(130.0));
                },
            );
        });
        let enable_changed = enabled != self.bx.enabled;
        if enable_changed
            || auto_classify != self.bx.auto_classify
            || hover_open != (self.bx.open_mode == OpenMode::Hover)
            || hotkey_on != self.bx.hotkey_enabled
            || hotkey != self.bx.hotkey
        {
            self.bx.enabled = enabled;
            self.bx.auto_classify = auto_classify;
            self.bx.open_mode = if hover_open {
                OpenMode::Hover
            } else {
                OpenMode::Click
            };
            self.bx.hotkey_enabled = hotkey_on;
            self.bx.hotkey = hotkey;
            if !enabled && enable_changed {
                restore_desktop_icons_if_owned();
            }
            // The box is now a separate process: keep apps.toml in step so the
            // master switch really starts / stops it instead of only hiding
            // a window that is still running.
            if enable_changed {
                if let Err(e) = crate::apps::set_enabled("box", enabled) {
                    self.error = Some(e);
                }
            }
        }
        if let Some(e) = crate::hotkey::last_box_error() {
            ui.add_space(6.0);
            ui.label(RichText::new(e).size(12.0).color(p.danger));
        }

        ui.add_space(18.0);
        // One rule, no knobs: the button sits in the band the engine reserves
        // right after the last program icon. The three old modes all resolved to
        // the same measured number and only made the button look unstable, so
        // this card now reports the position instead of configuring it.
        caption(ui, p, "任务栏按钮");
        let live_x = current_tile_x();
        card(ui, p, |ui| {
            row_tight(
                ui,
                p,
                RowIcon::BoxGlyph,
                "位置",
                "自动固定在最后一个程序图标之后；图标增减时整体平移，不会压到其它按钮上",
                |ui| {
                    ui.label(RichText::new("自动").size(12.0).color(p.tertiary));
                },
            );
            row_separator(ui, p);
            let sub = match live_x {
                Some(x) => format!("当前 x = {x} 物理像素（由注入引擎量出的图标行末尾）"),
                None => "尚未读到按钮位置，请先启动收纳盒".to_string(),
            };
            row_info(ui, p, RowIcon::Info, "当前坐标", &sub, |ui| {
                ui.label(RichText::new("").size(12.0).color(p.secondary));
            });
            row_separator(ui, p);
            row_tight(
                ui,
                p,
                RowIcon::Sort,
                "面板视图选项",
                "排序、列表/网格、显示隐藏项目、单击打开等，在面板右上角功能区调整",
                |ui| {
                    ui.label(RichText::new("面板内设置").size(12.0).color(p.tertiary));
                },
            );
        });

        ui.add_space(18.0);
        // Collection model: desk files stay native until collected; collected
        // files live in the library and are no longer on the desktop.
        caption(ui, p, "收纳库与桌面");
        let library = self.bx.library_dir();
        let library_default = BoxConfig::default().library_dir();
        let desktop = dcx_organizer::desktop_dir();
        let mut action: Option<u8> = None;
        let mut want_library = false;
        card(ui, p, |ui| {
            let place = if dcx_organizer::same_path(&library, &library_default) {
                "出厂默认位置"
            } else {
                "自定义位置"
            };
            let subtitle = format!("{place} · {}", library.display());
            row_reserve(
                ui,
                p,
                RowIcon::BoxGlyph,
                "收纳库目录",
                &subtitle,
                150.0,
                |ui| {
                    if standard_button(ui, p, "打开", true).clicked() {
                        action = Some(3);
                    }
                    if standard_button(ui, p, "更改…", !self.migrate_busy).clicked() {
                        want_library = true;
                    }
                },
            );
            row_separator(ui, p);
            let path = desktop.display().to_string();
            row(ui, p, RowIcon::Folder, "桌面目录", &path, |ui| {
                if standard_button(ui, p, "打开", true).clicked() {
                    action = Some(2);
                }
            });
            row_separator(ui, p);
            let overrides = self.bx.overrides.len();
            row(
                ui,
                p,
                RowIcon::BoxGlyph,
                "手动分类记录",
                "只作用于收纳库内的文件；拖出盒子会把文件移回桌面",
                |ui| {
                    if standard_button(ui, p, &format!("清除 {overrides} 项"), overrides > 0)
                        .clicked()
                    {
                        action = Some(0);
                    }
                },
            );
        });
        if want_library {
            if let Some(dir) = pick_folder("选择收纳库目录") {
                self.set_library_dir(&dir);
            }
        }
        match action {
            Some(0) => self.run_reset(),
            Some(2) => {
                let _ = std::process::Command::new("explorer").arg(&desktop).spawn();
            }
            Some(3) => {
                let _ = std::process::Command::new("explorer")
                    .arg(self.bx.library_dir())
                    .spawn();
            }
            _ => {}
        }
        if let Some((text, at)) = self.bx_result.clone() {
            if at.elapsed() < Duration::from_secs(6) {
                ui.add_space(8.0);
                ui.label(RichText::new(text).size(12.0).color(p.accent));
            } else {
                self.bx_result = None;
            }
        }

        ui.add_space(18.0);
        caption(ui, p, "分类规则");
        // `counts()` walks the whole library and stats every entry. It used to
        // run once per frame; 5 s is fresh enough for a rule card and keeps the
        // page responsive on a large collection.
        let stale = self
            .bx_counts_at
            .map(|at| at.elapsed() >= Duration::from_secs(5))
            .unwrap_or(true);
        if stale {
            self.bx_counts = BoxOrganizer::new(self.bx.clone()).counts();
            self.bx_counts_at = Some(Instant::now());
        }
        let counts = self.bx_counts.clone();
        let mut rows = std::mem::take(&mut self.bx_rows);
        let mut changed = false;
        let mut remove: Option<usize> = None;
        let mut add = false;
        let row_count = rows.len();
        card(ui, p, |ui| {
            for (i, r) in rows.iter_mut().enumerate() {
                if i > 0 {
                    rule_separator(ui, p);
                }
                let count = counts
                    .iter()
                    .find(|(k, _, _)| *k == r.key)
                    .map(|(_, n, _)| *n)
                    .unwrap_or(0);
                let deletable = r.key != dcx_organizer::CATCH_ALL
                    && r.key != dcx_organizer::FOLDER
                    && row_count > 2;
                let (c, del) = rule_row(ui, p, r, count, deletable);
                changed |= c;
                if del {
                    remove = Some(i);
                }
            }
            rule_separator(ui, p);
            egui::Frame::none()
                .inner_margin(Margin::symmetric(16.0, 10.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if icon_text_button(ui, p, glyph::PLUS, "新增分类", true).clicked() {
                        add = true;
                    }
                });
        });

        if let Some(i) = remove {
            let key = rows[i].key.clone();
            rows.remove(i);
            self.prune_overrides_of(&key);
            changed = true;
        }
        if add {
            let mut n = rows.len();
            let key = loop {
                let candidate = format!("cat{n}");
                if !rows.iter().any(|r| r.key == candidate) {
                    break candidate;
                }
                n += 1;
            };
            rows.push(RuleRow {
                key,
                label: "新分类".to_string(),
                exts: String::new(),
            });
            changed = true;
        }
        if changed {
            // Keep the catch-all last: it is where everything unmatched lands.
            if let Some(pos) = rows
                .iter()
                .position(|r| r.key == dcx_organizer::CATCH_ALL)
            {
                let row = rows.remove(pos);
                rows.push(row);
            }
            self.bx = RuleRow::build(&rows, &self.bx);
        }
        self.bx_rows = rows;
    }

    fn actions(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            if standard_button(ui, p, "恢复默认", !self.s.is_default()).clicked() {
                self.reset();
            }
            ui.add_space(2.0);
            if standard_button(ui, p, "重新应用", !self.reapply_busy).clicked() {
                // Rewrites the file, which bumps mtime and forces a reload even
                // when the content is unchanged (e.g. after an Explorer restart).
                self.persist();
                // `inject_now` enumerates processes and waits for
                // `wh_injector.exe`; if the injector hangs in
                // `CreateRemoteThread` this used to freeze the window forever.
                self.reapply_busy = true;
                if let Ok(mut slot) = REAPPLY_RESULT.lock() {
                    *slot = None;
                }
                std::thread::spawn(|| {
                    let r = crate::injector::inject_now();
                    if let Ok(mut slot) = REAPPLY_RESULT.lock() {
                        *slot = Some(r);
                    }
                });
                // The footer reads a 3 s sample; nudge it before that so the
                // "已应用" state lands without the user having to jiggle the mouse.
                ui.ctx().request_repaint_after(Duration::from_millis(1500));
            }
            ui.add_space(2.0);
            if standard_button(ui, p, "打开配置目录", true).clicked() {
                reveal_settings();
            }
        });
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }

    /// Only the standalone `--settings` loop exits; the viewport form flushes in
    /// `draw_in_viewport` / on Esc instead.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.flush();
    }
}

impl SettingsApp {
    /// Write an edit that is still inside the debounce window.
    ///
    /// Called when the window goes away — by `on_exit` for the standalone loop,
    /// and by the close handlers of the viewport form, which never exits the
    /// process (the panel keeps running).
    pub fn flush(&mut self) {
        if self.s != self.saved {
            self.persist();
        }
        // The search and box documents use their own debounce timers; closing
        // the window inside that window used to throw the last edit away.
        if self.search != self.search_saved {
            self.persist_search();
        }
        if self.bx != self.bx_saved {
            self.persist_box();
        }
        if self.island != self.island_saved {
            self.persist_island();
        }
    }

    /// The whole window, independent of the hosting event loop: `run_native`
    /// (dev entry point) and the panel's deferred viewport both end up here.
    pub fn ui(&mut self, ctx: &egui::Context) {
        // The shell owns these bits — the Windows Settings app can flip them
        // while we are open — but re-reading them every frame meant one
        // synchronous `SHAppBarMessage` plus three registry opens per frame,
        // which made this window "not responding" whenever Explorer was busy.
        // Twice a second is enough to notice a change made elsewhere.
        if self.shell_state_at.elapsed() >= Duration::from_secs(2) {
            self.shell_state_at = Instant::now();
            self.auto_hide = crate::auto_hide::is_enabled();
            self.autostart = crate::autostart::is_enabled();
        }
        // Pick up the background 「重新应用」 result.
        if let Ok(mut slot) = REAPPLY_RESULT.lock() {
            if let Some(r) = slot.take() {
                self.reapply_busy = false;
                if let Err(e) = r {
                    self.error = Some(format!("引擎注入失败：{e}"));
                }
            }
        }
        if self.reapply_busy {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        // Pick up a finished library migration.
        if let Ok(mut slot) = MIGRATE_RESULT.lock() {
            if let Some((new, result)) = slot.take() {
                self.finish_migration(new, result);
            }
        }
        if self.migrate_busy {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        // A failed restore happens on its own thread; surface it in the footer
        // instead of leaving the button looking like it did nothing.
        if let Some(e) = crate::shutdown::take_error() {
            self.error = Some(e);
        }
        // Keep the countdown honest while armed, and keep the "restarting" line
        // live while the shell comes back.
        if self.confirm_quit_at.is_some() || crate::shutdown::is_busy() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if FOCUS_REQUEST.swap(false, Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            // The window is its own process: flush the debounced edit before it
            // goes away, then let `on_exit` do the rest.
            self.flush();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        let p = match self.s.ui_theme {
            UiTheme::Light => Palette::light(),
            UiTheme::Dark => Palette::dark(),
        };
        apply_style(ctx, &p);

        // The native caption is part of the same surface: paint it in the
        // backdrop's top colour, and repaint when the theme flips.
        if self.caption_tint != Some(p.dark) {
            let (r, g, b) = (p.bg_top.r(), p.bg_top.g(), p.bg_top.b());
            let (tr, tg, tb) = (p.text.r(), p.text.g(), p.text.b());
            crate::boot::tint_caption((r, g, b), (tr, tg, tb), p.dark);
            self.caption_tint = Some(p.dark);
        }

        self.box_tick(ctx);
        self.search_tick(ctx);
        self.island_tick(ctx);

        // Debounced live apply: write at most once per ~220 ms of idle editing.
        if self.s != self.saved {
            let since = *self.dirty_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_millis(220) && Instant::now() >= self.retry_after {
                self.persist();
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            self.dirty_since = None;
        }

        // One surface for the whole window (see `paint_backdrop`); the panels
        // below are transparent frames, so nothing draws a vertical seam.
        paint_backdrop(ctx, &p);

        egui::SidePanel::left("dcx_nav")
            .exact_width(SIDEBAR_W)
            .resizable(false)
            // egui draws a 1 px vertical rule on the panel edge by default.
            // That rule (plus the per-panel fills it used to separate) is what
            // made the window read as two panes; the columns now share one
            // surface, so there is nothing left for it to divide.
            .show_separator_line(false)
            .frame(egui::Frame::none().inner_margin(Margin {
                left: 16.0,
                right: 16.0,
                top: 22.0,
                bottom: 16.0,
            }))
            .show(ctx, |ui| {
                ui.set_width(ui.available_width());
                self.sidebar(ui, &p);
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                // Width is measured **once** here. Asking for
                // `ui.available_width()` again inside the scroll area made the
                // content shrink by a scrollbar's width every time the bar
                // appeared, which re-wrapped every row and made scrolling look
                // jumpy.
                let full_w = ui.available_width();
                let content_w = (full_w - PAGE_GUTTER * 2.0).max(240.0);
                // Title stays put; only the rows scroll (same as Win11).
                egui::Frame::none()
                    .inner_margin(Margin {
                        left: PAGE_GUTTER,
                        right: PAGE_GUTTER,
                        top: 22.0,
                        bottom: 0.0,
                    })
                    .show(ui, |ui| {
                        ui.set_width(content_w);
                        match self.page {
                            Page::Look => {
                                self.page_head(
                                    ui,
                                    &p,
                                    "任务栏样式",
                                    "任务栏形状、图标缩放、高度、间距与自动隐藏",
                                );
                            }
                            Page::Box => {
                                self.page_head(
                                    ui,
                                    &p,
                                    "桌面整理",
                                    "任务栏收纳盒：自动分类、桌面位置与扩展名规则",
                                );
                            }
                            Page::Search => {
                                self.page_head(
                                    ui,
                                    &p,
                                    "文件检索",
                                    "文件名与内容检索：快捷键、检索范围与索引策略",
                                );
                            }
                            Page::Island => {
                                self.page_head(
                                    ui,
                                    &p,
                                    "桌面信息岛",
                                    "顶部悬浮胶囊：时间、性能、任务、剪贴板与消息聚合",
                                );
                            }
                            Page::About => {
                                self.page_head(ui, &p, "关于", "DCX 桌面增强 · 任务栏主题");
                            }
                        }
                    });
                ui.add_space(16.0);

                let salt = match self.page {
                    Page::Look => 0u8,
                    Page::Search => 3,
                    Page::Island => 4,
                    Page::Box => 2,
                    Page::About => 1,
                };
                // The scroll area spans the full panel width so its (floating)
                // bar lands on the window edge; padding moves into the content.
                let inner_h = ui.available_height();
                ui.scope(|ui| {
                    let v = ui.visuals_mut();
                    v.widgets.inactive.bg_fill = p.scroll_thumb;
                    v.widgets.hovered.bg_fill = p.scroll_thumb_hover;
                    v.widgets.active.bg_fill = p.scroll_thumb_hover;
                    egui::ScrollArea::vertical()
                        .id_salt(("dcx_page", salt))
                        .auto_shrink([false, false])
                        .max_height(inner_h)
                        .scroll_bar_visibility(
                            egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                        )
                        .show(ui, |ui| {
                            // The scroll area spans the whole panel so its
                            // floating bar rides the window edge; all padding
                            // lives in the frame around the cards.
                            ui.set_width(full_w);
                            egui::Frame::none()
                                .inner_margin(Margin {
                                    left: PAGE_GUTTER,
                                    right: PAGE_GUTTER + 10.0,
                                    top: 0.0,
                                    bottom: 10.0,
                                })
                                .show(ui, |ui| {
                                    ui.set_width((content_w - 10.0).max(200.0));
                                    match self.page {
                                        Page::Look => {
                                            caption(ui, &p, "外观");
                                            self.appearance_card(ui, &p);
                                            ui.add_space(18.0);
                                            caption(ui, &p, "尺寸与间距");
                                            self.metrics_card(ui, &p);
                                            ui.add_space(18.0);
                                            caption(ui, &p, "行为");
                                            self.behavior_card(ui, &p);
                                        }
                                        Page::Box => self.box_page(ui, &p),
                                        Page::Search => self.search_cards(ui, &p),
                                        Page::Island => self.island_cards(ui, &p),
                                        Page::About => {
                                            caption(ui, &p, "系统信息");
                                            self.about_card(ui, &p);
                                            ui.add_space(18.0);
                                            self.platform_card(ui, &p);
                                            ui.add_space(18.0);
                                            caption(ui, &p, "退出");
                                            self.shutdown_card(ui, &p);
                                        }
                                    }
                                    ui.add_space(18.0);
                                    caption(ui, &p, "维护");
                                    self.actions(ui, &p);
                                    ui.add_space(8.0);
                                });
                        });
                });
            });
    }
}

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

fn apply_compact(s: &mut Settings) {
    s.taskbar_shape = TaskbarShape::Capsule;
    s.icon_scale = 85;
    s.icon_spacing = Spacing { between: -4 };
    s.taskbar_height = None;
}

fn apply_standard(s: &mut Settings) {
    s.taskbar_shape = TaskbarShape::Capsule;
    s.icon_scale = 100;
    s.icon_spacing = Spacing { between: 0 };
    s.taskbar_height = None;
}

fn apply_roomy(s: &mut Settings) {
    s.taskbar_shape = TaskbarShape::Capsule;
    s.icon_scale = 125;
    s.icon_spacing = Spacing { between: 6 };
    s.taskbar_height = None;
}

fn preset_matches(s: &Settings, apply: fn(&mut Settings)) -> bool {
    let mut probe = s.clone();
    let shape = probe.taskbar_shape;
    let max_width = probe.max_width;
    apply(&mut probe);
    probe.taskbar_shape = shape;
    probe.max_width = max_width;
    probe == *s
}

fn is_compact(s: &Settings) -> bool {
    preset_matches(s, apply_compact)
}

fn is_standard(s: &Settings) -> bool {
    preset_matches(s, apply_standard)
}

fn is_roomy(s: &Settings) -> bool {
    preset_matches(s, apply_roomy)
}

fn summary(s: &Settings) -> String {
    let shape = match s.taskbar_shape {
        TaskbarShape::Capsule => "胶囊",
        TaskbarShape::Card => "卡片",
    };
    let size = format!("图标 {}%", s.effective_icon_scale());
    let height = format!("高 {} px", s.effective_height());
    format!(
        "{shape} · {size} · {height} · 间距 {} px · 宽 ≤ {} px",
        s.icon_spacing.between,
        s.effective_max_width()
    )
}


/// Physical x of the tile, published once a second by `dcx-box.exe` in
/// `%LOCALAPPDATA%\dcx\box_ui.txt`. Used by 「锁定当前位置」 so the settings
/// centre needs no new IPC channel with the app process.
fn current_tile_x() -> Option<i32> {
    let mut p = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push("dcx");
    p.push("box_ui.txt");
    let text = std::fs::read_to_string(p).ok()?;
    text.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        if k.trim() == "tile_x" {
            v.trim().parse::<f32>().ok().map(|x| x.round() as i32)
        } else {
            None
        }
    })
}

fn reveal_settings() {
    if let Some(dir) = Settings::path().parent().map(|p| p.to_path_buf()) {
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
}

/// Hand a URL to the user's default browser.
///
/// `ShellExecuteW("open", …)` rather than `Command::new("explorer", url)`: the
/// shell resolves the protocol handler itself, so an `http`/`https` URL never
/// passes through a process argv where it could be read as a file path.
fn open_url(url: &str) {
    let op = dcx_win32::wide("open");
    let target = dcx_win32::wide(url);
    const SW_SHOWNORMAL: i32 = 1;
    let code = unsafe {
        dcx_win32::ShellExecuteW(
            0,
            op.as_ptr(),
            target.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a value > 32 on success; otherwise it is a legacy
    // SE_ERR_* code. Nothing to recover from, but a silent no-op would look
    // like a broken button, so it goes to the log.
    if code <= 32 {
        eprintln!("[dcx-host] ShellExecuteW({url}) returned {code}");
    }
}

#[repr(C)]
struct BrowseInfoW {
    hwnd_owner: isize,
    pidl_root: *const core::ffi::c_void,
    display_name: *mut u16,
    title: *const u16,
    flags: u32,
    callback: *const core::ffi::c_void,
    l_param: isize,
    image: i32,
}

#[link(name = "shell32")]
extern "system" {
    fn SHBrowseForFolderW(info: *mut BrowseInfoW) -> *mut core::ffi::c_void;
    fn SHGetPathFromIDListW(pidl: *const core::ffi::c_void, path: *mut u16) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut core::ffi::c_void, coinit: u32) -> i32;
    fn CoTaskMemFree(p: *mut core::ffi::c_void);
}

/// Native folder picker for paths this window needs (search scope, library).
/// Only the path is needed, so the compact shell dialog is enough; a failure
/// just leaves the setting unchanged.
fn pick_folder(title: &str) -> Option<String> {
    const BIF_RETURNONLYFSDIRS: u32 = 0x0001;
    const BIF_EDITBOX: u32 = 0x0010;
    const BIF_NEWDIALOGSTYLE: u32 = 0x0040;
    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    // `BIF_NEWDIALOGSTYLE` needs an apartment; a second call returns S_FALSE
    // and is harmless, so no matching uninitialise is needed.
    unsafe {
        CoInitializeEx(core::ptr::null_mut(), COINIT_APARTMENTTHREADED);
    }
    let title = dcx_win32::wide(title);
    let mut display = [0u16; 260];
    let mut info = BrowseInfoW {
        hwnd_owner: settings_hwnd(),
        pidl_root: core::ptr::null(),
        display_name: display.as_mut_ptr(),
        title: title.as_ptr(),
        flags: BIF_RETURNONLYFSDIRS | BIF_EDITBOX | BIF_NEWDIALOGSTYLE,
        callback: core::ptr::null(),
        l_param: 0,
        image: 0,
    };
    let pidl = unsafe { SHBrowseForFolderW(&mut info) };
    if pidl.is_null() {
        return None;
    }
    let mut buf = [0u16; 32_768];
    let ok = unsafe { SHGetPathFromIDListW(pidl, buf.as_mut_ptr()) };
    unsafe { CoTaskMemFree(pidl) };
    if ok == 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(0);
    let path = String::from_utf16_lossy(&buf[..end]);
    (!path.is_empty()).then_some(path)
}

/// This window's HWND, so native dialogs stay owned (modal) instead of being
/// buried behind other topmost windows. 0 is fine before the window exists.
fn settings_hwnd() -> isize {
    let title = dcx_win32::wide("DCX 设置");
    unsafe { dcx_win32::FindWindowW(core::ptr::null(), title.as_ptr()) }
}

#[link(name = "user32")]
extern "system" {
    fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, utype: u32) -> i32;
}

/// Native yes/no question. A library change moves every collected file, so it
/// gets an explicit answer rather than another inline button.
fn confirm(title: &str, text: &str) -> bool {
    const MB_YESNO: u32 = 0x0000_0004;
    const MB_ICONQUESTION: u32 = 0x0000_0020;
    const IDYES: i32 = 6;
    let text = dcx_win32::wide(text);
    let title = dcx_win32::wide(title);
    unsafe {
        MessageBoxW(
            settings_hwnd(),
            text.as_ptr(),
            title.as_ptr(),
            MB_YESNO | MB_ICONQUESTION,
        ) == IDYES
    }
}

fn alert(title: &str, text: &str) {
    const MB_OK: u32 = 0;
    const MB_ICONWARNING: u32 = 0x0000_0030;
    let text = dcx_win32::wide(text);
    let title = dcx_win32::wide(title);
    unsafe {
        MessageBoxW(
            settings_hwnd(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONWARNING,
        );
    }
}

// ---------------------------------------------------------------------------
// Win11 building blocks
// ---------------------------------------------------------------------------

/// Small section caption above a card, the way Win11 labels card groups: the
/// caption is the same weight class as a subheading, not a muted footnote.
fn caption(ui: &mut egui::Ui, p: &Palette, text: &str) {
    ui.label(RichText::new(text).font(bold(14.5)).color(p.text));
    ui.add_space(8.0);
}

fn card<R>(ui: &mut egui::Ui, p: &Palette, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::none()
        .fill(p.card)
        .stroke(Stroke::new(1.0_f32, p.border))
        .rounding(Rounding::same(CARD_RADIUS))
        // A 1 px lift keeps the white cards from dissolving into the light
        // backdrop without the hard outline an old-style border gives.
        .shadow(egui::epaint::Shadow {
            offset: egui::vec2(0.0, 1.0),
            blur: 2.0,
            spread: 0.0,
            color: p.card_shadow,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// One Settings row: glyph, title + secondary line, right-aligned control.
fn row<R>(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: RowIcon,
    title: &str,
    subtitle: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    row_reserve(ui, p, icon, title, subtitle, ROW_ACTION_W, add)
}

/// Row whose control is a 40 px switch. Reserving a button's width here used to
/// cut long subtitles short with an ellipsis for no reason: the switch never
/// needs more than half of it.
fn row_tight<R>(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: RowIcon,
    title: &str,
    subtitle: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    row_reserve(ui, p, icon, title, subtitle, ROW_SWITCH_W, add)
}

/// Row whose right side only carries a dot or a short state word.
fn row_info<R>(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: RowIcon,
    title: &str,
    subtitle: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    row_reserve(ui, p, icon, title, subtitle, ROW_INFO_W, add)
}

/// `best` is the width the row must keep free for its action; the text column
/// gets everything else, so a short control lets a long subtitle breathe.
fn row_reserve<R>(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: RowIcon,
    title: &str,
    subtitle: &str,
    best: f32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::none()
        .inner_margin(Margin::symmetric(16.0, 13.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                row_icon(ui, p, icon);
                ui.add_space(12.0);
                let text_w = (ui.available_width() - best).max(120.0);
                ui.vertical(|ui| {
                    ui.set_max_width(text_w);
                    ui.add(
                        egui::Label::new(RichText::new(title).size(14.5).color(p.text)).truncate(),
                    );
                    if !subtitle.is_empty() {
                        ui.add_space(2.0);
                        ui.add(
                            egui::Label::new(RichText::new(subtitle).size(12.5).color(p.secondary))
                                .truncate(),
                        );
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| add(ui))
                    .inner
            })
            .inner
        })
        .inner
}

/// Slider row: title + value on one line, track spanning the text column below.
fn slider_row<T: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: RowIcon,
    title: &str,
    subtitle: &str,
    value: &mut T,
    lo: T,
    hi: T,
    unit: &str,
    enabled: bool,
) -> bool {
    let mut changed = false;
    egui::Frame::none()
        .inner_margin(Margin::symmetric(16.0, 13.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                row_icon(ui, p, icon);
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).size(14.5).color(if enabled {
                        p.text
                    } else {
                        p.tertiary
                    }));
                    if !subtitle.is_empty() {
                        ui.add_space(2.0);
                        ui.label(RichText::new(subtitle).size(12.5).color(p.secondary));
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // Transparent DragValue doubles as a Win11-style value label
                    // that still supports typing/dragging for precise input.
                    ui.scope(|ui| {
                        let w = &mut ui.style_mut().visuals.widgets;
                        w.inactive.bg_fill = Color32::TRANSPARENT;
                        w.inactive.bg_stroke = Stroke::NONE;
                        w.hovered.bg_fill = p.hover;
                        w.hovered.bg_stroke = Stroke::new(1.0_f32, p.border);
                        let dv = egui::DragValue::new(value)
                            .range(lo..=hi)
                            .suffix(unit)
                            .speed(0.4);
                        if ui.add_enabled(enabled, dv).changed() {
                            changed = true;
                        }
                    });
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                // Align the track with the text column (glyph 20 + gap 12).
                ui.add_space(32.0);
                let w = ui.available_width();
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(w, 26.0), Sense::click_and_drag());
                if slider(ui, p, rect, &resp, value, lo, hi, enabled) {
                    changed = true;
                }
            });
        });
    changed
}

/// Separator inside the rules card. There is no 20 px glyph column there, so
/// the line starts under the category name instead of 48 px in.
fn rule_separator(ui: &mut egui::Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
    let rect = Rect::from_min_max(
        egui::pos2(rect.left() + 34.0, rect.top()),
        egui::pos2(rect.right(), rect.bottom()),
    );
    ui.painter().rect_filled(rect, Rounding::ZERO, p.separator);
}

/// A bordered button carrying a glyph and a label side by side. egui's `Button`
/// takes a single `RichText`, and the label needs the CJK font while the icon
/// needs the icon font, so the two runs are placed by hand.
fn icon_text_button(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: char,
    text: &str,
    enabled: bool,
) -> Response {
    let text_w = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font(14.0), p.text)
        .size()
        .x;
    let w = 12.0 + 13.0 + 7.0 + text_w + 14.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 32.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let (fill, border) = if p.dark {
        (
            Color32::from_rgb(0x2D, 0x2D, 0x2D),
            Color32::from_rgb(0x3A, 0x3A, 0x3A),
        )
    } else {
        (
            Color32::from_rgb(0xFB, 0xFB, 0xFB),
            Color32::from_rgb(0xD4, 0xD4, 0xD4),
        )
    };
    let fill = if resp.hovered() { p.hover } else { fill };
    let fg = if enabled { p.text } else { p.tertiary };
    ui.painter()
        .rect_filled(rect, Rounding::same(CARD_RADIUS), fill);
    ui.painter().rect_stroke(
        rect,
        Rounding::same(CARD_RADIUS),
        Stroke::new(1.0_f32, border),
    );
    let cy = rect.center().y;
    if icons_available() {
        ui.painter().text(
            egui::pos2(rect.left() + 19.0, cy),
            Align2::CENTER_CENTER,
            icon,
            icon_font(11.0),
            fg,
        );
        ui.painter().text(
            egui::pos2(rect.left() + 32.0, cy),
            Align2::LEFT_CENTER,
            text,
            font(14.0),
            fg,
        );
    } else {
        ui.painter().text(
            egui::pos2(rect.left() + 13.0, cy),
            Align2::LEFT_CENTER,
            "+",
            font(14.0),
            fg,
        );
        ui.painter().text(
            egui::pos2(rect.left() + 28.0, cy),
            Align2::LEFT_CENTER,
            text,
            font(14.0),
            fg,
        );
    }
    resp
}

fn row_separator(ui: &mut egui::Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
    let rect = Rect::from_min_max(
        egui::pos2(rect.left() + 48.0, rect.top()),
        egui::pos2(rect.right(), rect.bottom()),
    );
    ui.painter().rect_filled(rect, Rounding::ZERO, p.separator);
}

#[derive(Clone, Copy)]
enum RowIcon {
    Shape,
    /// Taskbar tile / file box.
    BoxGlyph,
    /// Magnifier, for the file-search app.
    Search,
    /// Tidy up.
    Sort,
    /// Desktop.
    Desktop,
    /// Folder.
    Folder,
    Hover,
    Size,
    Preset,
    Spacing,
    Tall,
    Width,
    Hide,
    Power,
    Doc,
    Info,
    Exit,
    /// Box with an arrow leaving it — an external web link.
    Link,
}

/// 20 px line glyphs, drawn with the same thin stroke Win11 uses.
fn row_icon(ui: &mut egui::Ui, p: &Palette, kind: RowIcon) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), Sense::hover());
    let c = rect.center();
    let st = Stroke::new(1.2_f32, p.secondary);
    let painter = ui.painter();
    match kind {
        RowIcon::Shape => {
            let r = Rect::from_center_size(c, egui::vec2(16.0, 9.0));
            painter.rect_stroke(r, Rounding::same(4.5), st);
        }
        RowIcon::Hover => {
            // A bar with one tile lifted out of it.
            let bar = Rect::from_min_max(
                egui::pos2(c.x - 8.0, c.y + 3.0),
                egui::pos2(c.x + 8.0, c.y + 7.0),
            );
            painter.rect_stroke(bar, Rounding::same(2.0), st);
            let tile = Rect::from_min_max(
                egui::pos2(c.x - 2.5, c.y - 8.0),
                egui::pos2(c.x + 2.5, c.y + 1.0),
            );
            painter.rect_stroke(tile, Rounding::same(1.5), st);
        }
        RowIcon::Size => {
            for (i, s) in [5.0_f32, 7.5, 10.0].iter().enumerate() {
                let x = c.x - 6.0 + i as f32 * 5.5;
                let r = Rect::from_min_max(
                    egui::pos2(x, c.y + 5.0 - s),
                    egui::pos2(x + 4.0, c.y + 5.0),
                );
                painter.rect_stroke(r, Rounding::same(1.0), st);
            }
        }
        RowIcon::Preset => {
            for (i, w) in [11.0_f32, 8.0, 5.0].iter().enumerate() {
                let y = c.y - 4.5 + i as f32 * 4.5;
                painter.hline(c.x - 6.5..=c.x - 6.5 + w, y, st);
            }
        }
        RowIcon::Spacing => {
            painter.vline(c.x - 5.0, c.y - 6.0..=c.y + 6.0, st);
            painter.vline(c.x + 5.0, c.y - 6.0..=c.y + 6.0, st);
            painter.hline(c.x - 2.0..=c.x + 2.0, c.y, st);
        }
        RowIcon::Tall => {
            painter.vline(c.x, c.y - 7.0..=c.y + 5.0, st);
            painter.hline(c.x - 7.0..=c.x + 7.0, c.y + 6.0, st);
            painter.line_segment(
                [egui::pos2(c.x - 3.0, c.y - 3.0), egui::pos2(c.x, c.y - 7.0)],
                st,
            );
            painter.line_segment(
                [egui::pos2(c.x, c.y - 7.0), egui::pos2(c.x + 3.0, c.y - 3.0)],
                st,
            );
            painter.line_segment(
                [egui::pos2(c.x - 3.0, c.y + 1.0), egui::pos2(c.x, c.y + 5.0)],
                st,
            );
            painter.line_segment(
                [egui::pos2(c.x, c.y + 5.0), egui::pos2(c.x + 3.0, c.y + 1.0)],
                st,
            );
        }
        RowIcon::Hide => {
            // bar sinking below the screen edge
            painter.hline(c.x - 7.0..=c.x + 7.0, c.y + 6.0, st);
            painter.vline(c.x, c.y - 6.0..=c.y + 3.0, st);
            painter.line_segment([egui::pos2(c.x - 3.0, c.y), egui::pos2(c.x, c.y + 3.0)], st);
            painter.line_segment([egui::pos2(c.x, c.y + 3.0), egui::pos2(c.x + 3.0, c.y)], st);
        }
        RowIcon::Power => {
            // 电源符号：顶部留缺口的圆 + 一道竖线
            const GAP: f32 = 0.86; // 缺口张角（弧度），落在正上方
            let r = 6.2_f32;
            let start = -std::f32::consts::FRAC_PI_2 + GAP * 0.5;
            let sweep = std::f32::consts::TAU - GAP;
            let pts: Vec<egui::Pos2> = (0..=24)
                .map(|i| {
                    let a = start + sweep * (i as f32 / 24.0);
                    egui::pos2(c.x + r * a.cos(), c.y + r * a.sin())
                })
                .collect();
            painter.add(egui::Shape::line(pts, st));
            painter.vline(c.x, c.y - 7.6..=c.y - 0.6, st);
        }
        RowIcon::BoxGlyph => {
            let r = Rect::from_center_size(c, egui::vec2(13.0, 10.0));
            painter.rect_stroke(r, Rounding::same(2.5), st);
            painter.hline(r.left() + 1.0..=r.right() - 1.0, c.y - 1.0, st);
            painter.hline(c.x - 2.0..=c.x + 2.0, c.y - 1.0, Stroke::new(2.0_f32, p.secondary));
        }
        RowIcon::Search => {
            painter.circle_stroke(egui::pos2(c.x - 1.5, c.y - 1.5), 5.0, st);
            painter.line_segment(
                [egui::pos2(c.x + 2.3, c.y + 2.3), egui::pos2(c.x + 7.0, c.y + 7.0)],
                Stroke::new(1.6_f32, p.secondary),
            );
        }
        RowIcon::Sort => {
            for i in 0..3 {
                let y = c.y - 4.5 + i as f32 * 3.6;
                painter.hline(c.x - 6.5..=c.x + 1.5, y, st);
            }
            painter.vline(c.x + 6.0, c.y - 3.0..=c.y + 5.0, st);
            painter.line_segment(
                [egui::pos2(c.x + 3.6, c.y + 2.2), egui::pos2(c.x + 6.0, c.y + 5.4)],
                st,
            );
            painter.line_segment(
                [egui::pos2(c.x + 8.4, c.y + 2.2), egui::pos2(c.x + 6.0, c.y + 5.4)],
                st,
            );
        }
        RowIcon::Desktop => {
            let r = Rect::from_min_max(
                egui::pos2(c.x - 7.0, c.y - 6.0),
                egui::pos2(c.x + 7.0, c.y + 3.5),
            );
            painter.rect_stroke(r, Rounding::same(1.5), st);
            painter.hline(c.x - 3.5..=c.x + 3.5, c.y + 6.2, st);
        }
        RowIcon::Folder => {
            let body = Rect::from_min_max(
                egui::pos2(c.x - 7.0, c.y - 3.0),
                egui::pos2(c.x + 7.0, c.y + 6.0),
            );
            painter.rect_stroke(body, Rounding::same(1.5), st);
            painter.hline(c.x - 7.0..=c.x - 1.0, c.y - 5.5, st);
        }
        RowIcon::Doc => {
            let r = Rect::from_center_size(c, egui::vec2(11.0, 13.0));
            painter.rect_stroke(r, Rounding::same(1.5_f32), st);
            for i in 0..2 {
                let y = c.y - 3.0 + i as f32 * 4.0;
                painter.hline(c.x - 3.5..=c.x + 3.5, y, st);
            }
        }
        RowIcon::Info => {
            painter.circle_stroke(c, 7.0, st);
            painter.circle_filled(egui::pos2(c.x, c.y - 2.6), 1.0, p.secondary);
            painter.vline(c.x, c.y + 0.4..=c.y + 4.4, st);
        }
        RowIcon::Width => {
            painter.hline(c.x - 6.0..=c.x + 6.0, c.y, st);
            painter.vline(c.x - 6.0, c.y - 3.0..=c.y + 3.0, st);
            painter.vline(c.x + 6.0, c.y - 3.0..=c.y + 3.0, st);
        }
        RowIcon::Exit => {
            // open door on the left, arrow leaving to the right
            painter.vline(c.x - 6.5, c.y - 7.0..=c.y + 7.0, st);
            painter.hline(c.x - 6.5..=c.x - 1.5, c.y - 7.0, st);
            painter.hline(c.x - 6.5..=c.x - 1.5, c.y + 7.0, st);
            painter.hline(c.x - 3.5..=c.x + 6.5, c.y, st);
            painter.line_segment(
                [egui::pos2(c.x + 3.0, c.y - 3.2), egui::pos2(c.x + 6.5, c.y)],
                st,
            );
            painter.line_segment(
                [egui::pos2(c.x + 6.5, c.y), egui::pos2(c.x + 3.0, c.y + 3.2)],
                st,
            );
        }
        RowIcon::Link => {
            // Box with an arrow leaving it: "opens outside DCX".
            let r = Rect::from_min_max(
                egui::pos2(c.x - 7.5, c.y - 2.0),
                egui::pos2(c.x + 3.0, c.y + 7.5),
            );
            painter.rect_stroke(r, Rounding::same(2.0), st);
            painter.line_segment(
                [egui::pos2(c.x - 0.5, c.y + 1.5), egui::pos2(c.x + 6.5, c.y - 6.5)],
                st,
            );
            painter.line_segment(
                [egui::pos2(c.x + 2.0, c.y - 6.5), egui::pos2(c.x + 6.5, c.y - 6.5)],
                st,
            );
            painter.line_segment(
                [egui::pos2(c.x + 6.5, c.y - 6.5), egui::pos2(c.x + 6.5, c.y - 2.0)],
                st,
            );
        }
    }
}

/// Win11 radio group: 20 px ring, accent dot when selected. Items are added in
/// reverse so they read left-to-right inside a right-to-left parent layout.
fn radios<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    p: &Palette,
    value: &mut T,
    options: &[(T, &str)],
) -> bool {
    let mut changed = false;
    for (v, label) in options.iter().rev() {
        let selected = *v == *value;
        let text_font = font(14.0);
        let text_w = ui
            .painter()
            .layout_no_wrap(label.to_string(), text_font.clone(), p.text)
            .size()
            .x;
        let size = egui::vec2(27.0 + text_w + 10.0, 26.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        let center = egui::pos2(rect.left() + 10.0, rect.center().y);
        if selected {
            ui.painter()
                .circle_stroke(center, 8.0, Stroke::new(1.4_f32, p.accent));
            ui.painter().circle_filled(center, 4.0, p.accent);
        } else {
            ui.painter()
                .circle_stroke(center, 8.0, Stroke::new(1.2_f32, p.tertiary));
        }
        ui.painter().text(
            egui::pos2(rect.left() + 24.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            text_font,
            p.text,
        );
        if resp.clicked() {
            *value = *v;
            changed = true;
        }
        ui.add_space(8.0);
    }
    changed
}

/// Win11 toggle: 40×20 pill, round knob, accent track when on.
fn switch(ui: &mut egui::Ui, p: &Palette, on: &mut bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(40.0, 20.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let track = Rect::from_center_size(rect.center(), egui::vec2(40.0, 20.0));
    if *on {
        ui.painter()
            .rect_filled(track, Rounding::same(10.0), p.accent);
        ui.painter().circle_filled(
            egui::pos2(track.right() - 10.0, track.center().y),
            6.0,
            Color32::WHITE,
        );
    } else {
        ui.painter()
            .rect_filled(track, Rounding::same(10.0), p.switch_off);
        ui.painter().rect_stroke(
            track,
            Rounding::same(10.0),
            Stroke::new(1.0_f32, p.switch_off_border),
        );
        ui.painter().circle_filled(
            egui::pos2(track.left() + 10.0, track.center().y),
            6.0,
            if p.dark {
                Color32::from_rgb(0xD0, 0xD0, 0xD0)
            } else {
                Color32::from_rgb(0x5D, 0x5D, 0x5D)
            },
        );
    }
    if resp.clicked() {
        *on = !*on;
        true
    } else {
        false
    }
}

/// Win11 slider: 4 px track, accent fill, 18 px round thumb with a 2 px ring.
fn slider<T: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    p: &Palette,
    rect: Rect,
    resp: &Response,
    value: &mut T,
    lo: T,
    hi: T,
    enabled: bool,
) -> bool {
    let track = Rect::from_min_max(
        egui::pos2(rect.left() + 9.0, rect.center().y - 2.0),
        egui::pos2(rect.right() - 9.0, rect.center().y + 2.0),
    );
    let span = (hi.to_f64() - lo.to_f64()).max(1.0);
    let t = ((value.to_f64() - lo.to_f64()) / span).clamp(0.0, 1.0) as f32;
    let x = track.left() + track.width() * t;
    let accent = if enabled { p.accent } else { p.tertiary };

    ui.painter()
        .rect_filled(track, Rounding::same(2.0), p.track);
    let filled = Rect::from_min_max(track.min, egui::pos2(x, track.max.y));
    if filled.width() > 0.5 {
        ui.painter()
            .rect_filled(filled, Rounding::same(2.0), accent);
    }
    let center = egui::pos2(x, track.center().y);
    ui.painter().circle_filled(center, 9.0, p.card);
    ui.painter()
        .circle_stroke(center, 8.0, Stroke::new(1.5_f32, accent));
    ui.painter().circle_filled(center, 5.0, accent);

    let mut changed = false;
    if enabled && (resp.dragged() || resp.clicked()) {
        if let Some(pos) = resp.interact_pointer_pos() {
            let nt = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
            let nv = T::from_f64((lo.to_f64() + nt as f64 * span).round());
            if nv.to_f64() != value.to_f64() {
                *value = nv;
                changed = true;
            }
        }
    }
    changed
}

/// Win11 "standard" button: #FBFBFB fill, 1 px #D4D4D4 border, 4 px radius.
fn standard_button(ui: &mut egui::Ui, p: &Palette, text: &str, enabled: bool) -> Response {
    let fg = if enabled { p.text } else { p.tertiary };
    let btn = egui::Button::new(RichText::new(text).size(14.0).color(fg))
        .fill(if p.dark {
            Color32::from_rgb(0x2D, 0x2D, 0x2D)
        } else {
            Color32::from_rgb(0xFB, 0xFB, 0xFB)
        })
        .stroke(Stroke::new(
            1.0_f32,
            if p.dark {
                Color32::from_rgb(0x3A, 0x3A, 0x3A)
            } else {
                Color32::from_rgb(0xD4, 0xD4, 0xD4)
            },
        ))
        .rounding(Rounding::same(CARD_RADIUS))
        .min_size(egui::vec2(0.0, 32.0));
    ui.add_enabled(enabled, btn)
        .on_hover_cursor(CursorIcon::PointingHand)
}

/// The armed state of a destructive action: Win11's inline confirmation uses a
/// tinted fill with the warning colour, not a permanently red button.
fn danger_button(ui: &mut egui::Ui, p: &Palette, text: &str, enabled: bool) -> Response {
    let fill = if p.dark {
        Color32::from_rgb(0x44, 0x27, 0x26)
    } else {
        Color32::from_rgb(0xFD, 0xE7, 0xE9)
    };
    let btn = egui::Button::new(RichText::new(text).size(14.0).color(if enabled {
        p.danger
    } else {
        p.tertiary
    }))
    .fill(fill)
    .stroke(Stroke::new(1.0_f32, p.danger))
    .rounding(Rounding::same(CARD_RADIUS))
    .min_size(egui::vec2(0.0, 32.0));
    ui.add_enabled(enabled, btn)
        .on_hover_cursor(CursorIcon::PointingHand)
}

fn small_button(ui: &mut egui::Ui, p: &Palette, text: &str, active: bool) -> Response {
    let btn = egui::Button::new(RichText::new(text).size(13.5).color(if active {
        p.accent
    } else {
        p.text
    }))
    .fill(if active { p.accent_soft } else { p.card })
    .stroke(Stroke::new(
        1.0_f32,
        if active { p.accent } else { p.border },
    ))
    .rounding(Rounding::same(CARD_RADIUS))
    .min_size(egui::vec2(52.0, 30.0));
    ui.add(btn).on_hover_cursor(CursorIcon::PointingHand)
}

/// One credential row: masked editor, explicit「保存」/「清除」, and a status
/// word. Secrets are never part of `island.toml` and never auto-saved — a
/// debounced write would put a half-typed password on disk.
fn island_cred_row(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    subtitle: &str,
    key: &str,
    buf: &mut String,
    error: &mut Option<String>,
) {
    let saved = dcx_island_config::credential::exists(key);
    row_reserve(ui, p, RowIcon::Doc, title, subtitle, 380.0, |ui| {
        ui.label(
            RichText::new(if saved { "已保存" } else { "未保存" })
                .size(12.0)
                .color(if saved { p.ok } else { p.tertiary }),
        );
        if standard_button(ui, p, "清除", true).clicked() {
            dcx_island_config::credential::delete(key);
            buf.clear();
        }
        if standard_button(ui, p, "保存", true).clicked() && !buf.is_empty() {
            match dcx_island_config::credential::save(key, buf) {
                Ok(()) => {
                    buf.clear();
                    *error = None;
                }
                Err(e) => *error = Some(e),
            }
        }
        ui.add(
            egui::TextEdit::singleline(buf)
                .password(true)
                .desired_width(150.0)
                .hint_text("留空不修改"),
        );
    });
}

#[derive(Clone, Copy)]
enum NavIcon {
    Appearance,
    Box,
    Search,
    Island,
    About,
}

impl NavIcon {
    /// Segoe Fluent Icons codepoint for this entry.
    fn glyph(self) -> char {
        match self {
            NavIcon::Appearance => glyph::MONITOR,
            NavIcon::Box => glyph::FOLDER,
            NavIcon::Search => glyph::SEARCH,
            NavIcon::Island => glyph::CLOCK,
            NavIcon::About => glyph::INFO,
        }
    }

    /// One saturated hue per page, like the coloured category icons in Win11
    /// Settings. The shell's icon font is monochrome, so the glyph itself is
    /// drawn in that colour — no tile, badge or outline behind it.
    fn tint(self) -> Color32 {
        match self {
            NavIcon::Appearance => Color32::from_rgb(0x2B, 0x7D, 0xE9), // blue
            NavIcon::Box => Color32::from_rgb(0xE0, 0xA0, 0x20), // amber
            NavIcon::Search => Color32::from_rgb(0x92, 0x61, 0xDC), // violet
            NavIcon::Island => Color32::from_rgb(0xD1, 0x5C, 0x3E), // terracotta
            NavIcon::About => Color32::from_rgb(0x20, 0xA5, 0x6E), // green
        }
    }
}

/// Lift a saturated colour towards white so it stays legible on the dark
/// backdrop (0 = unchanged, 1 = white).
fn lift(c: Color32, k: f32) -> Color32 {
    let mix = |v: u8| (v as f32 + (255.0 - v as f32) * k).round() as u8;
    Color32::from_rgb(mix(c.r()), mix(c.g()), mix(c.b()))
}

fn nav_icon(ui: &egui::Ui, p: &Palette, icon: NavIcon, center: egui::Pos2) {
    let base = icon.tint();
    let color = if p.dark { lift(base, 0.28) } else { base };
    let painter = ui.painter();
    // A plain coloured glyph on the shared surface. No background shape: the
    // badge made the sidebar read as a row of app tiles instead of Win11's
    // quiet category list.
    if icons_available() {
        painter.text(center, Align2::CENTER_CENTER, icon.glyph(), icon_font(19.0), color);
        return;
    }
    // Fallback for a system without either icon font: the old hand-drawn set,
    // now in the page's own colour instead of neutral grey.
    let st = Stroke::new(1.6_f32, color);
    match icon {
        NavIcon::Appearance => {
            let r = Rect::from_center_size(center, egui::vec2(15.0, 11.0));
            painter.rect_stroke(r, Rounding::same(3.5), st);
            painter.hline(center.x - 3.0..=center.x + 3.0, center.y, st);
        }
        NavIcon::Box => {
            let r = Rect::from_center_size(center, egui::vec2(16.0, 12.0));
            painter.rect_stroke(r, Rounding::same(3.0), st);
            painter.hline(center.x - 2.5..=center.x + 2.5, center.y - 1.0, st);
        }
        NavIcon::Search => {
            painter.circle_stroke(egui::pos2(center.x - 1.5, center.y - 1.5), 5.0, st);
            painter.line_segment(
                [
                    egui::pos2(center.x + 2.0, center.y + 2.0),
                    egui::pos2(center.x + 6.0, center.y + 6.0),
                ],
                st,
            );
        }
        NavIcon::Island => {
            // A small capsule with a clock hand: one shape reads as "island"
            // without needing the icon font.
            let r = Rect::from_center_size(center, egui::vec2(17.0, 9.0));
            painter.rect_stroke(r, Rounding::same(4.5), st);
            painter.line_segment(
                [egui::pos2(center.x, center.y), egui::pos2(center.x + 3.0, center.y - 2.0)],
                st,
            );
        }
        NavIcon::About => {
            painter.circle_stroke(center, 8.0, st);
            painter.circle_filled(egui::pos2(center.x, center.y - 2.5), 1.2, color);
            painter.vline(center.x, center.y + 0.5..=center.y + 4.5, st);
        }
    }
}

/// A text field that reads as plain text until the pointer is over it.
///
/// The rule rows used to be six input boxes stacked in a white card, which made
/// the page look like a form rather than a list of categories. Win11 edits
/// inline: the value looks like text, hovering reveals a field, focusing gives
/// it the accent outline.
#[allow(clippy::too_many_arguments)]
fn ghost_edit(
    ui: &mut egui::Ui,
    p: &Palette,
    id: egui::Id,
    text: &mut String,
    width: f32,
    size: f32,
    color: Color32,
    hint: &str,
    height: f32,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    let focused = ui.memory(|m| m.has_focus(id));
    if focused {
        ui.painter()
            .rect_filled(rect, Rounding::same(4.0), p.accent_soft);
        ui.painter().rect_stroke(
            rect,
            Rounding::same(4.0),
            Stroke::new(1.0_f32, p.accent),
        );
    } else if ui.rect_contains_pointer(rect) {
        ui.painter().rect_filled(rect, Rounding::same(4.0), p.hover);
    }
    let mut edit = egui::TextEdit::singleline(text)
        .id(id)
        .frame(false)
        .font(font(size))
        .text_color(color)
        .margin(Margin::symmetric(6.0, 2.0));
    if !hint.is_empty() {
        edit = edit.hint_text(hint);
    }
    ui.put(rect, edit).changed()
}

/// Trash button with a hover halo. Drawn by hand so the glyph can switch from
/// quiet grey to the danger colour while the pointer is on it — a `Button` with
/// a fixed `RichText` colour cannot do that.
fn delete_button(ui: &mut egui::Ui, p: &Palette, tip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), Sense::click());
    let resp = resp
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tip);
    let hot = resp.hovered();
    if hot {
        ui.painter()
            .rect_filled(rect, Rounding::same(5.0), p.hover);
    }
    let color = if hot { p.danger } else { p.tertiary };
    if icons_available() {
        ui.painter()
            .text(rect.center(), Align2::CENTER_CENTER, glyph::TRASH, icon_font(14.0), color);
    } else {
        let c = rect.center();
        let st = Stroke::new(1.3_f32, color);
        ui.painter().line_segment(
            [egui::pos2(c.x - 4.5, c.y - 4.5), egui::pos2(c.x + 4.5, c.y + 4.5)],
            st,
        );
        ui.painter().line_segment(
            [egui::pos2(c.x + 4.5, c.y - 4.5), egui::pos2(c.x - 4.5, c.y + 4.5)],
            st,
        );
    }
    resp.clicked()
}

/// Removable tag used by the exclusion editor. The whole pill is the hit area;
/// the × on the right turns red on hover so the click is not a surprise.
fn exclude_chip(ui: &mut egui::Ui, p: &Palette, text: &str) -> bool {
    let f = font(12.0);
    let text_w = ui.fonts(|fonts| {
        fonts
            .layout_no_wrap(text.to_string(), f.clone(), Color32::WHITE)
            .size()
            .x
    });
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(text_w + 34.0, 24.0), Sense::click());
    let resp = resp
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("点击移除这条排除规则");
    let hot = resp.hovered();
    let fill = if hot { p.accent_soft } else { p.card };
    let painter = ui.painter();
    painter.rect_filled(rect, Rounding::same(12.0), fill);
    painter.rect_stroke(
        rect,
        Rounding::same(12.0),
        Stroke::new(1.0_f32, if hot { p.accent } else { p.border }),
    );
    painter.text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        f,
        p.text,
    );
    let c = egui::pos2(rect.right() - 11.0, rect.center().y);
    let st = Stroke::new(1.2_f32, if hot { p.danger } else { p.tertiary });
    painter.line_segment([egui::pos2(c.x - 3.5, c.y - 3.5), egui::pos2(c.x + 3.5, c.y + 3.5)], st);
    painter.line_segment([egui::pos2(c.x + 3.5, c.y - 3.5), egui::pos2(c.x - 3.5, c.y + 3.5)], st);
    resp.clicked()
}

/// One rule row, laid out like every other card row: the category name is the
/// title, the match count and delete button sit on that line, and the extension
/// list is the secondary line underneath.
///
/// The list used to share one line with the name, which left it a few hundred
/// pixels short of the longest rules — they ended mid-word with no indication
/// that anything was cut. A second line costs one row height and fits them all.
fn rule_row(
    ui: &mut egui::Ui,
    p: &Palette,
    r: &mut RuleRow,
    count: usize,
    deletable: bool,
) -> (bool, bool) {
    let mut changed = false;
    let mut delete = false;
    egui::Frame::none()
        .inner_margin(Margin::symmetric(16.0, 10.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                ui.painter()
                    .circle_filled(dot.center(), 4.0, category_color(&r.key, p));
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        changed |= ghost_edit(
                            ui,
                            p,
                            egui::Id::new(("rule-name", r.key.as_str())),
                            &mut r.label,
                            72.0,
                            13.5,
                            p.text,
                            "",
                            24.0,
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if deletable {
                                delete = delete_button(ui, p, "删除这个分类");
                            }
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new(format!("{count}"))
                                    .size(11.5)
                                    .color(p.tertiary),
                            );
                        });
                    });
                    let w = ui.available_width();
                    if r.key == dcx_organizer::FOLDER {
                        ui.allocate_ui_with_layout(
                            egui::vec2(w, 22.0),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.label(
                                    RichText::new("桌面上的文件夹会归到这一类，无需扩展名")
                                        .size(11.5)
                                        .color(p.tertiary),
                                );
                            },
                        );
                    } else {
                        changed |= ghost_edit(
                            ui,
                            p,
                            egui::Id::new(("rule-exts", r.key.as_str())),
                            &mut r.exts,
                            w,
                            12.5,
                            p.secondary,
                            "扩展名，空格分隔",
                            22.0,
                        );
                    }
                });
            });
        });
    (changed, delete)
}

/// Category colour, matching the panel's row glyphs so a rule row and its tab
/// read as the same thing.
fn category_color(key: &str, p: &Palette) -> Color32 {
    match key {
        "doc" => Color32::from_rgb(0x2F, 0x6F, 0xD0),
        "image" => Color32::from_rgb(0x1F, 0x9D, 0x67),
        "archive" => Color32::from_rgb(0xC0, 0x7A, 0x12),
        "media" => Color32::from_rgb(0x8B, 0x5C, 0xD6),
        "app" => p.accent,
        "folder" => Color32::from_rgb(0xD9, 0xA5, 0x1F),
        _ => Color32::from_rgb(0x6B, 0x72, 0x80),
    }
}

fn nav_item(
    ui: &mut egui::Ui,
    p: &Palette,
    selected: bool,
    icon: NavIcon,
    label: &str,
) -> Response {
    // 46 pt row (WinUI's NavigationViewItem is 40 pt, which read as too
    // cramped next to the 19 pt icon and 15 pt label). The pill keeps a 2 pt
    // inset on each side, so it follows the row height automatically.
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 46.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let pill = Rect::from_min_size(
        egui::pos2(rect.left(), rect.top() + 2.0),
        egui::vec2(rect.width(), rect.height() - 4.0),
    );
    if selected {
        ui.painter()
            .rect_filled(pill, Rounding::same(4.0), p.nav_selected);
        // Win11's indicator: ~3×16 physical px, centred on the pill.
        let bar = Rect::from_min_size(
            egui::pos2(pill.left() + 1.5, pill.center().y - 6.5),
            egui::vec2(3.0, 13.0),
        );
        ui.painter()
            .rect_filled(bar, Rounding::same(1.5), p.nav_indicator);
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(pill, Rounding::same(4.0), p.nav_hover);
    }
    nav_icon(
        ui,
        p,
        icon,
        egui::pos2(rect.left() + 27.0, rect.center().y),
    );
    ui.painter().text(
        egui::pos2(rect.left() + 52.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font(15.0),
        if selected { p.text } else { p.secondary },
    );
    resp
}

fn engine_state_text(p: &Palette) -> (String, Color32) {
    use crate::injector::{ENGINE_ABSENT, ENGINE_ATTACHED};
    match crate::injector::engine_state() {
        ENGINE_ATTACHED => ("已注入 explorer".to_string(), p.ok),
        ENGINE_ABSENT => ("未注入".to_string(), p.warn),
        _ => ("检测中".to_string(), p.tertiary),
    }
}

fn status_block(ui: &mut egui::Ui, p: &Palette, dirty: bool, error: Option<&str>) {
    use crate::injector::{ENGINE_ABSENT, ENGINE_ATTACHED};
    let (text, color) = if let Some(e) = error {
        (e.to_string(), p.warn)
    } else if dirty {
        ("应用中…".to_string(), p.secondary)
    } else {
        match crate::injector::engine_state() {
            ENGINE_ATTACHED => ("已应用 · 约 1 秒内生效".to_string(), p.ok),
            ENGINE_ABSENT => ("已写入 · 引擎未注入".to_string(), p.warn),
            _ => ("已写入配置".to_string(), p.secondary),
        }
    };
    ui.horizontal_wrapped(|ui| {
        let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
        ui.painter().circle_filled(dot.center(), 3.0, color);
        ui.label(RichText::new(text).size(12.0).color(color));
    });
}

/// Segmented light/dark switch: one pill track, the thumb slides to the active
/// side, and each half carries its own glyph. Replaces the old dropdown-looking
/// button, which promised a menu it did not have.
/// Returns `Some(dark)` when the user picked the other side.
fn theme_switch(ui: &mut egui::Ui, p: &Palette, dark: bool) -> Option<bool> {
    let size = egui::vec2(66.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("外观：浅色 / 深色");
    let half = rect.width() / 2.0;
    let thumb_fill = if p.dark {
        Color32::from_rgb(0x45, 0x45, 0x45)
    } else {
        Color32::WHITE
    };

    ui.painter()
        .rect_filled(rect, Rounding::same(rect.height() / 2.0), p.track);
    let thumb = Rect::from_min_size(
        egui::pos2(
            if dark { rect.left() + half + 2.0 } else { rect.left() + 2.0 },
            rect.top() + 2.0,
        ),
        egui::vec2(half - 4.0, rect.height() - 4.0),
    );
    ui.painter().rect_filled(
        thumb,
        Rounding::same(thumb.height() / 2.0),
        thumb_fill,
    );
    ui.painter().rect_stroke(
        thumb,
        Rounding::same(thumb.height() / 2.0),
        Stroke::new(1.0_f32, p.border),
    );

    let sun_c = egui::pos2(rect.left() + half / 2.0, rect.center().y);
    let moon_c = egui::pos2(rect.left() + half * 1.5, rect.center().y);
    let sun_color = if dark { p.tertiary } else { p.text };
    let moon_color = if dark { p.text } else { p.tertiary };
    if icons_available() {
        ui.painter().text(
            sun_c,
            Align2::CENTER_CENTER,
            glyph::SUN,
            icon_font(14.0),
            sun_color,
        );
        ui.painter().text(
            moon_c,
            Align2::CENTER_CENTER,
            glyph::MOON,
            icon_font(14.0),
            moon_color,
        );
    } else {
        sun_glyph(ui.painter(), sun_c, sun_color);
        // The crescent is cut out of a filled disc using whatever sits behind it.
        moon_glyph(
            ui.painter(),
            moon_c,
            moon_color,
            if dark { thumb_fill } else { p.track },
        );
    }

    if resp.clicked() {
        Some(!dark)
    } else {
        None
    }
}

fn sun_glyph(painter: &egui::Painter, c: egui::Pos2, color: Color32) {
    let st = Stroke::new(1.3_f32, color);
    painter.circle_stroke(c, 4.2, st);
    for i in 0..8 {
        let a = std::f32::consts::TAU * (i as f32 / 8.0);
        let (sin, cos) = a.sin_cos();
        painter.line_segment(
            [
                egui::pos2(c.x + cos * 6.2, c.y + sin * 6.2),
                egui::pos2(c.x + cos * 8.0, c.y + sin * 8.0),
            ],
            st,
        );
    }
}

fn moon_glyph(painter: &egui::Painter, c: egui::Pos2, color: Color32, cut: Color32) {
    painter.circle_filled(c, 7.0, color);
    painter.circle_filled(egui::pos2(c.x + 4.0, c.y - 2.4), 5.8, cut);
}

/// Put desktop icons back, but only when this app hid them. Icons the user
/// had already hidden before the box started stay hidden.
fn restore_desktop_icons_if_owned() {
    if dcx_win32::desktop_icons_owned() {
        let _ = dcx_win32::set_desktop_icons_hidden(false);
        dcx_win32::set_desktop_icons_owned(false);
    }
}
