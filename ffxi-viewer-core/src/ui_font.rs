use bevy::prelude::*;
use bevy::text::{Font, FontSource, TextFont};

// DejaVu Sans Mono (DejaVu license — see assets/fonts/DejaVu-LICENSE.txt). Bevy's
// built-in default is a FiraMono *subset* with no geometric shapes / arrows /
// symbols, so glyphs like ▶ render as tofu. This font covers them across the HUD.
pub const DEJAVU_SANS_MONO: &[u8] = include_bytes!("../assets/fonts/DejaVuSansMono.ttf");

#[derive(Resource, Default)]
pub struct UiFont(pub Handle<Font>);

pub fn load_ui_font(mut fonts: ResMut<Assets<Font>>, mut ui_font: ResMut<UiFont>) {
    // Parse errors surface later in the text pipeline under Parley; the
    // unit tests below gate the bundled bytes at build time instead.
    ui_font.0 = fonts.add(Font::from_bytes(DEJAVU_SANS_MONO.to_vec()));
}

pub fn apply_ui_font(ui_font: Res<UiFont>, mut q: Query<&mut TextFont, Added<TextFont>>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ab_glyph::{Font as _, FontArc};

    #[test]
    fn bundled_font_parses_for_ab_glyph() {
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
