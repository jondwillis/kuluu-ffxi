//! Animation *selection* — port of the id-producing methods of
//! `xim/poc/Actor.kt`: `getIdleAnimationId`, `getMovementAnimation`,
//! `getMovementDirection`, `getAnimationModeVariant`, plus the rest/sit/dead
//! routine ids from `DatResource.kt`.
//!
//! These produce parameterized [`DatId`]s (e.g. `idl?`, `run?`). The caller
//! resolves the trailing `?` against the actor's animation directories and
//! falls back to the base id when a mode variant has no clip — exactly as XIM
//! does via `getAnimationModeVariant`'s default-or-variant return.

use ffxi_dat::datid::DatId;

/// XIM `Direction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    None,
    Forward,
    Left,
    Right,
    Backward,
}

/// XIM `EngageAnimationState` — the four-state display machine that drives
/// battle-idle selection. This is distinct from logical combat state: only
/// `Engaged` and `Disengaging` are battle-idle (`btl?`); `Engaging` and
/// `NotEngaged` are normal idle (`idl?`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngageAnimationState {
    NotEngaged,
    Engaged,
    Engaging,
    Disengaging,
}

impl EngageAnimationState {
    /// XIM: `state == Engaged || state == Disengaging` selects `btl?`. Also the
    /// render-side engage-resolution key: when this flips, a future weapon-
    /// battle animation directory would win (or stop winning) the per-slot clip
    /// resolution, so the render path must re-resolve the registered clips even
    /// though the *selected id* (`run?`/`wlk?`) is unchanged.
    pub fn is_battle_idle(self) -> bool {
        matches!(self, Self::Engaged | Self::Disengaging)
    }
}

/// Rest postures. XIM dispatches these through `EffectRoutine` wrappers
/// (`res0`/`res1`/`res2` for the kneeling resting state; `chi0`/`chi2` only for
/// chair-sitting, which needs a chair furniture entity). Those routine ids are
/// *indirection*: each one's `SkeletonAnimationRoutine` opcode references a
/// parameterized `0x2B` animation id that actually drives the bones. The
/// player-driven `/sit` and `/heal` poses resolve (verified by scanning the
/// Hume-M skel DAT 7072) to these two-layer composites (like `run0`/`run1`):
///
/// - `/sit`  (ground sit)        -> `si0?`  (`si00` legs + `si01` torso/arms)
/// - `/heal` / kneel (CAMP pose) -> `rx0?`  (`rx00` legs + `rx01` torso/arms)
///
/// The routine names (`res0`/`sit0`/`chi0`) are NOT `0x2B` clips and never
/// match the render resolver, so the code must ask for the `si0?`/`rx0?` ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestKind {
    None,
    /// `/sit` — relaxed ground sit (`si0?`).
    Sit,
    /// `/heal` — the kneeling CAMP pose (`rx0?`), same resting state machine.
    Heal,
    /// Kneel — the kneeling resting state (`rx0?`).
    Kneel,
}

/// Inputs an actor exposes for animation selection. These mirror the fields
/// `Actor`/`ActorState` read inside the selection methods.
#[derive(Debug, Clone, Copy)]
pub struct ActorAnimInputs {
    pub moving: bool,
    pub walking: bool,
    /// Horizontal velocity projected onto the target/view direction
    /// (`velocity · targetDirection`). The caller MUST pre-project AND apply
    /// XIM's lock/strafe gate: pass `(0,0)` unless target-locked or strafing, so
    /// a free-moving actor yields `Direction::None` (see [`movement_direction`]).
    pub forward_vel: f32,
    /// Horizontal velocity projected onto the rightward axis (`velocity · right`).
    /// See `forward_vel` for the lock/strafe gating contract.
    pub strafe_vel: f32,
    pub heading_rate: f32,
    /// XIM `engageAnimationState` (display machine), NOT logical combat state.
    pub engage_state: EngageAnimationState,
    pub dead: bool,
    /// XIM `state.owner == null` — true when the actor has no owner. The `cor?`
    /// dead-idle branch only fires when dead AND un-owned; owned/pet corpses
    /// fall through to engaged/idle selection.
    pub owner_is_none: bool,
    /// XIM `state.mountedState?.getInfo()?.poseType` — `Some(poseType)` when this
    /// actor is a *rider* of a mount (distinct from being a mount itself).
    pub mount_pose_type: Option<u8>,
    /// XIM `isStaticNpc() && hasDftIdle()` — true only when a `dft0` clip
    /// actually resolves; otherwise the static NPC falls through to idle.
    pub has_dft_idle: bool,
    pub rest: RestKind,
    pub mount_or_chocobo: bool,
    pub static_npc: bool,
    /// `actorModel.idleAnimationMode`.
    pub idle_mode: u8,
    /// `actorModel.battleAnimationMode`.
    pub battle_mode: u8,
    /// `actorModel.runningAnimationMode` for walking.
    pub walking_mode: u8,
    /// `actorModel.runningAnimationMode` for running.
    pub running_mode: u8,
}

impl Default for ActorAnimInputs {
    fn default() -> Self {
        ActorAnimInputs {
            moving: false,
            walking: false,
            forward_vel: 0.0,
            strafe_vel: 0.0,
            heading_rate: 0.0,
            engage_state: EngageAnimationState::NotEngaged,
            dead: false,
            owner_is_none: true,
            mount_pose_type: None,
            has_dft_idle: false,
            rest: RestKind::None,
            mount_or_chocobo: false,
            static_npc: false,
            idle_mode: 0,
            battle_mode: 0,
            walking_mode: 0,
            running_mode: 0,
        }
    }
}

/// What the coordinator should play, and whether it is a low-priority idle clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedAnimation {
    pub id: DatId,
    /// True when this is an idle/low-priority clip (drives
    /// `registerIdleAnimation` vs `registerAnimation` at the call site).
    pub idle: bool,
}

/// XIM `getAnimationModeVariant`: when `mode` is 0, return `default_id`;
/// otherwise build the variant by prefixing the mode digit. Returns the
/// candidate ids in priority order — `[variant, default]` when a variant
/// exists, else `[default]`. The caller plays the first that resolves to a clip.
///
/// `variant_template` is the 3 base chars XIM uses inside the lambda, e.g.
/// for `idl?` the lambda is `DatId("${mode}dl?")`, so `variant_template = "dl"`
/// (the mode digit is prefixed and a trailing `?` appended).
pub fn animation_mode_variant(default_id: DatId, mode: u8, variant_template: &str) -> Vec<DatId> {
    if mode == 0 {
        return vec![default_id];
    }
    // e.g. mode=2, template "dl" -> "2dl?"
    let variant = DatId::from_str(&format!("{}{}?", mode, variant_template));
    vec![variant, default_id]
}

/// XIM `getIdleAnimationId`. Returns the candidate ids in priority order (mode
/// variant first when applicable). The caller resolves the first that has a
/// clip and falls back to the base.
pub fn idle_animation_id(inputs: &ActorAnimInputs) -> Vec<DatId> {
    // chocobo || isMount() => chi?  (the mount actor itself).
    if inputs.mount_or_chocobo {
        return vec![DatId::from_str("chi?")];
    }

    // mountedState?.getInfo() != null => `${poseType}un?`  (the rider).
    if let Some(pose_type) = inputs.mount_pose_type {
        return vec![DatId::from_str(&format!("{}un?", pose_type))];
    }

    // isStaticNpc() && hasDftIdle() => dft?  (only when a dft0 clip resolves).
    if inputs.static_npc && inputs.has_dft_idle {
        return vec![DatId::from_str("dft?")];
    }

    // isDisplayedDead() && owner == null => cor? variant {mode}cr?.
    if inputs.dead && inputs.owner_is_none {
        return animation_mode_variant(DatId::from_str("cor?"), inputs.idle_mode, "cr");
    }

    if inputs.engage_state.is_battle_idle() {
        // btl? with mode variant {mode}tl?
        return animation_mode_variant(DatId::from_str("btl?"), inputs.battle_mode, "tl");
    }

    // idl? with mode variant {mode}dl?
    animation_mode_variant(DatId::from_str("idl?"), inputs.idle_mode, "dl")
}

/// XIM `getMovementDirection`. Returns `Direction::None` when not effectively
/// moving (the velocity magnitude floor mirrors XIM's `1e-5` on the squared
/// horizontal speed). Thresholds: cos >= 0.25 forward; cos >= -0.75 horizontal
/// (right when strafe >= 0, else left); below that, backward.
///
/// IMPORTANT — lock/strafe gate: XIM's `getMovementDirection` first selects a
/// `targetDirection` (locked-target vector if target-locked, else camera view
/// vector if strafing, else immediately returns `Direction.None`) and only then
/// projects the horizontal velocity onto it. This function takes the ALREADY
/// PROJECTED scalars `forward_vel = velocity·targetDirection` and
/// `strafe_vel = velocity·right`, so the caller MUST replicate XIM's gate:
/// compute these projections ONLY when target-locked or strafing, and pass
/// `(0,0)` otherwise so a free-moving actor yields `Direction::None`. Passing a
/// non-zero projection for an unlocked, non-strafing actor would spuriously
/// select `mvl?`/`mvr?`/`mvb?`.
pub fn movement_direction(forward_vel: f32, strafe_vel: f32) -> Direction {
    let speed_sq = forward_vel * forward_vel + strafe_vel * strafe_vel;
    if speed_sq <= 1e-5 {
        return Direction::None;
    }

    let inv = 1.0 / speed_sq.sqrt();
    // cosAngle = normalizedVelocity . targetDirection. Treat forward_vel as the
    // component along the target/view direction, strafe_vel as the rightward
    // component (matching XIM's dot products against targetDirection / right).
    let cos_angle = forward_vel * inv;

    if cos_angle >= 0.25 {
        Direction::Forward
    } else if cos_angle >= -0.75 {
        let horizontal_cos = strafe_vel * inv;
        if horizontal_cos >= 0.0 {
            Direction::Right
        } else {
            Direction::Left
        }
    } else {
        Direction::Backward
    }
}

/// XIM `getMovementAnimation`. Returns candidate ids in priority order.
pub fn movement_animation(inputs: &ActorAnimInputs) -> Vec<DatId> {
    if inputs.walking {
        // wlk? with mode variant {mode}lk?
        return animation_mode_variant(DatId::from_str("wlk?"), inputs.walking_mode, "lk");
    }

    match movement_direction(inputs.forward_vel, inputs.strafe_vel) {
        Direction::None | Direction::Forward => {
            // run? with mode variant {mode}un?
            animation_mode_variant(DatId::from_str("run?"), inputs.running_mode, "un")
        }
        Direction::Left => vec![DatId::from_str("mvl?")],
        Direction::Right => vec![DatId::from_str("mvr?")],
        Direction::Backward => vec![DatId::from_str("mvb?")],
    }
}

/// The three phases of a rest posture. FFXI plays a transition-IN (kneel/sit
/// down), holds a LOOP (subtle breathing), then plays a transition-OUT (stand
/// up) on exit. XIM drives this by enqueuing `startResting`/`stopResting` model
/// routines (`Actor.kt:740` `updateRestingDisplay`); the underlying `0x2B`
/// clips encode the phase in the id's 3rd char — `*0?` in / `*1?` loop / `*2?`
/// out (ground sit `si0?`/`si1?`/`si2?`, kneel `rx0?`/`rx1?`/`rx2?`, all
/// present in the Hume-M skel DAT).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestPhase {
    /// Transition INTO the pose (kneel/sit down) — plays once.
    In,
    /// Held resting LOOP (subtle breathing) — loops while resting.
    Loop,
    /// Transition OUT of the pose (stand up) — plays once on exit.
    Out,
}

/// Rest-pose clip id for a posture + [`RestPhase`] — the underlying
/// parameterized `0x2B` clip the render resolver can `parameterized_match`, NOT
/// the `EffectRoutine` wrapper names (`res0`/`sit0`/`chi0`), which are `0x07`
/// chunks that never appear in the actor's animation set (so they'd resolve to
/// nothing and fall back to a standing idle — the bug this fixes). The render
/// path plays `In` once, loops `Loop`, then plays `Out` once when rest ends:
///   * `Sit`          -> `si{0,1,2}?` (ground sit: `si?0` legs + `si?1` torso/arms)
///   * `Heal`/`Kneel` -> `rx{0,1,2}?` (kneel/CAMP: `rx?0` legs + `rx?1` torso/arms)
///   * `None`         -> no clip.
pub fn rest_animation_id_phase(rest: RestKind, phase: RestPhase) -> Option<DatId> {
    let prefix = match rest {
        RestKind::None => return None,
        RestKind::Sit => b"si",
        RestKind::Heal | RestKind::Kneel => b"rx",
    };
    let digit = match phase {
        RestPhase::In => b'0',
        RestPhase::Loop => b'1',
        RestPhase::Out => b'2',
    };
    Some(DatId::from_name(&[prefix[0], prefix[1], digit, b'?']))
}

/// The two-layer START clip a rest posture begins with (`si0?`/`rx0?`); the
/// transition-IN phase of [`rest_animation_id_phase`]. Kept as the entry point
/// the clip resolver/regression tests reference.
pub fn rest_animation_id(rest: RestKind) -> Option<DatId> {
    rest_animation_id_phase(rest, RestPhase::In)
}

/// The corpse model routine (XIM `enqueueModelRoutineIfReady(DatId("corp"))`).
pub fn corpse_routine_id() -> DatId {
    DatId::from_str("corp")
}

/// Top-level selection: idle id when not moving, movement id when moving.
/// Returns the resolved primary candidate plus the idle flag. Use
/// [`idle_animation_id`]/[`movement_animation`] directly when you need the full
/// fallback list.
pub fn selected_animation(inputs: &ActorAnimInputs) -> SelectedAnimation {
    if inputs.moving {
        let id = movement_animation(inputs)[0];
        SelectedAnimation { id, idle: false }
    } else {
        let id = idle_animation_id(inputs)[0];
        SelectedAnimation { id, idle: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idstr(id: DatId) -> String {
        id.as_str()
    }

    #[test]
    fn idle_states_map_to_base_ids() {
        let mut i = ActorAnimInputs::default();
        assert_eq!(idstr(idle_animation_id(&i)[0]), "idl?");

        i.engage_state = EngageAnimationState::Engaged;
        assert_eq!(idstr(idle_animation_id(&i)[0]), "btl?");
        i.engage_state = EngageAnimationState::NotEngaged;

        i.dead = true; // owner_is_none defaults to true
        assert_eq!(idstr(idle_animation_id(&i)[0]), "cor?");
        i.dead = false;

        i.mount_or_chocobo = true;
        assert_eq!(idstr(idle_animation_id(&i)[0]), "chi?");
        i.mount_or_chocobo = false;

        // static_npc only yields dft? when a dft0 clip resolves.
        i.static_npc = true;
        i.has_dft_idle = true;
        assert_eq!(idstr(idle_animation_id(&i)[0]), "dft?");
    }

    #[test]
    fn static_npc_without_dft_idle_falls_through_to_idl() {
        // XIM: isStaticNpc() && hasDftIdle(); without a dft0 clip, fall through.
        let i = ActorAnimInputs {
            static_npc: true,
            has_dft_idle: false,
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "idl?");

        // And when engaged but no dft clip, fall through to btl?.
        let i = ActorAnimInputs {
            static_npc: true,
            has_dft_idle: false,
            engage_state: EngageAnimationState::Engaged,
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "btl?");
    }

    #[test]
    fn rider_uses_pose_type_un_branch() {
        // mountedState.getInfo() present (rider) => `${poseType}un?`. Distinct
        // from chi? (the mount itself).
        let i = ActorAnimInputs {
            mount_pose_type: Some(3),
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "3un?");

        // chi? (mount actor) takes priority over the rider branch.
        let i = ActorAnimInputs {
            mount_or_chocobo: true,
            mount_pose_type: Some(3),
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "chi?");
    }

    #[test]
    fn dead_owned_corpse_falls_through() {
        // dead but owned (owner != null) must NOT select cor?; falls to idl?.
        let i = ActorAnimInputs {
            dead: true,
            owner_is_none: false,
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "idl?");

        // dead, owned, AND engaged => btl? (engaged idle), not cor?.
        let i = ActorAnimInputs {
            dead: true,
            owner_is_none: false,
            engage_state: EngageAnimationState::Engaged,
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "btl?");

        // dead and un-owned => cor? (the original behavior).
        let i = ActorAnimInputs {
            dead: true,
            owner_is_none: true,
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "cor?");
    }

    #[test]
    fn engage_state_battle_idle_classification() {
        // Engaged and Disengaging => btl?; NotEngaged and Engaging => idl?.
        for state in [
            EngageAnimationState::Engaged,
            EngageAnimationState::Disengaging,
        ] {
            let i = ActorAnimInputs {
                engage_state: state,
                ..Default::default()
            };
            assert_eq!(idstr(idle_animation_id(&i)[0]), "btl?", "{state:?}");
        }
        for state in [
            EngageAnimationState::NotEngaged,
            EngageAnimationState::Engaging,
        ] {
            let i = ActorAnimInputs {
                engage_state: state,
                ..Default::default()
            };
            assert_eq!(idstr(idle_animation_id(&i)[0]), "idl?", "{state:?}");
        }
    }

    #[test]
    fn mount_takes_priority_over_dead_and_engaged() {
        let i = ActorAnimInputs {
            mount_or_chocobo: true,
            dead: true,
            engage_state: EngageAnimationState::Engaged,
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "chi?");
    }

    #[test]
    fn idle_mode_variant_produced() {
        let i = ActorAnimInputs {
            idle_mode: 2,
            ..Default::default()
        };
        let ids = idle_animation_id(&i);
        // variant first, base second
        assert_eq!(idstr(ids[0]), "2dl?");
        assert_eq!(idstr(ids[1]), "idl?");
    }

    #[test]
    fn battle_mode_variant_produced() {
        let i = ActorAnimInputs {
            engage_state: EngageAnimationState::Engaged,
            battle_mode: 3,
            ..Default::default()
        };
        let ids = idle_animation_id(&i);
        assert_eq!(idstr(ids[0]), "3tl?");
        assert_eq!(idstr(ids[1]), "btl?");
    }

    #[test]
    fn dead_mode_variant_produced() {
        let i = ActorAnimInputs {
            dead: true,
            idle_mode: 1,
            ..Default::default()
        };
        let ids = idle_animation_id(&i);
        assert_eq!(idstr(ids[0]), "1cr?");
        assert_eq!(idstr(ids[1]), "cor?");
    }

    #[test]
    fn movement_ids_by_direction() {
        // walking
        let walk = ActorAnimInputs {
            walking: true,
            ..Default::default()
        };
        assert_eq!(idstr(movement_animation(&walk)[0]), "wlk?");

        // forward
        let fwd = ActorAnimInputs {
            forward_vel: 1.0,
            ..Default::default()
        };
        assert_eq!(idstr(movement_animation(&fwd)[0]), "run?");

        // none -> run? (XIM groups None with Forward)
        let none = ActorAnimInputs::default();
        assert_eq!(idstr(movement_animation(&none)[0]), "run?");

        // left: cos_angle in [-0.75, 0.25), strafe < 0
        let left = ActorAnimInputs {
            forward_vel: -0.5,
            strafe_vel: -1.0,
            ..Default::default()
        };
        assert_eq!(idstr(movement_animation(&left)[0]), "mvl?");

        // right: strafe > 0, forward small
        let right = ActorAnimInputs {
            forward_vel: 0.0,
            strafe_vel: 1.0,
            ..Default::default()
        };
        assert_eq!(idstr(movement_animation(&right)[0]), "mvr?");

        // backward: cos_angle < -0.75
        let back = ActorAnimInputs {
            forward_vel: -1.0,
            strafe_vel: 0.0,
            ..Default::default()
        };
        assert_eq!(idstr(movement_animation(&back)[0]), "mvb?");
    }

    #[test]
    fn movement_direction_thresholds() {
        // pure forward
        assert_eq!(movement_direction(1.0, 0.0), Direction::Forward);
        // exactly at 0.25 boundary forward (cos = 0.25)
        assert_eq!(
            movement_direction(0.25, (1.0f32 - 0.0625).sqrt()),
            Direction::Forward
        );
        // pure backward
        assert_eq!(movement_direction(-1.0, 0.0), Direction::Backward);
        // pure right
        assert_eq!(movement_direction(0.0, 1.0), Direction::Right);
        // pure left
        assert_eq!(movement_direction(0.0, -1.0), Direction::Left);
        // stationary
        assert_eq!(movement_direction(0.0, 0.0), Direction::None);
    }

    #[test]
    fn running_mode_variant_for_run() {
        let i = ActorAnimInputs {
            forward_vel: 1.0,
            running_mode: 4,
            ..Default::default()
        };
        let ids = movement_animation(&i);
        assert_eq!(idstr(ids[0]), "4un?");
        assert_eq!(idstr(ids[1]), "run?");
    }

    #[test]
    fn walking_mode_variant() {
        let i = ActorAnimInputs {
            walking: true,
            walking_mode: 5,
            ..Default::default()
        };
        let ids = movement_animation(&i);
        assert_eq!(idstr(ids[0]), "5lk?");
        assert_eq!(idstr(ids[1]), "wlk?");
    }

    #[test]
    fn rest_ids() {
        assert!(rest_animation_id(RestKind::None).is_none());
        assert_eq!(idstr(rest_animation_id(RestKind::Sit).unwrap()), "si0?");
        assert_eq!(idstr(rest_animation_id(RestKind::Heal).unwrap()), "rx0?");
        assert_eq!(idstr(rest_animation_id(RestKind::Kneel).unwrap()), "rx0?");
        assert_eq!(idstr(corpse_routine_id()), "corp");
    }

    #[test]
    fn rest_phase_ids() {
        use RestPhase::{In, Loop, Out};
        // 3rd char encodes the phase: in=0, loop=1, out=2.
        assert_eq!(idstr(rest_animation_id_phase(RestKind::Sit, In).unwrap()), "si0?");
        assert_eq!(idstr(rest_animation_id_phase(RestKind::Sit, Loop).unwrap()), "si1?");
        assert_eq!(idstr(rest_animation_id_phase(RestKind::Sit, Out).unwrap()), "si2?");
        assert_eq!(idstr(rest_animation_id_phase(RestKind::Kneel, In).unwrap()), "rx0?");
        assert_eq!(idstr(rest_animation_id_phase(RestKind::Heal, Loop).unwrap()), "rx1?");
        assert_eq!(idstr(rest_animation_id_phase(RestKind::Kneel, Out).unwrap()), "rx2?");
        assert!(rest_animation_id_phase(RestKind::None, In).is_none());
    }

    #[test]
    fn selected_animation_switches_on_moving() {
        let idle = ActorAnimInputs::default();
        let sel = selected_animation(&idle);
        assert_eq!(idstr(sel.id), "idl?");
        assert!(sel.idle);

        let moving = ActorAnimInputs {
            moving: true,
            forward_vel: 1.0,
            ..Default::default()
        };
        let sel = selected_animation(&moving);
        assert_eq!(idstr(sel.id), "run?");
        assert!(!sel.idle);
    }
}
