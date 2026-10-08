//! Retail's gates on an actor's look-at pass, plus the state of the `0x89` LockLookAt tasks those
//! stages spawn. Both gates read the entity's wire animation byte (`enum ANIMATIONTYPE`,
//! vendor/server/data/enums/animation.yaml) — XIClient keeps that same byte as the actor's game status
//! (research/XIClient/src/XIClient/include/World/Actor/GameStatus.h).

use bevy::math::Vec2;
use ffxi_proto::decode::animation;

/// The `/sitchair` run of animation bytes, bounded as the look-at chain bounds it — a wider block than
/// the enum names (`enum ANIMATIONTYPE`, vendor/server/data/enums/animation.yaml; the chain compares each
/// value in the range, `FFXiMain.dll retail-2026-09` RVA 0x95790).
const SIT_ON_FURNITURE_FIRST: u8 = 63;
const SIT_ON_FURNITURE_LAST: u8 = 0x53;

/// The animation bytes whose actor runs its look-at pass at all (`FFXiMain.dll retail-2026-09` RVA
/// 0xD5C01..0xD5C5B). Every other byte — including attack, death, event and the fishing run — drops
/// the resolved target before any aiming happens. The chain's three furniture/ride helpers compare a
/// caller-supplied byte and fall back to own-status when handed `0xff` (RVA 0x95680 / RVA 0x956a0);
/// every call site in the chain passes the real byte, so that sentinel is not reachable here.
pub fn look_at_allowed(animation: u8) -> bool {
    matches!(
        animation,
        animation::NONE
            | animation::CHOCOBO
            | animation::SIT
            | animation::RANGED
            | animation::MOUNT
    ) || (SIT_ON_FURNITURE_FIRST..=SIT_ON_FURNITURE_LAST).contains(&animation)
}

/// What one actor's look-at pass is doing this frame. retail decides it *before* selecting anything: the
/// LockLookAt bit makes even a resolved target count as no target (`FFXiMain.dll retail-2026-09` RVA 0xD5B90,
/// `test byte [esi+0x840], 2`), and every frame that ends without a live target takes the release branch
/// (mode `-1.0` written at RVA 0xD5B9E). "Nothing to look at" is a state there, not an absence something
/// infers afterwards — so kuluu names it and lets each animation state keep writing its own bones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookState {
    /// No face target resolved: nothing aims, the skeleton is owned by the locomotion/battle states alone.
    IdleNoTarget,
    /// A target exists but this actor's status byte or a LockLookAt interval forbids aiming at it.
    Suppressed,
    /// A target resolved and every gate passed.
    Aiming,
}

pub fn look_state_of(gates_pass: bool, target_resolved: bool) -> LookState {
    match (gates_pass, target_resolved) {
        (false, _) => LookState::Suppressed,
        (true, false) => LookState::IdleNoTarget,
        (true, true) => LookState::Aiming,
    }
}

/// How many bend records an actor with this byte bends with: the bend prologue writes loop count 1 on a
/// match against the same furniture/ride/rest block and 2 otherwise (`FFXiMain.dll retail-2026-09` RVA
/// 0x2AE0D..0x2AEBB), so standing is the only state that bends shoulder along with head.
pub fn look_at_bend_records(animation: u8) -> usize {
    if look_at_allowed(animation) && animation != animation::NONE {
        ONE_BEND_RECORD
    } else {
        ffxi_actor::look_bend::BEND_RECORDS_MAX
    }
}

const ONE_BEND_RECORD: usize = 1;

/// One yalm. The unit is the model's own, since it is subtracted straight out of an authored attach-point offset.
const ANCHOR_DROP_YALMS: f32 = 1.0;

/// How far the look-at bend's anchor drops for this animation byte — `1.0f` off the second component of the
/// reference-4 object, applied before either shared vector is measured from it (`FFXiMain.dll retail-2026-09`:
/// the compare pair `cmp esi,5` / `cmp esi,0x55` at RVA 0x2AC9E..0x2ACA6 guards `fld [esp+0x30] /
/// fsub dword [0x1032961c] (= 1.0) / fstp [esp+0x30]` at RVA 0x2ACA8..0x2ACB2). Both bytes are rides — chocobo
/// (`animation::CHOCOBO`) and mount (`animation::MOUNT`) — which is the pair that changes how high a rider's head
/// sits relative to the skeleton's authored anchor. It is NOT the set that reduces the bend to one record
/// ([`look_at_bend_records`] keeps its own, wider set): sitting rests the shoulder, riding lowers the anchor.
pub fn look_at_anchor_y_drop(animation: u8) -> f32 {
    if matches!(animation, animation::CHOCOBO | animation::MOUNT) {
        ANCHOR_DROP_YALMS
    } else {
        0.0
    }
}

/// Whether an actor of this wire-record type may be looked at at all: retail tests the resolved
/// target six times against the record-type byte and releases on any non-member (`FFXiMain.dll
/// retail-2026-09`: getter calls at RVA 0xD5BB8–0xD5BF3 comparing `0,1,2,6,7`, with the sixth
/// comparison `cmp eax,8` / `jne` release tail @ RVA 0xD5BFB). The record-type byte is stamped
/// per entity by spawn/event handlers; its observed constants {0..8} match XIClient's `ActorType`
/// enum, whose named members put doors (3), lifts (4) and models (5) outside the accepted set —
/// exactly the values this test rejects.
pub fn look_at_target_type_allowed(record_type: u8) -> bool {
    matches!(record_type, 0 | 1 | 2 | 6 | 7 | 8)
}

/// Whether an entity with this wire look can be looked at: the target-type half of retail's gate chain
/// is `look_at_target_type_allowed` applied to the Type byte that the s2c 0x0E SubKind dispatch stamps
/// onto the entity record. That stamping was re-read here from bytes: the dispatch
/// `mov al, byte ptr [esi + 0x30] / and eax, 7` at `FFXiMain.dll retail-2026-09` RVA 0x9C916 jumps
/// through the eight-entry table at RVA 0x9CE88, whose arms store Types 3 (RVA 0x9CB72), 4
/// (RVA 0x9CC11) and 5 (RVA 0x9CC7E) for exactly the three sizes named below and Types 2 / 1-or-0 / 6 /
/// 7 / 8 for the rest (RVA 0x9C9B7, 0x9C96F with its alternate at 0x9C94F, 0x9CD47, 0x9CDB1, 0x9CE5D).
/// The rejected sizes are ModelDoor/ModelElevator/ModelShip
/// (`vendor/server/src/map/packets/entity_update.h MODELTYPE`), and they are precisely the two variants
/// that keep their size through `ffxi_proto::decode::LookData`; every size folded into Standard (0/5/6)
/// or Equipped (1/7) stamps one of {2, 6, 7} / {1, 8}, all accepted - pinned by
/// `the_look_sizes_kuluu_collapse_are_all_aimable`. An entity with no decoded look is aimable too: the
/// stamped byte only ever comes from a handler that also carries that size.
pub fn look_at_target_record_allows(look: Option<&kuluu_snapshot::EntityLook>) -> bool {
    match look {
        Some(kuluu_snapshot::EntityLook::Door { size, .. })
        | Some(kuluu_snapshot::EntityLook::Transport { size, .. }) => look_at_target_type_allowed(
            ffxi_proto::decode::LookData::retail_type_of_look_size(*size),
        ),
        _ => true,
    }
}

/// How far from where a `0x89` LockLookAt stage fired an actor may stand before that stage's task ends
/// itself (`FFXiMain.dll retail-2026-09`: the task's update compares each axis separately at RVA 0x5F5AA
/// / RVA 0x5F60F against `1.0f` from `.rdata`). Only the two horizontal axes exist in the comparison —
/// it reads the vec3 that RVA 0xD5CD6 hands to the componentwise subtract at RVA 0x270A0, skipping its
/// middle component.
pub const LOCK_WATCHDOG_DISTANCE_YALMS: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookAtLockInterval {
    pub routine_instance: u64,
    pub stage_index: usize,
    pub fire_frame: u32,
    pub end_frame: u32,
}

/// A running `0x89` LockLookAt task. Retail keeps one per fired stage — each takes its own anchor in its
/// constructor (`FFXiMain.dll retail-2026-09`: RVA 0x5F4BC / RVA 0x5F4D2) and ends on its own duration or
/// watchdog, so overlapping intervals are tracked separately instead of merged into one window.
#[derive(Debug, Clone, Copy)]
pub struct LookAtLockTask {
    interval: LookAtLockInterval,
    anchor_xz: Vec2,
    released_early: bool,
}

impl LookAtLockTask {
    fn fired_at(interval: LookAtLockInterval, actor_xz: Vec2) -> Self {
        Self {
            interval,
            anchor_xz: actor_xz,
            released_early: false,
        }
    }

    pub fn suppressing(&self) -> bool {
        !self.released_early
    }
}

pub fn advance_look_at_locks(
    tasks: &mut Vec<LookAtLockTask>,
    open_intervals: &[LookAtLockInterval],
    actor_xz: Vec2,
) -> bool {
    tasks.retain(|task| open_intervals.contains(&task.interval));
    for &interval in open_intervals {
        if !tasks.iter().any(|task| task.interval == interval) {
            tasks.push(LookAtLockTask::fired_at(interval, actor_xz));
        }
    }
    for task in tasks.iter_mut() {
        let walked = (actor_xz - task.anchor_xz).abs();
        if walked.x >= LOCK_WATCHDOG_DISTANCE_YALMS || walked.y >= LOCK_WATCHDOG_DISTANCE_YALMS {
            task.released_early = true;
        }
    }
    tasks.iter().any(LookAtLockTask::suppressing)
}

pub fn advance_look_at_suppression(
    tasks: &mut Vec<LookAtLockTask>,
    open_intervals: &[LookAtLockInterval],
    actor_xz: Vec2,
    wire_animation: u8,
) -> bool {
    let lock_suppressed = advance_look_at_locks(tasks, open_intervals, actor_xz);
    !look_at_allowed(wire_animation) || lock_suppressed
}

#[cfg(test)]
mod look_at_gate_tests {
    use super::*;

    fn test_intervals(frames: &[(u32, u32)]) -> Vec<LookAtLockInterval> {
        frames
            .iter()
            .map(|&(fire_frame, end_frame)| LookAtLockInterval {
                routine_instance: 0,
                stage_index: fire_frame as usize,
                fire_frame,
                end_frame,
            })
            .collect()
    }

    #[test]
    fn status_suppression_keeps_task_anchors_and_watchdogs_live() {
        const LOCK_END_FRAME: u32 = 600;
        let interval = test_intervals(&[(0, LOCK_END_FRAME)]);
        let mut tasks = Vec::new();
        assert!(advance_look_at_suppression(
            &mut tasks,
            &interval,
            Vec2::ZERO,
            animation::ATTACK,
        ));
        assert_eq!(tasks.len(), 1);
        assert!(advance_look_at_suppression(
            &mut tasks,
            &interval,
            Vec2::X * LOCK_WATCHDOG_DISTANCE_YALMS,
            animation::ATTACK,
        ));
        assert!(!tasks[0].suppressing());
        assert!(!advance_look_at_suppression(
            &mut tasks,
            &interval,
            Vec2::ZERO,
            animation::NONE,
        ));
        assert!(advance_look_at_suppression(
            &mut tasks,
            &[],
            Vec2::ZERO,
            animation::ATTACK,
        ));
        assert!(tasks.is_empty());
    }

    #[test]
    fn standing_actors_look_at_their_target() {
        assert!(look_at_allowed(animation::NONE));
    }

    /// The whole allowed family, byte by byte: rest/sit/furniture/rides keep aiming at the target.
    #[test]
    fn the_rest_and_ride_family_keeps_its_look_at() {
        for animation in [
            animation::SIT,
            animation::RANGED,
            animation::CHOCOBO,
            animation::MOUNT,
            SIT_ON_FURNITURE_FIRST,
            0x4a,
            SIT_ON_FURNITURE_LAST,
        ] {
            assert!(
                look_at_allowed(animation),
                "animation byte {animation} must keep its look-at"
            );
        }
    }

    /// The three states retail picks per frame, decided from current inputs only.
    #[test]
    fn no_target_is_a_state_not_an_absence() {
        assert_eq!(
            look_state_of(true, false),
            LookState::IdleNoTarget,
            "a frame with nothing to look at owns its bones"
        );
        assert_eq!(look_state_of(true, true), LookState::Aiming);
        assert_eq!(
            look_state_of(false, true),
            LookState::Suppressed,
            "a resolved target behind a closed gate still does not aim"
        );
        assert_eq!(
            look_state_of(false, false),
            LookState::Suppressed,
            "gates decide before selection does"
        );
    }

    /// Bytes outside the family drop the target before aiming: mid-attack, dead, in an event, and the
    /// fishing run (which sits below the furniture block).
    #[test]
    fn attack_death_event_and_fishing_do_not_look_at_the_target() {
        for animation in [
            animation::ATTACK,
            3,  // vendor/server/data/enums/animation.yaml death
            4,  // ... event
            38, // fishing (pre-overhaul run)
            56, // ... and its current form
            62,
        ] {
            assert!(
                !look_at_allowed(animation),
                "animation byte {animation} must not look at the target"
            );
        }
    }

    #[test]
    fn furniture_beyond_the_named_sitchair_run_still_counts_as_furniture() {
        // The chain's block is wider than the yaml's 63..=73: it compares through 0x53.
        assert!(look_at_allowed(SIT_ON_FURNITURE_LAST));
        assert!(!look_at_allowed(SIT_ON_FURNITURE_LAST + 1));
    }

    /// Standing is the only state that bends shoulder along with head; every other allowed byte bends
    /// the head record alone, and a disallowed byte never reaches the bend at all.
    #[test]
    fn only_standing_bends_two_records() {
        assert_eq!(look_at_bend_records(animation::NONE), 2);
        for animation in [animation::SIT, animation::CHOCOBO, animation::MOUNT] {
            assert_eq!(look_at_bend_records(animation), ONE_BEND_RECORD);
        }
    }

    /// The accepted members of retail's six-comparison run, and the rejections its fallthrough makes.
    #[test]
    fn target_record_types_accepted_and_rejected_by_the_chain() {
        for record_type in [0u8, 1, 2, 6, 7, 8] {
            assert!(
                look_at_target_type_allowed(record_type),
                "record type {record_type} is a member of the chain"
            );
        }
        for record_type in [3u8, 4, 5, 9, 0x80].into_iter().chain(0x10..=0xff) {
            assert!(
                !look_at_target_type_allowed(record_type),
                "record type {record_type} takes the release path"
            );
        }
    }

    #[test]
    fn an_open_interval_suppresses_until_its_end_frame() {
        let mut tasks = Vec::new();
        let here = Vec2::ZERO;
        let interval = test_intervals(&[(10, 30)]);
        assert!(advance_look_at_locks(&mut tasks, &interval, here));
        assert!(tasks.iter().any(LookAtLockTask::suppressing));

        advance_look_at_locks(&mut tasks, &[], Vec2::ZERO);
        assert!(tasks.is_empty(), "a closed interval retires its task");
    }

    #[test]
    fn walking_a_yalm_gives_the_target_back_and_stays_given() {
        let mut tasks = Vec::new();
        let interval = test_intervals(&[(0, 600)]);
        assert!(advance_look_at_locks(&mut tasks, &interval, Vec2::ZERO));

        let walked = Vec2::new(1.0, 0.4);
        assert!(!advance_look_at_locks(&mut tasks, &interval, walked));

        // The stage's own fire frame is spent: the interval still covers it, but nothing re-arms.
        for _ in 0..3 {
            assert!(!advance_look_at_locks(&mut tasks, &interval, walked));
        }
        let suppressing = tasks.iter().any(LookAtLockTask::suppressing);
        assert!(!suppressing);
    }

    /// The comparison is per-axis and horizontal only — a diagonal half-yalm drift on each axis holds.
    #[test]
    fn the_watchdog_is_per_axis() {
        let mut tasks = Vec::new();
        let interval = test_intervals(&[(0, 600)]);
        advance_look_at_locks(&mut tasks, &interval, Vec2::ZERO);
        let drift = Vec2::new(
            LOCK_WATCHDOG_DISTANCE_YALMS * 0.9,
            LOCK_WATCHDOG_DISTANCE_YALMS * 0.9,
        );
        assert!(advance_look_at_locks(&mut tasks, &interval, drift));
    }

    #[test]
    fn overlapping_stages_end_independently() {
        let mut tasks = Vec::new();
        let intervals = test_intervals(&[(0, 600), (5, 8)]);
        advance_look_at_locks(&mut tasks, &intervals, Vec2::ZERO);
        assert_eq!(tasks.len(), 2);

        // The short one is gone; the longer one still suppresses.
        let only_long = test_intervals(&[(0, 600)]);
        assert!(advance_look_at_locks(&mut tasks, &only_long, Vec2::ZERO));
        assert_eq!(tasks.len(), 1);

        // Walking releases the survivor without touching the retired one's slot.
        assert!(!advance_look_at_locks(
            &mut tasks,
            &only_long,
            Vec2::new(0.0, LOCK_WATCHDOG_DISTANCE_YALMS)
        ));
    }
    /// `vendor/server/src/map/packets/entity_update.h MODELTYPE`: ModelDoor/ModelElevator/ModelShip are the
    /// three sizes whose stamped Type the gate chain rejects, and they are exactly the looks kuluu
    /// decodes into a variant that keeps its size.
    #[test]
    fn doors_lifts_and_ships_are_never_look_at_targets() {
        let door = Some(kuluu_snapshot::EntityLook::Door {
            size: ffxi_vocab::transport::MODEL_DOOR,
            door_id: None,
        });
        let lift = Some(kuluu_snapshot::EntityLook::Transport {
            size: ffxi_vocab::transport::MODEL_ELEVATOR,
            model_id: None,
            animation_start: None,
            travel_secs: Some(8),
        });
        let ship = Some(kuluu_snapshot::EntityLook::Transport {
            size: ffxi_vocab::transport::MODEL_SHIP,
            model_id: None,
            animation_start: None,
            travel_secs: None,
        });
        for look in [door.as_ref(), lift.as_ref(), ship.as_ref()] {
            assert!(
                !look_at_target_record_allows(look),
                "a transport/door record type must take the release path"
            );
        }
    }

    /// What makes `_ => true` above honest: every size folded into Standard (0/5/6) or Equipped (1/7)
    /// stamps an accepted Type - {2, 6, 7} and {1, 8}.
    #[test]
    fn the_look_sizes_kuluu_collapse_are_all_aimable() {
        let standard = [
            ffxi_vocab::transport::MODEL_STANDARD,
            ffxi_vocab::transport::MODEL_UNK_5,
            ffxi_vocab::transport::MODEL_AUTOMATON,
        ];
        let equipped = [
            ffxi_vocab::transport::MODEL_EQUIPPED,
            ffxi_vocab::transport::MODEL_CHOCOBO,
        ];
        for size in standard.into_iter().chain(equipped) {
            let record_type = ffxi_proto::decode::LookData::retail_type_of_look_size(size);
            assert!(
                look_at_target_type_allowed(record_type),
                "look.size {size} stamps the rejected record type {record_type}"
            );
        }
    }

    /// The three rejected sizes are named by LSB, not guessed: 2/3/4 out of MODELTYPE.
    #[test]
    fn the_rejected_look_sizes_are_the_three_named_transport_kinds() {
        let rejected = [
            ffxi_vocab::transport::MODEL_DOOR,
            ffxi_vocab::transport::MODEL_ELEVATOR,
            ffxi_vocab::transport::MODEL_SHIP,
        ];
        assert_eq!(rejected, [2, 3, 4]);
        for size in rejected {
            let record_type = ffxi_proto::decode::LookData::retail_type_of_look_size(size);
            assert!(!look_at_target_type_allowed(record_type));
        }
    }

    /// Only the two ride bytes drop the anchor, and they both keep their full bend records — so a mounted actor
    /// aims from lower down but still bends shoulder along with head.
    #[test]
    fn only_the_two_rides_lower_the_bend_anchor() {
        assert_eq!(look_at_anchor_y_drop(animation::CHOCOBO), 1.0);
        assert_eq!(look_at_anchor_y_drop(animation::MOUNT), 1.0);
        for animation in [animation::NONE, animation::SIT, animation::RANGED] {
            assert_eq!(look_at_anchor_y_drop(animation), 0.0);
        }
        // Both ride bytes also sit in retail's one-record set ({5, 0x2F, 0x30, 0x3F..0x53, 0x55}), so they
        // lower the anchor AND keep only the head bend; the two rules overlap on these bytes rather than being
        // the same rule.
        for animation in [animation::CHOCOBO, animation::MOUNT] {
            assert_eq!(
                look_at_bend_records(animation),
                1,
                "animation byte {animation} is in retail's one-record set"
            );
        }
    }
}
