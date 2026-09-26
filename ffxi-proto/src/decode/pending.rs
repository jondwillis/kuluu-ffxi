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

/// One s2c 0x10E `GP_SERV_COMMAND_REQSUBMAPNUM`: uint32 MapNum, the answer to
/// the event VM's 0xA6 case 0 request (c2s 0x0EB). The server pushes 0 when
/// the char is npc-locked and nothing otherwise
/// (vendor/server/src/map/packets/s2c/0x10e_reqsubmapnum.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReqSubMapNum {
    pub map_num: u32,
}

impl ReqSubMapNum {
    /// Body size after the 4-byte sub-header: one uint32 MapNum.
    pub(crate) const SIZE: usize = std::mem::size_of::<u32>();

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let map_num = u32::from_le_bytes(body[..Self::SIZE].try_into().unwrap());
        Ok(Self { map_num })
    }
}

/// One s2c 0x059 `GP_SERV_COMMAND_FRIENDPASS`, the answer to the event VM's
/// 0x87/0x88 world-pass request (c2s 0x01B): int32 leftNum/leftDays/passPop,
/// the pass number as a 10-digit zero-padded String[16], and the Type/unknown
/// bytes the server's constructor fills (vendor/server/src/map/packets/s2c/
/// 0x059_friendpass.h). kuluu keeps the fields to clear the event's await;
/// the pass number has no display here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FriendPass {
    pub left_num: i32,
    pub left_days: i32,
    pub pass_pop: i32,
    pub string: [u8; 16],
    pub type_byte: u8,
}

impl FriendPass {
    /// Body size after the 4-byte sub-header: 3 + 16 + 1 + 1 + 2 bytes.
    pub(crate) const SIZE: usize =
        3 * std::mem::size_of::<i32>() + 16 + 2 + std::mem::size_of::<u16>();

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let left_num = i32::from_le_bytes(body[0..4].try_into().unwrap());
        let left_days = i32::from_le_bytes(body[4..8].try_into().unwrap());
        let pass_pop = i32::from_le_bytes(body[8..12].try_into().unwrap());
        let mut string = [0u8; 16];
        string.copy_from_slice(&body[12..28]);
        Ok(Self {
            left_num,
            left_days,
            pass_pop,
            string,
            type_byte: body[28],
        })
    }
}

/// One s2c 0x031 `GP_SERV_COMMAND_RECIPE`, the answer to the event VM's 0x8C
/// crafting-support request (c2s 0x058): the 48-byte union of the recipe
/// details (Type 1/3) and the 16-entry recipe list (Type 2), whose
/// GP_SERV_COMMAND_RECIPE_TYPE word both arms share at byte 44
/// (vendor/server/src/map/packets/s2c/0x031_recipe.h). kuluu keeps the
/// discriminator and the union's item words to clear the event's await; the
/// crafting menu that would read them is not this round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recipe {
    /// The GP_SERV_COMMAND_RECIPE_TYPE word (byte 44): 1 detail, 2 list, 3
    /// detail at offset (vendor/server/src/map/packets/s2c/0x031_recipe.h).
    pub type_word: u16,
    /// Type1_3.productitem: the recipe's result item; Type2's unused04[0].
    pub product_item: u16,
    /// The union's middle 16 words: Type1_3's itemnum[8] + itemcount[8],
    /// Type2's itemnum[16].
    pub items: [u16; 16],
}

impl Recipe {
    /// Body size after the 4-byte sub-header: 24 u16 words.
    pub(crate) const SIZE: usize = 24 * std::mem::size_of::<u16>();

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let mut items = [0u16; 16];
        for (slot, chunk) in items.iter_mut().zip(body[12..44].chunks_exact(2)) {
            *slot = u16::from_le_bytes(chunk.try_into().unwrap());
        }
        Ok(Self {
            type_word: u16::from_le_bytes(body[44..46].try_into().unwrap()),
            product_item: u16::from_le_bytes(body[0..2].try_into().unwrap()),
            items,
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

    #[test]
    fn recipe_decodes_the_type_word_and_item_words() {
        let mut body = [0u8; Recipe::SIZE];
        body[0..2].copy_from_slice(&0x0123u16.to_le_bytes());
        body[12..14].copy_from_slice(&0x0456u16.to_le_bytes());
        body[44..46].copy_from_slice(&2u16.to_le_bytes());
        let decoded = Recipe::decode(&body).unwrap();
        assert_eq!(decoded.type_word, 2);
        assert_eq!(decoded.product_item, 0x0123);
        assert_eq!(decoded.items[0], 0x0456);
        assert_eq!(decoded.items[1], 0);
        assert!(matches!(
            Recipe::decode(&[0; Recipe::SIZE - 1]),
            Err(DecodeError::Truncated(Recipe::SIZE, _))
        ));
    }

    #[test]
    fn friendpass_decodes_the_pass_fields() {
        let mut body = [0u8; FriendPass::SIZE];
        body[0..4].copy_from_slice(&1i32.to_le_bytes());
        body[4..8].copy_from_slice(&167i32.to_le_bytes());
        body[8..12].copy_from_slice(&10000i32.to_le_bytes());
        body[12..22].copy_from_slice(b"0000123456");
        body[28] = 0x06;
        let decoded = FriendPass::decode(&body).unwrap();
        assert_eq!(decoded.left_num, 1);
        assert_eq!(decoded.left_days, 167);
        assert_eq!(decoded.pass_pop, 10000);
        assert_eq!(&decoded.string[..10], b"0000123456");
        assert_eq!(decoded.type_byte, 0x06);
        assert!(matches!(
            FriendPass::decode(&[0; FriendPass::SIZE - 1]),
            Err(DecodeError::Truncated(FriendPass::SIZE, _))
        ));
    }
}
