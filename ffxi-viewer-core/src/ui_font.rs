use bevy::prelude::*;
use bevy::text::{Font, TextFont};

// DejaVu Sans Mono (DejaVu license — see assets/fonts/DejaVu-LICENSE.txt). Bevy's
// built-in default is a FiraMono *subset* with no geometric shapes / arrows /
// symbols, so glyphs like ▶ render as tofu. This font covers them across the HUD.
pub const DEJAVU_SANS_MONO: &[u8] = include_bytes!("../assets/fonts/DejaVuSansMono.ttf");

#[derive(Resource, Default)]
pub struct UiFont(pub Handle<Font>);

pub fn load_ui_font(mut fonts: ResMut<Assets<Font>>, mut ui_font: ResMut<UiFont>) {
    match Font::try_from_bytes(DEJAVU_SANS_MONO.to_vec()) {
        Ok(font) => ui_font.0 = fonts.add(font),
        Err(e) => error!("bundled UI font DejaVuSansMono.ttf failed to parse: {e}"),
    }
}

pub fn apply_ui_font(ui_font: Res<UiFont>, mut q: Query<&mut TextFont, Added<TextFont>>) {
    if ui_font.0 == Handle::default() {
        return;
    }
    for mut text_font in &mut q {
        if text_font.font != ui_font.0 {
            text_font.font = ui_font.0.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_glyph::{Font as _, FontArc};

    #[test]
    fn bundled_font_parses_for_bevy_and_ab_glyph() {
        assert!(Font::try_from_bytes(DEJAVU_SANS_MONO.to_vec()).is_ok());
        assert!(FontArc::try_from_slice(DEJAVU_SANS_MONO).is_ok());
    }

    #[test]
    fn bundled_font_covers_hud_glyphs_that_tofu_in_firamono() {
        let font = FontArc::try_from_slice(DEJAVU_SANS_MONO).expect("valid ttf");
        // glyph_id 0 is .notdef (tofu). These render as [] in Bevy's default
        // FiraMono subset; the whole point of vendoring DejaVu is to cover them.
        for ch in ['▶', '▸', '»', '→', '↑', '↓'] {
            assert_ne!(
                font.glyph_id(ch).0,
                0,
                "DejaVu Sans Mono must cover U+{:04X} ({ch})",
                ch as u32
            );
        }
    }
}
