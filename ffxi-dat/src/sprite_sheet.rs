use crate::chunk::walk;
use crate::kind::ChunkKind;
use crate::map_image::{scan_graphics, GraphicImage};
use crate::particle_gen::{DAYS_OF_WEEK, MOON_PHASES};

// Section 0x21 (SpriteSheetMesh). Layout, re-expressed from LandSandBoat-era
// retail DATs (cross-checked against research/xim SpriteSheetSection.kt):
// after the 0x10 chunk header the body is
//   u16 unk_flag, u16 num_mesh, u8 lens_flare, u8, u8, u8 norm_flag,
//   char[0x10] texture_name (two 8-byte tokens = category + id),
//   then num_mesh frames of { u16==1, u8 num_quads, u8, [16B if lens_flare],
//   (6*num_quads) verts of { vec3 pos, D3DCOLOR u32, f32 u, f32 v } }.
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
// the per-mesh `offset` fraction (research/xim SpriteSheetSection.kt read distance, the first
// of the four floats) that places the flare element along the sun->screen-centre
// axis at lineStart*(1-offset)+lineEnd*offset (ZoneDrawer.kt drawLensFlare), plus that mesh's
// own quad half-extent, which is what sizes the element on screen (ZoneDrawer.kt drawLensFlare scale).
#[derive(Debug, Clone)]
pub struct LensFlareSheet {
    pub frames: Vec<UvRect>,
    pub offsets: Vec<f32>,
    pub half_extents: Vec<[f32; 2]>,
    /// Each mesh's authored vertex colour, the D argument of retail's texture-stage chain —
    /// this is where the chain's core/halo/ghost intensities live (lf03 in file 201 ramps its
    /// alpha byte 100, 50, 50, 30, 20 down the chain). RGBA bytes; the consumer applies
    /// [`crate::d3m::VERTEX_COLOR_DIVISOR`] like every other stage-0 D.
    pub colors: Vec<[u8; 4]>,
    pub texture: GraphicImage,
}

fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

// The vertex diffuse is a D3DCOLOR (the sprite vertex is the D3DFVF_XYZ|DIFFUSE|TEX1 layout,
// hence the 24-byte stride), i.e. ARGB packed little-endian, so the file bytes run B,G,R,A --
// research/XIClient/src/XIClient/include/Rendering/Color/ARGBByte.h, the same order
// `crate::d3m` unpacks. (research/xim SpriteSheetSection.kt read reads this one field as RGBA,
// against its own nextBGRA everywhere else it walks a vertex buffer.)
fn rd_d3dcolor(b: &[u8], o: usize) -> [u8; 4] {
    [b[o + 2], b[o + 1], b[o], b[o + 3]]
}

// Per-mesh geometry summary: the UV sub-rect, the flare offset fraction, the quad's half
// extent and its vertex colour. One entry per mesh in sheet order.
#[derive(Default)]
struct SheetMeshes {
    frames: Vec<UvRect>,
    offsets: Vec<f32>,
    half_extents: Vec<[f32; 2]>,
    colors: Vec<[u8; 4]>,
}

// Parse the per-mesh frames and (for lens-flare sheets) per-mesh offset fractions.
// research/xim SpriteSheetSection.kt read: each mesh is { u16==1, u8 num_quads, u8,
// [lens_flare: f32 offset + 3 discarded floats], 6*num_quads verts }.
fn parse_frames_offsets(b: &[u8]) -> Option<SheetMeshes> {
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

    let mut out = SheetMeshes::default();
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
            out.offsets.push(rd_f32(b, p));
            p += 16;
        }
        let num_verts = 6 * num_quads;
        let (mut u0, mut v0, mut u1, mut v1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        // A flare element is a flat quad authored at one colour, so the first vertex's is the
        // mesh's; the D3m path keeps the full per-vertex array because its meshes shade.
        let mut color = [0u8; 4];
        for i in 0..num_verts {
            if p + 24 > b.len() {
                return None;
            }
            let x = rd_f32(b, p);
            let y = rd_f32(b, p + 4);
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
            if i == 0 {
                color = rd_d3dcolor(b, p + 12);
            }
            let u = rd_f32(b, p + 16) * uv_scale;
            let v = rd_f32(b, p + 20) * uv_scale;
            u0 = u0.min(u);
            u1 = u1.max(u);
            v0 = v0.min(v);
            v1 = v1.max(v);
            p += 24;
        }
        out.frames.push(UvRect { u0, v0, u1, v1 });
        out.half_extents.push([(x1 - x0) * 0.5, (y1 - y0) * 0.5]);
        out.colors.push(color);
    }
    Some(out)
}

fn parse_frames(b: &[u8]) -> Option<Vec<UvRect>> {
    parse_frames_offsets(b).map(|m| m.frames)
}

// A single flipbook frame's full quad geometry (positions + UVs + per-vertex colors),
// as a triangle-list (6 verts per quad). Unlike UvRect this retains the actual quad mesh
// so a SpriteSheet (0x0E) particle can be rebuilt frame-by-frame instead of only knowing
// the UV bounding rect.
#[derive(Debug, Clone, PartialEq)]
pub struct SpriteFrame {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[u8; 4]>,
}

// A 0x21 sprite sheet parsed for particle use: every mesh's full quad geometry plus the
// two texture-name tokens (category/id) that resolve the backing Img chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleSpriteSheet {
    pub frames: Vec<SpriteFrame>,
    pub category: String,
    pub id: String,
}

impl ParticleSpriteSheet {
    pub fn parse(b: &[u8]) -> Option<Self> {
        let frames = parse_particle_frames(b)?;
        if frames.is_empty() {
            return None;
        }
        let (category, id) = texture_tokens(b);
        Some(Self {
            frames,
            category,
            id,
        })
    }
}

// Same mesh walk as parse_frames_offsets (research/xim SpriteSheetSection.kt read), but
// retains each vertex's { vec3 pos, D3DCOLOR u32, f32 u, f32 v } instead of collapsing to a
// UV bounding rect.
fn parse_particle_frames(b: &[u8]) -> Option<Vec<SpriteFrame>> {
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
            if p + 16 > b.len() {
                return None;
            }
            p += 16;
        }
        let num_verts = 6 * num_quads;
        let mut positions = Vec::with_capacity(num_verts);
        let mut uvs = Vec::with_capacity(num_verts);
        let mut colors = Vec::with_capacity(num_verts);
        for _ in 0..num_verts {
            if p + 24 > b.len() {
                return None;
            }
            positions.push([rd_f32(b, p), rd_f32(b, p + 4), rd_f32(b, p + 8)]);
            colors.push(rd_d3dcolor(b, p + 12));
            uvs.push([rd_f32(b, p + 16) * uv_scale, rd_f32(b, p + 20) * uv_scale]);
            p += 24;
        }
        frames.push(SpriteFrame {
            positions,
            uvs,
            colors,
        });
    }
    Some(frames)
}

fn texture_tokens(b: &[u8]) -> (String, String) {
    crate::texture::split_qualified_name(&b[8..24])
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
        let m = parse_frames_offsets(b)?;
        if m.frames.is_empty() || m.offsets.len() != m.frames.len() {
            continue;
        }
        let texture = scan_graphics(dat_bytes).find(|g| g.category == category && g.id == id)?;
        return Some(LensFlareSheet {
            frames: m.frames,
            offsets: m.offsets,
            half_extents: m.half_extents,
            colors: m.colors,
            texture,
        });
    }
    None
}

// The moon disc's day-of-week (0x4E, 8xRGBA) and moon-phase (0x4F, 12xRGBA) color tables.
// research/xim ParticleUpdaters.kt DayOfWeekColorUpdater.
pub struct CelestialColorTables {
    pub day_of_week: Option<[[f32; 4]; DAYS_OF_WEEK]>,
    pub moon_phase: Option<[[f32; 4]; MOON_PHASES]>,
}

/// research/xim ParticleUpdaters.kt MoonPhaseSpriteSheetUpdater (0x45): scoped to the
/// generator that also carries it -- the moon sprite itself. In 164 of the 181 shipped DATs
/// that carry these tables a generator without 0x45 precedes the moon's, and none of the 164
/// repeats the moon's values: file 201's lunar halo f_ro/weat/fine/moon/kasa has
/// dow[6]=(0.50,0.50,0.50) against the moon's (0.70,0.70,0.70), so a first-match scrape
/// tints the disc with the halo's dimmer table. The walk is flat and weather-blind, unlike
/// the weather-scoped kuluu-render::celestial_particles::collect_celestial_defs: of the 143
/// DATs shipping more than one weather's moon generator, none varies its 0x4E/0x4F tables
/// across weathers (survey of all resolvable ids 1..4000; pinned by
/// real_dat_moon_tables_do_not_vary_by_weather), so the first 0x45 generator in file order
/// carries the active weather's tables too.
pub fn extract_celestial_color_tables(dat_bytes: &[u8]) -> Option<CelestialColorTables> {
    for c in walk(dat_bytes).filter_map(Result::ok) {
        if ChunkKind::from_u8(c.kind) != Some(ChunkKind::Generator) {
            continue;
        }
        let Ok(Some(def)) = crate::particle_gen::ParticleGeneratorDef::parse(c.data) else {
            continue;
        };
        if !def.moon_phase_sprite
            || (def.day_of_week_color.is_none() && def.moon_phase_color.is_none())
        {
            continue;
        }
        return Some(CelestialColorTables {
            day_of_week: def.day_of_week_color,
            moon_phase: def.moon_phase_color,
        });
    }
    None
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
        colored_quad(u0, v0, u1, v1, [0u8; 4], 0.0)
    }

    // One quad = 6 verts of { vec3 pos, D3DCOLOR u32, f32 u, f32 v } = 24B each, laid out as a
    // `half`-half-extent square so half_extents/colors can both be asserted.
    fn colored_quad(u0: f32, v0: f32, u1: f32, v1: f32, color: [u8; 4], half: f32) -> Vec<u8> {
        let corners = [
            (-half, -half, u0, v0),
            (half, -half, u1, v0),
            (half, half, u1, v1),
            (-half, -half, u0, v0),
            (half, half, u1, v1),
            (-half, half, u0, v1),
        ];
        let mut v = Vec::new();
        for (x, y, u, vv) in corners {
            v.extend_from_slice(&x.to_le_bytes());
            v.extend_from_slice(&y.to_le_bytes());
            v.extend_from_slice(&0f32.to_le_bytes());
            v.extend_from_slice(&color);
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

    // Emitter/matcher coupling guard: the 0x21 sheet's name field and the 0xA1 Img's name
    // field are the SAME 16 bytes in retail (verified against ROM/11/93.DAT, file 3020), so
    // both sides must split them identically or the sheet's texture lookup silently misses.
    #[test]
    fn sheet_and_img_split_the_same_qualified_name() {
        let raw = crate::texture::tests::QUALIFIED_FIR;
        let mut sheet = vec![0u8; 24];
        sheet[2..4].copy_from_slice(&1u16.to_le_bytes());
        sheet[7] = 1;
        sheet[8..24].copy_from_slice(raw);
        sheet.extend(mesh(1, None, (0.0, 0.0, 1.0, 1.0)));

        let parsed = ParticleSpriteSheet::parse(&sheet).unwrap();
        let img = crate::texture::tests::img_body_named(raw);
        assert_eq!(
            (parsed.category, parsed.id),
            crate::texture::extract_texture_tokens(&img).unwrap()
        );
    }

    #[test]
    fn parses_frame_uv_bounds() {
        let mut b = header(1, false, "moon", "moon");
        b.extend(mesh(1, None, (0.1, 0.2, 0.7, 0.8)));
        let frames = parse_frames(&b).unwrap();
        assert_eq!(frames.len(), 1);
        let f = frames[0];
        assert!((f.u0 - 0.1).abs() < 1e-6 && (f.v0 - 0.2).abs() < 1e-6);
        assert!((f.u1 - 0.7).abs() < 1e-6 && (f.v1 - 0.8).abs() < 1e-6);
    }

    #[test]
    fn lens_flare_captures_first_offset_float() {
        // research/xim SpriteSheetSection.kt read distance: the first of four floats is the
        // per-mesh offset; the next three are discarded.
        let mut b = header(2, true, "lf0a", "flar");
        b.extend(mesh(1, Some(0.25), (0.0, 0.0, 0.5, 0.5)));
        b.extend(mesh(1, Some(1.40), (0.5, 0.5, 1.0, 1.0)));
        let m = parse_frames_offsets(&b).unwrap();
        assert_eq!(m.frames.len(), 2);
        assert_eq!(m.offsets.len(), 2);
        assert!((m.offsets[0] - 0.25).abs() < 1e-6);
        assert!((m.offsets[1] - 1.40).abs() < 1e-6);
    }

    #[test]
    fn non_lens_flare_has_no_offsets() {
        let mut b = header(1, false, "moon", "moon");
        b.extend(mesh(1, None, (0.0, 0.0, 1.0, 1.0)));
        assert!(parse_frames_offsets(&b).unwrap().offsets.is_empty());
    }

    // The per-mesh vertex colour is the D argument of retail's texture-stage chain and carries
    // the flare chain's authored core/halo/ghost intensities (lf03 in file 201 ramps its alpha
    // byte 100, 50, 50, 30, 20). Collapsing the sheet to UV rects alone drew every element at
    // one brightness.
    #[test]
    fn lens_flare_captures_each_meshs_vertex_colour_and_half_extent() {
        let mut b = header(2, true, "lf0a", "flar");
        let mut core = vec![0u8; 4];
        core[0..2].copy_from_slice(&1u16.to_le_bytes());
        core[2] = 1;
        core.extend_from_slice(&0.0f32.to_le_bytes());
        core.extend_from_slice(&[0u8; 12]);
        core.extend(colored_quad(0.0, 0.0, 0.5, 0.5, [128, 128, 128, 100], 4.0));
        let mut ghost = vec![0u8; 4];
        ghost[0..2].copy_from_slice(&1u16.to_le_bytes());
        ghost[2] = 1;
        ghost.extend_from_slice(&0.5f32.to_le_bytes());
        ghost.extend_from_slice(&[0u8; 12]);
        ghost.extend(colored_quad(0.5, 0.5, 1.0, 1.0, [128, 128, 128, 20], 1.0));
        b.extend(core);
        b.extend(ghost);

        let m = parse_frames_offsets(&b).unwrap();
        assert_eq!(m.colors, vec![[128, 128, 128, 100], [128, 128, 128, 20]]);
        assert_eq!(m.half_extents, vec![[4.0, 4.0], [1.0, 1.0]]);
    }

    // Both parse paths must hand consumers true RGBA: `lens_flare.rs` and `particle_sim.rs`
    // index the array as (r,g,b,a), but the file stores a D3DCOLOR, whose little-endian bytes
    // run B,G,R,A (research/XIClient/src/XIClient/include/Rendering/Color/ARGBByte.h). Taking
    // them in file order swapped red and blue: `tam3`/`tam4`'s sheet, authored 0x80800000_u32
    // ARGB, drew blue instead of red, and the lf03 flare ghosts drew cold instead of warm.
    #[test]
    fn vertex_colour_unpacks_the_d3dcolor_word_as_rgba() {
        const AUTHORED_ARGB: u32 = 0x6040_80C0;
        const EXPECTED_RGBA: [u8; 4] = [0x40, 0x80, 0xC0, 0x60];
        const HALF_EXTENT: f32 = 1.0;

        let mut b = header(1, false, "lf0a", "flar");
        let mut m = vec![0u8; 4];
        m[0..2].copy_from_slice(&1u16.to_le_bytes());
        m[2] = 1;
        m.extend(colored_quad(
            0.0,
            0.0,
            1.0,
            1.0,
            AUTHORED_ARGB.to_le_bytes(),
            HALF_EXTENT,
        ));
        b.extend(m);

        assert_eq!(
            parse_frames_offsets(&b).unwrap().colors,
            vec![EXPECTED_RGBA]
        );
        let particle = ParticleSpriteSheet::parse(&b).unwrap();
        assert_eq!(
            particle.frames[0].colors,
            vec![EXPECTED_RGBA; particle.frames[0].positions.len()]
        );
    }

    // The fixture vertex's authored D3DCOLOR: A=40 R=10 G=20 B=30, distinct per channel so a
    // channel swap can't hide.
    const GEOM_MESH_ARGB: u32 = 0x280A_141E;
    const GEOM_MESH_RGBA: [u8; 4] = [10, 20, 30, 40];

    // A single-quad mesh with distinct per-vertex positions/uvs so the particle parser's
    // full-geometry retention (not just a UV bounding rect) can be asserted.
    fn geom_mesh(verts: &[([f32; 3], [f32; 2])]) -> Vec<u8> {
        assert_eq!(verts.len() % 6, 0, "verts are 6-per-quad triangle lists");
        let mut m = vec![0u8; 4];
        m[0..2].copy_from_slice(&1u16.to_le_bytes());
        m[2] = (verts.len() / 6) as u8;
        for (pos, uv) in verts {
            for c in pos {
                m.extend_from_slice(&c.to_le_bytes());
            }
            m.extend_from_slice(&GEOM_MESH_ARGB.to_le_bytes());
            m.extend_from_slice(&uv[0].to_le_bytes());
            m.extend_from_slice(&uv[1].to_le_bytes());
        }
        m
    }

    #[test]
    fn particle_sheet_retains_per_frame_quad_geometry() {
        let f0: Vec<_> = (0..6)
            .map(|i| ([i as f32, i as f32 + 1.0, 0.0], [i as f32 * 0.1, 0.2]))
            .collect();
        let f1: Vec<_> = (0..6)
            .map(|i| ([-(i as f32), 0.0, 5.0], [0.5, i as f32 * 0.1]))
            .collect();
        let mut b = header(2, false, "fir", "fir");
        b.extend(geom_mesh(&f0));
        b.extend(geom_mesh(&f1));

        let ss = ParticleSpriteSheet::parse(&b).expect("particle sheet parses");
        assert_eq!(ss.category, "fir");
        assert_eq!(ss.frames.len(), 2);
        // Full geometry retained (6 verts/quad), not a single bounding rect.
        assert_eq!(ss.frames[0].positions.len(), 6);
        assert_eq!(ss.frames[0].uvs.len(), 6);
        assert_eq!(ss.frames[0].colors.len(), 6);
        assert_eq!(ss.frames[0].positions[3], [3.0, 4.0, 0.0]);
        assert!((ss.frames[0].uvs[2][0] - 0.2).abs() < 1e-6);
        assert_eq!(ss.frames[1].positions[1], [-1.0, 0.0, 5.0]);
        assert_eq!(ss.frames[0].colors[0], GEOM_MESH_RGBA);
    }

    fn synth_chunk(name: &[u8; 4], kind: u8, body: &[u8]) -> Vec<u8> {
        let total = 16 + body.len();
        let padded_total = total.div_ceil(16) * 16;
        let size_units = (padded_total / 16) as u32;
        let value = (size_units << 7) | (kind as u32 & crate::chunk::CHUNK_KIND_MASK);
        let mut out = name.to_vec();
        out.extend_from_slice(&value.to_le_bytes());
        out.extend(std::iter::repeat_n(0u8, 8));
        out.extend_from_slice(body);
        out.resize(padded_total, 0);
        out
    }

    /// research/xim ParticleUpdaters.kt: the halo `kasa` is the earlier Moon-attached generator
    /// in every environment DAT and carries its own 0x4E/0x4F pair; only the moon sprite's own
    /// (0x45-carrying) generator tints the disc.
    #[test]
    fn celestial_tables_come_from_the_moon_sprite_generator_not_the_earlier_halo() {
        const HALO_DOW: [[u8; 4]; DAYS_OF_WEEK] = [[128, 128, 51, 41]; DAYS_OF_WEEK];
        const HALO_PHASE: [[u8; 4]; MOON_PHASES] = [[128, 128, 128, 0]; MOON_PHASES];
        const MOON_DOW: [[u8; 4]; DAYS_OF_WEEK] = [[128, 117, 69, 128]; DAYS_OF_WEEK];
        const MOON_PHASE: [[u8; 4]; MOON_PHASES] = [[128, 128, 128, 107]; MOON_PHASES];

        let mut dat = synth_chunk(
            b"kasa",
            ChunkKind::Generator as u8,
            &crate::particle_gen::test_support::celestial_generator_body(
                false,
                &HALO_DOW,
                &HALO_PHASE,
            ),
        );
        dat.extend(synth_chunk(
            b"moon",
            ChunkKind::Generator as u8,
            &crate::particle_gen::test_support::celestial_generator_body(
                true,
                &MOON_DOW,
                &MOON_PHASE,
            ),
        ));

        let t = extract_celestial_color_tables(&dat).expect("moon generator carries both tables");
        let dow = t.day_of_week.expect("0x4E scraped");
        let phase = t.moon_phase.expect("0x4F scraped");
        assert!((dow[0][1] - 117.0 / 255.0).abs() < 1e-6, "moon 0x4E green");
        assert!((dow[0][3] - 128.0 / 255.0).abs() < 1e-6, "moon 0x4E alpha");
        assert!(
            (phase[0][3] - 107.0 / 255.0).abs() < 1e-6,
            "moon 0x4F alpha"
        );
    }

    /// Without a moon sprite generator there is no disc tint to scrape: the halo's tables must
    /// not stand in for it (sun_moon falls back to its own constants on None).
    #[test]
    fn celestial_tables_absent_when_only_the_halo_carries_them() {
        const HALO_DOW: [[u8; 4]; DAYS_OF_WEEK] = [[128, 128, 51, 41]; DAYS_OF_WEEK];
        const HALO_PHASE: [[u8; 4]; MOON_PHASES] = [[128, 128, 128, 0]; MOON_PHASES];

        let dat = synth_chunk(
            b"kasa",
            ChunkKind::Generator as u8,
            &crate::particle_gen::test_support::celestial_generator_body(
                false,
                &HALO_DOW,
                &HALO_PHASE,
            ),
        );

        assert!(extract_celestial_color_tables(&dat).is_none());
    }

    /// West Ronfaure's environment DAT, or None on a machine without the retail install.
    fn west_ronfaure_env_dat() -> Option<Vec<u8>> {
        const WEST_RONFAURE_ENV_DAT: u32 = 201;
        let root = crate::archive::open_test_install()?;
        let Ok(loc) = root.resolve(WEST_RONFAURE_ENV_DAT) else {
            eprintln!("skipping: file 201 unresolvable");
            return None;
        };
        match std::fs::read(loc.path_under(&root)) {
            Ok(b) => Some(b),
            Err(_) => {
                eprintln!("skipping: file 201 unreadable");
                None
            }
        }
    }

    /// Real-DAT pin: West Ronfaure's fine-weather moon (f_ro/weat/fine/moon/moon) carries
    /// dow[6]=(0.70,0.70,0.70) at alpha 0.50 where the halo (f_ro/weat/fine/moon/kasa) that
    /// precedes it in chunk order carries (0.50,0.50,0.50) at alpha 0.16. Only the RGB reaches
    /// the drawn disc -- kuluu-render::sun_moon::celestial_moon_tint returns RGB and
    /// MoonMaterial's tint.w carries the sprite-vs-procedural mode flag -- so the halo's table
    /// shows up as a washed-out weekday hue, not as a transparency change.
    #[test]
    fn real_dat_west_ronfaure_scrapes_the_moon_tables_not_the_halos() {
        const MOON_DOW_LIGHT_RED: f32 = 0.70;
        const HALO_DOW_LIGHT_RED: f32 = 0.50;
        const MOON_DOW_ALPHA: f32 = 0.50;
        const MOON_PHASE_NEW_ALPHA: f32 = 0.42;
        const HALO_DOW_ALPHA: f32 = 0.16;
        const HALO_PHASE_NEW_ALPHA: f32 = 0.0;
        const BYTE_QUANTUM: f32 = 1.0 / 255.0;
        const LIGHT_DAY: usize = 6;

        let Some(bytes) = west_ronfaure_env_dat() else {
            return;
        };

        let t = extract_celestial_color_tables(&bytes).expect("file 201 ships a moon generator");
        let dow = t.day_of_week.expect("0x4E scraped");
        let phase = t.moon_phase.expect("0x4F scraped");
        assert!(
            (dow[LIGHT_DAY][0] - MOON_DOW_LIGHT_RED).abs() < BYTE_QUANTUM,
            "0x4E red is the moon's {MOON_DOW_LIGHT_RED}, not the halo's {HALO_DOW_LIGHT_RED}: {}",
            dow[LIGHT_DAY][0]
        );
        assert!(
            (dow[1][3] - MOON_DOW_ALPHA).abs() < BYTE_QUANTUM,
            "0x4E alpha is the moon's {MOON_DOW_ALPHA}, not the halo's {HALO_DOW_ALPHA}: {}",
            dow[1][3]
        );
        assert!(
            (phase[0][3] - MOON_PHASE_NEW_ALPHA).abs() < BYTE_QUANTUM,
            "0x4F new-moon alpha is the moon's {MOON_PHASE_NEW_ALPHA}, not the halo's \
             {HALO_PHASE_NEW_ALPHA}: {}",
            phase[0][3]
        );
    }

    /// research/xim ParticleUpdaters.kt: the scrape takes the first 0x45 generator in file
    /// order rather than the active weather's; this pins the shipped-data property that makes
    /// the two the same tables.
    #[test]
    fn real_dat_moon_tables_do_not_vary_by_weather() {
        let Some(bytes) = west_ronfaure_env_dat() else {
            return;
        };

        let tables: Vec<_> = walk(&bytes)
            .filter_map(Result::ok)
            .filter(|c| ChunkKind::from_u8(c.kind) == Some(ChunkKind::Generator))
            .filter_map(|c| {
                crate::particle_gen::ParticleGeneratorDef::parse(c.data)
                    .ok()
                    .flatten()
            })
            .filter(|d| d.moon_phase_sprite)
            .map(|d| (d.day_of_week_color, d.moon_phase_color))
            .collect();

        assert!(
            tables.len() > 1,
            "file 201 ships one moon sprite generator per weather, found {}",
            tables.len()
        );
        assert!(
            tables.windows(2).all(|w| w[0] == w[1]),
            "0x4E/0x4F tables differ between weathers: {tables:?}"
        );
    }
}
