//! The OCR result card: a compact, scrollable text window with copy and
//! offline translate actions. Painting and hit testing share one geometry, the
//! same rule the toolbar follows.

use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::ffi::*;
use crate::icons::{self, Icon};
use crate::layout::{
    ACCENT, ACTIVE, BORDER, DANGER, DISABLED, HOVER, INK, MUTED, PANEL, PRESSED, TEXT,
};
use crate::model::TextStyle;
use crate::paint;
use crate::surface::{Rect, Rgba, Surface};
use crate::text;
use crate::translate::{self, Direction};
use dcx_win32::{HWND, MSG};

const W_BASE: f32 = 520.0;
const H_BASE: f32 = 560.0;
const PAD_BASE: f32 = 16.0;
const HEADER_BASE: f32 = 48.0;
const TABS_BASE: f32 = 34.0;
const FOOTER_BASE: f32 = 50.0;
const RADIUS_BASE: f32 = 12.0;
const BTN_H_BASE: f32 = 30.0;
const FONT_TITLE: f32 = 15.0;
const FONT_BODY: f32 = 13.0;
const FONT_SMALL: f32 = 11.5;
const FONT_BTN: f32 = 12.0;
const TIMER_COPY: usize = 1;
const WM_DONE: u32 = 0x8000 + 1;
const NO_PROVIDER_HINT: &str =
    "未配置翻译服务。\n请到 设置中心 →「截图与标注」→「翻译」选择并填写服务（OpenAI 兼容 / LibreTranslate / 自定义 HTTP）。";

thread_local! {
    static CURRENT: core::cell::Cell<*mut Card> = const { core::cell::Cell::new(core::ptr::null_mut()) };
}

/// True while the OCR card owns the screen (and the session's nested loop).
pub fn is_active() -> bool {
    CURRENT.with(|c| !c.get().is_null())
}

/// Host stop / hotkey while the card is up: close the card, not the process.
pub fn request_close() {
    let ptr = CURRENT.with(|c| c.get());
    if !ptr.is_null() {
        unsafe {
            PostMessageW((*ptr).hwnd, WM_CLOSE, 0, 0);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Source,
    Target,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Btn {
    Close,
    TabSource,
    TabTarget,
    Direction,
    Copy,
    Translate,
}

struct Geom {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    header: Rect,
    tabs: Rect,
    body: Rect,
    footer: Rect,
    close: Rect,
    tab_source: Rect,
    tab_target: Rect,
    direction: Rect,
    copy: Rect,
    translate: Rect,
    s: f32,
}

impl Geom {
    fn new(
        monitor: (i32, i32, i32, i32),
        anchor: (i32, i32),
        scale: f32,
        source: &str,
        target: Option<&str>,
    ) -> Geom {
        let s = scale.max(1.0);
        let (mx, my, mw, mh) = monitor;
        let w = (W_BASE * s).min(mw as f32 - 24.0 * s).max(360.0 * s);
        let pad = PAD_BASE * s;
        let line_h = (FONT_BODY * 1.65 * s).round().max(18.0 * s);
        let text_w = (w - 2.0 * pad - 16.0 * s).max(40.0);
        let mut lines = wrap(source, text_w, FONT_BODY * s).len().max(1);
        if let Some(target) = target {
            lines = lines.max(wrap(target, text_w, FONT_BODY * s).len());
        }
        let chrome = (HEADER_BASE + TABS_BASE + 6.0 + FOOTER_BASE) * s;
        let max_h = (H_BASE * s).min((mh as f32 - 40.0 * s).max(260.0 * s));
        let h = (chrome + lines as f32 * line_h + 14.0 * s).clamp((340.0 * s).min(max_h), max_h);
        let x = (anchor.0 as f32 - w * 0.5).clamp(
            mx as f32 + 12.0 * s,
            (mx as f32 + mw as f32 - 12.0 * s - w).max(mx as f32 + 12.0 * s),
        );
        let y = (anchor.1 as f32 - h * 0.5).clamp(
            my as f32 + 12.0 * s,
            (my as f32 + mh as f32 - 12.0 * s - h).max(my as f32 + 12.0 * s),
        );
        let header = Rect::from_xywh(0.0, 0.0, w, HEADER_BASE * s);
        let tabs = Rect::from_xywh(pad, HEADER_BASE * s, w - 2.0 * pad, TABS_BASE * s);
        let footer = Rect::from_xywh(pad, h - FOOTER_BASE * s, w - 2.0 * pad, FOOTER_BASE * s);
        let body = Rect::from_xywh(
            pad,
            HEADER_BASE * s + TABS_BASE * s + 6.0 * s,
            w - 2.0 * pad,
            h - FOOTER_BASE * s - (HEADER_BASE + TABS_BASE + 6.0) * s,
        );
        let btn_h = BTN_H_BASE * s;
        let close = Rect::from_xywh(
            w - pad - 28.0 * s,
            (HEADER_BASE * s - 28.0 * s) * 0.5,
            28.0 * s,
            28.0 * s,
        );
        let tab_h = 27.0 * s;
        let tab_y = tabs.y0 + (tabs.height() - tab_h) * 0.5;
        let tab_source = Rect::from_xywh(tabs.x0, tab_y, 66.0 * s, tab_h);
        let tab_target = Rect::from_xywh(tabs.x0 + 72.0 * s, tab_y, 66.0 * s, tab_h);
        let btn_y = footer.y0 + (footer.height() - btn_h) * 0.5;
        let direction = Rect::from_xywh(footer.x0, btn_y, 88.0 * s, btn_h);
        let translate = Rect::from_xywh(footer.x1 - 92.0 * s, btn_y, 92.0 * s, btn_h);
        let copy = Rect::from_xywh(translate.x0 - 82.0 * s, btn_y, 74.0 * s, btn_h);
        Geom {
            x: x.round() as i32,
            y: y.round() as i32,
            w: w.round() as u32,
            h: h.round() as u32,

            header,
            tabs,
            body,
            footer,
            close,
            tab_source,
            tab_target,
            direction,
            copy,
            translate,
            s,
        }
    }

    fn hit(&self, x: f32, y: f32) -> Option<Btn> {
        for (rect, btn) in [
            (self.close, Btn::Close),
            (self.tab_source, Btn::TabSource),
            (self.tab_target, Btn::TabTarget),
            (self.direction, Btn::Direction),
            (self.copy, Btn::Copy),
            (self.translate, Btn::Translate),
        ] {
            if rect.contains(x, y) {
                return Some(btn);
            }
        }
        None
    }

    fn line_h(&self) -> f32 {
        (FONT_BODY * 1.65 * self.s).round().max(18.0 * self.s)
    }

    fn visible_lines(&self) -> usize {
        (self.body.height() / self.line_h()).floor().max(1.0) as usize
    }

    fn text_width(&self) -> f32 {
        (self.body.width() - 16.0 * self.s).max(40.0)
    }
}

pub struct View<'a> {
    pub source: &'a str,
    pub badge: &'a str,
    pub target: &'a str,
    pub note: &'a str,
    pub error: bool,
    pub busy: bool,
    pub direction: Direction,
    pub tab: Tab,
    pub scroll: usize,
    pub hover: Option<Btn>,
    pub pressed: Option<Btn>,
}

pub struct Ctx {
    pub text: String,
    pub badge: String,
    pub anchor: (i32, i32),
    pub monitor: (i32, i32, i32, i32),
    pub scale: f32,
    pub translate_now: bool,
}

struct Shared {
    target: String,
    note: String,
    error: bool,
    busy: bool,
}

struct Card {
    hwnd: HWND,
    surface: Surface,
    geom: Geom,
    source: String,
    badge: String,
    target: String,
    note: String,
    error: bool,
    busy: bool,
    direction: Direction,
    tab: Tab,
    scroll: usize,
    hover: Option<Btn>,
    pressed: Option<Btn>,
    shared: Arc<Mutex<Shared>>,
    copy_until: u64,
    done: bool,
}

pub fn run(ctx: Ctx) -> Result<(), String> {
    if ctx.text.trim().is_empty() {
        return Err("未识别到文字".to_string());
    }
    let geom = Geom::new(ctx.monitor, ctx.anchor, ctx.scale, &ctx.text, None);
    let surface = Surface::new(geom.w, geom.h).ok_or("无法创建识别结果表面")?;
    let hwnd = create_window(&geom)?;
    let direction = translate::detect_direction(&ctx.text);
    let translate_now = ctx.translate_now;
    let shared = Arc::new(Mutex::new(Shared {
        target: String::new(),
        note: String::new(),
        error: false,
        busy: false,
    }));
    let mut card = Card {
        hwnd,
        surface,
        geom,
        source: ctx.text,
        badge: ctx.badge,
        target: String::new(),
        note: String::new(),
        error: false,
        busy: false,
        direction,
        tab: Tab::Source,
        scroll: 0,
        hover: None,
        pressed: None,
        shared,
        copy_until: 0,
        done: false,
    };
    unsafe {
        dcx_win32::SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut card as *mut Card as isize);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
    }
    CURRENT.with(|c| c.set(&mut card as *mut Card));
    card.render();
    if translate_now {
        card.start_translate();
    }
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, 0, 0, 0) };
        if r <= 0 || card.done {
            break;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    CURRENT.with(|c| c.set(core::ptr::null_mut()));
    unsafe {
        dcx_win32::SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    }
    drop(card);
    Ok(())
}

/// Offscreen render used by `--demo-synth`; no windows, no OCR engine.
pub fn demo(out: &Path) -> Result<(), String> {
    let source = "DCX 文字识别\n工具栏新增「识别文字」，框选后按 T 即可。\n识别结果支持复制与翻译；在线服务由你在设置中配置。\n\nThis card is rendered entirely by the snapshot overlay's own vector and GDI text stack.";
    let target = "DCX offline OCR\nThe toolbar has a new “Recognize text” action; select a region and press T.\nThe result can be copied or translated locally, without network or upload.";
    let geom = Geom::new((0, 0, 600, 680), (300, 340), 1.0, source, Some(target));
    let stem = out
        .file_stem()
        .map(|x| x.to_string_lossy().into_owned())
        .unwrap_or_default();
    for (tab, name) in [(Tab::Source, "source"), (Tab::Target, "target")] {
        let mut surface = Surface::new(geom.w, geom.h).ok_or("无法创建演示表面")?;
        let view = View {
            source,
            badge: "离线 OCR",
            target,
            note: "",
            error: false,
            busy: false,
            direction: Direction::ZhEn,
            tab,
            scroll: 0,
            hover: Some(Btn::Translate),
            pressed: None,
        };
        render_surface(&mut surface, &geom, &view);
        let path = out.with_file_name(format!("{stem}-ocr-{name}.png"));
        crate::output::write_png(&path, surface.bytes(), surface.w, surface.h)?;
    }
    // No-provider state: one muted hint in the target page, no duplicate in
    // the tab status row.
    let mut surface = Surface::new(geom.w, geom.h).ok_or("无法创建演示表面")?;
    let view = View {
        source,
        badge: "离线 OCR",
        target: "",
        note: NO_PROVIDER_HINT,
        error: false,
        busy: false,
        direction: Direction::ZhEn,
        tab: Tab::Target,
        scroll: 0,
        hover: None,
        pressed: None,
    };
    render_surface(&mut surface, &geom, &view);
    let path = out.with_file_name(format!("{stem}-ocr-hint.png"));
    crate::output::write_png(&path, surface.bytes(), surface.w, surface.h)?;
    Ok(())
}

impl Card {
    fn render(&mut self) {
        let view = View {
            source: &self.source,
            badge: &self.badge,
            target: &self.target,
            note: &self.note,
            error: self.error,
            busy: self.busy,
            direction: self.direction,
            tab: self.tab,
            scroll: self.scroll,
            hover: self.hover,
            pressed: self.pressed,
        };
        render_surface(&mut self.surface, &self.geom, &view);
        unsafe {
            crate::longshot::present_layered(self.hwnd, &self.surface);
        }
    }

    fn active_text(&self) -> &str {
        match self.tab {
            Tab::Source => &self.source,
            Tab::Target => {
                if self.target.is_empty() {
                    &self.note
                } else {
                    &self.target
                }
            }
        }
    }

    fn max_scroll(&self) -> usize {
        let lines = wrap(
            self.active_text(),
            self.geom.text_width(),
            FONT_BODY * self.geom.s,
        );
        lines.len().saturating_sub(self.geom.visible_lines())
    }

    fn scroll_by(&mut self, delta: i32) {
        let max = self.max_scroll() as i32;
        let next = (self.scroll as i32 + delta).clamp(0, max);
        if next as usize != self.scroll {
            self.scroll = next as usize;
            self.render();
        }
    }

    fn act(&mut self, btn: Btn) {
        match btn {
            Btn::Close => self.done = true,
            Btn::TabSource => {
                self.tab = Tab::Source;
                self.scroll = 0;
                self.render();
            }
            Btn::TabTarget => {
                self.tab = Tab::Target;
                self.scroll = 0;
                self.render();
            }
            Btn::Direction => {
                if self.busy {
                    return;
                }
                self.direction = self.direction.toggled();
                self.target.clear();
                self.note.clear();
                self.error = false;
                self.tab = Tab::Source;
                self.scroll = 0;
                self.render();
            }
            Btn::Copy => {
                // The target tab shows `note` when it has no translation; that
                // is a hint or an error, never something to put on the
                // clipboard.
                let text = match self.tab {
                    Tab::Source => self.source.clone(),
                    Tab::Target => self.target.clone(),
                };
                if text.trim().is_empty() {
                    self.note = "没有可复制的内容".to_string();
                    self.error = true;
                } else {
                    match crate::output::to_clipboard_text(self.hwnd, &text) {
                        Ok(()) => {
                            self.note = "已复制".to_string();
                            self.error = false;
                            self.copy_until = now_ms() + 1200;
                            unsafe {
                                SetTimer(self.hwnd, TIMER_COPY, 1200, core::ptr::null());
                            }
                        }
                        Err(e) => {
                            self.note = e;
                            self.error = true;
                        }
                    }
                }
                self.render();
            }
            Btn::Translate => self.start_translate(),
        }
    }

    fn start_translate(&mut self) {
        if self.busy {
            return;
        }
        if !translate::has_translatable(&self.source) {
            self.note = "没有可翻译的文字".to_string();
            self.error = true;
            self.tab = Tab::Source;
            self.render();
            return;
        }
        if translation_missing() {
            self.busy = false;
            self.target.clear();
            self.note = NO_PROVIDER_HINT.to_string();
            self.error = false;
            self.copy_until = 0;
            self.tab = Tab::Target;
            self.scroll = 0;
            self.render();
            return;
        }
        self.busy = true;
        self.target.clear();
        self.note = "正在翻译…".to_string();
        self.error = false;
        self.copy_until = 0;
        self.tab = Tab::Target;
        self.scroll = 0;
        {
            let mut shared = match self.shared.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            shared.target.clear();
            shared.note = self.note.clone();
            shared.error = false;
            shared.busy = true;
        }
        self.render();
        let dir = self.direction;
        let source = self.source.clone();
        let shared = self.shared.clone();
        let hwnd = self.hwnd as isize;
        std::thread::spawn(move || {
            let result = translate::translate(&source, dir);
            if let Ok(mut shared) = shared.lock() {
                shared.busy = false;
                match result {
                    Ok(text) => {
                        shared.target = text;
                        shared.note.clear();
                        shared.error = false;
                    }
                    Err(e) => {
                        shared.target.clear();
                        shared.note = e;
                        shared.error = true;
                    }
                }
            }
            unsafe {
                PostMessageW(hwnd as HWND, WM_DONE, 0, 0);
            }
        });
    }

    fn poll(&mut self) {
        let (target, note, error, busy) = {
            let Ok(shared) = self.shared.lock() else {
                return;
            };
            (
                shared.target.clone(),
                shared.note.clone(),
                shared.error,
                shared.busy,
            )
        };
        self.target = target;
        self.note = note;
        self.error = error;
        self.busy = busy;
        if !self.target.is_empty() {
            self.tab = Tab::Target;
            self.scroll = 0;
        }
        if self.busy {
            self.tab = Tab::Target;
        }
        self.render();
    }

    fn on_message(&mut self, hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
        match msg {
            WM_MOUSEACTIVATE => 1, // MA_ACTIVATE: the card needs keyboard focus.
            WM_LBUTTONDOWN => {
                let (x, y) = point_of(lp);
                let hit = self.geom.hit(x, y);
                if hit.is_none() && self.geom.header.contains(x, y) {
                    // Header background drags the popup like a title bar.
                    unsafe {
                        ReleaseCapture();
                        SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION, 0);
                    }
                    return 0;
                }
                unsafe {
                    SetCapture(hwnd);
                }
                self.pressed = hit;
                self.hover = hit;
                self.render();
                0
            }
            WM_MOUSEMOVE => {
                let (x, y) = point_of(lp);
                let hit = self.geom.hit(x, y);
                if hit != self.hover {
                    self.hover = hit;
                    self.render();
                }
                0
            }
            WM_LBUTTONUP => {
                let pressed = self.pressed.take();
                unsafe {
                    ReleaseCapture();
                }
                let (x, y) = point_of(lp);
                let hit = self.geom.hit(x, y);
                self.hover = hit;
                if pressed == hit {
                    if let Some(btn) = hit {
                        self.act(btn);
                    }
                }
                self.render();
                0
            }
            WM_CAPTURECHANGED => {
                self.pressed = None;
                self.render();
                0
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp >> 16) & 0xFFFF) as u16 as i16 as i32;
                self.scroll_by(-(delta / 120) * 3);
                0
            }
            WM_KEYDOWN => {
                match wp as i32 {
                    VK_ESCAPE => self.done = true,
                    VK_TAB => {
                        self.tab = match self.tab {
                            Tab::Source => Tab::Target,
                            Tab::Target => Tab::Source,
                        };
                        self.scroll = 0;
                        self.render();
                    }
                    VK_PRIOR => self.scroll_by(-(self.geom.visible_lines() as i32)),
                    VK_NEXT => self.scroll_by(self.geom.visible_lines() as i32),
                    VK_HOME => self.scroll_by(-(self.max_scroll() as i32 + 1)),
                    VK_END => self.scroll_by(self.max_scroll() as i32 + 1),
                    _ => {
                        let ctrl = unsafe { GetKeyState(VK_CONTROL) } < 0;
                        if ctrl && wp as u32 == 'C' as u32 {
                            self.act(Btn::Copy);
                        }
                    }
                }
                0
            }
            WM_TIMER if wp == TIMER_COPY => {
                if now_ms() >= self.copy_until {
                    if self.note == "已复制" {
                        self.note.clear();
                    }
                    self.copy_until = 0;
                    unsafe {
                        KillTimer(hwnd, TIMER_COPY);
                    }
                    self.render();
                }
                0
            }
            WM_DONE => {
                self.poll();
                0
            }
            WM_SETCURSOR => {
                let (id, msg16) = ((lp & 0xFFFF) as usize, (lp >> 16) as usize);
                let _ = (id, msg16);
                let name = if self.hover.is_some() {
                    IDC_HAND
                } else {
                    IDC_ARROW
                };
                unsafe {
                    let cursor = LoadCursorW(0, name as *const u16);
                    SetCursor(cursor);
                }
                1
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                unsafe {
                    BeginPaint(hwnd, &mut ps);
                    EndPaint(hwnd, &ps);
                }
                0
            }
            WM_ERASEBKGND => 1,
            WM_CLOSE => {
                self.done = true;
                0
            }
            _ => unsafe { dcx_win32::DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }
}

impl Drop for Card {
    fn drop(&mut self) {
        unsafe {
            KillTimer(self.hwnd, TIMER_COPY);
            DestroyWindow(self.hwnd);
        }
    }
}

fn render_surface(surface: &mut Surface, geom: &Geom, view: &View<'_>) {
    let s = geom.s;
    let (w, h) = (surface.w as f32, surface.h as f32);
    surface.bytes_mut().fill(0);
    surface.canvas(|sk| {
        let full = Rect::from_xywh(0.5, 0.5, w - 1.0, h - 1.0);
        sk.fill_round(&full, RADIUS_BASE * s, PANEL);
        sk.stroke_round(&full, RADIUS_BASE * s, 1.0, BORDER);

        let pad = PAD_BASE * s;
        icons::draw(
            sk,
            Icon::Scan,
            Rect::from_xywh(pad, 13.0 * s, 18.0 * s, 18.0 * s),
            ACCENT,
            1.6,
        );
        paint::label(
            sk,
            "文字识别",
            pad + 26.0 * s,
            10.0 * s,
            FONT_TITLE * s,
            TEXT,
        );
        let title_w = text::measure("文字识别", FONT_TITLE * s).0;
        paint::label(
            sk,
            view.badge,
            pad + 26.0 * s + title_w + 9.0 * s,
            13.0 * s,
            FONT_SMALL * s,
            MUTED,
        );

        button_icon(
            sk,
            geom.close,
            Icon::Dismiss,
            view.hover == Some(Btn::Close),
            view.pressed == Some(Btn::Close),
            s,
        );

        tabs(sk, geom, view);
        let status = if view.note == "已复制" {
            view.note.to_string()
        } else {
            String::new()
        };
        if !status.is_empty() {
            let color = if view.error { DANGER } else { MUTED };
            let max = (geom.tabs.x1 - geom.tabs.x0 - 170.0 * s).max(40.0);
            let shown = ellipsis(&status, max, FONT_SMALL * s);
            let tw = text::measure(&shown, FONT_SMALL * s).0;
            paint::label(
                sk,
                &shown,
                geom.tabs.x1 - tw,
                geom.tabs.y0 + 6.0 * s,
                FONT_SMALL * s,
                color,
            );
        }

        sk.line(
            (geom.body.x0, geom.footer.y0 - 7.0 * s),
            (geom.body.x1, geom.footer.y0 - 7.0 * s),
            1.0,
            BORDER,
        );
        body(sk, geom, view);

        let translate_label = if view.busy {
            "翻译中…"
        } else if view.target.is_empty() {
            "翻译"
        } else {
            "重新翻译"
        };
        button(
            sk,
            geom.direction,
            view.direction.label(),
            if view.busy { DISABLED } else { TEXT },
            HOVER,
            BORDER,
            view.hover == Some(Btn::Direction),
            view.pressed == Some(Btn::Direction),
            FONT_BTN * s,
        );
        let copy_disabled =
            view.busy || view.target.is_empty() && view.tab == Tab::Target && view.note.is_empty();
        button(
            sk,
            geom.copy,
            "复制",
            if copy_disabled { DISABLED } else { TEXT },
            HOVER,
            BORDER,
            view.hover == Some(Btn::Copy),
            view.pressed == Some(Btn::Copy),
            FONT_BTN * s,
        );
        button(
            sk,
            geom.translate,
            translate_label,
            if view.busy { MUTED } else { INK },
            if view.busy { ACTIVE } else { ACCENT },
            Rgba::new(0, 0, 0, 0),
            view.hover == Some(Btn::Translate),
            view.pressed == Some(Btn::Translate),
            FONT_BTN * s,
        );
    });
}

fn tabs(sk: &mut crate::surface::Sk<'_>, geom: &Geom, view: &View<'_>) {
    let s = geom.s;
    for (rect, tab, label, btn) in [
        (geom.tab_source, Tab::Source, "原文", Btn::TabSource),
        (geom.tab_target, Tab::Target, "译文", Btn::TabTarget),
    ] {
        if view.hover == Some(btn) {
            sk.fill_round(&rect, 7.0 * s, HOVER);
        }
        if view.tab == tab {
            sk.fill_round(&rect, 7.0 * s, ACTIVE);
            let underline = Rect::from_xywh(
                rect.x0 + 12.0 * s,
                rect.y1 - 2.0 * s,
                rect.width() - 24.0 * s,
                2.0 * s,
            );
            sk.fill_round(&underline, 1.0 * s, ACCENT);
        }
        let color = if view.tab == tab { TEXT } else { MUTED };
        let tw = text::measure(label, FONT_BTN * s).0;
        let th = text::measure(label, FONT_BTN * s).1;
        paint::label(
            sk,
            label,
            rect.center().0 - tw * 0.5,
            rect.center().1 - th * 0.5,
            FONT_BTN * s,
            color,
        );
    }
}

fn body(sk: &mut crate::surface::Sk<'_>, geom: &Geom, view: &View<'_>) {
    let s = geom.s;
    let raw = match view.tab {
        Tab::Source => view.source,
        Tab::Target => {
            if view.target.is_empty() {
                view.note
            } else {
                view.target
            }
        }
    };
    let color = if view.tab == Tab::Target && view.target.is_empty() && !view.note.is_empty() {
        if view.error {
            DANGER
        } else {
            MUTED
        }
    } else {
        TEXT
    };
    if raw.trim().is_empty() {
        return;
    }
    let lines = wrap(raw, geom.text_width(), FONT_BODY * s);
    let visible = geom.visible_lines();
    let max_scroll = lines.len().saturating_sub(visible);
    let start = view.scroll.min(max_scroll);
    let line_h = geom.line_h();
    for (i, line) in lines.iter().skip(start).take(visible).enumerate() {
        paint::label(
            sk,
            line,
            geom.body.x0,
            geom.body.y0 + i as f32 * line_h,
            FONT_BODY * s,
            color,
        );
    }
    if lines.len() > visible {
        let track = Rect::from_xywh(
            geom.body.x1 - 3.0 * s,
            geom.body.y0,
            3.0 * s,
            geom.body.height(),
        );
        sk.fill_round(&track, 1.5 * s, Rgba::new(0xFF, 0xFF, 0xFF, 0x12));
        let frac = visible as f32 / lines.len() as f32;
        let thumb_h = (geom.body.height() * frac).max(14.0 * s);
        let offset = if max_scroll == 0 {
            0.0
        } else {
            (geom.body.height() - thumb_h) * start as f32 / max_scroll as f32
        };
        let thumb = Rect::from_xywh(track.x0, geom.body.y0 + offset, 3.0 * s, thumb_h);
        sk.fill_round(&thumb, 1.5 * s, Rgba::new(0xFF, 0xFF, 0xFF, 0x66));
    }
}

fn button(
    sk: &mut crate::surface::Sk<'_>,
    rect: Rect,
    label: &str,
    fg: Rgba,
    bg: Rgba,
    border: Rgba,
    hover: bool,
    pressed: bool,
    size: f32,
) {
    let radius = 7.0 * (size / FONT_BTN).max(1.0);
    let fill = if pressed {
        PRESSED
    } else if hover {
        paint::mix(bg, ACCENT, 0.22)
    } else {
        bg
    };
    sk.fill_round(&rect, radius, fill);
    if border.a > 0 {
        sk.stroke_round(&rect, radius, 1.0, if hover { ACTIVE } else { border });
    }
    let (tw, th) = text::measure_styled(label, size, &TextStyle::ui());
    paint::label(
        sk,
        label,
        rect.center().0 - tw * 0.5,
        rect.center().1 - th * 0.5,
        size,
        fg,
    );
}

fn button_icon(
    sk: &mut crate::surface::Sk<'_>,
    rect: Rect,
    icon: Icon,
    hover: bool,
    pressed: bool,
    s: f32,
) {
    if hover || pressed {
        sk.fill_round(&rect, 7.0 * s, if pressed { PRESSED } else { HOVER });
    }
    icons::draw(sk, icon, rect.inflate(-8.0 * s), TEXT, 1.6);
}

fn ellipsis(s: &str, max_w: f32, size: f32) -> String {
    if text::measure(s, size).0 <= max_w {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars() {
        out.push(ch);
        if text::measure(&format!("{out}…"), size).0 > max_w {
            out.pop();
            break;
        }
    }
    out.push('…');
    out
}

/// Word/CJK-aware wrap. CJK glyphs stand alone; Latin runs stay whole unless a
/// single run is wider than the body, in which case it is split by character.
///
/// Every wrap candidate costs a GDI `GetTextExtent` and the per-line candidate
/// string grows with the line, so a full-screen recognition re-wrapped on every
/// hover/scroll frame was O(n²). Results are cached by (text, width, size):
/// the card only ever wraps its source, target and hint.
fn wrap(text: &str, max_w: f32, size: f32) -> Rc<Vec<String>> {
    const CACHE_MAX: usize = 8;
    let (kw, ks) = (max_w.to_bits(), size.to_bits());
    let hit = WRAP_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let pos = cache
            .iter()
            .position(|(t, w, s, _)| *w == kw && *s == ks && t == text)?;
        let entry = cache.remove(pos);
        let lines = entry.3.clone();
        cache.push(entry);
        Some(lines)
    });
    if let Some(lines) = hit {
        return lines;
    }
    let lines = Rc::new(wrap_uncached(text, max_w, size));
    WRAP_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= CACHE_MAX {
            cache.remove(0);
        }
        cache.push((text.to_string(), kw, ks, lines.clone()));
    });
    lines
}

thread_local! {
    static WRAP_CACHE: std::cell::RefCell<Vec<(String, u32, u32, Rc<Vec<String>>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn wrap_uncached(text: &str, max_w: f32, size: f32) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split('\n') {
        if raw.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let mut cur = String::new();
        for token in tokens(raw) {
            let sep = if cur.is_empty() || !needs_space(&cur, &token) {
                ""
            } else {
                " "
            };
            let candidate = format!("{cur}{sep}{token}");
            if text::measure(&candidate, size).0 <= max_w {
                cur = candidate;
                continue;
            }
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            if text::measure(&token, size).0 <= max_w {
                cur = token;
            } else {
                // A single URL/long run: break it by character.
                let mut buf = String::new();
                for ch in token.chars() {
                    let cand = format!("{buf}{ch}");
                    if text::measure(&cand, size).0 > max_w && !buf.is_empty() {
                        out.push(std::mem::take(&mut buf));
                    }
                    buf.push(ch);
                }
                cur = buf;
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
}

fn tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for ch in line.chars() {
        if ch.is_whitespace() {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
        } else if is_cjk(ch) {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
            out.push(ch.to_string());
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

fn needs_space(left: &str, right: &str) -> bool {
    let l = left.chars().last();
    let r = right.chars().next();
    !l.is_some_and(is_cjk) && !r.is_some_and(is_cjk)
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0x3000..=0x303F
        | 0xFF00..=0xFFEF)
}

fn now_ms() -> u64 {
    unsafe { GetTickCount64() }
}

/// A provider counts as configured only when its required fields are present.
/// This keeps an empty "custom" selection on the quiet single hint instead of
/// starting a request that can only fail.
fn translation_missing() -> bool {
    let cfg = crate::config::load().translate;
    match cfg.provider.trim() {
        "none" | "" => true,
        "custom" => cfg.custom_url.trim().is_empty() || cfg.custom_body.trim().is_empty(),
        "openai" => {
            cfg.openai_base_url.trim().contains("api.openai.com")
                && cfg.openai_key.trim().is_empty()
        }
        _ => false,
    }
}

fn point_of(lp: isize) -> (f32, f32) {
    let x = (lp & 0xFFFF) as u16 as i16;
    let y = ((lp >> 16) & 0xFFFF) as u16 as i16;
    (x as f32, y as f32)
}

fn create_window(geom: &Geom) -> Result<HWND, String> {
    unsafe {
        let class = crate::wide("DCX.Snip.OcrCard");
        let cls = dcx_win32::WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(card_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(core::ptr::null()),
            hIcon: 0,
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: core::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        let _ = dcx_win32::RegisterClassW(&cls);
        let title = crate::wide("DCX 文字识别");
        let hwnd = dcx_win32::CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            geom.x,
            geom.y,
            geom.w as i32,
            geom.h as i32,
            0,
            0,
            GetModuleHandleW(core::ptr::null()),
            core::ptr::null_mut(),
        );
        if hwnd == 0 {
            Err("无法创建识别结果窗口".to_string())
        } else {
            Ok(hwnd)
        }
    }
}

unsafe extern "system" fn card_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    let ptr = unsafe { dcx_win32::GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Card;
    if ptr.is_null() {
        return unsafe { dcx_win32::DefWindowProcW(hwnd, msg, wp, lp) };
    }
    unsafe { (*ptr).on_message(hwnd, msg, wp, lp) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_wraps_without_spaces_and_latin_keeps_words() {
        let lines = wrap("这是 一段 中文 English words here", 80.0, 13.0);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| !l.is_empty()));
        assert!(lines
            .iter()
            .any(|l| l.contains("English") || l.contains("words")));
    }

    #[test]
    fn long_unbroken_run_is_split() {
        let lines = wrap(&"x".repeat(300), 80.0, 13.0);
        assert!(lines.len() > 2);
        assert!(lines.iter().all(|l| text::measure(l, 13.0).0 <= 88.0));
    }
}
