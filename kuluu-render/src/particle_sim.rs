use std::sync::OnceLock;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use ffxi_dat::particle_gen::{
    KeyFrameTrack, ParticleBillboard, ParticleGeneratorDef, ParticleMeshKind,
};
use ffxi_dat::sprite_sheet::ParticleSpriteSheet;

use crate::camera::OperatorCamera;
use crate::components::InGameEntity;
use crate::dat_d3m::{decoded_sky_texture_to_image, decoded_texture_to_image};
use crate::env_flags::env_flag;
use crate::ffxi_actor_render::FfxiRenderActor;
use crate::ffxi_particle_material::FfxiParticleMaterial;
use crate::scheduler_runtime::{
    assets_holding, ActionAssets, GlobalEffectDir, MmbSpriteMesh, SchedulerStageEvent, ROUTINE_FPS,
};
use ffxi_dat::scheduler::{StageKind, NO_LOCAL_DIR};

// CPU particle simulation. research/xim ParticleGenerator + Particle: a Particle stage (0x02)
// spawns a `LiveGenerator` that streams billboard particles over its window, each integrating
// velocity and following per-particle keyframe tracks (scale/alpha) by life progress. One retained
// mesh entity per generator is rebuilt each frame from its live particles — not an entity per
// particle.
#[derive(Resource, Default)]
pub struct ParticleSimulator {
    generators: Vec<LiveGenerator>,
    clock: CelestialClock,
}

// The Vana'diel clock inputs the celestial particle opcodes read. research/xim
// ParticleUpdaters.kt: ClockValueUpdater samples its keyframe curve at
// EnvironmentManager.getFullDayInterpolation() (the fraction of the Vana'diel day, NOT the
// particle's life progress); DayOfWeekColorUpdater / MoonPhaseColorUpdater /
// MoonPhaseSpriteSheetUpdater index their tables by the elemental weekday and moon phase.
#[derive(Clone, Copy, Debug, Default)]
pub struct CelestialClock {
    pub day_fraction: f32,
    pub day_of_week: usize,
    pub moon_phase: usize,
}

impl ParticleSimulator {
    pub fn drain_entities(&mut self) -> Vec<Entity> {
        self.generators.drain(..).map(|g| g.entity).collect()
    }

    pub fn set_celestial_clock(&mut self, clock: CelestialClock) {
        self.clock = clock;
    }

    // research/xi-model-viewer/ui/js/particle/runtime.js updateAssociatedPosition:
    // cameraAttachedBasePosition adds the base in fixed world axes, while followCamera anchors at
    // the camera itself. Both refresh every frame, but a cameraAttachedBasePosition particle reads
    // the result once — see `Particle::spawn_origin`.
    pub fn set_camera_relative_origins(&mut self, cam_pos: Vec3) {
        for g in &mut self.generators {
            if !g.camera_relative {
                continue;
            }
            let bp = g.def.base_position;
            g.origin = if g.def.camera_attached_base {
                cam_pos + Vec3::new(-bp[0], bp[1], -bp[2])
            } else {
                cam_pos + Vec3::from_array(bp) * g.vel_basis
            };
        }
    }

    // research/xim ParticleGeneratorAttachment / research/xi-model-viewer/ui/js/particle/runtime.js updateAssociatedPosition —
    // a Sun/Moon-attached generator's associated position is the celestial body's position
    // offset by the camera, refreshed every frame so the sky rides with the viewer.
    pub fn set_celestial_origins(&mut self, sun: Vec3, moon: Vec3) {
        use ffxi_dat::particle_gen::AttachType;
        for g in &mut self.generators {
            g.origin = match g.def.attach_type {
                AttachType::Sun => sun,
                AttachType::Moon => moon,
                _ => continue,
            };
        }
    }

    // research/xim EffectRoutineParser.kt parseSection2 StopParticleGeneratorRoutine — emission ceases
    // but the already-live particles play out their lifetime.
    pub fn stop_generator(&mut self, owner: Entity, gen_id: [u8; 4]) {
        self.stop_where(|o| o.owner == owner && o.gen_id == gen_id);
    }

    // research/xim EffectRoutineInstance.kt handleParticleEffectDampen — 0x1E ParticleDampen:
    // unlike StopParticle the already-live particles are force-expired at once (their audio
    // fades out there; this engine's particle generators carry no audio).
    pub fn dampen_generator(&mut self, owner: Entity, gen_id: [u8; 4]) {
        for g in &mut self.generators {
            if g.origin_routine
                .is_some_and(|o| o.owner == owner && o.gen_id == gen_id)
            {
                g.stopped = true;
                g.particles.clear();
            }
        }
    }

    pub fn stop_routine(&mut self, owner: Entity, routine: [u8; 4]) {
        self.stop_where(|o| o.owner == owner && o.routine == routine);
    }

    // A caster that despawns mid-cast (zone-out, death, out of range) never ends its cast pose,
    // so the aura's authored emit window would keep emitting at its last position without this.
    pub fn stop_generators_of_dead_owners(&mut self, alive: impl Fn(Entity) -> bool) {
        self.stop_where(|o| !alive(o.owner));
    }

    fn stop_where(&mut self, pred: impl Fn(&RoutineOrigin) -> bool) {
        for g in &mut self.generators {
            if g.origin_routine.is_some_and(|o| pred(&o)) {
                g.stopped = true;
            }
        }
    }
}

// Routine-spawned generators are addressable so a later StopParticle stage (or an interrupted
// cast) can end them: `owner` is the tracked entity the routine ran on, `gen_id` the generator
// chunk id, `routine` the top-level routine the stage was flattened from.
#[derive(Clone, Copy)]
struct RoutineOrigin {
    owner: Entity,
    gen_id: [u8; 4],
    routine: [u8; 4],
}

#[derive(Clone)]
struct SpriteTemplate {
    positions: Vec<Vec3>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    // Stage 0's D argument, one entry per `positions` entry. A particle mesh authors its
    // silhouette and its tint in this gradient rather than in the texture — the home point's
    // `sil` curtain (ROM/3/25.DAT) runs white -> purple (0x433F7D) -> black up each strip, and
    // the black end is what makes an additive plume fade out instead of ending on a lit quad
    // edge. Taking one vertex's colour for the whole mesh flattens all of that.
    colors: Vec<Vec4>,
}

// research/XIClient/src/XIClient/source/Resource/Derived/CMoD3m.cpp ZeroOneTSS — the D3m texture-stage
// tables, with D = diffuse/vertex, T = texture, F = TEXTUREFACTOR (the generator's particle
// colour). NonZeroTwoTSS is the textured default: stage 0 is MODULATE2X(D,T) for both channels,
// stage 1 MODULATE2X(CURRENT,F) for rgb and MODULATE4X(CURRENT,F) for alpha — totals 4 and 8.
// NonZeroOneTSS (renderStateFlags 0x1000) replaces stage 0's alpha with SELECTARG1(D.a), halving
// the alpha total to 4. The MMB-mesh branch
// (research/XIClient/src/XIClient/source/Rendering/ZoneRenderer.cpp ZoneRenderer::DoD3mDraw DoD3mDraw) reaches
// the same per-stage ops, so every template kind goes through `d3m_stage_chain`.
const D3M_STAGE1_RGB_GAIN: f32 = 2.0;
const D3M_STAGE1_ALPHA_GAIN: f32 = 4.0;
// Stage 0's MODULATE2X is already folded into `SpriteTemplate::colors` by the /128 vertex-colour
// normalise (ffxi_dat::d3m::VERTEX_COLOR_DIVISOR). NonZeroOneTSS's SELECTARG1 does not double, so
// the ignore-texture-alpha table divides it back out.
const D3M_VERTEX_BAKED_GAIN: f32 = 2.0;
// ZoneRenderer.cpp ZoneRenderer::DoD3mDraw, the `Texture == nullptr` branch: stage 0 is
// MODULATE2X(CURRENT, TFACTOR) for rgb and MODULATE4X(CURRENT, TFACTOR) for alpha with stage 1
// disabled — totals 2 and 4, half the textured table's. The /128 normalise already supplies
// one doubling of each, so the CPU gains are the textured ones halved.
const D3M_UNTEXTURED_RGB_GAIN: f32 = D3M_STAGE1_RGB_GAIN / 2.0;
const D3M_UNTEXTURED_ALPHA_GAIN: f32 = D3M_STAGE1_ALPHA_GAIN / 2.0;
// D3D saturates every texture-stage result. Stage 0's texture argument is only available in the
// sampler, so the CPU keeps only the clamp it can evaluate exactly — stage 0's, which is exact
// wherever the vertex colour is at or below the /128 midpoint (D * T <= 1 then, so the clamp is
// a no-op either way). Stage 1's clamp lands in ffxi_particle.wgsl, after the texel multiply:
// applying it here instead threw away the 4x/8x MODULATE gains before the texel could use them,
// which is why the home point crystal (D3m alpha 4 * 1.0 * 1.0, saturated in retail at every
// `kori` texel) drew at bare texture alpha and let the ground show through.
const D3M_STAGE_CLAMP: f32 = 1.0;

// research/XIClient/src/XIClient/source/World/Generator/Effects/CMoD3mElem.cpp CMoD3mElem::OnDraw — `OnDraw`
// sends the element through `DoMMBDraw` when its link is an MMB and `CMoD3m::Draw` otherwise. The
// two paths share the stage tables but not the blend bytes they honour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum D3mDrawPath {
    D3m,
    Mmb,
    // ZoneRenderer.cpp ZoneRenderer::DoD3mDraw — a submesh whose texture pointer is null
    // takes the one-stage table instead of the textured two-stage one.
    MmbUntextured,
}

// CMoD3mElem.cpp CMoD3mElem::DoMMBDraw — DoMMBDraw forces the ignore-texture-alpha table at this blend byte,
// whatever the render-state bit says.
const D3M_MMB_FORCE_IGNORE_TEXTURE_ALPHA_BLEND_BYTE: u8 = 0x64;
// CMoD3m.cpp CMoD3m::Draw — at blend byte 0x44 a TEXTUREFACTOR alpha at or above 0x7F is promoted to
// 0xFF before the stage math. DoMMBDraw carries no such promotion.
const D3M_TFACTOR_PROMOTE_BLEND_BYTE: u8 = 0x44;
const D3M_TFACTOR_PROMOTE_MIN: f32 = 0x7F as f32 / u8::MAX as f32;
const D3M_TFACTOR_PROMOTED: f32 = 1.0;

fn ignores_texture_alpha(def: &ParticleGeneratorDef, path: D3mDrawPath) -> bool {
    def.ignore_texture_alpha
        || (path == D3mDrawPath::Mmb
            && def.blend_byte == D3M_MMB_FORCE_IGNORE_TEXTURE_ALPHA_BLEND_BYTE)
}

fn tfactor_alpha(def: &ParticleGeneratorDef, path: D3mDrawPath, alpha: f32) -> f32 {
    if path == D3mDrawPath::D3m
        && def.blend_byte == D3M_TFACTOR_PROMOTE_BLEND_BYTE
        && alpha >= D3M_TFACTOR_PROMOTE_MIN
    {
        D3M_TFACTOR_PROMOTED
    } else {
        alpha
    }
}

// Resolve the generator's 0x60..0x63 time-of-day colour curves against the DAT's keyframe
// chunks. Absent on everything but the celestial billboards.
fn resolve_tod_tracks(
    def: &ParticleGeneratorDef,
    assets: &ActionAssets,
) -> [Option<KeyFrameTrack>; ffxi_dat::particle_gen::TOD_COLOR_CHANNELS] {
    def.tod_color_tracks
        .map(|id| id.and_then(|i| assets.keyframes.get(&i).cloned()))
}

// research/xim Particle.kt getColor — the day-of-week / moon-phase tints are applied with
// Color.modulateInPlace(c, 2f), a 2x modulate.
const CELESTIAL_MODULATE: f32 = 2.0;
// Index of the alpha channel in the 0x60..0x63 time-of-day track array (0x63 -> 0x3F).
const TOD_ALPHA_CHANNEL: usize = 3;

fn d3m_stage_chain(
    vertex_rgb: Vec3,
    vertex_alpha: f32,
    f_rgb: Vec3,
    f_alpha: f32,
    ignore_texture_alpha: bool,
    path: D3mDrawPath,
) -> (Vec3, f32) {
    let clamp = Vec3::splat(D3M_STAGE_CLAMP);
    let stage0_rgb = vertex_rgb.min(clamp);
    if path == D3mDrawPath::MmbUntextured {
        return (
            stage0_rgb * f_rgb * D3M_UNTEXTURED_RGB_GAIN,
            vertex_alpha.min(D3M_STAGE_CLAMP) * f_alpha * D3M_UNTEXTURED_ALPHA_GAIN,
        );
    }
    let stage0_alpha = if ignore_texture_alpha {
        vertex_alpha / D3M_VERTEX_BAKED_GAIN
    } else {
        vertex_alpha.min(D3M_STAGE_CLAMP)
    };
    (
        stage0_rgb * f_rgb * D3M_STAGE1_RGB_GAIN,
        stage0_alpha * f_alpha * D3M_STAGE1_ALPHA_GAIN,
    )
}

#[derive(Clone)]
struct LiveGenerator {
    immediate_parent: Option<Entity>,
    def: ParticleGeneratorDef,
    template: SpriteTemplate,
    draw_path: D3mDrawPath,
    // SpriteSheet (0x0E) flipbook frames; empty for a StaticMesh (0x0B) generator. When
    // non-empty each particle picks a frame by life progress in rebuild_mesh (research/xim
    // ParticleUpdaters.kt SpriteSheetFrameUpdater).
    sprite_frames: Vec<SpriteTemplate>,
    scale_x: Option<KeyFrameTrack>,
    scale_y: Option<KeyFrameTrack>,
    alpha: Option<KeyFrameTrack>,
    // The 0x60..0x63 time-of-day RGBA curves, resolved against the DAT's keyframe chunks.
    // Sampled at the Vana'diel day fraction, so unlike `alpha` above they do not advance
    // with the particle's own life.
    tod_color: [Option<KeyFrameTrack>; ffxi_dat::particle_gen::TOD_COLOR_CHANNELS],
    origin: Vec3,
    particles: Vec<Particle>,
    emit_accum: f32,
    age_frames: f32,
    emit_window_frames: f32,
    mesh: Handle<Mesh>,
    entity: Entity,
    // research/xim ParticleGenerator.kt isDoneEmitting — auto-run generators never finish
    // emitting; they live until their mesh entity (a child of the actor root)
    // is despawned.
    auto_run: bool,
    // Fixed particle orientation (init_rotation); None = camera billboard.
    orientation: Option<Quat>,
    // `is_solid_mesh(template)`, resolved once at spawn: whether the linked mesh has extent on
    // all three axes and can therefore carry the aim-at-eye world orientation.
    solid_mesh: bool,
    // The mesh entity is a child of the actor root, so vertex positions are
    // built in the actor's FFXI-local frame instead of world space.
    actor_local: bool,
    // Accumulated UV-translate (def.uv_scroll integrated over life) added to every
    // template UV so a scrolling water sheet/cascade slides its texture.
    tex_translate: Vec2,
    // Per-axis sign applied to init_velocity/accel. Actor-local generators integrate
    // in the DAT frame (ONE); world-space zone generators build positions directly in
    // Bevy space, so velocity gets the same mzb->bevy basis (x,-y,-z) as the origin.
    vel_basis: Vec3,
    origin_routine: Option<RoutineOrigin>,
    stopped: bool,
    // `origin` is rewritten from the camera each frame rather than fixed at spawn.
    camera_relative: bool,
    // research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp CYyGenerator::Idle case 0x0A —
    // outside the generator's authored camera-distance band the frame's emission is skipped.
    // Decided by sync_particle_meshes (the system that sees the camera), read by the next tick.
    emit_culled: bool,
    // research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp CYyGenerator::Idle —
    // `GetSomeGeneratorScalar() * 0.3` scales the per-emission count whenever field_DE bit 0 is
    // set, which Open() arms for every generator under the `taew` (weat) container (:418-434).
    // See weather_particles::WEATHER_EMIT_SCALE for why it is applied to batched generators too.
    emit_scale: f32,
    emit_rng: u64,
    // CYyGenerator.cpp CYyGenerator::ElemGenerate case 0x1F — retail's stepped azimuth indexes
    // the generator's element counter (field_9C), advanced once per emitted element; it resets
    // with the generator (field_9C = 0 at resource load and routine start).
    elements_emitted: u32,
    /// The camera rotation at the last sync, for the 0x1F camera-oriented ring: the ring is
    /// authored in the camera's frame, so the offset is mapped camera->local before it lands
    /// in the generator's local space (retail multiplies the offset chain by the inverse of
    /// attach x view). One sync behind like `emit_culled`
    /// (research/xim ParticleGeneratorParser.kt SphericalPositionVarianceFull).
    cam_view: Quat,
    /// The mesh entity's world rotation — the actor root's for actor-local generators, whose
    /// local frame is the actor's FFXI frame; identity otherwise.
    actor_rot: Quat,
    // Key of the last BUILT mesh (spawn writes `empty_mesh`, hence `MeshKey::Empty`), so
    // quantization error is bounded by one quantum and never accumulates across skipped frames.
    built_key: MeshKey,
    /// `template_bound_radius`, resolved once at spawn: the unscaled radius of the widest
    /// template/flipbook frame, which `generator_bounds` scales per particle.
    bound_radius: f32,
}

// The count scale for a generator outside retail's `taew` container: its authored count, as is.
const UNSCALED_EMISSION: f32 = 1.0;

// Spawn-time knobs a zone/weather caller sets that the generator body cannot carry: retail derives
// each of them from where the chunk sits in the DAT tree, not from its own fields.
// element_sort.rs keeps equal sort keys in DAT chunk order; an effect DAT's routines carry no
// such order, so their elements pass this and tie on the key alone.
pub const NO_DAT_ORDER: usize = 0;

#[derive(Clone, Copy)]
pub struct ZoneGeneratorOptions {
    pub camera_relative: bool,
    // The generator chunk's byte offset in the zone DAT (element_sort.rs tie-break).
    pub dat_offset: usize,
    pub emit_scale: f32,
    /// Set only by the weat/<type>/ celestial set, whose sheets carry FFXI's stored 4-bit alpha
    /// dither at a fixed on-screen size: see `dat_d3m.rs` decoded_sky_texture_to_image.
    pub resolve_alpha_dither: bool,
}

impl Default for ZoneGeneratorOptions {
    fn default() -> Self {
        Self {
            camera_relative: false,
            dat_offset: NO_DAT_ORDER,
            emit_scale: UNSCALED_EMISSION,
            resolve_alpha_dither: false,
        }
    }
}

// Auto-run particle generators embedded in an actor DAT (research/xim
// Actor.kt startAutoRunParticles), attached at actor spawn by
// ffxi_actor_render and started by `spawn_actor_auto_run_particles`.
#[derive(Component)]
pub struct ActorAutoRunEffects {
    pub assets: std::sync::Arc<ActionAssets>,
}

#[derive(Clone)]
struct Particle {
    pos: Vec3,
    // research/xim Particle.kt updateAssociatedPosition — cameraAttachedBasePosition resolves the offset from the
    // camera only while `age == 0`, so the particle is placed in front of the viewer once and
    // then lives in world space. Carrying the live generator origin instead glues the whole
    // emission to the camera as one rigid sheet that swings out of view on a pitch.
    spawn_origin: Vec3,
    vel: Vec3,
    age_frames: f32,
    life_frames: f32,
    rgb: Vec3,
    scale: Vec2,
    // The spawn-time scale: the scale keyframe track seeds its opening segment from the
    // particle's initial value, captured once (research/xim ParticleUpdaters.kt
    // ProgressValueUpdater — initialValueOverride is set only while null), so it must not
    // drift with the 0x12 growth.
    scale_seed: Vec2,
    // The 0x12 scale rate (x/y); zero while the generator's sec3 0x08 scale updater is off
    // (research/xim ParticleUpdaters.kt ScaleUpdater is the only integrator).
    scale_vel: Vec2,
    // Euler radians, seeded from the generator's 0x09 rotation and turned by its spin
    // (CYyGenerator.cpp CYyGenerator::ElemIdle case 0x05 integrates the 0x0B rate per frame).
    rotation: Vec3,
    // The 0x0B spin rate plus this particle's 0x0C variance draw; zero when the generator's
    // rotation updater is off (research/xim ParticleUpdaters.kt RotationUpdater — the
    // rotation transform's velocity, which holds the 0x0B + 0x0C sum, is what it integrates).
    spin: Vec3,
    // Armed by the sec2 0x3B block: the orientation step flips the rotation y
    // (research/xim Particle.kt — negateRotationY multiplies rotation.y by −1 in the
    // particle transform, set by IncrementalRotationApplier even for an all-zero payload).
    negate_rotation_y: bool,
    // The 0x08/0x41 relative-velocity portion in the integration frame, negated by 0x67: the
    // oscillation applier's direction source (research/xim ParticleUpdaters.kt
    // getOscillationDirection reads ParticleTransform.relativeVelocity, not the total
    // velocity; Particle.kt getTotalVelocity keeps the two transforms separate).
    rel_vel: Vec3,
    // Some while the generator carries the sec2 0x3D marker (research/xim
    // ParticleGeneratorSettings.kt OscillationParams): the per-axis acceleration (0x3E/0x3F/
    // 0x40 base plus one variance draw) and the applier's last-amplitude memory.
    osc: Option<Oscillation>,
}

// research/xim ParticleGeneratorSettings.kt OscillationParams — the per-particle oscillation
// state the sec3 appliers integrate: per-axis acceleration and the applier's previous-amplitude
// memory. [0]/[1]/[2] are the X/Y/Z axes.
#[derive(Clone)]
struct Oscillation {
    accel: [f32; 3],
    prev_amplitude: [f32; 3],
}

// research/xim ParticleGeneratorAttachment.kt resolveExtendedJoints — a source joint naming
// one of a mount's two footstep points is rewritten to reference 0 before it is ever resolved.
const MOUNT_FOOTSTEP_JOINTS: std::ops::RangeInclusive<u8> = 52..=53;
const MOUNT_FOOTSTEP_REFERENCE: usize = 0;

// research/xim ParticleGeneratorAttachment.kt updateAssociatedPosition jointRefIdx,103,111,125 updateAssociatedPosition — an
// actor-attached generator emits from the attach actor's position PLUS the position of the joint
// reference the def names: attachedJoint0 for the source-side attach types, attachedJoint1 for the
// target-side ones. The celestial and unattached types read neither. The field indexes the
// skeleton's reference table (ffxi_dat::skel::JointReference), not its joint array.
//
// SourceActorWeapon reads neither here: resolveExtendedJoints (:284-303) rewrites its source joint
// onto the PC hand/weapon references (31/33/35/55 -> 127, 32/34/54 -> 126, 36/37/56..60 -> 100..106)
// and returns without ever running the nearest-joint selector, but ONLY when the actor carries a PC
// model -- and FfxiRenderActor carries no PC-model flag to branch on. Resolving the raw field would
// place a PC weapon trail on whatever else that reference happens to be filed as, so weapon
// attachments keep the plain root origin until that flag exists.
fn attach_joint_reference(def: &ParticleGeneratorDef) -> Option<usize> {
    use ffxi_dat::particle_gen::AttachType;
    let source = if MOUNT_FOOTSTEP_JOINTS.contains(&def.attach_joint_source) {
        MOUNT_FOOTSTEP_REFERENCE
    } else {
        def.attach_joint_source as usize
    };
    match def.attach_type {
        AttachType::SourceActor
        | AttachType::SourceActorTargetFacing
        | AttachType::SourceToTargetBasis
        | AttachType::ZoneActorA
        | AttachType::ZoneActorB
        | AttachType::ZoneActorC => Some(source),
        AttachType::TargetActor
        | AttachType::TargetActorSourceFacing
        | AttachType::TargetToSourceBasis => Some(def.attach_joint_target as usize),
        AttachType::SourceActorWeapon | AttachType::None | AttachType::Sun | AttachType::Moon => {
            None
        }
    }
}

/// The pose an attach actor was last drawn in, plus the transform carrying its pose frame (FFXI
/// axes, -Y up) into Bevy world space.
struct AttachPose<'a> {
    pose: &'a [Mat4],
    skeleton: &'a ffxi_dat::skel::Skeleton,
    root: bevy::math::Affine3A,
}

/// The entity a routine runs on and the actor root holding the posed skeleton are not the same
/// entity on the live path — ffxi_actor_render.rs spawn_live_actor parents the root under the
/// wire entity — while the offline harnesses run the routine on the root itself. Doors and any
/// actor whose model has not loaded have no pose at all.
///
/// `Transform`, not `GlobalTransform`, for the same reason spawn_particle_generators reads it for
/// the origin (see scheduler_runtime.rs dispatch_sound_stages): the wire entity is a world root
/// and a frame-0 stage fires on the frame spawn_live_actor inserts the actor root, before
/// PostUpdate has propagated anything — a `GlobalTransform` read there is Ok-but-identity, which
/// would strip the FFXI->Bevy basis off the pose-frame offset and bury the effect under the
/// actor's feet, mirrored. The two local transforms are composed instead, so the basis comes
/// from the root the pose is in.
fn attach_pose<'a>(
    entity: Entity,
    q_children: &Query<&Children>,
    q_xf: &Query<&Transform>,
    q_render: &'a Query<&FfxiRenderActor>,
) -> Option<AttachPose<'a>> {
    let (actor, holder) = q_render
        .get(entity)
        .ok()
        .map(|actor| (actor, entity))
        .or_else(|| {
            q_children
                .get(entity)
                .ok()?
                .iter()
                .find_map(|child| Some((q_render.get(child).ok()?, child)))
        })?;
    let mut root = q_xf.get(entity).ok()?.compute_affine();
    if holder != entity {
        root *= q_xf.get(holder).ok()?.compute_affine();
    }
    Some(AttachPose {
        pose: actor.world_pose(),
        skeleton: &actor.skeleton,
        root,
    })
}

// World-space delta from the attach actor's root to the joint the generator hangs off.
// `other_world` is the other actor of the attachment, which is what a 49..51 nearest-joint
// selector measures against (research/xim ParticleGeneratorAttachment.kt resolveNearestJointSnapshot
// resolveNearestJointSnapshot).
fn attach_joint_offset(
    def: &ParticleGeneratorDef,
    attach: Option<AttachPose<'_>>,
    other_world: Option<Vec3>,
) -> Vec3 {
    let (Some(reference), Some(attach)) = (attach_joint_reference(def), attach) else {
        return Vec3::ZERO;
    };
    let toward = other_world.map(|w| attach.root.inverse().transform_point3(w));
    ffxi_actor::skeleton_instance::attach_joint_position(
        attach.pose,
        attach.skeleton,
        reference,
        toward,
    )
    .map(|local| attach.root.transform_vector3(local))
    .unwrap_or(Vec3::ZERO)
}

// The world origin a generator emits from: the attach actor's root plus the joint the def
// hangs off (research/xim ParticleGeneratorAttachment.kt updateAssociatedPosition). The spawn
// path computes it once; `track_attached_origins` recomputes it every frame for the defs that
// carry the 0x11 follow.
fn attached_origin(
    def: &ParticleGeneratorDef,
    owner: Entity,
    target: Option<Entity>,
    q_xf: &Query<&Transform>,
    q_children: &Query<&Children>,
    q_render: &Query<&FfxiRenderActor>,
) -> Option<Vec3> {
    let origin_entity =
        crate::scheduler_runtime::particle_origin_entity(def.attach_type, owner, target);
    let origin_xf = q_xf.get(origin_entity).ok()?;
    // research/xim SkeletonInstance.kt getStandardJointExtended has no source-vs-target
    // guard: it always walks the ring and keeps the reference nearest the other actor. On a
    // self-targeted action both sides ARE the same actor, and the winner is the ring point
    // nearest the actor's own origin — torso height, which is the whole point of this bead.
    // Only an attachment with no second actor at all falls back to the root.
    let other_world = if origin_entity == owner {
        target
    } else {
        Some(owner)
    }
    .and_then(|e| q_xf.get(e).ok())
    .map(|xf| xf.translation);
    let joint_offset = attach_joint_offset(
        def,
        attach_pose(origin_entity, q_children, q_xf, q_render),
        other_world,
    );
    Some(origin_xf.translation + joint_offset + Vec3::Y * def.base_position[1])
}

/// Spawns the live generator entities for the scheduler's particle stages.
///
/// A cast routine's generators ship in the global effect dir, not the caster's own
/// ActionAssets, and an actor-routine's (a model's `bind`) in the model DAT, so the def
/// resolves against whichever tier actually holds it.
///
/// Spawned mesh entities opt out of frustum culling: the mesh is rebuilt in place every
/// frame, but Bevy computes the frustum-culling Aabb once from the initially-empty mesh and
/// does not recompute it, so the entity would be culled permanently. `sync_particle_meshes`
/// owns the draw gate instead.
pub fn spawn_particle_generators(
    mut events: MessageReader<SchedulerStageEvent>,
    q_actors: Query<(&Transform, Option<&ActionAssets>)>,
    q_action_target: Query<&crate::scheduler_runtime::ActionTarget>,
    q_xf: Query<&Transform>,
    q_children: Query<&Children>,
    q_render: Query<&FfxiRenderActor>,
    global: Option<Res<GlobalEffectDir>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<FfxiParticleMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut sim: ResMut<ParticleSimulator>,
    mut commands: Commands,
) {
    for ev in events.read() {
        if ev.stage.stage.kind != StageKind::Particle {
            continue;
        }
        let Ok((actor_xf, local_assets)) = q_actors.get(ev.actor) else {
            continue;
        };
        let actor_assets = q_children
            .get(ev.actor)
            .ok()
            .and_then(|c| c.iter().find_map(|child| q_render.get(child).ok()))
            .map(|a| a.action_assets());
        let local_dir = ev.stage.stage.local_dir;
        let Some(assets) = assets_holding(
            local_assets,
            actor_assets,
            global.as_ref().map(|g| &g.assets),
            |a| a.particle_def(local_dir, &ev.stage.stage.id).is_some(),
        ) else {
            continue;
        };
        let Some((def_dir, def)) = assets
            .particle_def_scoped(local_dir, &ev.stage.stage.id)
            .map(|(dir, def)| (dir, *def))
        else {
            continue;
        };
        let Some((template, sprite_frames, tex)) =
            resolve_mesh(assets, def_dir, &def, &mut images, false)
        else {
            continue;
        };
        let target = q_action_target.get(ev.actor).ok().and_then(|t| t.0);
        let origin = attached_origin(&def, ev.actor, target, &q_xf, &q_children, &q_render)
            .unwrap_or(actor_xf.translation + Vec3::Y * def.base_position[1]);
        let mat = mats.add(FfxiParticleMaterial::for_def(&def, tex, NO_DAT_ORDER));
        let mesh = meshes.add(empty_mesh());

        let entity = commands
            .spawn((
                InGameEntity,
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat),
                Transform::IDENTITY,
                Visibility::default(),
                bevy::camera::visibility::NoFrustumCulling,
                bevy::light::NotShadowCaster,
                bevy::light::NotShadowReceiver,
            ))
            .id();

        debug!(
            "spawned particle generator {} mesh {} life {}",
            String::from_utf8_lossy(&ev.stage.stage.id),
            String::from_utf8_lossy(&def.mesh_id),
            def.max_life_frames
        );

        let resolve = |id: Option<[u8; 4]>| -> Option<KeyFrameTrack> {
            id.and_then(|i| assets.keyframes.get(&i).cloned())
        };

        let emit_window_frames = ev.stage.stage.duration_frames as f32;
        sim.generators.push(LiveGenerator {
            immediate_parent: None,
            scale_x: resolve(def.scale_x_track),
            scale_y: resolve(def.scale_y_track),
            alpha: resolve(def.alpha_track),
            tod_color: resolve_tod_tracks(&def, assets),
            solid_mesh: is_solid_mesh(&template),
            bound_radius: template_bound_radius(&template, &sprite_frames),
            template,
            draw_path: D3mDrawPath::D3m,
            sprite_frames,
            def,
            origin,
            particles: Vec::new(),
            emit_accum: 0.0,
            age_frames: 0.0,
            emit_window_frames,
            mesh,
            entity,
            auto_run: false,
            orientation: None,
            actor_local: false,
            tex_translate: Vec2::ZERO,
            vel_basis: crate::scene::mzb_to_bevy(kuluu_snapshot::Vec3 {
                x: Vec3::ONE.x,
                y: Vec3::ONE.y,
                z: Vec3::ONE.z,
            }),
            origin_routine: Some(RoutineOrigin {
                owner: ev.actor,
                gen_id: ev.stage.stage.id,
                routine: ev.scheduler,
            }),
            stopped: false,
            camera_relative: false,
            emit_culled: false,
            emit_scale: UNSCALED_EMISSION,
            emit_rng: emit_seed(entity),
            elements_emitted: 0,
            cam_view: Quat::IDENTITY,
            actor_rot: Quat::IDENTITY,
            built_key: MeshKey::Empty,
        });

        let mut chain = vec![(def_dir, ev.stage.stage.id)];
        let mut linked_dir = def_dir;
        while let Some(parent) = sim.generators.last() {
            let Some(id) = parent.def.immediate_generator else {
                break;
            };
            let Some((dir, linked_def)) = assets.particle_def_scoped(linked_dir, &id) else {
                break;
            };
            if chain.contains(&(dir, id)) {
                break;
            }
            chain.push((dir, id));
            linked_dir = dir;
            let linked_def = *linked_def;
            let Some((template, sprite_frames, texture)) =
                resolve_mesh(assets, dir, &linked_def, &mut images, false)
            else {
                break;
            };
            let mut linked = parent.clone();
            linked.immediate_parent = Some(parent.entity);
            linked.def = linked_def;
            linked.origin = parent.origin;
            linked.scale_x = resolve(linked_def.scale_x_track);
            linked.scale_y = resolve(linked_def.scale_y_track);
            linked.alpha = resolve(linked_def.alpha_track);
            linked.tod_color = resolve_tod_tracks(&linked_def, assets);
            linked.solid_mesh = is_solid_mesh(&template);
            linked.bound_radius = template_bound_radius(&template, &sprite_frames);
            linked.template = template;
            linked.sprite_frames = sprite_frames;
            linked.stopped = true;
            linked.mesh = meshes.add(empty_mesh());
            let material = mats.add(FfxiParticleMaterial::for_def(
                &linked_def,
                texture,
                NO_DAT_ORDER,
            ));
            linked.entity = commands
                .spawn((
                    InGameEntity,
                    Mesh3d(linked.mesh.clone()),
                    MeshMaterial3d(material),
                    Transform::IDENTITY,
                    Visibility::default(),
                    bevy::camera::visibility::NoFrustumCulling,
                    bevy::light::NotShadowCaster,
                    bevy::light::NotShadowReceiver,
                ))
                .id();
            linked.emit_rng = emit_seed(linked.entity);
            sim.generators.push(linked);
        }
    }
}

// research/xim Actor.kt createFrom — at model-ready, every generator in the
// actor DAT flagged auto-run starts immediately and emits forever. The mesh
// entity is a child of the actor root (which carries the FFXI->Bevy basis), so
// particle math stays in the DAT's own FFXI-local frame and the effect follows
// and despawns with the actor.
pub fn spawn_actor_auto_run_particles(
    q_added: Query<(Entity, &ActorAutoRunEffects), Added<ActorAutoRunEffects>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<FfxiParticleMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut sim: ResMut<ParticleSimulator>,
    mut commands: Commands,
) {
    for (actor_root, fx) in &q_added {
        for (name, def) in fx.assets.particle_defs.iter() {
            if !def.auto_run {
                continue;
            }
            let def = *def;
            let def_dir = fx
                .assets
                .particle_def_dirs
                .get(name)
                .copied()
                .unwrap_or(NO_LOCAL_DIR);
            let Some((template, sprite_frames, tex)) =
                resolve_mesh(&fx.assets, def_dir, &def, &mut images, false)
            else {
                continue;
            };
            let mat = mats.add(FfxiParticleMaterial::for_def(&def, tex, NO_DAT_ORDER));
            let mesh = meshes.add(empty_mesh());

            let entity = commands
                .spawn((
                    InGameEntity,
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(mat),
                    Transform::IDENTITY,
                    ChildOf(actor_root),
                    bevy::camera::visibility::NoFrustumCulling,
                    bevy::light::NotShadowCaster,
                    bevy::light::NotShadowReceiver,
                ))
                .id();

            debug!(
                "auto-run particle generator {} mesh {} blend {:?}",
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(&def.mesh_id),
                def.blend,
            );

            let resolve = |id: Option<[u8; 4]>| -> Option<KeyFrameTrack> {
                id.and_then(|i| fx.assets.keyframes.get(&i).cloned())
            };
            sim.generators.push(LiveGenerator {
                immediate_parent: None,
                scale_x: resolve(def.scale_x_track),
                scale_y: resolve(def.scale_y_track),
                alpha: resolve(def.alpha_track),
                tod_color: resolve_tod_tracks(&def, &fx.assets),
                solid_mesh: is_solid_mesh(&template),
                bound_radius: template_bound_radius(&template, &sprite_frames),
                template,
                draw_path: D3mDrawPath::D3m,
                sprite_frames,
                origin: Vec3::from_array(def.base_position),
                particles: Vec::new(),
                emit_accum: 0.0,
                age_frames: 0.0,
                emit_window_frames: 0.0,
                mesh,
                entity,
                auto_run: true,
                orientation: particle_orientation(&def),
                actor_local: true,
                tex_translate: Vec2::ZERO,
                vel_basis: Vec3::ONE,
                origin_routine: None,
                stopped: false,
                camera_relative: false,
                emit_culled: false,
                emit_scale: UNSCALED_EMISSION,
                emit_rng: emit_seed(entity),
                elements_emitted: 0,
                cam_view: Quat::IDENTITY,
                actor_rot: Quat::IDENTITY,
                built_key: MeshKey::Empty,
                def,
            });
        }
    }
}

// research/xim EnvironmentManager zone-static Generator: an auto-run particle
// generator embedded in the zone MZB DAT (Bastok Mines pump spray), placed in
// world space rather than parented to an actor. `origin` is already mzb->bevy;
// velocity/accel take the same basis so the spray arcs in Bevy space.
//
// `emit_culled` starts true when the def carries a camera-distance band
// (research/xim EnvironmentManager zone-static Generator), so a zone-in does not
// burst every out-of-band emitter once; the first sync settles the in-band ones
// a frame later.
pub fn spawn_zone_particle_generator(
    def: ParticleGeneratorDef,
    assets: &ActionAssets,
    global: Option<&ActionAssets>,
    origin: Vec3,
    opts: ZoneGeneratorOptions,
    meshes: &mut Assets<Mesh>,
    mats: &mut Assets<FfxiParticleMaterial>,
    images: &mut Assets<Image>,
    sim: &mut ParticleSimulator,
    commands: &mut Commands,
) -> Option<Entity> {
    let undither = opts.resolve_alpha_dither;
    let (template, sprite_frames, tex, draw_path) =
        resolve_zone_mesh(assets, &def, images, undither)
            .or_else(|| global.and_then(|g| resolve_zone_mesh(g, &def, images, undither)))?;
    let mat = mats.add(FfxiParticleMaterial::for_def(&def, tex, opts.dat_offset));
    let mesh = meshes.add(empty_mesh());

    let entity = commands
        .spawn((
            InGameEntity,
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mat),
            Transform::IDENTITY,
            Visibility::default(),
            bevy::camera::visibility::NoFrustumCulling,
            bevy::light::NotShadowCaster,
            bevy::light::NotShadowReceiver,
        ))
        .id();

    let resolve = |id: Option<[u8; 4]>| keyframe(assets, global, id);
    sim.generators.push(LiveGenerator {
        immediate_parent: None,
        scale_x: resolve(def.scale_x_track),
        scale_y: resolve(def.scale_y_track),
        alpha: resolve(def.alpha_track),
        tod_color: def.tod_color_tracks.map(|id| keyframe(assets, global, id)),
        solid_mesh: is_solid_mesh(&template),
        bound_radius: template_bound_radius(&template, &sprite_frames),
        template,
        draw_path,
        sprite_frames,
        origin,
        particles: Vec::new(),
        emit_accum: 0.0,
        age_frames: 0.0,
        emit_window_frames: 0.0,
        mesh,
        entity,
        auto_run: true,
        orientation: particle_orientation(&def),
        actor_local: false,
        tex_translate: Vec2::ZERO,
        vel_basis: Vec3::new(1.0, -1.0, -1.0),
        origin_routine: None,
        stopped: false,
        camera_relative: opts.camera_relative,
        emit_culled: def.emit_cull.is_some(),
        emit_scale: opts.emit_scale,
        emit_rng: emit_seed(entity),
        elements_emitted: 0,
        cam_view: Quat::IDENTITY,
        actor_rot: Quat::IDENTITY,
        built_key: MeshKey::Empty,
        def,
    });
    Some(entity)
}

// research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp HandleOne — a batched
// (CheckFlag29) D3a generator is the one element retail's reimplementation leaves as
// SPDLOG_ERROR("0x11"), so what a batched sprite sheet actually draws is not transcribable. Its
// sub-particles are camera-billboarded here: the precipitation curtains are what use the
// combination (154 of the 155 in the shipped zone DATs sit under weat/), and a rain sheet pinned
// to a world axis vanishes whenever the camera looks along its normal.
fn particle_orientation(def: &ParticleGeneratorDef) -> Option<Quat> {
    let batched_sheet = def.batched && def.mesh_kind == ParticleMeshKind::SpriteSheet;
    if def.camera_billboard || batched_sheet {
        return None;
    }
    let r = def.init_rotation;
    Some(Quat::from_euler(EulerRot::XYZ, r[0], r[1], r[2]))
}

// The live orientation of a fixed-orientation particle: its 0x09 seed plus whatever the spin
// has added, in the same Euler order `particle_orientation` seeds with.
fn particle_rotation(p: &Particle) -> Quat {
    let y = if p.negate_rotation_y {
        -p.rotation.y
    } else {
        p.rotation.y
    };
    Quat::from_euler(EulerRot::XYZ, p.rotation.x, y, p.rotation.z)
}

// Distinct per generator so two emitters sharing a def do not spawn identical particle clouds;
// deterministic so a rebuilt zone/weather set replays the same spread.
fn emit_seed(entity: Entity) -> u64 {
    let seed = crate::scheduler_runtime::SPLITMIX64_GOLDEN_RATIO;
    seed ^ entity.to_bits().wrapping_mul(seed)
}

fn next_unit(state: &mut u64) -> f32 {
    *state = crate::scheduler_runtime::lcg_next(*state);
    ((*state >> 40) as f32) / ((1u64 << 24) as f32)
}

pub fn stop_generators_for_despawned_owners(
    q_alive: Query<()>,
    mut sim: ResMut<ParticleSimulator>,
) {
    sim.stop_generators_of_dead_owners(|e| q_alive.get(e).is_ok());
}

// 0x11 AssociationUpdater: retail re-snaps the associated position to the attach actor's
// position plus joint every frame (research/xim ParticleGeneratorAttachment.kt
// updateAssociatedPosition - a hard copy; the follow-rate factor is parsed but unused
// there). The spawn path computes the origin once, so without this a cast aura or hit
// flash keeps emitting from where the actor stood when the stage fired.
//
// Only scheduled generators track: auto-run zone generators are parented to their
// actor root and ride along, and the camera/celestial origins have their own
// per-frame setters.
pub fn track_attached_origins(
    q_xf: Query<&Transform>,
    q_children: Query<&Children>,
    q_render: Query<&FfxiRenderActor>,
    q_action_target: Query<&crate::scheduler_runtime::ActionTarget>,
    mut sim: ResMut<ParticleSimulator>,
) {
    use ffxi_dat::particle_gen::AttachType;
    for g in &mut sim.generators {
        let Some(origin_routine) = g.origin_routine else {
            continue;
        };
        if !g
            .def
            .association
            .as_ref()
            .is_some_and(|a| a.follow_position)
        {
            continue;
        }
        // xim's AttachType.None branch updates nothing; Sun/Moon ride
        // `set_celestial_origins` instead.
        match g.def.attach_type {
            AttachType::None | AttachType::Sun | AttachType::Moon => continue,
            _ => {}
        }
        let target = q_action_target
            .get(origin_routine.owner)
            .ok()
            .and_then(|t| t.0);
        if let Some(origin) = attached_origin(
            &g.def,
            origin_routine.owner,
            target,
            &q_xf,
            &q_children,
            &q_render,
        ) {
            g.origin = origin;
        }
    }
}

pub fn tick_particle_simulator(time: Res<Time>, mut sim: ResMut<ParticleSimulator>) {
    let frames = time.delta_secs() * ROUTINE_FPS;
    advance_simulator(&mut sim, frames);
}

// .agents/skills/retail-observe/references/2026-10-02-level-up-linked-sparkle.md immediate emission and parent position addition.
fn advance_simulator(sim: &mut ParticleSimulator, frames: f32) {
    let parents: std::collections::HashSet<_> = sim
        .generators
        .iter()
        .filter_map(|g| g.immediate_parent)
        .collect();
    if parents.is_empty() {
        for g in &mut sim.generators {
            advance_generator(g, frames);
        }
        return;
    }
    let mut births = std::collections::HashMap::<Entity, Vec<Vec3>>::new();
    let mut pending = Vec::new();
    for (index, g) in sim.generators.iter_mut().enumerate() {
        let before = g.elements_emitted;
        advance_generator(g, frames);
        if g.immediate_parent.is_some() {
            pending.push(index);
        } else if parents.contains(&g.entity) {
            births.insert(
                g.entity,
                g.particles
                    .iter()
                    .skip(
                        g.particles
                            .len()
                            .saturating_sub((g.elements_emitted - before) as usize),
                    )
                    .map(|p| p.pos)
                    .collect(),
            );
        }
    }
    let entities: std::collections::HashSet<_> = sim.generators.iter().map(|g| g.entity).collect();
    while !pending.is_empty() {
        let before = pending.len();
        pending.retain(|index| {
            let g = &mut sim.generators[*index];
            let parent = g.immediate_parent.unwrap();
            if entities.contains(&parent) && !births.contains_key(&parent) {
                return true;
            }
            let mut emitted = Vec::new();
            if frames > 0.0 {
                for position in births.get(&parent).into_iter().flatten() {
                    for _ in 0..emission_count(g) {
                        emit(g, g.def.max_life_frames);
                        if let Some(particle) = g.particles.last_mut() {
                            if g.def.parent_position_copy {
                                particle.pos += *position;
                            }
                            emitted.push(particle.pos);
                        }
                    }
                }
            }
            births.insert(g.entity, emitted);
            false
        });
        if pending.len() == before {
            break;
        }
    }
}

fn advance_generator(g: &mut LiveGenerator, frames: f32) {
    g.age_frames += frames;

    // research/xim ParticleGenerator.kt emit — completed particles are swept
    // before emission, so a continuous singleton re-emits the same tick its
    // predecessor expires.
    reap_expired(g);

    // Particles emitted below were born during this tick, so the ageing pass must not charge them
    // the whole frame: at 30 fps retail that error is invisible, but one long frame (the blocking
    // action-DAT read) would otherwise age a freshly emitted short-life particle past its life and
    // sweep it before it ever renders.
    let pre_emit_len = g.particles.len();

    // research/xim: a maxLifeSpan of 0 marks a singleton — emit one particle once.
    let singleton = g.def.is_singleton();
    let emitting = !g.stopped
        && !g.emit_culled
        && (g.auto_run || g.age_frames <= g.emit_window_frames.max(1.0));
    if singleton {
        // `age_frames <= frames` already pins this to the first tick, so the emit window must not
        // gate it: a long frame (the blocking action-DAT read precedes these) makes age_frames
        // exceed a dur=0 stage's 1-frame window on that very tick and the singleton never fires.
        if !g.stopped && g.particles.is_empty() && g.age_frames <= frames {
            // research/xim ParticleInitializers.kt read — a maxLifeSpan of 0 is rewritten
            // to POSITIVE_INFINITY, "used for 'singleton' particles, like the sea and such":
            // the auto-run zone/weather billboards that stand as long as the zone does (the
            // sun, the moon, the sea). A 1-frame life made those vanish on the tick after
            // they spawned. A scheduled generator is NOT that population — its singleton
            // plays out the stage window and is reaped with the effect, so it keeps the
            // bounded life or a dur=0 cast aura would hang in the world forever.
            let bounded = g.emit_window_frames.max(g.def.max_life_frames);
            let life = if g.auto_run && bounded <= 0.0 {
                f32::INFINITY
            } else {
                bounded.max(1.0)
            };
            emit(g, life);
        }
    } else if !g.auto_run && g.emit_window_frames <= 0.0 {
        // .agents/skills/retail-observe/references/2026-10-01-level-up-zero-time-emission.md first-positive-update rule.
        if frames > 0.0 && !g.stopped && !g.emit_culled && g.age_frames <= frames {
            for _ in 0..emission_count(g) {
                emit(g, g.def.max_life_frames);
            }
        }
    } else if emitting {
        g.emit_accum += frames;
        while g.emit_accum >= g.def.frames_per_emission {
            // research/xim ParticleGenerator.kt emit — a continuous-singleton
            // generator holds one live particle and re-emits the moment it
            // expires (the accumulator stays primed, capped to one period).
            if g.def.continuous && !g.particles.is_empty() {
                g.emit_accum = g.def.frames_per_emission;
                break;
            }
            g.emit_accum -= g.def.frames_per_emission;
            for _ in 0..emission_count(g) {
                emit(g, g.def.max_life_frames);
                if g.def.continuous {
                    break;
                }
            }
        }
    }

    // research/xim ParticleUpdaters TextureCoordinateUpdater: scroll velocity is
    // per-generator (frames of life advance the shared UV offset), not per-particle.
    g.tex_translate += Vec2::from_array(g.def.uv_scroll) * frames;

    let accel = g
        .def
        .accel
        .map(|a| Vec3::from_array(a) * g.vel_basis * frames);
    let osc_applier_x = g.def.oscillation_applier_x;
    let osc_applier_y = g.def.oscillation_applier_y;
    let osc_applier_z = g.def.oscillation_applier_z;
    // CYyGenerator.cpp CYyGenerator::ElemIdle case 0x02 — retail position-steps the element
    // only while the generator's sec3 carries the 0x02 PositionUpdater, so a velocity without
    // the block never moves the particle (research/xim ParticleUpdaters.kt PositionUpdater is
    // the only integrator of the position transform's velocity).
    let position_updater = g.def.position_updater;
    for p in g.particles.iter_mut().take(pre_emit_len) {
        p.age_frames += frames;
        if let Some(a) = accel {
            p.vel += a;
        }
        if position_updater {
            p.pos += p.vel * frames;
        }
        // FFXiMain.dll retail-2026-09 VA 0x1004C6CB: authored damping raised to renderer delta.
        if let Some([damping, _]) = g.def.velocity_dampener {
            p.vel *= damping.powf(frames);
        }
        // sec3 0x29/0x2A/0x2B OscillationApplier (X/Y/Z): after the base position step, add
        // the amplitude change over the tick per active axis (research/xim
        // ParticleUpdaters.kt OscillationApplier — particle.position += direction × delta).
        for (axis, applier) in [(0, osc_applier_x), (1, osc_applier_y), (2, osc_applier_z)] {
            if let (Some(applier), Some(osc)) = (applier, p.osc.as_mut()) {
                p.pos += oscillation_delta(
                    applier,
                    axis,
                    osc,
                    p.rel_vel,
                    p.age_frames - frames,
                    p.age_frames,
                    g.actor_local,
                );
            }
        }
        p.scale += p.scale_vel * frames;
        p.rotation += p.spin * frames;
        if g.def.rotation_updater {
            if let Some(accel) = g.def.rotation_accel {
                p.spin += Vec3::from_array(accel) * frames;
            }
        }
        if g.def.scale_updater {
            if let Some(accel) = g.def.scale_accel {
                p.scale_vel += Vec2::new(accel[0], accel[1]) * frames;
            }
        }
    }
    reap_expired(g);

    // A continuous generator re-emits "the moment its particle expires"
    // (research/xim ParticleGenerator.kt emit). The aging above can push the lone
    // particle past its life within this same tick, after the pre-emit sweep
    // already ran — replace it now so the mesh is never empty at render and the
    // body does not blink out for a frame.
    if g.def.continuous && g.particles.is_empty() && !g.emit_culled && continuous_active(g) {
        emit(g, g.def.max_life_frames);
    }
}

fn continuous_active(g: &LiveGenerator) -> bool {
    !g.stopped && (g.auto_run || g.age_frames <= g.emit_window_frames.max(1.0))
}

// research/xim ParticleUpdaters.kt OscillationApplier — the position delta the applier adds
// to a particle: with oscillationRate = 180f / divisor (a zero divisor is an infinite rate,
// which xim skips), frequency = π × age / rate, and
// baseAmplitude = 0.5 × (sin(baseOffset + frequency − π/2) + cos(baseOffset)), the amplitude
// is 0.5 × accel × baseAmplitude × rate and the delta is its change since the previous
// frame. The engine ticks by a variable frame count, so the previous amplitude is evaluated
// at age − frames instead of read from memory (the two coincide while the applier runs
// every tick). The direction is the particle's relative-velocity direction (getOscillationDirection:
// X = forward, Y = forward × Ẑ, Z = forward × Ŷ, the FFXI-frame unit hats basised into the
// integration frame for world-space generators; vel_basis is an involution with det +1, so it
// commutes with the cross products), falling back to the unit axis when the relative velocity
// is near zero.
fn oscillation_delta(
    applier: [f32; 3],
    axis: usize,
    osc: &mut Oscillation,
    rel_vel: Vec3,
    prev_age_frames: f32,
    age_frames: f32,
    actor_local: bool,
) -> Vec3 {
    let divisor = applier[0];
    if divisor == 0.0 {
        return Vec3::ZERO;
    }
    let rate = 180.0 / divisor;
    let base_offset = applier[1];
    let amplitude = |age: f32| -> f32 {
        let frequency = std::f32::consts::PI * (age / rate);
        let base = 0.5
            * ((base_offset + frequency - std::f32::consts::FRAC_PI_2).sin() + base_offset.cos());
        0.5 * osc.accel[axis] * base * rate
    };
    let delta = amplitude(age_frames) - amplitude(prev_age_frames);
    osc.prev_amplitude[axis] = amplitude(age_frames);
    let (y_hat, z_hat) = if actor_local {
        (Vec3::Y, Vec3::Z)
    } else {
        (Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 0.0, -1.0))
    };
    let direction = if rel_vel.length() < 1e-7 {
        match axis {
            0 => Vec3::X,
            1 => y_hat,
            _ => z_hat,
        }
    } else {
        let forward = rel_vel.normalize();
        match axis {
            0 => forward,
            1 => forward.cross(z_hat).normalize_or_zero(),
            _ => forward.cross(y_hat).normalize_or_zero(),
        }
    };
    direction * delta
}

// research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp CYyGenerator::Idle counter — the emit loop
// runs `for counter in 0..=floor(v161)` over `v161 = (flags & 0x1FF) * scale`, i.e. floor + 1. That
// trailing +1 is deliberately not reproduced: it would raise every already-tuned non-weather
// population (10740 shipped generators author a non-zero count) by one particle, so the floor of 1
// below stands in for it and keeps an authored count of 0 emitting the single particle retail
// gives it.
fn emission_count(g: &LiveGenerator) -> u32 {
    ((g.def.particles_per_emission as f32 * g.emit_scale) as u32).max(1)
}

fn emit(g: &mut LiveGenerator, life_frames: f32) {
    // research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp CYyGenerator::ElemGenerate applies the sec2 0x06/0x07 spawn spread to the
    // elem, skipping it when CheckFlag29 is set because a batched elem carries its own
    // sub-particles. Our Particle models the sub-particle in that case, so the spread applies
    // either way — without it every drop of a rain curtain spawns on one point.

    // The 0x08 relative velocity lives in the generator's local space (the space the 0x02
    // base is scaled into by vel_basis), so its direction comes off the pre-basis offset
    // (research/xim ParticleInitializers.kt RelativeVelocitySetup).
    let mut pos_local = match g.def.position_variance {
        Some(v) => {
            let u = next_unit(&mut g.emit_rng);
            let yaw = (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * std::f32::consts::PI;
            let pitch = (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * std::f32::consts::PI;
            Vec3::from_array(v.offset(u, yaw, pitch))
        }
        None => Vec3::ZERO,
    };
    // 0x1F SphericalPositionVarianceFull: a spherical spawn spread whose azimuth is a random
    // draw or one of the generator's evenly spaced steps (CYyGenerator.cpp
    // CYyGenerator::ElemGenerate case 0x1F — the stepped azimuth indexes field_9C, the
    // element counter advanced once per emitted element). Added on top of the 0x06/0x07
    // spread, so the 0x08 relative direction sees the sum.
    if let Some(sp) = g.def.spherical_full {
        let u = next_unit(&mut g.emit_rng);
        let azimuth = if sp.azimuth_steps == 0 {
            (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * std::f32::consts::PI
        } else {
            let step = (g.elements_emitted % sp.azimuth_steps) as f32 / sp.azimuth_steps as f32;
            2.0 * std::f32::consts::PI * (step - 0.5)
        };
        let tilt = sp.tilt + (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * sp.tilt_variance;
        let offset = Vec3::from_array(sp.offset(u, azimuth, tilt));
        pos_local += if sp.camera_oriented {
            g.actor_rot.inverse() * g.cam_view * offset
        } else {
            offset
        };
    }
    g.elements_emitted += 1;
    let mut pos = pos_local * g.vel_basis;
    // 0x03 VelocityVarianceSetup: a uniform [-v, v] draw per axis on top of the 0x02 base
    // (research/xim ParticleInitializers.kt VelocityVarianceSetup — the shipped blocks all
    // sit after their 0x02, so base-plus-variance is the authored order).
    let mut vel = Vec3::from_array(g.def.init_velocity);
    if let Some(var) = g.def.velocity_variance {
        vel += Vec3::new(
            (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * var[0],
            (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * var[1],
            (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * var[2],
        );
    }
    // 0x08 RelativeVelocitySetup: the payload speed along the spawn offset's direction; with
    // no offset there is no direction and the block contributes nothing (research/xim
    // ParticleInitializers.kt RelativeVelocitySetup — normalize of the initial position
    // relative to the spawn point).
    let mut rel_vel = Vec3::ZERO;
    if let Some(speed) = g.def.relative_velocity {
        if pos_local.length_squared() > 0.0 {
            let add = pos_local.normalize() * speed;
            vel += add;
            rel_vel += add;
        }
    }
    // 0x41 RelativeVelocityVarianceSetup: a uniform [-v, v] draw along the same spawn offset
    // direction as 0x08, no direction without an offset (research/xim
    // ParticleInitializers.kt RelativeVelocityVarianceSetup — retail's CYyGenerator.cpp
    // CYyGenerator::ElemGenerate case 0x41 scales the normalized offset by frand of the
    // value and adds it to the same allocation vector as 0x08).
    if let Some(v) = g.def.relative_velocity_variance {
        if pos_local.length_squared() > 0.0 {
            let add = pos_local.normalize() * ((next_unit(&mut g.emit_rng) * 2.0 - 1.0) * v);
            vel += add;
            rel_vel += add;
        }
    }
    // 0x67 ReverseDisplacementSetup: the particle spawns at the trajectory's endpoint and
    // traces the path backward — position'(t) = P0 + v × (maxAge − t)
    // (research/xim ParticleInitializers.kt ReverseDisplacementSetup — position gains the
    // total velocity × maxAge, then the velocity is negated; the payload float is unused).
    if g.def.reverse_displacement.is_some() {
        pos += vel * g.vel_basis * life_frames;
        vel = -vel;
        rel_vel = -rel_vel;
    }
    // 0x0A RotationVarianceInitializer: a uniform [-v, v] draw per axis on top of the 0x09
    // base rotation (research/xim ParticleInitializers.kt RotationVarianceInitializer).
    let mut rotation = Vec3::from_array(g.def.init_rotation);
    if let Some(var) = g.def.rotation_variance {
        rotation += Vec3::new(
            (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * var[0],
            (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * var[1],
            (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * var[2],
        );
    }
    // 0x3B IncrementalRotationApplier: the increment × the element's index into this
    // generator's emission, on top of the base rotation, plus the render-time y-flip
    // (research/xim ParticleInitializers.kt IncrementalRotationApplier — rotation +=
    // incr × (1 + the particles emitted before this one); xim verifies the y-flip fires
    // even for an all-zero payload). The counter above already counts this element, so
    // its value is exactly xim's multiplier.
    let mut negate_rotation_y = false;
    if let Some(incr) = g.def.incremental_rotation {
        let m = g.elements_emitted as f32;
        rotation += Vec3::new(incr[0] * m, incr[1] * m, incr[2] * m);
        negate_rotation_y = true;
    }
    // 0x0C VelocityVarianceSetup (rotation): a uniform [-v, v] draw per axis on top of the
    // 0x0B spin rate, per particle (research/xim ParticleInitializers.kt
    // VelocityVarianceSetup — the allocationOffset binds it to the rotation transform; retail's
    // shared 0x03/0x0C/0x13 case adds frand(bounds) to the transform's velocity).
    let spin = match g.def.spin() {
        Some(base) => {
            let mut spin = Vec3::from_array(base);
            if let Some(var) = g.def.rotation_velocity_variance {
                spin += Vec3::new(
                    (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * var[0],
                    (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * var[1],
                    (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * var[2],
                );
            }
            spin
        }
        None => Vec3::ZERO,
    };
    // 0x11 SingleScaleVarianceInitializer: one [0, v) draw shared by every scale axis, on top
    // of the 0x0F base (research/xim ParticleInitializers.kt — scale += posRand(v); retail's
    // ElemGenerate case 0x11 adds a single ufrand to x, y and z).
    let mut scale = Vec2::new(g.def.init_scale[0], g.def.init_scale[1]);
    if let Some(v) = g.def.single_scale_variance {
        scale += Vec2::splat(next_unit(&mut g.emit_rng) * v);
    }
    // 0x10 ScaleVarianceInitializer: a per-axis [0, v) draw on top of the 0x0F base; the z
    // bound has no axis on the engine's 2D sprite (CYyGenerator.cpp
    // CYyGenerator::ElemGenerate case 0x10 — field_EC.x/y/z += ufrand(payload)).
    if let Some(var) = g.def.scale_variance {
        scale += Vec2::new(
            next_unit(&mut g.emit_rng) * var[0],
            next_unit(&mut g.emit_rng) * var[1],
        );
    }
    // 0x12 ScaleVelocitySetup: the per-frame growth the sec3 0x08 ScaleUpdater integrates
    // (research/xim ParticleUpdaters.kt — scale += velocity × elapsedFrames); zero while the
    // updater is off, so a rate without it stays inert.
    // 0x13 VelocityVarianceSetup (scale): a uniform [-v, v] draw per axis on top of the
    // 0x12 rate (research/xim ParticleInitializers.kt VelocityVarianceSetup — the
    // allocationOffset binds it to the scale transform; retail's shared 0x03/0x0C/0x13 case
    // adds frand(bounds) to the transform's velocity). The draw lands on the same transform
    // velocity the sec3 0x08 ScaleUpdater integrates, so it is inert without the updater,
    // as for 0x0C; the z bound has no axis on the engine's 2D sprite, as for 0x10.
    let mut scale_vel = Vec2::ZERO;
    if let Some(rate) = g.def.scale_rate() {
        scale_vel = Vec2::new(rate[0], rate[1]);
        if let Some(var) = g.def.scale_velocity_variance {
            scale_vel += Vec2::new(
                (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * var[0],
                (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * var[1],
            );
        }
    }
    // 0x17 ColorVarianceSetup: each rgb channel gains its bound times one [0, 1) draw, on top
    // of the 0x16 base (research/xim ParticleInitializers.kt ColorVarianceSetup — the shipped
    // alpha byte is always 0 and the engine's alpha comes from the 0x16 base / alpha track).
    let mut rgb = Vec3::from_slice(&g.def.init_color[..3]);
    if let Some(var) = g.def.color_variance {
        rgb += Vec3::new(
            var[0] * next_unit(&mut g.emit_rng),
            var[1] * next_unit(&mut g.emit_rng),
            var[2] * next_unit(&mut g.emit_rng),
        );
    }
    // sec2 0x3D OscillationSetup + 0x3E/0x3F/0x40 OscillationAccelerationSetup: the
    // per-particle oscillation state — each present axis gets acceleration + one [−1, 1)
    // variance draw, absent axes stay 0 (research/xim ParticleInitializers.kt
    // OscillationAccelerationSetup — acceleration + variance × RandHelper rand()).
    let osc = g.def.oscillation.then(|| {
        let mut accel = [0.0f32; 3];
        for (axis, block) in accel.iter_mut().zip([
            g.def.oscillation_accel_x,
            g.def.oscillation_accel_y,
            g.def.oscillation_accel_z,
        ]) {
            if let Some(a) = block {
                *axis = a[0] + a[1] * (next_unit(&mut g.emit_rng) * 2.0 - 1.0);
            }
        }
        Oscillation {
            accel,
            prev_amplitude: [0.0; 3],
        }
    });
    g.particles.push(Particle {
        pos,
        spawn_origin: g.origin,
        vel: vel * g.vel_basis,
        age_frames: 0.0,
        life_frames: life_frames.max(1.0),
        rgb,
        scale,
        scale_seed: scale,
        scale_vel,
        rotation,
        spin,
        negate_rotation_y,
        rel_vel: rel_vel * g.vel_basis,
        osc,
    });
}

// CYyGenerator.cpp CYyGenerator::ElemDie case 5 — a relife generator resets an expiring element's
// life and keeps it (its rotation, position and UV state carry on); any other generator's
// expired particles are swept.
fn reap_expired(g: &mut LiveGenerator) {
    if g.def.relife_on_expiry {
        for p in &mut g.particles {
            if p.age_frames >= p.life_frames {
                p.age_frames = p.age_frames.rem_euclid(p.life_frames);
            }
        }
    } else {
        g.particles.retain(|p| p.age_frames < p.life_frames);
    }
}

fn trace_celestial() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    env_flag(&ON, "FFXI_TRACE_CELESTIAL")
}

/// `FFXI_TRACE_PARTICLE_REBUILDS`: once a second, which generators rebuilt
/// their mesh and how many vertices each pushed — the per-frame `Assets<Mesh>`
/// churn the perf log counts as `mesh+N`.
fn trace_particle_rebuilds() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    env_flag(&ON, "FFXI_TRACE_PARTICLE_REBUILDS")
}

#[derive(Default)]
pub struct RebuildTrace {
    since_secs: f32,
    per_generator: std::collections::HashMap<String, (u32, usize)>,
    gated: u32,
}

/// Per-frame draw gate and mesh writer for the live generators.
///
/// A camera-pinned generator sits at the eye by construction, inside every band, so it
/// skips the cull test. The frustum test keeps the near plane and drops the far plane
/// (reverse-z infinite perspective), matching Bevy's own CPU-culling call shape.
/// `built_key` is left alone while a generator is hidden: it stays the key of what is
/// actually in the mesh asset, so the first drawable frame rebuilds to whatever the sim
/// has reached.
pub fn sync_particle_meshes(
    cam: Query<
        (&GlobalTransform, Option<&bevy::camera::primitives::Frustum>),
        With<OperatorCamera>,
    >,
    q_mesh_xf: Query<&GlobalTransform, With<Mesh3d>>,
    mut q_vis: Query<&mut Visibility, With<Mesh3d>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut sim: ResMut<ParticleSimulator>,
    mut commands: Commands,
    time: Res<Time>,
    draw: Option<Res<crate::dat_mzb::DrawDistance>>,
    mut trace: Local<RebuildTrace>,
) {
    let cam_xf = cam.iter().next().map(|(xf, _)| *xf).unwrap_or_default();
    let frusta: Vec<&bevy::camera::primitives::Frustum> =
        cam.iter().filter_map(|(_, f)| f).collect();
    let (cam_rot, cam_pos) = (cam_xf.rotation(), cam_xf.translation());
    // XiZone.cpp XiZone::GetDrawDistance: the band a 0x0A block with no authored maximum
    // falls back to.
    let zone_draw = draw
        .map(|d| d.world)
        .unwrap_or(crate::dat_mzb::RETAIL_FALLBACK_DRAW_DISTANCE);
    let clock = sim.clock;
    let trace_celestial = trace_celestial();
    let trace_rebuilds = trace_particle_rebuilds();

    // (index, despawn-needed); indices ascending so the reverse sweep below can
    // swap_remove safely.
    let parents: std::collections::HashSet<_> = sim.generators.iter().map(|g| g.entity).collect();
    let mut reap: Vec<(usize, bool)> = Vec::new();
    for (i, g) in sim.generators.iter_mut().enumerate() {
        // The mesh entity despawns with its actor (auto-run generators are
        // children of the actor root); reap the simulator entry when it's gone.
        let Ok(entity_xf) = q_mesh_xf.get(g.entity) else {
            reap.push((i, false));
            continue;
        };
        // The 0x1F camera-oriented ring resolves against the camera and the actor's world
        // rotation; sync runs after tick, so both reach emit() one frame behind like
        // `emit_culled` (research/xim ParticleGeneratorParser.kt SphericalPositionVarianceFull).
        g.cam_view = cam_rot;
        if g.actor_local {
            g.actor_rot = entity_xf.rotation();
        }
        if let Some(cull) = g.def.emit_cull.filter(|_| !g.camera_relative) {
            let emitter = if g.actor_local {
                entity_xf.transform_point(g.origin)
            } else {
                g.origin
            };
            g.emit_culled = cull.out_of_range(cam_pos.distance(emitter), zone_draw);
        }
        // In the actor-local frame a billboard must cancel the parent's
        // FFXI->Bevy basis: parent_rot * rot == cam_rot. Fixed-orientation
        // meshes use their DAT rotation directly in the local frame.
        let rot = match (g.orientation, g.actor_local) {
            (Some(q), _) => q,
            (None, true) => entity_xf.rotation().inverse() * cam_rot,
            (None, false) => cam_rot,
        };
        // The tracked get_mut marks the mesh Modified and forces a full GPU re-upload, so it
        // only runs when the rebuilt vertex output would differ from the last built mesh
        // (kuluu-b5nt).
        // The celestial billboards are the one particle population with no on-screen
        // debug affordance — they are 900 units away and often below the horizon, so a
        // wrong colour curve or sprite frame is indistinguishable from "not drawing".
        if trace_celestial
            && matches!(
                g.def.attach_type,
                ffxi_dat::particle_gen::AttachType::Sun | ffxi_dat::particle_gen::AttachType::Moon
            )
        {
            let draw = g.particles.first().map(|p| particle_draw(g, p, &clock));
            info!(
                mesh = %String::from_utf8_lossy(&g.def.mesh_id),
                verts = g.template.positions.len(),
                live = g.particles.len(),
                origin = ?g.origin,
                scale = ?draw.as_ref().map(|d| d.scale),
                rgb = ?draw.as_ref().map(|d| d.factor_rgb),
                frame = ?draw.as_ref().map(|d| d.flipbook_frame),
                "{:?} billboard",
                g.def.attach_type,
            );
        }
        // Simulation is deliberately NOT gated: `advance_generator` keeps running for every
        // generator, so a fountain is in the state it would have had when the camera comes back.
        // Only the mesh write and the draw submission are skipped.
        //
        // The bounds drive `Visibility` rather than a published `Aabb` for stock culling, for two
        // reasons an Aabb cannot meet: the mesh is written here in Update while stock
        // check_visibility runs in PostUpdate, so an Aabb-culled generator re-entering the frustum
        // would be drawn one frame with last-visible geometry; and an auto-run generator entity is
        // a child of the actor root, where kuluu/src/view_native/walker/obstacles.rs
        // snapshot_mob_block_radius consumes the first descendant Aabb as the mob block radius.
        // No operator camera at all gates nothing: the launcher backdrop camera is marked
        // `BackdropCamera`, so behind the character-select screen `frusta` is empty.
        let drawable = g.camera_relative
            || frusta.is_empty()
            || generator_bounds(g, &clock).is_some_and(|b| {
                frusta
                    .iter()
                    .any(|f| f.intersects_obb(&b, &entity_xf.affine(), true, false))
            });
        if let Ok(mut v) = q_vis.get_mut(g.entity) {
            v.set_if_neq(if drawable {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            });
        }
        let view = CameraView { rot, pos: cam_pos };
        if drawable {
            let key = mesh_key(g, view, &clock);
            if needs_rebuild(&g.built_key, &key) {
                if let Some(mut mesh) = meshes.get_mut(&g.mesh) {
                    rebuild_mesh(g, view, &clock, &mut mesh);
                    g.built_key = key;
                    if trace_rebuilds {
                        let row = trace
                            .per_generator
                            .entry(String::from_utf8_lossy(&g.def.mesh_id).into_owned())
                            .or_default();
                        row.0 += 1;
                        row.1 = g.particles.len() * g.template.positions.len();
                    }
                }
            }
        } else if trace_rebuilds {
            trace.gated += 1;
        }
        let window_over = if let Some(parent) = g.immediate_parent {
            !parents.contains(&parent)
        } else {
            g.stopped || (!g.auto_run && g.age_frames > g.emit_window_frames.max(1.0))
        };
        let done = window_over && g.particles.is_empty();
        if done {
            reap.push((i, true));
        }
    }

    for &(i, despawn) in reap.iter().rev() {
        let g = sim.generators.swap_remove(i);
        if despawn {
            commands.entity(g.entity).try_despawn();
        }
    }

    if trace_rebuilds && time.elapsed_secs() - trace.since_secs >= 1.0 {
        let mut rows: Vec<(String, (u32, usize))> = trace.per_generator.drain().collect();
        rows.sort_by_key(|(_, (rebuilds, _))| std::cmp::Reverse(*rebuilds));
        let summary: Vec<String> = rows
            .iter()
            .map(|(name, (rebuilds, verts))| format!("{name}x{rebuilds}({verts}v)"))
            .collect();
        info!(
            target: "perf",
            generators = sim.generators.len(),
            gated = trace.gated,
            "particle mesh rebuilds/s: {}",
            summary.join(" ")
        );
        trace.gated = 0;
        trace.since_secs = time.elapsed_secs();
    }
}

// The per-particle half of a draw. The other half is the template's per-vertex colour, folded
// in by `vertex_color` once per vertex.
struct ParticleDraw {
    flipbook_frame: usize,
    scale: Vec2,
    // Stage 1's F argument (TEXTUREFACTOR): the generator colour after the time-of-day,
    // day-of-week and moon-phase modulations.
    factor_rgb: Vec3,
    factor_alpha: f32,
    // The raw life curve, before the saturating stage-1 alpha gain.
    life_alpha: f32,
    world: Vec3,
}

fn particle_draw(g: &LiveGenerator, p: &Particle, clock: &CelestialClock) -> ParticleDraw {
    let progress = (p.age_frames / p.life_frames).clamp(0.0, 1.0);
    // A SpriteSheet particle flipbooks its frames over life (research/xim
    // ParticleUpdaters.kt SpriteSheetFrameUpdater), except under MoonPhaseSpriteSheetUpdater
    // (ParticleUpdaters.kt MoonPhaseSpriteSheetUpdater, opcode 0x45 at ParticleGeneratorParser.kt sec3Handler), which pins
    // the frame to the moon phase; a StaticMesh particle keeps its single template.
    let flipbook_frame = if g.def.moon_phase_sprite {
        clock
            .moon_phase
            .min(g.sprite_frames.len().saturating_sub(1))
    } else {
        flipbook_index(g, progress)
    };
    let sx = g
        .scale_x
        .as_ref()
        .map(|t| t.sample_from(progress, Some(p.scale_seed.x)))
        .unwrap_or(p.scale.x);
    let sy = g
        .scale_y
        .as_ref()
        .map(|t| t.sample_from(progress, Some(p.scale_seed.y)))
        .unwrap_or(p.scale.y);
    // research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp HandleOne
    // initializes field_F8 from opcode 0x16; persistent effects retain its authored alpha.
    let alpha = g
        .alpha
        .as_ref()
        .map(|t| t.sample_from(progress, Some(g.def.init_color[3])))
        .unwrap_or(if g.def.continuous || g.def.is_singleton() {
            g.def.init_color[3]
        } else {
            1.0 - progress
        });
    // research/xim ParticleGeneratorParser.kt sec3Handler ClockValueUpdater — 0x3C/0x3D/0x3E
    // assign the particle's colour channel from a time-of-day curve, 0x3F multiplies alpha.
    // This is the sun's authored dawn/noon/dusk ramp: the disc is not tinted by a formula.
    let mut rgb = p.rgb;
    let mut alpha = alpha;
    for (channel, track) in g.tod_color.iter().enumerate() {
        let Some(track) = track.as_ref().filter(|_| g.def.tod_color_driven[channel]) else {
            continue;
        };
        let v = track.sample(clock.day_fraction);
        match channel {
            TOD_ALPHA_CHANNEL => alpha *= v,
            _ => rgb[channel] = v,
        }
    }
    // research/xim Particle.kt getColor() — the day-of-week tint is applied first,
    // then the moon-phase tint, each as a 2x modulate (out = min(1, out * 2 * c)). Both use
    // Color.modulateInPlace (Color.kt), which scales alpha too, and NOT the rgb-only
    // Color.modulateRgbInPlace (Color.kt) sitting next to it: the tables' alpha lane is
    // what gates the lunar halo off outside the full-moon phases.
    for table in [
        g.def
            .day_of_week_color
            .map(|t| t[clock.day_of_week % ffxi_dat::particle_gen::DAYS_OF_WEEK]),
        g.def
            .moon_phase_color
            .map(|t| t[clock.moon_phase % ffxi_dat::particle_gen::MOON_PHASES]),
    ]
    .into_iter()
    .flatten()
    {
        rgb = (rgb * Vec3::from_slice(&table[..3]) * CELESTIAL_MODULATE).min(Vec3::ONE);
        alpha = (alpha * table[TOD_ALPHA_CHANNEL] * CELESTIAL_MODULATE).min(1.0);
    }

    ParticleDraw {
        flipbook_frame,
        scale: Vec2::new(sx, sy),
        factor_rgb: rgb,
        factor_alpha: tfactor_alpha(&g.def, g.draw_path, alpha),
        life_alpha: alpha,
        world: particle_origin(g, p) + p.pos,
    }
}

fn particle_origin(g: &LiveGenerator, p: &Particle) -> Vec3 {
    if g.def.camera_attached_base {
        p.spawn_origin
    } else {
        g.origin
    }
}

// D3D interpolates stage 0's D argument across the primitive, so the stage chain runs once per
// vertex against the template's authored colour — not once per particle against a single
// representative vertex.
fn vertex_color(g: &LiveGenerator, draw: &ParticleDraw, vertex: Vec4) -> [f32; 4] {
    let (stage_rgb, stage_alpha) = d3m_stage_chain(
        vertex.truncate(),
        vertex.w,
        draw.factor_rgb,
        draw.factor_alpha,
        ignores_texture_alpha(&g.def, g.draw_path),
        g.draw_path,
    );
    // An additive/subtractive element draws `SRCALPHA * colour`, so its alpha channel is a
    // brightness factor rather than a coverage one. We stand the raw life curve in for
    // retail's alpha stage chain there, and hand it to the blend state as the src alpha the
    // shader premultiplies with — the multiply then lands on the saturated stage-1 colour,
    // which is where retail applies it. Alpha-blended elements use the real stage-1 alpha.
    match (g.def.blend, g.draw_path) {
        (ffxi_dat::particle_gen::ParticleBlend::Blend, _) => {
            [stage_rgb.x, stage_rgb.y, stage_rgb.z, stage_alpha]
        }
        // An MMB's own vertex alpha is the shape, not a uniform: the sun/moon glow domes are
        // untextured gradients that ramp 128 at the centre to 0 at the rim, so folding the life
        // curve onto a flat 1.0 would draw them as hard-edged discs.
        (_, D3mDrawPath::Mmb | D3mDrawPath::MmbUntextured) => [
            stage_rgb.x,
            stage_rgb.y,
            stage_rgb.z,
            draw.life_alpha * vertex.w.min(D3M_STAGE_CLAMP),
        ],
        _ => [stage_rgb.x, stage_rgb.y, stage_rgb.z, draw.life_alpha],
    }
}

// One step is invisible on screen: 1/1024 world unit is sub-pixel at any playable camera
// distance, and the same step on a quat component (~0.11 deg) or a UV offset (sub-texel on
// retail sprite sheets) moves a vertex/texel by less than that.
const MESH_KEY_SPATIAL_QUANTUM: f32 = 1.0 / 1024.0;
// One 8-bit render-target step; a smaller colour delta cannot change the drawn pixel.
const MESH_KEY_COLOR_QUANTUM: f32 = 1.0 / 256.0;

// Quantized snapshot of every dynamic input rebuild_mesh consumes (via particle_draw, plus the
// billboard rotation and UV scroll it reads directly). Zero live particles rebuild to the same
// hidden primitive whatever those inputs are, hence the input-free Empty variant.
#[derive(Clone, PartialEq, Eq, Debug)]
enum MeshKey {
    Empty,
    Live {
        rot: [i32; 4],
        // Only an axial camera billboard reorients per particle from the eye position, so only
        // it puts the camera translation in the key; every other generator would rebuild on
        // every step the camera takes.
        cam_pos: Option<[i32; 3]>,
        uv_scroll: [i32; 2],
        particles: Vec<ParticleKey>,
    },
}

// The camera terms rebuild_mesh orients against: the screen-billboard rotation (already folded
// into the generator's local frame by the caller) and the eye position an axial camera billboard
// aims at.
#[derive(Clone, Copy)]
struct CameraView {
    rot: Quat,
    pos: Vec3,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct ParticleKey {
    world: [i32; 3],
    flipbook_frame: usize,
    scale: [i32; 2],
    // The per-particle colour inputs rather than the drawn colour: the template's per-vertex
    // half is fixed once `flipbook_frame` is, so these are the only terms that can move it.
    factor_rgb: [i32; 3],
    factor_alpha: i32,
    life_alpha: i32,
    rotation: [i32; 3],
}

fn quantized(v: f32, quantum: f32) -> i32 {
    (v / quantum).round() as i32
}

fn mesh_key(g: &LiveGenerator, cam: CameraView, clock: &CelestialClock) -> MeshKey {
    if g.particles.is_empty() {
        return MeshKey::Empty;
    }
    let spatial = |v: f32| quantized(v, MESH_KEY_SPATIAL_QUANTUM);
    let color = |v: f32| quantized(v, MESH_KEY_COLOR_QUANTUM);
    MeshKey::Live {
        rot: cam.rot.to_array().map(spatial),
        cam_pos: is_axial_camera_billboard(g).then(|| cam.pos.to_array().map(spatial)),
        uv_scroll: [spatial(g.tex_translate.x), spatial(g.tex_translate.y)],
        particles: g
            .particles
            .iter()
            .map(|p| {
                let draw = particle_draw(g, p, clock);
                ParticleKey {
                    world: draw.world.to_array().map(spatial),
                    flipbook_frame: draw.flipbook_frame,
                    scale: [spatial(draw.scale.x), spatial(draw.scale.y)],
                    factor_rgb: draw.factor_rgb.to_array().map(color),
                    factor_alpha: color(draw.factor_alpha),
                    life_alpha: color(draw.life_alpha),
                    rotation: p.rotation.to_array().map(spatial),
                }
            })
            .collect(),
    }
}

fn needs_rebuild(built: &MeshKey, next: &MeshKey) -> bool {
    built != next
}

// research/xim Particle.kt computeParticleSpaceOrientationTransform + GLDrawer.kt drawXimParticle — BillBoardType::Camera is not a screen
// billboard: retail leaves the modelview alone and gives the particle a world orientation that
// aims its mesh-local +X at the eye, so the mesh stays a solid with all three axes scaled. Only
// BillBoardType::XYZ replaces the modelview basis with the view basis. `solid_mesh` is what
// makes that description true of the linked geometry.
fn is_axial_camera_billboard(g: &LiveGenerator) -> bool {
    g.def.billboard == ParticleBillboard::Camera
        && g.orientation.is_none()
        && !g.actor_local
        && g.solid_mesh
}

// The distance from the particle centre to the farthest vertex any frame of this generator can
// place there, before per-particle scale. Taken as a norm rather than per-axis extents so it is
// rotation-invariant: the screen billboard, `particle_rotation` spin, `axial_camera_rotation` and
// the `vel_basis` sign flips all preserve it, and none of them can push a vertex outside it.
fn template_bound_radius(template: &SpriteTemplate, sprite_frames: &[SpriteTemplate]) -> f32 {
    std::iter::once(template)
        .chain(sprite_frames)
        .flat_map(|t| t.positions.iter())
        .fold(0.0f32, |acc, p| acc.max(p.length()))
}

/// The live particle set's bounds in the frame `rebuild_mesh` writes positions in
/// (entity-local for an actor-local generator, Bevy world space otherwise), so the caller
/// supplies the entity transform. `None` when nothing is alive to draw.
///
/// This over-approximates on purpose: a partially visible generator has to stay in, and the
/// per-particle radius folds in a scale the individual vertex may not use. The z-scale
/// follows `rebuild_mesh`: a flat screen billboard leaves its unused depth axis at 1.0,
/// everything else scales by the untracked init z.
fn generator_bounds(
    g: &LiveGenerator,
    clock: &CelestialClock,
) -> Option<bevy::camera::primitives::Aabb> {
    if g.particles.is_empty() {
        return None;
    }
    let axial = is_axial_camera_billboard(g);
    let sz = if g.orientation.is_some() || axial {
        g.def.init_scale[2]
    } else {
        1.0
    };
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for p in &g.particles {
        let draw = particle_draw(g, p, clock);
        let r = g.bound_radius * draw.scale.x.abs().max(draw.scale.y.abs()).max(sz.abs());
        lo = lo.min(draw.world - r);
        hi = hi.max(draw.world + r);
    }
    Some(bevy::camera::primitives::Aabb::from_min_max(lo, hi))
}

// A template with no extent on some axis is a flat authored sprite quad — every D3M billboard
// and SpriteSheet frame is an XY rectangle whose vertices carry z exactly 0, so its only face
// normal is the axis it is missing. Aiming such a quad's local +X at the eye lays its plane
// along the view ray and it draws edge-on, which is why the aim-at-eye rotation describes only
// the solids retail links to a Camera generator (the `suns`/`moon`/`hdhu` glow domes, thin in x
// and round in y/z).
fn is_solid_mesh(template: &SpriteTemplate) -> bool {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for p in &template.positions {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    (hi - lo).cmpgt(Vec3::ZERO).all()
}

// research/xim Particle.kt `applyMovementOrientation`, with the direction supplied by
// Particle.kt computeParticleSpaceOrientationTransform (`camera position - particle position`). `vel_basis` is an involution, so the
// same fold carries the Bevy-space direction into the DAT frame the template lives in.
fn axial_camera_rotation(particle_world: Vec3, cam_pos: Vec3, vel_basis: Vec3) -> Quat {
    const AXIS_ALIGNED_Y: f32 = 0.999;
    let m = (cam_pos - particle_world) * vel_basis;
    let Some(m) = m.try_normalize() else {
        return Quat::IDENTITY;
    };
    if m.y.abs() >= AXIS_ALIGNED_Y {
        return Quat::from_rotation_z(m.y.signum() * std::f32::consts::FRAC_PI_2);
    }
    let left = Vec3::Y.cross(m).normalize();
    let up = m.cross(left).normalize();
    let angle = -up.dot(Vec3::Y).clamp(-1.0, 1.0).acos() * m.y.signum();
    Quat::from_axis_angle(left, angle) * Quat::from_rotation_y(-m.z.atan2(m.x))
}

fn rebuild_mesh(g: &LiveGenerator, cam: CameraView, clock: &CelestialClock, mesh: &mut Mesh) {
    let verts_per = g.template.positions.len();
    let n = g.particles.len();
    let mut positions = Vec::with_capacity(n * verts_per);
    let mut uvs = Vec::with_capacity(n * verts_per);
    let mut colors = Vec::with_capacity(n * verts_per);
    let mut indices = Vec::with_capacity(n * g.template.indices.len());
    let axial = is_axial_camera_billboard(g);

    for p in &g.particles {
        let draw = particle_draw(g, p, clock);
        let tpl = flipbook_template(g, draw.flipbook_frame);

        let rot = if axial {
            axial_camera_rotation(draw.world, cam.pos, g.vel_basis)
        } else if g.orientation.is_some() {
            particle_rotation(p)
        } else {
            cam.rot * particle_rotation(p)
        };
        // Billboard sprites are flat (z unused); a 3-D particle mesh — a fixed-orientation
        // one, or an axial camera billboard, which stays a world-oriented solid — keeps its
        // DAT depth axis scaled by the untracked init z-scale.
        let sz = if g.orientation.is_some() || axial {
            g.def.init_scale[2]
        } else {
            1.0
        };
        // Fixed-orientation zone sheets carry raw FFXI-frame geometry; apply the
        // generator's FFXI->Bevy basis (the same flip on origin/velocity, matching
        // dat_mzb.rs to_bevy) so a falling water sheet hangs down into the basin
        // instead of standing up above the emitter. Actor-local generators integrate in the
        // actor frame, whose parent transform already carries the dat_mzb.rs to_bevy basis.
        let world_basis = (g.orientation.is_some() || axial) && !g.actor_local;
        // A screen billboard's template is DAT-frame geometry too (Y down: the campfire flame
        // `hi12` rises toward negative y). An actor-local generator inherits the FFXI->Bevy basis
        // from its parent transform; a world-space one folds it into the template before the
        // view rotation, or the flame hangs below its wick (the same flip dat_mzb.rs to_bevy
        // applies to zone origins).
        let screen_basis = g.orientation.is_none() && !axial && !g.actor_local;
        let base = positions.len() as u32;
        for ((tp, uv), vertex) in tpl.positions.iter().zip(&tpl.uvs).zip(&tpl.colors) {
            let local = Vec3::new(tp.x * draw.scale.x, tp.y * draw.scale.y, tp.z * sz);
            let local = if screen_basis {
                local * g.vel_basis
            } else {
                local
            };
            let oriented = rot * local;
            let oriented = if world_basis {
                oriented * g.vel_basis
            } else {
                oriented
            };
            positions.push((draw.world + oriented).to_array());
            uvs.push([uv[0] + g.tex_translate.x, uv[1] + g.tex_translate.y]);
            colors.push(vertex_color(g, &draw, *vertex));
        }
        indices.extend(tpl.indices.iter().map(|&idx| base + idx));
    }

    if positions.is_empty() {
        push_hidden_primitive(&mut positions, &mut uvs, &mut colors, &mut indices);
    }

    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
}

const HIDDEN_PRIMITIVE_VERTS: usize = 3;

// A generator with zero live particles (on spawn, and in the gaps between emit
// windows) would otherwise rebuild an empty mesh. Bevy's MeshAllocator skips the
// slab allocation for a zero-length vertex buffer but still runs the upload copy,
// logging "Use-after-free: attempted to copy element data for an unallocated key"
// (bevy_render slab_allocator.rs) every such frame. Keep the buffer non-empty with
// one zero-area, fully-transparent triangle so it uploads cleanly and draws nothing.
fn push_hidden_primitive(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    colors: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
) {
    let base = positions.len() as u32;
    for _ in 0..HIDDEN_PRIMITIVE_VERTS {
        positions.push([0.0, 0.0, 0.0]);
        uvs.push([0.0, 0.0]);
        colors.push([0.0, 0.0, 0.0, 0.0]);
    }
    indices.extend([base, base + 1, base + 2]);
}

fn sprite_template(d3m: &ffxi_dat::d3m::D3m) -> Option<SpriteTemplate> {
    if d3m.vertices.is_empty() {
        return None;
    }
    let positions = d3m
        .vertices
        .iter()
        .map(|v| Vec3::from_array(v.pos))
        .collect();
    let uvs = d3m.vertices.iter().map(|v| v.uv).collect();
    let indices = (0..d3m.vertices.len() as u32).collect();
    let colors = d3m
        .vertices
        .iter()
        .map(|v| Vec4::from_array(v.color))
        .collect();
    Some(SpriteTemplate {
        positions,
        uvs,
        indices,
        colors,
    })
}

// None when the referenced mesh isn't present, which leaves zone callers to fall back to an
// MMB mesh.
// Zone sprays link a D3M billboard, an MMB mesh, or a SpriteSheet by DatId (e.g. Bastok "abuk",
// Port Windurst "rivsea"); the MMB/SpriteSheet texture resolves by internal name.
fn resolve_zone_mesh(
    assets: &ActionAssets,
    def: &ParticleGeneratorDef,
    images: &mut Assets<Image>,
    undither: bool,
) -> Option<(
    SpriteTemplate,
    Vec<SpriteTemplate>,
    Option<Handle<Image>>,
    D3mDrawPath,
)> {
    // Zone and weather generators are collected by chunk name without their directory
    // (zone_particles.rs `zone_static_defs`), so there is no scope to resolve the mesh in and
    // the lookup falls through to the flat tier.
    if let Some((template, frames, tex)) = resolve_mesh(assets, NO_LOCAL_DIR, def, images, undither)
    {
        return Some((template, frames, tex, D3mDrawPath::D3m));
    }
    let mmb = assets.mmbs.get(&def.mesh_id)?;
    let template = mmb_sprite_template(mmb)?;
    let tex = assets
        .images_by_name
        .get(&mmb.texture_name)
        .map(|t| images.add(to_image(t, undither)));
    let path = if tex.is_some() {
        D3mDrawPath::Mmb
    } else {
        D3mDrawPath::MmbUntextured
    };
    Some((template, Vec::new(), tex, path))
}

fn keyframe(
    assets: &ActionAssets,
    global: Option<&ActionAssets>,
    id: Option<[u8; 4]>,
) -> Option<KeyFrameTrack> {
    let id = id?;
    assets
        .keyframes
        .get(&id)
        .or_else(|| global.and_then(|g| g.keyframes.get(&id)))
        .cloned()
}

fn to_image(t: &ffxi_dat::texture::DecodedTexture, undither: bool) -> Image {
    if undither {
        decoded_sky_texture_to_image(t)
    } else {
        decoded_texture_to_image(t)
    }
}

// `local_dir` is the directory the generator DEF was authored in (research/xim
// ParticleInitializers.kt apply `particle.creator.localDir`), not the routine's: mesh ids repeat
// across effect directories, so the flat maps alone bind whichever copy the walk saw last.
fn resolve_mesh(
    assets: &ActionAssets,
    local_dir: [u8; 4],
    def: &ParticleGeneratorDef,
    images: &mut Assets<Image>,
    undither: bool,
) -> Option<(SpriteTemplate, Vec<SpriteTemplate>, Option<Handle<Image>>)> {
    match def.mesh_kind {
        ParticleMeshKind::StaticMesh => {
            let d3m = assets.d3m(local_dir, &def.mesh_id)?;
            let template = sprite_template(d3m)?;
            let (namespace, local) = d3m.texture_name_tokens();
            // research/xim DatResource.kt getTextureResourceByNameAs — qualified (namespace, local) match, then
            // local-only. The truncated DatId stays as a last tier: a few meshes name a
            // texture whose local token outruns the Img chunk id (`kumori` vs `kumo`) and
            // resolve only that way.
            let by_name = (!local.is_empty()).then(|| {
                assets
                    .images_by_qualified_name
                    .get(&(namespace, local.clone()))
                    .or_else(|| assets.images_by_name.get(&local))
            });
            let tex = by_name
                .flatten()
                .or_else(|| assets.images.get(&d3m.texture_dat_id()))
                .map(|t| images.add(to_image(t, undither)));
            Some((template, Vec::new(), tex))
        }
        ParticleMeshKind::SpriteSheet => {
            let ss = assets.sprite_sheet(local_dir, &def.mesh_id)?;
            let frames = sprite_sheet_templates(ss);
            let first = frames.first().cloned()?;
            // research/xim DatResource.kt getTextureResourceByNameAs — try the qualified (namespace, local) pair
            // first, then fall back to a local-name-only match.
            let tex = assets
                .images_by_qualified_name
                .get(&(ss.category.clone(), ss.id.clone()))
                .or_else(|| assets.images_by_name.get(&ss.id))
                .map(|t| images.add(to_image(t, undither)));
            Some((first, frames, tex))
        }
    }
}

fn sprite_sheet_templates(ss: &ParticleSpriteSheet) -> Vec<SpriteTemplate> {
    ss.frames
        .iter()
        .filter_map(|f| {
            if f.positions.is_empty() {
                return None;
            }
            Some(SpriteTemplate {
                positions: f.positions.iter().map(|p| Vec3::from_array(*p)).collect(),
                uvs: f.uvs.clone(),
                indices: (0..f.positions.len() as u32).collect(),
                // FFXI vertex colors are 2x-overbright (see d3m.rs color parse); the venom-cloud
                // tint is then modulated by the generator's init_color in rebuild_mesh.
                colors: f
                    .colors
                    .iter()
                    .map(|c| {
                        Vec4::new(c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32)
                            / ffxi_dat::d3m::VERTEX_COLOR_DIVISOR
                    })
                    .collect(),
            })
        })
        .collect()
}

// research/xim ParticleUpdaters.kt SpriteSheetFrameUpdater — the spriteSheetIndex advances the flipbook across
// the particle's lifetime. StaticMesh particles carry no frames and use the single template.
fn flipbook_index(g: &LiveGenerator, progress: f32) -> usize {
    let n = g.sprite_frames.len();
    if n == 0 {
        return 0;
    }
    ((progress * n as f32) as usize).min(n - 1)
}

fn flipbook_template(g: &LiveGenerator, idx: usize) -> &SpriteTemplate {
    g.sprite_frames.get(idx).unwrap_or(&g.template)
}

fn mmb_sprite_template(mmb: &MmbSpriteMesh) -> Option<SpriteTemplate> {
    if mmb.positions.is_empty() || mmb.indices.is_empty() {
        return None;
    }
    Some(SpriteTemplate {
        positions: mmb.positions.iter().map(|p| Vec3::from_array(*p)).collect(),
        uvs: mmb.uvs.clone(),
        indices: mmb.indices.clone(),
        colors: mmb.colors.iter().map(|c| Vec4::from_array(*c)).collect(),
    })
}

fn empty_mesh() -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    let (mut positions, mut uvs, mut colors, mut indices) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    push_hidden_primitive(&mut positions, &mut uvs, &mut colors, &mut indices);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::particle_gen::ParticleGeneratorDef;

    fn def(life: f32, fpe: f32, ppe: u32) -> ParticleGeneratorDef {
        ParticleGeneratorDef {
            frames_per_emission: fpe,
            particles_per_emission: ppe,
            emission_variance: 0.0,
            mesh_id: *b"gr  ",
            mesh_kind: ffxi_dat::particle_gen::ParticleMeshKind::StaticMesh,
            base_position: [0.0, 0.5, 0.0],
            max_life_frames: life,
            camera_billboard: true,
            billboard: ParticleBillboard::Xyz,
            camera_relative: false,
            follow_camera: false,
            camera_attached_base: false,
            position_variance: None,
            spherical_full: None,
            continuous: false,
            auto_run: false,
            batched: false,
            attach_type: ffxi_dat::particle_gen::AttachType::SourceActor,
            tod_color_tracks: [None; ffxi_dat::particle_gen::TOD_COLOR_CHANNELS],
            tod_color_driven: [false; ffxi_dat::particle_gen::TOD_COLOR_CHANNELS],
            moon_phase_sprite: false,
            attach_joint_source: 0,
            attach_joint_target: 0,
            attach_source_oriented: false,
            init_scale: [0.1, 0.1, 1.0],
            single_scale_variance: None,
            scale_variance: None,
            init_color: [0.2, 0.2, 0.6, 0.5],
            color_variance: None,
            color_transform: None,
            color_transform_modifier: None,
            init_velocity: [0.0, 0.01, 0.0],
            velocity_variance: None,
            relative_velocity: None,
            relative_velocity_variance: None,
            reverse_displacement: None,
            rotation_variance: None,
            init_rotation: [0.0; 3],
            incremental_rotation: None,
            blend: ffxi_dat::particle_gen::ParticleBlend::Additive,
            blend_byte: 0x48,
            ignore_texture_alpha: false,
            fog_enabled: true,
            draw_priority: Default::default(),
            sort_offset: 0.0,
            projection_bias: None,
            depth_write: false,
            scale_x_track: None,
            scale_y_track: None,
            scale_z_track: None,
            alpha_track: None,
            color_r_track: None,
            color_g_track: None,
            color_b_track: None,
            day_of_week_color: None,
            moon_phase_color: None,
            uv_scroll: [0.0, 0.0],
            accel: None,
            rotation_accel: None,
            scale_accel: None,
            emit_cull: None,
            association: None,
            foot_mark: false,
            oscillation: false,
            parent_position_copy: false,
            parent_velocity: None,
            child_generator: None,
            immediate_generator: None,
            oscillation_accel_z: None,
            oscillation_accel_x: None,
            oscillation_accel_y: None,
            oscillation_applier_x: None,
            oscillation_applier_z: None,
            oscillation_applier_y: None,
            rotation_velocity: None,
            rotation_velocity_variance: None,
            rotation_updater: false,
            position_updater: true,
            scale_velocity: None,
            scale_updater: false,
            scale_velocity_variance: None,
            relife_on_expiry: false,
            specular_element: false,
            specular: None,
            specular_rot_y_track: None,
            camera_shake_track: None,
            camera_shake: None,
            haze_offset_x: None,
            parent_rotate: false,
            parent_color: false,
            parent_scale: false,
            velocity_dampener_track: None,
            velocity_dampener: None,
            velocity_rotator: None,
            fixed_point_position_variance: None,
            fixed_point_position_variance_2: None,
            child_generator_2: None,
            specular_rot_z_track: None,
            specular_color_a_track: None,
            parent_rotate_2: false,
            batching_setup: false,
            parent_tex_coord: false,
            point_list_position: None,
            velocity_y_track: None,
            specular_rot_x_track: None,
            specular_color_g_track: None,
        }
    }

    fn live(def: ParticleGeneratorDef, window: f32) -> LiveGenerator {
        LiveGenerator {
            immediate_parent: None,
            def,
            template: SpriteTemplate {
                positions: vec![Vec3::ZERO; 3],
                uvs: vec![[0.0, 0.0]; 3],
                indices: vec![0, 1, 2],
                colors: vec![Vec4::ONE; 3],
            },
            draw_path: D3mDrawPath::D3m,
            sprite_frames: Vec::new(),
            tod_color: [None, None, None, None],
            scale_x: None,
            scale_y: None,
            alpha: None,
            origin: Vec3::ZERO,
            particles: Vec::new(),
            emit_accum: 0.0,
            age_frames: 0.0,
            emit_window_frames: window,
            mesh: Handle::default(),
            entity: Entity::PLACEHOLDER,
            auto_run: false,
            orientation: None,
            solid_mesh: false,
            actor_local: false,
            tex_translate: Vec2::ZERO,
            vel_basis: Vec3::ONE,
            origin_routine: None,
            stopped: false,
            camera_relative: false,
            emit_culled: false,
            emit_scale: UNSCALED_EMISSION,
            emit_rng: emit_seed(Entity::PLACEHOLDER),
            elements_emitted: 0,
            cam_view: Quat::IDENTITY,
            actor_rot: Quat::IDENTITY,
            built_key: MeshKey::Empty,
            bound_radius: 0.0,
        }
    }

    // Drive the emission math directly (no Bevy world), one tick's worth of frames per call.
    fn advance(g: &mut LiveGenerator, frames: f32) {
        advance_generator(g, frames);
    }

    // 0x1E ParticleDampen: emission stops and the already-live particles are force-expired
    // at once (research/xim EffectRoutineInstance.kt handleParticleEffectDampen), unlike
    // StopParticle which lets them play out.
    #[test]
    fn authored_velocity_damping_applies_after_position_with_fractional_delta() {
        const DAMPING_PER_FRAME: f32 = 0.25;
        const HALF_FRAME: f32 = 0.5;
        let mut d = def(60.0, 1.0, 1);
        d.velocity_dampener = Some([DAMPING_PER_FRAME, 0.0]);
        let mut g = live(d, 0.0);
        emit(&mut g, 60.0);
        g.stopped = true;
        g.particles[0].pos = Vec3::ZERO;
        g.particles[0].vel = Vec3::X;
        advance_generator(&mut g, HALF_FRAME);
        assert_eq!(g.particles[0].pos, Vec3::X * HALF_FRAME);
        assert_eq!(g.particles[0].vel, Vec3::X * HALF_FRAME);
        advance_generator(&mut g, HALF_FRAME);
        assert_eq!(
            g.particles[0].pos,
            Vec3::X * (HALF_FRAME + DAMPING_PER_FRAME)
        );
        assert_eq!(g.particles[0].vel, Vec3::X * DAMPING_PER_FRAME);
    }

    #[test]
    fn dampen_generator_stops_emission_and_clears_live_particles() {
        let owner = Entity::from_raw_u32(1).unwrap();
        let mut sim = ParticleSimulator {
            generators: vec![live(def(60.0, 1.0, 1), 60.0)],
            clock: CelestialClock::default(),
        };
        sim.generators[0].origin_routine = Some(RoutineOrigin {
            owner,
            gen_id: *b"gr01",
            routine: *b"cate",
        });
        advance(&mut sim.generators[0], 2.0);
        assert!(
            !sim.generators[0].particles.is_empty(),
            "two frames emit two particles"
        );

        sim.dampen_generator(owner, *b"gr01");
        assert!(sim.generators[0].stopped);
        assert!(
            sim.generators[0].particles.is_empty(),
            "the live particles expire at once"
        );

        advance(&mut sim.generators[0], 10.0);
        assert!(
            sim.generators[0].particles.is_empty(),
            "no re-emission after the dampen"
        );
    }

    /// The campfire flame `hi12` rises toward negative DAT y; a world-space screen billboard
    /// has to fold the FFXI->Bevy basis into that template itself, while an actor-local one
    /// leaves it to the parent transform.
    #[test]
    fn world_space_screen_billboard_folds_the_dat_basis_into_its_template() {
        fn built_vertex(actor_local: bool) -> [f32; 3] {
            let d = ParticleGeneratorDef {
                auto_run: true,
                max_life_frames: 60.0,
                frames_per_emission: 1.0,
                particles_per_emission: 1,
                init_scale: [1.0; 3],
                init_color: [1.0; 4],
                ..Default::default()
            };
            let mut g = live(d, 0.0);
            g.template.positions = vec![Vec3::new(0.5, -1.0, -2.0); 3];
            g.orientation = None;
            g.solid_mesh = false;
            g.actor_local = actor_local;
            g.vel_basis = if actor_local {
                Vec3::ONE
            } else {
                Vec3::new(1.0, -1.0, -1.0)
            };
            emit(&mut g, 60.0);
            let mut mesh = empty_mesh();
            let view = CameraView {
                rot: Quat::IDENTITY,
                pos: Vec3::ZERO,
            };
            rebuild_mesh(&g, view, &ParticleSimulator::default().clock, &mut mesh);
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
                .and_then(|a| a.as_float3())
                .expect("positions")[0]
        }
        assert_eq!(built_vertex(false), [0.5, 1.0, 2.0], "zone flame rises");
        assert_eq!(
            built_vertex(true),
            [0.5, -1.0, -2.0],
            "actor-local template stays in the actor's DAT frame"
        );
    }

    #[test]
    fn emit_culled_generator_ages_without_emitting() {
        let mut g = live(def(600.0, 1.0, 1), 0.0);
        g.auto_run = true;
        g.emit_culled = true;
        advance(&mut g, 30.0);
        assert!(g.particles.is_empty(), "culled: nothing emitted");
        assert_eq!(g.age_frames, 30.0, "but the clock still runs");
        g.emit_culled = false;
        advance(&mut g, 30.0);
        assert_eq!(g.particles.len(), 30, "back in band: emits again");
    }

    #[test]
    fn sync_culls_emitters_outside_their_authored_camera_band() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Assets::<Mesh>::default());
        let cam = world
            .spawn((
                OperatorCamera,
                GlobalTransform::from_translation(Vec3::ZERO),
            ))
            .id();
        let mesh_entity = world
            .spawn((Mesh3d(Handle::default()), GlobalTransform::IDENTITY))
            .id();

        let mut d = def(600.0, 1.0, 1);
        d.emit_cull = Some(ffxi_dat::particle_gen::EmitCull {
            max_distance: 40.0,
            min_distance: 0.0,
            unlink_out_of_range: false,
        });
        let mut g = live(d, 0.0);
        g.auto_run = true;
        g.entity = mesh_entity;
        g.origin = Vec3::new(50.0, 0.0, 0.0);
        let mut sim = ParticleSimulator::default();
        sim.generators.push(g);
        world.insert_resource(sim);

        world.run_system_once(sync_particle_meshes).unwrap();
        assert!(
            world.resource::<ParticleSimulator>().generators[0].emit_culled,
            "50 units out on a 40-unit band"
        );

        *world.get_mut::<GlobalTransform>(cam).unwrap() =
            GlobalTransform::from_translation(Vec3::new(20.0, 0.0, 0.0));
        world.run_system_once(sync_particle_meshes).unwrap();
        assert!(
            !world.resource::<ParticleSimulator>().generators[0].emit_culled,
            "30 units: back in band"
        );
    }

    /// The default `PerspectiveProjection` looks down -Z with a 1:1 aspect, so a generator at
    /// +Z is behind the camera and one far enough out on +X is past a side plane.
    fn frustum_camera(world: &mut World, xf: GlobalTransform) -> Entity {
        use bevy::camera::{CameraProjection, PerspectiveProjection};
        let frustum = PerspectiveProjection::default().compute_frustum(&xf);
        world.spawn((OperatorCamera, xf, frustum)).id()
    }

    fn aim_camera(world: &mut World, cam: Entity, at: Vec3) {
        use bevy::camera::{CameraProjection, PerspectiveProjection};
        let xf =
            GlobalTransform::from(Transform::from_translation(Vec3::ZERO).looking_at(at, Vec3::Y));
        *world.get_mut::<GlobalTransform>(cam).unwrap() = xf;
        *world
            .get_mut::<bevy::camera::primitives::Frustum>(cam)
            .unwrap() = PerspectiveProjection::default().compute_frustum(&xf);
    }

    fn built_verts(world: &World, mesh: &Handle<Mesh>) -> usize {
        world
            .resource::<Assets<Mesh>>()
            .get(mesh)
            .and_then(|m| m.attribute(Mesh::ATTRIBUTE_POSITION))
            .map(|a| a.len())
            .expect("mesh positions")
    }

    /// A generator wired to a live mesh asset and mesh entity, with `frames` of emission
    /// already simulated so it has particles to bound.
    fn gated_world(origin: Vec3, bound_radius: f32, frames: f32) -> (World, Handle<Mesh>, Entity) {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Assets::<Mesh>::default());
        let mesh = world.resource_mut::<Assets<Mesh>>().add(empty_mesh());
        let entity = world
            .spawn((
                Mesh3d(mesh.clone()),
                GlobalTransform::IDENTITY,
                Visibility::default(),
            ))
            .id();
        let mut g = live(def(600.0, 1.0, 4), 0.0);
        g.auto_run = true;
        g.entity = entity;
        g.mesh = mesh.clone();
        g.bound_radius = bound_radius;
        g.origin = origin;
        advance(&mut g, frames);
        assert!(!g.particles.is_empty(), "test needs live particles");
        let mut sim = ParticleSimulator::default();
        sim.generators.push(g);
        world.insert_resource(sim);
        (world, mesh, entity)
    }

    #[test]
    fn offscreen_generator_keeps_simulating_but_stops_rebuilding() {
        use bevy::ecs::system::RunSystemOnce;

        let behind = Vec3::new(0.0, 0.0, 50.0);
        let (mut world, mesh, entity) = gated_world(behind, 1.0, 10.0);
        let cam = frustum_camera(&mut world, GlobalTransform::IDENTITY);

        world.run_system_once(sync_particle_meshes).unwrap();
        assert_eq!(
            *world.get::<Visibility>(entity).unwrap(),
            Visibility::Hidden,
            "behind the camera"
        );
        assert_eq!(
            built_verts(&world, &mesh),
            HIDDEN_PRIMITIVE_VERTS,
            "no mesh write while hidden"
        );

        let (want_verts, moved) = {
            let mut sim = world.resource_mut::<ParticleSimulator>();
            let g = &mut sim.generators[0];
            let (age, pos) = (g.age_frames, g.particles[0].pos);
            advance_generator(g, 10.0);
            assert!(g.age_frames > age, "hidden generators keep ageing");
            (
                g.particles.len() * g.template.positions.len(),
                g.particles[0].pos != pos,
            )
        };
        assert!(moved, "hidden particles keep integrating velocity");

        aim_camera(&mut world, cam, behind);
        world.run_system_once(sync_particle_meshes).unwrap();
        assert_eq!(
            *world.get::<Visibility>(entity).unwrap(),
            Visibility::Inherited,
            "back in frame"
        );
        assert_eq!(
            built_verts(&world, &mesh),
            want_verts,
            "rebuilt to the state the sim reached while hidden"
        );
    }

    #[test]
    fn partially_visible_generator_is_not_culled() {
        use bevy::ecs::system::RunSystemOnce;

        let edge = Vec3::new(6.0, 0.0, -10.0);
        for (bound_radius, want) in [(0.1, Visibility::Hidden), (4.0, Visibility::Inherited)] {
            let (mut world, _mesh, entity) = gated_world(edge, bound_radius, 1.0);
            frustum_camera(&mut world, GlobalTransform::IDENTITY);
            world.run_system_once(sync_particle_meshes).unwrap();
            assert_eq!(
                *world.get::<Visibility>(entity).unwrap(),
                want,
                "edge sits past the +X side plane; only the template radius pulls it back in: {bound_radius}"
            );
        }
    }

    #[test]
    fn camera_relative_generator_is_never_gated() {
        use bevy::ecs::system::RunSystemOnce;

        let (mut world, mesh, entity) = gated_world(Vec3::new(0.0, 0.0, 50.0), 1.0, 10.0);
        world.resource_mut::<ParticleSimulator>().generators[0].camera_relative = true;
        frustum_camera(&mut world, GlobalTransform::IDENTITY);

        world.run_system_once(sync_particle_meshes).unwrap();
        assert_eq!(
            *world.get::<Visibility>(entity).unwrap(),
            Visibility::Inherited
        );
        assert!(
            built_verts(&world, &mesh) > HIDDEN_PRIMITIVE_VERTS,
            "camera-pinned curtains rebuild wherever their origin reads"
        );
    }

    #[test]
    fn without_a_camera_frustum_nothing_is_gated() {
        use bevy::ecs::system::RunSystemOnce;

        let (mut world, mesh, entity) = gated_world(Vec3::new(0.0, 0.0, 50.0), 1.0, 10.0);
        world.spawn((OperatorCamera, GlobalTransform::IDENTITY));

        world.run_system_once(sync_particle_meshes).unwrap();
        assert_eq!(
            *world.get::<Visibility>(entity).unwrap(),
            Visibility::Inherited
        );
        assert!(
            built_verts(&world, &mesh) > HIDDEN_PRIMITIVE_VERTS,
            "an operator camera with no frustum gates nothing"
        );
    }

    /// One colour on every template vertex, so a stage-chain expectation is a single number
    /// per particle instead of a gradient.
    fn set_template_color(g: &mut LiveGenerator, rgba: Vec4) {
        g.template.colors = vec![rgba; g.template.positions.len()];
    }

    /// The colour a particle actually draws with: `particle_draw` returns only the
    /// per-particle half, and the template's per-vertex half folds in at `vertex_color`.
    fn drawn_color(g: &LiveGenerator, clock: &CelestialClock) -> Vec4 {
        let draw = particle_draw(g, &g.particles[0], clock);
        Vec4::from_array(vertex_color(g, &draw, g.template.colors[0]))
    }

    // A generator stage's duration is authored in 60 fps frames (research/xim util/Fps.kt Fps internalFps),
    // so a 30-frame emit window is half a second of wall time, not a whole one.
    #[test]
    fn emit_window_is_duration_frames_at_60fps() {
        const WINDOW_FRAMES: f32 = 30.0;
        const TICK_SECS: f32 = 1.0 / 120.0;

        let mut g = live(def(600.0, 1.0, 1), WINDOW_FRAMES);
        let run_for = |g: &mut LiveGenerator, secs: f32| {
            let mut t = 0.0;
            while t < secs {
                advance(g, TICK_SECS * ROUTINE_FPS);
                t += TICK_SECS;
            }
            g.particles.len()
        };
        let after_window = run_for(&mut g, 0.55);
        let half_second_later = run_for(&mut g, 0.5);
        assert!(after_window > 0, "the generator emitted inside its window");
        assert_eq!(
            after_window, half_second_later,
            "emission stops half a second in, not a whole one"
        );
    }

    // research/xim MainTool.kt loop currentLogicalFrameIncrement turns wall ms into frames at the single 60 fps internal
    // clock (util/Fps.kt Fps internalFps), MainTool.kt internalLoop hands that one value to EffectManager.update,
    // and Scene.kt registerEffectsRecursively registers the zone DAT's autoRun generators (braziers,
    // campfires) into that same manager — zone ambients share the ROUTINE_FPS clock with
    // action routines, with no half-rate zone clock (kuluu-rf4h).
    #[test]
    fn zone_auto_run_generators_share_the_routine_clock() {
        use bevy::ecs::system::RunSystemOnce;
        use std::time::Duration;

        const TICK_SECS: f32 = 0.25;

        let mut ambient = live(def(600.0, 1.0, 1), 0.0);
        ambient.auto_run = true;
        let routine = live(def(600.0, 1.0, 1), f32::MAX);

        let mut sim = ParticleSimulator::default();
        sim.generators.push(ambient);
        sim.generators.push(routine);

        let mut world = World::new();
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f32(TICK_SECS));
        world.insert_resource(time);
        world.insert_resource(sim);
        world.run_system_once(tick_particle_simulator).unwrap();

        let sim = world.resource::<ParticleSimulator>();
        let frames = TICK_SECS * ROUTINE_FPS;
        let [ambient, routine] = &sim.generators[..] else {
            panic!("two generators");
        };
        assert_eq!(ambient.age_frames, frames);
        assert_eq!(ambient.age_frames, routine.age_frames);
        assert_eq!(ambient.particles.len(), frames as usize);
        assert_eq!(ambient.particles.len(), routine.particles.len());
    }

    // La Theine's `~1ra` curtain is followCamera with a pure-Y base. The `rai2`/`~1du` sheets are
    // cameraAttachedBasePosition, whose base stays in fixed world axes (research/xi-model-viewer
    // ui/js/particle/runtime.js updateAssociatedPosition).
    #[test]
    fn camera_relative_origins_stay_in_world_axes() {
        let cam_pos = Vec3::new(100.0, 5.0, 200.0);

        let mut curtain = def(60.0, 30.0, 1);
        curtain.camera_relative = true;
        curtain.follow_camera = true;
        curtain.base_position = [0.0, -35.0, 0.0];
        let mut curtain = live(curtain, 0.0);
        curtain.camera_relative = true;
        curtain.vel_basis = Vec3::new(1.0, -1.0, -1.0);

        let mut sheet = def(60.0, 30.0, 1);
        sheet.camera_relative = true;
        sheet.camera_attached_base = true;
        sheet.base_position = [0.0, -10.0, 10.0];
        let mut sheet = live(sheet, 0.0);
        sheet.camera_relative = true;

        let mut sim = ParticleSimulator::default();
        sim.generators.push(curtain);
        sim.generators.push(sheet);

        let moved_cam = cam_pos + Vec3::new(5.0, -3.0, 9.0);
        sim.set_camera_relative_origins(moved_cam);
        assert_eq!(
            sim.generators[0].origin,
            moved_cam + Vec3::new(0.0, 35.0, 0.0)
        );
        assert_eq!(
            sim.generators[1].origin,
            moved_cam + Vec3::new(0.0, -10.0, -10.0)
        );

        sim.set_camera_relative_origins(cam_pos);
        assert_eq!(
            sim.generators[0].origin,
            cam_pos + Vec3::new(0.0, 35.0, 0.0)
        );
        assert_eq!(
            sim.generators[1].origin,
            cam_pos + Vec3::new(0.0, -10.0, -10.0)
        );
    }

    // research/xim Particle.kt updateAssociatedPosition — a cameraAttachedBasePosition particle reads the offset
    // from the camera only at age 0. Re-reading it every frame drags the whole live emission
    // along as one rigid sheet: the dust storm sits pinned in front of the player and swings off
    // screen the moment the camera pitches. New emissions still follow the camera.
    #[test]
    fn camera_attached_particles_keep_their_birth_origin() {
        let mut sheet = def(70.0, 10.0, 1);
        sheet.camera_relative = true;
        sheet.camera_attached_base = true;
        sheet.base_position = [0.0, 0.0, 13.0];
        sheet.init_velocity = [0.0; 3];
        let mut sheet = live(sheet, f32::MAX);
        sheet.camera_relative = true;

        let mut sim = ParticleSimulator::default();
        sim.generators.push(sheet);

        sim.set_camera_relative_origins(Vec3::ZERO);
        advance(&mut sim.generators[0], 10.0);
        let born = sim.generators[0].particles.len();
        assert!(born > 0);

        let moved = Vec3::new(100.0, 0.0, 0.0);
        sim.set_camera_relative_origins(moved);
        advance(&mut sim.generators[0], 10.0);

        let g = &sim.generators[0];
        let clock = CelestialClock::default();
        let worlds: Vec<Vec3> = g
            .particles
            .iter()
            .map(|p| particle_draw(g, p, &clock).world)
            .collect();
        for w in &worlds[..born] {
            assert!(
                (*w - Vec3::new(0.0, 0.0, -13.0)).length() < 1e-3,
                "already-live particle followed the camera: {w}"
            );
        }
        for w in &worlds[born..] {
            assert!(
                (*w - (moved + Vec3::new(0.0, 0.0, -13.0))).length() < 1e-3,
                "new emission did not follow the camera: {w}"
            );
        }
    }

    // La Theine's rain curtain authors 299 particles an emission on a 30-frame period with a
    // 60-frame life. Retail scales that by ~0.3 for everything under weat/, so the steady state
    // is two emissions' worth of drops, not 598.
    #[test]
    fn weather_emit_scale_thins_the_authored_count() {
        const AUTHORED: u32 = 299;
        const SCALED: usize = 89;
        let mut g = live(def(60.0, 30.0, AUTHORED), f32::MAX);
        g.emit_scale = 0.3;
        advance(&mut g, 30.0);
        assert_eq!(g.particles.len(), SCALED);
        advance(&mut g, 30.0);
        assert_eq!(g.particles.len(), 2 * SCALED);
    }

    // Everything outside weat/ keeps its authored count exactly, and an authored count of 0 still
    // emits the one particle retail's `floor(count) + 1` loop gives it.
    #[test]
    fn non_weather_emission_counts_are_unscaled() {
        let mut g = live(def(600.0, 1.0, 5), f32::MAX);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 5);

        let mut g = live(def(600.0, 1.0, 0), f32::MAX);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1);
    }

    // Without the sec2 0x06/0x07 spawn spread every drop of a curtain is emitted on one point.
    #[test]
    fn position_variance_spreads_emissions_through_the_sphere() {
        const RADIUS: f32 = 20.0;
        let mut d = def(60.0, 1.0, 200);
        d.init_velocity = [0.0; 3];
        d.position_variance = Some(ffxi_dat::particle_gen::PositionVariance {
            radius_variance: RADIUS,
            base_radius: 0.0,
            axis_scale: [1.0; 3],
        });
        let mut g = live(d, f32::MAX);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 200);

        let radii: Vec<f32> = g.particles.iter().map(|p| p.pos.length()).collect();
        let max = radii.iter().cloned().fold(0.0, f32::max);
        let centroid = g.particles.iter().map(|p| p.pos).sum::<Vec3>() / g.particles.len() as f32;
        assert!(max > RADIUS * 0.9 && max <= RADIUS, "outer radius {max}");
        assert!(
            radii.iter().filter(|r| **r < RADIUS * 0.5).count() > 20,
            "a constant-radius shell leaves the interior empty"
        );
        assert!(centroid.length() < RADIUS * 0.2, "off-centre: {centroid}");
    }

    // 0x1F SphericalPositionVarianceFull: with a zero tilt the ring lies flat, and the stepped
    // azimuth puts consecutive particles on consecutive steps of the ring (CYyGenerator.cpp
    // CYyGenerator::ElemGenerate case 0x1F — the step indexes the element counter).
    #[test]
    fn spherical_full_stepped_azimuth_walks_the_ring() {
        const STEPS: u32 = 6;
        let mut d = def(60.0, 1.0, STEPS);
        d.init_velocity = [0.0; 3];
        d.spherical_full = Some(ffxi_dat::particle_gen::SphericalPositionVarianceFull {
            radius_variance: 0.0,
            base_radius: 2.0,
            axis_scale: [1.0; 3],
            rotation_z: 0.0,
            rotation_y: 0.0,
            tilt: 0.0,
            tilt_variance: 0.0,
            camera_oriented: false,
            azimuth_steps: STEPS,
        });
        let mut g = live(d, f32::MAX);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), STEPS as usize);

        let mut angles: Vec<f32> = g
            .particles
            .iter()
            .map(|p| {
                let (x, z) = (p.pos.x, p.pos.z);
                let mut a = z.atan2(x);
                if a < 0.0 {
                    a += 2.0 * std::f32::consts::PI;
                }
                a
            })
            .collect();
        angles.sort_by(|a, b| a.total_cmp(b));
        let step = 2.0 * std::f32::consts::PI / STEPS as f32;
        for (i, a) in angles.iter().enumerate() {
            let expected = i as f32 * step;
            assert!(
                (a - expected).abs() < 1e-4,
                "step {i} at {a}, expected {expected}"
            );
        }
    }

    /// The camera flag authors the ring in the camera's frame: the same flat ring that a
    /// non-oriented block puts on the x/z axes rides the camera's rotation instead.
    #[test]
    fn spherical_full_camera_oriented_ring_follows_the_camera() {
        let ring = |camera_oriented: bool| {
            let mut d = def(60.0, 1.0, 2);
            d.init_velocity = [0.0; 3];
            d.spherical_full = Some(ffxi_dat::particle_gen::SphericalPositionVarianceFull {
                radius_variance: 0.0,
                base_radius: 2.0,
                axis_scale: [1.0; 3],
                rotation_z: 0.0,
                rotation_y: 0.0,
                tilt: 0.0,
                tilt_variance: 0.0,
                camera_oriented,
                azimuth_steps: 2,
            });
            let mut g = live(d, f32::MAX);
            g.cam_view = Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2);
            advance(&mut g, 1.0);
            g.particles
                .iter()
                .map(|p| (p.pos.x, p.pos.z))
                .collect::<Vec<_>>()
        };

        let free = ring(false);
        for (x, z) in &free {
            assert!(
                z.abs() < 1e-5,
                "the un-oriented ring stays on the x axis: {x} {z}"
            );
        }

        let oriented = ring(true);
        for (x, z) in &oriented {
            assert!(
                x.abs() < 1e-5,
                "the camera ring moves off the x axis: {x} {z}"
            );
        }
        assert!(
            oriented[0].1 > 0.0 && oriented[1].1 < 0.0,
            "the ring spans the z axis"
        );
    }

    // research/XIClient/src/XIClient/source/Resource/Derived/CMoD3m.cpp ZeroOneTSS. A template
    // colour already carries stage 0's MODULATE2X (the /128 normalise), so an input of 0.25
    // here stands for a retail D of 0.125.
    mod stage_chain {
        use super::*;

        // NonZeroTwoTSS: rgb = 4*D*T*F, alpha = 8*D.a*T.a*F.a, with T left to the sampler.
        #[test]
        fn textured_default_reaches_the_retail_totals_below_saturation() {
            let (rgb, alpha) = d3m_stage_chain(
                Vec3::splat(0.25),
                0.25,
                Vec3::splat(0.25),
                0.25,
                false,
                D3mDrawPath::D3m,
            );
            assert_eq!(rgb, Vec3::splat(4.0 * 0.125 * 0.25));
            assert_eq!(alpha, 8.0 * 0.125 * 0.25);
        }

        // NonZeroOneTSS (renderStateFlags 0x1000): stage 0 selects D.a instead of modulating it
        // with the texture alpha, so the total is 4*D.a*F.a — half the default, rgb untouched.
        #[test]
        fn ignoring_texture_alpha_halves_the_alpha_total() {
            let two = d3m_stage_chain(
                Vec3::splat(0.25),
                0.25,
                Vec3::splat(0.25),
                0.25,
                false,
                D3mDrawPath::D3m,
            );
            let one = d3m_stage_chain(
                Vec3::splat(0.25),
                0.25,
                Vec3::splat(0.25),
                0.25,
                true,
                D3mDrawPath::D3m,
            );
            assert_eq!(one.1, two.1 / 2.0);
            assert_eq!(one.0, two.0);
        }

        // D3D saturates each stage on its own: a 0xFF vertex byte clips at stage 0, so stage 1's
        // MODULATE4X starts from 1.0 instead of carrying the excess through it.
        #[test]
        fn stage_zero_saturates_before_the_stage_one_gain() {
            let vert = u8::MAX as f32 / ffxi_dat::d3m::VERTEX_COLOR_DIVISOR;
            let (rgb, alpha) = d3m_stage_chain(
                Vec3::splat(vert),
                vert,
                Vec3::ONE,
                0.15,
                false,
                D3mDrawPath::D3m,
            );
            assert_eq!(alpha, D3M_STAGE_CLAMP * 0.15 * D3M_STAGE1_ALPHA_GAIN);
            assert_eq!(rgb, Vec3::splat(D3M_STAGE_CLAMP * D3M_STAGE1_RGB_GAIN));
        }

        // Stage 1's own saturation is ffxi_particle.wgsl's, because it has to land after the
        // texel multiply. Clamping the gain away here is what left the D3m 4x/8x MODULATE
        // unable to lift a sub-unit texel to retail's ceiling.
        #[test]
        fn the_stage_one_gain_leaves_the_cpu_unsaturated_for_the_shader_to_clamp() {
            let (rgb, alpha) =
                d3m_stage_chain(Vec3::ONE, 1.0, Vec3::ONE, 1.0, false, D3mDrawPath::D3m);
            assert_eq!(rgb, Vec3::splat(D3M_STAGE1_RGB_GAIN));
            assert_eq!(alpha, D3M_STAGE1_ALPHA_GAIN);
        }

        // ZoneRenderer.cpp ZoneRenderer::DoD3mDraw — with no texture bound the chain is a single
        // MODULATE2X / MODULATE4X stage against TEXTUREFACTOR, so an identity (0x80) vertex under
        // an identity generator colour stays mid-grey rather than doubling to white. Lower
        // Jeuno's sea base plane (`col1` -> tshimonolowcol, untextured) is exactly that case.
        #[test]
        fn an_untextured_mmb_takes_the_one_stage_table() {
            let identity = 0x80 as f32 / ffxi_dat::d3m::VERTEX_COLOR_DIVISOR;
            let factor = 0x80 as f32 / u8::MAX as f32;
            let (rgb, alpha) = d3m_stage_chain(
                Vec3::splat(identity),
                identity,
                Vec3::splat(factor),
                factor,
                false,
                D3mDrawPath::MmbUntextured,
            );
            assert!((rgb.x - identity * factor).abs() < 1e-6, "rgb {rgb}");
            assert!(
                (alpha - identity * factor * 2.0).abs() < 1e-6,
                "alpha {alpha}"
            );
            let (textured, _) = d3m_stage_chain(
                Vec3::splat(identity),
                identity,
                Vec3::splat(factor),
                factor,
                false,
                D3mDrawPath::Mmb,
            );
            assert!((textured.x - rgb.x * 2.0).abs() < 1e-6);
            let (_, forced) = d3m_stage_chain(
                Vec3::splat(identity),
                identity,
                Vec3::splat(factor),
                factor,
                true,
                D3mDrawPath::MmbUntextured,
            );
            assert_eq!(forced, alpha);
        }

        // CMoD3mElem.cpp CMoD3mElem::DoMMBDraw — DoMMBDraw forces the ignore-texture-alpha table at blend byte
        // 0x64; CMoD3m::Draw has no such override.
        #[test]
        fn blend_byte_64_forces_the_one_tss_table_on_the_mmb_path_only() {
            let mut d = def(1.0, 1.0, 1);
            d.blend_byte = D3M_MMB_FORCE_IGNORE_TEXTURE_ALPHA_BLEND_BYTE;
            assert!(ignores_texture_alpha(&d, D3mDrawPath::Mmb));
            assert!(!ignores_texture_alpha(&d, D3mDrawPath::D3m));
            d.blend_byte = 0x03;
            assert!(!ignores_texture_alpha(&d, D3mDrawPath::Mmb));
            d.ignore_texture_alpha = true;
            assert!(ignores_texture_alpha(&d, D3mDrawPath::D3m));
        }

        // CMoD3m.cpp — blend byte 0x44 only, and only on the CMoD3m::Draw path.
        #[test]
        fn tfactor_alpha_promotes_at_half_only_for_blend_byte_44() {
            let promote = |byte: u8, path: D3mDrawPath, a: f32| {
                let mut d = def(1.0, 1.0, 1);
                d.blend_byte = byte;
                tfactor_alpha(&d, path, a)
            };
            let just_under = 0x7E as f32 / u8::MAX as f32;
            let at_threshold = 0x7F as f32 / u8::MAX as f32;
            assert_eq!(promote(0x44, D3mDrawPath::D3m, at_threshold), 1.0);
            assert_eq!(promote(0x44, D3mDrawPath::D3m, just_under), just_under);
            assert_eq!(promote(0x44, D3mDrawPath::Mmb, at_threshold), at_threshold);
            assert_eq!(promote(0x03, D3mDrawPath::D3m, at_threshold), at_threshold);
        }

        fn vertex_colors(g: &LiveGenerator) -> Vec<[f32; 4]> {
            let mut mesh = empty_mesh();
            rebuild_mesh(
                g,
                view(Quat::IDENTITY),
                &CelestialClock::default(),
                &mut mesh,
            );
            match mesh.attribute(Mesh::ATTRIBUTE_COLOR) {
                Some(bevy::mesh::VertexAttributeValues::Float32x4(v)) => v.clone(),
                _ => panic!("expected Float32x4 vertex colours"),
            }
        }

        // One particle at half life, where the untracked alpha curve gives F.a = 0.5.
        fn half_life_gen(blend: ffxi_dat::particle_gen::ParticleBlend, byte: u8) -> LiveGenerator {
            let mut d = def(100.0, 1.0, 1);
            d.blend = blend;
            d.blend_byte = byte;
            d.init_color = [1.0, 1.0, 1.0, 1.0];
            let mut g = live(d, 100.0);
            set_template_color(&mut g, Vec3::ONE.extend(0.5));
            g.particles.push(Particle {
                pos: Vec3::ZERO,
                spawn_origin: Vec3::ZERO,
                vel: Vec3::ZERO,
                age_frames: 50.0,
                life_frames: 100.0,
                rgb: Vec3::ONE,
                scale: Vec2::ONE,
                scale_seed: Vec2::ONE,
                scale_vel: Vec2::ZERO,
                rotation: Vec3::ZERO,
                spin: Vec3::ZERO,
                negate_rotation_y: false,
                rel_vel: Vec3::ZERO,
                osc: None,
            });
            g
        }

        #[test]
        fn blended_particle_carries_the_stage_one_rgb_gain() {
            let mut g = half_life_gen(ffxi_dat::particle_gen::ParticleBlend::Blend, 0x03);
            set_template_color(&mut g, Vec3::splat(0.25).extend(0.5));
            for c in vertex_colors(&g) {
                assert_eq!([c[0], c[1], c[2]], [0.5, 0.5, 0.5]);
            }
        }

        #[test]
        fn blended_particle_alpha_scales_with_vertex_alpha() {
            let mut g = half_life_gen(ffxi_dat::particle_gen::ParticleBlend::Blend, 0x03);
            set_template_color(&mut g, Vec3::ONE.extend(0.25));
            for c in vertex_colors(&g) {
                assert_eq!(c[3], 0.5);
            }
        }

        // The 0x44 promotion lifts F.a 0.5 -> 1.0 before the stage math.
        #[test]
        fn blend_byte_44_promotes_the_particle_alpha() {
            let mut g = half_life_gen(ffxi_dat::particle_gen::ParticleBlend::Blend, 0x44);
            set_template_color(&mut g, Vec3::ONE.extend(0.125));
            let promoted = vertex_colors(&g)[0][3];
            g.def.blend_byte = 0x03;
            let unpromoted = vertex_colors(&g)[0][3];
            assert_eq!(promoted, 0.5);
            assert_eq!(unpromoted, 0.25);
        }

        // An additive element hands the life curve to the blend state as src alpha instead of
        // pre-multiplying it into rgb, so the shader's premultiply applies it to the colour
        // stage 1 already saturated — retail's order.
        #[test]
        fn additive_particle_carries_the_life_curve_as_src_alpha() {
            let mut g = half_life_gen(ffxi_dat::particle_gen::ParticleBlend::Additive, 0x48);
            set_template_color(&mut g, Vec3::splat(0.25).extend(0.5));
            for c in vertex_colors(&g) {
                assert_eq!([c[0], c[1], c[2]], [0.5, 0.5, 0.5]);
                assert_eq!(c[3], 0.5);
            }
        }

        // The home point's `sil` curtain (ROM/3/25.DAT) authors its plume as a per-vertex
        // white -> purple -> black ramp up each strip. Folding the stage chain once per
        // particle instead of once per vertex drew the whole strip at the first vertex's
        // white, which is what made the rising streaks read as lit rectangles with no purple
        // and no fade-out at the top.
        #[test]
        fn a_template_colour_gradient_survives_into_the_mesh() {
            const WHITE: Vec4 = Vec4::ONE;
            const PURPLE: Vec4 = Vec4::new(0.26, 0.25, 0.49, 1.0);
            const BLACK: Vec4 = Vec4::new(0.0, 0.0, 0.0, 1.0);

            let mut g = half_life_gen(ffxi_dat::particle_gen::ParticleBlend::Additive, 0x48);
            g.template.colors = vec![WHITE, PURPLE, BLACK];

            let drawn = vertex_colors(&g);
            assert_eq!(drawn.len(), 3, "one colour per template vertex");
            assert!(drawn[0][0] > drawn[1][0], "white end outshines the purple");
            assert!(
                drawn[1][2] > drawn[1][0] && drawn[1][2] > drawn[1][1],
                "the purple vertex stays blue-dominant: {:?}",
                drawn[1]
            );
            assert_eq!(
                [drawn[2][0], drawn[2][1], drawn[2][2]],
                [0.0, 0.0, 0.0],
                "the black end adds nothing, so an additive plume fades out"
            );
        }

        // The life curve is the raw one, not the saturating stage-1 alpha — that would hold an
        // additive spray at full brightness until the last quarter of its life.
        #[test]
        fn additive_brightness_still_fades_late_in_life() {
            let mut g = half_life_gen(ffxi_dat::particle_gen::ParticleBlend::Additive, 0x48);
            g.particles[0].age_frames = 90.0;
            let late = vertex_colors(&g)[0][3];
            assert!((late - (1.0 - 0.9f32)).abs() < 1e-6, "{late}");
        }
    }

    // The Home Point crystal `bnd0`: 0x0B (0, -0.0157, 0) with the sec3 0x05 updater turns the
    // particle by that much every 60 Hz frame; the rate alone must not
    // (research/xim ParticleGeneratorParser.kt RotationUpdater).
    #[test]
    fn spin_integrates_only_with_the_rotation_updater() {
        const YAW_PER_FRAME: f32 = -0.0157;
        let mut d = def(120.0, 1.0, 1);
        d.camera_billboard = false;
        d.continuous = true;
        d.init_rotation = [0.0, 0.5, 0.0];
        d.rotation_velocity = Some([0.0, YAW_PER_FRAME, 0.0]);
        d.rotation_updater = true;
        let mut g = live(d, 1000.0);
        g.orientation = particle_orientation(&g.def);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1);
        assert_eq!(
            g.particles[0].rotation,
            Vec3::new(0.0, 0.5, 0.0),
            "born at the 0x09 seed"
        );
        advance(&mut g, 10.0);
        let yaw = g.particles[0].rotation.y;
        assert!((yaw - (0.5 + 10.0 * YAW_PER_FRAME)).abs() < 1e-5, "{yaw}");

        let mut still = g.def;
        still.rotation_updater = false;
        let mut g = live(still, 1000.0);
        advance(&mut g, 1.0);
        advance(&mut g, 10.0);
        assert_eq!(g.particles[0].rotation, Vec3::new(0.0, 0.5, 0.0));
    }

    // 0x0C VelocityVarianceSetup (rotation): every emitted particle draws a uniform [-v, v]
    // offset per axis on top of the 0x0B spin rate (research/xim ParticleInitializers.kt
    // VelocityVarianceSetup — the allocationOffset binds it to the rotation transform; retail's
    // shared 0x03/0x0C/0x13 case adds frand(bounds) to the transform's velocity). The burst is
    // held (stopped) so the 10-frame tick ages only the original eight, not the re-emissions it
    // would otherwise add behind them.
    #[test]
    fn rotation_velocity_variance_spreads_the_spin_per_particle() {
        let mut d = def(120.0, 1.0, 8);
        d.camera_billboard = false;
        d.rotation_velocity = Some([0.0, 0.01, 0.0]);
        d.rotation_velocity_variance = Some([0.0, 0.005, 0.0]);
        d.rotation_updater = true;
        let mut g = live(d, 1000.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 8, "one burst of eight");
        g.stopped = true;
        advance(&mut g, 10.0);
        let mut yaws = Vec::new();
        for p in &g.particles {
            let yaw = p.rotation.y;
            yaws.push(yaw);
            assert!(
                (0.05f32 - 1e-6..=0.15f32 + 1e-6).contains(&yaw),
                "spin out of band: {yaw}"
            );
        }
        assert!(
            yaws.windows(2).any(|w| w[0] != w[1]),
            "the variance must differ between particles: {yaws:?}"
        );

        let mut no_updater = d;
        no_updater.rotation_updater = false;
        let mut g = live(no_updater, 1000.0);
        advance(&mut g, 1.0);
        advance(&mut g, 10.0);
        for p in &g.particles {
            assert_eq!(p.rotation, Vec3::ZERO, "no rotation updater, no spin");
        }
    }

    // 0x13 VelocityVarianceSetup (scale): every emitted particle draws a uniform [-v, v]
    // offset per axis on top of the 0x12 scale velocity (research/xim
    // ParticleInitializers.kt VelocityVarianceSetup — the allocationOffset binds it to the
    // scale transform; retail's shared 0x03/0x0C/0x13 case adds frand(bounds) to the
    // transform's velocity). The burst is held (stopped) so the 10-frame tick ages only the
    // original eight, not the re-emissions it would otherwise add behind them.
    #[test]
    fn scale_velocity_variance_spreads_the_rate_per_particle() {
        let mut d = def(120.0, 1.0, 8);
        d.scale_velocity = Some([0.0, 0.01, 0.0]);
        d.scale_velocity_variance = Some([0.0, 0.005, 0.0]);
        d.scale_updater = true;
        let mut g = live(d, 1000.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 8, "one burst of eight");
        g.stopped = true;
        advance(&mut g, 10.0);
        let mut scales = Vec::new();
        for p in &g.particles {
            scales.push(p.scale.y);
            assert!(
                (0.15f32 - 1e-6..=0.25f32 + 1e-6).contains(&p.scale.y),
                "scale out of band: {}",
                p.scale.y
            );
        }
        assert!(
            scales.windows(2).any(|w| w[0] != w[1]),
            "the variance must differ between particles: {scales:?}"
        );

        let mut no_updater = d;
        no_updater.scale_updater = false;
        let mut g = live(no_updater, 1000.0);
        advance(&mut g, 1.0);
        advance(&mut g, 10.0);
        for p in &g.particles {
            assert_eq!(p.scale.y, 0.1, "no scale updater, no growth");
        }
    }

    // 0x11 SingleScaleVarianceInitializer: every emitted particle draws one [0, v) offset
    // shared by the scale axes, on top of the 0x0F base (research/xim ParticleInitializers.kt
    // — scale += posRand(v); retail's ElemGenerate case 0x11 adds a single ufrand to x, y, z).
    #[test]
    fn single_scale_variance_spreads_the_scale_per_particle() {
        let mut d = def(120.0, 1.0, 8);
        d.single_scale_variance = Some(0.05);
        let mut g = live(d, 1000.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 8, "one burst of eight");
        let mut scales = Vec::new();
        for p in &g.particles {
            scales.push(p.scale.x);
            assert!(
                (0.1f32..=0.15f32 + 1e-6).contains(&p.scale.x),
                "scale out of band: {}",
                p.scale.x
            );
            assert_eq!(p.scale.x, p.scale.y, "one draw feeds both axes");
        }
        assert!(
            scales.windows(2).any(|w| w[0] != w[1]),
            "the variance must differ between particles: {scales:?}"
        );
    }

    // 0x10 ScaleVarianceInitializer: each scale axis draws its own [0, v) offset on top of
    // the 0x0F base, so the axes decorrelate (CYyGenerator.cpp
    // CYyGenerator::ElemGenerate case 0x10 — field_EC.x/y/z += ufrand(payload)).
    #[test]
    fn scale_variance_spreads_each_axis_independently() {
        let mut d = def(120.0, 1.0, 16);
        d.scale_variance = Some([0.2, 0.1, 0.0]);
        let mut g = live(d, 1000.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 16, "one burst of sixteen");
        for p in &g.particles {
            assert!(
                (0.1f32..=0.3f32 + 1e-6).contains(&p.scale.x),
                "x scale out of band: {}",
                p.scale.x
            );
            assert!(
                (0.1f32..=0.2f32 + 1e-6).contains(&p.scale.y),
                "y scale out of band: {}",
                p.scale.y
            );
        }
        assert!(
            g.particles
                .iter()
                .any(|p| (p.scale.x - 0.1) / 0.2 != (p.scale.y - 0.1) / 0.1),
            "the axis draws must be independent"
        );
    }

    #[test]
    fn transform_acceleration_does_not_translate_particles() {
        let mut d = def(120.0, 1.0, 1);
        d.init_velocity = [0.0; 3];
        d.rotation_velocity = Some([0.0; 3]);
        d.scale_velocity = Some([0.0; 3]);
        d.rotation_updater = true;
        d.scale_updater = true;
        d.rotation_accel = Some([0.0, 0.0, 0.01]);
        d.scale_accel = Some([0.01, 0.0, 0.0]);
        let mut g = live(d, 1000.0);
        advance(&mut g, 1.0);
        g.stopped = true;
        let initial = g.particles[0].pos;
        advance(&mut g, 1.0);
        advance(&mut g, 1.0);
        let p = &g.particles[0];
        assert_eq!(p.pos, initial);
        assert!((p.rotation.z - 0.01).abs() < 1e-6);
        assert!((p.scale.x - 0.11).abs() < 1e-6);
    }

    // 0x12 ScaleVelocitySetup: the sec3 0x08 ScaleUpdater adds the per-axis rate every frame
    // (research/xim ParticleUpdaters.kt — scale += velocity × elapsedFrames); a rate without
    // the updater stays inert, and the track seed keeps the spawn-time scale. The burst is
    // held (stopped) so the 10-frame tick ages only the original eight, not the re-emissions it
    // would otherwise add behind them.
    #[test]
    fn scale_velocity_grows_the_scale_per_frame() {
        let mut d = def(120.0, 1.0, 8);
        d.scale_velocity = Some([0.0, 0.01, 0.0]);
        d.scale_updater = true;
        let mut g = live(d, 1000.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 8, "one burst of eight");
        g.stopped = true;
        advance(&mut g, 10.0);
        for p in &g.particles {
            assert_eq!(p.scale.x, 0.1, "no x rate, no x growth");
            let sy = p.scale.y;
            assert!((sy - 0.2).abs() < 1e-5, "10 frames at 0.01: {sy}");
            assert_eq!(p.scale_seed, Vec2::new(0.1, 0.1), "the seed stays at spawn");
        }

        let mut no_updater = d;
        no_updater.scale_updater = false;
        let mut g = live(no_updater, 1000.0);
        advance(&mut g, 1.0);
        advance(&mut g, 10.0);
        for p in &g.particles {
            assert_eq!(p.scale, Vec2::new(0.1, 0.1), "no scale updater, no growth");
        }
    }

    // CYyGenerator.cpp CYyGenerator::ElemDie case 5 — a relife generator keeps its element past
    // its life, so the spin accumulated over the first cycle survives into the next instead of
    // snapping back to the seed with a fresh particle.
    #[test]
    fn relife_keeps_the_particle_and_its_accumulated_rotation() {
        let mut d = def(120.0, 1.0, 1);
        d.camera_billboard = false;
        d.continuous = true;
        d.rotation_velocity = Some([0.0, 0.01, 0.0]);
        d.rotation_updater = true;
        d.relife_on_expiry = true;
        let mut g = live(d, 1000.0);
        g.auto_run = true;
        advance(&mut g, 1.0);
        for _ in 0..30 {
            advance(&mut g, 5.0);
        }
        assert_eq!(g.particles.len(), 1, "one element, never re-emitted");
        let p = &g.particles[0];
        assert!(p.age_frames < p.life_frames, "life was reset, not run out");
        assert!(
            (p.rotation.y - 1.5).abs() < 1e-4,
            "150 frames of spin: {}",
            p.rotation.y
        );

        let mut mortal = g.def;
        mortal.relife_on_expiry = false;
        let mut g = live(mortal, 1000.0);
        g.auto_run = true;
        advance(&mut g, 1.0);
        for _ in 0..30 {
            advance(&mut g, 5.0);
        }
        assert_eq!(g.particles.len(), 1);
        assert!(
            g.particles[0].rotation.y < 1.0,
            "the re-emitted particle restarted its spin"
        );
    }

    /// A turning particle changes the built mesh, so the rebuild key has to follow its
    /// rotation.
    #[test]
    fn rotation_changes_the_mesh_key() {
        let mut d = def(120.0, 1.0, 1);
        d.camera_billboard = false;
        d.continuous = true;
        d.rotation_velocity = Some([0.0, 0.1, 0.0]);
        d.rotation_updater = true;
        let mut g = live(d, 1000.0);
        g.orientation = particle_orientation(&g.def);
        advance(&mut g, 1.0);
        let cam = CameraView {
            rot: Quat::IDENTITY,
            pos: Vec3::ZERO,
        };
        let clock = CelestialClock::default();
        let before = mesh_key(&g, cam, &clock);
        advance(&mut g, 1.0);
        let after = mesh_key(&g, cam, &clock);
        assert!(needs_rebuild(&before, &after));
    }

    #[test]
    fn mesh_is_never_zero_length() {
        // Bevy's MeshAllocator errors on a zero-length vertex buffer, so an
        // empty generator (fresh spawn / between emit windows) must still
        // upload a non-empty mesh. Covers empty_mesh() and the empty rebuild.
        let count = |m: &Mesh| m.count_vertices();
        assert!(
            count(&empty_mesh()) > 0,
            "empty_mesh must not be zero-length"
        );

        let g = live(def(2.0, 1.0, 1), 3.0);
        assert!(g.particles.is_empty());
        let mut mesh = empty_mesh();
        rebuild_mesh(
            &g,
            view(Quat::IDENTITY),
            &CelestialClock::default(),
            &mut mesh,
        );
        assert!(count(&mesh) > 0, "empty rebuild must not be zero-length");
    }

    // sec2 0x3D + 0x3E with the sec3 0x29 applier: the particle's x position sways with the
    // applier's amplitude curve (research/xim ParticleUpdaters.kt OscillationApplier —
    // rate = 180f / 2 = 90, baseOffset 0, so the amplitude peaks at half a period, 90 frames,
    // and returns to zero at the full period).
    #[test]
    fn oscillation_applier_x_oscillates_the_position() {
        let mut d = def(1000.0, 1.0, 1);
        d.oscillation = true;
        d.oscillation_accel_x = Some([0.5, 0.0]);
        d.oscillation_applier_x = Some([2.0, 0.0, 0.0]);
        let mut g = live(d, 1000.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1, "one particle");
        g.stopped = true;
        for _ in 0..90 {
            advance(&mut g, 1.0);
        }
        let x_peak = g.particles[0].pos.x;
        assert!((x_peak - 22.5).abs() < 1e-2, "half-period peak: {x_peak}");
        for _ in 0..90 {
            advance(&mut g, 1.0);
        }
        let x_full = g.particles[0].pos.x;
        assert!(x_full.abs() < 1e-2, "full-period return: {x_full}");
    }

    // sec3 0x2B OscillationApplier (Z): the z position sways with the amplitude curve
    // (research/xim ParticleGeneratorParser.kt OscillationApplier). The default generator is
    // world-space, so the FFXI +Z unit hat lands on Bevy −Z through the (x, −y, −z) basis —
    // the peak is negative.
    #[test]
    fn oscillation_applier_z_oscillates_the_position() {
        let mut d = def(1000.0, 1.0, 1);
        d.oscillation = true;
        d.oscillation_accel_z = Some([0.5, 0.0]);
        d.oscillation_applier_z = Some([2.0, 0.0, 0.0]);
        let mut g = live(d, 1000.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1, "one particle");
        g.stopped = true;
        for _ in 0..90 {
            advance(&mut g, 1.0);
        }
        let z_peak = g.particles[0].pos.z;
        assert!((z_peak + 22.5).abs() < 1e-2, "half-period peak: {z_peak}");
        for _ in 0..90 {
            advance(&mut g, 1.0);
        }
        let z_full = g.particles[0].pos.z;
        assert!(z_full.abs() < 1e-2, "full-period return: {z_full}");
    }

    // sec3 0x2A OscillationApplier (Y): the y position sways with the amplitude curve
    // (research/xim ParticleGeneratorParser.kt OscillationApplier). The default generator is
    // world-space, so the FFXI +Y unit hat lands on Bevy −Y through the (x, −y, −z) basis —
    // the peak is negative. The base velocity is zeroed so its drift does not mix into the
    // asserted axis.
    #[test]
    fn oscillation_applier_y_oscillates_the_position() {
        let mut d = def(1000.0, 1.0, 1);
        d.init_velocity = [0.0; 3];
        d.oscillation = true;
        d.oscillation_accel_y = Some([0.5, 0.0]);
        d.oscillation_applier_y = Some([2.0, 0.0, 0.0]);
        let mut g = live(d, 1000.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1, "one particle");
        g.stopped = true;
        for _ in 0..90 {
            advance(&mut g, 1.0);
        }
        let y_peak = g.particles[0].pos.y;
        assert!((y_peak + 22.5).abs() < 1e-2, "half-period peak: {y_peak}");
        for _ in 0..90 {
            advance(&mut g, 1.0);
        }
        let y_full = g.particles[0].pos.y;
        assert!(y_full.abs() < 1e-2, "full-period return: {y_full}");
    }

    // The 0x3E acceleration without the sec3 0x29 applier is parsed but never moves the
    // particle (research/xim ParticleUpdaters.kt — the applier is the only integrator).
    #[test]
    fn oscillation_without_the_applier_stays_inert() {
        let mut d = def(1000.0, 1.0, 1);
        d.oscillation = true;
        d.oscillation_accel_x = Some([0.5, 0.0]);
        let mut g = live(d, 1000.0);
        advance(&mut g, 1.0);
        g.stopped = true;
        for _ in 0..180 {
            advance(&mut g, 1.0);
        }
        let p = &g.particles[0];
        assert_eq!(p.pos.x, 0.0, "no applier, no x motion");
    }

    // With a relative velocity the applier moves the particle along it, not along the axis
    // (research/xim ParticleUpdaters.kt getOscillationDirection — the X axis follows the
    // relative-velocity direction).
    #[test]
    fn oscillation_applier_x_follows_the_relative_velocity_direction() {
        let mut d = def(1000.0, 1.0, 1);
        d.oscillation_applier_x = Some([2.0, 0.0, 0.0]);
        let mut g = live(d, 1000.0);
        g.stopped = true;
        g.particles.push(Particle {
            pos: Vec3::ZERO,
            spawn_origin: Vec3::ZERO,
            vel: Vec3::ZERO,
            age_frames: 0.0,
            life_frames: 1000.0,
            rgb: Vec3::ONE,
            scale: Vec2::ONE,
            scale_seed: Vec2::ONE,
            scale_vel: Vec2::ZERO,
            rotation: Vec3::ZERO,
            spin: Vec3::ZERO,
            negate_rotation_y: false,
            rel_vel: Vec3::Y,
            osc: Some(Oscillation {
                accel: [0.5, 0.0, 0.0],
                prev_amplitude: [0.0; 3],
            }),
        });
        for _ in 0..90 {
            advance(&mut g, 1.0);
        }
        let p = &g.particles[0];
        assert!(
            (p.pos.y - 22.5).abs() < 1e-2,
            "along the relative velocity: {}",
            p.pos.y
        );
        assert_eq!(p.pos.x, 0.0, "no x motion");
    }

    // kuluu-b5nt: rebuild_mesh only fires when its quantized inputs differ from the last BUILT
    // mesh, so a tracked get_mut (AssetEvent::Modified, a full GPU re-upload) stops scaling
    // with fps.
    mod rebuild_skip {
        use super::*;

        fn one_particle_gen() -> LiveGenerator {
            let mut g = live(def(100.0, 1.0, 1), 100.0);
            g.particles.push(Particle {
                pos: Vec3::new(1.0, 2.0, 3.0),
                spawn_origin: Vec3::ZERO,
                vel: Vec3::ZERO,
                age_frames: 50.0,
                life_frames: 100.0,
                rgb: Vec3::ONE,
                scale: Vec2::ONE,
                scale_seed: Vec2::ONE,
                scale_vel: Vec2::ZERO,
                rotation: Vec3::ZERO,
                spin: Vec3::ZERO,
                negate_rotation_y: false,
                rel_vel: Vec3::ZERO,
                osc: None,
            });
            g
        }

        #[test]
        fn idle_generator_never_rebuilds_whatever_the_camera_does() {
            let mut g = live(def(100.0, 1.0, 1), 100.0);
            assert!(g.particles.is_empty());
            g.tex_translate = Vec2::new(3.7, -1.2);
            for rot in [
                Quat::IDENTITY,
                Quat::from_rotation_y(1.3),
                Quat::from_rotation_x(-0.4),
            ] {
                assert!(!needs_rebuild(
                    &g.built_key,
                    &mesh_key(&g, view(rot), &CelestialClock::default())
                ));
            }
        }

        #[test]
        fn sub_quantum_motion_skips() {
            let mut g = one_particle_gen();
            let built = mesh_key(&g, view(Quat::IDENTITY), &CelestialClock::default());
            g.particles[0].pos.x += MESH_KEY_SPATIAL_QUANTUM * 0.25;
            assert!(!needs_rebuild(
                &built,
                &mesh_key(&g, view(Quat::IDENTITY), &CelestialClock::default())
            ));
        }

        #[test]
        fn super_quantum_motion_rebuilds() {
            let mut g = one_particle_gen();
            let built = mesh_key(&g, view(Quat::IDENTITY), &CelestialClock::default());
            g.particles[0].pos.x += MESH_KEY_SPATIAL_QUANTUM * 2.0;
            assert!(needs_rebuild(
                &built,
                &mesh_key(&g, view(Quat::IDENTITY), &CelestialClock::default())
            ));
        }

        // Ageing feeds the untracked additive life curve through tfactor_alpha and the D3m
        // stage chain into the key's colour, so an alpha change alone dirties the mesh.
        #[test]
        fn alpha_stage_change_rebuilds() {
            let mut g = one_particle_gen();
            let built = mesh_key(&g, view(Quat::IDENTITY), &CelestialClock::default());
            g.particles[0].age_frames = 90.0;
            assert!(needs_rebuild(
                &built,
                &mesh_key(&g, view(Quat::IDENTITY), &CelestialClock::default())
            ));
        }

        #[test]
        fn camera_rotation_rebuilds_a_live_billboard() {
            let g = one_particle_gen();
            let built = mesh_key(&g, view(Quat::IDENTITY), &CelestialClock::default());
            assert!(needs_rebuild(
                &built,
                &mesh_key(
                    &g,
                    view(Quat::from_rotation_y(0.5)),
                    &CelestialClock::default()
                )
            ));
        }

        #[test]
        fn uv_scroll_change_rebuilds() {
            let mut g = one_particle_gen();
            let built = mesh_key(&g, view(Quat::IDENTITY), &CelestialClock::default());
            g.tex_translate.x += MESH_KEY_SPATIAL_QUANTUM * 2.0;
            assert!(needs_rebuild(
                &built,
                &mesh_key(&g, view(Quat::IDENTITY), &CelestialClock::default())
            ));
        }
    }

    // kuluu-czc6: a fixed-orientation zone sheet (e.g. the Lower Jeuno fountain
    // "sibj" cascade) carries raw FFXI-frame geometry extending local +Y (FFXI
    // down). rebuild_mesh must flip it through the generator's mzb->bevy vel_basis
    // so the sheet hangs DOWN from the emitter (Bevy -Y), not up above it. A camera
    // billboard (orientation None) must NOT be flipped — it orients in Bevy already.
    fn sheet_gen(orientation: Option<Quat>) -> LiveGenerator {
        let mut d = def(100.0, 1.0, 1);
        d.camera_billboard = orientation.is_none();
        d.init_scale = [1.0, 1.0, 1.0];
        let mut g = live(d, 5.0);
        // Flat quad extending local +Y (FFXI down), like the sibj water sheet.
        g.template.positions = vec![
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 4.0, 0.0),
        ];
        g.template.uvs = vec![[0.0, 0.0]; 3];
        g.template.indices = vec![0, 1, 2];
        g.origin = Vec3::new(0.0, 10.0, 0.0);
        g.orientation = orientation;
        g.actor_local = false;
        g.vel_basis = Vec3::new(1.0, -1.0, -1.0);
        emit(&mut g, 100.0);
        g
    }

    fn max_sheet_y(mesh: &Mesh) -> f32 {
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(pos)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("no positions");
        };
        // Ignore the far-below hidden primitive push_hidden_primitive leaves when needed.
        pos.iter()
            .map(|p| p[1])
            .filter(|y| *y > -1.0e6)
            .fold(f32::MIN, f32::max)
    }

    #[test]
    fn fixed_orientation_sheet_hangs_below_emitter() {
        let g = sheet_gen(Some(Quat::IDENTITY));
        let mut mesh = empty_mesh();
        rebuild_mesh(
            &g,
            view(Quat::IDENTITY),
            &CelestialClock::default(),
            &mut mesh,
        );
        // Local +Y (0..4) flipped through vel_basis -> Bevy -Y, so every sheet vertex
        // sits at or below the emit origin (y=10); none stand above it.
        assert!(
            max_sheet_y(&mesh) <= 10.0 + 1.0e-4,
            "fixed sheet vertices must not rise above the emitter (kuluu-czc6)"
        );
    }

    /// The campfire ribbon hi12 rises toward DAT -y, so a world-space screen billboard folds
    /// the FFXI->Bevy basis into its template, or the flame hangs below its wick.
    #[test]
    fn camera_billboard_sheet_flipped_into_the_bevy_frame() {
        let g = sheet_gen(None);
        let mut mesh = empty_mesh();
        rebuild_mesh(
            &g,
            view(Quat::IDENTITY),
            &CelestialClock::default(),
            &mut mesh,
        );
        assert!(
            max_sheet_y(&mesh) <= 10.0 + 1.0e-4,
            "a screen billboard's DAT +Y (down) must not rise above the emitter"
        );
    }

    #[test]
    fn emits_one_per_period_over_window() {
        let mut g = live(def(100.0, 5.0, 1), 20.0);
        // 20 frames at 1/frame, period 5 -> 4 emits within window (the emit at accum reset).
        for _ in 0..20 {
            advance(&mut g, 1.0);
        }
        assert_eq!(g.particles.len(), 4);
    }

    #[test]
    fn stops_emitting_after_window() {
        let mut g = live(def(2.0, 1.0, 1), 3.0);
        for _ in 0..10 {
            advance(&mut g, 1.0);
        }
        // window 3 -> ~3 emitted, each lives 2 frames, all expired by frame 10.
        assert!(g.particles.is_empty());
    }

    // research/xim EffectRoutineParser.kt parseSection2 StopParticleGeneratorRoutine: the cast aura's
    // authored emit window is 1800 frames (60 s), so retail's 0x2D stop is what ends it at the
    // end of the cast — emission ceases at once, live particles still play out their life.
    #[test]
    fn stopped_generator_ceases_emission_but_keeps_live_particles() {
        const LIFE_FRAMES: f32 = 10.0;
        const LONG_WINDOW_FRAMES: f32 = 1800.0;

        let mut sim = ParticleSimulator::default();
        let owner = Entity::from_raw_u32(7).unwrap();
        let mut g = live(def(LIFE_FRAMES, 1.0, 1), LONG_WINDOW_FRAMES);
        g.origin_routine = Some(RoutineOrigin {
            owner,
            gen_id: *b"gn10",
            routine: *b"cabk",
        });
        sim.generators.push(g);

        for _ in 0..5 {
            advance_generator(&mut sim.generators[0], 1.0);
        }
        let live_at_stop = sim.generators[0].particles.len();
        assert!(live_at_stop > 0, "generator emits inside its window");

        sim.stop_generator(owner, *b"gn10");
        advance_generator(&mut sim.generators[0], 1.0);
        assert_eq!(
            sim.generators[0].particles.len(),
            live_at_stop,
            "a stopped generator emits nothing new"
        );
        assert!(
            sim.generators[0].particles[0].age_frames > 0.0,
            "already-live particles keep ageing"
        );

        for _ in 0..LIFE_FRAMES as u32 {
            advance_generator(&mut sim.generators[0], 1.0);
        }
        assert!(
            sim.generators[0].particles.is_empty(),
            "live particles finish their lifetime and none replace them"
        );
    }

    // 0x11 AssociationUpdater: retail re-snaps the associated position to the attach actor's
    // position every frame (research/xim ParticleGeneratorAttachment.kt updateAssociatedPosition),
    // so the live origin tracks the actor instead of staying at the spawn-time position.
    #[test]
    fn attached_generator_origin_tracks_the_actor_while_the_effect_runs() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        let owner = world.spawn(Transform::from_translation(Vec3::ZERO)).id();
        let mut d = def(600.0, 1.0, 1);
        d.association = Some(ffxi_dat::particle_gen::AssociationFollow {
            follow_position: true,
            follow_facing: false,
            factor: 255,
        });
        let mut g = live(d, 0.0);
        g.origin_routine = Some(RoutineOrigin {
            owner,
            gen_id: *b"gn10",
            routine: *b"cabk",
        });
        g.origin = Vec3::new(9.0, 0.0, 0.0);
        let mut sim = ParticleSimulator::default();
        sim.generators.push(g);
        world.insert_resource(sim);

        world.run_system_once(track_attached_origins).unwrap();
        assert_eq!(
            world.resource::<ParticleSimulator>().generators[0].origin,
            Vec3::new(0.0, 0.5, 0.0),
            "the origin snaps to the actor's position plus base height"
        );

        *world.get_mut::<Transform>(owner).unwrap() =
            Transform::from_translation(Vec3::new(3.0, 0.0, -4.0));
        world.run_system_once(track_attached_origins).unwrap();
        assert_eq!(
            world.resource::<ParticleSimulator>().generators[0].origin,
            Vec3::new(3.0, 0.5, -4.0),
            "and keeps tracking as the actor moves"
        );
    }

    // Without the 0x11 updater the origin stays where the spawn path put it - retail's handler
    // only runs for defs that carry the updater (research/xim
    // ParticleGeneratorParser.kt AssociationUpdater).
    #[test]
    fn generator_without_the_association_updater_keeps_its_spawn_origin() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        let owner = world
            .spawn(Transform::from_translation(Vec3::new(3.0, 0.0, 0.0)))
            .id();
        let g = {
            let d = def(600.0, 1.0, 1);
            let mut g = live(d, 0.0);
            g.origin_routine = Some(RoutineOrigin {
                owner,
                gen_id: *b"gn10",
                routine: *b"cabk",
            });
            g.origin = Vec3::new(9.0, 0.0, 0.0);
            g
        };
        let mut sim = ParticleSimulator::default();
        sim.generators.push(g);
        world.insert_resource(sim);

        world.run_system_once(track_attached_origins).unwrap();
        assert_eq!(
            world.resource::<ParticleSimulator>().generators[0].origin,
            Vec3::new(9.0, 0.0, 0.0),
            "no 0x11 follow: the spawn-time origin is untouched"
        );
    }

    #[test]
    fn unlinked_screen_billboard_keeps_authored_rotation() {
        const TOLERANCE: f32 = 1e-4;
        let mut g = live(def(ROUTINE_FPS, ROUTINE_FPS, 1), 0.0);
        g.def.init_rotation[2] = std::f32::consts::FRAC_PI_2;
        emit(&mut g, ROUTINE_FPS);
        assert!(g.immediate_parent.is_none());
        let camera = view(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4));
        let (rotated, _) = rebuilt(&g, camera);
        g.particles[0].rotation = Vec3::ZERO;
        let (unrotated, _) = rebuilt(&g, camera);
        let expected =
            camera.rot * Quat::from_rotation_z(std::f32::consts::FRAC_PI_2) * camera.rot.inverse();
        for (actual, original) in rotated.iter().zip(unrotated.iter()) {
            assert!((*actual - expected * *original).length() < TOLERANCE);
        }
    }

    #[test]
    fn linked_multiple_births_keep_parent_insertion_order() {
        const BIRTHS: u32 = 3;
        let mut parent = live(def(ROUTINE_FPS, ROUTINE_FPS, BIRTHS), 0.0);
        parent.entity = Entity::from_bits(1);
        parent.def.position_variance = Some(ffxi_dat::particle_gen::PositionVariance {
            radius_variance: 1.0,
            base_radius: 1.0,
            axis_scale: [1.0; 3],
        });
        let mut child = live(def(ROUTINE_FPS, ROUTINE_FPS, 1), 0.0);
        child.entity = Entity::from_bits(2);
        child.immediate_parent = Some(parent.entity);
        child.def.parent_position_copy = true;
        child.stopped = true;
        let mut sim = ParticleSimulator {
            generators: vec![child, parent],
            ..default()
        };
        advance_simulator(&mut sim, 1.0);
        assert_eq!(sim.generators[0].particles.len(), BIRTHS as usize);
        assert_ne!(
            sim.generators[1].particles[0].pos,
            sim.generators[1].particles[1].pos
        );
        for (child, parent) in sim.generators[0]
            .particles
            .iter()
            .zip(&sim.generators[1].particles)
        {
            assert_eq!(child.pos, parent.pos);
        }
    }

    #[test]
    fn immediate_linked_emission_copies_birth_position_once_and_keeps_its_rotation() {
        const PARENT_POSITION: Vec3 = Vec3::new(11.0, 4.0, -7.0);
        let mut parent = live(def(30.0, 100.0, 1), 0.0);
        parent.origin = PARENT_POSITION;
        parent.entity = Entity::from_bits(1);
        let mut child = live(def(30.0, 100.0, 1), 0.0);
        child.entity = Entity::from_bits(2);
        child.immediate_parent = Some(parent.entity);
        child.origin = parent.origin;
        child.def.parent_position_copy = true;
        child.def.init_rotation[2] = std::f32::consts::FRAC_PI_2;
        child.stopped = true;
        let mut sim = ParticleSimulator {
            generators: vec![child, parent],
            ..default()
        };
        advance_simulator(&mut sim, 0.0);
        assert!(sim.generators.iter().all(|g| g.particles.is_empty()));
        advance_simulator(&mut sim, 1.0);
        assert_eq!(sim.generators[0].particles.len(), 1);
        let child = &sim.generators[0];
        assert_eq!(child.particles[0].pos, sim.generators[1].particles[0].pos);
        assert_eq!(
            particle_draw(child, &child.particles[0], &sim.clock).world,
            PARENT_POSITION
        );
        assert_eq!(
            child.particles[0].scale,
            Vec2::from_array(child.def.init_scale[..2].try_into().unwrap())
        );
        assert_eq!(child.particles[0].rotation.z, std::f32::consts::FRAC_PI_2);
        advance_simulator(&mut sim, 0.0);
        advance_simulator(&mut sim, 1.0);
        assert_eq!(sim.generators[0].elements_emitted, 1);
        sim.generators.remove(1);
        advance_simulator(&mut sim, 30.0);
        assert!(sim.generators[0].particles.is_empty());
    }

    #[test]
    fn scheduled_zero_window_waits_for_positive_time_and_does_not_repeat_when_paused() {
        const POSITIVE_TICK_FRAMES: f32 = 1.0;
        const PAUSED_TICK_FRAMES: f32 = 0.0;
        let mut g = live(def(ROUTINE_FPS, ROUTINE_FPS, 1), 0.0);
        advance(&mut g, PAUSED_TICK_FRAMES);
        advance(&mut g, PAUSED_TICK_FRAMES);
        assert_eq!(g.elements_emitted, 0);
        assert!(g.particles.is_empty());

        advance(&mut g, POSITIVE_TICK_FRAMES);
        assert_eq!(g.elements_emitted, 1);
        assert_eq!(g.particles.len(), 1);
        let age_before_pause = g.particles[0].age_frames;
        advance(&mut g, PAUSED_TICK_FRAMES);
        assert_eq!(g.elements_emitted, 1);
        assert_eq!(g.particles[0].age_frames, age_before_pause);
        advance(&mut g, POSITIVE_TICK_FRAMES);
        assert_eq!(g.elements_emitted, 1);
        assert_eq!(g.particles.len(), 1);
        assert!(g.particles[0].age_frames > age_before_pause);
    }

    // The cast aura's own generators sit on dur=0 Particle stages (global-dir `ner1`: gn1s dur=0;
    // `eis3`: ge3s/ge31 dur=0), giving a 1-frame emit window, and the frame that spawns them
    // carries a blocking action-DAT read. A singleton must still fire on its first tick however
    // long that frame ran, or the aura never appears at all.
    #[test]
    fn singleton_emits_on_a_first_frame_longer_than_its_emit_window() {
        const SINGLETON_LIFE: f32 = 0.0;
        const ZERO_DURATION_WINDOW: f32 = 0.0;
        const LONG_FRAME: f32 = 9.0;

        let mut g = live(def(SINGLETON_LIFE, 1.0, 1), ZERO_DURATION_WINDOW);
        assert!(g.def.is_singleton());
        advance(&mut g, LONG_FRAME);
        assert_eq!(
            g.particles.len(),
            1,
            "a long spawn frame must not swallow the singleton's only emission"
        );

        advance(&mut g, LONG_FRAME);
        assert!(
            g.particles.is_empty(),
            "it lives out its window and is not re-emitted"
        );
    }

    // research/xim ParticleInitializers.kt read — maxLifeSpan 0 means POSITIVE_INFINITY
    // for the auto-run zone billboards ("the sea and such"): the sun, the moon and the sea
    // must stand for as long as the zone does. The counterpart above pins that a SCHEDULED
    // dur=0 singleton still expires, so the two populations cannot be collapsed.
    #[test]
    fn auto_run_singleton_is_the_persistent_kind() {
        let mut g = live(def(0.0, 1.0, 1), 0.0);
        g.auto_run = true;
        assert!(g.def.is_singleton());

        advance(&mut g, 9.0);
        assert_eq!(g.particles.len(), 1);
        assert!(g.particles[0].life_frames.is_infinite());

        // Whatever the elapsed time, it neither expires nor re-emits.
        for _ in 0..100 {
            advance(&mut g, 60.0);
        }
        assert_eq!(
            g.particles.len(),
            1,
            "the zone billboard neither expires nor duplicates"
        );
        // An infinite life pins life progress at 0, which is what keeps a keyframe-tracked
        // channel on the curve's opening value instead of racing to its end.
        assert!(
            drawn_color(&g, &CelestialClock::default()).is_finite(),
            "infinite life must not poison the draw"
        );
    }

    #[test]
    fn stopped_singleton_never_emits() {
        let mut g = live(def(0.0, 1.0, 1), 0.0);
        g.stopped = true;
        advance(&mut g, 9.0);
        assert!(g.particles.is_empty());
    }

    #[test]
    fn stop_routine_ends_every_generator_the_routine_spawned() {
        let mut sim = ParticleSimulator::default();
        let owner = Entity::from_raw_u32(7).unwrap();
        let other = Entity::from_raw_u32(8).unwrap();
        for (o, gen_id) in [(owner, b"gn10"), (owner, b"gn11"), (other, b"gn12")] {
            let mut g = live(def(4.0, 1.0, 1), 600.0);
            g.origin_routine = Some(RoutineOrigin {
                owner: o,
                gen_id: *gen_id,
                routine: *b"cabk",
            });
            sim.generators.push(g);
        }
        sim.generators.push(live(def(4.0, 1.0, 1), 600.0));

        sim.stop_routine(owner, *b"cabk");
        let stopped: Vec<bool> = sim.generators.iter().map(|g| g.stopped).collect();
        assert_eq!(stopped, vec![true, true, false, false]);

        sim.stop_generators_of_dead_owners(|e| e == owner);
        let stopped: Vec<bool> = sim.generators.iter().map(|g| g.stopped).collect();
        assert_eq!(
            stopped,
            vec![true, true, true, false],
            "a despawned caster's aura stops; a zone/auto-run generator is untouched"
        );
    }

    #[test]
    fn singleton_emits_once() {
        let mut g = live(def(0.0, 1.0, 1), 30.0);
        for _ in 0..5 {
            advance(&mut g, 1.0);
        }
        assert_eq!(g.particles.len(), 1, "singleton emits exactly once");
        assert!(g.particles[0].pos.y > 0.0, "velocity integrated");
    }

    // 0x03 VelocityVarianceSetup: every emitted particle draws a uniform [-v, v] offset
    // per axis on top of the 0x02 base velocity (research/xim ParticleInitializers.kt
    // VelocityVarianceSetup; RandHelper rand() in [-1, 1)).
    #[test]
    fn velocity_variance_spreads_each_axis_around_the_base() {
        let mut d = def(10.0, 1.0, 1);
        d.velocity_variance = Some([0.001, 0.002, 0.003]);
        let mut g = live(d, 30.0);
        advance(&mut g, 5.0);
        assert_eq!(g.particles.len(), 5, "one draw per particle");
        for p in &g.particles {
            assert!(
                (-0.001..=0.001).contains(&p.vel.x)
                    && (0.008..=0.012).contains(&p.vel.y)
                    && (-0.003..=0.003).contains(&p.vel.z),
                "each axis inside base ± bound: {:?}",
                p.vel
            );
        }
        assert!(
            g.particles.windows(2).any(|w| w[0].vel != w[1].vel),
            "the variance is a per-particle draw, not a constant"
        );
    }

    #[test]
    fn velocity_without_variance_is_exactly_the_base() {
        let mut g = live(def(0.0, 1.0, 1), 30.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1);
        assert_eq!(g.particles[0].vel, Vec3::from_array([0.0, 0.01, 0.0]));
    }

    // sec3 0x02 PositionUpdater off: the velocity still exists (the 0x03/0x06/0x09 accelerators
    // keep charging it) but the position never steps, retail's ElemIdle behavior for the 8
    // shipped generators that carry a base velocity without the block (CYyGenerator.cpp
    // CYyGenerator::ElemIdle).
    #[test]
    fn position_updater_off_holds_the_particle_at_its_spawn() {
        let mut d = def(10.0, 1.0, 1);
        d.position_updater = false;
        d.accel = Some([0.0, 0.001, 0.0]);
        let mut g = live(d, 30.0);
        for _ in 0..3 {
            advance(&mut g, 1.0);
        }
        let p = &g.particles[0];
        assert_eq!(p.pos, Vec3::ZERO, "no position step");
        assert!(p.vel.y > 0.01, "the velocity keeps charging: {:?}", p.vel);
    }

    // 0x08 RelativeVelocitySetup: each particle's velocity gains the payload speed along its
    // own spawn offset's direction (research/xim ParticleInitializers.kt
    // RelativeVelocitySetup).
    #[test]
    fn relative_velocity_points_along_the_spawn_offset() {
        let mut d = def(10.0, 1.0, 1);
        d.position_variance = Some(ffxi_dat::particle_gen::PositionVariance {
            radius_variance: 0.0,
            base_radius: 1.0,
            axis_scale: [1.0; 3],
        });
        d.relative_velocity = Some(0.5);
        let mut g = live(d, 30.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1);
        let p = &g.particles[0];
        let extra = p.vel - Vec3::from_array([0.0, 0.01, 0.0]);
        let offset = p.pos;
        assert!(
            (extra - offset.normalize() * 0.5).length() < 1e-6,
            "the added velocity is 0.5 along the spawn offset: {extra:?} vs {offset:?}"
        );
    }

    // With no spawn offset there is no direction, so 0x08 contributes nothing (research/xim
    // ParticleInitializers.kt RelativeVelocitySetup).
    #[test]
    fn relative_velocity_without_a_spawn_offset_is_inert() {
        let mut d = def(0.0, 1.0, 1);
        d.relative_velocity = Some(0.5);
        let mut g = live(d, 30.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1);
        assert_eq!(g.particles[0].vel, Vec3::from_array([0.0, 0.01, 0.0]));
    }

    // 0x41 RelativeVelocityVarianceSetup: each particle's velocity gains a uniform
    // [-v, v] draw along its own spawn offset's direction, on top of the 0x08 speed
    // (research/xim ParticleInitializers.kt RelativeVelocityVarianceSetup).
    #[test]
    fn relative_velocity_variance_spreads_along_the_spawn_offset() {
        let mut d = def(10.0, 1.0, 1);
        d.position_variance = Some(ffxi_dat::particle_gen::PositionVariance {
            radius_variance: 0.0,
            base_radius: 1.0,
            axis_scale: [1.0; 3],
        });
        d.relative_velocity = Some(0.5);
        d.relative_velocity_variance = Some(0.2);
        let mut g = live(d, 30.0);
        advance(&mut g, 3.0);
        assert_eq!(g.particles.len(), 3);
        for p in &g.particles {
            let dir = p.pos.normalize();
            let extra = p.vel - Vec3::from_array([0.0, 0.01, 0.0]);
            let along = extra.dot(dir);
            assert!(
                (0.3..0.7).contains(&along),
                "the added speed is 0.5 plus a [-0.2, 0.2) draw along the offset: {along}"
            );
            let perpendicular = extra - dir * along;
            assert!(
                perpendicular.length() < 1e-5,
                "the added velocity stays on the spawn offset's direction: {perpendicular:?}"
            );
        }
    }

    // 0x17 ColorVarianceSetup: each rgb channel gains its bound times one [0, 1) draw on top
    // of the 0x16 base (research/xim ParticleInitializers.kt ColorVarianceSetup).
    #[test]
    fn color_variance_spreads_each_channel_upward() {
        let mut d = def(10.0, 1.0, 1);
        d.init_color = [0.5, 0.5, 0.5, 1.0];
        d.color_variance = Some([0.5, 0.25, 0.125, 0.0]);
        let mut g = live(d, 30.0);
        advance(&mut g, 3.0);
        assert_eq!(g.particles.len(), 3);
        for p in &g.particles {
            let c = p.rgb;
            assert!(
                (0.5..1.0).contains(&c.x),
                "the red channel stays in [base, base + bound): {c:?}"
            );
            assert!(
                (0.5..0.75).contains(&c.y),
                "the green channel stays in [base, base + bound): {c:?}"
            );
            assert!(
                (0.5..0.625).contains(&c.z),
                "the blue channel stays in [base, base + bound): {c:?}"
            );
        }
    }

    // With no spawn offset there is no direction, so 0x41 contributes nothing even with
    // 0x08 present (research/xim ParticleInitializers.kt RelativeVelocityVarianceSetup).
    #[test]
    fn relative_velocity_variance_without_a_spawn_offset_is_inert() {
        let mut d = def(0.0, 1.0, 1);
        d.relative_velocity = Some(0.5);
        d.relative_velocity_variance = Some(0.2);
        let mut g = live(d, 30.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1);
        assert_eq!(g.particles[0].vel, Vec3::from_array([0.0, 0.01, 0.0]));
    }

    // 0x67 ReverseDisplacementSetup: the particle spawns at the trajectory's endpoint and
    // traces the path backward (research/xim ParticleInitializers.kt ReverseDisplacementSetup
    // — position += total velocity × maxAge, then velocity ×= −1).
    #[test]
    fn reverse_displacement_spawns_at_the_endpoint_and_reverses() {
        let mut d = def(10.0, 1.0, 1);
        d.init_velocity = [0.0, 0.1, 0.0];
        d.reverse_displacement = Some(0.0);
        let mut g = live(d, 30.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1);
        let p = &g.particles[0];
        assert_eq!(p.vel, Vec3::from_array([0.0, -0.1, 0.0]));
        assert_eq!(p.pos, Vec3::from_array([0.0, 1.0, 0.0]));
        advance(&mut g, 5.0);
        assert_eq!(g.particles[0].pos, Vec3::from_array([0.0, 0.5, 0.0]));
    }

    // 0x3B IncrementalRotationApplier: element N's rotation gains the increment × (N + 1) on
    // top of the 0x09 base, and its orientation step flips the rotation y (research/xim
    // ParticleInitializers.kt IncrementalRotationApplier).
    #[test]
    fn incremental_rotation_scales_with_the_element_index_and_flips_y() {
        let mut d = def(10.0, 1.0, 1);
        d.init_rotation = [0.0, 0.05, 0.0];
        d.incremental_rotation = Some([0.0, 0.1, 0.0]);
        let mut g = live(d, 30.0);
        advance(&mut g, 3.0);
        assert_eq!(g.particles.len(), 3);
        for (i, p) in g.particles.iter().enumerate() {
            let y = 0.05 + 0.1 * (i + 1) as f32;
            assert_eq!(p.rotation, Vec3::new(0.0, y, 0.0));
            assert!(p.negate_rotation_y);
            assert_eq!(
                particle_rotation(p),
                Quat::from_euler(EulerRot::XYZ, 0.0, -y, 0.0)
            );
        }
    }

    // 0x0A RotationVarianceInitializer: each particle's rotation draws a uniform
    // [-v, v] offset per axis on top of the 0x09 base (research/xim
    // ParticleInitializers.kt RotationVarianceInitializer).
    #[test]
    fn rotation_variance_spreads_each_axis_around_the_base() {
        let mut d = def(10.0, 1.0, 1);
        d.init_rotation = [0.1, 0.2, 0.3];
        d.rotation_variance = Some([0.05, 0.1, 0.15]);
        let mut g = live(d, 30.0);
        advance(&mut g, 3.0);
        assert_eq!(g.particles.len(), 3);
        let bounds = [
            (0.1f32 - 0.05f32, 0.1f32 + 0.05f32),
            (0.2f32 - 0.1f32, 0.2f32 + 0.1f32),
            (0.3f32 - 0.15f32, 0.3f32 + 0.15f32),
        ];
        for p in &g.particles {
            let r = p.rotation;
            assert!(
                (bounds[0].0 - 1e-6..=bounds[0].1 + 1e-6).contains(&r.x)
                    && (bounds[1].0 - 1e-6..=bounds[1].1 + 1e-6).contains(&r.y)
                    && (bounds[2].0 - 1e-6..=bounds[2].1 + 1e-6).contains(&r.z),
                "each axis inside base ± bound (one f32 rounding of slack): {r:?}"
            );
        }
        assert!(
            g.particles
                .windows(2)
                .any(|w| w[0].rotation != w[1].rotation),
            "the variance is a per-particle draw, not a constant"
        );
    }

    #[test]
    fn rotation_without_variance_is_exactly_the_base() {
        let mut d = def(0.0, 1.0, 1);
        d.init_rotation = [0.1, 0.2, 0.3];
        let mut g = live(d, 30.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1);
        assert_eq!(g.particles[0].rotation, Vec3::from_array([0.1, 0.2, 0.3]));
    }

    #[test]
    fn auto_run_keeps_emitting_past_window() {
        let mut g = live(def(2.0, 1.0, 1), 3.0);
        g.auto_run = true;
        for _ in 0..30 {
            advance(&mut g, 1.0);
        }
        assert!(
            !g.particles.is_empty(),
            "auto-run generators never stop emitting"
        );
    }

    // A celestial billboard: continuous singleton, additive, one live particle whose colour
    // is what the sun/moon opcodes drive.
    fn celestial(def: ParticleGeneratorDef) -> LiveGenerator {
        let mut g = live(def, 1.0);
        g.auto_run = true;
        g.particles.push(Particle {
            pos: Vec3::ZERO,
            spawn_origin: Vec3::ZERO,
            vel: Vec3::ZERO,
            age_frames: 0.0,
            life_frames: 1.0,
            rgb: Vec3::from_slice(&g.def.init_color[..3]),
            scale: Vec2::ONE,
            scale_seed: Vec2::ONE,
            scale_vel: Vec2::ZERO,
            rotation: Vec3::ZERO,
            spin: Vec3::ZERO,
            negate_rotation_y: false,
            rel_vel: Vec3::ZERO,
            osc: None,
        });
        g
    }

    fn retail_assets(file_id: u32) -> Option<ActionAssets> {
        let root = ffxi_dat::archive::open_test_install()?;
        let loc = match root.resolve(file_id) {
            Ok(loc) => loc,
            Err(err) => {
                eprintln!("skipping: file {file_id} is not in this install ({err})");
                return None;
            }
        };
        let path = loc.path_under(&root);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                eprintln!("skipping: {} unreadable ({err})", path.display());
                return None;
            }
        };
        Some(crate::scheduler_runtime::parse_action_bytes(&bytes).1)
    }

    fn view(rot: Quat) -> CameraView {
        CameraView {
            rot,
            pos: Vec3::ZERO,
        }
    }

    // The zone/celestial FFXI->Bevy fold `spawn_zone_particle_generator` installs.
    const ZONE_VEL_BASIS: Vec3 = Vec3::new(1.0, -1.0, -1.0);

    fn axial_celestial(init_scale: [f32; 3], local: Vec3) -> LiveGenerator {
        let mut d = def(1.0, 1.0, 1);
        d.billboard = ParticleBillboard::Camera;
        d.init_scale = init_scale;
        let mut g = celestial(d);
        g.vel_basis = ZONE_VEL_BASIS;
        // The glow domes this stands in for are solids; `local` is one dome vertex.
        g.solid_mesh = true;
        g.template = SpriteTemplate {
            positions: vec![local],
            uvs: vec![[0.0, 0.0]],
            indices: vec![0, 0, 0],
            colors: vec![Vec4::ONE],
        };
        g
    }

    fn rebuilt(g: &LiveGenerator, cam: CameraView) -> (Vec<Vec3>, Vec<Vec4>) {
        use bevy::mesh::VertexAttributeValues::{Float32x3, Float32x4};
        let mut mesh = empty_mesh();
        rebuild_mesh(g, cam, &CelestialClock::default(), &mut mesh);
        let Some(Float32x3(pos)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
            panic!("rebuilt mesh has f32x3 positions");
        };
        let Some(Float32x4(col)) = mesh.attribute(Mesh::ATTRIBUTE_COLOR) else {
            panic!("rebuilt mesh has f32x4 colours");
        };
        (
            pos.iter().copied().map(Vec3::from_array).collect(),
            col.iter().copied().map(Vec4::from_array).collect(),
        )
    }

    // research/xim Particle.kt computeParticleSpaceOrientationTransform + 548-569 — BillBoardType::Camera orients the particle in the
    // world so mesh-local +X points at the eye. Drawing it as a screen billboard instead turns
    // the sun/moon glow dome's symmetry axis sideways (kuluu-fjd3).
    #[test]
    fn camera_billboard_points_mesh_local_x_at_the_camera() {
        const PARALLEL_TOLERANCE: f32 = 1e-4;
        let g = axial_celestial([1.0; 3], Vec3::X);
        for cam_pos in [
            Vec3::new(900.0, 0.0, 0.0),
            Vec3::new(0.0, 700.0, 0.0),
            Vec3::new(0.0, -700.0, 0.0),
            Vec3::new(0.0, 0.0, -12.0),
            Vec3::new(3.0, 4.0, 5.0),
        ] {
            let (positions, _) = rebuilt(
                &g,
                CameraView {
                    rot: Quat::IDENTITY,
                    pos: cam_pos,
                },
            );
            let offset = positions[0].normalize();
            assert!(
                (offset.dot(cam_pos.normalize()) - 1.0).abs() < PARALLEL_TOLERANCE,
                "cam {cam_pos} gave axis {offset}",
            );
        }
    }

    // The authored z-scale is a real third axis on a camera billboard — retail's
    // ScaleInitializer writes all three (research/xim ParticleInitializers.kt) and file
    // 104's `weat/suny/sun1` authors [40, 30, 100]. A screen sprite drops it; an axial dome
    // must not.
    #[test]
    fn camera_billboard_applies_the_authored_z_scale() {
        const Z_SCALE: f32 = 100.0;
        let g = axial_celestial([40.0, 30.0, Z_SCALE], Vec3::Z);
        let (positions, _) = rebuilt(
            &g,
            CameraView {
                rot: Quat::IDENTITY,
                pos: Vec3::new(900.0, 0.0, 0.0),
            },
        );
        assert!((positions[0].length() - Z_SCALE).abs() < 1e-3);
    }

    // Mirrors what `spawn_zone_particle_generator` wires up for a generator declared in a zone
    // DAT, so the guard below reads the same geometry the running client would.
    fn zone_generator(assets: &ActionAssets, name: &[u8; 4]) -> LiveGenerator {
        let def = *assets
            .particle_defs
            .get(name)
            .expect("the zone DAT declares the generator");
        let mut images = Assets::<Image>::default();
        let (template, sprite_frames, _, draw_path) =
            resolve_zone_mesh(assets, &def, &mut images, false).expect("its linked mesh resolves");
        let mut g = celestial(def);
        g.solid_mesh = is_solid_mesh(&template);
        g.template = template;
        g.sprite_frames = sprite_frames;
        g.draw_path = draw_path;
        g.orientation = particle_orientation(&g.def);
        g.actor_local = false;
        g.vel_basis = ZONE_VEL_BASIS;
        g
    }

    // The shipped zone DATs put two unrelated kinds of mesh behind a Camera generator. ROM 210's
    // `sun0` links the `suns` MMB dome, a solid authored around its x axis (extent 11.4 x 50.0 x
    // 49.5) — the case the aim-at-eye rotation describes. ROM 230's `bun4` links the `chob`
    // sprite sheet, a 2 x 2 xy quad whose z extent is exactly 0; aiming its local +X at the eye
    // puts its one face along the view ray and it draws edge-on, so it keeps the screen
    // billboard (kuluu-fjd3). Skips without a retail install.
    #[test]
    fn only_a_solid_mesh_takes_the_axial_camera_path() {
        const SUN_DOME_ZONE: u32 = 210;
        const SUN_DOME_GEN: [u8; 4] = *b"sun0";
        const SPRITE_SHEET_ZONE: u32 = 230;
        const SPRITE_SHEET_GEN: [u8; 4] = *b"bun4";
        // Far enough along +Z that the eye direction is that axis to well inside FACING.
        const EYE_DISTANCE: f32 = 500.0;
        const FACING: f32 = 0.999;

        let (Some(dome_assets), Some(sheet_assets)) = (
            retail_assets(SUN_DOME_ZONE),
            retail_assets(SPRITE_SHEET_ZONE),
        ) else {
            return;
        };
        let dome = zone_generator(&dome_assets, &SUN_DOME_GEN);
        let sheet = zone_generator(&sheet_assets, &SPRITE_SHEET_GEN);

        for g in [&dome, &sheet] {
            assert_eq!(g.def.billboard, ParticleBillboard::Camera);
            assert!(g.orientation.is_none());
        }
        assert!(
            is_solid_mesh(&dome.template),
            "the suns dome has extent on all three axes"
        );
        assert!(
            !is_solid_mesh(&sheet.template),
            "the chob sheet frame is a flat xy quad"
        );
        assert!(is_axial_camera_billboard(&dome));
        assert!(!is_axial_camera_billboard(&sheet));

        let eye = |z: f32| CameraView {
            rot: Quat::IDENTITY,
            pos: Vec3::new(0.0, 0.0, z),
        };
        // An identity camera rotation leaves the sheet's authored +Z normal alone, which is what
        // an eye on the +Z axis sees; the axial rotation swings that normal to +X instead.
        let (positions, _) = rebuilt(&sheet, eye(EYE_DISTANCE));
        let normal = (positions[1] - positions[0])
            .cross(positions[2] - positions[0])
            .normalize();
        assert!(
            normal.dot(Vec3::Z).abs() > FACING,
            "the flat sheet faces the eye, normal {normal}"
        );

        // The dome does carry the aim-at-eye orientation, so its drawn vertices move with the eye.
        assert_ne!(
            rebuilt(&dome, eye(EYE_DISTANCE)).0,
            rebuilt(&dome, eye(-EYE_DISTANCE)).0
        );
    }

    // Only an axial camera billboard reorients with the eye position, so only it may put the
    // camera translation in the rebuild key; every other generator would rebuild its mesh on
    // every step the camera takes (kuluu-b5nt).
    #[test]
    fn only_the_axial_camera_billboard_keys_on_camera_position() {
        let clock = CelestialClock::default();
        let at = |g: &LiveGenerator, x: f32| {
            mesh_key(
                g,
                CameraView {
                    rot: Quat::IDENTITY,
                    pos: Vec3::new(x, 0.0, 0.0),
                },
                &clock,
            )
        };
        let axial = axial_celestial([1.0; 3], Vec3::X);
        assert_ne!(at(&axial, 10.0), at(&axial, 20.0));

        let mut screen = axial_celestial([1.0; 3], Vec3::X);
        screen.def.billboard = ParticleBillboard::Xyz;
        assert_eq!(at(&screen, 10.0), at(&screen, 20.0));
    }

    // The sun/moon domes are untextured meshes whose whole shape is a vertex-alpha ramp (128 at
    // the centre to 0 at the rim), so substituting the flat life curve on the MMB draw path
    // renders them as hard-edged discs. The D3m path keeps the substitution.
    #[test]
    fn mmb_additive_keeps_the_vertex_alpha_gradient() {
        const VERTEX_ALPHAS: [f32; 3] = [1.0, 0.75, 0.0];
        const HALF_LIFE_ALPHA: f32 = 0.5;
        let mut g = axial_celestial([1.0; 3], Vec3::X);
        g.template.positions = vec![Vec3::X; VERTEX_ALPHAS.len()];
        g.template.uvs = vec![[0.0, 0.0]; VERTEX_ALPHAS.len()];
        g.template.indices = vec![0, 1, 2];
        g.template.colors = VERTEX_ALPHAS
            .iter()
            .map(|&a| Vec4::new(1.0, 1.0, 1.0, a))
            .collect();
        g.particles[0].age_frames = HALF_LIFE_ALPHA;

        let cam = CameraView {
            rot: Quat::IDENTITY,
            pos: Vec3::new(900.0, 0.0, 0.0),
        };

        g.draw_path = D3mDrawPath::Mmb;
        let (_, colors) = rebuilt(&g, cam);
        for (c, a) in colors.iter().zip(VERTEX_ALPHAS) {
            assert!(
                (c.w - HALF_LIFE_ALPHA * a).abs() < 1e-6,
                "mmb alpha {}",
                c.w
            );
        }

        g.draw_path = D3mDrawPath::D3m;
        let (_, colors) = rebuilt(&g, cam);
        for c in &colors {
            assert!((c.w - HALF_LIFE_ALPHA).abs() < 1e-6, "d3m alpha {}", c.w);
        }
    }

    fn ramp(from: f32, to: f32) -> KeyFrameTrack {
        KeyFrameTrack {
            points: vec![(0.0, from), (1.0, to)],
        }
    }

    // research/xim ParticleGeneratorParser.kt sec3Handler — the ClockValueUpdater curves are
    // sampled at the Vana'diel day fraction, so a celestial particle's colour tracks the
    // clock, NOT its own life progress. This is the sun's authored dawn/noon/dusk ramp;
    // sampling it by life would freeze the disc at the curve's opening value forever, since
    // a continuous singleton is re-emitted at progress 0 every frame.
    #[test]
    fn time_of_day_curves_sample_the_clock_not_particle_life() {
        let mut def = def(1.0, 1.0, 1);
        def.blend = ffxi_dat::particle_gen::ParticleBlend::Blend;
        def.init_color = [1.0, 1.0, 1.0, 1.0];
        def.tod_color_driven = [true, false, false, false];
        let mut g = celestial(def);
        g.tod_color[0] = Some(ramp(0.0, 1.0));

        // The particle never ages (life_frames == 1, age 0), so any change here is the clock.
        let at = |day_fraction: f32| {
            drawn_color(
                &g,
                &CelestialClock {
                    day_fraction,
                    ..Default::default()
                },
            )
            .x
        };
        let (dawn, dusk) = (at(0.25), at(0.75));
        assert!(
            dusk > dawn,
            "red channel must follow the day fraction: {dawn} -> {dusk}"
        );
    }

    // research/xim Particle.kt getColor — day-of-week first, then moon phase, each a 2x
    // modulate that saturates at 1. Order matters because the modulate clamps: applying the
    // brighter table second cannot recover what the first one crushed.
    #[test]
    fn celestial_tints_apply_day_of_week_then_moon_phase_at_2x() {
        let mut def = def(1.0, 1.0, 1);
        def.blend = ffxi_dat::particle_gen::ParticleBlend::Blend;
        // Low enough that the D3M stage-1 2x gain does not saturate the channel and hide
        // the tint (a 0.5 base already clamps to 1.0 untinted).
        def.init_color = [0.2, 0.2, 0.2, 1.0];
        // A 2x modulate makes 0.5 the identity entry, so 0.25 is the one that halves.
        // Weekday 3 halves red, phase 6 halves it again: 0.2 * 0.5 * 0.5 = 0.05.
        def.day_of_week_color = Some(halves_red_at(3));
        def.moon_phase_color = Some(halves_red_at(6));
        let g = celestial(def);
        let clock = CelestialClock {
            day_fraction: 0.5,
            day_of_week: 3,
            moon_phase: 6,
        };
        let untinted = celestial(blended_celestial_def());
        let plain = drawn_color(&untinted, &clock).x;
        let tinted = drawn_color(&g, &clock).x;
        assert!(
            (tinted - plain * 0.25).abs() < 1e-5,
            "two halving tables at 2x modulate should quarter the channel: {tinted} vs {plain}"
        );
    }

    // research/xim Particle.kt getColor modulates with Color.modulateInPlace (Color.kt),
    // which scales all four channels — dropping the tables' alpha lane leaves the lunar halo
    // lit at every moon phase instead of only around full moon.
    #[test]
    fn celestial_tints_modulate_alpha_not_just_rgb() {
        const IDENTITY: f32 = 0.5;
        let table = |alpha: f32| [IDENTITY, IDENTITY, IDENTITY, alpha];

        let alpha_at = |phase_alpha: Option<f32>| {
            let mut def = blended_celestial_def();
            if let Some(phase_alpha) = phase_alpha {
                def.day_of_week_color =
                    Some([table(IDENTITY); ffxi_dat::particle_gen::DAYS_OF_WEEK]);
                def.moon_phase_color =
                    Some([table(phase_alpha); ffxi_dat::particle_gen::MOON_PHASES]);
            }
            let g = celestial(def);
            particle_draw(
                &g,
                &g.particles[0],
                &CelestialClock {
                    day_fraction: 0.5,
                    day_of_week: 0,
                    moon_phase: 11,
                },
            )
            .life_alpha
        };

        assert_eq!(
            alpha_at(Some(0.0)),
            0.0,
            "a zero-alpha phase entry hides the sprite"
        );
        assert!(
            (alpha_at(Some(IDENTITY)) - alpha_at(None)).abs() < 1e-5,
            "a 0.5 entry at 2x modulate is the identity"
        );
    }

    fn zone_bytes(file_id: u32) -> Option<Vec<u8>> {
        let root = ffxi_dat::archive::open_test_install()?;
        let location = root.resolve(file_id).ok()?;
        std::fs::read(location.path_under(&root)).ok()
    }

    fn moon_attached_def(bytes: &[u8], name: &[u8; 4]) -> ParticleGeneratorDef {
        ffxi_dat::chunk::walk(bytes)
            .flatten()
            .filter(|c| {
                c.name == *name
                    && ffxi_dat::ChunkKind::from_u8(c.kind) == Some(ffxi_dat::ChunkKind::Generator)
            })
            .find_map(|c| ParticleGeneratorDef::parse(c.data).ok().flatten())
            .filter(|d| d.attach_type == ffxi_dat::particle_gen::AttachType::Moon)
            .expect("zone DAT declares the Moon-attached generator")
    }

    fn phase_alpha(def: &ParticleGeneratorDef, moon_phase: usize) -> f32 {
        let g = celestial(*def);
        particle_draw(
            &g,
            &g.particles[0],
            &CelestialClock {
                day_fraction: 0.5,
                day_of_week: 0,
                moon_phase,
            },
        )
        .life_alpha
    }

    // The shipped f_ro (zone DAT 210) tables: `kasa`, the lunar halo MMB, carries a 0x4F alpha
    // lane that is zero outside phases 5..=7, while the `moon` sprite's never drops below 0.42.
    // With the alpha lane dropped, the halo drew as a saturated disc ~20 degrees across that
    // swamped the moon at every phase. The drawn alpha is pinned to a value, not just to
    // "> 0", so a halo that regressed to near-invisible near full moon also fails.
    // Skips without a retail install.
    #[test]
    fn zone_210_lunar_halo_is_dark_except_near_full_moon() {
        const F_RO: u32 = 210;
        const SPRITE_MIN_ALPHA: f32 = 0.42;
        // `kasa`'s 0x4F alpha lane as shipped, dumped byte-for-byte from f_ro.
        const HALO_PHASE_ALPHA_BYTE: [u8; ffxi_dat::particle_gen::MOON_PHASES] =
            [0, 0, 0, 0, 0, 60, 128, 60, 0, 0, 0, 0];
        /// DAT 210 kasa: initializer alpha 128/255 and weekday/phase modulation gain 160/255.
        const HALO_CHAIN_GAIN: f32 = (128.0 / 255.0) * (160.0 / 255.0);
        const ALPHA_EPS: f32 = 1e-6;

        let Some(bytes) = zone_bytes(F_RO) else {
            eprintln!("skipping: no retail DAT root (set FFXI_DAT_PATH)");
            return;
        };
        let halo = moon_attached_def(&bytes, b"kasa");
        assert_eq!(halo.init_color[3], 128.0 / 255.0);
        let sprite = moon_attached_def(&bytes, b"moon");
        assert_eq!(sprite.init_color[3], 128.0 / 255.0);
        let halo_table = halo
            .moon_phase_color
            .expect("the halo generator carries a moon-phase colour table");

        for phase in 0..ffxi_dat::particle_gen::MOON_PHASES {
            let lane = HALO_PHASE_ALPHA_BYTE[phase] as f32 / u8::MAX as f32;
            assert!(
                (halo_table[phase][3] - lane).abs() < ALPHA_EPS,
                "halo alpha lane read back from the DAT, phase {phase}: \
                 {} vs {lane}",
                halo_table[phase][3]
            );

            let halo_alpha = phase_alpha(&halo, phase);
            let expected = lane * HALO_CHAIN_GAIN;
            assert!(
                (halo_alpha - expected).abs() < ALPHA_EPS,
                "halo draws its DAT alpha lane, phase {phase}: {halo_alpha} vs {expected}"
            );
            assert!(
                phase_alpha(&sprite, phase) > SPRITE_MIN_ALPHA,
                "moon phase {phase}: alpha {}, initializer {}",
                phase_alpha(&sprite, phase),
                sprite.init_color[3]
            );
        }
    }

    // The alpha lattice a decoded-then-remapped DXT3 texture can sit on: the 4-bit plane holds
    // multiples of the dither step, and `apply_ffxi_alpha_remap` doubles with saturation
    // (ffxi-dat/src/texture.rs DXT3_ALPHA_DITHER_STEP, ffxi_alpha_remap). Any other value is a
    // neighbourhood mean, i.e. proof the undither ran.
    fn off_nibble_lattice(alpha: &[u8]) -> usize {
        use ffxi_dat::texture::{ffxi_alpha_remap, DXT3_ALPHA_DITHER_STEP};

        let lattice: Vec<u8> = (0..=u8::MAX)
            .step_by(DXT3_ALPHA_DITHER_STEP as usize)
            .map(ffxi_alpha_remap)
            .collect();
        alpha.iter().filter(|a| !lattice.contains(a)).count()
    }

    fn image_alpha(images: &Assets<Image>, handle: &Handle<Image>) -> Vec<u8> {
        images
            .get(handle)
            .and_then(|i| i.data.clone())
            .expect("the loaded texture carries its texels")
            .chunks_exact(4)
            .map(|p| p[3])
            .collect()
    }

    // Read off the shipped f_ro DAT: the lunar halo sheet `kasa` is a DXT3 whose alpha is
    // entirely the nibble 7/8 dithered-opaque pair (ffxi-dat/examples/dat-sky-alpha-histogram.rs
    // on zone files 210/331), so the plain particle converter hands the GPU a 238/255
    // per-texel stipple and only the celestial converter averages it back to the authored
    // half-step. Skips without a retail install.
    #[test]
    fn zone_210_halo_sheet_is_dithered_and_only_the_celestial_converter_resolves_it() {
        const F_RO: u32 = 210;
        const HALO_TEX: [u8; 4] = *b"kasa";
        const DITHER_LO: u8 = 0x77;
        const DITHER_HI: u8 = 0x88;
        // 0x80's recovered mean is 127.5, which no 8-bit alpha holds; the remap doubles that
        // to a 254/255 split (ffxi-dat/src/texture.rs apply_ffxi_alpha_remap).
        const RESOLVED_RESIDUAL_MAX: u8 = 1;

        let Some(bytes) = zone_bytes(F_RO) else {
            eprintln!("skipping: no retail DAT root (set FFXI_DAT_PATH)");
            return;
        };
        let tex = ffxi_dat::chunk::walk(&bytes)
            .flatten()
            .filter(|c| {
                c.name == HALO_TEX
                    && ffxi_dat::ChunkKind::from_u8(c.kind) == Some(ffxi_dat::ChunkKind::Img)
            })
            .find_map(|c| ffxi_dat::texture::decode_texture(c.data).ok())
            .expect("f_ro ships the lunar halo sheet");
        assert!(
            tex.rgba
                .chunks_exact(4)
                .all(|p| p[3] == DITHER_LO || p[3] == DITHER_HI),
            "the shipped halo sheet is the nibble 7/8 dithered-opaque pair"
        );

        let mut images = Assets::<Image>::default();
        let plain = images.add(decoded_texture_to_image(&tex));
        let sky = images.add(decoded_sky_texture_to_image(&tex));

        let plain_alpha = image_alpha(&images, &plain);
        let lo = ffxi_dat::texture::ffxi_alpha_remap(DITHER_LO);
        let hi = ffxi_dat::texture::ffxi_alpha_remap(DITHER_HI);
        assert!(
            plain_alpha.contains(&lo) && plain_alpha.contains(&hi),
            "the shared particle converter keeps the stipple"
        );

        let sky_alpha = image_alpha(&images, &sky);
        let spread =
            sky_alpha.iter().max().expect("non-empty") - sky_alpha.iter().min().expect("non-empty");
        assert!(
            spread <= RESOLVED_RESIDUAL_MAX && *sky_alpha.iter().min().expect("non-empty") > lo,
            "the celestial converter left alpha spread {spread}"
        );
    }

    // The retail half of the wire: the undither argument has to survive the mesh/texture
    // resolution it is threaded through, on the sheet the celestial set really binds. f_ro's
    // `moon` generator draws `moonshap`, a 4-bit-alpha DXT3 the moon-material path already
    // undithers at moon_material.rs load_moon_sprite_sheet, so its texels are where the argument is observable:
    // undithered alpha leaves the nibble lattice, dithered alpha cannot. Skips without a
    // retail install. `celestial_particles::tests::the_celestial_spawn_binds_an_undithered_sheet`
    // pins the two production links this one does not reach.
    #[test]
    fn resolve_zone_mesh_undithers_the_moon_sheet_texels() {
        const F_RO: u32 = 210;
        const MOON_GEN: [u8; 4] = *b"moon";

        let Some(assets) = retail_assets(F_RO) else {
            return;
        };
        let def = *assets
            .particle_defs
            .get(&MOON_GEN)
            .expect("f_ro declares the moon generator");

        let alpha = |undither: bool| {
            let mut images = Assets::<Image>::default();
            let (_, _, tex, _) = resolve_zone_mesh(&assets, &def, &mut images, undither)
                .expect("the moon mesh resolves");
            let handle = tex.expect("the moon mesh links a texture");
            image_alpha(&images, &handle)
        };

        assert_eq!(
            off_nibble_lattice(&alpha(false)),
            0,
            "the shared particle converter only ever emits remapped nibble alpha"
        );
        assert!(
            off_nibble_lattice(&alpha(true)) > 0,
            "the undither argument never reached the texture converter"
        );
    }

    // A tint table that is the identity everywhere except `target`, where it halves red.
    fn halves_red_at<const N: usize>(target: usize) -> [[f32; 4]; N] {
        std::array::from_fn(|i| {
            let red = if i == target { 0.25 } else { 0.5 };
            [red, 0.5, 0.5, 1.0]
        })
    }

    fn blended_celestial_def() -> ParticleGeneratorDef {
        let mut def = def(1.0, 1.0, 1);
        def.blend = ffxi_dat::particle_gen::ParticleBlend::Blend;
        def.init_color = [0.2, 0.2, 0.2, 1.0];
        def
    }

    // research/xim ParticleGeneratorParser.kt sec3Handler MoonPhaseSpriteSheetUpdater — the moon's
    // sheet frame is the phase index, so it must NOT flipbook over the particle's life the
    // way every other sprite-sheet particle does.
    #[test]
    fn moon_phase_pins_the_sprite_frame() {
        let mut def = def(1.0, 1.0, 1);
        def.moon_phase_sprite = true;
        let mut g = celestial(def);
        g.sprite_frames = (0..ffxi_dat::particle_gen::MOON_PHASES)
            .map(|_| g.template.clone())
            .collect();

        for phase in 0..ffxi_dat::particle_gen::MOON_PHASES {
            let draw = particle_draw(
                &g,
                &g.particles[0],
                &CelestialClock {
                    moon_phase: phase,
                    ..Default::default()
                },
            );
            assert_eq!(draw.flipbook_frame, phase);
        }

        // Out-of-range phases clamp instead of indexing past the sheet.
        let draw = particle_draw(
            &g,
            &g.particles[0],
            &CelestialClock {
                moon_phase: 99,
                ..Default::default()
            },
        );
        assert_eq!(draw.flipbook_frame, ffxi_dat::particle_gen::MOON_PHASES - 1);
    }

    #[test]
    fn continuous_singleton_holds_one_particle_and_replaces_on_expiry() {
        let mut d = def(4.0, 1.0, 3);
        d.continuous = true;
        let mut g = live(d, 1.0);
        g.auto_run = true;
        let mut max_alive = 0usize;
        let mut empty_streak = 0usize;
        let mut max_empty_streak = 0usize;
        for _ in 0..20 {
            advance(&mut g, 1.0);
            max_alive = max_alive.max(g.particles.len());
            if g.particles.is_empty() {
                empty_streak += 1;
                max_empty_streak = max_empty_streak.max(empty_streak);
            } else {
                empty_streak = 0;
            }
        }
        assert_eq!(
            max_alive, 1,
            "continuous singleton caps at one live particle"
        );
        assert_eq!(
            max_empty_streak, 0,
            "a continuous generator is never empty at render — the expired particle \
             is replaced the same tick, so the body never blinks out for a frame"
        );
    }

    #[test]
    fn continuous_trackless_generator_holds_constant_alpha() {
        use ffxi_dat::particle_gen::ParticleBlend;
        let mut base = def(4.0, 1.0, 1);
        base.blend = ParticleBlend::Blend;
        base.init_color = [1.0, 1.0, 1.0, 0.8];

        // Vertex alpha well under the D3m stage clamp, so the two curves stay distinguishable
        // after the 4x TEXTUREFACTOR alpha gain instead of both saturating at 1.
        const VERT_ALPHA: f32 = 0.125;
        let mut cont = live(base, 1.0);
        cont.def.continuous = true;
        set_template_color(&mut cont, Vec3::ONE.extend(VERT_ALPHA));
        let mut spray = live(base, 1.0);
        set_template_color(&mut spray, Vec3::ONE.extend(VERT_ALPHA));

        let particle = |age: f32| Particle {
            pos: Vec3::ZERO,
            spawn_origin: Vec3::ZERO,
            vel: Vec3::ZERO,
            age_frames: age,
            life_frames: 4.0,
            rgb: Vec3::ONE,
            scale: Vec2::splat(0.1),
            scale_seed: Vec2::splat(0.1),
            scale_vel: Vec2::ZERO,
            rotation: Vec3::ZERO,
            spin: Vec3::ZERO,
            negate_rotation_y: false,
            rel_vel: Vec3::ZERO,
            osc: None,
        };
        cont.particles = vec![particle(3.0)];
        spray.particles = vec![particle(3.0)];

        let alpha_of = |g: &LiveGenerator| -> f32 {
            let mut mesh = empty_mesh();
            rebuild_mesh(
                g,
                view(Quat::IDENTITY),
                &CelestialClock::default(),
                &mut mesh,
            );
            match mesh.attribute(Mesh::ATTRIBUTE_COLOR).unwrap() {
                bevy::mesh::VertexAttributeValues::Float32x4(c) => c[0][3],
                _ => panic!("expected Float32x4 colours"),
            }
        };

        let expected = |curve: f32| VERT_ALPHA * curve * D3M_STAGE1_ALPHA_GAIN;
        assert!(
            (alpha_of(&cont) - expected(base.init_color[3])).abs() < 1e-4,
            "continuous body keeps authored opacity"
        );
        assert!(
            (alpha_of(&spray) - expected(0.25)).abs() < 1e-4,
            "a transient spray still fades 1.0-progress over life"
        );
    }

    #[test]
    fn real_dat_monument_shaft_retains_authored_alpha() {
        const LOWER_JEUNO_DAT: u32 = 345;
        const SHAFT_ALPHA: f32 = 50.0 / 255.0;
        let Some(assets) = retail_assets(LOWER_JEUNO_DAT) else {
            return;
        };
        let def = *assets.particle_defs.get(b"SPLT").expect("monument shaft");
        assert!(def.is_singleton());
        assert_eq!(def.init_color[3], SHAFT_ALPHA);
        let mut g = celestial(def);
        g.particles[0].life_frames = f32::INFINITY;
        for age in [0.0, 300.0, 30_000.0] {
            g.particles[0].age_frames = age;
            let draw = particle_draw(&g, &g.particles[0], &CelestialClock::default());
            assert_eq!(draw.factor_alpha, SHAFT_ALPHA);
        }
    }

    #[test]
    fn particle_expires_at_life() {
        let mut g = live(def(3.0, 1.0, 1), 1.0);
        advance(&mut g, 1.0); // emit one at age 0
        assert_eq!(g.particles.len(), 1);
        advance(&mut g, 5.0); // past life
        assert!(g.particles.is_empty());
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod sheet_texture {
        use super::*;
        use ffxi_dat::sprite_sheet::{ParticleSpriteSheet, SpriteFrame};
        use ffxi_dat::texture::{DecodedTexture, TexFormat};

        const SHEET_ID: [u8; 4] = *b"fir ";
        const CATEGORY: &str = "venom1";
        const LOCAL: &str = "fir";

        fn one_pixel() -> DecodedTexture {
            DecodedTexture {
                width: 1,
                height: 1,
                format_tag: TexFormat::Bgra32,
                rgba: vec![255, 255, 255, 255],
            }
        }

        fn sheet_assets(qualified: bool, local: bool, namespace_only: bool) -> ActionAssets {
            let mut assets = ActionAssets::default();
            assets.sprite_sheets.insert(
                SHEET_ID,
                ParticleSpriteSheet {
                    frames: vec![SpriteFrame {
                        positions: vec![[0.0; 3]; 3],
                        uvs: vec![[0.0, 0.0]; 3],
                        colors: vec![[128, 128, 128, 128]; 3],
                    }],
                    category: CATEGORY.to_string(),
                    id: LOCAL.to_string(),
                },
            );
            if qualified {
                assets
                    .images_by_qualified_name
                    .insert((CATEGORY.to_string(), LOCAL.to_string()), one_pixel());
            }
            if local {
                assets.images_by_name.insert(LOCAL.to_string(), one_pixel());
            }
            if namespace_only {
                assets
                    .images_by_name
                    .insert(CATEGORY.to_string(), one_pixel());
            }
            assets
        }

        fn sheet_def() -> ParticleGeneratorDef {
            let mut d = def(30.0, 1.0, 1);
            d.mesh_id = SHEET_ID;
            d.mesh_kind = ffxi_dat::particle_gen::ParticleMeshKind::SpriteSheet;
            d
        }

        fn resolved_texture(assets: &ActionAssets) -> Option<Handle<Image>> {
            let mut images = Assets::<Image>::default();
            resolve_mesh(assets, NO_LOCAL_DIR, &sheet_def(), &mut images, false)
                .expect("sheet mesh resolves")
                .2
        }

        // research/xim DatResource.kt getTextureResourceByNameAs — qualified (namespace, local) match first.
        #[test]
        fn sprite_sheet_texture_resolves_by_qualified_name() {
            assert!(resolved_texture(&sheet_assets(true, false, false)).is_some());
        }

        #[test]
        fn sprite_sheet_texture_falls_back_to_local_name() {
            assert!(resolved_texture(&sheet_assets(false, true, false)).is_some());
        }

        // The kuluu-7jpq regression: the Img was only ever looked up under the sheet's
        // NAMESPACE token, which is not how any tier resolves, so the cloud drew untextured.
        #[test]
        fn sprite_sheet_texture_does_not_resolve_by_namespace_alone() {
            assert!(resolved_texture(&sheet_assets(false, false, true)).is_none());
        }

        // ROM/0/28.DAT (file 100) carries 25 Imgs and every one of them is type byte 0x81; its
        // `smok` 0x21 sheet names `effect  smoke01 `, which only that 0x81 Img (offset 3389984)
        // supplies. The 0x81 header is the compressed-format flag extract_texture_tokens reads
        // (ffxi-dat/src/texture.rs FLG_FMT0_COMPRESSED); 105 of the install's 113
        // name-unresolvable sheets are this case.
        #[test]
        fn real_dat_sprite_sheet_resolves_a_format0_texture() {
            const SMOKE_FILE_ID: u32 = 100;
            const SMOKE_SHEET_ID: [u8; 4] = *b"smok";
            let Some(assets) = retail_assets(SMOKE_FILE_ID) else {
                return;
            };
            let sheet = assets
                .sprite_sheets
                .get(&SMOKE_SHEET_ID)
                .expect("file 100 ships a smok sheet");
            assert_eq!(
                (sheet.category.as_str(), sheet.id.as_str()),
                ("effect", "smoke01")
            );

            let mut def = sheet_def();
            def.mesh_id = SMOKE_SHEET_ID;
            let mut images = Assets::<Image>::default();
            assert!(
                resolve_mesh(&assets, NO_LOCAL_DIR, &def, &mut images, false)
                    .expect("smok sheet resolves")
                    .2
                    .is_some()
            );
        }
    }

    // Retail-DAT survey over this install (53,244 DATs): 258 D3M and 123 SpriteSheet chunk names
    // repeat across directories INSIDE a single DAT, and the generator defs that link one of
    // those names resolve to different geometry (134 D3M, 46 SpriteSheet) or a different texture
    // (422 D3M, 107 SpriteSheet) depending on the directory, so the flat last-writer-wins maps
    // hand them another directory's mesh. research/xim ParticleLinkedDataProviders.kt getParticleMesh
    // resolves a linked mesh in the generator's own directory first.
    #[cfg(not(target_arch = "wasm32"))]
    mod directory_scoped_mesh {
        use super::*;

        fn first_position(
            assets: &ActionAssets,
            local_dir: [u8; 4],
            def: &ParticleGeneratorDef,
        ) -> Vec3 {
            let mut images = Assets::<Image>::default();
            resolve_mesh(assets, local_dir, def, &mut images, false)
                .expect("the linked mesh resolves")
                .0
                .positions[0]
        }

        // ROM/338/100.DAT declares the D3M `grw1` in both `geo0` and `run0`; `geo0/gl02` links
        // it, and the two copies are a different width with a different texture. The scoped
        // lookup is scheduler_runtime.rs particle_def_scoped.
        #[test]
        fn real_dat_static_mesh_resolves_in_the_generators_own_directory() {
            const GEO_EFFECT_FILE_ID: u32 = 13259;
            const GEO_DIR: [u8; 4] = *b"geo0";
            const GEO_GEN: [u8; 4] = *b"gl02";
            const GEO0_FIRST_VERTEX_X: f32 = -10.0;
            const RUN0_FIRST_VERTEX_X: f32 = -5.0;
            let Some(assets) = retail_assets(GEO_EFFECT_FILE_ID) else {
                return;
            };
            let def = *assets
                .particle_defs_by_dir
                .get(&(GEO_DIR, GEO_GEN))
                .expect("ROM/338/100.DAT declares geo0/gl02");
            assert_eq!(
                assets
                    .particle_def_scoped(NO_LOCAL_DIR, &GEO_GEN)
                    .map(|(dir, _)| dir),
                Some(GEO_DIR),
                "a def reached through the flat tier still reports its authoring directory"
            );
            assert_eq!(
                assets
                    .d3m(GEO_DIR, &def.mesh_id)
                    .map(|d| d.texture_name_tokens()),
                Some(("eff1".to_string(), "grw1".to_string()))
            );
            assert_eq!(
                assets
                    .d3ms
                    .get(&def.mesh_id)
                    .map(|d| d.texture_name_tokens()),
                Some(("eff".to_string(), "grh1".to_string())),
                "the flat map keeps run0's `grw1`, the last one the walk saw"
            );

            assert_eq!(
                first_position(&assets, GEO_DIR, &def).x,
                GEO0_FIRST_VERTEX_X
            );
            assert_eq!(
                first_position(&assets, NO_LOCAL_DIR, &def).x,
                RUN0_FIRST_VERTEX_X,
                "resolving geo0/gl02's mesh outside its directory draws run0's half-width quad"
            );
        }

        // ROM/1/33.DAT declares the 0x21 sprite sheet `ligh` in `ligh`, `tour` and `fire`;
        // `ligh/lt05` links it, and the `fire` copy the flat map keeps is a different quad
        // backed by a different texture. The scoped lookup is scheduler_runtime.rs
        // particle_def_scoped.
        #[test]
        fn real_dat_sprite_sheet_resolves_in_the_generators_own_directory() {
            const LIGHT_EFFECT_FILE_ID: u32 = 333;
            const LIGH_DIR: [u8; 4] = *b"ligh";
            const LIGH_GEN: [u8; 4] = *b"lt05";
            const LIGH_FIRST_VERTEX_Y: f32 = -2.0625;
            const FIRE_FIRST_VERTEX_Y: f32 = -2.0;
            let Some(assets) = retail_assets(LIGHT_EFFECT_FILE_ID) else {
                return;
            };
            let def = *assets
                .particle_defs_by_dir
                .get(&(LIGH_DIR, LIGH_GEN))
                .expect("ROM/1/33.DAT declares ligh/lt05");
            assert_eq!(
                assets
                    .sprite_sheet(LIGH_DIR, &def.mesh_id)
                    .map(|s| (s.category.clone(), s.id.clone())),
                Some(("effect".to_string(), "light".to_string()))
            );
            assert_eq!(
                assets
                    .sprite_sheets
                    .get(&def.mesh_id)
                    .map(|s| (s.category.clone(), s.id.clone())),
                Some(("fireefc".to_string(), "light2".to_string())),
                "the flat map keeps fire's `ligh`, the last one the walk saw"
            );

            assert_eq!(
                first_position(&assets, LIGH_DIR, &def).y,
                LIGH_FIRST_VERTEX_Y
            );
            assert_eq!(
                first_position(&assets, NO_LOCAL_DIR, &def).y,
                FIRE_FIRST_VERTEX_Y,
                "resolving ligh/lt05's sheet outside its directory draws fire's quad"
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod static_mesh_texture {
        use super::*;
        use ffxi_dat::texture::{DecodedTexture, TexFormat};

        const MESH_ID: [u8; 4] = *b"pou1";
        // ROM/97/59.DAT (`ele_ice`): the d3m names texture `pou`, whose backing Img chunk id is
        // `pou1`, so the truncated-DatId key and the name key disagree.
        const QUALIFIED: &[u8; 16] = b"ele_ice pou     ";
        const NAMESPACE: &str = "ele_ice";
        const LOCAL: &str = "pou";
        const IMG_DAT_ID: [u8; 4] = *b"pou1";

        fn one_pixel() -> DecodedTexture {
            DecodedTexture {
                width: 1,
                height: 1,
                format_tag: TexFormat::Bgra32,
                rgba: vec![255, 255, 255, 255],
            }
        }

        fn mesh_assets(qualified: bool, local: bool, dat_id: bool) -> ActionAssets {
            let mut assets = ActionAssets::default();
            let mut texture_name = [0u8; 16];
            texture_name.copy_from_slice(QUALIFIED);
            assets.d3ms.insert(
                MESH_ID,
                ffxi_dat::d3m::D3m {
                    name: MESH_ID,
                    num_triangles: 1,
                    texture_name,
                    vertices: vec![
                        ffxi_dat::d3m::D3mVertex {
                            pos: [0.0; 3],
                            normal: [0.0, 1.0, 0.0],
                            color: [1.0; 4],
                            uv: [0.0, 0.0],
                        };
                        3
                    ],
                },
            );
            if qualified {
                assets
                    .images_by_qualified_name
                    .insert((NAMESPACE.to_string(), LOCAL.to_string()), one_pixel());
            }
            if local {
                assets.images_by_name.insert(LOCAL.to_string(), one_pixel());
            }
            if dat_id {
                assets.images.insert(IMG_DAT_ID, one_pixel());
            }
            assets
        }

        fn mesh_def() -> ParticleGeneratorDef {
            let mut d = def(30.0, 1.0, 1);
            d.mesh_id = MESH_ID;
            d.mesh_kind = ffxi_dat::particle_gen::ParticleMeshKind::StaticMesh;
            d
        }

        fn resolved_texture(assets: &ActionAssets) -> Option<Handle<Image>> {
            let mut images = Assets::<Image>::default();
            resolve_mesh(assets, NO_LOCAL_DIR, &mesh_def(), &mut images, false)
                .expect("static mesh resolves")
                .2
        }

        // research/xim DatResource.kt getTextureResourceByNameAs — qualified (namespace, local) match first.
        #[test]
        fn static_mesh_texture_resolves_by_qualified_name() {
            assert!(resolved_texture(&mesh_assets(true, false, false)).is_some());
        }

        #[test]
        fn static_mesh_texture_falls_back_to_local_name() {
            assert!(resolved_texture(&mesh_assets(false, true, false)).is_some());
        }

        // The bug: `pou` truncated to the 4-byte key `pou ` never matched the `pou1` chunk id,
        // so the ice mesh drew untextured even though its Img was loaded.
        #[test]
        fn static_mesh_texture_does_not_need_the_name_to_equal_the_chunk_dat_id() {
            assert!(resolved_texture(&mesh_assets(false, false, true)).is_none());
            assert!(resolved_texture(&mesh_assets(true, false, true)).is_some());
        }

        // ROM file 173 (`cld1`/`clo1`, `kumori`) only ever resolves through the truncated id.
        #[test]
        fn static_mesh_texture_keeps_the_dat_id_as_a_last_tier() {
            let mut assets = mesh_assets(false, false, false);
            let mut texture_name = [0u8; 16];
            texture_name.copy_from_slice(b"cld1    kumori  ");
            assets.d3ms.get_mut(&MESH_ID).unwrap().texture_name = texture_name;
            assets.images.insert(*b"kumo", one_pixel());
            assert!(resolved_texture(&assets).is_some());
        }

        #[test]
        fn static_mesh_texture_is_none_when_no_tier_matches() {
            assert!(resolved_texture(&mesh_assets(false, false, false)).is_none());
        }

        // A mesh that names no texture must not claim the blank key: 44 d3ms in this install
        // carry an all-blank qualified name, and a single blank-keyed Img would give every one
        // of them the same wrong texture.
        #[test]
        fn static_mesh_texture_ignores_the_name_tiers_when_the_name_is_blank() {
            let mut assets = mesh_assets(false, false, false);
            assets.d3ms.get_mut(&MESH_ID).unwrap().texture_name = [b' '; 16];
            assets
                .images_by_qualified_name
                .insert((String::new(), String::new()), one_pixel());
            assets.images_by_name.insert(String::new(), one_pixel());

            assert!(resolved_texture(&assets).is_none());
        }

        fn texture_for(assets: &ActionAssets, def: &ParticleGeneratorDef) -> Option<Handle<Image>> {
            let mut images = Assets::<Image>::default();
            resolve_mesh(assets, NO_LOCAL_DIR, def, &mut images, false)
                .expect("mesh resolves")
                .2
        }

        // ROM/97/59.DAT `ele_ice`: the d3m names texture `pou` while the Img chunk id is `pou1`,
        // so the truncated-DatId key left the ice mesh untextured.
        #[test]
        fn real_dat_static_mesh_resolves_a_texture_its_chunk_id_does_not_name() {
            const ELE_ICE_FILE_ID: u32 = 1309;
            let Some(assets) = retail_assets(ELE_ICE_FILE_ID) else {
                return;
            };
            let d3m = assets.d3ms.get(&MESH_ID).expect("ele_ice ships a pou1 d3m");
            assert_eq!(
                d3m.texture_name_tokens(),
                (NAMESPACE.to_string(), LOCAL.to_string())
            );
            assert!(!assets.images.contains_key(&d3m.texture_dat_id()));

            let mut def = mesh_def();
            def.mesh_id = MESH_ID;
            assert!(texture_for(&assets, &def).is_some());
        }

        // ROM3/0/0.DAT: sheet `lf01` is backed by a palettised 0xB1 Img, which never entered the
        // name-keyed maps while extract_texture_tokens accepted 0xA1 alone. The sheet tier has no
        // DatId fallback, so the leaf drew untextured.
        #[test]
        fn real_dat_sprite_sheet_resolves_a_palettised_texture() {
            const ENVIRONMENT_FILE_ID: u32 = 101;
            const LEAF_SHEET_ID: [u8; 4] = *b"lf01";
            let Some(assets) = retail_assets(ENVIRONMENT_FILE_ID) else {
                return;
            };
            let sheet = assets
                .sprite_sheets
                .get(&LEAF_SHEET_ID)
                .expect("environment dat ships an lf01 sheet");
            assert!(assets
                .images_by_qualified_name
                .contains_key(&(sheet.category.clone(), sheet.id.clone())));

            let mut def = mesh_def();
            def.mesh_id = LEAF_SHEET_ID;
            def.mesh_kind = ffxi_dat::particle_gen::ParticleMeshKind::SpriteSheet;
            assert!(texture_for(&assets, &def).is_some());
        }
    }

    // research/xim ParticleGeneratorAttachment.kt updateAssociatedPosition jointRefIdx,103,111,125 — which of the def's two joint
    // fields an attach type reads is fixed by the type, and the celestial/unattached ones read
    // neither.
    #[test]
    fn attach_joint_reference_follows_the_attach_type() {
        use ffxi_dat::particle_gen::AttachType;
        const SOURCE_JOINT: u8 = 48;
        const TARGET_JOINT: u8 = 49;
        let mut d = def(1.0, 1.0, 1);
        d.attach_joint_source = SOURCE_JOINT;
        d.attach_joint_target = TARGET_JOINT;

        for attach in [
            AttachType::SourceActor,
            AttachType::SourceActorTargetFacing,
            AttachType::SourceToTargetBasis,
            AttachType::ZoneActorA,
            AttachType::ZoneActorB,
            AttachType::ZoneActorC,
        ] {
            d.attach_type = attach;
            assert_eq!(
                attach_joint_reference(&d),
                Some(SOURCE_JOINT as usize),
                "{attach:?}"
            );
        }
        for attach in [
            AttachType::TargetActor,
            AttachType::TargetActorSourceFacing,
            AttachType::TargetToSourceBasis,
        ] {
            d.attach_type = attach;
            assert_eq!(
                attach_joint_reference(&d),
                Some(TARGET_JOINT as usize),
                "{attach:?}"
            );
        }
        for attach in [
            AttachType::None,
            AttachType::Sun,
            AttachType::Moon,
            AttachType::SourceActorWeapon,
        ] {
            d.attach_type = attach;
            assert_eq!(attach_joint_reference(&d), None, "{attach:?}");
        }
    }

    // research/xim ParticleGeneratorAttachment.kt resolveExtendedJoints — a mount's two footstep joints are rewritten
    // to reference 0 before resolution, and :284-303 takes SourceActorWeapon out of the joint path
    // entirely (its remap is PC-model-gated upstream and we carry no PC-model flag).
    #[test]
    fn attach_joint_reference_rewrites_the_joints_retail_rewrites() {
        use ffxi_dat::particle_gen::AttachType;
        let mut d = def(1.0, 1.0, 1);
        d.attach_type = AttachType::SourceActor;
        for joint in MOUNT_FOOTSTEP_JOINTS {
            d.attach_joint_source = joint;
            assert_eq!(
                attach_joint_reference(&d),
                Some(MOUNT_FOOTSTEP_REFERENCE),
                "footstep joint {joint}"
            );
        }

        d.attach_type = AttachType::SourceActorWeapon;
        for joint in [31u8, 32, 33, 34, 35, 36, 37, 54, 55, 56, 57, 58, 59, 60] {
            d.attach_joint_source = joint;
            assert_eq!(attach_joint_reference(&d), None, "weapon joint {joint}");
        }
    }

    // ROM/27/82.DAT `hm_s`, the HumeM skeleton whose reaction routines the melee chain walks
    // (scheduler_runtime.rs tests).
    const HUME_M_SKELETON_FILE: u32 = 7072;

    fn retail_hume_m_skeleton() -> Option<ffxi_dat::skel::Skeleton> {
        let root = ffxi_dat::archive::open_test_install()?;
        let loc = root.resolve(HUME_M_SKELETON_FILE).ok()?;
        let bytes = std::fs::read(loc.path_under(&root)).ok()?;
        ffxi_dat::resource_dir::ResourceDir::from_bytes(bytes)
            .collect_skeletons()
            .into_iter()
            .next()
    }

    // ROM/0/0.DAT as shipped (scheduler_runtime.rs parse_action_bytes,
    // GLOBAL_EFFECT_DIR_FILE_ID): the melee hit sparks the `chit` chain reaches, each attaching
    // to the victim at a nearest-joint selector.
    const HIT_SPARK_DIR: [u8; 4] = *b"hit1";
    const HIT_SPARK_JOINT_REFERENCE: u8 = 49;
    const HIT_SPARK_GENERATORS: [([u8; 4], ffxi_dat::particle_gen::AttachType); 4] = [
        (*b"g010", ffxi_dat::particle_gen::AttachType::TargetActor),
        (*b"g011", ffxi_dat::particle_gen::AttachType::TargetActor),
        (
            *b"g012",
            ffxi_dat::particle_gen::AttachType::TargetActorSourceFacing,
        ),
        (*b"g013", ffxi_dat::particle_gen::AttachType::TargetActor),
    ];

    fn retail_global_effect_assets() -> Option<crate::scheduler_runtime::ActionAssets> {
        let root = ffxi_dat::archive::open_test_install()?;
        let loc = root
            .resolve(crate::scheduler_runtime::GLOBAL_EFFECT_DIR_FILE_ID)
            .ok()?;
        let bytes = std::fs::read(loc.path_under(&root)).ok()?;
        Some(crate::scheduler_runtime::parse_action_bytes(&bytes).1)
    }

    // Directory-scoped, because ROM/0/0.DAT defines `g010` several times over and only the `hit1`
    // copy is the spark (scheduler_runtime.rs tests).
    fn retail_hit_spark_defs() -> Option<Vec<([u8; 4], ParticleGeneratorDef)>> {
        let assets = retail_global_effect_assets()?;
        Some(
            HIT_SPARK_GENERATORS
                .iter()
                .map(|(name, _)| {
                    let def = assets.particle_def(HIT_SPARK_DIR, name).unwrap_or_else(|| {
                        panic!("ROM/0/0.DAT hit1 defines {}", String::from_utf8_lossy(name))
                    });
                    (*name, *def)
                })
                .collect(),
        )
    }

    // Pinned against the install: every `hit1` spark generator attaches to the TARGET actor and
    // names a nearest-joint selector there (ffxi-dat/src/particle_gen.rs
    // ParticleGeneratorDef attach fields), so the spawn origin cannot be the victim's root
    // transform alone.
    #[test]
    fn real_dat_hit_sparks_name_a_target_joint_reference() {
        let Some(defs) = retail_hit_spark_defs() else {
            return;
        };
        for ((name, def), (_, attach)) in defs.iter().zip(HIT_SPARK_GENERATORS) {
            let name = String::from_utf8_lossy(name).to_string();
            assert_eq!(def.attach_type, attach, "{name}");
            assert_eq!(def.attach_joint_target, HIT_SPARK_JOINT_REFERENCE, "{name}");
            assert_eq!(
                attach_joint_reference(def),
                Some(HIT_SPARK_JOINT_REFERENCE as usize),
                "{name} reads the target-side joint field"
            );
            assert_eq!(def.base_position, [0.0; 3], "{name}");
        }
    }

    // The joint the def names is resolved in the actor's pose frame (FFXI axes, -Y up) and must
    // arrive in Bevy world space (ffxi-actor/src/skeleton_instance.rs pose_world), i.e. ABOVE
    // the victim's feet and on the side the attacker stands on.
    #[test]
    fn real_dat_hit_spark_offset_lands_on_the_struck_side_in_bevy_space() {
        let (Some(skeleton), Some(defs)) = (retail_hume_m_skeleton(), retail_hit_spark_defs())
        else {
            return;
        };
        let pose = ffxi_actor::skeleton_instance::pose_world(
            &skeleton,
            |_| None,
            ffxi_actor::skeleton_instance::RootTransform::identity(),
            &[],
        );
        const VICTIM_WORLD: Vec3 = Vec3::new(30.0, 2.0, -14.0);
        const ATTACKER_REACH: f32 = 3.0;
        // Read off the same install the pose came from; the ring geometry itself is pinned by
        // ffxi-actor's `real_dat_retail_skeleton_resolves_the_nearest_joint_selector_onto_its_ring`
        // (ffxi-actor/src/skeleton_instance.rs). Pose space is -Y up, so the Bevy-space height is
        // its negation; anything at or below 0 is the feet bug.
        let ring_height_above_root = -ffxi_actor::skeleton_instance::standard_joint_world_position(
            &pose,
            &skeleton,
            *ffxi_actor::skeleton_instance::RING_JOINT_REFERENCES.start(),
        )
        .expect("the retail HumeM skeleton files its ring references")
        .y;
        assert!(
            ring_height_above_root > 0.0,
            "the ring must sit above the root, not at the feet: {ring_height_above_root}"
        );

        for victim_facing in [0.0, 1.0, 2.5, -2.0] {
            let root = Transform {
                translation: VICTIM_WORLD,
                rotation: Quat::from_rotation_y(victim_facing)
                    * crate::ffxi_actor_render::ffxi_to_bevy_basis(),
                scale: Vec3::ONE,
            }
            .compute_affine();
            for bearing in 0..8 {
                let a = bearing as f32 * std::f32::consts::TAU / 8.0;
                let toward = Vec3::new(a.cos(), 0.0, a.sin());
                let attacker = VICTIM_WORLD + toward * ATTACKER_REACH;
                for (name, def) in &defs {
                    let offset = attach_joint_offset(
                        def,
                        Some(AttachPose {
                            pose: &pose,
                            skeleton: &skeleton,
                            root,
                        }),
                        Some(attacker),
                    );
                    let name = String::from_utf8_lossy(name).to_string();
                    assert!(
                        (offset.y - ring_height_above_root).abs() < 1e-3,
                        "{name} spawned {offset:?}, not {ring_height_above_root} above the root"
                    );
                    assert!(
                        offset.dot(toward) > 0.0,
                        "{name} spawned {offset:?} away from the attacker at {attacker:?}"
                    );
                }
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn spawn_posed_actor(
        app: &mut App,
        skeleton: &ffxi_dat::skel::Skeleton,
        pose: &[Mat4],
        world: Vec3,
    ) -> Entity {
        let wire = app
            .world_mut()
            .spawn(Transform::from_translation(world))
            .id();
        app.world_mut().spawn((
            // ffxi_actor_render.rs spawn_live_actor's actor root, verbatim: the FFXI->Bevy basis
            // in the LOCAL transform and a default (identity) GlobalTransform until PostUpdate
            // runs.
            Transform::from_rotation(crate::ffxi_actor_render::ffxi_to_bevy_basis()),
            GlobalTransform::default(),
            crate::ffxi_actor_render::render_actor_for_test(skeleton.clone(), pose.to_vec()),
            ChildOf(wire),
        ));
        wire
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn particle_stage(gen_id: [u8; 4]) -> ffxi_dat::scheduler::TimedStage {
        ffxi_dat::scheduler::TimedStage {
            frame: 0,
            stage: ffxi_dat::scheduler::SchedulerStage {
                stage_words: ffxi_dat::scheduler::SYNTHESIZED_STAGE_WORDS,
                kind: ffxi_dat::scheduler::StageKind::Particle,
                raw_type: 0,
                delay_frames: 0,
                duration_frames: 0,
                id: gen_id,
                max_loops: 0,
                transition_in: 0,
                transition_out: 0,
                random_group: None,
                local_dir: HIT_SPARK_DIR,
                model_transform: None,
                follow_points: None,
                screen_color: None,
                actor_fade: None,
                idle_transition_time: None,
                flinch_duration: None,
                model_visibility: None,
                spell_effect: None,
            },
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn run_hit_spark_stage(
        skeleton: &ffxi_dat::skel::Skeleton,
        pose: &[Mat4],
        assets: crate::scheduler_runtime::ActionAssets,
        gen_id: [u8; 4],
        attacker_world: Vec3,
        victim_world: Vec3,
    ) -> Option<Vec3> {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<Image>()
            .init_asset::<FfxiParticleMaterial>()
            .init_resource::<ParticleSimulator>()
            .add_message::<crate::scheduler_runtime::SchedulerStageEvent>()
            .add_systems(Update, spawn_particle_generators);

        let attacker = spawn_posed_actor(&mut app, skeleton, pose, attacker_world);
        let victim = if victim_world == attacker_world {
            attacker
        } else {
            spawn_posed_actor(&mut app, skeleton, pose, victim_world)
        };
        app.world_mut()
            .entity_mut(attacker)
            .insert((assets, crate::scheduler_runtime::ActionTarget(Some(victim))));
        app.world_mut()
            .write_message(crate::scheduler_runtime::SchedulerStageEvent {
                actor: attacker,
                target: Some(victim),
                stage: particle_stage(gen_id),
                scheduler: HIT_SPARK_DIR,
            });
        app.update();
        app.world()
            .resource::<ParticleSimulator>()
            .generators
            .last()
            .map(|g| g.origin)
    }

    /// The whole wiring, driven through the real system rather than through
    /// `attach_joint_offset` alone: the actor root carrying the pose is a CHILD of the wire
    /// entity the stage fires on and PostUpdate has propagated nothing on the frame it is
    /// inserted, so the child descent, the local-transform composition, the source/target side
    /// of the selector and the `+ joint_offset` at the spawn site all have to hold for the
    /// spark to leave the victim's feet.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_dat_hit_spark_spawns_on_the_victims_ring_not_its_root() {
        let (Some(skeleton), Some(defs)) = (retail_hume_m_skeleton(), retail_hit_spark_defs())
        else {
            return;
        };
        let Some(assets) = retail_global_effect_assets() else {
            return;
        };
        let pose = ffxi_actor::skeleton_instance::pose_world(
            &skeleton,
            |_| None,
            ffxi_actor::skeleton_instance::RootTransform::identity(),
            &[],
        );
        let ring_height_above_root = -ffxi_actor::skeleton_instance::standard_joint_world_position(
            &pose,
            &skeleton,
            *ffxi_actor::skeleton_instance::RING_JOINT_REFERENCES.start(),
        )
        .expect("the retail HumeM skeleton files its ring references")
        .y;

        const VICTIM_WORLD: Vec3 = Vec3::new(30.0, 2.0, -14.0);
        const ATTACKER_WORLD: Vec3 = Vec3::new(33.0, 2.0, -14.0);
        for (gen_id, _) in &defs {
            let name = String::from_utf8_lossy(gen_id).to_string();
            let origin = run_hit_spark_stage(
                &skeleton,
                &pose,
                assets.clone(),
                *gen_id,
                ATTACKER_WORLD,
                VICTIM_WORLD,
            )
            .unwrap_or_else(|| panic!("{name} spawned no generator"));
            assert!(
                (origin.y - (VICTIM_WORLD.y + ring_height_above_root)).abs() < 1e-3,
                "{name} spawned at {origin:?}, not {ring_height_above_root} above the victim"
            );
            assert!(
                origin.x > VICTIM_WORLD.x,
                "{name} spawned at {origin:?}, not on the attacker's side of the victim"
            );

            // research/xim SkeletonInstance.kt getStandardJointExtended runs the same selector when source and target
            // are one actor (a self-cast Cure), so a self-targeted def still leaves the feet.
            let self_origin = run_hit_spark_stage(
                &skeleton,
                &pose,
                assets.clone(),
                *gen_id,
                VICTIM_WORLD,
                VICTIM_WORLD,
            )
            .unwrap_or_else(|| panic!("{name} spawned no self-targeted generator"));
            assert!(
                (self_origin.y - (VICTIM_WORLD.y + ring_height_above_root)).abs() < 1e-3,
                "self-targeted {name} spawned at {self_origin:?}, not on the ring"
            );
        }
    }

    /// With no second actor the selector has nothing to measure against, and with no posed
    /// skeleton (a door, or a model still loading) there is no joint at all: both must fall
    /// back to the plain root origin rather than throwing the effect somewhere arbitrary.
    #[test]
    fn attach_joint_offset_falls_back_to_the_root() {
        let Some(skeleton) = retail_hume_m_skeleton() else {
            return;
        };
        let pose = ffxi_actor::skeleton_instance::pose_world(
            &skeleton,
            |_| None,
            ffxi_actor::skeleton_instance::RootTransform::identity(),
            &[],
        );
        let mut d = def(1.0, 1.0, 1);
        d.attach_type = ffxi_dat::particle_gen::AttachType::TargetActor;
        d.attach_joint_target =
            *ffxi_actor::skeleton_instance::NEAREST_JOINT_REFERENCES.start() as u8;

        assert_eq!(attach_joint_offset(&d, None, Some(Vec3::X)), Vec3::ZERO);
        assert_eq!(
            attach_joint_offset(
                &d,
                Some(AttachPose {
                    pose: &pose,
                    skeleton: &skeleton,
                    root: bevy::math::Affine3A::IDENTITY,
                }),
                None,
            ),
            Vec3::ZERO
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_dat_level_up_linked_sparkle_emits_a_rotated_cross_and_bounds_cycles() {
        use crate::scheduler_runtime::{parse_action_bytes, LEVEL_UP_EFFECT_DAT_ID};
        const SOURCE: [u8; 4] = *b"g001";
        const LINK: [u8; 4] = *b"g002";
        const ACTOR_POSITION: Vec3 = Vec3::new(11.0, 4.0, -7.0);
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let bytes = std::fs::read(
            root.resolve(LEVEL_UP_EFFECT_DAT_ID)
                .unwrap()
                .path_under(&root),
        )
        .unwrap();
        let (schedulers, mut assets, _) = parse_action_bytes(&bytes);
        let stage = schedulers
            .iter()
            .flat_map(|s| &s.stages)
            .find(|s| s.stage.kind == StageKind::Particle && s.stage.id == SOURCE)
            .copied()
            .unwrap();
        let source = *assets.particle_def(stage.stage.local_dir, &SOURCE).unwrap();
        assert_eq!(source.immediate_generator, Some(LINK));
        assert_eq!(source.child_generator, None);
        let (dir, _) = assets
            .particle_def_scoped(stage.stage.local_dir, &LINK)
            .unwrap();
        assets
            .particle_defs_by_dir
            .get_mut(&(dir, LINK))
            .unwrap()
            .immediate_generator = Some(SOURCE);
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<Image>()
            .init_asset::<FfxiParticleMaterial>()
            .init_resource::<ParticleSimulator>()
            .add_message::<SchedulerStageEvent>()
            .add_systems(Update, spawn_particle_generators);
        let skeleton = retail_hume_m_skeleton().expect("installed HumeM skeleton is readable");
        let pose = ffxi_actor::skeleton_instance::pose_world(
            &skeleton,
            |_| None,
            ffxi_actor::skeleton_instance::RootTransform::identity(),
            &[],
        );
        let turn = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let joint = attach_joint_reference(&source)
            .and_then(|reference| {
                ffxi_actor::skeleton_instance::attach_joint_position(
                    &pose,
                    &skeleton,
                    reference,
                    Some(Vec3::ZERO),
                )
            })
            .unwrap_or(Vec3::ZERO);
        let expected_origin = ACTOR_POSITION
            + turn * crate::ffxi_actor_render::ffxi_to_bevy_basis() * joint
            + Vec3::Y * source.base_position[1];
        let actor = spawn_posed_actor(&mut app, &skeleton, &pose, ACTOR_POSITION);
        app.world_mut().entity_mut(actor).insert((
            Transform::from_translation(ACTOR_POSITION).with_rotation(turn),
            assets,
            crate::scheduler_runtime::ActionTarget(Some(actor)),
        ));
        app.world_mut().write_message(SchedulerStageEvent {
            actor,
            target: Some(actor),
            stage,
            scheduler: *b"main",
        });
        app.update();
        let mut sim = app.world_mut().resource_mut::<ParticleSimulator>();
        assert_eq!(
            sim.generators.len(),
            2,
            "cyclic links must not allocate forever"
        );
        let first_delta = sim.generators[0].def.frames_per_emission;
        advance_simulator(&mut sim, first_delta);
        let parent = &sim.generators[0];
        let child = &sim.generators[1];
        assert!(!parent.particles.is_empty());
        assert_eq!(child.particles.len(), parent.particles.len());
        assert_eq!(child.particles[0].pos, parent.particles[0].pos);
        assert_eq!(child.origin, parent.origin);
        assert!(parent.origin.distance(expected_origin) < AXIS_TOLERANCE);
        assert!(
            particle_draw(child, &child.particles[0], &sim.clock)
                .world
                .distance(expected_origin + parent.particles[0].pos)
                < AXIS_TOLERANCE,
            "the actor transform must not apply twice"
        );
        sim.generators[0].stopped = true;
        advance_simulator(&mut sim, ROUTINE_FPS / 4.0);
        let child = &sim.generators[1];
        let mut mesh = empty_mesh();
        rebuild_mesh(
            child,
            view(Quat::IDENTITY),
            &CelestialClock::default(),
            &mut mesh,
        );
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .unwrap()
            .as_float3()
            .unwrap();
        let template = &child.template.positions;
        let (a, b) = (0..template.len())
            .flat_map(|a| (a + 1..template.len()).map(move |b| (a, b)))
            .find(|&(a, b)| template[a].x == template[b].x && template[a].y != template[b].y)
            .unwrap();
        let edge = Vec3::from_array(positions[a]) - Vec3::from_array(positions[b]);
        const AXIS_TOLERANCE: f32 = 0.001;
        assert!(edge.x.abs() > 0.0);
        assert!(
            edge.y.abs() < edge.x.abs() * AXIS_TOLERANCE,
            "the authored vertical edge rotates onto the horizontal camera axis: {edge:?}"
        );
        let entities = sim.drain_entities();
        assert_eq!(
            entities.len(),
            2,
            "session cleanup drains parent and linked mesh ownership"
        );
        assert!(sim.generators.is_empty());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_dat_level_up_zero_window_emits_and_rises_in_bevy_space() {
        use crate::scheduler_runtime::{parse_action_bytes, LEVEL_UP_EFFECT_DAT_ID};
        const LETTERING_MESH: [u8; 4] = *b"lvu1";
        const FIRST_TICK_FRAMES: f32 = ROUTINE_FPS;
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            eprintln!("SKIP: level-up DAT test needs a registered install");
            return;
        };
        let bytes = std::fs::read(
            root.resolve(LEVEL_UP_EFFECT_DAT_ID)
                .unwrap()
                .path_under(&root),
        )
        .unwrap();
        let g000_raw = ffxi_dat::chunk::walk(&bytes)
            .flatten()
            .find(|c| c.name == *b"g000")
            .expect("level-up DAT contains the lettering generator");
        const TEST_PINNED_POSITION_HEADER: u32 = 0x8102;
        const TEST_PINNED_DAMPING_HEADER: u32 = 0x832c;
        let header_offset = |header: u32| {
            g000_raw
                .data
                .windows(size_of::<u32>())
                .position(|bytes| bytes == header.to_le_bytes())
                .expect("level-up DAT carries the updater header")
        };
        assert!(
            header_offset(TEST_PINNED_POSITION_HEADER) < header_offset(TEST_PINNED_DAMPING_HEADER)
        );
        let (schedulers, assets, _) = parse_action_bytes(&bytes);
        let stage = schedulers
            .iter()
            .flat_map(|s| &s.stages)
            .find(|s| {
                s.stage.kind == StageKind::Particle
                    && assets
                        .particle_def(s.stage.local_dir, &s.stage.id)
                        .is_some_and(|d| d.mesh_id == LETTERING_MESH)
            })
            .copied()
            .expect("level-up DAT schedules its lettering generator");
        assert_eq!(stage.stage.duration_frames, 0);
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<Image>()
            .init_asset::<FfxiParticleMaterial>()
            .init_resource::<ParticleSimulator>()
            .add_message::<SchedulerStageEvent>()
            .add_systems(Update, spawn_particle_generators);
        let skeleton = retail_hume_m_skeleton().expect("installed HumeM skeleton is readable");
        let pose = ffxi_actor::skeleton_instance::pose_world(
            &skeleton,
            |_| None,
            ffxi_actor::skeleton_instance::RootTransform::identity(),
            &[],
        );
        const ACTOR_WORLD: Vec3 = Vec3::new(7.0, 2.0, -3.0);
        let def = assets
            .particle_def(stage.stage.local_dir, &stage.stage.id)
            .unwrap();
        let joint = ffxi_actor::skeleton_instance::standard_joint_world_position(
            &pose,
            &skeleton,
            def.attach_joint_source as usize,
        )
        .expect("the authored lettering source joint exists");
        let expected_origin = ACTOR_WORLD
            + crate::ffxi_actor_render::ffxi_to_bevy_basis() * joint
            + Vec3::Y * def.base_position[1];
        let actor = spawn_posed_actor(&mut app, &skeleton, &pose, ACTOR_WORLD);
        app.world_mut().entity_mut(actor).insert(assets);
        app.world_mut().write_message(SchedulerStageEvent {
            actor,
            target: None,
            stage,
            scheduler: *b"main",
        });
        app.update();
        let mut sim = app.world_mut().resource_mut::<ParticleSimulator>();
        let g = sim
            .generators
            .iter_mut()
            .find(|g| g.def.mesh_id == LETTERING_MESH)
            .unwrap();
        assert!(
            g.origin
                .abs_diff_eq(expected_origin, f32::EPSILON * ROUTINE_FPS),
            "lettering origin {:?} must follow the posed authored joint {:?}",
            g.origin,
            expected_origin
        );
        assert!(g.def.frames_per_emission > FIRST_TICK_FRAMES);
        assert!(g.def.init_velocity[1] < 0.0);
        advance_generator(g, FIRST_TICK_FRAMES);
        assert_eq!(g.particles.len(), 1);
        let initial_velocity = g.particles[0].vel;
        let authored_damping = g
            .def
            .velocity_dampener
            .expect("level-up lettering has authored damping")[0];
        advance_generator(g, FIRST_TICK_FRAMES);
        assert_eq!(
            g.particles[0].vel,
            initial_velocity * authored_damping.powf(FIRST_TICK_FRAMES)
        );
        assert_eq!(g.particles.len(), 1);
        assert!(
            g.particles[0].pos.y > 0.0,
            "authored upward DAT motion rises in Bevy"
        );
        assert_eq!(
            g.vel_basis,
            crate::scene::mzb_to_bevy(kuluu_snapshot::Vec3 {
                x: Vec3::ONE.x,
                y: Vec3::ONE.y,
                z: Vec3::ONE.z
            })
        );
        let life_frames = g.def.max_life_frames;
        advance_generator(g, life_frames);
        assert!(
            g.particles.is_empty(),
            "zero-window burst expires without respawning"
        );
    }
}
