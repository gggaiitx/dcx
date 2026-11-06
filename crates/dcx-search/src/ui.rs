//! UI basics: the app's own icon and system CJK fonts.
//!
//! Deliberately a private copy (same as `dcx-box/src/ui.rs`): a standalone app
//! must not link the host's settings module. Same typography, no coupling.

const APP_ICON: &[u8] = include_bytes!("../../../brand/embed/search-64.rgba");
const APP_ICON_PX: u32 = 64;

pub fn app_icon() -> egui::IconData {
    egui::IconData {
        rgba: APP_ICON.to_vec(),
        width: APP_ICON_PX,
        height: APP_ICON_PX,
    }
}

/// egui ships no CJK glyphs; every label here is Chinese.
pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
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
        crate::win::trace("no system CJK font found; Chinese labels will show as boxes");
    }
    ctx.set_fonts(fonts);
}
