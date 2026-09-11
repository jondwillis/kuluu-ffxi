use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

use bevy::prelude::*;
use ffxi_dat::generator::Generator;
use ffxi_dat::kind::ChunkKind;
use ffxi_dat::scheduler::{Scheduler, StageKind, TimedStage};
use ffxi_dat::sep::Sep;

// research/xim util/Fps.kt — `internalFps = 60.0` is the clock every effect routine and
// particle generator is authored against (poc/MainTool.kt internalLoop feeds the raw elapsed frames to
// EffectManager). Only the skeleton domain is halved: poc/ActorManager.kt updateAll "In game,
// skeletal animations are only updated every other frame" — see SKELETON_FRAME_DIVISOR.
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
    // are merged into one entry so the swing's 0x2B DamageCallback reports a single scheduler
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

    /// True while this routine's AnimationLock interval covers `frame`: any 0x07/0x59 stage with
    /// `stage.frame <= frame < stage.frame + duration_frames`. A routine with no
    /// lock stage never locks.
    pub fn locks_at(&self, frame: u32) -> bool {
        self.stages.iter().any(|t| {
            t.stage.kind == StageKind::AnimationLock
                && t.frame <= frame
                && frame < t.frame + t.stage.duration_frames as u32
        })
    }

    /// The routine timeline ends when its last stage ends, not when it starts: a trailing
    /// AnimationLock must keep the routine alive for its whole `duration_frames`.
    pub fn last_frame(&self) -> u32 {
        self.stages
            .iter()
            .map(|t| t.frame + t.stage.duration_frames as u32)
            .max()
            .unwrap_or(0)
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

    /// 0x5F StopRoutine: drop every entry named `name`. xim stops each matching sequence on the
    /// same actor (EffectRoutineInstance.kt handleStopRoutineEffect); stop() just clears the remaining queue - it
    /// does not run the stopped routine's 0x2D StopParticle stages, so no particle
    /// cleanup happens here either.
    pub fn remove_routine_named(&mut self, name: &[u8; 4]) {
        self.routines.retain(|r| r.name != *name);
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
    /// flashing cor?. A `dead` routine with no Motion stage never reports.
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

pub fn tick_active_schedulers(
    time: Res<Time>,
    mut q: Query<(Entity, &mut ActiveSchedulers)>,
    mut writer: MessageWriter<SchedulerStageEvent>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut scheds) in &mut q {
        // Each routine keeps its own clock; a hit reaction that started mid-swing runs on the
        // same entity without touching the swing's cursor or frame.
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
        }

        // Retire entries that finished more than the TTL ago (their last stages may still be in
        // flight on consumers); strip the component and its assets once none remain.
        scheds.routines.retain(|sched| {
            if !sched.finished() {
                return true;
            }
            let finish_secs = sched.last_frame() as f32 / ROUTINE_FPS;
            sched.elapsed < finish_secs + POST_FINISH_TTL_SECS
        });
        if scheds.routines.is_empty() {
            commands
                .entity(entity)
                .remove::<(ActiveSchedulers, ActionAssets, ActionTarget)>();
        }
    }
}

// 0x5F StopRoutine - the worm's dig (`ini1`) stops `init` and its pop-up stops `ini1` this way
//. xim stops every sequence named by the stage on the same actor; here that is a plain
// removal from the vec. The stopped routine's remaining stages simply never fire - including any
// 0x2D StopParticle, which retail does not run for a stopped sequence either
// (EffectRoutineInstance.kt stop).
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
    // The same meshes keyed by (containing directory, name), the tier a generator's linked mesh
    // resolves against before the flat, last-writer-wins map.
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
    // The same defs keyed by (containing directory, name). ROM/0/0.DAT defines four different
    // generators called `g010`, one per effect directory; the flat map keeps only the last.
    pub particle_defs_by_dir:
        HashMap<([u8; 4], [u8; 4]), ffxi_dat::particle_gen::ParticleGeneratorDef>,
    // The directory each entry of the flat `particle_defs` map came from, so a def that only
    // resolves through that last-writer-wins tier still knows the scope its own linked mesh,
    // sprite sheet and texture must resolve in.
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

// The global effect dir's `dada` is the swing impact carrier: every melee swing calls it at its
// impact frame, and it holds the 0x2B DamageCallback that hands off to the victim reaction.
// When flattening cannot inline it (the global dir degraded to empty), the CALL survives as a
// marker stage so `dispatch_damage_callback_stages` still fires on that frame instead of never
//.
const DADA_IMPACT_MARKER: [u8; 4] = *b"dada";

// Knuth's MMIX LCG. Every DAT-driven choice the format leaves unauthored (random routine
// branches, particle spawn spread, sound-emitter jitter) advances this same recurrence, so the
// pair lives here once rather than being retyped per consumer.
pub const LCG_MULTIPLIER: u64 = 6364136223846793005;
pub const LCG_INCREMENT: u64 = 1442695040888963407;

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
                // A control-flow routine is a switch we cannot evaluate (`dam0` picks one of ten
                // mutually exclusive additional-effect branches); inlining it would run every
                // branch. Callers that know the condition dispatch the branch itself - but the
                // CALL still survives flattening as a marker stage: `dada`, the swing's impact
                // carrier, tail-calls such switches, and with a degraded global dir its 0x2B
                // would otherwise vanish from the timeline entirely.
                match lookup.get(&t.stage.id) {
                    Some(c) if c.has_control_flow() => out.push(TimedStage {
                        frame,
                        stage: t.stage,
                    }),
                    // Unresolvable call to the impact marker itself: keep it so a degraded
                    // global dir still fires the reaction at the authored impact frame.
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
// resolves against whichever single ActionAssets actually holds it — the routine's own (on the
// tracked entity) or the global effect dir's.
pub fn assets_holding<'a>(
    local: Option<&'a ActionAssets>,
    global: Option<&'a ActionAssets>,
    has: impl Fn(&ActionAssets) -> bool,
) -> Option<&'a ActionAssets> {
    local
        .filter(|a| has(a))
        .or_else(|| global.filter(|a| has(a)))
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

pub fn parse_action_bytes(bytes: &[u8]) -> (Vec<Scheduler>, ActionAssets) {
    parse_action_tree(&ffxi_dat::chunk::walk_tree(bytes))
}

// Chunk ids are only unique within a directory, and a zone DAT repeats them across weat/ subtrees
// (zone 123 carries `clod` and `hm01..hm15` under both weat/rain and weat/squl), so a consumer
// that owns one subtree must build its assets from that subtree alone or it binds the wrong
// mesh/texture/keyframe.
pub fn parse_action_tree(node: &ffxi_dat::chunk::ChunkNode<'_>) -> (Vec<Scheduler>, ActionAssets) {
    let mut schedulers = Vec::new();
    let mut assets = ActionAssets::default();
    walk_with_dirs(node, &mut |dir, c| {
        let Some(kind) = ChunkKind::from_u8(c.kind) else {
            return;
        };
        match kind {
            ChunkKind::Scheduler => {
                if let Ok(s) = Scheduler::parse_in_dir(dir, c.name, c.data) {
                    schedulers.push(s);
                }
            }
            ChunkKind::Generator => {
                if let Ok(Some(g)) = Generator::parse(c.name, c.data) {
                    assets.generators.insert(c.name, g);
                }
                if let Ok(Some(e)) = Generator::parse_particle_emitter(c.data) {
                    assets.emitters.insert(c.name, e);
                }
                if let Ok(Some(d)) = ffxi_dat::particle_gen::ParticleGeneratorDef::parse(c.data) {
                    assets.particle_defs.insert(c.name, d);
                    assets.particle_def_dirs.insert(c.name, dir);
                    assets.particle_defs_by_dir.insert((dir, c.name), d);
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
    (schedulers, assets)
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

// Every action/emote DAT read resolves through one shared root: `DatRoot::open` re-reads and
// re-parses all 20 VTABLE/FTABLE files (3.3 MB on a retail install), so opening one per event or
// per cache miss is pure repeat work. Wired by kuluu's `insert_dat_roots` like every other
// `*DatRoot`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default, Clone)]
pub struct ActionDatRoot(pub Option<Arc<ffxi_dat::DatRoot>>);

// A `None` root is the host saying it has no install (kuluu wires one either way), so there is
// deliberately no env re-open here: the wired root carries the launcher's overlays and DAT-path
// setting, and a root opened behind the host's back would not.
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
    let task = bevy::tasks::AsyncComputeTaskPool::get()
        .spawn(async move { parse_action_bytes(&read_dat_bytes(root, GLOBAL_EFFECT_DIR_FILE_ID)) });
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
    Emote {
        actor_id: u32,
        target_id: u32,
        routine: [u8; 4],
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
    // Every cached parse, in-flight load and deferred dispatch belongs to the install it was
    // read from, so a launcher DAT-path change drops all three rather than serving the next cast
    // from the old one.
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
        self.pending.push((file_id, dispatch));
    }
}

// An unresolvable/unreadable file caches as an empty parse, so a broken DAT path degrades to the
// pre-existing "no effect" behaviour instead of re-spawning a load per cast.
#[cfg(not(target_arch = "wasm32"))]
fn load_action_dat(root: Option<Arc<ffxi_dat::DatRoot>>, file_id: u32) -> ParsedActionDat {
    let (schedulers, assets) = parse_action_bytes(&read_dat_bytes(root, file_id));
    ParsedActionDat { schedulers, assets }
}

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
    // A completion effect alongside a running cast (or vice versa) is normal retail behaviour -
    // push instead of replacing. The first writer's ActionAssets/ActionTarget stay put, matching
    // the `_if_new` inserts keep them when the component was already present.
    enqueue_routine(commands, actor_entity, active);
    commands
        .entity(actor_entity)
        .try_insert_if_new(parsed.assets.clone())
        .try_insert_if_new(ActionTarget(target_entity));
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_emote_dispatch(
    parsed: &ParsedActionDat,
    routine: &[u8; 4],
    actor_entity: Entity,
    target_entity: Option<Entity>,
    commands: &mut Commands,
) -> bool {
    let Some(active) = ActiveScheduler::from_main(&parsed.schedulers, routine) else {
        return false;
    };
    // Same insert-or-push as apply_action_dispatch: an emote mid-cast (or a cast mid-emote)
    // runs alongside the other instead of replacing it.
    enqueue_routine(commands, actor_entity, active);
    commands
        .entity(actor_entity)
        .try_insert_if_new(parsed.assets.clone())
        .try_insert_if_new(ActionTarget(target_entity));
    true
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
    mut commands: Commands,
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
        let Some(parsed) = cache.lru.get_and_promote(file_id) else {
            // Still in flight — or evicted before this entry drained, in which case re-request.
            cache.defer(file_id, dispatch);
            continue;
        };
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
            PendingActionDispatch::Emote {
                actor_id,
                target_id,
                routine,
            } => {
                let Some(&actor_entity) = tracked.by_id.get(&actor_id) else {
                    continue;
                };
                let target_entity = tracked.by_id.get(&target_id).copied();
                if !apply_emote_dispatch(
                    &parsed,
                    &routine,
                    actor_entity,
                    target_entity,
                    &mut commands,
                ) {
                    play_local_emote_clip(&routine, actor_entity, &q_children, &mut q_actors);
                }
            }
        }
    }
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
                        transition_in: stage.transition_in,
                        transition_out: stage.transition_out,
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
            // 0x25 flinches the target; no target, nothing to flinch.
            StageKind::FlinchOnTarget => q_target.get(ev.actor).ok().and_then(|t| t.0),
            _ => continue,
        };
        let Some(host) = host else { continue };
        // XIM skips the flinch when the model is animation-locked or not currentlyIdle - a
        // swing/cast/death clip owns the pose then, and overwriting it would fight the lock.
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
            // animationDuration drives the transition in/out - half-frame u16 units, so passing
            // it whole yields XIM's animationDuration/2 frames each side; no payload means no
            // transition window.
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
                    transition_in: anim_dur as u16,
                    transition_out: anim_dur as u16,
                },
            );
        }
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
    match action_kind {
        3 => weapon_skill_file_id(animation?, race?, main_dll?),
        4 => ffxi_vocab::action_anim::spell_file_id(action_id, animation),
        6 => ffxi_vocab::action_anim::ability_file_id(action_id, animation),
        // research/xim MobAbilityTable.kt getFileTableOffset - mob skills (category 11) and pet
        // skills (category 13) key the effect DAT by the result's animation index with a range-
        // dependent base; that DAT's `main` plays the caster's own sp?? clip.
        11 | 13 => Some(ffxi_vocab::action_anim::mob_skill_file_id(animation?)),
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
pub(crate) struct ActionMainDllTask(bevy::tasks::Task<Option<ffxi_dat::main_dll::MainDll>>);

// Both halves of a DAT-root change: the parsed-DAT cache re-keys onto the new install and the
// dll re-loads from it. The dll is dropped along with the cache rather than kept warm, because
// it is what turns an action into a *file id* -- serving the previous install's base tables
// while resolving them through the new root mixes the two installs. Until the new one lands the
// dispatchers take their existing no-dll paths.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn adopt_action_dat_root(
    root: Res<ActionDatRoot>,
    mut cache: ResMut<ActionDatCache>,
    mut commands: Commands,
) {
    cache.adopt_root(root.0.clone());
    let root = root.0.clone();
    let task = bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        root.and_then(|root| ffxi_dat::main_dll::MainDll::load(root.root()).ok())
    });
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
    commands.insert_resource(ActionMainDll(dll.map(Arc::new)));
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
                let suffix = spell_suffix.suffix(action_id);
                // Category 8 never reads the animation field; None keeps the call honest.
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
        // A cast alongside a running completion effect (or vice versa) runs concurrently in
        // retail; the push path leaves the first writer's ActionTarget alone.
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
    // The scheduler whose DamageCallback stage is allowed to fire this reaction. Every completion
    // routine ends in a 0x2B (a spell's `mdam`), so an unqualified pending reaction would be
    // consumed by whichever routine happened to reach its callback first.
    pub armed_by: [u8; 4],
}

// A Defeated result starts the victim's death path on this frame instead of waiting for the
// next 0x0E to report hp_pct 0. Latched on the render-actor child (the pose pass reads it
// there) and cleared once the snapshot reports the entity alive again.
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
// are `damh`/`damg`, and BOTH carry a 0x21 FlinchOnCaster stage (ROM/0/0.DAT: damh = chih + sdam
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
        // (vendor/server/src/map/entities/battleentity.cpp CBattleEntity::OnAttack). LSB's
        // hitDistortion is the damage share of max HP (action.cpp action_result_t::recordDamage),
        // so it cannot stand in for the flag. None/Light/Medium/Heavy non-crits all play `damg`
        // per retail's dam0 branch table - never sdam, which flinches nothing on its own.
        ActionResolution::Hit if outcome.is_critical() && model_has(b"ldam") => *b"ldam",
        ActionResolution::Hit => *b"damg",
        ActionResolution::Miss => *b"sway",
        ActionResolution::Guard => *b"gurd",
        ActionResolution::Parry => *b"pary",
        // `shld` is the model's block reaction; `gur1` (the PC gear variant) is the fallback
        // when it is absent.
        ActionResolution::Block if model_has(b"shld") => *b"shld",
        ActionResolution::Block => *b"gur1",
    };
    let mut routines = vec![out];
    // Retail plays the swy1..3 voice + 0x5E knockback stage alongside the damage reaction
    // whenever a knockback level is set.
    if outcome.knockback > 0 && out != *b"sway" {
        routines.push(*b"sway");
    }
    routines
}

// research/xim Actor.kt displayAutoAttack — the swing routine is chosen by which limb struck.
// Direction-of-movement variants (atf0/atb0/atl0/atr0) are not selected here; that needs the
// attacker's locomotion state at swing time. No attacker-side crit swing exists on purpose: LSB
// flags the crit only in the VICTIM's result block (CBattleEntity::OnAttack sets info CriticalHit
// + hitDistortion Heavy from one bool; vendor/server/src/map/entities/battleentity.cpp) and this
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
const MELEE_VOICE_ROUTINE: [u8; 4] = *b"atk0";

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
    // Same-batch insert buffer for entities without ActiveSchedulers yet (see run_routine_on);
    // flushed after the event loop so a Defeated `dead` and anything else queued this frame
    // merge into one component instead of overwriting each other.
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
        // An off-hand/kick routine is absent from some weapon-motion DATs; the main-hand swing
        // is the only routine every armed race base is known to carry. The event carries the
        // outcome bits (info/hitDistortion/knockback) separately from the (resolution,
        // animation) pair.
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
        // A swing alongside a running cast/completion effect runs concurrently in retail
        // (ActionTimer1 refcount); the push path leaves the first writer's
        // ActionTarget alone.
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
        // Retail flips StatusServer on the same frame as the HP packet, so start the victim's
        // death path now instead of waiting for the next 0x0E.
        if outcome.defeated() {
            tracing::debug!(target: "combat", "COMBAT_DEAD actor={} target={:?} info=0x{:X}",
                    actor_id, victim, outcome.info);
            latch_dead_from_action(victim, &q_children, &q_render, &mut commands);
            // XIM's onDisplayDeath enqueues the model's `dead` routine with
            // displayDead=true on the Defeated frame: ded? fall-over at its first Motion stage,
            // cor0 hold after. Play mode so those Motion stages fire through
            // dispatch_motion_stages; models without a `dead` routine keep today's
            // instant-corpse fallback (run_routine_on no-ops on an unresolvable name).
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
    flush_pending_routine_inserts(&mut pending_inserts, &mut q_scheds, &mut commands);
}

// Latch the victim's death path on its render-actor child (see DeadFromAction). No-op when the
// victim has no loaded model yet - the 0x0E hp_pct will still take over later. Only caller is
// dispatch_melee_action_started, which is native-only; gate matches so wasm compiles.
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
    // Same-batch insert buffer for victims without ActiveSchedulers yet (see run_routine_on);
    // flushed after the event loop so a damage reaction and its knockback sway merge into one
    // component instead of the sway insert overwriting the reaction.
    mut pending_inserts: Local<HashMap<Entity, Vec<ActiveScheduler>>>,
    global: Option<Res<GlobalEffectDir>>,
    mut commands: Commands,
) {
    for ev in events.read() {
        // The 0x2B itself, or a surviving call to `dada` - the impact marker that stands in
        // when flattening kept the call instead of inlining it (degraded global dir,
        // /D2). Steady state is unchanged: an inlined dada contributes its 0x2B
        // and no marker, so exactly one stage fires per swing.
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
        // The reaction is decided against the VICTIM's model: `shld` is only picked
        // when that DAT ships it, and a knockback level adds `sway` alongside.
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
    flush_pending_routine_inserts(&mut pending_inserts, &mut q_active, &mut commands);
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
    // Same-batch insert buffer for hosts without ActiveSchedulers yet (see run_routine_on);
    // flushed after the event loop so several 0x09 links landing on one fresh host in a frame
    // merge into one component instead of overwriting each other.
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
    flush_pending_routine_inserts(&mut pending_inserts, &mut q_active, &mut commands);
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
    // A victim mid-routine gets the reaction pushed alongside it: retail runs both (the
    // ActionTimer1 lock counted 2 and 3 when a hit reaction overlapped a swing). The old
    // single-slot guard dropped the lower-priority routine instead. When the entity has no
    // ActiveSchedulers yet the insert is a deferred command - a second routine queued on the same
    // entity in this batch would overwrite it (last insert wins), so buffer it and let the caller
    // merge at flush time instead (kuluu-df9t: a knockback hit on a fresh victim lost its damage
    // reaction to the sway insert).
    match q_active.get_mut(entity) {
        Ok(mut scheds) => scheds.push(active),
        Err(_) => pending_inserts.entry(entity).or_default().push(active),
    }
    // ActionTarget stays a single entity-level component: first writer wins, stripped when the
    // last routine finishes. Retail's per-sequence target context (cloneWithOverrideTarget) is a
    // known simplification - out of scope here.
    commands
        .entity(entity)
        .try_insert(ActionTarget(flipped_target));
}

// Apply the inserts buffered by `run_routine_on`'s Err branch (see there for why): re-check for
// an ActiveSchedulers that appeared since the call and merge into it, otherwise insert every
// queued routine at once so none is lost to a deferred-command overwrite. Commands apply per
// system, so within one batch only our own buffered inserts can change the answer between the
// call and this flush.
#[cfg(not(target_arch = "wasm32"))]
/// Queue a routine on an entity through commands so two routines landing on a not-yet-scheduled
/// entity in one system both survive: a deferred `insert(ActiveSchedulers::one)` would let the
/// second overwrite the first. Commands apply in order, so the push always sees the component.
pub fn enqueue_routine(commands: &mut Commands, entity: Entity, active: ActiveScheduler) {
    commands
        .entity(entity)
        .entry::<ActiveSchedulers>()
        .or_default()
        .and_modify(move |mut scheds| scheds.push(active));
}

fn flush_pending_routine_inserts(
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

pub const EMOTE_ROUTINES_PER_FILE: u16 = 8;

const SALUTE_NATION_MAX: u16 = 2;

fn em_routine(sub: u16) -> [u8; 4] {
    [
        b'e',
        b'm',
        b'0',
        b'0' + (sub % EMOTE_ROUTINES_PER_FILE) as u8,
    ]
}

/// Emote id → (emote-file offset from the FFXiMain.dll race base, `em0N`
/// routine). Derived empirically from the retail HumeM emote DATs (dump:
/// examples/zz-emote-probe.rs; each routine's Motion clip mnemonic names the
/// emote — bow/poi/sl1-3/kne/lau/wee, den/nod/wav/wel/gla/che/clp, …) and
/// pinned to XIM's only known points (Actor.kt onGatheringAttempt HELM: Logging=(5,0),
/// Mining=(6,0), Harvesting=(7,0) — confirmed by the files' Japanese tool
/// particles: ono0=axe, turu=pickaxe, kama=sickle). Notable non-uniformities
/// the old id/8 hypothesis missed: Point/Bow are swapped in file 0, Salute
/// occupies em02..em04 (one per nation, 0x05A Param = nation), and ids ≥ 6
/// sit at (id+2)/8 only through id 37. Returns None when no body routine
/// exists in the era DATs (face-only emotes, id gaps, unmapped job emotes).
pub fn emote_routine(emote_id: u16, param: u16) -> Option<(u32, [u8; 4])> {
    match emote_id {
        0 => Some((0, *b"em01")),
        1 => Some((0, *b"em00")),
        2 => Some((0, em_routine(2 + param.min(SALUTE_NATION_MAX)))),
        3 => Some((0, *b"em05")),
        4 => Some((0, *b"em06")),
        5 => Some((0, *b"em07")),
        6..=37 => {
            let shifted = emote_id + 2;
            Some((
                (shifted / EMOTE_ROUTINES_PER_FILE) as u32,
                em_routine(shifted % EMOTE_ROUTINES_PER_FILE),
            ))
        }
        // HELM (server-initiated): axe / pickaxe / sickle files.
        40 => Some((5, *b"em00")),
        41 => Some((6, *b"em00")),
        42 => Some((7, *b"em00")),
        // Hurray variants (xe0..xe6) are weapon-keyed; selection unmapped — em00 default.
        43 => Some((8, *b"em00")),
        44 => Some((11, *b"em00")),
        // Dance1-4 (dc0..dc3).
        65..=68 => Some((12, em_routine(emote_id - 65))),
        // Bell-ring motion variants (rx/rs); note→variant selection unmapped.
        73 => Some((10, *b"em00")),
        // Aim variants (ye0..ye6) are ranged-weapon-keyed; selection unmapped — em00 default.
        96 => Some((9, *b"em00")),
        _ => None,
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
        let Some((file_offset, routine)) = emote_routine(emote_id, param) else {
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
                        if apply_emote_dispatch(
                            &parsed,
                            &routine,
                            actor_entity,
                            tracked.by_id.get(&target_id).copied(),
                            &mut commands,
                        ) {
                            continue;
                        }
                    }
                    // The DAT-vs-local-clip decision needs the parse, so it is deferred with it.
                    None => {
                        cache.defer(
                            file_id,
                            PendingActionDispatch::Emote {
                                actor_id,
                                target_id,
                                routine,
                            },
                        );
                        continue;
                    }
                }
            }
        }

        play_local_emote_clip(&routine, actor_entity, &q_children, &mut q_actors);
    }
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
                    transition_in: 0,
                    transition_out: 0,
                },
            );
        }
    }
}

pub struct SchedulerRuntimePlugin;

impl Plugin for SchedulerRuntimePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SchedulerStageEvent>();

        #[cfg(target_arch = "wasm32")]
        app.add_systems(
            Update,
            (tick_active_schedulers, dispatch_stop_routine_stages).chain(),
        );

        #[cfg(not(target_arch = "wasm32"))]
        {
            app.init_resource::<crate::particle_sim::ParticleSimulator>();
            app.init_resource::<ActionDatCache>();
            app.init_resource::<ActionDatRoot>();
            app.add_systems(Startup, load_global_effect_dir);
            // Ordered ahead of the poll so a root change landing on the same frame as an
            // in-flight dll cannot have the poll's `remove_resource::<ActionMainDllTask>` applied
            // over the freshly spawned one, which has no retry path.
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
                    poll_action_dat_tasks,
                    // Chained between the routine inserters and the stage consumers so a
                    // routine's frame-0 stages fire on the frame it is inserted, and every
                    // stage is consumed the same frame it is written. StopRoutine removal runs
                    // right after the tick that emits its 0x5F stage.
                    tick_active_schedulers,
                    dispatch_stop_routine_stages,
                    crate::particle_sim::spawn_actor_auto_run_particles,
                    crate::particle_sim::spawn_particle_generators,
                    dispatch_stop_particle_stages,
                    crate::particle_sim::stop_generators_for_despawned_owners,
                    crate::particle_sim::tick_particle_simulator,
                    crate::particle_sim::sync_particle_meshes,
                    dispatch_sound_stages,
                    dispatch_motion_stages,
                    dispatch_flinch_stages,
                    (dispatch_damage_callback_stages, settle_dead_from_action).chain(),
                    dispatch_target_routine_stages,
                )
                    .chain()
                    // The overlay and this chain both drain EventLog with private cursors; the
                    // overlay's "no routine for this action" branch clears the looping action, so
                    // it must run before a completion routine's Motion stage begins here.
                    .after(crate::ffxi_actor_render::dispatch_action_overlay),
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

    // Foot Kick (mob skill 259) arrives as category 11 with animation 3; the effect DAT's file id
    // is the range-dependent base plus that index (research/xim MobAbilityTable.kt getFileTableOffset).
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
        // A result-less body carries no animation index and resolves to nothing.
        assert_eq!(action_dat_file_id(259, None, 11, None, None), None);
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

    // The whole point of the spatial SE path: a 0x0B impact has to mix from where the victim is
    // standing, not from the attacker, and neither may fall back to a 2D cue.
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
            },
        }
    }

    fn make_scheduler(name: [u8; 4], stages: Vec<TimedStage>) -> Scheduler {
        Scheduler { name, stages }
    }

    // ActionAssets holds a routine's decoded textures/meshes and ActionTarget its aim; both must
    // leave with ActiveSchedulers at the post-finish TTL or every actor that ever ran an action
    // retains one action DAT's decoded asset set (and a stale target) until despawn.
    #[test]
    fn tick_active_schedulers_strips_action_components_after_ttl() {
        let mut app = App::new();
        app.add_message::<SchedulerStageEvent>()
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
    // without touching the swing's cursor or frame (retail's ActionTimer1 counted 2 and 3).
    // Each entry keeps its own clock; entries retire on their OWN finish+TTL, so a short
    // reaction can lapse while the swing still runs.
    #[test]
    fn concurrent_routines_keep_separate_cursors_and_strip_together() {
        let mut app = App::new();
        app.add_message::<SchedulerStageEvent>()
            .init_resource::<Time>()
            .init_resource::<CapturedStages>()
            .add_systems(Update, (tick_active_schedulers, capture_stages).chain());

        // The swing's only stage is at frame 59; the reaction's only stage is at frame 19.
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

        // t=0.5 s (frame 30): the reaction has fired its stage; the swing is not at frame 59 yet.
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

        // t=1.0 s (frame 60): the swing fires too; both are finished but neither is past its
        // finish+TTL yet.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.5));
        app.update();
        assert!(app.world().entity(actor).contains::<ActiveSchedulers>());

        // t=2.5 s: the reaction (finished at frame 19, ~0.32 s) is past its TTL; the swing
        // (finished at frame 59, ~0.98 s) is not - the component survives on the swing alone.
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

        // t=3.1 s: past the swing's own finish+TTL - everything is stripped.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.6));
        app.update();
        assert!(!app.world().entity(actor).contains::<ActiveSchedulers>());
    }

    // 0x5F StopRoutine drops the named entry and only that one (xim EffectRoutineInstance.kt:
    // 910-915 stops each matching sequence on the same actor).
    #[test]
    fn stop_routine_stage_removes_only_the_named_entry() {
        let mut app = App::new();
        app.add_message::<SchedulerStageEvent>()
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

        app.update(); // frame 0: dig's StopRoutine fires and removes `init`
        let scheds = app.world().entity(actor).get::<ActiveSchedulers>().unwrap();
        assert_eq!(scheds.routines.len(), 1);
        assert_eq!(
            scheds.routines[0].name, *b"ini1",
            "the stopper survives its own stage"
        );
    }

    // The lock test is per-routine and interval-based: a routine with no 0x07/0x59 stage never
    // locks, and an overlapping second routine keeps the entity locked past either one's
    // own interval end - the refcount>0 behaviour retail measured on ActionTimer1.
    #[test]
    fn animation_lock_is_refcounted_across_concurrent_routines() {
        let lock_stage = |frame: u32, raw: u8, dur: u16| -> TimedStage {
            let mut t = stage(frame, StageKind::AnimationLock, raw, *b"    ");
            t.stage.duration_frames = dur;
            t
        };
        // Hare-swing-shaped lock [0, 50) and damage-reaction-shaped lock [28, 58).
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

    // A routine's timeline ends when its last stage ends: a trailing AnimationLock longer than
    // the post-finish TTL must not be retired (and its lock released) while it still holds.
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

    // the fall-over window: pending from insertion until the first Motion
    // stage fires, never reported for routines without one (instant-corpse fallback models)
    // or under any other name.
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

        // The frame-0 tick fires both stages: the cursor passes the first Motion.
        scheds.routines[0].cursor = 2;
        assert!(
            !scheds.dead_fall_over_pending(),
            "the fall-over has started"
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

        // The crit flag picks ldam when the lookup resolves it...
        assert_eq!(
            hit_reaction_routine(R::Hit, crit(0), has(vec![*b"ldam"])),
            vec![*b"ldam"]
        );
        // ...and back to damg when nothing in the lookup ships ldam (the pre-global fallback).
        assert_eq!(
            hit_reaction_routine(R::Hit, crit(0), has(vec![])),
            vec![*b"damg"]
        );
        // hitDistortion is the damage share of max HP, not the flag: a Heavy non-crit stays on
        // damg and a Light crit still plays ldam.
        assert_eq!(
            hit_reaction_routine(R::Hit, o(0, 3, 0), has(vec![*b"ldam"])),
            vec![*b"damg"]
        );
        assert_eq!(
            hit_reaction_routine(R::Hit, o(INFO_CRITICAL_HIT, 1, 0), has(vec![*b"ldam"])),
            vec![*b"ldam"]
        );
        // None/Light/Medium all route to damg per retail's dam0 branch table - never sdam, even
        // when the model ships it: ROM/0/0.DAT's damh/damg both carry the 0x21 flinch stage,
        // while sdam is sound-only (kuluu-df9t: the old sdam preference made normal hits on
        // sdam-shipping models invisible).
        assert_eq!(
            hit_reaction_routine(R::Hit, o(0, 0, 0), has(vec![*b"sdam"])),
            vec![*b"damg"]
        );
        assert_eq!(
            hit_reaction_routine(R::Hit, o(0, 1, 0), has(vec![*b"sdam"])),
            vec![*b"damg"]
        );
        // ...and damg when the model ships nothing; Medium stays on damg.
        assert_eq!(
            hit_reaction_routine(R::Hit, o(0, 0, 0), has(vec![])),
            vec![*b"damg"]
        );
        assert_eq!(
            hit_reaction_routine(R::Hit, o(0, 2, 0), has(vec![*b"sdam"])),
            vec![*b"damg"]
        );
        // The guard/parry/block cases; block prefers shld when present.
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
        // Miss is sway; a knockback level adds sway alongside the damage routine but never twice.
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
        // ...and the fallback keeps sway alongside damg.
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
        let (schedulers, _) = parse_action_bytes(&bytes);
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
        assert_eq!(a.last_frame(), 0);
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
        let (schedulers, assets) = parse_action_bytes(&bytes);
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
        let (scheds, assets) = parse_action_bytes(&[]);
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
        let (_scheds, assets) = parse_action_bytes(&bytes);

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
        let (_s, cure_assets) = parse_action_bytes(&cure_bytes);
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
        let (_scheds, assets) = parse_action_bytes(&bytes);

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
        let (schedulers, assets) = parse_action_bytes(&bytes);

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
        let (actor_scheds, _) = parse_action_bytes(&actor_bytes);
        let (global_scheds, _) = parse_action_bytes(&global_bytes);
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
            .filter(|t| t.stage.kind != StageKind::Unknown)
            .count(),
            0,
            "without the global tier the aura sub-routines resolve to nothing — the original bug"
        );
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
    // reaction instead of vanishing. The inlined 0x2B and the dam0 marker both land at call +
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

    // /D2 - with the global dir degraded to empty, `call dada` cannot resolve; the
    // call still survives as a marker so dispatch_damage_callback_stages fires at the impact
    // frame instead of never.
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

    // ...and every OTHER unresolvable call is still dropped (`aloc` and friends stay inert).
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
    // look_resolver::PC_MODEL_IDS[HumeM][main-hand] base — main-hand weapon model 0.
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
        Some(parse_action_bytes(&bytes))
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
        let (schedulers, _) = parse_action_bytes(&bytes);

        let damg = schedulers
            .iter()
            .find(|s| &s.name == b"damg")
            .expect("Rarab ships the damg reaction routine");
        let flinch = damg
            .stages
            .iter()
            .find(|t| t.stage.kind == StageKind::FlinchOnCaster)
            .expect("damg carries the 0x21 flinch stage");
        // Raw bytes (ROM/4/109.DAT, damg and ldam identical): payload after delay/duration is
        // f32,f32,u32(=2),f32,f32 animationDuration = 24.0,u32,u32 - the fifth value at +24.
        assert_eq!(
            flinch.stage.flinch_duration,
            Some(24.0),
            "retail authors Rarab's flinch at 24 frames"
        );

        let loaded =
            crate::ffxi_actor_render::load_npc(RARAB_FILE).expect("Rarab loads from the install");

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
            // Bevy maintains the parent's Children immediately via the ChildOf component hook.
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

    // Retail-DAT guard (skips without an install): the Carrion Worm's dig (`ini1`) locks for 112
    // ticks and its pop-up (`init`) for 188 - the retail-measured intervals that the
    // pose-pass hold keys on. Each also carries the 0x5F that stops the other (the worm
    // dig stops `init`, the pop stops `ini1`), so both halves of StopRoutine are exercised by one
    // file. Read straight off disk: which VTABLE app claims the file id is not the point here.
    #[test]
    fn real_dat_worm_dig_and_pop_carry_their_locks_and_stops() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let path = root.root().join("ROM").join("5").join("64.DAT");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skipping: no {}", path.display());
            return;
        };
        let (schedulers, _) = parse_action_bytes(&bytes);

        for (name, lock_dur, stops) in [(*b"ini1", 112u16, *b"init"), (*b"init", 188, *b"ini1")] {
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

    // A launcher DAT-path change re-inserts every `*DatRoot`; the parses already in hand belong
    // to the previous install, so serving them after the swap renders the old game's effects.
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

    // `adopt_action_dat_root` and `poll_action_main_dll` only reach a real client through
    // `SchedulerRuntimePlugin`, and the tests around them register their own copies -- so without
    // this pin the plugin's registrations can be deleted with every test still green while
    // `ActionMainDll` never exists: every weaponskill file-id lookup returns None and every emote
    // degrades to `play_local_emote_clip`. The kuluu side pins the other half of the wiring
    // (`insert_dat_roots_hands_the_scheduler_runtime_the_shared_root`).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_plugin_loads_the_main_dll_from_the_wired_root() {
        bevy::tasks::AsyncComputeTaskPool::get_or_init(Default::default);
        let mut app = App::new();
        // The two wiring systems need nothing but the resources the plugin itself installs; the
        // dispatchers sharing their schedule need a live session's, so their missing-parameter
        // errors are noise here.
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

    // The dispatchers must see the root the host wired, not one they open themselves: a cache
    // keyed to a different install is exactly the launcher-reload bug above.
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

    // Bounded so a never-landing task fails the test instead of hanging it; the load is one
    // ~2.8 MB read plus a handful of marker scans, so this is orders of magnitude of slack.
    #[cfg(not(target_arch = "wasm32"))]
    const MAIN_DLL_TASK_POLLS: usize = 600;
    // The playable look race the dispatchers key on most; HumeM=1 per
    // ffxi-dat/src/main_dll.rs::base_emote_index.
    #[cfg(not(target_arch = "wasm32"))]
    const HUME_MALE_LOOK_RACE: u8 = 1;
    #[cfg(not(target_arch = "wasm32"))]
    const MAIN_DLL_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

    // The tables `dispatch_action_started` (weaponskill file ids) and `dispatch_entity_emoted`
    // (emote file ids) read must survive the move off the render thread: what lands in
    // `ActionMainDll` has to answer identically to a direct `MainDll::load` of the same root.
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
        // is reached by non-playable look bytes too (ffxi-dat `MainDll::base_race_config_index`,
        // 32..=36 ridden chocobo), and an out-of-range index has to read `None` on both sides
        // just the same.
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

    // Same tier order for the 0x21 sprite sheets, whose texture tokens are the whole payload a
    // wrong-directory match gets wrong. Names from ROM/1/33.DAT's `ligh` and `fire` copies of
    // the `ligh` sheet.
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
}
