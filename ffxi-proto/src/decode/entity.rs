use super::*;
use std::fmt;

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

impl PosHead {
    pub(crate) const SIZE: usize = 40;

    pub(crate) const SIZE_WITH_BT_TARGET: usize = 44;

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

    // `Flags6.MountIndex` — the MOUNTTYPE this character last mounted. LSB's own
    // comment warns it stays set after dismounting
    // (vendor/server/src/map/packets/char_update.cpp,
    // CCharUpdatePacket::updateWith), so it is only
    // meaningful once `server_status` says the character is mounted. Flags6 sits
    // past `PosHead`, inside the `SendFlg.General` block, so a position-only
    // update does not carry it.
    const FLAGS6_OFFSET: usize = 64;
    const MOUNT_INDEX_SHIFT: u32 = 4;
    const MOUNT_INDEX_MASK: u32 = 0xFF;

    /// The `MOUNTTYPE` in a 0x0D `CHAR_PC`, or `None` when the packet stops short
    /// of `Flags6`. `MOUNT_CHOCOBO` is 0, which is also the not-mounted value.
    pub fn mount_index(body: &[u8]) -> Option<u8> {
        let b = body.get(Self::FLAGS6_OFFSET..Self::FLAGS6_OFFSET + 4)?;
        let flags6 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        Some(((flags6 >> Self::MOUNT_INDEX_SHIFT) & Self::MOUNT_INDEX_MASK) as u8)
    }

    pub fn decode_char_npc(body: &[u8]) -> Result<(Self, u32), DecodeError> {
        let head = Self::decode(body)?;
        Ok((head, head.bt_target_id))
    }

    pub(crate) const UPDATE_DESPAWN: u8 = 0x20;

    pub fn is_entity_despawn(opcode: u16, body: &[u8]) -> bool {
        use crate::map::s2c;
        (opcode == s2c::CHAR_PC || opcode == s2c::CHAR_NPC)
            && body
                .get(6)
                .copied()
                .is_some_and(|mask| mask & Self::UPDATE_DESPAWN != 0)
    }

    /// `PacketNameLength` (vendor/server/src/common/utils.h:71) — 15 chars plus
    /// the terminator, the cap on every name LSB copies into 0x0D/0x0E.
    const NAME_LEN: usize = 16;

    /// `sendflags_t.Name` — `UPDATE_NAME`, the ordinary "a name follows" bit
    /// (vendor/server/src/map/entities/baseentity.h:173).
    const SEND_NAME: u8 = 0x08;
    /// `sendflags_t.Name2` (entity_update.cpp:52). Set on every equipped-model
    /// spawn, which is why it alone does not imply a name is present.
    const SEND_NAME2: u8 = 0x40;

    pub fn try_extract_name(opcode: u16, body: &[u8]) -> Option<String> {
        use crate::map::s2c;

        let &send_flag = body.get(6)?;
        if opcode == s2c::CHAR_PC {
            const NAME_START: usize = 0x56;
            if send_flag & Self::SEND_NAME == 0 {
                return None;
            }
            return body.get(NAME_START..).and_then(read_name_slot);
        }
        if opcode != s2c::CHAR_NPC {
            return None;
        }

        // Two layouts, and they are flagged by different bits
        // (vendor/server/src/map/packets/entity_update.cpp:539-587).
        //
        // A renamed dynamic entity (targid >= 0x700) spawning with an equipment
        // model grows the packet, memcpy's `look_t` over 0x30 and puts the name
        // at 0x44. Its mask is a literal 0x57 — Name2|Look|HP|Status|Pos, with
        // UPDATE_NAME *clear* — so gating on UPDATE_NAME alone drops it. Name2
        // rides every equipped spawn though, so the real discriminator is the
        // `ref<uint8>(0x18) = 0x01` marker plus the growth: a plain equipped
        // spawn is `setSize(0x48)` (entity_update.cpp:463) and stops short of
        // the name field.
        //
        // Every other rename writes 0x34, shifted to 0x35 for targid < 1024, and
        // does set UPDATE_NAME. All offsets are packet-relative, so they land 4
        // bytes lower here.
        const LONG_NAME_MARKER: usize = 0x18 - 4;
        const LONG_NAME_START: usize = 0x44 - 4;
        const STANDARD_START: usize = 0x34 - 4;
        const RENAMED_START: usize = 0x35 - 4;
        /// Body length of a plain `setSize(0x48)` equipped spawn — anything at
        /// or below this never carries the long name.
        const PLAIN_EQUIPPED_BODY_LEN: usize = 0x48 - 4;

        let long_name = send_flag & Self::SEND_NAME2 != 0
            && body.get(LONG_NAME_MARKER) == Some(&0x01)
            && body.len() > PLAIN_EQUIPPED_BODY_LEN;
        let standard = (send_flag & Self::SEND_NAME != 0).then(|| {
            if body.get(STANDARD_START) == Some(&0x01) {
                RENAMED_START
            } else {
                STANDARD_START
            }
        });

        // The 0x18 marker shares its offset with `loc.p.moving`, which UPDATE_POS
        // writes first, so a moving entity can raise it by accident. Ordering
        // rather than branching keeps a false hit costing one failed parse
        // instead of the name.
        [long_name.then_some(LONG_NAME_START), standard]
            .into_iter()
            .flatten()
            .find_map(|start| {
                let end = body.len().min(start + Self::NAME_LEN);
                read_name_slot(body.get(start..end)?)
            })
    }
}

/// The `Flags1`/`Flags2`/`Flags3` bitfields shared by `CHAR_PC` (0x0D) and
/// `CHAR_NPC` (0x0E), named per
/// `vendor/server/src/map/packets/char_update.cpp` (`flags1_t`, `flags2_t`,
/// `flags3_t`); `entity_update.cpp` declares the same three layouts for 0x0E. Only meaningful when the packet's General send-flag bit
/// (0x04) is set — the server refreshes the words in that block alone.
///
/// Drives the retail nameplate: colour selection
/// (research/XIClient/.../ActorTelemetry.cpp `NameColorSet`) and the icon
/// markers prefixed to the name
/// (research/XIClient/.../ActorTelemetry.cpp `GetPrimaryActorNameMarker`).
/// `untargetable` is the targetability authority, not a nameplate concern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CharFlags {
    pub monster: bool,
    pub lfg: bool,
    pub anonymous: bool,
    pub yell: bool,
    pub away: bool,
    pub play_online: bool,
    pub linkshell: bool,
    pub linkdead: bool,
    pub gm_level: u8,
    pub bazaar: bool,

    /// `Flags2.r/g/b`: the equipped linkshell's pearl colour, already expanded
    /// from the 4-bit Exdata channel by the server as `(c << 4) + 15`
    /// (`CCharUpdatePacket::updateWith`). Meaningless unless `linkshell` is set.
    pub linkshell_color: [u8; 3],

    pub charm: bool,
    pub gm_icon: bool,
    pub auto_party: bool,
    pub trust: bool,
    pub lfg_master: bool,
    pub pet: bool,

    /// `Flags3.BallistaTeam`: LSB writes `PChar->allegiance` here
    /// (`CCharUpdatePacket::updateWith`, `CEntityUpdatePacket::updateWith`).
    /// ALLEGIANCE_TYPE, so 0 =
    /// MOB, 1 = PLAYER, 2..6 = the nation/team values that select the ballista
    /// name colours and markers.
    pub allegiance: u8,

    pub new_character: bool,
    pub mentor: bool,

    /// `Flags1.TargetOffFlag` (bit 19): the server's untargetable bit. For
    /// NPC/MOB/PET/TRUST that word carries `m_flags` — LSB writes it at
    /// `ref<uint32>(0x21)` under UPDATE_HP, so ENTITYFLAGS
    /// `FLAG_UNTARGETABLE = 0x800` (vendor/server/src/map/entities/baseentity.h)
    /// lands exactly on this bit; for CHAR_PC it is char_update's explicit
    /// "Untargetable player" field. vendor/server/src/map/packets/
    /// entity_update.cpp `flags1_t`, char_update.cpp:312.
    pub untargetable: bool,
}

impl CharFlags {
    pub fn from_pos_head(head: &PosHead) -> Self {
        let (f1, f2, f3) = (head.flags1, head.flags2, head.flags3);
        Self {
            monster: bit(f1, flags1::MONSTER),
            lfg: bit(f1, flags1::LFG),
            anonymous: bit(f1, flags1::ANONYMOUS),
            yell: bit(f1, flags1::YELL),
            away: bit(f1, flags1::AWAY),
            play_online: bit(f1, flags1::PLAY_ONLINE),
            linkshell: bit(f1, flags1::LINKSHELL),
            linkdead: bit(f1, flags1::LINKDEAD),
            gm_level: field(f1, flags1::GM_LEVEL, flags1::GM_LEVEL_BITS) as u8,
            bazaar: bit(f1, flags1::BAZAAR),
            linkshell_color: [
                field(f2, flags2::LS_R, flags2::CHANNEL_BITS) as u8,
                field(f2, flags2::LS_G, flags2::CHANNEL_BITS) as u8,
                field(f2, flags2::LS_B, flags2::CHANNEL_BITS) as u8,
            ],
            charm: bit(f2, flags2::CHARM),
            gm_icon: bit(f2, flags2::GM_ICON),
            auto_party: bit(f2, flags2::AUTO_PARTY),
            trust: bit(f3, flags3::TRUST),
            lfg_master: bit(f3, flags3::LFG_MASTER),
            pet: bit(f3, flags3::PET),
            allegiance: field(f3, flags3::BALLISTA_TEAM, flags3::BALLISTA_TEAM_BITS) as u8,
            new_character: bit(f3, flags3::NEW_CHARACTER),
            mentor: bit(f3, flags3::MENTOR),
            untargetable: bit(f1, flags1::TARGET_OFF),
        }
    }
}

fn bit(word: u32, shift: u32) -> bool {
    word >> shift & 1 != 0
}

fn field(word: u32, shift: u32, width: u32) -> u32 {
    word >> shift & ((1 << width) - 1)
}

// vendor/server/src/map/packets/char_update.cpp `flags1_t`
mod flags1 {
    pub const MONSTER: u32 = 0;
    pub const LFG: u32 = 11;
    pub const ANONYMOUS: u32 = 12;
    pub const YELL: u32 = 13;
    pub const AWAY: u32 = 14;
    pub const PLAY_ONLINE: u32 = 16;
    pub const LINKSHELL: u32 = 17;
    pub const LINKDEAD: u32 = 18;
    pub const GM_LEVEL: u32 = 24;
    pub const GM_LEVEL_BITS: u32 = 3;
    pub const BAZAAR: u32 = 31;
    /// `TargetOffFlag` — bit 19 in both char_update.cpp and entity_update.cpp
    /// `flags1_t`. For NPC/MOB this is where `m_flags & FLAG_UNTARGETABLE`
    /// (0x800) lands: LSB writes the u16 at upstream offset 0x21, i.e. bytes
    /// 1-2 of this word.
    pub const TARGET_OFF: u32 = 19;
}

// vendor/server/src/map/packets/char_update.cpp `flags2_t`
mod flags2 {
    pub const LS_R: u32 = 0;
    pub const LS_G: u32 = 8;
    pub const LS_B: u32 = 16;
    pub const CHANNEL_BITS: u32 = 8;
    pub const CHARM: u32 = 27;
    pub const GM_ICON: u32 = 28;
    pub const AUTO_PARTY: u32 = 31;
}

// vendor/server/src/map/packets/char_update.cpp `flags3_t`
mod flags3 {
    pub const TRUST: u32 = 0;
    pub const LFG_MASTER: u32 = 1;
    pub const PET: u32 = 6;
    pub const BALLISTA_TEAM: u32 = 8;
    pub const BALLISTA_TEAM_BITS: u32 = 8;
    pub const NEW_CHARACTER: u32 = 23;
    pub const MENTOR: u32 = 24;
}

/// The FourCC a `MODEL_DOOR` entity carries in `CHAR_NPC` (0x0E) —
/// `GP_SERV_CHAR_NPC` `packet_data_2.DoorId`
/// (research/XIClient/.../Game/Net/Packets/s2c/0x00E.h `CharNpcTypeFields`).
/// LSB fills it with the entity's `npc_list.name`
/// (vendor/server/src/map/packets/entity_update.cpp
/// `CEntityUpdatePacket::updateWith`, `case MODEL_DOOR`).
///
/// The same FourCC is the `BlockID` of the door's MMB placement group in the
/// zone MZB and names the zone-DAT directory holding its `open`/`clos`
/// Scheduler routines, so this — not the entity name, which only rides an
/// `UPDATE_NAME` packet — is the join from wire entity to geometry.
#[derive(Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct DoorId([u8; 4]);

impl DoorId {
    pub const LEN: usize = 4;

    /// `BlockID == 0` is the MZB "not in a FourCC group" sentinel that
    /// `MmbPlacement::in_underscore_at_group` (ffxi-dat) tests, and it is what
    /// LSB leaves in the field for a door entity with an empty name.
    const UNGROUPED: u32 = 0;

    pub fn new(bytes: [u8; Self::LEN]) -> Option<Self> {
        (u32::from_le_bytes(bytes) != Self::UNGROUPED).then_some(Self(bytes))
    }

    pub const fn bytes(self) -> [u8; Self::LEN] {
        self.0
    }

    /// The MZB `BlockID` form. Retail reads the FourCC as a little-endian
    /// `int32` and tests `(unsigned char)BlockID` for the group prefix
    /// (research/XIClient/.../World/Zone/Terrain/ZoneLayoutData.cpp
    /// `InitUnderscoreAtStructs`), so this compares directly against
    /// `MmbPlacement::block_id`.
    pub const fn block_id(self) -> u32 {
        u32::from_le_bytes(self.0)
    }
}

impl fmt::Display for DoorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.escape_ascii())
    }
}

impl fmt::Debug for DoorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DoorId({self})")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
        door_id: Option<DoorId>,
    },

    Transport {
        size: u16,
    },
}

impl LookData {
    pub(crate) const LOOK_BODY_OFFSET: usize = 0x2C;

    /// `CharNpcTypeFields::field_30`, whose retail struct offsets are relative
    /// to `GP_SERV_POS_HEAD` — exactly this `body` — so
    /// `offsetof(GP_SERV_CHAR_NPC, Data) + offsetof(CharNpcGenericData, Extra)
    /// == 0x30` (research/XIClient/.../s2c/0x00E.h) is this constant verbatim.
    /// LSB writes the same bytes at packet 0x34, four past the `look.size` it
    /// puts at packet 0x30.
    pub(crate) const DOOR_ID_BODY_OFFSET: usize = 0x30;

    fn door_id(body: &[u8]) -> Option<DoorId> {
        let off = Self::DOOR_ID_BODY_OFFSET;
        let raw = body.get(off..off + DoorId::LEN)?;
        DoorId::new(raw.try_into().ok()?)
    }

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
            2 => Some(LookData::Door {
                size,
                door_id: Self::door_id(body),
            }),
            3 | 4 => Some(LookData::Transport { size }),
            _ => None,
        }
    }

    pub const CHAR_PC_GRAP_OFFSET: usize = 0x44;

    /// `GP_SERV_COMMAND_GRAP_LIST::PacketData` opens with `GrapIDTbl`
    /// (vendor/server/src/map/packets/s2c/0x051_grap_list.h), so the table sits
    /// at the start of the body.
    pub const GRAP_LIST_TBL_OFFSET: usize = 0;

    pub const GRAP_ID_TBL_SLOTS: usize = 9;
    pub const GRAP_ID_TBL_LEN: usize = Self::GRAP_ID_TBL_SLOTS * 2;

    /// Slot tag stripped from `GrapIDTbl[i]`: LSB writes `look.<slot> + 0x{i}000`
    /// (vendor/server/src/map/packets/s2c/0x051_grap_list.cpp:32-39 and
    /// vendor/server/src/map/packets/char_update.cpp), the same encoding in all
    /// three carriers of the table (0x00D CHAR_PC, 0x00A LOGIN, 0x051 GRAP_LIST).
    const GRAP_ID_MODEL_MASK: u16 = 0x0FFF;

    pub fn decode_char_pc(body: &[u8]) -> Option<Self> {
        Self::decode_grap_id_tbl(body, Self::CHAR_PC_GRAP_OFFSET)
    }

    pub fn decode_grap_list(body: &[u8]) -> Option<Self> {
        Self::decode_grap_id_tbl(body, Self::GRAP_LIST_TBL_OFFSET)
    }

    pub fn decode_grap_id_tbl(body: &[u8], off: usize) -> Option<Self> {
        if body.len() < off + Self::GRAP_ID_TBL_LEN {
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
            u16::from_le_bytes([body[p], body[p + 1]]) & Self::GRAP_ID_MODEL_MASK
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
    pub(crate) const ANIMATION_OFFSET: usize = 0x1B;
    pub(crate) const STATUS_OFFSET: usize = 0x1C;
    pub(crate) const ANIMATIONSUB_OFFSET: usize = 0x26;

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
    assert!(LookData::LOOK_BODY_OFFSET < LookData::DOOR_ID_BODY_OFFSET);
};

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

pub(super) fn read_name_slot(slot: &[u8]) -> Option<String> {
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
    pub(crate) const SIZE: usize = 8;

    pub(crate) const MH_2F_UNLOCKED_OFFSET: usize = 0x23;

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

    pub(crate) const SIZE: usize = 0x14;

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
    pub(crate) const DESPAWN_SIZE: usize = 8;

    pub(crate) const FULL_HEADER_SIZE: usize = 0x14;

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

#[cfg(test)]
mod char_flags_tests {
    use super::*;

    fn head_with(flags1: u32, flags2: u32, flags3: u32) -> PosHead {
        let mut buf = vec![0u8; PosHead::SIZE];
        buf[28..32].copy_from_slice(&flags1.to_le_bytes());
        buf[32..36].copy_from_slice(&flags2.to_le_bytes());
        buf[36..40].copy_from_slice(&flags3.to_le_bytes());
        PosHead::decode(&buf).unwrap()
    }

    #[test]
    fn all_flags_clear_by_default() {
        let flags = CharFlags::from_pos_head(&head_with(0, 0, 0));
        assert_eq!(flags, CharFlags::default());
    }

    /// A named bit paired with the `CharFlags` field it is supposed to light.
    type FlagProbe = (u32, fn(&CharFlags) -> bool);

    /// Each named bit must light exactly its own field: a lone set bit at the
    /// documented shift, decoded, must differ from the all-clear decode in that
    /// one field only. Catches a shift that silently aliases a neighbour.
    #[test]
    fn each_flag1_bit_is_isolated() {
        let probes: [FlagProbe; 10] = [
            (flags1::MONSTER, |f| f.monster),
            (flags1::LFG, |f| f.lfg),
            (flags1::ANONYMOUS, |f| f.anonymous),
            (flags1::YELL, |f| f.yell),
            (flags1::AWAY, |f| f.away),
            (flags1::PLAY_ONLINE, |f| f.play_online),
            (flags1::LINKSHELL, |f| f.linkshell),
            (flags1::LINKDEAD, |f| f.linkdead),
            (flags1::TARGET_OFF, |f| f.untargetable),
            (flags1::BAZAAR, |f| f.bazaar),
        ];
        for (shift, get) in probes {
            let flags = CharFlags::from_pos_head(&head_with(1 << shift, 0, 0));
            assert!(get(&flags), "flags1 bit {shift} did not set its field");
            let others = probes
                .iter()
                .filter(|(s, _)| *s != shift)
                .filter(|(_, g)| g(&flags))
                .count();
            assert_eq!(others, 0, "flags1 bit {shift} bled into another field");
            assert_eq!(flags.gm_level, 0, "flags1 bit {shift} bled into GmLevel");
        }
    }

    /// Pins the byte mapping against LSB's write site: for NPC/MOB the
    /// `m_flags` u16 is written at upstream offset 0x21 — four bytes before our
    /// body base, i.e. our 0x1D — so ENTITYFLAGS bit N lands on flags1 word bit
    /// N+8. FLAG_UNTARGETABLE (0x800) must therefore light `untargetable` and
    /// nothing else.
    #[test]
    fn mob_m_flags_untargetable_lands_on_target_off() {
        let mut body = vec![0u8; PosHead::SIZE];
        // vendor/server/src/map/packets/entity_update.cpp:348/:387
        // `ref<uint32>(0x21) = m_flags` under UPDATE_HP.
        const M_FLAGS_OFFSET: usize = 0x1D;
        body[M_FLAGS_OFFSET..M_FLAGS_OFFSET + 4].copy_from_slice(&0x800u32.to_le_bytes());
        let flags = CharFlags::from_pos_head(&PosHead::decode(&body).unwrap());
        assert!(
            flags.untargetable,
            "FLAG_UNTARGETABLE did not light TargetOffFlag"
        );
        // The neighbouring ENTITYFLAGS bits (HIDE_NAME 0x8, CALL_FOR_HELP 0x20,
        // HIDE_MODEL 0x80, HIDE_HP 0x100) must not bleed into any decoded field.
        for m_flags in [0x008u32, 0x020, 0x080, 0x100] {
            let mut body = vec![0u8; PosHead::SIZE];
            body[M_FLAGS_OFFSET..M_FLAGS_OFFSET + 4].copy_from_slice(&m_flags.to_le_bytes());
            let flags = CharFlags::from_pos_head(&PosHead::decode(&body).unwrap());
            assert!(
                !flags.untargetable,
                "m_flags {m_flags:#x} bled into untargetable"
            );
        }
    }

    #[test]
    fn gm_level_is_a_three_bit_field_above_the_singles() {
        for level in 0..=7u8 {
            let flags =
                CharFlags::from_pos_head(&head_with(u32::from(level) << flags1::GM_LEVEL, 0, 0));
            assert_eq!(flags.gm_level, level);
            assert!(!flags.bazaar, "GmLevel {level} bled into BazaarFlag");
        }
    }

    /// `CCharUpdatePacket::updateWith` packs the pearl colour into the low three bytes
    /// of Flags2 as `(Exdata channel << 4) + 15`.
    #[test]
    fn linkshell_color_reads_the_low_three_bytes_of_flags2() {
        let flags2 = 0x11u32 | (0x22 << 8) | (0x33 << 16);
        let flags = CharFlags::from_pos_head(&head_with(0, flags2, 0));
        assert_eq!(flags.linkshell_color, [0x11, 0x22, 0x33]);
        assert!(!flags.charm);
        assert!(!flags.auto_party);
    }

    #[test]
    fn flags2_singles_sit_above_the_colour_channels() {
        for (shift, get) in [
            (
                flags2::CHARM,
                (|f: &CharFlags| f.charm) as fn(&CharFlags) -> bool,
            ),
            (flags2::GM_ICON, |f: &CharFlags| f.gm_icon),
            (flags2::AUTO_PARTY, |f: &CharFlags| f.auto_party),
        ] {
            let flags = CharFlags::from_pos_head(&head_with(0, 1 << shift, 0));
            assert!(get(&flags), "flags2 bit {shift} did not set its field");
            assert_eq!(
                flags.linkshell_color,
                [0, 0, 0],
                "flags2 bit {shift} bled into the pearl colour"
            );
        }
    }

    #[test]
    fn allegiance_is_the_ballista_team_byte() {
        // ALLEGIANCE_TYPE::WINDURST (vendor/server/src/map/entities/baseentity.h)
        const WINDURST: u8 = 4;
        let flags = CharFlags::from_pos_head(&head_with(
            0,
            0,
            u32::from(WINDURST) << flags3::BALLISTA_TEAM,
        ));
        assert_eq!(flags.allegiance, WINDURST);
        assert!(!flags.trust);
        assert!(!flags.new_character);
        assert!(!flags.mentor);
    }

    #[test]
    fn flags3_singles_straddle_the_ballista_team_byte() {
        let probes: [FlagProbe; 5] = [
            (flags3::TRUST, |f| f.trust),
            (flags3::LFG_MASTER, |f| f.lfg_master),
            (flags3::PET, |f| f.pet),
            (flags3::NEW_CHARACTER, |f| f.new_character),
            (flags3::MENTOR, |f| f.mentor),
        ];
        for (shift, get) in probes {
            let flags = CharFlags::from_pos_head(&head_with(0, 0, 1 << shift));
            assert!(get(&flags), "flags3 bit {shift} did not set its field");
            assert_eq!(
                flags.allegiance, 0,
                "flags3 bit {shift} bled into BallistaTeam"
            );
            let others = probes
                .iter()
                .filter(|(s, _)| *s != shift)
                .filter(|(_, g)| g(&flags))
                .count();
            assert_eq!(others, 0, "flags3 bit {shift} bled into another field");
        }
    }
}

#[cfg(test)]
mod look_data_tests {
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

    /// `MODELTYPE::MODEL_DOOR` (vendor/server/src/map/packets/entity_update.h).
    const MODEL_DOOR: u16 = 2;

    /// `case MODEL_DOOR` in `CEntityUpdatePacket::updateWith` calls
    /// `setSize(0x48)`, and body[0] is LSB packet 0x04.
    const DOOR_BODY_LEN: usize = 0x48 - 4;

    fn door_body(name: &[u8]) -> Vec<u8> {
        let mut buf = vec![0u8; DOOR_BODY_LEN];
        let look = LookData::LOOK_BODY_OFFSET;
        buf[look..look + 2].copy_from_slice(&MODEL_DOOR.to_le_bytes());
        let id = LookData::DOOR_ID_BODY_OFFSET;
        buf[id..id + name.len()].copy_from_slice(name);
        buf
    }

    /// Pins the two door offsets to LSB's packet-relative writes, minus the
    /// 4-byte sub-packet header this decoder's `body` starts past
    /// (vendor/server/src/map/packets/entity_update.cpp `case MODEL_DOOR`:
    /// `ref<uint16>(0x30) = look.size`, name memcpy'd to 0x34).
    #[test]
    fn door_offsets_map_to_lsb_packet_bytes_0x30_and_0x34() {
        assert_eq!(LookData::LOOK_BODY_OFFSET, 0x30 - 4);
        assert_eq!(LookData::DOOR_ID_BODY_OFFSET, 0x34 - 4);
    }

    #[test]
    fn look_data_decodes_door_fourcc() {
        // Southern San d'Oria's Chocobo Stables door
        // (vendor/server/sql/npc_list.sql, npcid 17719475, look 0x0200…).
        const SANDORIA_STABLES: &[u8; DoorId::LEN] = b"_6ey";

        let buf = door_body(SANDORIA_STABLES);
        let LookData::Door { size, door_id } =
            LookData::decode_char_npc(&buf).expect("MODEL_DOOR look")
        else {
            panic!("look.size 2 must classify as Door");
        };
        assert_eq!(size, MODEL_DOOR);

        let door_id = door_id.expect("a named door carries its FourCC");
        assert_eq!(door_id.bytes(), *SANDORIA_STABLES);
        assert_eq!(door_id.to_string(), "_6ey");

        // The join: the FourCC read little-endian is the MZB `BlockID`, whose
        // first byte is what `MmbPlacement::in_underscore_at_group` tests.
        assert_eq!(door_id.block_id(), u32::from_le_bytes(*SANDORIA_STABLES));
        assert_eq!(door_id.block_id().to_le_bytes()[0], b'_');
    }

    /// The FourCC is written by the `look.size` switch, which sits outside every
    /// `updatemask` branch, so it must survive a body carrying no send flags at
    /// all — unlike the name, which needs UPDATE_NAME.
    #[test]
    fn door_fourcc_survives_an_empty_updatemask() {
        let buf = door_body(b"_6ey");
        assert_eq!(buf[6], 0, "test body sanity: no send flags set");
        assert!(matches!(
            LookData::decode_char_npc(&buf),
            Some(LookData::Door {
                door_id: Some(_),
                ..
            })
        ));
    }

    #[test]
    fn nameless_door_still_classifies_as_a_door() {
        let buf = door_body(b"");
        assert_eq!(
            LookData::decode_char_npc(&buf),
            Some(LookData::Door {
                size: MODEL_DOOR,
                door_id: None,
            })
        );
    }

    /// A body that stops at the look word (a private server sending the short
    /// 0x0E form) must still classify as a door rather than vanish.
    #[test]
    fn door_body_truncated_before_the_fourcc_yields_no_id() {
        let mut buf = vec![0u8; LookData::DOOR_ID_BODY_OFFSET + DoorId::LEN - 1];
        let look = LookData::LOOK_BODY_OFFSET;
        buf[look..look + 2].copy_from_slice(&MODEL_DOOR.to_le_bytes());
        assert_eq!(
            LookData::decode_char_npc(&buf),
            Some(LookData::Door {
                size: MODEL_DOOR,
                door_id: None,
            })
        );
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

    fn grap_id_tbl_bytes() -> Vec<u8> {
        let mut tbl = vec![0u8; LookData::GRAP_ID_TBL_LEN];
        let slots: [u16; LookData::GRAP_ID_TBL_SLOTS] = [
            0x0307, 0x1001, 0x2002, 0x3003, 0x4004, 0x5005, 0x6006, 0x7007, 0x8008,
        ];
        for (i, v) in slots.iter().enumerate() {
            tbl[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
        }
        tbl
    }

    #[test]
    fn grap_list_and_char_pc_decode_the_same_table() {
        let tbl = grap_id_tbl_bytes();

        let mut grap_list = vec![0u8; LookData::GRAP_LIST_TBL_OFFSET + tbl.len()];
        grap_list[LookData::GRAP_LIST_TBL_OFFSET..].copy_from_slice(&tbl);

        let mut char_pc = vec![0u8; LookData::CHAR_PC_GRAP_OFFSET + tbl.len()];
        char_pc[LookData::CHAR_PC_GRAP_OFFSET..].copy_from_slice(&tbl);

        let expected = LookData::Equipped {
            face: 0x07,
            race: 0x03,
            head: 1,
            body: 2,
            hands: 3,
            legs: 4,
            feet: 5,
            main: 6,
            sub: 7,
            ranged: 8,
        };
        assert_eq!(LookData::decode_grap_list(&grap_list), Some(expected));
        assert_eq!(LookData::decode_char_pc(&char_pc), Some(expected));
    }

    #[test]
    fn grap_list_zero_slot0_returns_none() {
        let buf = vec![0u8; LookData::GRAP_ID_TBL_LEN];
        assert_eq!(LookData::decode_grap_list(&buf), None);
    }
}

#[cfg(test)]
mod npc_state_tests {
    use super::*;

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
}

#[cfg(test)]
mod pos_head_tests {
    use super::*;

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
    fn char_pc_mount_index_reads_flags6_and_needs_the_general_block() {
        // Flags6.MountIndex is bits 4..11; GateId occupies the low nibble and must
        // not bleed in (flags6_t, vendor/server/src/map/packets/char_update.cpp).
        let mut buf = vec![0u8; PosHead::FLAGS6_OFFSET + 4];
        let flags6 = (u32::from(34u8) << PosHead::MOUNT_INDEX_SHIFT) | 0x0F;
        buf[PosHead::FLAGS6_OFFSET..PosHead::FLAGS6_OFFSET + 4]
            .copy_from_slice(&flags6.to_le_bytes());
        assert_eq!(PosHead::mount_index(&buf), Some(34));

        // A position-only update stops before Flags6.
        let short = vec![0u8; PosHead::SIZE_WITH_BT_TARGET];
        assert_eq!(PosHead::mount_index(&short), None);
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

    /// entity_update.cpp:539-560 — a renamed dynamic entity (targid >= 0x700)
    /// spawning with an equipment model grows to `setSize(0x56)`, gets `look_t`
    /// memcpy'd over packet 0x30 and its name pushed to packet 0x44, flagged by
    /// `ref<uint8>(0x18) = 0x01`. Its mask is the literal 0x57, which carries
    /// Name2 but NOT UPDATE_NAME.
    fn dynamic_spawn_body(name: &[u8]) -> Vec<u8> {
        let mut buf = vec![0u8; 0x54];
        buf[6] = 0x57;
        buf[0x18 - 4] = 0x01;
        buf[0x30..0x44].copy_from_slice(&[0x11u8; 0x14]);
        buf[0x44 - 4..0x44 - 4 + name.len()].copy_from_slice(name);
        buf
    }

    #[test]
    fn try_extract_name_reads_the_dynamic_entity_spawn_slot() {
        use crate::map::s2c;

        let buf = dynamic_spawn_body(b"Ranger Trust\0");
        assert_eq!(
            PosHead::try_extract_name(s2c::CHAR_NPC, &buf).as_deref(),
            Some("Ranger Trust"),
            "0x57 carries Name2, not UPDATE_NAME — gating on UPDATE_NAME drops it"
        );
    }

    #[test]
    fn try_extract_name_dynamic_slot_needs_the_name2_flag() {
        use crate::map::s2c;

        let mut buf = dynamic_spawn_body(b"Ranger Trust\0");
        buf[6] &= !PosHead::SEND_NAME2;
        assert!(PosHead::try_extract_name(s2c::CHAR_NPC, &buf).is_none());
    }

    // The 0x18 marker shares its offset with loc.p.moving, so an ordinary
    // moving entity can raise it. That must reorder the candidates, never
    // discard the name sitting in the standard slot.
    #[test]
    fn try_extract_name_falls_back_when_the_long_name_marker_is_moving_data() {
        use crate::map::s2c;

        let mut buf = vec![0u8; 0x54];
        buf[6] = 0x08 | 0x40;
        buf[0x18 - 4] = 0x01;
        buf[0x30..0x30 + 9].copy_from_slice(b"Sigli-Sea");
        assert_eq!(
            PosHead::try_extract_name(s2c::CHAR_NPC, &buf).as_deref(),
            Some("Sigli-Sea")
        );
    }

    // A plain equipped spawn also flies Name2 and stops at setSize(0x48), so
    // look bytes must never be mined for a name that was never sent — this is
    // the packet an unnamed private-server NPC arrives on.
    #[test]
    fn try_extract_name_ignores_look_bytes_on_a_plain_equipped_spawn() {
        use crate::map::s2c;

        let mut buf = vec![0u8; 0x48 - 4];
        buf[6] = 0x57;
        buf[0x18 - 4] = 0x01;
        buf[0x30..0x44].copy_from_slice(b"ABCDEFGHIJKLMNOPQRST");
        assert!(PosHead::try_extract_name(s2c::CHAR_NPC, &buf).is_none());
    }
}

#[cfg(test)]
mod char_sync_tests {
    use super::*;

    /// Pins the 2F-unlock byte to LSB's full-packet offset 0x27 minus the 4-byte
    /// sub-packet header (vendor/server/src/map/packets/char_sync.cpp:61).
    #[test]
    fn char_sync_2f_flag_sits_at_lsb_packet_byte_0x27() {
        assert_eq!(CharSync::MH_2F_UNLOCKED_OFFSET, 0x27 - 4);
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
}

#[cfg(test)]
mod entity_set_name_tests {
    use super::*;

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
}

#[cfg(test)]
mod pet_sync_tests {
    use super::*;

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
}
