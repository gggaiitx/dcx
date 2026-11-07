//! Colours and fonts for the island.
//!
//! The island is translucent by design, so a palette cannot be derived from
//! egui's `Visuals` alone: the capsule needs a *surface* colour with alpha, a
//! hairline stroke that survives on both bright and dark wallpapers, and text
//! that stays readable when the wallpaper is a photo. Two palettes are defined
//! here and chosen from the same registry flag the rest of DCX follows
//! (`apps_use_light_theme`), so the island matches Explorer and the settings
//! centre instead of guessing.

use egui::Color32;

const APP_ICON: &[u8] = include_bytes!("../../../../brand/embed/island-64.rgba");
const APP_ICON_PX: u32 = 64;

pub fn app_icon() -> egui::IconData {
    egui::IconData {
        rgba: APP_ICON.to_vec(),
        width: APP_ICON_PX,
        height: APP_ICON_PX,
    }
}

/// egui ships no CJK glyphs and every label here is Chinese.
///
/// The font file is memory-mapped and leaked as `'static`: `FontData::from_owned`
/// would keep three private copies of a 19 MB collection alive (epaint clones
/// the definitions when building `Fonts` and `ab_glyph` copies again while
/// parsing). A borrowed slice clones as a pointer.
pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let candidates = [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
        r"C:\Windows\Fonts\malgun.ttf",
    ];
    let loaded = candidates.iter().find_map(|p| load_static(p));
    if let Some(bytes) = loaded {
        fonts
            .font_data
            .insert("cjk".to_string(), egui::FontData::from_static(bytes));
        // Proportional: CJK first, so Chinese labels use the system face.
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "cjk".to_string());
        // Monospace: CJK **last**. The clock and every numeric readout use the
        // monospace family for tabular digits, and putting YaHei in front would
        // replace them with proportional digits that make the clock jitter.
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("cjk".to_string());
    } else {
        crate::win::trace("no system CJK font found; labels will render as boxes");
    }
    ctx.set_fonts(fonts);
}

fn load_static(path: &str) -> Option<&'static [u8]> {
    if let Ok(file) = std::fs::File::open(path) {
        if let Ok(map) = unsafe { memmap2::Mmap::map(&file) } {
            let map: &'static memmap2::Mmap = Box::leak(Box::new(map));
            return Some(map.as_ref());
        }
    }
    let bytes = std::fs::read(path).ok()?;
    Some(Box::leak(bytes.into_boxed_slice()))
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub light: bool,
    /// Capsule / panel background with alpha.
    pub surface: Color32,
    /// A slightly stronger surface for selected rows and inputs.
    pub surface_alt: Color32,
    pub stroke: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
    /// Track behind bars and rings.
    pub track: Color32,
    /// Window shadow, layered as a few translucent rounded rectangles.
    pub shadow: Color32,
    pub divider: Color32,
}

impl Palette {
    /// The user's theme choice: `"system"` follows Windows, `"light"` and
    /// `"dark"` pin the island's own appearance. Only the island is themed;
    /// the settings window keeps following Windows.
    pub fn for_theme(theme: &str) -> Palette {
        let light = match theme {
            "light" => true,
            "dark" => false,
            _ => dcx_win32::apps_use_light_theme(),
        };
        if light {
            Palette::light()
        } else {
            Palette::dark()
        }
    }

    fn light() -> Palette {
        Palette {
            light: true,
            // Fully opaque: the panel is a solid card. It used to be 95 %
            // (`0xF2`) and a wallpaper with any colour in it showed through,
            // which reads as "the island still has a transparent
            // background". The translucent look belongs to the capsule
            // only — that is what the opacity setting says it controls.
            surface: Color32::from_rgba_unmultiplied(0xFA, 0xFA, 0xFC, 0xFF),
            surface_alt: Color32::from_rgba_unmultiplied(0xEE, 0xEE, 0xF4, 0xFF),
            stroke: Color32::from_rgba_unmultiplied(0x1A, 0x1A, 0x22, 0x1F),
            text: Color32::from_rgb(0x1B, 0x1B, 0x22),
            text_dim: Color32::from_rgb(0x5E, 0x5E, 0x6B),
            text_faint: Color32::from_rgb(0x8E, 0x8E, 0x9B),
            accent: Color32::from_rgb(0x50, 0x36, 0xE2),
            accent_soft: Color32::from_rgba_unmultiplied(0x50, 0x36, 0xE2, 0x1F),
            ok: Color32::from_rgb(0x18, 0x9A, 0x4A),
            warn: Color32::from_rgb(0xC0, 0x77, 0x00),
            danger: Color32::from_rgb(0xC4, 0x2B, 0x1C),
            track: Color32::from_rgba_unmultiplied(0x1A, 0x1A, 0x22, 0x14),
            shadow: Color32::from_rgba_unmultiplied(0x1A, 0x1A, 0x22, 0x24),
            divider: Color32::from_rgba_unmultiplied(0x1A, 0x1A, 0x22, 0x12),
        }
    }

    fn dark() -> Palette {
        Palette {
            light: false,
            // Opaque for the same reason as the light palette.
            surface: Color32::from_rgba_unmultiplied(0x1C, 0x1C, 0x22, 0xFF),
            surface_alt: Color32::from_rgba_unmultiplied(0x2A, 0x2A, 0x33, 0xFF),
            // A white hairline at 12 % is what the light theme's dark hairline
            // becomes at the same *perceived* strength — but white on dark reads
            // far stronger: the card's edge and the field's border looked like
            // drawn-on lines. Win11's dark surfaces use ~6 % white; this is that.
            stroke: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x10),
            text: Color32::from_rgb(0xF2, 0xF2, 0xF6),
            text_dim: Color32::from_rgb(0xB4, 0xB4, 0xC0),
            text_faint: Color32::from_rgb(0x82, 0x82, 0x8E),
            accent: Color32::from_rgb(0x92, 0x82, 0xFF),
            accent_soft: Color32::from_rgba_unmultiplied(0x92, 0x82, 0xFF, 0x12),
            ok: Color32::from_rgb(0x4A, 0xCF, 0x7E),
            warn: Color32::from_rgb(0xE8, 0xB0, 0x4B),
            danger: Color32::from_rgb(0xF0, 0x6A, 0x5C),
            // This window blends its surface in linear space, so a white overlay
            // lands much closer to white than its alpha suggests: 11 % rendered
            // as a pale block (the two "bar cylinders"). The dark palette's
            // translucent whites are therefore roughly a quarter of the light
            // palette's, which is what makes them read as hairlines.
            track: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x04),
            shadow: Color32::from_rgba_unmultiplied(0x00, 0x00, 0x00, 0x50),
            // Separators and the search field's outline: one step below the
            // card edge, as in the light palette.
            divider: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x02),
        }
    }

    /// Colour for a 0–100 percentage.
    pub fn level(&self, level: u8) -> Color32 {
        match level {
            2 => self.danger,
            1 => self.warn,
            _ => self.accent,
        }
    }
}
