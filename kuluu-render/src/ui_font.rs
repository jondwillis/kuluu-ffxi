use std::sync::OnceLock;

use ab_glyph::{Font as _, FontArc};
use bevy::prelude::*;
use bevy::text::{Font, FontSource, TextFont};

// DejaVu Sans Mono (DejaVu license — see assets/fonts/DejaVu-LICENSE.txt). Bevy's
// built-in default is a FiraMono *subset* with no geometric shapes / arrows /
// symbols, so glyphs like ▶ render as tofu. This font covers them across the HUD.
pub const DEJAVU_SANS_MONO: &[u8] = include_bytes!("../assets/fonts/DejaVuSansMono.ttf");
/// The same family's oblique cut — real slanted outlines, not a rotated text box (owner
/// ruling 2026-10-19 on the first attempt at an italic caption: "that is NOT italics.. you
/// just slanted the whole word"). Byte-identical family to `DEJAVU_SANS_MONO` (both are
/// upstream dejavu-fonts 2.37 releases; see `oblique_face_keeps_the_regular_advances`), so
/// every width the HUD measures from the regular face still fits its glyphs.
pub const DEJAVU_SANS_MONO_OBLIQUE: &[u8] =
    include_bytes!("../assets/fonts/DejaVuSansMono-Oblique.ttf");

#[derive(Resource, Default)]
pub struct UiFont(pub Handle<Font>);

/// The italic handle for the one HUD caption that asks for it. Deliberately separate from
/// `UiFont`: `apply_ui_font` rewrites every `TextFont` it sees to the regular face, so an
/// italic has to be requested explicitly and defended from that system.
#[derive(Resource, Default)]
pub struct UiFontItalic(pub Handle<Font>);

// Fonts load in Startup, before any HUD spawns: a `TextFont` naming a handle is taken at
// spawn time, so the handles must exist by then (the `Added<TextFont>` rewrites only ever
// patch the face, never the choice to use an explicit one).
pub fn load_ui_fonts(
    mut fonts: ResMut<Assets<Font>>,
    mut ui_font: ResMut<UiFont>,
    mut italic: ResMut<UiFontItalic>,
) {
    // Parse errors surface later in the text pipeline under Parley; the
    // unit tests below gate the bundled bytes at build time instead.
    ui_font.0 = fonts.add(Font::from_bytes(DEJAVU_SANS_MONO.to_vec()));
    italic.0 = fonts.add(Font::from_bytes(DEJAVU_SANS_MONO_OBLIQUE.to_vec()));
}

/// Marks text that chose its own face: `apply_ui_font` skips it. Needed because that system
/// rewrites every new `TextFont` to the regular HUD face, which would otherwise undo a
/// caption asking for a different cut of the family (the Status panel's oblique).
#[derive(Component)]
pub struct KeepOwnFont;

pub fn apply_ui_font(
    ui_font: Res<UiFont>,
    mut q: Query<&mut TextFont, (Added<TextFont>, Without<KeepOwnFont>)>,
) {
    if ui_font.0 == Handle::default() {
        return;
    }
    let ours = FontSource::Handle(ui_font.0.clone());
    for mut text_font in &mut q {
        if text_font.font != ours {
            text_font.font = ours.clone();
        }
    }
}

struct MonoMetrics {
    advance_em: f32,
    line_em: f32,
}

fn metrics() -> &'static MonoMetrics {
    static METRICS: OnceLock<MonoMetrics> = OnceLock::new();
    METRICS.get_or_init(|| {
        let font = FontArc::try_from_slice(DEJAVU_SANS_MONO)
            .expect("bundled DejaVuSansMono.ttf must parse as a valid TTF for ab_glyph");
        let upem = font
            .units_per_em()
            .expect("bundled DejaVuSansMono.ttf must declare units_per_em");
        MonoMetrics {
            advance_em: font.h_advance_unscaled(font.glyph_id('0')) / upem,
            line_em: font.height_unscaled() / upem,
        }
    })
}

/// Laid-out width of `text` at `font_size`, read from the bundled font's own
/// metrics. Exact rather than estimated because the HUD font is monospace, which
/// lets layout reserve room for a label's worst case (a countdown's longest
/// string, say) without a text-pipeline round trip.
pub fn text_width_px(text: &str, font_size: f32) -> f32 {
    text.chars().count() as f32 * metrics().advance_em * font_size
}

/// Height of one laid-out line at `font_size` — what a caller must clear to sit
/// something below a single-line label.
pub fn line_height_px(font_size: f32) -> f32 {
    metrics().line_em * font_size
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_glyph::FontArc;

    #[test]
    fn bundled_font_parses_for_ab_glyph() {
        assert!(FontArc::try_from_slice(DEJAVU_SANS_MONO).is_ok());
    }

    // text_width_px multiplies one advance by the char count, which only holds
    // while the bundled font is monospace across every glyph the HUD measures.
    #[test]
    fn bundled_font_is_monospace_over_hud_glyphs() {
        let font = FontArc::try_from_slice(DEJAVU_SANS_MONO).expect("valid ttf");
        let upem = font.units_per_em().expect("units_per_em");
        let want = font.h_advance_unscaled(font.glyph_id('0')) / upem;
        for ch in ['0', '9', ':', 'h', 'm', 's', 'W', ' '] {
            let got = font.h_advance_unscaled(font.glyph_id(ch)) / upem;
            assert!(
                (got - want).abs() < f32::EPSILON,
                "'{ch}' advances {got} em, not {want} em"
            );
        }
    }

    #[test]
    fn text_width_scales_with_length_and_size() {
        let one = text_width_px("0", 10.0);
        assert!(one > 0.0);
        assert!((text_width_px("00000", 10.0) - one * 5.0).abs() < 1e-3);
        assert!((text_width_px("0", 20.0) - one * 2.0).abs() < 1e-3);
        assert!(line_height_px(10.0) > 10.0, "line box exceeds the em size");
    }

    // The HUD measures every string from the regular face (`text_width_px`), including the
    // caption drawn in the oblique. That is only sound while the two cuts share advance and
    // line height — which they do upstream, and this catches it if a font refresh breaks it.
    #[test]
    fn oblique_face_keeps_the_regular_advances() {
        let regular = FontArc::try_from_slice(DEJAVU_SANS_MONO).expect("regular ttf parses");
        let oblique =
            FontArc::try_from_slice(DEJAVU_SANS_MONO_OBLIQUE).expect("oblique ttf parses");
        for (label, font) in [("regular", &regular), ("oblique", &oblique)] {
            assert!(
                font.units_per_em().is_some(),
                "{label} declares units_per_em"
            );
        }
        let upem = regular.units_per_em().expect("units_per_em");
        assert_eq!(
            oblique.units_per_em().expect("units_per_em"),
            upem,
            "same em square"
        );
        for ch in ['S', 't', 'a', 'u', 's', '0', ':', 'W', ' '] {
            assert_eq!(
                oblique.h_advance_unscaled(oblique.glyph_id(ch)),
                regular.h_advance_unscaled(regular.glyph_id(ch)),
                "'{ch}' advances differently in the oblique cut"
            );
        }
        assert_eq!(
            oblique.height_unscaled(),
            regular.height_unscaled(),
            "same line height, so the caption's mask row and pitch stay valid"
        );
    }

    #[test]
    fn bundled_font_covers_hud_glyphs_that_tofu_in_firamono() {
        let font = FontArc::try_from_slice(DEJAVU_SANS_MONO).expect("valid ttf");
        // glyph_id 0 is .notdef (tofu). These render as [] in Bevy's default
        // FiraMono subset; the whole point of vendoring DejaVu is to cover them.
        for ch in ['▶', '▸', '»', '→', '↑', '↓', '◄', '►'] {
            assert_ne!(
                font.glyph_id(ch).0,
                0,
                "DejaVu Sans Mono must cover U+{:04X} ({ch})",
                ch as u32
            );
        }
    }
}
