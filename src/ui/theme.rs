// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use std::sync::{Arc, OnceLock};

use egui::{Color32, FontData, FontDefinitions, FontFamily, Id};

/// Apply custom dark theme to the egui context.
///
/// This function is called every frame by the app's `update()` method.
/// egui internally deduplicates identical `set_visuals()` calls (it checks
/// if the new visuals differ from the current ones before marking the UI
/// as needing a repaint), so the per-frame overhead is just a comparison
/// of the Visuals struct — no heap allocation or GPU work.
///
/// `set_fonts()` is NOT called every frame because egui's internal
/// deduplication for fonts involves an expensive TTF binary comparison.
/// Instead, fonts are set once per context using a per-context flag stored
/// in `ctx.memory()`. This correctly handles multiple contexts (e.g. in
/// tests) unlike a global OnceLock would.
pub fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.window_fill = WINDOW_FILL;
    visuals.panel_fill = PANEL_FILL;
    visuals.extreme_bg_color = EXTREME_BG;
    visuals.widgets.noninteractive.bg_fill = WINDOW_FILL;
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(42, 42, 42);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(62, 62, 62);
    visuals.widgets.active.bg_fill = ACTIVE_BG;
    visuals.selection.bg_fill = SELECTION_BG;
    visuals.selection.stroke.color = Color32::from_rgb(255, 255, 255);
    visuals.override_text_color = Some(TEXT_COLOR);
    ctx.set_visuals(visuals);

    // Only set fonts once per context to avoid expensive TTF binary comparison
    // in egui's set_fonts() deduplication check. We use a per-context flag
    // via ctx.memory() so that each new context gets fonts installed exactly
    // once (unlike a global OnceLock which would prevent fonts from being set
    // on any context created after the first one).
    let fonts_installed = ctx
        .memory(|mem| {
            mem.data
                .get_temp::<bool>(Id::from("nitrosense_fonts_installed"))
        })
        .unwrap_or(false);
    if !fonts_installed {
        ctx.set_fonts(fonts().clone());
        ctx.memory_mut(|mem| {
            mem.data
                .insert_temp(Id::from("nitrosense_fonts_installed"), true)
        });
    }
}

/// Theme color constants (exported for testing)
pub const WINDOW_FILL: Color32 = Color32::from_rgb(53, 53, 53);
pub const PANEL_FILL: Color32 = Color32::from_rgb(37, 37, 37);
pub const EXTREME_BG: Color32 = Color32::from_rgb(28, 28, 28);
pub const ACTIVE_BG: Color32 = Color32::from_rgb(42, 130, 218);
pub const SELECTION_BG: Color32 = Color32::from_rgb(42, 130, 218);
pub const TEXT_COLOR: Color32 = Color32::from_rgb(230, 230, 230);

fn fonts() -> &'static FontDefinitions {
    static FONTS: OnceLock<FontDefinitions> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut fonts = FontDefinitions::default();
        for (name, bytes) in embedded_square_fonts() {
            fonts
                .font_data
                .insert((*name).to_owned(), Arc::new(FontData::from_static(bytes)));
        }
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "TT Squares".to_owned());
        fonts
    })
}

fn embedded_square_fonts() -> &'static [(&'static str, &'static [u8])] {
    &[
        (
            "TT Squares",
            include_bytes!("../../assets/fonts/Squares Regular.otf"),
        ),
        (
            "TT Squares Bold",
            include_bytes!("../../assets/fonts/Squares Bold.otf"),
        ),
        (
            "TT Squares Bold Italic",
            include_bytes!("../../assets/fonts/Squares Bold Italic.otf"),
        ),
        (
            "TT Squares Italic",
            include_bytes!("../../assets/fonts/Squares Italic.otf"),
        ),
        (
            "TT Squares Black",
            include_bytes!("../../assets/fonts/Squares Black.otf"),
        ),
        (
            "TT Squares Black Italic",
            include_bytes!("../../assets/fonts/Squares Black Italic.otf"),
        ),
        (
            "TT Squares Light",
            include_bytes!("../../assets/fonts/Squares Light.otf"),
        ),
        (
            "TT Squares Light Italic",
            include_bytes!("../../assets/fonts/Squares Light italic.otf"),
        ),
        (
            "TT Squares Thin",
            include_bytes!("../../assets/fonts/Squares Thin.otf"),
        ),
        (
            "TT Squares Thin Italic",
            include_bytes!("../../assets/fonts/Squares Thin Italic.otf"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── apply_theme integration tests ────────────────────────────────────────
    // These create a real egui::Context, invoke apply_theme, then read back the
    // visuals to verify that the function actually wrote the expected values.

    fn themed_visuals() -> egui::Visuals {
        let ctx = egui::Context::default();
        apply_theme(&ctx);
        ctx.style().visuals.clone()
    }

    #[test]
    fn apply_theme_sets_window_fill() {
        assert_eq!(themed_visuals().window_fill, WINDOW_FILL);
    }

    #[test]
    fn apply_theme_sets_panel_fill() {
        assert_eq!(themed_visuals().panel_fill, PANEL_FILL);
    }

    #[test]
    fn apply_theme_sets_extreme_bg() {
        assert_eq!(themed_visuals().extreme_bg_color, EXTREME_BG);
    }

    #[test]
    fn apply_theme_sets_active_bg() {
        assert_eq!(themed_visuals().widgets.active.bg_fill, ACTIVE_BG);
    }

    #[test]
    fn apply_theme_sets_selection_bg() {
        assert_eq!(themed_visuals().selection.bg_fill, SELECTION_BG);
    }

    #[test]
    fn apply_theme_sets_text_color() {
        assert_eq!(themed_visuals().override_text_color, Some(TEXT_COLOR),);
    }

    // ── palette sanity checks ─────────────────────────────────────────────────
    // Verify the palette is actually dark (background colors darker than text).

    #[test]
    fn palette_backgrounds_are_darker_than_text() {
        let text_luma = luma(TEXT_COLOR);
        assert!(
            luma(WINDOW_FILL) < text_luma,
            "WINDOW_FILL should be darker than text"
        );
        assert!(
            luma(PANEL_FILL) < text_luma,
            "PANEL_FILL should be darker than text"
        );
        assert!(
            luma(EXTREME_BG) < text_luma,
            "EXTREME_BG should be darker than text"
        );
    }

    // ── font tests ────────────────────────────────────────────────────────────

    #[test]
    fn theme_font_data_contains_tt_squares() {
        let defs = fonts();
        assert!(defs.font_data.contains_key("TT Squares"));
    }

    #[test]
    fn theme_embeds_all_tt_squares_variants() {
        let defs = fonts();
        for (name, bytes) in embedded_square_fonts() {
            let font = defs.font_data.get(*name).unwrap_or_else(|| {
                panic!("expected embedded font variant {name}");
            });
            assert_eq!(font.font.len(), bytes.len());
        }
    }

    #[test]
    fn theme_proportional_family_starts_with_tt_squares() {
        let defs = fonts();
        let proportional = defs.families.get(&FontFamily::Proportional).unwrap();
        assert_eq!(proportional.first().unwrap(), "TT Squares");
    }

    // ── helper ────────────────────────────────────────────────────────────────

    /// Perceived luminance approximation (sum of weighted channels).
    fn luma(c: Color32) -> u32 {
        299 * c.r() as u32 + 587 * c.g() as u32 + 114 * c.b() as u32
    }
}
