use super::*;

/// s2c 0x05A GP_SERV_COMMAND_MOTIONMES — an emote broadcast. Body offsets
/// follow the PacketData struct in
/// vendor/server/src/map/packets/s2c/0x05a_motionmes.h: CasUniqueNo u32 @0,
/// TarUniqueNo u32 @4 (0 = untargeted), CasActIndex u16 @8, TarActIndex u16 @10,
/// MesNum u16 @12 (emote id; job emotes 74..=95), Param u16 @14, unknown14 u16
/// @16, Mode u8 @18, pad @19, FaithUniqueNo[5] u32 @20, FaithActIndex[5] u16
/// @40, pad u16 @50. The Faith arrays are /emotefaith trust replication ids —
/// decoded but unused until trusts exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotionMes {
    pub cas_unique_no: u32,
    pub tar_unique_no: u32,
    pub cas_act_index: u16,
    pub tar_act_index: u16,
    pub mes_num: u16,
    pub param: u16,
    pub mode: u8,
    pub faith: [(u32, u16); 5],
}

impl MotionMes {
    pub(crate) const CAS_UNIQUE_NO_OFFSET: usize = 0;
    pub(crate) const TAR_UNIQUE_NO_OFFSET: usize = 4;
    pub(crate) const CAS_ACT_INDEX_OFFSET: usize = 8;
    pub(crate) const TAR_ACT_INDEX_OFFSET: usize = 10;
    pub(crate) const MES_NUM_OFFSET: usize = 12;
    pub(crate) const PARAM_OFFSET: usize = 14;
    pub(crate) const MODE_OFFSET: usize = 18;
    pub(crate) const FAITH_UNIQUE_NO_OFFSET: usize = 20;
    pub(crate) const FAITH_ACT_INDEX_OFFSET: usize = 40;
    pub(crate) const FAITH_COUNT: usize = 5;
    pub(crate) const MIN_LEN: usize = 52;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::MIN_LEN {
            return Err(DecodeError::Truncated(Self::MIN_LEN, body.len()));
        }
        let rd32 = |o: usize| u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        let rd16 = |o: usize| u16::from_le_bytes([body[o], body[o + 1]]);
        let mut faith = [(0u32, 0u16); Self::FAITH_COUNT];
        for (i, f) in faith.iter_mut().enumerate() {
            *f = (
                rd32(Self::FAITH_UNIQUE_NO_OFFSET + i * 4),
                rd16(Self::FAITH_ACT_INDEX_OFFSET + i * 2),
            );
        }
        Ok(Self {
            cas_unique_no: rd32(Self::CAS_UNIQUE_NO_OFFSET),
            tar_unique_no: rd32(Self::TAR_UNIQUE_NO_OFFSET),
            cas_act_index: rd16(Self::CAS_ACT_INDEX_OFFSET),
            tar_act_index: rd16(Self::TAR_ACT_INDEX_OFFSET),
            mes_num: rd16(Self::MES_NUM_OFFSET),
            param: rd16(Self::PARAM_OFFSET),
            mode: body[Self::MODE_OFFSET],
            faith,
        })
    }

    pub fn targeted(&self) -> bool {
        self.tar_unique_no != 0
    }
}

/// s2c 0x11A GP_SERV_COMMAND_EMOTE_LIST — job-emote and chair unlock bitfields.
/// jobemotes_t u32 @0 (bit 0 = WAR .. bit 21 = RUN, i.e. bit = job id - 1),
/// chairemotes_t u16 @4, pad u16 @6
/// (vendor/server/src/map/packets/s2c/0x11a_emote_list.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmoteList {
    pub job_bits: u32,
    pub chair_bits: u16,
}

impl EmoteList {
    pub(crate) const JOB_BITS_OFFSET: usize = 0;
    pub(crate) const CHAIR_BITS_OFFSET: usize = 4;
    pub(crate) const MIN_LEN: usize = 6;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::MIN_LEN {
            return Err(DecodeError::Truncated(Self::MIN_LEN, body.len()));
        }
        Ok(Self {
            job_bits: u32::from_le_bytes([
                body[Self::JOB_BITS_OFFSET],
                body[Self::JOB_BITS_OFFSET + 1],
                body[Self::JOB_BITS_OFFSET + 2],
                body[Self::JOB_BITS_OFFSET + 3],
            ]),
            chair_bits: u16::from_le_bytes([
                body[Self::CHAIR_BITS_OFFSET],
                body[Self::CHAIR_BITS_OFFSET + 1],
            ]),
        })
    }

    /// jobemotes_t bit for `job_id` (1 = WAR .. 22 = RUN): bit = job id - 1.
    pub fn job_unlocked(&self, job_id: u8) -> bool {
        Self::job_bit_set(self.job_bits, job_id)
    }

    /// jobemotes_t spans 22 job bits (vendor/server/src/map/packets/s2c/0x11a_emote_list.h
    /// jobemotes_t); `checked_shr` bounds a wire-supplied job id so out-of-range
    /// values read as locked instead of overflowing the shift.
    pub fn job_bit_set(job_bits: u32, job_id: u8) -> bool {
        job_id >= 1
            && job_bits
                .checked_shr(u32::from(job_id) - 1)
                .is_some_and(|bits| bits & 1 == 1)
    }
}

#[cfg(test)]
mod motionmes_tests {
    use super::*;

    /// Body laid out field-by-field at the LSB struct offsets
    /// (vendor/server/src/map/packets/s2c/0x05a_motionmes.h).
    fn body() -> Vec<u8> {
        let mut b = vec![0u8; MotionMes::MIN_LEN];
        b[MotionMes::CAS_UNIQUE_NO_OFFSET..MotionMes::CAS_UNIQUE_NO_OFFSET + 4]
            .copy_from_slice(&0x0100_0F42u32.to_le_bytes());
        b[MotionMes::TAR_UNIQUE_NO_OFFSET..MotionMes::TAR_UNIQUE_NO_OFFSET + 4]
            .copy_from_slice(&0x0100_0F43u32.to_le_bytes());
        b[MotionMes::CAS_ACT_INDEX_OFFSET..MotionMes::CAS_ACT_INDEX_OFFSET + 2]
            .copy_from_slice(&0x0442u16.to_le_bytes());
        b[MotionMes::TAR_ACT_INDEX_OFFSET..MotionMes::TAR_ACT_INDEX_OFFSET + 2]
            .copy_from_slice(&0x0443u16.to_le_bytes());
        b[MotionMes::MES_NUM_OFFSET..MotionMes::MES_NUM_OFFSET + 2]
            .copy_from_slice(&8u16.to_le_bytes());
        b[MotionMes::PARAM_OFFSET..MotionMes::PARAM_OFFSET + 2]
            .copy_from_slice(&5u16.to_le_bytes());
        b[MotionMes::MODE_OFFSET] = crate::map::emote::mode::MOTION;
        b[MotionMes::FAITH_UNIQUE_NO_OFFSET..MotionMes::FAITH_UNIQUE_NO_OFFSET + 4]
            .copy_from_slice(&0x0100_0F50u32.to_le_bytes());
        b[MotionMes::FAITH_ACT_INDEX_OFFSET..MotionMes::FAITH_ACT_INDEX_OFFSET + 2]
            .copy_from_slice(&0x0450u16.to_le_bytes());
        b
    }

    #[test]
    fn decodes_all_fields_at_lsb_offsets() {
        let m = MotionMes::decode(&body()).expect("decode");
        assert_eq!(m.cas_unique_no, 0x0100_0F42);
        assert_eq!(m.tar_unique_no, 0x0100_0F43);
        assert_eq!(m.cas_act_index, 0x0442);
        assert_eq!(m.tar_act_index, 0x0443);
        assert_eq!(m.mes_num, 8);
        assert_eq!(m.param, 5);
        assert_eq!(m.mode, crate::map::emote::mode::MOTION);
        assert_eq!(m.faith[0], (0x0100_0F50, 0x0450));
        assert_eq!(m.faith[1], (0, 0));
        assert!(m.targeted());
    }

    /// The s2c Mode byte sits at 18 — after Param/unknown14 — unlike the c2s
    /// layout where Mode precedes Param; pin it so a transposition cannot pass.
    #[test]
    fn mode_offset_is_18_not_11() {
        let mut b = body();
        b[MotionMes::MODE_OFFSET] = crate::map::emote::mode::TEXT;
        b[11] = 0xEE;
        assert_eq!(
            MotionMes::decode(&b).unwrap().mode,
            crate::map::emote::mode::TEXT
        );
    }

    #[test]
    fn zero_target_is_untargeted() {
        let mut b = body();
        b[MotionMes::TAR_UNIQUE_NO_OFFSET..MotionMes::TAR_UNIQUE_NO_OFFSET + 4].fill(0);
        assert!(!MotionMes::decode(&b).unwrap().targeted());
    }

    #[test]
    fn truncated_body_is_error() {
        assert!(matches!(
            MotionMes::decode(&[0u8; MotionMes::MIN_LEN - 1]),
            Err(DecodeError::Truncated(n, have))
                if n == MotionMes::MIN_LEN && have == MotionMes::MIN_LEN - 1
        ));
    }
}

#[cfg(test)]
mod emote_list_tests {
    use super::*;

    #[test]
    fn decodes_bitfields_at_lsb_offsets() {
        // WAR(bit 0) + RUN(bit 21) unlocked; Chair1 + Chair11.
        let mut b = vec![0u8; 8];
        b[EmoteList::JOB_BITS_OFFSET..EmoteList::JOB_BITS_OFFSET + 4]
            .copy_from_slice(&((1u32 << 0) | (1 << 21)).to_le_bytes());
        b[EmoteList::CHAIR_BITS_OFFSET..EmoteList::CHAIR_BITS_OFFSET + 2]
            .copy_from_slice(&((1u16 << 0) | (1 << 10)).to_le_bytes());
        let e = EmoteList::decode(&b).expect("decode");
        assert_eq!(e.job_bits, (1 << 0) | (1 << 21));
        assert_eq!(e.chair_bits, (1 << 0) | (1 << 10));
        assert!(e.job_unlocked(1), "WAR = bit 0 (1-indexed job id)");
        assert!(e.job_unlocked(22), "RUN = bit 21");
        assert!(!e.job_unlocked(2));
        assert!(!e.job_unlocked(0), "job id 0 is NONE, never unlocked");
    }

    #[test]
    fn truncated_body_is_error() {
        assert!(matches!(
            EmoteList::decode(&[0u8; 5]),
            Err(DecodeError::Truncated(6, 5))
        ));
    }

    /// jobemotes_t is 22 bits wide (vendor/server/src/map/packets/s2c/0x11a_emote_list.h
    /// jobemotes_t); wire-supplied ids past the u32 width must read as locked,
    /// not overflow the shift.
    #[test]
    fn job_unlocked_bounds_out_of_range_job_id() {
        let e = EmoteList {
            job_bits: u32::MAX,
            chair_bits: 0,
        };
        assert!(e.job_unlocked(32), "bit 31 is still addressable");
        assert!(!e.job_unlocked(33), "shift past u32 width reads locked");
        assert!(!e.job_unlocked(u8::MAX));
        assert!(!EmoteList::job_bit_set(u32::MAX, 0));
    }
}
