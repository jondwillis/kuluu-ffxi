use crate::{DatError, Result};

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
const RANDOM_BLOCK_OPEN: u8 = 0x3D;
const RANDOM_BLOCK_CLOSE: u8 = 0x3E;

// research/xim EffectRoutineParser.kt parseSection2 — 0x64/0x67 ControlFlowBranch, 0x69/0x6A
// ControlFlowBlock, 0x6B ControlFlowCondition.
const CONTROL_FLOW_BRANCH_TRUE: u8 = 0x64;
const CONTROL_FLOW_BRANCH_FALSE: u8 = 0x67;
const CONTROL_FLOW_BLOCK_OPEN: u8 = 0x69;
const CONTROL_FLOW_BLOCK_CLOSE: u8 = 0x6A;
const CONTROL_FLOW_CONDITION: u8 = 0x6B;

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

// A stage addresses a slot of the group `mzb::underscore_at_groups` builds, so the bound is
// that builder's rather than a second reading of the same retail array.
pub const MODEL_TRANSFORM_SUBCHUNK_SLOTS: u32 =
    crate::mzb::UNDERSCORE_AT_GROUP_MAX_SUBCHUNKS as u32;

const NO_STAGE_ID: [u8; 4] = [0; 4];

/// The MODULATE2X argument that leaves the scene untinted. research/XIClient
/// `GameManager::RenderSomething` composites the persistent screen colour with
/// `D3DTOP_MODULATE2X`, whose identity is 0x80 — which is why the authored
/// fade-in destination is 128,128,128 rather than 255,255,255.
pub const SCREEN_COLOR_UNIT: u8 = 0x80;

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelTransform {
    pub final_value: [f32; 3],
    pub subchunk: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SchedulerStage {
    pub kind: StageKind,

    pub raw_type: u8,

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

    // `Some` exactly for the two flinch kinds when the stage carries the full 9-dword payload:
    // the animationDuration f32 at +24 (research/xim EffectRoutineParser.kt parseFlinchEffect).
    // Retail plays the dfi?/dfm? flinch clip with transition in/out of
    // animationDuration/2 frames each (EffectRoutineInterpolatedEffects.kt FlinchAnimationInstance).
    pub flinch_duration: Option<f32>,

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

    DamageCallback,

    /// 0x07 / 0x59 - AnimationLock for `duration_frames` ticks (SE: `BondageActor` /
    /// `LockCasterMagic`; xim treats both as AnimationLockEffect). Retail's ActionTimer1
    /// lock; refcounted across overlapping routines, and a routine without a lock stage does
    /// not lock.
    AnimationLock,

    /// 0x5F - StopRoutine: stop the running routine named by `id` (xim EffectRoutineParser.kt
    /// parseSection2). The worm's `ini1` stops `init` and `init` stops `ini1` this way.
    StopRoutine,

    /// 0x21 (caster) / 0x25 (target) - flinch; SE `GetDamageDirId` picks the dfi/dbi/dfm/dbm
    /// front/back clip by hit direction.
    FlinchOnCaster,
    FlinchOnTarget,

    /// 0x5E / 0xBF - knockback (xim EffectRoutineParser.kt parseSection2, SE tag table).
    Knockback,

    /// 0x78 - DisplayDeadRoutine (xim EffectRoutineParser.kt parseSection2): the actor is
    /// dead from this stage on.
    DisplayDead,

    /// 0x28 - TransitionToIdle (xim EffectRoutineParser.kt parseSection2);
    /// `idle_transition_time` holds the payload's f32 transition time when present.
    TransitionToIdle,

    /// 0x29 (caster) / 0x2A (target) - ActorFade to `actor_fade` over `duration_frames`
    /// (SE `ActorColorDriveTask`; xim EffectRoutineParser.kt parseSection2). 0x80808080 is the
    /// neutral tint the worm's `init` uses.
    ActorFadeOnCaster,
    ActorFadeOnTarget,

    FollowPoints,
    Unknown,
}

// research/xim EffectRoutineParser.kt parseSection numInputs,141-154 — opcode 0x0A is overloaded: a
// 32-byte stage (length_words 8, XIM numArgs 7) is a Source (caster) sound emitter,
// while any other length is a LinkedEffectRoutine sub-routine. Disambiguate by length.
const SOUND_EMITTER_LENGTH_WORDS: usize = 8;

impl StageKind {
    fn from_stage(b: u8, length_words: usize) -> Self {
        match b {
            // Opcodes empirically confirmed against retail spell DATs (e.g. Cure = file 0xAF1):
            // 0x02 spawns a particle generator, 0x03 calls a sub-routine, 0x05 plays motion,
            // 0x0B/0x53 play sound on target/caster.
            0x02 => Self::Particle,
            0x03 => Self::SubRoutine,
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
            // research/xim EffectRoutineParser.kt parseSection2 — DamageCallbackRoutine, the stage the
            // damage/battle-message callback is invoked on (EffectRoutineInstance.kt handleDamageCallbackRoutine).
            // Every spell routine tail-calls a `mdam` sub-routine that holds exactly this stage.
            0x2B => Self::DamageCallback,
            // research/xim EffectRoutineParser.kt parseSection2, opcodes 0x07/0x59 -
            // AnimationLockEffect; SE `BondageActor` / `LockCasterMagic`, xim treats both as
            // the same lock. Retail's ActionTimer1 animation lock, refcounted across
            // overlapping routines. 0x07 carries a zero dword after
            // delay/duration; 0x59 is argument-less.
            0x07 | 0x59 => Self::AnimationLock,
            // research/xim EffectRoutineParser.kt parseSection2 - FlinchRoutine (SE `GetDamageDirId`
            // picks the dfi/dbi/dfm/dbm front/back clip by hit direction). 0x21 flinches
            // the caster, 0x25 the target.
            0x21 => Self::FlinchOnCaster,
            0x25 => Self::FlinchOnTarget,
            // research/xim EffectRoutineParser.kt parseSection2 - TransitionToIdleEffect; the f32 at
            // +8 is the transition time.
            0x28 => Self::TransitionToIdle,
            // research/xim EffectRoutineParser.kt parseSection2 - ActorFadeRoutine to the RGBA at +8
            // over `duration_frames` (SE `ActorColorDriveTask`). 0x80808080 is the neutral
            // tint the worm's `init` fades back to. 0x29 on the caster, 0x2A on the target.
            0x29 => Self::ActorFadeOnCaster,
            0x2A => Self::ActorFadeOnTarget,
            // research/xim EffectRoutineParser.kt parseSection2 - KnockBackEffect (SE tag table);
            // 0xBF dispatches the same payload.
            0x5E | 0xBF => Self::Knockback,
            // research/xim EffectRoutineParser.kt parseSection2 - StopRoutineEffect: stop the running
            // routine named by `id`. The worm's `ini1` stops `init` and `init` stops `ini1`
            // this way.
            0x5F => Self::StopRoutine,
            // research/xim EffectRoutineParser.kt parseSection2 - DisplayDeadRoutine: the actor is
            // dead from this stage on.
            0x78 => Self::DisplayDead,
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
            0x4A | 0x60 => Self::SoundNonPositional,
            // research/xim EffectRoutineParser.kt parseSection2 — a plain LinkedEffectRoutine, the
            // form every melee routine uses (`ati0` links the weapon's `skaz` whoosh, `atk0`
            // the race/face `vatk` grunt).
            0x57 => Self::SubRoutine,
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

pub const NO_LOCAL_DIR: [u8; 4] = [0; 4];

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
                let kind = if has_id {
                    StageKind::from_stage(raw_type, length_words)
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
                // Flinch animationDuration sits at +24, past the id slot - read it straight off
                // the stage bytes when the full 9-dword payload is present.
                let flinch_duration =
                    (matches!(kind, StageKind::FlinchOnCaster | StageKind::FlinchOnTarget)
                        && stage_bytes >= FLINCH_PAYLOAD_LEN)
                        .then(|| {
                            f32::from_le_bytes([
                                body[cursor + FLINCH_ANIMATION_DURATION_OFFSET],
                                body[cursor + FLINCH_ANIMATION_DURATION_OFFSET + 1],
                                body[cursor + FLINCH_ANIMATION_DURATION_OFFSET + 2],
                                body[cursor + FLINCH_ANIMATION_DURATION_OFFSET + 3],
                            ])
                        });
                // Flinch and knockback payloads are floats/ints from +8 on (research/xim
                // EffectRoutineParser.kt parseSection2), so their id slot is not a DatId either.
                let non_id_payload = model_transform.is_some()
                    || screen_color.is_some()
                    || actor_fade.is_some()
                    || idle_transition_time.is_some()
                    || matches!(
                        kind,
                        StageKind::FlinchOnCaster
                            | StageKind::FlinchOnTarget
                            | StageKind::Knockback
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

    // A routine built out of these is a switch (`daml` picks one hit reaction, `dam0` one
    // additional effect), so inlining it whole would run every branch at once. We do not
    // evaluate the conditions; callers pick the branch.
    pub fn has_control_flow(&self) -> bool {
        self.stages.iter().any(|t| {
            matches!(
                t.stage.raw_type,
                CONTROL_FLOW_BRANCH_TRUE
                    | CONTROL_FLOW_BRANCH_FALSE
                    | CONTROL_FLOW_BLOCK_OPEN
                    | CONTROL_FLOW_BLOCK_CLOSE
                    | CONTROL_FLOW_CONDITION
            )
        })
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
        for op in [0x21u8, 0x25] {
            body.extend_from_slice(&[op, 0x09, 0, 0]); // opcode, length=9 words
            body.extend_from_slice(&2u16.to_le_bytes()); // +4 delay
            body.extend_from_slice(&0u16.to_le_bytes()); // +6 duration
            body.extend_from_slice(&1.0f32.to_le_bytes()); // +8
            body.extend_from_slice(&1.0f32.to_le_bytes()); // +12
            body.extend_from_slice(&2u32.to_le_bytes()); // +16
            body.extend_from_slice(&1.0f32.to_le_bytes()); // +20
            body.extend_from_slice(&10.0f32.to_le_bytes()); // +24 animationDuration
            body.extend_from_slice(&0u32.to_le_bytes()); // +28
            body.extend_from_slice(&0u32.to_le_bytes()); // +32
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
            // The flinch payload's id slot is not a DatId (XIM reads no ref there).
            assert_eq!(&st.id, &[0; 4]);
        }
    }

    // A flinch stage shorter than the full payload carries no animationDuration: the consumer
    // must fall back to its default transitions rather than reading past the stage.
    #[test]
    fn short_flinch_stage_has_no_animation_duration() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        body.extend_from_slice(&[0x21, 0x03, 0, 0]); // opcode, length=3 words (12 bytes)
        body.extend_from_slice(&2u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&[0u8; 4]); // +8 id slot - not a DatId for flinch

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

        let Ok(root) = crate::DatRoot::from_env_or_default() else {
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

        let Ok(root) = crate::DatRoot::from_env_or_default() else {
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
        assert!(
            vatk.stages
                .iter()
                .all(|t| t.stage.random_group.is_some() || t.stage.kind == StageKind::Unknown),
            "every sound in vatk is an alternative, not an unconditional stage"
        );
    }

    // research/xim EffectRoutineParser.kt parseSection2 AnimationLockEffect — an argument-less opcode,
    // so the stage is 8 bytes and carries only delay/duration.
    const ANIMATION_LOCK_OPCODE: u8 = 0x07;
    const ARGLESS_STAGE_WORDS: u8 = (STAGE_HEADER_LEN / 4) as u8;

    fn timed_stage_bytes(opcode: u8, length_words: u8, delay: u16, duration: u16) -> Vec<u8> {
        let mut b = vec![opcode, length_words, 0, 0];
        b.extend_from_slice(&delay.to_le_bytes());
        b.extend_from_slice(&duration.to_le_bytes());
        b
    }

    // Opcodes cross-checked against research/xim EffectRoutineParser.kt parseSection2 on
    // synthetic bodies: 0x0307 is a 3-word 0x07 lock held for 0x70 ticks (the worm's dig hold),
    // 0x045F 'init' stops the named routine, and 0x0429 fades to the neutral 0x80808080 tint.
    #[test]
    fn mob_routine_opcodes_decode_lock_stop_and_fade() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        // 0x07, 3 words: AnimationLock for 112 ticks (the worm's dig hold).
        body.extend(timed_stage_bytes(0x07, 0x03, 0, 0x70));
        body.extend_from_slice(&0u32.to_le_bytes()); // xim expectZero32
                                                     // 0x5F, 4 words: stop the running routine named `init`.
        body.extend(timed_stage_bytes(0x5F, 0x04, 0, 0));
        body.extend_from_slice(b"init");
        body.extend_from_slice(&0u32.to_le_bytes()); // xim expectZero32
                                                     // 0x29, 4 words: fade the caster to neutral over 60 ticks.
        body.extend(timed_stage_bytes(0x29, 0x04, 0, 60));
        body.extend_from_slice(&[0x80; 4]);
        body.extend_from_slice(&0u32.to_le_bytes()); // xim expectZero32

        let s = Scheduler::parse(*b"ini1", &body).unwrap();
        assert_eq!(s.stages.len(), 3);
        let lock = s.stages[0].stage;
        assert_eq!(lock.kind, StageKind::AnimationLock);
        assert_eq!(lock.duration_frames, 0x70);
        assert_eq!(lock.id, NO_STAGE_ID);
        let stop = s.stages[1].stage;
        assert_eq!(stop.kind, StageKind::StopRoutine);
        assert_eq!(&stop.id, b"init");
        let fade = s.stages[2].stage;
        assert_eq!(fade.kind, StageKind::ActorFadeOnCaster);
        assert_eq!(fade.duration_frames, 60);
        assert_eq!(fade.actor_fade, Some([0x80; 4]));
        assert_eq!(
            fade.id, NO_STAGE_ID,
            "the fade destination must not read as a DatId"
        );
    }

    #[test]
    fn mob_routine_opcodes_map_to_their_kinds() {
        assert_eq!(StageKind::from_stage(0x59, 2), StageKind::AnimationLock);
        assert_eq!(StageKind::from_stage(0x21, 3), StageKind::FlinchOnCaster);
        assert_eq!(StageKind::from_stage(0x25, 3), StageKind::FlinchOnTarget);
        assert_eq!(StageKind::from_stage(0x5E, 6), StageKind::Knockback);
        assert_eq!(StageKind::from_stage(0xBF, 6), StageKind::Knockback);
        assert_eq!(StageKind::from_stage(0x78, 5), StageKind::DisplayDead);
        assert_eq!(StageKind::from_stage(0x28, 3), StageKind::TransitionToIdle);
        assert_eq!(StageKind::from_stage(0x2A, 4), StageKind::ActorFadeOnTarget);
    }

    // The flinch and knockback payloads are floats/ints from +8 on (research/xim
    // EffectRoutineParser.kt parseSection2), so their id slot must not surface as a DatId.
    #[test]
    fn flinch_and_knockback_payloads_are_not_datids() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        // 0x21, 9 words: the flinch payload is 28 bytes of floats/ints.
        body.extend(timed_stage_bytes(0x21, 0x09, 0, 0));
        body.extend(std::iter::repeat_n(0u8, 28));
        // 0x5E, 6 words: u16 u16 f32 f32 u32.
        body.extend(timed_stage_bytes(0x5E, 0x06, 0, 0));
        body.extend(std::iter::repeat_n(0u8, 16));

        let s = Scheduler::parse(*b"damg", &body).unwrap();
        assert_eq!(s.stages[0].stage.kind, StageKind::FlinchOnCaster);
        assert_eq!(s.stages[0].stage.id, NO_STAGE_ID);
        assert_eq!(s.stages[1].stage.kind, StageKind::Knockback);
        assert_eq!(s.stages[1].stage.id, NO_STAGE_ID);
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
        let Some(_) = schedulers_in_file(0) else {
            return;
        };
        for file_id in SILENT_WITHOUT_4A_60 {
            let Some(scheds) = schedulers_in_file(file_id) else {
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
        let root = crate::DatRoot::from_env_or_default().ok()?;
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
}
