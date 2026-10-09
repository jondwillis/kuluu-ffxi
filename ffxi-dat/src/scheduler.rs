use crate::{DatError, DatRoot, Result};

// The effect list of a routine whose control-flow section (section 1) is empty. Kept as the
// fallback for chunks whose section table reads implausibly — see `effect_section_start`.
pub const SCHEDULER_HEADER_LEN: usize = 64;

// research/xim EffectRoutineParser.kt read — after four zero dwords the routine header holds
// three u32 section offsets (section 1 = control-flow setup, 2 = the effect list, 3 = trailer),
// each measured from the CHUNK header, which begins `CHUNK_HEADER_LEN` before `body`.
const SECTION_TABLE_OFFSET: usize = 0x10;
const SECTION2_SLOT: usize = SECTION_TABLE_OFFSET + 4;
const CHUNK_HEADER_LEN: usize = 0x10;
// Three section offsets plus `totalDelay` (EffectRoutineParser.kt read sec1Offset).
const SECTION_TABLE_LEN: usize = 0x10;

// research/xim EffectRoutineParser.kt — `numInputs = (unkCombo and 0x1F) - 1`, counted from
// the dword that carries the opcode, so the stage spans `unkCombo & 0x1F` dwords in total.
const STAGE_LENGTH_MASK: u16 = 0x1F;

// research/xim EffectRoutineParser.kt parseSection,96-98 / :275-285.
const END_ROUTINE_OPCODE: u8 = 0x00;

// FFXiMain.dll retail-2026-09 RVA 0x5B5DA gates playback through the control-actor predicate.
pub const PLAYER_ONLY_SOUND_OPCODE: u8 = 0x4A;
const RANDOM_BLOCK_OPEN: u8 = 0x3D;
const RANDOM_BLOCK_CLOSE: u8 = 0x3E;

// research/xim EffectRoutineParser.kt parseSection2 — 0x64/0x67 ControlFlowBranch, 0x69/0x6A
// ControlFlowBlock, 0x6B ControlFlowCondition.
pub const CONTROL_FLOW_BRANCH_TRUE: u8 = 0x64;
pub const CONTROL_FLOW_BRANCH_FALSE: u8 = 0x67;
pub const CONTROL_FLOW_BLOCK_OPEN: u8 = 0x69;
pub const CONTROL_FLOW_BLOCK_CLOSE: u8 = 0x6A;
pub const CONTROL_FLOW_CONDITION: u8 = 0x6B;
const ANIMATION_LOCK_OPCODE: u8 = 0x07;
const ANIMATION_LOCK_MAGIC_OPCODE: u8 = 0x59;
// research/xim EffectRoutineParser.kt parseSection2 — the argument-less stages: StartRoutineMarker,
// ActorPositionSnapshotEffect, MovementLockEffect, FacingLockEffect, and the two
// ToggleBroadcastEffect arms.
const START_ROUTINE_MARKER_OPCODE: u8 = 0x01;
const ACTOR_POSITION_SNAPSHOT_OPCODE: u8 = 0x15;
const MOVEMENT_LOCK_OPCODE: u8 = 0x2E;
pub const FACING_LOCK_OPCODE: u8 = 0x2F;
const TOGGLE_BROADCAST_ON_OPCODE: u8 = 0x31;
const TOGGLE_BROADCAST_OFF_OPCODE: u8 = 0x32;
const FLINCH_CASTER_OPCODE: u8 = 0x21;
const FLINCH_TARGET_OPCODE: u8 = 0x25;
const TRANSITION_TO_IDLE_OPCODE: u8 = 0x28;
const ACTOR_FADE_CASTER_OPCODE: u8 = 0x29;
const ACTOR_FADE_TARGET_OPCODE: u8 = 0x2A;
const KNOCKBACK_OPCODE: u8 = 0x5E;
const KNOCKBACK_ALT_OPCODE: u8 = 0xBF;
const STOP_ROUTINE_OPCODE: u8 = 0x5F;
const DISPLAY_DEAD_OPCODE: u8 = 0x78;
// FFXiMain.dll retail-2026-09 dispatches stages as `case = byte - 2` (`lea edx,[eax-2]` at RVA
// 0x57FC9, jump table RVA 0x5DC1C); this byte's handler allocates its task at `0x80` bytes (RVA
// 0x5B15B) and builds it at RVA 0x5F450. The operand comes from the shared stage reader at RVA
// 0x5E590: signed word `[record+6]`, scaled by the routine context's timing scale.
const LOCK_LOOK_AT_OPCODE: u8 = 0x89;
// Both bytes build the same task class and read the same record layout; they differ only in which
// resolver gates construction. Stage 0xA9 (case 167, handler RVA 0x5B392) calls the gate at RVA
// 0x10062770 and jumps onto the shared argument tail from RVA 0x5B3DD; stage 0xAA (case 168,
// handler RVA 0x5B3DF) calls the gate at RVA 0x100627D0. Each allocates 0xA0 bytes (`push 0xa0`:
// RVA 0x5B3A1 / RVA 0x5B3EE) for `CMoActorRotationDriveTask`, whose constructor is at
// FFXiMain.dll retail-2026-09 RVA 0x5FA20. Only stage 0xA9 occurs in the shipped DATs.
pub const ACTOR_ROTATION_OPCODE: u8 = 0xA9;
const ACTOR_ROTATION_ALT_OPCODE: u8 = 0xAA;
// The constructor reads three degree floats at record +8/+0xC/+0x10 (each multiplied by pi/180 at
// FFXiMain.dll retail-2026-09 RVA 0x5FA95 / RVA 0x5FABB / RVA 0x5FACB) and one byte at record
// +0x14, after the shared delay/duration word pair - a six-dword stage with no DatId.
const ACTOR_ROTATION_PAYLOAD_LEN: usize = 24;
const ACTOR_ROTATION_ANGLES_OFFSET: usize = ID_OFFSET;
const ACTOR_ROTATION_MODE_OFFSET: usize = ID_OFFSET + 12;
// FFXiMain.dll retail-2026-09 dispatch case 96 (jump-table cell RVA 0x5DD9C, handler RVA 0x5AF2C):
// the stage queues a bounded yaw turn of its actor toward the routine's target. Both objects come
// from the routine context - the turning actor through RVA 0x10062770 and the object to face through
// RVA 0x100627D0 - and if either fails to resolve, nothing is stored at all (both branches go to RVA
// 0x5AC96). Positions come from vtable slot byte `0x1BC` and the heading being corrected from slot
// byte `0x1C0`. The authored float after delay/duration becomes radians with the same factor
// ActorRotation uses (multiplied at RVA 0x5B019 reading `.rdata 0x32A9F4`) and is stored on the actor,
// sign-matched to the geometric difference, as the turn's per-frame step. Its duration word is read by
// the shared stage reader (RVA 0x1005E590) but - unlike every lock task - never rounded down first, so
// the fetch keeps its fraction; it feeds the companion task at RVA 0x60F80, which releases its bump of
// the actor's turn-enable counter when that countdown runs out (destructor RVA 0x60F40).
pub const TURN_TOWARD_OPCODE: u8 = 0x62;
// The step is the only payload word: a three-dword stage, so the slot other kinds read as DatId holds
// it (`FFXiMain.dll retail-2026-09`: the load at RVA 0x5B016 sits on the record's +8 offset, which the
// shared delay/duration pair occupies two dwords earlier).
const TURN_TOWARD_PAYLOAD_LEN: usize = STAGE_WITH_ID_LEN;
const TURN_TOWARD_STEP_OFFSET: usize = ID_OFFSET;
// research/xim EffectRoutineParser.kt parseSection2 0x75 SetModelVisibilityRoutine: the payload
// after delay/duration is hidden (u32 == 1), slot (u16), ifEngaged (u16 == 1) - a 4-dword
// stage, no DatId.
const SET_MODEL_VISIBILITY_OPCODE: u8 = 0x75;
const SET_MODEL_VISIBILITY_PAYLOAD_LEN: usize = 16;
// research/xim EffectRoutineParser.kt parseSection2 0x22 JointSnapshotEffect: the u32 after
// delay/duration is consumed and unused; the handler only sets the context's joint-snapshot
// flag (EffectRoutineInstance.kt handleJointSnapshotEffect applyJointSnapshot(true)).
const JOINT_SNAPSHOT_OPCODE: u8 = 0x22;
// research/xim EffectRoutineParser.kt parseSection2 0x1E ParticleDampenRoutine: genRef
// (DatId) + zero32 after delay/duration - a 4-dword stage.
pub const PARTICLE_DAMPEN_OPCODE: u8 = 0x1E;
// research/xim EffectRoutineParser.kt parseSection2 0x19 SpellEffect: the u32 after
// delay/duration is the spell animation index, not a DatId - the handler resolves the
// spell file-table offset plus the index to the effect DAT and runs its `main` routine
// on the actor (EffectRoutineInstance.kt handleSpellEffect).
const SPELL_EFFECT_OPCODE: u8 = 0x19;
const ELEVATOR_TRAVEL_OPCODE: u8 = 0x1D;

// research/xim EffectRoutineParser.kt — parseSection2 reads delay(+4) and duration(+6)
// for EVERY opcode before dispatching, so the shortest stage the encoding admits is 8 bytes.
// Opcodes that take an id argument (+8) are 12 bytes or longer.
const STAGE_HEADER_LEN: usize = 8;
const STAGE_WITH_ID_LEN: usize = 12;
const DELAY_OFFSET: usize = 4;
const DURATION_OFFSET: usize = 6;
const ID_OFFSET: usize = 8;

// research/xim EffectRoutineParser.kt parseSection2: after id(+8), a zero32(+12) and two floats
// (+16,+20), the 0x05 motion payload carries transitionIn(+24), a zero u16(+26),
// transitionOut(+28), maxLoop(+30).
const MOTION_PAYLOAD_LEN: usize = 32;
const MOTION_TRANSITION_IN_OFFSET: usize = 24;
const MOTION_TRANSITION_OUT_OFFSET: usize = 28;
const MOTION_MAX_LOOP_OFFSET: usize = 30;

// research/xim EffectRoutineParser.kt parseSection2, opcodes 0x0C/0x0D: a Vector3f then a u32
// index, read straight after duration. Verified against the shipped DATs rather than trusted:
// every one of the 18829 0x0C/0x0D stages across the 85962 resolvable files is exactly six
// dwords long, none carries a byte past +24, and the +20 dword is only ever 0..=3.
const MODEL_TRANSFORM_PAYLOAD_LEN: usize = 24;
const MODEL_TRANSFORM_VECTOR_OFFSET: usize = 8;
const MODEL_TRANSFORM_SUBCHUNK_OFFSET: usize = 20;

// research/xim EffectRoutineParser.kt parseFlinchEffect: after delay/duration the
// flinch payload is f32, f32, u32, f32, f32 animationDuration, u32, u32 - a 9-dword stage.
// The duration drives retail's flinch transition times (animationDuration/2 each side,
// EffectRoutineInterpolatedEffects.kt FlinchAnimationInstance). Verified against the shipped
// DATs: Rarab's `damg` carries 10.0 here and its stage is exactly nine dwords.
const FLINCH_ANIMATION_DURATION_OFFSET: usize = 24;
const FLINCH_PAYLOAD_LEN: usize = FLINCH_ANIMATION_DURATION_OFFSET + 4;

// research/xim EffectRoutineParser.kt parseSoundEffectEmitter — after id(+8) come a zero32(+12),
// an unused u32(+16), far f32(+20), near f32(+24) and an unused f32(+28): the emitter's own
// AudioRangeSetup. A stage shorter than this ships no authored range; Calc3D substitutes its
// class defaults for a 0.0 (CYySepRes.cpp CYySepRes::Calc3D).
const SOUND_FAR_OFFSET: usize = ID_OFFSET + 12;
const SOUND_NEAR_OFFSET: usize = ID_OFFSET + 16;
const SOUND_EMITTER_PAYLOAD_LEN: usize = SOUND_NEAR_OFFSET + 4;

// research/xim EffectRoutineParser.kt parseSection2, 0x5E / 0xBF: after delay/duration the
// knockback payload is u16, u16, f32 animationDuration, f32, u32. The duration is how long
// the victim's bf0? knock-down plays before the bf1? stand-up
// (EffectRoutineInterpolatedEffects.kt KnockBackInstance).
const KNOCKBACK_ANIMATION_DURATION_OFFSET: usize = ID_OFFSET + 4;
const KNOCKBACK_DURATION_PAYLOAD_LEN: usize = KNOCKBACK_ANIMATION_DURATION_OFFSET + 4;

const CONTROL_FLOW_OPERAND_OFFSET: usize = ID_OFFSET + 4;
const CONTROL_FLOW_OPERAND_PAYLOAD_LEN: usize = CONTROL_FLOW_OPERAND_OFFSET + 4;

// A stage addresses a slot of the group `mzb::underscore_at_groups` builds, so the bound is
// that builder's rather than a second reading of the same retail array.
pub const MODEL_TRANSFORM_SUBCHUNK_SLOTS: u32 =
    crate::mzb::UNDERSCORE_AT_GROUP_MAX_SUBCHUNKS as u32;

pub const NO_STAGE_ID: [u8; 4] = [0; 4];

/// The MODULATE2X argument that leaves the scene untinted. research/XIClient
/// `GameManager::RenderSomething` composites the persistent screen colour with
/// `D3DTOP_MODULATE2X`, whose identity is 0x80 — which is why the authored
/// fade-in destination is 128,128,128 rather than 255,255,255.
pub const SCREEN_COLOR_UNIT: u8 = 0x80;

/// The untinted actor colour an ActorFade stage returns to: the modulate-2x
/// identity in every channel (the worm's `init` fades back to it).
pub const ACTOR_FADE_NEUTRAL: [u8; 4] = [SCREEN_COLOR_UNIT; 4];

/// A [`StageKind::ScreenColorDrive`] destination, in the DAT's own byte scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenColor {
    pub rgba: [u8; 4],
}

impl ScreenColor {
    /// The destination as a multiplier of the untinted scene: 1.0 leaves it
    /// alone, 0.0 drives the channel to black.
    pub fn tint(self) -> [f32; 4] {
        self.rgba
            .map(|c| f32::from(c) / f32::from(SCREEN_COLOR_UNIT))
    }
}

fn effect_section_start(body: &[u8]) -> usize {
    let Some(raw) = body
        .get(SECTION2_SLOT..SECTION2_SLOT + 4)
        .and_then(|b| b.try_into().ok())
        .map(u32::from_le_bytes)
    else {
        return SCHEDULER_HEADER_LEN;
    };
    let raw = raw as usize;
    let start = raw.saturating_sub(CHUNK_HEADER_LEN);
    // The effect list can never begin inside the header that describes it, and a truncated
    // chunk must not send the cursor past the end of the body.
    let first_body_offset = SECTION_TABLE_OFFSET + SECTION_TABLE_LEN;
    if raw >= CHUNK_HEADER_LEN + first_body_offset && start < body.len() {
        start
    } else {
        SCHEDULER_HEADER_LEN
    }
}

// research/xim EffectRoutineEffects.kt ModelTransformEffect. `final_value` is an OFFSET FROM
// the placement's authored transform, reached across the stage's `duration_frames`; rotation
// is radians about each axis, translation yalms. Read as absolute it would fling every shut
// door in the game to yaw 0: the DAT closes doors authored at 90°, 180°, 225° and 315° with
// the same `clos` value of 0,0,0, and parks Mea's `_pmd` lift at the world origin. XIClient's
// HandleTag0x0C/0x0D are undecompiled, so the DAT is the authority here, not the disassembly.
// `subchunk` selects one placement of the routine directory's BlockID group.
const FOLLOW_POINTS_PAYLOAD_LEN: usize = 40;
const FOLLOW_POINTS_FLAGS_OFFSET: usize = 16;
const FOLLOW_POINTS_ROTATION_OFFSET: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FollowPoints {
    pub flags: u32,
    pub easing: u32,
    pub rotation: f32,
}

/// Authored orientation for a [`StageKind::ActorRotation`] stage: three angles in degrees at the
/// record order retail passes them to its constructor, and the mode byte that selects which of the
/// two write paths the task takes (its tick branches on it at FFXiMain.dll retail-2026-09 RVA
/// 0x5FBCF..0x5FBDC).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActorRotation {
    pub angles_degrees: [f32; 3],
    pub mode: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelTransform {
    pub final_value: [f32; 3],
    pub subchunk: u32,
}

/// 0x75 SetModelVisibility payload (research/xim EffectRoutineParser.kt parseSection2):
/// show or hide one model slot of the actor for the stage's duration. Slot 2 is the
/// weapon slot, hidden by default in retail (research/xim ActorModel.kt getHiddenSlotIds);
/// `if_engaged` limits the override to engaged actors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelVisibility {
    pub hidden: bool,
    pub slot: u16,
    pub if_engaged: bool,
}

/// Which family a model resolves its motion ids against, as switched by
/// [`StageKind::AnimationMode`] (research/xim EffectRoutineInstance.kt handleAdjustAnimationModeRoutine:
/// case 0 battle, 1 idle, 2 walking, 3 running).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimModeSlot {
    Battle,
    Idle,
    Walking,
    Running,
}

impl AnimModeSlot {
    /// The handler's case number, which is also the index a consumer stores these by.
    pub fn index(self) -> usize {
        match self {
            Self::Battle => 0,
            Self::Idle => 1,
            Self::Walking => 2,
            Self::Running => 3,
        }
    }
}

/// Payload of the four animation-mode stages: one dword (`research/xim EffectRoutineParser.kt
/// parseSection2`) naming the variant the actor's later motion ids take, with the unmarked id kept as the
/// fallback (`Actor.kt getAnimationModeVariant`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationMode {
    pub slot: AnimModeSlot,
    pub variant: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SchedulerStage {
    pub kind: StageKind,

    pub raw_type: u8,

    /// `unkCombo & STAGE_LENGTH_MASK`, the dword count the stage spans. `StageKind::from_stage`
    /// is length-conditional for several opcodes, so a consumer that wants to re-classify a
    /// stage needs the same length the parser used.
    pub stage_words: u8,

    pub delay_frames: u16,

    pub duration_frames: u16,

    pub id: [u8; 4],

    // research/xim EffectRoutineParser.kt parseSection2 (opcode 0x05). Half-frame units (divide
    // by 2 for real frames). Zero when the stage is shorter than the motion payload.
    pub max_loops: u16,
    pub transition_in: u16,
    pub transition_out: u16,

    // `Some` exactly for the two model-transform kinds, whose payload occupies the dwords the
    // generic decoder reads `id` from; `id` is `NO_STAGE_ID` on those stages so a consumer can
    // never take rotation.x for a DatId.
    pub model_transform: Option<ModelTransform>,
    pub follow_points: Option<FollowPoints>,

    // `Some` exactly for `ScreenColorDrive`: research/XIClient HandleTag0x0F reads
    // destination{Red,Green,Blue,Alpha} out of the dword the generic decoder takes `id` from,
    // so `id` is `NO_STAGE_ID` there for the same reason as a model transform.
    pub screen_color: Option<ScreenColor>,

    // `Some` exactly for the two actor-fade kinds, whose +8 dword is an RGBA destination rather
    // than a DatId (research/xim EffectRoutineParser.kt parseSection2); `id` is `NO_STAGE_ID` there.
    pub actor_fade: Option<[u8; 4]>,

    // `Some` exactly for `TransitionToIdle`, whose +8 dword is an f32 transition time (research/
    // xim EffectRoutineParser.kt parseSection2); `id` is `NO_STAGE_ID` there.
    pub idle_transition_time: Option<f32>,

    // The stage's animationDuration. `Some` for the two flinch kinds when the stage carries
    // the full 9-dword payload (the f32 at +24, research/xim EffectRoutineParser.kt
    // parseFlinchEffect; retail plays the dfi?/dfm? flinch clip with transition in/out of
    // animationDuration/2 frames each, EffectRoutineInterpolatedEffects.kt
    // FlinchAnimationInstance), and for the knockback kind (the f32 at +12,
    // parseSection2 0x5E / 0xBF; the bf0? knock-down's length, KnockBackInstance).
    pub flinch_duration: Option<f32>,

    // `Some` exactly for `SetModelVisibility`, whose payload (hidden u32, slot u16, ifEngaged
    // u16) occupies the dwords the generic decoder reads `id` from (research/xim
    // EffectRoutineParser.kt parseSection2); `id` is `NO_STAGE_ID` there.
    pub model_visibility: Option<ModelVisibility>,

    /// `Some` for [`StageKind::ActorRotation`] stages that carry the full six-dword payload; the
    /// angles are degrees, as authored.
    pub actor_rotation: Option<ActorRotation>,

    /// `Some` for [`StageKind::TurnToward`] stages carrying their one payload dword: the turn's
    /// per-frame step in degrees, as authored. Retail reads that dword as a float (load at RVA 0x5B016,
    /// converted at RVA 0x5B019), so the slot is not a DatId (`FFXiMain.dll retail-2026-09`).
    pub turn_toward_step_degrees: Option<f32>,

    /// `Some` exactly for [`StageKind::AnimationMode`]: the +8 dword is the variant value, not a DatId.
    pub animation_mode: Option<AnimationMode>,

    // `Some` exactly for `SpellEffect`: the +8 dword is the spell animation index, not a
    // DatId (research/xim EffectRoutineParser.kt parseSection2 0x19); `id` is
    // `NO_STAGE_ID` there.
    pub spell_effect: Option<u32>,

    // `Some` for the sound-emitter kinds when the stage carries the full emitter payload:
    // the authored AudioRangeSetup `(far, near)` floats (research/xim EffectRoutineParser.kt
    // parseSoundEffectEmitter). A shipped 0.0 is not "silent" — Calc3D substitutes its class
    // defaults for it (CYySepRes.cpp CYySepRes::Calc3D).
    pub sound_range: Option<(f32, f32)>,

    // `Some` for CONTROL_FLOW_CONDITION stages (ROM/0/0.DAT dam0/daml/crtl switch tests); payload, not a DatId.
    pub control_flow: Option<ControlFlowArg>,

    // research/xim EffectRoutineParser.kt parseSection2,553-559 — stages between a 0x3D and its 0x3E
    // are children of one RandomChildRoutine, not siblings on the timeline: retail runs exactly
    // one of them per activation (`vatk`'s four atk1..atk4 grunts). Members of the same block
    // share a group index; `None` is an ordinary unconditional stage.
    pub random_group: Option<u16>,

    // research/xim EffectRoutineInstance.kt appendChildSequences findResource — a stage's ids resolve against
    // `resource.localDir`, the chunk directory the routine itself lives in, BEFORE any wider
    // scope. Retail relies on that: ROM/0/0.DAT holds four generators named `g010` in four
    // different directories, and only the one beside the routine that names it is meant. Carried
    // on the stage because a flatten merges many routines into one timeline. All-zero when the
    // routine was parsed without directory context.
    pub local_dir: [u8; 4],
}

// ROM/0/0.DAT dam0 switch-test word ops (LE u32s of the byte runs `1C 00 03 00` / `1C 00 01 00`).
pub const CF_FIELD_SELECTOR_OP: u32 = 0x0003_001C;
pub const CF_COMPARE_VALUE_OP: u32 = 0x0001_001C;
// .agents/skills/retail-observe/references/2026-10-04-crtl-condition-grammar.md Authored rule
pub const CF_MASK_TEST_OP: u32 = 0x11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlFlowArg {
    pub op: u32,
    pub operand: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageKind {
    Motion,

    // research/xim EffectRoutineParser.kt parseSection2 0x0C ModelTranslationRoutine / 0x0D
    // ModelRotationRoutine. Retail's swinging doors are these: `door/<BlockID>/open` rotates the
    // group's leaves to 80 degrees about Y, `clos` back to 0.
    ModelTranslation,
    ModelRotation,

    /// research/XIClient `Game::Scheduler::HandleTag0x0F` — drive the persistent
    /// full-screen colour linearly to [`SchedulerStage::screen_color`] over
    /// `duration_frames`, then latch there (`ScreenColorDriveTask`'s destructor
    /// snaps the field to the destination). The screen fade is one of these.
    ScreenColorDrive,

    SoundOnTarget,

    SoundOnCaster,

    /// 0x4A (`PlayerOnly`) / 0x60 (`Global`) — a sound emitter with no world
    /// position; it mixes dry at the listener rather than attenuating from an
    /// actor.
    SoundNonPositional,

    Particle,

    SubRoutine,

    // research/xim EffectRoutineParser.kt parseSection2 — LinkedEffectRoutine(useTarget = true): the
    // child sequence's source actor is the primary target, so it resolves its ids against the
    // TARGET's resource dirs (the victim's own hit grunt / flinch), not the caster's.
    SubRoutineOnTarget,

    BlockingSubRoutine,

    StopParticle,

    /// 0x1E - ParticleDampen: `id` names the generator; emission stops and the already-live
    /// particles are force-expired at once (research/xim EffectRoutineInstance.kt
    /// handleParticleEffectDampen: stopEmitting plus forceExpire, with the looping audio
    /// faded out).
    ParticleDampen,

    DamageCallback,

    FollowPoints,

    /// 0x07 / 0x59 - AnimationLock for `duration_frames` frames of the routine clock
    /// (SE: `BondageActor` /
    /// `LockCasterMagic`; xim treats both as AnimationLockEffect). Retail's ActionTimer1
    /// lock; refcounted across overlapping routines, and a routine without a lock stage does
    /// not lock.
    AnimationLock,

    /// 0x2E - MovementLock for `duration_frames` frames of the routine clock (research/xim
    /// EffectRoutineParser.kt parseSection2 MovementLockEffect): the actor's movement is
    /// withheld for the interval, the pose is untouched - facing is the separate 0x2F lock.
    MovementLock,

    /// 0x2F - HoldRotation (research/xim EffectRoutineParser.kt parseSection2 FacingLockEffect):
    /// for `duration_frames` retail stops copying the wire orientation onto the actor, so whatever a
    /// drive-task wrote stays. Retail's task takes one refcount on the actor's orientation
    /// (`FFXiMain.dll retail-2026-09`): handler RVA 0x5C8BA allocates it and its constructor at
    /// RVA 0x624B0 acquires through vtable
    /// slot byte `0x314` (RVA 0x62508), releasing in its destructor at RVA 0x6248B; the per-entity
    /// update tests that count at RVA 0x8FC08 and skips both the orientation copy and its angle wrap
    /// while it is non-zero. The hold length is the generic duration word, read by the shared stage
    /// reader (RVA 0x5E590), so the stage carries nothing past delay/duration.
    HoldRotation,

    /// 0x89 - LockLookAt: while it runs, the actor behaves as if it has no look-at target. The task
    /// sets bit 1 of `[actor+0x840]` when it is built and clears it when it ends (`FFXiMain.dll
    /// retail-2026-09` RVA 0x5F4A2..0x5F4AB / RVA 0x5F67E..0x5F68B), and the look-at owner tests that
    /// bit at RVA 0xD5B90..0xD5B9A, where a set bit throws away the target it just resolved. The task
    /// also ends itself once the actor stands `1.0f` yalm or more horizontally from where the stage
    /// fired (anchor written at RVA 0x5F4BC / RVA 0x5F4D2, compared at RVA 0x5F5AA / RVA 0x5F60F), so
    /// `duration_frames` is a bound rather than a promise.
    LockLookAt,

    /// 0xA9 / 0xAA - ActorRotation: drive one actor's three orientation angles from whatever they
    /// are when the stage fires to the authored absolute euler, over `duration_frames` (the task's
    /// tick interpolates between a start captured in its constructor and the authored target:
    /// FFXiMain.dll retail-2026-09 RVA 0x5FB67..0x5FBBa; progress is
    /// `1 - remaining / duration`, computed at RVA 0x5FCDC..0x5FCE5 against the countdown it keeps
    /// at task+0x74/+0x78). The payload is [`SchedulerStage::actor_rotation`]; axis identity of the
    /// three authored floats follows the record order, which is what retail feeds the lerp.
    ActorRotation,

    /// 0x62 - TurnToward: turn this actor's heading toward the routine target at the authored rate,
    /// for as long as the stage lasts. Retail does it in two halves, both fired by handler RVA 0x5AF2C.
    /// Once, on the spot: the angle between the direction to the target and the heading the actor holds
    /// right now is measured (positions through vtable slot byte `0x1BC`, current heading through slot
    /// byte `0x1C0` component 1) and stored as a remaining magnitude plus a signed per-frame step taken
    /// from this stage's float - magnitude via the setter at RVA 0x5E7C0 (call site RVA 0x5B08F), step
    /// through RVA 0x5E7D0 (RVA 0x5B0B6). Then per frame: while a turn is enabled, the actor's own update
    /// adds one signed step to its heading accumulator and shortens the magnitude by `|step|`, backing the
    /// heading off again by the leftover when a step overshoots so the net travel is exactly the angle it
    /// measured (`FFXiMain.dll retail-2026-09` RVA 0xC66CA..0xC67DA). Consumption needs four things true of
    /// the actor: no orientation refcount, `[actor+0x102] == 0`, `[actor+0x7A4] == -1` and a non-zero
    /// turn-enable nesting count `[actor+0x86C]`; this stage supplies that last one itself (bumped at RVA
    /// 0x5AF7D, released by its companion task's destructor at RVA 0x60F40), which is what bounds the turn
    /// to `duration_frames`. A fifth condition ends it early on any frame where the remaining magnitude is
    /// not strictly positive (RVA 0xC66CA..0xC66DD).
    TurnToward,

    /// 0x79 (battle) / 0x8C (idle) / 0xA4 (walking) / 0xA5 (running) — AdjustAnimationModeRoutine
    /// (`research/xim EffectRoutineParser.kt parseSection2`): from this stage on the actor resolves that
    /// slot's motion ids against a variant family and keeps the unmarked id as fallback, so `btl?` becomes
    /// e.g. `3tl?` with `btl?` still tried (`ActorModel.kt battleAnimationMode`, `Actor.kt
    /// getIdleAnimationId`). It is how an equipped weapon's motion group reaches the pose: the weapon-anchor
    /// bones are keyed only by that family's clips.
    AnimationMode,

    /// 0x75 - SetModelVisibility: show or hide one model slot of the actor for the stage's
    /// duration (research/xim EffectRoutineParser.kt parseSection2 SetModelVisibilityRoutine);
    /// the payload is [`SchedulerStage::model_visibility`].
    SetModelVisibility,

    /// 0x15 - ActorPositionSnapshot: argument-less. From this stage on, the routine's effect
    /// context freezes the actor's position and joints at their current values, so effects
    /// spawned later anchor where the routine fired even if the actor moves (research/xim
    /// EffectRoutineInstance.kt handleActorPositionSnapshot: applyPositionSnapshot plus
    /// applyJointSnapshot(true)).
    ActorPositionSnapshot,

    /// 0x22 - JointSnapshot: the u32 after delay/duration is consumed and unused in retail
    /// (research/xim EffectRoutineParser.kt parseSection2); the handler only sets the
    /// context's joint-snapshot flag (research/xim EffectRoutineInstance.kt
    /// handleJointSnapshotEffect: applyJointSnapshot(true)).
    JointSnapshot,

    /// 0x19 - SpellEffect: `spell_effect` is the spell animation index; the handler loads
    /// the effect DAT at the spell file-table offset plus the index and runs its `main`
    /// routine on the actor as a child sequence (research/xim EffectRoutineInstance.kt
    /// handleSpellEffect).
    SpellEffect,

    /// 0x01 - StartRoutineMarker: argument-less; retail's handler is a no-op
    /// (research/xim EffectRoutineInstance.kt handleEffect: StartRoutineMarker ->
    /// EffectResult.noop()), so the stage only marks the routine's start on the timeline.
    StartRoutineMarker,

    /// 0x5F - StopRoutine: stop the running routine named by `id` (research/xim
    /// EffectRoutineParser.kt parseSection2 StopRoutineEffect). The worm's `ini1` stops `init`
    /// and `init` stops `ini1` this way.
    StopRoutine,

    /// `FLINCH_CASTER_OPCODE` / `FLINCH_TARGET_OPCODE` - flinch (research/xim
    /// EffectRoutineParser.kt parseFlinchEffect); SE `GetDamageDirId` picks the dfi/dbi/dfm/dbm
    /// front/back clip by hit direction.
    FlinchOnCaster,
    FlinchOnTarget,

    /// 0x5E / 0xBF - knockback (research/xim EffectRoutineParser.kt parseSection2
    /// KnockBackRoutine).
    Knockback,

    /// 0x78 - DisplayDeadRoutine (research/xim EffectRoutineParser.kt parseSection2
    /// DisplayDeadRoutine): the actor is dead from this stage on.
    DisplayDead,

    /// 0x28 - TransitionToIdle (research/xim EffectRoutineParser.kt parseSection2
    /// TransitionToIdleEffect); `idle_transition_time` holds the payload's f32 transition time
    /// when present.
    TransitionToIdle,

    /// 0x29 (caster) / 0x2A (target) - ActorFade to `actor_fade` over `duration_frames`
    /// (SE `ActorColorDriveTask`; research/xim EffectRoutineParser.kt parseSection2
    /// ActorFadeRoutine). 0x80808080 is the neutral tint the worm's `init` uses.
    ActorFadeOnCaster,
    ActorFadeOnTarget,

    /// 0x04 - drive the camera along the kind 0x06 route named by `id` for the stage's scaled
    /// duration (research/XIClient Game/Scheduler/Tags/0x04.cpp HandleTag0x04 looks the
    /// resource up by the tag's four-char name and calls CameraResource::CreateCameraTask).
    CameraRoute,

    /// 0x1D - a lift platform's travel between the two floor heights its zone-DAT
    /// RID entry states, over `duration_frames`. The `@`-group routines
    /// research/xim/src/jsMain/kotlin/xim/poc/Actor.kt updateElevatorDisplay names
    /// (`mv01` up, `mv10` down, `mv00`/`mv11` settle) each carry exactly one;
    /// in the shipped zone DATs its length is the lift's travel time (480 frames
    /// for the Metalworks lifts, 720 for Pso'Xja's three tall shafts).
    ElevatorTravel,

    Unknown,
}

// research/xim EffectRoutineParser.kt parseSection numInputs,141-154 — opcode 0x0A is overloaded: a
// 32-byte stage (length_words 8, XIM numArgs 7) is a Source (caster) sound emitter,
// while any other length is a LinkedEffectRoutine sub-routine. Disambiguate by length.
const SOUND_EMITTER_LENGTH_WORDS: usize = 8;

// The opcodes `Scheduler::parse` acts on structurally — it opens/closes a random block, ends the
// section, or marks a control-flow branch the caller evaluates — rather than turning into a
// StageKind. Their `Unknown` kind is the parser's design, not an unhandled instruction, so a
// coverage census must not count them as a gap.
pub const fn is_structural_opcode(raw_type: u8) -> bool {
    is_control_flow_opcode(raw_type)
        || matches!(
            raw_type,
            END_ROUTINE_OPCODE | RANDOM_BLOCK_OPEN | RANDOM_BLOCK_CLOSE
        )
}

pub const fn is_control_flow_opcode(raw_type: u8) -> bool {
    matches!(
        raw_type,
        CONTROL_FLOW_BRANCH_TRUE
            | CONTROL_FLOW_BRANCH_FALSE
            | CONTROL_FLOW_BLOCK_OPEN
            | CONTROL_FLOW_BLOCK_CLOSE
            | CONTROL_FLOW_CONDITION
    )
}

// Opcodes whose stage carries only delay/duration — two dwords, no id/payload dword after them.
// research/xim EffectRoutineParser.kt parseSection2: each of these arms reads nothing past
// delay/duration, so the id-dword gate must not require the +8 slot for them. The id-carrying
// AnimationLock form is not listed: it reads a zero dword, so its stage is three dwords.
pub const fn argless_stage_opcode(raw_type: u8) -> bool {
    matches!(
        raw_type,
        START_ROUTINE_MARKER_OPCODE
            | ACTOR_POSITION_SNAPSHOT_OPCODE
            | MOVEMENT_LOCK_OPCODE
            | FACING_LOCK_OPCODE
            | TOGGLE_BROADCAST_ON_OPCODE
            | TOGGLE_BROADCAST_OFF_OPCODE
            | ANIMATION_LOCK_MAGIC_OPCODE
    )
}

/// The shortest stage `from_stage` can be asked about, and the longest the length field can
/// express: sweeping this range is how a consumer discovers the handled opcode set without
/// restating the match arms.
pub const STAGE_WORDS_RANGE: std::ops::RangeInclusive<usize> = 1..=STAGE_LENGTH_MASK as usize;

impl StageKind {
    pub fn from_stage(b: u8, length_words: usize) -> Self {
        match b {
            // Opcodes empirically confirmed against retail spell DATs (e.g. Cure = file 0xAF1):
            // 0x02 spawns a particle generator, 0x03 calls a sub-routine, 0x05 plays motion,
            // 0x0B/0x53 play sound on target/caster.
            0x02 => Self::Particle,
            0x03 => Self::SubRoutine,
            // research/XIClient Game/Scheduler/Tags/0x04.cpp HandleTag0x04 - the camera route
            // stage; `id` is the kind 0x06 chunk name in the same file.
            0x04 => Self::CameraRoute,
            0x05 => Self::Motion,
            // research/xim EffectRoutineParser.kt parseSection2.
            0x09 => Self::SubRoutineOnTarget,
            0x0A if length_words == SOUND_EMITTER_LENGTH_WORDS => Self::SoundOnCaster,
            0x0A => Self::SubRoutine,
            0x0B => Self::SoundOnTarget,
            0x0C if length_words * 4 >= MODEL_TRANSFORM_PAYLOAD_LEN => Self::ModelTranslation,
            0x0D if length_words * 4 >= MODEL_TRANSFORM_PAYLOAD_LEN => Self::ModelRotation,
            0x0F => Self::ScreenColorDrive,
            0x27 if length_words * 4 >= FOLLOW_POINTS_PAYLOAD_LEN => Self::FollowPoints,
            // research/xim EffectRoutineParser.kt parseSection2 — StopParticleGeneratorRoutine, id =
            // the generator DatId to stop (ROM/0/0.DAT `stbk` stops the cast aura's gn10..gn13).
            0x2D => Self::StopParticle,
            // research/xim EffectRoutineParser.kt parseSection2 - ParticleDampenRoutine:
            // genRef + zero32; the handler force-expires the generator's live particles as
            // well as stopping emission (EffectRoutineInstance.kt handleParticleEffectDampen).
            PARTICLE_DAMPEN_OPCODE => Self::ParticleDampen,
            // research/xim EffectRoutineParser.kt parseSection2 — DamageCallbackRoutine, the stage the
            // damage/battle-message callback is invoked on (EffectRoutineInstance.kt handleDamageCallbackRoutine).
            // Every spell routine tail-calls a `mdam` sub-routine that holds exactly this stage.
            0x2B => Self::DamageCallback,
            // research/xim EffectRoutineParser.kt parseSection2 - AnimationLockEffect; SE
            // `BondageActor` / `LockCasterMagic`, xim treats both as the same lock. Retail's
            // ActionTimer1 animation lock, refcounted across overlapping routines. The first
            // form carries a zero dword after delay/duration; the magic form is argument-less.
            ANIMATION_LOCK_OPCODE | ANIMATION_LOCK_MAGIC_OPCODE => Self::AnimationLock,
            // research/xim EffectRoutineParser.kt parseSection2 - MovementLockEffect, argument-less.
            MOVEMENT_LOCK_OPCODE => Self::MovementLock,
            // FFXiMain.dll retail-2026-09 dispatch case 45 (jump-table cell RVA 0x5DCD0) holds the
            // actor's orientation for its duration; see `StageKind::HoldRotation`.
            FACING_LOCK_OPCODE => Self::HoldRotation,
            TURN_TOWARD_OPCODE => Self::TurnToward,
            // research/xim EffectRoutineParser.kt parseSection2 - SetModelVisibilityRoutine:
            // hidden u32, slot u16, ifEngaged u16 after delay/duration; no DatId.
            SET_MODEL_VISIBILITY_OPCODE if length_words * 4 >= SET_MODEL_VISIBILITY_PAYLOAD_LEN => {
                Self::SetModelVisibility
            }
            // research/xim EffectRoutineParser.kt parseSection2 - ActorPositionSnapshotEffect,
            // argument-less.
            ACTOR_POSITION_SNAPSHOT_OPCODE => Self::ActorPositionSnapshot,
            // research/xim EffectRoutineParser.kt parseSection2 - JointSnapshotEffect: the +8
            // u32 is consumed and unused, so it is not a DatId.
            JOINT_SNAPSHOT_OPCODE => Self::JointSnapshot,
            // research/xim EffectRoutineParser.kt parseSection2 - SpellEffect: the +8 u32 is
            // the spell animation index, not a DatId.
            SPELL_EFFECT_OPCODE => Self::SpellEffect,
            // research/xim EffectRoutineParser.kt parseSection2 - StartRoutineMarker:
            // argument-less; retail's handler is a no-op.
            START_ROUTINE_MARKER_OPCODE => Self::StartRoutineMarker,
            // research/xim EffectRoutineParser.kt parseSection2 - FlinchRoutine (SE `GetDamageDirId`
            // picks the dfi/dbi/dfm/dbm front/back clip by hit direction).
            FLINCH_CASTER_OPCODE => Self::FlinchOnCaster,
            FLINCH_TARGET_OPCODE => Self::FlinchOnTarget,
            // research/xim EffectRoutineParser.kt parseSection2 - TransitionToIdleEffect; the f32 at
            // +8 is the transition time.
            TRANSITION_TO_IDLE_OPCODE => Self::TransitionToIdle,
            // research/xim EffectRoutineParser.kt parseSection2 - ActorFadeRoutine to the RGBA at +8
            // over `duration_frames` (SE `ActorColorDriveTask`).
            ACTOR_FADE_CASTER_OPCODE => Self::ActorFadeOnCaster,
            ACTOR_FADE_TARGET_OPCODE => Self::ActorFadeOnTarget,
            // research/xim EffectRoutineParser.kt parseSection2 - KnockBackEffect (SE tag table);
            // the alternate opcode dispatches the same payload.
            KNOCKBACK_OPCODE | KNOCKBACK_ALT_OPCODE => Self::Knockback,
            // research/xim EffectRoutineParser.kt parseSection2 - StopRoutineEffect: stop the running
            // routine named by `id`. The worm's `ini1` stops `init` and `init` stops `ini1`
            // this way.
            STOP_ROUTINE_OPCODE => Self::StopRoutine,
            // research/xim EffectRoutineParser.kt parseSection2 - DisplayDeadRoutine: the actor is
            // dead from this stage on.
            DISPLAY_DEAD_OPCODE => Self::DisplayDead,
            ELEVATOR_TRAVEL_OPCODE => Self::ElevatorTravel,
            // research/xim EffectRoutineParser.kt parseSection2 — LinkedEffectRoutine with
            // `blocking = true`: the same sub-routine call as 0x03, except the parent stalls
            // until the child finishes (EffectRoutineInstance.kt createChild `blockers += newSequences`).
            0x3B | 0x3C => Self::BlockingSubRoutine,
            0x53 => Self::SoundOnCaster,
            // research/xim EffectRoutineParser.kt parseSection2 (0x4A -> PlayerOnly) and :405
            // (0x60 -> Global): the same sound-emitter payload as 0x0A/0x0B, mixed
            // at the listener instead of at a world position. Both render dry from
            // one client's seat, so they share a kind; `raw_type` keeps them apart
            // for anyone who later needs the distinction. Without these arms both
            // fall to `Unknown` and never fire — eight effect DATs in 2800-3300 have
            // no other sound stage and are completely silent.
            PLAYER_ONLY_SOUND_OPCODE | 0x60 => Self::SoundNonPositional,
            // research/xim EffectRoutineParser.kt parseSection2 — a plain LinkedEffectRoutine, the
            // form every melee routine uses (`ati0` links the weapon's `skaz` whoosh, `atk0`
            // the race/face `vatk` grunt).
            0x57 => Self::SubRoutine,
            // research/xim EffectRoutineParser.kt parseSection2 - AdjustAnimationModeRoutine, one opcode
            // per slot; its +8 dword is the variant value, not a DatId.
            ADJUST_ANIM_MODE_BATTLE_OPCODE
            | ADJUST_ANIM_MODE_IDLE_OPCODE
            | ADJUST_ANIM_MODE_WALKING_OPCODE
            | ADJUST_ANIM_MODE_RUNNING_OPCODE => Self::AnimationMode,
            LOCK_LOOK_AT_OPCODE => Self::LockLookAt,
            ACTOR_ROTATION_OPCODE | ACTOR_ROTATION_ALT_OPCODE
                if length_words * 4 >= ACTOR_ROTATION_PAYLOAD_LEN =>
            {
                Self::ActorRotation
            }
            _ => Self::Unknown,
        }
    }

    pub fn is_model_transform(self) -> bool {
        matches!(self, Self::ModelTranslation | Self::ModelRotation)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Scheduler {
    pub name: [u8; 4],
    pub stages: Vec<TimedStage>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimedStage {
    pub frame: u32,
    pub stage: SchedulerStage,
}

/// The four AdjustAnimationModeRoutine opcodes, one per [`AnimModeSlot`] (research/xim
/// EffectRoutineParser.kt parseSection2).
pub const ADJUST_ANIM_MODE_BATTLE_OPCODE: u8 = 0x79;
pub const ADJUST_ANIM_MODE_IDLE_OPCODE: u8 = 0x8C;
pub const ADJUST_ANIM_MODE_WALKING_OPCODE: u8 = 0xA4;
pub const ADJUST_ANIM_MODE_RUNNING_OPCODE: u8 = 0xA5;

/// Which slot an animation-mode opcode switches, or `None` for any other opcode.
pub fn anim_mode_slot_of_opcode(raw_type: u8) -> Option<AnimModeSlot> {
    match raw_type {
        ADJUST_ANIM_MODE_BATTLE_OPCODE => Some(AnimModeSlot::Battle),
        ADJUST_ANIM_MODE_IDLE_OPCODE => Some(AnimModeSlot::Idle),
        ADJUST_ANIM_MODE_WALKING_OPCODE => Some(AnimModeSlot::Walking),
        ADJUST_ANIM_MODE_RUNNING_OPCODE => Some(AnimModeSlot::Running),
        _ => None,
    }
}

pub const NO_LOCAL_DIR: [u8; 4] = [0; 4];

// A stage built in code rather than decoded from a DAT spans no dwords at all, and a length the
// encoding cannot produce (`STAGE_LENGTH_MASK` counts the opcode's own dword) says so.
pub const SYNTHESIZED_STAGE_WORDS: u8 = 0;

impl Scheduler {
    pub fn parse(name: [u8; 4], body: &[u8]) -> Result<Self> {
        Self::parse_in_dir(NO_LOCAL_DIR, name, body)
    }

    pub fn parse_in_dir(local_dir: [u8; 4], name: [u8; 4], body: &[u8]) -> Result<Self> {
        if body.len() < SCHEDULER_HEADER_LEN {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: SCHEDULER_HEADER_LEN,
                available: body.len(),
            });
        }
        let mut stages = Vec::new();
        let mut cursor = effect_section_start(body);
        let mut running_frame: u32 = 0;
        let mut open_group: Option<u16> = None;
        let mut next_group: u16 = 0;

        while cursor + 4 <= body.len() {
            let raw_type = body[cursor];
            // research/xim EffectRoutineParser.kt parseSection — opcode(8), unkCombo(16), unk0(8); the
            // stage spans `(unkCombo & 0x1F)` dwords including the opcode dword itself.
            let length_words = (u16::from_le_bytes([body[cursor + 1], body[cursor + 2]])
                & STAGE_LENGTH_MASK) as usize;
            let stage_bytes = length_words.saturating_mul(4);
            if stage_bytes < 4 || cursor + stage_bytes > body.len() {
                break;
            }

            // research/xim EffectRoutineParser.kt parseSection2 — the closer is not a member of the
            // block it ends (`addEffectRoutine` is never called for it), so `open_group` must
            // already be cleared when the stage below is pushed.
            if raw_type == RANDOM_BLOCK_CLOSE {
                open_group = None;
            }

            if stage_bytes >= STAGE_HEADER_LEN {
                let read_u16 =
                    |off: usize| u16::from_le_bytes([body[cursor + off], body[cursor + off + 1]]);
                let read_u32 = |off: usize| {
                    u32::from_le_bytes([
                        body[cursor + off],
                        body[cursor + off + 1],
                        body[cursor + off + 2],
                        body[cursor + off + 3],
                    ])
                };
                // research/xim EffectRoutineParser.kt parseSection2 — ControlFlowBlock is constructed
                // with `delay = 0` whatever the bytes say.
                let delay = match raw_type {
                    CONTROL_FLOW_BLOCK_OPEN | CONTROL_FLOW_BLOCK_CLOSE => 0,
                    _ => read_u16(DELAY_OFFSET),
                };
                let duration = read_u16(DURATION_OFFSET);
                let has_id = stage_bytes >= STAGE_WITH_ID_LEN;
                // Argument-less opcodes are complete at two dwords; every other opcode needs the
                // +8 id/payload dword present to map, so a short stage of an id opcode stays
                // Unknown rather than misreading a neighbour's bytes as its id. The lift travel
                // stage is the one other id-less opcode with a meaning of its own: eight bytes,
                // delay and duration both the leg length.
                let kind = if has_id || argless_stage_opcode(raw_type) {
                    StageKind::from_stage(raw_type, length_words)
                } else if raw_type == ELEVATOR_TRAVEL_OPCODE {
                    StageKind::ElevatorTravel
                } else {
                    StageKind::Unknown
                };
                let model_transform = kind.is_model_transform().then(|| {
                    let component =
                        |i: usize| f32::from_bits(read_u32(MODEL_TRANSFORM_VECTOR_OFFSET + i * 4));
                    ModelTransform {
                        final_value: [component(0), component(1), component(2)],
                        subchunk: read_u32(MODEL_TRANSFORM_SUBCHUNK_OFFSET),
                    }
                });
                let payload = has_id.then(|| {
                    [
                        body[cursor + ID_OFFSET],
                        body[cursor + ID_OFFSET + 1],
                        body[cursor + ID_OFFSET + 2],
                        body[cursor + ID_OFFSET + 3],
                    ]
                });
                let screen_color = payload
                    .filter(|_| kind == StageKind::ScreenColorDrive)
                    .map(|rgba| ScreenColor { rgba });
                // research/xim EffectRoutineParser.kt parseSection2 - the +8 dword of these
                // kinds is a colour or a transition time, not a DatId.
                let actor_fade = payload.filter(|_| {
                    matches!(
                        kind,
                        StageKind::ActorFadeOnCaster | StageKind::ActorFadeOnTarget
                    )
                });
                let idle_transition_time = payload
                    .filter(|_| kind == StageKind::TransitionToIdle)
                    .map(f32::from_le_bytes);
                // research/xim EffectRoutineParser.kt parseSection2 0x75 - the payload is
                // hidden u32, slot u16, ifEngaged u16 straight after delay/duration.
                let model_visibility = (kind == StageKind::SetModelVisibility
                    && stage_bytes >= SET_MODEL_VISIBILITY_PAYLOAD_LEN)
                    .then(|| ModelVisibility {
                        hidden: read_u32(ID_OFFSET) == 1,
                        slot: read_u16(ID_OFFSET + 4),
                        if_engaged: read_u16(ID_OFFSET + 6) == 1,
                    });
                let actor_rotation = (kind == StageKind::ActorRotation
                    && stage_bytes >= ACTOR_ROTATION_PAYLOAD_LEN)
                    .then(|| ActorRotation {
                        angles_degrees: [
                            f32::from_bits(read_u32(ACTOR_ROTATION_ANGLES_OFFSET)),
                            f32::from_bits(read_u32(ACTOR_ROTATION_ANGLES_OFFSET + 4)),
                            f32::from_bits(read_u32(ACTOR_ROTATION_ANGLES_OFFSET + 8)),
                        ],
                        mode: body[cursor + ACTOR_ROTATION_MODE_OFFSET],
                    });
                // FFXiMain.dll retail-2026-09 handler RVA 0x5AF2C takes this kind's +8 dword as a float:
                // the turn's per-frame step in degrees, converted by `.rdata 0x32A9F4` before it is stored
                // on the actor. It is payload, not a DatId.
                let turn_toward_step_degrees = (kind == StageKind::TurnToward
                    && stage_bytes >= TURN_TOWARD_PAYLOAD_LEN)
                    .then(|| f32::from_bits(read_u32(TURN_TOWARD_STEP_OFFSET)));
                let animation_mode = anim_mode_slot_of_opcode(raw_type)
                    .filter(|_| kind == StageKind::AnimationMode)
                    .map(|slot| AnimationMode {
                        slot,
                        variant: read_u32(ID_OFFSET),
                    });
                // research/xim EffectRoutineParser.kt parseSection2 0x19 - the +8 dword is
                // the spell animation index, not a DatId.
                let spell_effect = payload
                    .filter(|_| kind == StageKind::SpellEffect)
                    .map(u32::from_le_bytes);
                // research/xim EffectRoutineParser.kt parseSoundEffectEmitter — id(+8),
                // zero32(+12), unused u32(+16), far f32(+20), near f32(+24). A short stage
                // ships no range; Calc3D substitutes the class defaults for a 0.0.
                let sound_range = (stage_bytes >= SOUND_EMITTER_PAYLOAD_LEN
                    && matches!(
                        kind,
                        StageKind::SoundOnCaster
                            | StageKind::SoundOnTarget
                            | StageKind::SoundNonPositional
                    ))
                .then(|| {
                    (
                        f32::from_bits(read_u32(SOUND_FAR_OFFSET)),
                        f32::from_bits(read_u32(SOUND_NEAR_OFFSET)),
                    )
                });
                // Switch-test words are payload, not a DatId.
                let control_flow =
                    (raw_type == CONTROL_FLOW_CONDITION && has_id).then(|| ControlFlowArg {
                        op: read_u32(ID_OFFSET),
                        operand: (stage_bytes >= CONTROL_FLOW_OPERAND_PAYLOAD_LEN)
                            .then(|| read_u32(CONTROL_FLOW_OPERAND_OFFSET)),
                    });
                let flinch_duration = match kind {
                    StageKind::FlinchOnCaster | StageKind::FlinchOnTarget
                        if stage_bytes >= FLINCH_PAYLOAD_LEN =>
                    {
                        Some(f32::from_bits(read_u32(FLINCH_ANIMATION_DURATION_OFFSET)))
                    }
                    StageKind::Knockback if stage_bytes >= KNOCKBACK_DURATION_PAYLOAD_LEN => Some(
                        f32::from_bits(read_u32(KNOCKBACK_ANIMATION_DURATION_OFFSET)),
                    ),
                    _ => None,
                };
                // Flinch and knockback payloads are floats/ints from +8 on (research/xim
                // EffectRoutineParser.kt parseSection2), so their id slot is not a DatId either.
                let non_id_payload = model_transform.is_some()
                    || screen_color.is_some()
                    || actor_fade.is_some()
                    || idle_transition_time.is_some()
                    || model_visibility.is_some()
                    || actor_rotation.is_some()
                    || turn_toward_step_degrees.is_some()
                    || animation_mode.is_some()
                    || control_flow.is_some()
                    || matches!(
                        kind,
                        StageKind::FlinchOnCaster
                            | StageKind::FlinchOnTarget
                            | StageKind::Knockback
                            | StageKind::JointSnapshot
                            | StageKind::SpellEffect
                    );
                let id = match payload {
                    Some(bytes) if !non_id_payload => bytes,
                    _ => NO_STAGE_ID,
                };
                let (max_loops, transition_in, transition_out) =
                    if kind == StageKind::Motion && stage_bytes >= MOTION_PAYLOAD_LEN {
                        (
                            read_u16(MOTION_MAX_LOOP_OFFSET),
                            read_u16(MOTION_TRANSITION_IN_OFFSET),
                            read_u16(MOTION_TRANSITION_OUT_OFFSET),
                        )
                    } else {
                        (0, 0, 0)
                    };
                // research/xim EffectRoutineInstance.kt runEffects: `storedFrames -=
                // head.delay` happens as each effect is popped and run, so a stage's
                // delay gates the stages AFTER it — never itself. Fire frame is the
                // sum of PRIOR delays (first stage always fires at 0: a lone Motion
                // with delay 152, e.g. the emote bow routine, plays immediately).
                stages.push(TimedStage {
                    frame: running_frame,
                    stage: SchedulerStage {
                        kind,
                        raw_type,
                        stage_words: length_words as u8,
                        delay_frames: delay,
                        duration_frames: duration,
                        id,
                        max_loops,
                        transition_in,
                        transition_out,
                        follow_points: (kind == StageKind::FollowPoints).then(|| FollowPoints {
                            flags: read_u32(FOLLOW_POINTS_FLAGS_OFFSET),
                            easing: read_u32(FOLLOW_POINTS_FLAGS_OFFSET + 4),
                            rotation: f32::from_bits(read_u32(FOLLOW_POINTS_ROTATION_OFFSET)),
                        }),
                        model_transform,
                        screen_color,
                        actor_fade,
                        idle_transition_time,
                        flinch_duration,
                        model_visibility,
                        actor_rotation,
                        turn_toward_step_degrees,
                        animation_mode,
                        spell_effect,
                        sound_range,
                        control_flow,
                        random_group: open_group,
                        local_dir,
                    },
                });
                // A random block's children are collected into the 0x3D marker rather than
                // appended to the parent timeline (EffectRoutineParser.kt addEffectRoutine), so only
                // the marker's own delay advances the parent clock.
                if open_group.is_none() {
                    running_frame = running_frame.saturating_add(delay as u32);
                }
            }
            if raw_type == RANDOM_BLOCK_OPEN {
                open_group = Some(next_group);
                next_group = next_group.saturating_add(1);
            }
            cursor += stage_bytes;
            // EffectRoutineParser.kt parseSection — opcode 0x00 ends the section; section 3 follows it in
            // the same chunk and would otherwise be misread as more effect stages.
            if raw_type == END_ROUTINE_OPCODE {
                break;
            }
        }
        Ok(Self { name, stages })
    }

    /// The frame at which this routine's effects end: the max over all stages of
    /// `stage.frame + stage.duration_frames`, a half-open bound. A plain stage ends on its own
    /// fire frame; an AnimationLock keeps holding until `frame + duration_frames`.
    pub fn end_frame(&self) -> u32 {
        Self::end_frame_for(&self.stages)
    }

    /// The same bound over an arbitrary stage list, for hosts that flatten sub-routine calls
    /// into one timeline before measuring it (kuluu-render's ActiveScheduler).
    pub fn end_frame_for(stages: &[TimedStage]) -> u32 {
        stages
            .iter()
            .map(|t| t.frame + t.stage.duration_frames as u32)
            .max()
            .unwrap_or(0)
    }

    // A routine built out of these is a switch (`daml` picks one hit reaction, `dam0` one
    // additional effect), so inlining it whole would run every branch at once. We do not
    // evaluate the conditions; callers pick the branch.
    pub fn has_control_flow(&self) -> bool {
        self.stages
            .iter()
            .any(|t| is_control_flow_opcode(t.stage.raw_type))
    }

    pub fn sound_events(&self) -> impl Iterator<Item = SoundEvent> + '_ {
        self.stages.iter().filter_map(|t| match t.stage.kind {
            StageKind::SoundOnCaster => Some(SoundEvent {
                frame: t.frame,
                id: t.stage.id,
                on_caster: true,
            }),
            StageKind::SoundOnTarget => Some(SoundEvent {
                frame: t.frame,
                id: t.stage.id,
                on_caster: false,
            }),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoundEvent {
    pub frame: u32,
    pub id: [u8; 4],
    pub on_caster: bool,
}

/// The entrance/instance zone pairs whose MAPSCHEDULOR scene keys resolve in
/// the partner zone's model DAT rather than their own: (242, 170) Heavens'
/// Tower -> Full Moon Fountain, (194, 192) Outer Horutoto Ruins -> Inner
/// Horutoto Ruins, (31, 34) Monarch's Linn -> Grand Palace of Hu'Xzoi,
/// (32, 11) Sealion's Den -> Oldton Movalpolos, (32, 8) Sealion's Den ->
/// Boneyard Gully. Hand-built; retail's loader rule for the partner fallback is
/// unknown, and these five are the observed clean instance/entrance cases.
const ZONE_SCENE_PARTNERS: [(u16, u16); 5] = [(242, 170), (194, 192), (31, 34), (32, 11), (32, 8)];

/// The handful of non-model files that carry MAPSCHEDULOR scene keys no
/// per-zone slot owns: the Spire of Holla/Dem/Mea, Sealion's Den and Al'Taieu
/// scene families (`sc11..sc41` / `kc51..kc54` / `kci1..kci4`), file ids 641
/// (ROM/3/48.DAT), 30705 (ROM/123/85.DAT), 57075 (ROM/213/92.DAT), 57082
/// (ROM/216/12.DAT), 57204 (ROM/241/3.DAT). Hand-built; retail's loader rule
/// for these is unknown. `zz-walk-errors` re-checks that all five walk clean.
pub const NON_MODEL_SCENE_CARRIERS: [u32; 5] = [641, 30705, 57075, 57082, 57204];

/// Resolve a MAPSCHEDULOR key (the `ffxi_event::vm` scene opcode) to the DAT file
/// that carries its routine, following retail's per-zone rule: the routine lives in the
/// CURRENT zone's own model DAT (already loaded for rendering via
/// [`zone_dat::zone_id_to_mzb_file_id`]); on a miss, the entrance/instance partner
/// zone's model DAT; on a further miss, the non-model scene carriers. Returns the
/// file id, or `None` when no candidate file carries the key. The host arms the
/// WAITMAPSCHEDULOR hold from the file the key resolved in; the renderer plays
/// it from the same file.
///
/// Memoized per process: the result is a pure function of the install's DATs,
/// while deriving it costs up to eight full DAT reads plus parses per call (the
/// zone's model DAT is a large MZB file) on every MAPSCHEDULOR cue. One install
/// per process (the renderer and the session are separate processes, each with
/// its own memo), so the memo keys on (zone, key) alone. An overlay swap changes
/// which file a resolve reads, so [`DatRoot::set_overlays`] clears it.
pub fn zone_scene_file_id(root: &DatRoot, zone: u16, key: [u8; 4]) -> Option<u32> {
    let cache = ZONE_SCENE_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(&hit) = cache.get(&(zone, key)) {
        return hit;
    }
    drop(cache);
    let resolved = zone_scene_file_id_uncached(root, zone, key);
    #[cfg(test)]
    {
        *ZONE_SCENE_RESOLVE_COUNTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry((zone, key))
            .or_insert(0) += 1;
    }
    ZONE_SCENE_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert((zone, key), resolved);
    resolved
}

/// Process-local memo for [`zone_scene_file_id`]; see its doc for the keying
/// and invalidation rules.
type ZoneSceneMemo = std::collections::HashMap<(u16, [u8; 4]), Option<u32>>;
static ZONE_SCENE_CACHE: std::sync::LazyLock<std::sync::Mutex<ZoneSceneMemo>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(ZoneSceneMemo::new()));

/// Drop the [`zone_scene_file_id`] memo: called from `DatRoot::set_overlays`,
/// because the swap changes which file a later resolve reads. A lookup racing
/// the swap may re-memoize a pre-swap result until the next swap; the memo
/// holds only MAPSCHEDULOR answers, so the exposure is one stale zone-scene file id.
pub(crate) fn clear_zone_scene_cache() {
    ZONE_SCENE_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

/// Per-(zone, key) count of full resolves, test-only: a monotonic observation
/// point the memoization test reads, immune to parallel memo clears.
#[cfg(test)]
type ZoneSceneResolveCounts = std::collections::HashMap<(u16, [u8; 4]), u64>;
#[cfg(test)]
static ZONE_SCENE_RESOLVE_COUNTS: std::sync::LazyLock<std::sync::Mutex<ZoneSceneResolveCounts>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(ZoneSceneResolveCounts::new()));

#[cfg(test)]
fn zone_scene_resolve_count(zone: u16, key: [u8; 4]) -> u64 {
    ZONE_SCENE_RESOLVE_COUNTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&(zone, key))
        .copied()
        .unwrap_or(0)
}

fn zone_scene_file_id_uncached(root: &DatRoot, zone: u16, key: [u8; 4]) -> Option<u32> {
    if let Some(file) = crate::zone_dat::zone_id_to_mzb_file_id(zone) {
        if scheduler_key_in_file(root, file, key) {
            return Some(file);
        }
    }
    for &(scene_zone, partner) in &ZONE_SCENE_PARTNERS {
        if scene_zone != zone {
            continue;
        }
        if let Some(file) = crate::zone_dat::zone_id_to_mzb_file_id(partner) {
            if scheduler_key_in_file(root, file, key) {
                return Some(file);
            }
        }
    }
    NON_MODEL_SCENE_CARRIERS
        .iter()
        .copied()
        .find(|&file| scheduler_key_in_file(root, file, key))
}

/// True when `key` names a cleanly-parsed scheduler chunk in DAT file `file_id` —
/// the same set the renderer's action cache can play (a broken chunk resolves to
/// nothing, so a miss here is a miss there too).
fn scheduler_key_in_file(root: &DatRoot, file_id: u32, key: [u8; 4]) -> bool {
    let loc = match root.resolve(file_id) {
        Ok(loc) => loc,
        Err(_) => return false,
    };
    let bytes = match std::fs::read(loc.path_under(root)) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    crate::resource_dir::ResourceDir::from_bytes(bytes)
        .collect_schedulers()
        .iter()
        .any(|s| s.name == key)
}

// The camera route names in the title-screen scene DAT (ROM/0/23.DAT, magic `titl`)
// run two lowercase hex digits plus a two-digit decimal index
// (research/cexi-docs/dats/ROM_0_23.md node naming conventions).
const ROUTE_NAME_HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
/// Two hex digits of the zone id fit this bound.
const ROUTE_NAME_ZONE_MAX: u16 = 0xFF;
/// A two-digit decimal index fits this bound.
const ROUTE_NAME_INDEX_MAX: u8 = 99;
/// The zone id's upper or lower hex digit.
const ROUTE_NAME_NIBBLE: u16 = 0xF;

/// The zone-coded camera route name in the title-screen scene DAT (ROM/0/23.DAT):
/// `zone_id` as two lowercase hex digits followed by a two-digit decimal `index`
/// (research/cexi-docs/dats/ROM_0_23.md).
pub fn zone_camera_route_name(zone_id: u16, index: u8) -> [u8; 4] {
    debug_assert!(zone_id <= ROUTE_NAME_ZONE_MAX && index <= ROUTE_NAME_INDEX_MAX);
    let mut name = [0u8; 4];
    name[0] = ROUTE_NAME_HEX_DIGITS[(zone_id >> 4 & ROUTE_NAME_NIBBLE) as usize];
    name[1] = ROUTE_NAME_HEX_DIGITS[(zone_id & ROUTE_NAME_NIBBLE) as usize];
    name[2] = b'0' + index / 10;
    name[3] = b'0' + index % 10;
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_payload_recovers_loop_and_transition() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        // 40-byte (10-word) motion opcode: header, delay, duration, id, then the 0x05 tail.
        body.extend_from_slice(&[0x05, 0x0A, 0, 0]); // opcode, length=10 words
        body.extend_from_slice(&0u16.to_le_bytes()); // +4 delay
        body.extend_from_slice(&64u16.to_le_bytes()); // +6 duration
        body.extend_from_slice(b"mae0"); // +8 id
        body.extend_from_slice(&0u32.to_le_bytes()); // +12 zero32
        body.extend_from_slice(&1.0f32.to_le_bytes()); // +16 float
        body.extend_from_slice(&1.0f32.to_le_bytes()); // +20 float
        body.extend_from_slice(&8u16.to_le_bytes()); // +24 transitionIn
        body.extend_from_slice(&0u16.to_le_bytes()); // +26 zero
        body.extend_from_slice(&12u16.to_le_bytes()); // +28 transitionOut
        body.extend_from_slice(&3u16.to_le_bytes()); // +30 maxLoop
        body.extend_from_slice(&0u32.to_le_bytes()); // +32 unk0
        body.extend_from_slice(&0u32.to_le_bytes()); // +36 unk1

        let s = Scheduler::parse(*b"mae0", &body).unwrap();
        assert_eq!(s.stages.len(), 1);
        let st = s.stages[0].stage;
        assert_eq!(st.kind, StageKind::Motion);
        assert_eq!(&st.id, b"mae0");
        assert_eq!(st.transition_in, 8);
        assert_eq!(st.transition_out, 12);
        assert_eq!(st.max_loops, 3);
    }

    #[test]
    fn short_motion_stage_has_zero_tail() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend_from_slice(&[0x05, 0x03, 0, 0]);
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&20u16.to_le_bytes());
        body.extend_from_slice(b"mot0");
        let s = Scheduler::parse(*b"sdam", &body).unwrap();
        let st = s.stages[0].stage;
        assert_eq!(st.max_loops, 0);
        assert_eq!(st.transition_in, 0);
        assert_eq!(st.transition_out, 0);
    }

    // research/xim EffectRoutineParser.kt parseFlinchEffect: the flinch payload is
    // f32, f32, u32, f32, f32 animationDuration, u32, u32 after delay/duration - a 9-dword
    // stage. The bytes mirror Rarab's `damg` flinch (ROM/4/109.DAT): delay 2, duration 10.0.
    #[test]
    fn flinch_stage_captures_animation_duration_at_offset_24() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        for op in [FLINCH_CASTER_OPCODE, FLINCH_TARGET_OPCODE] {
            body.extend_from_slice(&[op, FLINCH_STAGE_WORDS, 0, 0]);
            body.extend_from_slice(&2u16.to_le_bytes());
            body.extend_from_slice(&0u16.to_le_bytes());
            body.extend_from_slice(&1.0f32.to_le_bytes());
            body.extend_from_slice(&1.0f32.to_le_bytes());
            body.extend_from_slice(&2u32.to_le_bytes());
            body.extend_from_slice(&1.0f32.to_le_bytes());
            body.extend_from_slice(&10.0f32.to_le_bytes());
            body.extend_from_slice(&0u32.to_le_bytes());
            body.extend_from_slice(&0u32.to_le_bytes());
        }

        let s = Scheduler::parse(*b"damg", &body).unwrap();
        assert_eq!(s.stages.len(), 2);
        for (i, want_kind) in [StageKind::FlinchOnCaster, StageKind::FlinchOnTarget]
            .into_iter()
            .enumerate()
        {
            let st = s.stages[i].stage;
            assert_eq!(st.kind, want_kind, "opcode of stage {i}");
            assert_eq!(st.flinch_duration, Some(10.0), "animationDuration at +24");
            assert_eq!(&st.id, &[0; 4]);
        }
    }

    /// A flinch stage shorter than the full payload carries no animationDuration: the
    /// consumer falls back to its default transitions rather than reading past the stage.
    #[test]
    fn short_flinch_stage_has_no_animation_duration() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend_from_slice(&[FLINCH_CASTER_OPCODE, ARGLESS_STAGE_WORDS + 1, 0, 0]);
        body.extend_from_slice(&2u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&[0u8; 4]);

        let s = Scheduler::parse(*b"damg", &body).unwrap();
        assert_eq!(s.stages[0].stage.kind, StageKind::FlinchOnCaster);
        assert_eq!(s.stages[0].stage.flinch_duration, None);
    }

    #[test]
    fn parses_motion_then_sound_caster() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];

        body.extend_from_slice(&[0x05, 0x03, 0, 0]);
        body.extend_from_slice(&30u16.to_le_bytes());
        body.extend_from_slice(&20u16.to_le_bytes());
        body.extend_from_slice(b"mot0");

        body.extend_from_slice(&[0x53, 0x03, 0, 0]);
        body.extend_from_slice(&15u16.to_le_bytes());
        body.extend_from_slice(&1u16.to_le_bytes());
        body.extend_from_slice(b"snd0");

        let s = Scheduler::parse(*b"sdam", &body).unwrap();
        assert_eq!(s.stages.len(), 2);
        assert_eq!(s.stages[0].stage.kind, StageKind::Motion);
        assert_eq!(
            s.stages[0].frame, 0,
            "first stage fires at 0 despite its own delay"
        );
        assert_eq!(s.stages[0].stage.delay_frames, 30);
        assert_eq!(s.stages[1].stage.kind, StageKind::SoundOnCaster);
        assert_eq!(
            s.stages[1].frame, 30,
            "second stage fires after the first stage's delay"
        );
        assert_eq!(&s.stages[1].stage.id, b"snd0");

        let snd: Vec<_> = s.sound_events().collect();
        assert_eq!(snd.len(), 1);
        assert!(snd[0].on_caster);
        assert_eq!(snd[0].frame, 30);
    }

    // 0x4A and 0x60 carry the same emitter payload as 0x0A/0x0B but mix dry. They
    // used to fall to `Unknown` and never fire, leaving eight effect DATs in
    // 2800-3300 silent because they carry no other sound stage.
    #[test]
    fn opcodes_4a_and_60_are_non_positional_sounds() {
        for op in [0x4Au8, 0x60] {
            let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
            body.extend_from_slice(&[op, 0x08, 0, 0]);
            body.extend_from_slice(&0u16.to_le_bytes());
            body.extend_from_slice(&0u16.to_le_bytes());
            body.extend_from_slice(b"4063");
            body.extend(std::iter::repeat_n(0u8, 20));

            let s = Scheduler::parse(*b"main", &body).unwrap();
            assert_eq!(
                s.stages[0].stage.kind,
                StageKind::SoundNonPositional,
                "opcode {op:#04X}"
            );
            assert_eq!(s.stages[0].stage.raw_type, op, "raw opcode is preserved");
            assert_eq!(&s.stages[0].stage.id, b"4063");
        }
    }

    #[test]
    fn opcode_1d_is_the_lift_travel_stage() {
        const TRAVEL_FRAMES: u16 = 480;
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend_from_slice(&[ELEVATOR_TRAVEL_OPCODE, 0x02, 0, 0]);
        body.extend_from_slice(&TRAVEL_FRAMES.to_le_bytes());
        body.extend_from_slice(&TRAVEL_FRAMES.to_le_bytes());
        let s = Scheduler::parse(*b"mv01", &body).unwrap();
        assert_eq!(s.stages[0].stage.kind, StageKind::ElevatorTravel);
        assert_eq!(s.stages[0].stage.duration_frames, TRAVEL_FRAMES);
    }

    // Boost's effect DAT (ROM/16/0.DAT) plays its caster sound via opcode 0x0A with
    // length_words 8 (32-byte stage); a 0x0A of any other length is a sub-routine link.
    #[test]
    fn opcode_0a_len8_is_sound_else_subroutine() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        // 0x0A, length 8 words (32 bytes): a caster sound emitter.
        body.extend_from_slice(&[0x0A, 0x08, 0, 0]);
        body.extend_from_slice(&0u16.to_le_bytes()); // +4 delay
        body.extend_from_slice(&0u16.to_le_bytes()); // +6 duration
        body.extend_from_slice(b"7047"); // +8 id -> se_id 7047
        body.extend(std::iter::repeat_n(0u8, 20)); // pad to 32 bytes
                                                   // 0x0A, length 3 words (12 bytes): a sub-routine link, not a sound.
        body.extend_from_slice(&[0x0A, 0x03, 0, 0]);
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(b"sub0");

        let s = Scheduler::parse(*b"main", &body).unwrap();
        assert_eq!(s.stages[0].stage.kind, StageKind::SoundOnCaster);
        assert_eq!(&s.stages[0].stage.id, b"7047");
        assert_eq!(s.stages[1].stage.kind, StageKind::SubRoutine);
    }

    /// A lone Motion stage with a large delay (the emote-DAT shape, e.g. HumeM
    /// bow = `Motion delay=152`) fires at frame 0 — the delay only pads the
    /// routine tail (research/xim EffectRoutineInstance.kt runEffects).
    #[test]
    fn lone_delayed_motion_fires_immediately() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend_from_slice(&[0x05, 0x03, 0, 0]);
        body.extend_from_slice(&152u16.to_le_bytes());
        body.extend_from_slice(&152u16.to_le_bytes());
        body.extend_from_slice(b"bow?");
        let s = Scheduler::parse(*b"em00", &body).unwrap();
        assert_eq!(s.stages[0].frame, 0);
        assert_eq!(s.stages[0].stage.duration_frames, 152);
    }

    #[test]
    fn opcode_3c_is_blocking_subroutine_and_2d_is_stop_particle() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend_from_slice(&[0x3C, 0x04, 0, 0]);
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(b"shbk");
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&[0x3B, 0x04, 0, 0]);
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(b"wash");
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&[0x2D, 0x04, 0, 0]);
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(b"gn13");
        body.extend_from_slice(&0u32.to_le_bytes());

        let s = Scheduler::parse(*b"main", &body).unwrap();
        assert_eq!(s.stages[0].stage.kind, StageKind::BlockingSubRoutine);
        assert_eq!(&s.stages[0].stage.id, b"shbk");
        assert_eq!(s.stages[1].stage.kind, StageKind::BlockingSubRoutine);
        assert_eq!(&s.stages[1].stage.id, b"wash");
        assert_eq!(s.stages[2].stage.kind, StageKind::StopParticle);
        assert_eq!(&s.stages[2].stage.id, b"gn13");
    }

    // Retail-byte guard (skips without an install): Poison's effect DAT links the caster's
    // cast-complete routine with 0x3C, and the global system-effect dir stops the cast aura's
    // four generators with 0x2D. Both were dropped as Unknown before kuluu-ky8c.
    #[test]
    fn real_dat_spell_main_links_caster_finish_routine() {
        const POISON_FILE: u32 = 3020;
        const GLOBAL_EFFECT_DIR_FILE: u32 = 0;
        const BLOCKING_LINK_OPCODE: u8 = 0x3C;

        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let read = |id: u32| -> Option<Vec<u8>> {
            let loc = root.resolve(id).ok()?;
            std::fs::read(loc.path_under(&root)).ok()
        };
        let Some(poison) = read(POISON_FILE) else {
            return;
        };
        let scheds = |bytes: &[u8]| -> Vec<Scheduler> {
            crate::resource_dir::ResourceDir::from_bytes(bytes.to_vec()).collect_schedulers()
        };
        let main = scheds(&poison)
            .into_iter()
            .find(|s| &s.name == b"main")
            .expect("poison DAT has a main routine");
        let link = main
            .stages
            .iter()
            .find(|t| t.stage.raw_type == BLOCKING_LINK_OPCODE)
            .expect("main links a caster routine with 0x3C");
        assert_eq!(link.stage.kind, StageKind::BlockingSubRoutine);
        assert_eq!(&link.stage.id, b"shbk");

        let Some(global) = read(GLOBAL_EFFECT_DIR_FILE) else {
            return;
        };
        let stbk = scheds(&global)
            .into_iter()
            .find(|s| &s.name == b"stbk")
            .expect("global effect dir has the stbk stop routine");
        let stopped: Vec<[u8; 4]> = stbk
            .stages
            .iter()
            .filter(|t| t.stage.kind == StageKind::StopParticle)
            .map(|t| t.stage.id)
            .collect();
        for gen_id in [b"gn10", b"gn11", b"gn12", b"gn13"] {
            assert!(
                stopped.contains(gen_id),
                "stbk stops {}",
                String::from_utf8_lossy(gen_id)
            );
        }
    }

    // research/xim EffectRoutineParser.kt parseSection2 (0x2B DamageCallbackRoutine) and :270-274
    // (0x3B/0x3C LinkedEffectRoutine with blocking = true, unlike the 0x03 link).
    #[test]
    fn damage_callback_and_blocking_subroutine_opcodes() {
        assert_eq!(StageKind::from_stage(0x2B, 3), StageKind::DamageCallback);
        assert_eq!(
            StageKind::from_stage(0x3C, 4),
            StageKind::BlockingSubRoutine
        );
        assert_eq!(
            StageKind::from_stage(0x3B, 4),
            StageKind::BlockingSubRoutine
        );
        assert_eq!(StageKind::from_stage(0x03, 3), StageKind::SubRoutine);
        assert_ne!(
            StageKind::from_stage(0x3C, 4),
            StageKind::from_stage(0x03, 3)
        );
    }

    // Retail-byte guard (skips without an install): the global effect dir's `mdam` routine —
    // the sub-routine every spell's target routine tail-calls — IS the damage callback, a
    // single 0x2B stage. It decoded as Unknown before kuluu-k6tz.
    #[test]
    fn real_dat_global_mdam_routine_is_a_damage_callback() {
        const GLOBAL_EFFECT_DIR_FILE: u32 = 0;

        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let Ok(loc) = root.resolve(GLOBAL_EFFECT_DIR_FILE) else {
            return;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            return;
        };
        let mdam = crate::resource_dir::ResourceDir::from_bytes(bytes)
            .collect_schedulers()
            .into_iter()
            .find(|s| &s.name == b"mdam")
            .expect("global effect dir has the mdam routine");
        assert!(
            mdam.stages
                .iter()
                .any(|t| t.stage.kind == StageKind::DamageCallback),
            "mdam holds the 0x2B damage callback"
        );
    }

    const MODEL_TRANSFORM_WORDS: u8 = (MODEL_TRANSFORM_PAYLOAD_LEN / 4) as u8;

    fn model_transform_stage_bytes(
        opcode: u8,
        delay: u16,
        duration: u16,
        value: [f32; 3],
        subchunk: u32,
    ) -> Vec<u8> {
        let mut b = timed_stage_bytes(opcode, MODEL_TRANSFORM_WORDS, delay, duration);
        for v in value {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&subchunk.to_le_bytes());
        assert_eq!(b.len(), MODEL_TRANSFORM_PAYLOAD_LEN);
        b
    }

    // Layout pin, decoded by hand out of ROM zone DAT 330 `t_sa/door/_6ey/open`: the six-dword
    // stage puts delay at +4, duration at +6, three floats at +8/+12/+16 and the subchunk slot at
    // +20. The dwords at +8..+20 are exactly what the generic decoder would have handed back as a
    // DatId, so `id` must come back blank.
    #[test]
    fn model_transform_payload_is_a_vector_at_8_and_a_subchunk_at_20() {
        const SWING: f32 = 1.3962256;
        const DURATION: u16 = 70;
        for (opcode, kind) in [
            (0x0Cu8, StageKind::ModelTranslation),
            (0x0D, StageKind::ModelRotation),
        ] {
            let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
            body.extend(model_transform_stage_bytes(
                opcode,
                0,
                DURATION,
                [0.0, SWING, 0.0],
                1,
            ));
            assert_eq!(
                &body[SCHEDULER_HEADER_LEN + MODEL_TRANSFORM_VECTOR_OFFSET + 4
                    ..SCHEDULER_HEADER_LEN + MODEL_TRANSFORM_VECTOR_OFFSET + 8],
                &SWING.to_le_bytes(),
                "the y component sits at +12"
            );

            let s = Scheduler::parse(*b"open", &body).unwrap();
            let st = s.stages[0].stage;
            assert_eq!(st.kind, kind);
            assert_eq!(st.raw_type, opcode);
            assert_eq!(st.duration_frames, DURATION);
            assert_eq!(
                st.model_transform,
                Some(ModelTransform {
                    final_value: [0.0, SWING, 0.0],
                    subchunk: 1,
                })
            );
            assert_eq!(
                st.id, NO_STAGE_ID,
                "the transform payload must never surface as a DatId"
            );
        }
    }

    // A transform stage shorter than the payload cannot be decoded, and half a vector is worse
    // than none: it stays Unknown rather than claiming a garbage transform.
    #[test]
    fn model_transform_opcode_shorter_than_the_payload_stays_unknown() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(0x0D, 0x03, 0, 0));
        body.extend_from_slice(&0u32.to_le_bytes());
        let s = Scheduler::parse(*b"open", &body).unwrap();
        assert_eq!(s.stages[0].stage.kind, StageKind::Unknown);
        assert_eq!(s.stages[0].stage.model_transform, None);
    }

    // The two-leaf door shape, and the answer to whether the halves swing together: in
    // research/xim EffectRoutineInstance runEffects the `while (storedFrames >= 0f)` test is made
    // BEFORE `storedFrames -= head.delay`, so a stage always runs in the iteration that charges
    // its own delay. Leaf 1's delay of a whole swing gates only what comes after it — both leaves
    // start at frame 0 and swing together.
    #[test]
    fn both_door_leaves_start_on_the_same_frame() {
        const LEAF_FRAMES: u16 = 70;
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(model_transform_stage_bytes(
            0x0D,
            0,
            LEAF_FRAMES,
            [0.0, 1.0, 0.0],
            0,
        ));
        body.extend(model_transform_stage_bytes(
            0x0D,
            LEAF_FRAMES,
            LEAF_FRAMES,
            [0.0, 1.0, 0.0],
            1,
        ));
        body.extend(timed_stage_bytes(0x53, 0x03, 0, 0));
        body.extend_from_slice(b"snd0");

        let s = Scheduler::parse(*b"open", &body).unwrap();
        assert_eq!(s.stages[0].frame, 0);
        assert_eq!(s.stages[1].frame, 0);
        assert_eq!(
            s.stages[2].frame,
            u32::from(LEAF_FRAMES),
            "leaf 1's delay holds back the stage after it, not leaf 1 itself"
        );
    }

    // vendor/server/src/map/zone.h ZONE_SOUTHERN_SANDORIA.
    const SOUTHERN_SANDORIA: u16 = 230;
    // The BlockID of the two `door03` placements outside the S. San d'Oria stables, and the name
    // of the routine directory that swings them.
    const STABLE_DOOR_GROUP: [u8; 4] = *b"_6ey";

    fn zone_schedulers(zone_id: u16) -> Option<Vec<Scheduler>> {
        schedulers_in_file(crate::zone_dat::zone_id_to_mzb_file_id(zone_id)?)
    }

    // Retail-byte guard (skips without an install). Both leaves of the S. San d'Oria stable door
    // rotate about Y to the same angle and back to zero, addressed to subchunk 0 and 1 of the
    // `_6ey` placement group. Before the 0x0D arm existed this decoded as an Unknown stage whose
    // `id` was the four zero bytes of rotation.x.
    #[test]
    fn real_dat_ssandy_stable_door_rotates_two_subchunks() {
        // The DAT stores 1.3962256 rad; retail authored the round degree figure.
        const SWING_DEGREES: f32 = 80.0;
        // f32 radians round-tripped through degrees land ~0.003 deg off the authored value.
        const DEGREE_TOLERANCE: f32 = 0.01;
        const SWING_FRAMES: u16 = 70;

        let Some(scheds) = zone_schedulers(SOUTHERN_SANDORIA) else {
            return;
        };
        let routine = |name: &[u8; 4]| {
            scheds
                .iter()
                .find(|s| {
                    &s.name == name
                        && s.stages
                            .first()
                            .is_some_and(|t| t.stage.local_dir == STABLE_DOOR_GROUP)
                })
                .unwrap_or_else(|| {
                    panic!("{} has a {} routine", "_6ey", String::from_utf8_lossy(name))
                })
        };

        let open: Vec<ModelTransform> = routine(b"open")
            .stages
            .iter()
            .filter_map(|t| t.stage.model_transform)
            .collect();
        assert_eq!(open.len(), 2, "one stage per door leaf");
        for (slot, mt) in open.iter().enumerate() {
            assert_eq!(mt.subchunk, slot as u32);
            assert_eq!(mt.final_value[0], 0.0, "no pitch");
            assert_eq!(mt.final_value[2], 0.0, "no roll");
            assert!(
                (mt.final_value[1].to_degrees().abs() - SWING_DEGREES).abs() < DEGREE_TOLERANCE,
                "leaf {slot} swings {} deg",
                mt.final_value[1].to_degrees()
            );
        }

        let open_stages = &routine(b"open").stages;
        let rotations: Vec<&TimedStage> = open_stages
            .iter()
            .filter(|t| t.stage.kind == StageKind::ModelRotation)
            .collect();
        assert!(rotations.iter().all(|t| t.stage.id == NO_STAGE_ID));
        for t in &rotations {
            assert_eq!(t.stage.duration_frames, SWING_FRAMES);
            assert_eq!(
                t.frame, 0,
                "both leaves start together — leaf 1's authored delay gates what follows it"
            );
        }
        assert!(
            open_stages
                .iter()
                .any(|t| t.stage.kind == StageKind::SoundOnTarget && &t.stage.id == b"9021"),
            "the swing plays the door's SEP sound"
        );

        let clos: Vec<ModelTransform> = routine(b"clos")
            .stages
            .iter()
            .filter_map(|t| t.stage.model_transform)
            .collect();
        assert_eq!(clos.len(), 2);
        for (slot, mt) in clos.iter().enumerate() {
            assert_eq!(mt.subchunk, slot as u32);
            assert_eq!(
                mt.final_value, [0.0; 3],
                "closing drives the leaves back to the authored rest pose, not by a delta"
            );
        }
    }

    // Retail-byte guard (skips without an install). Pso'Xja's sliding stone blocks are the
    // translation opcode and Tavnazian Safehold's doors the rotation one, so between them these
    // three zones exercise both. Every transform stage in them must fit the same layout: a
    // subchunk inside the four slots retail keeps, and a rotation inside one turn.
    #[test]
    fn real_dat_model_transform_layout_holds_across_zones() {
        // vendor/server/src/map/zone.h ZONE_PSOXJA / ZONE_TAVNAZIAN_SAFEHOLD.
        const PSOXJA: u16 = 9;
        const TAVNAZIAN_SAFEHOLD: u16 = 26;

        let mut translations = 0usize;
        let mut rotations = 0usize;
        for zone_id in [PSOXJA, TAVNAZIAN_SAFEHOLD, SOUTHERN_SANDORIA] {
            let Some(scheds) = zone_schedulers(zone_id) else {
                return;
            };
            for stage in scheds.iter().flat_map(|s| s.stages.iter()).map(|t| t.stage) {
                let Some(mt) = stage.model_transform else {
                    continue;
                };
                assert!(
                    mt.subchunk < MODEL_TRANSFORM_SUBCHUNK_SLOTS,
                    "zone {zone_id} addresses subchunk {}",
                    mt.subchunk
                );
                assert_eq!(stage.id, NO_STAGE_ID);
                match stage.kind {
                    StageKind::ModelTranslation => translations += 1,
                    StageKind::ModelRotation => {
                        rotations += 1;
                        for v in mt.final_value {
                            assert!(
                                v.abs() <= std::f32::consts::TAU,
                                "zone {zone_id} rotates {v} rad — past a full turn, so the field
                                 is not an angle"
                            );
                        }
                    }
                    other => panic!("a transform payload on {other:?}"),
                }
            }
        }
        assert!(translations > 0 && rotations > 0);
    }

    #[test]
    fn unknown_type_is_preserved() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend_from_slice(&[0xAB, 0x03, 0, 0]);
        body.extend_from_slice(&5u16.to_le_bytes());
        body.extend_from_slice(&5u16.to_le_bytes());
        body.extend_from_slice(b"????");
        let s = Scheduler::parse(*b"sch0", &body).unwrap();
        assert_eq!(s.stages[0].stage.kind, StageKind::Unknown);
        assert_eq!(s.stages[0].stage.raw_type, 0xAB);
    }

    #[test]
    fn truncated_stage_stops_scan_without_panic() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];

        // STAGE_LENGTH_MASK words is the longest stage the encoding can express (124 bytes),
        // far past this 12-byte tail.
        body.extend_from_slice(&[0x05, STAGE_LENGTH_MASK as u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let s = Scheduler::parse(*b"trun", &body).unwrap();
        assert_eq!(s.stages.len(), 0);
    }

    // research/xim EffectRoutineParser.kt read — the effect list starts where the section-2
    // offset at body +0x14 says it does. Routines with a populated control-flow section put it
    // at raw 0x3C (body 0x2C); the old fixed 64-byte start read past it and found nothing.
    #[test]
    fn section_table_start_beats_fixed_64_byte_header() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        const SEC2_RAW: u32 = 0x3C;
        let sec2_body = SEC2_RAW as usize - CHUNK_HEADER_LEN;
        body[SECTION2_SLOT..SECTION2_SLOT + 4].copy_from_slice(&SEC2_RAW.to_le_bytes());
        body[sec2_body..sec2_body + 4].copy_from_slice(&[0x57, 0x03, 0, 0]);
        body[sec2_body + 4..sec2_body + 6].copy_from_slice(&0u16.to_le_bytes());
        body[sec2_body + 6..sec2_body + 8].copy_from_slice(&0u16.to_le_bytes());
        body[sec2_body + 8..sec2_body + 12].copy_from_slice(b"skaz");

        let s = Scheduler::parse(*b"ati0", &body).unwrap();
        assert_eq!(s.stages.len(), 1);
        assert_eq!(&s.stages[0].stage.id, b"skaz");
        assert!(
            body[SCHEDULER_HEADER_LEN..].iter().all(|&b| b == 0),
            "nothing lives at the fixed 64-byte start — the table is the only way in"
        );
    }

    // research/xim EffectRoutineParser.kt parseSection2 (0x09 useTarget) and :371-375 (0x57). Both were
    // dropped as Unknown, which is what muted every melee routine's linked sound.
    #[test]
    fn opcode_57_and_09_are_subroutine_links() {
        assert_eq!(StageKind::from_stage(0x57, 3), StageKind::SubRoutine);
        assert_eq!(
            StageKind::from_stage(0x09, 3),
            StageKind::SubRoutineOnTarget
        );
        assert_ne!(
            StageKind::from_stage(0x09, 3),
            StageKind::from_stage(0x57, 3),
            "a target-linked child resolves its ids against the victim, not the caster"
        );
    }

    // research/xim EffectRoutineParser.kt parseSection numInputs — the stage length is `unkCombo & 0x1F` dwords, so
    // the high bits of the u16 must not be read as length.
    #[test]
    fn stage_length_masks_the_high_combo_bits() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend_from_slice(&[0x57, 0x03, 0xE0, 0]);
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(b"vatk");
        body.extend_from_slice(&[0x57, 0x03, 0xE0, 0]);
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(b"skaz");

        let s = Scheduler::parse(*b"atk0", &body).unwrap();
        assert_eq!(s.stages.len(), 2);
        assert_eq!(&s.stages[1].stage.id, b"skaz");
    }

    // research/xim EffectRoutineParser.kt parseSection2,553-559 — 0x3D opens a block whose children
    // are alternatives, not siblings; retail runs exactly one per activation.
    #[test]
    fn random_block_tags_its_children_with_one_group() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend_from_slice(&[RANDOM_BLOCK_OPEN, 0x02, 0, 0, 0, 0, 0, 0]);
        for id in [b"atk1", b"atk2"] {
            body.extend_from_slice(&[0x0A, 0x03, 0, 0]);
            body.extend_from_slice(&7u16.to_le_bytes());
            body.extend_from_slice(&0u16.to_le_bytes());
            body.extend_from_slice(id);
        }
        body.extend_from_slice(&[RANDOM_BLOCK_CLOSE, 0x02, 0, 0, 0, 0, 0, 0]);
        body.extend_from_slice(&[0x57, 0x03, 0, 0]);
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(b"skaz");

        let s = Scheduler::parse(*b"vatk", &body).unwrap();
        let grouped: Vec<_> = s
            .stages
            .iter()
            .filter(|t| t.stage.random_group == Some(0))
            .map(|t| t.stage.id)
            .collect();
        assert_eq!(grouped, vec![*b"atk1", *b"atk2"]);
        let after = s
            .stages
            .iter()
            .find(|t| &t.stage.id == b"skaz")
            .expect("the stage after the block survives");
        assert_eq!(after.stage.random_group, None);
        assert_eq!(
            after.frame, 0,
            "an alternative's delay must not advance the parent timeline"
        );
    }

    // Retail-byte guard (skips without an install). `daml` in the global effect dir is the hit
    // reaction switch: four `context.hitTypeFlag` cases (research/xim
    // EffectRoutineInstance.kt resolveControlFlowVariable) whose branch order pins ActionResolution
    // Hit/Miss/Guard/Parry (vendor/server/src/map/enums/action/resolution.h) against the DAT.
    // Parsed to ZERO stages before the section table was read.
    #[test]
    fn real_dat_daml_switches_hit_type_to_reaction_routines() {
        let Some(scheds) = global_effect_schedulers() else {
            return;
        };
        let daml = scheds
            .iter()
            .find(|s| &s.name == b"daml")
            .expect("global effect dir has the daml hit-type switch");
        assert!(daml.has_control_flow(), "daml is a conditional switch");
        let branches: Vec<[u8; 4]> = daml
            .stages
            .iter()
            .filter(|t| t.stage.kind == StageKind::SubRoutineOnTarget)
            .map(|t| t.stage.id)
            .collect();
        assert_eq!(branches, vec![*b"ldam", *b"sway", *b"gurd", *b"pary"]);
    }

    // Retail-byte guard (skips without an install). `dam0` is the MELEE hit-reaction switch
    // (`dada` tail-calls it; `daml` above is the ranged `ldad` chain). Its `context.hitTypeFlag`
    // cases dispatch, in ActionResolution order (vendor/server/src/map/enums/action/resolution.h),
    // Hit -> damh|damg, Miss -> sway, Guard -> gurd, Parry -> pary, Block -> gur1 — the only
    // authority for the Block branch, which `daml` does not carry. The `sb00`..`sb09` additional
    // effects and the `cnt0` counter switch on a different variable and precede all of them.
    #[test]
    fn real_dat_dam0_switches_hit_type_to_melee_reaction_routines() {
        let Some(scheds) = global_effect_schedulers() else {
            return;
        };
        let dam0 = scheds
            .iter()
            .find(|s| &s.name == b"dam0")
            .expect("global effect dir has the dam0 melee hit switch");
        assert!(dam0.has_control_flow(), "dam0 is a conditional switch");
        let branches: Vec<[u8; 4]> = dam0
            .stages
            .iter()
            .filter(|t| t.stage.kind == StageKind::SubRoutineOnTarget)
            .map(|t| t.stage.id)
            .skip_while(|id| id.starts_with(b"sb") || id == b"cnt0")
            .collect();
        assert_eq!(
            branches,
            vec![*b"damh", *b"damg", *b"sway", *b"gurd", *b"pary", *b"gur1"]
        );
    }

    // Retail-byte guard: the global `ldam` (ActionResolution::Hit) routine is where the impact
    // sound and the victim's hurt grunt live, both behind opcode 0x57.
    #[test]
    fn real_dat_ldam_links_impact_and_hurt_sounds() {
        let Some(scheds) = global_effect_schedulers() else {
            return;
        };
        let ldam = scheds
            .iter()
            .find(|s| &s.name == b"ldam")
            .expect("global effect dir has the ldam hit reaction");
        let links: Vec<[u8; 4]> = ldam
            .stages
            .iter()
            .filter(|t| t.stage.kind == StageKind::SubRoutine)
            .map(|t| t.stage.id)
            .collect();
        assert!(links.contains(b"sdam"), "impact sound, got {links:?}");
        assert!(links.contains(b"vdam"), "hurt grunt, got {links:?}");
        assert!(
            ldam.stages
                .iter()
                .any(|t| t.stage.kind == StageKind::SubRoutineOnTarget && &t.stage.id == b"chit"),
            "the hit flash runs on the victim"
        );
    }

    // Retail-byte guard on ROM/32/13.DAT (HumeM weapon-motion base): the main-hand swing links
    // the weapon's `skaz` whoosh and the `dada` damage routine. Both were Unknown before.
    #[test]
    fn real_dat_ati0_links_swing_sound_and_damage_routine() {
        const HUME_M_WEAPON_MOTION_FILE: u32 = 9672;
        let Some(scheds) = schedulers_in_file(HUME_M_WEAPON_MOTION_FILE) else {
            return;
        };
        let ati0 = scheds
            .iter()
            .find(|s| &s.name == b"ati0")
            .expect("weapon-motion DAT has the main-hand swing");
        let link = |id: &[u8; 4]| ati0.stages.iter().find(|t| &t.stage.id == id);
        let skaz = link(b"skaz").expect("ati0 links the swing whoosh");
        assert_eq!(skaz.stage.kind, StageKind::SubRoutine);
        assert_eq!(skaz.stage.raw_type, 0x57);
        let dada = link(b"dada").expect("ati0 links the damage routine");
        assert_eq!(dada.stage.kind, StageKind::SubRoutine);
        assert!(
            skaz.frame < dada.frame,
            "the whoosh leads the impact: {} then {}",
            skaz.frame,
            dada.frame
        );
        let atk0 = scheds
            .iter()
            .find(|s| &s.name == b"atk0")
            .expect("weapon-motion DAT has the voice routine");
        assert!(
            atk0.stages
                .iter()
                .any(|t| t.stage.kind == StageKind::SubRoutine && &t.stage.id == b"vatk"),
            "atk0 links the attack grunt"
        );
    }

    // Retail-byte guard on ROM/27/87.DAT (HumeM face 0): `vatk` is a random block of four
    // grunts. Retail plays ONE; without the block they would all fire at frame 0 together.
    #[test]
    fn real_dat_vatk_random_block_holds_the_four_grunts() {
        const HUME_M_FACE_FILE: u32 = 7080;
        let Some(scheds) = schedulers_in_file(HUME_M_FACE_FILE) else {
            return;
        };
        let vatk = scheds
            .iter()
            .find(|s| &s.name == b"vatk")
            .expect("face DAT has the attack voice routine");
        let mut grouped: Vec<[u8; 4]> = vatk
            .stages
            .iter()
            .filter(|t| t.stage.random_group == Some(0) && t.stage.kind == StageKind::SoundOnCaster)
            .map(|t| t.stage.id)
            .collect();
        grouped.sort();
        assert_eq!(grouped, vec![*b"atk1", *b"atk2", *b"atk3", *b"atk4"]);
        // The routine opens with an unconditional `START_ROUTINE_MARKER_OPCODE` marker
        // whose retail handler is a no-op, so the inert stages are the unknowns plus
        // that marker.
        assert!(
            vatk.stages.iter().all(|t| {
                t.stage.random_group.is_some()
                    || t.stage.kind == StageKind::Unknown
                    || t.stage.kind == StageKind::StartRoutineMarker
            }),
            "every sound in vatk is an alternative, not an unconditional stage"
        );
    }

    // research/xim EffectRoutineParser.kt parseSection2 AnimationLockEffect — an argument-less opcode,
    // so the stage is 8 bytes and carries only delay/duration.
    const ARGLESS_STAGE_WORDS: u8 = (STAGE_HEADER_LEN / 4) as u8;
    const FLINCH_STAGE_WORDS: u8 = ((STAGE_HEADER_LEN + FLINCH_PAYLOAD_LEN) / 4) as u8;
    const KNOCKBACK_PAYLOAD_LEN: usize = 16;
    const KNOCKBACK_STAGE_WORDS: u8 = ((STAGE_HEADER_LEN + KNOCKBACK_PAYLOAD_LEN) / 4) as u8;

    fn timed_stage_bytes(opcode: u8, length_words: u8, delay: u16, duration: u16) -> Vec<u8> {
        let mut b = vec![opcode, length_words, 0, 0];
        b.extend_from_slice(&delay.to_le_bytes());
        b.extend_from_slice(&duration.to_le_bytes());
        b
    }

    // Opcodes cross-checked against research/xim EffectRoutineParser.kt parseSection2 on a
    // synthetic body: a lock, a stop of the routine named `init`, and a fade to neutral.
    #[test]
    fn mob_routine_opcodes_decode_lock_stop_and_fade() {
        const LOCK_TICKS: u16 = 112;
        const FADE_TICKS: u16 = 60;
        const LOCK_STAGE_WORDS: u8 = ARGLESS_STAGE_WORDS + 1;
        const NAMED_STAGE_WORDS: u8 = ARGLESS_STAGE_WORDS + 2;
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(
            ANIMATION_LOCK_OPCODE,
            LOCK_STAGE_WORDS,
            0,
            LOCK_TICKS,
        ));
        body.extend_from_slice(&0u32.to_le_bytes()); // xim expectZero32
        body.extend(timed_stage_bytes(
            STOP_ROUTINE_OPCODE,
            NAMED_STAGE_WORDS,
            0,
            0,
        ));
        body.extend_from_slice(b"init");
        body.extend_from_slice(&0u32.to_le_bytes()); // xim expectZero32
        body.extend(timed_stage_bytes(
            ACTOR_FADE_CASTER_OPCODE,
            NAMED_STAGE_WORDS,
            0,
            FADE_TICKS,
        ));
        body.extend_from_slice(&ACTOR_FADE_NEUTRAL);
        body.extend_from_slice(&0u32.to_le_bytes()); // xim expectZero32

        let s = Scheduler::parse(*b"ini1", &body).unwrap();
        assert_eq!(s.stages.len(), 3);
        let lock = s.stages[0].stage;
        assert_eq!(lock.kind, StageKind::AnimationLock);
        assert_eq!(lock.duration_frames, LOCK_TICKS);
        assert_eq!(lock.id, NO_STAGE_ID);
        let stop = s.stages[1].stage;
        assert_eq!(stop.kind, StageKind::StopRoutine);
        assert_eq!(&stop.id, b"init");
        let fade = s.stages[2].stage;
        assert_eq!(fade.kind, StageKind::ActorFadeOnCaster);
        assert_eq!(fade.duration_frames, FADE_TICKS);
        assert_eq!(fade.actor_fade, Some(ACTOR_FADE_NEUTRAL));
        assert_eq!(
            fade.id, NO_STAGE_ID,
            "the fade destination must not read as a DatId"
        );
    }

    #[test]
    fn mob_routine_opcodes_map_to_their_kinds() {
        for (opcode, words, kind) in [
            (ANIMATION_LOCK_MAGIC_OPCODE, 2, StageKind::AnimationLock),
            (MOVEMENT_LOCK_OPCODE, 2, StageKind::MovementLock),
            (LOCK_LOOK_AT_OPCODE, 3, StageKind::LockLookAt),
            (
                SET_MODEL_VISIBILITY_OPCODE,
                4,
                StageKind::SetModelVisibility,
            ),
            (
                ACTOR_POSITION_SNAPSHOT_OPCODE,
                2,
                StageKind::ActorPositionSnapshot,
            ),
            (JOINT_SNAPSHOT_OPCODE, 3, StageKind::JointSnapshot),
            (SPELL_EFFECT_OPCODE, 3, StageKind::SpellEffect),
            (
                START_ROUTINE_MARKER_OPCODE,
                2,
                StageKind::StartRoutineMarker,
            ),
            (PARTICLE_DAMPEN_OPCODE, 4, StageKind::ParticleDampen),
            (FLINCH_CASTER_OPCODE, 3, StageKind::FlinchOnCaster),
            (FLINCH_TARGET_OPCODE, 3, StageKind::FlinchOnTarget),
            (
                KNOCKBACK_OPCODE,
                KNOCKBACK_STAGE_WORDS as usize,
                StageKind::Knockback,
            ),
            (
                KNOCKBACK_ALT_OPCODE,
                KNOCKBACK_STAGE_WORDS as usize,
                StageKind::Knockback,
            ),
            (
                ACTOR_ROTATION_OPCODE,
                ACTOR_ROTATION_STAGE_WORDS,
                StageKind::ActorRotation,
            ),
            (
                ACTOR_ROTATION_ALT_OPCODE,
                ACTOR_ROTATION_STAGE_WORDS,
                StageKind::ActorRotation,
            ),
            (DISPLAY_DEAD_OPCODE, 5, StageKind::DisplayDead),
            (TRANSITION_TO_IDLE_OPCODE, 3, StageKind::TransitionToIdle),
            (ACTOR_FADE_TARGET_OPCODE, 4, StageKind::ActorFadeOnTarget),
        ] {
            assert_eq!(
                StageKind::from_stage(opcode, words),
                kind,
                "opcode {opcode:#x}"
            );
        }
    }

    // The 0x75 payload is hidden u32, slot u16, ifEngaged u16 (research/xim
    // EffectRoutineParser.kt parseSection2), so its id slot must not surface as a DatId.
    #[test]
    fn set_model_visibility_payload_is_not_a_datid() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(SET_MODEL_VISIBILITY_OPCODE, 4, 0, 0));
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&2u16.to_le_bytes());
        body.extend_from_slice(&1u16.to_le_bytes());

        let s = Scheduler::parse(*b"splg", &body).unwrap();
        let stage = &s.stages[0].stage;
        assert_eq!(stage.kind, StageKind::SetModelVisibility);
        assert_eq!(
            stage.model_visibility,
            Some(ModelVisibility {
                hidden: true,
                slot: 2,
                if_engaged: true
            })
        );
        assert_eq!(stage.id, NO_STAGE_ID);
    }

    // The 0x19 payload is the spell animation index (research/xim EffectRoutineParser.kt
    // parseSection2), so its id slot must not surface as a DatId.
    #[test]
    fn spell_effect_payload_is_not_a_datid() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(SPELL_EFFECT_OPCODE, 3, 0, 0));
        body.extend_from_slice(&617u32.to_le_bytes());

        let s = Scheduler::parse(*b"sdep", &body).unwrap();
        let stage = &s.stages[0].stage;
        assert_eq!(stage.kind, StageKind::SpellEffect);
        assert_eq!(stage.spell_effect, Some(617));
        assert_eq!(stage.id, NO_STAGE_ID);
    }

    // The flinch and knockback payloads are floats/ints from +8 on (research/xim
    // EffectRoutineParser.kt parseSection2), so their id slot must not surface as a DatId.
    // Knockback: u16 u16 f32 f32 u32.
    #[test]
    fn flinch_and_knockback_payloads_are_not_datids() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(
            FLINCH_CASTER_OPCODE,
            FLINCH_STAGE_WORDS,
            0,
            0,
        ));
        body.extend(std::iter::repeat_n(0u8, FLINCH_PAYLOAD_LEN));
        body.extend(timed_stage_bytes(
            KNOCKBACK_OPCODE,
            KNOCKBACK_STAGE_WORDS,
            0,
            0,
        ));
        body.extend(std::iter::repeat_n(0u8, KNOCKBACK_PAYLOAD_LEN));

        let s = Scheduler::parse(*b"damg", &body).unwrap();
        assert_eq!(s.stages[0].stage.kind, StageKind::FlinchOnCaster);
        assert_eq!(s.stages[0].stage.id, NO_STAGE_ID);
        assert_eq!(s.stages[1].stage.kind, StageKind::Knockback);
        assert_eq!(s.stages[1].stage.id, NO_STAGE_ID);
    }

    // research/xim EffectRoutineParser.kt parseSection2 0x5E: u16, u16, then the f32
    // animationDuration, so it sits four bytes into the payload.
    #[test]
    fn knockback_stage_reads_its_animation_duration() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(
            KNOCKBACK_OPCODE,
            KNOCKBACK_STAGE_WORDS,
            0,
            0,
        ));
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&12.5f32.to_le_bytes());
        body.extend_from_slice(&0f32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());

        let s = Scheduler::parse(*b"kb00", &body).unwrap();
        assert_eq!(s.stages[0].stage.kind, StageKind::Knockback);
        assert_eq!(s.stages[0].stage.flinch_duration, Some(12.5));
    }

    // The 0x28 payload is an f32 transition time in the id slot (research/xim
    // EffectRoutineParser.kt parseSection2).
    #[test]
    fn transition_to_idle_reads_the_f32_payload() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(0x28, 0x03, 0, 0));
        body.extend_from_slice(&1.5f32.to_le_bytes());

        let s = Scheduler::parse(*b"dead", &body).unwrap();
        let st = s.stages[0].stage;
        assert_eq!(st.kind, StageKind::TransitionToIdle);
        assert_eq!(st.idle_transition_time, Some(1.5));
        assert_eq!(st.id, NO_STAGE_ID);
    }

    // research/xim EffectRoutineParser.kt parseSection2 — the magic lock form is two dwords total, so
    // it must map without the +8 id dword: the blanket gate read every shipped two-dword lock
    // stage as Unknown.
    #[test]
    fn magic_animation_lock_maps_at_two_dwords() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(
            ANIMATION_LOCK_MAGIC_OPCODE,
            ARGLESS_STAGE_WORDS,
            0,
            112,
        ));

        let s = Scheduler::parse(*b"waso", &body).unwrap();
        let st = s.stages[0].stage;
        assert_eq!(st.kind, StageKind::AnimationLock);
        assert_eq!(st.duration_frames, 112);
        assert_eq!(st.id, NO_STAGE_ID);
    }

    // The look-at suppression stage carries one operand, the duration word at `[record+6]`; the dword
    // after it is zero in every shipped record of this install, so it must not surface as a DatId.
    #[test]
    fn lock_look_at_reads_its_duration_operand_and_no_id() {
        const LOCK_LOOK_AT_DURATION_FRAMES: u16 = 192;
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(
            LOCK_LOOK_AT_OPCODE,
            ARGLESS_STAGE_WORDS + 1,
            0,
            LOCK_LOOK_AT_DURATION_FRAMES,
        ));
        body.extend_from_slice(&0u32.to_le_bytes());

        let s = Scheduler::parse(*b"lkla", &body).unwrap();
        let st = s.stages[0].stage;
        assert_eq!(st.kind, StageKind::LockLookAt);
        assert_eq!(st.duration_frames, LOCK_LOOK_AT_DURATION_FRAMES);
        assert_eq!(st.id, NO_STAGE_ID);
    }

    const ACTOR_ROTATION_STAGE_WORDS: usize = ACTOR_ROTATION_PAYLOAD_LEN / 4;

    /// The shipped rotation records all hold a zero first/third angle, the turn on the middle one,
    /// and mode byte 0. A stage shorter than the payload is not a rotation: it would read past its
    /// own bytes.
    #[test]
    fn actor_rotation_reads_its_euler_and_mode_and_is_not_a_datid() {
        const TURN_DEGREES: f32 = -135.0;
        const ROTATION_FRAMES: u16 = 24;
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(
            ACTOR_ROTATION_OPCODE,
            ACTOR_ROTATION_STAGE_WORDS as u8,
            0,
            ROTATION_FRAMES,
        ));
        for angle in [0.0f32, TURN_DEGREES, 0.0] {
            body.extend_from_slice(&angle.to_le_bytes());
        }
        body.extend_from_slice(&[1, 0, 0, 0]);

        let s = Scheduler::parse(*b"rota", &body).unwrap();
        let st = s.stages[0].stage;
        assert_eq!(st.kind, StageKind::ActorRotation);
        assert_eq!(st.duration_frames, ROTATION_FRAMES);
        assert_eq!(
            st.actor_rotation,
            Some(ActorRotation {
                angles_degrees: [0.0, TURN_DEGREES, 0.0],
                mode: 1,
            })
        );
        assert_eq!(
            st.id, NO_STAGE_ID,
            "the euler and mode dwords must not read as a DatId"
        );
    }

    #[test]
    fn actor_rotation_needs_its_whole_payload_to_map() {
        for words in 3..ACTOR_ROTATION_STAGE_WORDS {
            assert_eq!(
                StageKind::from_stage(ACTOR_ROTATION_OPCODE, words),
                StageKind::Unknown,
                "{words} dwords cannot hold three angles and a mode"
            );
        }
    }

    // research/xim EffectRoutineParser.kt parseSection2 — delay is read for EVERY opcode, so an 8-byte
    // argument-less stage still advances the routine clock for the stages after it.
    #[test]
    fn argless_stage_still_advances_the_routine_clock() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(0x05, 0x03, 10, 20));
        body.extend_from_slice(b"mot0");
        body.extend(timed_stage_bytes(
            ANIMATION_LOCK_OPCODE,
            ARGLESS_STAGE_WORDS,
            25,
            0,
        ));
        body.extend(timed_stage_bytes(0x53, 0x03, 0, 1));
        body.extend_from_slice(b"snd0");

        let s = Scheduler::parse(*b"main", &body).unwrap();
        assert_eq!(s.stages.len(), 3);
        assert_eq!(s.stages[1].stage.raw_type, ANIMATION_LOCK_OPCODE);
        assert_eq!(s.stages[1].frame, 10);
        assert_eq!(s.stages[1].stage.delay_frames, 25);
        assert_eq!(&s.stages[1].stage.id, &NO_STAGE_ID);
        assert_eq!(
            s.stages[2].frame, 35,
            "the sound waits out the animation lock's delay too"
        );
    }

    // research/XIClient HandleTag0x0F: opcode 0x0F, three dwords, destination RGBA in the third.
    const SCREEN_COLOR_OPCODE: u8 = 0x0F;
    const SCREEN_COLOR_STAGE_WORDS: u8 = (STAGE_WITH_ID_LEN / 4) as u8;

    #[test]
    fn screen_color_drive_takes_the_id_dword_as_its_destination() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(
            SCREEN_COLOR_OPCODE,
            SCREEN_COLOR_STAGE_WORDS,
            30,
            30,
        ));
        body.extend_from_slice(&[0x00, 0x00, 0x00, SCREEN_COLOR_UNIT]);

        let s = Scheduler::parse(*b"fdo0", &body).unwrap();
        let stage = s.stages[0].stage;
        assert_eq!(stage.kind, StageKind::ScreenColorDrive);
        assert_eq!(stage.duration_frames, 30);
        assert_eq!(
            stage.screen_color,
            Some(ScreenColor {
                rgba: [0x00, 0x00, 0x00, SCREEN_COLOR_UNIT]
            })
        );
        assert_eq!(
            stage.id, NO_STAGE_ID,
            "the destination bytes must not be readable as a DatId"
        );
    }

    #[test]
    fn screen_color_unit_is_the_untinted_multiplier() {
        let identity = ScreenColor {
            rgba: [SCREEN_COLOR_UNIT; 4],
        };
        assert_eq!(identity.tint(), [1.0; 4]);
        assert_eq!(ScreenColor { rgba: [0; 4] }.tint(), [0.0; 4]);
    }

    // research/xim EffectRoutineParser.kt parseSection2 — ControlFlowBranch takes no argument, so a
    // switch is built entirely out of 8-byte stages.
    #[test]
    fn control_flow_is_seen_through_argless_branch_opcodes() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(
            CONTROL_FLOW_BRANCH_TRUE,
            ARGLESS_STAGE_WORDS,
            0,
            0,
        ));
        body.extend(timed_stage_bytes(0x09, 0x03, 0, 0));
        body.extend_from_slice(b"ldam");

        let s = Scheduler::parse(*b"daml", &body).unwrap();
        assert!(s.has_control_flow());
        assert_eq!(s.stages[1].stage.kind, StageKind::SubRoutineOnTarget);
    }

    #[test]
    fn short_control_flow_condition_at_end_of_body_carries_no_op() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(
            CONTROL_FLOW_CONDITION,
            ARGLESS_STAGE_WORDS,
            0,
            0,
        ));

        let s = Scheduler::parse(*b"dam0", &body).unwrap();
        assert_eq!(s.stages[0].stage.raw_type, CONTROL_FLOW_CONDITION);
        assert_eq!(s.stages[0].stage.control_flow, None);
        assert_eq!(s.stages[0].stage.id, NO_STAGE_ID);
    }

    // research/xim EffectRoutineParser.kt parseSection2 — ControlFlowBlock is built with `delay = 0`.
    #[test]
    fn control_flow_block_delay_is_forced_to_zero() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(
            CONTROL_FLOW_BLOCK_OPEN,
            ARGLESS_STAGE_WORDS,
            99,
            0,
        ));
        body.extend(timed_stage_bytes(0x53, 0x03, 0, 0));
        body.extend_from_slice(b"snd0");

        let s = Scheduler::parse(*b"blk0", &body).unwrap();
        assert_eq!(s.stages[0].stage.delay_frames, 0);
        assert_eq!(s.stages[1].frame, 0);
    }

    // research/xim EffectRoutineParser.kt parseSection2 — the closer calls no `addEffectRoutine`, so it
    // is not one of the block's alternatives. Tagged as a member it could be the pick, and the
    // whole block would run nothing.
    #[test]
    fn random_block_close_is_not_a_member() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend(timed_stage_bytes(
            RANDOM_BLOCK_OPEN,
            ARGLESS_STAGE_WORDS,
            0,
            0,
        ));
        for id in [b"atk1", b"atk2"] {
            body.extend(timed_stage_bytes(0x0A, 0x03, 7, 0));
            body.extend_from_slice(id);
        }
        // Id-bearing closer: the >= 12-byte variant, distinct from the 8-byte argless form.
        body.extend(timed_stage_bytes(RANDOM_BLOCK_CLOSE, 0x03, 0, 0));
        body.extend_from_slice(&NO_STAGE_ID);

        let s = Scheduler::parse(*b"vatk", &body).unwrap();
        let closer = s
            .stages
            .iter()
            .find(|t| t.stage.raw_type == RANDOM_BLOCK_CLOSE)
            .expect("the closer is a stage");
        assert_eq!(closer.stage.random_group, None);
        assert_eq!(
            s.stages
                .iter()
                .filter(|t| t.stage.random_group == Some(0))
                .count(),
            2,
            "only the two alternatives belong to the block"
        );
    }

    // Retail-byte guard (skips without an install). These eight effect DATs carry
    // no 0x0A/0x0B/0x53 stage at all — every sound they play is a 0x4A or 0x60, so
    // before those opcodes were recognised each one was completely silent.
    #[test]
    fn real_dat_non_positional_only_effects_are_no_longer_silent() {
        const SILENT_WITHOUT_4A_60: [u32; 8] = [3108, 3109, 3110, 3115, 3116, 3117, 3118, 3119];
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let schedulers_in = |file_id: u32| -> Option<Vec<Scheduler>> {
            let loc = root.resolve(file_id).ok()?;
            let bytes = std::fs::read(loc.path_under(&root)).ok()?;
            Some(crate::resource_dir::ResourceDir::from_bytes(bytes).collect_schedulers())
        };
        for file_id in SILENT_WITHOUT_4A_60 {
            let Some(scheds) = schedulers_in(file_id) else {
                continue;
            };
            let kinds: Vec<StageKind> = scheds
                .iter()
                .flat_map(|s| s.stages.iter())
                .map(|t| t.stage.kind)
                .filter(|k| {
                    matches!(
                        k,
                        StageKind::SoundOnCaster
                            | StageKind::SoundOnTarget
                            | StageKind::SoundNonPositional
                    )
                })
                .collect();
            assert!(
                !kinds.is_empty(),
                "file {file_id} has no sound stage of any kind"
            );
            assert!(
                kinds.iter().all(|k| *k == StageKind::SoundNonPositional),
                "file {file_id} was expected to be 0x4A/0x60-only, got {kinds:?}"
            );
        }
    }

    fn schedulers_in_file(file_id: u32) -> Option<Vec<Scheduler>> {
        let root = crate::archive::open_test_install()?;
        let loc = root.resolve(file_id).ok()?;
        let bytes = std::fs::read(loc.path_under(&root)).ok()?;
        Some(crate::resource_dir::ResourceDir::from_bytes(bytes).collect_schedulers())
    }

    fn global_effect_schedulers() -> Option<Vec<Scheduler>> {
        schedulers_in_file(0)
    }
}

#[cfg(test)]
mod vehicle_contract_tests {
    use super::*;

    const FOLLOW_POINTS_TAG: u8 = 0x27;
    const ABSOLUTE_FORWARD_HEADING: u32 = 11;
    const COSINE_EASING: u32 = 2;
    const NEXT_STAGE_DELAY: u16 = 600;
    const PATH_DURATION: u16 = 1800;

    fn stage() -> Vec<u8> {
        let mut bytes = vec![0; FOLLOW_POINTS_PAYLOAD_LEN];
        bytes[0] = FOLLOW_POINTS_TAG;
        bytes[1] = (FOLLOW_POINTS_PAYLOAD_LEN / 4) as u8;
        bytes[DELAY_OFFSET..DELAY_OFFSET + 2].copy_from_slice(&NEXT_STAGE_DELAY.to_le_bytes());
        bytes[DURATION_OFFSET..DURATION_OFFSET + 2].copy_from_slice(&PATH_DURATION.to_le_bytes());
        bytes[ID_OFFSET..ID_OFFSET + 4].copy_from_slice(b"pat1");
        bytes[FOLLOW_POINTS_FLAGS_OFFSET..FOLLOW_POINTS_FLAGS_OFFSET + 4]
            .copy_from_slice(&ABSOLUTE_FORWARD_HEADING.to_le_bytes());
        bytes[FOLLOW_POINTS_FLAGS_OFFSET + 4..FOLLOW_POINTS_FLAGS_OFFSET + 8]
            .copy_from_slice(&COSINE_EASING.to_le_bytes());
        bytes[FOLLOW_POINTS_ROTATION_OFFSET..FOLLOW_POINTS_ROTATION_OFFSET + 4]
            .copy_from_slice(&(-std::f32::consts::FRAC_PI_2).to_le_bytes());
        bytes
    }

    #[test]
    fn follow_points_preserves_direction_easing_rotation_and_prior_delay_timing() {
        let mut bytes = vec![0; SCHEDULER_HEADER_LEN];
        bytes.extend(stage());
        bytes.extend(stage());
        let scheduler = Scheduler::parse(*b"seq1", &bytes).unwrap();
        assert_eq!(scheduler.stages.len(), 2);
        assert_eq!(scheduler.stages[0].frame, 0);
        assert_eq!(scheduler.stages[1].frame, u32::from(NEXT_STAGE_DELAY));
        for stage in scheduler.stages {
            assert_eq!(stage.stage.kind, StageKind::FollowPoints);
            assert_eq!(stage.stage.id, *b"pat1");
            assert_eq!(stage.stage.duration_frames, PATH_DURATION);
            assert_eq!(
                stage.stage.follow_points,
                Some(FollowPoints {
                    flags: ABSOLUTE_FORWARD_HEADING,
                    easing: COSINE_EASING,
                    rotation: -std::f32::consts::FRAC_PI_2,
                })
            );
        }
    }

    #[test]
    fn short_follow_points_payload_cannot_become_a_motion_command() {
        let mut bytes = vec![0; SCHEDULER_HEADER_LEN];
        let mut short = stage();
        short.truncate(FOLLOW_POINTS_ROTATION_OFFSET);
        short[1] = (short.len() / 4) as u8;
        bytes.extend(short);
        let scheduler = Scheduler::parse(*b"seq1", &bytes).unwrap();
        assert_eq!(scheduler.stages.len(), 1);
        assert_eq!(scheduler.stages[0].stage.kind, StageKind::Unknown);
        assert_eq!(scheduler.stages[0].stage.follow_points, None);
    }

    /// Route anchors: ex1a plays the 1c* routes, ex1b the 2c* routes, mov2 the
    /// c1* through c4* routes; each assert's zone id is that route's hex prefix.
    #[test]
    fn zone_camera_route_name_spells_the_hex_zone_prefix_and_decimal_index() {
        assert_eq!(zone_camera_route_name(0x1C, 1), *b"1c01");
        assert_eq!(zone_camera_route_name(0x2C, 14), *b"2c14");
        assert_eq!(zone_camera_route_name(0xC1, 7), *b"c107");
        assert_eq!(zone_camera_route_name(0xC4, 99), *b"c499");
        assert_eq!(zone_camera_route_name(0, 0), *b"0000");
    }

    /// Retail-byte guard (skips without an install). The MAPSCHEDULOR keys of the
    /// Chamber of Oracles (168) live in zone 168's own model DAT (ROM/2/11.DAT),
    /// the dominant resolution rule.
    #[test]
    fn zone_scene_resolves_in_the_zones_own_model_dat() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let file =
            crate::zone_dat::zone_id_to_mzb_file_id(168).expect("zone 168 maps to a model DAT");
        for key in [b"215s", b"220a"] {
            assert_eq!(
                zone_scene_file_id(&root, 168, *key),
                Some(file),
                "{key:?} resolves in zone 168's own model DAT"
            );
        }
    }

    /// Retail-byte guard (skips without an install). Sealion's Den (32) event 100
    /// runs `lwon` out of zone 32's own model DAT (ROM/3/98.DAT).
    #[test]
    fn zone_scene_resolves_sealions_den_lwon_in_its_own_model_dat() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let file =
            crate::zone_dat::zone_id_to_mzb_file_id(32).expect("zone 32 maps to a model DAT");
        assert_eq!(zone_scene_file_id(&root, 32, *b"lwon"), Some(file));
    }

    /// Retail-byte guard (skips without an install). A repeated MAPSCHEDULOR lookup
    /// for the same (zone, key) is served from the memo instead of re-reading the
    /// zone's model DAT; an overlay-swap clear forces one re-resolve. The probe key
    /// is unique to this test so parallel tests resolving real keys leave its
    /// per-key counter alone.
    #[test]
    fn zone_scene_lookups_are_memoized_and_cleared() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let key = *b"zz99";
        let before = zone_scene_resolve_count(168, key);
        assert_eq!(zone_scene_file_id(&root, 168, key), None);
        assert_eq!(
            zone_scene_resolve_count(168, key),
            before + 1,
            "the first lookup resolves"
        );
        assert_eq!(zone_scene_file_id(&root, 168, key), None);
        assert_eq!(
            zone_scene_resolve_count(168, key),
            before + 1,
            "the repeat lookup is served from the memo"
        );
        clear_zone_scene_cache();
        assert_eq!(zone_scene_file_id(&root, 168, key), None);
        assert_eq!(
            zone_scene_resolve_count(168, key),
            before + 2,
            "the clear forces a re-resolve"
        );
    }

    /// Retail-byte guard (skips without an install). Heavens' Tower (242) carries no
    /// `hshi` in its own model DAT; the key resolves in the partner zone Full Moon
    /// Fountain (170)'s model DAT: a ZONE_SCENE_PARTNERS instance/entrance pair.
    #[test]
    fn zone_scene_falls_back_to_the_partner_zone_model_dat() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let own =
            crate::zone_dat::zone_id_to_mzb_file_id(242).expect("zone 242 maps to a model DAT");
        let partner =
            crate::zone_dat::zone_id_to_mzb_file_id(170).expect("zone 170 maps to a model DAT");
        assert_ne!(own, partner, "242 and 170 are distinct zones");
        assert_eq!(
            zone_scene_file_id(&root, 242, *b"hshi"),
            Some(partner),
            "242's hshi resolves in partner zone 170's model DAT"
        );
    }
}
