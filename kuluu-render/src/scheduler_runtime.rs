use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use crate::components::{IsSelf, WorldEntity};
#[cfg(not(target_arch = "wasm32"))]
use crate::cutscene_camera::CutsceneCameraTasks;
#[cfg(not(target_arch = "wasm32"))]
use crate::scene::BakedActor;
use bevy::prelude::*;
use ffxi_dat::generator::Generator;
use ffxi_dat::kind::ChunkKind;
use ffxi_dat::scheduler::{ModelVisibility, Scheduler, StageKind, TimedStage};
use ffxi_dat::sep::Sep;
#[cfg(not(target_arch = "wasm32"))]
use ffxi_event::vm::scene::{EVENT_COORD_UNITS, EVENT_HEADING_UNITS, EVENT_SPEED_SCALE};
#[cfg(not(target_arch = "wasm32"))]
use kuluu_snapshot::{CutsceneCue, ExtSchedulerMotion};

// research/xim util/Fps.kt — `internalFps = 60.0` is the clock every effect routine and
// particle generator is authored against (poc/MainTool.kt internalLoop feeds the raw elapsed frames to
// EffectManager). Only the skeleton domain is halved: poc/ActorManager.kt updateAll "In game,
// skeletal animations are only updated every other frame", see SKELETON_FRAME_DIVISOR. DAT stage
// durations count whole frames of this clock; DAT transition fields (CompletionMotion's HalfFrames)
// count half-frames, so a stored V plays as V/2 whole frames at ROUTINE_FPS.
pub const ROUTINE_FPS: f32 = 60.0;

// research/xim poc/ActorManager.kt updateAll — `elapsedFrames / 2f` into updateAnimation.
pub const SKELETON_FRAME_DIVISOR: f32 = 2.0;

// The rate the retail/vanilla client renders at, distinct from the 60 fps routine clock above.
// Anything authored per *rendered* frame — cloud texture-coordinate velocities, the targeted
// nameplate pulse — advances on this one.
pub const RETAIL_FPS: f32 = 30.0;

const POST_FINISH_TTL_SECS: f32 = 2.0;

// The entity the running routines are aimed at. A single entity-level component (first writer
// wins; retail's per-sequence target context is out of scope here), written in the same commands
// chain as `ActiveSchedulers` by every routine dispatcher and stripped with it at the post-finish
// TTL, so a routine never reads a predecessor's target.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ActionTarget(pub Option<Entity>);

// research/xim ParticleGeneratorAttachment.kt updateAssociatedPosition — Target*/TargetToSourceBasis read the
// primary target's position, every other attach type the source actor's. `None` falls back to
// the caster so an untracked target never drops the routine.
pub fn particle_origin_entity(
    attach: ffxi_dat::particle_gen::AttachType,
    caster: Entity,
    target: Option<Entity>,
) -> Entity {
    use ffxi_dat::particle_gen::AttachType;
    match attach {
        AttachType::TargetActor
        | AttachType::TargetActorSourceFacing
        | AttachType::TargetToSourceBasis => target.unwrap_or(caster),
        _ => caster,
    }
}

// ffxi-dat/src/action.rs::resolve_stage_to_se yields `on_caster` straight from the stage kind:
// a 0x0A/0x53 SoundOnCaster emits at the source actor, a 0x0B SoundOnTarget at the primary
// target. `None` falls back to the caster so an untracked target never silences the SE.
pub fn sound_origin_entity(on_caster: bool, caster: Entity, target: Option<Entity>) -> Entity {
    if on_caster {
        caster
    } else {
        target.unwrap_or(caster)
    }
}

// research/xim poc/MainTool.kt resourceDependenciesLoaded systemEffects — ROM/0/0.DAT is loaded as XIM's `GlobalDirectory`, the
// system-effect resource dir every routine falls back to (the cast aura `ner1` and its `stbk`
// stop live there, not in the caster's DAT). `DatRoot::resolve(0)` yields exactly that file.
pub const GLOBAL_EFFECT_DIR_FILE_ID: u32 = 0;

#[derive(Resource, Default)]
pub struct GlobalEffectDir {
    pub schedulers: Vec<Scheduler>,
    pub assets: ActionAssets,
}

// research/xim EffectRoutineInstance.kt appendChildSequences findResource — a routine id resolves against the
// routine's own DAT, then the actor's own dirs, then the global dir.
pub enum RoutineSource<'a> {
    Dat(&'a [Scheduler]),
    Actor(&'a HashMap<ffxi_dat::datid::DatId, Scheduler>),
}

#[derive(Default)]
pub struct RoutineLookup<'a> {
    tiers: Vec<RoutineSource<'a>>,
}

impl<'a> RoutineLookup<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_dat(mut self, schedulers: &'a [Scheduler]) -> Self {
        self.tiers.push(RoutineSource::Dat(schedulers));
        self
    }

    pub fn with_actor(mut self, routines: &'a HashMap<ffxi_dat::datid::DatId, Scheduler>) -> Self {
        self.tiers.push(RoutineSource::Actor(routines));
        self
    }

    pub fn get(&self, name: &[u8; 4]) -> Option<&'a Scheduler> {
        self.tiers.iter().find_map(|tier| match tier {
            RoutineSource::Dat(list) => list.iter().find(|s| &s.name == name),
            RoutineSource::Actor(map) => map.get(&ffxi_dat::datid::DatId::from_name(name)),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionStages {
    Play,

    // The caster's looping cast pose is owned by ffxi_actor_render::dispatch_action_overlay; a
    // cast routine's own Motion stage would replace it with a one-shot.
    Suppress,
}

// One running routine. Not a component on its own anymore: an entity can run several routines at
// once - retail runs a hit reaction alongside the swing that caused it (ActionTimer1 counted 2 and
// 3) - so the runtime keeps them in `ActiveSchedulers`, one entry per routine with its own
// frame clock.
#[derive(Debug, Clone)]
pub struct ActiveScheduler {
    pub stages: Vec<TimedStage>,

    pub elapsed: f32,

    pub cursor: usize,

    pub name: [u8; 4],

    /// The cutscene motion actor this routine was started for, as the wire value the
    /// cue named (the session matches the report against the same value it resolved
    /// the cue with): the SCHEDULOR and the file-routine motions (LOADEVENTSCHEDULER2
    /// non-fade, LOADEXTSCHEDULER, MAPSCHEDULOR; ffxi-event/src/cue.rs). `None` for
    /// routines no WAIT* hold waits on, so they do not report.
    pub cutscene_motion_actor: Option<kuluu_snapshot::CutsceneActor>,

    /// Set once the finish report went out: the routine lingers past its last
    /// stage for the post-finish TTL, so the report fires exactly once.
    pub done_reported: bool,
}

impl ActiveScheduler {
    pub fn from_scheduler(s: &Scheduler) -> Self {
        let mut stages = s.stages.clone();
        stages.sort_by_key(|t| t.frame);
        Self {
            stages,
            elapsed: 0.0,
            cursor: 0,
            name: s.name,
            cutscene_motion_actor: None,
            done_reported: false,
        }
    }

    // A retail effect routine's "main" scheduler delegates to sub-routines via 0x03 stages
    // (id = sub-scheduler name) — e.g. Cure's main calls tgt0, which holds the particle
    // spawns. Inline them at their call frame into one flat timeline.
    pub fn from_main(schedulers: &[Scheduler], name: &[u8; 4]) -> Option<Self> {
        Self::from_routine(&RoutineLookup::new().with_dat(schedulers), name)
    }

    pub fn from_routine(lookup: &RoutineLookup, name: &[u8; 4]) -> Option<Self> {
        Self::flatten(lookup, name, MotionStages::Play)
    }

    pub fn effects_only(lookup: &RoutineLookup, name: &[u8; 4]) -> Option<Self> {
        Self::flatten(lookup, name, MotionStages::Suppress)
    }

    // research/xim Actor.kt displayAutoAttack — one swing enqueues TWO routines on the attacker: the
    // self-targeted voice routine (`atk0`) and the weapon swing (`ati0`/`bti0`/…). Their timelines
    // are merged into one entry so the swing's DamageCallback reports a single scheduler
    // name - the value PendingHitReaction arms with.
    pub fn effects_only_merged(lookup: &RoutineLookup, names: &[[u8; 4]]) -> Option<Self> {
        let first = *names.iter().find(|n| lookup.get(n).is_some())?;
        let mut stages = Vec::new();
        for name in names {
            let mut path = Vec::new();
            flatten_routine(
                lookup,
                name,
                0,
                MotionStages::Suppress,
                &mut path,
                &mut stages,
            );
        }
        stages.sort_by_key(|t| t.frame);
        Some(Self {
            stages,
            elapsed: 0.0,
            cursor: 0,
            name: first,
            cutscene_motion_actor: None,
            done_reported: false,
        })
    }

    fn flatten(lookup: &RoutineLookup, name: &[u8; 4], motion: MotionStages) -> Option<Self> {
        lookup.get(name)?;
        let mut stages = Vec::new();
        let mut path = Vec::new();
        flatten_routine(lookup, name, 0, motion, &mut path, &mut stages);
        stages.sort_by_key(|t| t.frame);
        Some(Self {
            stages,
            elapsed: 0.0,
            cursor: 0,
            name: *name,
            cutscene_motion_actor: None,
            done_reported: false,
        })
    }

    pub fn name(&self) -> [u8; 4] {
        self.name
    }

    pub fn finished(&self) -> bool {
        self.cursor >= self.stages.len()
    }

    pub fn current_frame(&self) -> u32 {
        (self.elapsed * ROUTINE_FPS) as u32
    }

    /// True while this routine's AnimationLock interval covers `frame`: any AnimationLock stage with
    /// `stage.frame <= frame < stage.frame + duration_frames`. A routine with no
    /// lock stage locks nothing.
    pub fn locks_at(&self, frame: u32) -> bool {
        self.stages.iter().any(|t| {
            t.stage.kind == StageKind::AnimationLock
                && t.frame <= frame
                && frame < t.frame + t.stage.duration_frames as u32
        })
    }

    /// The MovementLock twin of [`Self::locks_at`]: true while a 0x2E interval covers `frame`.
    /// Parsed as data; players are never movement-locked by a routine (record:
    /// .agents/skills/retail-observe/references/2026-09-21-action-confirm-and-locks.md,
    /// "Players are never movement-locked by a cast or a ranged aim").
    pub fn movement_locks_at(&self, frame: u32) -> bool {
        self.stages.iter().any(|t| {
            t.stage.kind == StageKind::MovementLock
                && t.frame <= frame
                && frame < t.frame + t.stage.duration_frames as u32
        })
    }

    /// The 0x75 SetModelVisibility overrides live at `frame`
    /// (`stage.frame <= frame < stage.frame + duration_frames`), in timeline order. The render
    /// layer folds them into the actor's hidden-slot set (research/xim EffectRoutineInstance.kt
    /// handleSetModelVisibilityRoutine: each stage sets one model slot's visibility for the
    /// stage's duration).
    pub fn for_each_visibility_override_at(&self, frame: u32, mut f: impl FnMut(&ModelVisibility)) {
        for t in &self.stages {
            if t.stage.kind == StageKind::SetModelVisibility
                && t.frame <= frame
                && frame < t.frame + t.stage.duration_frames as u32
            {
                if let Some(mv) = t.stage.model_visibility {
                    f(&mv);
                }
            }
        }
    }

    /// The routine timeline ends when its last stage ends, not when it starts: a trailing
    /// AnimationLock must keep the routine alive for its whole `duration_frames`.
    pub fn last_frame(&self) -> u32 {
        self.end_frame()
    }

    /// The frame at which this routine's effects end: the max over all stages of
    /// `stage.frame + stage.duration_frames`, a half-open bound like `locks_at`'s. A plain
    /// stage ends on its own fire frame; an AnimationLock keeps holding until
    /// `frame + duration_frames`, so retiring on the last stage's fire time would drop a long
    /// lock early (the post-finish TTL then counts from the wrong start). Measured over this
    /// entry's flattened stages, so inlined sub-routine calls count toward it.
    pub fn end_frame(&self) -> u32 {
        Scheduler::end_frame_for(&self.stages)
    }
}

/// The routines an entity is running right now - one entry per routine. Retail's
/// AnimationLock is a refcount across overlapping routines, so the lock test is "any entry holds
/// its interval at its own current frame" rather than a separate counter. Stripped with
/// `ActionAssets`/`ActionTarget` when the last entry finishes.
#[derive(Component, Debug, Clone, Default)]
pub struct ActiveSchedulers {
    routines: Vec<ActiveScheduler>,
}

impl ActiveSchedulers {
    pub fn one(active: ActiveScheduler) -> Self {
        Self {
            routines: vec![active],
        }
    }

    /// Multiple queued routines at once - the flush path for a batch of inserts that landed on
    /// an entity with no ActiveSchedulers yet (see `run_routine_on`'s pending-insert buffer):
    /// one component holding every routine instead of N deferred inserts where the last would
    /// have overwritten the rest.
    pub fn many(entries: Vec<ActiveScheduler>) -> Self {
        Self { routines: entries }
    }

    /// Enqueue a routine alongside the running ones instead of replacing them.
    pub fn push(&mut self, active: ActiveScheduler) {
        self.routines.push(active);
    }

    /// True while any entry's AnimationLock interval covers its own current frame - the refcount>0
    /// test itself (ActionTimer1 reached 2 and 3 when a hit reaction overlapped a swing).
    pub fn is_locked_now(&self) -> bool {
        self.routines.iter().any(|r| r.locks_at(r.current_frame()))
    }

    /// The hidden model-slot set this entity's running routines produce at their own current
    /// frame: the ranged slot (2) starts hidden - retail's default (research/xim ActorModel.kt
    /// getHiddenSlotIds: "Ranged is hidden by default") - then each routine's active 0x75
    /// overrides apply in queue order, the last one on a slot winning. An override with
    /// `if_engaged` only applies while the actor is engaged (research/xim ActorModel.kt
    /// getHiddenSlotIds ifEngaged gate); slots outside 0..5 are ignored (research/xim
    /// ActorModel.kt getHiddenModelSlots).
    pub fn hidden_model_slots_now(&self, engaged: bool) -> [bool; 5] {
        let mut hidden = [false, false, true, false, false];
        for r in &self.routines {
            r.for_each_visibility_override_at(r.current_frame(), |mv| {
                if mv.if_engaged && !engaged {
                    return;
                }
                if (mv.slot as usize) < hidden.len() {
                    hidden[mv.slot as usize] = mv.hidden;
                }
            });
        }
        hidden
    }

    /// StopRoutine: drop every entry named `name`. xim stops each matching sequence on the
    /// same actor (EffectRoutineInstance.kt handleStopRoutineEffect); stop() just clears the remaining queue - it
    /// does not run the stopped routine's StopParticle stages, so no particle
    /// cleanup happens here either.
    pub fn remove_routine_named(&mut self, name: &[u8; 4]) {
        self.routines.retain(|r| r.name != *name);
    }

    /// 0x5E/0x6B stop action with no tag: clear the whole queue so the actor's pose
    /// falls back to its idle path (research/XiEvents/OpCodes/0x005E.md).
    pub fn stop_all(&mut self) {
        self.routines.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.routines.is_empty()
    }

    /// Read-only view of the queued routine names - the offline harnesses (and the rabbit
    /// tester's integration test) assert which reaction a victim actually got without needing
    /// to reach into the private vec.
    pub fn routine_names(&self) -> impl Iterator<Item = [u8; 4]> + '_ {
        self.routines.iter().map(|r| r.name)
    }

    /// True while an entry named `dead` has a Motion stage in its timeline but none has fired
    /// yet (the cursor has not passed the first one): the gap between the Defeated latch and
    /// the ded? fall-over starting. The pose pass holds idle across that window instead of
    /// flashing cor?. A `dead` routine with no Motion stage reports nothing.
    pub fn dead_fall_over_pending(&self) -> bool {
        self.routines.iter().any(|r| {
            r.name == *b"dead"
                && r.stages
                    .iter()
                    .position(|t| t.stage.kind == StageKind::Motion)
                    .is_some_and(|i| r.cursor <= i)
        })
    }
}

#[derive(Message, Debug, Clone, Copy)]
pub struct SchedulerStageEvent {
    pub actor: Entity,

    pub stage: TimedStage,

    pub scheduler: [u8; 4],
}

/// A cutscene motion routine the cue `(actor, key)` named has finished - or
/// could not be started at all: the host releases the event VM's pending hold
/// on the pair so the WAIT* past it advances. The actor is the wire value the
/// cue named, the same one the session resolved it against.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutsceneMotionDone {
    pub actor: kuluu_snapshot::CutsceneActor,

    pub key: [u8; 4],
}

pub fn tick_active_schedulers(
    time: Res<Time>,
    mut q: Query<(Entity, &mut ActiveSchedulers)>,
    mut writer: MessageWriter<SchedulerStageEvent>,
    mut motion_done: MessageWriter<CutsceneMotionDone>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut scheds) in &mut q {
        for sched in &mut scheds.routines {
            sched.elapsed += dt;
            let frame_now = sched.current_frame();

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
            if !sched.done_reported && sched.finished() {
                if let Some(actor) = sched.cutscene_motion_actor {
                    motion_done.write(CutsceneMotionDone {
                        actor,
                        key: scheduler_name,
                    });
                    sched.done_reported = true;
                }
            }
        }

        scheds.routines.retain(|sched| {
            if !sched.finished() {
                return true;
            }
            let finish_secs = sched.end_frame() as f32 / ROUTINE_FPS;
            sched.elapsed < finish_secs + POST_FINISH_TTL_SECS
        });
        if scheds.routines.is_empty() {
            commands
                .entity(entity)
                .remove::<(ActiveSchedulers, ActionAssets, ActionTarget)>();
        }
    }
}

// 0x5F StopRoutine - the worm's dig (`ini1`) stops `init` and its pop-up stops `ini1` this way.
// xim stops every sequence named by the stage on the same actor; here that is a plain removal
// from the vec. The stopped routine's remaining stages simply never fire - including any 0x2D
// StopParticle, which retail does not run for a stopped sequence either (research/xim
// EffectRoutineInstance.kt EffectSequence.stop).
pub fn dispatch_stop_routine_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    mut q: Query<&mut ActiveSchedulers>,
) {
    for ev in events.read() {
        if ev.stage.stage.kind != StageKind::StopRoutine {
            continue;
        }
        let Ok(mut scheds) = q.get_mut(ev.actor) else {
            continue;
        };
        scheds.remove_routine_named(&ev.stage.stage.id);
    }
}

// A zone-spray generator (e.g. Bastok "abuk", Port Windurst "rivsea") links an MMB
// mesh by its 4-byte DatId, not a D3M. Flattened here to sprite geometry so the
// particle sim can build a SpriteTemplate without re-parsing the MMB.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Default)]
pub struct MmbSpriteMesh {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    // Stage 0's D argument per `positions` entry, /128-normalised like a D3m vertex colour so
    // both particle mesh sources feed `SpriteTemplate::colors` on the same scale.
    pub colors: Vec<[f32; 4]>,
    pub texture_name: String,
}

#[derive(Component, Debug, Clone, Default)]
pub struct ActionAssets {
    pub generators: HashMap<[u8; 4], Generator>,
    #[cfg(not(target_arch = "wasm32"))]
    pub d3ms: HashMap<[u8; 4], ffxi_dat::d3m::D3m>,
    /// The same meshes keyed by (containing directory, name), the tier a generator's linked
    /// mesh resolves against before the flat, last-writer-wins map.
    #[cfg(not(target_arch = "wasm32"))]
    pub d3ms_by_dir: HashMap<([u8; 4], [u8; 4]), ffxi_dat::d3m::D3m>,
    #[cfg(not(target_arch = "wasm32"))]
    pub mmbs: HashMap<[u8; 4], MmbSpriteMesh>,
    // SpriteSheet (0x0E) particle meshes, keyed by the 0x21 chunk DatId a generator's
    // mesh_id references (e.g. Poison's `fir ` → 0x21 `fir`).
    #[cfg(not(target_arch = "wasm32"))]
    pub sprite_sheets: HashMap<[u8; 4], ffxi_dat::sprite_sheet::ParticleSpriteSheet>,
    #[cfg(not(target_arch = "wasm32"))]
    pub sprite_sheets_by_dir:
        HashMap<([u8; 4], [u8; 4]), ffxi_dat::sprite_sheet::ParticleSpriteSheet>,
    pub seps: HashMap<[u8; 4], Sep>,
    pub animations: Vec<ffxi_dat::skel_anim::SkeletonAnimation>,
    #[cfg(not(target_arch = "wasm32"))]
    pub images: HashMap<[u8; 4], ffxi_dat::texture::DecodedTexture>,
    // Img chunks keyed by their INTERNAL name (bytes 0x09..0x11), which is what an
    // MMB model's texture_name references — distinct from the Img chunk's DatId.
    #[cfg(not(target_arch = "wasm32"))]
    pub images_by_name: HashMap<String, ffxi_dat::texture::DecodedTexture>,
    // Img chunks keyed by their fully qualified (namespace, local) name pair — the tier a
    // 0x21 sprite sheet's own 16-byte name field resolves against.
    #[cfg(not(target_arch = "wasm32"))]
    pub images_by_qualified_name: HashMap<(String, String), ffxi_dat::texture::DecodedTexture>,
    pub emitters: HashMap<[u8; 4], ffxi_dat::generator::ParticleEmitter>,
    pub particle_defs: HashMap<[u8; 4], ffxi_dat::particle_gen::ParticleGeneratorDef>,
    /// Generators whose setup links a Sep instead of a mesh (the Home Point's `snd0` ambient
    /// loop).
    pub sound_defs: HashMap<[u8; 4], ffxi_dat::particle_gen::SoundGeneratorDef>,
    // The same defs keyed by (containing directory, name). ROM/0/0.DAT defines four different
    // generators called `g010`, one per effect directory; the flat map keeps only the last.
    pub particle_defs_by_dir:
        HashMap<([u8; 4], [u8; 4]), ffxi_dat::particle_gen::ParticleGeneratorDef>,
    /// The directory each entry of the flat `particle_defs` map came from, so a def that only
    /// resolves through that last-writer-wins tier still knows the scope its own linked mesh,
    /// sprite sheet and texture resolve in.
    pub particle_def_dirs: HashMap<[u8; 4], [u8; 4]>,
    pub keyframes: HashMap<[u8; 4], ffxi_dat::particle_gen::KeyFrameTrack>,
}

impl ActionAssets {
    // research/xim EffectRoutineInstance.kt appendChildSequences — `resource.localDir` first, wider scopes
    // after. `local_dir` is the directory of the routine the stage was authored in, carried on
    // the stage because flattening merges routines from several directories into one timeline.
    pub fn particle_def(
        &self,
        local_dir: [u8; 4],
        id: &[u8; 4],
    ) -> Option<&ffxi_dat::particle_gen::ParticleGeneratorDef> {
        self.particle_def_scoped(local_dir, id).map(|(_, d)| d)
    }

    // research/xim ParticleInitializers.kt apply — a generator's linked mesh resolves against
    // `particle.creator.localDir`, the directory the GENERATOR was authored in, which is the
    // caller's routine dir only when the def resolved through the dir-scoped tier.
    pub fn particle_def_scoped(
        &self,
        local_dir: [u8; 4],
        id: &[u8; 4],
    ) -> Option<([u8; 4], &ffxi_dat::particle_gen::ParticleGeneratorDef)> {
        if let Some(def) = self.particle_defs_by_dir.get(&(local_dir, *id)) {
            return Some((local_dir, def));
        }
        let def = self.particle_defs.get(id)?;
        let dir = self
            .particle_def_dirs
            .get(id)
            .copied()
            .unwrap_or(ffxi_dat::scheduler::NO_LOCAL_DIR);
        Some((dir, def))
    }

    // research/xim ParticleLinkedDataProviders.kt getParticleMesh resolveStaticMeshLink — the effect
    // directory first, wider scopes after.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn d3m(&self, local_dir: [u8; 4], id: &[u8; 4]) -> Option<&ffxi_dat::d3m::D3m> {
        self.d3ms_by_dir
            .get(&(local_dir, *id))
            .or_else(|| self.d3ms.get(id))
    }

    // research/xim ParticleLinkedDataProviders.kt resolveStaticMeshLink resolveSpriteSheetLink — same order.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn sprite_sheet(
        &self,
        local_dir: [u8; 4],
        id: &[u8; 4],
    ) -> Option<&ffxi_dat::sprite_sheet::ParticleSpriteSheet> {
        self.sprite_sheets_by_dir
            .get(&(local_dir, *id))
            .or_else(|| self.sprite_sheets.get(id))
    }
}

const MAX_SUBROUTINE_DEPTH: usize = 6;

/// The global effect dir's `dada` is the swing impact carrier: every melee swing calls it at its
/// impact frame, and it holds the DamageCallback that hands off to the victim reaction.
/// When flattening cannot inline it (the global dir degraded to empty), the CALL survives as a
/// marker stage so `dispatch_damage_callback_stages` still fires on that frame.
const DADA_IMPACT_MARKER: [u8; 4] = *b"dada";

// Knuth's MMIX LCG. Every DAT-driven choice the format leaves unauthored (random routine
// branches, particle spawn spread, sound-emitter jitter) advances this same recurrence, so the
// pair lives here once rather than being retyped per consumer.
pub const LCG_MULTIPLIER: u64 = 6364136223846793005;
pub const LCG_INCREMENT: u64 = 1442695040888963407;

/// splitmix64's increment (Steele et al., "Fast Splittable Pseudorandom Number
/// Generators"): the shared deterministic seed multiplier for the pseudo-random
/// spreads each system derives on its own (per-owner particle and sfx seeds,
/// lightning jitter, launcher vantage sequence).
pub const SPLITMIX64_GOLDEN_RATIO: u64 = 0x9E37_79B9_7F4A_7C15;

pub fn lcg_next(state: u64) -> u64 {
    state
        .wrapping_mul(LCG_MULTIPLIER)
        .wrapping_add(LCG_INCREMENT)
}

// research/xim EffectRoutineParser.kt parseSection2 — a random block runs exactly one of its children
// per activation, and which one is not authored in the DAT.
static RANDOM_PICK_STATE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_random_pick(len: usize) -> usize {
    use std::sync::atomic::Ordering;
    let next = RANDOM_PICK_STATE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |s| Some(lcg_next(s)))
        .unwrap_or(1);
    if len == 0 {
        0
    } else {
        ((next >> 33) as usize) % len
    }
}

fn flatten_routine(
    lookup: &RoutineLookup,
    name: &[u8; 4],
    base_frame: u32,
    motion: MotionStages,
    path: &mut Vec<[u8; 4]>,
    out: &mut Vec<TimedStage>,
) {
    if path.len() > MAX_SUBROUTINE_DEPTH || path.contains(name) {
        return;
    }
    let Some(s) = lookup.get(name) else {
        return;
    };
    let mut chosen: HashMap<u16, usize> = HashMap::new();
    let mut seen_in_group: HashMap<u16, usize> = HashMap::new();
    for t in &s.stages {
        if let Some(g) = t.stage.random_group {
            *seen_in_group.entry(g).or_insert(0) += 1;
        }
    }
    for (&g, &count) in &seen_in_group {
        chosen.insert(g, next_random_pick(count));
    }
    let mut index_in_group: HashMap<u16, usize> = HashMap::new();

    path.push(*name);
    for t in &s.stages {
        if let Some(g) = t.stage.random_group {
            let i = index_in_group.entry(g).or_insert(0);
            let is_pick = chosen.get(&g) == Some(&*i);
            *i += 1;
            if !is_pick {
                continue;
            }
        }
        let frame = base_frame + t.frame;
        match t.stage.kind {
            StageKind::SubRoutine | StageKind::BlockingSubRoutine => {
                // A control-flow routine is a switch (ffxi-dat/src/scheduler.rs
                // has_control_flow): inlining it whole would run every branch, so the CALL
                // survives flattening as a marker stage - `dada`, the swing's impact carrier,
                // tail-calls such switches, and with a degraded global dir its DamageCallback
                // would otherwise vanish from the timeline entirely.
                match lookup.get(&t.stage.id) {
                    Some(c) if c.has_control_flow() => out.push(TimedStage {
                        frame,
                        stage: t.stage,
                    }),
                    None if t.stage.id == DADA_IMPACT_MARKER => out.push(TimedStage {
                        frame,
                        stage: t.stage,
                    }),
                    _ => flatten_routine(lookup, &t.stage.id, frame, motion, path, out),
                }
            }
            StageKind::Motion if motion == MotionStages::Suppress => {}
            _ => out.push(TimedStage {
                frame,
                stage: t.stage,
            }),
        }
    }
    path.pop();
}

// A generator and the mesh/sheet/texture it references always ship in the same DAT, so a stage
// resolves against whichever single ActionAssets actually holds it, in retail's search order
// (research/xim EffectRoutineInstance.kt appendChildSequences): the routine DAT's own assets on
// the tracked entity, then the actor model's, then the global effect dir's. The Home Point's
// `bind` routine is an actor-tier case: its `tub0` is also a global-dir name, so skipping the
// actor tier drew ROM/0/0's hit spark in place of the crystal's `tubu` card.
pub fn assets_holding<'a>(
    local: Option<&'a ActionAssets>,
    actor: Option<&'a ActionAssets>,
    global: Option<&'a ActionAssets>,
    has: impl Fn(&ActionAssets) -> bool,
) -> Option<&'a ActionAssets> {
    [local, actor, global]
        .into_iter()
        .flatten()
        .find(|a| has(a))
}

// A routine and the generators it names share a chunk directory, and those names are only unique
// within it, so the walk carries the enclosing directory alongside each chunk.
fn walk_with_dirs(
    node: &ffxi_dat::chunk::ChunkNode<'_>,
    visit: &mut dyn FnMut([u8; 4], &ffxi_dat::chunk::Chunk<'_>),
) {
    fn rec<'a>(
        node: &ffxi_dat::chunk::ChunkNode<'a>,
        dir: [u8; 4],
        visit: &mut dyn FnMut([u8; 4], &ffxi_dat::chunk::Chunk<'a>),
    ) {
        let dir = if node.chunk.kind == ChunkKind::Rmp as u8 {
            node.chunk.name
        } else {
            visit(dir, &node.chunk);
            dir
        };
        for child in &node.children {
            rec(child, dir, visit);
        }
    }
    rec(node, ffxi_dat::scheduler::NO_LOCAL_DIR, visit);
}

/// The kind 0x06 camera routes of one action DAT, keyed by four-char name; a scheduler's
/// CameraRoute stage names one of these (research/XIClient Game/Scheduler/Tags/0x04.cpp).
pub type ActionDatCameras = HashMap<[u8; 4], ffxi_dat::camera::CameraResource>;

pub fn parse_action_bytes(bytes: &[u8]) -> (Vec<Scheduler>, ActionAssets, ActionDatCameras) {
    let (schedulers, assets, _report, cameras) = parse_action_bytes_reporting(bytes);
    (schedulers, assets, cameras)
}

pub fn parse_action_bytes_reporting(
    bytes: &[u8],
) -> (
    Vec<Scheduler>,
    ActionAssets,
    EffectCoverageReport,
    ActionDatCameras,
) {
    parse_action_tree_reporting(&ffxi_dat::chunk::walk_tree(bytes))
}

/// What one effect-DAT parse understood nothing of: scheduler stages whose opcode reaches no
/// `StageKind` arm, and generator blocks no section arm decoded. Both are silent degradations —
/// the routine still runs, missing whatever the instruction said — so they are counted rather
/// than dropped.
#[derive(Default, Debug, Clone)]
pub struct EffectCoverageReport {
    /// (routine name, raw opcode, stage length in dwords)
    pub unknown_stages: Vec<([u8; 4], u8, u8)>,
    /// (generator chunk name, section, opcode, outcome) for every generator block walked.
    pub generator_opcodes: Vec<(
        [u8; 4],
        ffxi_dat::particle_gen::GeneratorSection,
        u8,
        ffxi_dat::particle_gen::GeneratorOpcodeOutcome,
    )>,
}

impl EffectCoverageReport {
    pub fn dropped_generator_opcodes(
        &self,
    ) -> impl Iterator<Item = ([u8; 4], ffxi_dat::particle_gen::GeneratorSection, u8)> + '_ {
        self.generator_opcodes
            .iter()
            .filter(|(.., outcome)| {
                *outcome == ffxi_dat::particle_gen::GeneratorOpcodeOutcome::Dropped
            })
            .map(|&(name, section, opcode, _)| (name, section, opcode))
    }
}

// Chunk ids are only unique within a directory, and a zone DAT repeats them across weat/ subtrees
// (zone 123 carries `clod` and `hm01..hm15` under both weat/rain and weat/squl), so a consumer
// that owns one subtree must build its assets from that subtree alone or it binds the wrong
// mesh/texture/keyframe.
pub fn parse_action_tree(
    node: &ffxi_dat::chunk::ChunkNode<'_>,
) -> (Vec<Scheduler>, ActionAssets, ActionDatCameras) {
    let (schedulers, assets, _report, cameras) = parse_action_tree_reporting(node);
    (schedulers, assets, cameras)
}

pub fn parse_action_tree_reporting(
    node: &ffxi_dat::chunk::ChunkNode<'_>,
) -> (
    Vec<Scheduler>,
    ActionAssets,
    EffectCoverageReport,
    ActionDatCameras,
) {
    let mut schedulers = Vec::new();
    let mut assets = ActionAssets::default();
    let mut report = EffectCoverageReport::default();
    let mut cameras = ActionDatCameras::new();
    walk_with_dirs(node, &mut |dir, c| {
        let Some(kind) = ChunkKind::from_u8(c.kind) else {
            return;
        };
        match kind {
            ChunkKind::Scheduler => {
                if let Ok(s) = Scheduler::parse_in_dir(dir, c.name, c.data) {
                    report.unknown_stages.extend(
                        s.stages
                            .iter()
                            .map(|t| t.stage)
                            .filter(|st| {
                                st.kind == StageKind::Unknown
                                    && !ffxi_dat::scheduler::is_structural_opcode(st.raw_type)
                            })
                            .map(|st| (s.name, st.raw_type, st.stage_words)),
                    );
                    schedulers.push(s);
                }
            }
            ChunkKind::Generator => {
                let mut sink = |section, opcode, outcome| {
                    report
                        .generator_opcodes
                        .push((c.name, section, opcode, outcome));
                };
                if let Ok(Some(g)) = Generator::parse(c.name, c.data) {
                    assets.generators.insert(c.name, g);
                }
                if let Ok(Some(e)) = Generator::parse_particle_emitter(c.data) {
                    assets.emitters.insert(c.name, e);
                }
                if let Ok(Some(d)) =
                    ffxi_dat::particle_gen::ParticleGeneratorDef::parse_reporting(c.data, &mut sink)
                {
                    assets.particle_defs.insert(c.name, d);
                    assets.particle_def_dirs.insert(c.name, dir);
                    assets.particle_defs_by_dir.insert((dir, c.name), d);
                }
                if let Ok(Some(d)) =
                    ffxi_dat::particle_gen::SoundGeneratorDef::parse_reporting(c.data, &mut sink)
                {
                    assets.sound_defs.insert(c.name, d);
                }
            }
            ChunkKind::KeyFrame => {
                assets
                    .keyframes
                    .insert(c.name, ffxi_dat::particle_gen::KeyFrameTrack::parse(c.data));
            }
            #[cfg(not(target_arch = "wasm32"))]
            ChunkKind::D3m => {
                if let Ok(d) = ffxi_dat::d3m::D3m::parse(c.name, c.data) {
                    assets.d3ms_by_dir.insert((dir, c.name), d.clone());
                    assets.d3ms.insert(c.name, d);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            ChunkKind::Mmb => {
                if let Some(mesh) = mmb_sprite_mesh(c.data) {
                    assets.mmbs.insert(c.name, mesh);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            ChunkKind::SpriteSheet => {
                if let Some(ss) = ffxi_dat::sprite_sheet::ParticleSpriteSheet::parse(c.data) {
                    assets
                        .sprite_sheets_by_dir
                        .insert((dir, c.name), ss.clone());
                    assets.sprite_sheets.insert(c.name, ss);
                }
            }
            ChunkKind::Sep => {
                if let Ok(s) = Sep::parse(c.name, c.data) {
                    assets.seps.insert(c.name, s);
                }
            }
            // research/XIClient include/World/Camera/CameraFormat.h - the camera route a
            // scheduler's 0x04 stage drives; keyed by name like every other chunk here.
            ChunkKind::Camera => {
                if let Ok(cam) = ffxi_dat::camera::CameraResource::parse(c.name, c.data) {
                    cameras.insert(c.name, cam);
                }
            }
            ChunkKind::AnimMo2 => {
                let id = ffxi_dat::datid::DatId::from_name(&c.name);
                assets
                    .animations
                    .push(ffxi_dat::skel_anim::parse(id, c.data));
            }
            #[cfg(not(target_arch = "wasm32"))]
            ChunkKind::Img => {
                if let Ok(tex) = ffxi_dat::texture::decode_texture(c.data) {
                    if let Some((category, id)) = ffxi_dat::texture::extract_texture_tokens(c.data)
                    {
                        assets
                            .images_by_qualified_name
                            .insert((category, id.clone()), tex.clone());
                        assets.images_by_name.insert(id, tex.clone());
                    }
                    assets.images.insert(c.name, tex);
                }
            }
            _ => {}
        }
    });
    (schedulers, assets, report, cameras)
}

#[cfg(not(target_arch = "wasm32"))]
fn mmb_sprite_mesh(data: &[u8]) -> Option<MmbSpriteMesh> {
    let dec = ffxi_dat::mmb::decrypt(data).ok()?;
    let models = ffxi_dat::mmb::parse_models(&dec);
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut colors = Vec::new();
    let mut indices = Vec::new();
    let mut texture_name = String::new();
    for m in &models {
        if m.vertices.is_empty() || m.indices.is_empty() {
            continue;
        }
        if texture_name.is_empty() && !m.texture_name.is_empty() {
            texture_name = m.texture_name.clone();
        }
        let base = positions.len() as u32;
        let vert_count = m.vertices.len() as u16;
        for v in &m.vertices {
            positions.push(v.pos);
            uvs.push(v.uv);
            colors.push(
                v.rgba
                    .map(|c| c as f32 / ffxi_dat::d3m::VERTEX_COLOR_DIVISOR),
            );
        }
        for tri in m.indices.chunks_exact(3) {
            if tri.iter().all(|&i| i < vert_count) {
                indices.extend(tri.iter().map(|&i| base + i as u32));
            }
        }
    }
    if positions.is_empty() || indices.is_empty() {
        return None;
    }
    Some(MmbSpriteMesh {
        positions,
        uvs,
        colors,
        indices,
        texture_name,
    })
}

/// Every action/emote DAT read resolves through one shared root: `DatRoot::open` re-reads and
/// re-parses all 20 VTABLE/FTABLE files (3.3 MB on a retail install), so opening one per event or
/// per cache miss is pure repeat work. Wired by kuluu's `insert_dat_roots` like every other
/// `*DatRoot`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default, Clone)]
pub struct ActionDatRoot(pub Option<Arc<ffxi_dat::DatRoot>>);

/// A `None` root is the host saying it has no install (kuluu wires one either way), so there is
/// deliberately no env re-open here: the wired root carries the launcher's overlays and DAT-path
/// setting, and a root opened behind the host's back would not.
#[cfg(not(target_arch = "wasm32"))]
fn read_dat_bytes(root: Option<Arc<ffxi_dat::DatRoot>>, file_id: u32) -> Vec<u8> {
    root.and_then(|root| {
        let loc = root.resolve(file_id).ok()?;
        std::fs::read(loc.path_under(&root)).ok()
    })
    .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
pub(crate) struct GlobalEffectDirTask(bevy::tasks::Task<(Vec<Scheduler>, ActionAssets)>);

// ROM/0/0.DAT is ~540 KB of ~1000 chunks including many Img decodes; parsing it on the render
// thread reproduces the actor-load hitch, so it loads once off-thread and every lookup falls
// back to the pre-global behaviour until it lands.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn load_global_effect_dir(root: Res<ActionDatRoot>, mut commands: Commands) {
    let root = root.0.clone();
    let task = bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        // The global effect dir is spell effects, not cutscene camera routes
        // (ffxi-dat/src/scheduler.rs StageKind::CameraRoute); the parse's camera value
        // does not land here.
        let (schedulers, assets, report, _cameras) =
            parse_action_bytes_reporting(&read_dat_bytes(root, GLOBAL_EFFECT_DIR_FILE_ID));
        report_effect_coverage(GLOBAL_EFFECT_DIR_FILE_ID, &report);
        (schedulers, assets)
    });
    commands.insert_resource(GlobalEffectDirTask(task));
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn poll_global_effect_dir(
    task: Option<ResMut<GlobalEffectDirTask>>,
    mut commands: Commands,
) {
    use bevy::tasks::futures_lite::future;
    let Some(mut task) = task else { return };
    let Some((schedulers, assets)) = future::block_on(future::poll_once(&mut task.0)) else {
        return;
    };
    commands.remove_resource::<GlobalEffectDirTask>();
    commands.insert_resource(GlobalEffectDir { schedulers, assets });
}

#[cfg(not(target_arch = "wasm32"))]
pub struct ParsedActionDat {
    pub schedulers: Vec<Scheduler>,
    pub assets: ActionAssets,

    /// The kind 0x06 camera routes of the file, keyed by four-char name; a scheduler's
    /// CameraRoute stage names one of these (research/XIClient Game/Scheduler/Tags/0x04.cpp).
    pub cameras: ActionDatCameras,
}

// Populated Jeuno fires several casts/WS per second and each re-visits a handful of files, so a
// small window over the recently seen action DATs already turns repeat casts into pure hits.
#[cfg(not(target_arch = "wasm32"))]
const ACTION_DAT_CACHE_CAP: usize = 32;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(crate) struct ActionDatLru {
    map: HashMap<u32, Arc<ParsedActionDat>>,
    order: std::collections::VecDeque<u32>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ActionDatLru {
    pub(crate) fn get_and_promote(&mut self, file_id: u32) -> Option<Arc<ParsedActionDat>> {
        let hit = self.map.get(&file_id).cloned()?;
        self.order.retain(|k| *k != file_id);
        self.order.push_back(file_id);
        Some(hit)
    }

    pub(crate) fn insert(&mut self, file_id: u32, parsed: Arc<ParsedActionDat>) {
        if self.map.insert(file_id, parsed).is_some() {
            self.order.retain(|k| *k != file_id);
        }
        self.order.push_back(file_id);
        while self.map.len() > ACTION_DAT_CACHE_CAP {
            let Some(evict) = self.order.pop_front() else {
                break;
            };
            self.map.remove(&evict);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
enum PendingActionDispatch {
    Action {
        actor_id: u32,
        target_id: Option<u32>,
    },
    /// A named routine out of a file, on an actor, with a partner. Emotes and cutscene
    /// motions (the SCHEDULOR, LOADEVENTSCHEDULER2, LOADEXTSCHEDULER and MAPSCHEDULOR cues,
    /// ffxi-event/src/cue.rs) both dispatch through this; the name describes the operation,
    /// not one caller.
    Routine {
        actor_id: u32,
        target_id: u32,
        routine: [u8; 4],
        /// The Scheduler cue's duration operand (ffxi_event::SCHEDULER_DURATION_FROM_DAT when
        /// the cue carries none): it scales a CameraRoute stage's authored length against the
        /// routine's end frame, the way kuluu-session arms its WAIT* holds.
        duration: u16,
        /// The cue's wire actor when this dispatch is a motion the session's pending
        /// hold waits on; the miss paths report it done. `None` for emotes.
        cutscene_actor: Option<kuluu_snapshot::CutsceneActor>,
    },
    /// A Tpc routine (the LOADEXTSCHEDULER cue, ffxi-event/src/cue.rs): the routine's
    /// schedulers live in container A (the pending vec's file id); container B's clips join
    /// A's assets before the routine queues so its Motion stages can resolve the waist clip -
    /// the merge happens up front because the entity's ActionAssets is first-writer-wins, so a
    /// second DAT could not attach separately (A's clips keep any name B also ships). `b`
    /// is None when the actor's CIB waist byte loads A only.
    TpcRoutine {
        actor_id: u32,
        target_id: u32,
        b: Option<u32>,
        routine: [u8; 4],
        duration: u16,
        /// The cue's wire actor; the miss paths report it done.
        cutscene_actor: Option<kuluu_snapshot::CutsceneActor>,
    },
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
pub struct ActionDatCache {
    root: Option<Arc<ffxi_dat::DatRoot>>,
    lru: ActionDatLru,
    tasks: HashMap<u32, bevy::tasks::Task<ParsedActionDat>>,
    pending: Vec<(u32, PendingActionDispatch)>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ActionDatCache {
    /// Every cached parse, in-flight load and deferred dispatch belongs to the install it was
    /// read from, so a launcher DAT-path change drops all three rather than serving the next
    /// cast from a stale install.
    fn adopt_root(&mut self, root: Option<Arc<ffxi_dat::DatRoot>>) {
        self.root = root;
        self.lru = ActionDatLru::default();
        self.tasks.clear();
        self.pending.clear();
    }

    fn request(&mut self, file_id: u32) {
        if self.tasks.contains_key(&file_id) {
            return;
        }
        let root = self.root.clone();
        let task = bevy::tasks::AsyncComputeTaskPool::get()
            .spawn(async move { load_action_dat(root, file_id) });
        self.tasks.insert(file_id, task);
    }

    fn defer(&mut self, file_id: u32, dispatch: PendingActionDispatch) {
        self.request(file_id);
        if let PendingActionDispatch::TpcRoutine { b: Some(b), .. } = &dispatch {
            self.request(*b);
        }
        self.pending.push((file_id, dispatch));
    }
}

// An unresolvable/unreadable file caches as an empty parse, so a broken DAT path degrades to the
// pre-existing "no effect" behaviour instead of re-spawning a load per cast.
#[cfg(not(target_arch = "wasm32"))]
fn load_action_dat(root: Option<Arc<ffxi_dat::DatRoot>>, file_id: u32) -> ParsedActionDat {
    let (schedulers, assets, report, cameras) =
        parse_action_bytes_reporting(&read_dat_bytes(root, file_id));
    report_effect_coverage(file_id, &report);
    ParsedActionDat {
        schedulers,
        assets,
        cameras,
    }
}

#[cfg(not(target_arch = "wasm32"))]
type CoverageKey = (Option<ffxi_dat::particle_gen::GeneratorSection>, u8);

#[cfg(not(target_arch = "wasm32"))]
static REPORTED_COVERAGE: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashSet<CoverageKey>>,
> = std::sync::OnceLock::new();

/// One warn per (stream, opcode) for the whole process, naming the first effect DAT it was seen
/// in. The rate limit is the set, not a clock — a reload after a cache eviction or a DAT-root
/// switch stays quiet. Keying on the file as well would be a line per opcode per DAT, and the
/// corpus census (kuluu-render/tests/effect_instruction_census.rs) measures ~200 distinct codes
/// spread across nearly every file, so that key would flood a session's log rather than report.
#[cfg(not(target_arch = "wasm32"))]
fn report_effect_coverage(file_id: u32, report: &EffectCoverageReport) {
    let Ok(mut seen) = REPORTED_COVERAGE.get_or_init(Default::default).lock() else {
        return;
    };
    for &(routine, opcode, words) in &report.unknown_stages {
        if seen.insert((None, opcode)) {
            warn!(
                "effect DAT {file_id}: routine {} stage opcode {opcode:#04x} ({words} dwords) has no handler; the instruction is skipped",
                String::from_utf8_lossy(&routine)
            );
        }
    }
    for (chunk, section, opcode) in report.dropped_generator_opcodes() {
        if seen.insert((Some(section), opcode)) {
            warn!(
                "effect DAT {file_id}: generator {} {section:?} opcode {opcode:#04x} is dropped",
                String::from_utf8_lossy(&chunk)
            );
        }
    }
}

/// Queue the spell's `main` routine on the caster with its assets and target. A completion
/// effect alongside a running cast (or vice versa) is normal retail behaviour: the routine
/// pushes instead of replacing, and the first writer's ActionAssets/ActionTarget stay put
/// (the `_if_new` inserts keep them when the component was already present).
#[cfg(not(target_arch = "wasm32"))]
fn apply_action_dispatch(
    parsed: &ParsedActionDat,
    actor_routines: Option<&HashMap<ffxi_dat::datid::DatId, Scheduler>>,
    global: Option<&GlobalEffectDir>,
    actor_entity: Entity,
    target_entity: Option<Entity>,
    commands: &mut Commands,
) {
    // A spell DAT's `main` links the caster's own finish routine (0x3C `shbk`), which in turn
    // links global-dir routines — so the flatten must span all three tiers.
    let mut lookup = RoutineLookup::new().with_dat(&parsed.schedulers);
    if let Some(r) = actor_routines {
        lookup = lookup.with_actor(r);
    }
    if let Some(g) = global {
        lookup = lookup.with_dat(&g.schedulers);
    }
    let active = ActiveScheduler::from_routine(&lookup, b"main").or_else(|| {
        parsed
            .schedulers
            .first()
            .map(ActiveScheduler::from_scheduler)
    });
    let Some(active) = active else { return };
    enqueue_routine(commands, actor_entity, active);
    commands
        .entity(actor_entity)
        .try_insert_if_new(parsed.assets.clone())
        .try_insert_if_new(ActionTarget(target_entity));
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_routine_dispatch(
    parsed: &ParsedActionDat,
    routine: &[u8; 4],
    actor_entity: Entity,
    target_entity: Option<Entity>,
    q_scheds: &mut Query<&mut ActiveSchedulers>,
    pending_inserts: &mut HashMap<Entity, Vec<ActiveScheduler>>,
    commands: &mut Commands,
) -> bool {
    let Some(active) = ActiveScheduler::from_main(&parsed.schedulers, routine) else {
        return false;
    };
    queue_routine_on_actor(
        parsed,
        active,
        actor_entity,
        target_entity,
        q_scheds,
        pending_inserts,
        commands,
    );
    true
}

/// Same insert-or-push as apply_action_dispatch: a second routine (an emote or cutscene
/// motion) mid-cast runs alongside the other instead of replacing it.
#[cfg(not(target_arch = "wasm32"))]
fn queue_routine_on_actor(
    parsed: &ParsedActionDat,
    active: ActiveScheduler,
    actor_entity: Entity,
    target_entity: Option<Entity>,
    q_scheds: &mut Query<&mut ActiveSchedulers>,
    pending_inserts: &mut HashMap<Entity, Vec<ActiveScheduler>>,
    commands: &mut Commands,
) {
    queue_routine_on_actor_assets(
        &parsed.assets,
        active,
        actor_entity,
        target_entity,
        q_scheds,
        pending_inserts,
        commands,
    );
}

/// The Tpc form of the above (the LOADEXTSCHEDULER cue, ffxi-event/src/cue.rs): the assets are
/// the two containers' merged set, not one file's.
#[cfg(not(target_arch = "wasm32"))]
fn queue_routine_on_actor_assets(
    assets: &ActionAssets,
    active: ActiveScheduler,
    actor_entity: Entity,
    target_entity: Option<Entity>,
    q_scheds: &mut Query<&mut ActiveSchedulers>,
    pending_inserts: &mut HashMap<Entity, Vec<ActiveScheduler>>,
    commands: &mut Commands,
) {
    let fresh = queue_active_scheduler(actor_entity, active, q_scheds, pending_inserts);
    if fresh {
        commands
            .entity(actor_entity)
            .insert_if_new(assets.clone())
            .insert_if_new(ActionTarget(target_entity));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn actor_routines_via_mut<'a>(
    entity: Entity,
    q_children: &Query<&Children>,
    q_actors: &'a Query<&mut crate::ffxi_actor_render::FfxiRenderActor>,
) -> Option<&'a HashMap<ffxi_dat::datid::DatId, Scheduler>> {
    q_children
        .get(entity)
        .ok()?
        .iter()
        .find_map(|child| q_actors.get(child).ok())
        .map(|actor| actor.routines())
}

// A CameraRoute stage drives the operator camera instead of the skeleton (research/XIClient
// Game/Scheduler/Tags/0x04.cpp HandleTag0x04): one task per such stage, each scaled by the 0x45
// duration operand against the routine's authored end frame - the same ratio kuluu-session
// applies when it arms the WAIT* hold for this cue. The endpoints a route substitutes come from
// the operator camera's live state (START_AT_CURRENT_POS) and the player's default chase state
// (END_AT_CURRENT_POS), both captured at start time.
#[cfg(not(target_arch = "wasm32"))]
fn start_cutscene_camera_tasks(
    parsed: &ParsedActionDat,
    active: &ActiveScheduler,
    duration_override: u16,
    q_cam: &Query<(&Transform, &Projection), With<crate::camera::OperatorCamera>>,
    q_self: &Query<
        (&Transform, Option<&BakedActor>),
        (With<IsSelf>, Without<crate::camera::OperatorCamera>),
    >,
    mode: &crate::camera::CameraMode,
    tasks: &mut ResMut<CutsceneCameraTasks>,
    actor_entity: Entity,
    self_pos: &kuluu_snapshot::Position,
    q_attach: &Query<
        (&Transform, Option<&BakedActor>),
        (
            With<crate::components::WorldEntity>,
            Without<crate::camera::OperatorCamera>,
        ),
    >,
    q_children: &Query<&Children>,
    q_render: &Query<&mut crate::ffxi_actor_render::FfxiRenderActor>,
) {
    let Some(named) = parsed.schedulers.iter().find(|s| s.name == active.name()) else {
        return;
    };
    let ratio = crate::cutscene::scheduler_speed_ratio(duration_override, named.end_frame());

    let Some((cam_t, cam_proj)) = q_cam.single().ok() else {
        tracing::debug!(
            target: "kuluu_render::scheduler_runtime",
            routine = %fourcc(active.name()),
            "no operator camera to start the cutscene route from; its stages are dropped"
        );
        return;
    };
    let Some(current) = crate::cutscene_camera::capture_current_camera(cam_t, cam_proj) else {
        tracing::debug!(
            target: "kuluu_render::scheduler_runtime",
            routine = %fourcc(active.name()),
            "the operator camera is not a perspective projection; the route stages are dropped"
        );
        return;
    };
    let default_chase = q_self.single().ok().map(|(self_t, baked)| {
        crate::cutscene_camera::default_chase_endpoint(
            self_t,
            baked,
            matches!(*mode, crate::camera::CameraMode::FirstPerson),
        )
    });

    for stage in active
        .stages
        .iter()
        .filter(|t| t.stage.kind == StageKind::CameraRoute)
    {
        let Some(cam) = parsed.cameras.get(&stage.stage.id) else {
            tracing::debug!(
                target: "kuluu_render::scheduler_runtime",
                routine = %fourcc(active.name()),
                camera = %String::from_utf8_lossy(&stage.stage.id),
                "camera route stage names no kind 0x06 chunk in the file; the stage is dropped"
            );
            continue;
        };
        let Some(default_chase) = default_chase else {
            tracing::debug!(
                target: "kuluu_render::scheduler_runtime",
                routine = %fourcc(active.name()),
                camera = %String::from_utf8_lossy(&stage.stage.id),
                "no player entity for the route's end point; the stage is dropped"
            );
            continue;
        };
        let total_frames = stage.stage.duration_frames as f32 * ratio;
        // Modes 1 and 3 ride the cue's caster: Attachment.cpp MakeAttachMatrix places the
        // origin on the caster's EID locator, and xim's SourceToTargetBasis (mode 3) puts the
        // source-to-target origin on the source's joint 0, the same actor. Mode 0 plays in
        // world space (the decompilation's identity default arm); the unported modes (2-13
        // and 16-27 error out in retail, 14/15 anchor to zone positions) play in world space
        // here.
        let attach_actor = match cam.attach_mode() {
            ffxi_dat::camera::ATTACH_MODE_CASTER
            | ffxi_dat::camera::ATTACH_MODE_SOURCE_TO_TARGET => Some(actor_entity),
            _ => None,
        };
        if cam.attachment_info != 0 && attach_actor.is_none() {
            tracing::debug!(
                target: "kuluu_render::scheduler_runtime",
                routine = %fourcc(active.name()),
                camera = %String::from_utf8_lossy(&stage.stage.id),
                attachment_info = cam.attachment_info,
                "camera route attach mode is not ported; the route plays in world space"
            );
        }
        let mut attach = None;
        if let Some(actor) = attach_actor {
            match q_attach.get(actor) {
                Ok((xform, baked)) => {
                    let render = q_children.get(actor).ok().and_then(|children| {
                        children.iter().find_map(|child| q_render.get(child).ok())
                    });
                    let locator = cam.attach_locator_index();
                    // For the local player the event script's position/heading (the
                    // snapshot's self_pos, updated in the same ingest before this cue is
                    // processed) is authoritative at this instant; the rendered Transform
                    // lags it (the walker walks to it, the heading slerps at
                    // SELF_VISUAL_YAW_RATE), and a zero-interp attach freezes the matrix at
                    // start, so build it from the snapshot instead of the stale Transform.
                    let attach_xform = if q_self.get(actor).is_ok() {
                        Transform {
                            translation: crate::scene::ffxi_to_bevy(self_pos.pos),
                            rotation: crate::scene::heading_to_quat(self_pos.heading),
                            ..Default::default()
                        }
                    } else {
                        *xform
                    };
                    match crate::cutscene_camera::eid_model_point(locator, baked, render) {
                        Some(point) => {
                            attach = Some(crate::cutscene_camera::AttachStart {
                                actor,
                                locator,
                                interp: cam.interp_factor as f32
                                    / ffxi_dat::camera::INTERP_FACTOR_SCALE,
                                initial_matrix: crate::cutscene_camera::attach_matrix(
                                    &attach_xform,
                                    point,
                                ),
                            });
                        }
                        None => {
                            tracing::debug!(
                                target: "kuluu_render::scheduler_runtime",
                                routine = %fourcc(active.name()),
                                camera = %String::from_utf8_lossy(&stage.stage.id),
                                attachment_info = cam.attachment_info,
                                "camera route attach locator does not resolve; the stage is dropped"
                            );
                            continue;
                        }
                    }
                }
                // The caster is gone: retail's mode 1 with a null caster takes the identity
                // matrix (ffxi-dat/src/camera.rs ATTACH_MODE_CASTER), so the route plays in
                // world space.
                Err(_) => tracing::debug!(
                    target: "kuluu_render::scheduler_runtime",
                    routine = %fourcc(active.name()),
                    camera = %String::from_utf8_lossy(&stage.stage.id),
                    attachment_info = cam.attachment_info,
                    "camera route attach actor is gone; the route plays in world space"
                ),
            }
        }
        let task = crate::cutscene_camera::CutsceneCameraTask::start(
            cam,
            total_frames,
            current,
            default_chase,
            attach,
        );
        tracing::debug!(
            target: "kuluu_render::scheduler_runtime",
            routine = %fourcc(active.name()),
            camera = %String::from_utf8_lossy(&stage.stage.id),
            total_frames,
            attached = attach.is_some(),
            "cutscene camera route started"
        );
        tasks.start(task);
    }
}

// Applies dispatches whose action-DAT parse has landed. A cache miss therefore delays the
// completion effect by the load's frames-in-flight instead of stalling the frame it arrived on;
// the routine's internal timeline (motion + particles + SE) shifts as one unit.
#[cfg(not(target_arch = "wasm32"))]
pub fn poll_action_dat_tasks(
    mut cache: ResMut<ActionDatCache>,
    tracked: Res<crate::scene::TrackedEntities>,
    q_children: Query<&Children>,
    mut q_actors: Query<&mut crate::ffxi_actor_render::FfxiRenderActor>,
    global: Option<Res<GlobalEffectDir>>,
    mut tasks: ResMut<CutsceneCameraTasks>,
    q_cam: Query<(&Transform, &Projection), With<crate::camera::OperatorCamera>>,
    q_self: Query<
        (&Transform, Option<&BakedActor>),
        (With<IsSelf>, Without<crate::camera::OperatorCamera>),
    >,
    q_attach: Query<
        (&Transform, Option<&BakedActor>),
        (
            With<crate::components::WorldEntity>,
            Without<crate::camera::OperatorCamera>,
        ),
    >,
    mode: Res<crate::camera::CameraMode>,
    state: Res<crate::snapshot::SceneState>,
    mut q_scheds: Query<&mut ActiveSchedulers>,
    mut pending_inserts: Local<HashMap<Entity, Vec<ActiveScheduler>>>,
    mut commands: Commands,
    mut motion_done: MessageWriter<CutsceneMotionDone>,
) {
    use bevy::tasks::futures_lite::future;
    if cache.tasks.is_empty() && cache.pending.is_empty() {
        return;
    }
    let mut landed = Vec::new();
    cache.tasks.retain(
        |file_id, task| match future::block_on(future::poll_once(task)) {
            Some(parsed) => {
                landed.push((*file_id, Arc::new(parsed)));
                false
            }
            None => true,
        },
    );
    for (file_id, parsed) in landed {
        cache.lru.insert(file_id, parsed);
    }
    if cache.pending.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut cache.pending);
    for (file_id, dispatch) in pending {
        let b_file = match &dispatch {
            PendingActionDispatch::TpcRoutine { b: Some(b), .. } => Some(*b),
            _ => None,
        };
        let Some(parsed) = cache.lru.get_and_promote(file_id) else {
            // Still in flight — or evicted before this entry drained, in which case re-request.
            cache.defer(file_id, dispatch);
            continue;
        };
        let parsed_b = b_file.and_then(|b| cache.lru.get_and_promote(b));
        if b_file.is_some() && parsed_b.is_none() {
            cache.defer(file_id, dispatch);
            continue;
        }
        match dispatch {
            PendingActionDispatch::Action {
                actor_id,
                target_id,
            } => {
                let Some(&actor_entity) = tracked.by_id.get(&actor_id) else {
                    continue;
                };
                let target_entity = target_id.and_then(|id| tracked.by_id.get(&id).copied());
                let actor_routines = actor_routines_via_mut(actor_entity, &q_children, &q_actors);
                apply_action_dispatch(
                    &parsed,
                    actor_routines,
                    global.as_deref(),
                    actor_entity,
                    target_entity,
                    &mut commands,
                );
            }
            PendingActionDispatch::Routine {
                actor_id,
                target_id,
                routine,
                duration,
                cutscene_actor,
            } => {
                let Some(&actor_entity) = tracked.by_id.get(&actor_id) else {
                    // Report even when the actor is not tracked: the session's pending hold
                    // releases on this message (kuluu-session/src/state.rs
                    // AgentCommand::CutsceneMotionDone); a silent miss would sit it out its
                    // whole DAT-length deadline.
                    if let Some(actor) = cutscene_actor {
                        motion_done.write(CutsceneMotionDone {
                            actor,
                            key: routine,
                        });
                    }
                    continue;
                };
                let target_entity = tracked.by_id.get(&target_id).copied();
                // A spell DAT's `main` links the caster's own invoke routine (0x3C `shwh`,
                // research/xim DatResource.kt invokeWhiteMagic) out of the caster's skeleton
                // DAT, which in turn links global-dir routines — so the flatten spans the
                // same three tiers apply_action_dispatch uses: the file's schedulers, then
                // the actor's, then the global effect dir.
                let mut lookup = RoutineLookup::new().with_dat(&parsed.schedulers);
                if let Some(r) = actor_routines_via_mut(actor_entity, &q_children, &q_actors) {
                    lookup = lookup.with_actor(r);
                }
                if let Some(g) = global.as_ref() {
                    lookup = lookup.with_dat(&g.schedulers);
                }
                let Some(mut active) = ActiveScheduler::from_routine(&lookup, &routine) else {
                    // A cutscene motion's key is not an emote name: report the miss so the
                    // session's hold releases (kuluu-session/src/state.rs
                    // AgentCommand::CutsceneMotionDone), instead of playing a local clip.
                    if let Some(actor) = cutscene_actor {
                        motion_done.write(CutsceneMotionDone {
                            actor,
                            key: routine,
                        });
                    } else {
                        play_local_emote_clip(&routine, actor_entity, &q_children, &mut q_actors);
                    }
                    continue;
                };
                if active
                    .stages
                    .iter()
                    .any(|t| t.stage.kind == StageKind::CameraRoute)
                {
                    start_cutscene_camera_tasks(
                        &parsed,
                        &active,
                        duration,
                        &q_cam,
                        &q_self,
                        &mode,
                        &mut tasks,
                        actor_entity,
                        &state.snapshot.self_pos,
                        &q_attach,
                        &q_children,
                        &q_actors,
                    );
                    active
                        .stages
                        .retain(|t| t.stage.kind != StageKind::CameraRoute);
                }
                active.cutscene_motion_actor = cutscene_actor;
                if !active.stages.is_empty() {
                    queue_routine_on_actor(
                        &parsed,
                        active,
                        actor_entity,
                        target_entity,
                        &mut q_scheds,
                        &mut pending_inserts,
                        &mut commands,
                    );
                }
            }
            PendingActionDispatch::TpcRoutine {
                actor_id,
                target_id,
                routine,
                duration,
                cutscene_actor,
                ..
            } => {
                let Some(&actor_entity) = tracked.by_id.get(&actor_id) else {
                    // Report even when the actor is not tracked: the session's pending hold
                    // releases on this message (kuluu-session/src/state.rs
                    // AgentCommand::CutsceneMotionDone); a silent miss would sit it out its
                    // whole DAT-length deadline.
                    if let Some(actor) = cutscene_actor {
                        motion_done.write(CutsceneMotionDone {
                            actor,
                            key: routine,
                        });
                    }
                    continue;
                };
                let target_entity = tracked.by_id.get(&target_id).copied();
                let mut lookup = RoutineLookup::new().with_dat(&parsed.schedulers);
                if let Some(r) = actor_routines_via_mut(actor_entity, &q_children, &q_actors) {
                    lookup = lookup.with_actor(r);
                }
                if let Some(g) = global.as_ref() {
                    lookup = lookup.with_dat(&g.schedulers);
                }
                let Some(mut active) = ActiveScheduler::from_routine(&lookup, &routine) else {
                    // A cutscene motion's key is not an emote name: report the miss so the
                    // session's hold releases (kuluu-session/src/state.rs
                    // AgentCommand::CutsceneMotionDone), instead of playing a local clip.
                    if let Some(actor) = cutscene_actor {
                        motion_done.write(CutsceneMotionDone {
                            actor,
                            key: routine,
                        });
                    } else {
                        play_local_emote_clip(&routine, actor_entity, &q_children, &mut q_actors);
                    }
                    continue;
                };
                let assets = if let Some(parsed_b) = &parsed_b {
                    let mut merged = parsed.assets.clone();
                    let mut a_ids: std::collections::HashSet<ffxi_dat::datid::DatId> =
                        merged.animations.iter().map(|an| an.id).collect();
                    merged.animations.extend(
                        parsed_b
                            .assets
                            .animations
                            .iter()
                            .filter(|an| a_ids.insert(an.id))
                            .cloned(),
                    );
                    merged
                } else {
                    parsed.assets.clone()
                };
                if active
                    .stages
                    .iter()
                    .any(|t| t.stage.kind == StageKind::CameraRoute)
                {
                    start_cutscene_camera_tasks(
                        &parsed,
                        &active,
                        duration,
                        &q_cam,
                        &q_self,
                        &mode,
                        &mut tasks,
                        actor_entity,
                        &state.snapshot.self_pos,
                        &q_attach,
                        &q_children,
                        &q_actors,
                    );
                    active
                        .stages
                        .retain(|t| t.stage.kind != StageKind::CameraRoute);
                }
                active.cutscene_motion_actor = cutscene_actor;
                if !active.stages.is_empty() {
                    queue_routine_on_actor_assets(
                        &assets,
                        active,
                        actor_entity,
                        target_entity,
                        &mut q_scheds,
                        &mut pending_inserts,
                        &mut commands,
                    );
                }
            }
        }
    }
    flush_active_scheduler_inserts(&mut pending_inserts, &mut q_scheds, &mut commands);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_sound_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    q_actors: Query<&ActionAssets>,
    q_children: Query<&Children>,
    q_render: Query<&crate::ffxi_actor_render::FfxiRenderActor>,
    q_target: Query<&ActionTarget>,
    // `Transform`, not `GlobalTransform`, for the same reason spawn_particle_generators reads
    // it: world entities are roots, and a frame-0 stage fires on the insert frame, before
    // PostUpdate has propagated anything — a `GlobalTransform` read there is Ok-but-identity,
    // which would place the emitter at the world origin and get it culled to silence.
    q_transform: Query<&Transform>,
    global: Option<Res<GlobalEffectDir>>,
    mut sfx_writer: MessageWriter<crate::audio::SfxEvent>,
) {
    for ev in events.read() {
        let kind = ev.stage.stage.kind;
        if !matches!(
            kind,
            StageKind::SoundOnCaster | StageKind::SoundOnTarget | StageKind::SoundNonPositional
        ) {
            continue;
        }
        // research/xim EffectRoutineInstance.kt appendChildSequences,592-604 — routine DAT, then the actor's
        // own resource dirs (weapon `skaz`, face `atk1..4`), then the global dir.
        let actor_assets = q_children
            .get(ev.actor)
            .ok()
            .and_then(|c| c.iter().find_map(|child| q_render.get(child).ok()))
            .map(|a| a.action_assets());
        let tiers = [
            q_actors.get(ev.actor).ok(),
            actor_assets,
            global.as_ref().map(|g| &g.assets),
        ];
        let Some((se_id, on_caster)) = tiers.into_iter().flatten().find_map(|a| {
            ffxi_dat::action::resolve_stage_to_se(&ev.stage.stage.id, kind, &a.generators, &a.seps)
        }) else {
            continue;
        };

        // A 0x4A/0x60 stage has no world emitter: it mixes dry, like a UI or
        // weather cue, so it must not be sited on an actor and attenuated.
        if kind == StageKind::SoundNonPositional {
            sfx_writer.write(crate::audio::SfxEvent::new(se_id));
            continue;
        }
        let target = q_target.get(ev.actor).ok().and_then(|t| t.0);
        let origin = sound_origin_entity(on_caster, ev.actor, target);
        sfx_writer.write(match q_transform.get(origin) {
            Ok(xf) => crate::audio::SfxEvent::at(se_id, xf.translation),
            Err(_) => crate::audio::SfxEvent::new(se_id),
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_motion_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    q_children: Query<&Children>,
    q_assets: Query<&ActionAssets>,
    global: Option<Res<GlobalEffectDir>>,
    mut q_actors: Query<&mut crate::ffxi_actor_render::FfxiRenderActor>,
) {
    for ev in events.read() {
        if ev.stage.stage.kind != StageKind::Motion {
            continue;
        }
        // research/xim EffectRoutineInterpolatedEffects.kt SkeletonAnimationInstance animationDirs — a skill's body motion is
        // resolved against the skill DAT's own clips first, then the caster's animation
        // directories. ActionAssets lives on the tracked entity the scheduler runs on; the
        // render actor is its child.
        let stage = ev.stage.stage;
        let clip = ffxi_dat::datid::DatId::from_name(&stage.id);
        let local_clips: &[ffxi_dat::skel_anim::SkeletonAnimation] = assets_holding(
            q_assets.get(ev.actor).ok(),
            None,
            global.as_ref().map(|g| &g.assets),
            |a| {
                a.animations
                    .iter()
                    .any(|an| an.id.parameterized_match(&clip))
            },
        )
        .map(|a| a.animations.as_slice())
        .unwrap_or(&[]);
        let Ok(children) = q_children.get(ev.actor) else {
            continue;
        };
        for &child in children {
            if let Ok(mut actor) = q_actors.get_mut(child) {
                actor.begin_completion_motion(
                    clip,
                    crate::ffxi_actor_render::CompletionMotion {
                        local_clips,
                        duration_frames: stage.duration_frames as f32,
                        max_loops: stage.max_loops,
                        transition_in: crate::ffxi_actor_render::HalfFrames::from_dat(
                            stage.transition_in,
                        ),
                        transition_out: crate::ffxi_actor_render::HalfFrames::from_dat(
                            stage.transition_out,
                        ),
                    },
                );
            }
        }
    }
}

// research/xim EffectRoutineInterpolatedEffects.kt FlinchAnimationInstance - the 0x21/0x25 flinch stage plays the
// model's flinch clip ONCE (dfm? for PCs, dfi? otherwise), overwriting idle clips only and
// skipping when the model is animation-locked. This is the visual half of a hit reaction: the
// victim's `damg`/`ldam` routines are sound-only, so without this consumer a hit lands as SE +
// attacker-side sparks while the victim stands still.
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_flinch_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    q_children: Query<&Children>,
    mut q_render: Query<&mut crate::ffxi_actor_render::FfxiRenderActor>,
    q_scheds: Query<&ActiveSchedulers>,
    q_kind: Query<&crate::components::WorldEntity>,
    q_target: Query<&ActionTarget>,
) {
    for ev in events.read() {
        let host = match ev.stage.stage.kind {
            StageKind::FlinchOnCaster => Some(ev.actor),
            StageKind::FlinchOnTarget => q_target.get(ev.actor).ok().and_then(|t| t.0),
            _ => continue,
        };
        let Some(host) = host else { continue };
        // retail skips the flinch when the model is animation-locked or not currentlyIdle
        // (research/xim EffectRoutineInterpolatedEffects.kt) - a swing/cast/death clip owns the
        // pose then, and overwriting it would fight the lock.
        if q_scheds.get(host).is_ok_and(|s| s.is_locked_now()) {
            tracing::debug!(target: "combat", "COMBAT_FLINCH host={} skip-locked", host.index());
            continue;
        }
        let pc = q_kind
            .get(host)
            .is_ok_and(|w| w.kind == kuluu_snapshot::EntityKind::Pc);
        let Ok(children) = q_children.get(host) else {
            continue;
        };
        for &child in children {
            let Ok(mut actor) = q_render.get_mut(child) else {
                continue;
            };
            if !actor.is_pose_idle() {
                tracing::debug!(target: "combat", "COMBAT_FLINCH host={} skip-not-idle", host.index());
                continue;
            }
            let Some(clip) = actor.flinch_clip(pc) else {
                tracing::debug!(target: "combat", "COMBAT_FLINCH host={} no-flinch-clip pc={pc}", host.index());
                continue;
            };
            // animationDuration drives the transition in/out in half-frame u16 units
            // (research/xim EffectRoutineInterpolatedEffects.kt), so passing it whole yields
            // animationDuration/2 frames each side; no payload means no transition window.
            let anim_dur = ev.stage.stage.flinch_duration.unwrap_or(0.0).max(0.0);
            tracing::debug!(target: "combat", "COMBAT_FLINCH host={} clip={} pc={pc} dur={anim_dur}",
                    host.index(),
                    clip.as_str());
            actor.begin_completion_motion(
                clip,
                crate::ffxi_actor_render::CompletionMotion {
                    local_clips: &[],
                    duration_frames: anim_dur,
                    max_loops: 1,
                    transition_in: crate::ffxi_actor_render::HalfFrames::from_flinch_total(
                        anim_dur,
                    ),
                    transition_out: crate::ffxi_actor_render::HalfFrames::from_flinch_total(
                        anim_dur,
                    ),
                },
            );
        }
    }
}

/// How long a 0x028's knockback levels wait on the attacker for the skill
/// routine's knockback stage. The stage's own delay is authored in the
/// routine and is at most a few seconds; a skill whose routine has no
/// knockback stage never fires one, and its levels expire here instead of
/// piling up.
const KNOCKBACK_PENDING_TTL_SECS: f32 = 3.0;

/// The knockback levels of the 0x028s whose routines have not reached their
/// knockback stage yet, keyed by attacker id (research/xim
/// EffectRoutineInstance.kt handleKnockBackRoutine reads the magnitude off the
/// attack context when the stage fires, not when the packet lands).
#[derive(Resource, Default, Debug, PartialEq)]
pub struct PendingKnockbacks {
    by_actor: HashMap<u32, (Vec<(u32, u8)>, f32)>,
}

/// Parks each `ViewerEvent::Knockbacks` on its attacker until the routine
/// stage consumes it, and drops the ones no stage ever claimed.
#[cfg(not(target_arch = "wasm32"))]
pub fn collect_knockback_hits(
    time: Res<Time>,
    events: Res<crate::snapshot::EventLog>,
    mut pending: ResMut<PendingKnockbacks>,
    mut last_seen: Local<u64>,
) {
    let now = time.elapsed_secs();
    pending
        .by_actor
        .retain(|_, (_, expires_at)| *expires_at > now);

    let new_count =
        (events.pushed_total.saturating_sub(*last_seen)).min(events.recent.len() as u64) as usize;
    *last_seen = events.pushed_total;
    if new_count == 0 {
        return;
    }
    for ev in events.recent.iter().rev().take(new_count).rev() {
        let kuluu_snapshot::ViewerEvent::Knockbacks { actor_id, hits } = ev else {
            continue;
        };
        pending
            .by_actor
            .insert(*actor_id, (hits.clone(), now + KNOCKBACK_PENDING_TTL_SECS));
    }
}

/// FFXI x/y of a snapshot entity; the self entity reads the self position.
fn knockback_xy(snap: &kuluu_snapshot::SceneSnapshot, id: u32) -> Option<Vec2> {
    if snap.self_char_id == Some(id) {
        return Some(Vec2::new(snap.self_pos.pos.x, snap.self_pos.pos.y));
    }
    snap.entities
        .iter()
        .find(|e| e.id == id)
        .map(|e| Vec2::new(e.pos.x, e.pos.y))
}

/// The routine's knockback stage (ffxi-dat StageKind::Knockback, xim 0x5E /
/// 0xBF): every victim the 0x028 marked shoves away from the attacker, faces
/// it, and plays the knock-down (EffectRoutineInstance.kt
/// handleKnockBackRoutine, KnockBackInstance). The self victim's facing goes
/// to the walker through SelfKnockback so it rides the next Move.
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_knockback_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    state: Res<crate::snapshot::SceneState>,
    tracked: Res<crate::scene::TrackedEntities>,
    mut pending: ResMut<PendingKnockbacks>,
    mut self_kb: ResMut<crate::ffxi_actor_render::SelfKnockback>,
    q_kind: Query<&crate::components::WorldEntity>,
    q_roots: Query<&crate::ffxi_actor_render::FfxiRenderRoot>,
    mut q_render: Query<&mut crate::ffxi_actor_render::FfxiRenderActor>,
) {
    for ev in events.read() {
        if ev.stage.stage.kind != StageKind::Knockback {
            continue;
        }
        let Ok(caster) = q_kind.get(ev.actor) else {
            continue;
        };
        let Some((hits, _)) = pending.by_actor.remove(&caster.id) else {
            tracing::debug!(target: "combat", "KNOCKBACK stage caster={} no-pending-levels", caster.id);
            continue;
        };
        let snap = &state.snapshot;
        let Some(source) = knockback_xy(snap, caster.id) else {
            continue;
        };
        let animation_frames = ev.stage.stage.flinch_duration.unwrap_or(0.0);
        for (target_id, level) in hits {
            let Some(target) = knockback_xy(snap, target_id) else {
                continue;
            };
            let dir = target - source;
            if dir.length_squared() <= f32::EPSILON {
                continue;
            }
            let Some(&wire) = tracked.by_id.get(&target_id) else {
                continue;
            };
            let Ok(root) = q_roots.get(wire) else {
                continue;
            };
            let Ok(mut actor) = q_render.get_mut(root.0) else {
                continue;
            };
            tracing::debug!(target: "combat", "KNOCKBACK caster={} target={} level={} frames={}", caster.id, target_id, level, animation_frames);
            actor.begin_knockback(dir.normalize(), level, animation_frames);
            if snap.self_char_id == Some(target_id) {
                self_kb.face_toward = Some(source);
            }
        }
    }
}

/// Integrates every running knockback on the routine clock and hands the
/// self actor's shove and lock to the walker (KnockBackInstance updateEffect
/// adds the velocity after the movement-lock zeroing; here the walker adds
/// SelfKnockback.pending after zeroing the keys).
#[cfg(not(target_arch = "wasm32"))]
pub fn tick_knockbacks(
    time: Res<Time>,
    state: Res<crate::snapshot::SceneState>,
    mut self_kb: ResMut<crate::ffxi_actor_render::SelfKnockback>,
    mut q_render: Query<&mut crate::ffxi_actor_render::FfxiRenderActor>,
) {
    let elapsed_frames = time.delta_secs() * ROUTINE_FPS;
    let self_id = state.snapshot.self_char_id;
    let mut self_active = false;
    for mut actor in &mut q_render {
        let Some(shove) = actor.advance_knockback(elapsed_frames) else {
            continue;
        };
        if Some(actor.world_id) == self_id {
            self_kb.pending += shove;
            self_active |= actor.knockback_active();
        }
    }
    if self_kb.active != self_active {
        self_kb.active = self_active;
    }
}

pub fn action_dat_file_id(
    action_id: u32,
    animation: Option<u16>,
    action_kind: u8,
    race: Option<u8>,
    main_dll: Option<&ffxi_dat::main_dll::MainDll>,
) -> Option<u32> {
    // research/xim EffectDisplayer.displaySkill: the completion effect routine for a
    // skill lives in the file-table DAT keyed by the skill's animation index, which s2c 0x028
    // carries per result. Only the "finish" action categories carry that completed skill —
    // start categories drive the caster's cast-loop motion instead (see
    // ffxi_actor_render::action_routine). vendor/server enums/action/category.h:
    // 3 = weaponskill finish, 4 = magic finish, 6 = job-ability finish.
    use ffxi_proto::melee::{
        CATEGORY_ABILITY_FINISH, CATEGORY_MAGIC_FINISH, CATEGORY_MOB_SKILL_FINISH,
        CATEGORY_PET_SKILL_FINISH, CATEGORY_SKILL_FINISH,
    };
    match action_kind {
        CATEGORY_SKILL_FINISH => weapon_skill_file_id(animation?, race?, main_dll?),
        CATEGORY_MAGIC_FINISH => ffxi_vocab::action_anim::spell_file_id(action_id, animation),
        CATEGORY_ABILITY_FINISH => ffxi_vocab::action_anim::ability_file_id(action_id, animation),
        // research/xim MobAbilityTable.kt getFileTableOffset - mob skills (category 11) and pet
        // skills (category 13) key the effect DAT by the result's animation index with a range-
        // dependent base; that DAT's `main` plays the caster's own sp?? clip.
        CATEGORY_MOB_SKILL_FINISH | CATEGORY_PET_SKILL_FINISH => {
            Some(ffxi_vocab::action_anim::mob_skill_file_id(animation?))
        }
        // RangedFinish (2) carries no effect DAT: research/xim EffectDisplayer.kt
        // displaySkill returns early for the ranged attack ("Displayed as an
        // auto-attack"); the shot is the actor's own "shlg" routine
        // (ffxi_actor_render::action_routine).
        _ => None,
    }
}

// research/xim AbilityTable.kt getAnimationId — WS file id = race base (FFXiMain.dll) + per-skill index.
// `race` is the FFXI look race byte (HumeM=1..Galka=8), which is XIM's RaceGenderConfig.index.
fn weapon_skill_file_id(
    animation: u16,
    race: u8,
    main_dll: &ffxi_dat::main_dll::MainDll,
) -> Option<u32> {
    let base = main_dll.base_weapon_skill_index(race)?;
    Some(base as u32 + animation as u32)
}

// FFXiMain.dll is ~2.8 MB read whole and then scanned for several table markers
// (ffxi-dat/src/main_dll.rs::load), which is why it loads off-thread once per DAT root instead
// of on the first weaponskill or emote to reach the render thread.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
pub struct ActionMainDll(pub Option<Arc<ffxi_dat::main_dll::MainDll>>);

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
pub(crate) struct ActionMainDllTask(bevy::tasks::Task<Option<Arc<ffxi_dat::main_dll::MainDll>>>);

#[cfg(not(target_arch = "wasm32"))]
type MainDllCache =
    std::sync::Mutex<HashMap<std::path::PathBuf, Option<Arc<ffxi_dat::main_dll::MainDll>>>>;

#[cfg(not(target_arch = "wasm32"))]
static MAIN_DLLS: std::sync::OnceLock<MainDllCache> = std::sync::OnceLock::new();

/// The parsed FFXiMain.dll of one install root, shared by every consumer (action
/// dispatch, the actor loader, the look resolver, the minimap calibration). One
/// load per root; a root whose dll is unreadable is remembered as `None` so a
/// broken install is not re-read per actor. A different root is a different
/// entry, which is how a launcher install switch reaches every holder.
#[cfg(not(target_arch = "wasm32"))]
pub fn main_dll_for_root(root: &std::path::Path) -> Option<Arc<ffxi_dat::main_dll::MainDll>> {
    let cache = MAIN_DLLS.get_or_init(Default::default);
    let mut guard = cache.lock().ok()?;
    if let Some(entry) = guard.get(root) {
        return entry.clone();
    }
    let loaded = ffxi_dat::main_dll::MainDll::load(root)
        .inspect_err(|e| warn!("FFXiMain.dll unreadable under {}: {e}", root.display()))
        .ok()
        .map(Arc::new);
    guard.insert(root.to_path_buf(), loaded.clone());
    loaded
}

/// Drop the cached parse of `root` and read it again: a launcher setup or patch
/// rewrites FFXiMain.dll in place under the same path.
#[cfg(not(target_arch = "wasm32"))]
pub fn reload_main_dll_for_root(
    root: &std::path::Path,
) -> Option<Arc<ffxi_dat::main_dll::MainDll>> {
    if let Some(mut guard) = MAIN_DLLS.get().and_then(|cache| cache.lock().ok()) {
        guard.remove(root);
    }
    main_dll_for_root(root)
}

/// The install [`ffxi_dat::install::resolve`] names, without opening it:
/// callers that only need the dll must not pay the VTABLE/FTABLE parse a
/// `DatRoot` costs.
#[cfg(not(target_arch = "wasm32"))]
pub fn install_root_from_env() -> Option<std::path::PathBuf> {
    ffxi_dat::install::resolve().ok().map(|r| r.path)
}

/// [`main_dll_for_root`] for [`install_root_from_env`]; the entry point for
/// code paths that open their `DatRoot` from the environment.
#[cfg(not(target_arch = "wasm32"))]
pub fn main_dll_from_env() -> Option<Arc<ffxi_dat::main_dll::MainDll>> {
    main_dll_for_root(&install_root_from_env()?)
}

// Both halves of a DAT-root change: the parsed-DAT cache re-keys onto the new install and the
// dll is re-read from it (its per-root cache entry is evicted first, since a launcher setup or
// patch rewrites FFXiMain.dll in place under the same path). The dll is what turns an action
// into a *file id* -- serving the previous install's base tables while resolving them through
// the new root mixes the two installs -- so until the new one lands the dispatchers take their
// existing no-dll paths.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn adopt_action_dat_root(
    root: Res<ActionDatRoot>,
    mut cache: ResMut<ActionDatCache>,
    mut commands: Commands,
) {
    cache.adopt_root(root.0.clone());
    let root = root.0.clone();
    let task = bevy::tasks::AsyncComputeTaskPool::get()
        .spawn(async move { root.and_then(|root| reload_main_dll_for_root(root.root())) });
    commands.remove_resource::<ActionMainDll>();
    commands.insert_resource(ActionMainDllTask(task));
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn poll_action_main_dll(
    task: Option<ResMut<ActionMainDllTask>>,
    mut commands: Commands,
) {
    use bevy::tasks::futures_lite::future;
    let Some(mut task) = task else { return };
    let Some(dll) = future::block_on(future::poll_once(&mut task.0)) else {
        return;
    };
    commands.remove_resource::<ActionMainDllTask>();
    commands.insert_resource(ActionMainDll(dll));
}

#[cfg(not(target_arch = "wasm32"))]
fn look_race(look: &kuluu_snapshot::EntityLook) -> Option<u8> {
    match look {
        kuluu_snapshot::EntityLook::Equipped { race, .. } => Some(*race),
        _ => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_action_started(
    events: Res<crate::snapshot::EventLog>,
    tracked: Res<crate::scene::TrackedEntities>,
    q_look: Query<&crate::components::LookComp>,
    q_children: Query<&Children>,
    q_render: Query<&crate::ffxi_actor_render::FfxiRenderActor>,
    global: Option<Res<GlobalEffectDir>>,
    dll: Option<Res<ActionMainDll>>,
    mut cache: ResMut<ActionDatCache>,
    mut q_scheds: Query<&mut ActiveSchedulers>,
    mut pending_inserts: Local<HashMap<Entity, Vec<ActiveScheduler>>>,
    mut commands: Commands,
    mut last_seen: Local<u64>,
) {
    let new_count =
        (events.pushed_total.saturating_sub(*last_seen)).min(events.recent.len() as u64) as usize;
    *last_seen = events.pushed_total;
    if new_count == 0 {
        return;
    }
    for ev in events.recent.iter().rev().take(new_count).rev() {
        let kuluu_snapshot::ViewerEvent::ActionStarted {
            actor_id,
            action_id,
            action_kind,
            target_id,
            animation,
            ..
        } = *ev
        else {
            continue;
        };
        let Some(&actor_entity) = tracked.by_id.get(&actor_id) else {
            continue;
        };
        let target_entity = target_id.and_then(|id| tracked.by_id.get(&id).copied());
        let race = q_look.get(actor_entity).ok().and_then(|l| look_race(&l.0));
        let Some(file_id) = action_dat_file_id(
            action_id,
            animation,
            action_kind,
            race,
            dll.as_ref().and_then(|d| d.0.as_deref()),
        ) else {
            continue;
        };

        match cache.lru.get_and_promote(file_id) {
            Some(parsed) => {
                let actor_routines = actor_render_routines(actor_entity, &q_children, &q_render);
                apply_action_dispatch(
                    &parsed,
                    actor_routines,
                    global.as_deref(),
                    actor_entity,
                    target_entity,
                    &mut commands,
                );
            }
            None => cache.defer(
                file_id,
                PendingActionDispatch::Action {
                    actor_id,
                    target_id,
                },
            ),
        }
    }
    flush_active_scheduler_inserts(&mut pending_inserts, &mut q_scheds, &mut commands);
}

#[cfg(not(target_arch = "wasm32"))]
fn actor_render_routines<'a>(
    entity: Entity,
    q_children: &'a Query<&Children>,
    q_render: &'a Query<&crate::ffxi_actor_render::FfxiRenderActor>,
) -> Option<&'a HashMap<ffxi_dat::datid::DatId, Scheduler>> {
    q_children
        .get(entity)
        .ok()?
        .iter()
        .find_map(|child| q_render.get(child).ok())
        .map(|actor| actor.routines())
}

/// The server id a cutscene cue's actor operand names: the local player resolves
/// against the entity table's self id (None until it is known), everything else
/// is already a literal.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn cutscene_actor_server_id(
    self_id: Option<u32>,
    actor: kuluu_snapshot::CutsceneActor,
) -> Option<u32> {
    match actor {
        kuluu_snapshot::CutsceneActor::LocalPlayer => self_id,
        kuluu_snapshot::CutsceneActor::Entity { server_id } => Some(server_id),
    }
}

/// The CIB waist byte of the actor's render component (0 when the actor has no
/// render child or no CIB): which of a Tpc package's two tag-2 containers (the
/// LOADEXTSCHEDULER cue, ffxi-event/src/cue.rs) the renderer loads.
#[cfg(not(target_arch = "wasm32"))]
fn actor_waist_byte(
    entity: Entity,
    q_children: &Query<&Children>,
    q_render: &Query<&crate::ffxi_actor_render::FfxiRenderActor>,
) -> u8 {
    q_children
        .get(entity)
        .ok()
        .and_then(|children| children.iter().find_map(|child| q_render.get(child).ok()))
        .map(|actor| actor.body_armour_waist())
        .unwrap_or(0)
}

// Cutscene motion cues (research/XiEvents/OpCodes/0x002C.md, 0x0045.md, 0x005B.md):
// the event script's actor choreography. 0x2C names a routine in the actor's own model DAT,
// so it plays straight off the render component; 0x45 (non-fade) and 0x5B/0x66 name a
// routine out of an event motion resource file, which loads through the action cache like
// an emote. The session parks the VM's WAIT* on a pending hold this system's finish
// report releases (the DAT-authored length is the hold's deadline), so every path that
// cannot start a motion reports done; fades stay in cutscene.rs.
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_cutscene_motion(
    events: Res<crate::snapshot::EventLog>,
    tracked: Res<crate::scene::TrackedEntities>,
    table: Res<crate::entity_table::EntityTable>,
    q_children: Query<&Children>,
    q_render: Query<&crate::ffxi_actor_render::FfxiRenderActor>,
    mut cache: ResMut<ActionDatCache>,
    mut q_scheds: Query<&mut ActiveSchedulers>,
    mut pending_inserts: Local<HashMap<Entity, Vec<ActiveScheduler>>>,
    mut commands: Commands,
    mut motion_done: MessageWriter<CutsceneMotionDone>,
    mut last_seen: Local<u64>,
) {
    let new_count =
        (events.pushed_total.saturating_sub(*last_seen)).min(events.recent.len() as u64) as usize;
    *last_seen = events.pushed_total;
    if new_count == 0 {
        return;
    }
    let self_id = table.self_id();
    for ev in events.recent.iter().rev().take(new_count).rev() {
        let kuluu_snapshot::ViewerEvent::Cutscene { cue } = *ev else {
            continue;
        };
        let resolve = |a: kuluu_snapshot::CutsceneActor| -> Option<u32> {
            cutscene_actor_server_id(self_id, a)
        };
        match cue {
            // The SCHEDULOR cue (ffxi-event/src/cue.rs): the routine is in the actor's own
            // model DAT. Every path that cannot start it reports done immediately: the
            // session's pending hold must not wait for a finish that does not come.
            CutsceneCue::ActorMotion {
                actor,
                partner,
                key,
            } => {
                let (Some(actor_id), Some(partner_id)) = (resolve(actor), resolve(partner)) else {
                    motion_done.write(CutsceneMotionDone { actor, key });
                    continue;
                };
                let Some(&actor_entity) = tracked.by_id.get(&actor_id) else {
                    motion_done.write(CutsceneMotionDone { actor, key });
                    continue;
                };
                let Some(routines) = actor_render_routines(actor_entity, &q_children, &q_render)
                else {
                    motion_done.write(CutsceneMotionDone { actor, key });
                    continue;
                };
                let lookup = RoutineLookup::new().with_actor(routines);
                let Some(mut active) = ActiveScheduler::from_routine(&lookup, &key) else {
                    tracing::debug!(
                        target: "kuluu_render::scheduler_runtime",
                        key = %fourcc(key),
                        "cutscene actor motion has no routine on the actor"
                    );
                    motion_done.write(CutsceneMotionDone { actor, key });
                    continue;
                };
                active.cutscene_motion_actor = Some(actor);
                let target_entity = tracked.by_id.get(&partner_id).copied();
                if queue_active_scheduler(actor_entity, active, &mut q_scheds, &mut pending_inserts)
                {
                    commands
                        .entity(actor_entity)
                        .insert_if_new(ActionTarget(target_entity));
                }
            }
            // The LOADEVENTSCHEDULER2 cue with a non-fade DAT (ffxi-event/src/cue.rs): a
            // named routine out of a file, on an actor, with a partner. Same dispatch shape
            // the emote path uses; the duration operand scales any CameraRoute stage it carries.
            CutsceneCue::Scheduler {
                dat_id,
                actor,
                partner,
                tag,
                duration,
            } if dat_id != ffxi_event::SCHEDULER_FADE_DAT_ID => {
                let (Some(actor_id), Some(target_id)) = (resolve(actor), resolve(partner)) else {
                    continue;
                };
                cache.defer(
                    dat_id,
                    PendingActionDispatch::Routine {
                        actor_id,
                        target_id,
                        routine: tag,
                        duration,
                        cutscene_actor: Some(actor),
                    },
                );
            }
            // 0x5B bank and 0x66 package: same dispatch. Prefer a routine the actor already
            // owns under that key (research/cexi-docs/cutscene_authoring.md, Dialogue +
            // gestures: an actor's own routine outranks a same-named bank gesture, and bank
            // motion binds by joint index, which distorts fixed-model rigs); otherwise load
            // the file and dispatch the named routine.
            CutsceneCue::ExtScheduler {
                motion,
                actor,
                partner,
                key,
            } => {
                let (Some(actor_id), Some(target_id)) = (resolve(actor), resolve(partner)) else {
                    continue;
                };
                let Some(&actor_entity) = tracked.by_id.get(&actor_id) else {
                    continue;
                };
                let owned = actor_render_routines(actor_entity, &q_children, &q_render).and_then(
                    |routines| {
                        ActiveScheduler::from_routine(
                            &RoutineLookup::new().with_actor(routines),
                            &key,
                        )
                    },
                );
                match (owned, motion) {
                    (Some(mut active), _) => {
                        let target_entity = tracked.by_id.get(&target_id).copied();
                        active.cutscene_motion_actor = Some(actor);
                        if queue_active_scheduler(
                            actor_entity,
                            active,
                            &mut q_scheds,
                            &mut pending_inserts,
                        ) {
                            commands
                                .entity(actor_entity)
                                .insert_if_new(ActionTarget(target_entity));
                        }
                    }
                    (None, None) => {
                        tracing::debug!(
                            target: "kuluu_render::scheduler_runtime",
                            key = %fourcc(key),
                            "0x66 out-of-range package: no container and no owned routine; nothing to play"
                        );
                    }
                    (None, Some(ExtSchedulerMotion::Event(file_id))) => {
                        tracing::debug!(
                            target: "kuluu_render::scheduler_runtime",
                            file_id,
                            key = %fourcc(key),
                            "cutscene motion from event motion resource"
                        );
                        cache.defer(
                            file_id,
                            PendingActionDispatch::Routine {
                                actor_id,
                                target_id,
                                routine: key,
                                duration: ffxi_event::SCHEDULER_DURATION_FROM_DAT,
                                cutscene_actor: Some(actor),
                            },
                        );
                    }
                    (None, Some(ExtSchedulerMotion::Tpc { a, b_set, b_clear })) => {
                        let b = ffxi_event::tpc_b_for_waist(
                            b_set,
                            b_clear,
                            actor_waist_byte(actor_entity, &q_children, &q_render),
                        );
                        tracing::debug!(
                            target: "kuluu_render::scheduler_runtime",
                            a,
                            b,
                            key = %fourcc(key),
                            "cutscene motion from Tpc package"
                        );
                        cache.defer(
                            a,
                            PendingActionDispatch::TpcRoutine {
                                actor_id,
                                target_id,
                                b,
                                routine: key,
                                duration: ffxi_event::SCHEDULER_DURATION_FROM_DAT,
                                cutscene_actor: Some(actor),
                            },
                        );
                    }
                }
            }
            // The MAPSCHEDULOR cue (ffxi-event/src/cue.rs): the zone-level routine out of the
            // current zone's own model DAT (the cue carries the zone id); on a miss, the
            // entrance/instance partner zone's model DAT, then the non-model scene carriers.
            // Defer the file the key resolved in; its camera stages drive the operator
            // camera like any other routine's.
            CutsceneCue::ZoneScheduler {
                key,
                actor,
                partner,
                zone_id,
            } => {
                let (Some(actor_id), Some(target_id)) = (resolve(actor), resolve(partner)) else {
                    continue;
                };
                let Some(file_id) = cache
                    .root
                    .as_ref()
                    .and_then(|root| ffxi_dat::scheduler::zone_scene_file_id(root, zone_id, key))
                else {
                    // No install, or the key is in no candidate file: report done so the
                    // session's pending hold releases (kuluu-session/src/state.rs
                    // AgentCommand::CutsceneMotionDone) instead of sitting out its deadline.
                    motion_done.write(CutsceneMotionDone { actor, key });
                    continue;
                };
                cache.defer(
                    file_id,
                    PendingActionDispatch::Routine {
                        actor_id,
                        target_id,
                        routine: key,
                        duration: ffxi_event::SCHEDULER_DURATION_FROM_DAT,
                        cutscene_actor: Some(actor),
                    },
                );
            }
            _ => {}
        }
    }
    flush_active_scheduler_inserts(&mut pending_inserts, &mut q_scheds, &mut commands);
}

/// Client-side actor state owned by a running cutscene: pending walks plus every server id
/// this session moved. CutsceneEnded releases each touched entity back to its last
/// server-authored position (the way the fade and camera lock are released), and prediction
/// and grounding skip touched ids while they stay here so the authored choreography owns the
/// transform.
#[derive(Resource, Debug, Default)]
pub struct CutsceneActorState {
    /// Server id -> (goal in world units, speed in world units per second).
    walks: HashMap<u32, (Vec3, f32)>,
    touched: std::collections::HashSet<u32>,
    /// Server ids hidden by a running cutscene's EVENT_HIDE cue; cleared at CutsceneEnded.
    hidden: std::collections::HashSet<u32>,
    /// Server ids with a running 0x6C TRANSPAR fade (the
    /// crate::ffxi_actor_render::CutsceneTranspar component,
    /// research/XiEvents/OpCodes/0x006C.md); cleared at CutsceneEnded.
    faded: std::collections::HashSet<u32>,
}

/// A model root hidden by a running cutscene's ActorHide cue (ffxi-event/src/cue.rs).
/// Distance-culling and the entity sync respect it the way they already respect
/// server-invisible entities, so an in-range hide is not re-shown on the next cull
/// pass; release_cutscene_actors removes it at CutsceneEnded.
#[derive(Component, Debug, Default)]
pub struct CutsceneHidden;

impl CutsceneActorState {
    pub fn is_touched(&self, id: u32) -> bool {
        self.touched.contains(&id)
    }

    pub fn touch(&mut self, id: u32) {
        self.touched.insert(id);
    }

    pub fn begin_walk(&mut self, id: u32, goal: Vec3, speed: f32) {
        self.touch(id);
        self.walks.insert(id, (goal, speed));
    }

    pub fn hide(&mut self, id: u32) {
        self.hidden.insert(id);
    }

    pub fn unhide(&mut self, id: u32) {
        self.hidden.remove(&id);
    }

    pub fn fade(&mut self, id: u32) {
        self.faded.insert(id);
    }

    pub fn unfade(&mut self, id: u32) {
        self.faded.remove(&id);
    }

    pub fn is_empty(&self) -> bool {
        self.touched.is_empty()
            && self.walks.is_empty()
            && self.hidden.is_empty()
            && self.faded.is_empty()
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Event-coordinate position to Bevy world units: event y is height and x/z are the ground
/// plane (research/XiEvents/OpCodes/0x001F.md MOVE integrates [0]/[2] and snaps [1]), so the
/// vertical lands on Bevy's up axis, negated like every other placed asset.
fn event_to_world(x: i32, y: i32, z: i32) -> Vec3 {
    Vec3::new(
        x as f32 / EVENT_COORD_UNITS,
        -(y as f32 / EVENT_COORD_UNITS),
        -(z as f32 / EVENT_COORD_UNITS),
    )
}

#[cfg(not(target_arch = "wasm32"))]
/// Event-coordinate heading (4096 steps per full circle) to a Bevy yaw: the inverse of
/// scene.rs's wire-heading convention (heading_to_quat over the 1/256-step wire byte).
fn event_heading_to_quat(heading: i32) -> Quat {
    Quat::from_rotation_y(-std::f32::consts::TAU * heading as f32 / EVENT_HEADING_UNITS)
}

/// The rotation that points an actor's +X forward along the Bevy-space
/// horizontal offset `(dx, dz)` to its target. Same basis as
/// `crate::scene::heading_to_quat` and `event_heading_to_quat`: a rotation of
/// `-theta` about Y sends +X to `(cos theta, 0, sin theta)`
/// (combat_stance::heading_forward), so the yaw is the negated atan2.
fn look_at_rotation(dx: f32, dz: f32) -> Quat {
    Quat::from_rotation_y(-dz.atan2(dx))
}

// The five client-side actor cues (research/XiEvents/OpCodes/0x001F.md, 0x0037.md, 0x0039.md,
// 0x004A.md, 0x005E.md): the event script's NPC choreography. Every write is client-side
// transform state scoped to the running cutscene; release_cutscene_actors puts each touched
// entity back on its last server-authored position at CutsceneEnded.
#[cfg(not(target_arch = "wasm32"))]
pub fn apply_cutscene_actor_cues(
    events: Res<crate::snapshot::EventLog>,
    tracked: Res<crate::scene::TrackedEntities>,
    table: Res<crate::entity_table::EntityTable>,
    mut state: ResMut<CutsceneActorState>,
    mut q_xform: Query<&mut Transform, With<WorldEntity>>,
    mut q_vis: Query<&mut Visibility, With<WorldEntity>>,
    mut q_scheds: Query<&mut ActiveSchedulers>,
    q_children: Query<&Children>,
    mut q_actors: Query<&mut crate::ffxi_actor_render::FfxiRenderActor>,
    mut commands: Commands,
    mut last_seen: Local<u64>,
) {
    let new_count =
        (events.pushed_total.saturating_sub(*last_seen)).min(events.recent.len() as u64) as usize;
    *last_seen = events.pushed_total;
    if new_count == 0 {
        return;
    }
    let self_id = table.self_id();
    for ev in events.recent.iter().rev().take(new_count).rev() {
        let kuluu_snapshot::ViewerEvent::Cutscene { cue } = *ev else {
            continue;
        };
        // The local player's movement is the session's own scene lerp
        // (kuluu-session/src/event_dialog.rs), not a cue: driving it here would fight
        // first-person input and prediction. Look-at targets may still be the player
        // (an NPC turning to face you).
        let moved = |a: kuluu_snapshot::CutsceneActor| -> Option<u32> {
            cutscene_actor_server_id(self_id, a).filter(|id| Some(*id) != self_id)
        };
        match cue {
            // The authored arrival heading is not applied: the walk faces its travel
            // direction, and the scene's next facing cue (ffxi-event/src/cue.rs
            // ActorFace) owns what comes after.
            CutsceneCue::ActorMove {
                actor,
                x,
                y,
                z,
                speed,
                ..
            } => {
                let Some(id) = moved(actor) else {
                    continue;
                };
                let speed = speed as f32 * EVENT_SPEED_SCALE;
                state.begin_walk(id, event_to_world(x, y, z), speed);
                tracing::debug!(
                    target: "kuluu_render::scheduler_runtime",
                    id,
                    speed,
                    "cutscene actor walk"
                );
            }
            CutsceneCue::ActorPlace {
                actor,
                x,
                y,
                z,
                heading,
            } => {
                let Some(id) = moved(actor) else {
                    continue;
                };
                let Some(&entity) = tracked.by_id.get(&id) else {
                    continue;
                };
                if let Ok(mut t) = q_xform.get_mut(entity) {
                    t.translation = event_to_world(x, y, z);
                    t.rotation = event_heading_to_quat(heading);
                    state.touch(id);
                    tracing::debug!(
                        target: "kuluu_render::scheduler_runtime",
                        id,
                        heading,
                        "cutscene actor place"
                    );
                }
            }
            CutsceneCue::ActorFace { actor, heading } => {
                let Some(id) = moved(actor) else {
                    continue;
                };
                let Some(&entity) = tracked.by_id.get(&id) else {
                    continue;
                };
                if let Ok(mut t) = q_xform.get_mut(entity) {
                    t.rotation = event_heading_to_quat(heading);
                    state.touch(id);
                    tracing::debug!(
                        target: "kuluu_render::scheduler_runtime",
                        id,
                        heading,
                        "cutscene actor face"
                    );
                }
            }
            CutsceneCue::ActorLookAt { actor, target } => {
                let Some(id) = moved(actor) else {
                    continue;
                };
                let Some(target_id) = cutscene_actor_server_id(self_id, target) else {
                    continue;
                };
                let (Some(&entity), Some(&target_entity)) =
                    (tracked.by_id.get(&id), tracked.by_id.get(&target_id))
                else {
                    continue;
                };
                // The look-at yaw inverts scene.rs's travel-heading formula, and this system keeps
                // one exclusive Transform query (a second shared one trips B0001), so both
                // positions are read via .get() before the write.
                let (Ok(from), Ok(to)) = (q_xform.get(entity), q_xform.get(target_entity)) else {
                    continue;
                };
                let dx = to.translation.x - from.translation.x;
                let dz = to.translation.z - from.translation.z;
                if dx.abs() <= f32::EPSILON && dz.abs() <= f32::EPSILON {
                    continue;
                }
                if let Ok(mut t) = q_xform.get_mut(entity) {
                    t.rotation = look_at_rotation(dx, dz);
                    state.touch(id);
                    tracing::debug!(
                        target: "kuluu_render::scheduler_runtime",
                        id,
                        target_id,
                        "cutscene actor look-at"
                    );
                }
            }
            CutsceneCue::ActorStopAction { actor, key } => {
                let Some(id) = moved(actor) else {
                    continue;
                };
                let Some(&entity) = tracked.by_id.get(&id) else {
                    continue;
                };
                let queue_now_empty = if let Ok(mut scheds) = q_scheds.get_mut(entity) {
                    match key {
                        Some(name) => scheds.remove_routine_named(&name),
                        None => scheds.stop_all(),
                    }
                    scheds.is_empty()
                } else {
                    false
                };
                // The killed routine's Motion stage owns the caster's pose
                // (the gate guard's Signet arm-raise, research/XiEvents/OpCodes/0x0073.md):
                // dropping the queue entry leaves the held action, so clear it on the
                // render actor and let the pose path fall back to idle.
                if let Ok(children) = q_children.get(entity) {
                    for &child in children {
                        if let Ok(mut render) = q_actors.get_mut(child) {
                            render.clear_cutscene_action();
                        }
                    }
                }
                // The same strip tick_active_schedulers does when the last entry
                // retires: an entity with no running routines keeps no action components.
                if queue_now_empty {
                    commands
                        .entity(entity)
                        .remove::<(ActiveSchedulers, ActionAssets, ActionTarget)>();
                }
                tracing::debug!(
                    target: "kuluu_render::scheduler_runtime",
                    id,
                    key = %key.map(fourcc).unwrap_or_default(),
                    "cutscene actor stop action"
                );
            }
            CutsceneCue::ActorHide { target, hide } => {
                // Hiding the local player model is a valid ask (the EVENT_HIDE_SELF opcode
                // routes here too, ffxi-event/src/vm.rs OP_EVENT_HIDE_SELF), so resolve
                // without excluding self.
                let Some(id) = cutscene_actor_server_id(self_id, target) else {
                    continue;
                };
                let Some(&entity) = tracked.by_id.get(&id) else {
                    continue;
                };
                if hide {
                    commands.entity(entity).insert(CutsceneHidden);
                    state.hide(id);
                    if let Ok(mut v) = q_vis.get_mut(entity) {
                        *v = Visibility::Hidden;
                    }
                    tracing::debug!(
                        target: "kuluu_render::scheduler_runtime",
                        id,
                        "cutscene actor hide"
                    );
                } else {
                    commands.entity(entity).remove::<CutsceneHidden>();
                    state.unhide(id);
                    // Leave Visibility to culling/sync (scene.rs apply_invis_flag_system):
                    // next frame it is Inherited when in range and not server-invisible, so
                    // an out-of-range or buried actor is not force-shown.
                    tracing::debug!(
                        target: "kuluu_render::scheduler_runtime",
                        id,
                        "cutscene actor show"
                    );
                }
            }
            // 0x38: while CliEventModeLocal holds, the event hides the local
            // player model so it can drive the camera apart from it
            // (research/XiEvents/OpCodes/0x0038.md); the HUD half rides
            // CutsceneMode.local_mode in crate::cutscene, and
            // release_cutscene_actors owns the unhide at CutsceneEnded.
            CutsceneCue::LocalMode { .. } => {
                let Some(id) = self_id else {
                    continue;
                };
                let Some(&entity) = tracked.by_id.get(&id) else {
                    continue;
                };
                commands.entity(entity).insert(CutsceneHidden);
                state.hide(id);
                if let Ok(mut v) = q_vis.get_mut(entity) {
                    *v = Visibility::Hidden;
                }
                tracing::debug!(
                    target: "kuluu_render::scheduler_runtime",
                    id,
                    "cutscene local mode hides the self actor"
                );
            }
            // 0x6C: drive the target's opacity to the authored byte over the
            // authored frames; the fade stops at CutsceneEnded at whatever
            // value it has reached (ffxi-event/src/cue.rs Transpar).
            CutsceneCue::Transpar {
                target,
                end_alpha,
                duration_frames,
            } => {
                // Fading the local player model is a valid ask, so resolve
                // without excluding self (research/XiEvents/OpCodes/0x006C.md).
                let Some(id) = cutscene_actor_server_id(self_id, target) else {
                    continue;
                };
                let Some(&entity) = tracked.by_id.get(&id) else {
                    continue;
                };
                commands
                    .entity(entity)
                    .insert(crate::ffxi_actor_render::CutsceneTranspar::new(
                        (end_alpha as f32 / 255.0).clamp(0.0, 1.0),
                        (duration_frames as f32).max(1.0) / 60.0,
                    ));
                state.fade(id);
                tracing::debug!(
                    target: "kuluu_render::scheduler_runtime",
                    id,
                    end_alpha,
                    duration_frames,
                    "cutscene actor transpar"
                );
            }
            _ => {}
        }
    }
}

/// Per-frame progression of the walks apply_cutscene_actor_cues queued: each entity steps
/// toward its goal at the authored speed (world units per second, the same clock the session
/// arms the MOVE hold with) and faces its travel direction; arrival snaps and drops the walk.
#[cfg(not(target_arch = "wasm32"))]
pub fn advance_cutscene_walks(
    time: Res<Time>,
    tracked: Res<crate::scene::TrackedEntities>,
    mut state: ResMut<CutsceneActorState>,
    mut q_xform: Query<&mut Transform, With<WorldEntity>>,
) {
    if state.walks.is_empty() {
        return;
    }
    let dt = time.delta_secs();
    let mut arrived = Vec::new();
    for (&id, &(goal, speed)) in state.walks.iter() {
        let Some(&entity) = tracked.by_id.get(&id) else {
            arrived.push(id);
            continue;
        };
        let Ok(mut t) = q_xform.get_mut(entity) else {
            continue;
        };
        let dx = goal.x - t.translation.x;
        let dz = goal.z - t.translation.z;
        let dist = (dx * dx + dz * dz).sqrt();
        if dist <= f32::EPSILON {
            arrived.push(id);
            continue;
        }
        let travel = (speed * dt).min(dist);
        t.translation.x += dx / dist * travel;
        t.translation.z += dz / dist * travel;
        t.translation.y = goal.y;
        t.rotation = Quat::from_rotation_y(dz.atan2(dx));
        if travel >= dist {
            arrived.push(id);
        }
    }
    for id in arrived {
        state.walks.remove(&id);
    }
}

/// CutsceneEnded (and the zone/disconnect belt-and-braces, mirroring drain_cutscene_events):
/// release every entity this cutscene moved back to its last server-authored position and
/// drop the pending walks.
#[cfg(not(target_arch = "wasm32"))]
pub fn release_cutscene_actors(
    events: Res<crate::snapshot::EventLog>,
    scene_state: Res<crate::snapshot::SceneState>,
    tracked: Res<crate::scene::TrackedEntities>,
    mut state: ResMut<CutsceneActorState>,
    mut cursor: Local<u64>,
    mut q_xform: Query<&mut Transform, With<WorldEntity>>,
    q_hidden: Query<Entity, With<CutsceneHidden>>,
    q_faded: Query<Entity, With<crate::ffxi_actor_render::CutsceneTranspar>>,
    q_scheds: Query<(Entity, &ActiveSchedulers), With<WorldEntity>>,
    q_children: Query<&Children>,
    mut q_actors: Query<&mut crate::ffxi_actor_render::FfxiRenderActor>,
    mut commands: Commands,
) {
    let total = events.pushed_total;
    let first_global = total.saturating_sub(events.recent.len() as u64);
    let mut ended = false;
    for g in (*cursor).max(first_global)..total {
        match &events.recent[(g - first_global) as usize] {
            kuluu_snapshot::ViewerEvent::CutsceneEnded
            | kuluu_snapshot::ViewerEvent::ZoneChanged { .. }
            | kuluu_snapshot::ViewerEvent::Disconnected { .. } => ended = true,
            _ => {}
        }
    }
    *cursor = total;
    if !ended {
        return;
    }
    // A cutscene cast's Motion stage owns the caster's pose (the gate guard's
    // Signet arm-raise, research/XiEvents/OpCodes/0x0073.md). The event
    // ending must release it: stop the routine so its Motion stage cannot
    // re-arm the pose, and drop the held action so the pose pass falls back
    // to idle on its next run. This runs even when no entity was touched
    // (the cast does not move the guard), so it precedes the touched-only
    // position reset below.
    for (entity, scheds) in q_scheds.iter() {
        if !scheds
            .routines
            .iter()
            .any(|r| r.cutscene_motion_actor.is_some())
        {
            continue;
        }
        if let Ok(children) = q_children.get(entity) {
            for &child in children {
                if let Ok(mut actor) = q_actors.get_mut(child) {
                    actor.clear_cutscene_action();
                }
            }
        }
        commands
            .entity(entity)
            .remove::<(ActiveSchedulers, ActionAssets, ActionTarget)>();
    }
    if state.is_empty() {
        return;
    }
    for wire in &scene_state.snapshot.entities {
        if !state.is_touched(wire.id) {
            continue;
        }
        let Some(&entity) = tracked.by_id.get(&wire.id) else {
            continue;
        };
        if let Ok(mut t) = q_xform.get_mut(entity) {
            t.translation = crate::scene::ffxi_to_bevy(wire.pos);
            t.rotation = crate::scene::heading_to_quat(wire.heading);
        }
    }
    // Release every cutscene-hidden model so it reappears on its last server-authored
    // visibility (scene.rs apply_invis_flag_system owns Visibility from the next frame
    // once the marker is gone).
    for e in q_hidden.iter() {
        commands.entity(e).remove::<CutsceneHidden>();
    }
    // Stop every running 0x6C fade at its current value: retail drops the
    // fade's driver with the event's own ExtData
    // (research/XiEvents/OpCodes/0x006C.md).
    for e in q_faded.iter() {
        commands
            .entity(e)
            .remove::<crate::ffxi_actor_render::CutsceneTranspar>();
    }
    state.walks.clear();
    state.touched.clear();
    state.hidden.clear();
    state.faded.clear();
}

// The routine the caster's cast-start effects were flattened from, so an interrupt can stop the
// generators it spawned. research/xim Actor.kt startCasting enqueues the whole model
// routine, not just its Motion stage.
// `posed` latches once the caster is observed in the looping cast pose. Cast routines with no
// Motion stage (retail `caso`/`calg`/`cage`) never set it, so the heuristic teardown below must
// not read "not posing" as "cast over" — for those the 0x2D stops and the interrupt signal are
// the only correct ends.
#[derive(Component, Debug, Clone, Copy)]
pub struct CastRoutine {
    pub routine: [u8; 4],
    pub posed: bool,
}

// research/xim Actor.kt startCasting — a cast start runs the caster's full `ca<suffix>` model routine
// (the `ner1` aura, its sounds, its sub-routines). Only the magic-start category is routed here:
// the melee `ati0` routine carries its own sub-routines and would change every auto-attack swing.
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_cast_routine_started(
    events: Res<crate::snapshot::EventLog>,
    tracked: Res<crate::scene::TrackedEntities>,
    q_children: Query<&Children>,
    q_render: Query<&crate::ffxi_actor_render::FfxiRenderActor>,
    global: Option<Res<GlobalEffectDir>>,
    q_cast: Query<&CastRoutine>,
    mut sim: ResMut<crate::particle_sim::ParticleSimulator>,
    mut spell_suffix: ResMut<crate::ffxi_actor_render::SpellSuffixCache>,
    mut q_scheds: Query<&mut ActiveSchedulers>,
    mut pending_inserts: Local<HashMap<Entity, Vec<ActiveScheduler>>>,
    actor_root: Res<crate::ffxi_actor_render::ActorDatRoot>,
    mut commands: Commands,
    mut last_seen: Local<u64>,
) {
    let new_count =
        (events.pushed_total.saturating_sub(*last_seen)).min(events.recent.len() as u64) as usize;
    *last_seen = events.pushed_total;
    if new_count == 0 {
        return;
    }
    for ev in events.recent.iter().rev().take(new_count).rev() {
        let kuluu_snapshot::ViewerEvent::ActionStarted {
            actor_id,
            action_id,
            action_kind,
            target_id,
            ..
        } = *ev
        else {
            continue;
        };
        if action_kind != crate::ffxi_actor_render::MAGIC_START_CATEGORY {
            continue;
        }
        let Some(&actor_entity) = tracked.by_id.get(&actor_id) else {
            continue;
        };
        // cmd_arg is the routine FourCC, not a spell id (magic_state.cpp CState); an "sp*" FourCC is
        // an interrupt on the same category (interrupts.cpp MagicInterrupt) and must tear the cast down.
        let magic = ffxi_vocab::magic::magic_start_routine(action_id);
        if magic.is_some_and(|m| m.interrupt) {
            if let Ok(cast) = q_cast.get(actor_entity) {
                sim.stop_routine(actor_entity, cast.routine);
            }
            commands.entity(actor_entity).remove::<CastRoutine>();
            continue;
        }
        let routine = match magic {
            Some(m) => ffxi_dat::datid::DatId::from_name(&m.id),
            None => {
                let suffix = spell_suffix.suffix(actor_root.0.as_deref(), action_id);
                match crate::ffxi_actor_render::action_routine(action_kind, action_id, suffix, None)
                {
                    Some((routine, _looping)) => routine,
                    None => continue,
                }
            }
        };
        let Some(actor_routines) = actor_render_routines(actor_entity, &q_children, &q_render)
        else {
            continue;
        };
        let mut lookup = RoutineLookup::new().with_actor(actor_routines);
        if let Some(g) = global.as_ref() {
            lookup = lookup.with_dat(&g.schedulers);
        }
        let name = routine.0;
        let Some(active) = ActiveScheduler::effects_only(&lookup, &name) else {
            continue;
        };
        enqueue_routine(&mut commands, actor_entity, active);
        commands
            .entity(actor_entity)
            .try_insert(CastRoutine {
                routine: name,
                posed: false,
            })
            .try_insert(ActionTarget(
                target_id.and_then(|id| tracked.by_id.get(&id).copied()),
            ));
    }
    flush_active_scheduler_inserts(&mut pending_inserts, &mut q_scheds, &mut commands);
}

// The victim reaction the attacker's routine will hand off at its 0x2B DamageCallback stage.
// Held on the attacker between the swing dispatch and that stage so the flinch, the impact SE
// and the hurt grunt land on the frame retail invokes the damage callback, not on packet
// arrival (research/xim EffectRoutineInstance.kt handleDamageCallbackRoutine). The outcome is
// stored raw because the reaction is only decidable at callback time: `shld`/`gur1` selection
// depends on what the VICTIM's model ships.
#[derive(Component, Debug, Clone, Copy)]
pub struct PendingHitReaction {
    pub resolution: ffxi_proto::melee::ActionResolution,
    pub outcome: ffxi_proto::melee::ResultOutcome,
    /// The scheduler whose DamageCallback stage is allowed to fire this reaction. Every
    /// completion routine ends at its DamageCallback stage (a spell's `mdam` among them), so an
    /// unqualified pending reaction would be consumed by whichever routine reached its callback
    /// first.
    pub armed_by: [u8; 4],
}

// When the result's info bit carries Defeated, retail flips StatusServer on the same frame as the
// HP packet (.agents/skills/retail-observe/references/2026-09-09-wormwatch-runtime.md "First non-burrow routines"): the victim's death path starts immediately instead of waiting for the next
// 0x0E to report hp_pct 0. Latched on the render-actor child (the pose pass reads it there); it
// dies with the model on despawn/zone change, which is when a fresh `init` would run anyway.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeadFromAction {
    /// Set once the snapshot has caught up (hp_pct 0); the latch is released when the snapshot
    /// then reports the entity alive again (a Raise), so a revived actor stands back up.
    pub snapshot_confirmed: bool,
}

pub fn settle_dead_from_action(
    state: Res<crate::snapshot::SceneState>,
    q_world: Query<&crate::components::WorldEntity>,
    mut q_dead: Query<(Entity, &ChildOf, &mut DeadFromAction)>,
    mut commands: Commands,
) {
    for (child, child_of, mut latch) in &mut q_dead {
        let Ok(world) = q_world.get(child_of.parent()) else {
            continue;
        };
        let Some(snap) = state.snapshot.entities.iter().find(|e| e.id == world.id) else {
            continue;
        };
        match (snap.hp_pct == Some(0), latch.snapshot_confirmed) {
            (true, false) => latch.snapshot_confirmed = true,
            (false, true) => {
                commands.entity(child).remove::<DeadFromAction>();
            }
            _ => {}
        }
    }
}

// The global effect dir's `dam0` chunk is the MELEE hit-reaction switch (`dada` tail-calls it;
// the ranged chain `ldad` uses `daml` instead). Its cases select on `context.hitTypeFlag`
// (research/xim EffectRoutineInstance.kt resolveControlFlowVariable) and their branch order is byte-for-byte the
// ActionResolution values in vendor/server/src/map/enums/action/resolution.h. The Hit branches
// are `damh`/`damg`, and BOTH carry a FlinchOnCaster stage (ROM/0/0.DAT: damh = chih + sdam
// + flinch + vdam; damg = chit + sdam + flinch + vdam) - retail ALWAYS flinches on a hit. `sdam`
// is never a top-level reaction choice: it is only the internal sound call inside dam*/ldam, so
// picking it for models that ship it (as this table used to) made normal hits sound-only with no
// flinch - the "animations not playing" symptom. research/xim leaves the `damh`-vs-`damg`
// selector (var 0x3B) unhandled (EffectRoutineInstance.kt resolveControlFlowVariable warns and defaults to 0),
// which is the `damg` branch - so every non-crit Hit routes to `damg`. A crit routes to `ldam`
// when the lookup resolves it, else back to `damg`; lookup still resolves victim-own-first,
// then global.
pub fn hit_reaction_routine(
    resolution: ffxi_proto::melee::ActionResolution,
    outcome: ffxi_proto::melee::ResultOutcome,
    model_has: impl Fn(&[u8; 4]) -> bool,
) -> Vec<[u8; 4]> {
    use ffxi_proto::melee::ActionResolution;
    let out = match resolution {
        // The crit rides the VICTIM's result block as `info & CriticalHit`
        // (vendor/server/src/map/entities/battle_entity.cpp CBattleEntity::OnAttack). LSB's
        // hitDistortion is the damage share of max HP (action.cpp action_result_t::recordDamage),
        // so it cannot stand in for the flag. None/Light/Medium/Heavy non-crits all play `damg`
        // per retail's dam0 branch table - never sdam, which flinches nothing on its own.
        ActionResolution::Hit if outcome.is_critical() && model_has(b"ldam") => *b"ldam",
        ActionResolution::Hit => *b"damg",
        ActionResolution::Miss => *b"sway",
        ActionResolution::Guard => *b"gurd",
        ActionResolution::Parry => *b"pary",
        ActionResolution::Block if model_has(b"shld") => *b"shld",
        ActionResolution::Block => *b"gur1",
    };
    let mut routines = vec![out];
    if outcome.knockback > 0 && out != *b"sway" {
        routines.push(*b"sway");
    }
    routines
}

// research/xim Actor.kt displayAutoAttack: the swing routine is chosen by which limb struck.
// Direction-of-movement variants (atf0/atb0/atl0/atr0) are not selected here; that needs the
// attacker's locomotion state at swing time. No attacker-side crit swing exists on purpose: LSB
// flags the crit only in the VICTIM's result block (CBattleEntity::OnAttack sets info CriticalHit
// + hitDistortion Heavy from one bool; vendor/server/src/map/entities/battle_entity.cpp) and this
// `animation` field is limb-selected, never
// crit-selected - do not re-add a crit variant here.
pub fn swing_routine(animation: ffxi_proto::melee::AttackAnimation) -> Option<[u8; 4]> {
    use ffxi_proto::melee::AttackAnimation;
    Some(match animation {
        AttackAnimation::RightAttack => *b"ati0",
        AttackAnimation::LeftAttack => *b"bti0",
        AttackAnimation::RightKick => *b"cti0",
        AttackAnimation::LeftKick => *b"dti0",
        AttackAnimation::Throw => return None,
    })
}

// vendor/server/src/map/enums/four_cc.h — BasicAttack's FourCC is "atk0", the self-targeted
// voice routine research/xim Actor.kt displayAutoAttack enqueues alongside the swing.
#[cfg(not(target_arch = "wasm32"))]
const MELEE_VOICE_ROUTINE: [u8; 4] = *b"atk0";

/// KULUU_COMBAT_LOG=1 - live trace of which BATTLE2 results reach the
/// render, what gets armed on the attacker, whether the DamageCallback fires and where the
/// victim's reaction routine resolves. Read-only; no state, no behaviour change (same pattern
/// as KULUU_MOTION_LOG).
#[cfg(not(target_arch = "wasm32"))]
fn combat_log_enabled() -> bool {
    static ONCE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    crate::env_flags::env_flag(&ONCE, "KULUU_COMBAT_LOG")
}

/// Printable form of a FourCC for COMBAT_ log lines.
#[cfg(not(target_arch = "wasm32"))]
fn fourcc(name: [u8; 4]) -> String {
    ffxi_dat::datid::DatId::from_name(&name).as_str()
}

// A basic attack's routines live in the attacker's own battle/equipment dirs and the global effect
// dir, keyed by the swing animation rather than by a DAT file id, which is why the category is
// dispatched here rather than through `action_dat_file_id`. BATTLE2 cmd_arg does carry a FourCC —
// vendor/server/src/map/action/action.cpp action_t::normalize normalize() sets actionid = FourCC::BasicAttack —
// but it is the same constant for every swing, so it selects nothing.
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_melee_action_started(
    mut q_scheds: Query<&mut ActiveSchedulers>,
    events: Res<crate::snapshot::EventLog>,
    tracked: Res<crate::scene::TrackedEntities>,
    q_children: Query<&Children>,
    q_render: Query<&crate::ffxi_actor_render::FfxiRenderActor>,
    global: Option<Res<GlobalEffectDir>>,
    mut pending_inserts: Local<HashMap<Entity, Vec<ActiveScheduler>>>,
    mut commands: Commands,
    mut last_seen: Local<u64>,
) {
    let new_count =
        (events.pushed_total.saturating_sub(*last_seen)).min(events.recent.len() as u64) as usize;
    *last_seen = events.pushed_total;
    if new_count == 0 {
        return;
    }
    for ev in events.recent.iter().rev().take(new_count).rev() {
        let kuluu_snapshot::ViewerEvent::ActionStarted {
            actor_id,
            action_kind,
            target_id,
            result,
            outcome,
            ..
        } = *ev
        else {
            continue;
        };
        let outcome = outcome
            .map(|(info, hit_distortion, knockback)| {
                ffxi_proto::melee::ResultOutcome::from_wire(info, hit_distortion, knockback)
            })
            .unwrap_or_default();
        tracing::debug!(target: "combat", "COMBAT_ACT kind={} actor={} target={:?} result={:?} outcome={:?}",
                action_kind, actor_id, target_id, result, outcome);
        if action_kind != ffxi_proto::melee::CATEGORY_BASIC_ATTACK {
            continue;
        }
        let Some(&actor_entity) = tracked.by_id.get(&actor_id) else {
            tracing::debug!(target: "combat", "COMBAT_DROP actor={} not-tracked", actor_id);
            continue;
        };
        let Some(actor_routines) = actor_render_routines(actor_entity, &q_children, &q_render)
        else {
            tracing::debug!(target: "combat", "COMBAT_DROP actor={} no-actor-routines", actor_id);
            continue;
        };
        let mut lookup = RoutineLookup::new().with_actor(actor_routines);
        if let Some(g) = global.as_ref() {
            lookup = lookup.with_dat(&g.schedulers);
        }
        let raw_result = result;
        let result = raw_result.and_then(|(resolution, animation)| {
            Some((
                ffxi_proto::melee::ActionResolution::from_wire(resolution)?,
                ffxi_proto::melee::AttackAnimation::from_wire(animation)?,
            ))
        });
        if result.is_none() {
            tracing::debug!(target: "combat", "COMBAT_DROP actor={} result-none raw={:?}",
                actor_id, raw_result);
        }
        let resolution = result.map(|(r, _)| r);
        let swing = result
            .and_then(|(_, animation)| swing_routine(animation))
            .filter(|r| lookup.get(r).is_some())
            .unwrap_or(*b"ati0");
        let merged = [MELEE_VOICE_ROUTINE, swing];
        let Some(active) = ActiveScheduler::effects_only_merged(&lookup, &merged) else {
            tracing::debug!(target: "combat", "COMBAT_DROP actor={} merged-none swing={}",
                    actor_id,
                    fourcc(swing));
            continue;
        };
        let armed_by = active.name();
        enqueue_routine(&mut commands, actor_entity, active);
        let victim = target_id.and_then(|id| tracked.by_id.get(&id).copied());
        let mut entity = commands.entity(actor_entity);
        entity.try_insert(ActionTarget(victim));
        match resolution {
            Some(resolution) => {
                entity.try_insert(PendingHitReaction {
                    resolution,
                    outcome,
                    armed_by,
                });
            }
            None => {
                entity.remove::<PendingHitReaction>();
            }
        }
        if resolution.is_some() {
            tracing::debug!(target: "combat", "COMBAT_ARM actor={} target={:?} outcome={:?} swing={} armed_by={}",
                actor_id,
                victim,
                outcome,
                fourcc(swing),
                fourcc(armed_by));
        }
        // Info bit 1 (Defeated): retail flips StatusServer on the same frame as the HP packet
        // (.agents/skills/retail-observe/references/2026-09-09-wormwatch-runtime.md "First non-burrow routines"),
        // so start the victim's death path now instead of waiting for the next 0x0E.
        if outcome.defeated() {
            if combat_log_enabled() {
                tracing::debug!(target: "combat", "COMBAT_DEAD actor={} target={:?} info=0x{:X}",
                        actor_id, victim, outcome.info);
            }
            latch_dead_from_action(victim, &q_children, &q_render, &mut commands);
            // retail's onDisplayDeath enqueues the model's `dead` routine with
            // displayDead=true on the Defeated frame (research/xim Actor.kt onDisplayDeath):
            // ded? fall-over at its first Motion stage, cor0 hold after. Play mode so those
            // Motion stages fire through dispatch_motion_stages; models without a `dead`
            // routine keep the instant-corpse fallback (run_routine_on no-ops on an
            // unresolvable name).
            if let Some(victim) = victim {
                run_routine_on(
                    victim,
                    b"dead",
                    None,
                    &q_children,
                    &q_render,
                    &mut q_scheds,
                    &mut pending_inserts,
                    global.as_deref(),
                    &mut commands,
                );
            }
        }
    }
    flush_active_scheduler_inserts(&mut pending_inserts, &mut q_scheds, &mut commands);
}

/// Latch the victim's death path on its render-actor child (see DeadFromAction). No-op when
/// the victim has no loaded model yet - the entity hp_pct will still take over later. Only
/// caller is dispatch_melee_action_started, which is native-only; gate matches so wasm compiles.
#[cfg(not(target_arch = "wasm32"))]
fn latch_dead_from_action(
    victim: Option<Entity>,
    q_children: &Query<&Children>,
    q_render: &Query<&crate::ffxi_actor_render::FfxiRenderActor>,
    commands: &mut Commands,
) {
    let Some(victim) = victim else { return };
    let Ok(children) = q_children.get(victim) else {
        return;
    };
    let mut latched = false;
    for &child in children {
        if q_render.get(child).is_ok() {
            commands.entity(child).insert(DeadFromAction::default());
            latched = true;
        }
    }
    if latched {
        tracing::debug!(target: "combat", "COMBAT_DEAD_LATCH victim={}", victim.index());
    }
}

// research/xim EffectRoutineInstance.kt handleDamageCallbackRoutine — the 0x2B stage is where retail hands control
// to the damage callback. That is the frame the victim's reaction routine starts, so the flinch
// and impact SE line up with the swing instead of with packet arrival.
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_damage_callback_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    q_pending: Query<(&PendingHitReaction, &ActionTarget)>,
    q_children: Query<&Children>,
    q_render: Query<&crate::ffxi_actor_render::FfxiRenderActor>,
    mut q_active: Query<&mut ActiveSchedulers>,
    mut pending_inserts: Local<HashMap<Entity, Vec<ActiveScheduler>>>,
    global: Option<Res<GlobalEffectDir>>,
    mut commands: Commands,
) {
    for ev in events.read() {
        let stage = &ev.stage.stage;
        if !(stage.kind == StageKind::DamageCallback
            || matches!(
                stage.kind,
                StageKind::SubRoutine | StageKind::BlockingSubRoutine
            ) && stage.id == DADA_IMPACT_MARKER)
        {
            continue;
        }
        let Ok((pending, target)) = q_pending.get(ev.actor) else {
            tracing::debug!(target: "combat", "COMBAT_CB actor={} sched={} no-pending",
                    ev.actor.index(),
                    fourcc(ev.scheduler));
            continue;
        };
        if pending.armed_by != ev.scheduler {
            tracing::debug!(target: "combat", "COMBAT_CB actor={} sched={} armed_by={} mismatch",
                    ev.actor.index(),
                    fourcc(ev.scheduler),
                    fourcc(pending.armed_by));
            continue;
        }
        commands.entity(ev.actor).remove::<PendingHitReaction>();
        let Some(victim) = target.0 else {
            tracing::debug!(target: "combat", "COMBAT_CB actor={} no-victim", ev.actor.index());
            continue;
        };
        let Some(victim_routines) = actor_render_routines(victim, &q_children, &q_render) else {
            tracing::debug!(target: "combat", "COMBAT_CB victim={} no-victim-routines", victim.index());
            continue;
        };
        let mut lookup = RoutineLookup::new().with_actor(victim_routines);
        if let Some(g) = global.as_ref() {
            lookup = lookup.with_dat(&g.schedulers);
        }
        let chosen = hit_reaction_routine(pending.resolution, pending.outcome, |name| {
            lookup.get(name).is_some()
        });
        for routine in &chosen {
            tracing::debug!(target: "combat", "COMBAT_RX victim={} res={:?} outcome={:?} routine={} found={}",
                    victim.index(),
                    pending.resolution,
                    pending.outcome,
                    fourcc(*routine),
                    lookup.get(routine).is_some());
            run_routine_on(
                victim,
                routine,
                Some(ev.actor),
                &q_children,
                &q_render,
                &mut q_active,
                &mut pending_inserts,
                global.as_deref(),
                &mut commands,
            );
        }
    }
    flush_active_scheduler_inserts(&mut pending_inserts, &mut q_active, &mut commands);
}

// research/xim EffectRoutineParser.kt parseSection2 + EffectRoutineInstance.kt createChild newSequences — a 0x09 link
// runs its child ON the primary target, under a context flipped by `cloneWithOverrideTarget`:
// the parent becomes the child's target. Resource lookup follows that flip
// (EffectRoutineInstance.kt appendChildSequences,592-604 searchAssociatedDir), which is the only reason the
// melee hit chain resolves at all — the victim's `damg` links `chit` back onto the ATTACKER, so
// `ef h` is found in the attacker's equipped-weapon DAT and its `hit1` sparks, being
// AttachType::TargetActor, land on the victim again.
// Returns (entity the child runs on, the child's flipped target). A routine with no target of
// its own keeps its child where it is and never flips a context onto itself.
pub fn target_link_context(actor: Entity, target: Option<Entity>) -> (Entity, Option<Entity>) {
    match target {
        Some(t) if t != actor => (t, Some(actor)),
        _ => (actor, None),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_target_routine_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    q_target: Query<&ActionTarget>,
    q_children: Query<&Children>,
    q_render: Query<&crate::ffxi_actor_render::FfxiRenderActor>,
    mut q_active: Query<&mut ActiveSchedulers>,
    mut pending_inserts: Local<HashMap<Entity, Vec<ActiveScheduler>>>,
    global: Option<Res<GlobalEffectDir>>,
    mut commands: Commands,
) {
    for ev in events.read() {
        if ev.stage.stage.kind != StageKind::SubRoutineOnTarget {
            continue;
        }
        let (host, flipped_target) =
            target_link_context(ev.actor, q_target.get(ev.actor).ok().and_then(|t| t.0));
        run_routine_on(
            host,
            &ev.stage.stage.id,
            flipped_target,
            &q_children,
            &q_render,
            &mut q_active,
            &mut pending_inserts,
            global.as_deref(),
            &mut commands,
        );
    }
    flush_active_scheduler_inserts(&mut pending_inserts, &mut q_active, &mut commands);
}

#[cfg(not(target_arch = "wasm32"))]
fn run_routine_on(
    entity: Entity,
    routine: &[u8; 4],
    flipped_target: Option<Entity>,
    q_children: &Query<&Children>,
    q_render: &Query<&crate::ffxi_actor_render::FfxiRenderActor>,
    q_active: &mut Query<&mut ActiveSchedulers>,
    pending_inserts: &mut HashMap<Entity, Vec<ActiveScheduler>>,
    global: Option<&GlobalEffectDir>,
    commands: &mut Commands,
) {
    let Some(routines) = actor_render_routines(entity, q_children, q_render) else {
        return;
    };
    let mut lookup = RoutineLookup::new().with_actor(routines);
    if let Some(g) = global {
        lookup = lookup.with_dat(&g.schedulers);
    }
    let Some(active) = ActiveScheduler::from_routine(&lookup, routine) else {
        return;
    };
    match q_active.get_mut(entity) {
        Ok(mut scheds) => scheds.push(active),
        Err(_) => pending_inserts.entry(entity).or_default().push(active),
    }
    // ActionTarget stays a single entity-level component: first writer wins, stripped
    // when the last routine finishes. Retail's per-sequence target context
    // (research/xim EffectRoutineInstance.kt cloneWithOverrideTarget) is a known
    // simplification here.
    commands
        .entity(entity)
        .try_insert(ActionTarget(flipped_target));
}

#[cfg(not(target_arch = "wasm32"))]
/// Queue a routine on an entity through commands so two routines landing on a not-yet-scheduled
/// entity in one system both survive: a deferred `insert(ActiveSchedulers::one)` would let the
/// second overwrite the first. Commands apply in order, so the push sees the component.
pub fn enqueue_routine(commands: &mut Commands, entity: Entity, active: ActiveScheduler) {
    commands
        .entity(entity)
        .entry::<ActiveSchedulers>()
        .or_default()
        .and_modify(move |mut scheds| scheds.push(active));
}

/// Insert-or-push an ActiveScheduler onto `entity`. Push when the component already exists;
/// otherwise buffer into `pending_inserts` instead of issuing a deferred insert: two routines
/// queued on the same fresh entity in one batch would overwrite each other (last insert wins),
/// which would drop a queued knockback hit's damage reaction to the sway insert. The caller
/// must run [`flush_active_scheduler_inserts`] after all of its queueing.
///
/// Returns true when `entity` had no ActiveSchedulers yet (the insert was buffered), so sites
/// that attach entity-level side components can keep first-writer-wins for them.
pub fn queue_active_scheduler(
    entity: Entity,
    active: ActiveScheduler,
    q_scheds: &mut Query<&mut ActiveSchedulers>,
    pending_inserts: &mut HashMap<Entity, Vec<ActiveScheduler>>,
) -> bool {
    match q_scheds.get_mut(entity) {
        Ok(mut scheds) => {
            scheds.push(active);
            false
        }
        Err(_) => {
            pending_inserts.entry(entity).or_default().push(active);
            true
        }
    }
}

/// Apply the inserts buffered by `queue_active_scheduler` (see there for why): re-check for an
/// ActiveSchedulers that appeared since the call and merge into it, otherwise insert every
/// queued routine at once so none is lost to a deferred-command overwrite. Commands apply per
/// system, so within one batch only our own buffered inserts can change the answer between the
/// call and this flush.
pub fn flush_active_scheduler_inserts(
    pending: &mut HashMap<Entity, Vec<ActiveScheduler>>,
    q_active: &mut Query<&mut ActiveSchedulers>,
    commands: &mut Commands,
) {
    for (entity, entries) in std::mem::take(pending) {
        match q_active.get_mut(entity) {
            Ok(mut scheds) => {
                for entry in entries {
                    scheds.push(entry);
                }
            }
            Err(_) => {
                commands
                    .entity(entity)
                    .insert(ActiveSchedulers::many(entries));
            }
        }
    }
}

// Belt-and-braces stop for the case retail's 0x2D StopParticle stages cannot reach: an
// interrupted cast never runs the spell DAT's `main`. Only a cast that was OBSERVED posing and
// then stopped counts as ended — see CastRoutine::posed.
#[cfg(not(target_arch = "wasm32"))]
pub fn stop_cast_effects_when_cast_ends(
    mut q_cast: Query<(Entity, &mut CastRoutine, &Children)>,
    q_render: Query<&crate::ffxi_actor_render::FfxiRenderActor>,
    mut sim: ResMut<crate::particle_sim::ParticleSimulator>,
    mut commands: Commands,
) {
    for (entity, mut cast, children) in &mut q_cast {
        let Some(actor) = children.iter().find_map(|c| q_render.get(c).ok()) else {
            continue;
        };
        if actor.cast_posing() {
            cast.posed = true;
            continue;
        }
        if !cast.posed {
            continue;
        }
        sim.stop_routine(entity, cast.routine);
        commands.entity(entity).remove::<CastRoutine>();
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_stop_particle_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    mut sim: ResMut<crate::particle_sim::ParticleSimulator>,
) {
    for ev in events.read() {
        if ev.stage.stage.kind != StageKind::StopParticle {
            continue;
        }
        sim.stop_generator(ev.actor, ev.stage.stage.id);
    }
}

// 0x1E ParticleDampen: emission stops and the already-live particles are force-expired at
// once (research/xim EffectRoutineInstance.kt handleParticleEffectDampen).
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_particle_dampen_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    mut sim: ResMut<crate::particle_sim::ParticleSimulator>,
) {
    for ev in events.read() {
        if ev.stage.stage.kind != StageKind::ParticleDampen {
            continue;
        }
        sim.dampen_generator(ev.actor, ev.stage.stage.id);
    }
}

// 0x19 SpellEffect: the spell animation index resolves to the effect DAT at the spell
// file-table offset plus the index; its `main` routine runs on the actor as a child
// sequence (research/xim EffectRoutineInstance.kt handleSpellEffect). The shipped DATs
// carry it only in the summon deploy/pop routines, which no trigger fires yet.
#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_spell_effect_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    q_id: Query<&crate::components::WorldEntity>,
    q_target: Query<&ActionTarget>,
    mut cache: ResMut<ActionDatCache>,
) {
    for ev in events.read() {
        let Some(spell_index) = ev.stage.stage.spell_effect else {
            continue;
        };
        let Some(world) = q_id.get(ev.actor).ok() else {
            continue;
        };
        let target_id = q_target
            .get(ev.actor)
            .ok()
            .and_then(|t| t.0)
            .and_then(|target| q_id.get(target).ok())
            .map(|world| world.id)
            .unwrap_or(0);
        cache.defer(
            ffxi_vocab::action_anim::SPELL_FILE_TABLE_OFFSET + spell_index,
            PendingActionDispatch::Routine {
                actor_id: world.id,
                target_id,
                routine: *b"main",
                duration: ffxi_event::SCHEDULER_DURATION_FROM_DAT,
                cutscene_actor: None,
            },
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_entity_emoted(
    events: Res<crate::snapshot::EventLog>,
    tracked: Res<crate::scene::TrackedEntities>,
    q_look: Query<&crate::components::LookComp>,
    q_children: Query<&Children>,
    mut q_actors: Query<&mut crate::ffxi_actor_render::FfxiRenderActor>,
    dll: Option<Res<ActionMainDll>>,
    mut cache: ResMut<ActionDatCache>,
    mut q_scheds: Query<&mut ActiveSchedulers>,
    mut pending_inserts: Local<HashMap<Entity, Vec<ActiveScheduler>>>,
    mut commands: Commands,
    mut last_seen: Local<u64>,
) {
    use ffxi_proto::map::emote;

    let new_count =
        (events.pushed_total.saturating_sub(*last_seen)).min(events.recent.len() as u64) as usize;
    *last_seen = events.pushed_total;
    if new_count == 0 {
        return;
    }
    for ev in events.recent.iter().rev().take(new_count).rev() {
        let kuluu_snapshot::ViewerEvent::EntityEmoted {
            actor_id,
            target_id,
            emote_id,
            param,
            mode,
        } = *ev
        else {
            continue;
        };
        if mode == emote::mode::TEXT {
            continue;
        }
        // Job emotes (MesNum 74..=95) live in a separate per-job file range
        // not yet mapped (bead kuluu-d4u retail_unknowns) — text only for now.
        if (emote::JOB_MESNUM_BASE..=emote::JOB_MESNUM_MAX).contains(&emote_id) {
            continue;
        }
        let Some(&actor_entity) = tracked.by_id.get(&actor_id) else {
            continue;
        };
        let Some((file_offset, routine)) = ffxi_vocab::emote_anim::emote_routine(emote_id, param)
        else {
            continue;
        };
        let race = q_look.get(actor_entity).ok().and_then(|l| look_race(&l.0));

        if let Some(race) = race {
            let base = dll
                .as_ref()
                .and_then(|d| d.0.as_deref())
                .and_then(|d| d.base_emote_index(race));
            if let Some(base) = base {
                let file_id = base as u32 + file_offset;
                match cache.lru.get_and_promote(file_id) {
                    Some(parsed) => {
                        if apply_routine_dispatch(
                            &parsed,
                            &routine,
                            actor_entity,
                            tracked.by_id.get(&target_id).copied(),
                            &mut q_scheds,
                            &mut pending_inserts,
                            &mut commands,
                        ) {
                            continue;
                        }
                    }
                    // The DAT-vs-local-clip decision needs the parse, so it is deferred with it.
                    None => {
                        cache.defer(
                            file_id,
                            PendingActionDispatch::Routine {
                                actor_id,
                                target_id,
                                routine,
                                duration: ffxi_event::SCHEDULER_DURATION_FROM_DAT,
                                cutscene_actor: None,
                            },
                        );
                        continue;
                    }
                }
            }
        }

        play_local_emote_clip(&routine, actor_entity, &q_children, &mut q_actors);
    }
    flush_active_scheduler_inserts(&mut pending_inserts, &mut q_scheds, &mut commands);
}

// NPC casters (lua sendEmote) and PCs whose emote DAT lacks the routine:
// play the actor's own em0N clip when it has one; silent no-op
// otherwise (XIM findLocalAnimationRoutine, Actor.kt).
#[cfg(not(target_arch = "wasm32"))]
fn play_local_emote_clip(
    routine: &[u8; 4],
    actor_entity: Entity,
    q_children: &Query<&Children>,
    q_actors: &mut Query<&mut crate::ffxi_actor_render::FfxiRenderActor>,
) {
    let clip = ffxi_dat::datid::DatId::from_name(routine);
    let Ok(children) = q_children.get(actor_entity) else {
        return;
    };
    for &child in children {
        if let Ok(mut actor) = q_actors.get_mut(child) {
            actor.begin_completion_motion(
                clip,
                crate::ffxi_actor_render::CompletionMotion {
                    local_clips: &[],
                    duration_frames: 0.0,
                    max_loops: 1,
                    transition_in: crate::ffxi_actor_render::HalfFrames::ZERO,
                    transition_out: crate::ffxi_actor_render::HalfFrames::ZERO,
                },
            );
        }
    }
}

pub struct SchedulerRuntimePlugin;

impl Plugin for SchedulerRuntimePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SchedulerStageEvent>();
        app.add_message::<CutsceneMotionDone>();
        // Prediction and grounding read this on every target (combat_stance.rs
        // predict_entities_system, ground_remote_movers_system); only the native cutscene
        // systems write it.
        app.init_resource::<CutsceneActorState>();

        #[cfg(target_arch = "wasm32")]
        app.add_systems(
            Update,
            (tick_active_schedulers, dispatch_stop_routine_stages).chain(),
        );

        #[cfg(not(target_arch = "wasm32"))]
        {
            app.init_resource::<crate::particle_sim::ParticleSimulator>();
            app.init_resource::<ActionDatCache>();
            // The running cutscene camera route; the advance system
            // (advance_cutscene_camera_task) registers in kuluu's view native module
            // (kuluu/src/view_native/mod.rs), after resolve_camera.
            app.init_resource::<CutsceneCameraTasks>();
            app.init_resource::<ActionDatRoot>();
            app.init_resource::<PendingKnockbacks>();
            app.init_resource::<crate::ffxi_actor_render::SelfKnockback>();
            app.add_systems(Startup, load_global_effect_dir);
            // Ordered ahead of the poll so a root change landing on the same frame as an
            // in-flight dll cannot have the poll's `remove_resource::<ActionMainDllTask>`
            // applied over the freshly spawned one (Bevy applies commands per system,
            // bevy_ecs schedule/config.rs), which has no retry path.
            app.add_systems(
                Update,
                adopt_action_dat_root
                    .run_if(resource_exists_and_changed::<ActionDatRoot>)
                    .before(poll_action_main_dll),
            );
            app.add_systems(
                Update,
                (
                    poll_global_effect_dir,
                    poll_action_main_dll,
                    dispatch_action_started,
                    dispatch_cast_routine_started,
                    dispatch_melee_action_started,
                    dispatch_entity_emoted,
                    dispatch_cutscene_motion,
                    poll_action_dat_tasks,
                )
                    .chain()
                    // The overlay and this chain both drain EventLog with private cursors; the
                    // overlay's "no routine for this action" branch clears the looping action
                    // (ffxi_actor_render.rs dispatch_action_overlay), so it runs before a
                    // completion routine's Motion stage begins here.
                    .after(crate::ffxi_actor_render::dispatch_action_overlay)
                    // Bevy chains tuples of at most 20 systems (bevy_ecs schedule/config.rs,
                    // IntoScheduleConfigs tuple impls), so the inserters and the stage
                    // consumers are two chained halves pinned together: every inserter runs
                    // before tick_active_schedulers, which keeps a routine's frame-0 stages
                    // firing on the frame it is inserted.
                    .before(tick_active_schedulers),
            );
            app.add_systems(
                Update,
                (
                    // Chained after the inserter half so every stage is consumed the same
                    // frame it is written. StopRoutine removal runs right after the tick that
                    // emits its StopRoutine stage (ffxi-dat/src/scheduler.rs StageKind).
                    tick_active_schedulers,
                    dispatch_stop_routine_stages,
                    crate::particle_sim::spawn_actor_auto_run_particles,
                    crate::particle_sim::spawn_particle_generators,
                    dispatch_stop_particle_stages,
                    dispatch_particle_dampen_stages,
                    dispatch_spell_effect_stages,
                    crate::particle_sim::stop_generators_for_despawned_owners,
                    crate::particle_sim::track_attached_origins,
                    crate::particle_sim::tick_particle_simulator,
                    crate::particle_sim::sync_particle_meshes,
                    dispatch_sound_stages,
                    dispatch_motion_stages,
                    dispatch_flinch_stages,
                    collect_knockback_hits,
                    dispatch_knockback_stages,
                    tick_knockbacks,
                    (dispatch_damage_callback_stages, settle_dead_from_action).chain(),
                    dispatch_target_routine_stages,
                )
                    .chain(),
            );
            app.add_systems(
                Update,
                (
                    apply_cutscene_actor_cues,
                    advance_cutscene_walks,
                    release_cutscene_actors,
                )
                    .chain()
                    .after(crate::combat_stance::predict_entities_system)
                    .after(crate::combat_stance::ground_remote_movers_system),
            );
            app.add_systems(
                Update,
                stop_cast_effects_when_cast_ends
                    .after(crate::ffxi_actor_render::tick_live_ffxi_actors),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::scheduler::{SchedulerStage, StageKind};
    use ffxi_vocab::emote_anim::emote_routine;

    /// A look-at must send the actor's +X forward along the offset to its
    /// target, on the same basis every other heading in the renderer uses.
    #[test]
    fn look_at_rotation_faces_the_offset() {
        for (dx, dz) in [
            (1.0, 0.0),
            (0.0, 1.0),
            (-1.0, 0.0),
            (0.0, -1.0),
            (0.6, -0.8),
        ] {
            let forward = look_at_rotation(dx, dz) * Vec3::X;
            let want = Vec3::new(dx, 0.0, dz).normalize();
            assert!(
                (forward - want).length() < 1e-5,
                "offset ({dx}, {dz}): forward {forward:?}, want {want:?}"
            );
        }
        let quarter = (EVENT_HEADING_UNITS / 4.0) as i32;
        let a = event_heading_to_quat(quarter) * Vec3::X;
        let b = look_at_rotation(0.0, 1.0) * Vec3::X;
        assert!((a - b).length() < 1e-4, "{a:?} vs {b:?}");
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn event_coordinates_and_heading_convert_to_bevy_space() {
        // Event y is height; Bevy's up axis is -native-y (scene.rs ffxi_to_bevy
        // convention) like every other placed asset.
        assert_eq!(event_to_world(2000, -500, 400), Vec3::new(2.0, 0.5, -0.4));
        // A quarter circle of event heading steps is a quarter Bevy yaw, signed the way
        // scene.rs's wire-heading convention (heading_to_quat) signs it.
        let q = event_heading_to_quat(1024);
        assert_eq!(
            q,
            Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
            "-TAU * 1024 / 4096 rounds to exactly -FRAC_PI_2 in f32; angle_between's dot of two identical quaternions rounds just under 1.0 and reports about 7e-4 radians for zero rotation"
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn cutscene_actor_server_id_resolves_local_player_and_entities() {
        assert_eq!(
            cutscene_actor_server_id(None, kuluu_snapshot::CutsceneActor::LocalPlayer),
            None
        );
        assert_eq!(
            cutscene_actor_server_id(Some(7), kuluu_snapshot::CutsceneActor::LocalPlayer),
            Some(7)
        );
        assert_eq!(
            cutscene_actor_server_id(
                None,
                kuluu_snapshot::CutsceneActor::Entity {
                    server_id: 0x010E_6001
                }
            ),
            Some(0x010E_6001)
        );
    }

    // whirl_claws (mob skill 259) arrives as category 11 with animation 3; the effect DAT's file
    // id is the range-dependent base plus that index (research/xim resource/table/MobAbilityTable.kt
    // getFileTableOffset).
    #[test]
    fn mob_skill_categories_key_the_effect_dat_by_animation() {
        assert_eq!(
            action_dat_file_id(259, Some(3), 11, None, None),
            Some(0x0F3C + 3)
        );
        assert_eq!(
            action_dat_file_id(259, Some(3), 13, None, None),
            Some(0x0F3C + 3)
        );
        assert_eq!(
            action_dat_file_id(259, None, 11, None, None),
            None,
            "a result-less body carries no animation index and resolves to nothing"
        );
    }

    #[test]
    fn particle_origin_entity_routes_by_attach_type() {
        use ffxi_dat::particle_gen::AttachType;
        let caster = Entity::from_raw_u32(1).unwrap();
        let target = Entity::from_raw_u32(2).unwrap();

        for attach in [
            AttachType::None,
            AttachType::SourceActor,
            AttachType::SourceActorWeapon,
            AttachType::SourceActorTargetFacing,
            AttachType::SourceToTargetBasis,
            AttachType::Sun,
        ] {
            assert_eq!(
                particle_origin_entity(attach, caster, Some(target)),
                caster,
                "{attach:?}"
            );
        }

        for attach in [
            AttachType::TargetActor,
            AttachType::TargetActorSourceFacing,
            AttachType::TargetToSourceBasis,
        ] {
            assert_eq!(
                particle_origin_entity(attach, caster, Some(target)),
                target,
                "{attach:?}"
            );
            assert_eq!(
                particle_origin_entity(attach, caster, None),
                caster,
                "{attach:?} falls back to the caster when the target is untracked"
            );
        }
    }

    // A 0x0B SoundOnTarget is the victim's impact, a 0x53 SoundOnCaster the attacker's whoosh;
    // resolve_stage_to_se hands the flag over and the dispatcher must mix them from different
    // world positions.
    #[test]
    fn sound_origin_entity_routes_by_on_caster_flag() {
        let caster = Entity::from_raw_u32(1).unwrap();
        let target = Entity::from_raw_u32(2).unwrap();

        assert_eq!(sound_origin_entity(true, caster, Some(target)), caster);
        assert_eq!(sound_origin_entity(true, caster, None), caster);
        assert_eq!(sound_origin_entity(false, caster, Some(target)), target);
        assert_eq!(
            sound_origin_entity(false, caster, None),
            caster,
            "an untracked target falls back to the caster instead of silencing the SE"
        );
    }

    // The flag the dispatcher routes on comes straight from the stage kind, so a parser change
    // that stopped distinguishing the two opcodes would silently collapse both to the caster.
    #[test]
    fn resolve_stage_to_se_reports_on_caster_from_the_stage_kind() {
        let seps = HashMap::from([(*b"se01", Sep::parse(*b"se01", &[0u8; 12]).unwrap())]);
        let generators = HashMap::new();

        assert_eq!(
            ffxi_dat::action::resolve_stage_to_se(
                b"se01",
                StageKind::SoundOnCaster,
                &generators,
                &seps
            ),
            Some((0, true))
        );
        assert_eq!(
            ffxi_dat::action::resolve_stage_to_se(
                b"se01",
                StageKind::SoundOnTarget,
                &generators,
                &seps
            ),
            Some((0, false))
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[derive(Resource, Default)]
    struct CapturedSfx(Vec<crate::audio::SfxEvent>);

    #[cfg(not(target_arch = "wasm32"))]
    fn capture_sfx(
        mut reader: MessageReader<crate::audio::SfxEvent>,
        mut out: ResMut<CapturedSfx>,
    ) {
        out.0.extend(reader.read().copied());
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn sep_assets(stage_id: [u8; 4], se_id: u32) -> ActionAssets {
        let mut body = [0u8; 12];
        body[8..12].copy_from_slice(&se_id.to_le_bytes());
        ActionAssets {
            seps: HashMap::from([(stage_id, Sep::parse(stage_id, &body).unwrap())]),
            ..Default::default()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn run_sound_stage(
        kind: StageKind,
        stage_id: [u8; 4],
        assets: ActionAssets,
        caster_pos: Option<Vec3>,
        target: Option<Vec3>,
    ) -> Vec<crate::audio::SfxEvent> {
        let mut app = App::new();
        app.add_message::<SchedulerStageEvent>()
            .add_message::<crate::audio::SfxEvent>()
            .init_resource::<CapturedSfx>()
            .add_systems(Update, (dispatch_sound_stages, capture_sfx).chain());

        let target_entity =
            target.map(|p| app.world_mut().spawn(Transform::from_translation(p)).id());
        let mut caster = app.world_mut().spawn((assets, ActionTarget(target_entity)));
        if let Some(p) = caster_pos {
            caster.insert(Transform::from_translation(p));
        }
        let caster = caster.id();

        app.world_mut().write_message(SchedulerStageEvent {
            actor: caster,
            stage: stage(0, kind, 0, stage_id),
            scheduler: *b"test",
        });
        app.update();
        std::mem::take(&mut app.world_mut().resource_mut::<CapturedSfx>().0)
    }

    /// The spatial SE path: an impact mixes from where the victim is standing, not from the
    /// attacker, and neither may fall back to a 2D cue.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn dispatch_sound_stages_emits_each_stage_kind_from_its_own_actor() {
        const STAGE_ID: [u8; 4] = *b"se01";
        const SE_ID: u32 = 4242;
        let caster_pos = Vec3::new(10.0, 2.0, -40.0);
        let target_pos = Vec3::new(-25.0, 6.0, 120.0);

        for (kind, expected) in [
            (StageKind::SoundOnTarget, target_pos),
            (StageKind::SoundOnCaster, caster_pos),
        ] {
            let got = run_sound_stage(
                kind,
                STAGE_ID,
                sep_assets(STAGE_ID, SE_ID),
                Some(caster_pos),
                Some(target_pos),
            );
            assert_eq!(got.len(), 1, "{kind:?} produced {got:?}");
            assert_eq!(got[0].se_id, SE_ID, "{kind:?}");
            assert_eq!(
                got[0].emitter,
                Some(expected),
                "{kind:?} must mix from {expected:?}"
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn dispatch_sound_stages_falls_back_to_the_caster_and_then_to_a_dry_cue() {
        const STAGE_ID: [u8; 4] = *b"se01";
        const SE_ID: u32 = 4242;
        let caster_pos = Vec3::new(10.0, 2.0, -40.0);

        let untracked_target = run_sound_stage(
            StageKind::SoundOnTarget,
            STAGE_ID,
            sep_assets(STAGE_ID, SE_ID),
            Some(caster_pos),
            None,
        );
        assert_eq!(
            untracked_target.first().map(|e| e.emitter),
            Some(Some(caster_pos)),
            "an untracked target falls back to the caster, not to silence"
        );

        let unpositioned = run_sound_stage(
            StageKind::SoundOnCaster,
            STAGE_ID,
            sep_assets(STAGE_ID, SE_ID),
            None,
            None,
        );
        assert_eq!(
            unpositioned.first().map(|e| e.emitter),
            Some(None),
            "an actor with no transform yet mixes dry rather than from the world origin"
        );
    }

    fn stage(frame: u32, kind: StageKind, raw_type: u8, id: [u8; 4]) -> TimedStage {
        TimedStage {
            frame,
            stage: SchedulerStage {
                stage_words: ffxi_dat::scheduler::SYNTHESIZED_STAGE_WORDS,
                kind,
                raw_type,
                delay_frames: 0,
                duration_frames: 0,
                id,
                max_loops: 0,
                transition_in: 0,
                transition_out: 0,
                random_group: None,
                local_dir: ffxi_dat::scheduler::NO_LOCAL_DIR,
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

    fn make_scheduler(name: [u8; 4], stages: Vec<TimedStage>) -> Scheduler {
        Scheduler { name, stages }
    }

    /// ActionAssets holds a routine's decoded textures/meshes and ActionTarget its aim; both
    /// leave with ActiveSchedulers at the post-finish TTL or every actor that ever ran an
    /// action retains one action DAT's decoded asset set (and a stale target) until despawn.
    #[test]
    fn tick_active_schedulers_strips_action_components_after_ttl() {
        let mut app = App::new();
        app.add_message::<SchedulerStageEvent>()
            .add_message::<CutsceneMotionDone>()
            .init_resource::<Time>()
            .add_systems(Update, tick_active_schedulers);

        let actor = app
            .world_mut()
            .spawn((
                ActiveSchedulers::one(ActiveScheduler::from_scheduler(&make_scheduler(
                    *b"test",
                    Vec::new(),
                ))),
                ActionAssets::default(),
                ActionTarget(None),
            ))
            .id();

        let half_ttl = std::time::Duration::from_secs_f32(POST_FINISH_TTL_SECS / 2.0);
        app.world_mut().resource_mut::<Time>().advance_by(half_ttl);
        app.update();
        let entity = app.world().entity(actor);
        assert!(entity.contains::<ActiveSchedulers>());
        assert!(entity.contains::<ActionAssets>());
        assert!(entity.contains::<ActionTarget>());

        app.world_mut().resource_mut::<Time>().advance_by(half_ttl);
        app.update();
        let entity = app.world().entity(actor);
        assert!(!entity.contains::<ActiveSchedulers>());
        assert!(!entity.contains::<ActionAssets>());
        assert!(!entity.contains::<ActionTarget>());
    }

    #[derive(Resource, Default)]
    struct CapturedStages(Vec<SchedulerStageEvent>);

    fn capture_stages(
        mut reader: MessageReader<SchedulerStageEvent>,
        mut out: ResMut<CapturedStages>,
    ) {
        out.0.extend(reader.read().copied());
    }

    // The whole point of the vec: a hit reaction that lands mid-swing runs on the same entity
    // without touching the swing's cursor or frame (retail's ActionTimer1 counted 2 and 3;
    // .agents/skills/retail-observe/references/2026-09-09-wormwatch-runtime.md "First non-burrow routines"
    // and "Target-side reactions"). Each entry keeps its own clock; entries retire on their OWN
    // finish+TTL, so a short
    // reaction can lapse while the swing still runs.
    #[test]
    fn concurrent_routines_keep_separate_cursors_and_strip_together() {
        let mut app = App::new();
        app.add_message::<SchedulerStageEvent>()
            .add_message::<CutsceneMotionDone>()
            .init_resource::<Time>()
            .init_resource::<CapturedStages>()
            .add_systems(Update, (tick_active_schedulers, capture_stages).chain());

        let swing = make_scheduler(
            *b"ati0",
            vec![stage(59, StageKind::SoundOnCaster, 0x53, *b"snd1")],
        );
        let reaction = make_scheduler(
            *b"damg",
            vec![stage(19, StageKind::SoundOnCaster, 0x53, *b"snd2")],
        );
        let mut scheds = ActiveSchedulers::one(ActiveScheduler::from_scheduler(&swing));
        scheds.push(ActiveScheduler::from_scheduler(&reaction));
        let actor = app.world_mut().spawn(scheds).id();

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.5));
        app.update();
        let fired: Vec<[u8; 4]> =
            std::mem::take(&mut app.world_mut().resource_mut::<CapturedStages>().0)
                .iter()
                .map(|e| e.scheduler)
                .collect();
        assert_eq!(
            fired,
            vec![*b"damg"],
            "only the reaction's stage has fired yet"
        );

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.5));
        app.update();
        assert!(
            app.world().entity(actor).contains::<ActiveSchedulers>(),
            "both are finished but neither is past its finish+TTL yet"
        );

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.5));
        app.update();
        let entity = app.world().entity(actor);
        assert!(entity.contains::<ActiveSchedulers>());
        let scheds = entity.get::<ActiveSchedulers>().unwrap();
        assert_eq!(
            scheds.routines.len(),
            1,
            "the swing outlives the reaction's TTL"
        );
        assert_eq!(scheds.routines[0].name, *b"ati0");

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.6));
        app.update();
        assert!(
            !app.world().entity(actor).contains::<ActiveSchedulers>(),
            "past the swing's own finish+TTL, everything is stripped"
        );
    }

    #[derive(Resource, Default)]
    struct CapturedMotionDone(Vec<CutsceneMotionDone>);

    fn capture_motion_done(
        mut reader: MessageReader<CutsceneMotionDone>,
        mut out: ResMut<CapturedMotionDone>,
    ) {
        out.0.extend(reader.read().copied());
    }

    /// The WAITSCHEDULOR past a SCHEDULOR cue (ffxi-event/src/vm.rs OP_WAITSCHEDULOR)
    /// parks on this report: it fires the frame the routine's last stage lands,
    /// exactly once, carrying the wire actor and key the cue named.
    #[test]
    fn a_marked_routine_reports_done_exactly_once_on_finish() {
        let mut app = App::new();
        app.add_message::<SchedulerStageEvent>()
            .add_message::<CutsceneMotionDone>()
            .init_resource::<Time>()
            .init_resource::<CapturedMotionDone>()
            .add_systems(
                Update,
                (tick_active_schedulers, capture_motion_done).chain(),
            );

        let actor = kuluu_snapshot::CutsceneActor::Entity {
            server_id: 0x010E_6032,
        };
        let mut sched = ActiveScheduler::from_scheduler(&make_scheduler(
            *b"kue0",
            vec![stage(30, StageKind::SoundOnCaster, 0x53, *b"snd1")],
        ));
        sched.cutscene_motion_actor = Some(actor);
        let entity = app.world_mut().spawn(ActiveSchedulers::one(sched)).id();

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.1));
        app.update();
        assert!(
            app.world().resource::<CapturedMotionDone>().0.is_empty(),
            "an unfinished routine does not report"
        );

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.5));
        app.update();
        let done = std::mem::take(&mut app.world_mut().resource_mut::<CapturedMotionDone>().0);
        assert_eq!(
            done,
            vec![CutsceneMotionDone {
                actor,
                key: *b"kue0"
            }],
            "the routine finished on this frame; the report carries the wire actor and key"
        );

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.5));
        app.update();
        assert!(app.world().entity(entity).contains::<ActiveSchedulers>());
        assert!(
            app.world().resource::<CapturedMotionDone>().0.is_empty(),
            "the report fires exactly once"
        );
    }

    #[test]
    fn unmarked_routines_never_report_done() {
        let mut app = App::new();
        app.add_message::<SchedulerStageEvent>()
            .add_message::<CutsceneMotionDone>()
            .init_resource::<Time>()
            .init_resource::<CapturedMotionDone>()
            .add_systems(
                Update,
                (tick_active_schedulers, capture_motion_done).chain(),
            );

        let actor = app
            .world_mut()
            .spawn(ActiveSchedulers::one(ActiveScheduler::from_scheduler(
                &make_scheduler(
                    *b"em01",
                    vec![stage(5, StageKind::SoundOnCaster, 0x53, *b"snd1")],
                ),
            )))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.5));
        app.update();
        assert!(
            app.world().entity(actor).contains::<ActiveSchedulers>(),
            "the routine is finished but still within its TTL"
        );
        assert!(
            app.world().resource::<CapturedMotionDone>().0.is_empty(),
            "only a 0x2C-marked routine reports"
        );
    }

    // 0x5F StopRoutine drops the named entry and only that one (xim EffectRoutineInstance.kt:
    // 910-915 stops each matching sequence on the same actor).
    #[test]
    fn stop_routine_stage_removes_only_the_named_entry() {
        let mut app = App::new();
        app.add_message::<SchedulerStageEvent>()
            .add_message::<CutsceneMotionDone>()
            .init_resource::<Time>()
            .add_systems(
                Update,
                (tick_active_schedulers, dispatch_stop_routine_stages).chain(),
            );

        let init = make_scheduler(
            *b"init",
            vec![stage(59, StageKind::SoundOnCaster, 0x53, *b"snd0")],
        );
        let dig = make_scheduler(
            *b"ini1",
            vec![stage(0, StageKind::StopRoutine, 0x5F, *b"init")],
        );
        let mut scheds = ActiveSchedulers::one(ActiveScheduler::from_scheduler(&init));
        scheds.push(ActiveScheduler::from_scheduler(&dig));
        let actor = app.world_mut().spawn(scheds).id();

        app.update();
        let scheds = app.world().entity(actor).get::<ActiveSchedulers>().unwrap();
        assert_eq!(scheds.routines.len(), 1);
        assert_eq!(
            scheds.routines[0].name, *b"ini1",
            "the stopper survives its own stage"
        );
    }

    /// The lock test is per-routine and interval-based: a routine with no AnimationLock stage
    /// does not lock, and an overlapping second routine keeps the entity locked past either one's
    /// own interval end - the refcount>0 behaviour retail measured on ActionTimer1.
    #[test]
    fn animation_lock_is_refcounted_across_concurrent_routines() {
        let lock_stage = |frame: u32, raw: u8, dur: u16| -> TimedStage {
            let mut t = stage(frame, StageKind::AnimationLock, raw, *b"    ");
            t.stage.duration_frames = dur;
            t
        };
        let swing = ActiveScheduler::from_scheduler(&make_scheduler(
            *b"ati0",
            vec![lock_stage(0, 0x07, 50)],
        ));
        let reaction = ActiveScheduler::from_scheduler(&make_scheduler(
            *b"damg",
            vec![lock_stage(28, 0x59, 30)],
        ));

        assert!(swing.locks_at(10), "the swing locks from its frame-0 stage");
        assert!(
            !swing.locks_at(50),
            "the swing's interval is half-open at the end"
        );
        let plain = ActiveScheduler::from_scheduler(&make_scheduler(
            *b"cate",
            vec![stage(0, StageKind::SoundOnCaster, 0x53, *b"snd1")],
        ));
        assert!(
            !plain.locks_at(10),
            "a routine with no lock stage never locks"
        );

        let mut both = ActiveSchedulers::one(swing);
        both.push(reaction);
        for (frame, locked) in [(10u32, true), (49, true), (57, true), (58, false)] {
            let mut probe = both.clone();
            for r in &mut probe.routines {
                r.elapsed = frame as f32 / ROUTINE_FPS;
            }
            assert_eq!(probe.is_locked_now(), locked, "frame {frame}");
        }
    }

    // 0x2E is the movement twin of 0x59: same interval rules, a different lock. The 0x2E
    // parse stays data even though no routine locks a player's movement (record:
    // "Players are never movement-locked by a cast or a ranged aim").
    #[test]
    fn movement_lock_interval_is_independent_of_the_animation_lock() {
        let lock_stage = |frame: u32, kind: StageKind, raw: u8, dur: u16| -> TimedStage {
            let mut t = stage(frame, kind, raw, *b"    ");
            t.stage.duration_frames = dur;
            t
        };
        let cast = ActiveScheduler::from_scheduler(&make_scheduler(
            *b"cate",
            vec![
                lock_stage(0, StageKind::AnimationLock, 0x59, 60),
                lock_stage(4, StageKind::MovementLock, 0x2E, 50),
            ],
        ));

        assert!(cast.locks_at(10), "the animation lock holds from frame 0");
        assert!(
            !cast.movement_locks_at(2),
            "the movement lock starts on its own frame"
        );
        assert!(cast.movement_locks_at(4));
        assert!(cast.movement_locks_at(53));
        assert!(
            !cast.movement_locks_at(54),
            "the movement interval is half-open at the end"
        );
        assert!(
            cast.locks_at(54),
            "the animation lock outlives the movement lock"
        );

        let anim_only = ActiveScheduler::from_scheduler(&make_scheduler(
            *b"ini1",
            vec![lock_stage(0, StageKind::AnimationLock, 0x07, 30)],
        ));
        assert!(
            !anim_only.movement_locks_at(10),
            "an animation-only routine never reads as a movement lock"
        );
    }

    // 0x75 SetModelVisibility: the hidden-slot set starts ranged-only (slot 2) - retail's
    // default (research/xim ActorModel.kt getHiddenSlotIds) - and each active stage sets its
    // slot for the interval, the later stage winning while both are live.
    #[test]
    fn hidden_model_slots_fold_the_ranged_default_with_the_active_overrides() {
        let vis_stage =
            |frame: u32, hidden: bool, slot: u16, if_engaged: bool, dur: u16| -> TimedStage {
                let mut t = stage(frame, StageKind::SetModelVisibility, 0x75, *b"    ");
                t.stage.duration_frames = dur;
                t.stage.model_visibility = Some(ModelVisibility {
                    hidden,
                    slot,
                    if_engaged,
                });
                t
            };

        let idle = ActiveSchedulers::one(ActiveScheduler::from_scheduler(&make_scheduler(
            *b"ini1",
            vec![stage(0, StageKind::AnimationLock, 0x59, *b"    ")],
        )));
        assert_eq!(
            idle.hidden_model_slots_now(false),
            [false, false, true, false, false]
        );

        let cast = ActiveSchedulers::one(ActiveScheduler::from_scheduler(&make_scheduler(
            *b"cate",
            vec![
                vis_stage(0, true, 0, false, 30),
                vis_stage(10, false, 0, false, 30),
            ],
        )));
        for (frame, main_hidden) in [(0u32, true), (10, false), (30, false), (39, false)] {
            let mut probe = cast.clone();
            for r in &mut probe.routines {
                r.elapsed = frame as f32 / ROUTINE_FPS;
            }
            assert_eq!(
                probe.hidden_model_slots_now(false),
                [main_hidden, false, true, false, false],
                "frame {frame}"
            );
        }

        // An ifEngaged override applies only to an engaged actor (research/xim ActorModel.kt
        // getHiddenSlotIds ifEngaged gate).
        let engaged_only = ActiveSchedulers::one(ActiveScheduler::from_scheduler(&make_scheduler(
            *b"atkr",
            vec![vis_stage(0, false, 2, true, 60)],
        )));
        assert_eq!(
            engaged_only.hidden_model_slots_now(false),
            [false, false, true, false, false]
        );
        assert_eq!(
            engaged_only.hidden_model_slots_now(true),
            [false, false, false, false, false]
        );

        let mut both = ActiveSchedulers::one(ActiveScheduler::from_scheduler(&make_scheduler(
            *b"cate",
            vec![vis_stage(0, true, 1, false, 60)],
        )));
        both.push(ActiveScheduler::from_scheduler(&make_scheduler(
            *b"damg",
            vec![vis_stage(0, false, 1, false, 60)],
        )));
        assert_eq!(
            both.hidden_model_slots_now(false),
            [false, false, true, false, false],
            "the later routine re-shows sub"
        );
    }

    /// A routine's timeline ends when its last stage ends: a trailing AnimationLock longer than
    /// the post-finish TTL must not be retired (and its lock released) while it still holds.
    #[test]
    fn last_frame_covers_a_trailing_lock_duration() {
        let mut lock = stage(0, StageKind::AnimationLock, 0x07, *b"    ");
        lock.stage.duration_frames = 300;
        let held = ActiveScheduler::from_scheduler(&make_scheduler(
            *b"ini1",
            vec![
                stage(0, StageKind::SoundOnCaster, 0x53, *b"snd1"),
                lock,
                stage(40, StageKind::SoundOnCaster, 0x53, *b"snd2"),
            ],
        ));
        assert_eq!(held.last_frame(), 300);
        assert!(held.locks_at(299));
        assert!(!held.locks_at(300));
    }

    // Every completion routine ends in a 0x2B DamageCallback (a spell's `mdam`), so a melee
    // reaction armed by the swing must not be consumed by an unrelated routine reaching its
    // callback first. The DamageCallback dispatcher gates on this pairing.
    #[test]
    fn pending_hit_reaction_is_bound_to_the_scheduler_that_armed_it() {
        let pending = PendingHitReaction {
            resolution: ffxi_proto::melee::ActionResolution::Hit,
            outcome: ffxi_proto::melee::ResultOutcome::default(),
            armed_by: *b"atk0",
        };
        assert_eq!(pending.armed_by, *b"atk0");
        assert_ne!(
            pending.armed_by, *b"mdam",
            "a spell's mdam must not match a melee-armed reaction"
        );
    }

    /// The fall-over window: pending from insertion until the first Motion stage fires; routines
    /// without a Motion stage (instant-corpse fallback models) are not reported, nor are routines
    /// under any other name.
    #[test]
    fn dead_fall_over_pending_tracks_the_first_motion() {
        let mut scheds = ActiveSchedulers::one(ActiveScheduler::from_scheduler(&make_scheduler(
            *b"dead",
            vec![
                stage(0, StageKind::SoundOnCaster, 0x53, *b"vded"),
                stage(0, StageKind::Motion, 0x05, *b"ded?"),
                stage(100, StageKind::Motion, 0x05, *b"cor0"),
            ],
        )));
        assert!(
            scheds.dead_fall_over_pending(),
            "queued with the fall-over unfired"
        );

        scheds.routines[0].cursor = 2;
        assert!(
            !scheds.dead_fall_over_pending(),
            "the frame-0 tick fired both stages, so the fall-over has started"
        );

        let bare = ActiveSchedulers::one(ActiveScheduler::from_scheduler(&make_scheduler(
            *b"dead",
            vec![stage(0, StageKind::SoundOnCaster, 0x53, *b"vded")],
        )));
        assert!(
            !bare.dead_fall_over_pending(),
            "no Motion stage, never pending"
        );

        let other = ActiveSchedulers::one(ActiveScheduler::from_scheduler(&make_scheduler(
            *b"damg",
            vec![stage(0, StageKind::Motion, 0x05, *b"gud?")],
        )));
        assert!(
            !other.dead_fall_over_pending(),
            "only the `dead` routine reports"
        );
    }

    #[test]
    fn hit_reaction_routine_table() {
        use ffxi_proto::melee::ActionResolution as R;
        use ffxi_proto::melee::{ResultOutcome, INFO_CRITICAL_HIT};
        let has = |names: Vec<[u8; 4]>| move |name: &[u8; 4]| names.iter().any(|n| n == name);
        let o = |info: u8, hit_distortion: u8, knockback: u8| {
            ResultOutcome::from_wire(info, hit_distortion, knockback)
        };
        let crit = |knockback: u8| o(INFO_CRITICAL_HIT, 3, knockback);

        assert_eq!(
            hit_reaction_routine(R::Hit, crit(0), has(vec![*b"ldam"])),
            vec![*b"ldam"]
        );
        assert_eq!(
            hit_reaction_routine(R::Hit, crit(0), has(vec![])),
            vec![*b"damg"]
        );
        // hitDistortion is the damage share of max HP, not the flag: a Heavy non-crit stays on
        // damg and a Light crit still plays ldam (ffxi-proto/src/melee.rs hit_distortion).
        assert_eq!(
            hit_reaction_routine(R::Hit, o(0, 3, 0), has(vec![*b"ldam"])),
            vec![*b"damg"]
        );
        assert_eq!(
            hit_reaction_routine(R::Hit, o(INFO_CRITICAL_HIT, 1, 0), has(vec![*b"ldam"])),
            vec![*b"ldam"]
        );
        assert_eq!(
            hit_reaction_routine(R::Hit, o(0, 0, 0), has(vec![*b"sdam"])),
            vec![*b"damg"]
        );
        assert_eq!(
            hit_reaction_routine(R::Hit, o(0, 1, 0), has(vec![*b"sdam"])),
            vec![*b"damg"]
        );
        assert_eq!(
            hit_reaction_routine(R::Hit, o(0, 0, 0), has(vec![])),
            vec![*b"damg"]
        );
        assert_eq!(
            hit_reaction_routine(R::Hit, o(0, 2, 0), has(vec![*b"sdam"])),
            vec![*b"damg"]
        );
        assert_eq!(
            hit_reaction_routine(R::Guard, o(0, 0, 0), has(vec![])),
            vec![*b"gurd"]
        );
        assert_eq!(
            hit_reaction_routine(R::Parry, o(0, 0, 0), has(vec![])),
            vec![*b"pary"]
        );
        assert_eq!(
            hit_reaction_routine(R::Block, o(0, 0, 0), has(vec![*b"shld"])),
            vec![*b"shld"]
        );
        assert_eq!(
            hit_reaction_routine(R::Block, o(0, 0, 0), has(vec![])),
            vec![*b"gur1"]
        );
        assert_eq!(
            hit_reaction_routine(R::Miss, o(0, 0, 0), has(vec![])),
            vec![*b"sway"]
        );
        assert_eq!(
            hit_reaction_routine(R::Miss, o(0, 0, 2), has(vec![])),
            vec![*b"sway"]
        );
        assert_eq!(
            hit_reaction_routine(R::Hit, crit(1), has(vec![*b"ldam"])),
            vec![*b"ldam", *b"sway"]
        );
        assert_eq!(
            hit_reaction_routine(R::Hit, crit(1), has(vec![])),
            vec![*b"damg", *b"sway"]
        );
    }

    // effects_only_merged names the merged timeline after the first routine that resolved, which
    // is what a stage event reports as its scheduler — the value the reaction is armed with.
    #[test]
    fn merged_scheduler_reports_its_first_resolved_routine_as_its_name() {
        let voice = make_scheduler(*b"atk0", vec![stage(0, StageKind::Motion, 0x01, *b"at0?")]);
        let swing = make_scheduler(*b"ati0", vec![stage(2, StageKind::Motion, 0x01, *b"ati?")]);
        let dat = [voice, swing];
        let lookup = RoutineLookup::new().with_dat(&dat);

        let merged = ActiveScheduler::effects_only_merged(&lookup, &[*b"atk0", *b"ati0"]).unwrap();
        assert_eq!(merged.name(), *b"atk0");

        let only_swing =
            ActiveScheduler::effects_only_merged(&lookup, &[*b"zzzz", *b"ati0"]).unwrap();
        assert_eq!(
            only_swing.name(),
            *b"ati0",
            "an absent voice routine leaves the swing as the merged name"
        );
    }

    #[test]
    fn current_frame_advances_by_fps() {
        let sched = make_scheduler(*b"main", vec![]);
        let mut a = ActiveScheduler::from_scheduler(&sched);
        a.elapsed = 0.5;
        assert_eq!(a.current_frame(), 30);
        a.elapsed = 1.0;
        assert_eq!(a.current_frame(), 60);
    }

    // research/xim util/Fps.kt `internalFps = 60.0` is the clock effect routines and particle
    // generators are authored against; poc/ActorManager.kt updateAll halves it — and only it — for
    // skeletal animation. Neither constant may be "fixed" without the other.
    #[test]
    fn routine_clock_is_double_the_skeleton_clock() {
        assert_eq!(ROUTINE_FPS, 60.0);
        assert_eq!(crate::ffxi_actor_render::FRAME_RATE, 30.0);
        assert_eq!(
            ROUTINE_FPS,
            SKELETON_FRAME_DIVISOR * crate::ffxi_actor_render::FRAME_RATE
        );
    }

    // A stage authored 60 frames after its predecessor lands one second later, not two.
    #[test]
    fn stage_delay_60_fires_after_one_second() {
        const DELAY_FRAMES: u32 = 60;
        let sched = make_scheduler(
            *b"main",
            vec![
                stage(0, StageKind::SoundOnCaster, 0x53, *b"snd0"),
                stage(DELAY_FRAMES, StageKind::SoundOnCaster, 0x53, *b"snd1"),
            ],
        );
        let mut a = ActiveScheduler::from_scheduler(&sched);

        a.elapsed = 0.9;
        assert_eq!(a.current_frame(), 54);
        assert!(a.current_frame() < DELAY_FRAMES);

        a.elapsed = 1.05;
        assert!(a.current_frame() >= DELAY_FRAMES);
    }

    // Retail-byte fixture (skips without an install): Cure's effect DAT (file 2801 = 0xAF1) runs
    // its target routine `tgt0` out to frame 239 — 3.98 s at the authored 60 fps. That frame is
    // the routine's own `totalDelay` header field (research/xim EffectRoutineParser.kt read), the
    // DAT's independent statement of its length, which the summed stage delays must reproduce.
    #[test]
    fn real_dat_cure_target_routine_completes_in_retail_wall_time() {
        const CURE_FILE: u32 = 2801;
        const TGT0_LAST_FRAME: u32 = 239;
        const TGT0_SECS: f32 = 3.983;

        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let Ok(loc) = root.resolve(CURE_FILE) else {
            return;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            return;
        };
        let (schedulers, _, _) = parse_action_bytes(&bytes);
        let tgt0 = schedulers
            .iter()
            .find(|s| &s.name == b"tgt0")
            .expect("cure DAT has a tgt0 routine");
        let last = tgt0.stages.last().expect("tgt0 has stages").frame;
        assert_eq!(last, TGT0_LAST_FRAME);
        let secs = last as f32 / ROUTINE_FPS;
        assert!(
            (secs - TGT0_SECS).abs() < 0.1,
            "cure tgt0 runs {secs}s, retail authors {TGT0_SECS}s"
        );
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
        assert_eq!(a.end_frame(), 0);
    }

    /// A trailing AnimationLock holds until its own end frame: `end_frame` is the max over all
    /// stages of fire time + duration_frames, so this routine locks [0, 130) and retirement
    /// (which counts down from that end frame plus the post-finish TTL) does not release it early.
    #[test]
    fn trailing_lock_holds_until_its_end_frame_not_the_ttl() {
        let mut lk = stage(0, StageKind::AnimationLock, 0x07, *b"lk01");
        lk.stage.duration_frames = 130;
        let sched = make_scheduler(*b"lock", vec![lk]);

        let a = ActiveScheduler::from_scheduler(&sched);
        assert_eq!(a.end_frame(), 130);
        assert!(a.locks_at(129), "frame 129 is inside [0, 130)");
        assert!(!a.locks_at(130), "the lock ends at frame 130");

        let mut app = App::new();
        app.add_message::<SchedulerStageEvent>()
            .add_message::<CutsceneMotionDone>()
            .init_resource::<Time>()
            .add_systems(Update, tick_active_schedulers);
        let actor = app
            .world_mut()
            .spawn(ActiveSchedulers::one(ActiveScheduler::from_scheduler(
                &sched,
            )))
            .id();

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(125.0 / ROUTINE_FPS));
        app.update();
        let scheds = app
            .world()
            .entity(actor)
            .get::<ActiveSchedulers>()
            .expect("the entry must survive to tick 125, inside its lock window");
        assert!(scheds.is_locked_now(), "tick 125 is locked");

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(15.0 / ROUTINE_FPS));
        app.update();
        let scheds = app
            .world()
            .entity(actor)
            .get::<ActiveSchedulers>()
            .expect("the entry retires only after end_frame + TTL, not at the lock's end");
        assert!(
            !scheds.is_locked_now(),
            "tick 140 is past the [0, 130) window"
        );
    }

    /// End-to-end against the installed retail DATs (skips without them):
    /// /bow on a HumeM resolves to a routine whose Motion fires at frame 0
    /// with the bow? clip, and the file's assets carry the matching clips —
    /// the two defects that made emotes play the wrong clip 5s late.
    #[test]
    fn real_dat_bow_routine_fires_bow_clip_at_frame_zero() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let Ok(dll) = ffxi_dat::main_dll::MainDll::load(root.root()) else {
            return;
        };
        let base = dll.base_emote_index(1).expect("HumeM emote base") as u32;
        let (offset, routine) = emote_routine(1, 0).expect("bow is mapped");
        let loc = root.resolve(base + offset).expect("emote file resolves");
        let bytes = std::fs::read(loc.path_under(&root)).expect("emote DAT readable");
        let (schedulers, assets, _cameras) = parse_action_bytes(&bytes);
        let active = ActiveScheduler::from_main(&schedulers, &routine).expect("em00 exists");
        let motion = active
            .stages
            .iter()
            .find(|t| t.stage.kind == StageKind::Motion)
            .expect("bow routine has a Motion stage");
        assert_eq!(motion.frame, 0, "bow motion fires immediately");
        assert_eq!(&motion.stage.id, b"bow?");
        let clip_id = ffxi_dat::datid::DatId::from_name(&motion.stage.id);
        assert!(
            assets
                .animations
                .iter()
                .any(|a| a.id.parameterized_match(&clip_id)),
            "emote file carries bow clips matching the parameterized id"
        );
    }

    /// Pins the empirically-derived emote table against the DAT dump
    /// (examples/zz-emote-probe.rs clip mnemonics) and the XIM HELM points
    /// (Actor.kt onGatheringAttempt). Point/Bow swap and Salute nation variants are
    /// the file-0 irregularities the old id/8 hypothesis got wrong.
    #[test]
    fn emote_table_matches_dat_clip_mnemonics() {
        assert_eq!(emote_routine(1, 0), Some((0, *b"em00")), "bow → bow? clip");
        assert_eq!(
            emote_routine(0, 0),
            Some((0, *b"em01")),
            "point → poi? clip"
        );
        assert_eq!(
            emote_routine(2, 0),
            Some((0, *b"em02")),
            "salute san d'oria → sl1?"
        );
        assert_eq!(
            emote_routine(2, 2),
            Some((0, *b"em04")),
            "salute windurst → sl3?"
        );
        assert_eq!(
            emote_routine(2, 9),
            Some((0, *b"em04")),
            "salute clamps unknown nations"
        );
        assert_eq!(emote_routine(3, 0), Some((0, *b"em05")), "kneel → kne?");
        assert_eq!(emote_routine(5, 0), Some((0, *b"em07")), "cry → wee?");
        assert_eq!(emote_routine(6, 0), Some((1, *b"em00")), "no → den?");
        assert_eq!(emote_routine(8, 0), Some((1, *b"em02")), "wave → wav?");
        assert_eq!(
            emote_routine(9, 0),
            Some((1, *b"em03")),
            "goodbye → wav? (second)"
        );
        assert_eq!(emote_routine(13, 0), Some((1, *b"em07")), "clap → clp?");
        assert_eq!(emote_routine(32, 0), Some((4, *b"em02")), "think → thk?");
        assert_eq!(emote_routine(36, 0), Some((4, *b"em06")), "psych → gut?");
        assert_eq!(emote_routine(37, 0), Some((4, *b"em07")));
        assert_eq!(
            emote_routine(40, 0),
            Some((5, *b"em00")),
            "logging → ono0 axe (XIM 5,0)"
        );
        assert_eq!(
            emote_routine(41, 0),
            Some((6, *b"em00")),
            "excavation → turu pickaxe (XIM 6,0)"
        );
        assert_eq!(
            emote_routine(42, 0),
            Some((7, *b"em00")),
            "harvesting → kama sickle (XIM 7,0)"
        );
        assert_eq!(emote_routine(44, 0), Some((11, *b"em00")), "toss → tos?");
        assert_eq!(emote_routine(65, 0), Some((12, *b"em00")), "dance1 → dc0?");
        assert_eq!(emote_routine(68, 0), Some((12, *b"em03")), "dance4 → dc3?");
        assert_eq!(
            emote_routine(38, 0),
            None,
            "shocked has no body routine in the era DATs"
        );
        assert_eq!(emote_routine(39, 0), None, "id gap");
        assert_eq!(emote_routine(45, 0), None, "id gap");
    }

    #[test]
    fn parse_action_bytes_handles_empty_input() {
        let (scheds, assets, _cameras) = parse_action_bytes(&[]);
        assert!(scheds.is_empty());
        assert!(assets.generators.is_empty());
        assert!(assets.seps.is_empty());
        #[cfg(not(target_arch = "wasm32"))]
        assert!(assets.d3ms.is_empty());
    }

    // End-to-end against the installed retail DATs (skips without them): Poison's completion
    // effect (file 3020, 'veno') carries a 0x0E SpriteSheet particle cloud backed by a 0x21
    // 'fir' sheet. The fir0/fir1/fir2 generators must parse as SpriteSheet defs whose mesh_id
    // resolves to a retained sprite sheet — the regression that dropped every 0x0E generator so
    // only the neutral pk00/pk01 smoke survived. Cure (file 2801) must be unaffected: its
    // static-mesh (0x0B) particle defs still parse.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_dat_poison_renders_sprite_sheet_particles_cure_unaffected() {
        use ffxi_dat::particle_gen::ParticleMeshKind;

        const POISON_FILE: u32 = 3020;
        const CURE_FILE: u32 = 2801;

        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let Ok(loc) = root.resolve(POISON_FILE) else {
            return;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            return;
        };
        let (_scheds, assets, _cameras) = parse_action_bytes(&bytes);

        assert!(
            !assets.sprite_sheets.is_empty(),
            "poison DAT carries at least one 0x21 sprite sheet"
        );
        for name in [b"fir0", b"fir1", b"fir2"] {
            let def = assets
                .particle_defs
                .get(name)
                .unwrap_or_else(|| panic!("{} generator present", String::from_utf8_lossy(name)));
            assert_eq!(
                def.mesh_kind,
                ParticleMeshKind::SpriteSheet,
                "{} is a SpriteSheet particle",
                String::from_utf8_lossy(name)
            );
            assert!(
                assets.sprite_sheets.contains_key(&def.mesh_id),
                "{}'s mesh {} resolves to a retained sprite sheet",
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(&def.mesh_id),
            );
        }

        let Ok(cure_loc) = root.resolve(CURE_FILE) else {
            return;
        };
        let Ok(cure_bytes) = std::fs::read(cure_loc.path_under(&root)) else {
            return;
        };
        let (_s, cure_assets, _) = parse_action_bytes(&cure_bytes);
        assert!(
            cure_assets
                .particle_defs
                .values()
                .any(|d| d.mesh_kind == ParticleMeshKind::StaticMesh),
            "cure still parses its static-mesh particle generators"
        );
    }

    // Retail-DAT coupling guard (skips without an install): Poison's 0x21 'fir' sheet names its
    // backing Img with the qualified pair ("venom1", "fir"). Looking the Img up by the sheet's
    // namespace token alone misses, which is what rendered the venom cloud as an untextured
    // quad (kuluu-7jpq). research/xim DatResource.kt getTextureResourceByNameAs matches qualified, then local.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_dat_poison_sheet_name_indexes_its_backing_img() {
        const POISON_FILE: u32 = 3020;

        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let Ok(loc) = root.resolve(POISON_FILE) else {
            return;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            return;
        };
        let (_scheds, assets, _cameras) = parse_action_bytes(&bytes);

        let sheet = assets
            .sprite_sheets
            .values()
            .find(|s| s.id == "fir")
            .expect("poison DAT carries the 'fir' sprite sheet");
        assert_eq!(sheet.category, "venom1");
        assert!(
            assets
                .images_by_qualified_name
                .contains_key(&(sheet.category.clone(), sheet.id.clone())),
            "the sheet's qualified name indexes an Img chunk"
        );
        assert!(
            assets.images_by_name.contains_key(&sheet.id),
            "the local-name fallback tier also indexes it"
        );
        assert!(
            !assets.images_by_name.contains_key(&sheet.category),
            "the namespace token is NOT a local-name key — the original miss"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn read_dat(file_id: u32) -> Option<Vec<u8>> {
        let root = ffxi_dat::archive::open_test_install()?;
        let loc = root.resolve(file_id).ok()?;
        std::fs::read(loc.path_under(&root)).ok()
    }

    /// The Home Point's `bind` names `tub0`, which ROM/0/0 also defines: the actor tier has to
    /// win over the global dir, and the routine's own assets over both.
    #[test]
    fn assets_holding_searches_routine_then_actor_then_global() {
        let mut local = ActionAssets::default();
        let mut actor = ActionAssets::default();
        let mut global = ActionAssets::default();
        local
            .seps
            .insert(*b"l   ", Sep::parse(*b"l   ", &[0u8; 12]).unwrap());
        actor
            .seps
            .insert(*b"a   ", Sep::parse(*b"a   ", &[0u8; 12]).unwrap());
        actor
            .seps
            .insert(*b"both", Sep::parse(*b"both", &[0u8; 12]).unwrap());
        global
            .seps
            .insert(*b"g   ", Sep::parse(*b"g   ", &[0u8; 12]).unwrap());
        global
            .seps
            .insert(*b"both", Sep::parse(*b"both", &[0u8; 12]).unwrap());
        let find = |name: &[u8; 4]| {
            assets_holding(Some(&local), Some(&actor), Some(&global), |a| {
                a.seps.contains_key(name)
            })
            .map(|a| a as *const ActionAssets)
        };
        assert_eq!(find(b"l   "), Some(&local as *const _));
        assert_eq!(find(b"a   "), Some(&actor as *const _));
        assert_eq!(find(b"g   "), Some(&global as *const _));
        assert_eq!(
            find(b"both"),
            Some(&actor as *const _),
            "actor tier beats global"
        );
        assert_eq!(find(b"none"), None);
        assert_eq!(
            assets_holding(None, None, Some(&global), |a| a.seps.contains_key(b"g   "))
                .map(|a| a as *const ActionAssets),
            Some(&global as *const _)
        );
    }

    /// Retail-DAT guard (skips without an install): the Home Point model (DAT 1351, ROM/3/25)
    /// ships its activation as routine `bind` — the four non-auto-run generators plus the Sep
    /// section named `6023`, whose embedded id is 16023 — which the actor-motion cue starts
    /// through the actor tier, so both halves have to resolve from the model's own assets.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_dat_home_point_bind_routine_resolves_from_the_model() {
        const HOME_POINT_MODEL_DAT: u32 = 1351;
        const ACTIVATION_SE: u32 = 16023;
        const ACTIVATION_GENERATORS: [&[u8; 4]; 4] = [b"pou1", b"tub0", b"pou0", b"sil2"];

        let Some(bytes) = read_dat(HOME_POINT_MODEL_DAT) else {
            return;
        };
        let (schedulers, assets, _cameras) = parse_action_bytes(&bytes);
        let active = ActiveScheduler::from_main(&schedulers, b"bind").expect("bind routine");

        let particles: Vec<[u8; 4]> = active
            .stages
            .iter()
            .filter(|t| t.stage.kind == StageKind::Particle)
            .map(|t| t.stage.id)
            .collect();
        for name in ACTIVATION_GENERATORS {
            assert!(
                particles.contains(name),
                "{:?} missing {:?}",
                particles,
                name
            );
            assert!(
                !assets.particle_defs[name].auto_run,
                "activation generators do not auto-run"
            );
        }

        let sounds: Vec<u32> = active
            .stages
            .iter()
            .filter_map(|t| {
                ffxi_dat::action::resolve_stage_to_se(
                    &t.stage.id,
                    t.stage.kind,
                    &assets.generators,
                    &assets.seps,
                )
            })
            .map(|(se, _)| se)
            .collect();
        assert_eq!(sounds, vec![ACTIVATION_SE]);
        assert!(
            ActiveScheduler::from_main(&schedulers, b"aper").is_some(),
            "the idle routine is the other scheduler in the file"
        );
    }

    // Retail-DAT guard (skips without an install): the cast aura `ner1` and its `stbk` shutdown
    // live in ROM/0/0.DAT, XIM's GlobalDirectory (research/xim poc/MainTool.kt resourceDependenciesLoaded systemEffects) — not in the
    // caster's own DAT — so a DAT-root or resolver change cannot silently un-resolve them.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_dat_global_dir_holds_cast_aura_and_its_stop() {
        const AURA_GENERATORS: [&[u8; 4]; 4] = [b"gn10", b"gn11", b"gn12", b"gn13"];

        let Some(bytes) = read_dat(GLOBAL_EFFECT_DIR_FILE_ID) else {
            return;
        };
        let (schedulers, assets, _cameras) = parse_action_bytes(&bytes);

        let ner1 = schedulers
            .iter()
            .find(|s| &s.name == b"ner1")
            .expect("global effect dir holds the cast aura routine");
        let spawned: Vec<[u8; 4]> = ner1
            .stages
            .iter()
            .filter(|t| t.stage.kind == StageKind::Particle)
            .map(|t| t.stage.id)
            .collect();
        for gen_id in AURA_GENERATORS {
            assert!(
                spawned.contains(gen_id),
                "ner1 spawns {}",
                String::from_utf8_lossy(gen_id)
            );
            assert!(
                assets.particle_defs.contains_key(gen_id),
                "the global dir also carries {}'s generator def",
                String::from_utf8_lossy(gen_id)
            );
        }

        let stbk = schedulers
            .iter()
            .find(|s| &s.name == b"stbk")
            .expect("global effect dir holds the cast-aura stop routine");
        let stopped: Vec<[u8; 4]> = stbk
            .stages
            .iter()
            .filter(|t| t.stage.kind == StageKind::StopParticle)
            .map(|t| t.stage.id)
            .collect();
        for gen_id in AURA_GENERATORS {
            assert!(
                stopped.contains(gen_id),
                "stbk stops {}",
                String::from_utf8_lossy(gen_id)
            );
        }
    }

    // Retail-DAT guard (skips without an install): HumeM's black-magic cast routine `cabk` is a
    // Motion stage plus two SubRoutine calls that only resolve in the global dir, so the flatten
    // has to span both tiers. `effects_only` drops the Motion because the caster's looping cast
    // pose is owned by ffxi_actor_render::dispatch_action_overlay.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn cast_routine_flattens_across_actor_and_global_dirs() {
        const HUME_M_SKELETON_FILE: u32 = 7072;
        const CAST_ROUTINE: [u8; 4] = *b"cabk";

        let (Some(actor_bytes), Some(global_bytes)) = (
            read_dat(HUME_M_SKELETON_FILE),
            read_dat(GLOBAL_EFFECT_DIR_FILE_ID),
        ) else {
            return;
        };
        let (actor_scheds, _, _) = parse_action_bytes(&actor_bytes);
        let (global_scheds, _, _) = parse_action_bytes(&global_bytes);
        let lookup = RoutineLookup::new()
            .with_dat(&actor_scheds)
            .with_dat(&global_scheds);

        let full = ActiveScheduler::from_routine(&lookup, &CAST_ROUTINE).expect("cabk exists");
        assert!(
            full.stages
                .iter()
                .any(|t| t.stage.kind == StageKind::Motion && &t.stage.id == b"mb0?"),
            "the full cast routine still carries the mb0? cast motion"
        );

        let effects = ActiveScheduler::effects_only(&lookup, &CAST_ROUTINE).expect("cabk exists");
        assert!(
            effects
                .stages
                .iter()
                .all(|t| t.stage.kind != StageKind::Motion),
            "effects_only suppresses every Motion stage"
        );
        let particles = effects
            .stages
            .iter()
            .filter(|t| t.stage.kind == StageKind::Particle)
            .count();
        assert!(
            particles >= 4,
            "the aura's generators are inlined from the global dir, got {particles}"
        );

        assert_eq!(
            ActiveScheduler::effects_only(
                &RoutineLookup::new().with_dat(&actor_scheds),
                &CAST_ROUTINE
            )
            .expect("cabk exists")
            .stages
            .iter()
            .filter(|t| {
                !matches!(
                    t.stage.kind,
                    StageKind::Unknown | StageKind::StartRoutineMarker
                )
            })
            .count(),
            0,
            "without the global tier the aura sub-routines resolve to nothing — the original bug"
        );
    }

    // Retail-DAT guard (skips without an install): the gate guard's Signet cast (zone 231,
    // event 32762) is spell DAT 3297's `main` over 0x73. Its 0x3C `shwh` invoke routine lives
    // in the caster's skeleton DAT, and `shwh`'s `sswh` holds the cast's Motion stages — so a
    // flatten over the spell DAT alone drops the arm raise (research/xim DatResource.kt
    // invokeWhiteMagic).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn spell_main_flattens_the_caster_invoke_routine_from_the_actor_tier() {
        const SIGNET_SPELL_FILE: u32 = 3297;
        const HUME_M_SKELETON_FILE: u32 = 7072;

        let (Some(spell_bytes), Some(actor_bytes), Some(global_bytes)) = (
            read_dat(SIGNET_SPELL_FILE),
            read_dat(HUME_M_SKELETON_FILE),
            read_dat(GLOBAL_EFFECT_DIR_FILE_ID),
        ) else {
            return;
        };
        let (spell_scheds, _, _) = parse_action_bytes(&spell_bytes);
        let (actor_scheds, _, _) = parse_action_bytes(&actor_bytes);
        let (global_scheds, _, _) = parse_action_bytes(&global_bytes);

        let spell_only = ActiveScheduler::from_main(&spell_scheds, b"main").expect("main exists");
        assert!(
            spell_only
                .stages
                .iter()
                .all(|t| t.stage.kind != StageKind::Motion),
            "the spell DAT alone carries no cast motion — shwh is on the caster"
        );

        let lookup = RoutineLookup::new()
            .with_dat(&spell_scheds)
            .with_dat(&actor_scheds)
            .with_dat(&global_scheds);
        let full = ActiveScheduler::from_routine(&lookup, b"main").expect("main exists");
        let motions: Vec<[u8; 4]> = full
            .stages
            .iter()
            .filter(|t| t.stage.kind == StageKind::Motion)
            .map(|t| t.stage.id)
            .collect();
        assert!(
            motions.contains(b"mw1?") && motions.contains(b"mw2?"),
            "the skeleton tier's sswh cast motion must flatten into main, got {motions:?}"
        );
    }

    // The same flatten driven through the runtime the 0x73 cue lands in: the
    // routine ticks, its Motion stages start the caster's action, and the pose
    // path runs with the routine's own lock state. The cast's pose must be
    // released by the routine's authored end, and CutsceneEnded must release
    // whatever the routine still holds (research/XiEvents/OpCodes/0x0073.md).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn signet_cast_releases_the_caster_pose_at_event_end() {
        const SIGNET_SPELL_FILE: u32 = 3297;
        const HUME_M_SKELETON_FILE: u32 = 7072;
        const GUARD_ID: u32 = 0x010E_704F;

        let (Some(spell_bytes), Some(actor_bytes), Some(global_bytes)) = (
            read_dat(SIGNET_SPELL_FILE),
            read_dat(HUME_M_SKELETON_FILE),
            read_dat(GLOBAL_EFFECT_DIR_FILE_ID),
        ) else {
            return;
        };
        let (spell_scheds, _, _) = parse_action_bytes(&spell_bytes);
        let (actor_scheds, actor_assets, _) = parse_action_bytes(&actor_bytes);
        let (global_scheds, _, _) = parse_action_bytes(&global_bytes);

        // The tiers poll_action_dat_tasks assembles for the 0x73 cue.
        let lookup = RoutineLookup::new()
            .with_dat(&spell_scheds)
            .with_dat(&actor_scheds)
            .with_dat(&global_scheds);
        let mut active = ActiveScheduler::from_routine(&lookup, b"main").expect("main exists");
        active.cutscene_motion_actor = Some(kuluu_snapshot::CutsceneActor::Entity {
            server_id: GUARD_ID,
        });
        let end_frame = active.end_frame();

        // The guard as load_pc builds it: a WorldEntity parent running the
        // routine, the render-actor child carrying the skeleton's clips.
        let actor = crate::ffxi_actor_render::render_actor_with_skeleton_clips(
            GUARD_ID,
            actor_assets.animations.clone(),
        );

        let mut app = App::new();
        app.add_message::<SchedulerStageEvent>()
            .add_message::<CutsceneMotionDone>()
            .init_resource::<Time>()
            .add_systems(
                Update,
                (
                    tick_active_schedulers,
                    dispatch_motion_stages,
                    signet_pose_pass,
                )
                    .chain(),
            );

        let child = app.world_mut().spawn(actor).id();
        let parent = app
            .world_mut()
            .spawn((
                crate::components::WorldEntity {
                    id: GUARD_ID,
                    act_index: 0,
                    kind: kuluu_snapshot::EntityKind::Pc,
                },
                Transform::default(),
                ActiveSchedulers::one(active),
                ActionAssets::default(),
                ActionTarget(None),
            ))
            .id();
        app.world_mut().entity_mut(child).insert(ChildOf(parent));

        // One pose tick per frame to the routine's authored end plus the
        // post-finish TTL: the point tick_active_schedulers retires the entry,
        // where a self-releasing pose is already idle.
        let step = std::time::Duration::from_secs_f32(1.0 / crate::ffxi_actor_render::FRAME_RATE);
        let ticks = ((end_frame as f32 / ROUTINE_FPS + POST_FINISH_TTL_SECS)
            * crate::ffxi_actor_render::FRAME_RATE)
            .ceil() as u32;
        for _ in 0..ticks {
            app.world_mut().resource_mut::<Time>().advance_by(step);
            app.update();
        }

        let actor = app
            .world()
            .entity(child)
            .get::<crate::ffxi_actor_render::FfxiRenderActor>()
            .unwrap();
        assert!(
            !actor.has_action(),
            "the cast's pose must clear by the routine's authored end (lock {} frames, routine end frame {end_frame})",
            active_lock_frames(&lookup)
        );
        assert!(
            actor.is_pose_idle(),
            "the caster is idle by the routine's authored end"
        );

        // CutsceneEnded: release whatever the routine still holds.
        app.init_resource::<crate::snapshot::EventLog>();
        app.init_resource::<crate::snapshot::SceneState>();
        app.init_resource::<crate::scene::TrackedEntities>();
        app.init_resource::<CutsceneActorState>();
        app.add_systems(Update, release_cutscene_actors);
        app.world_mut()
            .resource_mut::<crate::snapshot::EventLog>()
            .push(kuluu_snapshot::ViewerEvent::CutsceneEnded);
        app.world_mut()
            .resource_mut::<crate::scene::TrackedEntities>()
            .by_id
            .insert(GUARD_ID, parent);
        app.world_mut().resource_mut::<Time>().advance_by(step);
        app.update();

        let entity = app.world().entity(parent);
        assert!(!entity.contains::<ActiveSchedulers>());
        let actor = app
            .world()
            .entity(child)
            .get::<crate::ffxi_actor_render::FfxiRenderActor>()
            .unwrap();
        assert!(!actor.has_action(), "the event end releases the cast pose");
    }

    /// A stop-action cue kills the routine and releases the pose it started:
    /// the queue entry drops, the held action clears, and the emptied entity is
    /// stripped in the same cue (research/XiEvents/OpCodes/0x0050.md).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stop_action_cue_clears_the_cast_pose_and_strips_the_entity() {
        const NPC_ID: u32 = 0x010E_704F;
        const CAST_CLIP: [u8; 4] = *b"mw2?";
        const HUME_M_SKELETON_FILE: u32 = 7072;

        let Some(actor_bytes) = read_dat(HUME_M_SKELETON_FILE) else {
            return;
        };
        let (_, actor_assets, _) = parse_action_bytes(&actor_bytes);
        let clips = actor_assets.animations.clone();
        let clip = ffxi_dat::datid::DatId::from_name(&CAST_CLIP);
        assert!(
            clips.iter().any(|a| a.id.parameterized_match(&clip)),
            "the stub must own the cast clip"
        );

        let mut lock = stage(0, StageKind::AnimationLock, 0x07, *b"lock");
        lock.stage.duration_frames = 600;
        let motion = stage(1, StageKind::Motion, 0x05, CAST_CLIP);
        let active = ActiveScheduler::from_scheduler(&make_scheduler(*b"cast", vec![lock, motion]));

        let mut app = actor_cue_app();
        app.add_message::<SchedulerStageEvent>()
            .add_message::<CutsceneMotionDone>()
            .init_resource::<Time>()
            .add_systems(
                Update,
                (tick_active_schedulers, dispatch_motion_stages).chain(),
            );

        let child = app
            .world_mut()
            .spawn(crate::ffxi_actor_render::render_actor_with_skeleton_clips(
                NPC_ID, clips,
            ))
            .id();
        let parent = app
            .world_mut()
            .spawn((
                crate::components::WorldEntity {
                    id: NPC_ID,
                    act_index: 0,
                    kind: kuluu_snapshot::EntityKind::Pc,
                },
                Transform::default(),
                ActiveSchedulers::one(active),
                ActionAssets::default(),
                ActionTarget(None),
            ))
            .id();
        app.world_mut().entity_mut(child).insert(ChildOf(parent));

        // One tick: the Motion stage fires and starts the cast's pose.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(
                1.0 / crate::ffxi_actor_render::FRAME_RATE,
            ));
        app.update();
        let actor = app
            .world()
            .entity(child)
            .get::<crate::ffxi_actor_render::FfxiRenderActor>()
            .unwrap();
        assert!(actor.has_action(), "the Motion stage must start the pose");
        assert!(
            app.world().entity(parent).contains::<ActiveSchedulers>(),
            "the routine is still queued before the stop"
        );

        // The stop cue: kill the routine, clear the pose, strip the entity.
        app.world_mut()
            .resource_mut::<crate::snapshot::EventLog>()
            .push(kuluu_snapshot::ViewerEvent::Cutscene {
                cue: CutsceneCue::ActorStopAction {
                    actor: kuluu_snapshot::CutsceneActor::Entity { server_id: NPC_ID },
                    key: None,
                },
            });
        app.world_mut()
            .resource_mut::<crate::scene::TrackedEntities>()
            .by_id
            .insert(NPC_ID, parent);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(
                1.0 / crate::ffxi_actor_render::FRAME_RATE,
            ));
        app.update();

        let entity = app.world().entity(parent);
        assert!(
            !entity.contains::<ActiveSchedulers>(),
            "an emptied queue is stripped with the stop cue"
        );
        assert!(!entity.contains::<ActionAssets>());
        assert!(!entity.contains::<ActionTarget>());
        let mut actor = app
            .world_mut()
            .get_mut::<crate::ffxi_actor_render::FfxiRenderActor>(child)
            .unwrap();
        assert!(!actor.has_action(), "the stop cue clears the held pose");
        crate::ffxi_actor_render::advance_actor_pose_standalone_locked(&mut actor, 1.0, false);
        assert!(actor.is_pose_idle(), "one pose pass after the stop is idle");
    }

    /// The flattened routine's AnimationLock span, for the failure message.
    fn active_lock_frames(lookup: &RoutineLookup) -> u32 {
        ActiveScheduler::from_routine(lookup, b"main")
            .expect("main exists")
            .stages
            .iter()
            .filter(|t| t.stage.kind == StageKind::AnimationLock)
            .map(|t| t.stage.duration_frames as u32)
            .sum()
    }

    /// The pose pass the full app runs, with the routine's own lock state: the
    /// test double for ffxi_actor_render's pose system in the signet test.
    fn signet_pose_pass(
        time: Res<Time>,
        q_parent: Query<(Entity, Option<&ActiveSchedulers>), With<crate::components::WorldEntity>>,
        q_children: Query<&Children>,
        mut q_actors: Query<&mut crate::ffxi_actor_render::FfxiRenderActor>,
    ) {
        let dt = time.delta_secs();
        for (parent, scheds) in &q_parent {
            let locked = scheds.is_some_and(|s| s.is_locked_now());
            let Ok(children) = q_children.get(parent) else {
                continue;
            };
            for &child in children {
                let Ok(mut actor) = q_actors.get_mut(child) else {
                    continue;
                };
                crate::ffxi_actor_render::advance_actor_pose_standalone_locked(
                    &mut actor,
                    dt * crate::ffxi_actor_render::FRAME_RATE,
                    locked,
                );
            }
        }
    }

    fn tagged_stage(
        frame: u32,
        kind: StageKind,
        raw_type: u8,
        id: [u8; 4],
        random_group: Option<u16>,
    ) -> TimedStage {
        let mut t = stage(frame, kind, raw_type, id);
        t.stage.random_group = random_group;
        t
    }

    // The whole point of the melee path: `ati0` (weapon-motion DAT) links `skaz` with 0x57, and
    // `skaz` resolves in the EQUIPPED WEAPON's DAT — three tiers away from the routine that
    // named it. Flattening must carry the link across and keep the frame the whoosh is authored
    // at (research/xim EffectRoutineParser.kt parseSection2).
    #[test]
    fn melee_swing_flattens_to_the_weapon_swing_sound() {
        const SKAZ_FRAME: u32 = 34;
        let weapon_motion = vec![
            make_scheduler(
                *b"ati0",
                vec![
                    stage(0, StageKind::Motion, 0x05, *b"at0?"),
                    stage(SKAZ_FRAME, StageKind::SubRoutine, 0x57, *b"skaz"),
                ],
            ),
            make_scheduler(
                *b"atk0",
                vec![stage(0, StageKind::SubRoutine, 0x57, *b"vatk")],
            ),
        ];
        let weapon_item = vec![make_scheduler(
            *b"skaz",
            vec![stage(0, StageKind::SoundOnCaster, 0x0A, *b"skaz")],
        )];
        let face = vec![make_scheduler(
            *b"vatk",
            vec![tagged_stage(
                0,
                StageKind::SoundOnCaster,
                0x0A,
                *b"atk1",
                Some(0),
            )],
        )];
        let lookup = RoutineLookup::new()
            .with_dat(&weapon_motion)
            .with_dat(&weapon_item)
            .with_dat(&face);

        let active = ActiveScheduler::effects_only_merged(&lookup, &[*b"atk0", *b"ati0"])
            .expect("the swing flattens");
        let whoosh = active
            .stages
            .iter()
            .find(|t| &t.stage.id == b"skaz" && t.stage.kind == StageKind::SoundOnCaster)
            .expect("the weapon swing sound survives the 0x57 link");
        assert_eq!(whoosh.frame, SKAZ_FRAME);
        assert!(
            active
                .stages
                .iter()
                .any(|t| &t.stage.id == b"atk1" && t.stage.kind == StageKind::SoundOnCaster),
            "the merged voice routine contributes its grunt"
        );
        assert!(
            active
                .stages
                .iter()
                .all(|t| t.stage.kind != StageKind::Motion),
            "the body clip stays with dispatch_action_overlay — running both double-fires it"
        );
    }

    // research/xim EffectRoutineParser.kt parseSection2 — one alternative per activation. Four
    // simultaneous `vatk` grunts is the regression this guards.
    #[test]
    fn random_block_contributes_exactly_one_member() {
        let dat = vec![make_scheduler(
            *b"vatk",
            (0..4)
                .map(|i| {
                    tagged_stage(
                        0,
                        StageKind::SoundOnCaster,
                        0x0A,
                        [b'a', b't', b'k', b'1' + i as u8],
                        Some(0),
                    )
                })
                .collect(),
        )];
        let lookup = RoutineLookup::new().with_dat(&dat);
        for _ in 0..16 {
            let active = ActiveScheduler::from_routine(&lookup, b"vatk").expect("flattens");
            assert_eq!(active.stages.len(), 1, "exactly one grunt per swing");
            assert!(active.stages[0].stage.id.starts_with(b"atk"));
        }
    }

    // `dada` tail-calls `dam0`, ten mutually exclusive additional-effect branches keyed on a
    // condition we do not evaluate (research/xim EffectRoutineParser.kt parseSection2). Inlining it
    // would fire every branch at once.
    #[test]
    fn control_flow_switch_is_not_inlined() {
        let mut switch = make_scheduler(
            *b"dam0",
            vec![
                stage(0, StageKind::SubRoutineOnTarget, 0x09, *b"sb00"),
                stage(0, StageKind::SubRoutineOnTarget, 0x09, *b"sb01"),
            ],
        );
        switch
            .stages
            .push(stage(0, StageKind::Unknown, 0x6B, *b"    "));
        let dat = vec![
            make_scheduler(
                *b"dada",
                vec![
                    stage(0, StageKind::DamageCallback, 0x2B, *b"    "),
                    stage(0, StageKind::SubRoutine, 0x03, *b"dam0"),
                ],
            ),
            switch,
        ];
        let lookup = RoutineLookup::new().with_dat(&dat);
        let active = ActiveScheduler::from_routine(&lookup, b"dada").expect("flattens");
        assert!(
            active.stages.iter().all(|t| !t.stage.id.starts_with(b"sb")),
            "no branch of an unevaluated switch is taken"
        );
        assert!(
            active
                .stages
                .iter()
                .any(|t| t.stage.kind == StageKind::DamageCallback),
            "the damage callback still reaches the runtime"
        );
    }

    // /D2 - a control-flow child survives flattening as a marker stage at its call
    // frame: with a degraded global dir, `call dada` (the impact carrier) must still fire the
    // reaction instead of vanishing. The inlined DamageCallback and the dam0 marker both land at call +
    // delay; no branch of the switch is taken.
    #[test]
    fn control_flow_call_survives_flattening_as_a_marker() {
        let mut switch = make_scheduler(
            *b"dam0",
            vec![stage(0, StageKind::SubRoutineOnTarget, 0x09, *b"sb00")],
        );
        switch
            .stages
            .push(stage(0, StageKind::Unknown, 0x6B, *b"    "));
        let dat = vec![
            make_scheduler(
                *b"swng",
                vec![stage(32, StageKind::SubRoutine, 0x57, *b"dada")],
            ),
            make_scheduler(
                *b"dada",
                vec![
                    stage(4, StageKind::DamageCallback, 0x2B, *b"    "),
                    stage(4, StageKind::SubRoutine, 0x03, *b"dam0"),
                ],
            ),
            switch,
        ];
        let lookup = RoutineLookup::new().with_dat(&dat);
        let active = ActiveScheduler::from_routine(&lookup, b"swng").expect("flattens");

        let cb = active
            .stages
            .iter()
            .find(|t| t.stage.kind == StageKind::DamageCallback)
            .expect("the inlined 0x2B survives");
        assert_eq!(cb.frame, 36, "call frame + the callback's own delay");
        let marker = active
            .stages
            .iter()
            .find(|t| {
                matches!(
                    t.stage.kind,
                    StageKind::SubRoutine | StageKind::BlockingSubRoutine
                ) && &t.stage.id == b"dam0"
            })
            .expect("the control-flow call survives as a marker");
        assert_eq!(marker.frame, 36);
        assert!(
            active.stages.iter().all(|t| !t.stage.id.starts_with(b"sb")),
            "no branch of an unevaluated switch is taken"
        );
    }

    /// With the global dir degraded to empty, `call dada` does not resolve; the call still
    /// survives as a marker, so dispatch_damage_callback_stages fires at the impact frame rather
    /// than losing the callback.
    #[test]
    fn unresolvable_dada_call_survives_flattening_as_a_marker() {
        let dat = vec![make_scheduler(
            *b"ati0",
            vec![stage(32, StageKind::SubRoutine, 0x57, *b"dada")],
        )];
        let lookup = RoutineLookup::new().with_dat(&dat);
        let active = ActiveScheduler::from_routine(&lookup, b"ati0").expect("flattens");
        assert_eq!(active.stages.len(), 1, "got {:?}", active.stages);
        assert!(matches!(
            active.stages[0].stage.kind,
            StageKind::SubRoutine | StageKind::BlockingSubRoutine
        ));
        assert_eq!(&active.stages[0].stage.id, b"dada");
        assert_eq!(active.stages[0].frame, 32);
    }

    /// Every other unresolvable call is still dropped (`aloc` and friends stay inert).
    #[test]
    fn unresolvable_calls_other_than_dada_stay_dropped() {
        let dat = vec![make_scheduler(
            *b"ati0",
            vec![stage(0, StageKind::SubRoutine, 0x57, *b"aloc")],
        )];
        let lookup = RoutineLookup::new().with_dat(&dat);
        let active = ActiveScheduler::from_routine(&lookup, b"ati0").expect("flattens");
        assert!(active.stages.is_empty(), "got {:?}", active.stages);
    }

    // A 0x09 link stays a stage rather than being inlined, so the runtime can start it on the
    // VICTIM and resolve the victim's own `sdam`/`vdam` (EffectRoutineParser.kt parseSection2).
    #[test]
    fn target_link_is_not_flattened_into_the_caster_timeline() {
        let dat = vec![
            make_scheduler(
                *b"dcnt",
                vec![stage(0, StageKind::SubRoutineOnTarget, 0x09, *b"damg")],
            ),
            make_scheduler(
                *b"damg",
                vec![stage(0, StageKind::SoundOnCaster, 0x0A, *b"sdam")],
            ),
        ];
        let lookup = RoutineLookup::new().with_dat(&dat);
        let active = ActiveScheduler::from_routine(&lookup, b"dcnt").expect("flattens");
        assert_eq!(active.stages.len(), 1);
        assert_eq!(active.stages[0].stage.kind, StageKind::SubRoutineOnTarget);
        assert_eq!(&active.stages[0].stage.id, b"damg");
    }

    // vendor/server/src/map/enums/action/resolution.h ordering, pinned to the branch order the
    // retail MELEE `dam0` chunk dispatches in (ffxi_dat guard
    // real_dat_dam0_switches_hit_type_to_melee_reaction_routines). `ldam` is the RANGED chain's
    // Hit branch (`ldad` -> `daml`) and links `lhit` -> eflg/selg, which no melee weapon DAT has.
    #[test]
    fn hit_reaction_routines_follow_lsb_resolution_order() {
        use ffxi_proto::melee::ActionResolution;
        let order: Vec<Vec<[u8; 4]>> = [
            ActionResolution::Hit,
            ActionResolution::Miss,
            ActionResolution::Guard,
            ActionResolution::Parry,
            ActionResolution::Block,
        ]
        .into_iter()
        .map(|r| hit_reaction_routine(r, ffxi_proto::melee::ResultOutcome::default(), |_| false))
        .collect();
        assert_eq!(
            order,
            vec![
                vec![*b"damg"],
                vec![*b"sway"],
                vec![*b"gurd"],
                vec![*b"pary"],
                vec![*b"gur1"],
            ]
        );
    }

    // research/xim EffectRoutineInstance.kt createChild newSequences — createChild for a 0x09 link builds
    // `ActorAssociation(target, context.cloneWithOverrideTarget(actor.id))`: the child runs on the
    // target and its own target is the parent. Without the flip the melee chain dead-ends on the
    // victim and the weapon's `ef h` sparks are never reached.
    #[test]
    fn target_link_flips_the_context_onto_the_parent() {
        let attacker = Entity::from_raw_u32(1).unwrap();
        let victim = Entity::from_raw_u32(2).unwrap();
        assert_eq!(
            target_link_context(attacker, Some(victim)),
            (victim, Some(attacker))
        );
        assert_eq!(
            target_link_context(victim, Some(attacker)),
            (attacker, Some(victim))
        );
        assert_eq!(target_link_context(victim, None), (victim, None));
        assert_eq!(
            target_link_context(victim, Some(victim)),
            (victim, None),
            "a self-targeted link must not make an actor its own target"
        );
    }

    // ROM/32/13.DAT — the HumeM weapon-motion base whose `ati0`/`atk0` the melee dispatcher runs.
    const HUME_M_WEAPON_MOTION_FILE: u32 = 9672;
    // ROM/27/82.DAT `hm_s` — the HumeM skeleton, which carries the reaction routines
    // (`damg`/`chit`/`sway`/`gurd`/`pary`).
    const HUME_M_SKELETON_FILE: u32 = 7072;
    // HumeM main-hand weapon model 0: the FFXiMain.dll equipment lookup row for
    // race 1 slot 6 (`MainDll::equipment_model_index`), identical on KNOWN_CLIENTS
    // horizonxi-2023 and retail-2026-09.
    const HUME_M_MAIN_WEAPON_FILE: u32 = 8392;

    fn routines_in_file(file_id: u32) -> Option<Vec<Scheduler>> {
        let root = ffxi_dat::archive::open_test_install()?;
        let loc = root.resolve(file_id).ok()?;
        let bytes = std::fs::read(loc.path_under(&root)).ok()?;
        Some(ffxi_dat::resource_dir::ResourceDir::from_bytes(bytes).collect_schedulers())
    }

    fn global_effect_dir() -> Option<(Vec<Scheduler>, ActionAssets)> {
        let root = ffxi_dat::archive::open_test_install()?;
        let loc = root.resolve(GLOBAL_EFFECT_DIR_FILE_ID).ok()?;
        let bytes = std::fs::read(loc.path_under(&root)).ok()?;
        let (schedulers, assets, _cameras) = parse_action_bytes(&bytes);
        Some((schedulers, assets))
    }

    // Retail-DAT guard (self-skips without an install) for the whole hit-spark chain. `chit`
    // lives in the victim's skeleton but is reached through the 0x09 flip back onto the ATTACKER,
    // which is why it resolves `ef h` in the equipped-weapon DAT; `ef h` links global `hit1`,
    // whose generators are AttachType::TargetActor and therefore land on the victim again
    // (research/xim ParticleGeneratorAttachment.kt updateAssociatedPosition). Every tier must be present for a
    // single spark to appear, so this pins all three at once.
    #[test]
    fn melee_hit_chain_flattens_to_target_attached_sparks() {
        let (Some(skeleton), Some(weapon), Some((global_scheds, global_assets))) = (
            routines_in_file(HUME_M_SKELETON_FILE),
            routines_in_file(HUME_M_MAIN_WEAPON_FILE),
            global_effect_dir(),
        ) else {
            return;
        };
        let lookup = RoutineLookup::new()
            .with_dat(&skeleton)
            .with_dat(&weapon)
            .with_dat(&global_scheds);

        let active = ActiveScheduler::from_routine(&lookup, b"chit")
            .expect("the hit-flash routine flattens across skeleton -> weapon -> global");
        let sparks: Vec<([u8; 4], [u8; 4])> = active
            .stages
            .iter()
            .filter(|t| t.stage.kind == StageKind::Particle)
            .map(|t| (t.stage.local_dir, t.stage.id))
            .collect();
        assert!(
            !sparks.is_empty(),
            "chit -> ef h -> hit1 must reach the spark generators, got {:?}",
            active.stages
        );

        // The reason the lookup has to be directory-scoped at all: ROM/0/0.DAT defines `g010`
        // several times over, and the flat by-name map keeps whichever the walk saw last.
        let g010_dirs = global_assets
            .particle_defs_by_dir
            .keys()
            .filter(|(_, name)| name == b"g010")
            .count();
        assert!(
            g010_dirs > 1,
            "expected duplicate `g010` generators across directories, found {g010_dirs}"
        );

        let attacker = Entity::from_raw_u32(1).unwrap();
        let victim = Entity::from_raw_u32(2).unwrap();
        for (local_dir, id) in &sparks {
            assert_eq!(
                local_dir, b"hit1",
                "the spark generators are authored in the `hit1` directory"
            );
            let def = global_assets
                .particle_def(*local_dir, id)
                .unwrap_or_else(|| panic!("global dir defines {}", String::from_utf8_lossy(id)));
            assert_eq!(
                particle_origin_entity(def.attach_type, attacker, Some(victim)),
                victim,
                "{} spawns on the victim, not the swinger",
                String::from_utf8_lossy(id)
            );
        }
    }

    // The victim's reaction routine must survive the same flatten: `damg` keeps its 0x09 `chit`
    // link as a stage (so the runtime can flip it) rather than inlining it onto the victim.
    #[test]
    fn real_dat_damg_keeps_the_hit_flash_as_a_target_link() {
        let (Some(skeleton), Some((global_scheds, _))) =
            (routines_in_file(HUME_M_SKELETON_FILE), global_effect_dir())
        else {
            return;
        };
        let lookup = RoutineLookup::new()
            .with_dat(&skeleton)
            .with_dat(&global_scheds);
        let active =
            ActiveScheduler::from_routine(&lookup, b"damg").expect("the skeleton has `damg`");
        assert!(
            active.stages.iter().any(|t| {
                t.stage.kind == StageKind::SubRoutineOnTarget && &t.stage.id == b"chit"
            }),
            "got {:?}",
            active.stages
        );
    }

    // (skips without an install): Rarab's `damg` flinch stage carries the
    // retail-authored animationDuration, and dispatch_flinch_stages plays it on a pose-idle
    // host - dfi? for the mob itself, dfm? when the same model is tracked as a PC. Before this
    // consumer existed the stage was parsed and dropped: hits landed as sound while the victim
    // stood still.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_dat_rarab_flinch_stage_plays_the_idle_clip() {
        const RARAB_FILE: u32 = 1569;

        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let Ok(loc) = root.resolve(RARAB_FILE) else {
            return;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            return;
        };
        let (schedulers, _, _) = parse_action_bytes(&bytes);

        let damg = schedulers
            .iter()
            .find(|s| &s.name == b"damg")
            .expect("Rarab ships the damg reaction routine");
        let flinch = damg
            .stages
            .iter()
            .find(|t| t.stage.kind == StageKind::FlinchOnCaster)
            .expect("damg carries the 0x21 flinch stage");
        assert_eq!(
            flinch.stage.flinch_duration,
            Some(24.0),
            "retail authors Rarab's flinch at 24 frames"
        );

        let loaded = crate::ffxi_actor_render::load_npc(&root, RARAB_FILE)
            .expect("Rarab loads from the install");

        for (kind, want_prefix) in [
            (kuluu_snapshot::EntityKind::Mob, "dfi"),
            (kuluu_snapshot::EntityKind::Pc, "dfm"),
        ] {
            let actor = crate::ffxi_actor_render::make_render_actor(
                &loaded,
                0,
                Vec::new(),
                RARAB_FILE,
                0.0,
                1.0,
            );
            assert!(actor.is_pose_idle(), "a fresh actor is pose-idle");

            let mut app = App::new();
            app.add_message::<SchedulerStageEvent>()
                .add_systems(Update, dispatch_flinch_stages);
            let child = app.world_mut().spawn(actor).id();
            let parent = app
                .world_mut()
                .spawn(crate::components::WorldEntity {
                    id: RARAB_FILE,
                    act_index: 0,
                    kind,
                })
                .id();
            app.world_mut().entity_mut(child).insert(ChildOf(parent));

            app.world_mut().write_message(SchedulerStageEvent {
                actor: parent,
                stage: TimedStage {
                    frame: 0,
                    stage: flinch.stage,
                },
                scheduler: *b"damg",
            });
            app.update();

            let clip = app
                .world()
                .entity(child)
                .get::<crate::ffxi_actor_render::FfxiRenderActor>()
                .unwrap()
                .active_action_clip()
                .expect("the flinch stage started a completion motion");
            assert!(
                clip.as_str().starts_with(want_prefix),
                "{kind:?} host plays the {want_prefix}? family, got {clip:?}"
            );
        }
    }

    // The swing routine the melee dispatcher merges must still reach the 0x2B damage callback —
    // that is the frame the reaction (and therefore the spark chain) is handed off on.
    #[test]
    fn real_dat_swing_reaches_the_damage_callback() {
        let (Some(motion), Some((global_scheds, _))) = (
            routines_in_file(HUME_M_WEAPON_MOTION_FILE),
            global_effect_dir(),
        ) else {
            return;
        };
        let lookup = RoutineLookup::new()
            .with_dat(&motion)
            .with_dat(&global_scheds);
        let active = ActiveScheduler::effects_only_merged(&lookup, &[*b"atk0", *b"ati0"])
            .expect("the swing flattens");
        assert!(
            active
                .stages
                .iter()
                .any(|t| t.stage.kind == StageKind::DamageCallback),
            "got {:?}",
            active.stages
        );
    }

    // vendor/server/src/map/attack.h AttackAnimation -> the limb routine
    // research/xim Actor.kt displayAutoAttack enqueues.
    #[test]
    fn swing_routines_follow_lsb_attack_animation_order() {
        use ffxi_proto::melee::AttackAnimation;
        assert_eq!(swing_routine(AttackAnimation::RightAttack), Some(*b"ati0"));
        assert_eq!(swing_routine(AttackAnimation::LeftAttack), Some(*b"bti0"));
        assert_eq!(swing_routine(AttackAnimation::RightKick), Some(*b"cti0"));
        assert_eq!(swing_routine(AttackAnimation::LeftKick), Some(*b"dti0"));
        assert_eq!(swing_routine(AttackAnimation::Throw), None);
    }

    // Retail-DAT guard (skips without an install): the Carrion Worm's dig (`ini1`) and pop-up
    // (`init`) each carry the AnimationLock the pose-pass hold keys on and the StopRoutine
    // that stops the other, so both halves of StopRoutine are exercised by one file. Read
    // straight off disk: which VTABLE app claims the file id is not the point here.
    #[test]
    fn real_dat_worm_dig_and_pop_carry_their_locks_and_stops() {
        const WORM_DIG_LOCK_TICKS: u16 = 112;
        const WORM_POP_LOCK_TICKS: u16 = 188;
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let path = root.root().join("ROM").join("5").join("64.DAT");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skipping: no {}", path.display());
            return;
        };
        let (schedulers, _, _) = parse_action_bytes(&bytes);

        for (name, lock_dur, stops) in [
            (*b"ini1", WORM_DIG_LOCK_TICKS, *b"init"),
            (*b"init", WORM_POP_LOCK_TICKS, *b"ini1"),
        ] {
            let routine = schedulers
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("worm DAT has {name:?}",));
            let lock = routine
                .stages
                .iter()
                .find(|t| t.stage.kind == StageKind::AnimationLock)
                .unwrap_or_else(|| {
                    panic!(
                        "{name:?} carries an AnimationLock stage, got {:?}",
                        routine.stages
                    )
                });
            assert_eq!(lock.frame, 0, "{name:?} locks from frame 0");
            assert_eq!(
                lock.stage.duration_frames, lock_dur,
                "retail measures the {name:?} lock at {lock_dur} ticks"
            );
            let stop = routine
                .stages
                .iter()
                .find(|t| t.stage.kind == StageKind::StopRoutine)
                .unwrap_or_else(|| {
                    panic!(
                        "{name:?} carries a StopRoutine stage, got {:?}",
                        routine.stages
                    )
                });
            assert_eq!(&stop.stage.id, &stops, "{name:?} stops {stops:?}");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn empty_parsed() -> Arc<ParsedActionDat> {
        Arc::new(ParsedActionDat {
            schedulers: Vec::new(),
            assets: ActionAssets::default(),
            cameras: ActionDatCameras::new(),
        })
    }

    // The bead's acceptance criterion: repeated casts of the same spell hit the cache. A hit must
    // also count as a use, or a spammed spell would be the first thing evicted.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn action_dat_lru_evicts_least_recently_used_not_promoted_hits() {
        let mut lru = ActionDatLru::default();
        for id in 0..ACTION_DAT_CACHE_CAP as u32 {
            lru.insert(id, empty_parsed());
        }
        assert!(
            lru.get_and_promote(0).is_some(),
            "filled to cap, no eviction"
        );

        lru.insert(ACTION_DAT_CACHE_CAP as u32, empty_parsed());
        assert!(
            lru.get_and_promote(0).is_some(),
            "the promoted entry survives the over-cap insert"
        );
        assert!(
            lru.get_and_promote(1).is_none(),
            "the least-recently-used entry is the one evicted"
        );
        assert!(lru.get_and_promote(2).is_some());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn action_dat_lru_reinsert_refreshes_recency_without_duplicating() {
        let mut lru = ActionDatLru::default();
        lru.insert(7, empty_parsed());
        for id in 100..100 + (ACTION_DAT_CACHE_CAP as u32 - 1) {
            lru.insert(id, empty_parsed());
        }
        assert_eq!(lru.map.len(), ACTION_DAT_CACHE_CAP);

        lru.insert(7, empty_parsed());
        assert_eq!(
            lru.map.len(),
            ACTION_DAT_CACHE_CAP,
            "re-insert does not double-count"
        );

        lru.insert(999, empty_parsed());
        assert!(
            lru.get_and_promote(100).is_none(),
            "the oldest untouched entry is evicted"
        );
        assert!(
            lru.get_and_promote(7).is_some(),
            "the re-insert refreshed 7's recency"
        );
        assert!(lru.get_and_promote(999).is_some());
    }

    /// A launcher DAT-path change re-inserts every `*DatRoot`; the cache's parses belong to the
    /// root they loaded from, so `adopt_root` drops them — serving them after the swap would
    /// render that install's effects.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn adopting_a_root_drops_the_previous_installs_parses() {
        const FILE_ID: u32 = 4242;
        let mut cache = ActionDatCache::default();
        cache.lru.insert(FILE_ID, empty_parsed());
        assert!(cache.lru.get_and_promote(FILE_ID).is_some());

        cache.adopt_root(None);
        assert!(
            cache.lru.get_and_promote(FILE_ID).is_none(),
            "a parse from the previous root must not survive the swap"
        );
    }

    /// `adopt_action_dat_root` and `poll_action_main_dll` only reach a real client through
    /// `SchedulerRuntimePlugin`, and the tests around them register their own copies — so without
    /// this pin the plugin's registrations could be deleted with every test still green while
    /// `ActionMainDll` stays absent: every weaponskill file-id lookup returns None and every
    /// emote degrades to `play_local_emote_clip`. The kuluu side pins the other half of the
    /// wiring (`insert_dat_roots_hands_the_scheduler_runtime_the_shared_root`). The wiring
    /// systems need only the resources the plugin installs; the dispatchers sharing their
    /// schedule need a live session's, so their missing-parameter errors are ignored here.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_plugin_loads_the_main_dll_from_the_wired_root() {
        bevy::tasks::AsyncComputeTaskPool::get_or_init(Default::default);
        let mut app = App::new();
        app.set_error_handler(bevy::ecs::error::ignore);
        app.add_plugins(SchedulerRuntimePlugin);

        for _ in 0..MAIN_DLL_TASK_POLLS {
            app.update();
            if app.world().get_resource::<ActionMainDll>().is_some() {
                return;
            }
            std::thread::sleep(MAIN_DLL_POLL_INTERVAL);
        }
        panic!("SchedulerRuntimePlugin must load ActionMainDll from the wired ActionDatRoot");
    }

    /// The dispatchers must see the root the host wired, not one they open themselves: a cache
    /// keyed to a different install is exactly the launcher-reload bug above.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn adopt_action_dat_root_hands_the_wired_root_to_the_cache() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        bevy::tasks::AsyncComputeTaskPool::get_or_init(Default::default);
        let root = Arc::new(root);
        let mut app = App::new();
        app.init_resource::<ActionDatCache>()
            .insert_resource(ActionDatRoot(Some(root.clone())))
            .add_systems(
                Update,
                adopt_action_dat_root.run_if(resource_exists_and_changed::<ActionDatRoot>),
            );
        app.update();

        let adopted = app
            .world()
            .resource::<ActionDatCache>()
            .root
            .clone()
            .expect("the wired root reaches the cache");
        assert!(
            Arc::ptr_eq(&adopted, &root),
            "the cache must load through the wired root, not its own"
        );
    }

    /// Bounded so a stuck task fails the test instead of hanging it; the load is one
    /// ~2.8 MB read plus a handful of marker scans, so this is orders of magnitude of slack.
    #[cfg(not(target_arch = "wasm32"))]
    const MAIN_DLL_TASK_POLLS: usize = 600;
    // The playable look race the dispatchers key on most; HumeM=1 per
    // ffxi-dat/src/main_dll.rs::base_emote_index.
    #[cfg(not(target_arch = "wasm32"))]
    const HUME_MALE_LOOK_RACE: u8 = 1;
    #[cfg(not(target_arch = "wasm32"))]
    const MAIN_DLL_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn main_dll_cache_is_one_arc_per_root_and_remembers_an_unreadable_root() {
        let missing = std::env::temp_dir().join(format!(
            "kuluu-render-main-dll-missing-{}",
            std::process::id()
        ));
        assert!(main_dll_for_root(&missing).is_none());
        assert!(main_dll_for_root(&missing).is_none());

        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let first = main_dll_for_root(root.root()).expect("FFXiMain.dll loads");
        let again = main_dll_for_root(root.root()).expect("FFXiMain.dll loads");
        assert!(Arc::ptr_eq(&first, &again), "one load per root");

        let copy =
            std::env::temp_dir().join(format!("kuluu-render-main-dll-copy-{}", std::process::id()));
        std::fs::create_dir_all(&copy).expect("temp root");
        std::fs::copy(root.root().join("FFXiMain.dll"), copy.join("FFXiMain.dll"))
            .expect("copy FFXiMain.dll");
        let other = main_dll_for_root(&copy).expect("the copy loads");
        assert!(
            !Arc::ptr_eq(&first, &other),
            "a different root must be a different dll"
        );
        assert_eq!(
            other.base_race_config_index(HUME_MALE_LOOK_RACE),
            first.base_race_config_index(HUME_MALE_LOOK_RACE)
        );
        let _ = std::fs::remove_dir_all(&copy);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn env_install_root_follows_the_dat_path_precedence() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        assert_eq!(
            install_root_from_env().as_deref(),
            Some(root.root()),
            "the env-resolved root must be the one the install registry names"
        );
    }

    /// The tables `dispatch_action_started` (weaponskill file ids) and `dispatch_entity_emoted`
    /// (emote file ids) read must survive the move off the render thread: what lands in
    /// `ActionMainDll` has to answer identically to a direct `MainDll::load` of the same root.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_dat_action_main_dll_lands_off_thread_with_the_same_tables() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let direct = ffxi_dat::main_dll::MainDll::load(root.root()).expect("FFXiMain.dll loads");
        bevy::tasks::AsyncComputeTaskPool::get_or_init(Default::default);

        let mut app = App::new();
        app.init_resource::<ActionDatCache>()
            .insert_resource(ActionDatRoot(Some(Arc::new(root))))
            .add_systems(
                Update,
                (
                    adopt_action_dat_root.run_if(resource_exists_and_changed::<ActionDatRoot>),
                    poll_action_main_dll,
                )
                    .chain(),
            );

        let mut landed = None;
        for _ in 0..MAIN_DLL_TASK_POLLS {
            app.update();
            if let Some(dll) = app.world().get_resource::<ActionMainDll>() {
                landed = dll.0.clone();
                break;
            }
            std::thread::sleep(MAIN_DLL_POLL_INTERVAL);
        }
        let landed = landed.expect("FFXiMain.dll lands as ActionMainDll");

        // Swept over the whole index space rather than the playable races: the same index space
        // is reached by non-playable look bytes too (ffxi-dat/src/main_dll.rs
        // `MainDll::base_race_config_index`, 32..=36 ridden chocobo), and an out-of-range index
        // has to read `None` on both sides just the same.
        for race in u8::MIN..=u8::MAX {
            assert_eq!(
                landed.base_weapon_skill_index(race),
                direct.base_weapon_skill_index(race),
                "weaponskill base for race {race}"
            );
            assert_eq!(
                landed.base_emote_index(race),
                direct.base_emote_index(race),
                "emote base for race {race}"
            );
        }
        assert!(
            landed
                .base_weapon_skill_index(HUME_MALE_LOOK_RACE)
                .is_some(),
            "the race bases the dispatchers key on are actually populated"
        );
    }

    // research/xim ParticleLinkedDataProviders.kt getParticleMesh — a generator's linked mesh resolves
    // in the directory the generator was authored in before any wider scope. Names taken from
    // ROM/338/100.DAT, which declares `grw1` in both `geo0` and `run0` (particle_sim.rs
    // `directory_scoped_mesh` pins the retail file itself).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn linked_mesh_lookups_prefer_the_generator_directory() {
        const MESH: [u8; 4] = *b"grw1";
        const DIR_A: [u8; 4] = *b"geo0";
        const DIR_B: [u8; 4] = *b"run0";
        const OTHER: [u8; 4] = *b"zzzz";

        fn d3m(texture: &[u8; 16]) -> ffxi_dat::d3m::D3m {
            ffxi_dat::d3m::D3m {
                name: MESH,
                num_triangles: 0,
                texture_name: *texture,
                vertices: Vec::new(),
            }
        }

        let a = d3m(b"eff1    grw1    ");
        let b = d3m(b"eff     grh1    ");
        let mut assets = ActionAssets::default();
        assets.d3ms_by_dir.insert((DIR_A, MESH), a.clone());
        assets.d3ms_by_dir.insert((DIR_B, MESH), b.clone());
        assets.d3ms.insert(MESH, b.clone());

        assert_eq!(
            assets.d3m(DIR_A, &MESH).map(|d| d.texture_name),
            Some(a.texture_name)
        );
        assert_eq!(
            assets.d3m(DIR_B, &MESH).map(|d| d.texture_name),
            Some(b.texture_name)
        );
        assert_eq!(
            assets.d3m(OTHER, &MESH).map(|d| d.texture_name),
            Some(b.texture_name),
            "an unscoped caller still falls back to the flat map"
        );
        assert!(assets.d3m(DIR_A, b"none").is_none());
    }

    /// Same tier order for the sprite sheets, whose texture tokens are the whole payload a
    /// wrong-directory match gets wrong. Names from ROM/1/33.DAT's `ligh` and `fire` copies of
    /// the `ligh` sheet.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn sprite_sheet_lookups_prefer_the_generator_directory() {
        const SHEET: [u8; 4] = *b"ligh";
        const DIR_A: [u8; 4] = *b"ligh";
        const DIR_B: [u8; 4] = *b"fire";

        fn sheet(category: &str, id: &str) -> ffxi_dat::sprite_sheet::ParticleSpriteSheet {
            ffxi_dat::sprite_sheet::ParticleSpriteSheet {
                frames: Vec::new(),
                category: category.to_string(),
                id: id.to_string(),
            }
        }

        let mut assets = ActionAssets::default();
        assets
            .sprite_sheets_by_dir
            .insert((DIR_A, SHEET), sheet("effect", "light"));
        assets
            .sprite_sheets_by_dir
            .insert((DIR_B, SHEET), sheet("fireefc", "light2"));
        assets
            .sprite_sheets
            .insert(SHEET, sheet("fireefc", "light2"));

        assert_eq!(
            assets
                .sprite_sheet(DIR_A, &SHEET)
                .map(|s| s.category.as_str()),
            Some("effect")
        );
        assert_eq!(
            assets
                .sprite_sheet(DIR_B, &SHEET)
                .map(|s| s.category.as_str()),
            Some("fireefc")
        );
        assert_eq!(
            assets
                .sprite_sheet(ffxi_dat::scheduler::NO_LOCAL_DIR, &SHEET)
                .map(|s| s.category.as_str()),
            Some("fireefc"),
            "an unscoped caller still falls back to the flat map"
        );
    }

    /// Fixture for the ActorHide cue (EVENT_HIDE / EVENT_HIDE_SELF): event 503's B4/C6/E1 reveals.
    fn actor_cue_app() -> App {
        let mut app = App::new();
        app.init_resource::<crate::snapshot::EventLog>()
            .init_resource::<crate::scene::TrackedEntities>()
            .init_resource::<crate::entity_table::EntityTable>()
            .init_resource::<CutsceneActorState>()
            .add_systems(Update, apply_cutscene_actor_cues);
        app
    }

    fn spawn_tracked_actor(app: &mut App, id: u32) -> Entity {
        let e = app
            .world_mut()
            .spawn((
                WorldEntity {
                    id,
                    act_index: 0,
                    kind: kuluu_snapshot::EntityKind::Pc,
                },
                Transform::from_xyz(1.0, 0.0, 2.0),
                Visibility::Inherited,
            ))
            .id();
        app.world_mut()
            .resource_mut::<crate::scene::TrackedEntities>()
            .by_id
            .insert(id, e);
        e
    }

    fn push_hide(app: &mut App, target: kuluu_snapshot::CutsceneActor, hide: bool) {
        app.world_mut()
            .resource_mut::<crate::snapshot::EventLog>()
            .push(kuluu_snapshot::ViewerEvent::Cutscene {
                cue: CutsceneCue::ActorHide { target, hide },
            });
    }

    fn push_transpar(
        app: &mut App,
        target: kuluu_snapshot::CutsceneActor,
        end_alpha: i32,
        duration_frames: i32,
    ) {
        app.world_mut()
            .resource_mut::<crate::snapshot::EventLog>()
            .push(kuluu_snapshot::ViewerEvent::Cutscene {
                cue: CutsceneCue::Transpar {
                    target,
                    end_alpha,
                    duration_frames,
                },
            });
    }

    /// Hides event 503's party lead (Curilla) and releases her on unhide.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn actor_hide_cue_hides_the_entity_and_unhide_releases_it() {
        const NPC: u32 = 0x010E_60D5;
        let mut app = actor_cue_app();
        let npc = spawn_tracked_actor(&mut app, NPC);

        push_hide(
            &mut app,
            kuluu_snapshot::CutsceneActor::Entity { server_id: NPC },
            true,
        );
        app.update();
        assert!(
            app.world().get::<CutsceneHidden>(npc).is_some(),
            "hide must insert the marker so culling keeps it hidden in range"
        );
        assert_eq!(
            *app.world().get::<Visibility>(npc).unwrap(),
            Visibility::Hidden
        );
        let state = app.world().resource::<CutsceneActorState>();
        assert!(
            state.hidden.contains(&NPC),
            "hide must record the id for release"
        );

        push_hide(
            &mut app,
            kuluu_snapshot::CutsceneActor::Entity { server_id: NPC },
            false,
        );
        app.update();
        assert!(
            app.world().get::<CutsceneHidden>(npc).is_none(),
            "unhide must remove the marker; culling/sync own Visibility from here"
        );
        let state = app.world().resource::<CutsceneActorState>();
        assert!(state.hidden.is_empty());
    }

    /// EVENT_HIDE_SELF routes here: the motion cues' `moved` closure excludes self, but a
    /// hide must not — event 503's A1 hides the player model for the whole opening.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn actor_hide_cue_resolves_the_local_player() {
        const SELF: u32 = 7;
        let mut app = actor_cue_app();
        let player = spawn_tracked_actor(&mut app, SELF);
        app.world_mut()
            .resource_mut::<crate::entity_table::EntityTable>()
            .set_self_id(Some(SELF));

        push_hide(&mut app, kuluu_snapshot::CutsceneActor::LocalPlayer, true);
        app.update();
        assert!(app.world().get::<CutsceneHidden>(player).is_some());
        assert_eq!(
            *app.world().get::<Visibility>(player).unwrap(),
            Visibility::Hidden
        );
    }

    /// 0x38's local mode hides the local player model for the event's whole
    /// run; release_cutscene_actors owns the unhide at CutsceneEnded
    /// (research/XiEvents/OpCodes/0x0038.md).
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn local_mode_cue_hides_the_self_actor() {
        const SELF: u32 = 7;
        let mut app = actor_cue_app();
        let player = spawn_tracked_actor(&mut app, SELF);
        app.world_mut()
            .resource_mut::<crate::entity_table::EntityTable>()
            .set_self_id(Some(SELF));

        app.world_mut()
            .resource_mut::<crate::snapshot::EventLog>()
            .push(kuluu_snapshot::ViewerEvent::Cutscene {
                cue: CutsceneCue::LocalMode { mode: 0x20 },
            });
        app.update();
        assert!(
            app.world().get::<CutsceneHidden>(player).is_some(),
            "local mode hides the self model for the event's whole run"
        );
        assert_eq!(
            *app.world().get::<Visibility>(player).unwrap(),
            Visibility::Hidden
        );
        let state = app.world().resource::<CutsceneActorState>();
        assert!(
            state.hidden.contains(&SELF),
            "the event-end release must know this id"
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn cutscene_ended_releases_cutscene_hidden_models() {
        const NPC: u32 = 0x010E_60D5;
        let mut app = actor_cue_app();
        app.init_resource::<crate::snapshot::SceneState>()
            .add_systems(Update, release_cutscene_actors);
        let npc = spawn_tracked_actor(&mut app, NPC);

        push_hide(
            &mut app,
            kuluu_snapshot::CutsceneActor::Entity { server_id: NPC },
            true,
        );
        app.update();
        assert!(app.world().get::<CutsceneHidden>(npc).is_some());

        app.world_mut()
            .resource_mut::<crate::snapshot::EventLog>()
            .push(kuluu_snapshot::ViewerEvent::CutsceneEnded);
        app.update();
        assert!(
            app.world().get::<CutsceneHidden>(npc).is_none(),
            "ended must release the marker so the model reappears on its server visibility"
        );
        let state = app.world().resource::<CutsceneActorState>();
        assert!(state.is_empty());
    }

    /// 0x6C inserts the fade component on the target and stops it, at whatever
    /// value it reached, on CutsceneEnded
    /// (research/XiEvents/OpCodes/0x006C.md).
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn transpar_cue_inserts_the_fade_and_end_stops_it() {
        const NPC: u32 = 0x010E_60D5;
        let mut app = actor_cue_app();
        app.init_resource::<crate::snapshot::SceneState>()
            .add_systems(Update, release_cutscene_actors);
        let npc = spawn_tracked_actor(&mut app, NPC);

        push_transpar(
            &mut app,
            kuluu_snapshot::CutsceneActor::Entity { server_id: NPC },
            0,
            60,
        );
        app.update();
        let fade = app
            .world()
            .get::<crate::ffxi_actor_render::CutsceneTranspar>(npc)
            .expect("the cue must insert the fade");
        assert!(
            (fade.end - 0.0).abs() < f32::EPSILON,
            "alpha byte 0 is fully transparent"
        );
        assert!(
            (fade.total_secs - 1.0).abs() < 1e-6,
            "60 frames is one second"
        );
        let state = app.world().resource::<CutsceneActorState>();
        assert!(
            state.faded.contains(&NPC),
            "the fade must be recorded so release finds it"
        );

        app.world_mut()
            .resource_mut::<crate::snapshot::EventLog>()
            .push(kuluu_snapshot::ViewerEvent::CutsceneEnded);
        app.update();
        assert!(
            app.world()
                .get::<crate::ffxi_actor_render::CutsceneTranspar>(npc)
                .is_none(),
            "ended must stop the fade at its current value"
        );
        assert!(app.world().resource::<CutsceneActorState>().is_empty());
    }

    /// The 0x45 camera route drives the operator camera to where the DAT
    /// authored it: the captured chocobo-rental CS cues (camera lock, then the
    /// 30906 c05i/c01i/c00i routines on the local player) are fed through the
    /// real dispatch -> load -> advance pipeline, and the camera must land on
    /// each route's authored eye/target/focal, not hold its chase position.
    /// Self-skips without a real install (ROM/62/112.DAT).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_cutscene_camera_route_drives_the_camera_to_the_authored_points() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        bevy::tasks::AsyncComputeTaskPool::get_or_init(Default::default);
        let root = Arc::new(root);

        // The file the 0x45 cue names, through the same resolution the runtime uses.
        let loc = root
            .resolve(30906)
            .expect("30906 resolves through the vtables");
        let bytes = std::fs::read(loc.path_under(&root)).expect("the camera route DAT reads");
        let (schedulers, _assets, _report, cameras) = parse_action_bytes_reporting(&bytes);
        let lookup = RoutineLookup::new().with_dat(&schedulers);

        const SELF_ID: u32 = 1;
        let player_pos = Vec3::new(10.0, 0.0, -5.0);

        // The route's authored eye/target (bevy space) for a player at `player`
        // with identity yaw: the point's model-frame coords out through the same
        // attach the runtime builds (EID_BODY_CENTER at the fallback height).
        let authored = |routine: &[u8; 4], player: Vec3| -> (Vec3, Vec3, f32) {
            let name = String::from_utf8_lossy(routine);
            let active = ActiveScheduler::from_routine(&lookup, routine)
                .unwrap_or_else(|| panic!("routine {name} missing from the file"));
            let cam_id = active
                .stages
                .iter()
                .find(|t| t.stage.kind == StageKind::CameraRoute)
                .map(|t| t.stage.id)
                .unwrap_or_else(|| panic!("routine {name} carries no CameraRoute stage"));
            let cam = cameras
                .get(&cam_id)
                .unwrap_or_else(|| panic!("routine {name} names no camera chunk"));
            let pt = &cam.points[0];
            let model_point = crate::cutscene_camera::eid_model_point(21, None, None)
                .expect("the body-center locator resolves without a model");
            let to_bevy = |p: [f32; 3]| {
                crate::scene::mzb_to_bevy(kuluu_snapshot::Vec3 {
                    x: p[0],
                    y: p[1],
                    z: p[2],
                })
            };
            let offset = to_bevy([model_point.x, model_point.y, model_point.z]);
            (
                player + offset + to_bevy(pt.position),
                player + offset + to_bevy(pt.target),
                pt.focal_length,
            )
        };

        let mut app = App::new();
        app.add_message::<CutsceneMotionDone>();
        app.init_resource::<Time>()
            .init_resource::<crate::snapshot::EventLog>()
            .init_resource::<crate::snapshot::SceneState>()
            .init_resource::<crate::cutscene::CutsceneMode>()
            .init_resource::<crate::cutscene::FadePrograms>()
            .init_resource::<crate::cutscene::ScreenFade>()
            .init_resource::<crate::cutscene::EventNameOverrides>()
            .init_resource::<crate::entity_table::EntityTable>()
            .init_resource::<crate::scene::TrackedEntities>()
            .init_resource::<ActionDatCache>()
            .init_resource::<CutsceneCameraTasks>()
            .init_resource::<crate::camera::CameraMode>()
            .init_resource::<crate::graphics_settings::GraphicsSettings>()
            .insert_resource(ActionDatRoot(Some(root.clone())));

        let player = app
            .world_mut()
            .spawn((
                Transform::from_translation(player_pos),
                crate::components::WorldEntity {
                    id: SELF_ID,
                    act_index: 72,
                    kind: kuluu_snapshot::EntityKind::Pc,
                },
                crate::components::IsSelf,
            ))
            .id();
        app.world_mut().spawn((
            Transform::from_translation(Vec3::new(0.0, 0.0, 4.0)).looking_at(Vec3::ZERO, Vec3::Y),
            Camera::default(),
            Projection::Perspective(PerspectiveProjection::default()),
            crate::camera::OperatorCamera,
        ));
        app.world_mut()
            .resource_mut::<crate::entity_table::EntityTable>()
            .set_self_id(Some(SELF_ID));
        app.world_mut()
            .resource_mut::<crate::scene::TrackedEntities>()
            .by_id
            .insert(SELF_ID, player);
        // The local-player attach reads the snapshot's self_pos, not the
        // rendered Transform: seed it so ffxi_to_bevy(self_pos.pos) is the
        // player's position at identity heading (ffxi_to_bevy is
        // (x, -z, -y), so bevy (10, 0, -5) is wire (10, 5, 0)).
        app.world_mut()
            .resource_mut::<crate::snapshot::SceneState>()
            .snapshot
            .self_pos = kuluu_snapshot::Position {
            pos: kuluu_snapshot::Vec3 {
                x: 10.0,
                y: 5.0,
                z: 0.0,
            },
            heading: 0,
            speed: 0,
            speed_base: 0,
        };

        app.add_systems(
            Update,
            (
                adopt_action_dat_root.run_if(resource_exists_and_changed::<ActionDatRoot>),
                crate::cutscene::drain_cutscene_events,
                (dispatch_cutscene_motion, poll_action_dat_tasks).chain(),
                crate::cutscene_camera::advance_cutscene_camera_task,
            )
                .chain(),
        );

        let step =
            |app: &mut App, frames: u32| {
                app.world_mut().resource_mut::<Time>().advance_by(
                    std::time::Duration::from_secs_f32(frames as f32 / ROUTINE_FPS),
                );
                app.update();
            };
        let scheduler_cue = |tag: [u8; 4]| kuluu_snapshot::ViewerEvent::Cutscene {
            cue: kuluu_snapshot::CutsceneCue::Scheduler {
                dat_id: 30906,
                actor: kuluu_snapshot::CutsceneActor::LocalPlayer,
                partner: kuluu_snapshot::CutsceneActor::LocalPlayer,
                tag,
                duration: 0,
            },
        };
        let camera = |app: &mut App| {
            let mut q = app
                .world_mut()
                .query_filtered::<&Transform, With<crate::camera::OperatorCamera>>();
            q.single(app.world())
                .expect("one operator camera")
                .translation
        };
        let focal = |app: &mut App| {
            let mut q = app
                .world_mut()
                .query_filtered::<&Projection, With<crate::camera::OperatorCamera>>();
            let proj = q.single(app.world()).expect("one operator camera");
            match proj {
                Projection::Perspective(p) => p.fov,
                _ => panic!("the operator camera is a perspective projection"),
            }
        };

        // Enter CS mode with the captured cues, then feed each camera routine.
        app.world_mut()
            .resource_mut::<crate::snapshot::EventLog>()
            .push(kuluu_snapshot::ViewerEvent::CutsceneStarted { event_id: 10002 });
        app.world_mut()
            .resource_mut::<crate::snapshot::EventLog>()
            .push(kuluu_snapshot::ViewerEvent::Cutscene {
                cue: kuluu_snapshot::CutsceneCue::CameraLock { lock: true },
            });
        step(&mut app, 1);
        assert!(
            app.world()
                .resource::<crate::cutscene::CutsceneMode>()
                .camera_locked,
            "the camera lock must hold the scene"
        );

        for routine in [b"c05i", b"c01i", b"c00i"] {
            let name = String::from_utf8_lossy(routine);
            app.world_mut()
                .resource_mut::<crate::snapshot::EventLog>()
                .push(scheduler_cue(*routine));
            // The load is async; the route starts once the parse lands.
            for _ in 0..120 {
                step(&mut app, 1);
                if app.world().resource::<CutsceneCameraTasks>().is_active() {
                    break;
                }
            }
            assert!(
                app.world().resource::<CutsceneCameraTasks>().is_active(),
                "routine {name} never started its camera route"
            );
            // The route is a single authored point: it holds it for its whole
            // duration, so sample after the full 60 frames have run out.
            step(&mut app, 60);
            let (eye, _target, focal_length) = authored(routine, player_pos);
            let got = camera(&mut app);
            assert!(
                (got - eye).length() < 1e-3,
                "routine {name}: camera at {got:?}, authored eye {eye:?}"
            );
            let want_fov = 2.0
                * (crate::graphics_settings::RETAIL_PROJECTION_HALF_HEIGHT / focal_length).atan();
            assert!(
                (focal(&mut app) - want_fov).abs() < 1e-4,
                "routine {name}: fov {} rad, authored focal {focal_length}",
                focal(&mut app)
            );
        }
    }
}
