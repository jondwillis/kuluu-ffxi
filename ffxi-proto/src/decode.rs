pub mod animation {

    pub const NONE: u8 = 0;

    pub const ATTACK: u8 = 1;

    pub const HEALING: u8 = 33;

    pub const SIT: u8 = 47;
}

use crate::framing::SubPacket;

#[derive(Debug, Clone, Copy)]
pub struct PosHead {
    pub unique_no: u32,

    pub act_index: u16,

    pub send_flag: u8,

    pub dir: u8,

    pub x: f32,

    pub z: f32,

    pub y: f32,

    pub flags0: u32,

    pub speed: u8,

    pub speed_base: u8,

    pub hpp: u8,

    pub server_status: u8,
    pub flags1: u32,
    pub flags2: u32,
    pub flags3: u32,

    pub bt_target_id: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("expected at least {0} bytes, have {1}")]
    Truncated(usize, usize),
    #[error("opcode 0x{got:03x} does not match expected 0x{expected:03x}")]
    OpcodeMismatch { expected: u16, got: u16 },
}

impl PosHead {
    pub const SIZE: usize = 40;

    pub const SIZE_WITH_BT_TARGET: usize = 44;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let bt_target_id = if body.len() >= Self::SIZE_WITH_BT_TARGET {
            u32::from_le_bytes(body[40..44].try_into().unwrap())
        } else {
            0
        };
        Ok(Self {
            unique_no: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            act_index: u16::from_le_bytes(body[4..6].try_into().unwrap()),
            send_flag: body[6],
            dir: body[7],
            x: f32::from_le_bytes(body[8..12].try_into().unwrap()),
            z: f32::from_le_bytes(body[12..16].try_into().unwrap()),
            y: f32::from_le_bytes(body[16..20].try_into().unwrap()),
            flags0: u32::from_le_bytes(body[20..24].try_into().unwrap()),
            speed: body[24],
            speed_base: body[25],
            hpp: body[26],
            server_status: body[27],
            flags1: u32::from_le_bytes(body[28..32].try_into().unwrap()),
            flags2: u32::from_le_bytes(body[32..36].try_into().unwrap()),
            flags3: u32::from_le_bytes(body[36..40].try_into().unwrap()),
            bt_target_id,
        })
    }

    pub fn decode_char_npc(body: &[u8]) -> Result<(Self, u32), DecodeError> {
        let head = Self::decode(body)?;

        let claim_id = if body.len() >= Self::SIZE_WITH_BT_TARGET {
            u32::from_le_bytes(body[40..44].try_into().unwrap())
        } else {
            0
        };
        Ok((head, claim_id))
    }

    pub const UPDATE_DESPAWN: u8 = 0x20;

    pub fn is_entity_despawn(opcode: u16, body: &[u8]) -> bool {
        use crate::map::s2c;
        (opcode == s2c::CHAR_PC || opcode == s2c::CHAR_NPC)
            && body
                .get(6)
                .copied()
                .is_some_and(|mask| mask & Self::UPDATE_DESPAWN != 0)
    }

    pub fn try_extract_name(opcode: u16, body: &[u8]) -> Option<String> {
        use crate::map::s2c;

        const NAME_FLAG: u8 = 0x08;
        if body.len() < 7 || body[6] & NAME_FLAG == 0 {
            return None;
        }
        let slot: &[u8] = if opcode == s2c::CHAR_PC {
            const NAME_START: usize = 0x56;
            if body.len() <= NAME_START {
                return None;
            }
            &body[NAME_START..]
        } else if opcode == s2c::CHAR_NPC {
            const STANDARD_START: usize = 0x30;
            const RENAMED_START: usize = 0x31;
            if body.len() <= STANDARD_START {
                return None;
            }
            let start = if body[STANDARD_START] == 0x01 {
                RENAMED_START
            } else {
                STANDARD_START
            };
            if body.len() <= start {
                return None;
            }
            let end = body.len().min(start + 16);
            &body[start..end]
        } else {
            return None;
        };
        let n = slot.iter().position(|&b| b == 0).unwrap_or(slot.len());

        if n < 3 {
            return None;
        }
        let name_bytes = &slot[..n];
        if !name_bytes.iter().all(|&b| (0x20..=0x7E).contains(&b)) {
            return None;
        }
        Some(String::from_utf8_lossy(name_bytes).into_owned())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookData {
    Standard {
        modelid: u16,
    },

    Equipped {
        face: u8,

        race: u8,
        head: u16,
        body: u16,
        hands: u16,
        legs: u16,
        feet: u16,
        main: u16,
        sub: u16,
        ranged: u16,
    },

    Door {
        size: u16,
    },

    Transport {
        size: u16,
    },
}

impl LookData {
    pub const LOOK_BODY_OFFSET: usize = 0x2C;

    pub fn decode_char_npc(body: &[u8]) -> Option<Self> {
        let off = Self::LOOK_BODY_OFFSET;
        if body.len() < off + 4 {
            return None;
        }
        let size = u16::from_le_bytes([body[off], body[off + 1]]);

        match size {
            0 | 5 | 6 => {
                let modelid = u16::from_le_bytes([body[off + 2], body[off + 3]]);
                Some(LookData::Standard { modelid })
            }
            1 | 7 => {
                if body.len() < off + 20 {
                    return None;
                }
                Some(LookData::Equipped {
                    face: body[off + 2],
                    race: body[off + 3],
                    head: u16::from_le_bytes([body[off + 4], body[off + 5]]),
                    body: u16::from_le_bytes([body[off + 6], body[off + 7]]),
                    hands: u16::from_le_bytes([body[off + 8], body[off + 9]]),
                    legs: u16::from_le_bytes([body[off + 10], body[off + 11]]),
                    feet: u16::from_le_bytes([body[off + 12], body[off + 13]]),
                    main: u16::from_le_bytes([body[off + 14], body[off + 15]]),
                    sub: u16::from_le_bytes([body[off + 16], body[off + 17]]),
                    ranged: u16::from_le_bytes([body[off + 18], body[off + 19]]),
                })
            }
            2 => Some(LookData::Door { size }),
            3 | 4 => Some(LookData::Transport { size }),
            _ => None,
        }
    }

    pub const CHAR_PC_GRAP_OFFSET: usize = 0x44;

    pub fn decode_char_pc(body: &[u8]) -> Option<Self> {
        let off = Self::CHAR_PC_GRAP_OFFSET;
        if body.len() < off + 18 {
            return None;
        }
        let slot0 = u16::from_le_bytes([body[off], body[off + 1]]);
        if slot0 == 0 {
            return None;
        }
        let face = (slot0 & 0x00FF) as u8;
        let race = ((slot0 >> 8) & 0x00FF) as u8;

        let read_slot = |i: usize| -> u16 {
            let p = off + 2 * i;
            u16::from_le_bytes([body[p], body[p + 1]]) & 0x0FFF
        };
        Some(LookData::Equipped {
            face,
            race,
            head: read_slot(1),
            body: read_slot(2),
            hands: read_slot(3),
            legs: read_slot(4),
            feet: read_slot(5),
            main: read_slot(6),
            sub: read_slot(7),
            ranged: read_slot(8),
        })
    }
}

/// NPC/MOB appearance-state from the General block of the 0x0E `CHAR_NPC`
/// packet, alongside the [`LookData`] at 0x2C. Offsets per
/// `vendor/server/src/map/packets/entity_update.cpp` (`updateWith`), with
/// body[0] == LSB packet 0x04: `animation` at LSB 0x1F → body[0x1B],
/// `status` at LSB 0x20 → body[0x1C], `animationsub` at LSB 0x2A → body[0x26].
///
/// `animationsub != 0` is the server's "active sub-animation effect" signal that
/// drives brazier/lamp/torch flames. On spawn LSB sets 0x2A to `4 | animationsub`
/// (bit 2 is a spawn flag), so the raw byte is kept and consumers mask 0x04 for
/// the bare selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NpcState {
    pub animation: u8,
    pub animationsub: u8,
    pub status: u8,
}

impl NpcState {
    pub const ANIMATION_OFFSET: usize = 0x1B;
    pub const STATUS_OFFSET: usize = 0x1C;
    pub const ANIMATIONSUB_OFFSET: usize = 0x26;

    /// Decode the appearance-state bytes from a `CHAR_NPC` (0x0E) body. Returns
    /// `None` if the body is too short to reach `animationsub` (the furthest of
    /// the three fields). Callers should only trust `animation`/`animationsub`
    /// when the packet's General/UPDATE_HP send-flag bit (0x04) is set — the
    /// server only refreshes them in that block — whereas `status` (0x20) is
    /// written on every update.
    pub fn decode_char_npc(body: &[u8]) -> Option<Self> {
        if body.len() <= Self::ANIMATIONSUB_OFFSET {
            return None;
        }
        Some(Self {
            animation: body[Self::ANIMATION_OFFSET],
            animationsub: body[Self::ANIMATIONSUB_OFFSET],
            status: body[Self::STATUS_OFFSET],
        })
    }

    /// Decode appearance-state from a `CHAR_PC` (0x0D) body. PCs share the
    /// `GP_SERV_POS_HEAD` prefix, so `animation` (`server_status`) sits at the
    /// same body[0x1B] — but unlike `CHAR_NPC` the 0x1C/0x26 bytes fall inside
    /// the PC `Flags1`/`Flags3` bitfields, so only `animation` is meaningful
    /// (`animationsub`/`status` left zero). Drives PC death pose / cast / sit.
    /// vendor/server/src/map/packets/char_update.cpp (`GP_SERV_CHAR_PC`).
    /// Trust only when the General send-flag bit (0x04) is set.
    pub fn decode_char_pc(body: &[u8]) -> Option<Self> {
        if body.len() <= Self::ANIMATION_OFFSET {
            return None;
        }
        Some(Self {
            animation: body[Self::ANIMATION_OFFSET],
            animationsub: 0,
            status: 0,
        })
    }

    /// `status` (LSB 0x20 → body[0x1C]) alone, for `CHAR_NPC`. Unlike the General
    /// block's `animation`/`animationsub`, the server writes this byte on every
    /// update regardless of the UPDATE_HP send-flag, so it is valid on pos-only /
    /// status-only ticks. vendor/server/src/map/packets/entity_update.cpp.
    pub fn decode_char_npc_status(body: &[u8]) -> Option<u8> {
        body.get(Self::STATUS_OFFSET).copied()
    }
}

const _: () = {
    assert!(NpcState::ANIMATION_OFFSET < NpcState::STATUS_OFFSET);
    assert!(NpcState::STATUS_OFFSET < NpcState::ANIMATIONSUB_OFFSET);
    assert!(NpcState::ANIMATIONSUB_OFFSET < LookData::LOOK_BODY_OFFSET);
};

#[derive(Debug, Clone, Copy)]
pub struct ServerLogin {
    pub unique_no: u32,
    pub act_index: u16,
    pub zone_no: u16,

    pub game_time: Option<u32>,

    pub pos_head: PosHead,

    pub music_num: Option<[u16; 5]>,
}

impl ServerLogin {
    pub const SIZE: usize = 48;

    pub const MUSIC_NUM_OFFSET: usize = 0x52;
    pub const MUSIC_NUM_SIZE: usize = 5 * 2;

    pub const GAME_TIME_OFFSET: usize = 0x38;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }

        let zone_u32 = u32::from_le_bytes(body[44..48].try_into().unwrap());
        let pos_head = PosHead::decode(&body[..PosHead::SIZE_WITH_BT_TARGET])?;
        let game_time = if body.len() >= Self::GAME_TIME_OFFSET + 4 {
            Some(u32::from_le_bytes(
                body[Self::GAME_TIME_OFFSET..Self::GAME_TIME_OFFSET + 4]
                    .try_into()
                    .unwrap(),
            ))
        } else {
            None
        };

        let music_num = if body.len() >= Self::MUSIC_NUM_OFFSET + Self::MUSIC_NUM_SIZE {
            let base = Self::MUSIC_NUM_OFFSET;
            Some([
                u16::from_le_bytes([body[base], body[base + 1]]),
                u16::from_le_bytes([body[base + 2], body[base + 3]]),
                u16::from_le_bytes([body[base + 4], body[base + 5]]),
                u16::from_le_bytes([body[base + 6], body[base + 7]]),
                u16::from_le_bytes([body[base + 8], body[base + 9]]),
            ])
        } else {
            None
        };
        Ok(Self {
            unique_no: pos_head.unique_no,
            act_index: pos_head.act_index,
            zone_no: zone_u32 as u16,
            game_time,
            pos_head,
            music_num,
        })
    }
}

/// s2c 0x037 GP_SERV_SERVERSTATUS (char status). Only the fields we consume are
/// decoded: the subject id, its HP%, and the death/homepoint counters.
/// vendor/server/src/map/packets/char_status.cpp
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharStatus {
    pub unique_no: u32,
    pub hpp: u8,
    pub dead_counter1: u32,
    pub dead_counter2: u32,
}

impl CharStatus {
    pub const UNIQUE_NO_OFFSET: usize = 0x20;
    pub const FLAGS0_OFFSET: usize = 0x24;
    pub const DEAD_COUNTER1_OFFSET: usize = 0x38;
    pub const DEAD_COUNTER2_OFFSET: usize = 0x3C;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        let need = Self::DEAD_COUNTER2_OFFSET + 4;
        if body.len() < need {
            return Err(DecodeError::Truncated(need, body.len()));
        }
        let rd = |o: usize| u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        let flags0 = rd(Self::FLAGS0_OFFSET);
        Ok(Self {
            unique_no: rd(Self::UNIQUE_NO_OFFSET),
            // flags0_t bitfield: hpp occupies bits 16..24.
            hpp: ((flags0 >> 16) & 0xFF) as u8,
            dead_counter1: rd(Self::DEAD_COUNTER1_OFFSET),
            dead_counter2: rd(Self::DEAD_COUNTER2_OFFSET),
        })
    }

    /// Seconds until the server force-warps a KO'd player home. LSB sends
    /// dead_counter1 = 60 * (6min + (60min - timeSinceDeath)); the leading 6min is fixed
    /// padding, so stripping it (`dead_counter1/60 - 360`) yields the real time left,
    /// which hits 0 when the server-side CDeathState completes at death + 60min.
    /// vendor/server/src/map/packets/char_status.cpp,
    /// charentity.cpp::GetTimeUntilDeathHomepoint, ai/states/death_state.cpp
    pub fn seconds_until_homepoint(&self) -> u32 {
        (self.dead_counter1 / 60).saturating_sub(360)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ServerLogout {
    pub logout_state: u32,
    pub new_server_ip: u32,
    pub new_server_port: u16,
    pub error_code: u32,
}

impl ServerLogout {
    pub const SIZE: usize = 24;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            logout_state: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            new_server_ip: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            new_server_port: u32::from_le_bytes(body[8..12].try_into().unwrap()) as u16,
            error_code: u32::from_le_bytes(body[20..24].try_into().unwrap()),
        })
    }

    pub fn is_zone_change(&self) -> bool {
        self.logout_state == 2
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SystemMessage {
    pub para: u32,
    pub para2: u32,
    pub message_id: u16,
}

impl SystemMessage {
    pub const SIZE: usize = 12;

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

pub fn decode_system_message(sub: &SubPacket<'_>) -> Result<SystemMessage, DecodeError> {
    SystemMessage::decode(sub.data)
}

#[derive(Debug, Clone, Copy)]
pub struct WeatherPacket {
    pub start_time: u32,
    pub weather_number: u16,
    pub offset_time: u16,
}

impl WeatherPacket {
    pub const SIZE: usize = 8;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            start_time: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            weather_number: u16::from_le_bytes(body[4..6].try_into().unwrap()),
            offset_time: u16::from_le_bytes(body[6..8].try_into().unwrap()),
        })
    }
}

pub fn decode_weather(sub: &SubPacket<'_>) -> Result<WeatherPacket, DecodeError> {
    WeatherPacket::decode(sub.data)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PosMode {
    Normal = 0x00,
    Event = 0x01,
    Clear = 0x02,
    Pop = 0x03,
    Reset = 0x05,
    Materialize = 0x06,
    Lock = 0x08,
    Unlock = 0x09,
    Rotate = 0x0A,
}

impl PosMode {
    pub fn from_u8(raw: u8) -> Option<Self> {
        Some(match raw {
            0x00 => PosMode::Normal,
            0x01 => PosMode::Event,
            0x02 => PosMode::Clear,
            0x03 => PosMode::Pop,
            0x05 => PosMode::Reset,
            0x06 => PosMode::Materialize,
            0x08 => PosMode::Lock,
            0x09 => PosMode::Unlock,
            0x0A => PosMode::Rotate,
            _ => return None,
        })
    }

    pub fn carries_position(&self) -> bool {
        matches!(
            self,
            PosMode::Normal | PosMode::Event | PosMode::Pop | PosMode::Reset | PosMode::Materialize
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForcedMove {
    pub unique_no: u32,
    pub act_index: u16,
    pub mode: PosMode,
    pub x: f32,

    pub y: f32,

    pub z: f32,
    pub heading: u8,

    pub raw_mode: u8,
}

impl ForcedMove {
    pub const SIZE: usize = 24;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let lsb_x = f32::from_le_bytes(body[0..4].try_into().unwrap());
        let lsb_y = f32::from_le_bytes(body[4..8].try_into().unwrap());
        let lsb_z = f32::from_le_bytes(body[8..12].try_into().unwrap());
        let unique_no = u32::from_le_bytes(body[12..16].try_into().unwrap());
        let act_index = u16::from_le_bytes(body[16..18].try_into().unwrap());
        let raw_mode = body[18];

        let mode = PosMode::from_u8(raw_mode).unwrap_or(PosMode::Normal);
        let heading = body[19];
        Ok(Self {
            unique_no,
            act_index,
            mode,
            x: lsb_x,
            y: lsb_z,
            z: lsb_y,
            heading,
            raw_mode,
        })
    }
}

pub fn decode_forced_move(sub: &SubPacket<'_>) -> Result<ForcedMove, DecodeError> {
    ForcedMove::decode(sub.data)
}

#[cfg(test)]
mod despawn_tests {
    use super::*;
    use crate::map::s2c;

    fn body_with_updatemask(mask: u8) -> Vec<u8> {
        let mut body = vec![0u8; PosHead::SIZE_WITH_BT_TARGET];
        body[6] = mask;
        body
    }

    #[test]
    fn lsb_despawn_byte_0x30_on_char_npc_is_despawn() {
        let body = body_with_updatemask(0x30);
        assert!(PosHead::is_entity_despawn(s2c::CHAR_NPC, &body));
    }

    #[test]
    fn despawn_bit_alone_is_despawn() {
        let body = body_with_updatemask(PosHead::UPDATE_DESPAWN);
        assert!(PosHead::is_entity_despawn(s2c::CHAR_NPC, &body));
    }

    #[test]
    fn spawn_and_normal_updatemasks_are_not_despawn() {
        for mask in [0x0F, 0x57, 0x01, 0x07, 0x08, 0x10, 0x1F] {
            assert_eq!(mask & PosHead::UPDATE_DESPAWN, 0, "test mask sanity");
            let body = body_with_updatemask(mask);
            assert!(
                !PosHead::is_entity_despawn(s2c::CHAR_NPC, &body),
                "CHAR_NPC updatemask 0x{mask:02x} must not be treated as despawn",
            );
            assert!(
                !PosHead::is_entity_despawn(s2c::CHAR_PC, &body),
                "CHAR_PC SendFlg 0x{mask:02x} must not be treated as despawn",
            );
        }
    }

    #[test]
    fn despawn_bit_on_char_pc_is_despawn() {
        let body = body_with_updatemask(PosHead::UPDATE_DESPAWN);
        assert!(PosHead::is_entity_despawn(s2c::CHAR_PC, &body));
    }

    #[test]
    fn truncated_body_is_not_despawn() {
        assert!(!PosHead::is_entity_despawn(s2c::CHAR_NPC, &[]));
        assert!(!PosHead::is_entity_despawn(s2c::CHAR_NPC, &[0u8; 4]));
        assert!(!PosHead::is_entity_despawn(s2c::CHAR_PC, &[0u8; 4]));
    }
}

#[cfg(test)]
mod forced_move_tests {
    use super::*;

    #[test]
    fn forced_move_decodes_normal_mode_and_swaps_axes() {
        let mut body = vec![0u8; ForcedMove::SIZE];
        body[0..4].copy_from_slice(&12.5f32.to_le_bytes());
        body[4..8].copy_from_slice(&3.25f32.to_le_bytes());
        body[8..12].copy_from_slice(&(-7.0f32).to_le_bytes());
        body[12..16].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        body[16..18].copy_from_slice(&42u16.to_le_bytes());
        body[18] = 0x00;
        body[19] = 64;
        let fm = ForcedMove::decode(&body).expect("decode");
        assert!((fm.x - 12.5).abs() < 1e-3);
        assert!((fm.y - (-7.0)).abs() < 1e-3, "NS (our.y) ← LSB.z");
        assert!((fm.z - 3.25).abs() < 1e-3, "height (our.z) ← LSB.y");
        assert_eq!(fm.unique_no, 0xDEADBEEF);
        assert_eq!(fm.act_index, 42);
        assert_eq!(fm.mode, PosMode::Normal);
        assert!(fm.mode.carries_position());
        assert_eq!(fm.heading, 64);
    }

    #[test]
    fn forced_move_lock_unlock_clear_do_not_carry_position() {
        for raw in [0x08u8, 0x09u8, 0x02u8, 0x0A] {
            let mut body = vec![0u8; ForcedMove::SIZE];
            body[18] = raw;
            let fm = ForcedMove::decode(&body).expect("decode");
            assert!(
                !fm.mode.carries_position(),
                "mode 0x{raw:02x} must not be authoritative for position",
            );
        }
    }

    #[test]
    fn forced_move_truncated_errors() {
        let body = vec![0u8; ForcedMove::SIZE - 1];
        assert!(matches!(
            ForcedMove::decode(&body),
            Err(DecodeError::Truncated(s, n)) if s == ForcedMove::SIZE && n == ForcedMove::SIZE - 1
        ));
    }

    #[test]
    fn forced_move_unknown_mode_falls_back_to_normal() {
        let mut body = vec![0u8; ForcedMove::SIZE];
        body[18] = 0x7F;
        let fm = ForcedMove::decode(&body).expect("decode");
        assert_eq!(fm.raw_mode, 0x7F);
        assert_eq!(fm.mode, PosMode::Normal);
    }
}

#[derive(Debug, Clone)]
pub struct PartyAttrs {
    pub unique_no: u32,
    pub act_index: u16,
    pub hp: u32,
    pub mp: u32,
    pub tp: u32,
    pub hpp: u8,
    pub mpp: u8,
    pub kind: u8,

    pub moghouse_flg: u8,
    pub zone_no: u16,
    pub mjob_no: u8,
    pub mjob_lv: u8,
    pub sjob_no: u8,
    pub sjob_lv: u8,
}

#[derive(Debug, Clone)]
pub struct PartyListExtra {
    pub member_number: u8,
    pub is_party_leader: bool,
    pub is_alliance_leader: bool,

    pub name: Option<String>,
}

impl PartyAttrs {
    pub fn decode_group_attr(body: &[u8]) -> Result<Self, DecodeError> {
        const NEEDED: usize = 32;
        if body.len() < NEEDED {
            return Err(DecodeError::Truncated(NEEDED, body.len()));
        }
        Ok(Self {
            unique_no: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            hp: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            mp: u32::from_le_bytes(body[8..12].try_into().unwrap()),
            tp: u32::from_le_bytes(body[12..16].try_into().unwrap()),
            act_index: u16::from_le_bytes(body[16..18].try_into().unwrap()),
            hpp: body[18],
            mpp: body[19],
            kind: body[20],
            moghouse_flg: body[21],
            zone_no: u16::from_le_bytes(body[22..24].try_into().unwrap()),
            mjob_no: body[28],
            mjob_lv: body[29],
            sjob_no: body[30],
            sjob_lv: body[31],
        })
    }

    pub fn decode_group_list(body: &[u8]) -> Result<(Self, PartyListExtra), DecodeError> {
        const NEEDED: usize = 52;
        if body.len() < NEEDED {
            return Err(DecodeError::Truncated(NEEDED, body.len()));
        }
        let attrs = Self {
            unique_no: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            hp: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            mp: u32::from_le_bytes(body[8..12].try_into().unwrap()),
            tp: u32::from_le_bytes(body[12..16].try_into().unwrap()),
            act_index: u16::from_le_bytes(body[20..22].try_into().unwrap()),
            kind: body[24],
            hpp: body[25],
            mpp: body[26],
            moghouse_flg: body[23],
            zone_no: u16::from_le_bytes(body[28..30].try_into().unwrap()),
            mjob_no: body[30],
            mjob_lv: body[31],
            sjob_no: body[32],
            sjob_lv: body[33],
        };
        let gattr = u32::from_le_bytes(body[16..20].try_into().unwrap());

        let is_party_leader = (gattr >> 2) & 1 == 1;
        let is_alliance_leader = (gattr >> 3) & 1 == 1;
        let name_bytes = &body[36..52];
        let n = name_bytes
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(name_bytes.len());
        let name = if n > 0 && name_bytes[..n].iter().all(|&b| (0x20..=0x7E).contains(&b)) {
            Some(String::from_utf8_lossy(&name_bytes[..n]).into_owned())
        } else {
            None
        };
        let extra = PartyListExtra {
            member_number: body[22],
            is_party_leader,
            is_alliance_leader,
            name,
        };
        Ok((attrs, extra))
    }
}

pub fn decode_pos_head(sub: &SubPacket<'_>) -> Result<PosHead, DecodeError> {
    PosHead::decode(sub.data)
}

pub fn decode_logout(sub: &SubPacket<'_>) -> Result<ServerLogout, DecodeError> {
    ServerLogout::decode(sub.data)
}

pub fn decode_login(sub: &SubPacket<'_>) -> Result<ServerLogin, DecodeError> {
    ServerLogin::decode(sub.data)
}

fn read_name_slot(slot: &[u8]) -> Option<String> {
    let n = slot.iter().position(|&b| b == 0).unwrap_or(slot.len());
    if n < 3 {
        return None;
    }
    let bytes = &slot[..n];
    if !bytes.iter().all(|&b| (0x20..=0x7E).contains(&b)) {
        return None;
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
}

#[derive(Debug, Clone, Copy)]
pub struct CharSync {
    pub targid: u16,
    pub id: u32,
}

impl CharSync {
    pub const SUB_TYPE: u8 = 0x02;
    pub const SIZE: usize = 8;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            targid: u16::from_le_bytes(body[2..4].try_into().unwrap()),
            id: u32::from_le_bytes(body[4..8].try_into().unwrap()),
        })
    }
}

#[derive(Debug, Clone)]
pub struct EntitySetName {
    pub targid: u16,
    pub id: u32,
    pub master_targid: u16,
    pub name: Option<String>,
}

impl EntitySetName {
    pub const SUB_TYPE: u8 = 0x03;

    pub const SIZE: usize = 0x14;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let name = read_name_slot(&body[0x14..]);
        Ok(Self {
            targid: u16::from_le_bytes(body[2..4].try_into().unwrap()),
            id: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            master_targid: u16::from_le_bytes(body[8..10].try_into().unwrap()),
            name,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PetSync {
    pub owner_targid: u16,
    pub owner_id: u32,
    pub pet_targid: u16,
    pub hp_pct: u8,
    pub mp_pct: u8,
    pub tp: u16,
    pub bt_target_id: u32,
    pub name: Option<String>,
}

impl PetSync {
    pub const DESPAWN_SIZE: usize = 8;

    pub const FULL_HEADER_SIZE: usize = 0x14;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::DESPAWN_SIZE {
            return Err(DecodeError::Truncated(Self::DESPAWN_SIZE, body.len()));
        }
        let owner_targid = u16::from_le_bytes(body[2..4].try_into().unwrap());
        let owner_id = u32::from_le_bytes(body[4..8].try_into().unwrap());
        if body.len() < Self::FULL_HEADER_SIZE {
            return Ok(Self {
                owner_targid,
                owner_id,
                pet_targid: 0,
                hp_pct: 0,
                mp_pct: 0,
                tp: 0,
                bt_target_id: 0,
                name: None,
            });
        }
        let name = read_name_slot(&body[0x14..]);
        Ok(Self {
            owner_targid,
            owner_id,
            pet_targid: u16::from_le_bytes(body[8..10].try_into().unwrap()),
            hp_pct: body[0x0A],
            mp_pct: body[0x0B],
            tp: u16::from_le_bytes(body[0x0C..0x0E].try_into().unwrap()),
            bt_target_id: u32::from_le_bytes(body[0x10..0x14].try_into().unwrap()),
            name,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ItemMax {
    pub capacities: [u16; 18],
}

impl ItemMax {
    pub const SIZE: usize = 96;
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let mut capacities = [0u16; 18];
        for (i, cap) in capacities.iter_mut().enumerate() {
            let legacy = body[i] as u16;
            let wide_off = 18 + 14 + i * 2;
            let wide = u16::from_le_bytes(body[wide_off..wide_off + 2].try_into().unwrap());
            let raw = if wide != 0 { wide } else { legacy };
            *cap = raw.saturating_sub(1);
        }
        Ok(Self { capacities })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemSameState {
    StillLoading,
    AllLoaded,
}

#[derive(Debug, Clone, Copy)]
pub struct ItemSame {
    pub state: ItemSameState,
    pub flags: u32,
}

impl ItemSame {
    pub const SIZE: usize = 8;
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let state = match body[0] {
            0 => ItemSameState::StillLoading,

            _ => ItemSameState::AllLoaded,
        };
        let flags = u32::from_le_bytes(body[4..8].try_into().unwrap());
        Ok(Self { state, flags })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ItemNum {
    pub quantity: u32,

    pub category: u8,

    pub index: u8,

    pub lock_flg: u8,
}

impl ItemNum {
    pub const SIZE: usize = 8;
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            quantity: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            category: body[4],
            index: body[5],
            lock_flg: body[6],
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ItemList {
    pub quantity: u32,

    pub item_no: u16,
    pub category: u8,
    pub index: u8,
    pub lock_flg: u8,
}

impl ItemList {
    pub const SIZE: usize = 12;
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            quantity: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            item_no: u16::from_le_bytes(body[4..6].try_into().unwrap()),
            category: body[6],
            index: body[7],
            lock_flg: body[8],
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ItemAttr {
    pub quantity: u32,
    pub price: u32,
    pub item_no: u16,
    pub category: u8,
    pub index: u8,
    pub lock_flg: u8,
    pub extdata: [u8; 24],
}

impl ItemAttr {
    pub const SIZE: usize = 37;
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let mut extdata = [0u8; 24];
        extdata.copy_from_slice(&body[13..37]);
        Ok(Self {
            quantity: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            price: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            item_no: u16::from_le_bytes(body[8..10].try_into().unwrap()),
            category: body[10],
            index: body[11],
            lock_flg: body[12],
            extdata,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EquipList {
    pub container_index: u8,

    pub equip_slot: u8,

    pub container: u8,
}

impl EquipList {
    pub const SIZE: usize = 4;
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            container_index: body[0],
            equip_slot: body[1],
            container: body[2],
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MagicData<'a> {
    pub bitmap: &'a [u8; MAGIC_DATA_SIZE],
}

pub const MAGIC_DATA_SIZE: usize = 128;

impl<'a> MagicData<'a> {
    pub const SIZE: usize = MAGIC_DATA_SIZE;

    pub const SPELL_ID_LIMIT: usize = Self::SIZE * 8;
    pub fn decode(body: &'a [u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let bitmap: &[u8; MAGIC_DATA_SIZE] = body[..Self::SIZE].try_into().unwrap();
        Ok(Self { bitmap })
    }

    pub fn known_ids(&self) -> Vec<u16> {
        collect_set_bits(self.bitmap)
    }
    pub fn is_known(&self, id: u16) -> bool {
        let idx = id as usize;
        if idx >= Self::SPELL_ID_LIMIT {
            return false;
        }
        self.bitmap[idx >> 3] & (1 << (idx & 7)) != 0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CommandData<'a> {
    pub weapon_skills: &'a [u8; 64],
    pub job_abilities: &'a [u8; 64],
    pub pet_abilities: &'a [u8; 64],
    pub traits: &'a [u8; 32],
}

impl<'a> CommandData<'a> {
    pub const SIZE: usize = 64 + 64 + 64 + 32;
    pub fn decode(body: &'a [u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            weapon_skills: body[0..64].try_into().unwrap(),
            job_abilities: body[64..128].try_into().unwrap(),
            pet_abilities: body[128..192].try_into().unwrap(),
            traits: body[192..224].try_into().unwrap(),
        })
    }
}

pub fn collect_set_bits(bitmap: &[u8]) -> Vec<u16> {
    let mut out = Vec::new();
    for (byte_idx, byte) in bitmap.iter().enumerate() {
        if *byte == 0 {
            continue;
        }
        for bit in 0..8 {
            if byte & (1 << bit) != 0 {
                out.push((byte_idx * 8 + bit) as u16);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_data_decodes_standard_modelid() {
        let mut buf = vec![0u8; 0x40];
        buf[0x2C..0x2E].copy_from_slice(&0u16.to_le_bytes());
        buf[0x2E..0x30].copy_from_slice(&0x1234u16.to_le_bytes());
        assert_eq!(
            LookData::decode_char_npc(&buf),
            Some(LookData::Standard { modelid: 0x1234 })
        );
    }

    #[test]
    fn look_data_decodes_equipped_look_t() {
        let mut buf = vec![0u8; 0x50];
        buf[0x2C..0x2E].copy_from_slice(&1u16.to_le_bytes());
        buf[0x2E] = 0x07;
        buf[0x2F] = 0x03;
        for (i, v) in [
            0xA001u16, 0xA002, 0xA003, 0xA004, 0xA005, 0xA006, 0xA007, 0xA008,
        ]
        .iter()
        .enumerate()
        {
            buf[0x30 + 2 * i..0x32 + 2 * i].copy_from_slice(&v.to_le_bytes());
        }
        assert_eq!(
            LookData::decode_char_npc(&buf),
            Some(LookData::Equipped {
                face: 0x07,
                race: 0x03,
                head: 0xA001,
                body: 0xA002,
                hands: 0xA003,
                legs: 0xA004,
                feet: 0xA005,
                main: 0xA006,
                sub: 0xA007,
                ranged: 0xA008,
            })
        );
    }

    #[test]
    fn look_data_truncated_returns_none() {
        let buf = vec![0u8; 0x20];
        assert_eq!(LookData::decode_char_npc(&buf), None);
    }

    #[test]
    fn look_data_unknown_sentinel_returns_none() {
        let mut buf = vec![0u8; 0x40];
        buf[0x2C..0x2E].copy_from_slice(&0x00FFu16.to_le_bytes());
        assert_eq!(LookData::decode_char_npc(&buf), None);
    }

    #[test]
    fn look_data_decodes_pc_grapidtbl() {
        let mut buf = vec![0u8; 0x60];
        let off = LookData::CHAR_PC_GRAP_OFFSET;

        buf[off..off + 2].copy_from_slice(&0x0107u16.to_le_bytes());

        let gear: [u16; 8] = [0x111, 0x222, 0x333, 0x444, 0x555, 0x666, 0x777, 0x888];
        for (i, raw) in gear.iter().enumerate() {
            let slot_idx = i + 1;
            let masked = *raw | ((slot_idx as u16) << 12);
            let p = off + 2 * slot_idx;
            buf[p..p + 2].copy_from_slice(&masked.to_le_bytes());
        }
        assert_eq!(
            LookData::decode_char_pc(&buf),
            Some(LookData::Equipped {
                face: 0x07,
                race: 0x01,
                head: 0x111,
                body: 0x222,
                hands: 0x333,
                legs: 0x444,
                feet: 0x555,
                main: 0x666,
                sub: 0x777,
                ranged: 0x888,
            })
        );
    }

    #[test]
    fn look_data_pc_zero_modelid_returns_none() {
        let buf = vec![0u8; 0x60];
        assert_eq!(LookData::decode_char_pc(&buf), None);
    }

    #[test]
    fn look_data_pc_truncated_returns_none() {
        let mut buf = vec![0u8; 0x55];

        buf[LookData::CHAR_PC_GRAP_OFFSET..LookData::CHAR_PC_GRAP_OFFSET + 2]
            .copy_from_slice(&0x0107u16.to_le_bytes());
        assert_eq!(LookData::decode_char_pc(&buf), None);
    }

    #[test]
    fn npc_state_decodes_lsb_general_block_offsets() {
        let mut body = vec![0u8; 0x30];
        body[NpcState::ANIMATION_OFFSET] = 0x21;
        body[NpcState::STATUS_OFFSET] = 0x02;
        body[NpcState::ANIMATIONSUB_OFFSET] = 0x05;
        assert_eq!(
            NpcState::decode_char_npc(&body),
            Some(NpcState {
                animation: 0x21,
                animationsub: 0x05,
                status: 0x02,
            })
        );
    }

    #[test]
    fn npc_state_matches_fireworks_effect_npc() {
        const SPAWN_FLAG: u8 = 0x04;
        let mut body = vec![0u8; 0x48];
        body[NpcState::ANIMATION_OFFSET] = 0;
        body[NpcState::STATUS_OFFSET] = 2;
        body[NpcState::ANIMATIONSUB_OFFSET] = SPAWN_FLAG | 1;
        let st = NpcState::decode_char_npc(&body).expect("decode");
        assert_eq!(st.animation, 0);
        assert_eq!(st.status, 2);
        assert_ne!(st.animationsub, 0);
        assert_eq!(st.animationsub & !SPAWN_FLAG, 1);
    }

    #[test]
    fn npc_state_truncated_returns_none() {
        assert_eq!(NpcState::decode_char_npc(&[0u8; 0x26]), None);
        assert!(NpcState::decode_char_npc(&[0u8; 0x27]).is_some());
    }

    #[test]
    fn npc_state_status_readable_without_general_block() {
        let mut body = vec![0u8; NpcState::STATUS_OFFSET + 1];
        body[NpcState::STATUS_OFFSET] = 3;
        assert_eq!(
            NpcState::decode_char_npc(&body),
            None,
            "full NpcState needs the General block at ANIMATIONSUB_OFFSET"
        );
        assert_eq!(
            NpcState::decode_char_npc_status(&body),
            Some(3),
            "status alone reads from a body reaching only STATUS_OFFSET"
        );

        assert_eq!(
            NpcState::decode_char_npc_status(&[0u8; NpcState::STATUS_OFFSET]),
            None,
            "body not reaching 0x1C yields no status"
        );
    }

    #[test]
    fn npc_state_char_pc_reads_only_animation() {
        const DEATH: u8 = 3;
        let mut body = vec![0u8; PosHead::SIZE];
        body[NpcState::ANIMATION_OFFSET] = DEATH;
        // Bytes that are status/animationsub for CHAR_NPC are PC bitfield bits
        // here; decode_char_pc must ignore them.
        body[NpcState::STATUS_OFFSET] = 0xFF;
        let st = NpcState::decode_char_pc(&body).expect("decode");
        assert_eq!(st.animation, DEATH);
        assert_eq!(st.status, 0);
        assert_eq!(st.animationsub, 0);
    }

    #[test]
    fn npc_state_char_pc_truncated_returns_none() {
        assert_eq!(
            NpcState::decode_char_pc(&[0u8; NpcState::ANIMATION_OFFSET]),
            None
        );
        assert!(NpcState::decode_char_pc(&[0u8; NpcState::ANIMATION_OFFSET + 1]).is_some());
    }

    #[test]
    fn pos_head_minimal_decode() {
        let mut buf = vec![0u8; PosHead::SIZE];
        buf[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        buf[4..6].copy_from_slice(&0x0042u16.to_le_bytes());
        buf[6] = 0b0000_0001;
        buf[7] = 64;
        buf[8..12].copy_from_slice(&123.5f32.to_le_bytes());
        buf[12..16].copy_from_slice(&(-12.0f32).to_le_bytes());
        buf[16..20].copy_from_slice(&7.25f32.to_le_bytes());
        buf[24] = 25;
        buf[25] = 25;
        buf[26] = 100;
        buf[27] = 1;

        let h = PosHead::decode(&buf).unwrap();
        assert_eq!(h.unique_no, 0xDEAD_BEEF);
        assert_eq!(h.act_index, 0x42);
        assert_eq!(h.send_flag, 1);
        assert_eq!(h.dir, 64);
        assert_eq!(h.x, 123.5);
        assert_eq!(h.z, -12.0);
        assert_eq!(h.y, 7.25);
        assert_eq!(h.speed, 25);
        assert_eq!(h.speed_base, 25);
        assert_eq!(h.hpp, 100);
    }

    #[test]
    fn server_login_decodes_zone_no() {
        let mut buf = vec![0u8; ServerLogin::SIZE];
        buf[0..4].copy_from_slice(&0x0123_4567u32.to_le_bytes());
        buf[4..6].copy_from_slice(&0x00FFu16.to_le_bytes());
        buf[44..48].copy_from_slice(&230u32.to_le_bytes());
        let l = ServerLogin::decode(&buf).unwrap();
        assert_eq!(l.unique_no, 0x0123_4567);
        assert_eq!(l.act_index, 0x00FF);
        assert_eq!(l.zone_no, 230);
    }

    #[test]
    fn server_login_truncated_errors() {
        let buf = vec![0u8; ServerLogin::SIZE - 1];
        assert!(matches!(
            ServerLogin::decode(&buf),
            Err(DecodeError::Truncated(48, _))
        ));
    }

    #[test]
    fn server_login_carries_pos_head_for_spawn_seed() {
        let mut buf = vec![0u8; ServerLogin::SIZE];
        buf[0..4].copy_from_slice(&0x0123_4567u32.to_le_bytes());
        buf[4..6].copy_from_slice(&0x00FFu16.to_le_bytes());
        buf[7] = 96;
        buf[8..12].copy_from_slice(&(-115.5f32).to_le_bytes());
        buf[12..16].copy_from_slice(&(7.25f32).to_le_bytes());
        buf[16..20].copy_from_slice(&(280.0f32).to_le_bytes());
        buf[24] = 40;
        buf[25] = 40;
        buf[44..48].copy_from_slice(&230u32.to_le_bytes());
        let l = ServerLogin::decode(&buf).unwrap();
        assert_eq!(l.pos_head.x, -115.5);
        assert_eq!(l.pos_head.z, 7.25);
        assert_eq!(l.pos_head.y, 280.0);
        assert_eq!(l.pos_head.dir, 96);
        assert_eq!(l.pos_head.speed, 40);
        assert_eq!(l.pos_head.speed_base, 40);
    }

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

    #[test]
    fn server_logout_zone_change() {
        let mut buf = vec![0u8; ServerLogout::SIZE];
        buf[0..4].copy_from_slice(&2u32.to_le_bytes());
        buf[4..8].copy_from_slice(&0x6F00_A8C0u32.to_le_bytes());
        buf[8..12].copy_from_slice(&54230u32.to_le_bytes());
        let l = ServerLogout::decode(&buf).unwrap();
        assert!(l.is_zone_change());
        assert_eq!(l.new_server_port, 54230);
        assert_eq!(l.new_server_ip, 0x6F00_A8C0);
    }

    #[test]
    fn pos_head_truncated_errors() {
        let buf = vec![0u8; PosHead::SIZE - 1];
        assert!(matches!(
            PosHead::decode(&buf),
            Err(DecodeError::Truncated(_, _))
        ));
    }

    #[test]
    fn pos_head_extracts_bt_target_id_when_present() {
        let mut buf = vec![0u8; PosHead::SIZE_WITH_BT_TARGET];
        buf[0..4].copy_from_slice(&0xCAFE_F00Du32.to_le_bytes());
        buf[40..44].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        let h = PosHead::decode(&buf).unwrap();
        assert_eq!(h.unique_no, 0xCAFE_F00D);
        assert_eq!(h.bt_target_id, 0xDEAD_BEEF);
    }

    #[test]
    fn decode_char_npc_extracts_claim_id() {
        let mut buf = vec![0u8; PosHead::SIZE_WITH_BT_TARGET];
        buf[0..4].copy_from_slice(&0xAABB_CCDDu32.to_le_bytes());
        buf[4..6].copy_from_slice(&0x07F0u16.to_le_bytes());
        buf[40..44].copy_from_slice(&0x0123_4567u32.to_le_bytes());
        let (head, claim_id) = PosHead::decode_char_npc(&buf).unwrap();
        assert_eq!(head.unique_no, 0xAABB_CCDD);
        assert_eq!(head.act_index, 0x07F0);
        assert_eq!(claim_id, 0x0123_4567);
    }

    #[test]
    fn decode_char_npc_unclaimed_yields_zero_claim() {
        let buf = vec![0u8; PosHead::SIZE];
        let (_, claim_id) = PosHead::decode_char_npc(&buf).unwrap();
        assert_eq!(claim_id, 0);
    }

    #[test]
    fn pos_head_legacy_40_byte_body_yields_zero_bt_target() {
        let buf = vec![0u8; PosHead::SIZE];
        let h = PosHead::decode(&buf).unwrap();
        assert_eq!(h.bt_target_id, 0);
    }

    #[test]
    fn party_attrs_group_attr_decodes() {
        let mut buf = vec![0u8; 36];
        buf[0..4].copy_from_slice(&0x0001_0042u32.to_le_bytes());
        buf[4..8].copy_from_slice(&1500u32.to_le_bytes());
        buf[8..12].copy_from_slice(&500u32.to_le_bytes());
        buf[12..16].copy_from_slice(&1234u32.to_le_bytes());
        buf[16..18].copy_from_slice(&0x0042u16.to_le_bytes());
        buf[18] = 75;
        buf[19] = 50;
        buf[20] = 0;
        buf[21] = 1;
        buf[22..24].copy_from_slice(&234u16.to_le_bytes());
        buf[28] = 6;
        buf[29] = 75;
        buf[30] = 1;
        buf[31] = 37;

        let p = PartyAttrs::decode_group_attr(&buf).unwrap();
        assert_eq!(p.unique_no, 0x0001_0042);
        assert_eq!(p.hp, 1500);
        assert_eq!(p.mp, 500);
        assert_eq!(p.tp, 1234);
        assert_eq!(p.act_index, 0x42);
        assert_eq!(p.hpp, 75);
        assert_eq!(p.mpp, 50);
        assert_eq!(p.moghouse_flg, 1);
        assert_eq!(p.zone_no, 234);
        assert_eq!(p.mjob_no, 6);
        assert_eq!(p.mjob_lv, 75);
        assert_eq!(p.sjob_no, 1);
        assert_eq!(p.sjob_lv, 37);
    }

    #[test]
    fn party_attrs_group_list_decodes_with_name_and_leader() {
        let mut buf = vec![0u8; 56];
        buf[0..4].copy_from_slice(&0x0010_0001u32.to_le_bytes());
        buf[4..8].copy_from_slice(&2000u32.to_le_bytes());
        buf[8..12].copy_from_slice(&100u32.to_le_bytes());
        buf[12..16].copy_from_slice(&0u32.to_le_bytes());

        buf[16..20].copy_from_slice(&0x0000_0005u32.to_le_bytes());
        buf[20..22].copy_from_slice(&0x0007u16.to_le_bytes());
        buf[22] = 1;
        buf[23] = 1;
        buf[24] = 0;
        buf[25] = 100;
        buf[26] = 100;
        buf[28..30].copy_from_slice(&230u16.to_le_bytes());
        buf[30] = 1;
        buf[31] = 75;
        buf[36..36 + 6].copy_from_slice(b"Vanari");

        let (attrs, extra) = PartyAttrs::decode_group_list(&buf).unwrap();
        assert_eq!(attrs.unique_no, 0x0010_0001);
        assert_eq!(attrs.hp, 2000);
        assert_eq!(attrs.act_index, 7);
        assert_eq!(attrs.zone_no, 230);
        assert_eq!(attrs.moghouse_flg, 1);
        assert_eq!(extra.member_number, 1);
        assert!(extra.is_party_leader);
        assert!(!extra.is_alliance_leader);
        assert_eq!(extra.name.as_deref(), Some("Vanari"));
    }

    #[test]
    fn party_attrs_group_list_truncated_errors() {
        let buf = vec![0u8; 40];
        assert!(matches!(
            PartyAttrs::decode_group_list(&buf),
            Err(DecodeError::Truncated(52, 40))
        ));
    }

    #[test]
    fn item_max_decodes_legacy_and_wide_capacity() {
        let mut buf = vec![0u8; ItemMax::SIZE];

        buf[0] = 81;

        buf[1] = 81;
        let wide_off = 18 + 14 + 2;
        buf[wide_off..wide_off + 2].copy_from_slice(&201u16.to_le_bytes());

        let wide_off = 18 + 14 + 10 * 2;
        buf[wide_off..wide_off + 2].copy_from_slice(&81u16.to_le_bytes());

        let m = ItemMax::decode(&buf).unwrap();
        assert_eq!(
            m.capacities[0], 80,
            "Inventory: legacy fallback, +1 inverted"
        );
        assert_eq!(
            m.capacities[1], 200,
            "Mog Safe: wide takes precedence, +1 inverted"
        );
        assert_eq!(m.capacities[10], 80, "Wardrobe2: wide-only, +1 inverted");
        assert_eq!(
            m.capacities[17], 0,
            "Recycle Bin: zeroed (disabled sentinel)"
        );
    }

    #[test]
    fn item_max_disabled_container_stays_zero() {
        let mut buf = vec![0u8; ItemMax::SIZE];

        buf[4] = 21;

        let m = ItemMax::decode(&buf).unwrap();
        assert_eq!(m.capacities[4], 20, "moglocker: legacy decoded with -1");
        assert_eq!(
            m.capacities[1], 0,
            "fully-disabled stays at 0, no underflow"
        );
    }

    #[test]
    fn item_max_truncated_errors() {
        let buf = vec![0u8; ItemMax::SIZE - 1];
        assert!(matches!(
            ItemMax::decode(&buf),
            Err(DecodeError::Truncated(96, _))
        ));
    }

    #[test]
    fn item_same_decodes_state_and_flags() {
        let mut buf = vec![0u8; ItemSame::SIZE];
        buf[0] = 0;
        buf[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
        let s = ItemSame::decode(&buf).unwrap();
        assert_eq!(s.state, ItemSameState::StillLoading);
        assert_eq!(s.flags, 0xCAFE);

        buf[0] = 1;
        let s = ItemSame::decode(&buf).unwrap();
        assert_eq!(s.state, ItemSameState::AllLoaded);
    }

    #[test]
    fn item_num_decodes() {
        let mut buf = vec![0u8; ItemNum::SIZE];
        buf[0..4].copy_from_slice(&12345u32.to_le_bytes());
        buf[4] = 0;
        buf[5] = 7;
        buf[6] = 1;
        let n = ItemNum::decode(&buf).unwrap();
        assert_eq!(n.quantity, 12345);
        assert_eq!(n.category, 0);
        assert_eq!(n.index, 7);
        assert_eq!(n.lock_flg, 1);
    }

    #[test]
    fn item_list_decodes() {
        let mut buf = vec![0u8; ItemList::SIZE];
        buf[0..4].copy_from_slice(&1u32.to_le_bytes());
        buf[4..6].copy_from_slice(&4112u16.to_le_bytes());
        buf[6] = 5;
        buf[7] = 12;
        buf[8] = 0;
        let l = ItemList::decode(&buf).unwrap();
        assert_eq!(l.quantity, 1);
        assert_eq!(l.item_no, 4112);
        assert_eq!(l.category, 5);
        assert_eq!(l.index, 12);
    }

    #[test]
    fn item_attr_decodes_with_extdata() {
        let mut buf = vec![0u8; ItemAttr::SIZE];
        buf[0..4].copy_from_slice(&1u32.to_le_bytes());
        buf[4..8].copy_from_slice(&500_000u32.to_le_bytes());
        buf[8..10].copy_from_slice(&8000u16.to_le_bytes());
        buf[10] = 0;
        buf[11] = 3;
        buf[12] = 0;
        for (i, b) in buf[13..37].iter_mut().enumerate() {
            *b = i as u8;
        }
        let a = ItemAttr::decode(&buf).unwrap();
        assert_eq!(a.quantity, 1);
        assert_eq!(a.price, 500_000);
        assert_eq!(a.item_no, 8000);
        assert_eq!(a.category, 0);
        assert_eq!(a.index, 3);
        assert_eq!(a.extdata[0], 0);
        assert_eq!(a.extdata[23], 23);
    }

    #[test]
    fn item_attr_truncated_errors() {
        let buf = vec![0u8; ItemAttr::SIZE - 1];
        assert!(matches!(
            ItemAttr::decode(&buf),
            Err(DecodeError::Truncated(37, _))
        ));
    }

    #[test]
    fn try_extract_name_recovers_char_npc_with_update_name() {
        use crate::map::s2c;

        let mut buf = vec![0u8; 64];
        buf[6] = 0x08;
        buf[0x30..0x30 + 9].copy_from_slice(b"Sigli-Sea");
        let name = PosHead::try_extract_name(s2c::CHAR_NPC, &buf);
        assert_eq!(name.as_deref(), Some("Sigli-Sea"));
    }

    #[test]
    fn try_extract_name_returns_none_without_update_name() {
        use crate::map::s2c;

        let mut buf = vec![0u8; 64];
        buf[0x30..0x30 + 5].copy_from_slice(b"Junk!");
        assert!(PosHead::try_extract_name(s2c::CHAR_NPC, &buf).is_none());
    }

    #[test]
    fn try_extract_name_char_npc_renamed_low_targid_shift() {
        use crate::map::s2c;

        let mut buf = vec![0u8; 68];
        buf[6] = 0x08;
        buf[0x30] = 0x01;
        buf[0x31..0x31 + 12].copy_from_slice(b"Big Bad Bee\0");
        let name = PosHead::try_extract_name(s2c::CHAR_NPC, &buf);
        assert_eq!(name.as_deref(), Some("Big Bad Bee"));
    }

    #[test]
    fn try_extract_name_char_pc_uses_fixed_offset_with_send_flag() {
        use crate::map::s2c;

        let mut buf = vec![0u8; 0x60];
        buf[6] = 0x08;
        buf[0x56..0x56 + 6].copy_from_slice(b"Cleric");
        let name = PosHead::try_extract_name(s2c::CHAR_PC, &buf);
        assert_eq!(name.as_deref(), Some("Cleric"));
    }

    #[test]
    fn try_extract_name_char_pc_rejects_when_send_flag_clear() {
        use crate::map::s2c;

        let mut buf = vec![0u8; 0x60];
        buf[6] = 0x01;
        buf[0x56..0x56 + 6].copy_from_slice(b"Junked");
        assert!(PosHead::try_extract_name(s2c::CHAR_PC, &buf).is_none());
    }

    #[test]
    fn entity_set_name_decodes_trust_name() {
        let mut buf = vec![0u8; 0x28];
        buf[0] = 0x03;
        buf[1] = 0x05;
        buf[2..4].copy_from_slice(&0x07F2u16.to_le_bytes());
        buf[4..8].copy_from_slice(&0x0123_45F2u32.to_le_bytes());
        buf[8..10].copy_from_slice(&0x0042u16.to_le_bytes());
        buf[0x14..0x14 + 13].copy_from_slice(b"Mihli Aliapoh");

        let ent = EntitySetName::decode(&buf).unwrap();
        assert_eq!(ent.targid, 0x07F2);
        assert_eq!(ent.id, 0x0123_45F2);
        assert_eq!(ent.master_targid, 0x0042);
        assert_eq!(ent.name.as_deref(), Some("Mihli Aliapoh"));
    }

    #[test]
    fn entity_set_name_short_name_rejected() {
        let mut buf = vec![0u8; 0x28];
        buf[0] = 0x03;
        buf[4..8].copy_from_slice(&0x42u32.to_le_bytes());
        buf[0x14..0x14 + 2].copy_from_slice(b"Mi");

        let ent = EntitySetName::decode(&buf).unwrap();
        assert!(ent.name.is_none());
    }

    #[test]
    fn entity_set_name_truncated_errors() {
        let buf = vec![0u8; EntitySetName::SIZE - 1];
        assert!(matches!(
            EntitySetName::decode(&buf),
            Err(DecodeError::Truncated(_, _))
        ));
    }

    #[test]
    fn pet_sync_decodes_full_pet_record() {
        let mut buf = vec![0u8; 0x28];
        buf[0] = 0x04;
        buf[2..4].copy_from_slice(&0x0001u16.to_le_bytes());
        buf[4..8].copy_from_slice(&0x0010_0001u32.to_le_bytes());
        buf[8..10].copy_from_slice(&0x07A5u16.to_le_bytes());
        buf[0x0A] = 87;
        buf[0x0B] = 60;
        buf[0x0C..0x0E].copy_from_slice(&1234u16.to_le_bytes());
        buf[0x10..0x14].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        buf[0x14..0x14 + 11].copy_from_slice(b"Crab Family");

        let pet = PetSync::decode(&buf).unwrap();
        assert_eq!(pet.owner_targid, 0x0001);
        assert_eq!(pet.owner_id, 0x0010_0001);
        assert_eq!(pet.pet_targid, 0x07A5);
        assert_eq!(pet.hp_pct, 87);
        assert_eq!(pet.mp_pct, 60);
        assert_eq!(pet.tp, 1234);
        assert_eq!(pet.bt_target_id, 0xDEAD_BEEF);
        assert_eq!(pet.name.as_deref(), Some("Crab Family"));
    }

    #[test]
    fn pet_sync_despawn_variant_skips_pet_fields() {
        let mut buf = vec![0u8; 0x18];
        buf[0] = 0x04;
        buf[2..4].copy_from_slice(&0x0001u16.to_le_bytes());
        buf[4..8].copy_from_slice(&0x0010_0001u32.to_le_bytes());

        let pet = PetSync::decode(&buf).unwrap();
        assert_eq!(pet.owner_targid, 0x0001);
        assert_eq!(pet.owner_id, 0x0010_0001);
        assert_eq!(pet.pet_targid, 0);
        assert_eq!(pet.hp_pct, 0);
        assert!(pet.name.is_none());
    }

    #[test]
    fn pet_sync_truncated_below_owner_header_errors() {
        let buf = vec![0u8; PetSync::DESPAWN_SIZE - 1];
        assert!(matches!(
            PetSync::decode(&buf),
            Err(DecodeError::Truncated(_, _))
        ));
    }

    #[test]
    fn char_sync_decodes_ids() {
        let mut buf = vec![0u8; CharSync::SIZE];
        buf[0] = 0x02;
        buf[1] = 0x09;
        buf[2..4].copy_from_slice(&0x07F0u16.to_le_bytes());
        buf[4..8].copy_from_slice(&0x0123_4567u32.to_le_bytes());

        let sync = CharSync::decode(&buf).unwrap();
        assert_eq!(sync.targid, 0x07F0);
        assert_eq!(sync.id, 0x0123_4567);
    }

    #[test]
    fn weather_packet_decodes_fields() {
        let mut buf = [0u8; WeatherPacket::SIZE];
        buf[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        buf[4..6].copy_from_slice(&6u16.to_le_bytes());
        buf[6..8].copy_from_slice(&0x0123u16.to_le_bytes());
        let w = WeatherPacket::decode(&buf).unwrap();
        assert_eq!(w.start_time, 0xDEAD_BEEF);
        assert_eq!(w.weather_number, 6);
        assert_eq!(w.offset_time, 0x0123);
    }

    #[test]
    fn weather_packet_truncated_returns_err() {
        let buf = [0u8; WeatherPacket::SIZE - 1];
        assert!(matches!(
            WeatherPacket::decode(&buf),
            Err(DecodeError::Truncated(WeatherPacket::SIZE, n)) if n == WeatherPacket::SIZE - 1
        ));
    }

    #[test]
    fn equip_list_decodes_field_order() {
        let buf = [0x05u8, 0x04, 0x08, 0x00];
        let e = EquipList::decode(&buf).expect("decode");
        assert_eq!(e.container_index, 5);
        assert_eq!(e.equip_slot, 4);
        assert_eq!(e.container, 8);
    }

    #[test]
    fn equip_list_truncated_returns_err() {
        let buf = [0u8; EquipList::SIZE - 1];
        assert!(matches!(
            EquipList::decode(&buf),
            Err(DecodeError::Truncated(EquipList::SIZE, n)) if n == EquipList::SIZE - 1
        ));
    }

    #[test]
    fn magic_data_known_ids_picks_set_bits() {
        let mut buf = [0u8; MagicData::SIZE];

        buf[0] = 0b1000_0001;
        buf[1] = 0b0000_0001;
        buf[2] = 0b0000_0010;
        buf[127] = 0b1000_0000;
        let m = MagicData::decode(&buf).unwrap();
        assert_eq!(m.known_ids(), vec![0, 7, 8, 17, 1023]);
        assert!(m.is_known(0));
        assert!(m.is_known(7));
        assert!(m.is_known(1023));
        assert!(!m.is_known(1));

        assert!(!m.is_known(u16::MAX));
    }

    #[test]
    fn magic_data_truncated_returns_err() {
        let buf = [0u8; MagicData::SIZE - 1];
        assert!(matches!(
            MagicData::decode(&buf),
            Err(DecodeError::Truncated(MagicData::SIZE, n)) if n == MagicData::SIZE - 1
        ));
    }

    #[test]
    fn command_data_splits_into_four_bitsets() {
        let mut buf = [0u8; CommandData::SIZE];

        buf[0] = 0xA1;
        buf[64] = 0xA2;
        buf[128] = 0xA3;
        buf[192] = 0xA4;
        let c = CommandData::decode(&buf).unwrap();
        assert_eq!(c.weapon_skills[0], 0xA1);
        assert_eq!(c.job_abilities[0], 0xA2);
        assert_eq!(c.pet_abilities[0], 0xA3);
        assert_eq!(c.traits[0], 0xA4);

        assert_eq!(c.weapon_skills.len(), 64);
        assert_eq!(c.job_abilities.len(), 64);
        assert_eq!(c.pet_abilities.len(), 64);
        assert_eq!(c.traits.len(), 32);
    }

    #[test]
    fn command_data_truncated_returns_err() {
        let buf = [0u8; CommandData::SIZE - 1];
        assert!(matches!(
            CommandData::decode(&buf),
            Err(DecodeError::Truncated(CommandData::SIZE, n)) if n == CommandData::SIZE - 1
        ));
    }

    #[test]
    fn char_status_decodes_death_counter_and_homepoint_seconds() {
        // Full wire body: GP_SERV_SERVERSTATUS is 0x60 incl. the 4-byte sub-header, so
        // the body (which `sub.data` exposes) is 0x5C. Sizing to that — rather than just
        // past dead_counter2 — keeps the fields anchored if a trailing field shifts.
        let mut body = vec![0u8; 0x5C];
        body[CharStatus::UNIQUE_NO_OFFSET..CharStatus::UNIQUE_NO_OFFSET + 4]
            .copy_from_slice(&0x000B_C5EBu32.to_le_bytes());
        // Flags0 with hpp (bits 16..24) == 0 → KO'd.
        body[CharStatus::FLAGS0_OFFSET..CharStatus::FLAGS0_OFFSET + 4]
            .copy_from_slice(&0u32.to_le_bytes());
        // 60 * (360 + 1800): 30 min until the forced home-point warp.
        body[CharStatus::DEAD_COUNTER1_OFFSET..CharStatus::DEAD_COUNTER1_OFFSET + 4]
            .copy_from_slice(&129_600u32.to_le_bytes());
        body[CharStatus::DEAD_COUNTER2_OFFSET..CharStatus::DEAD_COUNTER2_OFFSET + 4]
            .copy_from_slice(&0x1122_3344u32.to_le_bytes());

        let cs = CharStatus::decode(&body).unwrap();
        assert_eq!(cs.unique_no, 0x000B_C5EB);
        assert_eq!(cs.hpp, 0);
        assert_eq!(cs.dead_counter1, 129_600);
        assert_eq!(cs.dead_counter2, 0x1122_3344);
        assert_eq!(cs.seconds_until_homepoint(), 1800);
    }

    #[test]
    fn char_status_homepoint_seconds_boundaries() {
        let secs = |dc1: u32| {
            CharStatus {
                unique_no: 0,
                hpp: 0,
                dead_counter1: dc1,
                dead_counter2: 0,
            }
            .seconds_until_homepoint()
        };
        // Fresh death: 60 * (6min + 60min) → full 60 min remaining.
        assert_eq!(secs(60 * (360 + 3600)), 3600);
        // At/below the 6-min padding floor, saturate at 0 instead of wrapping.
        assert_eq!(secs(60 * 360), 0);
        assert_eq!(secs(0), 0);
    }

    #[test]
    fn char_status_extracts_hpp_from_flags0() {
        let mut body = vec![0u8; CharStatus::DEAD_COUNTER2_OFFSET + 4];
        body[CharStatus::FLAGS0_OFFSET..CharStatus::FLAGS0_OFFSET + 4]
            .copy_from_slice(&(75u32 << 16).to_le_bytes());
        assert_eq!(CharStatus::decode(&body).unwrap().hpp, 75);
    }

    #[test]
    fn char_status_truncated_returns_err() {
        let need = CharStatus::DEAD_COUNTER2_OFFSET + 4;
        let buf = vec![0u8; need - 1];
        assert!(matches!(
            CharStatus::decode(&buf),
            Err(DecodeError::Truncated(n, have)) if n == need && have == need - 1
        ));
    }
}
