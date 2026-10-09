use std::sync::OnceLock;
use std::time::Instant;

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

// The per-particle TEXTUREFACTOR (F) rides the TANGENT slot: Bevy's material pipeline only
// exposes standard attributes at fixed shader locations (bevy_pbr render/mesh.rs
// MeshPipeline::specialize — position 0, normal 1, uv 2, uv_b 3, tangent 4, color 5), and a
// custom attribute id never reaches the vertex buffer layout. TANGENT is the one standard
// vec4 slot these meshes do not otherwise use; ffxi_particle.wgsl reads it at location 4.

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
#[derive(Clone, Copy, Debug)]
pub struct CelestialClock {
    pub day_fraction: f32,
    pub day_of_week: usize,
    pub moon_phase: usize,
    // The animation-test slider values: lamp_halos_lift is the lamp halos' (PATH_LAMP_ALPHAMAP)
    // peak wall-brighten; wash_alpha_lift multiplies the authored alpha of the ghu*/li* wall-wash
    // volumes (1.0 = exactly as authored). Both live on the clock so every draw site reads them
    // without new plumbing.
    pub lamp_halos_lift: f32,
    pub wash_alpha_lift: f32,
    // Seconds accumulator for the halo flicker wave (the DAT ships no flicker keyframes; retail
    // wavers at runtime, so both modes ride the hand-tuned `lamp_flicker` model on this clock).
    pub lamp_flicker_phase: f32,
}

impl Default for CelestialClock {
    fn default() -> Self {
        Self {
            day_fraction: 0.0,
            day_of_week: 0,
            moon_phase: 0,
            lamp_halos_lift: LAMP_ALPHAMAP_LIFT_DEFAULT,
            wash_alpha_lift: WASH_ALPHA_LIFT_DEFAULT,
            lamp_flicker_phase: 0.0,
        }
    }
}

impl ParticleSimulator {
    pub fn drain_entities(&mut self) -> Vec<Entity> {
        self.generators.drain(..).map(|g| g.entity).collect()
    }

    pub fn set_celestial_clock(&mut self, mut clock: CelestialClock) {
        clock.lamp_halos_lift = self.clock.lamp_halos_lift;
        clock.wash_alpha_lift = self.clock.wash_alpha_lift;
        clock.lamp_flicker_phase = self.clock.lamp_flicker_phase;
        self.clock = clock;
    }

    pub fn set_lamp_halos_lift(&mut self, lift: f32) {
        self.clock.lamp_halos_lift = lift.clamp(0.0, 1.0);
    }

    /// Wall-wash brightness knob: multiplier on the ghu*/li* volumes' authored alpha
    /// (1.0 = exactly as authored).
    pub fn set_wash_alpha_lift(&mut self, lift: f32) {
        self.clock.wash_alpha_lift = lift.clamp(0.0, WASH_ALPHA_LIFT_MAX);
    }

    // The animation-room sliders read and reset these through the simulator; the
    // room's teardown restores the shipped defaults so a tester visit cannot leak a
    // lighting change into a session.
    pub fn reset_test_lighting(&mut self) {
        self.clock.lamp_halos_lift = LAMP_ALPHAMAP_LIFT_DEFAULT;
        self.clock.wash_alpha_lift = WASH_ALPHA_LIFT_DEFAULT;
    }

    pub fn lamp_halos_lift(&self) -> f32 {
        self.clock.lamp_halos_lift
    }

    pub fn wash_alpha_lift(&self) -> f32 {
        self.clock.wash_alpha_lift
    }

    /// The Vana'diel day fraction the time-of-day tracks sample at (the zone lighting's clock —
    /// the ambient mixer multiplies its ToD volume track by this, as retail's ClockValueUpdater does).
    pub fn clock(&self) -> CelestialClock {
        self.clock
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

// The fixed-function colour tables live in ffxi_particle.wgsl (per stage, with D3D8 saturation
// after every op): CMoD3m.cpp's TSS blocks for the D3m path (NonZeroTwoTSS textured default,
// NonZeroOneTSS when renderStateFlags 0x1000 drops the texture alpha, ZeroOneTSS untextured) and
// ZoneRenderer.cpp DoD3mDraw's tables for MMB meshes. The path selects which table; nothing is
// pre-scaled on the CPU — D3m/sheet template colours carry stage 0's MODULATE2X via the /128
// normalise (ffxi_dat::d3m::VERTEX_COLOR_DIVISOR), MMB colours are raw byte/255, and the factor
// attribute carries the particle's TEXTUREFACTOR.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum D3mDrawPath {
    // CMoD3m.cpp NonZeroTwoTSS / NonZeroOneTSS (the ignore flag picks between them).
    D3m,
    // ZoneRenderer.cpp DoD3mDraw textured tables.
    Mmb,
    // The untextured pair shares one stage table, MODULATE2X/MODULATE4X against TFACTOR:
    // CMoD3m.cpp ZeroOneTSS for a textureless D3m submesh, DoD3mDraw's one-stage table for an
    // MMB without its texture. They stay apart because only CMoD3m::Draw promotes TFACTOR alpha.
    D3mUntextured,
    MmbUntextured,
}

fn d3m_draw_path(tex: &Option<Handle<Image>>) -> D3mDrawPath {
    if tex.is_some() {
        D3mDrawPath::D3m
    } else {
        D3mDrawPath::D3mUntextured
    }
}

// CMoD3mElem.cpp CMoD3mElem::DoMMBDraw — DoMMBDraw forces the ignore-texture-alpha table at this blend byte,
// whatever the render-state bit says.
const D3M_MMB_FORCE_IGNORE_TEXTURE_ALPHA_BLEND_BYTE: u8 = 0x64;
// CMoD3m.cpp CMoD3m::Draw — at blend byte 0x44 a TEXTUREFACTOR alpha at or above 0x7F is promoted to
// 0xFF before the stage math. DoMMBDraw carries no such promotion.
const D3M_TFACTOR_PROMOTE_BLEND_BYTE: u8 = 0x44;
const D3M_TFACTOR_PROMOTE_MIN: f32 = 0x7F as f32 / u8::MAX as f32;
const D3M_TFACTOR_PROMOTED: f32 = 1.0;

// Lamp halo billboards — the soft glow sheets retail authors under ligh/ (SG's lig* quad,
// Bastok's lt__/gl*/lp* families, the lglt wall wash): additive sprite sheets whose whole
// brightness is their ToD gate. The class is content-based so every zone's lanterns ride one
// code path: lift/flicker/range knobs and Enhanced-mode replacement all apply identically.
const LAMP_HALO_INIT_ALPHA_MAX: f32 = 0.2;

pub(crate) fn is_lamp_halo_def(def: &ParticleGeneratorDef) -> bool {
    def.mesh_kind == ParticleMeshKind::SpriteSheet
        && def.blend == ffxi_dat::particle_gen::ParticleBlend::Additive
        && def.tod_color_driven[TOD_ALPHA_CHANNEL]
        // Halos author a near-zero alpha (SG lig2 0.071, Bastok lt__ 0.0) and let the ToD gate
        // do all the work; flame sheets carry their own authored alpha (fi* 0.5) and stay out.
        && def.init_color[TOD_ALPHA_CHANNEL] <= LAMP_HALO_INIT_ALPHA_MAX
}

const WALL_WASH_MESH_ID: [u8; 4] = *b"ligh";

pub(crate) fn is_wall_wash_def(def: &ParticleGeneratorDef) -> bool {
    def.mesh_id == WALL_WASH_MESH_ID
        && def.blend == ffxi_dat::particle_gen::ParticleBlend::Additive
        && matches!(
            def.mesh_kind,
            ParticleMeshKind::StaticMesh | ParticleMeshKind::WeightedMesh
        )
}

/// The AnimationTest box's lamps kill switch (panel checkbox): while set, the tick hides every
/// lig* halo billboard (and restores it on uncheck), so the lantern glow sprites draw nothing.
/// Only the box inserts it; without it (a real session) halos render as authored.
#[derive(Resource, Default)]
pub struct LampHalosOff(pub bool);

/// The AnimationTest box's wall-glow kill switch (panel checkbox): while set, the tick hides
/// every additive ligh wash volume (`is_wall_wash_def`) and restores it on uncheck. Only the box
/// inserts it; without it (a real session) the washes render as authored.
#[derive(Resource, Default)]
pub struct WallWashOff(pub bool);

/// Halo suppression marker (AnimationTest lamps/enhance/wall-glow rows): while present,
/// `sync_particle_meshes` keeps the mesh hidden whatever the frustum says — tick runs before the
/// culler, so a direct Visibility write from there would be overwritten every frame. The tick
/// inserts/removes it per checkbox state, live both ways.
#[derive(Component)]
pub struct HaloSuppressed;

// A hand tuning, not a measured retail value: at 0.12 the halo reads as lantern light on stone
// without washing out the wall texture; past roughly 0.6 it covers the stone instead of
// lighting it.
// pub: the AnimationTest box seeds its lantern-alpha slider from this default.
pub const LAMP_ALPHAMAP_LIFT_DEFAULT: f32 = 0.12;
// Wall-wash slider seed (1.0 = authored alpha) and ceiling. The seed has to be
// the identity multiplier: it scales authored wash alpha, so any other default
// dims every wall wash in a normal session rather than only inside the tester.
pub const WASH_ALPHA_LIFT_DEFAULT: f32 = 1.0;
pub const WASH_ALPHA_LIFT_MAX: f32 = 2.0;
// The DAT ships no flicker keyframes, so each halo's phase in the shared `lamp_flicker` wave is
// derived from its mesh id: halos of different families waver out of step. The stride is a hand
// tuning.
const LAMP_FLICKER_SEED_STRIDE: f32 = 0.37;

fn lamp_flicker_seed(def: &ParticleGeneratorDef) -> f32 {
    def.mesh_id.iter().map(|b| f32::from(*b)).sum::<f32>() * LAMP_FLICKER_SEED_STRIDE
}

pub(crate) fn ignores_texture_alpha(def: &ParticleGeneratorDef, path: D3mDrawPath) -> bool {
    def.ignore_texture_alpha
        || (path == D3mDrawPath::Mmb
            && def.blend_byte == D3M_MMB_FORCE_IGNORE_TEXTURE_ALPHA_BLEND_BYTE)
}

fn tfactor_alpha(def: &ParticleGeneratorDef, path: D3mDrawPath, alpha: f32) -> f32 {
    if matches!(path, D3mDrawPath::D3m | D3mDrawPath::D3mUntextured)
        && def.blend_byte == D3M_TFACTOR_PROMOTE_BLEND_BYTE
        && alpha >= D3M_TFACTOR_PROMOTE_MIN
    {
        D3M_TFACTOR_PROMOTED
    } else {
        alpha
    }
}

// research/xim Particle.kt getColor — the day-of-week / moon-phase tints are applied with
// Color.modulateInPlace(c, 2f), a 2x modulate.
const CELESTIAL_MODULATE: f32 = 2.0;
// Index of the alpha channel in the 0x60..0x63 time-of-day track array (0x63 -> 0x3F).
const TOD_ALPHA_CHANNEL: usize = 3;

// The parent particle's state a child generator copies when its own def carries the sec2
// 0x45..0x49 parent-copy blocks (research/xim ParticleInitializers.kt Parent*Config). Bevy world
// space, because every child generator is world-space: `pos` is where the anchor particle draws,
// `vel` its total velocity.
#[derive(Clone, Copy)]
struct AnchorState {
    pos: Vec3,
    vel: Vec3,
    rotation: Vec3,
    rgb: Vec3,
    scale: Vec2,
}

impl AnchorState {
    // research/xim Particle.kt getTotalVelocity — the velocityRotation rotates the total
    // velocity; the anchor captures what this engine integrates.
    fn from_particle(g: &LiveGenerator, p: &Particle) -> Self {
        let total = p.vel + p.rel_vel;
        let vel = if p.vel_rot == Vec3::ZERO {
            total
        } else {
            velocity_rotation(p.vel_rot, p.negate_rotation_y) * total
        };
        AnchorState {
            pos: particle_world(g, p),
            vel: g.entity_world.affine().transform_vector3(vel),
            rotation: p.rotation,
            rgb: p.rgb,
            scale: p.scale,
        }
    }
}

// The payload a child binding resolves to at parent spawn time. Draw carries everything the
// mesh path needs; the non-draw kinds carry their resolved cues so instantiation calls the
// same arm_* dispatch the routine scheduler uses — a sec2 0x44 binding and a 0x02 stage that
// name the same def behave identically.
#[derive(Clone)]
struct ChildDraw {
    def: ParticleGeneratorDef,
    template: SpriteTemplate,
    sprite_frames: Vec<SpriteTemplate>,
    mat: Handle<FfxiParticleMaterial>,
    draw_path: D3mDrawPath,
    tracks: GeneratorTracks,
}

// A generator's keyframe channels, resolved against the DAT's keyframe chunks at spawn.
#[derive(Clone, Default)]
struct GeneratorTracks {
    scale_x: Option<KeyFrameTrack>,
    scale_y: Option<KeyFrameTrack>,
    // sec2 0x21..0x23 position tracks — each frame the bound track replaces the particle's
    // channel, key 0 seeded from its spawn-time value (research/xim ParticleUpdaters.kt
    // ProgressValueUpdater initialValueOverride; CYyGenerator.cpp ElemIdle cases 0x0F..0x11).
    position_x: Option<KeyFrameTrack>,
    position_y: Option<KeyFrameTrack>,
    position_z: Option<KeyFrameTrack>,
    // The sec2 0x69 velocity-dampener track, resolved only while the sec3 0x44 applier is
    // present — without it nothing samples the track (research/xim ParticleUpdaters.kt
    // VelocityDampener getDampeningFactor: transform.dampeningFactor ?: dampen).
    dampening_factor: Option<KeyFrameTrack>,
    alpha: Option<KeyFrameTrack>,
    // The 0x60..0x63 time-of-day RGBA curves. Sampled at the Vana'diel day fraction, so unlike
    // `alpha` above they do not advance with the particle's own life.
    tod_color: [Option<KeyFrameTrack>; ffxi_dat::particle_gen::TOD_COLOR_CHANNELS],
}

impl GeneratorTracks {
    // Zone generators fall back to the global tier; an effect-DAT generator passes None and
    // resolves only in the tier that holds its def.
    fn resolve(
        def: &ParticleGeneratorDef,
        assets: &ActionAssets,
        global: Option<&ActionAssets>,
    ) -> Self {
        let resolve = |id: Option<[u8; 4]>| keyframe(assets, global, id);
        GeneratorTracks {
            scale_x: resolve(def.scale_x_track),
            scale_y: resolve(def.scale_y_track),
            position_x: resolve(def.position_x_track),
            position_y: resolve(def.position_y_track),
            position_z: resolve(def.position_z_track),
            dampening_factor: def
                .dampening_factor_applier
                .then(|| resolve(def.velocity_dampener_track))
                .flatten(),
            alpha: resolve(def.alpha_track),
            tod_color: def.tod_color_tracks.map(resolve),
        }
    }
}

#[derive(Clone)]
enum ChildPayload {
    Draw(Box<ChildDraw>),
    Sound {
        se_id: u32,
        near: f32,
        far: f32,
        vertical_weight: f32,
    },
    Distortion {
        haze_offset: f32,
        life_frames: f32,
        envelope: Option<ffxi_dat::particle_gen::KeyFrameTrack>,
        // Built-in footprint at the authored scale, resolved once at factory time. None = a
        // degenerate scale; the child then arms nothing but keeps running.
        half_extent: Option<Vec2>,
    },
    Rumble {
        envelope: ffxi_dat::particle_gen::KeyFrameTrack,
        near: f32,
        far: f32,
        life_frames: f32,
    },
}

// A child generator resolved at parent spawn time (research/xim ParticleInitializers.kt
// ChildGeneratorSetup), so per-particle instantiation never touches the DAT again. `on_expiry`
// marks a sec4 0x01 binding — one burst when the parent particle dies (research/xim
// ParticleExpirationHandlers.kt EmitChildHandler). `children` are this def's own bindings,
// resolved recursively.
#[derive(Clone)]
struct ChildFactory {
    // The bound generator's name — the trace and diagnostics need it.
    name: [u8; 4],
    on_expiry: bool,
    payload: ChildPayload,
    children: Vec<ChildFactory>,
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
    tracks: GeneratorTracks,
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
    // in the DAT frame (ONE); world-space generators build positions directly in Bevy
    // space, so velocity gets the same mzb->bevy basis (x,-y,-z) as the origin.
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
    /// The attach frame's rotation and fit vectors (`AttachFrame`); identity and ONE for every
    /// generator kind but the routine-spawned ones.
    frame_rot: Quat,
    position_fit: Vec3,
    model_fit: Vec3,
    /// The sec2 0x01 setup position in the DAT frame, an element offset inside the attach frame
    /// rather than an origin shift. Zero for the zone, weather, camera and auto-run kinds, whose
    /// spawn sites fold it into the origin.
    setup_position: Vec3,
    // The mesh entity's GlobalTransform at the last sync — identity for a world-space generator,
    // the actor root's for an actor-local one. Draw-distance falloff (sec3 0x2E) measures
    // camera-to-particle in true world space, so the particle's local-frame position passes
    // through it before the distance is taken.
    entity_world: GlobalTransform,
    // The owning parent of a child generator: index into ParticleSimulator::generators plus the
    // id of the parent particle. None for top-level generators (research/xim Particle.kt —
    // children live on their parent particle and die with it).
    parent: Option<(usize, u64)>,
    // The anchor particle's state at the last sync; refreshed every frame while `parent` is
    // Some. None for top-level generators.
    anchor: Option<AnchorState>,
    // This def's child bindings, resolved at spawn (research/xim ParticleInitializers.kt
    // ChildGeneratorSetup + sec4 EmitChildHandler).
    child_factories: Vec<ChildFactory>,
    // Next unique id for a particle of this generator (child generators reference their parent
    // particle by it; ids only need to be unique within one generator).
    next_particle_id: u64,
    // Child-generator indices whose parent particle was reaped this tick, drained by the tick
    // system after advance.
    dead_child_gens: Vec<usize>,
    // (world position, factory index) pairs queued by reap for sec4 0x01 emit-on-expiry, drained
    // by the tick system after advance.
    pending_expiry_spawns: Vec<(Vec3, usize)>,
    // Key of the last BUILT mesh (spawn writes `empty_mesh`, hence `MeshKey::Empty`), so
    // quantization error is bounded by one quantum and never accumulates across skipped frames.
    built_key: MeshKey,
    /// `template_bound_radius`, resolved once at spawn: the unscaled radius of the widest
    /// template/flipbook frame, which `generator_bounds` scales per particle.
    bound_radius: f32,
}

impl LiveGenerator {
    // A world-space generator primed to emit on its first tick (research/xim
    // ParticleGenerator.kt emit starts framesUntilNextParticle at 0); spawn sites override
    // the fields their kind sets.
    fn new(
        def: ParticleGeneratorDef,
        tracks: GeneratorTracks,
        template: SpriteTemplate,
        sprite_frames: Vec<SpriteTemplate>,
        draw_path: D3mDrawPath,
        mesh: Handle<Mesh>,
        entity: Entity,
        origin: Vec3,
    ) -> Self {
        LiveGenerator {
            immediate_parent: None,
            solid_mesh: is_solid_mesh(&template),
            bound_radius: template_bound_radius(&template, &sprite_frames),
            def,
            template,
            draw_path,
            sprite_frames,
            tracks,
            origin,
            particles: Vec::new(),
            emit_accum: def.frames_per_emission,
            age_frames: 0.0,
            emit_window_frames: 0.0,
            mesh,
            entity,
            auto_run: false,
            orientation: None,
            actor_local: false,
            tex_translate: Vec2::ZERO,
            // World-space origin, so DAT velocities integrate through the mzb->bevy basis:
            // retail steps elements in FFXI space (CYyGenerator.cpp ElemIdle case 0x02) and
            // the attach matrix carries them to world at draw time.
            vel_basis: WORLD_PARTICLE_VEL_BASIS,
            origin_routine: None,
            stopped: false,
            camera_relative: false,
            emit_culled: false,
            emit_scale: UNSCALED_EMISSION,
            emit_rng: emit_seed(entity),
            elements_emitted: 0,
            cam_view: Quat::IDENTITY,
            actor_rot: Quat::IDENTITY,
            frame_rot: Quat::IDENTITY,
            position_fit: Vec3::ONE,
            model_fit: Vec3::ONE,
            setup_position: Vec3::ZERO,
            entity_world: GlobalTransform::IDENTITY,
            parent: None,
            anchor: None,
            child_factories: Vec::new(),
            next_particle_id: 0,
            dead_child_gens: Vec::new(),
            pending_expiry_spawns: Vec::new(),
            built_key: MeshKey::Empty,
        }
    }

    fn place_in(&mut self, frame: &AttachFrame) {
        self.origin = frame.translation;
        self.frame_rot = frame.rotation;
        self.position_fit = frame.position_fit;
        self.model_fit = frame.model_fit;
    }
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
    // The spawn-time offset from the generator origin: the sec2 0x21..0x23 position tracks
    // seed their opening segment from it (research/xim ParticleUpdaters.kt ProgressValueUpdater
    // — initialValueOverride is captured once, while null).
    spawn_pos: Vec3,
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
    // Euler radians of the particle transform's velocityRotation: accumulated by the sec3
    // 0x26 VelocityRotator and replaced wholesale by the sec3 0x2F VelocityRotationUpdater.
    // Zero while neither block is present, so the integration path stays a plain add.
    vel_rot: Vec3,
    // The sec2 0x19 color transform for this element (base plus one 0x1A variance draw per
    // channel), drifted by the sec3 0x0C modifier and applied to rgb by the sec3 0x0B
    // applier. None while the generator carries no setup — xim allocates the slot only from
    // ColorTransformSetup. The a channel is parsed and drifted but not applied: every shipped
    // transform's alpha is 0, and per-particle alpha has no draw path of its own.
    color_transform: Option<[i32; 4]>,

    // Some while the generator carries the sec2 0x3D marker (research/xim
    // ParticleGeneratorSettings.kt OscillationParams): the per-axis acceleration (0x3E/0x3F/
    // 0x40 base plus one variance draw) and the applier's last-amplitude memory.
    osc: Option<Oscillation>,
    // Unique within its generator; child generators reference their parent particle by it
    // (research/xim Particle.kt — children live on the parent particle).
    id: u64,
    // Indices into ParticleSimulator::generators of this particle's child generators.
    child_gens: Vec<usize>,
    // Child-factory indices awaiting entity spawn, drained by the tick system after emit.
    pending_child_factories: Vec<usize>,
}

// research/xim ParticleGeneratorSettings.kt OscillationParams — the per-particle oscillation
// state the sec3 appliers integrate: per-axis acceleration and the applier's previous-amplitude
// memory. [0]/[1]/[2] are the X/Y/Z axes.
#[derive(Clone)]
struct Oscillation {
    accel: [f32; 3],
    prev_amplitude: [f32; 3],
}

// .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Reference
// point — the aliases a reference index passes through before the locator table: 54/55 are the
// hand locators and 56-60 the five extra weapon locators. 52/53 are the actor's last touch-floor
// and water-surface points, which this client does not track, so they fall back to the root.
const MOUNT_FOOTSTEP_JOINTS: std::ops::RangeInclusive<u8> = 52..=53;
const MOUNT_FOOTSTEP_REFERENCE: usize = 0;
const HAND_REFERENCE_ALIASES: std::ops::RangeInclusive<u8> = 54..=55;
const EXTRA_WEAPON_REFERENCE_ALIASES: std::ops::RangeInclusive<u8> = 56..=60;
const EXTRA_WEAPON_REFERENCE_FIRST: usize = 102;
// Reference 51 selects the ring entry nearest the camera eye; 49 and 50 the one nearest the
// other actor.
const CAMERA_NEAREST_REFERENCE: u8 = 51;

fn resolve_reference_alias(reference: u8) -> usize {
    if MOUNT_FOOTSTEP_JOINTS.contains(&reference) {
        MOUNT_FOOTSTEP_REFERENCE
    } else if HAND_REFERENCE_ALIASES.contains(&reference) {
        ffxi_dat::skel::standard_position::LEFT_HAND
            + usize::from(reference - HAND_REFERENCE_ALIASES.start())
    } else if EXTRA_WEAPON_REFERENCE_ALIASES.contains(&reference) {
        EXTRA_WEAPON_REFERENCE_FIRST
            + usize::from(reference - EXTRA_WEAPON_REFERENCE_ALIASES.start())
    } else {
        usize::from(reference)
    }
}

// The mzb->bevy axis mapping (dat_mzb.rs to_bevy) for world-space particle math: FFXI's -Y up
// becomes Bevy +Y up and Z mirrors, so a DAT velocity/spread authored in the FFXI frame lands
// where retail puts it.
const WORLD_PARTICLE_VEL_BASIS: Vec3 = Vec3::new(1.0, -1.0, -1.0);

// research/xim ParticleUpdaters.kt VelocityRotator — "TODO - 0.5 is needed for Ice Spikes to
// work": the rotateAmount accumulates into the velocity rotation at half the authored rate per
// frame. No XIClient symbol found; flagged for retail verification.
const VELOCITY_ROTATOR_RATE_HALF: f32 = 0.5;

// research/xim ParticleUpdaters.kt ColorTransformApplier — the transform's shr-7 value is
// added to the colour at half rate per frame.
const COLOR_TRANSFORM_STEP_HALF: f32 = 0.5;

// research/xim ParticleUpdaters.kt ColorTransformModifier — the drift divisor: the modifier
// accumulates floor(modifier × frames/30) into the transform each frame.
const COLOR_TRANSFORM_MODIFIER_RATE_FRAMES: f32 = 30.0;

// The particle colour is a D3DCOLOR (byte/255), so xim's raw-byte-space transform delta
// normalises against the full byte range to land on p.rgb.
const COLOR_TRANSFORM_BYTE_SCALE: f32 = u8::MAX as f32;

// research/xim Particle.kt getTotalVelocity — the velocityRotation rotates the total velocity
// before integration, negate_rotation_y flipping the y angle sign (yRotationMultiplier). xim's
// Matrix4f.rotateZYXInPlace builds the transpose of the standard ZYX Euler product,
// Rx(−x)·Ry(−y)·Rz(−z), which is glam's intrinsic XYZ euler order at negated angles.
fn velocity_rotation(vel_rot: Vec3, negate_y: bool) -> Quat {
    let y = if negate_y { -vel_rot.y } else { vel_rot.y };
    Quat::from_euler(EulerRot::XYZ, -vel_rot.x, -y, -vel_rot.z)
}

/// Everything the attach frame reads from the world: the attach actors' transforms and posed
/// skeletons, and the operator camera for the eye-nearest selector.
#[derive(bevy::ecs::system::SystemParam)]
pub struct AttachQueries<'w, 's> {
    xf: Query<'w, 's, &'static Transform>,
    children: Query<'w, 's, &'static Children>,
    render: Query<'w, 's, &'static FfxiRenderActor>,
    cam: Query<'w, 's, &'static GlobalTransform, With<OperatorCamera>>,
}

impl AttachQueries<'_, '_> {
    fn camera_eye(&self) -> Option<Vec3> {
        self.cam.iter().next().map(|xf| xf.translation())
    }
}

/// The pose an attach actor was last drawn in, the transform carrying its pose frame (FFXI
/// axes, -Y up) into Bevy world space, and the facing and scale the pose was built under.
#[derive(Clone, Copy)]
struct AttachPose<'a> {
    pose: &'a [Mat4],
    skeleton: &'a ffxi_dat::skel::Skeleton,
    root: bevy::math::Affine3A,
    facing_dir: f32,
    scale: f32,
}

impl AttachPose<'_> {
    // The actor's facing in Bevy space: the root's rotation stripped of the FFXI->Bevy basis,
    // plus the facing the pose itself was composed under (a yaw about FFXI Y turns the other
    // way once the basis flips that axis).
    fn yaw(&self) -> Quat {
        let (_, rotation, _) = self.root.to_scale_rotation_translation();
        rotation
            * crate::ffxi_actor_render::ffxi_to_bevy_basis().inverse()
            * Quat::from_rotation_y(-self.facing_dir)
    }
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
fn attach_pose<'a>(entity: Entity, q: &'a AttachQueries<'_, '_>) -> Option<AttachPose<'a>> {
    let (actor, holder) = q
        .render
        .get(entity)
        .ok()
        .map(|actor| (actor, entity))
        .or_else(|| {
            q.children
                .get(entity)
                .ok()?
                .iter()
                .find_map(|child| Some((q.render.get(child).ok()?, child)))
        })?;
    let mut root = q.xf.get(entity).ok()?.compute_affine();
    if holder != entity {
        root *= q.xf.get(holder).ok()?.compute_affine();
    }
    Some(AttachPose {
        pose: actor.world_pose(),
        skeleton: &actor.skeleton,
        root,
        facing_dir: actor.facing_dir,
        scale: actor.scale,
    })
}

/// One actor of an attachment: its feet in Bevy world space, its yaw, and the pose its
/// reference points are read from (None for a door or a model still loading).
#[derive(Clone, Copy)]
struct AttachActor<'a> {
    world: Vec3,
    yaw: Quat,
    pose: Option<AttachPose<'a>>,
}

fn attach_actor<'a>(entity: Entity, q: &'a AttachQueries<'_, '_>) -> Option<AttachActor<'a>> {
    let xf = q.xf.get(entity).ok()?;
    let pose = attach_pose(entity, q);
    Some(AttachActor {
        world: xf.translation,
        yaw: pose.as_ref().map_or(xf.rotation, AttachPose::yaw),
        pose,
    })
}

// .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Reference
// point: `point(i, A, B)` in Bevy world space. Reference 2 is the static nameplate rule (table
// translation scaled by the model, no pose, no facing); every other index is the posed locator,
// with 49/50 standing for the ring entry nearest the other actor (`other`) and 51 for the one
// nearest the camera eye. An actor without a pose, or an index the table lacks, gives its feet.
fn reference_point(
    actor: &AttachActor<'_>,
    reference: u8,
    other: Option<Vec3>,
    camera_eye: Option<Vec3>,
) -> Vec3 {
    let Some(pose) = &actor.pose else {
        return actor.world;
    };
    let reference = resolve_reference_alias(reference);
    if reference == ffxi_dat::skel::standard_position::ABOVE_HEAD {
        return ffxi_actor::skeleton_instance::nameplate_locator_offset(
            pose.skeleton,
            Vec3::splat(pose.scale),
        )
        .map(|offset| actor.world + crate::ffxi_actor_render::ffxi_to_bevy_basis() * offset)
        .unwrap_or(actor.world);
    }
    let toward_world = if reference == usize::from(CAMERA_NEAREST_REFERENCE) {
        camera_eye.or(other)
    } else {
        other
    };
    let toward = toward_world.map(|w| pose.root.inverse().transform_point3(w));
    ffxi_actor::skeleton_instance::attach_joint_position(
        pose.pose,
        pose.skeleton,
        reference,
        toward,
    )
    .map(|local| pose.root.transform_point3(local))
    .unwrap_or(actor.world)
}

/// The frame an actor-attached generator's elements are placed in:
/// `world = translation + rotation * (local x position_fit)`, `local` being the DAT-frame
/// element position through the FFXI->Bevy basis.
#[derive(Clone, Copy, Debug, PartialEq)]
struct AttachFrame {
    translation: Vec3,
    rotation: Quat,
    position_fit: Vec3,
    model_fit: Vec3,
}

impl AttachFrame {
    fn at(translation: Vec3) -> Self {
        AttachFrame {
            translation,
            rotation: Quat::IDENTITY,
            position_fit: Vec3::ONE,
            model_fit: Vec3::ONE,
        }
    }

    fn point(&self, local_dat: Vec3) -> Vec3 {
        self.translation
            + self.rotation * (local_dat * WORLD_PARTICLE_VEL_BASIS * self.position_fit)
    }
}

// .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Scale — a fit
// nibble of 1-4 sizes by the caster, 5-8 by the target; its low two bits pick the axes, none
// meaning uniform by the larger of width and height. Width is the box x extent times the actor
// scale over 1.7, height the y extent over 1.9, from the skeleton's first bounding box. The
// record's per-generator factor has no located DAT field, so it keeps its unset value of 1,
// under which the fit is the raw scale.
const FIT_CASTER_NIBBLES: std::ops::RangeInclusive<u8> = 1..=4;
const FIT_TARGET_NIBBLES: std::ops::RangeInclusive<u8> = 5..=8;
const FIT_AXES_MASK: u8 = 0b11;
const FIT_HORIZONTAL_BIT: u8 = 0b01;
const FIT_VERTICAL_BIT: u8 = 0b10;
const FIT_WIDTH_DIVISOR: f32 = 1.7;
const FIT_HEIGHT_DIVISOR: f32 = 1.9;

struct FitScales {
    width: f32,
    height: f32,
}

fn actor_fit_scales(actor: &AttachActor<'_>) -> FitScales {
    let unscaled = FitScales {
        width: 1.0,
        height: 1.0,
    };
    let Some(pose) = &actor.pose else {
        return unscaled;
    };
    let Some(b) = pose.skeleton.bounding_boxes.first() else {
        return unscaled;
    };
    FitScales {
        width: (b.x_max - b.x_min).abs() * pose.scale / FIT_WIDTH_DIVISOR,
        height: (b.y_max - b.y_min).abs() * pose.scale / FIT_HEIGHT_DIVISOR,
    }
}

fn fit_vector(nibble: u8, caster: &AttachActor<'_>, target: &AttachActor<'_>) -> Vec3 {
    let actor = if FIT_CASTER_NIBBLES.contains(&nibble) {
        caster
    } else if FIT_TARGET_NIBBLES.contains(&nibble) {
        target
    } else {
        return Vec3::ONE;
    };
    let FitScales { width, height } = actor_fit_scales(actor);
    let axes = nibble & FIT_AXES_MASK;
    if axes == 0 {
        return Vec3::splat(width.max(height));
    }
    let horizontal = if axes & FIT_HORIZONTAL_BIT != 0 {
        width
    } else {
        1.0
    };
    let vertical = if axes & FIT_VERTICAL_BIT != 0 {
        height
    } else {
        1.0
    };
    Vec3::new(horizontal, vertical, horizontal)
}

// The two-point basis: X along `span`, pitch and yaw taken from it and no roll, so Z stays
// horizontal. A zero span has no direction and leaves the frame unturned.
fn two_point_rotation(span: Vec3) -> Quat {
    let Some(x) = span.try_normalize() else {
        return Quat::IDENTITY;
    };
    let z = x.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::Z);
    let y = z.cross(x);
    Quat::from_mat3(&Mat3::from_cols(x, y, z))
}

// The placement the legacy attach type gave the modes whose frame the record leaves open
// (9-12 and 16-31): the caster's source reference, or for a target-side type the target's ring
// entry nearest the caster unless the source reference is itself a selector; the weapon,
// celestial and unattached types sit at the caster's feet.
fn legacy_attach_point(
    def: &ParticleGeneratorDef,
    caster: &AttachActor<'_>,
    target: &AttachActor<'_>,
    camera_eye: Option<Vec3>,
) -> Vec3 {
    use ffxi_dat::particle_gen::AttachType;
    match def.attach_type {
        AttachType::TargetActor
        | AttachType::TargetActorSourceFacing
        | AttachType::TargetToSourceBasis => {
            let selectors = ffxi_actor::skeleton_instance::NEAREST_JOINT_REFERENCES;
            let reference = if selectors.contains(&usize::from(def.attach_eid)) {
                def.attach_eid
            } else {
                *selectors.start() as u8
            };
            reference_point(target, reference, Some(caster.world), camera_eye)
        }
        AttachType::SourceActor
        | AttachType::SourceActorTargetFacing
        | AttachType::SourceToTargetBasis
        | AttachType::ZoneActorA
        | AttachType::ZoneActorB
        | AttachType::ZoneActorC => {
            reference_point(caster, def.attach_eid, Some(target.world), camera_eye)
        }
        AttachType::SourceActorWeapon | AttachType::None | AttachType::Sun | AttachType::Moon => {
            caster.world
        }
    }
}

// .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Attach frame.
// Modes 1/5 translate to the caster's source reference, 2/4 to the target's target reference,
// turned by the caster's (1, 4) or the target's (2, 5) yaw; 3/6 span the two points, the span
// length replacing the X position fit. An untracked target stands in as the caster so a routine
// never loses its effect; the record's identity frame for a missing actor is not reproduced.
fn attach_frame(
    def: &ParticleGeneratorDef,
    owner: Entity,
    target: Option<Entity>,
    q: &AttachQueries<'_, '_>,
) -> Option<AttachFrame> {
    use ffxi_dat::particle_gen::attach_mode as mode;
    let camera_eye = q.camera_eye();
    let caster = attach_actor(owner, q)?;
    let target = target
        .filter(|t| *t != owner)
        .and_then(|t| attach_actor(t, q))
        .unwrap_or(caster);
    let source_point = || reference_point(&caster, def.attach_eid, Some(target.world), camera_eye);
    let target_point = || {
        reference_point(
            &target,
            def.attach_target_reference,
            Some(caster.world),
            camera_eye,
        )
    };
    let (translation, rotation, span) = match def.attach_mode {
        mode::SOURCE => (source_point(), caster.yaw, None),
        mode::SOURCE_WITH_TARGET_YAW => (source_point(), target.yaw, None),
        mode::TARGET => (target_point(), target.yaw, None),
        mode::TARGET_WITH_SOURCE_YAW => (target_point(), caster.yaw, None),
        mode::SOURCE_TO_TARGET | mode::TARGET_TO_SOURCE => {
            let (start, end) = if def.attach_mode == mode::SOURCE_TO_TARGET {
                (source_point(), target_point())
            } else {
                (target_point(), source_point())
            };
            let span = end - start;
            (start, two_point_rotation(span), Some(span.length()))
        }
        _ => (
            legacy_attach_point(def, &caster, &target, camera_eye),
            Quat::IDENTITY,
            None,
        ),
    };
    let (mut position_fit, model_fit) = if def.attach_mode == mode::UNATTACHED {
        (Vec3::ONE, Vec3::ONE)
    } else {
        (
            fit_vector(def.attach_position_fit, &caster, &target),
            fit_vector(def.attach_model_fit, &caster, &target),
        )
    };
    if let Some(distance) = span {
        position_fit.x = distance;
    }
    Some(AttachFrame {
        translation,
        rotation,
        position_fit,
        model_fit,
    })
}

// The per-tick share of a per-update follow rate; a snap stays a snap at any frame time.
fn follow_blend(rate: f32, frames: f32) -> f32 {
    if rate >= 1.0 {
        1.0
    } else {
        1.0 - (1.0 - rate.max(0.0)).powf(frames)
    }
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
/// Tester-only: generator names whose authored alpha is forced to 1.0 at spawn, so an a=0
/// additive flash (g141/g144) can be inspected in isolation. Absent/empty in production.
#[derive(Resource, Default)]
pub struct TestAlphaOverride(pub std::collections::HashSet<[u8; 4]>);

// The non-drawable generator kinds share their envelope resolution (`authored_envelope`), sound
// weight and arm_* dispatch between the routine scheduler and child bindings: a 0x02 stage and a
// sec2 0x44/0x53/0x6A binding that name the same def must behave identically.
fn rescale_track(
    t: &ffxi_dat::particle_gen::KeyFrameTrack,
) -> ffxi_dat::particle_gen::KeyFrameTrack {
    ffxi_dat::particle_gen::KeyFrameTrack {
        points: t
            .points
            .iter()
            .map(|&(time, v)| (time, ffxi_dat::particle_gen::ps2_float_rescale(v)))
            .collect(),
    }
}

// The footprint a 0x22 element draws: its built-in unit diamond (center plus four axis endpoints at
// ±1) at the authored init scale. Nothing is resolved for it — no mesh, image or sprite sheet — so an
// absent linked name is normal here and arms anyway (`kuluu-render/src/distortion_pass.rs`, record
// `.agents/skills/retail-observe/references/2026-10-07-procedural-distortion.md`). A zero scale is
// degenerate, so that def draws nothing rather than something guessed.
fn procedural_half_extent(
    generator: &ffxi_dat::particle_gen::ParticleGeneratorDef,
) -> Option<Vec2> {
    let sx = generator.init_scale[0].abs();
    let sy = generator.init_scale[1].abs();
    if sx == 0.0 || sy == 0.0 {
        return None;
    }
    Some(Vec2::new(
        sx * crate::distortion_pass::FOOTPRINT_LOCAL[3][0].abs(),
        sy * crate::distortion_pass::FOOTPRINT_LOCAL[4][1].abs(),
    ))
}

// Anchor a live haze field at its attach-frame origin (the hit site): built-in footprint, authored
// scale and haze offset, alpha track fading coverage only.
fn push_distortion_field(
    dist: &ffxi_dat::particle_gen::DistortionGeneratorDef,
    origin: Vec3,
    envelope: Option<ffxi_dat::particle_gen::KeyFrameTrack>,
    follow: Option<Entity>,
    distortion: &mut crate::distortion_pass::ActiveDistortion,
) -> bool {
    let Some(half_extent) = procedural_half_extent(&dist.generator_view) else {
        return false;
    };
    distortion.push(crate::distortion_pass::LiveField {
        center: origin,
        half_extent,
        haze_offset: dist.haze_offset,
        started_at: Instant::now(),
        duration_secs: dist.max_life_frames / ROUTINE_FPS,
        envelope,
        follow: follow.map(crate::distortion_pass::FieldFollow::new),
    });
    true
}

/// Which actor carries a directly armed field, mirroring how [`attach_frame`] chooses its reference:
/// a target-reference mode rides the target and falls back to the routine's owner when there is no
/// target, as `attach_frame` does. Retail sets the unattached flag exactly when the attach code is 0,
/// and an unattached element is carried by nothing.
fn field_carry_actor(
    def: &ParticleGeneratorDef,
    owner: Entity,
    target: Option<Entity>,
) -> Option<Entity> {
    use ffxi_dat::particle_gen::attach_mode as mode;
    match def.attach_mode {
        mode::UNATTACHED => None,
        mode::TARGET | mode::TARGET_WITH_SOURCE_YAW | mode::TARGET_TO_SOURCE => {
            Some(target.unwrap_or(owner))
        }
        _ => Some(owner),
    }
}

// Retail sets the unattached flag exactly when the attach code is 0.
fn sound_vertical_weight(sound: &ffxi_dat::particle_gen::SoundGeneratorDef) -> f32 {
    if sound.attach_type == ffxi_dat::particle_gen::AttachType::None {
        crate::audio::UNATTACHED_VERTICAL_WEIGHT
    } else {
        crate::audio::ATTACHED_VERTICAL_WEIGHT
    }
}

fn play_generator_sound(
    se_id: u32,
    near: f32,
    far: f32,
    vertical_weight: f32,
    origin_pos: Vec3,
    sfx_writer: &mut MessageWriter<crate::audio::SfxEvent>,
) {
    // sec2 0x4C AudioRangeSetup: full inside near, linear to silence at far.
    sfx_writer.write(crate::audio::SfxEvent::at_ranged(
        se_id,
        origin_pos,
        near,
        far,
        vertical_weight,
    ));
}

fn arm_rumble_effect(
    envelope: ffxi_dat::particle_gen::KeyFrameTrack,
    near: f32,
    far: f32,
    life_frames: f32,
    origin: Vec3,
    commands: &mut Commands,
) {
    // sec2 0x82 + sec3 0x5F: a rumble generator never draws — it drives gamepad vibration
    // (kuluu-render/src/rumble.rs).
    commands.spawn((
        InGameEntity,
        Transform::from_translation(origin),
        crate::rumble::RumbleSource::new(envelope, near, far, life_frames),
    ));
}

pub fn spawn_particle_generators(
    mut events: MessageReader<SchedulerStageEvent>,
    q_actors: Query<(&Transform, Option<&ActionAssets>)>,
    q_action_target: Query<&crate::scheduler_runtime::ActionTarget>,
    attach: AttachQueries,
    global: Option<Res<GlobalEffectDir>>,
    alpha_override: Option<Res<TestAlphaOverride>>,
    trace: Option<Res<crate::scheduler_runtime::VfxTrace>>,
    mut trace_writer: MessageWriter<crate::scheduler_runtime::ParticleSpawnTrace>,
    mut sfx_writer: MessageWriter<crate::audio::SfxEvent>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<FfxiParticleMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut sim: ResMut<ParticleSimulator>,
    mut distortion: ResMut<crate::distortion_pass::ActiveDistortion>,
    mut commands: Commands,
) {
    let tracing = trace.is_some_and(|t| t.0);
    for ev in events.read() {
        if ev.stage.stage.kind != StageKind::Particle {
            continue;
        }
        let Ok((actor_xf, local_assets)) = q_actors.get(ev.actor) else {
            continue;
        };
        let actor_assets = attach
            .children
            .get(ev.actor)
            .ok()
            .and_then(|c| c.iter().find_map(|child| attach.render.get(child).ok()))
            .map(|a| a.action_assets());
        let local_dir = ev.stage.stage.local_dir;
        let Some(assets) = assets_holding(
            local_assets,
            actor_assets,
            global.as_ref().map(|g| &g.assets),
            |a| a.particle_def(local_dir, &ev.stage.stage.id).is_some(),
        ) else {
            // A SpawnGenerator whose target links a Sep (not a mesh) is a sound cue, not a
            // particle: play its sep at the impact point. g14s in hit1/hi14 is the crit SFX.
            let sound_pair = [
                local_assets,
                actor_assets,
                global.as_ref().map(|g| &g.assets),
            ]
            .into_iter()
            .flatten()
            .find_map(|a| a.sound_defs.get(&ev.stage.stage.id).map(|s| (a, s)));
            let mut played_sound = false;
            if let Some((sound_assets, sound)) = sound_pair {
                if let Some(se_id) = sound_assets.seps.get(&sound.sep_id).map(|sep| sep.se_id) {
                    // sec2 0x4C AudioRangeSetup: full inside near, linear to silence at far.
                    let vertical_weight = sound_vertical_weight(sound);
                    let origin = q_action_target
                        .get(ev.actor)
                        .ok()
                        .and_then(|t| t.0)
                        .unwrap_or(ev.actor);
                    match attach.xf.get(origin) {
                        Ok(xf) => {
                            play_generator_sound(
                                se_id,
                                sound.near,
                                sound.far,
                                vertical_weight,
                                xf.translation,
                                &mut sfx_writer,
                            );
                        }
                        Err(_) => {
                            sfx_writer.write(crate::audio::SfxEvent::new(se_id));
                        }
                    };
                    played_sound = true;
                }
            }
            if !played_sound {
                // A SpawnGenerator whose target links a 0x22 Distortion def is a screen-space
                // haze cue, not a particle: arm the distortion pass for the generator's life.
                // g142 in hi14 (the crit chain) is retail's motion smear.
                let dist_pair = [
                    local_assets,
                    actor_assets,
                    global.as_ref().map(|g| &g.assets),
                ]
                .into_iter()
                .flatten()
                .find_map(|a| a.distortion_defs.get(&ev.stage.stage.id).map(|d| (a, d)));
                if let Some((dist_assets, dist)) = dist_pair {
                    // sec2 0x2D strength/alpha envelope: resolve the track against the owning
                    // tier + global and PS2-rescale its points (authored at half scale).
                    let Ok(envelope) = authored_envelope(
                        dist_assets,
                        global.as_ref().map(|g| &g.assets),
                        dist.envelope_track,
                        ev.stage.stage.id,
                        local_dir,
                    ) else {
                        continue;
                    };
                    let life_secs = dist.max_life_frames / ROUTINE_FPS;
                    let envelope_pts = envelope.as_ref().map(|t| t.points.len()).unwrap_or(0);
                    // The field anchors at its attach-frame origin — the hit site, same
                    // convention as the sound and rumble cues in this chain.
                    let target_ent = q_action_target.get(ev.actor).ok().and_then(|t| t.0);
                    let origin = attach_frame(&dist.generator_view, ev.actor, target_ent, &attach)
                        .map(|f| f.point(Vec3::from_array(dist.generator_view.base_position)))
                        .unwrap_or(actor_xf.translation);
                    let carry = field_carry_actor(&dist.generator_view, ev.actor, target_ent);
                    let armed =
                        push_distortion_field(dist, origin, envelope, carry, &mut distortion);
                    if tracing {
                        info!(
                            "animationtest trace: particle stage {} [{}] — DISTORTION field haze={:.3} life {:.1}s envelope={} pts{}",
                            String::from_utf8_lossy(&ev.stage.stage.id),
                            String::from_utf8_lossy(&local_dir),
                            dist.haze_offset,
                            life_secs,
                            envelope_pts,
                            if armed {
                                ""
                            } else {
                                " — NOT ARMED: authored scale is degenerate"
                            },
                        );
                    }
                } else if tracing {
                    info!(
                        "animationtest trace: particle stage {} [{}] unresolved — no tier holds the def",
                        String::from_utf8_lossy(&ev.stage.stage.id),
                        String::from_utf8_lossy(&local_dir),
                    );
                }
            }
            continue;
        };
        let Some((def_dir, mut def)) = assets
            .particle_def_scoped(local_dir, &ev.stage.stage.id)
            .map(|(dir, def)| (dir, *def))
        else {
            if tracing {
                info!(
                    "animationtest trace: particle stage {} [{}] — scoped lookup missed",
                    String::from_utf8_lossy(&ev.stage.stage.id),
                    String::from_utf8_lossy(&local_dir),
                );
            }
            continue;
        };
        // Tester-only alpha override: force an a=0 additive flash to opaque for inspection.
        if alpha_override
            .as_ref()
            .is_some_and(|o| o.0.contains(&ev.stage.stage.id))
        {
            def.init_color[3] = 1.0;
        }
        // sec2 0x82 + sec3 0x5F: a rumble generator never draws — it drives gamepad
        // vibration (kuluu-render/src/rumble.rs). Skip the mesh path entirely.
        if let (Some(track_id), Some([near, far, _])) = (def.rumble_track, def.rumble_falloff) {
            let Ok(Some(envelope)) = authored_envelope(
                assets,
                global.as_ref().map(|g| &g.assets),
                Some(track_id),
                ev.stage.stage.id,
                def_dir,
            ) else {
                continue;
            };
            let target = q_action_target.get(ev.actor).ok().and_then(|t| t.0);
            let frame = attach_frame(&def, ev.actor, target, &attach)
                .unwrap_or(AttachFrame::at(actor_xf.translation));
            let origin = frame.point(Vec3::from_array(def.base_position));
            arm_rumble_effect(
                envelope.clone(),
                near,
                far,
                def.max_life_frames,
                origin,
                &mut commands,
            );
            if tracing {
                info!(
                    "animationtest trace: particle stage {} [{}] — RUMBLE armed envelope={} pts near={:.1} far={:.1} life {:.1}s",
                    String::from_utf8_lossy(&ev.stage.stage.id),
                    String::from_utf8_lossy(&local_dir),
                    envelope.points.len(),
                    near,
                    far,
                    def.max_life_frames / ROUTINE_FPS,
                );
            }
            continue;
        }
        let Some((template, sprite_frames, tex)) = resolve_mesh(
            assets,
            global.as_deref().map(|g| &g.assets),
            def_dir,
            &def,
            &mut images,
            false,
        ) else {
            if tracing {
                info!(
                    "animationtest trace: particle stage {} [{}] — def found but mesh {} [{}] missing",
                    String::from_utf8_lossy(&ev.stage.stage.id),
                    String::from_utf8_lossy(&local_dir),
                    String::from_utf8_lossy(&def.mesh_id),
                    String::from_utf8_lossy(&def_dir),
                );
            }
            continue;
        };
        let target = q_action_target.get(ev.actor).ok().and_then(|t| t.0);
        let frame = attach_frame(&def, ev.actor, target, &attach)
            .unwrap_or(AttachFrame::at(actor_xf.translation));
        let origin = frame.translation;
        let draw_path = d3m_draw_path(&tex);
        let mat = mats.add(FfxiParticleMaterial::for_def(
            &def,
            tex,
            NO_DAT_ORDER,
            draw_path,
        ));
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

        if tracing {
            let line = format!(
                "route {} spawned particle generator {} mesh {} frame={} delay={} win={} life {} origin=({:.2},{:.2},{:.2})",
                String::from_utf8_lossy(&ev.scheduler),
                String::from_utf8_lossy(&ev.stage.stage.id),
                String::from_utf8_lossy(&def.mesh_id),
                ev.stage.frame,
                ev.stage.stage.delay_frames,
                ev.stage.stage.duration_frames,
                def.max_life_frames,
                origin.x,
                origin.y,
                origin.z
            );
            info!("animationtest trace: {line}");
            trace_writer.write(crate::scheduler_runtime::ParticleSpawnTrace(line));
        } else {
            debug!(
                "spawned particle generator {} mesh {} life {}",
                String::from_utf8_lossy(&ev.stage.stage.id),
                String::from_utf8_lossy(&def.mesh_id),
                def.max_life_frames
            );
        }

        let child_factories = resolve_child_factories(
            &def,
            assets,
            global.as_ref().map(|g| &g.assets),
            def_dir,
            &mut images,
            &mut mats,
        );

        // A zero-duration stage (hit1's g01x) emits exactly the primed first burst — the
        // flash — and stops; its particles live out their own max_life.
        let mut generator = LiveGenerator {
            emit_window_frames: ev.stage.stage.duration_frames as f32,
            origin_routine: Some(RoutineOrigin {
                owner: ev.actor,
                gen_id: ev.stage.stage.id,
                routine: ev.scheduler,
            }),
            child_factories,
            setup_position: Vec3::from_array(def.base_position),
            ..LiveGenerator::new(
                def,
                GeneratorTracks::resolve(&def, assets, None),
                template,
                sprite_frames,
                draw_path,
                mesh,
                entity,
                origin,
            )
        };
        generator.place_in(&frame);
        sim.generators.push(generator);

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
            let Some((template, sprite_frames, texture)) = resolve_mesh(
                assets,
                global.as_deref().map(|g| &g.assets),
                dir,
                &linked_def,
                &mut images,
                false,
            ) else {
                break;
            };
            let mut linked = parent.clone();
            linked.immediate_parent = Some(parent.entity);
            linked.def = linked_def;
            linked.origin = parent.origin;
            linked.setup_position = Vec3::from_array(linked_def.base_position);
            // .agents/skills/retail-observe/references/2026-10-04-immediate-generator-bindings.md Supported rule.
            linked.tracks = GeneratorTracks::resolve(&linked_def, assets, None);
            linked.child_factories = resolve_child_factories(
                &linked_def,
                assets,
                global.as_ref().map(|g| &g.assets),
                dir,
                &mut images,
                &mut mats,
            );
            linked.solid_mesh = is_solid_mesh(&template);
            linked.bound_radius = template_bound_radius(&template, &sprite_frames);
            linked.template = template;
            linked.sprite_frames = sprite_frames;
            linked.stopped = true;
            linked.mesh = meshes.add(empty_mesh());
            linked.draw_path = d3m_draw_path(&texture);
            let material = mats.add(FfxiParticleMaterial::for_def(
                &linked_def,
                texture,
                NO_DAT_ORDER,
                linked.draw_path,
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
    global: Option<Res<GlobalEffectDir>>,
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
            let Some((template, sprite_frames, tex)) = resolve_mesh(
                &fx.assets,
                global.as_deref().map(|g| &g.assets),
                def_dir,
                &def,
                &mut images,
                false,
            ) else {
                continue;
            };
            let draw_path = d3m_draw_path(&tex);
            let mat = mats.add(FfxiParticleMaterial::for_def(
                &def,
                tex,
                NO_DAT_ORDER,
                draw_path,
            ));
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

            let child_factories = resolve_child_factories(
                &def,
                &fx.assets,
                global.as_ref().map(|g| &g.assets),
                def_dir,
                &mut images,
                &mut mats,
            );
            sim.generators.push(LiveGenerator {
                auto_run: true,
                orientation: particle_orientation(&def),
                actor_local: true,
                vel_basis: Vec3::ONE,
                child_factories,
                ..LiveGenerator::new(
                    def,
                    GeneratorTracks::resolve(&def, &fx.assets, None),
                    template,
                    sprite_frames,
                    draw_path,
                    mesh,
                    entity,
                    Vec3::from_array(def.base_position),
                )
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
    let mat = mats.add(FfxiParticleMaterial::for_def(
        &def,
        tex,
        opts.dat_offset,
        draw_path,
    ));
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

    let child_factories = resolve_child_factories(&def, assets, global, NO_LOCAL_DIR, images, mats);
    sim.generators.push(LiveGenerator {
        auto_run: true,
        orientation: particle_orientation(&def),
        camera_relative: opts.camera_relative,
        emit_culled: def.emit_cull.is_some(),
        emit_scale: opts.emit_scale,
        child_factories,
        ..LiveGenerator::new(
            def,
            GeneratorTracks::resolve(&def, assets, global),
            template,
            sprite_frames,
            draw_path,
            mesh,
            entity,
            origin,
        )
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

// .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Per-update
// follow — a generator carrying updater 0x11 rebuilds its attach frame every update and blends
// the stored frame toward it: bit 0 the translation, bit 1 the rotation, at the authored rate.
// The level-up word is translation-only at a snap, so the origin tracks the posed reference
// while the rotation stays the activation yaw. The spawn path builds the frame once, so without
// this a cast aura or hit flash keeps emitting from where the actor stood when the stage fired.
//
// Only scheduled generators track: auto-run zone generators are parented to their
// actor root and ride along, and the camera/celestial origins have their own
// per-frame setters.
pub fn track_attached_origins(
    attach: AttachQueries,
    q_action_target: Query<&crate::scheduler_runtime::ActionTarget>,
    time: Res<Time>,
    mut sim: ResMut<ParticleSimulator>,
) {
    use ffxi_dat::particle_gen::AttachType;
    let frames = time.delta_secs() * ROUTINE_FPS;
    for g in &mut sim.generators {
        let Some(origin_routine) = g.origin_routine else {
            continue;
        };
        let Some(follow) = g
            .def
            .association
            .filter(|a| a.follow_position || a.follow_facing)
        else {
            continue;
        };
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
        let Some(frame) = attach_frame(&g.def, origin_routine.owner, target, &attach) else {
            continue;
        };
        let blend = follow_blend(follow.rate(), frames);
        if follow.follow_position {
            g.origin = g.origin.lerp(frame.translation, blend);
        }
        if follow.follow_facing {
            g.frame_rot = g.frame_rot.slerp(frame.rotation, blend);
        }
    }
}

pub fn tick_particle_simulator(
    time: Res<Time>,
    mut sim: ResMut<ParticleSimulator>,
    mut suppressed: Local<std::collections::BTreeSet<Entity>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    trace: Option<Res<crate::scheduler_runtime::VfxTrace>>,
    lamp_halos: Option<Res<LampHalosOff>>,
    wall_washes: Option<Res<WallWashOff>>,
    mut trace_writer: MessageWriter<crate::scheduler_runtime::ParticleSpawnTrace>,
    mut sfx_writer: MessageWriter<crate::audio::SfxEvent>,
    mut distortion: ResMut<crate::distortion_pass::ActiveDistortion>,
) {
    // Box kill switches, applied as a per-entity marker diff (live both ways): glow billboards
    // hide while the box's lamps row is set; wash volumes hide when their box row is unchecked.
    // Zone-static generators only re-dispatch on a zone load, so despawning would never bring
    // them back; the marker is non-destructive. The diff keeps steady-state command traffic at
    // zero, and an entry whose generator died with a zone change is dropped without a command
    // (the entity — and its marker — is already gone).
    // A real session inserts neither switch, so with nothing suppressed there is no diff to run.
    if lamp_halos.is_some() || wall_washes.is_some() || !suppressed.is_empty() {
        sync_halo_suppression(
            &sim,
            &mut suppressed,
            lamp_halos.is_some_and(|o| o.0),
            wall_washes.is_some_and(|o| o.0),
            &mut commands,
        );
    }
    // Advance the shared flicker wave before any draw factor samples it this frame.
    sim.clock.lamp_flicker_phase += time.delta_secs();
    let frames = time.delta_secs() * ROUTINE_FPS;
    // research/xim Particle.kt update — children read the parent's state before anything ages.
    let orphans = anchor_children(&mut sim);
    advance_simulator(&mut sim, frames);
    remove_dead_generators(&mut sim, &mut commands, orphans);
    instantiate_child_generators(
        &mut sim,
        &mut commands,
        &mut meshes,
        trace.is_some_and(|t| t.0),
        &mut trace_writer,
        &mut sfx_writer,
        &mut distortion,
    );
}

fn sync_halo_suppression(
    sim: &ParticleSimulator,
    suppressed: &mut std::collections::BTreeSet<Entity>,
    halos_hidden: bool,
    washes_hidden: bool,
    commands: &mut Commands,
) {
    let live: std::collections::BTreeSet<Entity> =
        sim.generators.iter().map(|g| g.entity).collect();
    let desired: std::collections::BTreeSet<Entity> = sim
        .generators
        .iter()
        .filter(|g| {
            if is_lamp_halo_def(&g.def) {
                halos_hidden
            } else {
                washes_hidden && is_wall_wash_def(&g.def)
            }
        })
        .map(|g| g.entity)
        .collect();
    for &e in desired.difference(suppressed) {
        commands.entity(e).insert(HaloSuppressed);
    }
    for &e in suppressed.difference(&desired) {
        if live.contains(&e) {
            commands.entity(e).remove::<HaloSuppressed>();
        }
    }
    *suppressed = desired;
}

// .agents/skills/retail-observe/references/2026-10-02-level-up-linked-sparkle.md Native immediate emission.
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

// research/xim Particle.kt update — a child generator's origin is its parent particle's current
// world position; the anchor state also carries what the sec2 0x45..0x49 copies read. A child
// whose parent particle is gone is orphaned (research/xim removes children with their parent).
fn anchor_children(sim: &mut ParticleSimulator) -> std::collections::BTreeSet<usize> {
    let mut anchors = Vec::with_capacity(sim.generators.len());
    let mut orphans = std::collections::BTreeSet::new();
    for (gi, g) in sim.generators.iter().enumerate() {
        if let Some((pgi, pid)) = g.parent {
            match sim
                .generators
                .get(pgi)
                .and_then(|pg| pg.particles.iter().find(|p| p.id == pid))
            {
                Some(p) => anchors.push(Some(AnchorState::from_particle(&sim.generators[pgi], p))),
                None => {
                    anchors.push(None);
                    orphans.insert(gi);
                }
            }
        } else {
            anchors.push(None);
        }
    }
    // research/xim ParticleUpdaters.kt ChildGeneratorUpdater — the child emits from the parent
    // particle's current position every frame, so the origin follows it.
    for (g, a) in sim.generators.iter_mut().zip(anchors) {
        if let Some(a) = a {
            g.origin = a.pos;
            g.anchor = Some(a);
        }
    }
    orphans
}

// research/xim Particle.kt update — children are removed with their parent particle, so the
// removal cascades through every nesting level; surviving references remap across compaction.
fn remove_dead_generators(
    sim: &mut ParticleSimulator,
    commands: &mut Commands,
    mut set: std::collections::BTreeSet<usize>,
) {
    for g in &mut sim.generators {
        set.extend(std::mem::take(&mut g.dead_child_gens));
    }
    if set.is_empty() {
        return;
    }
    loop {
        let mut added = Vec::new();
        for &gi in set.iter() {
            if let Some(g) = sim.generators.get(gi) {
                for p in &g.particles {
                    for &ci in &p.child_gens {
                        if !set.contains(&ci) {
                            added.push(ci);
                        }
                    }
                }
            }
        }
        if added.is_empty() {
            break;
        }
        set.extend(added);
    }
    let old_len = sim.generators.len();
    for (i, g) in sim.generators.iter().enumerate() {
        if set.contains(&i) {
            commands.entity(g.entity).try_despawn();
        }
    }
    let mut remap: Vec<Option<usize>> = vec![None; old_len];
    let mut ni = 0usize;
    for (oi, slot) in remap.iter_mut().enumerate() {
        if !set.contains(&oi) {
            *slot = Some(ni);
            ni += 1;
        }
    }
    let mut idx = 0usize;
    sim.generators.retain(|_| {
        let keep = remap[idx].is_some();
        idx += 1;
        keep
    });
    for g in &mut sim.generators {
        if let Some((pgi, pid)) = g.parent {
            if let Some(n) = remap.get(pgi).copied().flatten() {
                g.parent = Some((n, pid));
            }
        }
        for p in &mut g.particles {
            p.child_gens
                .retain(|ci| remap.get(*ci).copied().flatten().is_some());
            for ci in &mut p.child_gens {
                *ci = remap[*ci].unwrap();
            }
        }
    }
}

// research/xim ParticleInitializers.kt ChildGeneratorSetup + sec4
// EmitChildHandler — each parent particle gets its own child generator instance; a child is an
// ordinary LiveGenerator anchored to the parent, so nothing new in the draw path.
fn instantiate_child_generators(
    sim: &mut ParticleSimulator,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    tracing: bool,
    trace_writer: &mut MessageWriter<crate::scheduler_runtime::ParticleSpawnTrace>,
    sfx_writer: &mut MessageWriter<crate::audio::SfxEvent>,
    distortion: &mut crate::distortion_pass::ActiveDistortion,
) {
    struct SpawnReq {
        // The generator that holds child_factories[factory_idx].
        factory_owner: usize,
        // (generator index, particle id) the child is anchored to; None for an independent
        // sec4 expiry spawn.
        owner: Option<(usize, u64)>,
        pos: Vec3,
        anchor: Option<AnchorState>,
        factory_idx: usize,
        window: f32,
        // The parent's attachment context, which a child inherits
        // (.agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md
        // Per-update follow).
        frame: AttachFrame,
    }
    let inherited_frame = |g: &LiveGenerator, translation: Vec3| AttachFrame {
        translation,
        rotation: g.frame_rot,
        position_fit: g.position_fit,
        model_fit: g.model_fit,
    };
    let mut reqs: Vec<SpawnReq> = Vec::new();
    for (gi, g) in sim.generators.iter().enumerate() {
        for &(pos, fidx) in &g.pending_expiry_spawns {
            if g.child_factories.get(fidx).is_some_and(|f| f.on_expiry) {
                reqs.push(SpawnReq {
                    factory_owner: gi,
                    owner: None,
                    pos,
                    anchor: None,
                    factory_idx: fidx,
                    // research/xim ParticleExpirationHandlers.kt EmitChildHandler — one burst at
                    // expiry, never again (nothing re-emits the child afterwards).
                    window: 0.0,
                    frame: inherited_frame(g, pos),
                });
            }
        }
        for p in &g.particles {
            if p.pending_child_factories.is_empty() {
                continue;
            }
            let anchor = AnchorState::from_particle(g, p);
            // research/xim ParticleInitializers.kt ChildGeneratorSetup — the child's max emit
            // time is the parent particle's life (infinite for a continuous-singleton parent).
            let window = if g.def.continuous {
                f32::INFINITY
            } else {
                p.life_frames
            };
            for &fidx in &p.pending_child_factories {
                if g.child_factories.get(fidx).is_some_and(|f| !f.on_expiry) {
                    reqs.push(SpawnReq {
                        factory_owner: gi,
                        owner: Some((gi, p.id)),
                        pos: anchor.pos,
                        anchor: Some(anchor),
                        factory_idx: fidx,
                        window,
                        frame: inherited_frame(g, anchor.pos),
                    });
                }
            }
        }
    }
    for g in &mut sim.generators {
        g.pending_expiry_spawns.clear();
        for p in &mut g.particles {
            p.pending_child_factories.clear();
        }
    }
    for r in reqs {
        let Some(f) = sim
            .generators
            .get(r.factory_owner)
            .and_then(|g| g.child_factories.get(r.factory_idx))
        else {
            continue;
        };
        // Clone the factory's payload out before pushing: the push shifts every index.
        let children = f.children.clone();
        match &f.payload {
            ChildPayload::Sound {
                se_id,
                near,
                far,
                vertical_weight,
            } => {
                if tracing {
                    let line = format!(
                        "child {} — SOUND se_id={} near={:.1} far={:.1}",
                        String::from_utf8_lossy(&f.name),
                        *se_id,
                        *near,
                        *far
                    );
                    info!("animationtest trace: {line}");
                    trace_writer.write(crate::scheduler_runtime::ParticleSpawnTrace(line));
                }
                play_generator_sound(*se_id, *near, *far, *vertical_weight, r.pos, sfx_writer);
            }
            ChildPayload::Distortion {
                haze_offset,
                life_frames,
                envelope,
                half_extent,
            } => {
                let armed = match half_extent {
                    Some(half) => {
                        distortion.push(crate::distortion_pass::LiveField {
                            center: r.pos,
                            half_extent: *half,
                            haze_offset: *haze_offset,
                            started_at: Instant::now(),
                            duration_secs: *life_frames / ROUTINE_FPS,
                            envelope: envelope.clone(),
                            // A child fires at a point along its parent's path, and no attach mode
                            // governs that placement, so nothing anchors it to an actor.
                            follow: None,
                        });
                        true
                    }
                    None => false,
                };
                if tracing {
                    let line = format!(
                        "child {} — DISTORTION field haze={:.3} life {:.1}s{}",
                        String::from_utf8_lossy(&f.name),
                        *haze_offset,
                        *life_frames / ROUTINE_FPS,
                        if armed {
                            ""
                        } else {
                            " — NOT ARMED: authored scale is degenerate"
                        },
                    );
                    info!("animationtest trace: {line}");
                    trace_writer.write(crate::scheduler_runtime::ParticleSpawnTrace(line));
                }
            }
            ChildPayload::Rumble {
                envelope,
                near,
                far,
                life_frames,
            } => {
                if tracing {
                    let line = format!(
                        "child {} — RUMBLE armed near={:.1} far={:.1} life {:.1}s",
                        String::from_utf8_lossy(&f.name),
                        *near,
                        *far,
                        *life_frames / ROUTINE_FPS
                    );
                    info!("animationtest trace: {line}");
                    trace_writer.write(crate::scheduler_runtime::ParticleSpawnTrace(line));
                }
                arm_rumble_effect(envelope.clone(), *near, *far, *life_frames, r.pos, commands);
            }
            ChildPayload::Draw(d) => {
                let d = ChildDraw::clone(d);
                let mesh = meshes.add(empty_mesh());
                let entity = commands
                    .spawn((
                        InGameEntity,
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(d.mat.clone()),
                        Transform::IDENTITY,
                        Visibility::default(),
                        bevy::camera::visibility::NoFrustumCulling,
                        bevy::light::NotShadowCaster,
                        bevy::light::NotShadowReceiver,
                    ))
                    .id();
                let new_idx = sim.generators.len();
                let mut child = LiveGenerator {
                    emit_window_frames: r.window,
                    parent: r.owner,
                    anchor: r.anchor,
                    child_factories: children,
                    ..LiveGenerator::new(
                        d.def,
                        d.tracks,
                        d.template,
                        d.sprite_frames,
                        d.draw_path,
                        mesh,
                        entity,
                        r.pos,
                    )
                };
                child.place_in(&r.frame);
                sim.generators.push(child);
                if let Some((ogi, pid)) = r.owner {
                    if let Some(p) = sim
                        .generators
                        .get_mut(ogi)
                        .and_then(|g| g.particles.iter_mut().find(|p| p.id == pid))
                    {
                        p.child_gens.push(new_idx);
                    }
                }
            }
        }
    }
}

// research/xim ParticleInitializers.kt ChildGeneratorSetup — localDir.getNullableChildRecursivelyAs,
// then root().getNullableChildRecursivelyAs. DAT directories are flat 4-char names, so the
// recursion degenerates to a scoped lookup (own dir first, then any dir of the same tier).
fn resolve_child_factories(
    def: &ParticleGeneratorDef,
    assets: &ActionAssets,
    global: Option<&ActionAssets>,
    def_dir: [u8; 4],
    images: &mut Assets<Image>,
    mats: &mut Assets<FfxiParticleMaterial>,
) -> Vec<ChildFactory> {
    let mut visited = std::collections::HashSet::new();
    resolve_child_bindings(def, assets, global, def_dir, images, mats, &mut visited)
}

fn resolve_child_bindings(
    def: &ParticleGeneratorDef,
    assets: &ActionAssets,
    global: Option<&ActionAssets>,
    def_dir: [u8; 4],
    images: &mut Assets<Image>,
    mats: &mut Assets<FfxiParticleMaterial>,
    visited: &mut std::collections::HashSet<([u8; 4], [u8; 4])>,
) -> Vec<ChildFactory> {
    // (id, on_expiry): the sec2 per-particle bindings in authored order, then the sec4 expiry
    // binding.
    let mut out = Vec::new();
    for (id_opt, on_expiry) in [
        (def.child_generator, false),
        (def.child_generator_2, false),
        (def.child_generator_3, false),
        (def.emit_child_id, true),
    ] {
        let Some(id) = id_opt else { continue };
        // research/xim ParticleInitializers.kt ChildGeneratorSetup — the 0x44 family stays in
        // the parent's tier.
        let resolved = assets
            .particle_def_scoped(def_dir, &id)
            .map(|(dir, d)| (assets, dir, d));
        let Some((tier, child_dir, child_def)) = resolved else {
            // Not a particle def: the same id may name a sound or distortion generator — those
            // kinds spawn through the shared dispatch too (ai90 in the zone DATs is a 0x22
            // distortion bound by i900's sec2 0x44).
            if let Some(sound) = assets
                .sound_defs
                .get(&id)
                .or_else(|| global.and_then(|g| g.sound_defs.get(&id)))
            {
                let se_id = assets.seps.get(&sound.sep_id).map(|s| s.se_id).or_else(|| {
                    global
                        .and_then(|g| g.seps.get(&sound.sep_id))
                        .map(|s| s.se_id)
                });
                if let Some(se_id) = se_id {
                    out.push(ChildFactory {
                        name: id,
                        on_expiry,
                        payload: ChildPayload::Sound {
                            se_id,
                            near: sound.near,
                            far: sound.far,
                            vertical_weight: sound_vertical_weight(sound),
                        },
                        children: Vec::new(),
                    });
                }
            } else if let Some(dist) = assets
                .distortion_defs
                .get(&id)
                .or_else(|| global.and_then(|g| g.distortion_defs.get(&id)))
            {
                if let Ok(envelope) =
                    authored_envelope(assets, global, dist.envelope_track, id, def_dir)
                {
                    out.push(ChildFactory {
                        name: id,
                        on_expiry,
                        payload: ChildPayload::Distortion {
                            haze_offset: dist.haze_offset,
                            life_frames: dist.max_life_frames,
                            envelope,
                            half_extent: procedural_half_extent(&dist.generator_view),
                        },
                        children: Vec::new(),
                    });
                }
            } else {
                error!(
                    "child generator '{}' of gen '{}' [{}] unresolved — no tier holds a particle, sound or distortion def; parent keeps running",
                    String::from_utf8_lossy(&id),
                    String::from_utf8_lossy(&def.mesh_id),
                    String::from_utf8_lossy(&def_dir),
                );
            }
            continue;
        };
        // A cycle in the binding chain would recurse forever; retail data has none.
        if !visited.insert((child_dir, id)) {
            error!(
                "child generator '{}' of gen '{}' [{}] forms a binding cycle — chain dropped",
                String::from_utf8_lossy(&id),
                String::from_utf8_lossy(&def.mesh_id),
                String::from_utf8_lossy(&def_dir),
            );
            continue;
        }
        // sec2 0x82 + sec3 0x5F: a rumble child never draws — same dispatch as the routine path.
        if let (Some(track_id), Some([near, far, _])) =
            (child_def.rumble_track, child_def.rumble_falloff)
        {
            let Ok(Some(envelope)) = authored_envelope(tier, global, Some(track_id), id, child_dir)
            else {
                continue;
            };
            out.push(ChildFactory {
                name: id,
                on_expiry,
                payload: ChildPayload::Rumble {
                    envelope,
                    near,
                    far,
                    life_frames: child_def.max_life_frames,
                },
                children: Vec::new(),
            });
            continue;
        }
        let Some((template, sprite_frames, tex)) =
            resolve_mesh(tier, global, child_dir, child_def, images, false)
        else {
            error!(
                "child generator '{}' of gen '{}' [{}] has no drawable mesh — binding dropped",
                String::from_utf8_lossy(&id),
                String::from_utf8_lossy(&def.mesh_id),
                String::from_utf8_lossy(&def_dir),
            );
            continue;
        };
        let draw_path = d3m_draw_path(&tex);
        let mat = mats.add(FfxiParticleMaterial::for_def(
            child_def,
            tex,
            NO_DAT_ORDER,
            draw_path,
        ));
        out.push(ChildFactory {
            name: id,
            children: resolve_child_bindings(
                child_def, tier, global, child_dir, images, mats, visited,
            ),
            on_expiry,
            payload: ChildPayload::Draw(Box::new(ChildDraw {
                def: *child_def,
                template,
                sprite_frames,
                mat,
                draw_path,
                tracks: GeneratorTracks::resolve(child_def, tier, None),
            })),
        });
    }
    out
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
    let emitting = !g.stopped && !g.emit_culled && !emit_done(g);
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
        // sec3 0x26 VelocityRotator: rotateAmount × (0.5 × dt) into the velocity rotation; an
        // actor-attached generator authors it in actor space, where z is forward — xim swaps
        // the axes to (-z, y, x) for those (research/xim ParticleUpdaters.kt VelocityRotator,
        // flagged for retail verification).
        if let Some(amount) = g.def.velocity_rotator {
            let hacked = if g.actor_local {
                Vec3::new(-amount[2], amount[1], amount[0])
            } else {
                Vec3::from_array(amount)
            };
            p.vel_rot += hacked * (VELOCITY_ROTATOR_RATE_HALF * frames);
        }
        // sec3 0x2F VelocityRotationUpdater: collapse all velocity into +x and copy the
        // particle rotation into the velocity rotation (research/xim ParticleUpdaters.kt
        // VelocityRotationUpdater).
        if g.def.velocity_rotation_updater {
            let magnitude = p.vel.length() + p.rel_vel.length();
            p.vel = Vec3::new(magnitude, 0.0, 0.0);
            p.rel_vel = Vec3::ZERO;
            p.vel_rot = p.rotation;
        }
        if position_updater {
            let step = if p.vel_rot == Vec3::ZERO {
                p.vel
            } else {
                velocity_rotation(p.vel_rot, p.negate_rotation_y) * p.vel
            };
            p.pos += step * frames;
        }
        // .agents/skills/retail-observe/references/2026-10-01-level-up-keyframe-computation.md Verified computation.
        if let Some([dampen, _]) = g.def.velocity_dampener {
            let progress = (p.age_frames / p.life_frames).clamp(0.0, 1.0);
            let factor = g
                .tracks
                .dampening_factor
                .as_ref()
                .map(|t| t.sample(progress))
                .unwrap_or(dampen);
            let f = factor.powf(frames);
            p.vel *= f;
            p.rel_vel *= f;
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
        // sec3 0x0C ColorTransformModifier: the transform itself drifts — floor(modifier ×
        // dt/30) per channel (research/xim ParticleUpdaters.kt ColorTransformModifier). The
        // applier reads the drifted value; no shipped generator carries both a nonzero base
        // and a modifier, so xim's table order between them is unobservable in the data.
        if let Some(ct) = p.color_transform.as_mut() {
            if let Some(modifier) = g.def.color_transform_modifier {
                for (ch, m) in ct.iter_mut().zip(modifier) {
                    *ch +=
                        (m as f32 * frames / COLOR_TRANSFORM_MODIFIER_RATE_FRAMES).floor() as i32;
                }
            }
            // sec3 0x0B ColorTransformApplier: color += (transform shr 7) × (0.5 × dt), in
            // xim's raw byte space — normalised by the full D3DCOLOR range so it lands on this
            // particle's rgb (research/xim ParticleUpdaters.kt ColorTransformApplier).
            if g.def.color_transform_applier {
                p.rgb.x += (ct[0] >> 7) as f32 * COLOR_TRANSFORM_STEP_HALF * frames
                    / COLOR_TRANSFORM_BYTE_SCALE;
                p.rgb.y += (ct[1] >> 7) as f32 * COLOR_TRANSFORM_STEP_HALF * frames
                    / COLOR_TRANSFORM_BYTE_SCALE;
                p.rgb.z += (ct[2] >> 7) as f32 * COLOR_TRANSFORM_STEP_HALF * frames
                    / COLOR_TRANSFORM_BYTE_SCALE;
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

// research/xim ParticleGenerator.kt isDoneEmitting — a scheduled generator stops once its
// emit window has elapsed AND it has emitted at least one burst; the primed accumulator gives
// that first burst on the generator's first tick whatever that frame's length (a 30 fps tick
// advances two frames and would otherwise skip past a dur=0/1 window before ever emitting).
fn emit_done(g: &LiveGenerator) -> bool {
    !g.auto_run && g.age_frames > g.emit_window_frames && g.elements_emitted > 0
}

fn continuous_active(g: &LiveGenerator) -> bool {
    !g.stopped && !emit_done(g)
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

// .agents/skills/retail-observe/references/2026-10-04-generator-emission-count.md Ordinary batch count:
// an unscaled burst emits ppe + 1, so a ppe=0 def (hit2's g020/g022 spark emitters) fires one
// particle every period. A scaled emit_scale is the weat/ stand-in for the batched-element branch
// (weather_particles::WEATHER_EMIT_SCALE), not the record's density-reduced ordinary path, so it
// keeps trunc(ppe * scale) with a floor of one and no closing iteration.
fn emission_count(g: &LiveGenerator) -> u32 {
    if g.emit_scale == UNSCALED_EMISSION {
        return g.def.particles_per_emission + 1;
    }
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
            dat_frame_rotation(g.frame_rot.inverse(), g.vel_basis)
                * g.actor_rot.inverse()
                * g.cam_view
                * offset
        } else {
            offset
        };
    }
    g.elements_emitted += 1;
    let mut pos = (g.setup_position + pos_local) * g.vel_basis;
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
    // sec2 0x31 RandomVelocitySetup: one [0, v) draw written to every axis — replaces the
    // base-plus-variance velocity transform; the relative-velocity portion is a separate
    // transform and survives (research/xim ParticleInitializers.kt RandomVelocitySetup).
    if let Some(v) = g.def.random_velocity {
        vel = Vec3::splat(v * next_unit(&mut g.emit_rng));
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
    // Both are raw D3DCOLOR bytes/255: sum in that space. The PS2 half-scale doubling happens
    // in the fixed-function stages (ffxi_particle.wgsl), so nothing rescales here.
    let mut rgb = Vec3::from_slice(&g.def.init_color[..3]);
    if let Some(var) = g.def.color_variance {
        rgb += Vec3::new(
            var[0] * next_unit(&mut g.emit_rng),
            var[1] * next_unit(&mut g.emit_rng),
            var[2] * next_unit(&mut g.emit_rng),
        );
    }
    // sec2 0x19 ColorTransformSetup + 0x1A ColorTransformVariance: the per-element transform
    // — base plus one round(posRand(1) × variance) draw per channel (research/xim
    // ParticleInitializers.kt ColorTransformSetup / ColorTransformVariance).
    let color_transform = g.def.color_transform.map(|base| {
        let mut ct: [i32; 4] = base.map(i32::from);
        if let Some(var) = g.def.color_transform_variance {
            for (ch, v) in ct.iter_mut().zip(var) {
                *ch += (next_unit(&mut g.emit_rng) * v as f32).round() as i32;
            }
        }
        ct
    });
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
    // sec2 0x45..0x49 parent-copy blocks (research/xim ParticleInitializers.kt Parent*Config):
    // a child particle copies its anchor's state at init. World space, applied after every
    // other initializer so the copy wins.
    let mut vel_world = vel * g.vel_basis;
    if let Some(a) = g.anchor {
        if g.def.parent_position_copy {
            pos = frame_local(g, a.pos);
        }
        if let Some(mult) = g.def.parent_velocity {
            vel_world += a.vel * mult;
        }
        if g.def.parent_rotate {
            rotation = a.rotation;
        }
        if g.def.parent_color {
            rgb = a.rgb;
        }
        if g.def.parent_scale {
            scale = a.scale;
        }
    }
    let id = g.next_particle_id;
    g.next_particle_id += 1;
    // research/xim ParticleInitializers.kt ChildGeneratorSetup — each
    // particle gets its own child generator instance at init; the tick system spawns it.
    let pending_child_factories: Vec<usize> = (0..g.child_factories.len())
        .filter(|i| !g.child_factories[*i].on_expiry)
        .collect();
    g.particles.push(Particle {
        pos,
        spawn_pos: pos,
        spawn_origin: g.origin,
        vel: vel_world,
        age_frames: 0.0,
        life_frames: life_frames.max(1.0),
        rgb,
        color_transform,
        scale,
        scale_seed: scale,
        scale_vel,
        rotation,
        spin,
        negate_rotation_y,
        rel_vel: rel_vel * g.vel_basis,
        vel_rot: Vec3::ZERO,
        osc,
        id,
        child_gens: Vec::new(),
        pending_child_factories,
    });
}

// CYyGenerator.cpp CYyGenerator::ElemDie case 5 — a relife generator resets an expiring element's
// life and keeps it (its rotation, position and UV state carry on); any other generator's
// expired particles are swept.
// research/xim Particle.kt update — children live on the parent particle and die with it, so a
// reaped particle takes its child generators with it; sec4 0x01 (research/xim
// ParticleExpirationHandlers.kt EmitChildHandler) queues one burst at the death position.
fn reap_expired(g: &mut LiveGenerator) {
    if g.def.relife_on_expiry {
        for p in &mut g.particles {
            if p.age_frames >= p.life_frames {
                p.age_frames = p.age_frames.rem_euclid(p.life_frames);
            }
        }
        return;
    }
    let expired = |p: &Particle| p.age_frames >= p.life_frames;
    let mut dead_child_gens = Vec::new();
    let mut expiry_spawns = Vec::new();
    for p in g.particles.iter().filter(|p| expired(p)) {
        dead_child_gens.extend_from_slice(&p.child_gens);
        for (fidx, f) in g.child_factories.iter().enumerate() {
            if f.on_expiry {
                expiry_spawns.push((particle_world(g, p), fidx));
            }
        }
    }
    g.dead_child_gens.extend(dead_child_gens);
    g.pending_expiry_spawns.extend(expiry_spawns);
    g.particles.retain(|p| !expired(p));
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
    mut q_vis: Query<(&mut Visibility, Option<&HaloSuppressed>), With<Mesh3d>>,
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

    let parents: std::collections::HashSet<_> = sim.generators.iter().map(|g| g.entity).collect();
    let mut reap = std::collections::BTreeSet::new();
    for (i, g) in sim.generators.iter_mut().enumerate() {
        // The mesh entity despawns with its actor (auto-run generators are
        // children of the actor root); reap the simulator entry when it's gone.
        let Ok(entity_xf) = q_mesh_xf.get(g.entity) else {
            reap.insert(i);
            continue;
        };
        // The 0x1F camera-oriented ring resolves against the camera and the actor's world
        // rotation; sync runs after tick, so both reach emit() one frame behind like
        // `emit_culled` (research/xim ParticleGeneratorParser.kt SphericalPositionVarianceFull).
        g.cam_view = cam_rot;
        g.entity_world = *entity_xf;
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
        if let Ok((mut v, suppressed)) = q_vis.get_mut(g.entity) {
            // Suppressed halos stay hidden even while in frustum; the frame the marker comes off,
            // culling resumes normal per-frame control.
            let visible = drawable && suppressed.is_none();
            v.set_if_neq(if visible {
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
            reap.insert(i);
        }
    }

    remove_dead_generators(&mut sim, &mut commands, reap);

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
    // day-of-week and moon-phase modulations, in raw byte/255 space.
    factor_rgb: Vec3,
    factor_alpha: f32,
    world: Vec3,
}

fn particle_progress(p: &Particle) -> f32 {
    (p.age_frames / p.life_frames).clamp(0.0, 1.0)
}

// Where rebuild_mesh draws the particle, in Bevy world space: child generators and expiry
// bursts anchor here so they sit on the drawn parent, whatever frame the parent integrates in.
fn particle_world(g: &LiveGenerator, p: &Particle) -> Vec3 {
    g.entity_world
        .transform_point(track_position(g, p, particle_progress(p)))
}

fn particle_draw(g: &LiveGenerator, p: &Particle, clock: &CelestialClock) -> ParticleDraw {
    let progress = particle_progress(p);
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
        .tracks
        .scale_x
        .as_ref()
        .map(|t| t.sample_from(progress, Some(p.scale_seed.x)))
        .unwrap_or(p.scale.x);
    let sy = g
        .tracks
        .scale_y
        .as_ref()
        .map(|t| t.sample_from(progress, Some(p.scale_seed.y)))
        .unwrap_or(p.scale.y);
    // research/XIClient CMoElem.cpp VirtOt1: the element's draw colour is field_F8 (the 0x16
    // ColorSetup RGBA, CYyGenerator.cpp ElemGenerate) with alpha scaled by field_138 * field_134.
    // field_138 is set to 1.0 in ElemIdle and field_134 is initialised to 1.0 (CMoElem ctor)
    // and never written, so retail applies no life-based fade: the authored alpha holds for the
    // whole element life; fades come from explicit keyframe tracks, sampled above.
    // Alpha is a raw D3DCOLOR byte/255: the 0x16 base and the alpha track's points share that
    // space (the seed must stay in it for the opening-segment override), and the PS2 half-scale
    // doubling happens in the fixed-function stages, not here.
    let alpha = g
        .tracks
        .alpha
        .as_ref()
        .map(|t| t.sample_from(progress, Some(g.def.init_color[3])))
        .unwrap_or(g.def.init_color[3]);
    // research/xim ParticleGeneratorParser.kt sec3Handler ClockValueUpdater — 0x3C/0x3D/0x3E
    // assign the particle's colour channel from a time-of-day curve, 0x3F multiplies alpha.
    // This is the sun's authored dawn/noon/dusk ramp: the disc is not tinted by a formula.
    let mut rgb = p.rgb;
    let mut alpha = alpha;
    let mut tod_alpha_gate = 1.0f32;
    for (channel, track) in g.tracks.tod_color.iter().enumerate() {
        let Some(track) = track.as_ref().filter(|_| g.def.tod_color_driven[channel]) else {
            continue;
        };
        let v = track.sample(clock.day_fraction);
        match channel {
            TOD_ALPHA_CHANNEL => {
                alpha *= v;
                tod_alpha_gate *= v;
            }
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

    // Enhanced build only: lift every particle's final alpha by 20% (clamped) — a deliberate
    // non-retail brightness tuning for the hit flashes retail authors at ~50%.
    #[cfg(feature = "enhanced-particle-alpha-20")]
    let alpha = (alpha * ENHANCED_ALPHA_GAIN).min(1.0);

    // sec3 0x0F..0x11 ProgressValueUpdater (research/xim ParticleUpdaters.kt — p.position.x = v):
    // a bound track replaces the channel each frame; key 0 is seeded from the particle's
    // spawn-time value, so the curve starts where the element was emitted. Keys are authored in
    // the DAT frame, so the track runs on the DAT-frame seed and its value goes back through
    // vel_basis like every other position source.
    let world = track_position(g, p, progress);

    let lamp_halo = is_lamp_halo_def(&g.def);
    let factor_alpha = if lamp_halo {
        // PATH_LAMP_ALPHAMAP multiplies this against the texel's own alpha in the shader; the
        // day gate (tkaa) is what turns lamps on at night and off by day. Retail's visible
        // wavering is runtime behavior (no flicker keyframes ship), so retail mode rides the
        // same hand-tuned lamp_flicker model as the Enhanced lights — a timer going up/down.
        (clock.lamp_halos_lift
            * tod_alpha_gate
            * crate::zone_point_lights::lamp_flicker(
                clock.lamp_flicker_phase,
                lamp_flicker_seed(&g.def),
            ))
        .clamp(0.0, 1.0)
    } else if is_wall_wash_def(&g.def) {
        // Wash volumes keep the authored D3m alpha path; the slider multiplies it (1.0 = as
        // authored) so the test box can dial the wash down without touching retail defaults.
        tfactor_alpha(&g.def, g.draw_path, alpha) * clock.wash_alpha_lift
    } else {
        tfactor_alpha(&g.def, g.draw_path, alpha)
    };

    ParticleDraw {
        flipbook_frame,
        scale: Vec2::new(sx, sy),
        // The shader's lamp branch adds neutral light, so a halo's factor colour is white.
        factor_rgb: if lamp_halo { Vec3::ONE } else { rgb },
        factor_alpha,
        world,
    }
}

#[cfg(feature = "enhanced-particle-alpha-20")]
const ENHANCED_ALPHA_GAIN: f32 = 1.2;

// Retail pins assert the raw authored alpha; under the enhancement the same build lifts it,
// so tests compare against whichever value this feature set produces.
#[cfg(test)]
fn expected_factor_alpha(raw: f32) -> f32 {
    #[cfg(feature = "enhanced-particle-alpha-20")]
    {
        (raw * ENHANCED_ALPHA_GAIN).min(1.0)
    }
    #[cfg(not(feature = "enhanced-particle-alpha-20"))]
    {
        raw
    }
}

fn track_position(g: &LiveGenerator, p: &Particle, progress: f32) -> Vec3 {
    let mut local = p.pos;
    for (axis, track) in [
        &g.tracks.position_x,
        &g.tracks.position_y,
        &g.tracks.position_z,
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(t) = track {
            let basis = g.vel_basis[axis];
            local[axis] = basis * t.sample_from(progress, Some(p.spawn_pos[axis] * basis));
        }
    }
    particle_origin(g, p) + g.frame_rot * (local * g.position_fit)
}

// A Bevy-space rotation re-expressed in the DAT frame a `vel_basis` sign flip maps from.
fn dat_frame_rotation(rotation: Quat, vel_basis: Vec3) -> Quat {
    let flip = Mat3::from_diagonal(vel_basis);
    Quat::from_mat3(&(flip * Mat3::from_quat(rotation) * flip))
}

// The inverse of `track_position`'s placement for a world point; an axis whose fit collapsed to
// zero has no inverse and is left unscaled.
fn frame_local(g: &LiveGenerator, world: Vec3) -> Vec3 {
    let local = g.frame_rot.inverse() * (world - g.origin);
    let unfit = |value: f32, fit: f32| if fit == 0.0 { value } else { value / fit };
    Vec3::new(
        unfit(local.x, g.position_fit.x),
        unfit(local.y, g.position_fit.y),
        unfit(local.z, g.position_fit.z),
    )
}

fn particle_origin(g: &LiveGenerator, p: &Particle) -> Vec3 {
    if g.def.camera_attached_base {
        p.spawn_origin
    } else {
        g.origin
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
    rotation: [i32; 3],
}

fn quantized(v: f32, quantum: f32) -> i32 {
    (v / quantum).round() as i32
}

// sec3 0x2E DrawDistanceUpdater's per-frame alpha multiplier from camera-to-particle distance;
// 1.0 for a generator that carries no updater (research/xim ParticleUpdaters.kt
// DrawDistanceUpdater — the multiplier lands on the element's colour alpha).
fn draw_distance_alpha(g: &LiveGenerator, world_pos: Vec3, cam_pos: Vec3) -> f32 {
    match (g.def.draw_distance_near, g.def.draw_distance_far) {
        (Some(near), Some(far)) => {
            crate::rumble::distance_falloff(cam_pos.distance(world_pos), near, far)
        }
        _ => 1.0,
    }
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
                // sec3 0x2E: the key quantizes the same falloff-multiplied alphas rebuild_mesh
                // draws, so a camera move that fades an element by one colour step rebuilds.
                let m = draw_distance_alpha(g, g.entity_world.transform_point(draw.world), cam.pos);
                ParticleKey {
                    world: draw.world.to_array().map(spatial),
                    flipbook_frame: draw.flipbook_frame,
                    scale: [spatial(draw.scale.x), spatial(draw.scale.y)],
                    factor_rgb: draw.factor_rgb.to_array().map(color),
                    factor_alpha: color(draw.factor_alpha * m),
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
    let model_fit = g.model_fit.abs().max_element();
    for p in &g.particles {
        let draw = particle_draw(g, p, clock);
        let r =
            g.bound_radius * draw.scale.x.abs().max(draw.scale.y.abs()).max(sz.abs()) * model_fit;
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

// research/xim Particle.kt applyMovementOrientation — the Movement billboard's world basis from its
// velocity (identity while the element stands still). The rotateY post-multiply lands leftmost in
// column convention because GLDrawer uploads xim's row-major matrices untransposed. `vel_basis` is an
// involution, so a Bevy-space velocity folds into the DAT frame the template lives in.
fn movement_orientation(movement: Vec3) -> Quat {
    const AXIS_ALIGNED_Y: f32 = 0.999;
    if movement.length_squared() == 0.0 {
        return Quat::IDENTITY;
    }
    let m = movement.normalize();
    if m.y.abs() >= AXIS_ALIGNED_Y {
        return Quat::from_rotation_z(m.y.signum() * std::f32::consts::FRAC_PI_2);
    }
    let left = Vec3::Y.cross(m).normalize();
    let up = m.cross(left).normalize();
    let angle = -up.dot(Vec3::Y).clamp(-1.0, 1.0).acos() * m.y.signum();
    Quat::from_rotation_y(-m.z.atan2(m.x)) * Quat::from_axis_angle(left, angle)
}

fn rebuild_mesh(g: &LiveGenerator, cam: CameraView, clock: &CelestialClock, mesh: &mut Mesh) {
    let verts_per = g.template.positions.len();
    let n = g.particles.len();
    let mut positions = Vec::with_capacity(n * verts_per);
    let mut uvs = Vec::with_capacity(n * verts_per);
    let mut colors = Vec::with_capacity(n * verts_per);
    let mut factors = Vec::with_capacity(n * verts_per);
    let mut indices = Vec::with_capacity(n * g.template.indices.len());
    let axial = is_axial_camera_billboard(g);

    for p in &g.particles {
        let mut draw = particle_draw(g, p, clock);
        // sec3 0x2E DrawDistanceUpdater: fade the element's alpha by camera distance and cull
        // it at zero (research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp
        // CYyGenerator::ElemIdle case 0x2E; research/xim ParticleUpdaters.kt
        // DrawDistanceUpdater — drawDistanceCulled skips the element in the draw list).
        let m = draw_distance_alpha(g, g.entity_world.transform_point(draw.world), cam.pos);
        if m == 0.0 {
            continue;
        }
        draw.factor_alpha *= m;
        let tpl = flipbook_template(g, draw.flipbook_frame);

        // research/xim Particle.kt applyMovementOrientation — a Movement billboard keeps its world
        // orientation (identity while the element stands still) and carries the full per-particle
        // Euler; hi14's g140 authors a ±π x-variance, so its burst fans out into retail's crit flash.
        let movement_bb = matches!(
            g.def.billboard,
            ParticleBillboard::Movement | ParticleBillboard::MovementHorizontal
        );
        let rot = if axial {
            axial_camera_rotation(draw.world, cam.pos, g.vel_basis)
        } else if g.orientation.is_some() {
            particle_rotation(p)
        } else if movement_bb {
            let mut vel_dat = (g.frame_rot * p.vel) * g.vel_basis;
            if g.def.billboard == ParticleBillboard::MovementHorizontal {
                vel_dat.y = 0.0;
            }
            movement_orientation(vel_dat) * particle_rotation(p)
        } else {
            // A screen billboard re-faces the camera every frame and keeps the element's full Euler in
            // the view basis (research/xim GLDrawer.kt drawXimParticle XYZ branch). hit1's g010 authors
            // a ±π z-variance, so its burst fans out into retail's starburst; without it every particle
            // of a burst lies along the same line.
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
        let world_basis = (g.orientation.is_some() || axial || movement_bb) && !g.actor_local;
        // .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md
        // Element placement and billboard: only a non-billboard element takes the frame's
        // rotation; a screen billboard reads the frame through its world point alone.
        let turned_by_frame = g.orientation.is_some() && !g.actor_local;
        // A screen billboard's template is DAT-frame geometry too (Y down: the campfire flame
        // `hi12` rises toward negative y). An actor-local generator inherits the FFXI->Bevy basis
        // from its parent transform; a world-space one folds it into the template before the
        // view rotation, or the flame hangs below its wick (the same flip dat_mzb.rs to_bevy
        // applies to zone origins).
        let screen_basis = g.orientation.is_none() && !axial && !movement_bb && !g.actor_local;
        let base = positions.len() as u32;
        for ((tp, uv), vertex) in tpl.positions.iter().zip(&tpl.uvs).zip(&tpl.colors) {
            let local =
                Vec3::new(tp.x * draw.scale.x, tp.y * draw.scale.y, tp.z * sz) * g.model_fit;
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
            let oriented = if turned_by_frame {
                g.frame_rot * oriented
            } else {
                oriented
            };
            positions.push((draw.world + oriented).to_array());
            uvs.push([uv[0] + g.tex_translate.x, uv[1] + g.tex_translate.y]);
            // The template colour is stage 0's D argument verbatim (D3m/sheet: /128-normalised,
            // MMB: raw byte/255); the per-particle F rides its own attribute. ffxi_particle.wgsl
            // runs the selected table against both, so nothing is folded in here.
            colors.push(vertex.to_array());
            factors.push(draw.factor_rgb.extend(draw.factor_alpha).to_array());
        }
        indices.extend(tpl.indices.iter().map(|&idx| base + idx));
    }

    if positions.is_empty() {
        push_hidden_primitive(
            &mut positions,
            &mut uvs,
            &mut colors,
            &mut factors,
            &mut indices,
        );
    }

    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, factors);
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
    factors: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
) {
    let base = positions.len() as u32;
    for _ in 0..HIDDEN_PRIMITIVE_VERTS {
        positions.push([0.0, 0.0, 0.0]);
        uvs.push([0.0, 0.0]);
        colors.push([0.0, 0.0, 0.0, 0.0]);
        factors.push([0.0, 0.0, 0.0, 0.0]);
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
    // The caller (zone_static_defs dispatch) retries against the global tier itself.
    if let Some((template, frames, tex)) =
        resolve_mesh(assets, None, NO_LOCAL_DIR, def, images, undither)
    {
        // A textureless D3m submesh (the Bastok tunnel `ligh` lamp fixtures) takes the
        // ZeroOneTSS one-stage table; on the textured chain it would sample a null texture.
        let path = d3m_draw_path(&tex);
        return Some((template, frames, tex, path));
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

struct UnresolvedTrack;

// An envelope the def names but no tier holds is a resolution failure, not an authoring choice:
// the effect is dropped with an error instead of running on a curve no DAT supplies. A def that
// names no track keeps its constant strength (Ok(None)).
fn authored_envelope(
    assets: &ActionAssets,
    global: Option<&ActionAssets>,
    track: Option<[u8; 4]>,
    generator: [u8; 4],
    dir: [u8; 4],
) -> Result<Option<KeyFrameTrack>, UnresolvedTrack> {
    let Some(id) = track else {
        return Ok(None);
    };
    match keyframe(assets, global, Some(id)) {
        Some(t) => Ok(Some(rescale_track(&t))),
        None => {
            error!(
                "generator '{}' [{}] names envelope track '{}' but no tier holds it — effect dropped",
                String::from_utf8_lossy(&generator),
                String::from_utf8_lossy(&dir),
                String::from_utf8_lossy(&id),
            );
            Err(UnresolvedTrack)
        }
    }
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
    global: Option<&ActionAssets>,
    local_dir: [u8; 4],
    def: &ParticleGeneratorDef,
    images: &mut Assets<Image>,
    undither: bool,
) -> Option<(SpriteTemplate, Vec<SpriteTemplate>, Option<Handle<Image>>)> {
    // research/xim ParticleLinkedDataProviders.kt resolveStaticMeshLink — the effect directory
    // first, wider scopes after. Retail's tree is one merged virtual filesystem, so a mesh or
    // texture absent from this tier resolves against the global effect dir: i900 in 120.DAT
    // binds asi1, which exists only under syst/effe of 0.DAT.
    let tiers = [Some(assets), global];
    // research/xim ParticleGeneratorSettings.kt LinkedDataType: WeightedMesh(0x1D) resolves like
    // StaticMesh; the draw path is identical.
    match def.mesh_kind {
        ParticleMeshKind::StaticMesh | ParticleMeshKind::WeightedMesh => {
            let d3m = tiers
                .into_iter()
                .flatten()
                .find_map(|a| a.d3m(local_dir, &def.mesh_id))?;
            let template = sprite_template(d3m)?;
            let (namespace, local) = d3m.texture_name_tokens();
            // research/xim DatResource.kt getTextureResourceByNameAs — qualified (namespace, local) match, then
            // local-only. The truncated DatId stays as a last tier: a few meshes name a
            // texture whose local token outruns the Img chunk id (`kumori` vs `kumo`) and
            // resolve only that way.
            let tex = tiers
                .into_iter()
                .flatten()
                .find_map(|a| {
                    let by_name = (!local.is_empty()).then(|| {
                        a.images_by_qualified_name
                            .get(&(namespace.clone(), local.clone()))
                            .or_else(|| a.images_by_name.get(&local))
                    });
                    by_name
                        .flatten()
                        .or_else(|| a.images.get(&d3m.texture_dat_id()))
                })
                .map(|t| images.add(to_image(t, undither)));
            Some((template, Vec::new(), tex))
        }
        ParticleMeshKind::SpriteSheet => {
            let ss = tiers
                .into_iter()
                .flatten()
                .find_map(|a| a.sprite_sheet(local_dir, &def.mesh_id))?;
            let frames = sprite_sheet_templates(ss);
            let first = frames.first().cloned()?;
            // research/xim DatResource.kt getTextureResourceByNameAs — try the qualified (namespace, local) pair
            // first, then fall back to a local-name-only match.
            let tex = tiers
                .into_iter()
                .flatten()
                .find_map(|a| {
                    a.images_by_qualified_name
                        .get(&(ss.category.clone(), ss.id.clone()))
                        .or_else(|| a.images_by_name.get(&ss.id))
                })
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
    let (mut positions, mut uvs, mut colors, mut factors, mut indices) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    push_hidden_primitive(
        &mut positions,
        &mut uvs,
        &mut colors,
        &mut factors,
        &mut indices,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, factors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    // Tests use Duration directly; non-test code does not import it at the file root.
    use std::time::Duration;

    mod cleanup_capture;
    #[cfg(not(target_arch = "wasm32"))]
    mod immediate_bindings;
    mod wall_wash_capture;
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
            attach_mode: ffxi_dat::particle_gen::attach_mode::SOURCE,
            attach_eid: 0,
            attach_target_reference: 0,
            attach_position_fit: 0,
            attach_model_fit: 0,
            init_scale: [0.1, 0.1, 1.0],
            single_scale_variance: None,
            scale_variance: None,
            init_color: [0.2, 0.2, 0.6, 0.5],
            color_variance: None,
            color_transform: None,
            color_transform_variance: None,
            color_transform_applier: false,
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
            rumble_track: None,
            rumble_falloff: None,
            draw_distance_near: None,
            draw_distance_far: None,
            position_x_track: None,
            position_y_track: None,
            position_z_track: None,
            weighted_mesh_weight_tracks: [None; ffxi_dat::particle_gen::WEIGHTED_MESH_WEIGHTS],
            tod_volume_track: None,
            haze_offset: None,
            parent_rotate: false,
            parent_color: false,
            parent_scale: false,
            velocity_dampener_track: None,
            dampening_factor_applier: false,
            velocity_dampener: None,
            velocity_rotator: None,
            velocity_rotation_updater: false,
            random_velocity: None,
            fixed_point_position_variance: None,
            fixed_point_position_variance_2: None,
            child_generator_2: None,
            child_generator_3: None,
            emit_child_id: None,
            child_emit_basic: false,
            child_emit_full: false,
            child_emit_billboard: false,
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
            tracks: GeneratorTracks::default(),
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
            frame_rot: Quat::IDENTITY,
            position_fit: Vec3::ONE,
            model_fit: Vec3::ONE,
            setup_position: Vec3::ZERO,
            entity_world: GlobalTransform::IDENTITY,
            parent: None,
            anchor: None,
            child_factories: Vec::new(),
            next_particle_id: 0,
            dead_child_gens: Vec::new(),
            pending_expiry_spawns: Vec::new(),
            built_key: MeshKey::Empty,
            bound_radius: 0.0,
        }
    }

    // Drive the emission math directly (no Bevy world), one tick's worth of frames per call.
    fn advance(g: &mut LiveGenerator, frames: f32) {
        advance_generator(g, frames);
    }

    // The scheduled spawn path's wiring for a real def (spawn_particle_generators): the
    // world-space velocity basis plus the keyframe tracks resolved against the DAT, so a test
    // drives a shipped generator exactly as the live client does.
    fn live_scheduled(
        def: ParticleGeneratorDef,
        window: f32,
        assets: &crate::scheduler_runtime::ActionAssets,
    ) -> LiveGenerator {
        let mut g = live(def, window);
        g.vel_basis = WORLD_PARTICLE_VEL_BASIS;
        g.tracks = GeneratorTracks::resolve(&def, assets, None);
        g
    }

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

    // 0x1E ParticleDampen: emission stops and the already-live particles are force-expired
    // at once (research/xim EffectRoutineInstance.kt handleParticleEffectDampen), unlike
    // StopParticle which lets them play out.
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

    // sec2 0x21..0x23 position-track keys are authored in the DAT frame (-Y up). On a
    // world-space generator the track runs on the DAT-frame seed and lands through vel_basis,
    // so a negative DAT y draws above the origin.
    #[test]
    fn world_space_position_track_lands_through_the_basis() {
        const SPAWN_UP: f32 = 1.0;
        const TRACK_END_DAT_Y: f32 = -2.0;
        let d = def(ROUTINE_FPS, ROUTINE_FPS, 0);
        let mut g = live(d, 0.0);
        g.vel_basis = WORLD_PARTICLE_VEL_BASIS;
        g.tracks.position_y = Some(KeyFrameTrack {
            points: vec![(0.0, 0.0), (1.0, TRACK_END_DAT_Y)],
        });
        emit(&mut g, d.max_life_frames);
        let p = &mut g.particles[0];
        p.pos = Vec3::Y * SPAWN_UP;
        p.spawn_pos = p.pos;
        let clock = CelestialClock::default();

        p.age_frames = 0.0;
        assert_eq!(
            particle_draw(&g, &g.particles[0], &clock).world.y,
            SPAWN_UP,
            "the curve starts where the element was emitted"
        );
        g.particles[0].age_frames = g.particles[0].life_frames;
        assert_eq!(
            particle_draw(&g, &g.particles[0], &clock).world.y,
            TRACK_END_DAT_Y * WORLD_PARTICLE_VEL_BASIS.y
        );
        assert!(particle_draw(&g, &g.particles[0], &clock).world.y > 0.0);
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
        assert_eq!(
            g.particles.len(),
            60,
            "back in band: emits again (two per period)"
        );
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
        world.insert_resource(crate::distortion_pass::ActiveDistortion::default());

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
        world.insert_resource(crate::distortion_pass::ActiveDistortion::default());
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

    // The per-particle factor after ToD/day-of-week/moon modulation — what the shader's stage 1
    // multiplies against the template colour.
    fn drawn_factor(g: &LiveGenerator, clock: &CelestialClock) -> Vec4 {
        let draw = particle_draw(g, &g.particles[0], clock);
        draw.factor_rgb.extend(draw.factor_alpha)
    }

    // Outside the tester, mesh particles must draw at their authored alpha: no slider
    // multiplier rides the default state.
    #[test]
    fn default_mesh_alpha_preserves_authored_factor() {
        use ffxi_dat::particle_gen::ParticleMeshKind;

        for kind in [ParticleMeshKind::StaticMesh, ParticleMeshKind::WeightedMesh] {
            let mut generator = live(def(60.0, 1.0, 1), 60.0);
            generator.def.mesh_kind = kind;
            advance(&mut generator, 2.0);
            let factor = drawn_factor(&generator, &CelestialClock::default());
            assert_eq!(
                factor.w,
                expected_factor_alpha(generator.def.init_color[TOD_ALPHA_CHANNEL]),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn wall_wash_controls_preserve_unrelated_effects() {
        use ffxi_dat::particle_gen::ParticleBlend;

        const ONE_FRAME: f32 = 1.0;
        let mut world = child_test_world(ParticleSimulator::default());
        let mut cases = Vec::new();
        for kind in [ParticleMeshKind::StaticMesh, ParticleMeshKind::WeightedMesh] {
            for mesh in [WALL_WASH_MESH_ID, *b"gr  "] {
                for blend in [ParticleBlend::Additive, ParticleBlend::Blend] {
                    let mut definition = def(f32::INFINITY, ROUTINE_FPS, 0);
                    definition.mesh_kind = kind;
                    definition.mesh_id = mesh;
                    definition.blend = blend;
                    let mut g = live(definition, f32::INFINITY);
                    prime(&mut g);
                    advance_generator(&mut g, ONE_FRAME);
                    g.stopped = true;
                    g.entity = world
                        .spawn((
                            Mesh3d(Handle::default()),
                            GlobalTransform::IDENTITY,
                            Visibility::Inherited,
                        ))
                        .id();
                    cases.push((
                        g.entity,
                        mesh == WALL_WASH_MESH_ID && blend == ParticleBlend::Additive,
                    ));
                    world.resource_mut::<ParticleSimulator>().generators.push(g);
                }
            }
        }
        let mut schedule = Schedule::default();
        schedule.add_systems((tick_particle_simulator, sync_particle_meshes).chain());
        world.insert_resource(WallWashOff(true));
        schedule.run(&mut world);
        for &(entity, wash) in &cases {
            assert_eq!(
                world.get::<HaloSuppressed>(entity).is_some(),
                wash,
                "{entity:?}"
            );
            assert_eq!(
                *world.get::<Visibility>(entity).unwrap(),
                if wash {
                    Visibility::Hidden
                } else {
                    Visibility::Inherited
                }
            );
        }
        world.resource_mut::<WallWashOff>().0 = false;
        schedule.run(&mut world);
        for &(entity, _) in &cases {
            assert!(world.get::<HaloSuppressed>(entity).is_none());
            assert_eq!(
                *world.get::<Visibility>(entity).unwrap(),
                Visibility::Inherited
            );
        }
        let sim = world.resource::<ParticleSimulator>();
        let mut clock = sim.clock;
        clock.wash_alpha_lift = 0.0;
        for (g, &(_, wash)) in sim.generators.iter().zip(&cases) {
            let baseline = drawn_factor(g, &sim.clock).w;
            assert!(baseline > 0.0);
            assert_eq!(drawn_factor(g, &clock).w, if wash { 0.0 } else { baseline });
        }
    }

    // Lamp halos (`lig*` sprite sheets) are LIGHTS: their drawn alpha is the lift knob times the
    // ToD gate and must not carry the `enhanced-particle-alpha-20` boost — that enhancement is
    // for hit-flash and other effect particles. Pins both sides so either half drifting fails a
    // test whichever feature set hits it.
    #[test]
    fn lamp_halo_alpha_bypasses_enhanced_particle_gain() {
        let mut g = live(def(60.0, 1.0, 1), 60.0);
        advance(&mut g, 2.0);
        assert!(!g.particles.is_empty(), "two frames emit");

        let clock = CelestialClock::default();
        let raw_alpha = g.def.init_color[3];
        let plain = drawn_factor(&g, &clock);
        assert_eq!(
            plain.w,
            expected_factor_alpha(raw_alpha),
            "non-halo particles carry the build's alpha gain"
        );

        g.def.mesh_id = *b"lig0";
        g.def.mesh_kind = ffxi_dat::particle_gen::ParticleMeshKind::SpriteSheet;
        // The content rule: ToD-gated alpha over a near-zero authored alpha.
        g.def.tod_color_driven[TOD_ALPHA_CHANNEL] = true;
        g.def.init_color[TOD_ALPHA_CHANNEL] = 0.071;
        let halo = drawn_factor(&g, &clock);
        assert!(
            is_lamp_halo_def(&g.def),
            "ToD-gated low-alpha sheet must hit the halo branch"
        );
        // No ToD track here: gate holds at 1.0, so alpha is the lift knob times the flicker wave
        // at this phase — still independent of the build's alpha gain.
        let seed = lamp_flicker_seed(&g.def);
        assert_eq!(
            halo.w,
            (clock.lamp_halos_lift
                * crate::zone_point_lights::lamp_flicker(clock.lamp_flicker_phase, seed))
            .clamp(0.0, 1.0)
        );
        assert_eq!(halo.xyz(), Vec3::ONE);
        let mut other_family = g.def;
        other_family.mesh_id = *b"lt__";
        assert_ne!(
            lamp_flicker_seed(&other_family),
            seed,
            "halo families waver out of step"
        );
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
        // Messages<T> has no FromWorld, so the bare test world inserts it for the SfxEvent writer.
        world.insert_resource(bevy::ecs::message::Messages::<crate::audio::SfxEvent>::default());
        world.insert_resource(bevy::ecs::message::Messages::<
            crate::scheduler_runtime::ParticleSpawnTrace,
        >::default());
        // tick reads the Dynamic Lights setting (Default = Vanilla, texture path).
        world.insert_resource(crate::graphics_settings::GraphicsSettings::default());
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(crate::distortion_pass::ActiveDistortion::default());
        world.insert_resource(sim);
        world.insert_resource(crate::distortion_pass::ActiveDistortion::default());
        world.run_system_once(tick_particle_simulator).unwrap();

        let sim = world.resource::<ParticleSimulator>();
        let frames = TICK_SECS * ROUTINE_FPS;
        let [ambient, routine] = &sim.generators[..] else {
            panic!("two generators");
        };
        assert_eq!(ambient.age_frames, frames);
        assert_eq!(ambient.age_frames, routine.age_frames);
        // ppe=1 emits two per period (authored count plus retail's closing iteration).
        assert_eq!(ambient.particles.len(), (frames * 2.0) as usize);
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

    // Everything outside weat/ emits its authored count plus retail's closing iteration, and an
    // authored count of 0 still emits the one particle that loop gives it.
    #[test]
    fn non_weather_emission_counts_are_unscaled() {
        let mut g = live(def(600.0, 1.0, 5), f32::MAX);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 6);

        let mut g = live(def(600.0, 1.0, 0), f32::MAX);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1);
    }

    // Without the sec2 0x06/0x07 spawn spread every drop of a curtain is emitted on one point.
    #[test]
    fn position_variance_spreads_emissions_through_the_sphere() {
        const RADIUS: f32 = 20.0;
        let mut d = def(60.0, 1.0, 199);
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
        // Authored count plus retail's closing iteration lands exactly one particle per step.
        let mut d = def(60.0, 1.0, STEPS - 1);
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

    // The camera flag lays the ring out against the camera in the world, so the attach frame's
    // rotation, which every other element offset passes through, must not turn it.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn camera_oriented_ring_ignores_the_attach_frame_rotation() {
        const RING_STEPS: u32 = 2;
        let ring = |frame_rot: Quat| {
            let mut d = def(ROUTINE_FPS, 1.0, RING_STEPS);
            d.init_velocity = [0.0; 3];
            d.spherical_full = Some(ffxi_dat::particle_gen::SphericalPositionVarianceFull {
                radius_variance: 0.0,
                base_radius: 1.0,
                axis_scale: [1.0; 3],
                rotation_z: 0.0,
                rotation_y: 0.0,
                tilt: 0.0,
                tilt_variance: 0.0,
                camera_oriented: true,
                azimuth_steps: RING_STEPS,
            });
            let mut g = live(d, f32::MAX);
            g.vel_basis = WORLD_PARTICLE_VEL_BASIS;
            g.frame_rot = frame_rot;
            g.cam_view = Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2);
            advance(&mut g, 1.0);
            let clock = CelestialClock::default();
            g.particles
                .iter()
                .map(|p| particle_draw(&g, p, &clock).world)
                .collect::<Vec<_>>()
        };
        let unturned = ring(Quat::IDENTITY);
        let turned = ring(Quat::from_rotation_y(CASTER_FACING));
        assert!(!unturned.is_empty());
        for (a, b) in unturned.iter().zip(&turned) {
            assert!(
                a.distance(*b) < FRAME_TOLERANCE,
                "the camera ring must not turn with the frame: {unturned:?} vs {turned:?}"
            );
        }
    }

    // ffxi_particle.wgsl runs retail's fixed-function tables per stage with D3D8 saturation
    // after every op; this mirror reproduces that math in Rust so the canonical values are
    // pinned without a GPU. The WGSL is the source of truth — keep the two in lockstep.
    mod stage_chain {
        use super::*;

        const STAGE_MODULATE_2X: f32 = 2.0;
        const STAGE_MODULATE_4X: f32 = 4.0;

        // Mirror of ffxi_particle.wgsl fragment(): d is the template colour (D3m/sheet stored
        // /128, MMB raw byte/255), texel the sampled T (white on untextured paths), f the
        // per-particle factor in raw byte/255. DoD3mDraw's textured table modulates TEXTURE
        // against CURRENT with a4 set and against TFACTOR with it clear, stage 1 taking the
        // other argument — both channels.
        fn retail_stages(
            path: D3mDrawPath,
            ignore_texture_alpha: bool,
            d: Vec4,
            texel: Vec4,
            f: Vec4,
        ) -> [f32; 4] {
            let d = d.min(Vec4::ONE);
            let (s0_rgb, s0_a) = match path {
                D3mDrawPath::D3m => (
                    Vec3::new(
                        (STAGE_MODULATE_2X * d.x * texel.x).min(1.0),
                        (STAGE_MODULATE_2X * d.y * texel.y).min(1.0),
                        (STAGE_MODULATE_2X * d.z * texel.z).min(1.0),
                    ),
                    if ignore_texture_alpha {
                        d.w
                    } else {
                        (STAGE_MODULATE_2X * d.w * texel.w).min(1.0)
                    },
                ),
                D3mDrawPath::Mmb => (
                    if ignore_texture_alpha {
                        Vec3::new(d.x * texel.x, d.y * texel.y, d.z * texel.z)
                    } else {
                        Vec3::new(texel.x * f.x, texel.y * f.y, texel.z * f.z)
                    },
                    if ignore_texture_alpha {
                        d.w
                    } else {
                        (STAGE_MODULATE_2X * f.w * texel.w).min(1.0)
                    },
                ),
                D3mDrawPath::D3mUntextured | D3mDrawPath::MmbUntextured => {
                    (Vec3::new(d.x, d.y, d.z), d.w)
                }
            };
            let (rgb_gain, arg) = match path {
                D3mDrawPath::Mmb => (STAGE_MODULATE_4X, if ignore_texture_alpha { f } else { d }),
                _ => (STAGE_MODULATE_2X, f),
            };
            [
                (rgb_gain * s0_rgb.x * arg.x).min(1.0),
                (rgb_gain * s0_rgb.y * arg.y).min(1.0),
                (rgb_gain * s0_rgb.z * arg.z).min(1.0),
                (STAGE_MODULATE_4X * s0_a * arg.w).min(1.0),
            ]
        }

        // NonZeroTwoTSS: rgb = 4*D*T*F, alpha = 8*D.a*T.a*F.a below saturation. A template
        // colour of 0.25 is a D3m byte-64 vertex — stage 0's MODULATE2X already folded in by
        // the /128 normalise.
        #[test]
        fn d3m_textured_default_reaches_the_retail_totals() {
            let out = retail_stages(
                D3mDrawPath::D3m,
                false,
                Vec4::splat(0.25),
                Vec4::ONE,
                Vec4::splat(0.25),
            );
            assert_eq!(out[..3], [4.0 * 0.25 * 1.0 * 0.25; 3]);
            assert!((out[3] - 8.0 * 0.25 * 1.0 * 0.25).abs() < 1e-6);
        }

        // NonZeroOneTSS (renderStateFlags 0x1000): stage 0's alpha is SELECTARG1(D.a) — no
        // texture alpha, no doubling — so with a full-alpha texel the TwoTSS table doubles
        // the vertex alpha and OneTSS halves the total; rgb untouched.
        #[test]
        fn d3m_ignoring_texture_alpha_halves_the_alpha_total() {
            let texel = Vec4::new(1.0, 1.0, 1.0, 1.0);
            let two = retail_stages(
                D3mDrawPath::D3m,
                false,
                Vec4::splat(0.25),
                texel,
                Vec4::splat(0.25),
            );
            let one = retail_stages(
                D3mDrawPath::D3m,
                true,
                Vec4::splat(0.25),
                texel,
                Vec4::splat(0.25),
            );
            assert_eq!(one[..3], two[..3]);
            assert!((one[3] - two[3] / 2.0).abs() < 1e-6);
        }

        // D3D saturates each stage on its own: a byte-255 vertex (stored 2.0 at /128) clips to
        // 1.0 before the texel multiply, so stage 1 starts from the clamped value.
        #[test]
        fn d3m_stage_zero_saturates_before_the_texel_and_factor() {
            let out = retail_stages(
                D3mDrawPath::D3m,
                false,
                Vec4::splat(u8::MAX as f32 / ffxi_dat::d3m::VERTEX_COLOR_DIVISOR),
                Vec4::ONE,
                Vec4::new(1.0, 1.0, 1.0, 0.15),
            );
            assert_eq!(out[..3], [1.0; 3]);
            assert!((out[3] - STAGE_MODULATE_4X * 0.15).abs() < 1e-6);
        }

        // ZeroOneTSS: a textureless D3m submesh is one stage against TFACTOR. An identity
        // (byte-128) vertex stores as 1.0 at /128, so under an identity raw factor the rgb
        // saturates to retail's full — the PS2 "0x80 = 1.0" convention expressed in stages.
        #[test]
        fn d3m_untextured_identity_vertex_reaches_full() {
            let stored = 0x80 as f32 / ffxi_dat::d3m::VERTEX_COLOR_DIVISOR;
            let factor = 0x80 as f32 / u8::MAX as f32;
            let out = retail_stages(
                D3mDrawPath::D3mUntextured,
                false,
                Vec4::splat(stored),
                Vec4::ONE,
                Vec4::splat(factor),
            );
            // The unsaturated products run past 1.0; D3D clamps at the stage boundary.
            assert!((out[0] - (STAGE_MODULATE_2X * stored * factor).min(1.0)).abs() < 1e-6);
            assert_eq!(out[0], 1.0, "byte-128 vertex under byte-128 factor is full");
            assert!((out[3] - (STAGE_MODULATE_4X * stored * factor).min(1.0)).abs() < 1e-6);
        }

        // DoD3mDraw untextured: MMB colours are raw byte/255 (no upload doubling), so the same
        // identity inputs land at half the D3m brightness — 0.504 rgb, saturated alpha.
        #[test]
        fn mmb_untextured_runs_the_raw_byte_tables() {
            let identity = 0x80 as f32 / u8::MAX as f32;
            let out = retail_stages(
                D3mDrawPath::MmbUntextured,
                false,
                Vec4::splat(identity),
                Vec4::ONE,
                Vec4::splat(identity),
            );
            assert!((out[0] - STAGE_MODULATE_2X * identity * identity).abs() < 1e-6);
            assert_eq!(out[3], 1.0, "alpha saturates: 4 * (128/255)^2 > 1");
        }

        // DoD3mDraw textured: with a full-alpha/full-colour texel the a4-clear ordering
        // (stage 0 MODULATE(T,TFACTOR), stage 1 MODULATE4X(CURRENT, DIFFUSE)) lands on the
        // same total as T*D*F — the doubling lives at stage 1 only.
        #[test]
        fn mmb_textured_doubles_only_at_stage_one() {
            let out = retail_stages(
                D3mDrawPath::Mmb,
                false,
                Vec4::splat(0.5),
                Vec4::ONE,
                Vec4::splat(0.25),
            );
            assert!((out[0] - STAGE_MODULATE_4X * 0.5 * 1.0 * 0.25).abs() < 1e-6);
        }

        // DoD3mDraw a4-clear alpha lane: MODULATE2X(TEXTURE, TFACTOR) at stage 0 — the factor,
        // not the vertex alpha — then MODULATE4X(CURRENT, DIFFUSE).
        #[test]
        fn mmb_a4_clear_alpha_takes_the_factor_at_stage_zero() {
            let out = retail_stages(
                D3mDrawPath::Mmb,
                false,
                Vec4::new(1.0, 1.0, 1.0, 0.5),
                Vec4::new(1.0, 1.0, 1.0, 0.5),
                Vec4::new(1.0, 1.0, 1.0, 0.25),
            );
            let s0a = (STAGE_MODULATE_2X * 0.25 * 0.5).min(1.0);
            assert!((out[3] - (STAGE_MODULATE_4X * s0a * 0.5).min(1.0)).abs() < 1e-6);
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
            let promote_byte = D3M_TFACTOR_PROMOTE_BLEND_BYTE;
            assert_eq!(
                promote(promote_byte, D3mDrawPath::D3mUntextured, at_threshold),
                D3M_TFACTOR_PROMOTED,
                "CMoD3m::Draw promotes ahead of its texture branch"
            );
            assert_eq!(
                promote(promote_byte, D3mDrawPath::MmbUntextured, at_threshold),
                at_threshold,
                "DoMMBDraw carries no promotion, textured or not"
            );
        }

        // The mesh carries D (template colour, COLOR) and F (per-particle factor, TANGENT
        // slot) as separate attributes — the shader runs the table against both.
        fn mesh_colors_and_factors(g: &LiveGenerator) -> (Vec<[f32; 4]>, Vec<[f32; 4]>) {
            let mut mesh = empty_mesh();
            // Authored fixed-function factors are the subject here — pin a neutral wash lift
            // (the default carries the box's slider seed, which multiplies this D3m path).
            let clock = CelestialClock {
                wash_alpha_lift: 1.0,
                ..CelestialClock::default()
            };
            rebuild_mesh(g, view(Quat::IDENTITY), &clock, &mut mesh);
            let colors = match mesh.attribute(Mesh::ATTRIBUTE_COLOR) {
                Some(bevy::mesh::VertexAttributeValues::Float32x4(v)) => v.clone(),
                _ => panic!("expected Float32x4 vertex colours"),
            };
            let factors = match mesh.attribute(Mesh::ATTRIBUTE_TANGENT) {
                Some(bevy::mesh::VertexAttributeValues::Float32x4(v)) => v.clone(),
                _ => panic!("expected Float32x4 particle factors"),
            };
            (colors, factors)
        }

        // One particle at half life; a trackless generator holds its authored init_color[3]
        // (1.0 here) as F.a for the whole element life — retail has no life-based fade.
        fn half_life_gen(blend: ffxi_dat::particle_gen::ParticleBlend, byte: u8) -> LiveGenerator {
            let mut d = def(100.0, 1.0, 1);
            d.blend = blend;
            d.blend_byte = byte;
            d.init_color = [1.0, 1.0, 1.0, 1.0];
            let mut g = live(d, 100.0);
            set_template_color(&mut g, Vec3::ONE.extend(0.5));
            g.particles.push(Particle {
                pos: Vec3::ZERO,
                spawn_pos: Vec3::ZERO,
                spawn_origin: Vec3::ZERO,
                vel: Vec3::ZERO,
                age_frames: 50.0,
                life_frames: 100.0,
                rgb: Vec3::ONE,
                color_transform: None,
                scale: Vec2::ONE,
                scale_seed: Vec2::ONE,
                scale_vel: Vec2::ZERO,
                rotation: Vec3::ZERO,
                spin: Vec3::ZERO,
                negate_rotation_y: false,
                rel_vel: Vec3::ZERO,
                vel_rot: Vec3::ZERO,
                osc: None,
                id: 0,
                child_gens: Vec::new(),
                pending_child_factories: Vec::new(),
            });
            g
        }

        // The template colour reaches the mesh verbatim as D; the stage gains live in the
        // shader, so nothing is folded into the attribute here.
        #[test]
        fn blended_particle_carries_the_template_colour_verbatim() {
            let mut g = half_life_gen(ffxi_dat::particle_gen::ParticleBlend::Blend, 0x03);
            set_template_color(&mut g, Vec3::splat(0.25).extend(0.5));
            let (colors, _) = mesh_colors_and_factors(&g);
            for c in &colors {
                assert_eq!(*c, [0.25, 0.25, 0.25, 0.5]);
            }
        }

        // The factor attribute carries the authored init_color (F.a = 1.0 here, held constant
        // over life — retail has no life fade), and the template's vertex alpha stays its own
        // value in COLOR; the shader's MODULATE4X does the combining.
        #[test]
        fn blended_particle_factor_carries_the_authored_alpha() {
            let mut g = half_life_gen(ffxi_dat::particle_gen::ParticleBlend::Blend, 0x03);
            set_template_color(&mut g, Vec3::ONE.extend(0.25));
            let (colors, factors) = mesh_colors_and_factors(&g);
            for c in &colors {
                assert_eq!(c[3], 0.25);
            }
            for f in &factors {
                assert_eq!(f[3], 1.0);
            }
        }

        // The 0x44 promotion lifts F.a from its raw byte value to full before the stage math.
        #[test]
        fn blend_byte_44_promotes_the_particle_alpha() {
            let mut g = half_life_gen(ffxi_dat::particle_gen::ParticleBlend::Blend, 0x44);
            // The authored alpha (retail's constant field_F8.a) the promotion acts on.
            g.def.init_color[3] = 0.5;
            set_template_color(&mut g, Vec3::ONE.extend(0.125));
            let (_, promoted) = mesh_colors_and_factors(&g);
            g.def.blend_byte = 0x03;
            let (_, unpromoted) = mesh_colors_and_factors(&g);
            assert_eq!(promoted[0][3], 1.0, "byte >= 0x7F promotes to full");
            assert!(
                (unpromoted[0][3] - expected_factor_alpha(0.5)).abs() < 1e-6,
                "raw byte/255 stays raw"
            );
        }

        // An additive element hands its alpha to the blend state as src alpha instead of
        // pre-multiplying it into rgb, so the shader's premultiply applies it to the colour
        // stage 1 already saturated — retail's order. The factor is the authored 0x16 value,
        // held constant mid-life (CMoElem.cpp VirtOt1: no life-based fade).
        #[test]
        fn additive_particle_carries_the_authored_alpha_as_src_alpha() {
            let mut g = half_life_gen(ffxi_dat::particle_gen::ParticleBlend::Additive, 0x48);
            set_template_color(&mut g, Vec3::splat(0.25).extend(0.5));
            let (colors, factors) = mesh_colors_and_factors(&g);
            for c in &colors {
                assert_eq!(*c, [0.25, 0.25, 0.25, 0.5]);
            }
            for f in &factors {
                assert_eq!(f[3], 1.0);
            }
        }

        // The home point's `sil` curtain (ROM/3/25.DAT) authors its plume as a per-vertex
        // white -> purple -> black ramp up each strip. Folding the colour once per particle
        // instead of keeping it per vertex drew the whole strip at the first vertex's white,
        // which is what made the rising streaks read as lit rectangles with no purple and no
        // fade-out at the top. The mesh carries the gradient verbatim; the shader applies the
        // table against it per fragment.
        #[test]
        fn a_template_colour_gradient_survives_into_the_mesh() {
            const WHITE: Vec4 = Vec4::ONE;
            const PURPLE: Vec4 = Vec4::new(0.26, 0.25, 0.49, 1.0);
            const BLACK: Vec4 = Vec4::new(0.0, 0.0, 0.0, 1.0);

            let mut g = half_life_gen(ffxi_dat::particle_gen::ParticleBlend::Additive, 0x48);
            g.template.colors = vec![WHITE, PURPLE, BLACK];

            let (drawn, _) = mesh_colors_and_factors(&g);
            assert_eq!(drawn.len(), 3, "one colour per template vertex");
            assert_eq!(drawn[0], WHITE.to_array());
            assert_eq!(drawn[1], PURPLE.to_array());
            assert_eq!(
                drawn[2],
                BLACK.to_array(),
                "the black end adds nothing, so an additive plume fades out"
            );
        }

        // Retail applies no life-based fade: VirtOt1 scales field_F8's alpha by field_138 *
        // field_134, both 1.0 (CYyGenerator.cpp ElemIdle sets field_138; the CMoElem ctor
        // initialises field_134 and nothing writes it), so a trackless additive spray holds its
        // authored brightness to end of life.
        #[test]
        fn additive_brightness_holds_the_authored_alpha_to_end_of_life() {
            let mut g = half_life_gen(ffxi_dat::particle_gen::ParticleBlend::Additive, 0x48);
            g.particles[0].age_frames = 90.0;
            let (_, factors) = mesh_colors_and_factors(&g);
            assert_eq!(factors[0][3], 1.0, "no life fade: the authored alpha holds");
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
        let mut d = def(120.0, 1.0, 7);
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
        let mut d = def(120.0, 1.0, 7);
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
        let mut d = def(120.0, 1.0, 7);
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
        let mut d = def(120.0, 1.0, 15);
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
        let mut d = def(120.0, 1.0, 7);
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

    // sec3 0x2E DrawDistanceUpdater: the element's alpha fades linearly from full at `near`
    // to zero at `far` by camera distance and is culled beyond (research/XIClient/src/
    // XIClient/source/World/Generator/CYyGenerator.cpp CYyGenerator::ElemIdle case 0x2E;
    // research/xim ParticleUpdaters.kt DrawDistanceUpdater).
    #[test]
    fn draw_distance_fades_alpha_and_culls_beyond_far() {
        let make = |near: Option<f32>, far: Option<f32>| -> LiveGenerator {
            let mut d = def(120.0, 1.0, 1);
            d.camera_billboard = false;
            d.continuous = true;
            d.draw_distance_near = near;
            d.draw_distance_far = far;
            let mut g = live(d, 1000.0);
            advance(&mut g, 1.0);
            assert_eq!(g.particles.len(), 1);
            g
        };
        let clock = CelestialClock::default();
        let cam = view(Quat::IDENTITY);
        let alphas = |g: &LiveGenerator| -> Vec<f32> {
            use bevy::mesh::VertexAttributeValues;
            let mut mesh = empty_mesh();
            rebuild_mesh(g, cam, &clock, &mut mesh);
            match mesh.attribute(Mesh::ATTRIBUTE_TANGENT) {
                Some(VertexAttributeValues::Float32x4(v)) => v.iter().map(|c| c[3]).collect(),
                _ => panic!("rebuilt mesh has f32x4 factors"),
            }
        };

        // Inside `near` the fade is 1.0: identical to a generator without the updater.
        let mut plain = make(None, None);
        plain.origin = Vec3::new(5.0, 0.0, 0.0);
        let mut faded = make(Some(10.0), Some(20.0));
        faded.origin = Vec3::new(5.0, 0.0, 0.0);
        assert_eq!(alphas(&plain), alphas(&faded));

        // Mid-band: the fade is (far - dist) / (far - near) = 0.5.
        plain.origin = Vec3::new(15.0, 0.0, 0.0);
        faded.origin = Vec3::new(15.0, 0.0, 0.0);
        let full = alphas(&plain);
        let half = alphas(&faded);
        assert!(
            full.iter().all(|a| *a > 0.0),
            "the plain generator draws at mid-band"
        );
        for (f, h) in full.iter().zip(half.iter()) {
            assert!((h - f * 0.5).abs() < 1e-6, "mid-band fade: {f} vs {h}");
        }

        // Beyond `far` the element is culled from the draw list: only the hidden primitive.
        faded.origin = Vec3::new(30.0, 0.0, 0.0);
        let mut mesh = empty_mesh();
        rebuild_mesh(&faded, cam, &clock, &mut mesh);
        assert_eq!(mesh.count_vertices(), HIDDEN_PRIMITIVE_VERTS);
    }

    // sec2 0x21..0x23 + sec3 0x0F..0x11: a bound track replaces the position channel each
    // frame, key 0 seeded from the spawn-time value (research/xim ParticleUpdaters.kt
    // ProgressValueUpdater — p.position.x = v).
    #[test]
    fn position_tracks_replace_the_channel() {
        let make = |track: Option<ffxi_dat::particle_gen::KeyFrameTrack>| -> LiveGenerator {
            let mut d = def(120.0, 1.0, 1);
            d.camera_billboard = false;
            d.continuous = true;
            let mut g = live(d, 1000.0);
            g.tracks.position_x = track;
            advance(&mut g, 1.0);
            assert_eq!(g.particles.len(), 1);
            // Pin the element at half life: progress is what the track samples.
            g.particles[0].age_frames = g.particles[0].life_frames * 0.5;
            g
        };
        let cam = view(Quat::IDENTITY);
        let x_at = |g: &LiveGenerator| rebuilt(g, cam).0[0].x;

        // No track: the channel keeps its spawn value.
        assert_eq!(x_at(&make(None)), 0.0);

        // Track (0 -> 5): key 0 is overridden by the spawn value 0, so at half life the
        // channel is halfway to 5.
        let g = make(Some(ffxi_dat::particle_gen::KeyFrameTrack {
            points: vec![(0.0, 99.0), (1.0, 5.0)],
        }));
        assert!((x_at(&g) - 2.5).abs() < 1e-6);
    }

    #[test]
    fn velocity_dampener_decays_the_velocity() {
        let make = |dampen: Option<f32>| -> LiveGenerator {
            let mut d = def(100.0, 1.0, 1);
            d.continuous = true;
            d.init_velocity = [1.0, 0.0, 0.0];
            d.velocity_dampener = dampen.map(|v| [v, 0.0]);
            let mut g = live(d, 1000.0);
            advance(&mut g, 1.0);
            assert_eq!(g.particles.len(), 1);
            g.stopped = true;
            for _ in 0..3 {
                advance(&mut g, 1.0);
            }
            g
        };
        // No dampener: three unit steps.
        let plain = make(None);
        assert!((plain.particles[0].pos.x - 3.0).abs() < 1e-6);
        let damped = make(Some(0.5));
        const DAMPED_DISPLACEMENT: f32 = 1.0 + 0.5 + 0.25;
        const DAMPED_VELOCITY: f32 = 0.125;
        assert!((damped.particles[0].pos.x - DAMPED_DISPLACEMENT).abs() < 1e-6);
        assert!((damped.particles[0].vel.x - DAMPED_VELOCITY).abs() < 1e-6);
    }

    // sec2 0x69 + sec3 0x44: the bound track overrides the authored base factor per frame —
    // a constant-1.0 track disables the decay entirely (research/xim ParticleUpdaters.kt
    // VelocityDampener getDampeningFactor).
    #[test]
    fn dampening_factor_track_overrides_the_base() {
        let mut d = def(100.0, 1.0, 1);
        d.continuous = true;
        d.init_velocity = [1.0, 0.0, 0.0];
        d.velocity_dampener = Some([0.5, 0.0]);
        d.dampening_factor_applier = true;
        let mut g = live(d, 1000.0);
        g.tracks.dampening_factor = Some(ffxi_dat::particle_gen::KeyFrameTrack {
            points: vec![(0.0, 1.0), (1.0, 1.0)],
        });
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1);
        g.stopped = true;
        for _ in 0..3 {
            advance(&mut g, 1.0);
        }
        assert!((g.particles[0].pos.x - 3.0).abs() < 1e-6);
    }

    // sec3 0x26 VelocityRotator: the rotateAmount accumulates into the velocity rotation at
    // half rate per frame and turns the trajectory; an actor-local generator authors it in
    // actor space, where z is forward — xim swaps the axes to (-z, y, x) for those
    // (research/xim ParticleUpdaters.kt VelocityRotator).
    #[test]
    fn velocity_rotator_turns_the_trajectory() {
        let make = |actor_local: bool| -> LiveGenerator {
            let mut d = def(100.0, 1.0, 1);
            d.continuous = true;
            if actor_local {
                d.init_velocity = [0.0, 1.0, 0.0];
            } else {
                d.init_velocity = [1.0, 0.0, 0.0];
            }
            d.velocity_rotator = Some([0.0, 0.0, std::f32::consts::FRAC_PI_2]);
            let mut g = live(d, 1000.0);
            g.actor_local = actor_local;
            advance(&mut g, 1.0);
            assert_eq!(g.particles.len(), 1);
            g.stopped = true;
            for _ in 0..4 {
                advance(&mut g, 1.0);
            }
            g
        };
        // World-space: the authored z amount rotates about z — four half-rate quarter-turns
        // land the +x velocity on −x.
        let world = make(false);
        assert!((world.particles[0].pos.x - (-1.0)).abs() < 1e-5);
        // Actor-local: the same authored z amount becomes a negative x rotation — the y
        // velocity swings through +z and ends on −y.
        let local = make(true);
        assert!((local.particles[0].pos.y - (-1.0)).abs() < 1e-5);
        assert!(
            (local.particles[0].pos.z - (std::f32::consts::SQRT_2 + 1.0)).abs() < 1e-5,
            "actor-local z sweep: {}",
            local.particles[0].pos.z
        );
    }

    // sec3 0x2F VelocityRotationUpdater: all velocity collapses into +x and the particle's
    // rotation becomes the velocity rotation (research/xim ParticleUpdaters.kt
    // VelocityRotationUpdater).
    #[test]
    fn velocity_rotation_updater_collapses_into_x() {
        let mut d = def(100.0, 1.0, 1);
        d.continuous = true;
        d.init_velocity = [3.0, 4.0, 0.0];
        d.velocity_rotation_updater = true;
        let mut g = live(d, 1000.0);
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1);
        g.stopped = true;
        advance(&mut g, 1.0);
        let p = &g.particles[0];
        // |vel| + |rel_vel| = 5 on +x; the zero rotation keeps it there.
        assert!((p.vel.x - 5.0).abs() < 1e-6 && p.vel.y.abs() < 1e-6);
        assert!((p.pos.x - 5.0).abs() < 1e-6);
    }

    // sec2 0x31 RandomVelocitySetup: the base velocity is replaced by one [0, v) draw shared
    // by all three axes (research/xim ParticleInitializers.kt RandomVelocitySetup).
    #[test]
    fn random_velocity_replaces_the_base() {
        let make = |random: Option<f32>| -> LiveGenerator {
            let mut d = def(100.0, 1.0, 1);
            d.init_velocity = [9.0, 9.0, 9.0];
            d.random_velocity = random;
            let mut g = live(d, 1000.0);
            advance(&mut g, 1.0);
            // One burst: ppe plus retail's closing iteration.
            assert_eq!(g.particles.len(), 2);
            g
        };
        // Without the block the base survives.
        for p in &make(None).particles {
            assert!((p.vel - Vec3::new(9.0, 9.0, 9.0)).length() < 1e-6);
        }
        // With it, every axis carries the same draw in [0, v).
        for p in &make(Some(2.0)).particles {
            assert!((p.vel.x - p.vel.y).abs() < 1e-6 && (p.vel.y - p.vel.z).abs() < 1e-6);
            assert!(
                p.vel.x >= 0.0 && p.vel.x < 2.0,
                "draw out of range: {}",
                p.vel.x
            );
        }
    }

    // sec2 0x19 + sec3 0x0B: each frame rgb += (transform shr 7) × 0.5, in xim's raw byte
    // space divided by the PS2 half-scale — a red transform of 256 adds 2 × 0.5 / 128 per
    // frame (research/xim ParticleUpdaters.kt ColorTransformApplier).
    #[test]
    fn color_transform_applies_the_shifted_rate() {
        let make = |applier: bool| -> LiveGenerator {
            let mut d = def(100.0, 1.0, 1);
            d.continuous = true;
            d.color_transform = Some([256, -256, 0, 0]);
            d.color_transform_applier = applier;
            let mut g = live(d, 1000.0);
            advance(&mut g, 1.0);
            assert_eq!(g.particles.len(), 1);
            g.stopped = true;
            for _ in 0..4 {
                advance(&mut g, 1.0);
            }
            g
        };
        let plain = make(false);
        let applied = make(true);
        // Four frames × (256 shr 7) × 0.5 / 255 on red, the negative of it on green.
        let step = 4.0 * 2.0 * COLOR_TRANSFORM_STEP_HALF / COLOR_TRANSFORM_BYTE_SCALE;
        assert!((applied.particles[0].rgb.x - plain.particles[0].rgb.x - step).abs() < 1e-6);
        assert!((plain.particles[0].rgb.y - applied.particles[0].rgb.y - step).abs() < 1e-6);
        // Blue untouched.
        assert_eq!(applied.particles[0].rgb.z, plain.particles[0].rgb.z);
    }

    // sec3 0x0C: the transform drifts by floor(modifier × dt/30) per frame and the applier
    // reads the drifted value — a red modifier of 960 adds 32/frame, so shr-7 stays 0 until
    // frame 4 (research/xim ParticleUpdaters.kt ColorTransformModifier).
    #[test]
    fn color_transform_modifier_drifts_the_transform() {
        let make = |modifier: Option<[i16; 4]>| -> LiveGenerator {
            let mut d = def(100.0, 1.0, 1);
            d.continuous = true;
            d.color_transform = Some([0, 0, 0, 0]);
            d.color_transform_applier = true;
            d.color_transform_modifier = modifier;
            let mut g = live(d, 1000.0);
            advance(&mut g, 1.0);
            assert_eq!(g.particles.len(), 1);
            g.stopped = true;
            for _ in 0..5 {
                advance(&mut g, 1.0);
            }
            g
        };
        let plain = make(None);
        let drifted = make(Some([960, 0, 0, 0]));
        // Frames 4 and 5 each add 1 × 0.5 / 255 to red.
        let expected = 2.0 * COLOR_TRANSFORM_STEP_HALF / COLOR_TRANSFORM_BYTE_SCALE;
        assert!(
            (drifted.particles[0].rgb.x - plain.particles[0].rgb.x - expected).abs() < 1e-6,
            "drifted: {}",
            drifted.particles[0].rgb.x
        );
    }

    // sec2 0x1A: each element's transform gains round(posRand(1) × variance) per channel on
    // top of the base (research/xim ParticleInitializers.kt ColorTransformVariance).
    #[test]
    fn color_transform_variance_spreads_the_base() {
        let mut d = def(100.0, 1.0, 1);
        d.color_transform = Some([100, 0, 0, 0]);
        d.color_transform_variance = Some([-100, 0, 0, 0]);
        let mut g = live(d, 1000.0);
        advance(&mut g, 1.0);
        assert!(!g.particles.is_empty());
        for p in &g.particles {
            let ct = p.color_transform.expect("setup allocates the transform");
            assert!(ct[0] >= 0 && ct[0] <= 100, "draw out of range: {}", ct[0]);
        }
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
        assert_eq!(
            g.particles.len(),
            2,
            "one per period plus retail's closing iteration"
        );
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
        assert_eq!(
            g.particles.len(),
            2,
            "one per period plus retail's closing iteration"
        );
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
        assert_eq!(
            g.particles.len(),
            2,
            "one per period plus retail's closing iteration"
        );
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
            spawn_pos: Vec3::ZERO,
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
            vel_rot: Vec3::ZERO,
            color_transform: None,
            osc: Some(Oscillation {
                accel: [0.5, 0.0, 0.0],
                prev_amplitude: [0.0; 3],
            }),
            id: 0,
            child_gens: Vec::new(),
            pending_child_factories: Vec::new(),
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
                spawn_pos: Vec3::new(1.0, 2.0, 3.0),
                spawn_origin: Vec3::ZERO,
                vel: Vec3::ZERO,
                age_frames: 50.0,
                life_frames: 100.0,
                rgb: Vec3::ONE,
                color_transform: None,
                scale: Vec2::ONE,
                scale_seed: Vec2::ONE,
                scale_vel: Vec2::ZERO,
                rotation: Vec3::ZERO,
                spin: Vec3::ZERO,
                negate_rotation_y: false,
                rel_vel: Vec3::ZERO,
                vel_rot: Vec3::ZERO,
                osc: None,
                id: 0,
                child_gens: Vec::new(),
                pending_child_factories: Vec::new(),
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

        // The D3m stage chain folds the element's alpha into the key's colour, so an alpha
        // change alone dirties the mesh. Retail fades come from an explicit keyframe track
        // (CMoElem.cpp VirtOt1 has no life-based fade), so drive the change through one.
        #[test]
        fn alpha_stage_change_rebuilds() {
            let mut g = one_particle_gen();
            g.tracks.alpha = Some(ffxi_dat::particle_gen::KeyFrameTrack {
                points: vec![(0.0, 1.0), (1.0, 0.25)],
            });
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
        // 20 frames at 1/frame, period 5 -> 4 emits within window (the emit at accum reset),
        // two particles each.
        for _ in 0..20 {
            advance(&mut g, 1.0);
        }
        assert_eq!(g.particles.len(), 8);
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

    // A dur=0 stage (hit1's g01x) must land its primed burst on the first tick even when that
    // tick advances more than one frame — at 30 fps every tick is two frames.
    #[test]
    fn zero_window_emits_first_burst_on_a_long_first_tick() {
        let mut g = live(def(20.0, 5.0, 3), 0.0);
        g.emit_accum = def(20.0, 5.0, 3).frames_per_emission;
        advance(&mut g, 2.0); // one 30 fps tick
        assert_eq!(
            g.particles.len(),
            4,
            "the primed burst lands on the first tick"
        );
        advance(&mut g, 2.0);
        assert_eq!(
            g.particles.len(),
            4,
            "a dur=0 generator emits exactly one burst"
        );
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
        world.insert_resource(crate::distortion_pass::ActiveDistortion::default());
        world.insert_resource(Time::<()>::default());

        world.run_system_once(track_attached_origins).unwrap();
        assert_eq!(
            world.resource::<ParticleSimulator>().generators[0].origin,
            Vec3::ZERO,
            "the origin snaps to the actor's reference point"
        );

        *world.get_mut::<Transform>(owner).unwrap() =
            Transform::from_translation(Vec3::new(3.0, 0.0, -4.0));
        world.run_system_once(track_attached_origins).unwrap();
        assert_eq!(
            world.resource::<ParticleSimulator>().generators[0].origin,
            Vec3::new(3.0, 0.0, -4.0),
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
        world.insert_resource(crate::distortion_pass::ActiveDistortion::default());
        world.insert_resource(Time::<()>::default());

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
        let mut parent = live(def(ROUTINE_FPS, ROUTINE_FPS, BIRTHS - 1), 0.0);
        parent.entity = Entity::from_bits(1);
        parent.def.position_variance = Some(ffxi_dat::particle_gen::PositionVariance {
            radius_variance: 1.0,
            base_radius: 1.0,
            axis_scale: [1.0; 3],
        });
        let mut child = live(def(ROUTINE_FPS, ROUTINE_FPS, 0), 0.0);
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
        let mut parent = live(def(30.0, 100.0, 0), 0.0);
        parent.origin = PARENT_POSITION;
        parent.entity = Entity::from_bits(1);
        let mut child = live(def(30.0, 100.0, 0), 0.0);
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
        let mut g = live(def(ROUTINE_FPS, ROUTINE_FPS, 0), 0.0);
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
            drawn_factor(&g, &CelestialClock::default()).is_finite(),
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
        assert_eq!(
            g.particles.len(),
            10,
            "one draw per particle (two per period)"
        );
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
        assert_eq!(g.particles.len(), 2);
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
        assert_eq!(g.particles.len(), 6);
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
    // of the 0x16 base (research/xim ParticleInitializers.kt ColorVarianceSetup). Both are raw
    // D3DCOLOR bytes/255 and stay that way — the stage doubling happens in the shader.
    #[test]
    fn color_variance_spreads_each_channel_upward() {
        let mut d = def(10.0, 1.0, 1);
        d.init_color = [0.2, 0.2, 0.2, 1.0];
        d.color_variance = Some([0.3, 0.15, 0.075, 0.0]);
        let mut g = live(d, 30.0);
        advance(&mut g, 3.0);
        assert_eq!(g.particles.len(), 6);
        for p in &g.particles {
            let c = p.rgb;
            // raw [base, base + bound) per channel.
            assert!(
                (0.2..0.5).contains(&c.x),
                "the red channel stays in the raw [base, base + bound): {c:?}"
            );
            assert!(
                (0.2..0.35).contains(&c.y),
                "the green channel stays in the raw [base, base + bound): {c:?}"
            );
            assert!(
                (0.2..0.275).contains(&c.z),
                "the blue channel stays in the raw [base, base + bound): {c:?}"
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
        // life=0 is retail's singleton marker: one particle, not the ppe+1 burst.
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
        assert_eq!(g.particles.len(), 2);
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
        assert_eq!(g.particles.len(), 6);
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
        assert_eq!(g.particles.len(), 6);
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
            spawn_pos: Vec3::ZERO,
            spawn_origin: Vec3::ZERO,
            vel: Vec3::ZERO,
            age_frames: 0.0,
            life_frames: 1.0,
            rgb: Vec3::from_slice(&g.def.init_color[..3]),
            color_transform: None,
            scale: Vec2::ONE,
            scale_seed: Vec2::ONE,
            scale_vel: Vec2::ZERO,
            rotation: Vec3::ZERO,
            spin: Vec3::ZERO,
            negate_rotation_y: false,
            rel_vel: Vec3::ZERO,
            vel_rot: Vec3::ZERO,
            osc: None,
            id: 0,
            child_gens: Vec::new(),
            pending_child_factories: Vec::new(),
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

    // The same rebuild, reading D (template colour) and F (per-particle factor) as separate
    // attributes — the shader runs the table against both.
    fn rebuilt_colors_and_factors(g: &LiveGenerator, cam: CameraView) -> (Vec<Vec4>, Vec<Vec4>) {
        use bevy::mesh::VertexAttributeValues::Float32x4;
        let mut mesh = empty_mesh();
        // These tests assert authored fixed-function factors; the wash-alpha slider multiplies
        // them on the D3m path, so pin a neutral lift (the default carries the box's seed).
        let clock = CelestialClock {
            wash_alpha_lift: 1.0,
            ..CelestialClock::default()
        };
        rebuild_mesh(g, cam, &clock, &mut mesh);
        let Some(Float32x4(col)) = mesh.attribute(Mesh::ATTRIBUTE_COLOR) else {
            panic!("rebuilt mesh has f32x4 colours");
        };
        let Some(Float32x4(fac)) = mesh.attribute(Mesh::ATTRIBUTE_TANGENT) else {
            panic!("rebuilt mesh has f32x4 particle factors");
        };
        (
            col.iter().copied().map(Vec4::from_array).collect(),
            fac.iter().copied().map(Vec4::from_array).collect(),
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
        // Along +Z so the eye direction is that axis to well inside FACING, and inside
        // bun4's authored sec3 0x2E draw-distance band (near 20 / far 30), beyond which the
        // sheet is culled from the draw list.
        const EYE_DISTANCE: f32 = 15.0;
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

    // research/xim Particle.kt applyMovementOrientation through GLDrawer's untransposed upload — pin
    // the full basis by its images of the local axes so both order and sign show up.
    #[test]
    fn movement_orientation_matches_the_viewers_fold() {
        assert_eq!(movement_orientation(Vec3::ZERO), Quat::IDENTITY);
        let vertical = movement_orientation(Vec3::new(0.0, -1.0, 0.0));
        assert!((vertical - Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2)).length() < 1e-5);
        let q = movement_orientation(Vec3::new(0.0, -0.4472136, 0.8944272));
        assert!((q * Vec3::X - Vec3::Z).length() < 1e-4);
        assert!((q * Vec3::Y - Vec3::new(-0.4472136, 0.8944272, 0.0)).length() < 1e-4);
        assert!((q * Vec3::Z - Vec3::new(-0.8944272, -0.4472136, 0.0)).length() < 1e-4);
    }

    // hi14's g140 shape: a Movement billboard whose ±π x-variance must fan the burst out of one ray —
    // the screen-billboard fold stacked every band on the same line (the single-ray crit).
    #[test]
    fn movement_billboard_fans_the_rotation_variance_out() {
        let mut d = def(10.0, 1.0, 3);
        d.billboard = ParticleBillboard::Movement;
        d.camera_billboard = false;
        d.init_velocity = [0.0; 3];
        d.rotation_variance = Some([std::f32::consts::PI, 0.0, 0.0]);
        let mut g = live(d, 100.0);
        g.template.positions = vec![
            Vec3::new(-1.0, -0.5, 0.0),
            Vec3::new(1.0, -0.5, 0.0),
            Vec3::new(0.0, 0.5, 0.0),
        ];
        advance(&mut g, 1.0);
        assert_eq!(g.particles.len(), 4);
        let (pos, _) = rebuilt(&g, view(Quat::IDENTITY));
        // First vertex of each band: the x-rotation tilts it out of the XY plane by -0.05·sin(θ).
        let zs: Vec<f32> = pos.iter().step_by(3).map(|p| p.z).collect();
        let spread = (zs.iter().cloned().fold(f32::INFINITY, f32::min)
            - zs.iter().cloned().fold(f32::NEG_INFINITY, f32::max))
        .abs();
        assert!(spread > 1e-3, "the bands fan out: {zs:?}");
    }

    // The sun/moon domes are untextured meshes whose whole shape is a vertex-alpha ramp (128 at
    // the centre to 0 at the rim). Folding the per-particle factor into that colour on the CPU
    // flattens the ramp; the mesh carries D (template, verbatim) and F (factor) as separate
    // attributes and ffxi_particle.wgsl runs the table against both. Nothing is rescaled here:
    // F.a is def()'s raw init alpha byte (0.5), held for the whole life — independent of
    // progress, on every draw path.
    #[test]
    fn mmb_additive_keeps_the_vertex_alpha_gradient() {
        const VERTEX_ALPHAS: [f32; 3] = [1.0, 0.75, 0.0];
        let mut g = axial_celestial([1.0; 3], Vec3::X);
        g.template.positions = vec![Vec3::X; VERTEX_ALPHAS.len()];
        g.template.uvs = vec![[0.0, 0.0]; VERTEX_ALPHAS.len()];
        g.template.indices = vec![0, 1, 2];
        g.template.colors = VERTEX_ALPHAS
            .iter()
            .map(|&a| Vec4::new(1.0, 1.0, 1.0, a))
            .collect();

        let cam = CameraView {
            rot: Quat::IDENTITY,
            pos: Vec3::new(900.0, 0.0, 0.0),
        };

        for path in [D3mDrawPath::Mmb, D3mDrawPath::D3m] {
            g.draw_path = path;
            let (colors, factors) = rebuilt_colors_and_factors(&g, cam);
            for ((c, f), a) in colors.iter().zip(factors).zip(VERTEX_ALPHAS) {
                assert!(
                    (c.w - a).abs() < 1e-6,
                    "{path:?} colour keeps the vertex ramp: {}",
                    c.w
                );
                assert!(
                    (f[3] - expected_factor_alpha(g.def.init_color[3])).abs() < 1e-6,
                    "{path:?} factor is the raw init alpha byte: {}",
                    f[3]
                );
            }
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
        g.tracks.tod_color[0] = Some(ramp(0.0, 1.0));

        // The particle never ages (life_frames == 1, age 0), so any change here is the clock.
        let at = |day_fraction: f32| {
            drawn_factor(
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
        // Low enough that the 2x modulates do not saturate the channel and hide the tint.
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
            ..Default::default()
        };
        let untinted = celestial(blended_celestial_def());
        let plain = drawn_factor(&untinted, &clock).x;
        let tinted = drawn_factor(&g, &clock).x;
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
                    ..Default::default()
                },
            )
            .factor_alpha
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
            // Neutral wash lift: the assertion is on the DAT's own alpha lane.
            &CelestialClock {
                day_fraction: 0.5,
                day_of_week: 0,
                moon_phase,
                wash_alpha_lift: 1.0,
                ..Default::default()
            },
        )
        .factor_alpha
    }

    // The shipped f_ro (zone DAT 210) tables: `kasa`, the lunar halo, carries a phase alpha
    // lane that is zero outside phases 5..=7, while the `moon` sprite's never drops below its
    // byte-108 entry. With the alpha lane dropped, the halo drew as a saturated disc ~20
    // degrees across that swamped the moon at every phase. Every value here is a raw D3DCOLOR
    // byte /255 and each tint table is one saturating 2x modulate (research/xim Particle.kt
    // getColor), weekday first then phase — the same chain particle_draw runs, so the expected
    // values mirror it in byte space. The drawn alpha is pinned to a value, not just "> 0",
    // so a halo that regressed to near-invisible near full moon also fails. Skips without a
    // retail install.
    #[test]
    fn zone_210_lunar_halo_is_dark_except_near_full_moon() {
        const F_RO: u32 = 210;
        // `kasa`'s phase alpha lane as shipped, dumped byte-for-byte from f_ro.
        const HALO_PHASE_ALPHA_BYTE: [u8; ffxi_dat::particle_gen::MOON_PHASES] =
            [0, 0, 0, 0, 0, 60, 128, 60, 0, 0, 0, 0];
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

        // particle_draw's tint chain in raw byte space: each present table is a saturating
        // 2x modulate, weekday (day 0 here) first, then the phase lane.
        let tinted = |def: &ParticleGeneratorDef, phase: usize| {
            let mut a = def.init_color[3];
            if let Some(t) = &def.day_of_week_color {
                a = (a * t[0][TOD_ALPHA_CHANNEL] * CELESTIAL_MODULATE).min(1.0);
            }
            if let Some(t) = &def.moon_phase_color {
                a = (a * t[phase][TOD_ALPHA_CHANNEL] * CELESTIAL_MODULATE).min(1.0);
            };
            a
        };

        for phase in 0..ffxi_dat::particle_gen::MOON_PHASES {
            let lane = HALO_PHASE_ALPHA_BYTE[phase] as f32 / u8::MAX as f32;
            assert!(
                (halo_table[phase][3] - lane).abs() < ALPHA_EPS,
                "halo alpha lane read back from the DAT, phase {phase}: \
                 {} vs {lane}",
                halo_table[phase][3]
            );

            let halo_alpha = phase_alpha(&halo, phase);
            let expected = expected_factor_alpha(tinted(&halo, phase));
            assert!(
                (halo_alpha - expected).abs() < ALPHA_EPS,
                "halo draws its DAT alpha lane, phase {phase}: {halo_alpha} vs {expected}"
            );

            let sprite_alpha = phase_alpha(&sprite, phase);
            let sprite_expected = expected_factor_alpha(tinted(&sprite, phase));
            assert!(
                (sprite_alpha - sprite_expected).abs() < ALPHA_EPS,
                "moon draws its own table, phase {phase}: {sprite_alpha} vs {sprite_expected}"
            );

            if (5..=7).contains(&phase) {
                assert!(halo_alpha > 0.0, "halo is lit near full moon");
            } else {
                assert_eq!(halo_alpha, 0.0, "halo is dark outside the full-moon phases");
            }
        }
    }

    // The alpha lattice a decoded DXT3 texture sits on before conversion: the 4-bit plane holds
    // multiples of the dither step (ffxi-dat/src/texture.rs DXT3_ALPHA_DITHER_STEP), so any other
    // value is a neighbourhood mean, i.e. proof the undither ran.
    fn off_nibble_lattice(alpha: &[u8]) -> usize {
        use ffxi_dat::texture::DXT3_ALPHA_DITHER_STEP;

        let step = DXT3_ALPHA_DITHER_STEP as usize;
        alpha
            .iter()
            .filter(|a| !(**a as usize).is_multiple_of(step))
            .count()
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
    // on zone files 210/331). The plain particle converter only applies the shared alpha remap;
    // only the celestial converter undithers and expands it to the authored half-step. Skips
    // without a retail install.
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

        // The plain D3M path never undithers, but it does apply the shared alpha
        // remap (dat_d3m::convert): each stored nibble lands on its remapped value
        // and nothing between them.
        let mut plain_alpha: Vec<u8> = image_alpha(&images, &plain);
        plain_alpha.sort_unstable();
        plain_alpha.dedup();
        let remapped_lo = ffxi_dat::texture::ffxi_alpha_remap(DITHER_LO);
        let remapped_hi = ffxi_dat::texture::ffxi_alpha_remap(DITHER_HI);
        assert_eq!(
            plain_alpha,
            [remapped_lo, remapped_hi].to_vec(),
            "the shared converter applies the alpha remap to both nibbles and no undither"
        );

        let sky_alpha = image_alpha(&images, &sky);
        let spread =
            sky_alpha.iter().max().expect("non-empty") - sky_alpha.iter().min().expect("non-empty");
        // The celestial path still expands: the undithered mean 127.5 doubles to a 254/255 split.
        let lo = ffxi_dat::texture::ffxi_alpha_remap(DITHER_LO);
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
            "the shared particle converter passes stored nibble alpha through as-is"
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
        // Raw alpha byte/255: the factor carries it verbatim for the whole life — retail has
        // no CPU rescale and no life fade (CMoElem.cpp VirtOt1).
        base.init_color = [1.0, 1.0, 1.0, 0.25];

        // Vertex alpha stays its own value in COLOR; only the factor is asserted here.
        const VERT_ALPHA: f32 = 0.125;
        let mut cont = live(base, 1.0);
        cont.def.continuous = true;
        set_template_color(&mut cont, Vec3::ONE.extend(VERT_ALPHA));
        let mut spray = live(base, 1.0);
        set_template_color(&mut spray, Vec3::ONE.extend(VERT_ALPHA));

        let particle = |age: f32| Particle {
            pos: Vec3::ZERO,
            spawn_pos: Vec3::ZERO,
            spawn_origin: Vec3::ZERO,
            vel: Vec3::ZERO,
            age_frames: age,
            life_frames: 4.0,
            rgb: Vec3::ONE,
            color_transform: None,
            scale: Vec2::splat(0.1),
            scale_seed: Vec2::splat(0.1),
            scale_vel: Vec2::ZERO,
            rotation: Vec3::ZERO,
            spin: Vec3::ZERO,
            negate_rotation_y: false,
            rel_vel: Vec3::ZERO,
            vel_rot: Vec3::ZERO,
            osc: None,
            id: 0,
            child_gens: Vec::new(),
            pending_child_factories: Vec::new(),
        };
        cont.particles = vec![particle(3.0)];
        spray.particles = vec![particle(3.0)];

        let expected_alpha = expected_factor_alpha(base.init_color[3]);

        // The wash-alpha slider multiplies authored alpha on this D3m path; the assertion is on
        // the factor itself, so pin a neutral lift (the clock default carries the box's seed).
        let clock = CelestialClock {
            wash_alpha_lift: 1.0,
            ..CelestialClock::default()
        };
        let factor_alpha_of = |g: &LiveGenerator| -> f32 {
            let mut mesh = empty_mesh();
            rebuild_mesh(g, view(Quat::IDENTITY), &clock, &mut mesh);
            match mesh.attribute(Mesh::ATTRIBUTE_TANGENT).unwrap() {
                bevy::mesh::VertexAttributeValues::Float32x4(f) => f[0][3],
                _ => panic!("expected Float32x4 particle factors"),
            }
        };

        assert!(
            (factor_alpha_of(&cont) - expected_alpha).abs() < 1e-6,
            "continuous body keeps authored opacity"
        );
        assert!(
            (factor_alpha_of(&spray) - expected_alpha).abs() < 1e-6,
            "a transient spray holds the authored alpha too — retail has no life fade"
        );
    }

    // The authored byte 50/255 is raw D3DCOLOR space: particle_draw carries it verbatim (no CPU
    // rescale — the fixed-function stages do the doubling). Pinned here is that the value holds
    // for the whole life — retail has no life-based fade.
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
        let expected = expected_factor_alpha(SHAFT_ALPHA);
        // Neutral wash lift: the assertion is on the authored byte holding for the whole life,
        // not on the slider seed that multiplies it.
        let clock = CelestialClock {
            wash_alpha_lift: 1.0,
            ..CelestialClock::default()
        };
        let mut g = celestial(def);
        g.particles[0].life_frames = f32::INFINITY;
        for age in [0.0, 300.0, 30_000.0] {
            g.particles[0].age_frames = age;
            let draw = particle_draw(&g, &g.particles[0], &clock);
            assert_eq!(draw.factor_alpha, expected);
        }
    }

    #[test]
    fn particle_expires_at_life() {
        let mut g = live(def(3.0, 1.0, 1), 1.0);
        advance(&mut g, 1.0); // emit the period's pair at age 0
        assert_eq!(g.particles.len(), 2);
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
            resolve_mesh(assets, None, NO_LOCAL_DIR, &sheet_def(), &mut images, false)
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
                resolve_mesh(&assets, None, NO_LOCAL_DIR, &def, &mut images, false)
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
            resolve_mesh(assets, None, local_dir, def, &mut images, false)
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
            resolve_mesh(assets, None, NO_LOCAL_DIR, &mesh_def(), &mut images, false)
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

        // i900 (120.DAT fefr/fefs) binds asi1, which exists only under syst/effe of 0.DAT — a
        // mesh absent from this tier must resolve through the global effect dir.
        #[test]
        fn static_mesh_resolves_through_the_global_tier() {
            let empty = ActionAssets::default();
            assert!(empty.d3m(NO_LOCAL_DIR, &MESH_ID).is_none());
            let global = mesh_assets(true, false, false);
            let mut images = Assets::<Image>::default();
            let Some((template, _, tex)) = resolve_mesh(
                &empty,
                Some(&global),
                NO_LOCAL_DIR,
                &mesh_def(),
                &mut images,
                false,
            ) else {
                panic!("the mesh must resolve through the global tier")
            };
            assert!(!template.positions.is_empty());
            assert!(tex.is_some());
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
            resolve_mesh(assets, None, NO_LOCAL_DIR, def, &mut images, false)
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

    // .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Reference
    // point: 54/55 are the hand locators, 56-60 the extra weapon locators, 52/53 fall back to
    // the root here, and every other index is itself.
    #[test]
    fn reference_aliases_remap_the_hand_weapon_and_footstep_indices() {
        use ffxi_dat::skel::standard_position::{LEFT_HAND, RIGHT_HAND};
        assert_eq!(resolve_reference_alias(54), LEFT_HAND);
        assert_eq!(resolve_reference_alias(55), RIGHT_HAND);
        for (alias, locator) in (56u8..=60).zip(102usize..=106) {
            assert_eq!(resolve_reference_alias(alias), locator, "alias {alias}");
        }
        for footstep in MOUNT_FOOTSTEP_JOINTS {
            assert_eq!(resolve_reference_alias(footstep), MOUNT_FOOTSTEP_REFERENCE);
        }
        for plain in [0u8, 1, 2, 21, 48, 49, 50, 51, 61, 127] {
            assert_eq!(
                resolve_reference_alias(plain),
                usize::from(plain),
                "{plain}"
            );
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
    // GLOBAL_EFFECT_DIR_FILE_ID): the melee hit sparks the `chit` chain reaches, target-side
    // modes carrying target reference 49
    // (.agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Other
    // attached effects).
    const HIT_SPARK_DIR: [u8; 4] = *b"hit1";
    const HIT_SPARK_SOURCE_REFERENCE: u8 = 0;
    const HIT_SPARK_GENERATORS: [([u8; 4], u8); 4] = [
        (*b"g010", ffxi_dat::particle_gen::attach_mode::TARGET),
        (*b"g011", ffxi_dat::particle_gen::attach_mode::TARGET),
        (
            *b"g012",
            ffxi_dat::particle_gen::attach_mode::TARGET_WITH_SOURCE_YAW,
        ),
        (*b"g013", ffxi_dat::particle_gen::attach_mode::TARGET),
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
    // research/xim ParticleGenerator.kt emit — framesUntilNextParticle starts at 0, so a
    // scheduled generator's first burst lands on its first tick. hit1's g01x stages all carry
    // zero duration: their flash IS that one immediate burst (s5c/s7 pin the landing).
    #[test]
    fn real_dat_hit_sparks_burst_on_their_first_tick() {
        let Some(defs) = retail_hit_spark_defs() else {
            return;
        };
        for (name, def) in &defs {
            let mut g = live(*def, 0.0);
            g.emit_accum = def.frames_per_emission;
            advance(&mut g, 1.0);
            assert!(
                !g.particles.is_empty(),
                "{} must burst on its first tick",
                String::from_utf8_lossy(name)
            );
        }
    }

    // Retail steps elements in FFXI space, where -Y is up (CYyGenerator.cpp ElemIdle case 0x02):
    // a world-space generator's DAT velocity integrates through WORLD_PARTICLE_VEL_BASIS, so a
    // +Y drift settles toward the ground.
    #[test]
    fn world_space_velocity_integrates_through_the_mzb_bevy_basis() {
        let Some(assets) = retail_global_effect_assets() else {
            return;
        };
        let mut d = def(1.0, 1.0, 1);
        d.init_velocity = [0.0, 0.5, 0.0];
        d.position_updater = true;
        let mut g = live_scheduled(d, 60.0, &assets);
        emit(&mut g, 60.0);
        advance(&mut g, 10.0);
        assert!(
            (g.particles[0].pos.y - (-5.0)).abs() < 1e-3,
            "the +Y DAT drift must integrate downward in Bevy space: {:?}",
            g.particles[0].pos
        );
    }

    // Pinned against the install: hit1's g010 authors a ±π rotation variance on all axes — each
    // streak of the 11-particle burst fans out at its own angle into retail's starburst. A screen
    // billboard must keep that per-particle spin about the view axis; without it every particle
    // of a burst lies along the same line.
    #[test]
    fn real_dat_hit_spark_streaks_fan_out_per_particle() {
        let Some(assets) = retail_global_effect_assets() else {
            return;
        };
        let def = assets
            .particle_def(HIT_SPARK_DIR, b"g010")
            .expect("hit1 defines g010");
        assert!(
            def.rotation_variance.is_some(),
            "the DAT authors a per-particle rotation variance"
        );
        let mut g = live_scheduled(*def, 60.0, &assets);
        // A horizontal streak: with identity camera the only thing that can turn it is the
        // particle's own spin about the view axis.
        g.template.positions = vec![
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
        ];
        emit(&mut g, def.max_life_frames);
        // Mid-life: the resolved scale tracks are non-zero here (init scale is (0,0)).
        advance(&mut g, def.max_life_frames * 0.5);
        let view = CameraView {
            rot: Quat::IDENTITY,
            pos: Vec3::ZERO,
        };
        let clock = ParticleSimulator::default().clock;

        g.particles[0].rotation.z = 0.0;
        let mut mesh_a = empty_mesh();
        rebuild_mesh(&g, view, &clock, &mut mesh_a);
        g.particles[0].rotation.z = std::f32::consts::FRAC_PI_4;
        let mut mesh_b = empty_mesh();
        rebuild_mesh(&g, view, &clock, &mut mesh_b);

        let a: Vec<[f32; 3]> = mesh_a
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|v| v.as_float3())
            .expect("positions")
            .to_vec();
        let b: Vec<[f32; 3]> = mesh_b
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|v| v.as_float3())
            .expect("positions")
            .to_vec();
        assert_ne!(
            a, b,
            "the per-particle spin must turn the streak about the view axis"
        );
    }

    // Pinned against the install: the crit burst's gs11 (sho1) rides the same eis1 streak sheet
    // as g010, but is fixed-orientation with no per-particle rotation — its 19 particles stack
    // into one streak that the scale velocity grows wide and thin over life.
    #[test]
    fn real_dat_crit_spark_streak_is_fixed_orientation() {
        let Some(assets) = retail_global_effect_assets() else {
            return;
        };
        // Directory-scoped: ROM/0/0.DAT defines these generator names in several directories.
        let def = assets
            .particle_def_scoped(*b"sho1", b"gs11")
            .expect("sho1 defines gs11")
            .1;
        assert_eq!(
            def.mesh_id, *b"eis1",
            "the crit streak rides the same sheet as g010"
        );
        assert!(
            def.rotation_variance.is_none(),
            "no per-particle rotation: the burst is one stacked streak"
        );
        let Some(vel) = def.scale_velocity else {
            panic!("the streak's growth is authored as a scale velocity");
        };
        assert!(
            vel[0] > 0.0 && vel[1] < 0.0,
            "it grows wide and thin: {vel:?}"
        );
    }

    // Pinned against the install: hit1's dust g012 authors a +Y base drift — settling from the
    // contact point toward the ground in retail's -Y-up frame (its spherical scatter rides on
    // top, so per-particle motion is not monotonic).
    #[test]
    fn real_dat_hit_dust_authors_a_downward_drift() {
        let Some(defs) = retail_hit_spark_defs() else {
            return;
        };
        let def = defs
            .iter()
            .find(|(name, _)| *name == *b"g012")
            .expect("hit1 defines g012")
            .1;
        assert!(def.init_velocity[1] > 0.0, "the DAT authors a +Y drift");
    }

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

    // Pinned against the install: every `hit1` spark generator is a target-side mode whose
    // target reference is the 49 selector, which is what puts the flash at the contact point
    // rather than the victim's root
    // (.agents/skills/retail-observe/references/2026-09-27-hit-effect-contact-point.md).
    #[test]
    fn real_dat_hit_sparks_carry_the_target_reference_selector() {
        let Some(defs) = retail_hit_spark_defs() else {
            return;
        };
        for ((name, def), (_, mode)) in defs.iter().zip(HIT_SPARK_GENERATORS) {
            let name = String::from_utf8_lossy(name).to_string();
            assert_eq!(def.attach_mode, mode, "{name}");
            assert_eq!(def.attach_eid, HIT_SPARK_SOURCE_REFERENCE, "{name}");
            assert_eq!(
                usize::from(def.attach_target_reference),
                *ffxi_actor::skeleton_instance::NEAREST_JOINT_REFERENCES.start(),
                "{name} attaches through the nearest-ring selector"
            );
            assert_eq!(def.base_position, [0.0; 3], "{name}");
        }
    }

    // The target reference 49 resolves to the ring entry nearest the attacker: the offset is a
    // ring locator at torso height on the struck side, at every victim facing and attacker
    // bearing (.agents/skills/retail-observe/references/2026-09-27-hit-effect-contact-point.md).
    #[test]
    fn real_dat_hit_spark_offset_is_the_contact_point() {
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

        for victim_facing in [0.0, 1.0, 2.5, -2.0] {
            let yaw = Quat::from_rotation_y(victim_facing);
            let root = Transform {
                translation: VICTIM_WORLD,
                rotation: yaw * crate::ffxi_actor_render::ffxi_to_bevy_basis(),
                scale: Vec3::ONE,
            }
            .compute_affine();
            let victim = AttachActor {
                world: VICTIM_WORLD,
                yaw,
                pose: Some(AttachPose {
                    pose: &pose,
                    skeleton: &skeleton,
                    root,
                    facing_dir: 0.0,
                    scale: 1.0,
                }),
            };
            for bearing in 0..8 {
                let a = bearing as f32 * std::f32::consts::TAU / 8.0;
                let toward = Vec3::new(a.cos(), 0.0, a.sin());
                let attacker = VICTIM_WORLD + toward * ATTACKER_REACH;
                for (name, def) in &defs {
                    let offset =
                        reference_point(&victim, def.attach_target_reference, Some(attacker), None)
                            - VICTIM_WORLD;
                    let name = String::from_utf8_lossy(name).to_string();
                    assert!(
                        offset.y.abs() > 0.5,
                        "{name} offset {offset:?} is not at torso height"
                    );
                    let horizontal = Vec3::new(offset.x, 0.0, offset.z);
                    if horizontal.length_squared() > 1e-6 {
                        assert!(
                            horizontal.normalize().dot(toward) > 0.5,
                            "{name} offset {offset:?} does not face the attacker at bearing {a:.2}"
                        );
                    }
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
                actor_rotation: None,
                animation_mode: None,
                turn_toward_step_degrees: None,
                local_dir: HIT_SPARK_DIR,
                model_transform: None,
                follow_points: None,
                screen_color: None,
                actor_fade: None,
                idle_transition_time: None,
                flinch_duration: None,
                model_visibility: None,
                spell_effect: None,
                sound_range: None,
                control_flow: None,
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
            .init_resource::<crate::distortion_pass::ActiveDistortion>()
            .add_message::<crate::scheduler_runtime::SchedulerStageEvent>()
            .add_message::<crate::scheduler_runtime::ParticleSpawnTrace>()
            .add_message::<crate::audio::SfxEvent>()
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
                identity: Default::default(),
                actor: attacker,
                target: None,
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
    /// `reference_point` alone: the actor root carrying the pose is a CHILD of the wire
    /// entity the stage fires on and PostUpdate has propagated nothing on the frame it is
    /// inserted, so the child descent, the local-transform composition and the attach frame's
    /// translation at the spawn site all have to hold for the spark to land at the
    /// contact point — the victim's ring locator nearest the attacker
    /// (.agents/skills/retail-observe/references/2026-09-27-hit-effect-contact-point.md).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_dat_hit_spark_spawns_at_the_contact_point() {
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

        const VICTIM_WORLD: Vec3 = Vec3::new(30.0, 2.0, -14.0);
        const ATTACKER_WORLD: Vec3 = Vec3::new(33.0, 2.0, -14.0);
        let toward_attacker = (ATTACKER_WORLD - VICTIM_WORLD).normalize();
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
            let offset = origin - VICTIM_WORLD;
            assert!(
                offset.y.abs() > 0.5,
                "{name} spawned at {origin:?}, not at torso height"
            );
            let horizontal = Vec3::new(offset.x, 0.0, offset.z);
            if horizontal.length_squared() > 1e-6 {
                assert!(
                    horizontal.normalize().dot(toward_attacker) > 0.5,
                    "{name} spawned at {origin:?}, not on the attacker's side"
                );
            }

            // A self-targeted def resolves against the same actor: the ring point nearest its own
            // origin, torso height above the root.
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
                (self_origin - VICTIM_WORLD).y.abs() > 0.5,
                "self-targeted {name} spawned at {self_origin:?}, not at torso height"
            );
        }
    }

    /// With no second actor the selector has nothing to measure against, and with no posed
    /// skeleton (a door, or a model still loading) there is no joint at all: both must fall
    /// back to the actor's feet rather than throwing the effect somewhere arbitrary.
    #[test]
    fn reference_point_without_a_pose_or_a_second_actor_falls_back_to_the_feet() {
        let Some(skeleton) = retail_hume_m_skeleton() else {
            return;
        };
        let pose = ffxi_actor::skeleton_instance::pose_world(
            &skeleton,
            |_| None,
            ffxi_actor::skeleton_instance::RootTransform::identity(),
            &[],
        );
        const FEET: Vec3 = Vec3::new(2.0, 1.0, -5.0);
        let selector = *ffxi_actor::skeleton_instance::NEAREST_JOINT_REFERENCES.start() as u8;
        let unposed = AttachActor {
            world: FEET,
            yaw: Quat::IDENTITY,
            pose: None,
        };
        assert_eq!(
            reference_point(&unposed, selector, Some(Vec3::X), None),
            FEET
        );
        let posed = AttachActor {
            world: FEET,
            yaw: Quat::IDENTITY,
            pose: Some(AttachPose {
                pose: &pose,
                skeleton: &skeleton,
                root: bevy::math::Affine3A::from_translation(FEET),
                facing_dir: 0.0,
                scale: 1.0,
            }),
        };
        assert_eq!(reference_point(&posed, selector, None, None), FEET);
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
            .init_resource::<crate::distortion_pass::ActiveDistortion>()
            .add_message::<SchedulerStageEvent>()
            .add_message::<crate::scheduler_runtime::ParticleSpawnTrace>()
            .add_message::<crate::audio::SfxEvent>()
            .add_systems(Update, spawn_particle_generators);
        let skeleton = retail_hume_m_skeleton().expect("installed HumeM skeleton is readable");
        let pose = ffxi_actor::skeleton_instance::pose_world(
            &skeleton,
            |_| None,
            ffxi_actor::skeleton_instance::RootTransform::identity(),
            &[],
        );
        let turn = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        assert_eq!(usize::from(source.attach_eid), HUME_M_WAIST_REFERENCE);
        let expected_origin = placed(
            ACTOR_POSITION,
            turn,
            skeleton_reference_point(&skeleton, &pose, HUME_M_WAIST_REFERENCE),
        );
        let actor = spawn_posed_actor(&mut app, &skeleton, &pose, ACTOR_POSITION);
        app.world_mut().entity_mut(actor).insert((
            Transform::from_translation(ACTOR_POSITION).with_rotation(turn),
            assets,
            crate::scheduler_runtime::ActionTarget(Some(actor)),
        ));
        app.world_mut().write_message(SchedulerStageEvent {
            identity: Default::default(),
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
                .distance(expected_origin + turn * parent.particles[0].pos)
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
            .init_resource::<crate::distortion_pass::ActiveDistortion>()
            .add_message::<SchedulerStageEvent>()
            .add_message::<crate::scheduler_runtime::ParticleSpawnTrace>()
            .add_message::<crate::audio::SfxEvent>()
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
        assert_eq!(usize::from(def.attach_eid), HUME_M_WAIST_REFERENCE);
        let expected_origin = placed(
            ACTOR_WORLD,
            Quat::IDENTITY,
            skeleton_reference_point(&skeleton, &pose, HUME_M_WAIST_REFERENCE),
        );
        let actor = spawn_posed_actor(&mut app, &skeleton, &pose, ACTOR_WORLD);
        app.world_mut().entity_mut(actor).insert(assets);
        app.world_mut().write_message(SchedulerStageEvent {
            identity: Default::default(),
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

    #[cfg(not(target_arch = "wasm32"))]
    const SYNTHETIC_GENERATOR: [u8; 4] = *b"gsyn";
    #[cfg(not(target_arch = "wasm32"))]
    const SYNTHETIC_MESH: [u8; 4] = *b"msyn";
    // A two-joint stand-in for a playable skeleton, in FFXI axes (Y down): reference 1 is the
    // waist joint 1.05 above the feet and 0.1 to +x, reference 2 the nameplate 2.0 up, 13-20 a
    // ring at 1.1 up, 21 a chest point 1.24 up, 49-51 the unplaced selectors; the one bounding
    // box spans 3.4 x 2.85 so the fit scales are 2.0 wide and 1.5 tall.
    const SYNTHETIC_WAIST_JOINT: Vec3 = Vec3::new(0.1, -1.05, 0.0);
    const SYNTHETIC_NAMEPLATE_OFFSET: Vec3 = Vec3::new(0.0, -2.0, 0.0);
    const SYNTHETIC_CHEST_OFFSET: Vec3 = Vec3::new(0.0, -1.24, 0.0);
    const SYNTHETIC_CHEST_REFERENCE: usize = 21;
    const SYNTHETIC_LEFT_HAND_OFFSET: Vec3 = Vec3::new(0.4, -0.9, 0.0);
    const SYNTHETIC_LEFT_HAND_ALIAS: u8 = 54;
    const SYNTHETIC_RING_HEIGHT: f32 = -1.1;
    const SYNTHETIC_RING_RADIUS: f32 = 0.3;
    const SYNTHETIC_BOX_WIDTH: f32 = 3.4;
    const SYNTHETIC_BOX_HEIGHT: f32 = 2.85;
    const SYNTHETIC_REFERENCE_TABLE_LEN: usize = 128;

    fn synthetic_skeleton() -> ffxi_dat::skel::Skeleton {
        use ffxi_dat::skel::{BoundingBox, Joint, JointReference, Skeleton};
        let identity = [0.0, 0.0, 0.0, 1.0];
        let joints = vec![
            Joint {
                rotation: identity,
                translation: [0.0; 3],
                parent: None,
            },
            Joint {
                rotation: identity,
                translation: SYNTHETIC_WAIST_JOINT.to_array(),
                parent: Some(0),
            },
        ];
        let at_root = |offset: Vec3| JointReference {
            index: 0,
            rotation: [0.0; 3],
            position_offset: offset.to_array(),
        };
        let mut references: Vec<JointReference> = (0..SYNTHETIC_REFERENCE_TABLE_LEN)
            .map(|_| at_root(Vec3::ZERO))
            .collect();
        references[1] = JointReference {
            index: 1,
            rotation: [0.0; 3],
            position_offset: [0.0; 3],
        };
        references[ffxi_dat::skel::standard_position::ABOVE_HEAD] =
            at_root(SYNTHETIC_NAMEPLATE_OFFSET);
        references[SYNTHETIC_CHEST_REFERENCE] = at_root(SYNTHETIC_CHEST_OFFSET);
        references[ffxi_dat::skel::standard_position::LEFT_HAND] =
            at_root(SYNTHETIC_LEFT_HAND_OFFSET);
        let ring = ffxi_actor::skeleton_instance::RING_JOINT_REFERENCES;
        let ring_len = ring.clone().count() as f32;
        for (k, reference) in ring.enumerate() {
            let a = k as f32 * std::f32::consts::TAU / ring_len;
            references[reference] = at_root(Vec3::new(
                a.cos() * SYNTHETIC_RING_RADIUS,
                SYNTHETIC_RING_HEIGHT,
                a.sin() * SYNTHETIC_RING_RADIUS,
            ));
        }
        Skeleton {
            id: ffxi_dat::datid::DatId::from_str("synt"),
            joints,
            references,
            look_at_limits: Vec::new(),
            bounding_boxes: vec![BoundingBox {
                y_max: 0.0,
                y_min: -SYNTHETIC_BOX_HEIGHT,
                x_max: SYNTHETIC_BOX_WIDTH / 2.0,
                x_min: -SYNTHETIC_BOX_WIDTH / 2.0,
                z_max: 0.5,
                z_min: -0.5,
            }],
        }
    }

    fn synthetic_pose(skeleton: &ffxi_dat::skel::Skeleton) -> Vec<Mat4> {
        ffxi_actor::skeleton_instance::pose_world(
            skeleton,
            |_| None,
            ffxi_actor::skeleton_instance::RootTransform::identity(),
            &[],
        )
    }

    // Where the record's reference point lands in Bevy space for an actor standing at `world`
    // turned by `yaw`: the FFXI-frame offset through the FFXI->Bevy basis, then the yaw.
    fn placed(world: Vec3, yaw: Quat, offset_ffxi: Vec3) -> Vec3 {
        world + yaw * (crate::ffxi_actor_render::ffxi_to_bevy_basis() * offset_ffxi)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn synthetic_assets(
        def: ParticleGeneratorDef,
        id: [u8; 4],
    ) -> crate::scheduler_runtime::ActionAssets {
        let mut assets = crate::scheduler_runtime::ActionAssets::default();
        assets.d3ms.insert(
            def.mesh_id,
            ffxi_dat::d3m::D3m {
                name: def.mesh_id,
                num_triangles: 1,
                texture_name: [0; 16],
                vertices: [Vec3::NEG_X, Vec3::X, Vec3::Y]
                    .into_iter()
                    .map(|pos| ffxi_dat::d3m::D3mVertex {
                        pos: pos.to_array(),
                        normal: Vec3::Z.to_array(),
                        color: Vec4::ONE.to_array(),
                        uv: Vec2::ZERO.to_array(),
                    })
                    .collect(),
            },
        );
        assets.particle_defs.insert(id, def);
        assets
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn synthetic_attached_def() -> ParticleGeneratorDef {
        let mut d = def(ROUTINE_FPS, ROUTINE_FPS, 0);
        d.mesh_id = SYNTHETIC_MESH;
        d.base_position = [0.0; 3];
        d.init_velocity = [0.0; 3];
        d
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn particle_app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<Image>()
            .init_asset::<FfxiParticleMaterial>()
            .init_resource::<ParticleSimulator>()
            .init_resource::<crate::distortion_pass::ActiveDistortion>()
            .add_message::<SchedulerStageEvent>()
            .add_message::<crate::scheduler_runtime::ParticleSpawnTrace>()
            .add_message::<crate::audio::SfxEvent>()
            .add_systems(Update, spawn_particle_generators);
        app
    }

    // A posed synthetic actor whose wire entity stands at `world` facing `yaw`, the way the live
    // path parents the actor root (basis only) under the heading-carrying wire entity.
    #[cfg(not(target_arch = "wasm32"))]
    fn spawn_turned_actor(
        app: &mut App,
        skeleton: &ffxi_dat::skel::Skeleton,
        pose: &[Mat4],
        world: Vec3,
        yaw: Quat,
    ) -> Entity {
        let wire = spawn_posed_actor(app, skeleton, pose, world);
        app.world_mut()
            .entity_mut(wire)
            .insert(Transform::from_translation(world).with_rotation(yaw));
        wire
    }

    // Fires one particle stage of `def` from `caster` at `target` and returns the simulator
    // after the frame that spawns it.
    #[cfg(not(target_arch = "wasm32"))]
    fn fire_synthetic_stage(
        app: &mut App,
        def: ParticleGeneratorDef,
        caster: Entity,
        target: Entity,
    ) {
        app.world_mut().entity_mut(caster).insert((
            synthetic_assets(def, SYNTHETIC_GENERATOR),
            crate::scheduler_runtime::ActionTarget(Some(target)),
        ));
        app.world_mut().write_message(SchedulerStageEvent {
            identity: Default::default(),
            actor: caster,
            target: Some(target),
            stage: particle_stage(SYNTHETIC_GENERATOR),
            scheduler: SYNTHETIC_GENERATOR,
        });
        app.update();
    }

    const FRAME_TOLERANCE: f32 = 1e-4;

    // .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Element
    // placement and billboard: the setup position is a DAT-frame offset in the attach frame, so
    // a negative DAT y rises above the reference point and the whole offset turns with the
    // caster's yaw.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn authored_setup_position_lands_in_the_casters_attach_frame() {
        const SETUP_POSITION: Vec3 = Vec3::new(0.5, -0.3, 0.0);
        const CASTER_WORLD: Vec3 = Vec3::new(4.0, 1.0, -2.0);
        let yaw = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let skeleton = synthetic_skeleton();
        let pose = synthetic_pose(&skeleton);
        let mut d = synthetic_attached_def();
        d.attach_mode = ffxi_dat::particle_gen::attach_mode::SOURCE;
        d.attach_type = ffxi_dat::particle_gen::AttachType::SourceActor;
        d.attach_eid = 1;
        d.base_position = SETUP_POSITION.to_array();
        let mut app = particle_app();
        let caster = spawn_turned_actor(&mut app, &skeleton, &pose, CASTER_WORLD, yaw);
        fire_synthetic_stage(&mut app, d, caster, caster);
        let mut sim = app.world_mut().resource_mut::<ParticleSimulator>();
        let clock = sim.clock;
        let g = &mut sim.generators[0];
        advance_generator(g, 1.0);
        let element = particle_draw(g, &g.particles[0], &clock).world;
        let waist = placed(CASTER_WORLD, yaw, SYNTHETIC_WAIST_JOINT);
        let expected = placed(waist, yaw, SETUP_POSITION);
        assert!(
            element.distance(expected) < FRAME_TOLERANCE,
            "element {element:?} must sit at {expected:?}: 0.3 above the waist and 0.5 along the caster's x"
        );
    }

    // .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Attach
    // frame: a target-side mode resolves the TARGET reference (bits 10-15) on the target actor,
    // a plain index attaching to that reference rather than to the nearest-ring selector.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn target_side_mode_attaches_to_the_dats_target_reference() {
        const CASTER_WORLD: Vec3 = Vec3::new(0.0, 0.0, 0.0);
        const TARGET_WORLD: Vec3 = Vec3::new(5.0, 0.0, 3.0);
        let target_yaw = Quat::from_rotation_y(-1.2);
        let skeleton = synthetic_skeleton();
        let pose = synthetic_pose(&skeleton);
        let mut d = synthetic_attached_def();
        d.attach_mode = ffxi_dat::particle_gen::attach_mode::TARGET;
        d.attach_type = ffxi_dat::particle_gen::AttachType::TargetActor;
        d.attach_eid = 0;
        d.attach_target_reference = SYNTHETIC_CHEST_REFERENCE as u8;
        let mut app = particle_app();
        let caster = spawn_turned_actor(&mut app, &skeleton, &pose, CASTER_WORLD, Quat::IDENTITY);
        let target = spawn_turned_actor(&mut app, &skeleton, &pose, TARGET_WORLD, target_yaw);
        fire_synthetic_stage(&mut app, d, caster, target);
        let sim = app.world().resource::<ParticleSimulator>();
        let expected = placed(TARGET_WORLD, target_yaw, SYNTHETIC_CHEST_OFFSET);
        assert!(
            sim.generators[0].origin.distance(expected) < FRAME_TOLERANCE,
            "origin {:?} must be the target's reference 21 at {expected:?}",
            sim.generators[0].origin
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn level_up_dat() -> Option<Vec<u8>> {
        let root = ffxi_dat::archive::open_test_install()?;
        let loc = root
            .resolve(crate::scheduler_runtime::LEVEL_UP_EFFECT_DAT_ID)
            .ok()?;
        std::fs::read(loc.path_under(&root)).ok()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn level_up_stage(
        schedulers: &[ffxi_dat::scheduler::Scheduler],
        id: [u8; 4],
    ) -> ffxi_dat::scheduler::TimedStage {
        schedulers
            .iter()
            .flat_map(|s| &s.stages)
            .find(|s| s.stage.kind == StageKind::Particle && s.stage.id == id)
            .copied()
            .unwrap_or_else(|| {
                panic!(
                    "the level-up DAT schedules {}",
                    String::from_utf8_lossy(&id)
                )
            })
    }

    // .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Reference
    // point: for the playable skeletons reference 1 is the waist joint, Hume M's joint 25 at
    // 1.05 above the feet in bind pose.
    const HUME_M_WAIST_REFERENCE: usize = 1;
    const HUME_M_WAIST_JOINT: usize = 25;
    const HUME_M_WAIST_HEIGHT: f32 = 1.05;

    // A reference point from the skeleton alone: the posed joint the table entry names, read in
    // FFXI axes where height above the feet is -y.
    fn skeleton_reference_point(
        skeleton: &ffxi_dat::skel::Skeleton,
        pose: &[Mat4],
        reference: usize,
    ) -> Vec3 {
        let entry = &skeleton.references[reference];
        pose[entry.index].transform_point3(Vec3::from_array(entry.position_offset))
    }

    // .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md The
    // level-up case: the lettering's origin is the caster's posed reference 1 (Hume M joint 25,
    // 1.05 above the feet) and the sparkle generator's setup position puts its elements 0.3
    // above that point, with no height, nameplate or race offset.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_dat_level_up_places_the_lettering_at_the_waist_and_the_sparkles_above_it() {
        use crate::scheduler_runtime::parse_action_bytes;
        const LETTERING: [u8; 4] = *b"g000";
        const SPARKLE: [u8; 4] = *b"g001";
        const SPARKLE_RAISE: f32 = 0.3;
        const ACTOR_WORLD: Vec3 = Vec3::new(7.0, 2.0, -3.0);
        let Some(bytes) = level_up_dat() else {
            eprintln!("SKIP: level-up DAT test needs a registered install");
            return;
        };
        let (schedulers, mut assets, _) = parse_action_bytes(&bytes);
        let lettering_stage = level_up_stage(&schedulers, LETTERING);
        let sparkle_stage = level_up_stage(&schedulers, SPARKLE);
        let (sparkle_dir, _) = assets
            .particle_def_scoped(sparkle_stage.stage.local_dir, &SPARKLE)
            .unwrap();
        {
            let sparkle = assets
                .particle_defs_by_dir
                .get_mut(&(sparkle_dir, SPARKLE))
                .unwrap();
            assert!(
                (sparkle.base_position[1] + SPARKLE_RAISE).abs() < FRAME_TOLERANCE,
                "the DAT authors the sparkle setup position 0.3 up: {:?}",
                sparkle.base_position
            );
            // The authored spread scatters each element; only the setup position is under test.
            sparkle.spherical_full = None;
            sparkle.position_variance = None;
        }
        let skeleton = retail_hume_m_skeleton().expect("installed HumeM skeleton is readable");
        let pose = synthetic_pose(&skeleton);
        assert_eq!(
            skeleton.references[HUME_M_WAIST_REFERENCE].index,
            HUME_M_WAIST_JOINT
        );
        let waist_ffxi = skeleton_reference_point(&skeleton, &pose, HUME_M_WAIST_REFERENCE);
        assert!(
            (-waist_ffxi.y - HUME_M_WAIST_HEIGHT).abs() < 0.02,
            "Hume M reference 1 sits 1.05 above the feet in bind pose: {waist_ffxi:?}"
        );
        let waist = placed(ACTOR_WORLD, Quat::IDENTITY, waist_ffxi);

        let mut app = particle_app();
        let actor = spawn_posed_actor(&mut app, &skeleton, &pose, ACTOR_WORLD);
        app.world_mut().entity_mut(actor).insert(assets);
        for stage in [lettering_stage, sparkle_stage] {
            app.world_mut().write_message(SchedulerStageEvent {
                identity: Default::default(),
                actor,
                target: None,
                stage,
                scheduler: *b"main",
            });
        }
        app.update();
        let mut sim = app.world_mut().resource_mut::<ParticleSimulator>();
        let clock = sim.clock;
        let lettering = sim
            .generators
            .iter()
            .find(|g| g.origin_routine.is_some_and(|r| r.gen_id == LETTERING))
            .expect("the lettering generator spawned");
        assert!(
            lettering.origin.distance(waist) < FRAME_TOLERANCE,
            "lettering origin {:?} must be the posed waist {waist:?}",
            lettering.origin
        );
        let sparkle = sim
            .generators
            .iter_mut()
            .find(|g| g.origin_routine.is_some_and(|r| r.gen_id == SPARKLE))
            .expect("the sparkle generator spawned");
        advance_generator(sparkle, 1.0);
        let element = particle_draw(sparkle, &sparkle.particles[0], &clock).world;
        let raise = element - waist;
        assert!(
            raise.distance(Vec3::Y * SPARKLE_RAISE) < FRAME_TOLERANCE,
            "sparkle element {element:?} sits {raise:?} from the waist; the record puts it 0.3 above"
        );
    }

    // .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md The
    // level-up case: velocity 0.031 up per update damped by 0.924 per update rises about 0.4,
    // most of it within 40 updates.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_dat_level_up_lettering_rises_to_the_record_height_after_40_updates() {
        use crate::scheduler_runtime::parse_action_bytes;
        const LETTERING: [u8; 4] = *b"g000";
        const UPDATES: usize = 40;
        const RISE_AFTER_UPDATES: f32 = 0.4;
        const RISE_TOLERANCE: f32 = 0.02;
        let Some(bytes) = level_up_dat() else {
            eprintln!("SKIP: level-up DAT test needs a registered install");
            return;
        };
        let (schedulers, assets, _) = parse_action_bytes(&bytes);
        let stage = level_up_stage(&schedulers, LETTERING);
        let def = *assets
            .particle_def(stage.stage.local_dir, &LETTERING)
            .unwrap();
        let mut g = live_scheduled(def, 0.0, &assets);
        g.emit_accum = def.frames_per_emission;
        advance_generator(&mut g, 1.0);
        assert_eq!(g.particles.len(), 1);
        let clock = CelestialClock::default();
        let spawn = particle_draw(&g, &g.particles[0], &clock).world;
        for _ in 0..UPDATES {
            advance_generator(&mut g, 1.0);
        }
        let rise = particle_draw(&g, &g.particles[0], &clock).world - spawn;
        assert!(
            (rise.y - RISE_AFTER_UPDATES).abs() < RISE_TOLERANCE && rise.x == 0.0 && rise.z == 0.0,
            "the lettering rose {rise:?} after {UPDATES} updates; the record says about 0.4 straight up"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct TwoActors {
        app: App,
        caster: Entity,
        target: Entity,
    }

    #[cfg(not(target_arch = "wasm32"))]
    const CASTER_WORLD: Vec3 = Vec3::new(4.0, 1.0, -2.0);
    #[cfg(not(target_arch = "wasm32"))]
    const TARGET_WORLD: Vec3 = Vec3::new(9.0, 1.0, 1.0);
    #[cfg(not(target_arch = "wasm32"))]
    const CASTER_FACING: f32 = 0.7;
    #[cfg(not(target_arch = "wasm32"))]
    const TARGET_FACING: f32 = -1.2;

    #[cfg(not(target_arch = "wasm32"))]
    fn two_posed_actors(skeleton: &ffxi_dat::skel::Skeleton) -> TwoActors {
        let pose = synthetic_pose(skeleton);
        let mut app = particle_app();
        let caster = spawn_turned_actor(
            &mut app,
            skeleton,
            &pose,
            CASTER_WORLD,
            Quat::from_rotation_y(CASTER_FACING),
        );
        let target = spawn_turned_actor(
            &mut app,
            skeleton,
            &pose,
            TARGET_WORLD,
            Quat::from_rotation_y(TARGET_FACING),
        );
        TwoActors {
            app,
            caster,
            target,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn spawned_frame(app: &App) -> (Vec3, Quat, Vec3, Vec3) {
        let g = &app.world().resource::<ParticleSimulator>().generators[0];
        (g.origin, g.frame_rot, g.position_fit, g.model_fit)
    }

    fn same_rotation(a: Quat, b: Quat) -> bool {
        a.angle_between(b) < FRAME_TOLERANCE
    }

    // .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Attach
    // frame, modes 1/2/4/5: the translation is the stated actor's reference point and the
    // rotation the stated actor's yaw, with 54 reaching the hand locator through its alias.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn single_point_modes_take_the_stated_actors_point_and_yaw() {
        use ffxi_dat::particle_gen::attach_mode as mode;
        let skeleton = synthetic_skeleton();
        let caster_yaw = Quat::from_rotation_y(CASTER_FACING);
        let target_yaw = Quat::from_rotation_y(TARGET_FACING);
        let caster_waist = placed(CASTER_WORLD, caster_yaw, SYNTHETIC_WAIST_JOINT);
        let caster_hand = placed(CASTER_WORLD, caster_yaw, SYNTHETIC_LEFT_HAND_OFFSET);
        let target_chest = placed(TARGET_WORLD, target_yaw, SYNTHETIC_CHEST_OFFSET);
        let cases = [
            (mode::SOURCE, 1, caster_waist, caster_yaw),
            (
                mode::SOURCE,
                SYNTHETIC_LEFT_HAND_ALIAS,
                caster_hand,
                caster_yaw,
            ),
            (mode::SOURCE_WITH_TARGET_YAW, 1, caster_waist, target_yaw),
            (mode::TARGET, 1, target_chest, target_yaw),
            (mode::TARGET_WITH_SOURCE_YAW, 1, target_chest, caster_yaw),
        ];
        for (attach_mode, source_reference, point, yaw) in cases {
            let mut d = synthetic_attached_def();
            d.attach_mode = attach_mode;
            d.attach_type =
                ffxi_dat::particle_gen::AttachType::from_flag(u16::from(attach_mode)).unwrap();
            d.attach_eid = source_reference;
            d.attach_target_reference = SYNTHETIC_CHEST_REFERENCE as u8;
            let mut scene = two_posed_actors(&skeleton);
            fire_synthetic_stage(&mut scene.app, d, scene.caster, scene.target);
            let (origin, rotation, position_fit, model_fit) = spawned_frame(&scene.app);
            assert!(
                origin.distance(point) < FRAME_TOLERANCE,
                "mode {attach_mode} ref {source_reference}: origin {origin:?}, expected {point:?}"
            );
            assert!(
                same_rotation(rotation, yaw),
                "mode {attach_mode}: rotation {rotation:?}, expected {yaw:?}"
            );
            assert_eq!((position_fit, model_fit), (Vec3::ONE, Vec3::ONE));
        }
    }

    // .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Attach
    // frame, modes 3/6: X runs from the start point to the end point with the span length as
    // the X position fit, so an element authored at x = 1 lands on the far point; the frame
    // keeps no roll, so up stays up along a level span.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn two_point_modes_span_the_caster_and_target_points() {
        use ffxi_dat::particle_gen::attach_mode as mode;
        let skeleton = synthetic_skeleton();
        let caster_chest = placed(
            CASTER_WORLD,
            Quat::from_rotation_y(CASTER_FACING),
            SYNTHETIC_CHEST_OFFSET,
        );
        let target_chest = placed(
            TARGET_WORLD,
            Quat::from_rotation_y(TARGET_FACING),
            SYNTHETIC_CHEST_OFFSET,
        );
        for (attach_mode, start, end) in [
            (mode::SOURCE_TO_TARGET, caster_chest, target_chest),
            (mode::TARGET_TO_SOURCE, target_chest, caster_chest),
        ] {
            let mut d = synthetic_attached_def();
            d.attach_mode = attach_mode;
            d.attach_type =
                ffxi_dat::particle_gen::AttachType::from_flag(u16::from(attach_mode)).unwrap();
            d.attach_eid = SYNTHETIC_CHEST_REFERENCE as u8;
            d.attach_target_reference = SYNTHETIC_CHEST_REFERENCE as u8;
            d.base_position = Vec3::X.to_array();
            let mut scene = two_posed_actors(&skeleton);
            fire_synthetic_stage(&mut scene.app, d, scene.caster, scene.target);
            let (origin, rotation, position_fit, _) = spawned_frame(&scene.app);
            assert!(
                origin.distance(start) < FRAME_TOLERANCE,
                "mode {attach_mode}"
            );
            assert!(
                (position_fit.x - start.distance(end)).abs() < FRAME_TOLERANCE,
                "mode {attach_mode}: the span length replaces the X fit: {position_fit:?}"
            );
            assert!((rotation * Vec3::Y).distance(Vec3::Y) < FRAME_TOLERANCE);
            assert!(
                (rotation * Vec3::X).distance((end - start).normalize()) < FRAME_TOLERANCE,
                "mode {attach_mode}: X must run from {start:?} to {end:?}"
            );
            let mut sim = scene.app.world_mut().resource_mut::<ParticleSimulator>();
            let clock = sim.clock;
            let g = &mut sim.generators[0];
            advance_generator(g, 1.0);
            let element = particle_draw(g, &g.particles[0], &clock).world;
            assert!(
                element.distance(end) < FRAME_TOLERANCE,
                "mode {attach_mode}: element {element:?} must sit on the far point {end:?}"
            );
        }
    }

    // .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Scale:
    // the nibble selects the caster (1-4) or the target (5-8) and the axes its low two bits
    // name; the scales come from the skeleton's first bounding box times the actor scale.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn fit_nibbles_scale_positions_and_sprites_by_the_selected_actors_box() {
        use ffxi_dat::particle_gen::attach_mode as mode;
        const TARGET_SCALE: f32 = 2.0;
        const SETUP_POSITION: Vec3 = Vec3::new(1.0, -1.0, 1.0);
        let skeleton = synthetic_skeleton();
        let caster_width = SYNTHETIC_BOX_WIDTH / FIT_WIDTH_DIVISOR;
        let caster_height = SYNTHETIC_BOX_HEIGHT / FIT_HEIGHT_DIVISOR;
        let target_width = caster_width * TARGET_SCALE;
        let target_height = caster_height * TARGET_SCALE;
        let cases = [
            (1u8, Vec3::new(caster_width, 1.0, caster_width)),
            (2, Vec3::new(1.0, caster_height, 1.0)),
            (3, Vec3::new(caster_width, caster_height, caster_width)),
            (4, Vec3::splat(caster_width.max(caster_height))),
            (5, Vec3::new(target_width, 1.0, target_width)),
            (6, Vec3::new(1.0, target_height, 1.0)),
            (8, Vec3::splat(target_width.max(target_height))),
        ];
        let caster_yaw = Quat::from_rotation_y(CASTER_FACING);
        let waist = placed(CASTER_WORLD, caster_yaw, SYNTHETIC_WAIST_JOINT);
        for (nibble, fit) in cases {
            let mut d = synthetic_attached_def();
            d.attach_mode = mode::SOURCE;
            d.attach_eid = 1;
            d.attach_position_fit = nibble;
            d.attach_model_fit = nibble;
            d.base_position = SETUP_POSITION.to_array();
            let mut scene = two_posed_actors(&skeleton);
            let holder = scene.app.world().get::<Children>(scene.target).unwrap()[0];
            scene
                .app
                .world_mut()
                .get_mut::<FfxiRenderActor>(holder)
                .unwrap()
                .scale = TARGET_SCALE;
            fire_synthetic_stage(&mut scene.app, d, scene.caster, scene.target);
            let (_, _, position_fit, model_fit) = spawned_frame(&scene.app);
            assert!(
                position_fit.distance(fit) < FRAME_TOLERANCE
                    && model_fit.distance(fit) < FRAME_TOLERANCE,
                "nibble {nibble}: fits {position_fit:?} / {model_fit:?}, expected {fit:?}"
            );
            let mut sim = scene.app.world_mut().resource_mut::<ParticleSimulator>();
            let clock = sim.clock;
            let g = &mut sim.generators[0];
            advance_generator(g, 1.0);
            let element = particle_draw(g, &g.particles[0], &clock).world;
            let expected = waist
                + caster_yaw
                    * (crate::ffxi_actor_render::ffxi_to_bevy_basis() * SETUP_POSITION * fit);
            assert!(
                element.distance(expected) < FRAME_TOLERANCE,
                "nibble {nibble}: element {element:?}, expected {expected:?}"
            );
        }
    }

    // A screen billboard's sprite grows by the model fit along its own axes.
    #[test]
    fn model_fit_scales_the_sprite_quad() {
        let mut g = live(def(ROUTINE_FPS, ROUTINE_FPS, 0), 0.0);
        g.vel_basis = WORLD_PARTICLE_VEL_BASIS;
        g.model_fit = Vec3::new(4.0, 1.0, 4.0);
        g.template.positions = vec![Vec3::NEG_X, Vec3::X, Vec3::Y];
        emit(&mut g, ROUTINE_FPS);
        g.particles[0].pos = Vec3::ZERO;
        let (positions, _) = rebuilt(&g, view(Quat::IDENTITY));
        let sx = g.def.init_scale[0] * g.model_fit.x;
        let sy = g.def.init_scale[1] * g.model_fit.y;
        let expected = [
            Vec3::new(-sx, 0.0, 0.0),
            Vec3::new(sx, 0.0, 0.0),
            Vec3::new(0.0, -sy, 0.0),
        ];
        for (vertex, expected) in positions.iter().zip(expected) {
            assert!(
                vertex.distance(expected) < FRAME_TOLERANCE,
                "{positions:?} vs {expected:?}"
            );
        }
    }

    // .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Scale:
    // the installed Hume M skeleton gives width 0.82 and height 1.00 at scale 1.
    #[test]
    fn real_dat_hume_m_fit_scales_match_the_record() {
        const HUME_M_WIDTH_SCALE: f32 = 0.82;
        const HUME_M_HEIGHT_SCALE: f32 = 1.00;
        const SCALE_TOLERANCE: f32 = 0.01;
        let Some(skeleton) = retail_hume_m_skeleton() else {
            return;
        };
        let pose = synthetic_pose(&skeleton);
        let actor = AttachActor {
            world: Vec3::ZERO,
            yaw: Quat::IDENTITY,
            pose: Some(AttachPose {
                pose: &pose,
                skeleton: &skeleton,
                root: bevy::math::Affine3A::IDENTITY,
                facing_dir: 0.0,
                scale: 1.0,
            }),
        };
        let FitScales { width, height } = actor_fit_scales(&actor);
        assert!(
            (width - HUME_M_WIDTH_SCALE).abs() < SCALE_TOLERANCE
                && (height - HUME_M_HEIGHT_SCALE).abs() < SCALE_TOLERANCE,
            "Hume M fit scales {width:.3} x {height:.3}"
        );
    }

    // .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md
    // Per-update follow: a translation-only snap moves the origin to the rebuilt reference
    // point and leaves the rotation at the activation yaw; the facing bit turns it too.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn follow_snaps_the_origin_and_turns_only_with_the_facing_bit() {
        use bevy::ecs::system::RunSystemOnce;
        use ffxi_dat::particle_gen::attach_mode as mode;
        const MOVED_WORLD: Vec3 = Vec3::new(-3.0, 1.0, 6.0);
        const MOVED_FACING: f32 = 2.4;
        let skeleton = synthetic_skeleton();
        let activation_yaw = Quat::from_rotation_y(CASTER_FACING);
        let moved_yaw = Quat::from_rotation_y(MOVED_FACING);
        for follow_facing in [false, true] {
            let mut d = synthetic_attached_def();
            d.attach_mode = mode::SOURCE;
            d.attach_eid = 1;
            d.association = Some(ffxi_dat::particle_gen::AssociationFollow {
                follow_position: true,
                follow_facing,
                factor: ffxi_dat::particle_gen::ASSOCIATION_FOLLOW_RATE_SNAP,
            });
            let mut scene = two_posed_actors(&skeleton);
            scene.app.world_mut().insert_resource(Time::<()>::default());
            fire_synthetic_stage(&mut scene.app, d, scene.caster, scene.target);
            scene
                .app
                .world_mut()
                .entity_mut(scene.caster)
                .insert(Transform::from_translation(MOVED_WORLD).with_rotation(moved_yaw));
            scene
                .app
                .world_mut()
                .run_system_once(track_attached_origins)
                .unwrap();
            let (origin, rotation, _, _) = spawned_frame(&scene.app);
            let expected = placed(MOVED_WORLD, moved_yaw, SYNTHETIC_WAIST_JOINT);
            assert!(
                origin.distance(expected) < FRAME_TOLERANCE,
                "facing {follow_facing}: origin {origin:?} must snap to {expected:?}"
            );
            let expected_yaw = if follow_facing {
                moved_yaw
            } else {
                activation_yaw
            };
            assert!(
                same_rotation(rotation, expected_yaw),
                "facing {follow_facing}: rotation {rotation:?}, expected {expected_yaw:?}"
            );
        }
    }

    #[test]
    fn follow_blend_snaps_at_full_rate_and_compounds_a_partial_rate_per_update() {
        const HALF_RATE: f32 = 0.5;
        assert_eq!(follow_blend(1.0, 0.0), 1.0);
        assert_eq!(follow_blend(1.0, 0.3), 1.0);
        assert_eq!(follow_blend(HALF_RATE, 0.0), 0.0);
        assert!((follow_blend(HALF_RATE, 1.0) - HALF_RATE).abs() < FRAME_TOLERANCE);
        assert!((follow_blend(HALF_RATE, 2.0) - 0.75).abs() < FRAME_TOLERANCE);
    }

    // .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Reference
    // point: 51 is the ring entry nearest the camera eye; without a camera it falls back to the
    // entry nearest the other actor.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn camera_nearest_reference_picks_the_ring_entry_toward_the_eye() {
        use ffxi_dat::particle_gen::attach_mode as mode;
        const ACTOR_WORLD: Vec3 = Vec3::new(0.0, 0.0, 0.0);
        const OTHER_WORLD: Vec3 = Vec3::new(0.0, 0.0, 10.0);
        const EYE: Vec3 = Vec3::new(10.0, 1.0, 0.0);
        let skeleton = synthetic_skeleton();
        let pose = synthetic_pose(&skeleton);
        let ring_toward_eye = placed(
            ACTOR_WORLD,
            Quat::IDENTITY,
            Vec3::new(SYNTHETIC_RING_RADIUS, SYNTHETIC_RING_HEIGHT, 0.0),
        );
        let ring_toward_other = placed(
            ACTOR_WORLD,
            Quat::IDENTITY,
            Vec3::new(0.0, SYNTHETIC_RING_HEIGHT, -SYNTHETIC_RING_RADIUS),
        );
        for (with_camera, expected) in [(true, ring_toward_eye), (false, ring_toward_other)] {
            let mut d = synthetic_attached_def();
            d.attach_mode = mode::SOURCE;
            d.attach_eid = CAMERA_NEAREST_REFERENCE;
            let mut app = particle_app();
            let caster =
                spawn_turned_actor(&mut app, &skeleton, &pose, ACTOR_WORLD, Quat::IDENTITY);
            let other = spawn_turned_actor(&mut app, &skeleton, &pose, OTHER_WORLD, Quat::IDENTITY);
            if with_camera {
                app.world_mut()
                    .spawn((OperatorCamera, GlobalTransform::from_translation(EYE)));
            }
            fire_synthetic_stage(&mut app, d, caster, other);
            let (origin, _, _, _) = spawned_frame(&app);
            assert!(
                origin.distance(expected) < FRAME_TOLERANCE,
                "camera {with_camera}: origin {origin:?}, expected {expected:?}"
            );
        }
    }

    // .agents/skills/retail-observe/references/2026-10-05-attached-effect-placement.md Element
    // placement and billboard: the frame's rotation turns a fixed-orientation element and never
    // a screen billboard, which reads the frame through its world point alone.
    #[test]
    fn frame_rotation_turns_fixed_elements_but_not_screen_billboards() {
        let turn = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let offsets = |orientation: Option<Quat>, frame_rot: Quat| {
            let mut g = live(def(ROUTINE_FPS, ROUTINE_FPS, 0), 0.0);
            g.vel_basis = WORLD_PARTICLE_VEL_BASIS;
            g.orientation = orientation;
            g.frame_rot = frame_rot;
            g.template.positions = vec![Vec3::NEG_X, Vec3::X, Vec3::Y];
            emit(&mut g, ROUTINE_FPS);
            g.particles[0].pos = Vec3::ZERO;
            rebuilt(&g, view(Quat::IDENTITY)).0
        };
        let billboard_plain = offsets(None, Quat::IDENTITY);
        let billboard_turned = offsets(None, turn);
        for (a, b) in billboard_plain.iter().zip(&billboard_turned) {
            assert!(
                a.distance(*b) < FRAME_TOLERANCE,
                "a screen billboard ignores the frame rotation: {a:?} vs {b:?}"
            );
        }
        let fixed_plain = offsets(Some(Quat::IDENTITY), Quat::IDENTITY);
        let fixed_turned = offsets(Some(Quat::IDENTITY), turn);
        for (a, b) in fixed_plain.iter().zip(&fixed_turned) {
            assert!(
                (turn * *a).distance(*b) < FRAME_TOLERANCE,
                "a fixed-orientation element turns with the frame: {a:?} vs {b:?}"
            );
        }
    }

    // A mode the record leaves open keeps the single-point placement its legacy attach type
    // gave it: here mode 18 (bit 16 plus type 2) still lands on the target's ring entry
    // nearest the caster, unturned.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn modes_outside_the_record_keep_the_legacy_single_point_placement() {
        const LEGACY_TWO_POINT_MODE: u8 = 18;
        let skeleton = synthetic_skeleton();
        let mut d = synthetic_attached_def();
        d.attach_mode = LEGACY_TWO_POINT_MODE;
        d.attach_type = ffxi_dat::particle_gen::AttachType::TargetActor;
        d.attach_eid = 0;
        d.attach_target_reference = SYNTHETIC_CHEST_REFERENCE as u8;
        let mut scene = two_posed_actors(&skeleton);
        fire_synthetic_stage(&mut scene.app, d, scene.caster, scene.target);
        let (origin, rotation, _, _) = spawned_frame(&scene.app);
        let toward_caster = (CASTER_WORLD - TARGET_WORLD).normalize();
        let offset = origin - TARGET_WORLD;
        assert!(
            (offset.y + SYNTHETIC_RING_HEIGHT).abs() < FRAME_TOLERANCE,
            "legacy placement is a ring entry: {offset:?}"
        );
        assert!(
            Vec3::new(offset.x, 0.0, offset.z)
                .normalize()
                .dot(toward_caster)
                > 0.5
        );
        assert_eq!(rotation, Quat::IDENTITY);
    }

    fn child_factory(on_expiry: bool) -> ChildFactory {
        ChildFactory {
            name: *b"tst1",
            on_expiry,
            payload: ChildPayload::Draw(Box::new(ChildDraw {
                // fpe=30: only the primed first burst fires within the test's few ticks.
                def: def(30.0, 30.0, 1),
                template: SpriteTemplate {
                    positions: vec![Vec3::ZERO; 3],
                    uvs: vec![[0.0, 0.0]; 3],
                    indices: vec![0, 1, 2],
                    colors: vec![Vec4::ONE; 3],
                },
                sprite_frames: Vec::new(),
                mat: Handle::default(),
                draw_path: D3mDrawPath::D3m,
                tracks: GeneratorTracks::default(),
            })),
            children: Vec::new(),
        }
    }

    const ONE_FRAME_SECS: f32 = 1.0 / ROUTINE_FPS;

    // One full tick (anchor, advance, reap-removal, child instantiation) through the real system.
    fn tick_world(world: &mut World, secs: f32) {
        use bevy::ecs::system::RunSystemOnce;
        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(secs));
        world.run_system_once(tick_particle_simulator).unwrap();
    }

    fn child_test_world(sim: ParticleSimulator) -> World {
        let mut world = World::new();
        // Messages<T> has no FromWorld, so the bare test world inserts it for the SfxEvent writer.
        world.insert_resource(bevy::ecs::message::Messages::<crate::audio::SfxEvent>::default());
        world.insert_resource(bevy::ecs::message::Messages::<
            crate::scheduler_runtime::ParticleSpawnTrace,
        >::default());
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Assets::<Mesh>::default());
        // tick reads the Dynamic Lights setting for its enhance-mode suppression (Default =
        // Vanilla, so these tests exercise the retail texture path).
        world.insert_resource(crate::graphics_settings::GraphicsSettings::default());
        world.insert_resource(crate::distortion_pass::ActiveDistortion::default());
        world.insert_resource(sim);
        world.insert_resource(crate::distortion_pass::ActiveDistortion::default());
        world
    }

    #[test]
    fn mesh_cleanup_preserves_live_child_links() {
        use bevy::ecs::system::RunSystemOnce;

        const ONE_FRAME: f32 = 1.0;
        let mut world = child_test_world(ParticleSimulator::default());
        let mut expired = live(def(ROUTINE_FPS, ROUTINE_FPS, 0), 0.0);
        expired.stopped = true;
        let mut parent = live(def(ROUTINE_FPS, ROUTINE_FPS, 0), 0.0);
        prime(&mut parent);
        advance_generator(&mut parent, ONE_FRAME);
        let mut child = live(def(ROUTINE_FPS, ROUTINE_FPS, 0), 0.0);
        prime(&mut child);
        advance_generator(&mut child, ONE_FRAME);
        child.parent = Some((2, parent.particles[0].id));
        parent.particles[0].child_gens.push(1);
        for g in [&mut expired, &mut child, &mut parent] {
            g.entity = world
                .spawn((
                    Mesh3d(g.mesh.clone()),
                    GlobalTransform::IDENTITY,
                    Visibility::Inherited,
                ))
                .id();
        }
        let expired_entity = expired.entity;
        let child_entity = child.entity;
        let parent_entity = parent.entity;
        world.resource_mut::<ParticleSimulator>().generators = vec![expired, child, parent];

        world.run_system_once(sync_particle_meshes).unwrap();
        tick_world(&mut world, ONE_FRAME / ROUTINE_FPS);

        let sim = world.resource::<ParticleSimulator>();
        let survivors = [child_entity, parent_entity];
        assert_eq!(
            sim.generators.iter().map(|g| g.entity).collect::<Vec<_>>(),
            survivors,
            "the expired generator is reaped and the survivors keep their order"
        );
        assert!(world.get_entity(expired_entity).is_err());
        let child = sim
            .generators
            .iter()
            .find(|g| g.entity == child_entity)
            .expect("unrelated mesh cleanup must preserve the live child");
        let (parent_index, particle_id) = child.parent.expect("child retains its parent");
        let parent = &sim.generators[parent_index];
        assert_eq!(parent.entity, parent_entity);
        let particle = parent
            .particles
            .iter()
            .find(|p| p.id == particle_id)
            .expect("the referenced parent particle remains alive");
        let child_entities: Vec<_> = particle
            .child_gens
            .iter()
            .map(|&i| sim.generators[i].entity)
            .collect();
        assert_eq!(child_entities, [child_entity]);
    }

    #[test]
    fn mesh_cleanup_removes_children_of_missing_parent() {
        use bevy::ecs::system::RunSystemOnce;

        const ONE_FRAME: f32 = 1.0;
        let mut world = child_test_world(ParticleSimulator::default());
        let mut parent = live(def(ROUTINE_FPS, ROUTINE_FPS, 0), 0.0);
        parent.entity = world
            .spawn((
                Mesh3d(parent.mesh.clone()),
                GlobalTransform::IDENTITY,
                Visibility::Inherited,
            ))
            .id();
        let parent_entity = parent.entity;
        prime(&mut parent);
        parent.child_factories.push(child_factory(false));
        world
            .resource_mut::<ParticleSimulator>()
            .generators
            .push(parent);
        tick_world(&mut world, ONE_FRAME / ROUTINE_FPS);
        let children: Vec<_> = world
            .resource::<ParticleSimulator>()
            .generators
            .iter()
            .filter(|g| g.entity != parent_entity)
            .map(|g| g.entity)
            .collect();
        assert!(!children.is_empty(), "production tick must spawn children");
        world.despawn(parent_entity);

        world.run_system_once(sync_particle_meshes).unwrap();

        assert!(world.resource::<ParticleSimulator>().generators.is_empty());
        assert!(children.iter().all(|&e| world.get_entity(e).is_err()));
    }

    // The production spawn path primes the accumulator to one full period; `live()` does not.
    fn prime(g: &mut LiveGenerator) {
        g.emit_accum = g.def.frames_per_emission;
    }

    // research/xim ParticleInitializers.kt ChildGeneratorSetup — each parent particle gets its own
    // child generator instance, anchored at the particle's position. fpe=30 keeps emission to the
    // primed first burst so one frame tick emits exactly ppe+1 = 2 particles.
    #[test]
    fn child_generator_spawns_per_parent_particle() {
        let mut sim = ParticleSimulator::default();
        let mut parent = live(def(60.0, 30.0, 1), f32::MAX);
        prime(&mut parent);
        parent.child_factories.push(child_factory(false));
        sim.generators.push(parent);

        let mut world = child_test_world(sim);
        tick_world(&mut world, ONE_FRAME_SECS);

        let sim = world.resource::<ParticleSimulator>();
        assert_eq!(sim.generators.len(), 3);
        for (ci, g) in sim.generators.iter().enumerate().skip(1) {
            let Some((pgi, pid)) = g.parent else {
                panic!("child {ci} has no parent")
            };
            assert_eq!(pgi, 0);
            let p = &sim.generators[0]
                .particles
                .iter()
                .find(|p| p.id == pid)
                .expect("parent particle exists");
            assert_eq!(g.origin, sim.generators[0].origin + p.pos);
        }
    }

    // research/xim Particle.kt update — children are removed with their parent particle.
    #[test]
    fn child_generator_dies_with_its_parent_particle() {
        let mut sim = ParticleSimulator::default();
        let mut parent = live(def(1.0, 30.0, 1), f32::MAX);
        prime(&mut parent);
        parent.child_factories.push(child_factory(false));
        sim.generators.push(parent);

        let mut world = child_test_world(sim);
        tick_world(&mut world, ONE_FRAME_SECS); // emit + spawn children
        assert!(world.resource::<ParticleSimulator>().generators.len() > 1);
        tick_world(&mut world, ONE_FRAME_SECS); // particles expire (life 1 frame) -> children die

        let sim = world.resource::<ParticleSimulator>();
        assert_eq!(
            sim.generators.len(),
            1,
            "children must die with their parent particle"
        );
    }

    // An actor-local parent integrates in the actor's FFXI frame, but every child generator is
    // world-space: the child must anchor where the parent particle draws, with the parent's
    // velocity carried out of the actor frame.
    #[test]
    fn child_of_an_actor_local_parent_anchors_at_the_drawn_world_position() {
        const ACTOR_AT: Vec3 = Vec3::new(10.0, 0.0, -4.0);
        const LOCAL_ORIGIN: Vec3 = Vec3::new(1.0, 2.0, 3.0);
        let mut sim = ParticleSimulator::default();
        let mut parent = live(def(ROUTINE_FPS, ROUTINE_FPS, 1), f32::MAX);
        parent.actor_local = true;
        parent.origin = LOCAL_ORIGIN;
        parent.entity_world = GlobalTransform::from(
            Transform::from_translation(ACTOR_AT)
                .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        );
        prime(&mut parent);
        parent.child_factories.push(child_factory(false));
        sim.generators.push(parent);

        let mut world = child_test_world(sim);
        tick_world(&mut world, ONE_FRAME_SECS);

        let sim = world.resource::<ParticleSimulator>();
        let parent = &sim.generators[0];
        assert!(
            sim.generators.len() > 1,
            "the parent's burst spawns children"
        );
        for child in &sim.generators[1..] {
            let (_, pid) = child.parent.expect("anchored child");
            let p = parent.particles.iter().find(|p| p.id == pid).unwrap();
            let drawn = parent.entity_world.transform_point(parent.origin + p.pos);
            assert_eq!(child.origin, drawn);
            assert_ne!(child.origin, parent.origin + p.pos);
            assert_eq!(
                child.anchor.expect("anchor state").vel,
                parent
                    .entity_world
                    .affine()
                    .transform_vector3(p.vel + p.rel_vel)
            );
        }
    }

    // A sec2 0x82 rumble child whose named track no tier holds is dropped with an error; the
    // client never substitutes an envelope no DAT authored.
    #[test]
    fn rumble_child_with_an_unresolved_track_is_dropped() {
        const RUMBLE: [u8; 4] = *b"rmb1";
        const TRACK: [u8; 4] = *b"rtk1";
        const NEAR: f32 = 1.0;
        const FAR: f32 = 2.0;
        const HELD: f32 = 0.5;
        let mut parent = def(ROUTINE_FPS, ROUTINE_FPS, 0);
        parent.child_generator = Some(RUMBLE);
        let mut rumble = def(ROUTINE_FPS, ROUTINE_FPS, 0);
        rumble.rumble_track = Some(TRACK);
        rumble.rumble_falloff = Some([NEAR, FAR, 0.0]);
        let mut assets = ActionAssets::default();
        assets.particle_defs.insert(RUMBLE, rumble);
        let mut images = Assets::<Image>::default();
        let mut mats = Assets::<FfxiParticleMaterial>::default();

        let factories =
            resolve_child_factories(&parent, &assets, None, NO_LOCAL_DIR, &mut images, &mut mats);
        assert!(
            factories.is_empty(),
            "an unresolved envelope drops the child"
        );
        assert!(matches!(
            authored_envelope(&assets, None, None, RUMBLE, NO_LOCAL_DIR),
            Ok(None)
        ));

        assets.keyframes.insert(
            TRACK,
            KeyFrameTrack {
                points: vec![(0.0, HELD), (1.0, HELD)],
            },
        );
        let factories =
            resolve_child_factories(&parent, &assets, None, NO_LOCAL_DIR, &mut images, &mut mats);
        let [ChildFactory {
            payload: ChildPayload::Rumble { envelope, .. },
            ..
        }] = factories.as_slice()
        else {
            panic!("a resolved track arms the rumble child");
        };
        assert_eq!(
            envelope.points,
            rescale_track(assets.keyframes.get(&TRACK).unwrap()).points
        );
    }

    // research/xim ParticleExpirationHandlers.kt EmitChildHandler — one burst at the parent's
    // death position, independent of it.
    #[test]
    fn expiry_child_spawns_at_the_death_position() {
        let mut sim = ParticleSimulator::default();
        // ppe=0: the primed burst emits exactly one particle, so one death position.
        let mut parent = live(def(1.0, 30.0, 0), f32::MAX);
        prime(&mut parent);
        parent.child_factories.push(child_factory(true));
        sim.generators.push(parent);

        let mut world = child_test_world(sim);
        tick_world(&mut world, ONE_FRAME_SECS); // emit
        tick_world(&mut world, ONE_FRAME_SECS); // particle expires -> expiry spawn

        let sim = world.resource::<ParticleSimulator>();
        assert_eq!(sim.generators.len(), 2);
        let child = &sim.generators[1];
        assert!(
            child.parent.is_none(),
            "an expiry child is independent of the dead particle"
        );
        assert_eq!(child.emit_window_frames, 0.0);
    }

    // A distortion child (the ai90 shape: i900 binds it via sec2 0x44) arms the same screen-space
    // pass a 0x02 stage would — no mesh, no MissingResource.
    #[test]
    fn distortion_child_arms_the_shared_dispatch() {
        let mut sim = ParticleSimulator::default();
        let mut parent = live(def(60.0, 30.0, 1), f32::MAX);
        prime(&mut parent);
        parent.child_factories.push(ChildFactory {
            name: *b"ai90",
            on_expiry: false,
            payload: ChildPayload::Distortion {
                haze_offset: 0.02,
                life_frames: 60.0,
                envelope: None,
                half_extent: Some(Vec2::ONE),
            },
            children: Vec::new(),
        });
        sim.generators.push(parent);

        let mut world = child_test_world(sim);
        tick_world(&mut world, ONE_FRAME_SECS); // parent emits; the child arms on instantiation

        // The primed first tick emits ppe+1 = 2 parent particles; each particle arms its own
        // field at its own position.
        let dist = world.resource::<crate::distortion_pass::ActiveDistortion>();
        assert_eq!(dist.fields.len(), 2);
        for f in &dist.fields {
            assert_eq!(f.haze_offset, 0.02);
            assert_eq!(f.half_extent, Vec2::ONE);
        }
    }

    // Retail's resolver branches on element type 0x22 and returns before the named-resource search, so
    // a haze element whose linked name resolves to nothing is the ordinary case rather than a failure:
    // g142 in the crit chain names `dist`, and no mesh or image called `dist` exists in the global DAT
    // set at all (`.agents/skills/retail-observe/references/2026-10-07-procedural-distortion.md`).
    #[test]
    fn haze_def_arms_with_no_mesh_or_image_resolved() {
        let view = ParticleGeneratorDef {
            init_scale: [1.0, 1.0, 1.0],
            mesh_id: *b"dist",
            ..Default::default()
        };
        assert_eq!(procedural_half_extent(&view), Some(Vec2::ONE));

        let degenerate = ParticleGeneratorDef {
            init_scale: [0.0, 1.5, 1.0],
            ..view
        };
        assert_eq!(
            procedural_half_extent(&degenerate),
            None,
            "a zero authored scale leaves no footprint to draw"
        );

        let def = ffxi_dat::particle_gen::DistortionGeneratorDef {
            haze_offset: 0.02,
            max_life_frames: 45.0,
            generator_view: view,
            ..Default::default()
        };
        let mut distortion = crate::distortion_pass::ActiveDistortion::default();
        assert!(
            push_distortion_field(&def, Vec3::ZERO, None, None, &mut distortion),
            "an unresolvable linked name must not stop a 0x22 field from arming"
        );
        let armed = &distortion.fields[0];
        // The authored g142 values land on the field as written: unit axes at scale [1,1,1], haze
        // 0.02, a 45-frame life
        // (`.agents/skills/retail-observe/references/2026-10-07-procedural-distortion.md`).
        assert_eq!(armed.half_extent, Vec2::ONE);
        assert_eq!(armed.haze_offset, 0.02);
        assert_eq!(armed.duration_secs, def.max_life_frames / ROUTINE_FPS);
    }

    /// The carrying actor comes from the attach mode: a target-reference element rides the target and
    /// falls back to the routine's owner where `attach_frame` would, a source one rides the caster, and
    /// an unattached element is carried by nothing.
    #[test]
    fn authored_attach_mode_names_the_carrying_actor() {
        use ffxi_dat::particle_gen::attach_mode as mode;
        let owner = Entity::from_bits(7);
        let target = Entity::from_bits(9);
        let with = |attach_mode: u8| ParticleGeneratorDef {
            attach_mode,
            ..Default::default()
        };

        assert_eq!(
            field_carry_actor(&with(mode::TARGET), owner, Some(target)),
            Some(target),
            "a target-reference element rides its target"
        );
        assert_eq!(
            field_carry_actor(&with(mode::TARGET_TO_SOURCE), owner, None),
            Some(owner),
            "no target falls back to the routine's owner"
        );
        assert_eq!(
            field_carry_actor(&with(mode::SOURCE), owner, Some(target)),
            Some(owner),
            "a source-reference element rides its caster"
        );
        assert_eq!(
            field_carry_actor(&with(mode::UNATTACHED), owner, Some(target)),
            None
        );
    }

    // A sound child writes the same SfxEvent a 0x02 stage naming that generator would.
    #[test]
    fn sound_child_writes_the_sfx_event() {
        let mut sim = ParticleSimulator::default();
        let mut parent = live(def(60.0, 30.0, 1), f32::MAX);
        prime(&mut parent);
        parent.child_factories.push(ChildFactory {
            name: *b"g14s",
            on_expiry: false,
            payload: ChildPayload::Sound {
                se_id: 5008,
                near: 0.0,
                far: 30.0,
                vertical_weight: crate::audio::UNATTACHED_VERTICAL_WEIGHT,
            },
            children: Vec::new(),
        });
        sim.generators.push(parent);

        let mut world = child_test_world(sim);
        tick_world(&mut world, ONE_FRAME_SECS); // parent emits; the child plays on instantiation

        // The primed first tick emits ppe+1 = 2 parent particles (see
        // child_generator_spawns_per_parent_particle), so the child plays once per particle.
        let msgs = world.resource::<bevy::ecs::message::Messages<crate::audio::SfxEvent>>();
        assert_eq!(msgs.len(), 2);
        for ev in msgs.iter_current_update_messages() {
            assert_eq!(ev.se_id, 5008);
        }
    }

    // sec2 0x45..0x49 (research/xim ParticleInitializers.kt Parent*Config) — a child particle
    // copies its anchor's state at init.
    #[test]
    fn parent_copy_blocks_copy_the_anchor_state() {
        let mut pd = def(60.0, 30.0, 1);
        pd.init_velocity = [2.0, 0.0, 0.0];
        pd.init_color = [0.5, 0.25, 0.125, 1.0];
        pd.init_scale = [0.3, 0.4, 1.0];
        pd.init_rotation = [0.1, 0.2, 0.3];

        let mut cd = def(30.0, 30.0, 1);
        // The child's own init velocity would pass through the world basis; zero it so the
        // assertion isolates the anchor copy.
        cd.init_velocity = [0.0; 3];
        cd.parent_position_copy = true;
        cd.parent_velocity = Some(1.0);
        cd.parent_rotate = true;
        cd.parent_color = true;
        cd.parent_scale = true;

        let mut sim = ParticleSimulator::default();
        let mut parent = live(pd, f32::MAX);
        prime(&mut parent);
        let mut cf = child_factory(false);
        if let ChildPayload::Draw(d) = &mut cf.payload {
            d.def = cd;
        }
        parent.child_factories.push(cf);
        sim.generators.push(parent);

        let mut world = child_test_world(sim);
        tick_world(&mut world, ONE_FRAME_SECS); // parent emits; children spawn with the anchor
        tick_world(&mut world, ONE_FRAME_SECS); // children emit through the copy blocks

        let sim = world.resource::<ParticleSimulator>();
        let p = &sim.generators[0].particles[0];
        let c = &sim.generators[1].particles;
        assert!(!c.is_empty(), "the child must have emitted");
        let cp = &c[0];
        // 0x45: the particle spawns exactly at the anchor (zero local offset).
        assert_eq!(cp.pos, Vec3::ZERO);
        // 0x46: the parent's total velocity is added through the multiplier.
        assert_eq!(cp.vel, p.vel + p.rel_vel);
        // 0x47/0x48/0x49: rotation, color and scale are copied wholesale (the anchor carries
        // the parent's rescaled rgb).
        assert_eq!(cp.rotation, p.rotation);
        assert_eq!(cp.rgb, p.rgb);
        assert_eq!(cp.scale, Vec2::new(0.3, 0.4));
    }
}
