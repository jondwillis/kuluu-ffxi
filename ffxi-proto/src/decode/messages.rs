use super::*;

#[derive(Debug, Clone, Copy)]
pub struct SystemMessage {
    pub para: u32,
    pub para2: u32,
    pub message_id: u16,
}

impl SystemMessage {
    pub(crate) const SIZE: usize = 12;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            para: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            para2: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            message_id: u16::from_le_bytes(body[8..10].try_into().unwrap()),
        })
    }
}

/// s2c 0x02A GP_SERV_COMMAND_TALKNUMWORK — a zone-dialog message (LSB lua
/// `messageSpecial`) with up to 4 numeric parameters substituted into the
/// zone's dialog DAT entry `MesNum`.
/// vendor/server/src/map/packets/s2c/0x02a_talknumwork.h
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TalkNumWork {
    pub unique_no: u32,
    pub num: [i32; Self::NUM_COUNT],
    pub act_index: u16,
    pub mes_num: u16,
    pub kind: u8,
    pub flag: u8,
    pub name: [u8; Self::NAME_LEN],
}

impl TalkNumWork {
    pub(crate) const NUM_COUNT: usize = 4;
    pub const NAME_LEN: usize = 32;
    // vendor/server/src/map/packets/s2c/0x02a_talknumwork.cpp GP_SERV_COMMAND_TALKNUMWORK
    pub const SHORT_SIZE: usize = 28;
    pub const SIZE: usize = 4 + Self::NUM_COUNT * 4 + 2 + 2 + 1 + 1 + Self::NAME_LEN;
    /// Added to MesNum when the sender is a PC and ShowName is false — the
    /// dialog index is the low 15 bits.
    /// vendor/server/src/map/packets/s2c/0x02a_talknumwork.cpp
    pub const MESNUM_HIDE_NAME_FLAG: u16 = super::MESNUM_HIDE_NAME_FLAG;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE && body.len() != Self::SHORT_SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let mut num = [0i32; Self::NUM_COUNT];
        for (i, n) in num.iter_mut().enumerate() {
            let o = 4 + i * 4;
            *n = i32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        }
        let mut name = [0; Self::NAME_LEN];
        if body.len() >= Self::SIZE {
            name.copy_from_slice(&body[26..26 + Self::NAME_LEN]);
        }
        Ok(Self {
            unique_no: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            num,
            act_index: u16::from_le_bytes([body[20], body[21]]),
            mes_num: u16::from_le_bytes([body[22], body[23]]),
            kind: body[24],
            flag: body[25],
            name,
        })
    }

    pub fn message_index(&self) -> u16 {
        self.mes_num & !Self::MESNUM_HIDE_NAME_FLAG
    }

    pub fn hide_name(&self) -> bool {
        self.mes_num & Self::MESNUM_HIDE_NAME_FLAG != 0
    }

    pub fn speaker_name(&self) -> Option<String> {
        let end = self
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(Self::NAME_LEN);
        (end > 0).then(|| String::from_utf8_lossy(&self.name[..end]).into_owned())
    }
}

/// Retail's `Type` → chat-mode lookup, shared by the whole TALKNUM family.
/// Every member indexes the same 8-entry table and falls back to entry 0 for
/// anything `>= 8`. research/XiPackets/world/server/0x0036/README.md (identical
/// tables documented under 0x0027, 0x002A and 0x0043).
const TALK_CHAT_MODES: [u8; 8] = [0x8E, 0xA1, 0x90, 0x91, 0x92, 0xA1, 0x94, 0x95];

pub fn talk_chat_mode(ty: u8) -> u8 {
    TALK_CHAT_MODES
        .get(usize::from(ty))
        .copied()
        .unwrap_or(TALK_CHAT_MODES[0])
}

/// Added to `MesNum` when the entity name must not be prefixed to the message;
/// the dialog index is the low 15 bits. Shared by 0x027/0x02A/0x036/0x043.
pub const MESNUM_HIDE_NAME_FLAG: u16 = 0x8000;

/// s2c 0x036 GP_SERV_COMMAND_TALKNUM — a zone-dialog message with no
/// parameters. LSB sends it for most fishing outcomes (line break, rod break,
/// lost catch, hook feelings).
/// vendor/server/src/map/packets/s2c/0x036_talknum.h
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TalkNum {
    pub unique_no: u32,
    pub act_index: u16,
    pub mes_num: u16,
    pub kind: u8,
}

impl TalkNum {
    pub const SIZE: usize = 12;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            unique_no: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            act_index: u16::from_le_bytes([body[4], body[5]]),
            mes_num: u16::from_le_bytes([body[6], body[7]]),
            kind: body[8],
        })
    }

    pub fn message_index(&self) -> u16 {
        self.mes_num & !MESNUM_HIDE_NAME_FLAG
    }

    pub fn hide_name(&self) -> bool {
        self.mes_num & MESNUM_HIDE_NAME_FLAG != 0
    }

    pub fn chat_mode(&self) -> u8 {
        talk_chat_mode(self.kind)
    }
}

/// s2c 0x027 GP_SERV_COMMAND_TALKNUMWORK2 — a zone-dialog message with two
/// banks of numeric parameters and two string parameters. LSB's fishing
/// constructor puts the caught item id in `num1[0]`, the stack count in
/// `num1[1]`, and the angler's name in `string1`.
/// vendor/server/src/map/packets/s2c/0x027_talknumwork2.h
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TalkNumWork2 {
    pub unique_no: u32,
    pub act_index: u16,
    pub mes_num: u16,
    pub kind: u16,
    pub flags: u8,
    pub num1: [i32; Self::NUM1_COUNT],
    pub name1: [u8; Self::NAME1_LEN],
    pub name2: [u8; Self::NAME2_LEN],
    pub num2: [i32; Self::NUM2_COUNT],
}

impl TalkNumWork2 {
    pub const NUM1_COUNT: usize = 4;
    pub const NUM2_COUNT: usize = 8;
    pub const NAME1_LEN: usize = 32;
    pub const NAME2_LEN: usize = 16;
    const NUM1_OFF: usize = 12;
    const NAME1_OFF: usize = Self::NUM1_OFF + Self::NUM1_COUNT * 4;
    const NAME2_OFF: usize = Self::NAME1_OFF + Self::NAME1_LEN;
    const NUM2_OFF: usize = Self::NAME2_OFF + Self::NAME2_LEN;
    pub const SIZE: usize = Self::NUM2_OFF + Self::NUM2_COUNT * 4;

    /// `flags` bit 0: use the entity name looked up from `unique_no` rather
    /// than `string1`. Bit 1: `string2` overrides the name when `MesNum`'s
    /// hide-name flag is set. research/XiPackets/world/server/0x0027/README.md.
    pub const FLAG_LOOKUP_NAME: u8 = 1;
    pub const FLAG_STRING2_NAME: u8 = 2;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let rd_i32 = |o: usize| i32::from_le_bytes(body[o..o + 4].try_into().unwrap());
        let mut num1 = [0i32; Self::NUM1_COUNT];
        for (i, n) in num1.iter_mut().enumerate() {
            *n = rd_i32(Self::NUM1_OFF + i * 4);
        }
        let mut num2 = [0i32; Self::NUM2_COUNT];
        for (i, n) in num2.iter_mut().enumerate() {
            *n = rd_i32(Self::NUM2_OFF + i * 4);
        }
        Ok(Self {
            unique_no: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            act_index: u16::from_le_bytes([body[4], body[5]]),
            mes_num: u16::from_le_bytes([body[6], body[7]]),
            kind: u16::from_le_bytes([body[8], body[9]]),
            flags: body[10],
            num1,
            name1: body[Self::NAME1_OFF..Self::NAME1_OFF + Self::NAME1_LEN]
                .try_into()
                .unwrap(),
            name2: body[Self::NAME2_OFF..Self::NAME2_OFF + Self::NAME2_LEN]
                .try_into()
                .unwrap(),
            num2,
        })
    }

    pub fn message_index(&self) -> u16 {
        self.mes_num & !MESNUM_HIDE_NAME_FLAG
    }

    pub fn hide_name(&self) -> bool {
        self.mes_num & MESNUM_HIDE_NAME_FLAG != 0
    }

    pub fn chat_mode(&self) -> u8 {
        talk_chat_mode(u8::try_from(self.kind).unwrap_or(u8::MAX))
    }

    /// The name the message is about, independent of whether retail would also
    /// print it as a `Name : ` prefix. LSB's fishing constructor always sets the
    /// hide-name flag and still fills `string1` with the angler, because the
    /// dialog string embeds the name itself.
    pub fn actor_name(&self) -> Option<String> {
        cstr(&self.name1).or_else(|| cstr(&self.name2))
    }
}

/// s2c 0x043 GP_SERV_COMMAND_TALKNUMNAME — a zone-dialog message carrying one
/// name. LSB uses it for "\<Player\> caught a monster!" and the fished-up chest.
/// vendor/server/src/map/packets/s2c/0x043_talknumname.h
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TalkNumName {
    pub unique_no: u32,
    pub act_index: u16,
    pub mes_num: u16,
    pub kind: u8,
    pub name: [u8; Self::NAME_LEN],
}

impl TalkNumName {
    pub const NAME_LEN: usize = 16;
    const NAME_OFF: usize = 12;
    pub const SIZE: usize = Self::NAME_OFF + Self::NAME_LEN;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            unique_no: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            act_index: u16::from_le_bytes([body[4], body[5]]),
            mes_num: u16::from_le_bytes([body[6], body[7]]),
            kind: body[8],
            name: body[Self::NAME_OFF..Self::NAME_OFF + Self::NAME_LEN]
                .try_into()
                .unwrap(),
        })
    }

    pub fn message_index(&self) -> u16 {
        self.mes_num & !MESNUM_HIDE_NAME_FLAG
    }

    pub fn hide_name(&self) -> bool {
        self.mes_num & MESNUM_HIDE_NAME_FLAG != 0
    }

    pub fn chat_mode(&self) -> u8 {
        talk_chat_mode(self.kind)
    }

    pub fn actor_name(&self) -> Option<String> {
        cstr(&self.name)
    }
}

fn cstr(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    (end > 0).then(|| String::from_utf8_lossy(&bytes[..end]).into_owned())
}

#[cfg(test)]
mod talk_num_family_tests {
    use super::*;

    #[test]
    fn talk_num_decodes_at_lsb_offsets() {
        // UniqueNo u32 @0, ActIndex u16 @4, MesNum u16 @6, Type u8 @8.
        let mut b = vec![0u8; TalkNum::SIZE];
        b[0..4].copy_from_slice(&0x0100_0042u32.to_le_bytes());
        b[4..6].copy_from_slice(&0x0042u16.to_le_bytes());
        b[6..8].copy_from_slice(&(7113u16 | MESNUM_HIDE_NAME_FLAG).to_le_bytes());
        b[8] = 2;

        let t = TalkNum::decode(&b).unwrap();
        assert_eq!(t.unique_no, 0x0100_0042);
        assert_eq!(t.act_index, 0x0042);
        assert_eq!(t.message_index(), 7113);
        assert!(t.hide_name());
        assert_eq!(t.chat_mode(), 0x90);

        assert!(matches!(
            TalkNum::decode(&b[..TalkNum::SIZE - 1]),
            Err(DecodeError::Truncated(n, _)) if n == TalkNum::SIZE
        ));
    }

    /// Pins the layout LSB's fishing catch constructor writes:
    /// `MesNum = messageID + 0x8000`, `Num1[0] = fish id`, `Num1[1] = count`,
    /// `String1 = angler`. vendor/server/src/map/packets/s2c/0x027_talknumwork2.cpp
    #[test]
    fn talk_num_work2_decodes_a_fishing_catch() {
        let mut b = vec![0u8; TalkNumWork2::SIZE];
        b[0..4].copy_from_slice(&0x0100_0007u32.to_le_bytes());
        b[4..6].copy_from_slice(&9u16.to_le_bytes());
        b[6..8].copy_from_slice(&(7267u16 | MESNUM_HIDE_NAME_FLAG).to_le_bytes());
        b[8..10].copy_from_slice(&0u16.to_le_bytes());
        b[10] = 0;
        b[12..16].copy_from_slice(&4304i32.to_le_bytes()); // moat carp
        b[16..20].copy_from_slice(&1i32.to_le_bytes());
        b[28..28 + 5].copy_from_slice(b"Kuluu");

        let t = TalkNumWork2::decode(&b).unwrap();
        assert_eq!(t.message_index(), 7267);
        assert!(t.hide_name());
        assert_eq!(t.num1, [4304, 1, 0, 0]);
        assert_eq!(t.actor_name().as_deref(), Some("Kuluu"));
        assert_eq!(t.chat_mode(), 0x8E);
        assert_eq!(
            TalkNumWork2::SIZE,
            0x6C,
            "0x0070 packet minus the 4-byte header"
        );
    }

    #[test]
    fn talk_num_work2_falls_back_to_string2_for_the_name() {
        let mut b = vec![0u8; TalkNumWork2::SIZE];
        b[60..60 + 6].copy_from_slice(b"Trion\0");
        let t = TalkNumWork2::decode(&b).unwrap();
        assert_eq!(t.actor_name().as_deref(), Some("Trion"));
    }

    #[test]
    fn talk_num_name_decodes_at_lsb_offsets() {
        let mut b = vec![0u8; TalkNumName::SIZE];
        b[0..4].copy_from_slice(&0x0100_0007u32.to_le_bytes());
        b[6..8].copy_from_slice(&7269u16.to_le_bytes());
        b[8] = 9; // >= 8 falls back to mode 0
        b[12..12 + 5].copy_from_slice(b"Kuluu");

        let t = TalkNumName::decode(&b).unwrap();
        assert_eq!(t.message_index(), 7269);
        assert!(!t.hide_name());
        assert_eq!(t.actor_name().as_deref(), Some("Kuluu"));
        assert_eq!(t.chat_mode(), 0x8E, "out-of-table Type defaults to entry 0");
    }

    #[test]
    fn chat_mode_table_matches_the_documented_lookup() {
        let want = [0x8E, 0xA1, 0x90, 0x91, 0x92, 0xA1, 0x94, 0x95];
        for (ty, mode) in want.iter().enumerate() {
            assert_eq!(talk_chat_mode(ty as u8), *mode, "Type {ty}");
        }
        for ty in 8..=u8::MAX {
            assert_eq!(talk_chat_mode(ty), 0x8E, "Type {ty} defaults to entry 0");
        }
    }
}

#[cfg(test)]
mod talk_num_work_tests {
    use super::*;

    // Field offsets pinned against vendor/server/src/map/packets/s2c/
    // 0x02a_talknumwork.h: UniqueNo u32 @0, num i32[4] @4, ActIndex u16 @20,
    // MesNum u16 @22, Type u8 @24, Flag u8 @25, String u8[32] @26.
    fn body() -> Vec<u8> {
        let mut b = vec![0u8; TalkNumWork::SIZE];
        b[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        b[4..8].copy_from_slice(&512i32.to_le_bytes());
        b[8..12].copy_from_slice(&(-7i32).to_le_bytes());
        b[20..22].copy_from_slice(&0x0042u16.to_le_bytes());
        b[22..24].copy_from_slice(&(6438u16 | TalkNumWork::MESNUM_HIDE_NAME_FLAG).to_le_bytes());
        b[24] = 3;
        b[25] = 1;
        b[26..26 + 5].copy_from_slice(b"Trion");
        b
    }

    #[test]
    fn decodes_all_fields_at_lsb_offsets() {
        let t = TalkNumWork::decode(&body()).expect("decode");
        assert_eq!(t.unique_no, 0xDEAD_BEEF);
        assert_eq!(t.num, [512, -7, 0, 0]);
        assert_eq!(t.act_index, 0x0042);
        assert_eq!(t.mes_num, 6438 | TalkNumWork::MESNUM_HIDE_NAME_FLAG);
        assert_eq!(t.kind, 3);
        assert_eq!(t.flag, 1);
        assert_eq!(t.speaker_name().as_deref(), Some("Trion"));
    }

    #[test]
    fn decodes_short_typed_message_without_a_name() {
        let mut short = body();
        short.truncate(TalkNumWork::SHORT_SIZE);
        short[26..].fill(0);
        let decoded = TalkNumWork::decode(&short).unwrap();
        let full = TalkNumWork::decode(&body()).unwrap();
        assert_eq!(decoded.num, full.num);
        assert_eq!(decoded.mes_num, full.mes_num);
        assert_eq!(decoded.kind, full.kind);
        assert_eq!(decoded.speaker_name(), None);
        short.pop();
        assert!(TalkNumWork::decode(&short).is_err());
    }

    #[test]
    fn message_index_masks_the_hide_name_flag() {
        let t = TalkNumWork::decode(&body()).expect("decode");
        assert_eq!(t.message_index(), 6438);
        assert!(t.hide_name());

        let mut plain = body();
        plain[22..24].copy_from_slice(&6438u16.to_le_bytes());
        let t = TalkNumWork::decode(&plain).expect("decode");
        assert_eq!(t.message_index(), 6438);
        assert!(!t.hide_name());
    }

    #[test]
    fn empty_name_is_none() {
        let mut b = body();
        b[26..26 + TalkNumWork::NAME_LEN].fill(0);
        assert_eq!(TalkNumWork::decode(&b).unwrap().speaker_name(), None);
    }

    #[test]
    fn truncated_body_is_error() {
        let buf = vec![0u8; TalkNumWork::SIZE - 1];
        assert!(matches!(
            TalkNumWork::decode(&buf),
            Err(DecodeError::Truncated(n, have)) if n == TalkNumWork::SIZE && have == TalkNumWork::SIZE - 1
        ));
    }
}

#[cfg(test)]
mod system_message_tests {
    use super::*;

    #[test]
    fn system_message_decodes() {
        let mut buf = vec![0u8; SystemMessage::SIZE];
        buf[0..4].copy_from_slice(&30u32.to_le_bytes());
        buf[4..8].copy_from_slice(&0u32.to_le_bytes());
        buf[8..10].copy_from_slice(&7u16.to_le_bytes());
        let m = SystemMessage::decode(&buf).unwrap();
        assert_eq!(m.para, 30);
        assert_eq!(m.para2, 0);
        assert_eq!(m.message_id, 7);
    }

    #[test]
    fn system_message_truncated_errors() {
        let buf = vec![0u8; SystemMessage::SIZE - 1];
        assert!(matches!(
            SystemMessage::decode(&buf),
            Err(DecodeError::Truncated(12, _))
        ));
    }
}
