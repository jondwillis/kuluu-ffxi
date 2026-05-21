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

use bevy::prelude::*;
use ffxi_dat::scheduler::{Scheduler, TimedStage};

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

/// Bevy plugin: registers [`SchedulerStageEvent`] and the tick system.
/// Front-ends add this once; per-action components are inserted by
/// whichever subsystem decodes the action DAT and starts playback
/// (Stage D2 / E3 wiring, not part of this plugin).
pub struct SchedulerRuntimePlugin;

impl Plugin for SchedulerRuntimePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SchedulerStageEvent>()
            .add_systems(Update, tick_active_schedulers);
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
        assert_eq!(a.stages.iter().map(|t| t.frame).collect::<Vec<_>>(), vec![10, 30, 60]);
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
}
