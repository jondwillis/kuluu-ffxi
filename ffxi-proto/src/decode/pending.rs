use super::DecodeError;

/// One s2c 0x05C `GP_SERV_PENDINGNUM`: int32 num[8] that the client copies
/// into its event Work_Zone buffer starting at index 2, where the event system
/// reads them as loop conditions (research/XiPackets/world/server/0x005C).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingNum {
    pub num: [i32; 8],
}

impl PendingNum {
    /// Body size after the 4-byte sub-header (the packet is 36 bytes on the
    /// wire, research/XiPackets/world/server/0x005C).
    pub(crate) const SIZE: usize = 8 * std::mem::size_of::<i32>();

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let mut num = [0i32; 8];
        for (slot, chunk) in num
            .iter_mut()
            .zip(body.chunks_exact(std::mem::size_of::<i32>()))
        {
            *slot = i32::from_le_bytes(chunk.try_into().unwrap());
        }
        Ok(Self { num })
    }
}

/// One s2c 0x05D `GP_SERV_PENDINGSTR` (repurposed): int32 num[9] the client
/// ignores plus four 16-byte strings copied into PTR_EventStrings, the table
/// the event VM's 0xB4 case 1 reads (research/XiPackets/world/server/0x005D).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingStr {
    pub strings: [[u8; 16]; 4],
}

impl PendingStr {
    /// Body size after the 4-byte sub-header: num[9] + four 16-byte strings.
    pub(crate) const SIZE: usize = 9 * std::mem::size_of::<i32>() + 4 * 16;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let nums = 9 * std::mem::size_of::<i32>();
        let mut strings = [[0u8; 16]; 4];
        for (slot, chunk) in strings.iter_mut().zip(body[nums..].chunks_exact(16)) {
            slot.copy_from_slice(chunk);
        }
        Ok(Self { strings })
    }
}

// vendor/server/src/map/packets/s2c/0x10e_reqsubmapnum.h GP_SERV_COMMAND_REQSUBMAPNUM::PacketData.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReqSubMapNum {
    pub map_num: u32,
}

impl ReqSubMapNum {
    pub(crate) const SIZE: usize = std::mem::size_of::<u32>();

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let map_num = u32::from_le_bytes(body[..Self::SIZE].try_into().unwrap());
        Ok(Self { map_num })
    }
}

// vendor/server/src/map/packets/s2c/0x0bf_registration.h GP_SERV_COMMAND_REGISTRATION::PacketData.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registration {
    pub result: u16,
    pub act_index: u32,
}

impl Registration {
    const SIZE: usize = 24;
    const RESULT_OFFSET: usize = 2;
    const ACT_INDEX_OFFSET: usize = 8;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            result: u16::from_le_bytes(
                body[Self::RESULT_OFFSET..Self::RESULT_OFFSET + 2]
                    .try_into()
                    .unwrap(),
            ),
            act_index: u32::from_le_bytes(
                body[Self::ACT_INDEX_OFFSET..Self::ACT_INDEX_OFFSET + 4]
                    .try_into()
                    .unwrap(),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_eight_little_endian_ints() {
        let mut body = [0u8; PendingNum::SIZE];
        for (i, slot) in body.chunks_exact_mut(4).enumerate() {
            slot.copy_from_slice(&(i as i32 * 1000 - 500).to_le_bytes());
        }
        let decoded = PendingNum::decode(&body).unwrap();
        for (i, value) in decoded.num.iter().enumerate() {
            assert_eq!(*value, i as i32 * 1000 - 500);
        }
    }

    #[test]
    fn rejects_a_truncated_body() {
        assert!(matches!(
            PendingNum::decode(&[0; PendingNum::SIZE - 1]),
            Err(DecodeError::Truncated(PendingNum::SIZE, _))
        ));
    }

    #[test]
    fn pending_str_decodes_the_four_strings_after_nine_ignored_ints() {
        let mut body = [0u8; PendingStr::SIZE];
        for i in 0..9 {
            body[i * 4..i * 4 + 4].copy_from_slice(&(i as i32).to_le_bytes());
        }
        let raw: [&[u8]; 4] = [b"alpha", b"beta", b"gamma", b"delta"];
        let strings: [[u8; 16]; 4] = raw.map(|s| {
            let mut slot = [0u8; 16];
            slot[..s.len()].copy_from_slice(s);
            slot
        });
        for (slot, chunk) in body[36..].chunks_exact_mut(16).zip(strings) {
            slot.copy_from_slice(&chunk);
        }
        let decoded = PendingStr::decode(&body).unwrap();
        assert_eq!(decoded.strings, strings);
    }

    #[test]
    fn pending_str_rejects_a_truncated_body() {
        assert!(matches!(
            PendingStr::decode(&[0; PendingStr::SIZE - 1]),
            Err(DecodeError::Truncated(PendingStr::SIZE, _))
        ));
    }

    #[test]
    fn reqsubmapnum_decodes_the_map_num() {
        let decoded = ReqSubMapNum::decode(&0xDEAD_BEEFu32.to_le_bytes()).unwrap();
        assert_eq!(decoded.map_num, 0xDEAD_BEEF);
        assert!(matches!(
            ReqSubMapNum::decode(&[0; ReqSubMapNum::SIZE - 1]),
            Err(DecodeError::Truncated(ReqSubMapNum::SIZE, _))
        ));
    }
}
