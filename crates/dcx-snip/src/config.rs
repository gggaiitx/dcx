//! `%LOCALAPPDATA%\dcx\snip.toml` — read-only on this side.
//!
//! The settings centre in `dcx-host.exe` owns the document: it writes with
//! `config_file::FileLock` + `write_atomic` and refuses to overwrite a file it
//! could not parse. This process therefore **only reads**, and a malformed
//! document must fall back to the defaults without ever writing back — the same
//! rule the box and search apps follow.

use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub activation: Activation,
    pub output: Output,
    pub draw: Draw,
    pub ui: Ui,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Activation {
    /// Global hotkey, registered by `dcx-host`, shown here for the hint text.
    pub hotkey: String,
    /// Seconds to stay alive after a session when `apps.toml` says disabled.
    /// `0` keeps the process resident until it is closed.
    pub idle_exit_seconds: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Output {
    /// `png` or `jpeg`.
    pub format: String,
    /// Empty = `%USERPROFILE%\Pictures\DCX 截图`.
    pub dir: String,
    pub jpeg_quality: u8,
    /// Corner radius of the exported image, in physical pixels. `0` = square.
    pub corner_radius: u32,
    /// Copy to the clipboard as soon as `Enter` / 复制 is pressed (always) and
    /// also right after 保存 (`false` by default: saving twice is annoying).
    pub copy_after_save: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Draw {
    /// `#RRGGBB`.
    pub color: String,
    pub width: f32,
    pub font_size: f32,
    /// GDI family name for new text annotations (`Microsoft YaHei UI`,
    /// `SimHei`, …). Empty = the built-in default.
    pub font: String,
    pub bold: bool,
    pub italic: bool,
    /// Hard drop shadow behind text annotations.
    pub shadow: bool,
    /// Corner radius of the rectangle tool, in physical pixels.
    pub radius: f32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Ui {
    /// `0` = follow the DPI of the monitor under the cursor.
    pub scale: f32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            activation: Activation::default(),
            output: Output::default(),
            draw: Draw::default(),
            ui: Ui::default(),
        }
    }
}

impl Default for Activation {
    fn default() -> Self {
        Activation {
            hotkey: "Alt+j".to_string(),
            idle_exit_seconds: 30,
        }
    }
}

impl Default for Output {
    fn default() -> Self {
        Output {
            format: "png".to_string(),
            dir: String::new(),
            jpeg_quality: 92,
            corner_radius: 0,
            copy_after_save: false,
        }
    }
}

impl Default for Draw {
    fn default() -> Self {
        Draw {
            color: "#E5484D".to_string(),
            width: 3.0,
            font_size: 20.0,
            font: dcx_win32::font_catalog::DEFAULT_FACE.to_string(),
            bold: false,
            italic: false,
            shadow: false,
            radius: 0.0,
        }
    }
}

impl Default for Ui {
    fn default() -> Self {
        Ui { scale: 0.0 }
    }
}

pub fn local_dir() -> PathBuf {
    let mut p = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    p.push("dcx");
    let _ = std::fs::create_dir_all(&p);
    p
}

pub fn config_path() -> PathBuf {
    local_dir().join("snip.toml")
}

pub fn load() -> Config {
    let path = config_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Config::default();
    };
    match toml::from_str(&text) {
        Ok(c) => c,
        Err(e) => {
            // Reported, never repaired: the host owns this document and a
            // half-understood file must not be rewritten from defaults.
            eprintln!("[dcx-snip] parse {path:?} failed ({e}); using defaults");
            Config::default()
        }
    }
}

impl Config {
    /// Output directory, created on demand.
    pub fn output_dir(&self) -> PathBuf {
        let raw = self.output.dir.trim();
        if raw.is_empty() {
            let mut p = std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            p.push("Pictures");
            p.push("DCX 截图");
            return p;
        }
        PathBuf::from(raw)
    }

    pub fn jpeg(&self) -> bool {
        self.output.format.eq_ignore_ascii_case("jpeg")
            || self.output.format.eq_ignore_ascii_case("jpg")
    }

    /// `#RRGGBB` → (r, g, b). Unknown values fall back to the default red so a
    /// typo in the TOML cannot make annotations invisible.
    pub fn rgb(&self) -> [u8; 3] {
        parse_hex(&self.draw.color).unwrap_or([0xE5, 0x48, 0x4D])
    }

    /// Starting text style for a new session. Blank or unknown families fall
    /// back to the built-in one: GDI would substitute silently anyway, but an
    /// empty string in the toolbar label would look like a bug.
    pub fn text_style(&self) -> crate::model::TextStyle {
        let face = self.draw.font.trim();
        crate::model::TextStyle {
            face: if face.is_empty() {
                dcx_win32::font_catalog::DEFAULT_FACE.to_string()
            } else {
                face.to_string()
            },
            bold: self.draw.bold,
            italic: self.draw.italic,
            shadow: self.draw.shadow,
        }
    }
}

pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let t = s.trim().trim_start_matches('#');
    if t.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(t, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}
