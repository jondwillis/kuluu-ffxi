use crate::{DatError, Result};

pub const SCHEDULER_HEADER_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerStage {
    pub kind: StageKind,

    pub raw_type: u8,

    pub delay_frames: u16,

    pub duration_frames: u16,

    pub id: [u8; 4],

    // research/xim EffectRoutineParser.kt:115-130 (opcode 0x05). Half-frame units (divide
    // by 2 for real frames). Zero when the stage is shorter than the motion payload.
    pub max_loops: u16,
    pub transition_in: u16,
    pub transition_out: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageKind {
    Motion,

    SoundOnTarget,

    SoundOnCaster,

    Particle,

    SubRoutine,

    Unknown,
}

// research/xim EffectRoutineParser.kt:64,141-154 — opcode 0x0A is overloaded: a
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
            0x0A if length_words == SOUND_EMITTER_LENGTH_WORDS => Self::SoundOnCaster,
            0x0A => Self::SubRoutine,
            0x0B => Self::SoundOnTarget,
            0x53 => Self::SoundOnCaster,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Scheduler {
    pub name: [u8; 4],
    pub stages: Vec<TimedStage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedStage {
    pub frame: u32,
    pub stage: SchedulerStage,
}

impl Scheduler {
    pub fn parse(name: [u8; 4], body: &[u8]) -> Result<Self> {
        if body.len() < SCHEDULER_HEADER_LEN {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: SCHEDULER_HEADER_LEN,
                available: body.len(),
            });
        }
        let mut stages = Vec::new();
        let mut cursor = SCHEDULER_HEADER_LEN;
        let mut running_frame: u32 = 0;

        while cursor + 4 <= body.len() {
            let raw_type = body[cursor];
            let length_words = body[cursor + 1] as usize;
            let stage_bytes = length_words.saturating_mul(4);
            if stage_bytes < 4 || cursor + stage_bytes > body.len() {
                break;
            }

            if stage_bytes >= 12 && cursor + 12 <= body.len() {
                let delay = u16::from_le_bytes([body[cursor + 4], body[cursor + 5]]);
                let duration = u16::from_le_bytes([body[cursor + 6], body[cursor + 7]]);
                let id = [
                    body[cursor + 8],
                    body[cursor + 9],
                    body[cursor + 10],
                    body[cursor + 11],
                ];
                let kind = StageKind::from_stage(raw_type, length_words);
                // research/xim EffectRoutineParser.kt:115-130: after id(+8) and a zero32(+12)
                // and two floats(+16,+20), the 0x05 motion payload carries transitionIn(+24),
                // a zero u16(+26), transitionOut(+28), maxLoop(+30).
                let read_u16 =
                    |off: usize| u16::from_le_bytes([body[cursor + off], body[cursor + off + 1]]);
                let (max_loops, transition_in, transition_out) =
                    if kind == StageKind::Motion && stage_bytes >= 32 {
                        (read_u16(30), read_u16(24), read_u16(28))
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
                    },
                });
                running_frame = running_frame.saturating_add(delay as u32);
            }
            cursor += stage_bytes;
        }
        Ok(Self { name, stages })
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

        body.extend_from_slice(&[0x05, 99, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let s = Scheduler::parse(*b"trun", &body).unwrap();
        assert_eq!(s.stages.len(), 0);
    }
}
