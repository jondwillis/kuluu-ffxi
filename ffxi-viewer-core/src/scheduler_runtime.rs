//! Action-scheduler runtime: advance parsed FFXI Scheduler timelines
//! and fan stages out as Bevy messages.
//!
//! An FFXI action (spell, ability, mob-skill) ships its event timeline
//! inside the action DAT as a `Scheduler` chunk (kind `0x07`). Each
//! stage records `frame: u32` (running sum of `delay_frames`) plus a
//! `kind` and 4-char `id`. The id resolves to a sibling Generator or
//! Sep chunk that says *what* to fire — sound, particle, motion.
//!
//! This module turns a parsed [`Scheduler`] into a per-entity runtime
//! component ([`ActiveScheduler`]) and emits [`SchedulerStageEvent`]
//! when the playhead crosses each stage. Downstream consumers:
//!
//!   - **E3 SE**: subscribe to events where `stage.kind` is
//!     `SoundOnCaster` / `SoundOnTarget`, resolve generator/sep, write
//!     `SfxEvent`.
//!   - **D2 particles**: subscribe to events where `stage.kind` is
//!     `Particle`, look up the named Generator + child D3M, spawn a
//!     billboard entity (D3).
//!   - Future motion/animation: `StageKind::Motion` is forwarded too;
//!     animation playback can swap idle for the named motion.
//!
//! ## Why per-entity components, not a Resource
//!
//! An action plays *on* an actor. Multiple actors can have overlapping
//! schedulers (cleric casting Cure while monster swings axe), so the
//! natural shape is `Component<ActiveScheduler>` per acting entity.
//! That also gives us lifecycle for free: when the actor despawns
//! (zone change, death), the scheduler dies with it — no separate
//! drain needed beyond the existing `InGameEntity` cleanup the actor
//! already participates in (see [[feedback_bevy_lifecycle_symmetry]]).
//!
//! ## Time base
//!
//! FFXI animations run at 30 fps. The runtime uses real elapsed time
//! (Bevy `Time::delta_secs`) × 30 to compute the current frame. This
//! is independent of Vana time — actions are real-time events, not
//! diel-time keyframes. (Earlier plan revision incorrectly assumed
//! Scheduler keys off VanaSky; it does not.)

use std::collections::HashMap;

use bevy::prelude::*;
use ffxi_dat::chunk::walk;
use ffxi_dat::generator::Generator;
use ffxi_dat::kind::ChunkKind;
use ffxi_dat::scheduler::{Scheduler, StageKind, TimedStage};
use ffxi_dat::sep::Sep;

/// FFXI animation frame rate. All Scheduler `frame` fields are in
/// units of 1/30 second from action start.
pub const FFXI_FPS: f32 = 30.0;

/// Time-to-live appended after the last stage fires before the
/// component is auto-removed. Keeps the runtime visible to late
/// inspections (e.g. /debug overlays) without leaking forever.
const POST_FINISH_TTL_SECS: f32 = 2.0;

/// One actively playing Scheduler attached to an actor entity.
///
/// Stages are stored sorted ascending by `frame`; the playhead
/// (`cursor`) advances monotonically. Reaching `cursor == stages.len()`
/// means the timeline has finished firing; the component is removed
/// after [`POST_FINISH_TTL_SECS`].
#[derive(Component, Debug, Clone)]
pub struct ActiveScheduler {
    /// Stages sorted by frame, ascending.
    pub stages: Vec<TimedStage>,
    /// Real seconds since the scheduler started playing.
    pub elapsed: f32,
    /// Index of the next un-emitted stage. `cursor == stages.len()`
    /// when finished.
    pub cursor: usize,
    /// Scheduler chunk name (e.g. "main"), preserved for debugging.
    pub name: [u8; 4],
}

impl ActiveScheduler {
    /// Build a runtime component from a parsed [`Scheduler`].
    /// Stages are cloned and sorted; the source `Scheduler` may drop
    /// after this returns.
    pub fn from_scheduler(s: &Scheduler) -> Self {
        let mut stages = s.stages.clone();
        stages.sort_by_key(|t| t.frame);
        Self {
            stages,
            elapsed: 0.0,
            cursor: 0,
            name: s.name,
        }
    }

    /// True once every stage has been emitted.
    pub fn finished(&self) -> bool {
        self.cursor >= self.stages.len()
    }

    /// Current playhead in FFXI frames (= 1/30 second units).
    pub fn current_frame(&self) -> u32 {
        (self.elapsed * FFXI_FPS) as u32
    }

    /// Frame at which the last stage fires, or 0 for an empty timeline.
    pub fn last_frame(&self) -> u32 {
        self.stages.last().map(|t| t.frame).unwrap_or(0)
    }
}

/// Emitted once per stage-edge crossing. Listeners filter by
/// `stage.stage.kind` (motion / sound-on-caster / sound-on-target /
/// particle / unknown) and resolve `stage.stage.id` against their own
/// generator+sep tables.
#[derive(Message, Debug, Clone, Copy)]
pub struct SchedulerStageEvent {
    /// Actor entity the scheduler is playing on.
    pub actor: Entity,
    /// The crossed stage.
    pub stage: TimedStage,
    /// Scheduler chunk name (for telemetry / debugging).
    pub scheduler: [u8; 4],
}

/// Advance every `ActiveScheduler` and emit one
/// [`SchedulerStageEvent`] per stage crossed this tick. After a
/// timeline finishes plus [`POST_FINISH_TTL_SECS`] grace, the
/// component is removed so the entity stops being scanned.
pub fn tick_active_schedulers(
    time: Res<Time>,
    mut q: Query<(Entity, &mut ActiveScheduler)>,
    mut writer: MessageWriter<SchedulerStageEvent>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut sched) in &mut q {
        sched.elapsed += dt;
        let frame_now = sched.current_frame();

        // Fire every stage whose frame is at or before the current
        // playhead and hasn't been emitted yet. `stages` is sorted by
        // frame so we can stop as soon as we see a future stage.
        let scheduler_name = sched.name;
        while sched.cursor < sched.stages.len() {
            let next = sched.stages[sched.cursor];
            if next.frame > frame_now {
                break;
            }
            writer.write(SchedulerStageEvent {
                actor: entity,
                stage: next,
                scheduler: scheduler_name,
            });
            sched.cursor += 1;
        }

        // Garbage-collect finished timelines after a short grace.
        // Without this, a long-lived actor accumulates one finished
        // ActiveScheduler per action they cast.
        if sched.finished() {
            let finish_secs = sched.last_frame() as f32 / FFXI_FPS;
            if sched.elapsed >= finish_secs + POST_FINISH_TTL_SECS {
                commands.entity(entity).remove::<ActiveScheduler>();
            }
        }
    }
}

/// Resolved chunks for an in-flight action. Populated alongside
/// [`ActiveScheduler`] when an action starts; the dispatch systems
/// look up `stage.id` against these maps when a stage fires.
///
/// `generators`, `d3ms`, and `seps` are keyed by chunk name (the
/// 4-char id from the enclosing DAT header). Names are unique per
/// action DAT, so flat lookup is sufficient — see
/// `ffxi_dat::action::extract_se_schedule` for the established pattern.
#[derive(Component, Debug, Clone, Default)]
pub struct ActionAssets {
    pub generators: HashMap<[u8; 4], Generator>,
    #[cfg(not(target_arch = "wasm32"))]
    pub d3ms: HashMap<[u8; 4], ffxi_dat::d3m::D3m>,
    pub seps: HashMap<[u8; 4], Sep>,
}

/// Walk an action DAT's bytes once and produce its Schedulers +
/// asset bundle. Pure function — callers do their own fs::read /
/// async fetch (native or browser) and pass the bytes here.
///
/// Returns every parseable Scheduler the DAT contains plus a single
/// [`ActionAssets`] map of Generators / D3Ms / Seps. Real action
/// DATs typically ship one Scheduler ("main") plus a small bundle
/// of Generators and their referenced child chunks.
pub fn parse_action_bytes(bytes: &[u8]) -> (Vec<Scheduler>, ActionAssets) {
    let mut schedulers = Vec::new();
    let mut assets = ActionAssets::default();
    for c in walk(bytes).flatten() {
        let Some(kind) = ChunkKind::from_u8(c.kind) else {
            continue;
        };
        match kind {
            ChunkKind::Scheduler => {
                if let Ok(s) = Scheduler::parse(c.name, c.data) {
                    schedulers.push(s);
                }
            }
            ChunkKind::Generator => {
                if let Ok(Some(g)) = Generator::parse(c.name, c.data) {
                    assets.generators.insert(c.name, g);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            ChunkKind::D3m => {
                if let Ok(d) = ffxi_dat::d3m::D3m::parse(c.name, c.data) {
                    assets.d3ms.insert(c.name, d);
                }
            }
            ChunkKind::Sep => {
                if let Ok(s) = Sep::parse(c.name, c.data) {
                    assets.seps.insert(c.name, s);
                }
            }
            _ => {}
        }
    }
    (schedulers, assets)
}

/// Time-to-live (real seconds) before a particle entity despawns.
/// Set from the source stage's `duration_frames`, clamped to a
/// minimum of 0.5s so single-frame stages still register visually.
#[derive(Component, Debug, Clone, Copy)]
pub struct ParticleTtl(pub f32);

impl ParticleTtl {
    pub fn secs(s: f32) -> Self {
        Self(s.max(0.0))
    }
}

/// Map a Generator's `effect_type` byte to a D3M blend mode. Lotus
/// `generator.cppm` exposes many effect types; the type byte alone
/// doesn't disambiguate additive vs blended vs subtractive (that
/// lives in a flags byte we don't yet decode — see TODO below).
/// Default to additive, which matches the most common particle look
/// (flame, magic glyphs, casting motes).
///
/// TODO: once `ffxi_dat::generator::Generator` exposes the blend-mode
/// flags from the creation-command payload (lotus `generator.cppm:139`),
/// branch on it here instead of returning a fixed default.
#[cfg(not(target_arch = "wasm32"))]
pub fn d3m_blend_from_generator(_gen: &Generator) -> crate::dat_d3m::D3mBlendMode {
    crate::dat_d3m::D3mBlendMode::Additive
}

/// Spawn a D3M billboard for every `Particle`-kind stage event whose
/// id resolves through the actor's [`ActionAssets`] map. Each spawn
/// gets an `InGameEntity` marker (lifecycle drain per
/// [[feedback_bevy_lifecycle_symmetry]]) and a [`ParticleTtl`] sized
/// from the stage's `duration_frames`.
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_particle_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    q_actors: Query<(&Transform, &ActionAssets)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    for ev in events.read() {
        if ev.stage.stage.kind != StageKind::Particle {
            continue;
        }
        let Ok((actor_xf, assets)) = q_actors.get(ev.actor) else {
            continue;
        };
        // Stage.id names a Generator; the Generator's `id` names a
        // sibling D3M. Skip silently if either link is missing —
        // unknown stage ids are common in real DATs (some stages
        // reference Generator effect_types we don't render yet).
        let Some(gen) = assets.generators.get(&ev.stage.stage.id) else {
            continue;
        };
        let Some(d3m) = assets.d3ms.get(&gen.id) else {
            continue;
        };
        let blend = d3m_blend_from_generator(gen);
        let mesh_h = meshes.add(crate::dat_d3m::d3m_to_mesh(d3m));
        let mat_h = mats.add(crate::dat_d3m::d3m_material(blend, None));
        let duration = ev.stage.stage.duration_frames as f32 / FFXI_FPS;
        commands.spawn((
            crate::components::InGameEntity,
            ParticleTtl::secs(duration.max(0.5)),
            Mesh3d(mesh_h),
            MeshMaterial3d(mat_h),
            Transform::from_translation(actor_xf.translation),
            Visibility::default(),
            bevy::light::NotShadowCaster,
            bevy::light::NotShadowReceiver,
        ));
    }
}

/// Dispatch SoundOnCaster / SoundOnTarget stages from
/// [`SchedulerStageEvent`]: resolve the stage's id through the actor's
/// [`ActionAssets`] generator/sep tables and write a `SfxEvent` so
/// the existing audio plugin plays the SE.
///
/// Resolution mirrors `ffxi_dat::action::resolve_stage_to_se` — direct
/// Sep reference or indirect Generator→Sep. Stages that resolve to
/// nothing (Generator type isn't Sound, or id is unknown) are skipped
/// silently — action DATs commonly carry stages for other subsystems
/// (motion, model swap) that aren't audio-relevant.
///
/// Why this lives next to the particle dispatcher: both consume the
/// same `SchedulerStageEvent` stream and read the same `ActionAssets`
/// component. Keeping them in one place documents that they're
/// siblings under the same playback model — different stage kinds,
/// same data path.
pub fn dispatch_sound_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    q_actors: Query<&ActionAssets>,
    mut sfx_writer: MessageWriter<crate::audio::SfxEvent>,
) {
    for ev in events.read() {
        // Only sound-kind stages have a Sep to resolve. Particle /
        // Motion / Unknown ignored here (handled elsewhere).
        let kind = ev.stage.stage.kind;
        if !matches!(kind, StageKind::SoundOnCaster | StageKind::SoundOnTarget) {
            continue;
        }
        let Ok(assets) = q_actors.get(ev.actor) else {
            continue;
        };
        // Reuse the shared resolver from ffxi-dat so the indirect
        // Generator→Sep path is identical to the offline schedule
        // walk in `action::extract_se_schedule`.
        let Some((se_id, _on_caster)) = ffxi_dat::action::resolve_stage_to_se(
            &ev.stage.stage.id,
            kind,
            &assets.generators,
            &assets.seps,
        ) else {
            continue;
        };
        // We pass the SE id through unconditionally — the
        // on_caster/on_target distinction would let us 3D-position
        // the sound at the caster's vs target's transform in the
        // future, but `SfxEvent` is currently flat (no positional
        // audio). Plain SE id keeps the dispatch one-line.
        sfx_writer.write(crate::audio::SfxEvent::new(se_id));
    }
}

/// Decay [`ParticleTtl`] each tick; despawn the entity when it hits
/// zero. Pairs with [`dispatch_particle_stages`].
pub fn tick_particle_ttl(
    time: Res<Time>,
    mut q: Query<(Entity, &mut ParticleTtl)>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut ttl) in q.iter_mut() {
        ttl.0 -= dt;
        if ttl.0 <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

/// Resolve an action wire id + kind to the FFXI DAT file_id that
/// carries its Scheduler / Generator / D3M / Sep chunks.
///
/// **Stub.** Returns `None` for every input until a real spell/
/// ability/mob-skill table is wired in. The mapping is a POLUtils-
/// style data table — needs to be sourced from `vendor/server/sql`
/// or a build-time scrape similar to [[altanalistener_music_catalog]].
/// Once that lands, branch on `action_kind` (4=Magic, 6=Ability, …)
/// and look up the per-kind table.
///
/// Why this exists as a stub now: the wire-side `ActionStarted`
/// event already fires every time a 0x028 BATTLE2 lands, so as soon
/// as the table is populated the entire Action → Scheduler →
/// Particle/SE chain will start working end-to-end with zero
/// further plumbing.
pub fn action_dat_file_id(_action_id: u32, _action_kind: u8) -> Option<u32> {
    None
}

/// Consume `ViewerEvent::ActionStarted` events from [`EventLog`],
/// load the action's DAT, parse it, and attach
/// `(ActiveScheduler, ActionAssets)` to the casting actor's Bevy
/// entity. From there the existing dispatch systems (particles,
/// sound) run end-to-end without further wiring.
///
/// Silently no-ops when the actor entity isn't tracked yet (entity
/// hasn't been spawned), when the DAT mapping returns `None`
/// (stub), or when the DAT can't be read (no install path / file
/// missing).
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_action_started(
    events: Res<crate::snapshot::EventLog>,
    tracked: Res<crate::scene::TrackedEntities>,
    mut commands: Commands,
    mut last_seen: Local<usize>,
) {
    // Walk only the *newly-arrived* tail of EventLog.recent — without
    // this, the same event would re-fire every frame for as long as
    // it stays in the ring buffer (capped at 64 entries).
    let total = events.recent.len();
    let new_count = total.saturating_sub(*last_seen);
    *last_seen = total;
    if new_count == 0 {
        return;
    }
    for ev in events.recent.iter().rev().take(new_count).rev() {
        let ffxi_viewer_wire::ViewerEvent::ActionStarted {
            actor_id,
            action_id,
            action_kind,
        } = *ev
        else {
            continue;
        };
        let Some(&actor_entity) = tracked.by_id.get(&actor_id) else {
            continue;
        };
        let Some(file_id) = action_dat_file_id(action_id, action_kind) else {
            // Table not populated yet — drop the event. Once the
            // table lands the whole chain lights up. Don't log here:
            // every action would spam until then.
            continue;
        };
        // Resolve + read the DAT. All errors silently abort — a
        // missing DAT or no install path shouldn't surface as a
        // warning every action.
        let Ok(root) = ffxi_dat::DatRoot::from_env_or_default() else {
            continue;
        };
        let Ok(loc) = root.resolve(file_id) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(root.root())) else {
            continue;
        };
        let (schedulers, assets) = parse_action_bytes(&bytes);
        // Pick the "main" scheduler if present, else first. Real
        // action DATs typically ship one or two; lotus's actor uses
        // whichever was named "main" or the first parseable.
        let scheduler = schedulers
            .iter()
            .find(|s| &s.name == b"main")
            .or_else(|| schedulers.first());
        let Some(scheduler) = scheduler else { continue };
        // `try_insert`: the casting actor can despawn between the queued
        // ActionStarted event and this flush (e.g. a mob that casts and
        // immediately dies). Tolerate that race rather than panic.
        commands
            .entity(actor_entity)
            .try_insert(ActiveScheduler::from_scheduler(scheduler))
            .try_insert(assets);
    }
}

/// Bevy plugin: registers [`SchedulerStageEvent`] and the tick system.
/// Front-ends add this once; per-action components are inserted by
/// whichever subsystem decodes the action DAT and starts playback
/// (E3 sound-dispatch wiring will use the same pattern).
pub struct SchedulerRuntimePlugin;

impl Plugin for SchedulerRuntimePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SchedulerStageEvent>()
            .add_systems(Update, tick_active_schedulers);
        // Particle dispatch + TTL decay + sound dispatch only on
        // native: D3M mesh build uses Assets<Mesh>/Assets<StandardMaterial>
        // and `audio::SfxEvent` itself is native-only. Wasm gets the
        // scheduler tick + event channel but no particle/audio
        // dispatch until DAT-fetch lands for the browser viewer.
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            (
                // Action start: load DAT, build (Scheduler, Assets)
                // for the casting actor. Runs first so the same-frame
                // dispatchers see fresh ActiveSchedulers.
                dispatch_action_started,
                dispatch_particle_stages,
                dispatch_sound_stages,
                tick_particle_ttl,
            )
                .chain(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::scheduler::{SchedulerStage, StageKind};

    fn stage(frame: u32, kind: StageKind, raw_type: u8, id: [u8; 4]) -> TimedStage {
        TimedStage {
            frame,
            stage: SchedulerStage {
                kind,
                raw_type,
                delay_frames: 0,
                duration_frames: 0,
                id,
            },
        }
    }

    fn make_scheduler(name: [u8; 4], stages: Vec<TimedStage>) -> Scheduler {
        Scheduler { name, stages }
    }

    #[test]
    fn current_frame_advances_by_fps() {
        let sched = make_scheduler(*b"main", vec![]);
        let mut a = ActiveScheduler::from_scheduler(&sched);
        a.elapsed = 0.5; // half a real second = 15 FFXI frames
        assert_eq!(a.current_frame(), 15);
        a.elapsed = 1.0;
        assert_eq!(a.current_frame(), 30);
    }

    #[test]
    fn from_scheduler_sorts_by_frame() {
        let sched = make_scheduler(
            *b"main",
            vec![
                stage(60, StageKind::Motion, 0x05, *b"mot0"),
                stage(10, StageKind::SoundOnCaster, 0x53, *b"snd0"),
                stage(30, StageKind::Particle, 0x39, *b"prt0"),
            ],
        );
        let a = ActiveScheduler::from_scheduler(&sched);
        assert_eq!(
            a.stages.iter().map(|t| t.frame).collect::<Vec<_>>(),
            vec![10, 30, 60]
        );
    }

    #[test]
    fn finished_only_after_all_stages_emitted() {
        let sched = make_scheduler(
            *b"main",
            vec![stage(5, StageKind::SoundOnCaster, 0x53, *b"snd0")],
        );
        let mut a = ActiveScheduler::from_scheduler(&sched);
        assert!(!a.finished());
        a.cursor = 1;
        assert!(a.finished());
    }

    #[test]
    fn empty_scheduler_is_immediately_finished() {
        let sched = make_scheduler(*b"main", vec![]);
        let a = ActiveScheduler::from_scheduler(&sched);
        assert!(a.finished());
        assert_eq!(a.last_frame(), 0);
    }

    #[test]
    fn parse_action_bytes_handles_empty_input() {
        let (scheds, assets) = parse_action_bytes(&[]);
        assert!(scheds.is_empty());
        assert!(assets.generators.is_empty());
        assert!(assets.seps.is_empty());
        #[cfg(not(target_arch = "wasm32"))]
        assert!(assets.d3ms.is_empty());
    }

    #[test]
    fn particle_ttl_clamps_negative_to_zero() {
        let t = ParticleTtl::secs(-1.0);
        assert_eq!(t.0, 0.0);
    }
}
