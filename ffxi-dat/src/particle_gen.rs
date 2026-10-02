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
/// The generator opcode streams a caller can be told about when a block is decoded by no arm.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum GeneratorSection {
    /// Section 1 — generator-level per-frame updaters.
    Setup,
    /// Section 2 — particle initializers.
    Initializers,
    /// Section 3 — per-frame particle updaters.
    Updaters,
    /// Section 4 — the element-die script.
    ElementDie,
    /// `SoundGeneratorDef`'s section 2.
    SoundSetup,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum GeneratorOpcodeOutcome {
    /// A section arm read the block's payload.
    Decoded,
    /// No arm matched, or the arm's length guard failed: the block's payload is discarded.
    Dropped,
}

/// Notified once per generator block, so a caller can measure what the parser understands and
/// what it discards instead of inferring either from a missing visual.
pub type GeneratorOpcodeSink<'a> = &'a mut dyn FnMut(GeneratorSection, u8, GeneratorOpcodeOutcome);

/// A generator chunk is offered to every def parser in turn and only one claims it, so a block
/// is only honestly this parse's business once the parse that saw it returns a def. Blocks are
/// buffered until then; a sound generator must not report its whole stream as particle
/// initializers.
pub(crate) fn flush_blocks(sink: GeneratorOpcodeSink<'_>, blocks: &[(GeneratorSection, u8, bool)]) {
    for &(section, opcode, decoded) in blocks {
        sink(
            section,
            opcode,
            if decoded {
                GeneratorOpcodeOutcome::Decoded
            } else {
                GeneratorOpcodeOutcome::Dropped
            },
        );
    }
}

const HEADER_LEN: usize = 0x80;
const CHUNK_HEADER_LEN: usize = 0x10;
const OPCODE_MASK: u32 = 0xFF;
// research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp Get11FC.
const ALLOCATION_SHIFT: u32 = 13;
const ALLOCATION_MASK: u32 = 0x3F;
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
pub const ATTACH_JOINT0_MASK: u16 = 0x03F0;
pub const ATTACH_JOINT0_SHIFT: u32 = 4;
pub const ATTACH_JOINT1_MASK: u16 = 0xFC00;
pub const ATTACH_JOINT1_SHIFT: u32 = 10;
const ATTACH_SOURCE_ORIENTED: u16 = 0x0001;

// research/xim ParticleInitializers.kt — the StandardParticleSetup renderStateFlags u16
// sits directly after the billboard flags. Bit 0x1000 (`ignoreTextureAlpha`) is the same bit
// retail tests as `field_10C & 0x10000000` to pick the D3m element's texture-stage table.
// research/XIClient/src/XIClient/source/Resource/Derived/CMoD3m.cpp CMoD3m::Draw
const RENDER_STATE_IGNORE_TEXTURE_ALPHA: u16 = 0x1000;
// research/XIClient/src/XIClient/source/World/Generator/Effects/CMoElem.cpp CMoElem::PrepDX —
// `field_10C & 0x2000000` turns D3DRS_FOGENABLE off for the element; every other element takes
// the area's fog colour and range. research/xim ParticleInitializers.kt read `fogEnabled`.
const RENDER_STATE_FOG_DISABLED: u16 = 0x0200;
// CMoElem.cpp CMoElem::OnDraw — the ordering-table key is `field_128 - depth` by default, the
// constant `field_128` alone under 0x0040 (research/xim ParticleInitializers.kt read
// `drawPriorityOffset`), and the depth key plus 400 under 0x0800 (`lowPriorityDraw`), which is
// tested last and so wins over the constant key.
const RENDER_STATE_PINNED_DRAW: u16 = 0x0040;
const RENDER_STATE_LOW_PRIORITY_DRAW: u16 = 0x0800;
// CMoElem.cpp CMoElem::PrepDX — `field_10C & 0x1000`, a billboard-word bit, turns
// D3DRS_ZWRITEENABLE on for the element.
const BILLBOARD_DEPTH_WRITE: u16 = 0x1000;

/// Where an element ranks among translucent draws (CMoElem.cpp CMoElem::OnDraw).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrawPriority {
    /// Key = `sort_offset - view depth`: back to front with everything else.
    #[default]
    Depth,
    /// Key = `sort_offset` alone: pinned at the table's near end, after the depth-sorted set.
    Pinned,
    /// Key = depth key + 400: drawn as if far behind everything else.
    Low,
}
// research/xim ParticleInitializers.kt read `cameraAttachedBasePosition`.
const RENDER_STATE_CAMERA_ATTACHED_BASE: u16 = 0x0400;
// research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp CYyGenerator::HandleOne — the
// StandardSetup dword's 0x01000000 (0x00200000 CMoD3mSpecialElem and 0x00100000 CMoDistModelElem
// are tested first) hands the element to CMoD3mSpecularElem; it is the high u16's 0x0100 here.
const RENDER_STATE_SPECULAR_ELEMENT: u16 = 0x0100;

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

/// sec2 0x1F SphericalPositionVarianceFull: a spherical spawn spread whose ring azimuth is
/// either a random draw or one of a fixed number of evenly spaced steps; the ring can be
/// tilted and, with the camera flag, authored in the camera's frame (CYyGenerator.cpp
/// CYyGenerator::ElemGenerate case 0x1F).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SphericalPositionVarianceFull {
    pub radius_variance: f32,
    pub base_radius: f32,
    pub axis_scale: [f32; 3],
    pub rotation_z: f32,
    pub rotation_y: f32,
    pub tilt: f32,
    pub tilt_variance: f32,
    pub camera_oriented: bool,
    // 0 = random azimuth draw; k = k evenly spaced azimuth steps (CYyGenerator.cpp
    // CYyGenerator::ElemGenerate case 0x1F).
    pub azimuth_steps: u32,
}

impl SphericalPositionVarianceFull {
    pub fn max_radius(&self) -> f32 {
        self.radius_variance + self.base_radius
    }

    // `unit_radius` in 0..=1 is retail's `ufrand(A + B)` draw; `azimuth` and `tilt` are the
    // resolved angles (the caller does the step-or-random azimuth draw and the
    // `frand(tilt_variance)` tilt draw). The chain is retail's Rz(tilt) x Ry(azimuth) x S on
    // (r, 0, 0) in the D3D row-vector convention (CYyGenerator.cpp CYyGenerator::ElemGenerate
    // case 0x1F): the authored outer rotation is identity in every shipped block and only the
    // x-axis scale can act on an x-axis offset, so neither is observable here.
    pub fn offset(&self, unit_radius: f32, azimuth: f32, tilt: f32) -> [f32; 3] {
        let r = self.max_radius() * unit_radius * self.axis_scale[0];
        let (sa, ca) = azimuth.sin_cos();
        let (st, ct) = tilt.sin_cos();
        [r * ct * ca, r * st, -r * ct * sa]
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

    // sec2 0x1F SphericalPositionVarianceFull: the spherical spawn spread whose azimuth is a
    // random draw or one of the generator's evenly spaced steps (CYyGenerator.cpp
    // CYyGenerator::ElemGenerate case 0x1F — the stepped azimuth indexes the generator's
    // element counter; the camera flag maps the ring into the camera's frame).
    pub spherical_full: Option<SphericalPositionVarianceFull>,

    pub continuous: bool,
    pub auto_run: bool,
    pub batched: bool,

    pub attach_type: AttachType,
    pub attach_joint_source: u8,
    pub attach_joint_target: u8,
    pub attach_source_oriented: bool,

    pub init_scale: [f32; 3],

    // sec2 0x11 SingleScaleVarianceInitializer: one ufrand(payload) draw shared by every scale
    // axis, per particle (research/xim ParticleInitializers.kt — scale += posRand(v);
    // CYyGenerator.cpp CYyGenerator::ElemGenerate case 0x11 — a single ufrand added to x, y, z).
    pub single_scale_variance: Option<f32>,
    // sec2 0x10 ScaleVarianceInitializer: three floats, the per-axis ufrand bound added to the
    // 0x0F base scale per particle (CYyGenerator.cpp CYyGenerator::ElemGenerate case 0x10 —
    // field_EC.x/y/z += ufrand(payload); research/xim ParticleInitializers.kt
    // ScaleVarianceInitializer — scale += variance * posRand(1f) per axis).
    pub scale_variance: Option<[f32; 3]>,
    pub init_color: [f32; 4],
    // sec2 0x17 ColorVarianceSetup: four bytes (R,G,B,A) / 255 — the per-channel bound of the
    // upward color draw added to the 0x16 base per particle (research/xim
    // ParticleInitializers.kt ColorVarianceSetup — color.rgba[i] += (byte/255) * posRand(1f),
    // one [0, 1) draw per channel; the retail decompile's ElemGenerate has no 0x17 case, so
    // xim's mapping is the available evidence).
    pub color_variance: Option<[f32; 4]>,
    // sec2 0x19 ColorTransformSetup: four i16s (r,g,b,a) written to the element's allocation
    // slot. Parsed, not applied: the retail decompile's ElemGenerate has no 0x19 case
    // (XICLIENT_CODE_MISSING) and xim allocates the transform but its drawers never read it
    // (research/xim ParticleInitializers.kt ColorTransformSetup — particle.allocate only), so
    // the application is unknown and the shipped alpha is always 0.
    pub color_transform: Option<[i16; 4]>,
    // sec3 0x0C ColorTransformModifier: four i16s [r, g, b, a] — the per-frame rate on the
    // sec2 0x19 color transform over the particle's life (research/xim
    // ParticleUpdaters.kt ColorTransformModifier — colorTransform += floor(modifier ×
    // frames/30) per frame). The engine does not model the color transform's
    // application, so parse-only.
    pub color_transform_modifier: Option<[i16; 4]>,
    pub init_velocity: [f32; 3],
    // sec2 0x03 VelocityVarianceSetup (position): the per-axis bound of the uniform random
    // velocity added to the 0x02 base per particle (research/xim ParticleInitializers.kt
    // VelocityVarianceSetup — the allocationOffset binds it to the position transform).
    pub velocity_variance: Option<[f32; 3]>,
    // sec2 0x08 RelativeVelocitySetup: the magnitude of the per-particle velocity added along
    // the spawn offset's direction (research/xim ParticleInitializers.kt RelativeVelocitySetup —
    // direction = normalize of the initial position relative to the spawn point; research/XIClient
    // CYyGenerator.cpp CYyGenerator::ElemGenerate case 0x08 normalizes field_54 minus the
    // position captured at element spawn, i.e. the offsets the earlier blocks added).
    pub relative_velocity: Option<f32>,
    // sec2 0x41 RelativeVelocityVarianceSetup: the bound of the uniform random magnitude added
    // to the 0x08 relative velocity along the spawn offset's direction per particle
    // (research/xim ParticleInitializers.kt RelativeVelocityVarianceSetup; the retail
    // decompile's CYyGenerator.cpp CYyGenerator::ElemGenerate case 0x41 scales the normalized
    // spawn offset by frand of this value and adds it to the same allocation vector as 0x08).
    pub relative_velocity_variance: Option<f32>,
    // sec2 0x67 ReverseDisplacementSetup: the block's presence arms the spawn-at-endpoint
    // behavior; its single float payload is never read by the effect
    // (research/xim ParticleInitializers.kt ReverseDisplacementSetup — the read float is
    // stored but unused in apply; the retail decompile's ElemGenerate has no 0x67 case,
    // so xim's mapping is the available evidence).
    pub reverse_displacement: Option<f32>,
    // sec3 0x02 PositionUpdater: a no-payload marker — retail's ElemIdle case 0x02 adds the
    // element's total velocity × dt to its position, and only while the block is present
    // (research/xim ParticleUpdaters.kt PositionUpdater; CYyGenerator.cpp
    // CYyGenerator::ElemIdle case 0x02). A generator that carries a base velocity without
    // the block is not position-stepped by retail, so the flag gates the engine's velocity
    // integration.
    pub position_updater: bool,
    // sec2 0x0A RotationVarianceInitializer: the per-axis bound of the uniform random rotation
    // added to the 0x09 base per particle (research/xim ParticleInitializers.kt
    // RotationVarianceInitializer — the retail decompile's ElemGenerate default is
    // XICLIENT_CODE_MISSING, so xim's mapping is the available evidence).
    pub rotation_variance: Option<[f32; 3]>,
    pub init_rotation: [f32; 3],
    // sec2 0x3B IncrementalRotationApplier: the per-axis increment added to the 0x09 base
    // rotation, scaled by one plus the particles emitted before this one (research/xim
    // ParticleInitializers.kt IncrementalRotationApplier — rotation += incr × (1 +
    // totalParticlesEmitted); its apply also arms the render-time rotation-y negation, even
    // for an all-zero payload. The retail decompile's ElemGenerate has no 0x3B case, so xim
    // is the available evidence.
    pub incremental_rotation: Option<[f32; 3]>,
    pub blend: ParticleBlend,
    // The raw BlendFuncInitializer p0 (retail `field_16C & 0xFF`), kept alongside the collapsed
    // `blend` because the TEXTUREFACTOR-alpha promotion is keyed on byte 0x44 exactly.
    // research/XIClient/src/XIClient/source/Resource/Derived/CMoD3m.cpp CMoD3m::Draw
    pub blend_byte: u8,

    // Selects the D3m texture-stage table: set = NonZeroOneTSS (texture alpha ignored,
    // alpha = 4*D.a*F.a), clear = NonZeroTwoTSS (alpha = 8*D.a*T.a*F.a).
    // research/XIClient/src/XIClient/source/Resource/Derived/CMoD3m.cpp ZeroOneTSS
    pub ignore_texture_alpha: bool,

    // Clear = the element fogs toward the area's fog colour like terrain (CMoElem.cpp
    // CMoElem::PrepDX); the weat/ sky layers past the fog range set the bit.
    pub fog_enabled: bool,

    pub draw_priority: DrawPriority,
    // CYyGenerator.cpp CYyGenerator::ElemGenerate opcode 0x30 — the element's `field_128`
    // sort-key offset (research/xim Particle.kt `projectionBias`).
    pub sort_offset: f32,
    // sec2 0x72 ProjectionBiasInitializer: two floats. param0 is the same `field_128` 0x30
    // writes (the ordering-table key via CMoElem.cpp CMoElem::CheckSomethingWasTrue ->
    // OT->Insert), so it lands in `sort_offset`; param1 is the attached SkeletalMeshActor
    // depth-scale factor (CYyGenerator.cpp CYyGenerator::ElemGenerate case 0x72 —
    // field_128 *= (GetDepthScale() - 1) * (param1 != 0 ? param1 : 1) + 1), which the engine
    // does not reproduce (no actor depth scale) and xim ignores (research/xim
    // ParticleInitializers.kt ProjectionBiasInitializer — only param0 reaches the draw bias).
    pub projection_bias: Option<[f32; 2]>,
    // CMoElem.cpp CMoElem::PrepDX — D3DRS_ZWRITEENABLE for the element.
    pub depth_write: bool,

    // Per-particle keyframe tracks referenced by DAT-id (resolved against the action's 0x19 chunks).
    pub scale_x_track: Option<[u8; 4]>,
    pub scale_y_track: Option<[u8; 4]>,
    // sec2 0x29 KeyFrameValueSetup (scale.z): retail captures field_EC.z, the element's
    // scale z, as the track's initial value (CYyGenerator.cpp CYyGenerator::ElemGenerate
    // case 0x29 — same shape as 0x27/0x28). Parsed but not applied: the engine's 2D sprite
    // has no z axis (as for the 0x10/0x11 z bound).
    pub scale_z_track: Option<[u8; 4]>,
    pub alpha_track: Option<[u8; 4]>,
    // sec2 0x2A KeyFrameValueSetup (color.r): a keyframe track on the element's red channel
    // (research/xim ParticleGeneratorParser.kt — 0x2A/0x2B/0x2C are the Color.r/g/b
    // KeyFrameValueSetup; retail's keyframe pre-load pass references the same blocks as
    // Keyframe resources). Parsed but not applied: the engine sets the particle's rgb at
    // spawn from the 0x16 base / 0x17 variance and has no per-frame rgb track path.
    pub color_r_track: Option<[u8; 4]>,
    // sec2 0x2B KeyFrameValueSetup (color.g): the green-channel twin of 0x2A
    // (research/xim ParticleGeneratorParser.kt). Parsed but not applied, as for 0x2A.
    pub color_g_track: Option<[u8; 4]>,
    // sec2 0x2C KeyFrameValueSetup (color.b): the blue-channel twin of 0x2A
    // (research/xim ParticleGeneratorParser.kt). Parsed but not applied, as for 0x2A.
    pub color_b_track: Option<[u8; 4]>,

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
    // payload+0 and target their own transform allocation. [0,0]/None = static.
    pub uv_scroll: [f32; 2],
    pub accel: Option<[f32; 3]>,
    pub rotation_accel: Option<[f32; 3]>,
    pub scale_accel: Option<[f32; 3]>,

    // Section 1 (body[0x70]) generator-level updater 0x0A, research/xim
    // ParticleGeneratorParser.kt sec1Handler GeneratorCullUpdater.
    pub emit_cull: Option<EmitCull>,

    // Section 1 generator-level updater 0x11, research/xim ParticleGeneratorParser.kt
    // sec1Handler AssociationUpdater.
    pub association: Option<AssociationFollow>,

    // sec2 0x8E FootMarkEffectSetup (research/xim ParticleInitializers.kt): a no-payload marker.
    // The particle snaps to the actor's position + joint and facing on the spawn frame, then
    // stops following the generator (research/xim Particle.kt updateAssociatedPosition /
    // updateAssociatedFacing footMarkEffect branches).
    pub foot_mark: bool,

    // sec2 0x3D OscillationSetup: a no-payload marker allocating the particle's oscillation
    // state (research/xim ParticleInitializers.kt OscillationSetup — NoDataParticleInitializer,
    // apply is particle.allocate(allocationOffset, OscillationParams())); the 0x3E/0x3F/0x40
    // acceleration setups write it and the section-3 0x29/0x2A/0x2B appliers integrate it.
    pub oscillation: bool,

    // sec2 0x45 ParentPositionCopyConfig: a no-payload marker — the particle's associated
    // position copies its parent's (research/xim ParticleInitializers.kt
    // ParentPositionCopyConfig; apply is a no-op without a parent). Parsed but not applied
    // until the child-generator path lands (the sec2 0x44 ChildGeneratorSetup).
    pub parent_position_copy: bool,

    // sec2 0x46 ParentVelocityConfig: one float, the multiplier on the parent's total
    // velocity copied into the child's velocity transform (research/xim
    // ParticleInitializers.kt ParentVelocityConfig; apply is a no-op without a parent).
    // Parsed but not applied until the child-generator path lands.
    pub parent_velocity: Option<f32>,

    // sec2 0x44 ChildGeneratorSetup: [expectZero32, child generator DAT id] — the sibling
    // generator chunk emitted as a child of each particle of this one (research/xim
    // ParticleInitializers.kt ChildGeneratorSetup; the sec2 0x53 block is the same shape).
    // Parsed but not applied until the child-generator runtime lands (the sec3 0x25/0x33
    // child updaters).
    pub child_generator: Option<[u8; 4]>,

    // sec2 0x40 OscillationAccelerationSetup (Z): [acceleration, accelerationVariance]; the
    // particle's Z oscillation acceleration is acceleration + variance × one [−1, 1) draw
    // (research/xim ParticleInitializers.kt OscillationAccelerationSetup — RandHelper rand()
    // in [−1, 1)). Parsed but not applied until the section-3 applier lands.
    pub oscillation_accel_z: Option<[f32; 2]>,
    // sec2 0x3E OscillationAccelerationSetup (X): the X-axis twin of 0x40
    // (research/xim ParticleInitializers.kt OscillationAccelerationSetup). Parsed but not
    // applied until the section-3 applier lands.
    pub oscillation_accel_x: Option<[f32; 2]>,
    // sec2 0x3F OscillationAccelerationSetup (Y): the Y-axis twin of 0x40 (research/xim
    // ParticleInitializers.kt OscillationAccelerationSetup). Present in 30 shipped generators
    // though absent from the launch log. Parsed but not applied until the section-3 applier
    // lands.
    pub oscillation_accel_y: Option<[f32; 2]>,

    // sec3 0x29 OscillationApplier (X): [rate-divisor, base-offset, unused-in-xim] — the
    // integrator for the 0x3E acceleration: oscillationRate = 180f / payload0, baseOffset =
    // payload1, payload2 has no effect (research/xim ParticleUpdaters.kt OscillationApplier).
    // The acceleration is parsed but never moves a particle without it.
    pub oscillation_applier_x: Option<[f32; 3]>,
    // sec3 0x2B OscillationApplier (Z): the Z-axis twin of 0x29 (research/xim
    // ParticleUpdaters.kt OscillationApplier), the integrator for the 0x40 acceleration.
    pub oscillation_applier_z: Option<[f32; 3]>,
    // sec3 0x2A OscillationApplier (Y): the Y-axis twin of 0x29 (research/xim
    // ParticleUpdaters.kt OscillationApplier), the integrator for the 0x3F acceleration.
    pub oscillation_applier_y: Option<[f32; 3]>,

    // sec2 0x0B RotationVelocitySetup: radians per 60 Hz frame, stored on the element
    // (CYyGenerator.cpp CYyGenerator::ElemGenerate case 0x0B). It only turns the particle when the
    // sec3 0x05 RotationUpdater integrates it (CYyGenerator.cpp CYyGenerator::ElemIdle case 0x05;
    // research/xim ParticleGeneratorParser.kt sec3Handler RotationUpdater), so read [`Self::spin`].
    pub rotation_velocity: Option<[f32; 3]>,

    // sec2 0x0C VelocityVarianceSetup (rotation): per-particle uniform [-v, v] draw added to each
    // axis of the 0x0B spin rate (research/xim ParticleInitializers.kt — the allocationOffset
    // binds it to the rotation transform; CYyGenerator.cpp CYyGenerator::ElemGenerate shares one
    // frand-add body across 0x03/0x0C/0x13).
    pub rotation_velocity_variance: Option<[f32; 3]>,
    pub rotation_updater: bool,

    // sec2 0x12 ScaleVelocitySetup: scale units per 60 Hz frame on each axis. Retail's
    // ElemGenerate shares the 0x0B/0x12 case (a 12-byte memcpy into the scale transform's
    // velocity at the allocation offset); only the sec3 0x08 ScaleUpdater integrates it
    // (research/xim ParticleUpdaters.kt — scale += velocity × elapsedFrames), so read
    // [`Self::scale_rate`].
    pub scale_velocity: Option<[f32; 3]>,
    pub scale_updater: bool,

    // sec2 0x13 VelocityVarianceSetup (scale): a per-particle uniform [-v, v] draw added to
    // each axis of the 0x12 scale velocity (research/xim ParticleInitializers.kt
    // VelocityVarianceSetup — the allocationOffset binds it to the scale transform; retail's
    // shared 0x03/0x0C/0x13 case adds frand(bounds) to the transform's velocity).
    pub scale_velocity_variance: Option<[f32; 3]>,

    // Section 4 (body[0x7C]) opcode 0x05, CYyGenerator.cpp CYyGenerator::ElemDie case 5 — an expiring
    // element gets its life reset instead of dying, keeping its position, rotation and UV state.
    // Every idle Home Point layer authors it; without it the crystal would snap back to its
    // spawn rotation every 120 frames.
    pub relife_on_expiry: bool,

    // CYyGenerator.cpp CYyGenerator::HandleOne 0x01000000 — the element renders through
    // CMoD3mSpecularElem, whose draw the XIClient decompile leaves as missing code; the sec2 0x55
    // record is kept alongside so the reconstruction has its inputs.
    pub specular_element: bool,
    pub specular: Option<SpecularParams>,
    // sec2 0x5A KeyFrameValueSetup (specular rotation.y): a keyframe track on the specular
    // element's rotation y (research/xim ParticleGeneratorParser.kt — 0x59/0x5A/0x5B are the
    // Specular Rotation x/y/z KeyFrameValueSetup; retail's keyframe pre-load pass references
    // the same blocks as Keyframe resources). Parsed but not applied: the engine does not
    // model the specular element's rotation (the 0x55 record is kept for reconstruction
    // inputs only).
    pub specular_rot_y_track: Option<[u8; 4]>,
    // sec2 0x82 CameraShakeSetup: [expectZero32, keyframe track id, unk0 u32, unk1 f32,
    // unk2 u32] — the keyframe DAT id the section-3 0x5F CameraShakeUpdater samples at the
    // particle's progress (research/xim ParticleInitializers.kt CameraShakeSetup). Parsed
    // but not applied until the section-3 updater lands.
    pub camera_shake_track: Option<[u8; 4]>,
    // sec3 0x5F CameraShakeUpdater: near, far, and — only in the 4-word form — shakeFactor
    // (research/xim ParticleUpdaters.kt CameraShakeUpdater — the opCodeSize == 4 branch). The
    // runtime application (sampling the sec2 0x82 track at the particle's progress with the
    // distance falloff × 1000×progress×distance×shakeFactor capped at 0.33, then
    // camera.applyShake) is unmodeled, so parse-only.
    pub camera_shake: Option<[f32; 3]>,

    // sec2 0x32 HazeOffsetInitializer: two floats, of which xim applies only the second,
    // as particle.hazeOffset.x — a draw-time x translate the haze/distortion shader pass
    // offsets the previous-frame transform by (research/xim ParticleInitializers.kt
    // HazeOffsetInitializer; GLDrawer.kt previousFrameTransform). Parsed but not applied:
    // the engine has no haze/distortion pass yet; the sec3 0x24 ProgressValueUpdater
    // animates the same value over life.
    pub haze_offset_x: Option<f32>,

    // sec2 0x47 ParentRotateConfig: a no-payload marker — the child particle copies its
    // parent's rotation (research/xim ParticleInitializers.kt ParentRotateConfig; apply is
    // a no-op without a parent). Parsed but not applied until the child-generator path
    // lands (the sec2 0x44 ChildGeneratorSetup).
    pub parent_rotate: bool,

    // sec2 0x48 ParentColorConfig: a no-payload marker — the child particle copies its
    // parent's color (research/xim ParticleInitializers.kt ParentColorConfig; apply is a
    // no-op without a parent). Parsed but not applied until the child-generator path
    // lands (the sec2 0x44 ChildGeneratorSetup).
    pub parent_color: bool,

    // sec2 0x49 ParentScaleConfig: a no-payload marker — the child particle copies its
    // parent's scale (research/xim ParticleInitializers.kt ParentScaleConfig; apply is a
    // no-op without a parent). Parsed but not applied until the child-generator path
    // lands (the sec2 0x44 ChildGeneratorSetup).
    pub parent_scale: bool,

    // sec2 0x69 KeyFrameValueSetup (velocity dampener): the 0x27/0x28/0x29 track shape
    // bound to the element's velocity dampener (research/xim ParticleGeneratorParser.kt
    // sec2Handler 0x69; retail's keyframe pre-load pass references the same blocks as
    // Keyframe resources). Parsed but not applied.
    pub velocity_dampener_track: Option<[u8; 4]>,
    pub velocity_dampener: Option<[f32; 2]>,
    // sec3 0x26 VelocityRotator: three floats, the rotateAmount added to the velocity
    // rotation × (0.5 × dt) per frame (research/xim ParticleUpdaters.kt VelocityRotator —
    // the actor-space axis hack and the 0.5 factor are unmodeled). The engine has no
    // velocityRotation, so parse-only.
    pub velocity_rotator: Option<[f32; 3]>,

    // sec2 0x4E FixedPointPositionVarianceSetup: [expectZero32, point list DAT id,
    // expect32(0, 1)] — the point list whose points cycle as per-emitted-particle
    // position offsets (research/xim ParticleInitializers.kt
    // FixedPointPositionVarianceSetup). Retail's sec2 walk handles neither 0x4E nor 0x4F
    // (research/XIClient CYyGenerator.cpp ElemGenerate), so the id is kept for
    // reconstruction only.
    pub fixed_point_position_variance: Option<[u8; 4]>,
    // sec2 0x4F: the twin of 0x4E — xim maps both opcodes to the same class
    // (research/xim ParticleGeneratorParser.kt sec2Handler); a second slot so a
    // generator carrying both keeps both ids.
    pub fixed_point_position_variance_2: Option<[u8; 4]>,

    // sec2 0x53 ChildGeneratorSetup: [expectZero32, child generator DAT id] — xim maps
    // both 0x44 and 0x53 to the same class (research/xim ParticleGeneratorParser.kt
    // sec2Handler); a second slot so a generator carrying both keeps both ids. Parsed
    // but not applied until the child-generator runtime lands (the sec3 0x25/0x33 child
    // updaters).
    pub child_generator_2: Option<[u8; 4]>,

    // sec2 0x5B KeyFrameValueSetup (specular rotation.z): the 0x27/0x28/0x29 track shape
    // bound to the specular element's rotation z (research/xim ParticleGeneratorParser.kt
    // — 0x59/0x5A/0x5B are the Specular Rotation x/y/z KeyFrameValueSetup). Parsed but
    // not applied: the engine does not model the specular element's rotation.
    pub specular_rot_z_track: Option<[u8; 4]>,

    // sec2 0x5F KeyFrameValueSetup (specular color.a): the 0x27/0x28/0x29 track shape
    // bound to the specular element's color alpha (research/xim
    // ParticleGeneratorParser.kt — 0x5C..0x5F are the Specular Color r/g/b/a
    // KeyFrameValueSetup). Parsed but not applied: the engine does not model the
    // specular element's color.
    pub specular_color_a_track: Option<[u8; 4]>,

    // sec2 0x79 ParentRotateConfig: a no-payload marker — xim maps 0x79 to the same
    // class as 0x47 (research/xim ParticleGeneratorParser.kt sec2Handler, comment
    // "How does it differ from 0x47?"); a second slot so a generator carrying both
    // keeps both. Parsed but not applied until the child-generator path lands (the
    // sec2 0x44 ChildGeneratorSetup).
    pub parent_rotate_2: bool,

    // sec2 0x56 BatchingSetup: one expectZero32 word — xim's apply sets the particle's
    // batched flag, which skips movement-orientation (research/xim ParticleInitializers.kt
    // BatchingSetup; Particle.kt applyMovementOrientation). Kept separate from `batched`:
    // retail's generator walk has no 0x56 case (research/XIClient CYyGenerator.cpp), so the
    // block does not arm the GEN_FLAG_BATCHED flag's CheckFlag29 behavior — parsed only.
    pub batching_setup: bool,

    // sec2 0x4A ParentTexCoordConfig: a no-payload marker — a child particle copies the
    // parent's tex-coord translate (research/xim ParticleInitializers.kt
    // ParentTexCoordConfig). A no-op without a parent, so parsed but not applied until the
    // child-generator path lands (as for the 0x45 marker).
    pub parent_tex_coord: bool,

    // sec2 0x54 PointListPositionSetup: [in-mem ptr, keyframe DAT id, expect zero, in-mem
    // ptr, point list DAT id] — the spline a particle follows, the keyframe id remapping
    // its progress and zero when the raw progress drives it (research/xim
    // ParticleInitializers.kt PointListPositionSetup; retail's ElemGenerate case 0x54
    // offsets the first emitted elem by the spline's start point, a shared allocation slot
    // zeroing the delta for later elems). Parsed but not applied until the sec3 0x34
    // PointListPositionUpdater lands.
    pub point_list_position: Option<([u8; 4], [u8; 4])>,

    // sec2 0x51 KeyFrameValueSetup (velocity.y): the 0x27/0x28/0x29 track shape bound to
    // the element's velocity y (research/xim ParticleGeneratorParser.kt sec2Handler —
    // 0x50/0x51/0x52 are the Velocity x/y/z KeyFrameValueSetup). Parsed but not applied:
    // the engine does not model a per-frame velocity track.
    pub velocity_y_track: Option<[u8; 4]>,

    // sec2 0x59 KeyFrameValueSetup (specular rot.x): the 0x27/0x28/0x29 track shape bound
    // to the specular element's rotation x (research/xim ParticleGeneratorParser.kt —
    // 0x59/0x5A/0x5B are the Specular Rotation x/y/z KeyFrameValueSetup). Parsed but not
    // applied: the engine does not model the specular element's rotation.
    pub specular_rot_x_track: Option<[u8; 4]>,

    // sec2 0x5D KeyFrameValueSetup (specular color.g): the 0x27/0x28/0x29 track shape
    // bound to the specular element's color green (research/xim
    // ParticleGeneratorParser.kt — 0x5C..0x5F are the Specular Color r/g/b/a
    // KeyFrameValueSetup). Parsed but not applied: the engine does not model the
    // specular element's color.
    pub specular_color_g_track: Option<[u8; 4]>,
}

// sec2 0x55 SpecularParams (research/xim ParticleInitializers.kt SpecularParamsInitializer): a
// non-unit vector, the DatId of a 0x20 the element does not otherwise draw with (the Home Point
// crystal names `nami`), a zeroed in-memory pointer, two floats xim found no visible effect for
// (10.0 and 30.0 corpus-wide), a BGRA colour and a flags word. Only the texture link is understood.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpecularParams {
    pub vector: [f32; 3],
    pub texture: Option<[u8; 4]>,
    pub unknown_a: f32,
    pub unknown_b: f32,
    pub color_bgra: [u8; 4],
    pub flags: u32,
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

// research/xim ParticleGeneratorUpdaters.kt AssociationUpdater - the section-1 0x11
// config word: bit 0 re-snaps the generator's associated position to the attach actor
// every frame, bit 1 the associated facing. The high word is a follow-rate factor that
// retail parses but its own handler ignores - ParticleGeneratorAttachment.kt
// updateAssociatedPosition is a hard copy ("it's not supposed to be an instant update,
// but most effects are so fast that it doesn't really matter").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AssociationFollow {
    pub follow_position: bool,
    pub follow_facing: bool,
    pub factor: u32,
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
const SEC1_OPCODE_ASSOCIATION: u8 = 0x11;
const SEC2_OPCODE_VELOCITY_VARIANCE: u8 = 0x03;
const SEC2_OPCODE_ROTATION_VARIANCE: u8 = 0x0A;
const SEC2_OPCODE_ROTATION_VEL_VARIANCE: u8 = 0x0C;
const SEC2_OPCODE_SCALE_VARIANCE: u8 = 0x10;
const SEC2_OPCODE_SINGLE_SCALE_VARIANCE: u8 = 0x11;
const SEC2_OPCODE_SCALE_VELOCITY: u8 = 0x12;
const SEC2_OPCODE_SCALE_VEL_VARIANCE: u8 = 0x13;
const SEC2_OPCODE_COLOR_VARIANCE: u8 = 0x17;
const SEC2_OPCODE_COLOR_TRANSFORM_SETUP: u8 = 0x19;
const SEC2_OPCODE_SPRITE_SHEET_INIT: u8 = 0x1D;
const SEC2_OPCODE_SPHERICAL_VARIANCE_FULL: u8 = 0x1F;
const SEC2_OPCODE_SCALE_Z_TRACK: u8 = 0x29;
const SEC2_OPCODE_COLOR_R_TRACK: u8 = 0x2A;
const SEC2_OPCODE_COLOR_G_TRACK: u8 = 0x2B;
const SEC2_OPCODE_COLOR_B_TRACK: u8 = 0x2C;
const SEC2_OPCODE_HAZE_OFFSET: u8 = 0x32;
const SEC2_OPCODE_INCREMENTAL_ROTATION: u8 = 0x3B;
const SEC2_OPCODE_OSCILLATION_SETUP: u8 = 0x3D;
const SEC2_OPCODE_OSCILLATION_ACCEL_X: u8 = 0x3E;
const SEC2_OPCODE_OSCILLATION_ACCEL_Y: u8 = 0x3F;
const SEC2_OPCODE_OSCILLATION_ACCEL_Z: u8 = 0x40;
const SEC2_OPCODE_RELATIVE_VEL_VARIANCE: u8 = 0x41;
const SEC2_OPCODE_CHILD_GENERATOR: u8 = 0x44;
const SEC2_OPCODE_PARENT_POSITION_COPY: u8 = 0x45;
const SEC2_OPCODE_PARENT_VELOCITY: u8 = 0x46;
const SEC2_OPCODE_PARENT_ROTATE: u8 = 0x47;
const SEC2_OPCODE_PARENT_COLOR: u8 = 0x48;
const SEC2_OPCODE_PARENT_SCALE: u8 = 0x49;
const SEC2_OPCODE_PARENT_TEX_COORD: u8 = 0x4A;
const SEC2_OPCODE_FIXED_POINT_POSITION_VARIANCE: u8 = 0x4E;
const SEC2_OPCODE_FIXED_POINT_POSITION_VARIANCE_2: u8 = 0x4F;
const SEC2_OPCODE_VELOCITY_Y_TRACK: u8 = 0x51;
const SEC2_OPCODE_CHILD_GENERATOR_2: u8 = 0x53;
const SEC2_OPCODE_POINT_LIST_POSITION: u8 = 0x54;
const SEC2_OPCODE_BATCHING_SETUP: u8 = 0x56;
const SEC2_OPCODE_SPECULAR_ROT_X_TRACK: u8 = 0x59;
const SEC2_OPCODE_SPECULAR_ROT_Y_TRACK: u8 = 0x5A;
const SEC2_OPCODE_SPECULAR_ROT_Z_TRACK: u8 = 0x5B;
const SEC2_OPCODE_SPECULAR_COLOR_G_TRACK: u8 = 0x5D;
const SEC2_OPCODE_SPECULAR_COLOR_A_TRACK: u8 = 0x5F;
const SEC2_OPCODE_REVERSE_DISPLACEMENT: u8 = 0x67;
const SEC2_OPCODE_VELOCITY_DAMPENER_TRACK: u8 = 0x69;
const SEC2_OPCODE_PROJECTION_BIAS: u8 = 0x72;
const SEC2_OPCODE_PARENT_ROTATE_2: u8 = 0x79;
const SEC2_OPCODE_CAMERA_SHAKE_SETUP: u8 = 0x82;
const SEC2_OPCODE_FOOT_MARK: u8 = 0x8E;
const SEC3_OPCODE_POSITION: u8 = 0x02;
const SEC3_OPCODE_ROTATION_UPDATER: u8 = 0x05;
const SEC3_OPCODE_SCALE_UPDATER: u8 = 0x08;
const SEC3_OPCODE_COLOR_TRANSFORM_APPLIER: u8 = 0x0B;
const SEC3_OPCODE_COLOR_TRANSFORM_MODIFIER: u8 = 0x0C;
const SEC3_OPCODE_SPRITE_SHEET_FRAME: u8 = 0x0D;
const SEC3_OPCODE_NO_OP: u8 = 0x0E;
const SEC3_OPCODE_VELOCITY_ACCELERATOR_UNGATED_FIRST: u8 = 0x06;
const SEC3_OPCODE_VELOCITY_ACCELERATOR_UNGATED_LAST: u8 = 0x09;
const SEC3_OPCODE_SCALE_PROGRESS_FIRST: u8 = 0x15;
const SEC3_OPCODE_SCALE_PROGRESS_LAST: u8 = 0x17;
const SEC3_OPCODE_COLOR_RGB_PROGRESS_FIRST: u8 = 0x18;
const SEC3_OPCODE_COLOR_RGB_PROGRESS_LAST: u8 = 0x1A;
const SEC3_OPCODE_ALPHA_UPDATER: u8 = 0x1B;
const SEC3_OPCODE_CHILD_GENERATOR_BASIC: u8 = 0x25;
const SEC3_OPCODE_VELOCITY_ROTATOR: u8 = 0x26;
const SEC3_OPCODE_OSCILLATION_APPLIER_X: u8 = 0x29;
const SEC3_OPCODE_OSCILLATION_APPLIER_Y: u8 = 0x2A;
const SEC3_OPCODE_OSCILLATION_APPLIER_Z: u8 = 0x2B;
const SEC3_OPCODE_VELOCITY_DAMPENER: u8 = 0x2C;
const SEC3_OPCODE_VELOCITY_ROTATION_UPDATER: u8 = 0x2F;
const SEC3_OPCODE_CHILD_GENERATOR: u8 = 0x33;
const SEC3_OPCODE_POINT_LIST_POSITION: u8 = 0x34;
const SEC3_OPCODE_SPECULAR_ROT_Y_PROGRESS: u8 = 0x36;
const SEC3_OPCODE_SPECULAR_ROT_Z_PROGRESS: u8 = 0x37;
const SEC3_OPCODE_SPECULAR_COLOR_A_PROGRESS: u8 = 0x3B;
const SEC3_OPCODE_DAMPENING_FACTOR: u8 = 0x44;
const SEC3_OPCODE_CAMERA_SHAKE_UPDATER: u8 = 0x5F;
const SEC4_OFFSET: usize = 0x7C;
const SEC4_OPCODE_RELIFE: u8 = 0x05;
const SEC4_OPCODE_EMIT_CHILD: u8 = 0x01;

impl ParticleGeneratorDef {
    pub fn parse(body: &[u8]) -> Result<Option<Self>> {
        Self::parse_reporting(body, &mut |_, _, _| {})
    }

    pub fn parse_reporting(body: &[u8], sink: GeneratorOpcodeSink<'_>) -> Result<Option<Self>> {
        let mut blocks: Vec<(GeneratorSection, u8, bool)> = Vec::new();
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
        let mut spherical_full = None;
        let mut is_particle = false;
        let mut init_scale = [1.0f32; 3];
        let mut single_scale_variance = None;
        let mut scale_variance = None;
        let mut init_color = [1.0f32; 4];
        let mut color_variance = None;
        let mut color_transform = None;
        let mut init_velocity = [0.0f32; 3];
        let mut position_allocation = None;
        let mut rotation_allocation = None;
        let mut scale_allocation = None;
        let mut velocity_variance = None;
        let mut relative_velocity = None;
        let mut relative_velocity_variance = None;
        let mut reverse_displacement = None;
        let mut rotation_variance = None;
        let mut init_rotation = [0.0f32; 3];
        let mut incremental_rotation = None;
        let mut scale_x_track = None;
        let mut scale_y_track = None;
        let mut scale_z_track = None;
        let mut alpha_track = None;
        let mut color_r_track = None;
        let mut color_g_track = None;
        let mut color_b_track = None;
        let mut blend = ParticleBlend::Additive;
        let mut blend_byte = 0u8;
        let mut ignore_texture_alpha = false;
        let mut fog_enabled = true;
        let mut draw_priority = DrawPriority::Depth;
        let mut sort_offset = 0.0;
        let mut projection_bias = None;
        let mut depth_write = false;
        let mut tod_color_tracks: [Option<[u8; 4]>; TOD_COLOR_CHANNELS] =
            [None; TOD_COLOR_CHANNELS];
        let mut rotation_velocity = None;
        let mut rotation_velocity_variance = None;
        let mut scale_velocity = None;
        let mut scale_updater = false;
        let mut scale_velocity_variance = None;
        let mut specular = None;
        let mut specular_element = false;
        let mut specular_rot_y_track = None;
        let mut specular_rot_z_track = None;
        let mut specular_color_a_track = None;
        let mut camera_shake_track = None;
        let mut haze_offset_x = None;
        let mut parent_rotate = false;
        let mut parent_rotate_2 = false;
        let mut batching_setup = false;
        let mut parent_tex_coord = false;
        let mut point_list_position = None;
        let mut velocity_y_track = None;
        let mut specular_rot_x_track = None;
        let mut specular_color_g_track = None;
        let mut parent_color = false;
        let mut parent_scale = false;
        let mut velocity_dampener_track = None;
        let mut fixed_point_position_variance = None;
        let mut fixed_point_position_variance_2 = None;
        let mut foot_mark = false;
        let mut oscillation = false;
        let mut parent_position_copy = false;
        let mut parent_velocity = None;
        let mut child_generator = None;
        let mut child_generator_2 = None;
        let mut oscillation_accel_z = None;
        let mut oscillation_accel_x = None;
        let mut oscillation_accel_y = None;

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
            let mut decoded = true;
            match opcode {
                0x01 if payload + 32 <= body.len() => {
                    let bb = u16_le(body, payload);
                    billboard = ParticleBillboard::from_flags(bb);
                    camera_billboard = bb & BILLBOARD_XYZ != 0
                        || bb & BILLBOARD_CAMERA_MASK == BILLBOARD_CAMERA_MASK;
                    let render_state = u16_le(body, payload + 2);
                    ignore_texture_alpha = render_state & RENDER_STATE_IGNORE_TEXTURE_ALPHA != 0;
                    fog_enabled = render_state & RENDER_STATE_FOG_DISABLED == 0;
                    draw_priority = if render_state & RENDER_STATE_LOW_PRIORITY_DRAW != 0 {
                        DrawPriority::Low
                    } else if render_state & RENDER_STATE_PINNED_DRAW != 0 {
                        DrawPriority::Pinned
                    } else {
                        DrawPriority::Depth
                    };
                    depth_write = bb & BILLBOARD_DEPTH_WRITE != 0;
                    follow_camera = bb & BILLBOARD_FOLLOW_CAMERA != 0;
                    camera_attached_base = render_state & RENDER_STATE_CAMERA_ATTACHED_BASE != 0;
                    specular_element = render_state & RENDER_STATE_SPECULAR_ELEMENT != 0;
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
                    position_allocation = Some((cfg >> ALLOCATION_SHIFT) & ALLOCATION_MASK);
                    init_velocity = [
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ];
                }
                SEC2_OPCODE_VELOCITY_VARIANCE if payload + 12 <= body.len() => {
                    velocity_variance = Some([
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ]);
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
                0x08 if payload + 4 <= body.len() => {
                    relative_velocity = Some(f32_le(body, payload));
                }
                // CYyGenerator.cpp CYyGenerator::ElemGenerate case 0x1F: a raw 0 azimuth
                // step means random azimuth.
                SEC2_OPCODE_SPHERICAL_VARIANCE_FULL if payload + 42 <= body.len() => {
                    spherical_full = Some(SphericalPositionVarianceFull {
                        radius_variance: f32_le(body, payload),
                        base_radius: f32_le(body, payload + 4),
                        axis_scale: [
                            f32_le(body, payload + 8),
                            f32_le(body, payload + 12),
                            f32_le(body, payload + 16),
                        ],
                        rotation_z: f32_le(body, payload + 20),
                        rotation_y: f32_le(body, payload + 24),
                        tilt: f32_le(body, payload + 28),
                        tilt_variance: f32_le(body, payload + 32),
                        camera_oriented: u32_le(body, payload + 36) & 1 != 0,
                        azimuth_steps: {
                            let raw = u16_le(body, payload + 40);
                            if raw == 0 {
                                0
                            } else {
                                raw as u32 + 1
                            }
                        },
                    });
                }
                0x09 if payload + 12 <= body.len() => {
                    init_rotation = [
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ];
                }
                SEC2_OPCODE_ROTATION_VARIANCE if payload + 12 <= body.len() => {
                    rotation_variance = Some([
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ]);
                }
                0x0B if payload + 12 <= body.len() => {
                    rotation_allocation = Some((cfg >> ALLOCATION_SHIFT) & ALLOCATION_MASK);
                    rotation_velocity = Some([
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ]);
                }
                SEC2_OPCODE_ROTATION_VEL_VARIANCE if payload + 12 <= body.len() => {
                    rotation_velocity_variance = Some([
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ]);
                }
                // research/xim ParticleInitializers.kt VelocityVarianceSetup: the
                // allocationOffset binds the variance to the scale velocity.
                SEC2_OPCODE_SCALE_VEL_VARIANCE if payload + 12 <= body.len() => {
                    scale_velocity_variance = Some([
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ]);
                }
                // research/xim ParticleInitializers.kt IncrementalRotationApplier.
                SEC2_OPCODE_INCREMENTAL_ROTATION if payload + 12 <= body.len() => {
                    incremental_rotation = Some([
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ]);
                }
                0x0F if payload + 12 <= body.len() => {
                    init_scale = [
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ];
                }
                // CYyGenerator.cpp CYyGenerator::ElemGenerate case 0x10.
                SEC2_OPCODE_SCALE_VARIANCE if payload + 12 <= body.len() => {
                    scale_variance = Some([
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ]);
                }
                SEC2_OPCODE_SINGLE_SCALE_VARIANCE if payload + 4 <= body.len() => {
                    single_scale_variance = Some(f32_le(body, payload));
                }
                SEC2_OPCODE_SCALE_VELOCITY if payload + 12 <= body.len() => {
                    scale_allocation = Some((cfg >> ALLOCATION_SHIFT) & ALLOCATION_MASK);
                    scale_velocity = Some([
                        f32_le(body, payload),
                        f32_le(body, payload + 4),
                        f32_le(body, payload + 8),
                    ]);
                }
                0x55 if payload + 36 <= body.len() => {
                    specular = Some(SpecularParams {
                        vector: [
                            f32_le(body, payload),
                            f32_le(body, payload + 4),
                            f32_le(body, payload + 8),
                        ],
                        texture: track_id(body, payload + 12),
                        unknown_a: f32_le(body, payload + 20),
                        unknown_b: f32_le(body, payload + 24),
                        color_bgra: [
                            body[payload + 28],
                            body[payload + 29],
                            body[payload + 30],
                            body[payload + 31],
                        ],
                        flags: u32_le(body, payload + 32),
                    });
                }
                // research/xim ParticleGeneratorParser.kt sec2Handler: the keyframe
                // track bound to the specular element's rotation y.
                SEC2_OPCODE_SPECULAR_ROT_Y_TRACK if payload + 8 <= body.len() => {
                    specular_rot_y_track = track_id(body, payload + 4);
                }
                // research/xim ParticleGeneratorParser.kt sec2Handler: the keyframe
                // track bound to the specular element's rotation z.
                SEC2_OPCODE_SPECULAR_ROT_Z_TRACK if payload + 8 <= body.len() => {
                    specular_rot_z_track = track_id(body, payload + 4);
                }
                // research/xim ParticleGeneratorParser.kt sec2Handler: the keyframe
                // track bound to the specular element's color alpha.
                SEC2_OPCODE_SPECULAR_COLOR_A_TRACK if payload + 8 <= body.len() => {
                    specular_color_a_track = track_id(body, payload + 4);
                }
                // research/xim ParticleInitializers.kt CameraShakeSetup: the DAT id of
                // the keyframe track SEC3_OPCODE_CAMERA_SHAKE_UPDATER samples at the
                // particle's progress; the block's other words are consumed, not kept.
                SEC2_OPCODE_CAMERA_SHAKE_SETUP if payload + 8 <= body.len() => {
                    camera_shake_track = track_id(body, payload + 4);
                }
                // research/xim ParticleInitializers.kt HazeOffsetInitializer: xim applies
                // only the second float, as particle.hazeOffset.x.
                SEC2_OPCODE_HAZE_OFFSET if payload + 8 <= body.len() => {
                    haze_offset_x = Some(f32_le(body, payload + 4));
                }
                0x30 if payload + 4 <= body.len() => sort_offset = f32_le(body, payload),
                // research/xim ParticleInitializers.kt RelativeVelocityVarianceSetup: the
                // bound of the random magnitude added to the relative velocity.
                SEC2_OPCODE_RELATIVE_VEL_VARIANCE if payload + 4 <= body.len() => {
                    relative_velocity_variance = Some(f32_le(body, payload));
                }
                // research/xim ParticleInitializers.kt ProjectionBiasInitializer: param0
                // sets the same field as the sort offset (last write in stream order wins,
                // as in retail's sequential walk; the shipped data never carries both),
                // param1 is the actor depth-scale factor.
                SEC2_OPCODE_PROJECTION_BIAS if payload + 8 <= body.len() => {
                    let p0 = f32_le(body, payload);
                    let p1 = f32_le(body, payload + 4);
                    sort_offset = p0;
                    projection_bias = Some([p0, p1]);
                }
                0x16 if payload + 4 <= body.len() => {
                    init_color = [
                        body[payload] as f32 / 255.0,
                        body[payload + 1] as f32 / 255.0,
                        body[payload + 2] as f32 / 255.0,
                        body[payload + 3] as f32 / 255.0,
                    ];
                }
                // research/xim ParticleInitializers.kt ColorVarianceSetup.
                SEC2_OPCODE_COLOR_VARIANCE if payload + 4 <= body.len() => {
                    color_variance = Some([
                        body[payload] as f32 / 255.0,
                        body[payload + 1] as f32 / 255.0,
                        body[payload + 2] as f32 / 255.0,
                        body[payload + 3] as f32 / 255.0,
                    ]);
                }
                // Parsed only: CYyGenerator.cpp CYyGenerator::ElemGenerate has no 0x19
                // case, so the transform's application is unknown.
                SEC2_OPCODE_COLOR_TRANSFORM_SETUP if payload + 8 <= body.len() => {
                    color_transform = Some([
                        i16::from_le_bytes([body[payload], body[payload + 1]]),
                        i16::from_le_bytes([body[payload + 2], body[payload + 3]]),
                        i16::from_le_bytes([body[payload + 4], body[payload + 5]]),
                        i16::from_le_bytes([body[payload + 6], body[payload + 7]]),
                    ]);
                }
                // KeyFrameValueSetup: opcode selects the target channel; the track id is at payload+4.
                0x27 if payload + 8 <= body.len() => scale_x_track = track_id(body, payload + 4),
                0x28 if payload + 8 <= body.len() => scale_y_track = track_id(body, payload + 4),
                SEC2_OPCODE_SCALE_Z_TRACK if payload + 8 <= body.len() => {
                    scale_z_track = track_id(body, payload + 4)
                }
                0x2D if payload + 8 <= body.len() => alpha_track = track_id(body, payload + 4),
                // research/xim ParticleGeneratorParser.kt sec2Handler: the keyframe track
                // bound to the element's red channel.
                SEC2_OPCODE_COLOR_R_TRACK if payload + 8 <= body.len() => {
                    color_r_track = track_id(body, payload + 4)
                }
                // research/xim ParticleGeneratorParser.kt sec2Handler: the keyframe track
                // bound to the element's green channel.
                SEC2_OPCODE_COLOR_G_TRACK if payload + 8 <= body.len() => {
                    color_g_track = track_id(body, payload + 4)
                }
                // research/xim ParticleGeneratorParser.kt sec2Handler: the keyframe track
                // bound to the element's blue channel.
                SEC2_OPCODE_COLOR_B_TRACK if payload + 8 <= body.len() => {
                    color_b_track = track_id(body, payload + 4)
                }
                // research/xim ParticleGeneratorParser.kt sec2Handler — 0x60..0x63 are the same
                // KeyFrameValueSetup shape bound to the time-of-day color channels, read back by
                // the section-3 ClockValueUpdater 0x3C..0x3F.
                0x60..=0x63 if payload + 8 <= body.len() => {
                    tod_color_tracks[(opcode - 0x60) as usize] = track_id(body, payload + 4);
                }
                // research/xim ParticleInitializers.kt ReverseDisplacementSetup: parsed,
                // never read by the effect.
                SEC2_OPCODE_REVERSE_DISPLACEMENT if payload + 4 <= body.len() => {
                    reverse_displacement = Some(f32_le(body, payload));
                }
                // research/XIClient CYyGenerator.cpp CYyGenerator::ElemGenerate case 0x1D
                // retail derives the flipbook's per-frame interval from the CMoD3a resource's
                // frame count, not the DAT, so the payload word is never read.
                SEC2_OPCODE_SPRITE_SHEET_INIT if payload + 4 <= body.len() => {}
                // research/xim Particle.kt updateAssociatedPosition / updateAssociatedFacing
                // footMarkEffect branches: the particle snaps to the actor's position + joint
                // and facing on the spawn frame, then stops following the generator.
                SEC2_OPCODE_FOOT_MARK => foot_mark = true,
                // research/xim ParticleInitializers.kt OscillationSetup: the marker that
                // allocates the particle's oscillation state.
                SEC2_OPCODE_OSCILLATION_SETUP => oscillation = true,
                // research/xim ParticleInitializers.kt ParentPositionCopyConfig: the marker
                // that makes a child particle copy its parent's position.
                SEC2_OPCODE_PARENT_POSITION_COPY => parent_position_copy = true,
                // research/xim ParticleInitializers.kt ParentVelocityConfig: the multiplier
                // on the parent's total velocity copied into the child's velocity.
                SEC2_OPCODE_PARENT_VELOCITY if payload + 4 <= body.len() => {
                    parent_velocity = Some(f32_le(body, payload));
                }
                // research/xim ParticleInitializers.kt ChildGeneratorSetup: the sibling
                // generator emitted as a child of each particle.
                SEC2_OPCODE_CHILD_GENERATOR if payload + 8 <= body.len() => {
                    child_generator = track_id(body, payload + 4);
                }
                // research/xim ParticleGeneratorParser.kt sec2Handler: xim maps this opcode
                // to the same ChildGeneratorSetup as SEC2_OPCODE_CHILD_GENERATOR.
                SEC2_OPCODE_CHILD_GENERATOR_2 if payload + 8 <= body.len() => {
                    child_generator_2 = track_id(body, payload + 4);
                }
                // research/xim ParticleInitializers.kt ParentRotateConfig: the marker that
                // makes a child particle copy its parent's rotation.
                SEC2_OPCODE_PARENT_ROTATE => parent_rotate = true,
                // research/xim ParticleGeneratorParser.kt sec2Handler: xim maps this opcode
                // to the same ParentRotateConfig as SEC2_OPCODE_PARENT_ROTATE.
                SEC2_OPCODE_PARENT_ROTATE_2 => parent_rotate_2 = true,
                // research/xim ParticleInitializers.kt BatchingSetup: the word is an
                // expectZero32; only the marker is kept.
                SEC2_OPCODE_BATCHING_SETUP if payload + 4 <= body.len() => batching_setup = true,
                // research/xim ParticleInitializers.kt ParentColorConfig: the marker that
                // makes a child particle copy its parent's color.
                SEC2_OPCODE_PARENT_COLOR => parent_color = true,
                // research/xim ParticleInitializers.kt ParentScaleConfig: the marker that
                // makes a child particle copy its parent's scale.
                SEC2_OPCODE_PARENT_SCALE => parent_scale = true,
                // research/xim ParticleInitializers.kt ParentTexCoordConfig: the marker that
                // makes a child particle copy its parent's tex-coord translate.
                SEC2_OPCODE_PARENT_TEX_COORD => parent_tex_coord = true,
                // research/xim ParticleInitializers.kt PointListPositionSetup: the keyframe
                // and point-list DAT ids; the in-mem pointer words are consumed, not kept.
                SEC2_OPCODE_POINT_LIST_POSITION if payload + 20 <= body.len() => {
                    point_list_position = Some((
                        DatId::from(body, payload + 4).0,
                        DatId::from(body, payload + 16).0,
                    ));
                }
                // research/xim ParticleGeneratorParser.kt sec2Handler: the keyframe track
                // bound to the element's velocity y.
                SEC2_OPCODE_VELOCITY_Y_TRACK if payload + 8 <= body.len() => {
                    velocity_y_track = track_id(body, payload + 4);
                }
                // research/xim ParticleGeneratorParser.kt sec2Handler: the keyframe track
                // bound to the specular element's rotation x.
                SEC2_OPCODE_SPECULAR_ROT_X_TRACK if payload + 8 <= body.len() => {
                    specular_rot_x_track = track_id(body, payload + 4);
                }
                // research/xim ParticleGeneratorParser.kt sec2Handler: the keyframe track
                // bound to the specular element's color green.
                SEC2_OPCODE_SPECULAR_COLOR_G_TRACK if payload + 8 <= body.len() => {
                    specular_color_g_track = track_id(body, payload + 4);
                }
                // research/xim ParticleGeneratorParser.kt sec2Handler: the keyframe track
                // bound to the element's velocity dampener.
                SEC2_OPCODE_VELOCITY_DAMPENER_TRACK if payload + 8 <= body.len() => {
                    velocity_dampener_track = track_id(body, payload + 4);
                }
                // research/xim ParticleInitializers.kt FixedPointPositionVarianceSetup: the
                // point list a per-emitted-particle position offset cycles through.
                SEC2_OPCODE_FIXED_POINT_POSITION_VARIANCE if payload + 12 <= body.len() => {
                    fixed_point_position_variance = track_id(body, payload + 4);
                }
                // research/xim ParticleGeneratorParser.kt sec2Handler: xim maps this opcode
                // to the same FixedPointPositionVarianceSetup as 0x4E.
                SEC2_OPCODE_FIXED_POINT_POSITION_VARIANCE_2 if payload + 12 <= body.len() => {
                    fixed_point_position_variance_2 = track_id(body, payload + 4);
                }
                // research/xim ParticleInitializers.kt OscillationAccelerationSetup (Z)
                // [acceleration, variance].
                SEC2_OPCODE_OSCILLATION_ACCEL_Z if payload + 8 <= body.len() => {
                    oscillation_accel_z = Some([f32_le(body, payload), f32_le(body, payload + 4)]);
                }
                // research/xim ParticleInitializers.kt OscillationAccelerationSetup (X).
                SEC2_OPCODE_OSCILLATION_ACCEL_X if payload + 8 <= body.len() => {
                    oscillation_accel_x = Some([f32_le(body, payload), f32_le(body, payload + 4)]);
                }
                // research/xim ParticleInitializers.kt OscillationAccelerationSetup (Y).
                SEC2_OPCODE_OSCILLATION_ACCEL_Y if payload + 8 <= body.len() => {
                    oscillation_accel_y = Some([f32_le(body, payload), f32_le(body, payload + 4)]);
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
                _ => decoded = false,
            }
            blocks.push((GeneratorSection::Initializers, opcode, decoded));
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
        let mut rotation_accel = None;
        let mut scale_accel = None;
        let mut oscillation_applier_x = None;
        let mut oscillation_applier_z = None;
        let mut oscillation_applier_y = None;
        let mut day_of_week_color = None;
        let mut moon_phase_color = None;
        let mut moon_phase_sprite = false;
        let mut rotation_updater = false;
        let mut position_updater = false;
        let mut camera_shake = None;
        let mut velocity_dampener = None;
        let mut velocity_rotator = None;
        let mut color_transform_modifier = None;
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
                let mut decoded = true;
                match opcode {
                    SEC3_OPCODE_POSITION => position_updater = true,
                    SEC3_OPCODE_ROTATION_UPDATER => rotation_updater = true,
                    SEC3_OPCODE_SCALE_UPDATER => scale_updater = true,
                    0x27 if payload + 4 <= body.len() => uv_scroll[0] = f32_le(body, payload),
                    0x28 if payload + 4 <= body.len() => uv_scroll[1] = f32_le(body, payload),
                    // research/xim ParticleUpdaters.kt OscillationApplier: oscillationRate =
                    // 180f / payload0, baseOffset = payload1, payload2 has no effect.
                    SEC3_OPCODE_OSCILLATION_APPLIER_X if payload + 12 <= body.len() => {
                        oscillation_applier_x = Some([
                            f32_le(body, payload),
                            f32_le(body, payload + 4),
                            f32_le(body, payload + 8),
                        ]);
                    }
                    SEC3_OPCODE_OSCILLATION_APPLIER_Z if payload + 12 <= body.len() => {
                        oscillation_applier_z = Some([
                            f32_le(body, payload),
                            f32_le(body, payload + 4),
                            f32_le(body, payload + 8),
                        ]);
                    }
                    SEC3_OPCODE_OSCILLATION_APPLIER_Y if payload + 12 <= body.len() => {
                        oscillation_applier_y = Some([
                            f32_le(body, payload),
                            f32_le(body, payload + 4),
                            f32_le(body, payload + 8),
                        ]);
                    }
                    0x03 if payload + 12 <= body.len() => {
                        accel = Some([
                            f32_le(body, payload),
                            f32_le(body, payload + 4),
                            f32_le(body, payload + 8),
                        ]);
                    }
                    // research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp
                    // CYyGenerator::ElemIdle uses Get11FC to address separate transform allocations.
                    SEC3_OPCODE_VELOCITY_ACCELERATOR_UNGATED_FIRST
                    | SEC3_OPCODE_VELOCITY_ACCELERATOR_UNGATED_LAST
                        if payload + 12 <= body.len() =>
                    {
                        let allocation = Some((cfg >> ALLOCATION_SHIFT) & ALLOCATION_MASK);
                        let target = if allocation == rotation_allocation {
                            Some(&mut rotation_accel)
                        } else if allocation == scale_allocation {
                            Some(&mut scale_accel)
                        } else if allocation == position_allocation {
                            Some(&mut accel)
                        } else {
                            None
                        };
                        if let Some(target) = target {
                            let value = target.get_or_insert([0.0; 3]);
                            for (axis, value) in value.iter_mut().enumerate() {
                                *value += f32_le(body, payload + axis * size_of::<f32>());
                            }
                        } else {
                            decoded = false;
                        }
                    }
                    // research/xim ParticleUpdaters.kt VelocityDampener: velocity is scaled
                    // by dampeningFactor^dt, the factor from
                    // SEC2_OPCODE_VELOCITY_DAMPENER_TRACK when present. The engine does not
                    // model the dampener, so parse-only.
                    SEC3_OPCODE_VELOCITY_DAMPENER if payload + 8 <= body.len() => {
                        velocity_dampener =
                            Some([f32_le(body, payload), f32_le(body, payload + 4)]);
                    }
                    // research/xim ParticleGeneratorParser.kt sec3Handler: the dampening-factor
                    // ProgressValueUpdater: no payload, it samples
                    // SEC2_OPCODE_VELOCITY_DAMPENER_TRACK. The engine does not model the
                    // dampener, so the block arms nothing and only consumes.
                    SEC3_OPCODE_DAMPENING_FACTOR => {}
                    // research/xim ParticleUpdaters.kt ColorTransformModifier: the per-frame
                    // rate on the SEC2_OPCODE_COLOR_TRANSFORM_SETUP transform. The engine does
                    // not model the transform's application, so parse-only.
                    SEC3_OPCODE_COLOR_TRANSFORM_MODIFIER if payload + 8 <= body.len() => {
                        color_transform_modifier = Some([
                            i16::from_le_bytes([body[payload], body[payload + 1]]),
                            i16::from_le_bytes([body[payload + 2], body[payload + 3]]),
                            i16::from_le_bytes([body[payload + 4], body[payload + 5]]),
                            i16::from_le_bytes([body[payload + 6], body[payload + 7]]),
                        ]);
                    }
                    // research/xim ParticleGeneratorParser.kt sec3Handler: the scale.x/y/z
                    // ProgressValueUpdaters: no payload, they sample the sec2 scale tracks at
                    // life progress, which the render path already does from
                    // def.scale_x_track/scale_y_track; the engine's 2D sprite has no z axis,
                    // so the blocks arm nothing and only consume.
                    SEC3_OPCODE_SCALE_PROGRESS_FIRST..=SEC3_OPCODE_SCALE_PROGRESS_LAST => {}
                    // research/xim ParticleGeneratorParser.kt sec3Handler: the color.r/g/b
                    // ProgressValueUpdaters: no payload, they sample the sec2 color tracks at
                    // life progress. The engine sets the particle's rgb at spawn from the 0x16
                    // base / 0x17 variance and has no per-frame rgb track path, so the blocks
                    // arm nothing and only consume.
                    SEC3_OPCODE_COLOR_RGB_PROGRESS_FIRST..=SEC3_OPCODE_COLOR_RGB_PROGRESS_LAST => {}
                    // research/xim ParticleGeneratorParser.kt sec3Handler: the specular
                    // rotation.y/z and color.a ProgressValueUpdaters: no payload, they sample
                    // the sec2 specular tracks. The engine does not model the specular
                    // element, so the blocks arm nothing and only consume.
                    SEC3_OPCODE_SPECULAR_ROT_Y_PROGRESS
                    | SEC3_OPCODE_SPECULAR_ROT_Z_PROGRESS
                    | SEC3_OPCODE_SPECULAR_COLOR_A_PROGRESS => {}
                    // research/xim ParticleUpdaters.kt ColorTransformApplier: no payload —
                    // color += (transform shr 7) * (0.5 * dt) per frame. The engine does
                    // not model the transform's application, so the block arms nothing and
                    // only consumes.
                    SEC3_OPCODE_COLOR_TRANSFORM_APPLIER => {}
                    // research/xim ParticleGeneratorParser.kt sec3Handler: ChildGeneratorBasicUpdater
                    // / ChildGeneratorUpdater: no payload, they emit/update the sec2 child
                    // generator per particle. The engine has no child-particle path, so the
                    // blocks arm nothing and only consume.
                    SEC3_OPCODE_CHILD_GENERATOR_BASIC | SEC3_OPCODE_CHILD_GENERATOR => {}
                    // research/xim ParticleUpdaters.kt VelocityRotationUpdater: no payload
                    // converts all velocity into the +x axis and copies the particle's
                    // rotation into the velocity rotation. The engine has no velocityRotation,
                    // so the block arms nothing and only consumes.
                    SEC3_OPCODE_VELOCITY_ROTATION_UPDATER => {}
                    // research/xim ParticleUpdaters.kt PointListPositionUpdater: no payload
                    // samples the SEC2_OPCODE_POINT_LIST_POSITION spline at the particle's
                    // progress and copies it to the position. The engine has no point-list
                    // spline runtime, so the block arms nothing and only consumes.
                    SEC3_OPCODE_POINT_LIST_POSITION => {}
                    // research/xim ParticleUpdaters.kt VelocityRotator: the rotateAmount
                    // added to the velocity rotation * (0.5 * dt) per frame. The engine has
                    // no velocityRotation, so parse-only.
                    SEC3_OPCODE_VELOCITY_ROTATOR if payload + 12 <= body.len() => {
                        velocity_rotator = Some([
                            f32_le(body, payload),
                            f32_le(body, payload + 4),
                            f32_le(body, payload + 8),
                        ]);
                    }
                    // research/xim ParticleGeneratorParser.kt sec3Handler ClockValueUpdater
                    // no payload; marks which 0x60..0x63 track drives its channel.
                    0x3C..=0x3F => tod_color_driven[(opcode - 0x3C) as usize] = true,
                    // research/xim ParticleGeneratorParser.kt sec3Handler MoonPhaseSpriteSheetUpdater.
                    0x45 => moon_phase_sprite = true,
                    // research/xim ParticleUpdaters.kt SpriteSheetFrameUpdater: no payload
                    // the flipbook frame advances across the particle's life, which the
                    // engine's flipbook_index already does for every SpriteSheet (retail's
                    // ElemIdle case 0x0D accumulator is the same sequence).
                    SEC3_OPCODE_SPRITE_SHEET_FRAME => {}
                    // research/xim ParticleUpdaters.kt NoOpParticleUpdater: no payload
                    // retail's ElemIdle case 0x0E computes the keyframe progress as
                    // 1.0 - (Life / field_114), the elapsed-life fraction the engine's
                    // `progress` (age/life, particle_sim.rs) already is, so the block arms
                    // nothing and only consumes.
                    SEC3_OPCODE_NO_OP => {}
                    // research/xim ParticleGeneratorParser.kt sec3Handler: the color.a
                    // ProgressValueUpdater: no payload. It samples the sec2 alpha track at
                    // life progress, which particle_draw already does from def.alpha_track, so
                    // the block arms nothing and only consumes.
                    SEC3_OPCODE_ALPHA_UPDATER => {}
                    // research/xim ParticleUpdaters.kt DayOfWeekColorUpdater: the zero u32
                    // is at payload+0, then 8 RGBA quads (u8x4, 0..=255).
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
                    // research/xim ParticleUpdaters.kt CameraShakeUpdater: near and far
                    // always; shakeFactor only in the 4-word form (the opCodeSize == 4
                    // branch).
                    SEC3_OPCODE_CAMERA_SHAKE_UPDATER if payload + 8 <= body.len() => {
                        let shake_factor = if size_words == 4 && payload + 12 <= body.len() {
                            f32_le(body, payload + 8)
                        } else {
                            0.0
                        };
                        camera_shake = Some([
                            f32_le(body, payload),
                            f32_le(body, payload + 4),
                            shake_factor,
                        ]);
                    }
                    _ => decoded = false,
                }
                blocks.push((GeneratorSection::Updaters, opcode, decoded));
                cursor += block_len;
            }
        }

        let mut emit_cull = None;
        let mut association = None;
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
                let mut decoded = true;
                match opcode {
                    SEC1_OPCODE_EMIT_CULL if payload + 12 <= body.len() => {
                        emit_cull = Some(EmitCull {
                            max_distance: f32_le(body, payload),
                            min_distance: f32_le(body, payload + 4),
                            unlink_out_of_range: u32_le(body, payload + 8) & 1 != 0,
                        });
                    }
                    // research/xim ParticleGeneratorUpdaters.kt AssociationUpdater read:
                    // followPosition(0x1), followFacing(0x2), followFactor(>>2).
                    SEC1_OPCODE_ASSOCIATION if payload + 4 <= body.len() => {
                        let cfg = u32_le(body, payload);
                        association = Some(AssociationFollow {
                            follow_position: cfg & 1 != 0,
                            follow_facing: cfg & 2 != 0,
                            factor: cfg >> 2,
                        });
                    }
                    _ => decoded = false,
                }
                blocks.push((GeneratorSection::Setup, opcode, decoded));
                cursor += block_len;
            }
        }

        let mut relife_on_expiry = false;
        let sec4_raw = u32_le(body, SEC4_OFFSET) as usize;
        if sec4_raw >= CHUNK_HEADER_LEN && sec4_raw - CHUNK_HEADER_LEN < body.len() {
            let mut cursor = sec4_raw - CHUNK_HEADER_LEN;
            while cursor + 4 <= body.len() {
                let cfg = u32_le(body, cursor);
                let opcode = (cfg & OPCODE_MASK) as u8;
                let size_words = ((cfg >> 8) & u32::from(SIZE_WORDS_MASK)) as usize;
                if opcode == OPCODE_END || size_words == 0 {
                    break;
                }
                // 0x01 is the child-emitter block (research/xim ParticleExpirationHandlers.kt
                // EmitChildHandler: [expectZero32, child generator DAT id]); retail's ElemDie has
                // no case for it and the engine has no child-particle path, so decode-only.
                let decoded = opcode == SEC4_OPCODE_RELIFE || opcode == SEC4_OPCODE_EMIT_CHILD;
                relife_on_expiry |= opcode == SEC4_OPCODE_RELIFE;
                blocks.push((GeneratorSection::ElementDie, opcode, decoded));
                cursor += size_words * 4;
            }
        }

        flush_blocks(sink, &blocks);
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
            spherical_full,
            continuous,
            auto_run,
            batched,
            attach_type,
            attach_joint_source,
            attach_joint_target,
            attach_source_oriented,
            init_scale,
            single_scale_variance,
            scale_variance,
            init_color,
            color_variance,
            color_transform,
            color_transform_modifier,
            init_velocity,
            velocity_variance,
            relative_velocity,
            relative_velocity_variance,
            reverse_displacement,
            rotation_variance,
            init_rotation,
            incremental_rotation,
            blend,
            blend_byte,
            ignore_texture_alpha,
            fog_enabled,
            draw_priority,
            sort_offset,
            projection_bias,
            depth_write,
            scale_x_track,
            scale_y_track,
            scale_z_track,
            alpha_track,
            color_r_track,
            color_g_track,
            color_b_track,
            day_of_week_color,
            moon_phase_color,
            tod_color_tracks,
            tod_color_driven,
            moon_phase_sprite,
            uv_scroll,
            accel,
            rotation_accel,
            scale_accel,
            emit_cull,
            association,
            foot_mark,
            oscillation,
            parent_position_copy,
            parent_velocity,
            child_generator,
            oscillation_accel_z,
            oscillation_accel_x,
            oscillation_accel_y,
            oscillation_applier_x,
            oscillation_applier_z,
            oscillation_applier_y,
            rotation_velocity,
            rotation_velocity_variance,
            rotation_updater,
            position_updater,
            scale_velocity,
            scale_updater,
            scale_velocity_variance,
            relife_on_expiry,
            specular_element,
            specular,
            specular_rot_y_track,
            camera_shake_track,
            camera_shake,
            haze_offset_x,
            parent_rotate,
            parent_color,
            parent_scale,
            velocity_dampener_track,
            velocity_dampener,
            velocity_rotator,
            fixed_point_position_variance,
            fixed_point_position_variance_2,
            child_generator_2,
            specular_rot_z_track,
            specular_color_a_track,
            parent_rotate_2,
            batching_setup,
            parent_tex_coord,
            point_list_position,
            velocity_y_track,
            specular_rot_x_track,
            specular_color_g_track,
        }))
    }

    // The per-frame rotation the element actually turns by: a 0x0B rate with no sec3 0x05
    // updater never turns (research/xi-tools/docs/fx/effects.md "What MOVES an effect").
    pub fn spin(&self) -> Option<[f32; 3]> {
        self.rotation_velocity.filter(|_| self.rotation_updater)
    }

    // The per-frame scale rate the element actually changes: a 0x12 rate with no sec3 0x08
    // updater is never integrated (research/xim ParticleUpdaters.kt ScaleUpdater is the only
    // consumer of the scale transform's velocity).
    pub fn scale_rate(&self) -> Option<[f32; 3]> {
        self.scale_velocity.filter(|_| self.scale_updater)
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
        Self::parse_reporting(body, &mut |_, _, _| {})
    }

    pub fn parse_reporting(body: &[u8], sink: GeneratorOpcodeSink<'_>) -> Result<Option<Self>> {
        let mut blocks: Vec<(GeneratorSection, u8, bool)> = Vec::new();
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
            let mut decoded = true;
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
                _ => decoded = false,
            }
            blocks.push((GeneratorSection::SoundSetup, opcode, decoded));
            cursor += block_len;
        }

        if !is_sound {
            return Ok(None);
        }

        flush_blocks(sink, &blocks);
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

    /// The four zero bytes that terminate section 2's opcode walk.
    pub(crate) const SEC2_TERMINATOR: [u8; 4] = [0u8; 4];

    pub(crate) fn op(opcode: u8, size_words: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![opcode, size_words, 0, 0];
        v.extend_from_slice(payload);
        v.resize(size_words as usize * 4, 0);
        v
    }

    /// The block-relative offset of the StandardParticleSetup linked-data-kind byte:
    /// the 4-word chunk header plus the 29th payload byte (research/xim
    /// ParticleGeneratorSettings.kt LinkedDataType).
    pub(crate) const LINKED_DATA_KIND_OFFSET: usize = 4 + 29;

    /// A StandardParticleSetup block whose linked-data-kind byte selects `kind`;
    /// every other byte is zero.
    pub(crate) fn setup_with_link(kind: u8) -> Vec<u8> {
        let mut setup = op(OPCODE_STANDARD_SETUP, 12, &[]);
        setup[LINKED_DATA_KIND_OFFSET] = kind;
        setup
    }

    /// A generator whose section-3 stream carries the celestial updaters: the moon-phase
    /// sprite-sheet marker when `moon_phase_sprite`, then the day-of-week and moon-phase color
    /// tables (each an expectZero32 followed by RGBA u8 quads).
    pub(crate) fn celestial_generator_body(
        moon_phase_sprite: bool,
        day_of_week: &[[u8; 4]; DAYS_OF_WEEK],
        moon_phase: &[[u8; 4]; MOON_PHASES],
    ) -> Vec<u8> {
        let sec2 = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
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

    /// The two non-visual LinkedDataType values (PointLight / Null): they reject the mesh.
    const LINKED_DATA_POINT_LIGHT: u8 = 0x47;
    const LINKED_DATA_NULL_PARTICLE: u8 = 0x57;

    #[test]
    fn parses_particle_generator_header_and_setup() {
        let mut setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        // billboard XYZ
        setup[4] = 0x01;
        // mesh id at payload+8 (payload = cursor+4 = setup index 4)
        setup[4 + 8..4 + 12].copy_from_slice(b"kir1");
        // base position y at payload+20
        setup[4 + 20..4 + 24].copy_from_slice(&0.2f32.to_le_bytes());
        // max life u16 at payload+30
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
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut body = build(&setup, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
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
    fn velocity_accelerators_keep_their_transform_channels() {
        let allocated = |opcode, slot: u32, payload: &[u8]| {
            let mut block = op(opcode, 4, payload);
            let cfg = u32_le(&block, 0) | (slot << ALLOCATION_SHIFT);
            block[..4].copy_from_slice(&cfg.to_le_bytes());
            block
        };
        let mut setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        setup.extend(allocated(0x02, 1, &[0; 12]));
        setup.extend(allocated(0x0B, 2, &[0; 12]));
        setup.extend(allocated(SEC2_OPCODE_SCALE_VELOCITY, 3, &[0; 12]));
        let mut body = build(&setup, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        let vec3 = |x: f32, y: f32, z: f32| -> [u8; 12] {
            let mut p = [0u8; 12];
            p[0..4].copy_from_slice(&x.to_le_bytes());
            p[4..8].copy_from_slice(&y.to_le_bytes());
            p[8..12].copy_from_slice(&z.to_le_bytes());
            p
        };
        let mut sec3 = op(0x03, 4, &vec3(0.0, -0.03125, 0.0));
        sec3.extend(allocated(0x06, 2, &vec3(0.0, 0.015625, 0.0)));
        sec3.extend(allocated(0x09, 3, &vec3(0.0078125, 0.0, 0.0)));
        body.extend_from_slice(&sec3);

        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.accel, Some([0.0, -0.03125, 0.0]));
        assert_eq!(def.rotation_accel, Some([0.0, 0.015625, 0.0]));
        assert_eq!(def.scale_accel, Some([0.0078125, 0.0, 0.0]));

        let mut body = build(&setup, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        let mut sec3 = allocated(0x06, 2, &vec3(0.0, 0.015625, 0.0));
        sec3.extend(allocated(0x09, 3, &vec3(0.0078125, 0.0, 0.0)));
        body.extend_from_slice(&sec3);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.accel, None);
        assert_eq!(def.rotation_accel, Some([0.0, 0.015625, 0.0]));
        assert_eq!(def.scale_accel, Some([0.0078125, 0.0, 0.0]));
        body.extend(allocated(0x06, 3, &vec3(0.0078125, 0.0, 0.0)));
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.scale_accel, Some([0.015625, 0.0, 0.0]));
    }

    // The celestial opcodes live in the section-3 updater stream (body[0x78]), NOT the
    // section-2 initializer stream, where 0x4E/0x4F mean FixedPointPositionVarianceSetup and
    // 0x45 means ParentPositionCopyConfig (research/xim ParticleGeneratorParser.kt sec2Handler,
    // 239 vs 444, 454-455). Reading them from the wrong stream silently yields None on every
    // real DAT, which is what left the moon on hand-tuned fallback tints.
    #[test]
    fn celestial_updaters_come_from_section3_only() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);

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
        body.extend_from_slice(&SEC2_TERMINATOR);
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
        let mut sec2 = setup_with_link(LINKED_DATA_STATIC_MESH);
        for (opcode, id) in [(0x60u8, b"ksr1"), (0x61, b"ksg1"), (0x62, b"ksb1")] {
            let mut p = vec![0u8; 4];
            p.extend_from_slice(id);
            sec2.extend(op(opcode, 4, &p));
        }
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
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
        let Some(root) = crate::archive::open_test_install() else {
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
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.uv_scroll, [0.0, 0.0]);
        assert_eq!(def.accel, None);
    }

    #[test]
    fn gen_flags_decode_auto_run_and_continuous() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
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
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
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
        let setup = setup_with_link(LINKED_DATA_POINT_LIGHT);
        let body = build(&setup, 1, 1);
        assert!(ParticleGeneratorDef::parse(&body).unwrap().is_none());

        let setup = setup_with_link(LINKED_DATA_NULL_PARTICLE);
        let body = build(&setup, 1, 1);
        assert!(ParticleGeneratorDef::parse(&body).unwrap().is_none());
    }

    /// Poison's venom cloud pin: a `LINKED_DATA_SPRITE_SHEET` generator parses to Some with
    /// `mesh_kind == SpriteSheet`, not just the `LINKED_DATA_STATIC_MESH` kind.
    #[test]
    fn sprite_sheet_setup_parses_with_mesh_kind() {
        let mut setup = setup_with_link(LINKED_DATA_SPRITE_SHEET);
        setup[4 + 8..4 + 12].copy_from_slice(b"fir ");
        setup[4 + 30..4 + 32].copy_from_slice(&24u16.to_le_bytes());
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.mesh_kind, ParticleMeshKind::SpriteSheet);
        assert_eq!(def.mesh_id, *b"fir ");
        assert_eq!(def.max_life_frames, 24.0);
    }

    // 0x1D SpriteSheetInitializer: retail sets the flipbook interval from the CMoD3a resource's
    // frame count, not the DAT (research/XIClient CYyGenerator.cpp CYyGenerator::ElemGenerate
    // case 0x1D), so the block carries no state — the parse must keep the stream aligned for
    // the blocks after it.
    #[test]
    fn sprite_sheet_initializer_consumes_the_block_without_state() {
        let setup = setup_with_link(LINKED_DATA_SPRITE_SHEET);
        let mut sec2 = setup;
        sec2.extend(op(SEC2_OPCODE_SPRITE_SHEET_INIT, 2, &0u32.to_le_bytes()));
        let vel: [f32; 3] = [1.0, 2.0, 3.0];
        let mut vel_bytes = Vec::new();
        for f in vel {
            vel_bytes.extend_from_slice(&f.to_le_bytes());
        }
        sec2.extend(op(0x02, 4, &vel_bytes));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        let def = ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        assert_eq!(
            def.init_velocity, vel,
            "the 0x1D block must not desync the stream"
        );
        assert!(
            outcomes.iter().any(|(s, op, o)| {
                *s == GeneratorSection::Initializers
                    && *op == SEC2_OPCODE_SPRITE_SHEET_INIT
                    && *o == GeneratorOpcodeOutcome::Decoded
            }),
            "0x1D must report decoded: {outcomes:?}"
        );
    }

    // sec3 0x0D SpriteSheetFrameUpdater: a no-payload marker (research/xim
    // ParticleUpdaters.kt SpriteSheetFrameUpdater) — the engine's flipbook_index already
    // advances the frame, so the arm only consumes the block.
    #[test]
    fn sprite_sheet_frame_updater_consumes_the_block_without_state() {
        let setup = setup_with_link(LINKED_DATA_SPRITE_SHEET);
        let mut sec2 = setup.clone();
        sec2.extend(op(OPCODE_END, 0, &[]));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(SEC3_OPCODE_SPRITE_SHEET_FRAME, 1, &[]));
        body.extend_from_slice(&op(OPCODE_END, 0, &[]));

        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        let def = ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        assert!(def.mesh_kind == ParticleMeshKind::SpriteSheet);
        assert!(
            outcomes.iter().any(|(s, op, o)| {
                *s == GeneratorSection::Updaters
                    && *op == SEC3_OPCODE_SPRITE_SHEET_FRAME
                    && *o == GeneratorOpcodeOutcome::Decoded
            }),
            "sec3 0x0D must report decoded: {outcomes:?}"
        );
    }

    // 0x0E NoOpParticleUpdater is a no-payload marker (research/xim ParticleUpdaters.kt
    // NoOpParticleUpdater). retail's ElemIdle case 0x0E computes the keyframe progress as
    // 1.0 - (Life / field_114) — the elapsed-life fraction the engine's `progress`
    // (age/life, particle_sim.rs) already is — so the block arms nothing and only consumes,
    // as for 0x0D.
    #[test]
    fn no_op_particle_updater_consumes_the_block_without_state() {
        let setup = setup_with_link(LINKED_DATA_SPRITE_SHEET);
        let mut sec2 = setup.clone();
        sec2.extend(op(OPCODE_END, 0, &[]));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(SEC3_OPCODE_NO_OP, 1, &[]));
        body.extend_from_slice(&op(OPCODE_END, 0, &[]));

        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        let def = ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        assert!(def.mesh_kind == ParticleMeshKind::SpriteSheet);
        assert!(
            outcomes.iter().any(|(s, op, o)| {
                *s == GeneratorSection::Updaters
                    && *op == SEC3_OPCODE_NO_OP
                    && *o == GeneratorOpcodeOutcome::Decoded
            }),
            "sec3 0x0E must report decoded: {outcomes:?}"
        );
    }

    // sec3 0x02 PositionUpdater: a no-payload marker that arms the position gate (research/xim
    // ParticleUpdaters.kt PositionUpdater; CYyGenerator.cpp CYyGenerator::ElemIdle case 0x02).
    // The flag is set only while the block is present, so a generator without it stays off.
    #[test]
    fn position_updater_flag_arms_only_with_the_block() {
        let setup = setup_with_link(LINKED_DATA_SPRITE_SHEET);
        let mut sec2 = setup.clone();
        sec2.extend(op(OPCODE_END, 0, &[]));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(SEC3_OPCODE_POSITION, 1, &[]));
        body.extend_from_slice(&op(OPCODE_END, 0, &[]));

        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        let def = ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        assert!(def.position_updater);
        assert!(
            outcomes.iter().any(|(s, op, o)| {
                *s == GeneratorSection::Updaters
                    && *op == SEC3_OPCODE_POSITION
                    && *o == GeneratorOpcodeOutcome::Decoded
            }),
            "sec3 0x02 must report decoded: {outcomes:?}"
        );

        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(OPCODE_END, 0, &[]));
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(!def.position_updater);
    }

    // 0x1B color.a ProgressValueUpdater is a no-payload marker (research/xim
    // ParticleGeneratorParser.kt sec3Handler 0x1B). It samples the sec2 0x2D alpha track at
    // life progress, which particle_draw already does from def.alpha_track, so the block arms
    // nothing and only consumes.
    #[test]
    fn alpha_updater_consumes_the_block_without_state() {
        let setup = setup_with_link(LINKED_DATA_SPRITE_SHEET);
        let mut sec2 = setup.clone();
        sec2.extend(op(OPCODE_END, 0, &[]));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(SEC3_OPCODE_ALPHA_UPDATER, 1, &[]));
        body.extend_from_slice(&op(OPCODE_END, 0, &[]));

        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        let def = ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        assert!(def.mesh_kind == ParticleMeshKind::SpriteSheet);
        assert!(
            outcomes.iter().any(|(s, op, o)| {
                *s == GeneratorSection::Updaters
                    && *op == SEC3_OPCODE_ALPHA_UPDATER
                    && *o == GeneratorOpcodeOutcome::Decoded
            }),
            "sec3 0x1B must report decoded: {outcomes:?}"
        );
    }

    // 0x8E FootMarkEffectSetup is a no-payload marker block (research/xim
    // ParticleInitializers.kt FootMarkEffectSetup): a one-dword block between the
    // sprite-sheet initializer and the end of section 2.
    #[test]
    fn foot_mark_setup_sets_the_flag_without_payload() {
        let setup = setup_with_link(LINKED_DATA_SPRITE_SHEET);
        let mut sec2 = setup;
        sec2.extend(op(SEC2_OPCODE_FOOT_MARK, 1, &[]));
        let vel: [f32; 3] = [1.0, 2.0, 3.0];
        let mut vel_bytes = Vec::new();
        for f in vel {
            vel_bytes.extend_from_slice(&f.to_le_bytes());
        }
        sec2.extend(op(0x02, 4, &vel_bytes));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        let def = ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        assert!(def.foot_mark, "0x8E must set the foot-mark flag");
        assert_eq!(
            def.init_velocity, vel,
            "the no-payload block must not desync the stream"
        );
        assert!(
            outcomes.iter().any(|(s, op, o)| {
                *s == GeneratorSection::Initializers
                    && *op == SEC2_OPCODE_FOOT_MARK
                    && *o == GeneratorOpcodeOutcome::Decoded
            }),
            "0x8E must report decoded: {outcomes:?}"
        );
    }

    // 0x3D OscillationSetup is a no-payload marker (research/xim ParticleInitializers.kt
    // OscillationSetup), ahead of its 0x3E/0x40 acceleration setup in the section-2 stream.
    #[test]
    fn oscillation_setup_sets_the_flag_without_payload() {
        let setup = setup_with_link(LINKED_DATA_SPRITE_SHEET);
        let mut sec2 = setup.clone();
        sec2.extend(op(SEC2_OPCODE_OSCILLATION_SETUP, 1, &[]));
        let vel: [f32; 3] = [1.0, 2.0, 3.0];
        let mut vel_bytes = Vec::new();
        for f in vel {
            vel_bytes.extend_from_slice(&f.to_le_bytes());
        }
        sec2.extend(op(0x02, 4, &vel_bytes));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        let def = ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        assert!(def.oscillation, "0x3D must set the oscillation flag");
        assert_eq!(
            def.init_velocity, vel,
            "the no-payload block must not desync the stream"
        );
        assert!(
            outcomes.iter().any(|(s, op, o)| {
                *s == GeneratorSection::Initializers
                    && *op == SEC2_OPCODE_OSCILLATION_SETUP
                    && *o == GeneratorOpcodeOutcome::Decoded
            }),
            "0x3D must report decoded: {outcomes:?}"
        );
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert!(!plain.oscillation);
    }

    // 0x40 OscillationAccelerationSetup (Z): two floats, [acceleration, accelerationVariance]
    // (research/xim ParticleInitializers.kt OscillationAccelerationSetup), behind a 0x3D marker.
    #[test]
    fn oscillation_accel_z_reads_the_two_floats() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(SEC2_OPCODE_OSCILLATION_SETUP, 1, &[]));
        let mut p = Vec::new();
        p.extend_from_slice(&2.0f32.to_le_bytes());
        p.extend_from_slice(&0.5f32.to_le_bytes());
        sec2.extend(op(0x40, 3, &p));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.oscillation);
        assert_eq!(def.oscillation_accel_z, Some([2.0, 0.5]));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.oscillation_accel_z, None);
    }

    // 0x3E OscillationAccelerationSetup (X): the X-axis twin of 0x40 (research/xim
    // ParticleInitializers.kt OscillationAccelerationSetup), behind a 0x3D marker.
    #[test]
    fn oscillation_accel_x_reads_the_two_floats() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(SEC2_OPCODE_OSCILLATION_SETUP, 1, &[]));
        let mut p = Vec::new();
        p.extend_from_slice(&(-1.5f32).to_le_bytes());
        p.extend_from_slice(&0.25f32.to_le_bytes());
        sec2.extend(op(0x3E, 3, &p));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.oscillation);
        assert_eq!(def.oscillation_accel_x, Some([-1.5, 0.25]));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.oscillation_accel_x, None);
    }

    // 0x3F OscillationAccelerationSetup (Y): the Y-axis twin of 0x40 (research/xim
    // ParticleInitializers.kt OscillationAccelerationSetup), behind a 0x3D marker.
    #[test]
    fn oscillation_accel_y_reads_the_two_floats() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(SEC2_OPCODE_OSCILLATION_SETUP, 1, &[]));
        let mut p = Vec::new();
        p.extend_from_slice(&0.44f32.to_le_bytes());
        p.extend_from_slice(&0.6f32.to_le_bytes());
        sec2.extend(op(0x3F, 3, &p));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.oscillation);
        assert_eq!(def.oscillation_accel_y, Some([0.44, 0.6]));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.oscillation_accel_y, None);
    }

    // 0x29 OscillationApplier (X): [rate-divisor, base-offset, unused-in-xim] on the section-3
    // stream (research/xim ParticleUpdaters.kt OscillationApplier — oscillationRate = 180f /
    // payload0, baseOffset = payload1, payload2 "no effect?"); the integrator for the sec2
    // 0x3E acceleration, behind the 0x3D marker.
    #[test]
    fn oscillation_applier_x_reads_the_three_floats() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut p = Vec::new();
        p.extend_from_slice(&0.44f32.to_le_bytes());
        p.extend_from_slice(&0.6f32.to_le_bytes());
        let mut sec2 = setup.clone();
        sec2.extend(op(SEC2_OPCODE_OSCILLATION_SETUP, 1, &[]));
        sec2.extend(op(0x3E, 3, &p));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        let mut ap = Vec::new();
        ap.extend_from_slice(&2.0f32.to_le_bytes());
        ap.extend_from_slice(&1.5f32.to_le_bytes());
        ap.extend_from_slice(&0.25f32.to_le_bytes());
        body.extend_from_slice(&op(0x29, 4, &ap));

        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.oscillation);
        assert_eq!(def.oscillation_applier_x, Some([2.0, 1.5, 0.25]));

        let mut wrong = setup.clone();
        wrong.extend(op(0x29, 4, &ap));
        let plain = ParticleGeneratorDef::parse(&build(&wrong, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.oscillation_applier_x, None);
    }

    // 0x2B OscillationApplier (Z): the Z-axis twin of 0x29 on the section-3 stream (research/xim
    // ParticleUpdaters.kt OscillationApplier), the integrator for the sec2 0x40 acceleration,
    // behind the 0x3D marker.
    #[test]
    fn oscillation_applier_z_reads_the_three_floats() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut p = Vec::new();
        p.extend_from_slice(&0.9f32.to_le_bytes());
        p.extend_from_slice(&0.5f32.to_le_bytes());
        let mut sec2 = setup.clone();
        sec2.extend(op(SEC2_OPCODE_OSCILLATION_SETUP, 1, &[]));
        sec2.extend(op(0x40, 3, &p));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        let mut ap = Vec::new();
        ap.extend_from_slice(&3.0f32.to_le_bytes());
        ap.extend_from_slice(&1.0f32.to_le_bytes());
        ap.extend_from_slice(&0.0f32.to_le_bytes());
        body.extend_from_slice(&op(0x2B, 4, &ap));

        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.oscillation);
        assert_eq!(def.oscillation_applier_z, Some([3.0, 1.0, 0.0]));

        let mut wrong = setup.clone();
        wrong.extend(op(0x2B, 4, &ap));
        let plain = ParticleGeneratorDef::parse(&build(&wrong, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.oscillation_applier_z, None);
    }

    // 0x2A OscillationApplier (Y): the Y-axis twin of 0x29 on the section-3 stream (research/xim
    // ParticleUpdaters.kt OscillationApplier), the integrator for the sec2 0x3F acceleration,
    // behind the 0x3D marker.
    #[test]
    fn oscillation_applier_y_reads_the_three_floats() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut p = Vec::new();
        p.extend_from_slice(&0.44f32.to_le_bytes());
        p.extend_from_slice(&0.6f32.to_le_bytes());
        let mut sec2 = setup.clone();
        sec2.extend(op(SEC2_OPCODE_OSCILLATION_SETUP, 1, &[]));
        sec2.extend(op(0x3F, 3, &p));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        let mut ap = Vec::new();
        ap.extend_from_slice(&4.0f32.to_le_bytes());
        ap.extend_from_slice(&0.5f32.to_le_bytes());
        ap.extend_from_slice(&0.0f32.to_le_bytes());
        body.extend_from_slice(&op(0x2A, 4, &ap));

        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.oscillation);
        assert_eq!(def.oscillation_applier_y, Some([4.0, 0.5, 0.0]));

        let mut wrong = setup.clone();
        wrong.extend(op(0x2A, 4, &ap));
        let plain = ParticleGeneratorDef::parse(&build(&wrong, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.oscillation_applier_y, None);
    }

    // 0x03 VelocityVarianceSetup: the three floats are the per-axis bounds of the random
    // velocity added to the 0x02 base per particle (research/xim ParticleInitializers.kt
    // VelocityVarianceSetup), after its generator's 0x02.
    #[test]
    fn velocity_variance_reads_the_three_axis_bounds() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        let axis = |values: [f32; 3], block: &mut Vec<u8>, opcode: u8| {
            let mut bytes = Vec::new();
            for f in values {
                bytes.extend_from_slice(&f.to_le_bytes());
            }
            block.extend(op(opcode, 4, &bytes));
        };
        let vel: [f32; 3] = [0.5, -0.25, 0.0];
        axis(vel, &mut sec2, 0x02);
        let var: [f32; 3] = [0.1, 0.2, 0.3];
        axis(var, &mut sec2, 0x03);
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.init_velocity, vel);
        assert_eq!(def.velocity_variance, Some(var));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.velocity_variance, None);
    }

    // 0x08 RelativeVelocitySetup: a single float — the magnitude of the per-particle velocity
    // along the spawn offset's direction (research/xim ParticleInitializers.kt
    // RelativeVelocitySetup), after its generator's 0x02 base-velocity block.
    #[test]
    fn relative_velocity_reads_the_single_float() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        let mut base: Vec<u8> = Vec::new();
        for f in [0.5f32, -0.25, 0.0] {
            base.extend_from_slice(&f.to_le_bytes());
        }
        sec2.extend(op(0x02, 4, &base));
        sec2.extend(op(0x08, 2, &0.25f32.to_le_bytes()));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.relative_velocity, Some(0.25));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.relative_velocity, None);
    }

    // 0x41 RelativeVelocityVarianceSetup: a single float — the bound of the uniform random
    // magnitude added to the 0x08 relative velocity along the spawn offset's direction
    // (research/xim ParticleInitializers.kt RelativeVelocityVarianceSetup).
    #[test]
    fn relative_velocity_variance_reads_the_single_float() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x41, 2, &0.2f32.to_le_bytes()));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.relative_velocity_variance, Some(0.2));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.relative_velocity_variance, None);
    }

    // 0x67 ReverseDisplacementSetup: a single float, never read by the effect — the block's
    // presence arms the spawn-at-endpoint behavior
    // (research/xim ParticleInitializers.kt ReverseDisplacementSetup).
    #[test]
    fn reverse_displacement_reads_the_single_float() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x67, 2, &0.0f32.to_le_bytes()));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.reverse_displacement, Some(0.0));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.reverse_displacement, None);
    }

    // 0x29 KeyFrameValueSetup (scale.z): the same block shape as 0x27/0x28 — in-memory
    // pointer, keyframe DAT id, cycle/interpolation config
    // (CYyGenerator.cpp CYyGenerator::ElemGenerate case 0x29).
    #[test]
    fn scale_z_track_reads_the_keyframe_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x29, 4, &[0, 0, 0, 0, b'k', b'1', b'z', b'0']));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.scale_z_track, Some(*b"k1z0"));
        assert_eq!(def.scale_x_track, None);
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.scale_z_track, None);
    }

    // 0x5A KeyFrameValueSetup (specular rotation.y): the 0x27/0x28/0x29 track shape bound to
    // the specular element's rotation y (research/xim ParticleGeneratorParser.kt), behind
    // a 0x55 SpecularParams record in the same generator.
    #[test]
    fn specular_rot_y_track_reads_the_keyframe_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x5A, 4, &[0, 0, 0, 0, b'n', b'0', b'r', b'y']));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.specular_rot_y_track, Some(*b"n0ry"));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.specular_rot_y_track, None);
    }

    // 0x82 CameraShakeSetup: [expectZero32, keyframe track id, unk0 u32, unk1 f32, unk2
    // u32] (research/xim ParticleInitializers.kt CameraShakeSetup); the generator also
    // carries the section-3 0x5F CameraShakeUpdater.
    #[test]
    fn camera_shake_setup_reads_the_keyframe_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        let mut payload = [0u8; 20];
        payload[4..8].copy_from_slice(b"shak");
        sec2.extend(op(0x82, 6, &payload));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.camera_shake_track, Some(*b"shak"));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.camera_shake_track, None);
    }

    // sec3 0x5F CameraShakeUpdater: near, far, and shakeFactor only in the 4-word form
    // (research/xim ParticleUpdaters.kt CameraShakeUpdater — the opCodeSize == 4 branch,
    // behind a sec2 0x82).
    #[test]
    fn camera_shake_updater_reads_the_payload_shape() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let vec3 = |x: f32, y: f32, z: f32| -> [u8; 12] {
            let mut p = [0u8; 12];
            p[0..4].copy_from_slice(&x.to_le_bytes());
            p[4..8].copy_from_slice(&y.to_le_bytes());
            p[8..12].copy_from_slice(&z.to_le_bytes());
            p
        };

        let mut body = build(&setup, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(0x5F, 4, &vec3(2.0, 8.0, 0.001)));
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.camera_shake, Some([2.0, 8.0, 0.001]));

        let mut payload = [0u8; 8];
        payload[0..4].copy_from_slice(&2.0f32.to_le_bytes());
        payload[4..8].copy_from_slice(&8.0f32.to_le_bytes());
        let mut body = build(&setup, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(0x5F, 3, &payload));
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.camera_shake, Some([2.0, 8.0, 0.0]));

        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.camera_shake, None);
    }

    // 0x32 HazeOffsetInitializer: [unused f32, horizontal offset] — xim applies only the
    // second float, as particle.hazeOffset.x (research/xim ParticleInitializers.kt
    // HazeOffsetInitializer).
    #[test]
    fn haze_offset_reads_the_second_float() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        let mut payload = [0u8; 8];
        payload[0..4].copy_from_slice(&0.5f32.to_le_bytes());
        payload[4..8].copy_from_slice(&1.25f32.to_le_bytes());
        sec2.extend(op(0x32, 3, &payload));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.haze_offset_x, Some(1.25));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.haze_offset_x, None);
    }

    // 0x47 ParentRotateConfig: a no-payload marker (research/xim
    // ParticleInitializers.kt ParentRotateConfig).
    #[test]
    fn parent_rotate_is_a_no_payload_marker() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x47, 1, &[]));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.parent_rotate);
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert!(!plain.parent_rotate);
    }

    // 0x79 ParentRotateConfig: the 0x47 marker — xim maps both opcodes to the same
    // class (research/xim ParticleGeneratorParser.kt sec2Handler).
    #[test]
    fn parent_rotate_twin_is_a_no_payload_marker() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x79, 1, &[]));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.parent_rotate_2);
        assert!(!def.parent_rotate);
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert!(!plain.parent_rotate_2);
    }

    // 0x56 BatchingSetup: one expectZero32 word (research/xim ParticleInitializers.kt
    // BatchingSetup).
    #[test]
    fn batching_setup_is_a_single_word_marker() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x56, 2, &[0, 0, 0, 0]));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.batching_setup);
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert!(!plain.batching_setup);
    }

    // 0x4A ParentTexCoordConfig: a no-payload marker (research/xim
    // ParticleInitializers.kt ParentTexCoordConfig).
    #[test]
    fn parent_tex_coord_is_a_no_payload_marker() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x4A, 1, &[]));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.parent_tex_coord);
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert!(!plain.parent_tex_coord);
    }

    // 0x54 PointListPositionSetup: [in-mem ptr, keyframe DAT id, expect zero, in-mem ptr,
    // point list DAT id] (research/xim ParticleInitializers.kt PointListPositionSetup).
    #[test]
    fn point_list_position_reads_the_two_dat_ids() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        let mut payload = Vec::new();
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&0x1122_3344u32.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&0x5566_7788u32.to_le_bytes());
        sec2.extend(op(0x54, 6, &payload));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(
            def.point_list_position,
            Some(([0x44, 0x33, 0x22, 0x11], [0x88, 0x77, 0x66, 0x55]))
        );
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.point_list_position, None);
    }

    // 0x51 KeyFrameValueSetup (velocity.y): the 0x27/0x28/0x29 track shape (research/xim
    // ParticleGeneratorParser.kt sec2Handler).
    #[test]
    fn velocity_y_track_reads_the_keyframe_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(
            0x51,
            4,
            &[0, 0, 0, 0, b'v', b'y', b't', b'k', 0, 0, 0, 0],
        ));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.velocity_y_track, Some(*b"vytk"));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.velocity_y_track, None);
    }

    // 0x59 KeyFrameValueSetup (specular rot.x): the 0x27/0x28/0x29 track shape (research/xim
    // ParticleGeneratorParser.kt sec2Handler).
    #[test]
    fn specular_rot_x_track_reads_the_keyframe_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(
            0x59,
            4,
            &[0, 0, 0, 0, b's', b'r', b'x', b'0', 0, 0, 0, 0],
        ));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.specular_rot_x_track, Some(*b"srx0"));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.specular_rot_x_track, None);
    }

    // 0x5D KeyFrameValueSetup (specular color.g): the 0x27/0x28/0x29 track shape
    // (research/xim ParticleGeneratorParser.kt sec2Handler).
    #[test]
    fn specular_color_g_track_reads_the_keyframe_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(
            0x5D,
            4,
            &[0, 0, 0, 0, b's', b'c', b'g', b'0', 0, 0, 0, 0],
        ));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.specular_color_g_track, Some(*b"scg0"));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.specular_color_g_track, None);
    }

    // 0x48 ParentColorConfig: a no-payload marker (research/xim
    // ParticleInitializers.kt ParentColorConfig).
    #[test]
    fn parent_color_is_a_no_payload_marker() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x48, 1, &[]));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.parent_color);
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert!(!plain.parent_color);
    }

    // 0x49 ParentScaleConfig: a no-payload marker (research/xim
    // ParticleInitializers.kt ParentScaleConfig).
    #[test]
    fn parent_scale_is_a_no_payload_marker() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x49, 1, &[]));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.parent_scale);
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert!(!plain.parent_scale);
    }

    // 0x69 KeyFrameValueSetup (velocity dampener): the 0x27/0x28/0x29 track shape bound to
    // the element's velocity dampener (research/xim ParticleGeneratorParser.kt sec2Handler).
    #[test]
    fn velocity_dampener_track_reads_the_keyframe_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x69, 4, &[0, 0, 0, 0, b'v', b'd', b'm', b'0']));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.velocity_dampener_track, Some(*b"vdm0"));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.velocity_dampener_track, None);
    }

    // sec3 0x2C VelocityDampener: two floats [dampen, unk] (research/xim
    // ParticleUpdaters.kt VelocityDampener).
    #[test]
    fn velocity_dampener_reads_the_two_floats() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut payload = [0u8; 8];
        payload[0..4].copy_from_slice(&0.9f32.to_le_bytes());
        payload[4..8].copy_from_slice(&0.25f32.to_le_bytes());
        let mut body = build(&setup, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(0x2C, 3, &payload));
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.velocity_dampener, Some([0.9, 0.25]));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.velocity_dampener, None);
    }

    // sec3 0x26 VelocityRotator: three floats, the rotateAmount (research/xim
    // ParticleUpdaters.kt VelocityRotator).
    #[test]
    fn velocity_rotator_reads_the_three_floats() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut payload = [0u8; 12];
        payload[0..4].copy_from_slice(&0.1f32.to_le_bytes());
        payload[4..8].copy_from_slice(&(-0.2f32).to_le_bytes());
        payload[8..12].copy_from_slice(&0.3f32.to_le_bytes());
        let mut body = build(&setup, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(0x26, 4, &payload));
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.velocity_rotator, Some([0.1, -0.2, 0.3]));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.velocity_rotator, None);
    }

    // sec3 0x44 dampening-factor ProgressValueUpdater: no payload, it samples the sec2 0x69
    // track (research/xim ParticleGeneratorParser.kt sec3Handler 0x44), behind a sec2 0x69.
    #[test]
    fn dampening_factor_updater_consumes_the_block_without_state() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(OPCODE_END, 0, &[]));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(SEC3_OPCODE_DAMPENING_FACTOR, 1, &[]));
        body.extend_from_slice(&op(OPCODE_END, 0, &[]));

        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        let def = ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        assert_eq!(def.velocity_dampener, None);
        assert!(
            outcomes.iter().any(|(s, op, o)| {
                *s == GeneratorSection::Updaters
                    && *op == SEC3_OPCODE_DAMPENING_FACTOR
                    && *o == GeneratorOpcodeOutcome::Decoded
            }),
            "sec3 0x44 must report decoded: {outcomes:?}"
        );
    }

    // sec3 0x15/0x16/0x17 scale.x/y/z ProgressValueUpdaters: no payload — they sample the
    // sec2 0x27/0x28/0x29 scale tracks at life progress, which the render path already does
    // from def.scale_x_track/scale_y_track (research/xim ParticleGeneratorParser.kt
    // sec3Handler), behind their sec2 scale track.
    #[test]
    fn scale_progress_updaters_consume_the_blocks_without_state() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(OPCODE_END, 0, &[]));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(0x15, 1, &[]));
        body.extend_from_slice(&op(0x16, 1, &[]));
        body.extend_from_slice(&op(0x17, 1, &[]));
        body.extend_from_slice(&op(OPCODE_END, 0, &[]));

        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        for op in [0x15, 0x16, 0x17] {
            assert!(
                outcomes.iter().any(|(s, o, outcome)| {
                    *s == GeneratorSection::Updaters
                        && *o == op
                        && *outcome == GeneratorOpcodeOutcome::Decoded
                }),
                "sec3 {op:02X} must report decoded: {outcomes:?}"
            );
        }
    }

    // sec3 0x18/0x19/0x1A color.r/g/b ProgressValueUpdaters: no payload — they sample the
    // sec2 0x2A/0x2B/0x2C color tracks at life progress; the engine's rgb is spawn-time
    // only (research/xim ParticleGeneratorParser.kt sec3Handler), behind their sec2 color
    // track.
    #[test]
    fn color_rgb_progress_updaters_consume_the_blocks_without_state() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(OPCODE_END, 0, &[]));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(0x18, 1, &[]));
        body.extend_from_slice(&op(0x19, 1, &[]));
        body.extend_from_slice(&op(0x1A, 1, &[]));
        body.extend_from_slice(&op(OPCODE_END, 0, &[]));

        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        for op in [0x18, 0x19, 0x1A] {
            assert!(
                outcomes.iter().any(|(s, o, outcome)| {
                    *s == GeneratorSection::Updaters
                        && *o == op
                        && *outcome == GeneratorOpcodeOutcome::Decoded
                }),
                "sec3 {op:02X} must report decoded: {outcomes:?}"
            );
        }
    }

    // sec3 0x36/0x37/0x3B specular rotation.y/z and color.a ProgressValueUpdaters: no
    // payload — they sample the sec2 0x5A/0x5B/0x5F specular tracks; the engine does not
    // model the specular element (research/xim ParticleGeneratorParser.kt sec3Handler),
    // behind their sec2 specular track.
    #[test]
    fn specular_progress_updaters_consume_the_blocks_without_state() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(OPCODE_END, 0, &[]));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(0x36, 1, &[]));
        body.extend_from_slice(&op(0x37, 1, &[]));
        body.extend_from_slice(&op(0x3B, 1, &[]));
        body.extend_from_slice(&op(OPCODE_END, 0, &[]));

        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        for op in [0x36, 0x37, 0x3B] {
            assert!(
                outcomes.iter().any(|(s, o, outcome)| {
                    *s == GeneratorSection::Updaters
                        && *o == op
                        && *outcome == GeneratorOpcodeOutcome::Decoded
                }),
                "sec3 {op:02X} must report decoded: {outcomes:?}"
            );
        }
    }

    // sec3 0x0B ColorTransformApplier: no payload — color += (transform shr 7) × (0.5 × dt)
    // per frame; the engine does not model the color transform's application (research/xim
    // ParticleUpdaters.kt ColorTransformApplier).
    #[test]
    fn color_transform_applier_consumes_the_block_without_state() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(OPCODE_END, 0, &[]));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(SEC3_OPCODE_COLOR_TRANSFORM_APPLIER, 1, &[]));
        body.extend_from_slice(&op(OPCODE_END, 0, &[]));

        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        assert!(
            outcomes.iter().any(|(s, o, outcome)| {
                *s == GeneratorSection::Updaters
                    && *o == SEC3_OPCODE_COLOR_TRANSFORM_APPLIER
                    && *outcome == GeneratorOpcodeOutcome::Decoded
            }),
            "sec3 0x0B must report decoded: {outcomes:?}"
        );
    }

    // sec3 0x25/0x33 ChildGeneratorBasicUpdater / ChildGeneratorUpdater: no payload — they
    // emit/update the sec2 0x44/0x53 child generator per particle; the engine has no
    // child-particle path (research/xim ParticleGeneratorParser.kt sec3Handler), behind
    // their sec2 child link.
    #[test]
    fn child_generator_updaters_consume_the_blocks_without_state() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(OPCODE_END, 0, &[]));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(0x25, 1, &[]));
        body.extend_from_slice(&op(0x33, 1, &[]));
        body.extend_from_slice(&op(OPCODE_END, 0, &[]));

        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        for op in [0x25, 0x33] {
            assert!(
                outcomes.iter().any(|(s, o, outcome)| {
                    *s == GeneratorSection::Updaters
                        && *o == op
                        && *outcome == GeneratorOpcodeOutcome::Decoded
                }),
                "sec3 {op:02X} must report decoded: {outcomes:?}"
            );
        }
    }

    // sec3 0x2F VelocityRotationUpdater: no payload — converts all velocity into the +x
    // axis and copies the particle's rotation into the velocity rotation; the engine has
    // no velocityRotation (research/xim ParticleUpdaters.kt VelocityRotationUpdater).
    #[test]
    fn velocity_rotation_updater_consumes_the_block_without_state() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(OPCODE_END, 0, &[]));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(SEC3_OPCODE_VELOCITY_ROTATION_UPDATER, 1, &[]));
        body.extend_from_slice(&op(OPCODE_END, 0, &[]));

        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        assert!(
            outcomes.iter().any(|(s, o, outcome)| {
                *s == GeneratorSection::Updaters
                    && *o == SEC3_OPCODE_VELOCITY_ROTATION_UPDATER
                    && *outcome == GeneratorOpcodeOutcome::Decoded
            }),
            "sec3 0x2F must report decoded: {outcomes:?}"
        );
    }

    // sec3 0x34 PointListPositionUpdater: no payload — samples the sec2 0x54 point-list
    // spline at the particle's progress and copies it to the position; the engine has no
    // point-list spline runtime (research/xim ParticleUpdaters.kt
    // PointListPositionUpdater), paired with the sec2 0x54 setup.
    #[test]
    fn point_list_position_updater_consumes_the_block_without_state() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(OPCODE_END, 0, &[]));
        let mut body = build(&sec2, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(SEC3_OPCODE_POINT_LIST_POSITION, 1, &[]));
        body.extend_from_slice(&op(OPCODE_END, 0, &[]));

        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        assert!(
            outcomes.iter().any(|(s, o, outcome)| {
                *s == GeneratorSection::Updaters
                    && *o == SEC3_OPCODE_POINT_LIST_POSITION
                    && *outcome == GeneratorOpcodeOutcome::Decoded
            }),
            "sec3 0x34 must report decoded: {outcomes:?}"
        );
    }

    // 0x4E FixedPointPositionVarianceSetup: [expectZero32, point list DAT id, expect32
    // (0, 1)] (research/xim ParticleInitializers.kt FixedPointPositionVarianceSetup).
    #[test]
    fn fixed_point_position_variance_reads_the_point_list_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        let mut payload = [0u8; 12];
        payload[4..8].copy_from_slice(b"pts0");
        payload[8..12].copy_from_slice(&1u32.to_le_bytes());
        sec2.extend(op(0x4E, 4, &payload));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.fixed_point_position_variance, Some(*b"pts0"));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.fixed_point_position_variance, None);
    }

    // 0x4F FixedPointPositionVarianceSetup: the twin of 0x4E — xim maps both opcodes to
    // the same class (research/xim ParticleGeneratorParser.kt sec2Handler).
    #[test]
    fn fixed_point_position_variance_twin_reads_the_point_list_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        let mut payload = [0u8; 12];
        payload[4..8].copy_from_slice(b"pts1");
        payload[8..12].copy_from_slice(&0u32.to_le_bytes());
        sec2.extend(op(0x4F, 4, &payload));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.fixed_point_position_variance_2, Some(*b"pts1"));
        assert_eq!(def.fixed_point_position_variance, None);
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.fixed_point_position_variance_2, None);
    }

    // 0x53 ChildGeneratorSetup: the 0x44 shape — xim maps both opcodes to the same class
    // (research/xim ParticleGeneratorParser.kt sec2Handler).
    #[test]
    fn child_generator_twin_reads_the_child_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x53, 3, &[0, 0, 0, 0, b'k', b'i', b'd', b'2']));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.child_generator_2, Some(*b"kid2"));
        assert_eq!(def.child_generator, None);
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.child_generator_2, None);
    }

    // 0x5B KeyFrameValueSetup (specular rotation.z): the 0x5A shape bound to the specular
    // element's rotation z (research/xim ParticleGeneratorParser.kt).
    #[test]
    fn specular_rot_z_track_reads_the_keyframe_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x5B, 4, &[0, 0, 0, 0, b'n', b'0', b'r', b'z']));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.specular_rot_z_track, Some(*b"n0rz"));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.specular_rot_z_track, None);
    }

    // 0x5F KeyFrameValueSetup (specular color.a): the 0x5A shape bound to the specular
    // element's color alpha (research/xim ParticleGeneratorParser.kt).
    #[test]
    fn specular_color_a_track_reads_the_keyframe_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x5F, 4, &[0, 0, 0, 0, b's', b'p', b'a', b'0']));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.specular_color_a_track, Some(*b"spa0"));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.specular_color_a_track, None);
    }

    // 0x45 ParentPositionCopyConfig: a no-payload marker (research/xim
    // ParticleInitializers.kt ParentPositionCopyConfig); some generators are referenced as
    // a child by a sec2 0x44 link.
    #[test]
    fn parent_position_copy_is_a_no_payload_marker() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x45, 1, &[]));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.parent_position_copy);
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert!(!plain.parent_position_copy);
    }

    // 0x46 ParentVelocityConfig: one float, the multiplier on the parent's total velocity
    // (research/xim ParticleInitializers.kt ParentVelocityConfig).
    #[test]
    fn parent_velocity_reads_the_single_float() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x46, 2, &2.5f32.to_le_bytes()));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.parent_velocity, Some(2.5));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.parent_velocity, None);
    }

    // 0x44 ChildGeneratorSetup: [expectZero32, child generator DAT id] (research/xim
    // ParticleInitializers.kt ChildGeneratorSetup).
    #[test]
    fn child_generator_setup_reads_the_child_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x44, 3, &[0, 0, 0, 0, b'k', b'i', b'd', b'0']));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.child_generator, Some(*b"kid0"));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.child_generator, None);
    }

    // 0x2A KeyFrameValueSetup (color.r): the 0x27/0x28/0x29 track shape bound to the
    // element's red channel (research/xim ParticleGeneratorParser.kt).
    #[test]
    fn color_r_track_reads_the_keyframe_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x2A, 4, &[0, 0, 0, 0, b'c', b'r', b'0', b'1']));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.color_r_track, Some(*b"cr01"));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.color_r_track, None);
    }

    // 0x2B KeyFrameValueSetup (color.g): the 0x27/0x28/0x29 track shape bound to the
    // element's green channel (research/xim ParticleGeneratorParser.kt).
    #[test]
    fn color_g_track_reads_the_keyframe_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x2B, 4, &[0, 0, 0, 0, b'c', b'g', b'0', b'1']));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.color_g_track, Some(*b"cg01"));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.color_g_track, None);
    }

    // 0x2C KeyFrameValueSetup (color.b): the 0x27/0x28/0x29 track shape bound to the
    // element's blue channel (research/xim ParticleGeneratorParser.kt).
    #[test]
    fn color_b_track_reads_the_keyframe_id() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x2C, 4, &[0, 0, 0, 0, b'c', b'b', b'0', b'1']));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.color_b_track, Some(*b"cb01"));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.color_b_track, None);
    }

    // 0x3B IncrementalRotationApplier: three floats, the per-axis rotation increment
    // (research/xim ParticleInitializers.kt IncrementalRotationApplier); the payloads are
    // radian angles.
    #[test]
    fn incremental_rotation_reads_the_three_floats() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        sec2.extend(op(0x3B, 4, &{
            let mut p = Vec::new();
            p.extend_from_slice(&0.1f32.to_le_bytes());
            p.extend_from_slice(&(-0.2f32).to_le_bytes());
            p.extend_from_slice(&0.3f32.to_le_bytes());
            p
        }));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.incremental_rotation, Some([0.1, -0.2, 0.3]));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.incremental_rotation, None);
    }

    // 0x0A RotationVarianceInitializer: three floats, the per-axis bounds of the random
    // rotation added to the 0x09 base per particle (research/xim ParticleInitializers.kt
    // RotationVarianceInitializer); the payloads are radian angles.
    #[test]
    fn rotation_variance_reads_the_three_axis_bounds() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        let mut base: Vec<u8> = Vec::new();
        for f in [0.1f32, 0.2, 0.3] {
            base.extend_from_slice(&f.to_le_bytes());
        }
        sec2.extend(op(0x09, 4, &base));
        let mut var: Vec<u8> = Vec::new();
        for f in [0.05f32, 0.1, 0.15] {
            var.extend_from_slice(&f.to_le_bytes());
        }
        sec2.extend(op(0x0A, 4, &var));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.init_rotation, [0.1, 0.2, 0.3]);
        assert_eq!(def.rotation_variance, Some([0.05, 0.1, 0.15]));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.rotation_variance, None);
    }

    // 0x0C VelocityVarianceSetup (rotation): three floats, the per-axis bounds of the random
    // spin added to the 0x0B rate per particle (research/xim ParticleInitializers.kt — the
    // allocationOffset binds it to the rotation transform).
    #[test]
    fn rotation_velocity_variance_reads_the_three_axis_bounds() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        let mut rate: Vec<u8> = Vec::new();
        for f in [0.0f32, 0.01, 0.0] {
            rate.extend_from_slice(&f.to_le_bytes());
        }
        sec2.extend(op(0x0B, 4, &rate));
        let mut var: Vec<u8> = Vec::new();
        for f in [0.0f32, 0.005, 0.0] {
            var.extend_from_slice(&f.to_le_bytes());
        }
        sec2.extend(op(0x0C, 4, &var));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.rotation_velocity, Some([0.0, 0.01, 0.0]));
        assert_eq!(def.rotation_velocity_variance, Some([0.0, 0.005, 0.0]));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.rotation_velocity_variance, None);
    }

    // 0x11 SingleScaleVarianceInitializer: one float, the ufrand bound shared by every scale
    // axis per particle (research/xim ParticleInitializers.kt — scale += posRand(v)).
    #[test]
    fn single_scale_variance_reads_the_single_float() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut sec2 = setup.clone();
        let mut base: Vec<u8> = Vec::new();
        for f in [0.1f32, 0.2, 0.3] {
            base.extend_from_slice(&f.to_le_bytes());
        }
        sec2.extend(op(0x0F, 4, &base));
        sec2.extend(op(0x11, 2, &0.05f32.to_le_bytes()));
        sec2.extend(op(OPCODE_END, 0, &[]));
        let body = build(&sec2, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.init_scale, [0.1, 0.2, 0.3]);
        assert_eq!(def.single_scale_variance, Some(0.05));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.single_scale_variance, None);
    }

    // 0x10 ScaleVarianceInitializer: three floats, the per-axis ufrand bound added to the 0x0F
    // base scale per particle (CYyGenerator.cpp CYyGenerator::ElemGenerate case 0x10 —
    // field_EC.x/y/z += ufrand(payload)), behind a 0x0F base scale.
    #[test]
    fn scale_variance_reads_the_three_floats() {
        let mut setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut payload = Vec::new();
        for f in [0.5f32, 0.25, 0.125] {
            payload.extend_from_slice(&f.to_le_bytes());
        }
        setup.extend(op(0x10, 4, &payload));
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.scale_variance, Some([0.5, 0.25, 0.125]));
        let plain =
            ParticleGeneratorDef::parse(&build(&setup_with_link(LINKED_DATA_STATIC_MESH), 1, 1))
                .unwrap()
                .unwrap();
        assert_eq!(plain.scale_variance, None);
    }

    // 0x1F SphericalPositionVarianceFull: nine floats, the camera flag u32, and the
    // azimuth-step u16, which CYyGenerator.cpp CYyGenerator::ElemGenerate case 0x1F reads as
    // 0 in the random-azimuth case and one higher than the step count otherwise.
    #[test]
    fn spherical_full_reads_the_payload_fields() {
        let payload = |camera: u32, raw_steps: u16| {
            let mut p: Vec<u8> = Vec::new();
            for f in [0.25f32, 0.5, 1.0, 1.5, 2.0, 0.1, 0.2, 0.3, 0.4] {
                p.extend_from_slice(&f.to_le_bytes());
            }
            p.extend_from_slice(&camera.to_le_bytes());
            p.extend_from_slice(&raw_steps.to_le_bytes());
            p
        };
        let mut sec2 = setup_with_link(LINKED_DATA_STATIC_MESH);
        sec2.extend(op(0x1F, 12, &payload(1, 4)));
        let def = ParticleGeneratorDef::parse(&build(&sec2, 1, 1))
            .unwrap()
            .unwrap();
        let sp = def.spherical_full.expect("sec2 0x1F block");
        assert_eq!(sp.radius_variance, 0.25);
        assert_eq!(sp.base_radius, 0.5);
        assert_eq!(sp.axis_scale, [1.0, 1.5, 2.0]);
        assert_eq!(sp.rotation_z, 0.1);
        assert_eq!(sp.rotation_y, 0.2);
        assert_eq!(sp.tilt, 0.3);
        assert_eq!(sp.tilt_variance, 0.4);
        assert!(sp.camera_oriented);
        assert_eq!(sp.azimuth_steps, 5);

        let mut sec2 = setup_with_link(LINKED_DATA_STATIC_MESH);
        sec2.extend(op(0x1F, 12, &payload(0, 0)));
        let def = ParticleGeneratorDef::parse(&build(&sec2, 1, 1))
            .unwrap()
            .unwrap();
        let sp = def.spherical_full.expect("sec2 0x1F block");
        assert!(!sp.camera_oriented);
        assert_eq!(sp.azimuth_steps, 0, "raw 0 is the random-azimuth case");

        let plain =
            ParticleGeneratorDef::parse(&build(&setup_with_link(LINKED_DATA_STATIC_MESH), 1, 1))
                .unwrap()
                .unwrap();
        assert_eq!(plain.spherical_full, None);
    }

    // 0x12 ScaleVelocitySetup: three floats, the per-frame scale rate per axis; only the sec3
    // 0x08 ScaleUpdater integrates it (research/xim ParticleUpdaters.kt — scale += velocity ×
    // elapsedFrames; retail's ElemGenerate shares the 0x0B/0x12 memcpy case).
    #[test]
    fn scale_velocity_reads_the_three_axis_rate() {
        let mut sec2 = setup_with_link(LINKED_DATA_STATIC_MESH);
        sec2.extend(op(0x12, 4, &vec3_payload([0.0, 0.01, 0.0])));
        let rate_only = ParticleGeneratorDef::parse(&build(&sec2, 120, 0x1400))
            .unwrap()
            .unwrap();
        assert_eq!(rate_only.scale_velocity, Some([0.0, 0.01, 0.0]));
        assert!(!rate_only.scale_updater);
        assert_eq!(rate_only.scale_rate(), None);

        let body = with_section(build(&sec2, 120, 0x1400), 0x78, &op(0x08, 1, &[]));
        let growing = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(growing.scale_updater);
        assert_eq!(growing.scale_rate(), Some([0.0, 0.01, 0.0]));
    }

    // 0x13 VelocityVarianceSetup (scale): three floats, the per-axis variance bound on the
    // 0x12 scale velocity (research/xim ParticleInitializers.kt VelocityVarianceSetup — the
    // allocationOffset binds it to the scale transform; retail's shared 0x03/0x0C/0x13 case
    // adds frand(bounds) to the transform's velocity).
    #[test]
    fn scale_velocity_variance_reads_the_three_floats() {
        let mut sec2 = setup_with_link(LINKED_DATA_STATIC_MESH);
        sec2.extend(op(0x13, 4, &vec3_payload([0.0, 0.005, 0.0])));
        let def = ParticleGeneratorDef::parse(&build(&sec2, 120, 0x1400))
            .unwrap()
            .unwrap();
        assert_eq!(def.scale_velocity_variance, Some([0.0, 0.005, 0.0]));
        let plain = ParticleGeneratorDef::parse(&build(
            &setup_with_link(LINKED_DATA_STATIC_MESH),
            120,
            0x1400,
        ))
        .unwrap()
        .unwrap();
        assert_eq!(plain.scale_velocity_variance, None);
    }

    // research/xim ParticleInitializers.kt — renderStateFlags is the u16 after the
    // billboard flags; 0x1000 picks retail's NonZeroOneTSS element (CMoD3m.cpp CMoD3m::Draw).
    #[test]
    fn render_state_flag_selects_ignore_texture_alpha_element() {
        let element = |render_state: u16| {
            let mut setup = setup_with_link(LINKED_DATA_STATIC_MESH);
            setup[4 + 2..4 + 4].copy_from_slice(&render_state.to_le_bytes());
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

    // CMoElem.cpp CMoElem::PrepDX — fog is on unless the element sets 0x0200.
    #[test]
    fn render_state_flag_0x0200_disables_fog() {
        let fogged = |render_state: u16| {
            let mut setup = setup_with_link(LINKED_DATA_STATIC_MESH);
            setup[4 + 2..4 + 4].copy_from_slice(&render_state.to_le_bytes());
            let body = build(&setup, 1, 1);
            ParticleGeneratorDef::parse(&body)
                .unwrap()
                .unwrap()
                .fog_enabled
        };
        assert!(fogged(0x0000));
        assert!(fogged(0x1000));
        assert!(!fogged(0x0200));
        assert!(!fogged(0x1200));
    }

    // CMoElem.cpp CMoElem::OnDraw / CMoElem::PrepDX — Lower Jeuno's sea group as shipped in
    // DAT 345: `down` is billboard 0x1000 / render-state 0x0800, `col1` and both sheets are
    // render-state 0x0040.
    #[test]
    fn render_state_and_billboard_words_select_draw_priority_and_depth_write() {
        let parsed = |billboard: u16, render_state: u16| {
            let mut setup = setup_with_link(LINKED_DATA_STATIC_MESH);
            setup[4..4 + 2].copy_from_slice(&billboard.to_le_bytes());
            setup[4 + 2..4 + 4].copy_from_slice(&render_state.to_le_bytes());
            let body = build(&setup, 1, 1);
            let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
            (def.draw_priority, def.depth_write)
        };
        assert_eq!(parsed(0x0000, 0x0000), (DrawPriority::Depth, false));
        assert_eq!(parsed(0x1000, 0x0800), (DrawPriority::Low, true));
        assert_eq!(parsed(0x0000, 0x0040), (DrawPriority::Pinned, false));
        assert_eq!(
            parsed(0x0000, 0x0840),
            (DrawPriority::Low, false),
            "low priority is tested after the pinned key and overrides it"
        );
    }

    // CYyGenerator.cpp CYyGenerator::ElemGenerate opcode 0x30 — one float, the sort offset.
    #[test]
    fn opcode_0x30_sets_the_sort_offset() {
        let mut setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        setup.extend(op(0x30, 2, &7.5f32.to_le_bytes()));
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.sort_offset, 7.5);
        let plain = setup_with_link(LINKED_DATA_STATIC_MESH);
        let body = build(&plain, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.sort_offset, 0.0);
    }

    // 0x72 ProjectionBiasInitializer: two floats — param0 lands in sort_offset (the same
    // field_128 0x30 writes), param1 is kept as the actor depth-scale factor
    // (research/xim ParticleInitializers.kt ProjectionBiasInitializer); no 0x30 co-occurrence.
    #[test]
    fn projection_bias_reads_the_two_floats() {
        let mut setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut payload = Vec::new();
        for f in [-0.5f32, 2.0] {
            payload.extend_from_slice(&f.to_le_bytes());
        }
        setup.extend(op(0x72, 3, &payload));
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.sort_offset, -0.5);
        assert_eq!(def.projection_bias, Some([-0.5, 2.0]));
        let plain =
            ParticleGeneratorDef::parse(&build(&setup_with_link(LINKED_DATA_STATIC_MESH), 1, 1))
                .unwrap()
                .unwrap();
        assert_eq!(plain.sort_offset, 0.0);
        assert_eq!(plain.projection_bias, None);
    }

    // 0x17 ColorVarianceSetup: four bytes / 255, the per-channel upward variance bound
    // (research/xim ParticleInitializers.kt ColorVarianceSetup); the alpha byte is 0,
    // behind a 0x16 base color.
    #[test]
    fn color_variance_reads_the_four_bytes() {
        let mut setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        setup.extend(op(0x17, 2, &[20u8, 10, 5, 0]));
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(
            def.color_variance,
            Some([20.0 / 255.0, 10.0 / 255.0, 5.0 / 255.0, 0.0])
        );
        let plain =
            ParticleGeneratorDef::parse(&build(&setup_with_link(LINKED_DATA_STATIC_MESH), 1, 1))
                .unwrap()
                .unwrap();
        assert_eq!(plain.color_variance, None);
    }

    // 0x19 ColorTransformSetup: four i16s, parsed only — the retail decompile's ElemGenerate
    // has no 0x19 case and xim's drawers never read the allocated transform
    // (research/xim ParticleInitializers.kt ColorTransformSetup); the alpha is 0, behind a
    // 0x16 base color.
    #[test]
    fn color_transform_reads_the_four_i16s() {
        let mut setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut payload = Vec::new();
        for v in [-160i16, -160, 0, 0] {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        setup.extend(op(0x19, 3, &payload));
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.color_transform, Some([-160, -160, 0, 0]));
        let plain =
            ParticleGeneratorDef::parse(&build(&setup_with_link(LINKED_DATA_STATIC_MESH), 1, 1))
                .unwrap()
                .unwrap();
        assert_eq!(plain.color_transform, None);
    }

    // sec3 0x0C ColorTransformModifier: four i16s, the per-frame rate on the sec2 0x19
    // color transform (research/xim ParticleUpdaters.kt ColorTransformModifier).
    #[test]
    fn color_transform_modifier_reads_the_four_i16s() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let mut payload = Vec::new();
        for v in [1i16, -2, 3, 0] {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        let mut body = build(&setup, 1, 1);
        body.extend_from_slice(&SEC2_TERMINATOR);
        let sec3_body_index = body.len();
        body[0x78..0x7C].copy_from_slice(&((sec3_body_index + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(&op(0x0C, 3, &payload));
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.color_transform_modifier, Some([1, -2, 3, 0]));
        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.color_transform_modifier, None);
    }

    fn vec3_payload(v: [f32; 3]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Appends a section stream after the body and points the section word at it.
    fn with_section(mut body: Vec<u8>, offset_word: usize, stream: &[u8]) -> Vec<u8> {
        body.extend_from_slice(&SEC2_TERMINATOR);
        let at = body.len();
        body[offset_word..offset_word + 4].copy_from_slice(&((at + 0x10) as u32).to_le_bytes());
        body.extend_from_slice(stream);
        body
    }

    // The Home Point crystal `bnd0`: sec2 0x0B (0, -0.0157, 0) with a sec3 0x05 updater turns;
    // the same rate with no updater does not (effects.md "What MOVES an effect").
    #[test]
    fn rotation_velocity_spins_only_with_the_sec3_updater() {
        const BND0_YAW_PER_FRAME: f32 = -0.015_707_5;
        let mut sec2 = setup_with_link(LINKED_DATA_STATIC_MESH);
        sec2.extend(op(0x0B, 4, &vec3_payload([0.0, BND0_YAW_PER_FRAME, 0.0])));
        let rate_only = ParticleGeneratorDef::parse(&build(&sec2, 120, 0x1400))
            .unwrap()
            .unwrap();
        assert_eq!(
            rate_only.rotation_velocity,
            Some([0.0, BND0_YAW_PER_FRAME, 0.0])
        );
        assert!(!rate_only.rotation_updater);
        assert_eq!(rate_only.spin(), None);

        let body = with_section(build(&sec2, 120, 0x1400), 0x78, &op(0x05, 1, &[]));
        let spinning = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(spinning.rotation_updater);
        assert_eq!(spinning.spin(), Some([0.0, BND0_YAW_PER_FRAME, 0.0]));

        let updater_only = ParticleGeneratorDef::parse(&with_section(
            build(&setup_with_link(LINKED_DATA_STATIC_MESH), 120, 0x1400),
            0x78,
            &op(0x05, 1, &[]),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(
            updater_only.spin(),
            None,
            "an updater with no rate has nothing to add"
        );
    }

    // CYyGenerator.cpp CYyGenerator::ElemDie case 5 — the section-4 relife opcode every idle Home
    // Point layer carries.
    #[test]
    fn section_4_relife_opcode_is_read() {
        let plain = ParticleGeneratorDef::parse(&build(
            &setup_with_link(LINKED_DATA_STATIC_MESH),
            120,
            0x1400,
        ))
        .unwrap()
        .unwrap();
        assert!(!plain.relife_on_expiry);

        let body = with_section(
            build(&setup_with_link(LINKED_DATA_STATIC_MESH), 120, 0x1400),
            0x7C,
            &op(0x05, 1, &[]),
        );
        let relife = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(relife.relife_on_expiry);

        let other = with_section(
            build(&setup_with_link(LINKED_DATA_STATIC_MESH), 120, 0x1400),
            0x7C,
            &op(0x04, 1, &[]),
        );
        assert!(
            !ParticleGeneratorDef::parse(&other)
                .unwrap()
                .unwrap()
                .relife_on_expiry
        );
    }

    // research/xim ParticleExpirationHandlers.kt EmitChildHandler — section-4 0x01 carries
    // [expectZero32, child generator DAT id]; retail's ElemDie has no case for it and the engine
    // has no child-particle path, so the block is decode-only.
    #[test]
    fn section_4_emit_child_opcode_is_read() {
        let body = with_section(
            build(&setup_with_link(LINKED_DATA_STATIC_MESH), 120, 0x1400),
            0x7C,
            &op(
                SEC4_OPCODE_EMIT_CHILD,
                3,
                &[0, 0, 0, 0, b'c', b'h', b'i', b'1'],
            ),
        );
        let mut outcomes: Vec<(GeneratorSection, u8, GeneratorOpcodeOutcome)> = Vec::new();
        let def = ParticleGeneratorDef::parse_reporting(&body, &mut |s, op, o| {
            outcomes.push((s, op, o));
        })
        .unwrap()
        .unwrap();
        assert!(!def.relife_on_expiry);
        assert!(outcomes.iter().any(|(s, o, r)| {
            *s == GeneratorSection::ElementDie
                && *o == SEC4_OPCODE_EMIT_CHILD
                && *r == GeneratorOpcodeOutcome::Decoded
        }));
    }

    /// `bnd0`'s StandardSetup carries the specular selector in the high u16 of its setup dword.
    #[test]
    fn specular_selector_and_params_are_read() {
        let mut setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        setup[4 + 2..4 + 4].copy_from_slice(&0x0101u16.to_le_bytes());
        let mut payload = vec3_payload([0.0349, -0.5410, 0.6108]);
        payload.extend_from_slice(b"nami");
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&10.0f32.to_le_bytes());
        payload.extend_from_slice(&30.0f32.to_le_bytes());
        payload.extend_from_slice(&[0xAA, 0xAA, 0x8C, 0x80]);
        payload.extend_from_slice(&3u32.to_le_bytes());
        setup.extend(op(0x55, 10, &payload));
        let def = ParticleGeneratorDef::parse(&build(&setup, 120, 0x1400))
            .unwrap()
            .unwrap();
        assert!(def.specular_element);
        let spec = def.specular.expect("0x55 record");
        assert_eq!(spec.texture, Some(*b"nami"));
        assert_eq!(spec.vector, [0.0349, -0.5410, 0.6108]);
        assert_eq!((spec.unknown_a, spec.unknown_b), (10.0, 30.0));
        assert_eq!(spec.color_bgra, [0xAA, 0xAA, 0x8C, 0x80]);
        assert_eq!(spec.flags, 3);

        let common = ParticleGeneratorDef::parse(&build(
            &setup_with_link(LINKED_DATA_STATIC_MESH),
            120,
            0x1400,
        ))
        .unwrap()
        .unwrap();
        assert!(!common.specular_element);
        assert_eq!(common.specular, None);
    }

    // CMoD3m.cpp CMoD3m::Draw keys the TEXTUREFACTOR-alpha promotion on the exact blend byte, which
    // the ParticleBlend collapse (0x03/0x44/0x64 all -> Blend) cannot express.
    #[test]
    fn blend_byte_survives_the_blend_func_collapse() {
        let parsed = |p0: u8| {
            let mut setup = setup_with_link(LINKED_DATA_STATIC_MESH);
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
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert_eq!(def.mesh_kind, ParticleMeshKind::StaticMesh);
    }

    #[test]
    fn singleton_when_max_life_zero() {
        let mut setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        setup[4 + 8..4 + 12].copy_from_slice(b"sea0");
        // max life left at 0
        let body = build(&setup, 1, 1);
        let def = ParticleGeneratorDef::parse(&body).unwrap().unwrap();
        assert!(def.is_singleton());
    }

    // Pins the XIM attachFlags bit layout (ParticleGeneratorParser.kt) against the
    // ground-truth word 0x5402 read out of Poison's effect DAT (file 3020).
    #[test]
    fn attach_flags_split_type_and_joints() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);

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
        let Some(root) = crate::archive::open_test_install() else {
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
        let root = crate::archive::open_test_install()?;
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
        let mut setup = setup_with_link(LINKED_DATA_SOUND);
        setup[4 + 8..4 + 12].copy_from_slice(b"2024");
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
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
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

    // 0x11's config word is followPosition(0x1), followFacing(0x2), factor(>>2) - research/xim
    // ParticleGeneratorUpdaters.kt AssociationUpdater read.
    #[test]
    fn association_follow_reads_the_flags_and_factor_from_section_1() {
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
        let parse_cfg = |cfg: u32| {
            let mut sec1 = op(0x04, 2, &[0, 0, 0, 0]);
            sec1.extend(op(SEC1_OPCODE_ASSOCIATION, 2, &cfg.to_le_bytes()));
            sec1.extend(op(OPCODE_END, 0, &[]));
            let body = with_sec1(build(&setup, 1, GEN_FLAG_AUTO_RUN | 1), &sec1);
            ParticleGeneratorDef::parse(&body).unwrap().unwrap()
        };
        assert_eq!(
            parse_cfg(0x0003fd).association,
            Some(AssociationFollow {
                follow_position: true,
                follow_facing: false,
                factor: 0x3fd >> 2,
            })
        );
        assert_eq!(
            parse_cfg(0x0003ff).association,
            Some(AssociationFollow {
                follow_position: true,
                follow_facing: true,
                factor: 0x3ff >> 2,
            })
        );

        let plain = ParticleGeneratorDef::parse(&build(&setup, 1, GEN_FLAG_AUTO_RUN | 1))
            .unwrap()
            .unwrap();
        assert_eq!(plain.association, None, "no section 1: no follow");
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
        let setup = setup_with_link(LINKED_DATA_STATIC_MESH);
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
        let Some(root) = crate::archive::open_test_install() else {
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
