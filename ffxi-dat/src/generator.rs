use crate::{DatError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generator {
    pub name: [u8; 4],

    pub effect_type: u8,

    pub id: [u8; 4],
}

impl Generator {
    pub fn is_sound(&self) -> bool {
        self.effect_type == 0x3D
    }

    pub fn parse(name: [u8; 4], body: &[u8]) -> Result<Option<Self>> {
        const HEADER_LEN: usize = 0x80;
        if body.len() < HEADER_LEN {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: HEADER_LEN,
                available: body.len(),
            });
        }
        let creation_offset = u32_le(body, 0x74) as usize;
        let tick_offset = u32_le(body, 0x78) as usize;

        if creation_offset < 16 || creation_offset - 16 >= body.len() {
            return Ok(None);
        }
        let creation_start = creation_offset - 16;
        let creation_end = if tick_offset >= 16 && tick_offset - 16 <= body.len() {
            tick_offset - 16
        } else {
            body.len()
        };

        let mut cursor = creation_start;
        while cursor + 4 <= creation_end {
            let data_type = body[cursor];
            let data_size_nibble = (body[cursor + 1] & 0x0F) as usize;
            let advance = data_size_nibble.saturating_mul(4);
            if data_type == 0x00 {
                break;
            }
            if data_type == 0x01 && advance >= 32 && cursor + 4 + 32 <= body.len() {
                let payload = cursor + 4;
                let id = [
                    body[payload + 8],
                    body[payload + 9],
                    body[payload + 10],
                    body[payload + 11],
                ];
                let effect_type = body[payload + 29];
                return Ok(Some(Self {
                    name,
                    effect_type,
                    id,
                }));
            }

            if advance == 0 {
                break;
            }
            cursor = cursor.saturating_add(advance);
        }
        Ok(None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointLightDef {
    pub range: f32,
    pub attenuation: f32,
    pub color: [f32; 4],
    pub base_position: [f32; 3],
}

// research/xim ParticleGeneratorAttachment.kt:46-62 — the StandardParticleSetup
// attachment nibble (body[0] & 0x0F) selects how the generator's particles are
// placed: 0xE = Sun (getSunPosition + camera), 0xF = Moon, 0x0 = None (clouds,
// camera-follow when cfg bit 0x0004 is set; Particle.kt:232-258). The linked DAT
// id at setup+8 is the model the generator instances; linked_type at setup+29 is
// the linked-data class (0x0B StaticMesh-particle, etc.).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudGeneratorDef {
    pub name: [u8; 4],
    pub attach: u8,
    pub follow_camera: bool,
    pub linked_id: [u8; 4],
    pub linked_type: u8,
    pub base_position: [f32; 3],

    // research/xim ParticleGeneratorParser.kt:271-274 the setup-section "ToD Color"
    // KeyFrameValueSetup opcodes 0x60/0x61/0x62/0x63 (R/G/B/A; the 0x3C-0x3F
    // ClockValueUpdaters at runtime) and 0x6D-0x70 "ToD Specular Color" (the
    // 0x4A-0x4D sun-specular updaters). KeyFrameValueSetup.read (ParticleInitializers.kt
    // :481-489) is expect32(0) then nextDatId — so the 0x19 keyframe DAT-id sits at
    // payload+4. kcr1/kcg1/kcb1 drive cloud/sky RGB, ksr1/ksg1/ksb1 the sun; sampled
    // at the full-day fraction (ParticleUpdaters.kt:172-183 ClockValueUpdater).
    pub color_r_track: Option<[u8; 4]>,
    pub color_g_track: Option<[u8; 4]>,
    pub color_b_track: Option<[u8; 4]>,
    pub alpha_mult_track: Option<[u8; 4]>,
}

impl Generator {
    const ATTACH_NONE: u8 = 0x0;
    const ATTACH_SUN: u8 = 0xE;
    const CONFIG_FOLLOW_CAMERA: u16 = 0x0004;

    // research/xim EnvironmentManager.kt:453-515 weat/<type>/ cld1/cld2/sun1 0x05
    // generators. Mirrors dat-cloud-probe parse_setup: the StandardParticleSetup
    // (sec1 op 0x01) carries the config word (camFollow bit), the linked model id
    // and the base position; the chunk-body attachment nibble is body[0] & 0x0F.
    pub fn parse_cloud_generator(name: [u8; 4], body: &[u8]) -> Result<Option<CloudGeneratorDef>> {
        const HEADER_LEN: usize = 0x80;
        if body.len() < HEADER_LEN {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: HEADER_LEN,
                available: body.len(),
            });
        }
        let attach = body[0] & 0x0F;
        let creation_offset = u32_le(body, 0x74) as usize;
        if creation_offset < 16 || creation_offset - 16 >= body.len() {
            return Ok(None);
        }
        let mut cursor = creation_offset - 16;
        let mut setup: Option<CloudGeneratorDef> = None;
        let mut color_r_track = None;
        let mut color_g_track = None;
        let mut color_b_track = None;
        let mut alpha_mult_track = None;
        while cursor + 4 <= body.len() {
            let opcode = body[cursor];
            if opcode == 0x00 {
                break;
            }
            let size_words = (body[cursor + 1] & 0x1F) as usize;
            if size_words == 0 {
                break;
            }
            let block_len = size_words * 4;
            let payload = cursor + 4;
            if cursor + block_len > body.len() {
                break;
            }
            match opcode {
                0x01 if payload + 30 <= body.len() => {
                    let config = u16::from_le_bytes([body[payload], body[payload + 1]]);
                    setup = Some(CloudGeneratorDef {
                        name,
                        attach,
                        follow_camera: config & Self::CONFIG_FOLLOW_CAMERA != 0,
                        linked_id: [
                            body[payload + 8],
                            body[payload + 9],
                            body[payload + 10],
                            body[payload + 11],
                        ],
                        linked_type: body[payload + 29],
                        base_position: [
                            f32_le(body, payload + 16),
                            f32_le(body, payload + 20),
                            f32_le(body, payload + 24),
                        ],
                        color_r_track: None,
                        color_g_track: None,
                        color_b_track: None,
                        alpha_mult_track: None,
                    });
                }
                // ToD Color setup (0x60-0x63) and ToD Specular Color setup (0x6D-0x70):
                // KeyFrameValueSetup stores the 0x19 keyframe DAT-id at payload+4.
                0x60 | 0x6D if payload + 8 <= body.len() => {
                    color_r_track = clock_track_id(body, payload + 4)
                }
                0x61 | 0x6E if payload + 8 <= body.len() => {
                    color_g_track = clock_track_id(body, payload + 4)
                }
                0x62 | 0x6F if payload + 8 <= body.len() => {
                    color_b_track = clock_track_id(body, payload + 4)
                }
                0x63 | 0x70 if payload + 8 <= body.len() => {
                    alpha_mult_track = clock_track_id(body, payload + 4)
                }
                _ => {}
            }
            cursor += block_len;
        }
        Ok(setup.map(|mut def| {
            def.color_r_track = color_r_track;
            def.color_g_track = color_g_track;
            def.color_b_track = color_b_track;
            def.alpha_mult_track = alpha_mult_track;
            def
        }))
    }
}

fn clock_track_id(b: &[u8], off: usize) -> Option<[u8; 4]> {
    let id = [b[off], b[off + 1], b[off + 2], b[off + 3]];
    (id != [0, 0, 0, 0]).then_some(id)
}

impl CloudGeneratorDef {
    pub fn is_camera_cloud(&self) -> bool {
        self.attach == Generator::ATTACH_NONE
    }
    pub fn is_sun_attached(&self) -> bool {
        self.attach == Generator::ATTACH_SUN
    }
}

// A particle-emitting generator (effect_type 0x0B). research/xim ParticleGeneratorParser.kt
// Sec2: the 0x01 StandardParticleSetup carries the billboard flag, base position and the
// billboard-mesh id; 0x0F is the initial scale; 0x16 the particle color. A faithful render
// needs all of these — the raw D3M is a unit quad with overbright-white vertex colour, so
// without scale/colour/billboard it renders as a giant white slab.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleEmitter {
    pub mesh_id: [u8; 4],
    pub base_position: [f32; 3],
    pub scale: [f32; 3],
    pub color: [f32; 4],
    pub camera_billboard: bool,
}

impl Generator {
    const LINKED_DATA_PARTICLE: u8 = 0x0B;
    const BILLBOARD_XYZ: u16 = 0x0001;

    pub fn parse_particle_emitter(body: &[u8]) -> Result<Option<ParticleEmitter>> {
        const HEADER_LEN: usize = 0x80;
        if body.len() < HEADER_LEN {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: HEADER_LEN,
                available: body.len(),
            });
        }
        let creation_offset = u32_le(body, 0x74) as usize;
        if creation_offset < 16 || creation_offset - 16 >= body.len() {
            return Ok(None);
        }
        let mut cursor = creation_offset - 16;

        let mut mesh_id = [0u8; 4];
        let mut base_position = [0.0f32; 3];
        let mut scale = [1.0f32; 3];
        let mut color = [1.0f32; 4];
        let mut camera_billboard = false;
        let mut is_particle = false;

        while cursor + 4 <= body.len() {
            let opcode = body[cursor];
            if opcode == 0x00 {
                break;
            }
            let size_words = (body[cursor + 1] & 0x1F) as usize;
            if size_words == 0 {
                break;
            }
            let block_len = size_words * 4;
            let payload = cursor + 4;
            if cursor + block_len > body.len() {
                break;
            }
            match opcode {
                0x01 if payload + 30 <= body.len() => {
                    let bb = u16::from_le_bytes([body[payload], body[payload + 1]]);
                    camera_billboard = bb & Self::BILLBOARD_XYZ != 0;
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
                    is_particle = body[payload + 29] == Self::LINKED_DATA_PARTICLE;
                }
                0x0F if payload + 12 <= body.len() => {
                    scale = [
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ];
                }
                0x16 if payload + 4 <= body.len() => {
                    color = [
                        body[payload] as f32 / 255.0,
                        body[payload + 1] as f32 / 255.0,
                        body[payload + 2] as f32 / 255.0,
                        body[payload + 3] as f32 / 255.0,
                    ];
                }
                _ => {}
            }
            cursor += block_len;
        }

        if !is_particle {
            return Ok(None);
        }
        Ok(Some(ParticleEmitter {
            mesh_id,
            base_position,
            scale,
            color,
            camera_billboard,
        }))
    }
}

impl Generator {
    const LINKED_DATA_POINT_LIGHT: u8 = 0x47;

    pub fn parse_point_light(body: &[u8]) -> Result<Option<PointLightDef>> {
        const HEADER_LEN: usize = 0x80;
        if body.len() < HEADER_LEN {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: HEADER_LEN,
                available: body.len(),
            });
        }
        let creation_offset = u32_le(body, 0x74) as usize;
        if creation_offset < 16 || creation_offset - 16 >= body.len() {
            return Ok(None);
        }
        let mut cursor = creation_offset - 16;

        let mut is_point_light = false;
        let mut base_position = [0.0f32; 3];
        let mut color = [1.0f32; 4];

        let mut params: Option<(f32, f32, f32, f32)> = None;

        while cursor + 4 <= body.len() {
            let opcode = body[cursor];
            if opcode == 0x00 {
                break;
            }
            let size_words = (body[cursor + 1] & 0x1F) as usize;
            if size_words == 0 {
                break;
            }
            let block_len = size_words * 4;
            let payload = cursor + 4;
            if cursor + block_len > body.len() {
                break;
            }
            match opcode {
                0x01 => {
                    if payload + 28 <= body.len() {
                        base_position = [
                            f32_le(body, payload + 16),
                            f32_le(body, payload + 20),
                            f32_le(body, payload + 24),
                        ];
                    }
                    if payload + 30 <= body.len() {
                        is_point_light = body[payload + 29] == Self::LINKED_DATA_POINT_LIGHT;
                    }
                }
                0x16 => {
                    if payload + 4 <= body.len() {
                        color = [
                            body[payload] as f32 / 255.0,
                            body[payload + 1] as f32 / 255.0,
                            body[payload + 2] as f32 / 255.0,
                            body[payload + 3] as f32 / 255.0,
                        ];
                    }
                }
                0x58 if payload + 16 <= body.len() => {
                    params = Some((
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        map_multiplier(f32_le(body, payload + 8)),
                        map_multiplier(f32_le(body, payload + 12)),
                    ));
                }
                _ => {}
            }
            cursor += block_len;
        }

        if !is_point_light {
            return Ok(None);
        }
        let Some((range, theta, range_mult, theta_mult)) = params else {
            return Ok(None);
        };
        let denom = theta * theta_mult;
        let attenuation = if denom.abs() > 1e-9 { 1.0 / denom } else { 0.0 };
        Ok(Some(PointLightDef {
            range: range * range_mult,
            attenuation,
            color: [color[0] * 2.0, color[1] * 2.0, color[2] * 2.0, color[3]],
            base_position,
        }))
    }
}

fn map_multiplier(base: f32) -> f32 {
    if base >= 0.0 {
        2.0f32.powf(base)
    } else if base >= -1.0 {
        1.0 + base
    } else {
        0.0
    }
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

    fn make_body() -> Vec<u8> {
        let mut buf = vec![0u8; 0x80];

        buf[0x74..0x78].copy_from_slice(&(0x80u32 + 16).to_le_bytes());

        buf[0x78..0x7C].copy_from_slice(&(0x80u32 + 16 + 0x30).to_le_bytes());
        buf
    }

    #[test]
    fn parses_sound_generator() {
        let mut body = make_body();

        let mut cmd = vec![0u8; 36];
        cmd[0] = 0x01;
        cmd[1] = 0x09;

        cmd[4 + 8..4 + 12].copy_from_slice(b"snd0");
        cmd[4 + 29] = 0x3D;
        body.extend_from_slice(&cmd);

        body.resize(body.len().max(0x80 + 0x30), 0);

        let g = Generator::parse(*b"gen0", &body).unwrap().unwrap();
        assert_eq!(g.name, *b"gen0");
        assert_eq!(g.id, *b"snd0");
        assert_eq!(g.effect_type, 0x3D);
        assert!(g.is_sound());
    }

    #[test]
    fn non_sound_generator_still_parses() {
        let mut body = make_body();
        let mut cmd = vec![0u8; 36];
        cmd[0] = 0x01;
        cmd[1] = 0x09;
        cmd[4 + 8..4 + 12].copy_from_slice(b"ring");
        cmd[4 + 29] = 0x3B;
        body.extend_from_slice(&cmd);
        body.resize(body.len().max(0x80 + 0x30), 0);

        let g = Generator::parse(*b"gen1", &body).unwrap().unwrap();
        assert!(!g.is_sound());
        assert_eq!(g.effect_type, 0x3B);
    }

    #[test]
    fn parses_particle_emitter() {
        let mut body = make_body();
        // 0x01 StandardParticleSetup: billboard XYZ, mesh id, base position, particle type.
        let mut c1 = vec![0u8; 36];
        c1[0] = 0x01;
        c1[1] = 0x09;
        c1[4] = 0x01; // billboard flag low byte (XYZ)
        c1[4 + 8..4 + 12].copy_from_slice(b"gr  ");
        c1[4 + 20..4 + 24].copy_from_slice(&0.7f32.to_le_bytes()); // base_position.y
        c1[4 + 29] = 0x0B; // particle
        body.extend_from_slice(&c1);
        // 0x0F scale.
        let mut c2 = vec![0u8; 16];
        c2[0] = 0x0F;
        c2[1] = 0x04;
        c2[4..8].copy_from_slice(&0.024f32.to_le_bytes());
        c2[8..12].copy_from_slice(&0.020f32.to_le_bytes());
        body.extend_from_slice(&c2);
        // 0x16 colour.
        let mut c3 = vec![0u8; 8];
        c3[0] = 0x16;
        c3[1] = 0x02;
        c3[4..8].copy_from_slice(&[46, 46, 158, 128]);
        body.extend_from_slice(&c3);

        let e = Generator::parse_particle_emitter(&body).unwrap().unwrap();
        assert_eq!(e.mesh_id, *b"gr  ");
        assert!(e.camera_billboard);
        assert!((e.base_position[1] - 0.7).abs() < 1e-6);
        assert!((e.scale[0] - 0.024).abs() < 1e-6);
        assert!((e.color[2] - 158.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn sound_generator_is_not_a_particle_emitter() {
        let mut body = make_body();
        let mut cmd = vec![0u8; 36];
        cmd[0] = 0x01;
        cmd[1] = 0x09;
        cmd[4 + 29] = 0x3D; // sound, not particle
        body.extend_from_slice(&cmd);
        assert!(Generator::parse_particle_emitter(&body).unwrap().is_none());
    }

    #[test]
    fn missing_0x01_returns_none() {
        let body = make_body();

        let g = Generator::parse(*b"empt", &body).unwrap();
        assert!(g.is_none());
    }

    #[test]
    fn truncated_header_errors() {
        let body = vec![0u8; 0x40];
        assert!(matches!(
            Generator::parse(*b"shrt", &body),
            Err(DatError::TruncatedChunk { needed: 0x80, .. })
        ));
    }

    fn push_block(buf: &mut Vec<u8>, opcode: u8, size_words: u8, payload: &[u8]) {
        let block_len = size_words as usize * 4;
        let start = buf.len();
        buf.push(opcode);
        buf.push(size_words);
        buf.push(0);
        buf.push(0);
        buf.extend_from_slice(payload);
        buf.resize(start + block_len, 0);
    }

    #[test]
    fn parses_point_light_generator() {
        let mut body = make_body();

        let mut setup = vec![0u8; 32];
        setup[16..20].copy_from_slice(&1.0f32.to_le_bytes());
        setup[20..24].copy_from_slice(&2.0f32.to_le_bytes());
        setup[24..28].copy_from_slice(&3.0f32.to_le_bytes());
        setup[29] = Generator::LINKED_DATA_POINT_LIGHT;
        push_block(&mut body, 0x01, 9, &setup);

        push_block(&mut body, 0x16, 2, &[255, 128, 64, 255]);

        let mut params = Vec::new();
        params.extend_from_slice(&4.0f32.to_le_bytes());
        params.extend_from_slice(&2.0f32.to_le_bytes());
        params.extend_from_slice(&0.0f32.to_le_bytes());
        params.extend_from_slice(&0.0f32.to_le_bytes());
        push_block(&mut body, 0x58, 5, &params);

        body.push(0x00);

        let pl = Generator::parse_point_light(&body).unwrap().unwrap();
        assert_eq!(pl.range, 4.0, "range = range * rangeMult");
        assert_eq!(pl.attenuation, 0.5, "atten = 1/(theta * thetaMult)");
        assert_eq!(pl.base_position, [1.0, 2.0, 3.0]);

        assert_eq!(pl.color[0], 2.0);
        assert!((pl.color[1] - (128.0 / 255.0) * 2.0).abs() < 1e-6);
        assert_eq!(pl.color[3], 1.0, "alpha is not doubled");
    }

    #[test]
    fn non_point_light_generator_is_none() {
        let mut body = make_body();
        let mut setup = vec![0u8; 32];
        setup[29] = 0x3D;
        push_block(&mut body, 0x01, 9, &setup);
        push_block(&mut body, 0x16, 2, &[255, 255, 255, 255]);
        body.push(0x00);
        assert!(Generator::parse_point_light(&body).unwrap().is_none());
    }

    #[test]
    fn point_light_without_params_is_none() {
        let mut body = make_body();
        let mut setup = vec![0u8; 32];
        setup[29] = 0x47;
        push_block(&mut body, 0x01, 9, &setup);
        body.push(0x00);
        assert!(Generator::parse_point_light(&body).unwrap().is_none());
    }

    #[test]
    fn parses_cloud_generator_camfollow_and_attach() {
        // cld1: attach None, cfg camFollow, linked StaticMesh id "clod", base [0,20,0].
        let mut body = make_body();
        body[0] = 0x00; // attach nibble = None
        let mut cmd = vec![0u8; 36];
        cmd[0] = 0x01;
        cmd[1] = 0x09;
        cmd[4..6].copy_from_slice(&0x0004u16.to_le_bytes()); // config: camFollow
        cmd[4 + 8..4 + 12].copy_from_slice(b"clod");
        cmd[4 + 20..4 + 24].copy_from_slice(&20.0f32.to_le_bytes()); // base.y
        cmd[4 + 29] = 0x0B; // linked StaticMesh particle
        body.extend_from_slice(&cmd);

        let g = Generator::parse_cloud_generator(*b"cld1", &body)
            .unwrap()
            .unwrap();
        assert_eq!(g.linked_id, *b"clod");
        assert!(g.follow_camera);
        assert!(g.is_camera_cloud());
        assert!(!g.is_sun_attached());
        assert!((g.base_position[1] - 20.0).abs() < 1e-6);
        assert_eq!(g.color_r_track, None);
    }

    fn clock_block(opcode: u8, id: &[u8; 4]) -> Vec<u8> {
        // KeyFrameValueSetup: size_words=4 (16-byte block); expect32(0) at payload+0,
        // keyframe DAT-id at payload+4, config u32 at payload+8.
        let mut blk = vec![0u8; 16];
        blk[0] = opcode;
        blk[1] = 0x04;
        blk[8..12].copy_from_slice(id);
        blk
    }

    #[test]
    fn cloud_generator_collects_tod_color_tracks() {
        // cld1 with 0x60/0x61/0x62 RGB + 0x63 alpha-mult ToD-color setups after 0x01.
        let mut body = make_body();
        body[0] = 0x00;
        let mut cmd = vec![0u8; 36];
        cmd[0] = 0x01;
        cmd[1] = 0x09;
        cmd[4 + 8..4 + 12].copy_from_slice(b"clod");
        cmd[4 + 29] = 0x0B;
        body.extend_from_slice(&cmd);
        for (opcode, id) in [
            (0x60u8, b"kcr1"),
            (0x61, b"kcg1"),
            (0x62, b"kcb1"),
            (0x63, b"kca1"),
        ] {
            body.extend_from_slice(&clock_block(opcode, id));
        }

        let g = Generator::parse_cloud_generator(*b"cld1", &body)
            .unwrap()
            .unwrap();
        assert_eq!(g.color_r_track, Some(*b"kcr1"));
        assert_eq!(g.color_g_track, Some(*b"kcg1"));
        assert_eq!(g.color_b_track, Some(*b"kcb1"));
        assert_eq!(g.alpha_mult_track, Some(*b"kca1"));
    }

    #[test]
    fn sun_generator_collects_specular_color_tracks() {
        // sun1 with 0x6D/0x6E/0x6F ToD-specular RGB setups (ksr1/ksg1/ksb1).
        let mut body = make_body();
        body[0] = 0x0E;
        let mut cmd = vec![0u8; 36];
        cmd[0] = 0x01;
        cmd[1] = 0x09;
        cmd[4 + 8..4 + 12].copy_from_slice(b"suns");
        cmd[4 + 29] = 0x0B;
        body.extend_from_slice(&cmd);
        for (opcode, id) in [(0x6Du8, b"ksr1"), (0x6E, b"ksg1"), (0x6F, b"ksb1")] {
            body.extend_from_slice(&clock_block(opcode, id));
        }

        let g = Generator::parse_cloud_generator(*b"sun1", &body)
            .unwrap()
            .unwrap();
        assert_eq!(g.color_r_track, Some(*b"ksr1"));
        assert_eq!(g.color_g_track, Some(*b"ksg1"));
        assert_eq!(g.color_b_track, Some(*b"ksb1"));
        assert_eq!(g.alpha_mult_track, None);
    }

    #[test]
    fn parses_sun_attached_generator() {
        // sun1: attach 0xE (Sun), no camFollow, linked "suns".
        let mut body = make_body();
        body[0] = 0x0E;
        let mut cmd = vec![0u8; 36];
        cmd[0] = 0x01;
        cmd[1] = 0x09;
        cmd[4..6].copy_from_slice(&0x00c0u16.to_le_bytes());
        cmd[4 + 8..4 + 12].copy_from_slice(b"suns");
        cmd[4 + 29] = 0x0B;
        body.extend_from_slice(&cmd);

        let g = Generator::parse_cloud_generator(*b"sun1", &body)
            .unwrap()
            .unwrap();
        assert_eq!(g.linked_id, *b"suns");
        assert!(!g.follow_camera);
        assert!(g.is_sun_attached());
        assert!(!g.is_camera_cloud());
    }

    #[test]
    fn map_multiplier_branches() {
        assert_eq!(map_multiplier(0.0), 1.0);
        assert_eq!(map_multiplier(1.0), 2.0);
        assert_eq!(map_multiplier(-0.5), 0.5);
        assert_eq!(map_multiplier(-2.0), 0.0);
    }
}
