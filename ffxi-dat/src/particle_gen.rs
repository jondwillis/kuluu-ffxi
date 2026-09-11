use crate::{DatError, Result};

// research/xim ParticleGeneratorParser.kt + ParticleInitializers.kt + ParticleKeyFrameSection.kt
//
// A 0x05 Generator chunk whose StandardParticleSetup (sec2 op 0x01) links data-type 0x0B is a
// particle emitter. The generator header and four section-offset words sit in the chunk body
// (which already excludes the 16-byte chunk header, so a ByteReader `sectionStart + X` maps to
// body index `X - 0x10`):
//   body[0x00] u16  attachFlags           (XIM reads these two via offsetFromDataStart, i.e. body
//   body[0x02] u16  additionalAttachFlags  index 0 — ParticleGeneratorParser.kt read)
//   body[0x64] u16  emissionVariance
//   body[0x66] u16  framesPerEmission - 1
//   body[0x68] u32  flags (particle count in the low 9 bits, XIM's genFlags in byte 0x69)
//   body[0x70..0x80] four u32 section offsets (section data at value - 0x10)
// Each section is a stream of opcodeConfig u32s: opcode = cfg & 0xFF, size_words = (cfg>>8)&0x1F,
// allocationOffset = cfg>>0xD; the block is size_words*4 bytes; a 0 opcode/size terminates.
// Only section 2 (particle initializers) is needed for the visible stream.
const HEADER_LEN: usize = 0x80;
const CHUNK_HEADER_LEN: usize = 0x10;
const OPCODE_MASK: u32 = 0xFF;
pub(crate) const OPCODE_END: u8 = 0x00;
pub(crate) const OPCODE_STANDARD_SETUP: u8 = 0x01;
pub(crate) const SIZE_WORDS_MASK: u8 = 0x1F;
// research/xim ParticleGeneratorSettings.kt LinkedDataType — the StandardParticleSetup linked_data_type
// (setup byte payload+29) selects the particle's mesh source: 0x0B StaticMesh (a D3M billboard),
// 0x0E SpriteSheet (a 0x21 flipbook quad). 0x57 Null / 0x47 PointLight and any other value are
// non-visual particle types and are rejected (parse returns None).
const LINKED_DATA_STATIC_MESH: u8 = 0x0B;
const LINKED_DATA_SPRITE_SHEET: u8 = 0x0E;

// research/xim ParticleGeneratorSettings.kt LinkedDataType (mesh source) + Particle.kt Particle spriteSheetIndex (the per-particle
// spriteSheetIndex cursor) + ParticleUpdaters.kt (SpriteSheetFrameUpdater advances it over
// life). StaticMesh binds a D3M; SpriteSheet binds a 0x21 sprite-sheet whose frames flipbook
// across the particle's lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParticleMeshKind {
    #[default]
    StaticMesh,
    SpriteSheet,
}
// research/xim ParticleGeneratorSettings.kt `enum class AttachType(val flag: Int)` — which
// actor (and whose facing) a generator's emission origin is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AttachType {
    #[default]
    None,
    SourceActor,
    TargetActor,
    SourceToTargetBasis,
    TargetActorSourceFacing,
    SourceActorTargetFacing,
    TargetToSourceBasis,
    SourceActorWeapon,
    ZoneActorA,
    ZoneActorB,
    ZoneActorC,
    Sun,
    Moon,
}

impl AttachType {
    pub fn from_flag(flag: u16) -> Option<Self> {
        Some(match flag {
            0x0 => Self::None,
            0x1 => Self::SourceActor,
            0x2 => Self::TargetActor,
            0x3 => Self::SourceToTargetBasis,
            0x4 => Self::TargetActorSourceFacing,
            0x5 => Self::SourceActorTargetFacing,
            0x6 => Self::TargetToSourceBasis,
            0x9 => Self::SourceActorWeapon,
            0xA => Self::ZoneActorA,
            0xB => Self::ZoneActorB,
            0xC => Self::ZoneActorC,
            0xE => Self::Sun,
            0xF => Self::Moon,
            _ => return None,
        })
    }
}

// research/xim ParticleGeneratorParser.kt — attachFlags bit layout, then
// additionalAttachFlags bit 0x0001 = attachSourceOriented.
const ATTACH_TYPE_MASK: u16 = 0x000F;
const ATTACH_JOINT0_MASK: u16 = 0x03F0;
const ATTACH_JOINT0_SHIFT: u32 = 4;
const ATTACH_JOINT1_MASK: u16 = 0xFC00;
const ATTACH_JOINT1_SHIFT: u32 = 10;
const ATTACH_SOURCE_ORIENTED: u16 = 0x0001;

// research/xim ParticleInitializers.kt — the StandardParticleSetup renderStateFlags u16
// sits directly after the billboard flags. Bit 0x1000 (`ignoreTextureAlpha`) is the same bit
// retail tests as `field_10C & 0x10000000` to pick the D3m element's texture-stage table.
// research/XIClient/src/XIClient/source/Resource/Derived/CMoD3m.cpp CMoD3m::Draw
const RENDER_STATE_IGNORE_TEXTURE_ALPHA: u16 = 0x1000;
// research/xim ParticleInitializers.kt read `cameraAttachedBasePosition`.
const RENDER_STATE_CAMERA_ATTACHED_BASE: u16 = 0x0400;

// research/xim ParticleInitializers.kt read `followCamera` — orthogonal to the billboard-type bits
// in the same word. The weat/ precipitation curtains ride it (La Theine's `~1ra` is cfg 0x0004:
// camera-following and NOT billboarded).
const BILLBOARD_FOLLOW_CAMERA: u16 = 0x0004;

// research/xim ParticleInitializers.kt read — the billboard-type ladder over the same word,
// tested in this order. Retail keeps the modes distinct: a `Camera` particle keeps a world
// orientation that aims its mesh-local +X at the eye, while `Xyz` replaces the modelview's
// upper 3x3 with the view basis (research/xim GLDrawer.kt drawXimParticle). Collapsing the two draws an
// axial 3-D mesh (the sun/moon glow domes) as a flat screen sprite.
const BILLBOARD_CAMERA_MASK: u16 = 0x00C0;
const BILLBOARD_MOVEMENT_MASK: u16 = 0x0081;
const BILLBOARD_MOVEMENT_HORIZONTAL: u16 = 0x0080;
const BILLBOARD_MOVEMENT: u16 = 0x0040;
const BILLBOARD_XZ: u16 = 0x4000;
const BILLBOARD_XYZ: u16 = 0x0001;

/// Retail's `BillBoardType` (research/xim ParticleInitializers.kt read).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParticleBillboard {
    #[default]
    None,
    Camera,
    Movement,
    MovementHorizontal,
    Xz,
    Xyz,
}

impl ParticleBillboard {
    fn from_flags(bb: u16) -> Self {
        if bb & BILLBOARD_CAMERA_MASK == BILLBOARD_CAMERA_MASK {
            Self::Camera
        } else if bb & BILLBOARD_MOVEMENT_MASK == BILLBOARD_MOVEMENT_MASK {
            Self::Movement
        } else if bb & BILLBOARD_MOVEMENT_HORIZONTAL != 0 {
            Self::MovementHorizontal
        } else if bb & BILLBOARD_MOVEMENT != 0 {
            Self::Movement
        } else if bb & BILLBOARD_XZ != 0 {
            Self::Xz
        } else if bb & BILLBOARD_XYZ != 0 {
            Self::Xyz
        } else {
            Self::None
        }
    }
}

// research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp CYyGenerator::ElemGenerate — sec2 0x06/0x07
// offset each new elem by a random direction (two rng angles) at a radius derived from
// `fpos[1] + fpos[2]`. 0x07 additionally scales that offset per axis, which is how the
// ground-splash rings (`~1h*`, scale [1.3, 0.0, 1.2]) spread as flat ellipses instead of balls.
// Retail gates both cases on `CheckFlag29() == false` (:860), i.e. a batched generator's single
// elem carries its own spread; the consumer decides whether that applies to it (see
// `kuluu_render::particle_sim::emit`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionVariance {
    pub radius_variance: f32,
    pub base_radius: f32,
    pub axis_scale: [f32; 3],
}

impl PositionVariance {
    pub fn max_radius(&self) -> f32 {
        self.radius_variance + self.base_radius
    }

    // `unit_radius` in 0..=1 and `yaw`/`pitch` in -PI..=PI are the three draws retail takes
    // (`ufrand(rmax)`, two `frand(ANGLE_PI)`). The transcription at CYyGenerator.cpp CYyGenerator::ElemGenerate v376
    // computes `ufrand(rmax)` and then writes the un-randomised `rmax` into the offset vector —
    // the two cannot both be intended, and a shell of drops at one fixed radius is not what the
    // discarded draw is for, so the random radius wins. research/xim (tier 6) reads it as
    // `base + variance * u^(1/3)` (ParticleGeneratorSettings.kt getOffset), a solid ball with
    // uniform density rather than uniform radius.
    pub fn offset(&self, unit_radius: f32, yaw: f32, pitch: f32) -> [f32; 3] {
        let r = self.max_radius() * unit_radius;
        let (sp, cp) = pitch.sin_cos();
        let (sy, cy) = yaw.sin_cos();
        [
            r * cp * cy * self.axis_scale[0],
            r * sp * self.axis_scale[1],
            r * cp * sy * self.axis_scale[2],
        ]
    }
}

// research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp CYyGenerator::ConstructFromData size — the resource
// body from byte 0x60 is memcpy'd onto the object at `field_C0`, so object offset X reads back at
// body index X - 0x70 (our `body` already drops the 16-byte chunk header). `flags` (CYyGenerator.h
// object 0xD8) is therefore the u32 at body[0x68], and Script1..4 (0xE0..0xEC) land on the four
// section-offset words at body[0x70..0x80], which is what pins the mapping.
//
// XIM reads byte 0x68 as an 8-bit particle count and byte 0x69 as `genFlags`
// (ParticleGeneratorParser.kt read); both are views onto this one word, so the flag bits sit
// eight higher than XIM's. Continuous-singleton + auto-run semantics: xim Actor.kt startAutoRunParticles.
const GEN_FLAGS_OFFSET: usize = 0x68;
// CYyGenerator.cpp CYyGenerator::Idle v161 `(double)(this->flags & 0x1FF)` — the count is 9 bits, not 8.
const PARTICLE_COUNT_MASK: u32 = 0x1FF;
const GEN_FLAG_CONTINUOUS: u32 = 0x0400;
// The bit retail's WeatherTransition.cpp ActivateWeatherGenerators tests to decide whether a weat/<tag> generator
// activates, and the same bit XIM calls genFlags 0x10.
const GEN_FLAG_AUTO_RUN: u32 = 0x1000;
// CYyGenerator.cpp CYyGenerator::CheckFlag29 CheckFlag29 — a batched generator emits one elem per emission (:2814)
// and that elem is itself a multi-particle batch.
const GEN_FLAG_BATCHED: u32 = 0x2000_0000;
const BLEND_FUNC_OPAQUE_BIT: u8 = 0x01;
const BLEND_FUNC_MODE_MASK: u8 = 0x0F;

// Vana'diel's elemental week (research/xim EnvironmentManager.kt DayOfWeek) and the
// 12 moon-phase buckets the 0x45/0x4F celestial opcodes index.
pub const DAYS_OF_WEEK: usize = 8;
pub const MOON_PHASES: usize = 12;
// RGBA — one time-of-day keyframe track per channel (0x60 r .. 0x63 a).
pub const TOD_COLOR_CHANNELS: usize = 4;

fn rgba_u8(b: &[u8], o: usize) -> [f32; 4] {
    std::array::from_fn(|i| b[o + i] as f32 / 255.0)
}

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

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ParticleGeneratorDef {
    pub frames_per_emission: f32,
    pub particles_per_emission: u32,
    pub emission_variance: f32,

    pub mesh_id: [u8; 4],
    pub mesh_kind: ParticleMeshKind,
    pub base_position: [f32; 3],
    pub max_life_frames: f32,
    pub camera_billboard: bool,
    pub billboard: ParticleBillboard,
    // `base_position` is an offset from the camera rather than a world placement. Two independent
    // flags express it: the billboard word's followCamera bit and the render-state's
    // cameraAttachedBasePosition bit (La Theine's rain uses the first for the `~1ra` curtain and
    // the second for the `rai2` mist puff). They place differently — followCamera pins the
    // generator to the camera position outright, cameraAttachedBasePosition rotates the offset
    // by the view matrix (research/xim Particle.kt updateAssociatedPosition) — so both are kept alongside the
    // union.
    pub camera_relative: bool,
    pub follow_camera: bool,
    pub camera_attached_base: bool,
    // The spawn spread applied to every emitted particle; None puts them all on one point.
    pub position_variance: Option<PositionVariance>,

    pub continuous: bool,
    pub auto_run: bool,
    pub batched: bool,

    pub attach_type: AttachType,
    pub attach_joint_source: u8,
    pub attach_joint_target: u8,
    pub attach_source_oriented: bool,

    pub init_scale: [f32; 3],
    pub init_color: [f32; 4],
    pub init_velocity: [f32; 3],
    pub init_rotation: [f32; 3],
    pub blend: ParticleBlend,
    // The raw BlendFuncInitializer p0 (retail `field_16C & 0xFF`), kept alongside the collapsed
    // `blend` because the TEXTUREFACTOR-alpha promotion is keyed on byte 0x44 exactly.
    // research/XIClient/src/XIClient/source/Resource/Derived/CMoD3m.cpp CMoD3m::Draw
    pub blend_byte: u8,

    // Selects the D3m texture-stage table: set = NonZeroOneTSS (texture alpha ignored,
    // alpha = 4*D.a*F.a), clear = NonZeroTwoTSS (alpha = 8*D.a*T.a*F.a).
    // research/XIClient/src/XIClient/source/Resource/Derived/CMoD3m.cpp ZeroOneTSS
    pub ignore_texture_alpha: bool,

    // Per-particle keyframe tracks referenced by DAT-id (resolved against the action's 0x19 chunks).
    pub scale_x_track: Option<[u8; 4]>,
    pub scale_y_track: Option<[u8; 4]>,
    pub alpha_track: Option<[u8; 4]>,

    // research/xim ParticleUpdaters.kt DayOfWeekColorUpdater (0x4E, 8xRGBA) and
    // MoonPhaseColorUpdater (0x4F, 12xRGBA): indexed by day-of-week / moon-phase frame and
    // applied as a 2x modulate (Particle.kt getColor). RGBA in 0..=1.
    pub day_of_week_color: Option<[[f32; 4]; DAYS_OF_WEEK]>,
    pub moon_phase_color: Option<[[f32; 4]; MOON_PHASES]>,

    // The time-of-day color curves: initializer 0x60..0x63 name a keyframe track per RGBA
    // channel, and section-3 ClockValueUpdater 0x3C..0x3F sample it at the Vana'diel day
    // fraction rather than the particle's life progress. This is how retail authors the
    // sun's dawn/noon/dusk ramp and the moon's daytime fade — 0x3F multiplies alpha, the
    // other three assign their channel.
    // research/xim ParticleGeneratorParser.kt sec2Handler,431-434
    pub tod_color_tracks: [Option<[u8; 4]>; TOD_COLOR_CHANNELS],
    pub tod_color_driven: [bool; TOD_COLOR_CHANNELS],

    // research/xim ParticleGeneratorParser.kt sec3Handler MoonPhaseSpriteSheetUpdater (0x45): the
    // sprite-sheet frame is the current moon phase, not the particle's life progress.
    pub moon_phase_sprite: bool,

    // research/xim ParticleUpdaters.kt section-3 updaters (offset at body[0x78], same
    // sectionHeader+offset-0x10 convention as the setup section). TextureCoordinateUpdater
    // 0x27/0x28 carry the per-frame UV-translate velocity that scrolls the sprite/sheet
    // texture (cascade/moat water). VelocityAccelerator 0x03/0x06/0x09 read a Vector3f at
    // payload+0; only 0x03 (gravity) affects the visible arc. [0,0]/None = static.
    pub uv_scroll: [f32; 2],
    pub accel: Option<[f32; 3]>,

    // Section 1 (body[0x70]) generator-level updater 0x0A, research/xim
    // ParticleGeneratorParser.kt sec1Handler GeneratorCullUpdater.
    pub emit_cull: Option<EmitCull>,
}

// research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp CYyGenerator::Idle case 0x0A —
// each frame the camera-eye distance to the generator is tested against `fpos[1]` (0 defers to
// XiZone::GetDrawDistance) and `fpos[2]`; out of range, the rest of the generator's update
// script (its emission among it) is skipped for the frame, and with `pos[3] & 1` the generator
// unlinks for good.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmitCull {
    pub max_distance: f32,
    pub min_distance: f32,
    pub unlink_out_of_range: bool,
}

impl EmitCull {
    pub fn out_of_range(&self, distance: f32, zone_draw_distance: f32) -> bool {
        let max = if self.max_distance == 0.0 {
            zone_draw_distance
        } else {
            self.max_distance
        };
        distance > max || distance < self.min_distance
    }
}

const SEC1_OPCODE_EMIT_CULL: u8 = 0x0A;

impl ParticleGeneratorDef {
    pub fn parse(body: &[u8]) -> Result<Option<Self>> {
        if body.len() < HEADER_LEN {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: HEADER_LEN,
                available: body.len(),
            });
        }

        let attach_flags = u16_le(body, 0x00);
        let additional_attach = u16_le(body, 0x02);
        let attach_type =
            AttachType::from_flag(attach_flags & ATTACH_TYPE_MASK).unwrap_or_default();
        let attach_joint_source =
            ((attach_flags & ATTACH_JOINT0_MASK) >> ATTACH_JOINT0_SHIFT) as u8;
        let attach_joint_target =
            ((attach_flags & ATTACH_JOINT1_MASK) >> ATTACH_JOINT1_SHIFT) as u8;
        let attach_source_oriented = additional_attach & ATTACH_SOURCE_ORIENTED != 0;

        let frames_per_emission = u16_le(body, 0x66) as f32 + 1.0;
        let emission_variance = u16_le(body, 0x64) as f32;
        let flags = u32_le(body, GEN_FLAGS_OFFSET);
        let particles_per_emission = flags & PARTICLE_COUNT_MASK;
        let continuous = flags & GEN_FLAG_CONTINUOUS != 0;
        let auto_run = flags & GEN_FLAG_AUTO_RUN != 0;
        let batched = flags & GEN_FLAG_BATCHED != 0;

        // Section 2 = particle initializers.
        let sec2_raw = u32_le(body, 0x74) as usize;
        if sec2_raw < CHUNK_HEADER_LEN || sec2_raw - CHUNK_HEADER_LEN >= body.len() {
            return Ok(None);
        }
        let mut cursor = sec2_raw - CHUNK_HEADER_LEN;

        let mut mesh_id = [0u8; 4];
        let mut mesh_kind = ParticleMeshKind::StaticMesh;
        let mut base_position = [0.0f32; 3];
        let mut max_life_frames = 0.0f32;
        let mut camera_billboard = false;
        let mut billboard = ParticleBillboard::None;
        let mut follow_camera = false;
        let mut camera_attached_base = false;
        let mut position_variance = None;
        let mut is_particle = false;
        let mut init_scale = [1.0f32; 3];
        let mut init_color = [1.0f32; 4];
        let mut init_velocity = [0.0f32; 3];
        let mut init_rotation = [0.0f32; 3];
        let mut scale_x_track = None;
        let mut scale_y_track = None;
        let mut alpha_track = None;
        let mut blend = ParticleBlend::Additive;
        let mut blend_byte = 0u8;
        let mut ignore_texture_alpha = false;
        let mut tod_color_tracks: [Option<[u8; 4]>; TOD_COLOR_CHANNELS] =
            [None; TOD_COLOR_CHANNELS];

        while cursor + 4 <= body.len() {
            let cfg = u32_le(body, cursor);
            let opcode = (cfg & OPCODE_MASK) as u8;
            let size_words = ((cfg >> 8) & u32::from(SIZE_WORDS_MASK)) as usize;
            if opcode == OPCODE_END || size_words == 0 {
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
                    billboard = ParticleBillboard::from_flags(bb);
                    camera_billboard = bb & BILLBOARD_XYZ != 0
                        || bb & BILLBOARD_CAMERA_MASK == BILLBOARD_CAMERA_MASK;
                    let render_state = u16_le(body, payload + 2);
                    ignore_texture_alpha = render_state & RENDER_STATE_IGNORE_TEXTURE_ALPHA != 0;
                    follow_camera = bb & BILLBOARD_FOLLOW_CAMERA != 0;
                    camera_attached_base = render_state & RENDER_STATE_CAMERA_ATTACHED_BASE != 0;
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
                    (is_particle, mesh_kind) = match body[payload + 29] {
                        LINKED_DATA_STATIC_MESH => (true, ParticleMeshKind::StaticMesh),
                        LINKED_DATA_SPRITE_SHEET => (true, ParticleMeshKind::SpriteSheet),
                        _ => (false, ParticleMeshKind::StaticMesh),
                    };
                    max_life_frames = u16_le(body, payload + 30) as f32;
                }
                0x02 if payload + 12 <= body.len() => {
                    init_velocity = [
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ];
                }
                0x06 if payload + 8 <= body.len() => {
                    position_variance = Some(PositionVariance {
                        radius_variance: f32_le(body, payload),
                        base_radius: f32_le(body, payload + 4),
                        axis_scale: [1.0; 3],
                    });
                }
                0x07 if payload + 20 <= body.len() => {
                    position_variance = Some(PositionVariance {
                        radius_variance: f32_le(body, payload),
                        base_radius: f32_le(body, payload + 4),
                        axis_scale: [
                            f32_le(body, payload + 8),
                            f32_le(body, payload + 12),
                            f32_le(body, payload + 16),
                        ],
                    });
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
                // KeyFrameValueSetup: opcode selects the target channel; the track id is at payload+4.
                0x27 if payload + 8 <= body.len() => scale_x_track = track_id(body, payload + 4),
                0x28 if payload + 8 <= body.len() => scale_y_track = track_id(body, payload + 4),
                0x2D if payload + 8 <= body.len() => alpha_track = track_id(body, payload + 4),
                // research/xim ParticleGeneratorParser.kt sec2Handler — 0x60..0x63 are the same
                // KeyFrameValueSetup shape bound to the time-of-day color channels, read back by
                // the section-3 ClockValueUpdater 0x3C..0x3F.
                0x60..=0x63 if payload + 8 <= body.len() => {
                    tod_color_tracks[(opcode - 0x60) as usize] = track_id(body, payload + 4);
                }
                // BlendFuncInitializer: p0 @payload+0 — high nibble bit 0x01 = opaque, else low
                // nibble selects (0x8 additive, 0x4/0x6 alpha blend, 0x1/0x2 reverse-subtract).
                0x1E if payload < body.len() => {
                    let p0 = body[payload];
                    blend_byte = p0;
                    blend = if (p0 >> 4) & BLEND_FUNC_OPAQUE_BIT != 0 {
                        ParticleBlend::Blend
                    } else {
                        match p0 & BLEND_FUNC_MODE_MASK {
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
        let mut day_of_week_color = None;
        let mut moon_phase_color = None;
        let mut moon_phase_sprite = false;
        let mut tod_color_driven = [false; TOD_COLOR_CHANNELS];
        let sec3_raw = u32_le(body, 0x78) as usize;
        if sec3_raw >= CHUNK_HEADER_LEN && sec3_raw - CHUNK_HEADER_LEN < body.len() {
            let mut cursor = sec3_raw - CHUNK_HEADER_LEN;
            while cursor + 4 <= body.len() {
                let cfg = u32_le(body, cursor);
                let opcode = (cfg & OPCODE_MASK) as u8;
                let size_words = ((cfg >> 8) & u32::from(SIZE_WORDS_MASK)) as usize;
                if opcode == OPCODE_END || size_words == 0 {
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
                    // research/xim ParticleGeneratorParser.kt sec3Handler ClockValueUpdater — these
                    // carry no payload; they mark which 0x60..0x63 track drives its channel.
                    0x3C..=0x3F => tod_color_driven[(opcode - 0x3C) as usize] = true,
                    // research/xim ParticleGeneratorParser.kt sec3Handler MoonPhaseSpriteSheetUpdater.
                    0x45 => moon_phase_sprite = true,
                    // research/xim ParticleUpdaters.kt DayOfWeekColorUpdater: expectZero32
                    // then 8 RGBA quads (u8x4, 0..=255). payload+0 is the zero u32.
                    0x4E if payload + 4 + 4 * DAYS_OF_WEEK <= body.len() => {
                        day_of_week_color =
                            Some(std::array::from_fn(|i| rgba_u8(body, payload + 4 + i * 4)));
                    }
                    // research/xim ParticleUpdaters.kt MoonPhaseColorUpdater: same shape,
                    // 12 quads.
                    0x4F if payload + 4 + 4 * MOON_PHASES <= body.len() => {
                        moon_phase_color =
                            Some(std::array::from_fn(|i| rgba_u8(body, payload + 4 + i * 4)));
                    }
                    _ => {}
                }
                cursor += block_len;
            }
        }

        // Section 1 (body[0x70]) — generator-level per-frame updaters, the same block framing.
        let mut emit_cull = None;
        let sec1_raw = u32_le(body, 0x70) as usize;
        if sec1_raw >= CHUNK_HEADER_LEN && sec1_raw - CHUNK_HEADER_LEN < body.len() {
            let mut cursor = sec1_raw - CHUNK_HEADER_LEN;
            while cursor + 4 <= body.len() {
                let cfg = u32_le(body, cursor);
                let opcode = (cfg & OPCODE_MASK) as u8;
                let size_words = ((cfg >> 8) & u32::from(SIZE_WORDS_MASK)) as usize;
                if opcode == OPCODE_END || size_words == 0 {
                    break;
                }
                let block_len = size_words * 4;
                let payload = cursor + 4;
                if cursor + block_len > body.len() {
                    break;
                }
                if opcode == SEC1_OPCODE_EMIT_CULL && payload + 12 <= body.len() {
                    emit_cull = Some(EmitCull {
                        max_distance: f32_le(body, payload),
                        min_distance: f32_le(body, payload + 4),
                        unlink_out_of_range: u32_le(body, payload + 8) & 1 != 0,
                    });
                }
                cursor += block_len;
            }
        }

        Ok(Some(Self {
            frames_per_emission,
            particles_per_emission,
            emission_variance,
            mesh_id,
            mesh_kind,
            base_position,
            max_life_frames,
            camera_billboard,
            billboard,
            camera_relative: follow_camera || camera_attached_base,
            follow_camera,
            camera_attached_base,
            position_variance,
            continuous,
            auto_run,
            batched,
            attach_type,
            attach_joint_source,
            attach_joint_target,
            attach_source_oriented,
            init_scale,
            init_color,
            init_velocity,
            init_rotation,
            blend,
            blend_byte,
            ignore_texture_alpha,
            scale_x_track,
            scale_y_track,
            alpha_track,
            day_of_week_color,
            moon_phase_color,
            tod_color_tracks,
            tod_color_driven,
            moon_phase_sprite,
            uv_scroll,
            accel,
            emit_cull,
        }))
    }

    pub fn is_singleton(&self) -> bool {
        self.max_life_frames == 0.0
    }
}

// research/XIClient/src/XIClient/include/Resource/ResourceType.h `Sep = 61`, dispatched
// at CYyGenerator.cpp HandleOne (`modelType` = the same setup byte payload+29 the particle kinds
// come from) and :193 (`case Sep: elem = new CYySoundElem()`).
pub(crate) const LINKED_DATA_SOUND: u8 = 0x3D;

// research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp CYyGenerator::ElemGenerate 0x4Cu —
// initializer 0x4C is the sound elem's setup: `s_far = fpos[1]`, `s_near = fpos[2]`, and
// `s_width = 0.0` unconditionally, so the third shipped word (non-zero in 22 of the 5,895
// generators) is discarded rather than read.
const SOUND_SETUP_OPCODE: u8 = 0x4C;

/// A 0x05 Generator whose setup links a 0x3D `Sep` — a placed sound emitter rather than a
/// particle. [`ParticleGeneratorDef::parse`] rejects the same chunks, so the two views
/// never overlap.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SoundGeneratorDef {
    pub sep_id: [u8; 4],
    pub base_position: [f32; 3],

    /// Retail's `CYySoundElem::s_far` / `s_near`. A shipped 0.0 is not "silent" — Calc3D
    /// substitutes the class defaults (CYySepRes.cpp CYySepRes::Calc3D), which 591 generators rely on.
    pub far: f32,
    pub near: f32,

    /// CYyGenerator.cpp CYyGenerator::Idle — the re-emission period is
    /// `frames_per_emission + uirand(emission_variance)`.
    pub frames_per_emission: f32,
    pub emission_variance: f32,

    pub auto_run: bool,

    /// CYyGenerator.cpp CYyGenerator::IsNever `IsNever()` — `flags & 0x400` (continuous) or a zero
    /// life. Such a generator never runs the timed emission loop at all: :2789-2794 emits a
    /// single elem and only re-emits once that one is gone.
    pub continuous: bool,
    pub max_life_frames: f32,

    pub attach_type: AttachType,
}

impl SoundGeneratorDef {
    pub fn parse(body: &[u8]) -> Result<Option<Self>> {
        if body.len() < HEADER_LEN {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: HEADER_LEN,
                available: body.len(),
            });
        }

        let attach_flags = u16_le(body, 0x00);
        let flags = u32_le(body, GEN_FLAGS_OFFSET);

        let sec2_raw = u32_le(body, 0x74) as usize;
        if sec2_raw < CHUNK_HEADER_LEN || sec2_raw - CHUNK_HEADER_LEN >= body.len() {
            return Ok(None);
        }
        let mut cursor = sec2_raw - CHUNK_HEADER_LEN;

        let mut is_sound = false;
        let mut sep_id = [0u8; 4];
        let mut base_position = [0.0f32; 3];
        let mut max_life_frames = 0.0f32;
        let mut far = 0.0f32;
        let mut near = 0.0f32;

        while cursor + 4 <= body.len() {
            let cfg = u32_le(body, cursor);
            let opcode = (cfg & OPCODE_MASK) as u8;
            let size_words = ((cfg >> 8) & u32::from(SIZE_WORDS_MASK)) as usize;
            if opcode == OPCODE_END || size_words == 0 {
                break;
            }
            let block_len = size_words * 4;
            let payload = cursor + 4;
            if cursor + block_len > body.len() {
                break;
            }
            match opcode {
                0x01 if payload + 32 <= body.len() => {
                    sep_id = [
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
                    is_sound = body[payload + 29] == LINKED_DATA_SOUND;
                    max_life_frames = u16_le(body, payload + 30) as f32;
                }
                SOUND_SETUP_OPCODE if payload + 8 <= body.len() => {
                    far = f32_le(body, payload);
                    near = f32_le(body, payload + 4);
                }
                _ => {}
            }
            cursor += block_len;
        }

        if !is_sound {
            return Ok(None);
        }

        Ok(Some(Self {
            sep_id,
            base_position,
            far,
            near,
            frames_per_emission: u16_le(body, 0x66) as f32 + 1.0,
            emission_variance: u16_le(body, 0x64) as f32,
            auto_run: flags & GEN_FLAG_AUTO_RUN != 0,
            continuous: flags & GEN_FLAG_CONTINUOUS != 0,
            max_life_frames,
            attach_type: AttachType::from_flag(attach_flags & ATTACH_TYPE_MASK).unwrap_or_default(),
        }))
    }

    pub fn is_placed(&self) -> bool {
        self.base_position != [0.0, 0.0, 0.0]
    }

    /// CYyGenerator.cpp CYyGenerator::IsNever + :2789-2794 — a "never" generator holds exactly one live
    /// elem and re-emits only once it is gone, instead of running the timed emission loop.
    pub fn is_singleton(&self) -> bool {
        self.continuous || self.max_life_frames == 0.0
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
pub(crate) mod test_support {
    use super::*;

    // Build a generator body matching the real layout: header at 0x64, section-2 offset word at
    // body[0x74] (value = body_index + 0x10), then the initializer opcode stream. `flags` is the
    // whole u32 at body[0x68] — particle count in the low 9 bits, gen flags above it.
    pub(crate) fn build(sec2: &[u8], frames_per_em: u16, flags: u32) -> Vec<u8> {
        build_attached(sec2, frames_per_em, flags, 0, 0)
    }

    pub(crate) fn build_attached(
        sec2: &[u8],
        frames_per_em: u16,
        flags: u32,
        attach_flags: u16,
        additional_attach: u16,
    ) -> Vec<u8> {
        let mut body = vec![0u8; HEADER_LEN];
        body[0x00..0x02].copy_from_slice(&attach_flags.to_le_bytes());
        body[0x02..0x04].copy_from_slice(&additional_attach.to_le_bytes());
        body[0x66..0x68].copy_from_slice(&(frames_per_em - 1).to_le_bytes());
        body[GEN_FLAGS_OFFSET..GEN_FLAGS_OFFSET + 4].copy_from_slice(&flags.to_le_bytes());
        let sec2_body_index = HEADER_LEN;
        body[0x74..0x78].copy_from_slice(&((sec2_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(sec2);
        body
    }

    pub(crate) fn op(opcode: u8, size_words: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![opcode, size_words, 0, 0];
        v.extend_from_slice(payload);
        v.resize(size_words as usize * 4, 0);
        v
    }

    // A generator whose section-3 stream carries the celestial updaters: 0x45
    // MoonPhaseSpriteSheetUpdater when `moon_phase_sprite`, then the 0x4E/0x4F color tables
    // (each an expectZero32 followed by RGBA u8 quads).
    pub(crate) fn celestial_generator_body(
        moon_phase_sprite: bool,
        day_of_week: &[[u8; 4]; DAYS_OF_WEEK],
        moon_phase: &[[u8; 4]; MOON_PHASES],
    ) -> Vec<u8> {
        let mut sec2 = op(0x01, 12, &[]);
        sec2[4 + 29] = LINKED_DATA_STATIC_MESH;
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&[0u8; 4]);
        let sec3_at = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_at + 0x10) as u32).to_le_bytes());

        let table = |opcode: u8, quads: &[[u8; 4]]| {
            let mut p = vec![0u8; 4];
            p.extend(quads.iter().flatten());
            let size_words = (4 + p.len()).div_ceil(4) as u8;
            op(opcode, size_words, &p)
        };
        let mut sec3 = Vec::new();
        if moon_phase_sprite {
            sec3.extend(op(0x45, 1, &[]));
        }
        sec3.extend(table(0x4E, day_of_week));
        sec3.extend(table(0x4F, moon_phase));
        body.extend_from_slice(&sec3);
        body
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

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
        setup[4 + 29] = LINKED_DATA_STATIC_MESH;
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
        assert_eq!(def.particles_per_emission, 0);
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
        setup[4 + 29] = LINKED_DATA_STATIC_MESH;
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

    // The celestial opcodes live in the section-3 updater stream (body[0x78]), NOT the
    // section-2 initializer stream, where 0x4E/0x4F mean FixedPointPositionVarianceSetup and
    // 0x45 means ParentPositionCopyConfig (research/xim ParticleGeneratorParser.kt sec2Handler,
    // 239 vs 444, 454-455). Reading them from the wrong stream silently yields None on every
    // real DAT, which is what left the moon on hand-tuned fallback tints.
    #[test]
    fn celestial_updaters_come_from_section3_only() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 29] = LINKED_DATA_STATIC_MESH;

        let dow = |base: u8| {
            let mut p = vec![0u8; 4];
            p.extend((0..DAYS_OF_WEEK as u8).flat_map(|i| [base + i, 0, 0, 255]));
            op(0x4E, 10, &p)
        };
        let phase = || {
            let mut p = vec![0u8; 4];
            p.extend((0..MOON_PHASES as u8).flat_map(|i| [0, 0, i, 255]));
            op(0x4F, 14, &p)
        };

        // In section 2 they must be ignored outright.
        let mut sec2 = setup.clone();
        sec2.extend(dow(0));
        sec2.extend(phase());
        sec2.extend(op(0x45, 1, &[]));
        let def = ParticleGeneratorDef::parse(&build(&sec2, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(def.day_of_week_color, None);
        assert_eq!(def.moon_phase_color, None);
        assert!(!def.moon_phase_sprite);

        // In section 3 they decode.
        let mut body = build(&setup, 1, 1);
        body.extend_from_slice(&[0u8; 4]);
        let sec3_at = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_at + 0x10) as u32).to_le_bytes());
        let mut sec3 = op(0x45, 1, &[]);
        sec3.extend(dow(16));
        sec3.extend(phase());
        body.extend_from_slice(&sec3);

        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.moon_phase_sprite, "0x45 -> moon-phase sprite frame");
        let dow = def.day_of_week_color.expect("0x4E decodes");
        assert!((dow[0][0] - 16.0 / 255.0).abs() < 1e-6);
        assert!((dow[7][0] - 23.0 / 255.0).abs() < 1e-6);
        let mp = def.moon_phase_color.expect("0x4F decodes");
        assert!((mp[11][2] - 11.0 / 255.0).abs() < 1e-6);
    }

    // research/xim ParticleGeneratorParser.kt sec2Handler — 0x60..0x63 are KeyFrameValueSetup
    // (track id at payload+4, same shape as the 0x27/0x28/0x2D life tracks) naming the
    // time-of-day RGBA curves; the section-3 ClockValueUpdater 0x3C..0x3F arms each channel.
    #[test]
    fn tod_color_tracks_pair_setup_with_updater() {
        let mut sec2 = op(0x01, 12, &[]);
        sec2[4 + 29] = LINKED_DATA_STATIC_MESH;
        for (opcode, id) in [(0x60u8, b"ksr1"), (0x61, b"ksg1"), (0x62, b"ksb1")] {
            let mut p = vec![0u8; 4];
            p.extend_from_slice(id);
            sec2.extend(op(opcode, 4, &p));
        }
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&[0u8; 4]);
        let sec3_at = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_at + 0x10) as u32).to_le_bytes());
        // Arm red and blue only: an unarmed channel keeps its track but must not be applied.
        let mut sec3 = op(0x3C, 1, &[]);
        sec3.extend(op(0x3E, 1, &[]));
        body.extend_from_slice(&sec3);

        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(
            def.tod_color_tracks,
            [Some(*b"ksr1"), Some(*b"ksg1"), Some(*b"ksb1"), None]
        );
        assert_eq!(def.tod_color_driven, [true, false, true, false]);
    }

    // Real-DAT guard for both of the above: West Ronfaure's fine-weather celestial set is the
    // canonical shape — the sun carries three time-of-day colour curves, the moon carries a
    // phase-indexed sprite plus both tint tables. If the section split regresses these all
    // go quietly empty again.
    #[test]
    fn real_dat_west_ronfaure_celestial_generators() {
        let Ok(root) = crate::DatRoot::from_env_or_default() else {
            eprintln!("skipping: no DAT root");
            return;
        };
        let Ok(loc) = root.resolve(201) else {
            eprintln!("skipping: file 201 unresolvable");
            return;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            eprintln!("skipping: file 201 unreadable");
            return;
        };

        let mut saw_sun = false;
        let mut saw_moon = false;
        for c in crate::chunk::walk(&bytes).flatten() {
            if crate::kind::ChunkKind::from_u8(c.kind) != Some(crate::kind::ChunkKind::Generator) {
                continue;
            }
            let Ok(Some(def)) = ParticleGeneratorDef::parse(c.data) else {
                continue;
            };
            match (&c.name, def.attach_type) {
                (b"sun1", AttachType::Sun) => {
                    saw_sun = true;
                    assert_eq!(
                        def.tod_color_driven[..3],
                        [true, true, true],
                        "sun1 drives r/g/b from time-of-day curves"
                    );
                    assert!(def.tod_color_tracks[..3].iter().all(Option::is_some));
                }
                (b"moon", AttachType::Moon) => {
                    saw_moon = true;
                    assert!(def.moon_phase_sprite, "moon picks its frame by phase");
                    assert!(def.day_of_week_color.is_some(), "moon has a 0x4E table");
                    assert!(def.moon_phase_color.is_some(), "moon has a 0x4F table");
                }
                _ => {}
            }
        }
        assert!(saw_sun, "file 201 defines a Sun-attached `sun1`");
        assert!(saw_moon, "file 201 defines a Moon-attached `moon`");
    }

    #[test]
    fn no_section3_leaves_defaults() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 29] = LINKED_DATA_STATIC_MESH;
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.uv_scroll, [0.0, 0.0]);
        assert_eq!(def.accel, None);
    }

    #[test]
    fn gen_flags_decode_auto_run_and_continuous() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 29] = LINKED_DATA_STATIC_MESH;
        let body = build(&setup, 1, GEN_FLAG_AUTO_RUN | GEN_FLAG_CONTINUOUS | 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.auto_run);
        assert!(def.continuous);
        assert!(!def.batched);
        assert_eq!(
            def.particles_per_emission, 1,
            "flag bits stay out of the count"
        );

        let def = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert!(!def.auto_run);
        assert!(!def.continuous);
    }

    // The count is 9 bits wide (CYyGenerator.cpp CYyGenerator::Idle v161 `flags & 0x1FF`), so its top bit is bit 0
    // of the byte XIM calls genFlags. Reading either as a byte truncates the primary weather
    // curtains: La Theine's `~1ra` authors 299 and an 8-bit read yields 43.
    #[test]
    fn particle_count_is_nine_bits_wide() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 29] = LINKED_DATA_STATIC_MESH;
        let def = ParticleGeneratorDef::parse(&build(&setup, 30, 0x2000_112B))
            .unwrap()
            .unwrap();
        assert_eq!(def.particles_per_emission, 299);
        assert!(def.auto_run);
        assert!(def.batched, "0x2000_0000 is CheckFlag29");
        assert!(!def.continuous);
    }

    #[test]
    fn non_particle_setup_is_none() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 29] = 0x47; // point light, not particle
        let body = build(&setup, 1, 1);
        assert!(ParticleGeneratorDef::parse(&body).unwrap().is_none());

        // 0x57 Null particle type is likewise non-visual and rejected.
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 29] = 0x57;
        let body = build(&setup, 1, 1);
        assert!(ParticleGeneratorDef::parse(&body).unwrap().is_none());
    }

    // Regression pin (Poison's venom cloud): a 0x0E SpriteSheet generator used to be dropped
    // because only 0x0B was accepted. It must now parse to Some with mesh_kind == SpriteSheet.
    #[test]
    fn sprite_sheet_setup_parses_with_mesh_kind() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 8..4 + 12].copy_from_slice(b"fir ");
        setup[4 + 29] = LINKED_DATA_SPRITE_SHEET;
        setup[4 + 30..4 + 32].copy_from_slice(&24u16.to_le_bytes());
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.mesh_kind, ParticleMeshKind::SpriteSheet);
        assert_eq!(def.mesh_id, *b"fir ");
        assert_eq!(def.max_life_frames, 24.0);
    }

    // research/xim ParticleInitializers.kt — renderStateFlags is the u16 after the
    // billboard flags; 0x1000 picks retail's NonZeroOneTSS element (CMoD3m.cpp CMoD3m::Draw).
    #[test]
    fn render_state_flag_selects_ignore_texture_alpha_element() {
        let element = |render_state: u16| {
            let mut setup = op(0x01, 12, &[]);
            setup[4 + 2..4 + 4].copy_from_slice(&render_state.to_le_bytes());
            setup[4 + 29] = LINKED_DATA_STATIC_MESH;
            let body = build(&setup, 1, 1);
            ParticleGeneratorDef::parse(&body)
                .unwrap()
                .unwrap()
                .ignore_texture_alpha
        };
        assert!(!element(0x0000));
        assert!(!element(0x0FFF), "only bit 0x1000 selects the element");
        assert!(element(0x1000));
        assert!(element(0x1200), "other render-state bits do not mask it");
    }

    // CMoD3m.cpp CMoD3m::Draw keys the TEXTUREFACTOR-alpha promotion on the exact blend byte, which
    // the ParticleBlend collapse (0x03/0x44/0x64 all -> Blend) cannot express.
    #[test]
    fn blend_byte_survives_the_blend_func_collapse() {
        let parsed = |p0: u8| {
            let mut setup = op(0x01, 12, &[]);
            setup[4 + 29] = LINKED_DATA_STATIC_MESH;
            setup.extend(op(0x1E, 2, &[p0, 0, 0, 0]));
            let body = build(&setup, 1, 1);
            let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
            (def.blend, def.blend_byte)
        };
        assert_eq!(parsed(0x44), (ParticleBlend::Blend, 0x44));
        assert_eq!(parsed(0x64), (ParticleBlend::Blend, 0x64));
        assert_eq!(parsed(0x48), (ParticleBlend::Additive, 0x48));
    }

    #[test]
    fn static_mesh_setup_reports_static_mesh_kind() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 29] = LINKED_DATA_STATIC_MESH;
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.mesh_kind, ParticleMeshKind::StaticMesh);
    }

    #[test]
    fn singleton_when_max_life_zero() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 8..4 + 12].copy_from_slice(b"sea0");
        setup[4 + 29] = LINKED_DATA_STATIC_MESH;
        // max life left at 0
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.is_singleton());
    }

    // Pins the XIM attachFlags bit layout (ParticleGeneratorParser.kt) against the
    // ground-truth word 0x5402 read out of Poison's effect DAT (file 3020).
    #[test]
    fn attach_flags_split_type_and_joints() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 29] = LINKED_DATA_STATIC_MESH;

        let body = build_attached(&setup, 1, 1, 0x5402, ATTACH_SOURCE_ORIENTED);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.attach_type, AttachType::TargetActor);
        assert_eq!(def.attach_joint_source, 0);
        assert_eq!(def.attach_joint_target, 21);
        assert!(def.attach_source_oriented);

        let body = build_attached(&setup, 1, 1, 0x5402, 0);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(!def.attach_source_oriented);

        // Joint 0 lives in bits 4..10, joint 1 in bits 10..16, type in the low nibble.
        let body = build_attached(&setup, 1, 1, 0x0409 | (7 << ATTACH_JOINT0_SHIFT), 0);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.attach_type, AttachType::SourceActorWeapon);
        assert_eq!(def.attach_joint_source, 7);
        assert_eq!(def.attach_joint_target, 1);

        // 0x7 / 0x8 / 0xD are not AttachType flags; XIM warns and falls back to None.
        for unknown in [0x7u16, 0x8, 0xD] {
            assert_eq!(AttachType::from_flag(unknown), None);
            let body = build_attached(&setup, 1, 1, unknown, 0);
            let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
            assert_eq!(def.attach_type, AttachType::None);
        }
    }

    // Real-DAT guard: every generator in Poison's completion-effect file attaches to the
    // target actor at joint 21, which is what makes the venom cloud land on the victim.
    #[test]
    fn real_dat_poison_generators_attach_to_target() {
        const POISON_EFFECT_FILE_ID: u32 = 3020;
        let Ok(root) = crate::DatRoot::from_env_or_default() else {
            return;
        };
        let Ok(loc) = root.resolve(POISON_EFFECT_FILE_ID) else {
            return;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            return;
        };
        let mut seen = 0;
        for c in crate::chunk::walk(&bytes).flatten() {
            if crate::kind::ChunkKind::from_u8(c.kind) != Some(crate::kind::ChunkKind::Generator) {
                continue;
            }
            let Ok(Some(def)) = ParticleGeneratorDef::parse(c.data) else {
                continue;
            };
            seen += 1;
            assert_eq!(
                def.attach_type,
                AttachType::TargetActor,
                "generator {}",
                String::from_utf8_lossy(&c.name)
            );
            assert_eq!(def.attach_joint_target, 21);
        }
        assert!(seen > 0, "no particle generators parsed from file 3020");
    }

    // kuluu-ln1q was filed on the premise that retail gates weat/<tag> activation on a predicate
    // other than the auto-run bit we test. It does not: WeatherTransition.cpp ActivateWeatherGenerators reads
    // `gen->flags & 0x1000`, and the ConstructFromData offset mapping puts that bit on the byte
    // XIM calls genFlags. Pin the two views onto one field so nobody re-derives it.
    #[test]
    fn real_dat_auto_run_is_the_retail_weather_activation_bit() {
        const XIM_GEN_FLAGS_BYTE: usize = GEN_FLAGS_OFFSET + 1;
        const XIM_GEN_FLAG_AUTO_RUN: u8 = 0x10;
        let Some(bytes) = real_zone_dat(LA_THEINE_ZONE_DAT) else {
            return;
        };
        let mut seen = 0;
        for c in crate::chunk::walk(&bytes).flatten() {
            if crate::kind::ChunkKind::from_u8(c.kind) != Some(crate::kind::ChunkKind::Generator) {
                continue;
            }
            let Ok(Some(def)) = ParticleGeneratorDef::parse(c.data) else {
                continue;
            };
            seen += 1;
            assert_eq!(
                def.auto_run,
                c.data[XIM_GEN_FLAGS_BYTE] & XIM_GEN_FLAG_AUTO_RUN != 0,
                "generator {}",
                String::from_utf8_lossy(&c.name)
            );
        }
        assert!(
            seen > 0,
            "no particle generators in DAT {LA_THEINE_ZONE_DAT}"
        );
    }

    const LA_THEINE_ZONE_DAT: u32 = 202;

    fn real_zone_dat(file_id: u32) -> Option<Vec<u8>> {
        let root = crate::DatRoot::from_env_or_default().ok()?;
        let loc = root.resolve(file_id).ok()?;
        std::fs::read(loc.path_under(&root)).ok()
    }

    // La Theine's rain curtain is the canonical precipitation generator: camera-following, a
    // sprite-sheet flipbook, a 9-bit particle count, a 20-unit spawn sphere and downward
    // FFXI-frame velocity + gravity. Every one of those is a field this module had to learn to
    // read; if any silently regresses to a default the rain goes back to a point emitter.
    #[test]
    fn real_dat_la_theine_rain_curtain() {
        let Some(bytes) = real_zone_dat(LA_THEINE_ZONE_DAT) else {
            return;
        };
        let mut found = false;
        for c in crate::chunk::walk(&bytes).flatten() {
            if c.name != *b"~1ra"
                || crate::kind::ChunkKind::from_u8(c.kind)
                    != Some(crate::kind::ChunkKind::Generator)
            {
                continue;
            }
            let def = ParticleGeneratorDef::parse(c.data).unwrap().unwrap();
            found = true;
            assert!(def.auto_run);
            assert!(def.batched);
            assert!(def.camera_relative);
            assert!(!def.camera_billboard);
            assert_eq!(def.mesh_kind, ParticleMeshKind::SpriteSheet);
            assert_eq!(def.mesh_id, *b"rain");
            assert_eq!(def.particles_per_emission, 299);
            assert_eq!(def.frames_per_emission, 30.0);
            assert_eq!(def.max_life_frames, 60.0);
            assert_eq!(def.base_position, [0.0, -35.0, 0.0]);
            assert!((def.init_velocity[1] - 0.3).abs() < 1e-6);
            assert!((def.accel.unwrap()[1] - 0.005).abs() < 1e-6);
            let pv = def.position_variance.expect("sec2 0x06 spawn sphere");
            assert_eq!(pv.radius_variance, 20.0);
            assert_eq!(pv.base_radius, 0.0);
            assert_eq!(pv.axis_scale, [1.0; 3]);
        }
        assert!(found, "DAT {LA_THEINE_ZONE_DAT} defines weat/rain/~1ra");
    }

    // The 0x07 form scales the offset per axis; La Theine's ground-splash rings zero the Y scale
    // so the spread is a flat ellipse on the ground rather than a ball around the emitter.
    #[test]
    fn real_dat_la_theine_rain_splash_spreads_flat() {
        let Some(bytes) = real_zone_dat(LA_THEINE_ZONE_DAT) else {
            return;
        };
        let mut found = false;
        for c in crate::chunk::walk(&bytes).flatten() {
            if c.name != *b"~1h1"
                || crate::kind::ChunkKind::from_u8(c.kind)
                    != Some(crate::kind::ChunkKind::Generator)
            {
                continue;
            }
            let def = ParticleGeneratorDef::parse(c.data).unwrap().unwrap();
            found = true;
            assert!(!def.camera_relative, "splashes are placed in the world");
            let pv = def.position_variance.expect("sec2 0x07 spawn ellipse");
            assert!((pv.max_radius() - 10.0).abs() < 1e-4);
            assert_eq!(pv.axis_scale[1], 0.0);
            assert_eq!(pv.offset(1.0, 0.0, std::f32::consts::FRAC_PI_2)[1], 0.0);
        }
        assert!(found, "DAT {LA_THEINE_ZONE_DAT} defines weat/rain/~1h1");
    }

    // CYyGenerator.cpp CYyGenerator::ElemGenerate assigns far from the FIRST 0x4C word and near from the
    // second, and 25 shipped generators author near > far — swapping them would make those
    // silent everywhere instead of loud everywhere inside far.
    #[test]
    fn sound_setup_reads_far_then_near_and_ignores_the_third_word() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 8..4 + 12].copy_from_slice(b"2024");
        setup[4 + 29] = LINKED_DATA_SOUND;
        setup[4 + 16..4 + 20].copy_from_slice(&(-293.5f32).to_le_bytes());
        let mut p = Vec::new();
        p.extend_from_slice(&50.0f32.to_le_bytes());
        p.extend_from_slice(&30.0f32.to_le_bytes());
        p.extend_from_slice(&6.0f32.to_le_bytes());
        setup.extend(op(SOUND_SETUP_OPCODE, 4, &p));

        let body = build(&setup, 30, GEN_FLAG_AUTO_RUN);
        let def = SoundGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.sep_id, *b"2024");
        assert_eq!(def.far, 50.0);
        assert_eq!(def.near, 30.0);
        assert!(def.auto_run);
        assert!(def.is_placed());
        assert_eq!(def.frames_per_emission, 30.0);

        assert!(
            ParticleGeneratorDef::parse(&body).unwrap().is_none(),
            "a sound generator must never reach the particle sim"
        );
    }

    fn with_sec1(mut body: Vec<u8>, sec1: &[u8]) -> Vec<u8> {
        let at = body.len();
        body[0x70..0x74].copy_from_slice(&((at + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(sec1);
        body
    }

    #[test]
    fn emit_cull_reads_max_then_min_then_unlink_bit_from_section_1() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 29] = LINKED_DATA_STATIC_MESH;
        let mut p = Vec::new();
        p.extend_from_slice(&40.0f32.to_le_bytes());
        p.extend_from_slice(&(-1.0f32).to_le_bytes());
        p.extend_from_slice(&1u32.to_le_bytes());
        let mut sec1 = op(0x04, 2, &[0, 0, 0, 0]);
        sec1.extend(op(SEC1_OPCODE_EMIT_CULL, 4, &p));
        sec1.extend(op(OPCODE_END, 0, &[]));

        let body = with_sec1(build(&setup, 1, GEN_FLAG_AUTO_RUN | 1), &sec1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(
            def.emit_cull,
            Some(EmitCull {
                max_distance: 40.0,
                min_distance: -1.0,
                unlink_out_of_range: true,
            })
        );

        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, GEN_FLAG_AUTO_RUN | 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.emit_cull, None, "no section 1: never culled");
    }

    #[test]
    fn emit_cull_zero_max_defers_to_the_zone_draw_distance() {
        let authored = EmitCull {
            max_distance: 40.0,
            min_distance: -1.0,
            unlink_out_of_range: false,
        };
        assert!(!authored.out_of_range(40.0, 80.0));
        assert!(authored.out_of_range(40.5, 80.0));

        let zone = EmitCull {
            max_distance: 0.0,
            ..authored
        };
        assert!(!zone.out_of_range(79.0, 80.0));
        assert!(zone.out_of_range(81.0, 80.0));

        let near = EmitCull {
            min_distance: 5.0,
            ..authored
        };
        assert!(near.out_of_range(4.0, 80.0), "inside the near band");
        assert!(!near.out_of_range(5.0, 80.0));
    }

    #[test]
    fn particle_setups_are_not_sound_generators() {
        let mut setup = op(0x01, 12, &[]);
        setup[4 + 29] = LINKED_DATA_STATIC_MESH;
        let body = build(&setup, 1, 1);
        assert!(SoundGeneratorDef::parse(&body).unwrap().is_none());
    }

    // West Ronfaure's waterfall spray (`taki/sef1`, a looping cue at far 30 / near 3) and
    // its bird calls (`aose/mb01`, a one-shot at far 10 with near left at 0 so Calc3D
    // substitutes the 3.0 default).
    #[test]
    fn real_dat_west_ronfaure_placed_sound_generators() {
        const WEST_RONFAURE_ZONE_DAT: u32 = 200;
        let Some(bytes) = real_zone_dat(WEST_RONFAURE_ZONE_DAT) else {
            return;
        };
        let mut waterfall = 0;
        let mut birds = 0;
        for c in crate::chunk::walk(&bytes).flatten() {
            if crate::kind::ChunkKind::from_u8(c.kind) != Some(crate::kind::ChunkKind::Generator) {
                continue;
            }
            let Ok(Some(def)) = SoundGeneratorDef::parse(c.data) else {
                continue;
            };
            if c.name == *b"sef1" {
                waterfall += 1;
                assert_eq!(def.sep_id, *b"2024");
                assert_eq!((def.far, def.near), (30.0, 3.0));
                assert!(def.is_placed());
                assert!(def.auto_run);
            }
            if c.name == *b"mb01" {
                birds += 1;
                assert_eq!(def.sep_id, *b"2084");
                assert_eq!((def.far, def.near), (10.0, 0.0));
                assert!(def.is_placed());
            }
        }
        assert!(waterfall >= 1, "f_ro/mode/ligh/taki/sef1");
        assert!(birds >= 1, "f_ro/effe/aose/mb01");
    }

    // Census guard over every shipped zone DAT: 5,895 sound generators, 5,735 of them
    // placed, and every one carrying a 0x4C block.
    #[test]
    fn real_dat_sound_generator_census() {
        const MIN_SOUND_GENERATORS: usize = 5800;
        const MIN_PLACED: usize = 5700;
        let Ok(root) = crate::DatRoot::from_env_or_default() else {
            return;
        };
        let mut seen = std::collections::HashSet::new();
        let (mut total, mut placed) = (0usize, 0usize);
        for &(_zone, file_id) in crate::zone_dat::ZONE_DAT_TABLE {
            if !seen.insert(file_id) {
                continue;
            }
            let Ok(loc) = root.resolve(file_id) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
                continue;
            };
            for c in crate::chunk::walk(&bytes).flatten() {
                if crate::kind::ChunkKind::from_u8(c.kind)
                    != Some(crate::kind::ChunkKind::Generator)
                {
                    continue;
                }
                if let Ok(Some(def)) = SoundGeneratorDef::parse(c.data) {
                    total += 1;
                    placed += usize::from(def.is_placed());
                }
            }
        }
        assert!(total >= MIN_SOUND_GENERATORS, "sound generators: {total}");
        assert!(placed >= MIN_PLACED, "placed: {placed}");
    }

    #[test]
    fn position_variance_spreads_over_the_full_radius() {
        let pv = PositionVariance {
            radius_variance: 20.0,
            base_radius: 4.0,
            axis_scale: [1.0; 3],
        };
        assert_eq!(pv.max_radius(), 24.0);
        let len = |o: [f32; 3]| (o[0] * o[0] + o[1] * o[1] + o[2] * o[2]).sqrt();
        assert!(
            len(pv.offset(0.0, 1.0, 0.5)).abs() < 1e-5,
            "u=0 is the origin"
        );
        assert!((len(pv.offset(1.0, 1.0, 0.5)) - 24.0).abs() < 1e-4);
        assert!((len(pv.offset(0.5, -2.0, 1.2)) - 12.0).abs() < 1e-4);
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
    fn billboard_flag_ladder_matches_xim() {
        use ParticleBillboard::*;
        for (flags, want) in [
            (0x0000u16, None),
            (0x00C0, Camera),
            (0x00C1, Camera),
            (0x0081, Movement),
            (0x0080, MovementHorizontal),
            (0x0040, Movement),
            (0x4000, Xz),
            (0x0001, Xyz),
        ] {
            assert_eq!(
                ParticleBillboard::from_flags(flags),
                want,
                "flags 0x{flags:04X}",
            );
        }
    }

    const WEST_RONFAURE_ZONE_DAT: u32 = 201;

    // The two modes must not re-collapse into one bool: `sun0`/`kasa` are BillBoardType::Camera,
    // an axially-oriented solid whose mesh-local +X aims at the eye, while the moon sprite is
    // BillBoardType::XYZ, a flat screen billboard (research/xim GLDrawer.kt drawXimParticle). Drawing
    // the first as the second flattens the sun/moon glow domes into sky-filling sails.
    #[test]
    fn real_dat_celestial_billboard_modes_split() {
        let Some(bytes) = real_zone_dat(WEST_RONFAURE_ZONE_DAT) else {
            return;
        };
        let mut seen = [0usize; 3];
        for c in crate::chunk::walk(&bytes).flatten() {
            if crate::kind::ChunkKind::from_u8(c.kind) != Some(crate::kind::ChunkKind::Generator) {
                continue;
            }
            let Ok(Some(def)) = ParticleGeneratorDef::parse(c.data) else {
                continue;
            };
            match &c.name {
                b"sun0" => {
                    seen[0] += 1;
                    assert_eq!(def.billboard, ParticleBillboard::Camera);
                    assert_eq!(def.mesh_kind, ParticleMeshKind::StaticMesh);
                    assert_eq!(def.mesh_id, *b"suns");
                    assert_eq!(def.init_scale, [70.0; 3]);
                }
                b"kasa" => {
                    seen[1] += 1;
                    assert_eq!(def.billboard, ParticleBillboard::Camera);
                    assert_eq!(def.mesh_kind, ParticleMeshKind::StaticMesh);
                    assert_eq!(def.mesh_id, *b"moon");
                    assert_eq!(def.init_scale, [20.0; 3]);
                }
                b"moon" => {
                    seen[2] += 1;
                    assert_eq!(def.billboard, ParticleBillboard::Xyz);
                    assert_eq!(def.mesh_kind, ParticleMeshKind::SpriteSheet);
                }
                _ => {}
            }
        }
        assert_eq!(
            seen,
            [1, 2, 2],
            "f_ro suny/sun0, {{fine,suny}}/moon/{{kasa,moon}}"
        );
    }

    // Why this is pinned (kuluu-nykm): the client used to resolve the sun billboard from a 0x21
    // sprite sheet whose texture category is "suns"/"suny". No shipped zone DAT has one — a
    // survey over all 298 resolvable zone DATs found zero — so that path was dead and the sun
    // silently stayed a procedural primitive. Retail's sun art is these Sun-attached StaticMesh
    // generators plus the lf0x screen-space flare chain. If this test ever fails the sprite-sheet
    // hypothesis is worth revisiting; until then it must not be re-added on a hunch.
    #[test]
    fn real_dat_sun_is_a_sun_attached_static_mesh_not_a_sprite_sheet() {
        let Some(bytes) = real_zone_dat(WEST_RONFAURE_ZONE_DAT) else {
            return;
        };
        let mut sun_meshes = 0usize;
        for c in crate::chunk::walk(&bytes).flatten() {
            match crate::kind::ChunkKind::from_u8(c.kind) {
                Some(crate::kind::ChunkKind::Generator) => {
                    let Ok(Some(def)) = ParticleGeneratorDef::parse(c.data) else {
                        continue;
                    };
                    if def.attach_type != AttachType::Sun {
                        continue;
                    }
                    if !matches!(&c.name, b"sun0" | b"sun1") {
                        continue;
                    }
                    sun_meshes += 1;
                    assert_eq!(
                        def.mesh_kind,
                        ParticleMeshKind::StaticMesh,
                        "sun generator {} is an MMB, not a sprite sheet",
                        String::from_utf8_lossy(&c.name)
                    );
                    // `suns` under fine/suny weather, `sun2` under the overcast variants.
                    assert!(
                        matches!(&def.mesh_id, b"suns" | b"sun2"),
                        "unexpected sun mesh {}",
                        String::from_utf8_lossy(&def.mesh_id)
                    );
                }
                Some(crate::kind::ChunkKind::SpriteSheet) => {
                    let Some(sheet) = crate::sprite_sheet::ParticleSpriteSheet::parse(c.data)
                    else {
                        continue;
                    };
                    assert!(
                        sheet.category != "suns" && sheet.category != "suny",
                        "unexpected sun sprite sheet: {}/{}",
                        sheet.category,
                        sheet.id
                    );
                }
                _ => {}
            }
        }
        assert!(sun_meshes > 0, "file 201 defines Sun-attached sun0/sun1");
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
