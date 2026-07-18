pub mod animation {

    pub const NONE: u8 = 0;

    pub const ATTACK: u8 = 1;

    pub const HEALING: u8 = 33;

    pub const SIT: u8 = 47;

    // ANIMATIONTYPE, vendor/server/src/map/entities/baseentity.h:60. The server writes
    // these into the entity's server_status (the 0x0D/0x37 animation byte) and broadcasts
    // them; the client maps each to the matching fsh* model clip (research/xim Actor.kt:361).
    // The pre-overhaul (38-43,50) and current (56-62) fishing systems share fsh0..fsh6.
    pub const FISHING_FISH_OLD: u8 = 38;
    pub const FISHING_CAUGHT_OLD: u8 = 39;
    pub const FISHING_ROD_BREAK_OLD: u8 = 40;
    pub const FISHING_LINE_BREAK_OLD: u8 = 41;
    pub const FISHING_MONSTER_OLD: u8 = 42;
    pub const FISHING_STOP_OLD: u8 = 43;
    pub const FISHING_START_OLD: u8 = 50;

    pub const FISHING_START: u8 = 56;
    pub const FISHING_FISH: u8 = 57;
    pub const FISHING_CAUGHT: u8 = 58;
    pub const FISHING_ROD_BREAK: u8 = 59;
    pub const FISHING_LINE_BREAK: u8 = 60;
    pub const FISHING_MONSTER: u8 = 61;
    pub const FISHING_STOP: u8 = 62;

    /// Phase index 0..=6 of a fishing macro-state animation byte (current or pre-overhaul),
    /// or `None` if the byte is not a fishing animation. The index selects the `fsh<n>` clip:
    /// 0=cast/wait, 1=fighting, 2=caught fish, 3=rod break, 4=line break, 5=caught monster,
    /// 6=stop/cancel.
    pub fn fishing_phase(animation: u8) -> Option<u8> {
        Some(match animation {
            FISHING_START | FISHING_START_OLD => 0,
            FISHING_FISH | FISHING_FISH_OLD => 1,
            FISHING_CAUGHT | FISHING_CAUGHT_OLD => 2,
            FISHING_ROD_BREAK | FISHING_ROD_BREAK_OLD => 3,
            FISHING_LINE_BREAK | FISHING_LINE_BREAK_OLD => 4,
            FISHING_MONSTER | FISHING_MONSTER_OLD => 5,
            FISHING_STOP | FISHING_STOP_OLD => 6,
            _ => return None,
        })
    }
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

    // Head-look target = the targid the entity has selected, packed into Flags0
    // bits 17..31. Both 0x0D (char_update.cpp `Flags0.facetarget = m_TargID`) and
    // 0x0E (entity_update.cpp `ref<uint16>(0x1A) = m_TargID << 1`) write it here.
    // Distinct from bt_target_id (the combat-claim UniqueNo).
    const FACETARGET_SHIFT: u32 = 17;
    const FACETARGET_MASK: u32 = 0x7FFF;

    pub fn facetarget(&self) -> u16 {
        ((self.flags0 >> Self::FACETARGET_SHIFT) & Self::FACETARGET_MASK) as u16
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

/// s2c 0x00A Mog House cluster. Body offsets follow
/// vendor/server/src/map/packets/s2c/0x00a_login.h:115-127; `login_state` values are
/// the SAVE_LOGIN_STATE enum (h:50-59). `map_number` is the MH interior MODEL id
/// (GetMogHouseModelID, 0x00a_login.cpp:35-72), NOT a zone id; `mog_zone_flag` is
/// only assigned in the non-MH branch (.cpp: CanUseMisc(MISC_MOGMENU)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerLoginMyroom {
    pub login_state: u32,
    pub sub_map_number: u8,
    pub map_number: u16,
    pub exit_bit: u8,
    pub mog_zone_flag: u8,
}

impl ServerLoginMyroom {
    pub const LOGIN_STATE_MYROOM: u32 = 1;
    pub const LOGIN_STATE_GAME: u32 = 2;

    /// MyroomMapNumber sentinel "not in a Mog House" (0x00a_login.cpp non-MH branch).
    pub const MYROOM_NONE: u16 = 0x01FF;

    /// MyroomSubMapNumber value while on the MH second floor (0x00a_login.cpp MH branch).
    pub const SUB_MAP_2F: u8 = 0x02;

    /// LSB reuses LoginState MYROOM + this MyroomMapNumber for ZONE_FERETORY
    /// (Monstrosity), which is not a Mog House server-side
    /// (0x00a_login.cpp:234-239 sets it with no m_moghouseID).
    pub const MYROOM_FERETORY: u16 = 0x02D9;

    pub const LOGIN_STATE_OFFSET: usize = 0x7C;
    pub const SUB_MAP_NUMBER_OFFSET: usize = 0xA4;
    pub const MAP_NUMBER_OFFSET: usize = 0xA6;
    pub const EXIT_BIT_OFFSET: usize = 0xAA;
    pub const MOG_ZONE_FLAG_OFFSET: usize = 0xAB;
    pub const MIN_LEN: usize = Self::MOG_ZONE_FLAG_OFFSET + 1;

    fn decode(body: &[u8]) -> Option<Self> {
        if body.len() < Self::MIN_LEN {
            return None;
        }
        Some(Self {
            login_state: u32::from_le_bytes(
                body[Self::LOGIN_STATE_OFFSET..Self::LOGIN_STATE_OFFSET + 4]
                    .try_into()
                    .unwrap(),
            ),
            sub_map_number: body[Self::SUB_MAP_NUMBER_OFFSET],
            map_number: u16::from_le_bytes(
                body[Self::MAP_NUMBER_OFFSET..Self::MAP_NUMBER_OFFSET + 2]
                    .try_into()
                    .unwrap(),
            ),
            exit_bit: body[Self::EXIT_BIT_OFFSET],
            mog_zone_flag: body[Self::MOG_ZONE_FLAG_OFFSET],
        })
    }

    /// The MH interior model id, only when the server actually placed the player
    /// in a Mog House: LoginState MYROOM excluding the [`Self::MYROOM_NONE`]
    /// sentinel and the [`Self::MYROOM_FERETORY`] alias.
    pub fn myroom_model(&self) -> Option<u16> {
        (self.login_state == Self::LOGIN_STATE_MYROOM
            && self.map_number != Self::MYROOM_NONE
            && self.map_number != Self::MYROOM_FERETORY)
            .then_some(self.map_number)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ServerLogin {
    pub unique_no: u32,
    pub act_index: u16,
    pub zone_no: u16,

    pub game_time: Option<u32>,

    pub pos_head: PosHead,

    pub music_num: Option<[u16; 5]>,

    pub myroom: Option<ServerLoginMyroom>,

    pub zone_in_event: Option<ZoneInEvent>,
}

/// Zone-in cutscene carried inside s2c 0x00A LOGIN: when `currentEvent` is
/// already set at zone-in (e.g. the new-character intro, a Mog House 2F unlock
/// CS), LSB delivers it via the login packet instead of a 0x032/0x034 push
/// (vendor/server/src/map/packets/s2c/0x00a_login.cpp:183-192). The client
/// must answer with 0x05B `End` (`EventPara` = this `event_para`) or the char
/// stays InEvent server-side — zonelines/logout rejected — and the CS re-fires
/// on every subsequent login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneInEvent {
    /// `EventNum`: the zone id the event belongs to.
    pub event_num: u16,
    /// `EventPara`: the cutscene/event id (`currentEvent->eventId`).
    pub event_para: u16,
    /// `EventMode`: `currentEvent->eventFlags` low half.
    pub event_mode: u16,
}

impl ServerLogin {
    pub const SIZE: usize = 48;

    pub const MUSIC_NUM_OFFSET: usize = 0x52;
    pub const MUSIC_NUM_SIZE: usize = 5 * 2;

    pub const GAME_TIME_OFFSET: usize = 0x38;

    pub const EVENT_NUM_OFFSET: usize = 0x5E;
    pub const EVENT_PARA_OFFSET: usize = 0x60;
    pub const EVENT_MODE_OFFSET: usize = 0x62;

    /// `PosHead.server_status` while a zone-in event is pending — the packet's
    /// event fields are only written then, and event id 0 is a real cutscene
    /// (Bastok Markets intro), so presence keys off the status byte
    /// (0x00a_login.cpp:191, ANIMATION_EVENT in
    /// vendor/server/src/map/entities/baseentity.h:66).
    pub const SERVER_STATUS_EVENT: u8 = 4;

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
        let zone_in_event = (pos_head.server_status == Self::SERVER_STATUS_EVENT
            && body.len() >= Self::EVENT_MODE_OFFSET + 2)
            .then(|| ZoneInEvent {
                event_num: u16::from_le_bytes(
                    body[Self::EVENT_NUM_OFFSET..Self::EVENT_NUM_OFFSET + 2]
                        .try_into()
                        .unwrap(),
                ),
                event_para: u16::from_le_bytes(
                    body[Self::EVENT_PARA_OFFSET..Self::EVENT_PARA_OFFSET + 2]
                        .try_into()
                        .unwrap(),
                ),
                event_mode: u16::from_le_bytes(
                    body[Self::EVENT_MODE_OFFSET..Self::EVENT_MODE_OFFSET + 2]
                        .try_into()
                        .unwrap(),
                ),
            });
        Ok(Self {
            unique_no: pos_head.unique_no,
            act_index: pos_head.act_index,
            zone_no: zone_u32 as u16,
            game_time,
            pos_head,
            music_num,
            myroom: ServerLoginMyroom::decode(body),
            zone_in_event,
        })
    }
}

/// s2c 0x037 GP_SERV_SERVERSTATUS (char status). Only the fields we consume are
/// decoded: the subject id, its HP%, the death/homepoint counters, the animation
/// (`server_status`) byte, and the fishing hook-delay timer.
/// vendor/server/src/map/packets/char_status.cpp
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharStatus {
    pub unique_no: u32,
    pub hpp: u8,
    pub dead_counter1: u32,
    pub dead_counter2: u32,
    /// The self player's animation byte (ANIMATIONTYPE); see [`animation`]. Mirrors the
    /// `server_status` that 0x0D broadcasts for other players.
    pub server_status: u8,
    /// Frames the client waits before the cast settles and it requests a hook check.
    /// Only meaningful while `server_status == animation::FISHING_START`. 0 if the packet
    /// was truncated before this field.
    pub fishing_timer: u8,
    /// Movement speed, 0 while bound
    /// (vendor/server/src/map/packets/char_status.cpp `Flags1.Speed`).
    pub speed: u16,
}

impl CharStatus {
    pub const UNIQUE_NO_OFFSET: usize = 0x20;
    pub const FLAGS0_OFFSET: usize = 0x24;
    pub const SPEED_OFFSET: usize = 0x28;
    pub const SERVER_STATUS_OFFSET: usize = 0x2C;
    pub const DEAD_COUNTER1_OFFSET: usize = 0x38;
    pub const DEAD_COUNTER2_OFFSET: usize = 0x3C;
    pub const FISHING_TIMER_OFFSET: usize = 0x46;
    pub const SPEED_MASK: u16 = 0x0FFF;
    pub const MIN_LEN: usize = Self::DEAD_COUNTER2_OFFSET + 4;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        let need = Self::MIN_LEN;
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
            server_status: body[Self::SERVER_STATUS_OFFSET],
            fishing_timer: body.get(Self::FISHING_TIMER_OFFSET).copied().unwrap_or(0),
            speed: u16::from_le_bytes([body[Self::SPEED_OFFSET], body[Self::SPEED_OFFSET + 1]])
                & Self::SPEED_MASK,
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

const _: () = assert!(CharStatus::SPEED_OFFSET + 2 <= CharStatus::MIN_LEN);

/// s2c 0x061 GP_SERV_COMMAND_CLISTATUS — the self-character stat block.
/// Field offsets follow the `CLISTATUS` struct in
/// vendor/server/src/map/packets/s2c/0x061_clistatus.h (mirror of
/// research/XiPackets/world/server/0x0061). `bp_base`/`bp_adj` are STR, DEX, VIT,
/// AGI, INT, MND, CHR in order; `bp_adj` is the signed gear/buff delta retail shows
/// as the "+N" beside each stat. `def_elem` is Fire, Ice, Wind, Earth, Lightning,
/// Water, Light, Dark. The struct declares `atk`/`def` as int16_t, but they are
/// sourced from the non-negative ATT()/DEF() (.cpp:63-64), so we read them as u16.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CliStatus {
    pub hp_max: u32,
    pub mp_max: u32,
    pub mjob_no: u8,
    pub mjob_lv: u8,
    pub sjob_no: u8,
    pub sjob_lv: u8,
    pub bp_base: [u16; 7],
    pub bp_adj: [i16; 7],
    pub attack: u16,
    pub defense: u16,
    pub def_elem: [i16; 8],
    pub ilvl: u8,
}

impl CliStatus {
    // vendor/server/src/map/packets/s2c/0x061_clistatus.h:45-82 — the four job bytes
    // sit between mpmax (@4) and exp_now (@12).
    const MJOB_NO_OFFSET: usize = 8;
    const MJOB_LV_OFFSET: usize = 9;
    const SJOB_NO_OFFSET: usize = 10;
    const SJOB_LV_OFFSET: usize = 11;
    const ILVL_OFFSET: usize = 81;
    const NEEDED: usize = Self::ILVL_OFFSET + 1;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::NEEDED {
            return Err(DecodeError::Truncated(Self::NEEDED, body.len()));
        }
        let rd32 = |o: usize| u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        let rd16 = |o: usize| u16::from_le_bytes([body[o], body[o + 1]]);
        let rdi16 = |o: usize| i16::from_le_bytes([body[o], body[o + 1]]);
        let mut bp_base = [0u16; 7];
        let mut bp_adj = [0i16; 7];
        for i in 0..7 {
            bp_base[i] = rd16(16 + i * 2);
            bp_adj[i] = rdi16(30 + i * 2);
        }
        let mut def_elem = [0i16; 8];
        for (i, e) in def_elem.iter_mut().enumerate() {
            *e = rdi16(48 + i * 2);
        }
        Ok(Self {
            hp_max: rd32(0),
            mp_max: rd32(4),
            mjob_no: body[Self::MJOB_NO_OFFSET],
            mjob_lv: body[Self::MJOB_LV_OFFSET],
            sjob_no: body[Self::SJOB_NO_OFFSET],
            sjob_lv: body[Self::SJOB_LV_OFFSET],
            bp_base,
            bp_adj,
            attack: rd16(44),
            defense: rd16(46),
            def_elem,
            ilvl: body[Self::ILVL_OFFSET],
        })
    }
}

/// s2c 0x01B GP_SERV_COMMAND_JOB_INFO — per-job levels + unlocked-jobs bitmask for
/// the self character. Body offsets follow the GP_MYROOM_DANCER struct in
/// vendor/server/src/map/packets/s2c/0x01b_job_info.h:28-62 (filled in .cpp:30-57).
/// `job_levels` reads `job_lev2` (the full `jobs.job[24]` memcpy, index = JOBTYPE);
/// the legacy `job_lev[16]` @0x0C truncates at 16 jobs and is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobInfo {
    pub mjob_no: u8,
    pub sjob_no: u8,
    /// Bit N set = JOBTYPE N unlocked. Bit 0 is the subjob-feature flag, not a job
    /// (`sjobflg = jobs.unlocked & 1`, 0x01b_job_info.cpp).
    pub unlocked: u32,
    pub sub_job_unlocked: bool,
    pub job_levels: [u8; Self::MAX_JOBTYPE],
    pub hp_max: i32,
    pub mp_max: i32,
    pub sjobflg: u8,
}

impl JobInfo {
    /// MAX_JOBTYPE, vendor/server/src/map/entities/battleentity.h (JOBTYPE 1=WAR..23=MON).
    pub const MAX_JOBTYPE: usize = 24;

    pub const MJOB_NO_OFFSET: usize = 0x04;
    pub const SJOB_NO_OFFSET: usize = 0x07;
    pub const UNLOCKED_OFFSET: usize = 0x08;
    pub const HP_MAX_OFFSET: usize = 0x38;
    pub const MP_MAX_OFFSET: usize = 0x3C;
    pub const SJOBFLG_OFFSET: usize = 0x40;
    pub const JOB_LEVELS_OFFSET: usize = 0x44;
    pub const MIN_LEN: usize = Self::JOB_LEVELS_OFFSET + Self::MAX_JOBTYPE;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::MIN_LEN {
            return Err(DecodeError::Truncated(Self::MIN_LEN, body.len()));
        }
        let rdi32 = |o: usize| i32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        let unlocked = u32::from_le_bytes(
            body[Self::UNLOCKED_OFFSET..Self::UNLOCKED_OFFSET + 4]
                .try_into()
                .unwrap(),
        );
        let mut job_levels = [0u8; Self::MAX_JOBTYPE];
        job_levels.copy_from_slice(
            &body[Self::JOB_LEVELS_OFFSET..Self::JOB_LEVELS_OFFSET + Self::MAX_JOBTYPE],
        );
        Ok(Self {
            mjob_no: body[Self::MJOB_NO_OFFSET],
            sjob_no: body[Self::SJOB_NO_OFFSET],
            unlocked,
            sub_job_unlocked: unlocked & 1 != 0,
            job_levels,
            hp_max: rdi32(Self::HP_MAX_OFFSET),
            mp_max: rdi32(Self::MP_MAX_OFFSET),
            sjobflg: body[Self::SJOBFLG_OFFSET],
        })
    }
}

/// s2c 0x115 GP_SERV_COMMAND_FISH. The server sends this once a fish bites to hand the
/// client every parameter it needs to simulate the catch mini-game locally.
/// vendor/server/src/map/packets/s2c/0x115_fish.h, research/XiPackets/world/server/0x0115
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FishPacket {
    /// The fish's starting (and maximum) stamina.
    pub stamina: u16,
    /// Base reaction window for an arrow press (client adjusts by intuition).
    pub arrow_delay: u16,
    /// Per-tick stamina regen, biased by 128 server-side (`regen - 128`).
    pub regen: u16,
    /// How often the fish thrashes left/right (client scales by 20).
    pub move_frequency: u16,
    /// Stamina removed on a correct, on-time arrow press.
    pub arrow_damage: u16,
    /// Stamina restored on a missed/late arrow press.
    pub arrow_regen: u16,
    /// Time limit to land the fish, in seconds (client scales by 60 → frames).
    pub time: u16,
    /// Angler-sense flags: bit0 alters the music/arrow timing, bit1 triggers the
    /// "intuition" light-bulb animation when the fish is first hooked.
    pub angler_sense: u8,
    /// Fishing intuition; reflected back to the server and used for the golden arrows.
    pub intuition: u32,
}

impl FishPacket {
    pub const SIZE: usize = 20;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let rd16 = |o: usize| u16::from_le_bytes([body[o], body[o + 1]]);
        Ok(Self {
            stamina: rd16(0),
            arrow_delay: rd16(2),
            regen: rd16(4),
            move_frequency: rd16(6),
            arrow_damage: rd16(8),
            arrow_regen: rd16(10),
            time: rd16(12),
            angler_sense: body[14],
            intuition: u32::from_le_bytes([body[16], body[17], body[18], body[19]]),
        })
    }

    /// `true` when the angler-sense bit that drives the "intuition" hook animation
    /// (the light bulb) is set. vendor `fish->sense2`.
    pub fn shows_intuition(&self) -> bool {
        (self.angler_sense >> 1) & 1 == 1
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

// vendor/server/src/map/packets/s2c/0x057_weather.h:32-37 (StartTime u32, WeatherNumber, WeatherOffsetTime u16)
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

/// s2c 0x055 GP_SERV_COMMAND_SCENARIOITEM (key items). One packet carries a
/// single 512-bit table: 16 u32 `GetItemFlag` (owned) followed by 16 u32
/// `LookItemFlag` (examined), then the `TableIndex`. A key-item's global id is
/// `table_index * 512 + bit`.
/// vendor/server/src/map/packets/s2c/0x055_scenarioitem.h
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScenarioItem {
    pub table_index: u16,
    pub get_flags: [u32; Self::WORDS],
    pub look_flags: [u32; Self::WORDS],
}

impl ScenarioItem {
    pub const WORDS: usize = 16;
    pub const BITS_PER_TABLE: usize = Self::WORDS * 32;
    pub const SIZE: usize = Self::WORDS * 4 * 2 + 4;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let rd = |o: usize| u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        let mut get_flags = [0u32; Self::WORDS];
        let mut look_flags = [0u32; Self::WORDS];
        for i in 0..Self::WORDS {
            get_flags[i] = rd(i * 4);
            look_flags[i] = rd(Self::WORDS * 4 + i * 4);
        }
        let table_index = u16::from_le_bytes([body[Self::WORDS * 8], body[Self::WORDS * 8 + 1]]);
        Ok(Self {
            table_index,
            get_flags,
            look_flags,
        })
    }

    pub fn owned_key_item_ids(&self) -> Vec<u16> {
        let base = self.table_index as usize * Self::BITS_PER_TABLE;
        let mut ids = Vec::new();
        for (word, &flags) in self.get_flags.iter().enumerate() {
            for bit in 0..32 {
                if flags & (1 << bit) != 0 {
                    ids.push((base + word * 32 + bit) as u16);
                }
            }
        }
        ids
    }
}

pub fn decode_scenario_item(sub: &SubPacket<'_>) -> Result<ScenarioItem, DecodeError> {
    ScenarioItem::decode(sub.data)
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
    /// MogExpansionFlag: MH second floor unlocked (`mhflag & 0x20`), byte 0x27 of the
    /// full packet = body 0x23. vendor/server/src/map/packets/char_sync.cpp:61.
    /// `None` when the packet is too short to carry it.
    pub mh_2f_unlocked: Option<bool>,
}

impl CharSync {
    pub const SUB_TYPE: u8 = 0x02;
    pub const SIZE: usize = 8;

    pub const MH_2F_UNLOCKED_OFFSET: usize = 0x23;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            targid: u16::from_le_bytes(body[2..4].try_into().unwrap()),
            id: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            mh_2f_unlocked: body.get(Self::MH_2F_UNLOCKED_OFFSET).map(|&b| b != 0),
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

#[derive(Debug, Clone, Copy)]
pub struct ItemMax {
    pub capacities: [u16; Self::CONTAINER_COUNT],
}

impl ItemMax {
    /// One capacity per LSB CONTAINER_ID (LOC_INVENTORY..=LOC_RECYCLEBIN),
    /// vendor/server/src/map/item_container.h:32-49.
    pub const CONTAINER_COUNT: usize = 18;
    pub const SIZE: usize = 96;
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let mut capacities = [0u16; Self::CONTAINER_COUNT];
        // Fall back to the legacy u8 array only when the whole wide array is
        // absent (pre-widening servers). A per-slot fallback would erase LSB's
        // "container disabled" sentinel — ItemNum2 = 0 while the legacy byte
        // stays sized, e.g. a lapsed Mog Locker lease
        // (vendor/server/src/map/packets/s2c/0x01c_item_max.cpp:52-57).
        let wide_at = |i: usize| {
            let off = 18 + 14 + i * 2;
            u16::from_le_bytes(body[off..off + 2].try_into().unwrap())
        };
        let wide_present = (0..Self::CONTAINER_COUNT).any(|i| wide_at(i) != 0);
        for (i, cap) in capacities.iter_mut().enumerate() {
            let raw = if wide_present {
                wide_at(i)
            } else {
                body[i] as u16
            };
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

/// GP_POST_BOX_STATE item payload of the full-form s2c 0x04B
/// (vendor/server/src/map/packets/s2c/0x04b_pbx_result.h:57-67). `counterpart`
/// is the GC_PBOX name field: sender (Incoming box) or recipient (Outgoing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PbxBoxState {
    pub stat: u32,
    pub counterpart: Option<String>,
    pub item_sub_no: i32,
    pub item_no: u16,
    pub kind: i32,
    pub stack: u32,
    pub extra: [u8; 28],
}

/// GP_SERV_COMMAND_PBX_RESULT (vendor/server/src/map/packets/s2c/
/// 0x04b_pbx_result.h:71-94). `state` is present only in the full 0x58 form;
/// the short 0x14 form carries just the header fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PbxResult {
    pub command: u8,
    pub box_no: i8,
    pub post_work_no: i8,
    pub item_work_no: i8,
    pub item_stacks: i32,
    pub result: u8,
    pub res_param1: i8,
    pub res_param2: i8,
    pub res_param3: i8,
    pub state: Option<PbxBoxState>,
}

impl PbxResult {
    /// setSize(0x14) minus the 4-byte subpacket header (0x04b_pbx_result.cpp:31).
    pub const SHORT_SIZE: usize = 16;
    /// setSize(0x58) minus the header (0x04b_pbx_result.cpp:67).
    pub const FULL_SIZE: usize = 84;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SHORT_SIZE {
            return Err(DecodeError::Truncated(Self::SHORT_SIZE, body.len()));
        }
        let state = (body.len() >= Self::FULL_SIZE).then(|| {
            let mut extra = [0u8; 28];
            extra.copy_from_slice(&body[56..84]);
            PbxBoxState {
                stat: u32::from_le_bytes(body[12..16].try_into().unwrap()),
                // Not read_name_slot: its 3-char minimum would drop short
                // auction-house senders ("AH…"), which retail special-cases.
                counterpart: {
                    let raw = &body[16..32];
                    let n = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                    (n > 0).then(|| String::from_utf8_lossy(&raw[..n]).into_owned())
                },
                item_sub_no: i32::from_le_bytes(body[40..44].try_into().unwrap()),
                item_no: u16::from_le_bytes(body[44..46].try_into().unwrap()),
                kind: i32::from_le_bytes(body[48..52].try_into().unwrap()),
                stack: u32::from_le_bytes(body[52..56].try_into().unwrap()),
                extra,
            }
        });
        Ok(Self {
            command: body[0],
            box_no: body[1] as i8,
            post_work_no: body[2] as i8,
            item_work_no: body[3] as i8,
            item_stacks: i32::from_le_bytes(body[4..8].try_into().unwrap()),
            result: body[8],
            res_param1: body[9] as i8,
            res_param2: body[10] as i8,
            res_param3: body[11] as i8,
            state,
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

    /// Pins the GP_SERV_COMMAND_PBX_RESULT layout to LSB's PacketData struct
    /// (vendor/server/src/map/packets/s2c/0x04b_pbx_result.h): header fields at
    /// 0-11, GP_POST_BOX_STATE at 12 (Stat, name[16], request id/time, sub id,
    /// ItemNo, Kind, Stack, Data[28]) in the 0x58 full form.
    #[test]
    fn pbx_result_reads_lsb_offsets_full_form() {
        let mut buf = vec![0u8; PbxResult::FULL_SIZE];
        buf[0] = 0x06; // Command = Recv
        buf[1] = 1; // BoxNo = Incoming
        buf[2] = 3; // PostWorkNo
        buf[3] = 0xFF; // ItemWorkNo = -1
        buf[4..8].copy_from_slice(&(-1i32).to_le_bytes()); // ItemStacks
        buf[8] = 0x01; // Result = OK
        buf[9] = 1; // ResParam1 = count
        buf[10] = 0xFF; // ResParam2 = -1
        buf[11] = 0xFF; // ResParam3 = -1
        buf[12..16].copy_from_slice(&7u32.to_le_bytes()); // Stat = incoming
        buf[16..20].copy_from_slice(b"Atti"); // From (NUL-padded)
        buf[40..44].copy_from_slice(&0i32.to_le_bytes()); // sub id
        buf[44..46].copy_from_slice(&5075u16.to_le_bytes()); // ItemNo
        buf[52..56].copy_from_slice(&2u32.to_le_bytes()); // Stack
        buf[56] = 0xAB; // Data[0] (m_extra)

        let r = PbxResult::decode(&buf).expect("decodes");
        assert_eq!(r.command, 0x06);
        assert_eq!(r.box_no, 1);
        assert_eq!(r.post_work_no, 3);
        assert_eq!(r.item_work_no, -1);
        assert_eq!(r.item_stacks, -1);
        assert_eq!(r.result, 0x01);
        assert_eq!(r.res_param1, 1);
        assert_eq!(r.res_param2, -1);
        let s = r.state.expect("full form carries GP_POST_BOX_STATE");
        assert_eq!(s.stat, 7);
        assert_eq!(s.counterpart.as_deref(), Some("Atti"));
        assert_eq!(s.item_no, 5075);
        assert_eq!(s.stack, 2);
        assert_eq!(s.extra[0], 0xAB);
    }

    /// The short 0x14 form (4-arg LSB ctor) has no box state; a Check response
    /// carries the new-item count in ResParam2 (Incoming) / ResParam3 (Outgoing)
    /// (0x04b_pbx_result.cpp:44-54).
    #[test]
    fn pbx_result_short_form_check_counts() {
        let mut buf = vec![0u8; PbxResult::SHORT_SIZE];
        buf[0] = 0x05; // Check
        buf[1] = 1; // Incoming
        buf[2] = 0xFF;
        buf[3] = 0xFF;
        buf[4..8].copy_from_slice(&(-1i32).to_le_bytes());
        buf[8] = 0x01; // Result = OK
        buf[9] = 0xFF;
        buf[10] = 2; // ResParam2 = 2 new items
        buf[11] = 0xFF;

        let r = PbxResult::decode(&buf).expect("decodes");
        assert_eq!(r.command, 0x05);
        assert_eq!(r.res_param2, 2);
        assert!(r.state.is_none(), "short form has no state");

        assert!(PbxResult::decode(&buf[..12]).is_err(), "truncated rejects");
    }

    #[test]
    fn clistatus_reads_stat_block_offsets() {
        let mut buf = vec![0u8; 84];
        buf[0..4].copy_from_slice(&1946u32.to_le_bytes()); // hp_max
        buf[4..8].copy_from_slice(&1295u32.to_le_bytes()); // mp_max
        buf[8] = 5; // mjob_no (RDM)
        buf[9] = 75; // mjob_lv
        buf[10] = 4; // sjob_no (BLM)
        buf[11] = 37; // sjob_lv
        for i in 0..7 {
            buf[16 + i * 2..18 + i * 2].copy_from_slice(&((10 + i as u16) * 5).to_le_bytes());
            buf[30 + i * 2..32 + i * 2].copy_from_slice(&((i as i16 + 1) * 7).to_le_bytes());
        }
        buf[44..46].copy_from_slice(&1048u16.to_le_bytes()); // attack
        buf[46..48].copy_from_slice(&1006u16.to_le_bytes()); // defense
        buf[48..50].copy_from_slice(&(-15i16).to_le_bytes()); // fire resist
        buf[81] = 119; // ilvl

        let cs = CliStatus::decode(&buf).expect("decodes");
        assert_eq!(cs.hp_max, 1946);
        assert_eq!(cs.mp_max, 1295);
        assert_eq!(cs.mjob_no, 5);
        assert_eq!(cs.mjob_lv, 75);
        assert_eq!(cs.sjob_no, 4);
        assert_eq!(cs.sjob_lv, 37);
        assert_eq!(cs.bp_base[0], 50, "STR base");
        assert_eq!(cs.bp_base[6], 80, "CHR base");
        assert_eq!(cs.bp_adj[0], 7, "STR gear delta");
        assert_eq!(cs.attack, 1048);
        assert_eq!(cs.defense, 1006);
        assert_eq!(cs.def_elem[0], -15, "fire resist signed");
        assert_eq!(cs.ilvl, 119);
        assert!(
            CliStatus::decode(&buf[..80]).is_err(),
            "truncation rejected"
        );
    }

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
    fn server_login_zone_in_event_keys_off_status_byte_not_event_id() {
        let mut buf = vec![0u8; 0x100];
        buf[44..48].copy_from_slice(&234u32.to_le_bytes());
        buf[ServerLogin::EVENT_NUM_OFFSET..ServerLogin::EVENT_NUM_OFFSET + 2]
            .copy_from_slice(&234u16.to_le_bytes());
        // Bastok Markets intro cutscene is event id 0 — a zeroed EventPara must
        // still decode as an event when the status byte says so.
        buf[ServerLogin::EVENT_MODE_OFFSET..ServerLogin::EVENT_MODE_OFFSET + 2]
            .copy_from_slice(&32u16.to_le_bytes());

        let no_event = ServerLogin::decode(&buf).unwrap();
        assert_eq!(no_event.zone_in_event, None);

        buf[27] = ServerLogin::SERVER_STATUS_EVENT;
        let with_event = ServerLogin::decode(&buf).unwrap();
        assert_eq!(
            with_event.zone_in_event,
            Some(ZoneInEvent {
                event_num: 234,
                event_para: 0,
                event_mode: 32,
            })
        );
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
    fn server_login_myroom_cluster_roundtrips() {
        let mut buf = vec![0u8; 0x100];
        buf[44..48].copy_from_slice(&230u32.to_le_bytes());
        buf[ServerLoginMyroom::LOGIN_STATE_OFFSET..ServerLoginMyroom::LOGIN_STATE_OFFSET + 4]
            .copy_from_slice(&ServerLoginMyroom::LOGIN_STATE_MYROOM.to_le_bytes());
        buf[ServerLoginMyroom::SUB_MAP_NUMBER_OFFSET] = ServerLoginMyroom::SUB_MAP_2F;
        buf[ServerLoginMyroom::MAP_NUMBER_OFFSET..ServerLoginMyroom::MAP_NUMBER_OFFSET + 2]
            .copy_from_slice(&617u16.to_le_bytes());
        buf[ServerLoginMyroom::EXIT_BIT_OFFSET] = 3;
        buf[ServerLoginMyroom::MOG_ZONE_FLAG_OFFSET] = 1;

        let l = ServerLogin::decode(&buf).unwrap();
        let myroom = l.myroom.expect("full-size body carries the cluster");
        assert_eq!(myroom.login_state, ServerLoginMyroom::LOGIN_STATE_MYROOM);
        assert_eq!(myroom.sub_map_number, ServerLoginMyroom::SUB_MAP_2F);
        assert_eq!(myroom.map_number, 617);
        assert_eq!(myroom.exit_bit, 3);
        assert_eq!(myroom.mog_zone_flag, 1);
        assert_eq!(myroom.myroom_model(), Some(617));
    }

    #[test]
    fn server_login_truncated_body_yields_no_myroom() {
        let mut buf = vec![0u8; ServerLoginMyroom::MIN_LEN - 1];
        buf[44..48].copy_from_slice(&230u32.to_le_bytes());
        let l = ServerLogin::decode(&buf).unwrap();
        assert_eq!(l.zone_no, 230);
        assert!(l.myroom.is_none());
    }

    #[test]
    fn server_login_myroom_jeuno_model_decodes() {
        let mut buf = vec![0u8; 0x100];
        buf[44..48].copy_from_slice(&243u32.to_le_bytes());
        buf[ServerLoginMyroom::LOGIN_STATE_OFFSET..ServerLoginMyroom::LOGIN_STATE_OFFSET + 4]
            .copy_from_slice(&ServerLoginMyroom::LOGIN_STATE_MYROOM.to_le_bytes());
        buf[ServerLoginMyroom::MAP_NUMBER_OFFSET..ServerLoginMyroom::MAP_NUMBER_OFFSET + 2]
            .copy_from_slice(&0x0100u16.to_le_bytes());

        let myroom = ServerLogin::decode(&buf).unwrap().myroom.unwrap();
        assert_eq!(myroom.myroom_model(), Some(0x0100));
    }

    #[test]
    fn server_login_myroom_model_gated_on_state_and_sentinel() {
        let mut buf = vec![0u8; 0x100];
        buf[ServerLoginMyroom::LOGIN_STATE_OFFSET..ServerLoginMyroom::LOGIN_STATE_OFFSET + 4]
            .copy_from_slice(&ServerLoginMyroom::LOGIN_STATE_GAME.to_le_bytes());
        buf[ServerLoginMyroom::MAP_NUMBER_OFFSET..ServerLoginMyroom::MAP_NUMBER_OFFSET + 2]
            .copy_from_slice(&ServerLoginMyroom::MYROOM_NONE.to_le_bytes());
        let myroom = ServerLogin::decode(&buf).unwrap().myroom.unwrap();
        assert_eq!(myroom.login_state, ServerLoginMyroom::LOGIN_STATE_GAME);
        assert_eq!(myroom.myroom_model(), None, "GAME state carries no model");

        buf[ServerLoginMyroom::LOGIN_STATE_OFFSET..ServerLoginMyroom::LOGIN_STATE_OFFSET + 4]
            .copy_from_slice(&ServerLoginMyroom::LOGIN_STATE_MYROOM.to_le_bytes());
        let myroom = ServerLogin::decode(&buf).unwrap().myroom.unwrap();
        assert_eq!(
            myroom.myroom_model(),
            None,
            "MYROOM with the 0x01FF sentinel carries no model"
        );

        buf[ServerLoginMyroom::MAP_NUMBER_OFFSET..ServerLoginMyroom::MAP_NUMBER_OFFSET + 2]
            .copy_from_slice(&ServerLoginMyroom::MYROOM_FERETORY.to_le_bytes());
        let myroom = ServerLogin::decode(&buf).unwrap().myroom.unwrap();
        assert_eq!(
            myroom.myroom_model(),
            None,
            "Feretory MYROOM alias is not a Mog House"
        );
    }

    /// Pins the myroom cluster to LSB's GP_SERV_COMMAND_LOGIN PacketData layout
    /// (vendor/server/src/map/packets/s2c/0x00a_login.h:96-131; body offsets, no
    /// sub-packet header) so an offset edit can't pass the roundtrip tests, which
    /// build buffers through these same consts.
    #[test]
    fn myroom_cluster_offsets_and_sentinels_match_lsb_login_layout() {
        assert_eq!(ServerLoginMyroom::LOGIN_STATE_OFFSET, 0x7C);
        assert_eq!(ServerLoginMyroom::SUB_MAP_NUMBER_OFFSET, 0xA4);
        assert_eq!(ServerLoginMyroom::MAP_NUMBER_OFFSET, 0xA6);
        assert_eq!(ServerLoginMyroom::EXIT_BIT_OFFSET, 0xAA);
        assert_eq!(ServerLoginMyroom::MOG_ZONE_FLAG_OFFSET, 0xAB);
        assert_eq!(ServerLoginMyroom::LOGIN_STATE_MYROOM, 1, "SAVE_LOGIN_STATE");
        assert_eq!(ServerLoginMyroom::LOGIN_STATE_GAME, 2, "SAVE_LOGIN_STATE");
        assert_eq!(ServerLoginMyroom::MYROOM_NONE, 0x01FF);
        assert_eq!(ServerLoginMyroom::SUB_MAP_2F, 0x02);
        assert_eq!(ServerLoginMyroom::MYROOM_FERETORY, 0x02D9);
    }

    /// Pins JobInfo to LSB's GP_MYROOM_DANCER layout
    /// (vendor/server/src/map/packets/s2c/0x01b_job_info.h:28-45; job_lev2, not
    /// the legacy job_lev[16] @0x0C) and MAX_JOBTYPE
    /// (vendor/server/src/map/entities/battleentity.h:100), since the decode
    /// tests build buffers through these same consts.
    #[test]
    fn job_info_offsets_match_gp_myroom_dancer_layout() {
        assert_eq!(JobInfo::MJOB_NO_OFFSET, 0x04);
        assert_eq!(JobInfo::SJOB_NO_OFFSET, 0x07);
        assert_eq!(JobInfo::UNLOCKED_OFFSET, 0x08);
        assert_eq!(JobInfo::HP_MAX_OFFSET, 0x38);
        assert_eq!(JobInfo::MP_MAX_OFFSET, 0x3C);
        assert_eq!(JobInfo::SJOBFLG_OFFSET, 0x40);
        assert_eq!(JobInfo::JOB_LEVELS_OFFSET, 0x44);
        assert_eq!(JobInfo::MAX_JOBTYPE, 24);
    }

    /// Pins the 2F-unlock byte to LSB's full-packet offset 0x27 minus the 4-byte
    /// sub-packet header (vendor/server/src/map/packets/char_sync.cpp:61).
    #[test]
    fn char_sync_2f_flag_sits_at_lsb_packet_byte_0x27() {
        assert_eq!(CharSync::MH_2F_UNLOCKED_OFFSET, 0x27 - 4);
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
    fn pos_head_extracts_facetarget_from_flags0() {
        // facetarget occupies Flags0 bits 17..31; targid 0x1A2 must round-trip
        // and not bleed into the low MovTime/RunMode/GroundFlag/KingFlag bits.
        let mut buf = vec![0u8; PosHead::SIZE_WITH_BT_TARGET];
        let flags0 = (0x01A2u32 << 17) | 0x0001_FFFF;
        buf[20..24].copy_from_slice(&flags0.to_le_bytes());
        let h = PosHead::decode(&buf).unwrap();
        assert_eq!(h.facetarget(), 0x01A2);
    }

    #[test]
    fn pos_head_zero_flags0_has_no_facetarget() {
        let buf = vec![0u8; PosHead::SIZE];
        let h = PosHead::decode(&buf).unwrap();
        assert_eq!(h.facetarget(), 0);
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
            m.capacities[1], 200,
            "Mog Safe: wide takes precedence, +1 inverted"
        );
        assert_eq!(m.capacities[10], 80, "Wardrobe2: wide-only, +1 inverted");
        assert_eq!(
            m.capacities[17], 0,
            "Recycle Bin: zeroed (disabled sentinel)"
        );
    }

    /// LSB's only ItemNum2 = 0 emitter is a DISABLED container (a lapsed Mog
    /// Locker lease keeps its legacy byte sized), so once any wide value is
    /// present a zero must stay zero rather than fall back per-slot
    /// (vendor/server/src/map/packets/s2c/0x01c_item_max.cpp:52-57).
    #[test]
    fn item_max_wide_zero_is_the_disable_sentinel_not_a_fallback() {
        let mut buf = vec![0u8; ItemMax::SIZE];
        buf[0] = 31;
        buf[4] = 31; // lapsed locker: legacy still sized...
        let wide_off = 18 + 14;
        buf[wide_off..wide_off + 2].copy_from_slice(&31u16.to_le_bytes());
        // ...but ItemNum2[LOC_MOGLOCKER] stays 0.

        let m = ItemMax::decode(&buf).unwrap();
        assert_eq!(m.capacities[0], 30);
        assert_eq!(m.capacities[4], 0, "wide 0 = disabled, no legacy fallback");
    }

    #[test]
    fn item_max_falls_back_to_legacy_only_when_wide_is_absent() {
        let mut buf = vec![0u8; ItemMax::SIZE];

        buf[0] = 81;
        buf[4] = 21;

        let m = ItemMax::decode(&buf).unwrap();
        assert_eq!(m.capacities[0], 80, "pre-widening server: legacy decoded");
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
        assert_eq!(
            sync.mh_2f_unlocked, None,
            "minimal body does not reach the 2F byte"
        );
    }

    #[test]
    fn char_sync_reads_mh_2f_unlock_bit() {
        // char_sync.cpp builds a 0x28-byte packet → 0x24-byte body.
        let mut buf = vec![0u8; 0x24];
        buf[0] = CharSync::SUB_TYPE;
        buf[4..8].copy_from_slice(&0x0123_4567u32.to_le_bytes());

        let sync = CharSync::decode(&buf).unwrap();
        assert_eq!(sync.mh_2f_unlocked, Some(false));

        buf[CharSync::MH_2F_UNLOCKED_OFFSET] = 1;
        let sync = CharSync::decode(&buf).unwrap();
        assert_eq!(sync.mh_2f_unlocked, Some(true));
    }

    #[test]
    fn job_info_decodes_synthetic_body() {
        let mut buf = vec![0u8; 0x80];
        buf[JobInfo::MJOB_NO_OFFSET] = 5; // RDM
        buf[JobInfo::SJOB_NO_OFFSET] = 4; // BLM
                                          // bit 0 = subjob feature, bits 1..6 = WAR..THF unlocked.
        let unlocked: u32 = 0b0111_1111;
        buf[JobInfo::UNLOCKED_OFFSET..JobInfo::UNLOCKED_OFFSET + 4]
            .copy_from_slice(&unlocked.to_le_bytes());
        buf[JobInfo::HP_MAX_OFFSET..JobInfo::HP_MAX_OFFSET + 4]
            .copy_from_slice(&1946i32.to_le_bytes());
        buf[JobInfo::MP_MAX_OFFSET..JobInfo::MP_MAX_OFFSET + 4]
            .copy_from_slice(&1295i32.to_le_bytes());
        buf[JobInfo::SJOBFLG_OFFSET] = 1;
        for j in 0..JobInfo::MAX_JOBTYPE {
            buf[JobInfo::JOB_LEVELS_OFFSET + j] = j as u8 * 3;
        }
        // Legacy truncated job_lev[16] @0x0C left zeroed: proves we read job_lev2.
        let info = JobInfo::decode(&buf).unwrap();
        assert_eq!(info.mjob_no, 5);
        assert_eq!(info.sjob_no, 4);
        assert_eq!(info.unlocked, unlocked);
        assert!(info.sub_job_unlocked);
        assert_eq!(info.hp_max, 1946);
        assert_eq!(info.mp_max, 1295);
        assert_eq!(info.sjobflg, 1);
        assert_eq!(info.job_levels[1], 3, "WAR");
        assert_eq!(
            info.job_levels[22], 66,
            "RUN — beyond the legacy 16-job array"
        );
    }

    #[test]
    fn job_info_truncated_errors() {
        let buf = vec![0u8; JobInfo::MIN_LEN - 1];
        assert!(matches!(
            JobInfo::decode(&buf),
            Err(DecodeError::Truncated(_, _))
        ));
    }

    #[test]
    fn job_info_without_subjob_flag() {
        let mut buf = vec![0u8; JobInfo::MIN_LEN];
        buf[JobInfo::UNLOCKED_OFFSET..JobInfo::UNLOCKED_OFFSET + 4]
            .copy_from_slice(&0b0000_0110u32.to_le_bytes());
        let info = JobInfo::decode(&buf).unwrap();
        assert!(!info.sub_job_unlocked);
        assert_eq!(info.sjobflg, 0);
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
        body[CharStatus::SERVER_STATUS_OFFSET] = animation::FISHING_START;
        body[CharStatus::FISHING_TIMER_OFFSET] = 42;

        let cs = CharStatus::decode(&body).unwrap();
        assert_eq!(cs.unique_no, 0x000B_C5EB);
        assert_eq!(cs.hpp, 0);
        assert_eq!(cs.dead_counter1, 129_600);
        assert_eq!(cs.dead_counter2, 0x1122_3344);
        assert_eq!(cs.seconds_until_homepoint(), 1800);
        assert_eq!(cs.server_status, animation::FISHING_START);
        assert_eq!(cs.fishing_timer, 42);
    }

    #[test]
    fn char_status_fishing_timer_zero_when_truncated_before_field() {
        // dead_counter2 is the last guaranteed field; fishing_timer sits past it and must
        // default to 0 rather than panic when the body stops short.
        let body = vec![0u8; CharStatus::DEAD_COUNTER2_OFFSET + 4];
        let cs = CharStatus::decode(&body).unwrap();
        assert_eq!(cs.fishing_timer, 0);
    }

    #[test]
    fn fish_packet_decodes_minigame_params() {
        let mut body = vec![0u8; FishPacket::SIZE];
        body[0..2].copy_from_slice(&200u16.to_le_bytes()); // stamina
        body[2..4].copy_from_slice(&5u16.to_le_bytes()); // arrow_delay
        body[4..6].copy_from_slice(&130u16.to_le_bytes()); // regen
        body[6..8].copy_from_slice(&3u16.to_le_bytes()); // move_frequency
        body[8..10].copy_from_slice(&40u16.to_le_bytes()); // arrow_damage
        body[10..12].copy_from_slice(&10u16.to_le_bytes()); // arrow_regen
        body[12..14].copy_from_slice(&30u16.to_le_bytes()); // time
        body[14] = 0b11; // angler_sense: both bits set
        body[16..20].copy_from_slice(&0x0000_0064u32.to_le_bytes()); // intuition

        let f = FishPacket::decode(&body).unwrap();
        assert_eq!(f.stamina, 200);
        assert_eq!(f.arrow_delay, 5);
        assert_eq!(f.regen, 130);
        assert_eq!(f.move_frequency, 3);
        assert_eq!(f.arrow_damage, 40);
        assert_eq!(f.arrow_regen, 10);
        assert_eq!(f.time, 30);
        assert_eq!(f.intuition, 100);
        assert!(f.shows_intuition());

        assert!(matches!(
            FishPacket::decode(&[0u8; FishPacket::SIZE - 1]),
            Err(DecodeError::Truncated(n, _)) if n == FishPacket::SIZE
        ));
    }

    #[test]
    fn char_status_homepoint_seconds_boundaries() {
        let secs = |dc1: u32| {
            CharStatus {
                unique_no: 0,
                hpp: 0,
                dead_counter1: dc1,
                dead_counter2: 0,
                server_status: 0,
                fishing_timer: 0,
                speed: 0,
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

    #[test]
    fn char_status_decodes_speed_and_masks_high_nibble() {
        let mut body = vec![0u8; CharStatus::MIN_LEN];
        body[CharStatus::SPEED_OFFSET..CharStatus::SPEED_OFFSET + 2]
            .copy_from_slice(&0xA078u16.to_le_bytes());
        assert_eq!(CharStatus::decode(&body).unwrap().speed, 0x078);
    }

    #[test]
    fn char_status_base_speed_decodes() {
        let mut body = vec![0u8; CharStatus::MIN_LEN];
        body[CharStatus::SPEED_OFFSET..CharStatus::SPEED_OFFSET + 2]
            .copy_from_slice(&0x0032u16.to_le_bytes());
        assert_eq!(CharStatus::decode(&body).unwrap().speed, 50);
    }

    #[test]
    fn char_status_bound_speed_zero_decodes() {
        let body = vec![0u8; CharStatus::MIN_LEN];
        assert_eq!(CharStatus::decode(&body).unwrap().speed, 0);
    }
}

#[cfg(test)]
mod scenario_item_tests {
    use super::*;

    fn body_with(table_index: u16, get: &[(usize, u32)], look: &[(usize, u32)]) -> Vec<u8> {
        let mut body = vec![0u8; ScenarioItem::SIZE];
        for &(word, flags) in get {
            body[word * 4..word * 4 + 4].copy_from_slice(&flags.to_le_bytes());
        }
        for &(word, flags) in look {
            let o = ScenarioItem::WORDS * 4 + word * 4;
            body[o..o + 4].copy_from_slice(&flags.to_le_bytes());
        }
        let o = ScenarioItem::WORDS * 8;
        body[o..o + 2].copy_from_slice(&table_index.to_le_bytes());
        body
    }

    #[test]
    fn decodes_table_index_and_flags() {
        let body = body_with(2, &[(0, 0b101), (3, 1 << 7)], &[(0, 0b10)]);
        let si = ScenarioItem::decode(&body).expect("decode");
        assert_eq!(si.table_index, 2);
        assert_eq!(si.get_flags[0], 0b101);
        assert_eq!(si.get_flags[3], 1 << 7);
        assert_eq!(si.look_flags[0], 0b10);
    }

    #[test]
    fn owned_ids_account_for_table_offset() {
        let body = body_with(2, &[(0, 0b101), (3, 1 << 7)], &[]);
        let si = ScenarioItem::decode(&body).expect("decode");
        let base = 2 * ScenarioItem::BITS_PER_TABLE;
        assert_eq!(
            si.owned_key_item_ids(),
            vec![base as u16, (base + 2) as u16, (base + 3 * 32 + 7) as u16,]
        );
    }

    #[test]
    fn truncated_body_is_error() {
        let buf = vec![0u8; ScenarioItem::SIZE - 1];
        assert!(matches!(
            ScenarioItem::decode(&buf),
            Err(DecodeError::Truncated(n, have)) if n == ScenarioItem::SIZE && have == ScenarioItem::SIZE - 1
        ));
    }
}
