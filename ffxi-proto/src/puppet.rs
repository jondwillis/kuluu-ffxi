//! s2c 0x044 GP_SERV_COMMAND_EXTENDED_JOB — the automaton (PUP) and monstrosity
//! (MON) extended-info frames. Layouts are read verbatim from the LSB packet
//! structs: vendor/server/src/map/packets/s2c/0x044_extended_job_pup.h,
//! vendor/server/src/map/packets/s2c/0x044_extended_job_mon.h; when they arrive
//! is lawed by SendExtendedJobPackets in vendor/server/src/map/utils/charutils.cpp.
//! Menu-side law: the Equipment-row variant gate at FFXiMain.dll retail-2026-09
//! RVA 0x1404D5 (vault main_menu.md §39).

/// `xi::Job::PUP` — vendor/server/data/enums/job.yaml (`pup: 18`). The same 18
/// is the byte the DLL's Equipment-row gate compares against
/// (FFXiMain.dll retail-2026-09 RVA 0x1404FB, `mov al, 0x12`).
pub const JOB_PUP: u8 = 18;

/// `xi::Job::MON` — vendor/server/data/enums/job.yaml (`mon: 23`; "MON is not a
/// full job"). The MON frame's Job byte (set in
/// vendor/server/src/map/packets/s2c/0x044_extended_job_mon.cpp).
pub const JOB_MON: u8 = 23;

// PUP PacketData offsets (vendor/server/src/map/packets/s2c/0x044_extended_job_pup.h):
// Job, IsSubJob, pad[2], Head, Frame, Attachments[12], unknown00[2],
// UnlockedHeads u32, UnlockedFrames u32, padding01[24], UnlockedAttachments u32 x8.
const PUP_OFF_HEAD: usize = 4;
const PUP_OFF_FRAME: usize = 5;
const PUP_OFF_ATTACHMENTS: usize = 6; // 12 bytes
const PUP_OFF_UNLOCKED_HEADS: usize = 20;
const PUP_OFF_UNLOCKED_FRAMES: usize = 24;
/// Name[16] through the packet's trailing fields sit past this length; frames
/// are only parsed to the unlocked masks, so any full-size frame qualifies.
const PUP_BODY_MIN_LEN: usize = 28;

// MON PacketData offsets (vendor/server/src/map/packets/s2c/0x044_extended_job_mon.h):
// Job, IsSubJob, pad[2], Species u16, padding02[2], EquippedInstincts u16 x12.
const MON_OFF_SPECIES: usize = 4;
const MON_OFF_INSTINCTS: usize = 8; // 12 words -> ends at 32
const MON_BODY_LEN: usize = 32;

/// What one s2c 0x044 body describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtendedJobFrame {
    Pup(PupExtended),
    Mon(MonExtended),
    /// Other extended jobs (BLU spell sets today) — recognized by Job byte but
    /// not modeled yet.
    #[allow(dead_code)]
    Other {
        job: u8,
    },
}

/// The automaton record from one PUP frame; field meanings are documented on the
/// folded snapshot type kuluu_snapshot::AutomatonInfo (vault main_menu.md §39).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PupExtended {
    pub is_sub_job: bool,
    /// `AutomatonHead` enum value (vendor/server/src/map/enums/automaton.h).
    pub head: u8,
    /// `AutomatonFrame` enum value (same header; 0x20..=0x23).
    pub frame: u8,
    /// One low-byte item id per slot in AutomatonSlot order after Head/Frame
    /// (vendor/server/src/map/packets/c2s/0x102_extended_job.h).
    pub attachments: [u8; 12],
    pub unlocked_heads: u32,
    pub unlocked_frames: u32,
}

/// The monstrosity record from one MON frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonExtended {
    pub species: u16,
    /// The packet's EquippedInstincts words (vendor/server/src/map/packets/
    /// s2c/0x044_extended_job_mon.cpp fills 12 from CMonstrosity).
    pub equipped_instincts: [u16; 12],
}

/// Classify and parse one EXTENDED_JOB subpacket body.
pub fn parse_extended_job(body: &[u8]) -> Option<ExtendedJobFrame> {
    let job = *body.first()?;
    match job {
        JOB_PUP if body.len() >= PUP_BODY_MIN_LEN => Some(ExtendedJobFrame::Pup(PupExtended {
            is_sub_job: body[1] != 0,
            head: body[PUP_OFF_HEAD],
            frame: body[PUP_OFF_FRAME],
            attachments: body[PUP_OFF_ATTACHMENTS..PUP_OFF_ATTACHMENTS + 12]
                .try_into()
                .ok()?,
            unlocked_heads: u32::from_le_bytes(
                body[PUP_OFF_UNLOCKED_HEADS..][..4].try_into().ok()?,
            ),
            unlocked_frames: u32::from_le_bytes(
                body[PUP_OFF_UNLOCKED_FRAMES..][..4].try_into().ok()?,
            ),
        })),
        JOB_MON if body.len() >= MON_BODY_LEN => {
            let mut equipped_instincts = [0u16; 12];
            for (i, w) in equipped_instincts.iter_mut().enumerate() {
                *w = u16::from_le_bytes(body[MON_OFF_INSTINCTS + i * 2..][..2].try_into().ok()?);
            }
            Some(ExtendedJobFrame::Mon(MonExtended {
                species: u16::from_le_bytes(body[MON_OFF_SPECIES..][..2].try_into().ok()?),
                equipped_instincts,
            }))
        }
        // A frame claiming one of the modeled jobs but too short to parse is
        // malformed — refuse rather than masquerade as an unmodeled job.
        JOB_PUP | JOB_MON => None,
        other => Some(ExtendedJobFrame::Other { job: other }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pup_frame() -> Vec<u8> {
        let mut body = vec![0u8; 256];
        body[0] = JOB_PUP;
        body[1] = 0; // main job
        body[PUP_OFF_HEAD] = 0x03; // AutomatonHead::Sharpshot (enums/automaton.h)
        body[PUP_OFF_FRAME] = 0x22; // AutomatonFrame::Sharpshot
        for (i, slot) in body
            .iter_mut()
            .enumerate()
            .take(PUP_OFF_ATTACHMENTS + 12)
            .skip(PUP_OFF_ATTACHMENTS)
        {
            *slot = i as u8; // 6..=17 -> distinct per-slot bytes for the assert
        }
        body[PUP_OFF_UNLOCKED_HEADS..][..4].copy_from_slice(&0x3Fu32.to_le_bytes());
        body[PUP_OFF_UNLOCKED_FRAMES..][..4].copy_from_slice(&0x0Fu32.to_le_bytes());
        body
    }

    #[test]
    fn pup_frame_parses_at_lsb_offsets() {
        let body = pup_frame();
        let frame = parse_extended_job(&body).unwrap();
        let ExtendedJobFrame::Pup(pup) = frame else {
            panic!("expected the PUP arm, got {frame:?}");
        };
        assert!(!pup.is_sub_job);
        assert_eq!(pup.head, 0x03);
        assert_eq!(pup.frame, 0x22);
        let expected: Vec<u8> = (6u8..18).collect();
        assert_eq!(pup.attachments.to_vec(), expected);
        assert_eq!(pup.unlocked_heads, 0x3F); // all six heads
        assert_eq!(pup.unlocked_frames, 0x0F); // all four frames
    }

    #[test]
    fn short_pup_frame_is_rejected_not_half_read() {
        let mut body = pup_frame();
        body.truncate(PUP_BODY_MIN_LEN - 1);
        assert!(parse_extended_job(&body).is_none());
    }

    #[test]
    fn mon_frame_parses_species_and_twelve_instinct_words() {
        let mut body = vec![0u8; MON_BODY_LEN];
        body[0] = JOB_MON;
        body[MON_OFF_SPECIES..][..2].copy_from_slice(&71u16.to_le_bytes());
        for i in 0..12usize {
            let off = MON_OFF_INSTINCTS + i * 2;
            body[off..][..2].copy_from_slice(&(i as u16 + 1).to_le_bytes());
        }
        let frame = parse_extended_job(&body).unwrap();
        let ExtendedJobFrame::Mon(mon) = frame else {
            panic!("expected the MON arm, got {frame:?}");
        };
        assert_eq!(mon.species, 71);
        assert_eq!(mon.equipped_instincts[0], 1);
        assert_eq!(mon.equipped_instincts[11], 12);
    }

    #[test]
    fn blu_frames_are_recognized_but_not_modeled() {
        let body = vec![16u8; 40]; // xi::Job::BLU = 16 (vendor/server/data/enums/job.yaml)
        assert_eq!(
            parse_extended_job(&body),
            Some(ExtendedJobFrame::Other { job: 16 })
        );
    }
}
