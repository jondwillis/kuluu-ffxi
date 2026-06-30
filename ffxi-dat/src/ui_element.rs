//! UI-element groups (DAT section kind 0x31) and sprite extraction.
//! Re-expresses research/xim/.../resource/UiElementSection.kt; the menu UI DATs
//! address sprites by (group-name, index) — e.g. the Vana'diel clock day orb is
//! group "menu    frames  ", index 106 + element.

use crate::chunk::walk;
use crate::texture::{decode_texture, DecodedTexture};

pub const UI_ELEMENT_GROUP_KIND: u8 = 0x31;
pub const TEXTURE_KIND: u8 = 0x20;

const NAME_LEN: usize = 0x10;

// Component binary layout (research/xim/.../resource/UiElementSection.kt:141-203).
mod comp {
    pub const UV_WIDTH: usize = 16;
    pub const UV_HEIGHT: usize = 18;
    pub const UV_OFFSET_X: usize = 20;
    pub const UV_OFFSET_Y: usize = 22;
    pub const FLIP_MODE: usize = 24;
    pub const COLORS: usize = 25;
    pub const UNK7: usize = 42;
    pub const REF: usize = 45;
    pub const LEN: usize = 61;
}

// UiFlipMode (UiElementSection.kt:8-13).
const FLIP_HORIZONTAL: u8 = 1;
const FLIP_VERTICAL: u8 = 2;
const FLIP_BOTH: u8 = 3;

// unk7 == 2 suppresses drawing (UiElementSection.kt:172-174).
const UNK7_DRAW_DISABLED: u8 = 2;

#[derive(Debug, Clone)]
pub struct UiElementComponent {
    pub positions: [(i16, i16); 4],
    pub uv_offset_x: u16,
    pub uv_offset_y: u16,
    pub uv_width: u16,
    pub uv_height: u16,
    pub flip_mode: u8,
    pub colors: [[u8; 4]; 4],
    pub texture_ref: String,
    pub draw_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct UiElement {
    pub components: Vec<UiElementComponent>,
}

#[derive(Debug, Clone)]
pub struct UiElementGroup {
    pub name: String,
    pub texture_names: Vec<String>,
    pub elements: Vec<UiElement>,
}

#[derive(Debug, Clone)]
pub struct UiSprite {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn read_u16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn read_i16(b: &[u8], o: usize) -> Option<i16> {
    Some(i16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn normalize_name(raw: &[u8]) -> String {
    let s: String = raw
        .iter()
        .map(|&b| {
            if (0x20..0x7f).contains(&b) {
                b as char
            } else {
                '\0'
            }
        })
        .take_while(|&c| c != '\0')
        .collect();
    s.trim_end().to_ascii_lowercase()
}

fn parse_component(c: &[u8]) -> Option<UiElementComponent> {
    let mut positions = [(0i16, 0i16); 4];
    for (i, pos) in positions.iter_mut().enumerate() {
        *pos = (read_i16(c, i * 4)?, read_i16(c, i * 4 + 2)?);
    }
    let mut colors = [[0u8; 4]; 4];
    for (i, color) in colors.iter_mut().enumerate() {
        let o = comp::COLORS + i * 4;
        *color = c.get(o..o + 4)?.try_into().ok()?;
    }
    Some(UiElementComponent {
        positions,
        uv_offset_x: read_u16(c, comp::UV_OFFSET_X)?,
        uv_offset_y: read_u16(c, comp::UV_OFFSET_Y)?,
        uv_width: read_u16(c, comp::UV_WIDTH)?,
        uv_height: read_u16(c, comp::UV_HEIGHT)?,
        flip_mode: *c.get(comp::FLIP_MODE)?,
        colors,
        texture_ref: normalize_name(c.get(comp::REF..comp::REF + NAME_LEN)?),
        draw_enabled: *c.get(comp::UNK7)? != UNK7_DRAW_DISABLED,
    })
}

fn parse_element(d: &[u8]) -> Option<(UiElement, usize)> {
    let num_components = *d.first()? as usize;
    let mut off = 1;
    let mut components = Vec::with_capacity(num_components);
    for _ in 0..num_components {
        components.push(parse_component(d.get(off..off + comp::LEN)?)?);
        off += comp::LEN;
    }
    Some((UiElement { components }, off))
}

pub fn parse_ui_element_group(data: &[u8]) -> Option<UiElementGroup> {
    let name = normalize_name(data.get(0..NAME_LEN)?);
    let num_sets = *data.get(NAME_LEN)? as usize;
    let mut off = NAME_LEN + 1;

    let mut texture_names = Vec::with_capacity(num_sets);
    for _ in 0..num_sets {
        texture_names.push(normalize_name(data.get(off..off + NAME_LEN)?));
        off += NAME_LEN;
    }

    let count = read_u16(data, off)? as usize;
    off += 2;

    let mut elements = Vec::with_capacity(count);
    for _ in 0..count {
        let (element, consumed) = parse_element(data.get(off..)?)?;
        elements.push(element);
        off += consumed;
    }

    Some(UiElementGroup {
        name,
        texture_names,
        elements,
    })
}

pub fn find_ui_element_group(dat_bytes: &[u8], group_name: &str) -> Option<UiElementGroup> {
    let want = normalize_name(group_name.as_bytes());
    walk(dat_bytes)
        .flatten()
        .filter(|c| c.kind == UI_ELEMENT_GROUP_KIND)
        .filter_map(|c| parse_ui_element_group(c.data))
        .find(|g| g.name == want)
}

fn texture_section_name(body: &[u8]) -> Option<String> {
    Some(normalize_name(body.get(1..1 + NAME_LEN)?))
}

fn find_texture(dat_bytes: &[u8], name: &str) -> Option<DecodedTexture> {
    walk(dat_bytes)
        .flatten()
        .filter(|c| c.kind == TEXTURE_KIND)
        .filter(|c| texture_section_name(c.data).as_deref() == Some(name))
        .find_map(|c| decode_texture(c.data).ok())
}

pub fn crop_sprite(
    tex: &DecodedTexture,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    flip_mode: u8,
) -> Option<UiSprite> {
    let (x, y, w, h) = (x as u32, y as u32, w as u32, h as u32);
    if w == 0 || h == 0 || x + w > tex.width || y + h > tex.height {
        return None;
    }
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for row in 0..h {
        for col in 0..w {
            let (sx, sy) = match flip_mode {
                FLIP_HORIZONTAL => (w - 1 - col, row),
                FLIP_VERTICAL => (col, h - 1 - row),
                FLIP_BOTH => (w - 1 - col, h - 1 - row),
                _ => (col, row),
            };
            let src = (((y + sy) * tex.width + (x + sx)) * 4) as usize;
            let dst = ((row * w + col) * 4) as usize;
            rgba[dst..dst + 4].copy_from_slice(&tex.rgba[src..src + 4]);
        }
    }
    Some(UiSprite {
        width: w,
        height: h,
        rgba,
    })
}

pub fn ui_sprite(dat_bytes: &[u8], group_name: &str, index: usize) -> Option<UiSprite> {
    let group = find_ui_element_group(dat_bytes, group_name)?;
    let element = group.elements.get(index)?;
    let component = element
        .components
        .iter()
        .find(|c| c.draw_enabled)
        .or_else(|| element.components.first())?;
    let tex = find_texture(dat_bytes, &component.texture_ref)?;
    crop_sprite(
        &tex,
        component.uv_offset_x,
        component.uv_offset_y,
        component.uv_width,
        component.uv_height,
        component.flip_mode,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::TexFormat;

    fn name16(s: &str) -> [u8; 16] {
        let mut out = [b' '; 16];
        for (i, b) in s.bytes().take(16).enumerate() {
            out[i] = b;
        }
        out
    }

    fn build_component(uv: (u16, u16, u16, u16), flip: u8, unk7: u8, ref_name: &str) -> Vec<u8> {
        let mut c = vec![0u8; comp::LEN];
        c[comp::UV_WIDTH..comp::UV_WIDTH + 2].copy_from_slice(&uv.2.to_le_bytes());
        c[comp::UV_HEIGHT..comp::UV_HEIGHT + 2].copy_from_slice(&uv.3.to_le_bytes());
        c[comp::UV_OFFSET_X..comp::UV_OFFSET_X + 2].copy_from_slice(&uv.0.to_le_bytes());
        c[comp::UV_OFFSET_Y..comp::UV_OFFSET_Y + 2].copy_from_slice(&uv.1.to_le_bytes());
        c[comp::FLIP_MODE] = flip;
        c[comp::UNK7] = unk7;
        c[comp::REF..comp::REF + NAME_LEN].copy_from_slice(&name16(ref_name));
        c
    }

    fn build_group(group_name: &str, tex_names: &[&str], elements: &[Vec<Vec<u8>>]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&name16(group_name));
        d.push(tex_names.len() as u8);
        for t in tex_names {
            d.extend_from_slice(&name16(t));
        }
        d.extend_from_slice(&(elements.len() as u16).to_le_bytes());
        for components in elements {
            d.push(components.len() as u8);
            for c in components {
                d.extend_from_slice(c);
            }
        }
        d
    }

    fn synth_chunk(name: &[u8; 4], kind: u8, body: &[u8]) -> Vec<u8> {
        let total = 16 + body.len();
        let padded = total.div_ceil(16) * 16;
        let size_units = (padded / 16) as u32;
        let value = (size_units << 7) | (kind as u32 & 0x7F);
        let mut out = Vec::with_capacity(padded);
        out.extend_from_slice(name);
        out.extend_from_slice(&value.to_le_bytes());
        out.extend(std::iter::repeat_n(0u8, 8));
        out.extend_from_slice(body);
        out.extend(std::iter::repeat_n(0u8, padded - total));
        out
    }

    // A minimal type-0x91 palettized texture whose every texel is `color` (RGBA).
    fn synth_palettized(name: &str, w: u16, h: u16, color: [u8; 4]) -> Vec<u8> {
        let pixel_off = 0x439usize;
        let mut body = vec![0u8; pixel_off + (w as usize) * (h as usize)];
        body[0] = 0x91;
        body[1..17].copy_from_slice(&name16(name));
        body[0x15..0x19].copy_from_slice(&(w as i32).to_le_bytes());
        body[0x19..0x1D].copy_from_slice(&(h as i32).to_le_bytes());
        // palette[1] in file order is b,g,r,a_raw; decode halves->doubles alpha.
        let pal1 = 0x39 + 4;
        body[pal1] = color[2];
        body[pal1 + 1] = color[1];
        body[pal1 + 2] = color[0];
        body[pal1 + 3] = (color[3] as u16).div_ceil(2) as u8;
        for px in body[pixel_off..].iter_mut() {
            *px = 1;
        }
        body
    }

    #[test]
    fn parses_group_fields_and_component() {
        let c = build_component((2, 3, 4, 5), 0, 0, "menu    tex0");
        let body = build_group("menu    frames  ", &["menu    frames  "], &[vec![c]]);

        let g = parse_ui_element_group(&body).unwrap();
        assert_eq!(g.name, "menu    frames");
        assert_eq!(g.texture_names, vec!["menu    frames".to_string()]);
        assert_eq!(g.elements.len(), 1);
        let comp = &g.elements[0].components[0];
        assert_eq!(
            (
                comp.uv_offset_x,
                comp.uv_offset_y,
                comp.uv_width,
                comp.uv_height
            ),
            (2, 3, 4, 5)
        );
        assert_eq!(comp.texture_ref, "menu    tex0");
        assert!(comp.draw_enabled);
    }

    #[test]
    fn unk7_two_disables_draw() {
        let c = build_component((0, 0, 1, 1), 0, UNK7_DRAW_DISABLED, "t");
        let body = build_group("g", &[], &[vec![c]]);
        let g = parse_ui_element_group(&body).unwrap();
        assert!(!g.elements[0].components[0].draw_enabled);
    }

    #[test]
    fn finds_group_among_sections() {
        let c = build_component((0, 0, 1, 1), 0, 0, "t");
        let group_body = build_group("menu    frames  ", &[], &[vec![c]]);
        let mut dat = synth_chunk(b"junk", 0x04, &[0xABu8; 8]);
        dat.extend(synth_chunk(b"ui31", UI_ELEMENT_GROUP_KIND, &group_body));

        let g = find_ui_element_group(&dat, "menu    frames  ").unwrap();
        assert_eq!(g.name, "menu    frames");
        assert!(find_ui_element_group(&dat, "menu    nope    ").is_none());
    }

    #[test]
    fn crop_extracts_subrect() {
        let mut rgba = vec![0u8; 4 * 4 * 4];
        for i in 0..16 {
            rgba[i * 4] = i as u8;
        }
        let tex = DecodedTexture {
            width: 4,
            height: 4,
            format_tag: TexFormat::Bgra32,
            rgba,
        };
        let s = crop_sprite(&tex, 1, 1, 2, 2, 0).unwrap();
        assert_eq!((s.width, s.height), (2, 2));
        // Row 1 of a 4-wide image starts at index 4; cols 1..3 -> texels 5,6.
        assert_eq!(s.rgba[0], 5);
        assert_eq!(s.rgba[4], 6);
    }

    #[test]
    fn crop_vertical_flip_swaps_rows() {
        let mut rgba = vec![0u8; 2 * 2 * 4];
        rgba[0] = 10; // (0,0)
        rgba[1 * 4] = 11; // (1,0)
        rgba[2 * 4] = 12; // (0,1)
        rgba[3 * 4] = 13; // (1,1)
        let tex = DecodedTexture {
            width: 2,
            height: 2,
            format_tag: TexFormat::Bgra32,
            rgba,
        };
        let s = crop_sprite(&tex, 0, 0, 2, 2, FLIP_VERTICAL).unwrap();
        // Vertical flip puts the bottom row (12,13) first.
        assert_eq!(s.rgba[0], 12);
        assert_eq!(s.rgba[4], 13);
    }

    #[test]
    fn crop_rejects_out_of_bounds() {
        let tex = DecodedTexture {
            width: 2,
            height: 2,
            format_tag: TexFormat::Bgra32,
            rgba: vec![0u8; 2 * 2 * 4],
        };
        assert!(crop_sprite(&tex, 1, 0, 2, 1, 0).is_none());
        assert!(crop_sprite(&tex, 0, 0, 0, 1, 0).is_none());
    }

    #[test]
    fn ui_sprite_end_to_end_matches_palette_color() {
        let red = [255u8, 0, 0, 255];
        let tex_body = synth_palettized("menu    frames  ", 2, 2, red);
        let comp = build_component((0, 0, 2, 2), 0, 0, "menu    frames  ");
        let group_body = build_group("menu    frames  ", &["menu    frames  "], &[vec![comp]]);

        let mut dat = synth_chunk(b"itex", TEXTURE_KIND, &tex_body);
        dat.extend(synth_chunk(b"uiel", UI_ELEMENT_GROUP_KIND, &group_body));

        let sprite = ui_sprite(&dat, "menu    frames  ", 0).unwrap();
        assert_eq!((sprite.width, sprite.height), (2, 2));
        assert_eq!(&sprite.rgba[0..4], &red);
        assert_eq!(&sprite.rgba[4..8], &red);
    }
}
