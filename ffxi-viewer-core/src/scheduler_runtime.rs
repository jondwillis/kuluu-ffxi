use std::collections::HashMap;

use bevy::prelude::*;
use ffxi_dat::chunk::walk;
use ffxi_dat::generator::Generator;
use ffxi_dat::kind::ChunkKind;
use ffxi_dat::scheduler::{Scheduler, StageKind, TimedStage};
use ffxi_dat::sep::Sep;

pub const FFXI_FPS: f32 = 30.0;

const POST_FINISH_TTL_SECS: f32 = 2.0;

#[derive(Component, Debug, Clone)]
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
        if !schedulers.iter().any(|s| &s.name == name) {
            return None;
        }
        let mut stages = Vec::new();
        flatten_routine(schedulers, name, 0, 0, &mut stages);
        stages.sort_by_key(|t| t.frame);
        Some(Self {
            stages,
            elapsed: 0.0,
            cursor: 0,
            name: *name,
        })
    }

    pub fn finished(&self) -> bool {
        self.cursor >= self.stages.len()
    }

    pub fn current_frame(&self) -> u32 {
        (self.elapsed * FFXI_FPS) as u32
    }

    pub fn last_frame(&self) -> u32 {
        self.stages.last().map(|t| t.frame).unwrap_or(0)
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
    mut q: Query<(Entity, &mut ActiveScheduler)>,
    mut writer: MessageWriter<SchedulerStageEvent>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut sched) in &mut q {
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

        if sched.finished() {
            let finish_secs = sched.last_frame() as f32 / FFXI_FPS;
            if sched.elapsed >= finish_secs + POST_FINISH_TTL_SECS {
                commands.entity(entity).remove::<ActiveScheduler>();
            }
        }
    }
}

#[derive(Component, Debug, Clone, Default)]
pub struct ActionAssets {
    pub generators: HashMap<[u8; 4], Generator>,
    #[cfg(not(target_arch = "wasm32"))]
    pub d3ms: HashMap<[u8; 4], ffxi_dat::d3m::D3m>,
    pub seps: HashMap<[u8; 4], Sep>,
    pub animations: Vec<ffxi_dat::skel_anim::SkeletonAnimation>,
    #[cfg(not(target_arch = "wasm32"))]
    pub images: HashMap<[u8; 4], ffxi_dat::texture::DecodedTexture>,
    pub emitters: HashMap<[u8; 4], ffxi_dat::generator::ParticleEmitter>,
    pub particle_defs: HashMap<[u8; 4], ffxi_dat::particle_gen::ParticleGeneratorDef>,
    pub keyframes: HashMap<[u8; 4], ffxi_dat::particle_gen::KeyFrameTrack>,
}

fn flatten_routine(
    schedulers: &[Scheduler],
    name: &[u8; 4],
    base_frame: u32,
    depth: u8,
    out: &mut Vec<TimedStage>,
) {
    if depth > 6 {
        return;
    }
    let Some(s) = schedulers.iter().find(|s| &s.name == name) else {
        return;
    };
    for t in &s.stages {
        let frame = base_frame + t.frame;
        if t.stage.kind == StageKind::SubRoutine {
            flatten_routine(schedulers, &t.stage.id, frame, depth + 1, out);
        } else {
            out.push(TimedStage {
                frame,
                stage: t.stage,
            });
        }
    }
}

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
                if let Ok(Some(e)) = Generator::parse_particle_emitter(c.data) {
                    assets.emitters.insert(c.name, e);
                }
                if let Ok(Some(d)) = ffxi_dat::particle_gen::ParticleGeneratorDef::parse(c.data) {
                    assets.particle_defs.insert(c.name, d);
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
                    assets.d3ms.insert(c.name, d);
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
                    assets.images.insert(c.name, tex);
                }
            }
            _ => {}
        }
    }
    (schedulers, assets)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_sound_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    q_actors: Query<&ActionAssets>,
    mut sfx_writer: MessageWriter<crate::audio::SfxEvent>,
) {
    for ev in events.read() {
        let kind = ev.stage.stage.kind;
        if !matches!(kind, StageKind::SoundOnCaster | StageKind::SoundOnTarget) {
            continue;
        }
        let Ok(assets) = q_actors.get(ev.actor) else {
            continue;
        };

        let Some((se_id, _on_caster)) = ffxi_dat::action::resolve_stage_to_se(
            &ev.stage.stage.id,
            kind,
            &assets.generators,
            &assets.seps,
        ) else {
            continue;
        };

        sfx_writer.write(crate::audio::SfxEvent::new(se_id));
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_motion_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    q_children: Query<&Children>,
    q_assets: Query<&ActionAssets>,
    mut q_actors: Query<&mut crate::ffxi_actor_render::FfxiRenderActor>,
) {
    for ev in events.read() {
        if ev.stage.stage.kind != StageKind::Motion {
            continue;
        }
        // research/xim EffectRoutineInterpolatedEffects.kt:49 — a skill's body motion is
        // resolved against the skill DAT's own clips first, then the caster's animation
        // directories. ActionAssets lives on the tracked entity the scheduler runs on; the
        // render actor is its child.
        let stage = ev.stage.stage;
        let clip = ffxi_dat::datid::DatId::from_name(&stage.id);
        let local_clips: &[ffxi_dat::skel_anim::SkeletonAnimation] = q_assets
            .get(ev.actor)
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

pub fn action_dat_file_id(
    action_id: u32,
    action_kind: u8,
    race: Option<u8>,
    main_dll: Option<&ffxi_dat::main_dll::MainDll>,
) -> Option<u32> {
    // research/xim EffectDisplayer.displaySkill: the completion effect routine for a
    // skill lives in the file-table DAT keyed by the skill's animation index. Only the
    // "finish" action categories carry that completed skill — start categories drive the
    // caster's cast-loop motion instead (see ffxi_actor_render::action_routine).
    // vendor/server map/utils/battleutils action categories: 3 = weaponskill finish,
    // 4 = magic finish, 6 = job-ability finish.
    match action_kind {
        3 => weapon_skill_file_id(action_id, race?, main_dll?),
        4 => ffxi_proto::action_anim::spell_file_id(action_id),
        6 => ffxi_proto::action_anim::ability_file_id(action_id),
        _ => None,
    }
}

// research/xim AbilityTable.kt:103 — WS file id = race base (FFXiMain.dll) + per-skill index.
// `race` is the FFXI look race byte (HumeM=1..Galka=8), which is XIM's RaceGenderConfig.index.
fn weapon_skill_file_id(
    weapon_skill_id: u32,
    race: u8,
    main_dll: &ffxi_dat::main_dll::MainDll,
) -> Option<u32> {
    let index = ffxi_proto::action_anim::weapon_skill_animation_index(weapon_skill_id)?;
    let base = main_dll.base_weapon_skill_index(race)?;
    Some(base as u32 + index as u32)
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub struct MainDllCache {
    loaded: bool,
    dll: Option<ffxi_dat::main_dll::MainDll>,
}

#[cfg(not(target_arch = "wasm32"))]
fn look_race(look: &ffxi_viewer_wire::EntityLook) -> Option<u8> {
    match look {
        ffxi_viewer_wire::EntityLook::Equipped { race, .. } => Some(*race),
        _ => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_action_started(
    events: Res<crate::snapshot::EventLog>,
    tracked: Res<crate::scene::TrackedEntities>,
    q_look: Query<&crate::components::LookComp>,
    mut dll_cache: Local<MainDllCache>,
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
        let race = q_look.get(actor_entity).ok().and_then(|l| look_race(&l.0));
        // FFXiMain.dll is only needed for weaponskill base indices; load it lazily once.
        if action_kind == 3 && !dll_cache.loaded {
            dll_cache.loaded = true;
            if let Ok(root) = ffxi_dat::DatRoot::from_env_or_default() {
                dll_cache.dll = ffxi_dat::main_dll::MainDll::load(root.root()).ok();
            }
        }
        let Some(file_id) =
            action_dat_file_id(action_id, action_kind, race, dll_cache.dll.as_ref())
        else {
            continue;
        };

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

        let active = ActiveScheduler::from_main(&schedulers, b"main")
            .or_else(|| schedulers.first().map(ActiveScheduler::from_scheduler));
        let Some(active) = active else { continue };

        commands
            .entity(actor_entity)
            .try_insert(active)
            .try_insert(assets);
    }
}

pub const EMOTE_ROUTINES_PER_FILE: u16 = 8;

/// Emote id → (emote-file offset, `em0N` routine name). HYPOTHESIS
/// (bead kuluu-d4u retail_unknowns): file = id/8, routine = em0(id%8). Grounded
/// in XIM Actor.kt:674-693 playEmote (file = race base + mainId, routine
/// `em0{subId}`) with HELM Logging id 40 → (5, em00), and in retail HumeM emote
/// DATs (base..base+12, ROM/32/40…) each holding exactly em00..em07. Every
/// call site must stay on this one function until retail verification.
pub fn emote_file_offset_and_routine(emote_id: u16) -> (u32, [u8; 4]) {
    let sub = (emote_id % EMOTE_ROUTINES_PER_FILE) as u8;
    (
        (emote_id / EMOTE_ROUTINES_PER_FILE) as u32,
        [b'e', b'm', b'0', b'0' + sub],
    )
}

#[cfg(not(target_arch = "wasm32"))]
pub fn dispatch_entity_emoted(
    events: Res<crate::snapshot::EventLog>,
    tracked: Res<crate::scene::TrackedEntities>,
    q_look: Query<&crate::components::LookComp>,
    q_children: Query<&Children>,
    mut q_actors: Query<&mut crate::ffxi_actor_render::FfxiRenderActor>,
    mut dll_cache: Local<MainDllCache>,
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
        let ffxi_viewer_wire::ViewerEvent::EntityEmoted {
            actor_id,
            emote_id,
            mode,
            ..
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
        let (file_offset, routine) = emote_file_offset_and_routine(emote_id);
        let race = q_look.get(actor_entity).ok().and_then(|l| look_race(&l.0));

        if let Some(race) = race {
            if !dll_cache.loaded {
                dll_cache.loaded = true;
                if let Ok(root) = ffxi_dat::DatRoot::from_env_or_default() {
                    dll_cache.dll = ffxi_dat::main_dll::MainDll::load(root.root()).ok();
                }
            }
            let base = dll_cache
                .dll
                .as_ref()
                .and_then(|d| d.base_emote_index(race));
            if let Some(base) = base {
                let file_id = base as u32 + file_offset;
                if let Some(active) = load_emote_scheduler(file_id, &routine) {
                    commands
                        .entity(actor_entity)
                        .try_insert(active.0)
                        .try_insert(active.1);
                    continue;
                }
            }
        }

        // NPC casters (lua sendEmote) and PCs whose emote DAT failed to load:
        // play the actor's own em0N clip when it has one; silent no-op
        // otherwise (XIM findLocalAnimationRoutine, Actor.kt:695-697).
        let clip = ffxi_dat::datid::DatId::from_name(&routine);
        let Ok(children) = q_children.get(actor_entity) else {
            continue;
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
}

#[cfg(not(target_arch = "wasm32"))]
fn load_emote_scheduler(
    file_id: u32,
    routine: &[u8; 4],
) -> Option<(ActiveScheduler, ActionAssets)> {
    let root = ffxi_dat::DatRoot::from_env_or_default().ok()?;
    let loc = root.resolve(file_id).ok()?;
    let bytes = std::fs::read(loc.path_under(root.root())).ok()?;
    let (schedulers, assets) = parse_action_bytes(&bytes);
    let active = ActiveScheduler::from_main(&schedulers, routine)?;
    Some((active, assets))
}

pub struct SchedulerRuntimePlugin;

impl Plugin for SchedulerRuntimePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SchedulerStageEvent>()
            .add_systems(Update, tick_active_schedulers);

        #[cfg(not(target_arch = "wasm32"))]
        {
            app.init_resource::<crate::particle_sim::ParticleSimulator>();
            app.add_systems(
                Update,
                (
                    dispatch_action_started,
                    dispatch_entity_emoted,
                    crate::particle_sim::spawn_particle_generators,
                    crate::particle_sim::tick_particle_simulator,
                    crate::particle_sim::sync_particle_meshes,
                    dispatch_sound_stages,
                    dispatch_motion_stages,
                )
                    .chain(),
            );
        }
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
                max_loops: 0,
                transition_in: 0,
                transition_out: 0,
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
        a.elapsed = 0.5;
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

    /// Pins the emote-mapping hypothesis at its only verified point — XIM
    /// HELM Logging id 40 → file offset 5, routine em00 — plus the shape of
    /// the division/modulo split across the id space.
    #[test]
    fn emote_mapping_hypothesis_pins_xim_helm_point() {
        assert_eq!(emote_file_offset_and_routine(40), (5, *b"em00"));
        assert_eq!(emote_file_offset_and_routine(0), (0, *b"em00"));
        assert_eq!(emote_file_offset_and_routine(8), (1, *b"em00"));
        assert_eq!(emote_file_offset_and_routine(31), (3, *b"em07"));
        assert_eq!(emote_file_offset_and_routine(65), (8, *b"em01"));
        assert_eq!(emote_file_offset_and_routine(73), (9, *b"em01"));
        assert_eq!(emote_file_offset_and_routine(96), (12, *b"em00"));
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
}
