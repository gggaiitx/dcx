//! Clipboard history: capture, de-duplicate, persist, write back.
//!
//! # Threading
//!
//! `AddClipboardFormatListener` requires a window and a message loop, so a
//! dedicated thread owns an `HWND_MESSAGE` window and does nothing but convert
//! `WM_CLIPBOARDUPDATE` into a [`RawClip`] on a channel. **The UI thread owns
//! the store** and performs every write: `config_file`'s lock is a cross-process
//! mutex with a 3 s timeout, and a second writer would eventually block a frame.
//!
//! # What is captured
//!
//! * text (`CF_UNICODETEXT`) up to `max_text_chars`;
//! * images: `PNG` when the source published it (browsers do, and it keeps the
//!   alpha channel), otherwise the DIB is converted to RGBA;
//! * nothing at all when the clipboard carries `CanIncludeInClipboardHistory`
//!   with a zero value — the documented opt-out that password managers and
//!   Windows' own clipboard history use.
//!
//! Images are stored as PNG files under `%LOCALAPPDATA%\dcx\island_clipboard`
//! and referenced by name; the JSON index therefore stays small and a 4 MB
//! screenshot does not turn into 5.5 MB of base64.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sys;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Text,
    Image,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClipItem {
    pub id: u64,
    pub kind: Kind,
    #[serde(default)]
    pub text: String,
    /// File name inside the clipboard image directory.
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    pub hash: u64,
    pub created_at: u64,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub note: String,
}

impl ClipItem {
    pub fn preview(&self) -> String {
        match self.kind {
            Kind::Text => {
                let one_line = self.text.replace(['\r', '\n'], " ");
                let trimmed = one_line.trim();
                if trimmed.chars().count() > 120 {
                    let head: String = trimmed.chars().take(120).collect();
                    format!("{head}…")
                } else {
                    trimmed.to_string()
                }
            }
            Kind::Image => format!("图片 {}×{}", self.width, self.height),
        }
    }

    pub fn size_label(&self) -> String {
        match self.kind {
            Kind::Text => format!("{} 字", self.text.chars().count()),
            Kind::Image => format!("{}×{}", self.width, self.height),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ClipFile {
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub items: Vec<ClipItem>,
}

pub struct Store {
    pub file: ClipFile,
    pub path: PathBuf,
    pub dir: PathBuf,
    pub error: Option<String>,
    dirty: bool,
    /// Sequence number our own write produced, so the listener can ignore it.
    pub self_write: Option<u32>,
}

impl Store {
    pub fn load(path: PathBuf, dir: PathBuf) -> Store {
        let _ = std::fs::create_dir_all(&dir);
        let file = read_file(&path);
        Store {
            file,
            path,
            dir,
            error: None,
            dirty: false,
            self_write: None,
        }
    }

    pub fn save_if_dirty(&mut self) {
        if !self.dirty {
            return;
        }
        match write_file(&self.path, &self.file) {
            Ok(()) => {
                self.dirty = false;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Insert a captured item, replacing an identical one that is already at
    /// the top (copying the same text twice must not grow the list).
    pub fn push(&mut self, raw: RawClip, cfg: &dcx_island_config::ClipboardCfg) -> Option<u64> {
        let (item, png) = match raw {
            RawClip::Text(text) => {
                if text.trim().is_empty() {
                    return None;
                }
                let mut text = text;
                if text.chars().count() > cfg.max_text_chars {
                    text = text.chars().take(cfg.max_text_chars).collect();
                }
                if is_excluded(&text, &cfg.exclude_patterns) {
                    return None;
                }
                let hash = fnv(text.as_bytes());
                (
                    ClipItem {
                        id: 0,
                        kind: Kind::Text,
                        text,
                        image: None,
                        width: 0,
                        height: 0,
                        hash,
                        created_at: now_secs(),
                        pinned: false,
                        note: String::new(),
                    },
                    None,
                )
            }
            RawClip::Image { rgba, width, height } => {
                if !cfg.capture_images || width == 0 || height == 0 {
                    return None;
                }
                let hash = fnv(&rgba);
                (
                    ClipItem {
                        id: 0,
                        kind: Kind::Image,
                        text: String::new(),
                        image: None,
                        width,
                        height,
                        hash,
                        created_at: now_secs(),
                        pinned: false,
                        note: String::new(),
                    },
                    Some(rgba),
                )
            }
        };

        // Same content already recorded: move it to the front and reuse the row
        // instead of creating a duplicate. Pinned note and pin state survive.
        if let Some(pos) = self.file.items.iter().position(|i| i.hash == item.hash) {
            let mut existing = self.file.items.remove(pos);
            existing.created_at = item.created_at;
            self.file.items.insert(0, existing);
            self.dirty = true;
            return None;
        }

        let id = self.file.next_id.max(1);
        self.file.next_id = id + 1;
        let mut item = item;
        item.id = id;

        if let Some(rgba) = png {
            let name = format!("{id}.png");
            match save_png(&self.dir.join(&name), item.width, item.height, &rgba) {
                Ok(()) => item.image = Some(name),
                Err(e) => {
                    self.error = Some(format!("保存图片失败：{e}"));
                    return None;
                }
            }
        }

        self.file.items.insert(0, item);
        self.trim(cfg);
        self.dirty = true;
        Some(id)
    }

    /// Enforce both budgets: item count and total image bytes. Pinned items are
    /// never dropped automatically — a pin means "I need this later".
    pub fn trim(&mut self, cfg: &dcx_island_config::ClipboardCfg) {
        if self.file.items.len() > cfg.max_items {
            let mut kept = Vec::with_capacity(self.file.items.len());
            for item in std::mem::take(&mut self.file.items) {
                if kept.len() >= cfg.max_items && !item.pinned {
                    self.delete_image(&item);
                    continue;
                }
                kept.push(item);
            }
            self.file.items = kept;
            self.dirty = true;
        }
        let budget = cfg.max_image_mb * 1024 * 1024;
        if budget == 0 {
            return;
        }
        let total: u64 = self
            .file
            .items
            .iter()
            .filter_map(|i| i.image.as_ref())
            .filter_map(|n| std::fs::metadata(self.dir.join(n)).ok())
            .map(|m| m.len())
            .sum();
        if total <= budget {
            return;
        }
        let mut over = total;
        let mut drop_ids = Vec::new();
        for item in self.file.items.iter().rev() {
            if over <= budget {
                break;
            }
            if item.pinned || item.image.is_none() {
                continue;
            }
            if let Some(name) = &item.image {
                if let Ok(meta) = std::fs::metadata(self.dir.join(name)) {
                    over = over.saturating_sub(meta.len());
                    drop_ids.push(item.id);
                }
            }
        }
        if !drop_ids.is_empty() {
            for item in self.file.items.iter().filter(|i| drop_ids.contains(&i.id)) {
                self.delete_image(item);
            }
            self.file.items.retain(|i| !drop_ids.contains(&i.id));
            self.dirty = true;
        }
    }

    fn delete_image(&self, item: &ClipItem) {
        if let Some(name) = &item.image {
            let _ = std::fs::remove_file(self.dir.join(name));
        }
    }

    pub fn remove(&mut self, id: u64) {
        if let Some(pos) = self.file.items.iter().position(|i| i.id == id) {
            let item = self.file.items.remove(pos);
            self.delete_image(&item);
            self.dirty = true;
        }
    }

    pub fn toggle_pin(&mut self, id: u64) {
        if let Some(item) = self.file.items.iter_mut().find(|i| i.id == id) {
            item.pinned = !item.pinned;
            self.dirty = true;
        }
    }

    pub fn set_note(&mut self, id: u64, note: String) {
        if let Some(item) = self.file.items.iter_mut().find(|i| i.id == id) {
            if item.note != note {
                item.note = note;
                self.dirty = true;
            }
        }
    }

    pub fn clear_unpinned(&mut self) {
        let items = std::mem::take(&mut self.file.items);
        let mut keep = Vec::with_capacity(items.len());
        for item in items {
            if item.pinned {
                keep.push(item);
            } else if let Some(name) = &item.image {
                let _ = std::fs::remove_file(self.dir.join(name));
            }
        }
        self.file.items = keep;
        self.dirty = true;
    }

    /// Copy an entry back to the clipboard. Returns false when the source file
    /// disappeared (the budget cleanup may have removed it).
    pub fn write_back(&mut self, id: u64) -> bool {
        let Some(item) = self.file.items.iter().find(|i| i.id == id).cloned() else {
            return false;
        };
        let ok = match item.kind {
            Kind::Text => sys::set_clipboard_text(&item.text),
            Kind::Image => match item.image.as_ref().and_then(|n| load_png(&self.dir.join(n))) {
                Some((rgba, w, h)) => match rgba_to_dib(&rgba, w, h) {
                    Some(dib) => sys::set_clipboard_dib(&dib),
                    None => false,
                },
                None => false,
            },
        };
        if ok {
            // Record our own sequence so the listener does not add a duplicate
            // row for a copy the user made from the history itself.
            self.self_write = Some(sys::clipboard_sequence());
        }
        ok
    }

    pub fn filtered(&self, query: &str) -> Vec<&ClipItem> {
        let q = query.trim().to_lowercase();
        self.file
            .items
            .iter()
            .filter(|i| {
                if q.is_empty() {
                    return true;
                }
                i.text.to_lowercase().contains(&q) || i.note.to_lowercase().contains(&q)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Capture thread
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum RawClip {
    Text(String),
    Image {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
    },
}

pub struct Listener {
    pub rx: Receiver<RawClip>,
    stop: std::sync::Arc<AtomicBool>,
    hwnd: std::sync::Arc<Mutex<isize>>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(h) = self.hwnd.lock() {
            if *h != 0 {
                unsafe { dcx_win32::PostMessageW(*h, WM_CLOSE, 0, 0) };
            }
        }
    }
}

const WM_CLOSE: u32 = 0x0010;
const WM_CLIPBOARDUPDATE: u32 = 0x031D;
const WM_DESTROY: u32 = 0x0002;
const HWND_MESSAGE: isize = -3;

/// Formats Windows itself uses to opt content out of clipboard history. A
/// password manager that sets `CanIncludeInClipboardHistory = 0` is honoured;
/// nothing else is guessed from the foreground window.
fn opt_out_format() -> u32 {
    let name = dcx_win32::wide("CanIncludeInClipboardHistory");
    unsafe { sys::register_clipboard_format(&name) }
}

static LISTENER_HWND: Mutex<isize> = Mutex::new(0);
static SINK: Mutex<Option<Sender<RawClip>>> = Mutex::new(None);

pub fn start(capture_images: bool) -> Option<Listener> {
    let (tx, rx) = channel();
    if let Ok(mut slot) = SINK.lock() {
        *slot = Some(tx);
    }
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let hwnd_slot = std::sync::Arc::new(Mutex::new(0isize));
    let stop2 = std::sync::Arc::clone(&stop);
    let hwnd2 = std::sync::Arc::clone(&hwnd_slot);

    std::thread::Builder::new()
        .name("dcx-island-clip".to_string())
        .spawn(move || unsafe {
            let class = dcx_win32::wide("DCX.Island.Clipboard");
            let cls = dcx_win32::WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(clip_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: dcx_win32::GetModuleHandleW(core::ptr::null()),
                hIcon: 0,
                hCursor: 0,
                hbrBackground: 0,
                lpszMenuName: core::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            dcx_win32::RegisterClassW(&cls);
            let title = dcx_win32::wide("DCX clipboard listener");
            let hwnd = dcx_win32::CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                0,
                0,
                core::ptr::null_mut(),
            );
            if hwnd == 0 {
                crate::win::trace("clipboard listener window could not be created");
                return;
            }
            if let Ok(mut h) = hwnd2.lock() {
                *h = hwnd;
            }
            if let Ok(mut h) = LISTENER_HWND.lock() {
                *h = hwnd;
            }
            CAPTURE_IMAGES.store(capture_images, Ordering::SeqCst);
            if !sys::clipboard_listener(hwnd, true) {
                crate::win::trace("AddClipboardFormatListener failed");
            }
            let mut msg = dcx_win32::MSG::default();
            while dcx_win32::GetMessageW(&mut msg, 0, 0, 0) > 0 {
                dcx_win32::TranslateMessage(&msg);
                dcx_win32::DispatchMessageW(&msg);
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
            }
            sys::clipboard_listener(hwnd, false);
            if let Ok(mut h) = LISTENER_HWND.lock() {
                *h = 0;
            }
        })
        .ok()?;

    Some(Listener {
        rx,
        stop,
        hwnd: hwnd_slot,
    })
}

/// Restart the listener after the user toggled image capture.
pub fn set_capture_images(on: bool) {
    CAPTURE_IMAGES.store(on, Ordering::SeqCst);
}

static CAPTURE_IMAGES: AtomicBool = AtomicBool::new(true);
static LAST_SEQ: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn clip_proc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    match msg {
        WM_CLIPBOARDUPDATE => {
            // Never do the conversion on this thread's message loop: reading a
            // large bitmap can take tens of milliseconds and would delay the
            // next update. The UI thread pulls from the channel.
            let seq = sys::clipboard_sequence();
            if seq != 0 && seq == LAST_SEQ.load(Ordering::SeqCst) {
                return 0;
            }
            LAST_SEQ.store(seq, Ordering::SeqCst);
            std::thread::spawn(move || capture_once());
            0
        }
        WM_CLOSE => {
            dcx_win32::DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            dcx_win32::PostQuitMessage(0);
            0
        }
        _ => unsafe { dcx_win32::DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn capture_once() {
    if opted_out() {
        return;
    }
    // Prefer PNG: it keeps alpha and is what browsers and Office publish.
    let png = sys::png_format();
    if CAPTURE_IMAGES.load(Ordering::SeqCst) && png != 0 && sys::clipboard_has(png) {
        if let Some(bytes) = sys::clipboard_bytes(png) {
            if let Ok(img) = image::load_from_memory(&bytes) {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                send(RawClip::Image {
                    rgba: rgba.into_raw(),
                    width: w,
                    height: h,
                });
                return;
            }
        }
    }
    if let Some(text) = sys::clipboard_text() {
        send(RawClip::Text(text));
        return;
    }
    if CAPTURE_IMAGES.load(Ordering::SeqCst) && sys::clipboard_has(sys::CF_DIB) {
        if let Some(dib) = sys::clipboard_bytes(sys::CF_DIB) {
            if let Some((rgba, w, h)) = dib_to_rgba(&dib) {
                send(RawClip::Image {
                    rgba,
                    width: w,
                    height: h,
                });
            }
        }
    }
}

fn send(raw: RawClip) {
    if let Ok(slot) = SINK.lock() {
        if let Some(tx) = slot.as_ref() {
            let _ = tx.send(raw);
        }
    }
}

fn opted_out() -> bool {
    let fmt = opt_out_format();
    if fmt == 0 || !sys::clipboard_has(fmt) {
        return false;
    }
    match sys::clipboard_bytes(fmt) {
        Some(bytes) if bytes.len() >= 4 => {
            let value = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            value == 0
        }
        // Present but unreadable: be conservative and keep the content out.
        Some(_) => true,
        None => false,
    }
}

// ---------------------------------------------------------------------------
// DIB ↔ RGBA
// ---------------------------------------------------------------------------

/// Convert a clipboard DIB to straight RGBA.
///
/// Only uncompressed 24- and 32-bit bitmaps are accepted. 32-bit DIBs from
/// screenshots almost always have a zero alpha channel (the format has no alpha
/// contract), so a fully transparent alpha plane is treated as opaque — the
/// alternative is an image that pastes back as an invisible rectangle.
pub fn dib_to_rgba(dib: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    if dib.len() < 40 {
        return None;
    }
    let header_size = u32::from_le_bytes(dib[0..4].try_into().ok()?) as usize;
    let width = i32::from_le_bytes(dib[4..8].try_into().ok()?);
    let height_raw = i32::from_le_bytes(dib[8..12].try_into().ok()?);
    let bpp = u16::from_le_bytes(dib[14..16].try_into().ok()?);
    let compression = u32::from_le_bytes(dib[16..20].try_into().ok()?);
    if width <= 0 || width > 16_384 || header_size < 40 {
        return None;
    }
    if compression != 0 {
        // BI_BITFIELDS (3) carries its own masks and is rare on the clipboard;
        // refusing it is better than emitting scrambled colours.
        return None;
    }
    if bpp != 24 && bpp != 32 {
        return None;
    }
    let top_down = height_raw < 0;
    let height = height_raw.unsigned_abs();
    if height == 0 || height > 16_384 {
        return None;
    }
    let bytes_pp = (bpp / 8) as usize;
    let stride = ((width as usize * bytes_pp + 3) / 4) * 4;
    let need = header_size + stride * height as usize;
    if dib.len() < need {
        return None;
    }
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    let mut any_alpha = false;
    for y in 0..height as usize {
        let src_row = if top_down {
            y
        } else {
            height as usize - 1 - y
        };
        let src = header_size + src_row * stride;
        for x in 0..width as usize {
            let i = src + x * bytes_pp;
            let b = dib[i];
            let g = dib[i + 1];
            let r = dib[i + 2];
            let a = if bytes_pp == 4 { dib[i + 3] } else { 255 };
            if a != 0 {
                any_alpha = true;
            }
            let o = (y * width as usize + x) * 4;
            rgba[o] = r;
            rgba[o + 1] = g;
            rgba[o + 2] = b;
            rgba[o + 3] = a;
        }
    }
    if bytes_pp == 4 && !any_alpha {
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
    }
    Some((rgba, width as u32, height))
}

/// Build a bottom-up 32-bit `BITMAPINFOHEADER` + pixels block for `CF_DIB`.
pub fn rgba_to_dib(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let px = width as usize * height as usize;
    if px == 0 || rgba.len() < px * 4 {
        return None;
    }
    let stride = width as usize * 4;
    let mut out = vec![0u8; 40 + stride * height as usize];
    out[0..4].copy_from_slice(&40u32.to_le_bytes());
    out[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    out[8..12].copy_from_slice(&(height as i32).to_le_bytes());
    out[12..14].copy_from_slice(&1u16.to_le_bytes()); // planes
    out[14..16].copy_from_slice(&32u16.to_le_bytes()); // bit count
    out[16..20].copy_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out[20..24].copy_from_slice(&((stride * height as usize) as u32).to_le_bytes());
    for y in 0..height as usize {
        let dst_row = height as usize - 1 - y;
        let dst = 40 + dst_row * stride;
        for x in 0..width as usize {
            let s = (y * width as usize + x) * 4;
            let d = dst + x * 4;
            out[d] = rgba[s + 2];
            out[d + 1] = rgba[s + 1];
            out[d + 2] = rgba[s];
            out[d + 3] = rgba[s + 3];
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// PNG on disk
// ---------------------------------------------------------------------------

fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let buf = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| "图片尺寸与像素数不符".to_string())?;
    buf.save_with_format(path, image::ImageFormat::Png)
        .map_err(|e| e.to_string())
}

fn load_png(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    let img = image::open(path).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some((img.into_raw(), w, h))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn is_excluded(text: &str, patterns: &[String]) -> bool {
    for pat in patterns {
        if pat.trim().is_empty() {
            continue;
        }
        if let Ok(re) = regex::Regex::new(pat) {
            if re.is_match(text) {
                return true;
            }
        }
    }
    false
}

fn read_file(path: &Path) -> ClipFile {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<ClipFile>(&text) {
            Ok(mut f) => {
                dcx_win32::config_file::note_parse_ok(path);
                if f.next_id == 0 {
                    f.next_id = f.items.iter().map(|i| i.id).max().unwrap_or(0) + 1;
                }
                f
            }
            Err(e) => {
                eprintln!("[dcx-island] clipboard index parse failed ({e})");
                dcx_win32::config_file::note_parse_error(path);
                ClipFile {
                    next_id: 1,
                    items: Vec::new(),
                }
            }
        },
        Err(_) => ClipFile {
            next_id: 1,
            items: Vec::new(),
        },
    }
}

fn write_file(path: &Path, file: &ClipFile) -> Result<(), String> {
    let _guard = dcx_win32::config_file::FileLock::acquire(
        path,
        dcx_win32::config_file::LOCK_TIMEOUT,
    )
    .map_err(|e| format!("剪贴板索引被占用：{e}"))?;
    if dcx_win32::config_file::is_poisoned(path) {
        return Err(dcx_win32::config_file::poisoned_error(path).to_string());
    }
    let text =
        serde_json::to_string_pretty(file).map_err(|e| format!("序列化剪贴板失败：{e}"))?;
    dcx_win32::config_file::write_atomic(path, text.as_bytes())
        .map_err(|e| format!("写入剪贴板失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> dcx_island_config::ClipboardCfg {
        dcx_island_config::ClipboardCfg {
            max_items: 3,
            max_image_mb: 1,
            capture_images: true,
            max_text_chars: 100,
            exclude_patterns: vec![r"(?i)password\s*:".to_string()],
            skip_password_fields: true,
            ..Default::default()
        }
    }

    fn store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "dcx-island-clip-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        Store::load(dir.join("index.json"), dir.join("img"))
    }

    #[test]
    fn identical_text_is_moved_not_duplicated() {
        let mut s = store("dup");
        s.push(RawClip::Text("hello".into()), &cfg());
        s.push(RawClip::Text("other".into()), &cfg());
        s.push(RawClip::Text("hello".into()), &cfg());
        assert_eq!(s.file.items.len(), 2);
        assert_eq!(s.file.items[0].text, "hello");
    }

    #[test]
    fn excluded_patterns_are_not_recorded() {
        let mut s = store("exclude");
        assert!(s
            .push(RawClip::Text("Password: hunter2".into()), &cfg())
            .is_none());
        assert!(s.file.items.is_empty());
    }

    #[test]
    fn the_list_is_trimmed_to_the_configured_length() {
        let mut s = store("trim");
        for i in 0..6 {
            s.push(RawClip::Text(format!("item {i}")), &cfg());
        }
        assert_eq!(s.file.items.len(), 3);
        assert_eq!(s.file.items[0].text, "item 5");
    }

    #[test]
    fn pinned_items_survive_trimming() {
        let mut s = store("pin");
        let first = s.push(RawClip::Text("keep me".into()), &cfg()).unwrap();
        s.toggle_pin(first);
        for i in 0..5 {
            s.push(RawClip::Text(format!("filler {i}")), &cfg());
        }
        assert!(s.file.items.iter().any(|i| i.id == first));
    }

    #[test]
    fn long_text_is_truncated_without_splitting_a_char() {
        let mut s = store("trunc");
        let long = "汉".repeat(500);
        s.push(RawClip::Text(long), &cfg());
        assert_eq!(s.file.items[0].text.chars().count(), 100);
    }

    #[test]
    fn a_dib_round_trips_through_rgba() {
        let (w, h) = (4u32, 3u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[(x * 60) as u8, (y * 80) as u8, 128, 255]);
            }
        }
        let dib = rgba_to_dib(&rgba, w, h).unwrap();
        let (back, bw, bh) = dib_to_rgba(&dib).unwrap();
        assert_eq!((bw, bh), (w, h));
        assert_eq!(back, rgba);
    }

    #[test]
    fn a_32_bit_dib_with_no_alpha_becomes_opaque() {
        let mut dib = vec![0u8; 40 + 4 * 1 * 2];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&2i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        // Two pixels with BGR set and A = 0.
        dib[40..44].copy_from_slice(&[10, 20, 30, 0]);
        dib[44..48].copy_from_slice(&[40, 50, 60, 0]);
        let (rgba, w, h) = dib_to_rgba(&dib).unwrap();
        assert_eq!((w, h), (1, 2));
        // A positive height means bottom-up: the first row in the file is the
        // *bottom* scanline, so the RGBA output starts with the second pixel.
        assert_eq!(rgba[0..4], [60, 50, 40, 255]);
        assert_eq!(rgba[4..8], [30, 20, 10, 255]);
    }

    #[test]
    fn a_bitfields_dib_is_refused_rather_than_scrambled() {
        let mut dib = vec![0u8; 40 + 16];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&2i32.to_le_bytes());
        dib[8..12].copy_from_slice(&2i32.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[16..20].copy_from_slice(&3u32.to_le_bytes());
        assert!(dib_to_rgba(&dib).is_none());
    }

    #[test]
    fn a_png_survives_a_save_and_load_round_trip() {
        let s = store("png");
        let rgba = vec![200u8, 10, 20, 255].repeat(9);
        save_png(&s.dir.join("t.png"), 3, 3, &rgba).unwrap();
        let (back, w, h) = load_png(&s.dir.join("t.png")).unwrap();
        assert_eq!((w, h), (3, 3));
        assert_eq!(back.len(), rgba.len());
    }
}
