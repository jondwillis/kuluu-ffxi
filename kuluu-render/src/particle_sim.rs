use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use ffxi_dat::particle_gen::{
    KeyFrameTrack, ParticleBillboard, ParticleGeneratorDef, ParticleMeshKind,
};
use ffxi_dat::sprite_sheet::ParticleSpriteSheet;

use crate::camera::OperatorCamera;
use crate::components::InGameEntity;
use crate::dat_d3m::{decoded_texture_to_image, D3mBlendMode};
use crate::ffxi_particle_material::FfxiParticleMaterial;
use crate::scheduler_runtime::{
    assets_holding, ActionAssets, GlobalEffectDir, MmbSpriteMesh, SchedulerStageEvent, ROUTINE_FPS,
};
use ffxi_dat::scheduler::StageKind;

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

    // research/xim Particle.kt:238-254 — the two camera flags place differently. followCamera
    // pins the generator to the camera position outright (the base offset then lands per
    // particle through the billboard transform; the shipped curtains author pure-Y bases, so the
    // yaw-invariant vel_basis fold below is equivalent). cameraAttachedBasePosition rotates the
    // base offset by the view matrix — xim's `left*-x + up*y + forward*-z` with its
    // backward-pointing lookAtForward is `rot * (-x, y, -z)` here (Matrix4f.kt:265-327) — so
    // the mist/dust sheet is born in front of the viewer however they are turned (+z authors a
    // placement ahead of the camera). Both refresh every frame, but a cameraAttachedBasePosition
    // particle reads the result once — see `Particle::spawn_origin`.
    pub fn set_camera_relative_origins(&mut self, cam_pos: Vec3, cam_rot: Quat) {
        for g in &mut self.generators {
            if !g.camera_relative {
                continue;
            }
            let bp = g.def.base_position;
            g.origin = if g.def.camera_attached_base {
                cam_pos + cam_rot * Vec3::new(-bp[0], bp[1], -bp[2])
            } else {
                cam_pos + Vec3::from_array(bp) * g.vel_basis
            };
        }
    }

    // research/xim ParticleGeneratorAttachment / cexi-viewer particle/runtime.js:517-524 —
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

    // research/xim EffectRoutineParser.kt:253-258 StopParticleGeneratorRoutine — emission ceases
    // but the already-live particles play out their lifetime.
    pub fn stop_generator(&mut self, owner: Entity, gen_id: [u8; 4]) {
        self.stop_where(|o| o.owner == owner && o.gen_id == gen_id);
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

// research/XIClient/src/XIClient/source/Resource/Derived/CMoD3m.cpp:16-104 — the D3m texture-stage
// tables, with D = diffuse/vertex, T = texture, F = TEXTUREFACTOR (the generator's particle
// colour). NonZeroTwoTSS is the textured default: stage 0 is MODULATE2X(D,T) for both channels,
// stage 1 MODULATE2X(CURRENT,F) for rgb and MODULATE4X(CURRENT,F) for alpha — totals 4 and 8.
// NonZeroOneTSS (renderStateFlags 0x1000) replaces stage 0's alpha with SELECTARG1(D.a), halving
// the alpha total to 4. The MMB-mesh branch
// (research/XIClient/src/XIClient/source/Rendering/ZoneRenderer.cpp:1396-1433 DoD3mDraw) reaches
// the same per-stage ops, so every template kind goes through `d3m_stage_chain`.
const D3M_STAGE1_RGB_GAIN: f32 = 2.0;
const D3M_STAGE1_ALPHA_GAIN: f32 = 4.0;
// Stage 0's MODULATE2X is already folded into `SpriteTemplate::colors` by the /128 vertex-colour
// normalise (ffxi_dat::d3m::VERTEX_COLOR_DIVISOR). NonZeroOneTSS's SELECTARG1 does not double, so
// the ignore-texture-alpha table divides it back out.
const D3M_VERTEX_BAKED_GAIN: f32 = 2.0;
// D3D saturates every texture-stage result. Stage 0's texture argument is only available in the
// sampler, so the CPU keeps only the clamp it can evaluate exactly — stage 0's, which is exact
// wherever the vertex colour is at or below the /128 midpoint (D * T <= 1 then, so the clamp is
// a no-op either way). Stage 1's clamp lands in ffxi_particle.wgsl, after the texel multiply:
// applying it here instead threw away the 4x/8x MODULATE gains before the texel could use them,
// which is why the home point crystal (D3m alpha 4 * 1.0 * 1.0, saturated in retail at every
// `kori` texel) drew at bare texture alpha and let the ground show through.
const D3M_STAGE_CLAMP: f32 = 1.0;

// research/XIClient/src/XIClient/source/World/Generator/Effects/CMoD3mElem.cpp:57-63 — `OnDraw`
// sends the element through `DoMMBDraw` when its link is an MMB and `CMoD3m::Draw` otherwise. The
// two paths share the stage tables but not the blend bytes they honour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum D3mDrawPath {
    D3m,
    Mmb,
}

// CMoD3mElem.cpp:108-112 — DoMMBDraw forces the ignore-texture-alpha table at this blend byte,
// whatever the render-state bit says.
const D3M_MMB_FORCE_IGNORE_TEXTURE_ALPHA_BLEND_BYTE: u8 = 0x64;
// CMoD3m.cpp:345-349 — at blend byte 0x44 a TEXTUREFACTOR alpha at or above 0x7F is promoted to
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

// research/xim Particle.kt:217-218 — the day-of-week / moon-phase tints are applied with
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
) -> (Vec3, f32) {
    let clamp = Vec3::splat(D3M_STAGE_CLAMP);
    let stage0_rgb = vertex_rgb.min(clamp);
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

struct LiveGenerator {
    def: ParticleGeneratorDef,
    template: SpriteTemplate,
    draw_path: D3mDrawPath,
    // SpriteSheet (0x0E) flipbook frames; empty for a StaticMesh (0x0B) generator. When
    // non-empty each particle picks a frame by life progress in rebuild_mesh (research/xim
    // ParticleUpdaters.kt:196-211 SpriteSheetFrameUpdater).
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
    // research/xim ParticleGenerator.kt:56 — auto-run generators never finish
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
    // research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp:2817-2831 —
    // `GetSomeGeneratorScalar() * 0.3` scales the per-emission count whenever field_DE bit 0 is
    // set, which Open() arms for every generator under the `taew` (weat) container (:418-434).
    // See weather_particles::WEATHER_EMIT_SCALE for why it is applied to batched generators too.
    emit_scale: f32,
    emit_rng: u64,
    // Key of the last BUILT mesh (spawn writes `empty_mesh`, hence `MeshKey::Empty`), so
    // quantization error is bounded by one quantum and never accumulates across skipped frames.
    built_key: MeshKey,
}

// The count scale for a generator outside retail's `taew` container: its authored count, as is.
const UNSCALED_EMISSION: f32 = 1.0;

// Spawn-time knobs a zone/weather caller sets that the generator body cannot carry: retail derives
// both from where the chunk sits in the DAT tree, not from its own fields.
#[derive(Clone, Copy)]
pub struct ZoneGeneratorOptions {
    pub camera_relative: bool,
    pub emit_scale: f32,
}

impl Default for ZoneGeneratorOptions {
    fn default() -> Self {
        Self {
            camera_relative: false,
            emit_scale: UNSCALED_EMISSION,
        }
    }
}

// Auto-run particle generators embedded in an actor DAT (research/xim
// Actor.kt:724-734 startAutoRunParticles), attached at actor spawn by
// ffxi_actor_render and started by `spawn_actor_auto_run_particles`.
#[derive(Component)]
pub struct ActorAutoRunEffects {
    pub assets: std::sync::Arc<ActionAssets>,
}

struct Particle {
    pos: Vec3,
    // research/xim Particle.kt:238-241 — cameraAttachedBasePosition resolves the offset from the
    // camera only while `age == 0`, so the particle is placed in front of the viewer once and
    // then lives in world space. Carrying the live generator origin instead glues the whole
    // emission to the camera as one rigid sheet that swings out of view on a pitch.
    spawn_origin: Vec3,
    vel: Vec3,
    age_frames: f32,
    life_frames: f32,
    rgb: Vec3,
    scale: Vec2,
}

pub fn spawn_particle_generators(
    mut events: MessageReader<SchedulerStageEvent>,
    q_actors: Query<(&Transform, Option<&ActionAssets>)>,
    q_action_target: Query<&crate::scheduler_runtime::ActionTarget>,
    q_xf: Query<&Transform>,
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
        // A cast routine's generators ship in the global effect dir, never in the caster's own
        // ActionAssets, so the def resolves against whichever tier actually holds it.
        let local_dir = ev.stage.stage.local_dir;
        let Some(assets) = assets_holding(local_assets, global.as_ref().map(|g| &g.assets), |a| {
            a.particle_def(local_dir, &ev.stage.stage.id).is_some()
        }) else {
            continue;
        };
        let Some(def) = assets.particle_def(local_dir, &ev.stage.stage.id).copied() else {
            continue;
        };
        let Some((template, sprite_frames, tex)) = resolve_mesh(assets, &def, &mut images) else {
            continue;
        };
        let origin_entity = crate::scheduler_runtime::particle_origin_entity(
            def.attach_type,
            ev.actor,
            q_action_target.get(ev.actor).ok().and_then(|t| t.0),
        );
        let origin_xf = if origin_entity == ev.actor {
            actor_xf
        } else {
            q_xf.get(origin_entity).unwrap_or(actor_xf)
        };
        let blend = match def.blend {
            ffxi_dat::particle_gen::ParticleBlend::Additive => D3mBlendMode::Additive,
            ffxi_dat::particle_gen::ParticleBlend::Blend => D3mBlendMode::Blended,
            ffxi_dat::particle_gen::ParticleBlend::Subtract => D3mBlendMode::Subtractive,
        };
        let mat = mats.add(FfxiParticleMaterial::new(blend, tex));
        let mesh = meshes.add(empty_mesh());

        let entity = commands
            .spawn((
                InGameEntity,
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat),
                Transform::IDENTITY,
                Visibility::default(),
                // The mesh is rebuilt in place every frame; Bevy computes a frustum-culling Aabb
                // once from the initially-empty mesh and never recomputes it, so the entity would
                // be culled forever. Opt out of culling instead.
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
            scale_x: resolve(def.scale_x_track),
            scale_y: resolve(def.scale_y_track),
            alpha: resolve(def.alpha_track),
            tod_color: resolve_tod_tracks(&def, assets),
            solid_mesh: is_solid_mesh(&template),
            template,
            draw_path: D3mDrawPath::D3m,
            sprite_frames,
            def,
            origin: origin_xf.translation + Vec3::Y * def.base_position[1],
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
            vel_basis: Vec3::ONE,
            origin_routine: Some(RoutineOrigin {
                owner: ev.actor,
                gen_id: ev.stage.stage.id,
                routine: ev.scheduler,
            }),
            stopped: false,
            camera_relative: false,
            emit_scale: UNSCALED_EMISSION,
            emit_rng: emit_seed(entity),
            built_key: MeshKey::Empty,
        });
    }
}

// research/xim Actor.kt:127,724-734 — at model-ready, every generator in the
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
            let Some((template, sprite_frames, tex)) = resolve_mesh(&fx.assets, &def, &mut images)
            else {
                continue;
            };
            let blend = match def.blend {
                ffxi_dat::particle_gen::ParticleBlend::Additive => D3mBlendMode::Additive,
                ffxi_dat::particle_gen::ParticleBlend::Blend => D3mBlendMode::Blended,
                ffxi_dat::particle_gen::ParticleBlend::Subtract => D3mBlendMode::Subtractive,
            };
            let mat = mats.add(FfxiParticleMaterial::new(blend, tex));
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
                scale_x: resolve(def.scale_x_track),
                scale_y: resolve(def.scale_y_track),
                alpha: resolve(def.alpha_track),
                tod_color: resolve_tod_tracks(&def, &fx.assets),
                solid_mesh: is_solid_mesh(&template),
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
                emit_scale: UNSCALED_EMISSION,
                emit_rng: emit_seed(entity),
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
    let (template, sprite_frames, tex, draw_path) = resolve_zone_mesh(assets, &def, images)
        .or_else(|| global.and_then(|g| resolve_zone_mesh(g, &def, images)))?;
    let blend = match def.blend {
        ffxi_dat::particle_gen::ParticleBlend::Additive => D3mBlendMode::Additive,
        ffxi_dat::particle_gen::ParticleBlend::Blend => D3mBlendMode::Blended,
        ffxi_dat::particle_gen::ParticleBlend::Subtract => D3mBlendMode::Subtractive,
    };
    let mat = mats.add(FfxiParticleMaterial::new(blend, tex));
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
        scale_x: resolve(def.scale_x_track),
        scale_y: resolve(def.scale_y_track),
        alpha: resolve(def.alpha_track),
        tod_color: def.tod_color_tracks.map(|id| keyframe(assets, global, id)),
        solid_mesh: is_solid_mesh(&template),
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
        emit_scale: opts.emit_scale,
        emit_rng: emit_seed(entity),
        built_key: MeshKey::Empty,
        def,
    });
    Some(entity)
}

// research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp:158 — a batched
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

// Distinct per generator so two emitters sharing a def do not spawn identical particle clouds;
// deterministic so a rebuilt zone/weather set replays the same spread.
fn emit_seed(entity: Entity) -> u64 {
    const SEED: u64 = 0x9E37_79B9_7F4A_7C15;
    SEED ^ entity.to_bits().wrapping_mul(SEED)
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

pub fn tick_particle_simulator(time: Res<Time>, mut sim: ResMut<ParticleSimulator>) {
    let frames = time.delta_secs() * ROUTINE_FPS;
    for g in &mut sim.generators {
        advance_generator(g, frames);
    }
}

fn advance_generator(g: &mut LiveGenerator, frames: f32) {
    g.age_frames += frames;

    // research/xim ParticleGenerator.kt:66 — completed particles are swept
    // before emission, so a continuous singleton re-emits the same tick its
    // predecessor expires.
    g.particles.retain(|p| p.age_frames < p.life_frames);

    // Particles emitted below were born during this tick, so the ageing pass must not charge them
    // the whole frame: at 30 fps retail that error is invisible, but one long frame (the blocking
    // action-DAT read) would otherwise age a freshly emitted short-life particle past its life and
    // sweep it before it ever renders.
    let pre_emit_len = g.particles.len();

    // research/xim: a maxLifeSpan of 0 marks a singleton — emit one particle once.
    let singleton = g.def.is_singleton();
    let emitting = !g.stopped && (g.auto_run || g.age_frames <= g.emit_window_frames.max(1.0));
    if singleton {
        // `age_frames <= frames` already pins this to the first tick, so the emit window must not
        // gate it: a long frame (the blocking action-DAT read precedes these) makes age_frames
        // exceed a dur=0 stage's 1-frame window on that very tick and the singleton never fires.
        if !g.stopped && g.particles.is_empty() && g.age_frames <= frames {
            // research/xim ParticleInitializers.kt:130-131 — a maxLifeSpan of 0 is rewritten
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
    } else if emitting {
        g.emit_accum += frames;
        while g.emit_accum >= g.def.frames_per_emission {
            // research/xim ParticleGenerator.kt:80 — a continuous-singleton
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
    for p in g.particles.iter_mut().take(pre_emit_len) {
        p.age_frames += frames;
        if let Some(a) = accel {
            p.vel += a;
        }
        p.pos += p.vel * frames;
    }
    g.particles.retain(|p| p.age_frames < p.life_frames);

    // A continuous generator re-emits "the moment its particle expires"
    // (research/xim ParticleGenerator.kt:80). The aging above can push the lone
    // particle past its life within this same tick, after the pre-emit sweep
    // already ran — replace it now so the mesh is never empty at render and the
    // body does not blink out for a frame.
    if g.def.continuous && g.particles.is_empty() && continuous_active(g) {
        emit(g, g.def.max_life_frames);
    }
}

fn continuous_active(g: &LiveGenerator) -> bool {
    !g.stopped && (g.auto_run || g.age_frames <= g.emit_window_frames.max(1.0))
}

// research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp:2818-2830 — the emit loop
// runs `for counter in 0..=floor(v161)` over `v161 = (flags & 0x1FF) * scale`, i.e. floor + 1. That
// trailing +1 is deliberately not reproduced: it would raise every already-tuned non-weather
// population (10740 shipped generators author a non-zero count) by one particle, so the floor of 1
// below stands in for it and keeps an authored count of 0 emitting the single particle retail
// gives it.
fn emission_count(g: &LiveGenerator) -> u32 {
    ((g.def.particles_per_emission as f32 * g.emit_scale) as u32).max(1)
}

fn emit(g: &mut LiveGenerator, life_frames: f32) {
    // research/XIClient/.../CYyGenerator.cpp:857-871 applies the sec2 0x06/0x07 spawn spread to the
    // elem, skipping it when CheckFlag29 is set because a batched elem carries its own
    // sub-particles. Our Particle models the sub-particle in that case, so the spread applies
    // either way — without it every drop of a rain curtain spawns on one point.
    let pos = match g.def.position_variance {
        Some(v) => {
            let u = next_unit(&mut g.emit_rng);
            let yaw = (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * std::f32::consts::PI;
            let pitch = (next_unit(&mut g.emit_rng) * 2.0 - 1.0) * std::f32::consts::PI;
            Vec3::from_array(v.offset(u, yaw, pitch)) * g.vel_basis
        }
        None => Vec3::ZERO,
    };
    g.particles.push(Particle {
        pos,
        spawn_origin: g.origin,
        vel: Vec3::from_array(g.def.init_velocity) * g.vel_basis,
        age_frames: 0.0,
        life_frames: life_frames.max(1.0),
        rgb: Vec3::from_slice(&g.def.init_color[..3]),
        scale: Vec2::new(g.def.init_scale[0], g.def.init_scale[1]),
    });
}

pub fn sync_particle_meshes(
    cam: Query<&GlobalTransform, With<OperatorCamera>>,
    q_mesh_xf: Query<&GlobalTransform, With<Mesh3d>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut sim: ResMut<ParticleSimulator>,
    mut commands: Commands,
) {
    let cam_xf = cam.iter().next().copied().unwrap_or_default();
    let (cam_rot, cam_pos) = (cam_xf.rotation(), cam_xf.translation());
    let clock = sim.clock;
    let trace_celestial = std::env::var_os("FFXI_TRACE_CELESTIAL").is_some();

    // (index, despawn-needed); indices ascending so the reverse sweep below can
    // swap_remove safely.
    let mut reap: Vec<(usize, bool)> = Vec::new();
    for (i, g) in sim.generators.iter_mut().enumerate() {
        // The mesh entity despawns with its actor (auto-run generators are
        // children of the actor root); reap the simulator entry when it's gone.
        let Ok(entity_xf) = q_mesh_xf.get(g.entity) else {
            reap.push((i, false));
            continue;
        };
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
        let view = CameraView { rot, pos: cam_pos };
        let key = mesh_key(g, view, &clock);
        if needs_rebuild(&g.built_key, &key) {
            if let Some(mut mesh) = meshes.get_mut(&g.mesh) {
                rebuild_mesh(g, view, &clock, &mut mesh);
                g.built_key = key;
            }
        }
        let window_over =
            g.stopped || (!g.auto_run && g.age_frames > g.emit_window_frames.max(1.0));
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
    // ParticleUpdaters.kt:196-211), except under MoonPhaseSpriteSheetUpdater
    // (ParticleUpdaters.kt:319-324, opcode 0x45 at ParticleGeneratorParser.kt:444), which pins
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
        .map(|t| t.sample_from(progress, Some(p.scale.x)))
        .unwrap_or(p.scale.x);
    let sy = g
        .scale_y
        .as_ref()
        .map(|t| t.sample_from(progress, Some(p.scale.y)))
        .unwrap_or(p.scale.y);
    // Additive blend ignores alpha, so the alpha track drives brightness. With
    // no track, a transient spray fades linearly to nothing over life; a
    // continuous generator (one particle re-emitted on expiry — the steady
    // crystal body) holds full opacity, or each re-emit cycle would fade the
    // single particle out and strobe the whole model transparent.
    let alpha = g
        .alpha
        .as_ref()
        .map(|t| t.sample_from(progress, Some(g.def.init_color[3])))
        .unwrap_or(if g.def.continuous {
            1.0
        } else {
            1.0 - progress
        });
    // research/xim ParticleGeneratorParser.kt:431-434 ClockValueUpdater — 0x3C/0x3D/0x3E
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
    // research/xim Particle.kt:217-218 getColor() — the day-of-week tint is applied first,
    // then the moon-phase tint, each as a 2x modulate (out = min(1, out * 2 * c)). Both use
    // Color.modulateInPlace (Color.kt:102-108), which scales alpha too, and NOT the rgb-only
    // Color.modulateRgbInPlace (Color.kt:95-100) sitting next to it: the tables' alpha lane is
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
        (_, D3mDrawPath::Mmb) => [
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
#[derive(PartialEq, Eq, Debug)]
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

#[derive(PartialEq, Eq, Debug)]
struct ParticleKey {
    world: [i32; 3],
    flipbook_frame: usize,
    scale: [i32; 2],
    // The per-particle colour inputs rather than the drawn colour: the template's per-vertex
    // half is fixed once `flipbook_frame` is, so these are the only terms that can move it.
    factor_rgb: [i32; 3],
    factor_alpha: i32,
    life_alpha: i32,
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
                }
            })
            .collect(),
    }
}

fn needs_rebuild(built: &MeshKey, next: &MeshKey) -> bool {
    built != next
}

// research/xim Particle.kt:326-334 + GLDrawer.kt:474-489 — BillBoardType::Camera is not a screen
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

// research/xim Particle.kt:548-569 `applyMovementOrientation`, with the direction supplied by
// Particle.kt:330 (`camera position - particle position`). `vel_basis` is an involution, so the
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
        } else {
            cam.rot
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
        // instead of standing up above the emitter (kuluu-czc6). Screen billboards
        // orient in Bevy already; actor-local generators integrate in the actor frame.
        let world_basis = (g.orientation.is_some() || axial) && !g.actor_local;
        let base = positions.len() as u32;
        for ((tp, uv), vertex) in tpl.positions.iter().zip(&tpl.uvs).zip(&tpl.colors) {
            let local = Vec3::new(tp.x * draw.scale.x, tp.y * draw.scale.y, tp.z * sz);
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
    for _ in 0..3 {
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
) -> Option<(
    SpriteTemplate,
    Vec<SpriteTemplate>,
    Option<Handle<Image>>,
    D3mDrawPath,
)> {
    if let Some((template, frames, tex)) = resolve_mesh(assets, def, images) {
        return Some((template, frames, tex, D3mDrawPath::D3m));
    }
    let mmb = assets.mmbs.get(&def.mesh_id)?;
    let template = mmb_sprite_template(mmb)?;
    let tex = assets
        .images_by_name
        .get(&mmb.texture_name)
        .map(|t| images.add(decoded_texture_to_image(t)));
    Some((template, Vec::new(), tex, D3mDrawPath::Mmb))
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

fn resolve_mesh(
    assets: &ActionAssets,
    def: &ParticleGeneratorDef,
    images: &mut Assets<Image>,
) -> Option<(SpriteTemplate, Vec<SpriteTemplate>, Option<Handle<Image>>)> {
    match def.mesh_kind {
        ParticleMeshKind::StaticMesh => {
            let d3m = assets.d3ms.get(&def.mesh_id)?;
            let template = sprite_template(d3m)?;
            let (namespace, local) = d3m.texture_name_tokens();
            // research/xim DatResource.kt:488-493 — qualified (namespace, local) match, then
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
                .map(|t| images.add(decoded_texture_to_image(t)));
            Some((template, Vec::new(), tex))
        }
        ParticleMeshKind::SpriteSheet => {
            let ss = assets.sprite_sheets.get(&def.mesh_id)?;
            let frames = sprite_sheet_templates(ss);
            let first = frames.first().cloned()?;
            // research/xim DatResource.kt:483-493 — try the qualified (namespace, local) pair
            // first, then fall back to a local-name-only match.
            let tex = assets
                .images_by_qualified_name
                .get(&(ss.category.clone(), ss.id.clone()))
                .or_else(|| assets.images_by_name.get(&ss.id))
                .map(|t| images.add(decoded_texture_to_image(t)));
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

// research/xim ParticleUpdaters.kt:196-211 — the spriteSheetIndex advances the flipbook across
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
            init_color: [0.2, 0.2, 0.6, 0.5],
            init_velocity: [0.0, 0.01, 0.0],
            init_rotation: [0.0; 3],
            blend: ffxi_dat::particle_gen::ParticleBlend::Additive,
            blend_byte: 0x48,
            ignore_texture_alpha: false,
            scale_x_track: None,
            scale_y_track: None,
            alpha_track: None,
            day_of_week_color: None,
            moon_phase_color: None,
            uv_scroll: [0.0, 0.0],
            accel: None,
        }
    }

    fn live(def: ParticleGeneratorDef, window: f32) -> LiveGenerator {
        LiveGenerator {
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
            emit_scale: UNSCALED_EMISSION,
            emit_rng: emit_seed(Entity::PLACEHOLDER),
            built_key: MeshKey::Empty,
        }
    }

    // Drive the emission math directly (no Bevy world), one tick's worth of frames per call.
    fn advance(g: &mut LiveGenerator, frames: f32) {
        advance_generator(g, frames);
    }

    // One colour on every template vertex, so a stage-chain expectation is a single number
    // per particle instead of a gradient.
    fn set_template_color(g: &mut LiveGenerator, rgba: Vec4) {
        g.template.colors = vec![rgba; g.template.positions.len()];
    }

    // The colour a particle actually draws with: `particle_draw` now returns only the
    // per-particle half, and the template's per-vertex half folds in at `vertex_color`.
    fn drawn_color(g: &LiveGenerator, clock: &CelestialClock) -> Vec4 {
        let draw = particle_draw(g, &g.particles[0], clock);
        Vec4::from_array(vertex_color(g, &draw, g.template.colors[0]))
    }

    // A generator stage's duration is authored in 60 fps frames (research/xim util/Fps.kt:9),
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

    // research/xim MainTool.kt:64 turns wall ms into frames at the single 60 fps internal
    // clock (util/Fps.kt:9), MainTool.kt:118 hands that one value to EffectManager.update,
    // and Scene.kt:125-126 registers the zone DAT's autoRun generators (braziers,
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

    // La Theine's `~1ra` curtain is followCamera with a pure-Y base: it rides the camera with
    // its 35-up offset however the camera yaws. The `rai2`/`~1du` sheets are
    // cameraAttachedBasePosition: the authored offset is view-space, so a +z placement stays in
    // front of the viewer as the camera turns (research/xim Particle.kt:238-254).
    #[test]
    fn camera_relative_origins_split_by_flag() {
        let cam_pos = Vec3::new(100.0, 5.0, 200.0);
        let yaw180 = Quat::from_rotation_y(std::f32::consts::PI);

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

        sim.set_camera_relative_origins(cam_pos, Quat::IDENTITY);
        assert_eq!(
            sim.generators[0].origin,
            cam_pos + Vec3::new(0.0, 35.0, 0.0)
        );
        // Identity view looks along -Z: the authored (0, -10, 10) lands 10 below and 10 ahead.
        assert_eq!(
            sim.generators[1].origin,
            cam_pos + Vec3::new(0.0, -10.0, -10.0)
        );

        sim.set_camera_relative_origins(cam_pos, yaw180);
        // The curtain's vertical fold is yaw-invariant; the sheet swings behind the turn.
        assert_eq!(
            sim.generators[0].origin,
            cam_pos + Vec3::new(0.0, 35.0, 0.0)
        );
        let got = sim.generators[1].origin;
        let want = cam_pos + Vec3::new(0.0, -10.0, 10.0);
        assert!((got - want).length() < 1e-4, "{got} != {want}");
    }

    // research/xim Particle.kt:238-241 — a cameraAttachedBasePosition particle reads the offset
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

        sim.set_camera_relative_origins(Vec3::ZERO, Quat::IDENTITY);
        advance(&mut sim.generators[0], 10.0);
        let born = sim.generators[0].particles.len();
        assert!(born > 0);

        // The camera walks 100 yalms and turns to look the other way.
        let moved = Vec3::new(100.0, 0.0, 0.0);
        sim.set_camera_relative_origins(moved, Quat::from_rotation_y(std::f32::consts::PI));
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
                (*w - (moved + Vec3::new(0.0, 0.0, 13.0))).length() < 1e-3,
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

    // research/XIClient/src/XIClient/source/Resource/Derived/CMoD3m.cpp:16-104. A template
    // colour already carries stage 0's MODULATE2X (the /128 normalise), so an input of 0.25
    // here stands for a retail D of 0.125.
    mod stage_chain {
        use super::*;

        // NonZeroTwoTSS: rgb = 4*D*T*F, alpha = 8*D.a*T.a*F.a, with T left to the sampler.
        #[test]
        fn textured_default_reaches_the_retail_totals_below_saturation() {
            let (rgb, alpha) =
                d3m_stage_chain(Vec3::splat(0.25), 0.25, Vec3::splat(0.25), 0.25, false);
            assert_eq!(rgb, Vec3::splat(4.0 * 0.125 * 0.25));
            assert_eq!(alpha, 8.0 * 0.125 * 0.25);
        }

        // NonZeroOneTSS (renderStateFlags 0x1000): stage 0 selects D.a instead of modulating it
        // with the texture alpha, so the total is 4*D.a*F.a — half the default, rgb untouched.
        #[test]
        fn ignoring_texture_alpha_halves_the_alpha_total() {
            let two = d3m_stage_chain(Vec3::splat(0.25), 0.25, Vec3::splat(0.25), 0.25, false);
            let one = d3m_stage_chain(Vec3::splat(0.25), 0.25, Vec3::splat(0.25), 0.25, true);
            assert_eq!(one.1, two.1 / 2.0);
            assert_eq!(one.0, two.0);
        }

        // D3D saturates each stage on its own: a 0xFF vertex byte clips at stage 0, so stage 1's
        // MODULATE4X starts from 1.0 instead of carrying the excess through it.
        #[test]
        fn stage_zero_saturates_before_the_stage_one_gain() {
            let vert = u8::MAX as f32 / ffxi_dat::d3m::VERTEX_COLOR_DIVISOR;
            let (rgb, alpha) = d3m_stage_chain(Vec3::splat(vert), vert, Vec3::ONE, 0.15, false);
            assert_eq!(alpha, D3M_STAGE_CLAMP * 0.15 * D3M_STAGE1_ALPHA_GAIN);
            assert_eq!(rgb, Vec3::splat(D3M_STAGE_CLAMP * D3M_STAGE1_RGB_GAIN));
        }

        // Stage 1's own saturation is ffxi_particle.wgsl's, because it has to land after the
        // texel multiply. Clamping the gain away here is what left the D3m 4x/8x MODULATE
        // unable to lift a sub-unit texel to retail's ceiling.
        #[test]
        fn the_stage_one_gain_leaves_the_cpu_unsaturated_for_the_shader_to_clamp() {
            let (rgb, alpha) = d3m_stage_chain(Vec3::ONE, 1.0, Vec3::ONE, 1.0, false);
            assert_eq!(rgb, Vec3::splat(D3M_STAGE1_RGB_GAIN));
            assert_eq!(alpha, D3M_STAGE1_ALPHA_GAIN);
        }

        // CMoD3mElem.cpp:108-112 — DoMMBDraw forces the ignore-texture-alpha table at blend byte
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

        // CMoD3m.cpp:345-349 — blend byte 0x44 only, and only on the CMoD3m::Draw path.
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

    #[test]
    fn camera_billboard_sheet_not_flipped() {
        let g = sheet_gen(None);
        let mut mesh = empty_mesh();
        rebuild_mesh(
            &g,
            view(Quat::IDENTITY),
            &CelestialClock::default(),
            &mut mesh,
        );
        // Billboard: no basis flip, so the same +Y geometry rises above the emitter.
        assert!(
            max_sheet_y(&mesh) > 10.0 + 1.0,
            "camera billboards must keep their unflipped local frame"
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

    // research/xim EffectRoutineParser.kt:253-258 StopParticleGeneratorRoutine: the cast aura's
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

    // research/xim ParticleInitializers.kt:130-131 — maxLifeSpan 0 means POSITIVE_INFINITY
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

    // research/xim Particle.kt:330 + 548-569 — BillBoardType::Camera orients the particle in the
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
    // ScaleInitializer writes all three (research/xim ParticleInitializers.kt:846-857) and file
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
            resolve_zone_mesh(assets, &def, &mut images).expect("its linked mesh resolves");
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

    // research/xim ParticleGeneratorParser.kt:431-434 — the ClockValueUpdater curves are
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

    // research/xim Particle.kt:217-218 — day-of-week first, then moon phase, each a 2x
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

    // research/xim Particle.kt:217-218 modulates with Color.modulateInPlace (Color.kt:102-108),
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
        let root = ffxi_dat::DatRoot::from_env_or_default().ok()?;
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
        const SPRITE_MIN_ALPHA: f32 = 0.5;
        // `kasa`'s 0x4F alpha lane as shipped, dumped byte-for-byte from f_ro.
        const HALO_PHASE_ALPHA_BYTE: [u8; ffxi_dat::particle_gen::MOON_PHASES] =
            [0, 0, 0, 0, 0, 60, 128, 60, 0, 0, 0, 0];
        // The rest of `kasa`'s modulate chain (its day-of-week lane and init colour, both
        // phase-independent) is a constant gain on that lane: 160/255 as shipped.
        const HALO_CHAIN_GAIN: f32 = 160.0 / u8::MAX as f32;
        const ALPHA_EPS: f32 = 1e-6;

        let Some(bytes) = zone_bytes(F_RO) else {
            eprintln!("skipping: no retail DAT root (set FFXI_DAT_PATH)");
            return;
        };
        let halo = moon_attached_def(&bytes, b"kasa");
        let sprite = moon_attached_def(&bytes, b"moon");
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
                "the moon disc itself stays visible at phase {phase}"
            );
        }
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

    // research/xim ParticleGeneratorParser.kt:444 MoonPhaseSpriteSheetUpdater — the moon's
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
        // A continuous generator holds one particle re-emitted on expiry (the
        // steady crystal body). Track-less, it must stay fully opaque — if it fell
        // back to the 1.0-progress spray fade, the single particle would fade out
        // each cycle and strobe the whole model transparent.
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
            (alpha_of(&cont) - expected(1.0)).abs() < 1e-4,
            "continuous body stays fully opaque, not the life fade"
        );
        assert!(
            (alpha_of(&spray) - expected(0.25)).abs() < 1e-4,
            "a transient spray still fades 1.0-progress over life"
        );
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
            resolve_mesh(assets, &sheet_def(), &mut images)
                .expect("sheet mesh resolves")
                .2
        }

        // research/xim DatResource.kt:483-493 — qualified (namespace, local) match first.
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
            resolve_mesh(assets, &mesh_def(), &mut images)
                .expect("static mesh resolves")
                .2
        }

        // research/xim DatResource.kt:488-493 — qualified (namespace, local) match first.
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
            resolve_mesh(assets, def, &mut images)
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
}
