use crate::chunk::walk;
use crate::kind::ChunkKind;
use crate::map_image::{scan_graphics, GraphicImage};

// Section 0x21 (SpriteSheetMesh). Layout, re-expressed from LandSandBoat-era
// retail DATs (cross-checked against research/xim SpriteSheetSection.kt):
// after the 0x10 chunk header the body is
//   u16 unk_flag, u16 num_mesh, u8 lens_flare, u8, u8, u8 norm_flag,
//   char[0x10] texture_name (two 8-byte tokens = category + id),
//   then num_mesh frames of { u16==1, u8 num_quads, u8, [16B if lens_flare],
//   (6*num_quads) verts of { vec3 pos, rgba u8x4, f32 u, f32 v } }.
// When unk_flag==1 && norm_flag==0 the UVs are texel-space and scale by 1/256.
pub const MOON_PHASE_FRAMES: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvRect {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

#[derive(Debug, Clone)]
pub struct MoonSpriteSheet {
    pub frames: Vec<UvRect>,
    pub texture: GraphicImage,
}

// A lens-flare sprite sheet (0x21 with the lens_flare flag set). Each frame carries
// the per-mesh `offset` fraction (research/xim SpriteSheetSection.kt:52-58, the first
// of the four floats) that places the flare element along the sun->screen-centre
// axis at lineStart*(1-offset)+lineEnd*offset (ZoneDrawer.kt:233-236).
#[derive(Debug, Clone)]
pub struct LensFlareSheet {
    pub frames: Vec<UvRect>,
    pub offsets: Vec<f32>,
    pub texture: GraphicImage,
}

fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

// Parse the per-mesh frames and (for lens-flare sheets) per-mesh offset fractions.
// research/xim SpriteSheetSection.kt:44-79: each mesh is { u16==1, u8 num_quads, u8,
// [lens_flare: f32 offset + 3 discarded floats], 6*num_quads verts }.
fn parse_frames_offsets(b: &[u8]) -> Option<(Vec<UvRect>, Vec<f32>)> {
    if b.len() < 24 {
        return None;
    }
    let unk_flag = rd_u16(b, 0);
    let num_mesh = rd_u16(b, 2) as usize;
    let lens_flare = b[4] == 1;
    let norm_flag = b[7];
    let uv_scale = if unk_flag == 1 && norm_flag == 0 {
        1.0 / 256.0
    } else {
        1.0
    };

    let mut frames = Vec::with_capacity(num_mesh);
    let mut offsets = Vec::with_capacity(if lens_flare { num_mesh } else { 0 });
    let mut p = 24usize;
    for _ in 0..num_mesh {
        if p + 4 > b.len() {
            return None;
        }
        let num_quads = b[p + 2] as usize;
        p += 4;
        if lens_flare {
            if p + 16 > b.len() {
                return None;
            }
            offsets.push(rd_f32(b, p));
            p += 16;
        }
        let num_verts = 6 * num_quads;
        let (mut u0, mut v0, mut u1, mut v1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for _ in 0..num_verts {
            if p + 24 > b.len() {
                return None;
            }
            let u = rd_f32(b, p + 16) * uv_scale;
            let v = rd_f32(b, p + 20) * uv_scale;
            u0 = u0.min(u);
            u1 = u1.max(u);
            v0 = v0.min(v);
            v1 = v1.max(v);
            p += 24;
        }
        frames.push(UvRect { u0, v0, u1, v1 });
    }
    Some((frames, offsets))
}

fn parse_frames(b: &[u8]) -> Option<Vec<UvRect>> {
    parse_frames_offsets(b).map(|(frames, _)| frames)
}

fn texture_tokens(b: &[u8]) -> (String, String) {
    let trim = |s: &[u8]| {
        String::from_utf8_lossy(s)
            .trim_end_matches([' ', '\0'])
            .to_string()
    };
    (trim(&b[8..16]), trim(&b[16..24]))
}

// Locate the retail moon sprite sheet (12 phase frames, texture "moon"/"moonshap")
// inside an environment/zone DAT and pair it with its decoded texture.
pub fn extract_moon_sprite_sheet(dat_bytes: &[u8]) -> Option<MoonSpriteSheet> {
    for c in walk(dat_bytes).filter_map(Result::ok) {
        if ChunkKind::from_u8(c.kind) != Some(ChunkKind::SpriteSheet) {
            continue;
        }
        let b = c.data;
        if b.len() < 24 || rd_u16(b, 2) as usize != MOON_PHASE_FRAMES || b[4] != 0 {
            continue;
        }
        let (category, id) = texture_tokens(b);
        if category != "moon" {
            continue;
        }
        let frames = parse_frames(b)?;
        if frames.len() != MOON_PHASE_FRAMES {
            continue;
        }
        let texture = scan_graphics(dat_bytes).find(|g| g.category == category && g.id == id)?;
        return Some(MoonSpriteSheet { frames, texture });
    }
    None
}

// Locate the retail sun sprite sheet (texture category "suns"/"suny", non-lens-flare,
// attach=0xE Sun in the weather tree) and pair it with its decoded texture. Unlike the
// moon this is not a 12-frame phase sheet, so frame-count is not constrained.
pub fn extract_sun_sprite_sheet(dat_bytes: &[u8]) -> Option<MoonSpriteSheet> {
    for c in walk(dat_bytes).filter_map(Result::ok) {
        if ChunkKind::from_u8(c.kind) != Some(ChunkKind::SpriteSheet) {
            continue;
        }
        let b = c.data;
        if b.len() < 24 || b[4] != 0 {
            continue;
        }
        let (category, id) = texture_tokens(b);
        if category != "suns" && category != "suny" {
            continue;
        }
        let frames = parse_frames(b)?;
        if frames.is_empty() {
            continue;
        }
        let texture = scan_graphics(dat_bytes).find(|g| g.category == category && g.id == id)?;
        return Some(MoonSpriteSheet { frames, texture });
    }
    None
}

// Locate a lens-flare sprite sheet (lf0x chain: lens_flare flag set) and pair it with
// its decoded texture, capturing each mesh's offset fraction along the sun axis.
pub fn extract_lens_flare_sheet(dat_bytes: &[u8]) -> Option<LensFlareSheet> {
    for c in walk(dat_bytes).filter_map(Result::ok) {
        if ChunkKind::from_u8(c.kind) != Some(ChunkKind::SpriteSheet) {
            continue;
        }
        let b = c.data;
        if b.len() < 24 || b[4] != 1 {
            continue;
        }
        let (category, id) = texture_tokens(b);
        let (frames, offsets) = parse_frames_offsets(b)?;
        if frames.is_empty() || offsets.len() != frames.len() {
            continue;
        }
        let texture = scan_graphics(dat_bytes).find(|g| g.category == category && g.id == id)?;
        return Some(LensFlareSheet {
            frames,
            offsets,
            texture,
        });
    }
    None
}

// Scrape the day-of-week (0x4E, 8xRGBA) and moon-phase (0x4F, 12xRGBA) color tables
// from the first particle generator in a DAT that carries them (the sun/moon billboard
// generator). research/xim ParticleUpdaters.kt:289-317.
pub struct CelestialColorTables {
    pub day_of_week: Option<[[f32; 4]; 8]>,
    pub moon_phase: Option<[[f32; 4]; 12]>,
}

pub fn extract_celestial_color_tables(dat_bytes: &[u8]) -> Option<CelestialColorTables> {
    let mut day_of_week = None;
    let mut moon_phase = None;
    for c in walk(dat_bytes).filter_map(Result::ok) {
        if ChunkKind::from_u8(c.kind) != Some(ChunkKind::Generator) {
            continue;
        }
        if let Ok(Some(def)) = crate::particle_gen::ParticleGeneratorDef::parse(c.data) {
            if day_of_week.is_none() {
                day_of_week = def.day_of_week_color;
            }
            if moon_phase.is_none() {
                moon_phase = def.moon_phase_color;
            }
        }
        if day_of_week.is_some() && moon_phase.is_some() {
            break;
        }
    }
    if day_of_week.is_none() && moon_phase.is_none() {
        return None;
    }
    Some(CelestialColorTables {
        day_of_week,
        moon_phase,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a 0x21 SpriteSheet body: u16 unk_flag, u16 num_mesh, u8 lens_flare, 2 pad,
    // u8 norm_flag, char[0x10] texture_name, then per-mesh records.
    fn header(num_mesh: u16, lens_flare: bool, category: &str, id: &str) -> Vec<u8> {
        let mut b = vec![0u8; 24];
        b[0..2].copy_from_slice(&0u16.to_le_bytes());
        b[2..4].copy_from_slice(&num_mesh.to_le_bytes());
        b[4] = lens_flare as u8;
        b[7] = 1; // norm_flag != 0 => uv_scale 1.0
        let mut tok = [b' '; 16];
        tok[..category.len()].copy_from_slice(category.as_bytes());
        tok[8..8 + id.len()].copy_from_slice(id.as_bytes());
        b[8..24].copy_from_slice(&tok);
        b
    }

    fn quad(u0: f32, v0: f32, u1: f32, v1: f32) -> Vec<u8> {
        // One quad = 6 verts of { vec3 pos, rgba u8x4, f32 u, f32 v } = 24B each.
        let uvs = [(u0, v0), (u1, v0), (u1, v1), (u0, v0), (u1, v1), (u0, v1)];
        let mut v = Vec::new();
        for (u, vv) in uvs {
            v.extend_from_slice(&[0u8; 16]); // pos(12) + rgba(4)
            v.extend_from_slice(&u.to_le_bytes());
            v.extend_from_slice(&vv.to_le_bytes());
        }
        v
    }

    fn mesh(num_quads: u8, flare_offset: Option<f32>, frame: (f32, f32, f32, f32)) -> Vec<u8> {
        let mut m = vec![0u8; 4];
        m[0..2].copy_from_slice(&1u16.to_le_bytes());
        m[2] = num_quads;
        if let Some(off) = flare_offset {
            m.extend_from_slice(&off.to_le_bytes());
            m.extend_from_slice(&[0u8; 12]); // 3 discarded floats
        }
        let (u0, v0, u1, v1) = frame;
        for _ in 0..num_quads {
            m.extend(quad(u0, v0, u1, v1));
        }
        m
    }

    #[test]
    fn parses_frame_uv_bounds() {
        let mut b = header(1, false, "suns", "suny");
        b.extend(mesh(1, None, (0.1, 0.2, 0.7, 0.8)));
        let frames = parse_frames(&b).unwrap();
        assert_eq!(frames.len(), 1);
        let f = frames[0];
        assert!((f.u0 - 0.1).abs() < 1e-6 && (f.v0 - 0.2).abs() < 1e-6);
        assert!((f.u1 - 0.7).abs() < 1e-6 && (f.v1 - 0.8).abs() < 1e-6);
    }

    #[test]
    fn lens_flare_captures_first_offset_float() {
        // research/xim SpriteSheetSection.kt:52-58: the first of four floats is the
        // per-mesh offset; the next three are discarded.
        let mut b = header(2, true, "lf0a", "flar");
        b.extend(mesh(1, Some(0.25), (0.0, 0.0, 0.5, 0.5)));
        b.extend(mesh(1, Some(1.40), (0.5, 0.5, 1.0, 1.0)));
        let (frames, offsets) = parse_frames_offsets(&b).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(offsets.len(), 2);
        assert!((offsets[0] - 0.25).abs() < 1e-6);
        assert!((offsets[1] - 1.40).abs() < 1e-6);
    }

    #[test]
    fn non_lens_flare_has_no_offsets() {
        let mut b = header(1, false, "suns", "suny");
        b.extend(mesh(1, None, (0.0, 0.0, 1.0, 1.0)));
        let (_frames, offsets) = parse_frames_offsets(&b).unwrap();
        assert!(offsets.is_empty());
    }
}
