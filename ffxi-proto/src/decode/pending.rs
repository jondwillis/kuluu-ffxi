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

    #[test]
    fn friendpass_preserves_signed_extremes_and_all_string_bytes() {
        // vendor/server/src/map/packets/s2c/0x059_friendpass.h GP_SERV_COMMAND_FRIENDPASS::PacketData.
        const LSB_PINNED_PASS_STRING: &[u8; 16] = b"0123456789ABCDEF";
        const LSB_PINNED_TYPE: u8 = 0xFE;
        const LSB_PINNED_TRAILER: [u8; 4] = [LSB_PINNED_TYPE, 0xA5, 0x5A, 0x5A];
        let body = [
            (-1i32).to_le_bytes().as_slice(),
            i32::MIN.to_le_bytes().as_slice(),
            i32::MAX.to_le_bytes().as_slice(),
            LSB_PINNED_PASS_STRING.as_slice(),
            LSB_PINNED_TRAILER.as_slice(),
        ]
        .concat();

        let decoded = FriendPass::decode(&body).unwrap();
        assert_eq!(decoded.left_num, -1);
        assert_eq!(decoded.left_days, i32::MIN);
        assert_eq!(decoded.pass_pop, i32::MAX);
        assert_eq!(&decoded.string, LSB_PINNED_PASS_STRING);
        assert_eq!(decoded.type_byte, LSB_PINNED_TYPE);
    }

    #[test]
    fn recipe_detail_keeps_ingredients_before_their_counts() {
        // vendor/server/src/map/packets/s2c/0x031_recipe.h GP_SERV_COMMAND_RECIPE_TYPE1_3.
        const LSB_PINNED_DETAIL_PREFIX: [u16; 6] = [0x1111, 0x2222, 0x3333, 0x4444, 0x5555, 0x6666];
        const LSB_PINNED_INGREDIENTS: [u16; 8] = [
            0x8101, 0x8102, 0x8103, 0x8104, 0x8105, 0x8106, 0x8107, 0x8108,
        ];
        const LSB_PINNED_COUNTS: [u16; 8] = [
            0x0101, 0x0102, 0x0103, 0x0104, 0x0105, 0x0106, 0x0107, 0x0108,
        ];
        const LSB_PINNED_DETAIL_TYPES: [u16; 2] = [1, 3];
        const LSB_PINNED_UNKNOWN: u16 = 0xBEEF;

        for type_word in LSB_PINNED_DETAIL_TYPES {
            let body: Vec<_> = LSB_PINNED_DETAIL_PREFIX
                .into_iter()
                .chain(LSB_PINNED_INGREDIENTS)
                .chain(LSB_PINNED_COUNTS)
                .chain([type_word, LSB_PINNED_UNKNOWN])
                .flat_map(u16::to_le_bytes)
                .collect();
            let decoded = Recipe::decode(&body).unwrap();
            assert_eq!(decoded.product_item, LSB_PINNED_DETAIL_PREFIX[0]);
            assert_eq!(decoded.type_word, type_word);
            assert_eq!(
                decoded.items.as_slice(),
                [LSB_PINNED_INGREDIENTS, LSB_PINNED_COUNTS]
                    .concat()
                    .as_slice()
            );
        }
    }

    #[test]
    fn recipe_list_keeps_all_ids_separate_from_next_page() {
        // vendor/server/src/map/packets/s2c/0x031_recipe.h GP_SERV_COMMAND_RECIPE_TYPE2.
        const LSB_PINNED_UNUSED: [u16; 6] = [0xA101, 0xA102, 0xA103, 0xA104, 0xA105, 0xA106];
        const LSB_PINNED_LIST: [u16; 16] = [
            0x9101, 0x9102, 0x9103, 0x9104, 0x9105, 0x9106, 0x9107, 0x9108, 0x9109, 0x910A, 0x910B,
            0x910C, 0x910D, 0x910E, 0x910F, 0x9110,
        ];
        const LSB_PINNED_LIST_TYPE: u16 = 2;
        const LSB_PINNED_NEXT_PAGE: u16 = 0x9111;
        let body: Vec<_> = LSB_PINNED_UNUSED
            .into_iter()
            .chain(LSB_PINNED_LIST)
            .chain([LSB_PINNED_LIST_TYPE, LSB_PINNED_NEXT_PAGE])
            .flat_map(u16::to_le_bytes)
            .collect();

        let decoded = Recipe::decode(&body).unwrap();
        assert_eq!(decoded.type_word, LSB_PINNED_LIST_TYPE);
        assert_eq!(decoded.items, LSB_PINNED_LIST);
    }
}
