//! Scheduler chunk parser — kind `0x07`. Drives event timelines
//! for action / spell / ability / mob-skill animations.
//!
//! Each scheduler describes a sequence of *stages*; each stage
//! fires after a per-frame `delay` and identifies a 4-char target
//! (the name of a sibling Generator chunk) plus a `type` byte that
//! selects what kind of behavior fires.
//!
//! Wire layout (port of `vendor/lotus-ffxi/ffxi/dat/scheduler.cppm`):
//!
//! ```text
//! SchedulerHeader: 60 bytes of `u32`s, semantics unknown to lotus.
//!                  We read them as opaque metadata.
//!
//! Then 0..N stage records:
//!   0x00  u8     type       0x05=motion, 0x0b=sound-on-target,
//!                            0x53=sound-on-caster, 0x39=particle,
//!                            others observed (lotus ignores most)
//!   0x01  u8     length     stage record length in u32-words
//!                            (i.e. byte length = length * 4)
//!   0x02  u16    flags / unk
//!   0x04  u16 le delay      frame delay relative to previous stage
//!   0x06  u16 le duration   frames the effect lasts
//!   0x08  char[4] id        target generator's 4-char name
//!   0x0C+ ...               type-specific payload (lotus' switch
//!                            consumes only `id`, not the rest)
//! ```
//!
//! Sound-event semantics (lotus comment at `scheduler.cpp:112`):
//!   - `type == 0x0b` → play the named generator's SE on the
//!     **target** entity (the action's victim).
//!   - `type == 0x53` → play it on the **caster** (the action's
//!     source).
//!
//! Lotus stubs both sound cases (`break;` with no body); we surface
//! them as data so a Bevy-side player can subscribe.

use crate::{DatError, Result};

/// Header bytes (first 64 of every Scheduler chunk's body).
/// Empirically determined by hexdumping real DATs (`ROM/0/58.DAT`
/// scheduler `ckem` has stages starting at body offset 0x40). Lotus'
/// struct (`scheduler.cppm`) sums to 72 bytes if you read the cppm
/// literally (4 u32 + 14 u32), but lotus also never validated
/// against real data — the cppm comment notes every field is `unk`.
/// 64 bytes matches what real schedulers in `ROM/0/58.DAT` actually
/// use and produces sensible stage records.
pub const SCHEDULER_HEADER_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerStage {
    pub kind: StageKind,
    /// Raw type byte (preserved for unknown kinds — useful for
    /// reverse-engineering when `kind == Unknown`).
    pub raw_type: u8,
    /// Frame delay relative to the previous stage.
    pub delay_frames: u16,
    /// Frame duration this effect lasts.
    pub duration_frames: u16,
    /// 4-char id of the target generator chunk (or, for non-name
    /// types, the raw 4 bytes — UTF-8 wasn't a requirement).
    pub id: [u8; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageKind {
    /// `0x05` — play an animation/motion on the actor.
    Motion,
    /// `0x0b` — play a sound effect localized to the action's target.
    SoundOnTarget,
    /// `0x53` — play a sound effect localized to the action's caster.
    SoundOnCaster,
    /// `0x39` — particle / VFX (not consumed by audio path).
    Particle,
    /// Any other type byte. The `raw_type` is preserved on the
    /// parent `SchedulerStage` for further inspection.
    Unknown,
}

impl StageKind {
    fn from_byte(b: u8) -> Self {
        match b {
            0x05 => Self::Motion,
            0x0B => Self::SoundOnTarget,
            0x39 => Self::Particle,
            0x53 => Self::SoundOnCaster,
            _ => Self::Unknown,
        }
    }
}

/// One Scheduler chunk's parsed stages, with the wall-clock frame
/// of each stage already accumulated from `delay_frames`.
#[derive(Debug, Clone, Default)]
pub struct Scheduler {
    /// 4-char name from the chunk header. Useful for cross-ref to
    /// caller's lookup tables (which scheduler in this DAT?).
    pub name: [u8; 4],
    pub stages: Vec<TimedStage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedStage {
    /// Absolute frame this stage fires (running sum of preceding
    /// `delay_frames`). Frame 0 = action start.
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
        // Each stage starts with `(type, length_words, ...)` where
        // `length_words * 4` is the total stage byte size. Real DATs
        // mix short (length=2 = 8 bytes — "marker") stages with
        // full (length=4 = 16 bytes, type+length+pad+delay+duration+id)
        // stages. We need to walk past the short ones, not abort.
        while cursor + 4 <= body.len() {
            let raw_type = body[cursor];
            let length_words = body[cursor + 1] as usize;
            let stage_bytes = length_words.saturating_mul(4);
            if stage_bytes < 4 || cursor + stage_bytes > body.len() {
                // length=0 would be an infinite loop; length over EOF
                // is corruption / padding. Either way, stop.
                break;
            }
            // Stages need ≥12 bytes to carry delay+duration+id (lotus
            // assumes 12; real schedulers also have 8-byte markers
            // with no payload). Only emit full stages; advance past
            // short ones so subsequent stages decode correctly.
            if stage_bytes >= 12 && cursor + 12 <= body.len() {
                let delay = u16::from_le_bytes([body[cursor + 4], body[cursor + 5]]);
                let duration = u16::from_le_bytes([body[cursor + 6], body[cursor + 7]]);
                let id = [
                    body[cursor + 8],
                    body[cursor + 9],
                    body[cursor + 10],
                    body[cursor + 11],
                ];
                let kind = StageKind::from_byte(raw_type);
                running_frame = running_frame.saturating_add(delay as u32);
                stages.push(TimedStage {
                    frame: running_frame,
                    stage: SchedulerStage {
                        kind,
                        raw_type,
                        delay_frames: delay,
                        duration_frames: duration,
                        id,
                    },
                });
            }
            cursor += stage_bytes;
        }
        Ok(Self { name, stages })
    }

    /// Frames at which a sound-effect event fires (either on caster
    /// or target), with the generator id and which side it plays on.
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

    /// Two stages: one motion at frame 0 (delay 0), one
    /// sound-on-caster at frame 30 (delay 30). Each stage is 12
    /// bytes = `length = 3` words.
    #[test]
    fn parses_motion_then_sound_caster() {
        let mut body = vec![0u8; SCHEDULER_HEADER_LEN];
        // Stage 1: motion, length=3, delay=0, duration=20, id=mot0
        body.extend_from_slice(&[0x05, 0x03, 0, 0]);
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&20u16.to_le_bytes());
        body.extend_from_slice(b"mot0");
        // Stage 2: sound-on-caster, length=3, delay=30, duration=1, id=snd0
        body.extend_from_slice(&[0x53, 0x03, 0, 0]);
        body.extend_from_slice(&30u16.to_le_bytes());
        body.extend_from_slice(&1u16.to_le_bytes());
        body.extend_from_slice(b"snd0");

        let s = Scheduler::parse(*b"sdam", &body).unwrap();
        assert_eq!(s.stages.len(), 2);
        assert_eq!(s.stages[0].stage.kind, StageKind::Motion);
        assert_eq!(s.stages[0].frame, 0);
        assert_eq!(s.stages[1].stage.kind, StageKind::SoundOnCaster);
        assert_eq!(s.stages[1].frame, 30);
        assert_eq!(&s.stages[1].stage.id, b"snd0");

        let snd: Vec<_> = s.sound_events().collect();
        assert_eq!(snd.len(), 1);
        assert!(snd[0].on_caster);
        assert_eq!(snd[0].frame, 30);
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
        // length=99 → way past end. Should stop, not panic.
        body.extend_from_slice(&[0x05, 99, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let s = Scheduler::parse(*b"trun", &body).unwrap();
        assert_eq!(s.stages.len(), 0);
    }
}
