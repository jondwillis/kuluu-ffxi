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

fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn parse_frames(b: &[u8]) -> Option<Vec<UvRect>> {
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
    let mut p = 24usize;
    for _ in 0..num_mesh {
        if p + 4 > b.len() {
            return None;
        }
        let num_quads = b[p + 2] as usize;
        p += 4;
        if lens_flare {
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
    Some(frames)
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
