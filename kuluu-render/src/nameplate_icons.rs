//! The retail icon glyphs the nameplate prefixes to a name, decoded from the
//! `font    fontshp ` shape group and kept as raw RGBA so they can be composited
//! straight into the billboard texture.
//!
//! research/XIClient/.../CXiActorNameDraw.cpp `GetActorNameGlyphData` — the
//! on-screen size of a glyph is its quad's vertex
//! span, not the texture crop, and the crop comes off a separate sheet.

use bevy::prelude::*;
use ffxi_dat::ui_element::{find_ui_element_group, ui_sprite, UiSprite};

use crate::nameplate_marker::FIRST_GLYPH_CODE;
use crate::ui_element_atlas::{read_ui_dats, UiElementDatRoot};

const FONT_SHAPE_GROUP: &str = "font    fontshp ";

/// One icon glyph: the pixels to sample and the on-screen box retail draws them
/// into, in the same glyph units as the name text.
pub struct IconGlyph {
    pub sprite: UiSprite,

    /// `GetActorNameGlyphData` — the drawn size is the quad's vertex span
    /// less one, in glyph units.
    pub width_units: f32,
    pub height_units: f32,

    /// `GetActorNameGlyphData` — the quad's top-left bound, which offsets
    /// the glyph against the text line.
    pub x_offset_units: f32,
    pub y_offset_units: f32,
}

/// The letter cell every icon is sized against. Retail measures both from the
/// same shape group and lays them out in one run, so the only honest way to
/// scale an icon against *our* font is the ratio of the two DAT boxes.
#[derive(Debug, Clone, Copy)]
pub struct LetterCell {
    pub width_units: f32,
    pub height_units: f32,
    pub y_offset_units: f32,
}

/// A capital letter — every A..Z shares one cell in the retail shape group, so
/// any of them measures it.
pub const REFERENCE_LETTER: u8 = b'A';

#[derive(Resource, Default)]
pub struct NameplateIcons {
    glyphs: std::collections::HashMap<u8, IconGlyph>,
    letter_cell: Option<LetterCell>,
    loaded: bool,
}

impl NameplateIcons {
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn get(&self, code: u8) -> Option<&IconGlyph> {
        self.glyphs.get(&code)
    }

    /// The retail letter cell the icon boxes are proportioned against.
    pub fn letter_cell(&self) -> Option<LetterCell> {
        self.letter_cell
    }

    pub fn load_from_dat(&mut self, dat_bytes: &[u8], codes: &[u8]) -> bool {
        let Some(group) = find_ui_element_group(dat_bytes, FONT_SHAPE_GROUP) else {
            return false;
        };
        let cell = |code: u8| -> Option<(f32, f32, f32, f32)> {
            let index = usize::from(code - FIRST_GLYPH_CODE);
            let component = group
                .elements
                .get(index)
                .and_then(|e| e.components.first())?;
            let [top_left, top_right, bottom_left, _] = component.positions;
            // `GetActorNameGlyphData` — the drawn box is the vertex span
            // less one.
            Some((
                f32::from(top_right.0 - top_left.0 - 1),
                f32::from(bottom_left.1 - top_left.1 - 1),
                f32::from(top_left.0),
                f32::from(top_left.1),
            ))
        };

        let Some((w, h, _, y)) = cell(REFERENCE_LETTER) else {
            return false;
        };
        self.letter_cell = Some(LetterCell {
            width_units: w,
            height_units: h,
            y_offset_units: y,
        });

        for &code in codes {
            let index = usize::from(code - FIRST_GLYPH_CODE);
            let Some((width_units, height_units, x_offset_units, y_offset_units)) = cell(code)
            else {
                continue;
            };
            let Some(sprite) = ui_sprite(dat_bytes, FONT_SHAPE_GROUP, index) else {
                continue;
            };
            self.glyphs.insert(
                code,
                IconGlyph {
                    sprite,
                    width_units,
                    height_units,
                    x_offset_units,
                    y_offset_units,
                },
            );
        }
        self.loaded = !self.glyphs.is_empty();
        self.loaded
    }
}

/// Every glyph `nameplate_marker` can emit. Loading is one pass over the shape
/// group, so the set is enumerated rather than faulted in per plate.
fn marker_codes() -> [u8; 24] {
    use crate::nameplate_marker::glyph::*;
    [
        PLAY_ONLINE,
        LINKDEAD,
        AWAY,
        SEEKING,
        LINKSHELL,
        GM_1,
        GM_4,
        GM_5,
        GM_6,
        GM_7,
        GM_8,
        BAZAAR,
        NEW_PLAYER,
        AUTO_PARTY,
        NATION_SAN_DORIA,
        NATION_BASTOK,
        NATION_WINDURST,
        NATION_BEAUFORT,
        NATION_RABANASTRE,
        BESIEGED_EVEN,
        BESIEGED_ODD,
        MONSTROSITY,
        JOB_MASTER,
        JOB_MASTER_TAIL,
    ]
}

pub fn load_nameplate_icons_system(
    mut icons: ResMut<NameplateIcons>,
    dat_root: Res<UiElementDatRoot>,
) {
    if dat_root.is_changed() {
        *icons = NameplateIcons::default();
    }
    if icons.is_loaded() {
        return;
    }
    let Some(root) = dat_root.0.as_ref() else {
        return;
    };
    let codes = marker_codes();
    for (id, bytes) in read_ui_dats(root) {
        if icons.load_from_dat(&bytes, &codes) {
            info!(
                glyphs = icons.glyphs.len(),
                dat = id,
                "loaded retail nameplate icon glyphs"
            );
            return;
        }
    }
}

pub struct NameplateIconsPlugin;

impl Plugin for NameplateIconsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NameplateIcons>()
            .add_systems(Update, load_nameplate_icons_system);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retail_dats() -> Vec<Vec<u8>> {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return Vec::new();
        };
        read_ui_dats(&root)
            .into_iter()
            .map(|(_, bytes)| bytes)
            .collect()
    }

    /// Gated on a retail install (self-skips). Every marker the selector can
    /// emit must resolve to a real glyph with a positive on-screen box.
    #[test]
    fn real_dat_resolves_every_marker_glyph() {
        let dats = retail_dats();
        if dats.is_empty() {
            return;
        }
        let codes = marker_codes();
        let mut icons = NameplateIcons::default();
        assert!(
            dats.iter().any(|b| icons.load_from_dat(b, &codes)),
            "the font shape group must resolve from the retail UI DATs"
        );

        for code in codes {
            let glyph = icons
                .get(code)
                .unwrap_or_else(|| panic!("marker glyph 0x{code:02X} missing"));
            assert!(
                glyph.width_units > 0.0 && glyph.height_units > 0.0,
                "marker glyph 0x{code:02X} has an empty box"
            );
            assert!(
                glyph.sprite.width > 0 && glyph.sprite.height > 0,
                "marker glyph 0x{code:02X} has no pixels"
            );
        }
    }

    /// The icons are noticeably taller than a text line — retail draws them at
    /// roughly two line heights off the status sheet.
    #[test]
    fn real_dat_icons_are_taller_than_a_text_line() {
        let dats = retail_dats();
        if dats.is_empty() {
            return;
        }
        let mut icons = NameplateIcons::default();
        let codes = marker_codes();
        assert!(dats.iter().any(|b| icons.load_from_dat(b, &codes)));

        let pearl = icons
            .get(crate::nameplate_marker::glyph::LINKSHELL)
            .unwrap();
        assert!(
            pearl.height_units > crate::nameplate_billboard::NAME_LINE_HEIGHT_UNITS,
            "pearl {} units vs an {} unit line",
            pearl.height_units,
            crate::nameplate_billboard::NAME_LINE_HEIGHT_UNITS
        );
    }
}
