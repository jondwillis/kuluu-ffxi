use crate::{DatError, Result};

// research/xim ParticleGeneratorParser.kt + ParticleInitializers.kt + ParticleKeyFrameSection.kt
//
// A 0x05 Generator chunk whose StandardParticleSetup (sec2 op 0x01) links data-type 0x0B is a
// particle emitter. The generator header and four section-offset words sit in the chunk body
// (which already excludes the 16-byte chunk header, so a ByteReader `sectionStart + X` maps to
// body index `X - 0x10`):
//   body[0x64] u16  emissionVariance
//   body[0x66] u16  framesPerEmission - 1
//   body[0x68] u8   particlesPerEmission
//   body[0x69] u8   genFlags
//   body[0x70..0x80] four u32 section offsets (section data at value - 0x10)
// Each section is a stream of opcodeConfig u32s: opcode = cfg & 0xFF, size_words = (cfg>>8)&0x1F,
// allocationOffset = cfg>>0xD; the block is size_words*4 bytes; a 0 opcode/size terminates.
// Only section 2 (particle initializers) is needed for the visible stream.
const HEADER_LEN: usize = 0x80;
const PARTICLE_LINKED_DATA: u8 = 0x0B;
// research/xim ParticleGeneratorParser.kt:68-70 (genFlags at body[0x69]);
// continuous singleton + auto-run-at-model-ready semantics: Actor.kt:724-734.
const GEN_FLAG_CONTINUOUS: u8 = 0x04;
const GEN_FLAG_AUTO_RUN: u8 = 0x10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DatId(pub [u8; 4]);

impl DatId {
    fn from(b: &[u8], off: usize) -> Self {
        Self([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }
    fn is_zero(&self) -> bool {
        self.0 == [0, 0, 0, 0]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParticleBlend {
    #[default]
    Additive,
    Blend,
    Subtract,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleGeneratorDef {
    pub frames_per_emission: f32,
    pub particles_per_emission: u32,
    pub emission_variance: f32,

    pub mesh_id: [u8; 4],
    pub base_position: [f32; 3],
    pub max_life_frames: f32,
    pub camera_billboard: bool,

    pub continuous: bool,
    pub auto_run: bool,

    pub init_scale: [f32; 3],
    pub init_color: [f32; 4],
    pub init_velocity: [f32; 3],
    pub init_rotation: [f32; 3],
    pub blend: ParticleBlend,

    // Per-particle keyframe tracks referenced by DAT-id (resolved against the action's 0x19 chunks).
    pub scale_x_track: Option<[u8; 4]>,
    pub scale_y_track: Option<[u8; 4]>,
    pub alpha_track: Option<[u8; 4]>,

    // research/xim ParticleUpdaters.kt:289-317 DayOfWeekColorUpdater (0x4E, 8xRGBA) and
    // MoonPhaseColorUpdater (0x4F, 12xRGBA): indexed by day-of-week / moon-phase frame and
    // applied as a 2x modulate (Particle.kt:218). RGBA in 0..=1.
    pub day_of_week_color: Option<[[f32; 4]; 8]>,
    pub moon_phase_color: Option<[[f32; 4]; 12]>,

    // research/xim ParticleUpdaters.kt section-3 updaters (offset at body[0x78], same
    // sectionHeader+offset-0x10 convention as the setup section). TextureCoordinateUpdater
    // 0x27/0x28 carry the per-frame UV-translate velocity that scrolls the sprite/sheet
    // texture (cascade/moat water). VelocityAccelerator 0x03/0x06/0x09 read a Vector3f at
    // payload+0; only 0x03 (gravity) affects the visible arc. [0,0]/None = static.
    pub uv_scroll: [f32; 2],
    pub accel: Option<[f32; 3]>,
}

impl ParticleGeneratorDef {
    pub fn parse(body: &[u8]) -> Result<Option<Self>> {
        if body.len() < HEADER_LEN {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: HEADER_LEN,
                available: body.len(),
            });
        }

        let frames_per_emission = u16_le(body, 0x66) as f32 + 1.0;
        let particles_per_emission = (body[0x68] as u32).max(1);
        let emission_variance = u16_le(body, 0x64) as f32;
        let gen_flags = body[0x69];
        let continuous = gen_flags & GEN_FLAG_CONTINUOUS != 0;
        let auto_run = gen_flags & GEN_FLAG_AUTO_RUN != 0;

        // Section 2 = particle initializers.
        let sec2_raw = u32_le(body, 0x74) as usize;
        if sec2_raw < 0x10 || sec2_raw - 0x10 >= body.len() {
            return Ok(None);
        }
        let mut cursor = sec2_raw - 0x10;

        let mut mesh_id = [0u8; 4];
        let mut base_position = [0.0f32; 3];
        let mut max_life_frames = 0.0f32;
        let mut camera_billboard = false;
        let mut is_particle = false;
        let mut init_scale = [1.0f32; 3];
        let mut init_color = [1.0f32; 4];
        let mut init_velocity = [0.0f32; 3];
        let mut init_rotation = [0.0f32; 3];
        let mut scale_x_track = None;
        let mut scale_y_track = None;
        let mut alpha_track = None;
        let mut blend = ParticleBlend::Additive;
        let mut day_of_week_color = None;
        let mut moon_phase_color = None;

        while cursor + 4 <= body.len() {
            let cfg = u32_le(body, cursor);
            let opcode = (cfg & 0xFF) as u8;
            let size_words = ((cfg >> 8) & 0x1F) as usize;
            if opcode == 0x00 || size_words == 0 {
                break;
            }
            let block_len = size_words * 4;
            let payload = cursor + 4;
            if cursor + block_len > body.len() {
                break;
            }
            match opcode {
                0x01 if payload + 32 <= body.len() => {
                    let bb = u16_le(body, payload);
                    camera_billboard = bb & 0x0001 != 0 || bb & 0x00C0 == 0x00C0;
                    mesh_id = [
                        body[payload + 8],
                        body[payload + 9],
                        body[payload + 10],
                        body[payload + 11],
                    ];
                    base_position = [
                        f32_le(body, payload + 16),
                        f32_le(body, payload + 20),
                        f32_le(body, payload + 24),
                    ];
                    is_particle = body[payload + 29] == PARTICLE_LINKED_DATA;
                    max_life_frames = u16_le(body, payload + 30) as f32;
                }
                0x02 if payload + 12 <= body.len() => {
                    init_velocity = [
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ];
                }
                0x09 if payload + 12 <= body.len() => {
                    init_rotation = [
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ];
                }
                0x0F if payload + 12 <= body.len() => {
                    init_scale = [
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ];
                }
                0x16 if payload + 4 <= body.len() => {
                    init_color = [
                        body[payload] as f32 / 255.0,
                        body[payload + 1] as f32 / 255.0,
                        body[payload + 2] as f32 / 255.0,
                        body[payload + 3] as f32 / 255.0,
                    ];
                }
                // research/xim ParticleUpdaters.kt:289-301 DayOfWeekColorUpdater: expectZero32
                // then 8 RGBA quads (u8x4, 0..=255). payload+0 is the zero u32.
                0x4E if payload + 4 + 32 <= body.len() => {
                    let mut colors = [[0.0f32; 4]; 8];
                    for (i, c) in colors.iter_mut().enumerate() {
                        let o = payload + 4 + i * 4;
                        *c = [
                            body[o] as f32 / 255.0,
                            body[o + 1] as f32 / 255.0,
                            body[o + 2] as f32 / 255.0,
                            body[o + 3] as f32 / 255.0,
                        ];
                    }
                    day_of_week_color = Some(colors);
                }
                // research/xim ParticleUpdaters.kt:304-316 MoonPhaseColorUpdater: expectZero32
                // then 12 RGBA quads.
                0x4F if payload + 4 + 48 <= body.len() => {
                    let mut colors = [[0.0f32; 4]; 12];
                    for (i, c) in colors.iter_mut().enumerate() {
                        let o = payload + 4 + i * 4;
                        *c = [
                            body[o] as f32 / 255.0,
                            body[o + 1] as f32 / 255.0,
                            body[o + 2] as f32 / 255.0,
                            body[o + 3] as f32 / 255.0,
                        ];
                    }
                    moon_phase_color = Some(colors);
                }
                // KeyFrameValueSetup: opcode selects the target channel; the track id is at payload+4.
                0x27 if payload + 8 <= body.len() => scale_x_track = track_id(body, payload + 4),
                0x28 if payload + 8 <= body.len() => scale_y_track = track_id(body, payload + 4),
                0x2D if payload + 8 <= body.len() => alpha_track = track_id(body, payload + 4),
                // BlendFuncInitializer: p0 @payload+0 — high nibble bit 0x01 = opaque, else low
                // nibble selects (0x8 additive, 0x4/0x6 alpha blend, 0x1/0x2 reverse-subtract).
                0x1E if payload < body.len() => {
                    let p0 = body[payload];
                    blend = if (p0 >> 4) & 0x01 != 0 {
                        ParticleBlend::Blend
                    } else {
                        match p0 & 0x0F {
                            0x8 => ParticleBlend::Additive,
                            0x1 | 0x2 => ParticleBlend::Subtract,
                            _ => ParticleBlend::Blend,
                        }
                    };
                }
                _ => {}
            }
            cursor += block_len;
        }

        if !is_particle {
            return Ok(None);
        }

        // Section 3 (body[0x78]) — per-frame updaters (same walk as
        // generator.rs::parse_cloud_generator). 0x27/0x28 TextureCoordinateUpdater UV
        // scroll; 0x03 VelocityAccelerator gravity (Vector3f at payload+0).
        let mut uv_scroll = [0.0f32; 2];
        let mut accel = None;
        let sec3_raw = u32_le(body, 0x78) as usize;
        if sec3_raw >= 0x10 && sec3_raw - 0x10 < body.len() {
            let mut cursor = sec3_raw - 0x10;
            while cursor + 4 <= body.len() {
                let cfg = u32_le(body, cursor);
                let opcode = (cfg & 0xFF) as u8;
                let size_words = ((cfg >> 8) & 0x1F) as usize;
                if opcode == 0x00 || size_words == 0 {
                    break;
                }
                let block_len = size_words * 4;
                let payload = cursor + 4;
                if cursor + block_len > body.len() {
                    break;
                }
                match opcode {
                    0x27 if payload + 4 <= body.len() => uv_scroll[0] = f32_le(body, payload),
                    0x28 if payload + 4 <= body.len() => uv_scroll[1] = f32_le(body, payload),
                    0x03 if payload + 12 <= body.len() => {
                        accel = Some([
                            f32_le(body, payload),
                            f32_le(body, payload + 4),
                            f32_le(body, payload + 8),
                        ]);
                    }
                    _ => {}
                }
                cursor += block_len;
            }
        }

        Ok(Some(Self {
            frames_per_emission,
            particles_per_emission,
            emission_variance,
            mesh_id,
            base_position,
            max_life_frames,
            camera_billboard,
            continuous,
            auto_run,
            init_scale,
            init_color,
            init_velocity,
            init_rotation,
            blend,
            scale_x_track,
            scale_y_track,
            alpha_track,
            day_of_week_color,
            moon_phase_color,
            uv_scroll,
            accel,
        }))
    }

    pub fn is_singleton(&self) -> bool {
        self.max_life_frames == 0.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyFrameTrack {
    pub points: Vec<(f32, f32)>,
}

impl KeyFrameTrack {
    // research/xim ParticleKeyFrameValueSection.read: (time, value) f32 pairs from the chunk body,
    // terminated by an entry whose time == 1.0.
    pub fn parse(body: &[u8]) -> Self {
        let mut points = Vec::new();
        let mut o = 0;
        while o + 8 <= body.len() {
            let t = f32_le(body, o);
            let v = f32_le(body, o + 4);
            points.push((t, v));
            o += 8;
            if t >= 1.0 {
                break;
            }
        }
        Self { points }
    }

    pub fn sample(&self, progress: f32) -> f32 {
        self.sample_from(progress, None)
    }

    // research/xim ParticleKeyFrameData.getCurrentValue. `initial` overrides the value of the very
    // first keyframe when interpolating the opening segment (a ProgressValueUpdater seeds the curve
    // with the particle's initial channel value, e.g. its starting scale).
    pub fn sample_from(&self, progress: f32, initial: Option<f32>) -> f32 {
        match self.points.as_slice() {
            [] => 0.0,
            [single] => single.1,
            pts => {
                if progress >= 1.0 {
                    return pts.last().unwrap().1;
                }
                let next = pts
                    .iter()
                    .position(|&(t, _)| t > progress)
                    .unwrap_or(pts.len() - 1);
                let next = next.max(1);
                let (pt, pv) = pts[next - 1];
                let (nt, nv) = pts[next];
                let pv = match initial {
                    Some(i) if next - 1 == 0 => i,
                    _ => pv,
                };
                let span = nt - pt;
                if span.abs() < 1e-9 {
                    return pv;
                }
                let f = (progress - pt) / span;
                (1.0 - f) * pv + f * nv
            }
        }
    }
}

fn track_id(b: &[u8], off: usize) -> Option<[u8; 4]> {
    let id = DatId::from(b, off);
    (!id.is_zero()).then_some(id.0)
}

#[inline]
fn u16_le(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

#[inline]
fn u32_le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[inline]
fn f32_le(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a generator body matching the real layout: header at 0x64, section-2 offset word at
    // body[0x74] (value = body_index + 0x10), then the initializer opcode stream.
    fn build(sec2: &[u8], frames_per_em: u16, ppe: u8) -> Vec<u8> {
        let mut body = vec![0u8; HEADER_LEN];
        body[0x66..0x68].copy_from_slice(&(frames_per_em - 1).to_le_bytes());
        body[0x68] = ppe;
        let sec2_body_index = HEADER_LEN;
        body[0x74..0x78].copy_from_slice(&((sec2_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(sec2);
        body
    }

    fn op(opcode: u8, size_words: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![opcode, size_words, 0, 0];
        v.extend_from_slice(payload);
        v.resize(size_words as usize * 4, 0);
        v
    }

    #[test]
    fn parses_particle_generator_header_and_setup() {
        let mut setup = op(0x01, 12, &[]);
        // billboard XYZ
        setup[4] = 0x01;
        // mesh id at payload+8 (payload = cursor+4 = setup index 4)
        setup[4 + 8..4 + 12].copy_from_slice(b"kir1");
        // base position y at payload+20
        setup[4 + 20..4 + 24].copy_from_slice(&0.2f32.to_le_bytes());
        // linked-data type at payload+29 = particle; max life u16 at payload+30
        setup[4 + 29] = PARTICLE_LINKED_DATA;
        setup[4 + 30..4 + 32].copy_from_slice(&36u16.to_le_bytes());

        let mut sec2 = setup;
        sec2.extend(op(0x0F, 4, &{
            let mut p = Vec::new();
            p.extend_from_slice(&0.05f32.to_le_bytes());
            p.extend_from_slice(&0.05f32.to_le_bytes());
            p.extend_from_slice(&1.0f32.to_le_bytes());
            p
        }));
        sec2.extend(op(0x16, 2, &[46, 46, 158, 255]));
        sec2.extend(op(0x02, 4, &{
            let mut p = Vec::new();
            p.extend_from_slice(&0.0f32.to_le_bytes());
            p.extend_from_slice(&(-0.005f32).to_le_bytes());
            p.extend_from_slice(&0.0f32.to_le_bytes());
            p
        }));
        sec2.extend(op(0x2D, 4, &{
            let mut p = Vec::new();
            p.extend_from_slice(&0u32.to_le_bytes());
            p.extend_from_slice(b"k1a0");
            p
        }));

        let body = build(&sec2, 5, 0);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.mesh_id, *b"kir1");
        assert!(def.camera_billboard);
        assert_eq!(def.frames_per_emission, 5.0);
        assert_eq!(def.particles_per_emission, 1, "ppe 0 clamps to 1");
        assert!((def.base_position[1] - 0.2).abs() < 1e-6);
        assert_eq!(def.max_life_frames, 36.0);
        assert!(!def.is_singleton());
        assert!((def.init_scale[0] - 0.05).abs() < 1e-6);
        assert!((def.init_color[2] - 158.0 / 255.0).abs() < 1e-6);
        assert!((def.init_velocity[1] + 0.005).abs() < 1e-6);
        assert_eq!(def.alpha_track, Some(*b"k1a0"));
        assert_eq!(def.scale_x_track, None);
    }

    #[test]
    fn parses_section3_uv_scroll_and_accel() {
        // Minimal particle setup in section 2, terminated, then a section-3 stream at
        // body[0x78] with TextureCoordinateUpdater 0x27/0x28 and VelocityAccelerator 0x03.
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 29] = PARTICLE_LINKED_DATA;
        let mut body = build(&setup, 1, 1);
        body.extend_from_slice(&[0u8; 4]); // terminate section 2
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        let mut sec3 = op(0x27, 2, &(-0.015f32).to_le_bytes());
        sec3.extend(op(0x28, 2, &0.001f32.to_le_bytes()));
        sec3.extend(op(0x03, 4, &{
            let mut p = Vec::new();
            p.extend_from_slice(&0.0f32.to_le_bytes());
            p.extend_from_slice(&(-0.02f32).to_le_bytes());
            p.extend_from_slice(&0.0f32.to_le_bytes());
            p
        }));
        body.extend_from_slice(&sec3);

        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(
            (def.uv_scroll[0] + 0.015).abs() < 1e-9,
            "0x27 -> uv_scroll[0]"
        );
        assert!(
            (def.uv_scroll[1] - 0.001).abs() < 1e-9,
            "0x28 -> uv_scroll[1]"
        );
        assert_eq!(def.accel, Some([0.0, -0.02, 0.0]), "0x03 -> accel");
    }

    #[test]
    fn no_section3_leaves_defaults() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 29] = PARTICLE_LINKED_DATA;
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.uv_scroll, [0.0, 0.0]);
        assert_eq!(def.accel, None);
    }

    #[test]
    fn gen_flags_decode_auto_run_and_continuous() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 29] = PARTICLE_LINKED_DATA;
        let mut body = build(&setup, 1, 1);
        body[0x69] = GEN_FLAG_AUTO_RUN | GEN_FLAG_CONTINUOUS;
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.auto_run);
        assert!(def.continuous);

        let mut body = build(&setup, 1, 1);
        body[0x69] = 0;
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(!def.auto_run);
        assert!(!def.continuous);
    }

    #[test]
    fn non_particle_setup_is_none() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 29] = 0x47; // point light, not particle
        let body = build(&setup, 1, 1);
        assert!(ParticleGeneratorDef::parse(&body).unwrap().is_none());
    }

    #[test]
    fn singleton_when_max_life_zero() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 8..4 + 12].copy_from_slice(b"sea0");
        setup[4 + 29] = PARTICLE_LINKED_DATA;
        // max life left at 0
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.is_singleton());
    }

    #[test]
    fn keyframe_track_interpolates_and_clamps() {
        let mut b = Vec::new();
        for (t, v) in [(0.0f32, 0.0f32), (0.5, 0.22), (1.0, 0.12)] {
            b.extend_from_slice(&t.to_le_bytes());
            b.extend_from_slice(&v.to_le_bytes());
        }
        let kf = KeyFrameTrack::parse(&b);
        assert_eq!(kf.points.len(), 3);
        assert!((kf.sample(0.0) - 0.0).abs() < 1e-6);
        assert!((kf.sample(0.25) - 0.11).abs() < 1e-6);
        assert!((kf.sample(0.5) - 0.22).abs() < 1e-6);
        assert!((kf.sample(0.75) - 0.17).abs() < 1e-6);
        assert!((kf.sample(1.5) - 0.12).abs() < 1e-6, "clamps to last");
    }

    #[test]
    fn keyframe_stops_at_time_one() {
        let mut b = Vec::new();
        b.extend_from_slice(&0.0f32.to_le_bytes());
        b.extend_from_slice(&0.0f32.to_le_bytes());
        b.extend_from_slice(&1.0f32.to_le_bytes());
        b.extend_from_slice(&0.5f32.to_le_bytes());
        // trailing garbage past the terminator must be ignored
        b.extend_from_slice(&[0xAA; 16]);
        let kf = KeyFrameTrack::parse(&b);
        assert_eq!(kf.points.len(), 2);
    }
}
