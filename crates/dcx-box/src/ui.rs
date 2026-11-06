//! UI basics owned by `dcx-box.exe`: CJK fonts and this application's own icon.
//!
//! Deliberately a private copy rather than a re-export from `dcx-host`: the box
//! is a standalone binary and must not reference the host's settings module
//! (that was one of the coupling points the split removed). Both sides use the
//! same Win11-style typography, but neither links against the other.

const APP_ICON: &[u8] = include_bytes!("../../../brand/embed/box-64.rgba");
const APP_ICON_PX: u32 = 64;

/// The window icon, embedded as raw RGBA at build time by `tools/gen_brand.py`.
/// 64 px is what the title bar and the Alt-Tab thumbnail sample from.
pub fn app_icon() -> egui::IconData {
    egui::IconData {
        rgba: APP_ICON.to_vec(),
        width: APP_ICON_PX,
        height: APP_ICON_PX,
    }
}

/// The same icon as a texture for the taskbar tile, so the slot shows the
/// application's real identity instead of a hand-drawn glyph.
pub fn app_icon_image() -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([APP_ICON_PX as usize, APP_ICON_PX as usize], APP_ICON)
}

/// Load a system CJK font into egui so Chinese labels render. egui's bundled
/// fonts have no CJK coverage and every tab label here is Chinese.
///
/// The bytes are handed to epaint as `'static` slices on purpose. `FontData`
/// owns a `Cow`, and epaint clones the definitions when it builds a `Fonts`
/// and again when `ab_glyph` parses them — with `from_owned` that was three
/// full copies of a 19 MB collection (~56 MB) for the life of the process.
/// A borrowed slice clones as a reference only, and a memory map means the
/// glyph data stays file-backed and pageable instead of counting as private
/// memory.
pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // `msyh.ttc` is the Microsoft YaHei collection on every Win10/11 box;
    // index 0 is the regular weight.
    let candidates = [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
        r"C:\Windows\Fonts\malgun.ttf",
    ];
    let loaded = candidates.iter().find_map(|p| load_static(p));
    if let Some(bytes) = loaded {
        fonts.font_data.insert(
            "cjk".to_string(),
            egui::FontData::from_static(bytes),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, "cjk".to_string());
        }
    } else {
        eprintln!("[dcx-box] no system CJK font found; Chinese labels will show as boxes");
    }
    ctx.set_fonts(fonts);
}

/// Map a font file for the process lifetime. Falls back to a read (also kept
/// for the process lifetime) if the map fails, so the font is still loaded.
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
