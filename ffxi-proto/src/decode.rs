//! Typed decoders for the most useful S2C map packets.
//!
//! Both `CHAR_PC` (0x00D) and `CHAR_NPC` (0x00E) share the same 40-byte
//! `GP_SERV_POS_HEAD` prefix (after the 4-byte sub-packet header). After
//! that, the layout is variable based on the `SendFlg` bits. For v1 we
//! decode the position-head only; later we can pull `name` from the trailing
//! fields when `SendFlg.Name` is set.
//!
//! References:
//! - `server/src/map/packets/char_update.cpp::GP_SERV_CHAR_PC`
//! - `server/src/map/packets/entity_update.cpp::GP_SERV_CHAR_NPC`
//! - `server/src/map/packets/s2c/0x00b_logout.h`

/// Animation byte values for [`PosHead::server_status`] (which despite the
/// field name carries `PChar->animation`, see field doc). Constants mirror
/// `vendor/server/src/map/entities/baseentity.h::ANIMATIONTYPE`.
pub mod animation {
    /// Default — no special animation.
    pub const NONE: u8 = 0;
    /// Auto-attack swing animation.
    pub const ATTACK: u8 = 1;
    /// `/heal` resting pose. Set/cleared by `EFFECT_HEALING`'s Lua
    /// `onEffectGain`/`onEffectLose` (`vendor/server/scripts/effects/healing.lua`).
    pub const HEALING: u8 = 33;
    /// `/sit` (cross-legged ground sit).
    pub const SIT: u8 = 47;
}

use crate::framing::SubPacket;

/// Position-and-status head shared by `GP_SERV_CHAR_PC` (0x00D) and
/// `GP_SERV_CHAR_NPC` (0x00E). The first 36 bytes of the *body* (i.e.
/// the bytes after the 4-byte sub-packet header).
#[derive(Debug, Clone, Copy)]
pub struct PosHead {
    /// Globally-unique entity ID (server-side).
    pub unique_no: u32,
    /// Per-zone short index.
    pub act_index: u16,
    /// `SendFlg` bits — which optional fields trail.
    pub send_flag: u8,
    /// Heading 0..=255 (0°..360°).
    pub dir: u8,
    /// World X coordinate.
    pub x: f32,
    /// World Z coordinate (height).
    pub z: f32,
    /// World Y coordinate.
    pub y: f32,
    /// Movement timer / flags (bit-packed) — opaque for now.
    pub flags0: u32,
    /// Current speed.
    pub speed: u8,
    /// Base speed.
    pub speed_base: u8,
    /// HP percentage 0..=100.
    pub hpp: u8,
    /// **Animation byte** at packet offset 0x1F (= body[27]). LSB's struct
    /// field at this offset is named `server_status` (see
    /// `vendor/server/src/map/packets/char_update.cpp:183`,
    /// `entity_update.cpp:209`), but the actual write at
    /// `char_update.cpp:284` assigns `PChar->animation` to this slot —
    /// `packet->server_status = PChar->animation`. The status-type enum
    /// goes a byte later, in the `flags1` slot we read at body[28..32].
    /// Naming kept as `server_status` for historical compatibility, but
    /// semantic is animation (e.g. `ANIMATION_HEALING = 33` for /heal,
    /// `ANIMATION_NONE = 0` otherwise). Only authoritative when
    /// `body[6] & UPDATE_HP (0x04)` is set.
    pub server_status: u8,
    pub flags1: u32,
    pub flags2: u32,
    pub flags3: u32,
    /// Battle target ID, 0 if not in combat.
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
    /// Minimum body length for a POS_HEAD without `BtTargetID` — older
    /// PS2-era layouts ended at `Flags3`. We accept these but report
    /// `bt_target_id = 0`.
    pub const SIZE: usize = 40;

    /// Body length when the trailing `BtTargetID` field is present.
    /// Phoenix and modern LSB always send this — see
    /// `Phoenix/src/map/packets/char_update.cpp:187`. Below this length,
    /// `bt_target_id` decodes as 0 (= "no target").
    pub const SIZE_WITH_BT_TARGET: usize = 44;

    /// Decode from the *body* of a CHAR_PC/CHAR_NPC sub-packet
    /// (`SubPacket::data`).
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

    /// Decode a CHAR_NPC (0x00E) body, surfacing the mob-owner / claim id.
    ///
    /// CHAR_PC and CHAR_NPC share the same first 44 bytes of body layout:
    /// `GP_SERV_POS_HEAD` followed by what looks structurally like
    /// `BtTargetID` at body[40..44]. For CHAR_PC that slot is genuinely the
    /// battle-target id; for CHAR_NPC the same byte slot is repurposed as
    /// `m_OwnerID` — the FFXI claim-id, set when a player tags a mob.
    ///
    /// Server reference: `entity_update.cpp::updateWith` writes
    /// `ref<uint32>(0x2C) = PMob->m_OwnerID.id` (packet-absolute 0x2C =
    /// body offset 0x28, with the 4-byte sub-packet header subtracted).
    /// Width: u32. `0` means "unclaimed".
    ///
    /// Returns `(PosHead, claim_id)`. The PosHead's `bt_target_id` field
    /// will hold the same u32 — semantically it's the claim id when this
    /// helper is the entry point. We surface it under a distinct name so
    /// callers don't have to remember the dual semantics.
    pub fn decode_char_npc(body: &[u8]) -> Result<(Self, u32), DecodeError> {
        let head = Self::decode(body)?;
        // The same 4 bytes the PosHead decoder reads as `bt_target_id` —
        // CHAR_NPC repurposes that slot as `m_OwnerID`. We re-expose with
        // the right name; bodies shorter than 44 bytes (legacy / position-
        // only updates) report claim_id = 0, matching `bt_target_id`'s
        // truncation fallback.
        let claim_id = if body.len() >= Self::SIZE_WITH_BT_TARGET {
            u32::from_le_bytes(body[40..44].try_into().unwrap())
        } else {
            0
        };
        Ok((head, claim_id))
    }

    /// Extract the NUL-terminated ASCII name from a CHAR_PC or CHAR_NPC body.
    ///
    /// Both opcodes share the `GP_SERV_POS_HEAD` prefix and reuse byte
    /// `body[6]` (packet offset 0x0A) as a bit-mask: `SendFlg` for CHAR_PC
    /// and the cumulative `updatemask` for CHAR_NPC. LSB defines
    /// `SendFlg.Name : 1` at bit 3 (= 0x08), and `entity_update.cpp:278`
    /// ORs the same byte with `updatemask`, so the *same bit* (0x08)
    /// signals "name present" for both packets.
    ///
    /// Layout when the Name bit is set (server references):
    /// - `CHAR_PC` (`char_update.cpp:202-208, 412-421`): name lives at
    ///   struct offset `0x5A` — body offset `0x56` after stripping the
    ///   4-byte sub-packet header (`id+size+sync`). LSB writes
    ///   `std::memcpy(packet->name, ..., min(16, len))` and recomputes
    ///   `packet->size` to fit the actual name. The slot runs from
    ///   body offset `0x56` to the end of the body.
    /// - `CHAR_NPC` (`entity_update.cpp:361-435`): name at body offset
    ///   `0x30` (packet `0x34`), max 16 bytes. Renamed entities with
    ///   `targid < 1024` write a `0x01` marker at `0x30` and shift the
    ///   name to `0x31` (`entity_update.cpp:575-578`), so callers must
    ///   detect that case.
    ///
    /// Returns `None` for any other opcode, when the Name bit is clear,
    /// or when the slot fails ASCII validation.
    pub fn try_extract_name(opcode: u16, body: &[u8]) -> Option<String> {
        use crate::map::s2c;
        // SendFlg / updatemask byte. Both packets reuse body[6]; the Name
        // bit lives at 0x08 in either context.
        const NAME_FLAG: u8 = 0x08;
        if body.len() < 7 || body[6] & NAME_FLAG == 0 {
            return None;
        }
        let slot: &[u8] = if opcode == s2c::CHAR_PC {
            // `GP_SERV_CHAR_PC.name` lives at struct offset 0x5A — that's
            // `4 (id+size+sync) + 44 (PosHead w/ BtTargetID) + 46 (PC tail
            // through GrapIDTbl[9])`. Body offset = struct - 4 = 0x56.
            // Verified empirically via debug://name_misses on a live
            // session: a CHAR_PC for "Cleric" placed `43 6c 65 72 69 63`
            // at body[0x56..0x5C]. An earlier off-by-4 analysis read at
            // 0x5A and only saw the last two characters, which fell
            // below the 3-char floor.
            const NAME_START: usize = 0x56;
            if body.len() <= NAME_START {
                return None;
            }
            &body[NAME_START..]
        } else if opcode == s2c::CHAR_NPC {
            // Standard offset is 0x30; the renamed-mob low-targid variant
            // uses `0x01` as a marker at 0x30 and shifts the name by one
            // byte. Real NPC names start at 0x20 or higher, so a `0x01`
            // first byte is unambiguously the marker.
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
        // 3-char floor: filters false positives — real names are 3+ chars
        // and the slot can sometimes start with a stray flag byte if a
        // future LSB version reshapes the layout.
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

/// Per-entity model selector decoded from a CHAR_NPC / CHAR_PC body. The
/// variant matches LSB's `MODELTYPE` enum
/// (`vendor/server/src/map/packets/entity_update.h:29-39`) and the wire
/// layout matches the switch at `entity_update.cpp:451-484`.
///
/// Wire layout (body offsets — packet offset minus the 4-byte sub-packet
/// header, so body[44] corresponds to packet offset 0x30):
/// ```text
///   body[44..46]  = look.size  (the MODELTYPE sentinel)
///   body[46..48]  = look.modelid (STANDARD/UNK_5/AUTOMATON only)
///   body[44..64]  = full 20-byte look_t (EQUIPPED/CHOCOBO)
///   body[44..46]  + body[48..60] = size + 12-byte name (DOOR/SHIP/ELEVATOR)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookData {
    /// `MODEL_STANDARD = 0`, `MODEL_UNK_5 = 5`, `MODEL_AUTOMATON = 6`.
    /// Modelid is the FFXI model identifier; resolution to an MMB
    /// file_id happens in a separate (POLUtils-derived) lookup.
    Standard { modelid: u16 },
    /// `MODEL_EQUIPPED = 1`, `MODEL_CHOCOBO = 7`. Full 20-byte `look_t`
    /// with race + face packed into the modelid slot and 8 equipment
    /// slot model ids. PC equipment-layering not yet wired client-side.
    Equipped {
        /// `look.modelid` low byte. For monstrosity mobs this is
        /// `m_Costume2`; for regular PCs this is `look_t.face` per
        /// the `look_t` union in `vendor/server/src/common/mmo.h:157-169`.
        face: u8,
        /// `look.modelid` high byte. Race id (1..=8) for PCs;
        /// `look.race` for monstrosities (`char_update.cpp:388`).
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
    /// `MODEL_DOOR = 2`. Used for static doors / scenery NPCs. The
    /// `size` byte itself is the MODELTYPE tag; the actual visual is
    /// inferred from the NPC name and zone (per LSB convention).
    Door { size: u16 },
    /// `MODEL_SHIP = 4`, `MODEL_ELEVATOR = 3`. Transport NPCs — only
    /// the type tag is carried; the actual model selection happens in
    /// the client via `getTransportNPCName` (server side) and a
    /// hard-coded client mapping. We don't render these as MMBs yet.
    Transport { size: u16 },
}

impl LookData {
    /// Body offset of the start of the look block — packet-absolute
    /// `0x30` minus the 4-byte sub-packet header.
    pub const LOOK_BODY_OFFSET: usize = 0x2C;

    /// Decode the look block from a `CHAR_NPC` (0x00E) body. Returns
    /// `None` when the body is truncated before the size+modelid pair
    /// or the size sentinel is unrecognized.
    ///
    /// Reference: `vendor/server/src/map/packets/entity_update.cpp:451-484`.
    pub fn decode_char_npc(body: &[u8]) -> Option<Self> {
        let off = Self::LOOK_BODY_OFFSET;
        if body.len() < off + 4 {
            return None;
        }
        let size = u16::from_le_bytes([body[off], body[off + 1]]);
        // MODELTYPE enum from `entity_update.h:29-39`.
        match size {
            0 | 5 | 6 => {
                // MODEL_STANDARD / MODEL_UNK_5 / MODEL_AUTOMATON. The
                // `look_t` union puts `modelid` at offset 2; the server
                // writes 4 bytes of look_t to body[off..off+4] for these
                // types (`entity_update.cpp:458`).
                let modelid = u16::from_le_bytes([body[off + 2], body[off + 3]]);
                Some(LookData::Standard { modelid })
            }
            1 | 7 => {
                // MODEL_EQUIPPED / MODEL_CHOCOBO. Full 20-byte look_t at
                // body[off..off+20] (`entity_update.cpp:465`).
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

    /// Body offset of `GP_SERV_CHAR_PC.GrapIDTbl[0]` — packet-absolute
    /// `0x48` minus the 4-byte sub-packet header. Derived from the same
    /// struct layout that places `name` at body[0x56]: 0x56 - 18 bytes
    /// of `GrapIDTbl[9]` = 0x44. See `try_extract_name`'s `CHAR_PC`
    /// branch for the matching offset derivation.
    pub const CHAR_PC_GRAP_OFFSET: usize = 0x44;

    /// Decode the look block from a `CHAR_PC` (0x00D) body. PCs ship
    /// their look in `GrapIDTbl[9]` rather than the `look_t` union —
    /// slot 0 carries (face, race) and slots 1..=8 carry equipment
    /// model ids XOR'd with a per-slot `0xN000` mask.
    ///
    /// Returns `None` when:
    ///   - body is truncated before GrapIDTbl ends (body.len() < 0x44 + 18), or
    ///   - GrapIDTbl[0] is 0 (server hasn't populated look yet — happens
    ///     on initial CHAR_PC bursts before `SendFlg.Model` is set).
    ///
    /// Reference: `vendor/server/src/map/packets/char_update.cpp:374-401`
    /// and `vendor/server/src/common/mmo.h:157-169`.
    ///
    /// Note: monstrosity / costume PCs reuse the `Equipped` variant on
    /// the wire — slot 0 becomes `(race << 8 | costume_or_monstrosity_id)`
    /// and slot 8 becomes 0xFFFF. We surface that identically to a
    /// regular PC; downstream consumers can treat `ranged == 0x0FFF`
    /// (post-mask `0xFFFF & 0x0FFF`) as a hint if needed.
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
        // Strip the per-slot `0xN000` mask added at char_update.cpp:377-384.
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

/// Server's `GP_SERV_COMMAND_LOGIN` (0x00A) — sent on zone-in (both initial
/// connect and after every zone transition). Reference:
/// `vendor/Phoenix/src/map/packets/s2c/0x00a_login.h::GP_SERV_COMMAND_LOGIN`.
///
/// Body layout (bytes after the 4-byte sub-packet header):
/// ```text
///   [ 0..44]  GP_SERV_POS_HEAD       (UniqueNo..BtTargetID — 44B with BT)
///   [44..48]  ZoneNo:u32             (low 16 bits = FFXI zone id)
///   [48..52]  ntTime, ntTimeSec, GameTime …
///   …         (many trailing fields — see header for full layout)
/// ```
///
/// The wire field is a `u32` but FFXI zone ids fit in `u16` (max ~300);
/// we surface it as `u16` to match `state::SessionState::zone_id` and
/// `PartyAttrs::zone_no`. We only decode the fields needed for v1 zone
/// tracking; the rest of the packet is ignored.
#[derive(Debug, Clone, Copy)]
pub struct ServerLogin {
    pub unique_no: u32,
    pub act_index: u16,
    pub zone_no: u16,
    /// Full `GP_SERV_POS_HEAD` carried in body[0..44]. The position
    /// fields here are the server-authoritative spawn coordinates on
    /// every zone-in (LSB sets `packet.PosHead = { .x = loc.p.x,
    /// .z = loc.p.y, .y = loc.p.z, ... }` — see
    /// `server/src/map/packets/s2c/0x00a_login.cpp`). Surface them so
    /// the client can seed its self-entity from the very first packet
    /// of zone-in without waiting for a CHAR_PC echo.
    pub pos_head: PosHead,
    /// `MusicNum[5]` at body[82..92]. Pre-fills the BGM slots
    /// 0..=4 on zone-in (LSB pushes these inline rather than via
    /// 0x05F):
    ///   [0] = ZoneDay     (music_day column)
    ///   [1] = ZoneNight   (music_night column)
    ///   [2] = CombatSolo  (battlesolo column)
    ///   [3] = CombatParty (battlemulti column)
    ///   [4] = Mount       (0x54 if mounted at zone-in, else 0xD4)
    /// `None` only if the LOGIN body is too short to reach this
    /// offset (extremely unusual — real LSB bodies are ~180+B).
    pub music_num: Option<[u16; 5]>,
}

impl ServerLogin {
    /// Minimum body length to safely read `ZoneNo` — `PosHead` (44B) +
    /// `ZoneNo` (4B) = 48 bytes. Phoenix's full LOGIN body is much larger
    /// (~180+ bytes) but we only need the prefix.
    pub const SIZE: usize = 48;
    /// Body offset of `MusicNum[0]`. Layout from LSB's `0x00a_login.h`:
    ///   0x00 PosHead (44B)
    ///   0x2C ZoneNo (4B)
    ///   0x30 ntTime (4B)
    ///   0x34 ntTimeSec (4B)
    ///   0x38 GameTime (4B)
    ///   0x3C EventNo (2B)
    ///   0x3E MapNumber (2B)
    ///   0x40 GrapIDTbl[9] (18B)
    ///   0x52 MusicNum[5] (10B) ← we read here
    pub const MUSIC_NUM_OFFSET: usize = 0x52;
    pub const MUSIC_NUM_SIZE: usize = 5 * 2;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        // The trailing BT-target slot may legitimately be absent on legacy
        // 40-byte heads, but a real LOGIN body always carries the full
        // 44-byte head plus ZoneNo — Phoenix sends `sizeof(PacketData)`
        // every time. So we read at the fixed offset.
        let zone_u32 = u32::from_le_bytes(body[44..48].try_into().unwrap());
        let pos_head = PosHead::decode(&body[..PosHead::SIZE_WITH_BT_TARGET])?;
        // MusicNum is optional: short bodies (in synthetic tests, or
        // hypothetical legacy clients) simply don't carry it.
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
            pos_head,
            music_num,
        })
    }
}

/// Server's `GP_SERV_COMMAND_LOGOUT` (0x00B) — used for zone changes and
/// disconnects. Carries the destination map server's IP/port and the
/// reason code.
///
/// Layout (after the 4-byte sub-packet header):
/// ```text
///   [0..4]   LogoutState (u32 enum)
///   [4..8]   Iwasaki.ip (u32, LE — the new map server IP, host-LE order)
///   [8..12]  Iwasaki.port (u32, LE — only the low 16 bits are meaningful)
///   [12..20] Iwasaki.padding (8 bytes, zero)
///   [20..24] cliErrCode (u32 enum)
/// ```
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

    /// `LogoutState` enum values from
    /// `server/src/map/packets/s2c/0x00b_logout.h::GP_GAME_LOGOUT_STATE`.
    pub fn is_zone_change(&self) -> bool {
        self.logout_state == 2 // ZONECHANGE
    }
}

/// `GP_SERV_COMMAND_SYSTEMMES` (0x053) — formatted system message. Body
/// layout per `vendor/server/src/map/packets/s2c/0x053_systemmes.h`:
/// `u32 para, u32 para2, u16 Number, u16 padding` = 12 bytes. `Number`
/// is an id into `xi.msg.system`; `para`/`para2` substitute into
/// placeholders like `<seconds>` in the message text.
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

/// `GP_SERV_COMMAND_WEATHER` (0x057) — current zone weather. Body layout
/// per `vendor/server/src/map/packets/s2c/0x057_weather.h`:
/// `u32 StartTime, u16 WeatherNumber, u16 WeatherOffsetTime` = 8 bytes.
/// `weather_number` is an index into LSB's `Weather` enum
/// (`vendor/server/src/map/enums/weather.h`); map to a typed variant via
/// `ffxi_viewer_wire::Weather::from_lsb`.
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

/// `POSMODE` enum mirrored from `vendor/server/src/map/packets/s2c/0x05b_wpos.h:28-39`.
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

    /// True when the mode carries an authoritative position the client
    /// must adopt — mirrors the `if (mode == NORMAL || EVENT || POP ||
    /// RESET || MATERIALIZE)` branch at
    /// `vendor/server/src/map/packets/s2c/0x05b_wpos.cpp:33-44`.
    /// LOCK / UNLOCK / CLEAR / ROTATE do not re-anchor the player.
    pub fn carries_position(&self) -> bool {
        matches!(
            self,
            PosMode::Normal
                | PosMode::Event
                | PosMode::Pop
                | PosMode::Reset
                | PosMode::Materialize
        )
    }
}

/// `GP_SERV_COMMAND_WPOS` (0x05B) and `GP_SERV_COMMAND_WPOS2` (0x065) —
/// server-initiated forced position for the local player. Both opcodes
/// share the same body layout (24 bytes):
///
/// ```text
///   [ 0.. 4]  float x       (LSB.x — east/west)
///   [ 4.. 8]  float y       (LSB.y — vertical / height)
///   [ 8..12]  float z       (LSB.z — north/south)
///   [12..16]  u32   UniqueNo
///   [16..18]  u16   ActIndex
///   [18]      u8    Mode    (POSMODE)
///   [19]      i8    dir     (heading byte)
///   [20..24]  u32   padding
/// ```
///
/// References:
/// - `vendor/server/src/map/packets/s2c/0x05b_wpos.h:43-59`
/// - `vendor/server/src/map/packets/s2c/0x05b_wpos.cpp:28-65`
/// - `vendor/server/src/map/packets/s2c/0x065_wpos2.h`
///
/// **Coordinate remap**: our `Vec3` is z-up (`.y = north/south`,
/// `.z = height`), matching the `PosHead` decoder's `body[12..16]→z`,
/// `body[16..20]→y` swap. We apply the same remap: LSB.y → our `.z`,
/// LSB.z → our `.y`.
///
/// **Knockback note**: LSB does NOT emit WPOS for combat knockback.
/// Knockback is purely an animation hint in the BATTLE2 (0x028) result —
/// 3 bits at `vendor/server/src/map/packets/s2c/0x028_battle2.cpp:76`,
/// table in `vendor/server/src/map/enums/action/knockback.h`. The retail
/// client integrates the displacement locally over the per-level timer.
/// WPOS covers the *other* forced-move family: cutscene-end teleports
/// (`vendor/server/src/map/packets/c2s/0x05c_eventendxzy.cpp:68-73`),
/// zone-line re-anchoring (`0x05e_maprect.cpp:47`), homepoint, GM warp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForcedMove {
    pub unique_no: u32,
    pub act_index: u16,
    pub mode: PosMode,
    pub x: f32,
    /// North-south in our z-up frame (= LSB.z on the wire).
    pub y: f32,
    /// Height in our z-up frame (= LSB.y on the wire).
    pub z: f32,
    pub heading: u8,
    /// Raw mode byte preserved for diagnostics when the wire value falls
    /// outside the documented enum.
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
        // Unknown mode bytes fall back to Normal so a future LSB enum
        // addition still re-anchors the player rather than silently dropping
        // the packet. `raw_mode` is preserved separately for diagnostics.
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
mod forced_move_tests {
    use super::*;

    #[test]
    fn forced_move_decodes_normal_mode_and_swaps_axes() {
        let mut body = vec![0u8; ForcedMove::SIZE];
        body[0..4].copy_from_slice(&12.5f32.to_le_bytes()); // LSB.x
        body[4..8].copy_from_slice(&3.25f32.to_le_bytes()); // LSB.y (height)
        body[8..12].copy_from_slice(&(-7.0f32).to_le_bytes()); // LSB.z (NS)
        body[12..16].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        body[16..18].copy_from_slice(&42u16.to_le_bytes());
        body[18] = 0x00; // POSMODE::NORMAL
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

/// Common party-member fields shared by `0x0DD GROUP_LIST` (other members)
/// and `0x0DF GROUP_ATTR` (self + Trust). Field offsets / types mirror
/// `Phoenix/src/map/packets/s2c/0x0d{d,f}_group_*.h`.
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
    /// `MoghouseFlg` — non-zero when the server considers this member to be
    /// inside a Mog House (`PChar->m_moghouseID != 0`). In LSB the zone id
    /// stays equal to the surrounding city (e.g. S. San d'Oria = 230) even
    /// inside the Mog House — this flag is the only wire signal of mog-house
    /// occupancy. See `zone_entities.cpp:1167-1208` for how the server uses
    /// it to filter the entity stream; without checking it client-side, a
    /// rezone-into-mog looks indistinguishable from a normal rezone.
    pub moghouse_flg: u8,
    pub zone_no: u16,
    pub mjob_no: u8,
    pub mjob_lv: u8,
    pub sjob_no: u8,
    pub sjob_lv: u8,
}

/// Additional fields only present in `0x0DD GROUP_LIST` (other members):
/// the trailing 16-byte name and the `GAttr` bitfield with leader flags.
#[derive(Debug, Clone)]
pub struct PartyListExtra {
    pub member_number: u8,
    pub is_party_leader: bool,
    pub is_alliance_leader: bool,
    /// Up-to-15-character ASCII name. NUL-terminated.
    pub name: Option<String>,
}

impl PartyAttrs {
    /// Decode the body of `0x0DF GROUP_ATTR` (self / Trust). Layout:
    /// `[0..4]UniqueNo [4..8]Hp [8..12]Mp [12..16]Tp [16..18]ActIndex
    ///  [18]Hpp [19]Mpp [20]Kind [21]MoghouseFlg [22..24]ZoneNo
    ///  [24..28]Monstrosity… [28]mjob [29]mjob_lv [30]sjob [31]sjob_lv …`.
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

    /// Decode the body of `0x0DD GROUP_LIST` (other members). Layout:
    /// `[0..4]UniqueNo [4..8]Hp [8..12]Mp [12..16]Tp [16..20]GAttr
    ///  [20..22]ActIndex [22]MemberNumber [23]MoghouseFlg [24]Kind
    ///  [25]Hpp [26]Mpp [27]pad [28..30]ZoneNo [30]mjob [31]mjob_lv
    ///  [32]sjob [33]sjob_lv [34]masterjob_lv [35]masterjob_flags
    ///  [36..52]Name`.
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
        // Bitfield: PartyNo:2, PartyLeaderFlg:1, AllianceLeaderFlg:1, …
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

/// Convenience: attempt to decode a sub-packet by opcode.
pub fn decode_pos_head(sub: &SubPacket<'_>) -> Result<PosHead, DecodeError> {
    PosHead::decode(sub.data)
}

pub fn decode_logout(sub: &SubPacket<'_>) -> Result<ServerLogout, DecodeError> {
    ServerLogout::decode(sub.data)
}

pub fn decode_login(sub: &SubPacket<'_>) -> Result<ServerLogin, DecodeError> {
    ServerLogin::decode(sub.data)
}

/// Read a NUL-terminated ASCII name from a packet slot. Mirrors the
/// validation in `PosHead::try_extract_name`: at least 3 printable bytes
/// (0x20..=0x7E) before the first NUL. Returns `None` for empty, truncated,
/// or non-ASCII content.
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

/// `CCharSyncPacket` — one of two packet variants that share opcode 0x067.
/// Carries PC status sync (level-sync icon, mount data, main-job slot);
/// **no position and no name**.
///
/// Identified by the sub-type byte at body[0] = 0x02. Server reference:
/// `vendor/server/src/map/packets/char_sync.cpp:28-62`.
///
/// Wire layout (body offsets = LSB packet offsets minus the 4-byte
/// sub-packet header that `SubPacket::data` already strips):
/// ```text
///   [0]      SubType (0x02)
///   [1]      0x09
///   [2..4]   PChar->targid (act_index)
///   [4..8]   PChar->id
///   [8..]    Status / mount fields — not surfaced by this decoder
/// ```
///
/// We decode the identifying fields for completeness; the rest is
/// status-sync data the client doesn't currently track.
#[derive(Debug, Clone, Copy)]
pub struct CharSync {
    pub targid: u16,
    pub id: u32,
}

impl CharSync {
    /// Sub-type byte value at body[0] that identifies this packet variant
    /// within opcode 0x067.
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

/// `CEntitySetNamePacket` — the other 0x067 variant. LSB sends this to
/// name **trusts** (on spawn), fellows, and pankration entities. The
/// distinguishing feature: the entity may not have a corresponding name
/// in CHAR_NPC, so this packet is the only authoritative name source for
/// those entity types.
///
/// Identified by sub-type byte body[0] = 0x03. Server reference:
/// `vendor/server/src/map/packets/entity_set_name.cpp:30-54`.
///
/// Wire layout:
/// ```text
///   [0]       SubType (0x03)
///   [1]       0x05
///   [2..4]    PEntity->targid
///   [4..8]    PEntity->id
///   [8..10]   PMaster->targid (zero for non-trust entities)
///   [0xC]     0x04 — opaque flag
///   [0x14..]  Entity name (NUL-terminated, ASCII, capped by sub.size)
/// ```
#[derive(Debug, Clone)]
pub struct EntitySetName {
    pub targid: u16,
    pub id: u32,
    pub master_targid: u16,
    pub name: Option<String>,
}

impl EntitySetName {
    pub const SUB_TYPE: u8 = 0x03;
    /// Minimum body length to safely read the name slot (offset 0x14).
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

/// `CPetSyncPacket` — opcode 0x068. The server emits this for the
/// **owner** of a pet/avatar/automaton/wyvern to sync the pet's
/// HP%/MP%/TP/target/name. The pet itself separately has a CHAR_NPC
/// stream for position; this packet enriches that record.
///
/// Server reference: `vendor/server/src/map/packets/pet_sync.cpp:34-59`.
///
/// Wire layout:
/// ```text
///   [0]       Bitfield, `0x04` set
///   [2..4]    Owner targid (PChar->targid)
///   [4..8]    Owner id (PChar->id)
///   [8..10]   Pet targid (PChar->PPet->targid)  — absent on despawn
///   [0xA]     Pet HP%
///   [0xB]     Pet MP%
///   [0xC..0xE] Pet TP
///   [0x10..0x14] Pet's battle-target id (only set during ANIMATION_ATTACK)
///   [0x14..]  Pet name (NUL-terminated)
/// ```
///
/// **Despawn variant**: when the owner's pet is gone, LSB sends just the
/// header (sub.size = 0x1C → body length 0x18). All pet fields after
/// `owner_id` are absent. The decoder returns the owner fields and
/// `pet_targid = 0`, name = None — the caller treats that as "owner has
/// no active pet right now".
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
    /// Body length when the despawn variant is on the wire — only the
    /// owner header is present. Anything below this is truncation.
    pub const DESPAWN_SIZE: usize = 8;
    /// Body length once the full pet header is present (still no name).
    pub const FULL_HEADER_SIZE: usize = 0x14;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::DESPAWN_SIZE {
            return Err(DecodeError::Truncated(Self::DESPAWN_SIZE, body.len()));
        }
        let owner_targid = u16::from_le_bytes(body[2..4].try_into().unwrap());
        let owner_id = u32::from_le_bytes(body[4..8].try_into().unwrap());
        if body.len() < Self::FULL_HEADER_SIZE {
            // Despawn variant — no pet fields present.
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

/// `GP_SERV_COMMAND_ITEM_MAX` (0x01C) — container size table. Phoenix
/// sends this once during zone-in to tell the client how many slots
/// every container can hold.
///
/// Wire layout (`Phoenix/src/map/packets/s2c/0x01c_item_max.h`):
///   `uint8_t ItemNum[18]; uint8_t padding00[14]; uint16_t ItemNum2[18];
///    uint8_t padding01[28];` → 18 + 14 + 36 + 28 = 96 bytes total.
///
/// `ItemNum[i]` is the legacy PS2-era u8 capacity for container `i`;
/// `ItemNum2[i]` is the modern u16 capacity used when capacity exceeds
/// 255. **Both fields are sent as `1 + GetSize()`** by Phoenix
/// (`0x01c_item_max.cpp:32–69`), so the decoder subtracts 1 (saturating)
/// to reconstruct the real slot count. A wire `ItemNum2 == 0` is a
/// sentinel — Phoenix uses it to disable a container (e.g. moglocker
/// without access). When wide == 0 we fall back to legacy, which mirrors
/// how the retail client resolves the two fields.
#[derive(Debug, Clone)]
pub struct ItemMax {
    /// Capacity per CONTAINER_ID (0..=17). Length = 18. Already
    /// normalized: 0 means "disabled", non-zero means real slot count.
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

/// `GP_SERV_COMMAND_ITEM_SAME` (0x01D) — load-state signal. After the
/// initial flood the server emits this with `state == 1` to signal
/// "all containers populated, you can rely on slot counts now."
///
/// Body: `State:u8 padding00[3] Flags:u32` → 8 bytes.
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
            // Phoenix only emits 0 or 1; any non-zero value reads as
            // "loaded" for forward-compat (newer LSB might add a third
            // state, but we want our existing logic to fire on the
            // existing terminal state without false negatives).
            _ => ItemSameState::AllLoaded,
        };
        let flags = u32::from_le_bytes(body[4..8].try_into().unwrap());
        Ok(Self { state, flags })
    }
}

/// `GP_SERV_COMMAND_ITEM_NUM` (0x01E) — quantity change for one slot.
/// Server emits this when the player picks up a stack increment or
/// uses a charge of an existing item.
///
/// Body: `ItemNum:u32 Category:u8 ItemIndex:u8 LockFlg:u8 padding00:u8`
/// → 8 bytes.
#[derive(Debug, Clone, Copy)]
pub struct ItemNum {
    /// New quantity for the slot.
    pub quantity: u32,
    /// CONTAINER_ID (0..=17).
    pub category: u8,
    /// Slot index inside the container.
    pub index: u8,
    /// Item-lock flag — server-side bookkeeping; we surface the raw value.
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

/// `GP_SERV_COMMAND_ITEM_LIST` (0x01F) — full slot definition without
/// extdata. Sent during the zone-in flood for every populated slot.
///
/// Body: `ItemNum:u32 ItemNo:u16 Category:u8 ItemIndex:u8 LockFlg:u8
/// padding00[3]` → 12 bytes.
#[derive(Debug, Clone, Copy)]
pub struct ItemList {
    pub quantity: u32,
    /// FFXI item id from `Items.dat`.
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

/// `GP_SERV_COMMAND_ITEM_ATTR` (0x0020) — slot definition with
/// item-type-specific extdata. Sent on equip changes, augment updates,
/// and other "the slot's attributes changed" events. We surface the
/// 24-byte `Attr` payload as raw bytes — interpretation lives in
/// upstream item logic and isn't needed for v1 banking.
///
/// Body: `ItemNum:u32 Price:u32 ItemNo:u16 Category:u8 ItemIndex:u8
/// LockFlg:u8 Attr[24]` → 37 bytes.
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

/// `GP_SERV_COMMAND_EQUIP_LIST` (0x050) — one slot of equipped-gear
/// state. The server emits these once per equipment slot on login
/// (after a preceding `EQUIP_CLEAR`) and whenever the player equips
/// or unequips. The payload carries only the *reference* into the
/// inventory (container + slot index); resolving to the actual
/// `item_no` requires reading the inventory mirror.
///
/// Body: `PropertyItemIndex:u8 EquipKind:u8 Category:u8 padding00:u8`
/// → 4 bytes. See
/// `vendor/server/src/map/packets/s2c/0x050_equip_list.h`.
#[derive(Debug, Clone, Copy)]
pub struct EquipList {
    /// Slot index inside the source container.
    pub container_index: u8,
    /// Equipment slot id (SLOTTYPE: Main=0, Sub=1, Ranged=2, Ammo=3,
    /// Head=4, Body=5, Hands=6, Legs=7, Feet=8, Neck=9, Waist=10,
    /// LEar=11, REar=12, LRing=13, RRing=14, Back=15).
    pub equip_slot: u8,
    /// Source container id (CONTAINER_ID: Inventory=0, Wardrobe1=8,
    /// etc.).
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

/// `GP_SERV_COMMAND_MAGIC_DATA` (0x0AA) — 128-byte packed bitset of
/// learned spells. Bit `N` set means spell id `N` is known. Mirror
/// of `CCharEntity::m_SpellList: xi::bitset<1024>`. Server emits this
/// once on login and again on every spell-learned event.
///
/// Body: 128 bytes, little-endian-bit (bit `N` is at
/// `body[N >> 3] & (1 << (N & 7))`).
#[derive(Debug, Clone, Copy)]
pub struct MagicData<'a> {
    pub bitmap: &'a [u8; MAGIC_DATA_SIZE],
}

/// Body size of `GP_SERV_COMMAND_MAGIC_DATA` — top-level because
/// associated constants can't appear in type position in stable Rust.
pub const MAGIC_DATA_SIZE: usize = 128;

impl<'a> MagicData<'a> {
    pub const SIZE: usize = MAGIC_DATA_SIZE;
    /// Number of representable spell ids (`SIZE * 8`).
    pub const SPELL_ID_LIMIT: usize = Self::SIZE * 8;
    pub fn decode(body: &'a [u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let bitmap: &[u8; MAGIC_DATA_SIZE] = body[..Self::SIZE].try_into().unwrap();
        Ok(Self { bitmap })
    }
    /// Collect every set bit into a `Vec<u16>` of spell ids — caller
    /// owns the allocation. Cheaper alternative for streaming consumers
    /// is `is_known(id)`.
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

/// `GP_SERV_COMMAND_COMMAND_DATA` (0x0AC) — four packed bitsets in a
/// fixed order: WeaponSkills[64], JobAbilities[64], PetAbilities[64],
/// Traits[32]. Sent once on login, on job-change, and after any
/// trait/ability-acquisition event.
///
/// Bit `N` of each bitset indexes the corresponding LSB id (e.g.
/// `JobAbilities` bit N ↔ ability id N from `abilities.sql`). The
/// packet pads each section beyond what the server's `CCharEntity`
/// actually stores (e.g. server `m_WeaponSkills[32]`, packet 64);
/// the trailing bytes are always zero — see
/// `vendor/server/src/map/packets/s2c/0x0ac_command_data.cpp`.
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

/// Pull every set bit's ordinal out of `bitmap` as a `Vec<u16>` of ids.
/// Both `MagicData` and `CommandData` consumers use this to collapse a
/// bitset into a fixed list of ids the HUD can iterate.
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

    /// `MODEL_STANDARD = 0` (most mobs/NPCs): body[0x2C..0x30] is
    /// `(size=0, modelid)`. Decoder must surface `modelid`.
    #[test]
    fn look_data_decodes_standard_modelid() {
        let mut buf = vec![0u8; 0x40];
        buf[0x2C..0x2E].copy_from_slice(&0u16.to_le_bytes()); // size = MODEL_STANDARD
        buf[0x2E..0x30].copy_from_slice(&0x1234u16.to_le_bytes()); // modelid
        assert_eq!(
            LookData::decode_char_npc(&buf),
            Some(LookData::Standard { modelid: 0x1234 })
        );
    }

    /// `MODEL_EQUIPPED = 1`: full 20-byte look_t. Verify race+face split
    /// and equipment-slot ordering match the `look_t` union layout in
    /// `vendor/server/src/common/mmo.h:157-169`.
    #[test]
    fn look_data_decodes_equipped_look_t() {
        let mut buf = vec![0u8; 0x50];
        buf[0x2C..0x2E].copy_from_slice(&1u16.to_le_bytes()); // size = MODEL_EQUIPPED
        buf[0x2E] = 0x07; // face
        buf[0x2F] = 0x03; // race
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

    /// Truncated body (no room for the size sentinel) returns None
    /// instead of panicking.
    #[test]
    fn look_data_truncated_returns_none() {
        let buf = vec![0u8; 0x20];
        assert_eq!(LookData::decode_char_npc(&buf), None);
    }

    /// Unknown MODELTYPE sentinel returns None so the caller leaves
    /// look = None rather than fabricating a model.
    #[test]
    fn look_data_unknown_sentinel_returns_none() {
        let mut buf = vec![0u8; 0x40];
        buf[0x2C..0x2E].copy_from_slice(&0x00FFu16.to_le_bytes());
        assert_eq!(LookData::decode_char_npc(&buf), None);
    }

    /// CHAR_PC `GrapIDTbl[9]` at body[0x44..0x56]: slot 0 packs
    /// `(race<<8 | face)` and slots 1..=8 are gear ids OR'd with a
    /// per-slot `0xN000` mask. Decoder must strip the mask and surface
    /// the raw 12-bit model id.
    #[test]
    fn look_data_decodes_pc_grapidtbl() {
        // Body length must extend through GrapIDTbl: 0x44 + 18 = 0x56.
        let mut buf = vec![0u8; 0x60];
        let off = LookData::CHAR_PC_GRAP_OFFSET;
        // slot 0: face = 0x07 (low), race = 0x01 (high; Hume Male)
        buf[off..off + 2].copy_from_slice(&0x0107u16.to_le_bytes());
        // Build each gear slot exactly the way the server does:
        //   GrapIDTbl[i] = look->slot + 0xi000
        // with a model id that has nonzero high nibble to prove
        // mask-stripping (e.g. 0xABC + mask must come back as 0xABC).
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

    /// `GrapIDTbl[0] == 0` means the server hasn't populated look yet
    /// (SendFlg.Model clear). Decoder returns None so the caller leaves
    /// the existing look untouched rather than overwriting with a
    /// nonsense (face=0, race=0) entry.
    #[test]
    fn look_data_pc_zero_modelid_returns_none() {
        let buf = vec![0u8; 0x60];
        assert_eq!(LookData::decode_char_pc(&buf), None);
    }

    /// Truncated body (less than 0x44 + 18 = 0x56 bytes) returns None
    /// without panicking on the GrapIDTbl read.
    #[test]
    fn look_data_pc_truncated_returns_none() {
        // 0x55 bytes — one byte short of slot 8's tail.
        let mut buf = vec![0u8; 0x55];
        // Even with a valid slot 0, truncation must dominate.
        buf[LookData::CHAR_PC_GRAP_OFFSET..LookData::CHAR_PC_GRAP_OFFSET + 2]
            .copy_from_slice(&0x0107u16.to_le_bytes());
        assert_eq!(LookData::decode_char_pc(&buf), None);
    }

    #[test]
    fn pos_head_minimal_decode() {
        // Build a minimal 40-byte body with known field values.
        let mut buf = vec![0u8; PosHead::SIZE];
        buf[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes()); // UniqueNo
        buf[4..6].copy_from_slice(&0x0042u16.to_le_bytes()); // ActIndex
        buf[6] = 0b0000_0001; // SendFlg.Position
        buf[7] = 64; // dir
        buf[8..12].copy_from_slice(&123.5f32.to_le_bytes()); // x
        buf[12..16].copy_from_slice(&(-12.0f32).to_le_bytes()); // z
        buf[16..20].copy_from_slice(&7.25f32.to_le_bytes()); // y
        buf[24] = 25; // speed
        buf[25] = 25; // speed_base
        buf[26] = 100; // hpp
        buf[27] = 1; // server_status

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
        // Fabricate a 48-byte LOGIN body prefix with PosHead + ZoneNo.
        // ZoneNo for Southern San d'Oria is 230 (0xE6) per
        // `vendor/Phoenix/src/map/zone_id.h`.
        let mut buf = vec![0u8; ServerLogin::SIZE];
        buf[0..4].copy_from_slice(&0x0123_4567u32.to_le_bytes()); // UniqueNo
        buf[4..6].copy_from_slice(&0x00FFu16.to_le_bytes()); // ActIndex
        buf[44..48].copy_from_slice(&230u32.to_le_bytes()); // ZoneNo
        let l = ServerLogin::decode(&buf).unwrap();
        assert_eq!(l.unique_no, 0x0123_4567);
        assert_eq!(l.act_index, 0x00FF);
        assert_eq!(l.zone_no, 230);
    }

    #[test]
    fn server_login_truncated_errors() {
        // 47 bytes — one byte short of ZoneNo's tail.
        let buf = vec![0u8; ServerLogin::SIZE - 1];
        assert!(matches!(
            ServerLogin::decode(&buf),
            Err(DecodeError::Truncated(48, _))
        ));
    }

    #[test]
    fn server_login_carries_pos_head_for_spawn_seed() {
        let mut buf = vec![0u8; ServerLogin::SIZE];
        buf[0..4].copy_from_slice(&0x0123_4567u32.to_le_bytes()); // UniqueNo
        buf[4..6].copy_from_slice(&0x00FFu16.to_le_bytes()); // ActIndex
        buf[7] = 96; // dir
        buf[8..12].copy_from_slice(&(-115.5f32).to_le_bytes()); // x
        buf[12..16].copy_from_slice(&(7.25f32).to_le_bytes()); // z (height)
        buf[16..20].copy_from_slice(&(280.0f32).to_le_bytes()); // y (north)
        buf[24] = 40; // speed
        buf[25] = 40; // speed_base
        buf[44..48].copy_from_slice(&230u32.to_le_bytes()); // ZoneNo
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
        // EXECUTING_LOGOUT (id=7) with 30 seconds remaining.
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
        buf[0..4].copy_from_slice(&2u32.to_le_bytes()); // ZONECHANGE
        buf[4..8].copy_from_slice(&0x6F00_A8C0u32.to_le_bytes()); // 192.168.0.111-ish
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
        // 44-byte body — full Phoenix layout including BtTargetID.
        let mut buf = vec![0u8; PosHead::SIZE_WITH_BT_TARGET];
        buf[0..4].copy_from_slice(&0xCAFE_F00Du32.to_le_bytes()); // UniqueNo
        buf[40..44].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes()); // BtTargetID
        let h = PosHead::decode(&buf).unwrap();
        assert_eq!(h.unique_no, 0xCAFE_F00D);
        assert_eq!(h.bt_target_id, 0xDEAD_BEEF);
    }

    #[test]
    fn decode_char_npc_extracts_claim_id() {
        // Fabricate a CHAR_NPC body with a non-zero claim_id (= m_OwnerID).
        // Per `entity_update.cpp::updateWith`, packet-absolute 0x2C is the
        // owner-id slot; subtracting the 4-byte sub-packet header puts it
        // at body offset 40..44 — the same slot PosHead names
        // `bt_target_id` for the CHAR_PC repurposing.
        let mut buf = vec![0u8; PosHead::SIZE_WITH_BT_TARGET];
        buf[0..4].copy_from_slice(&0xAABB_CCDDu32.to_le_bytes()); // UniqueNo (mob id)
        buf[4..6].copy_from_slice(&0x07F0u16.to_le_bytes()); // ActIndex (mob range)
        buf[40..44].copy_from_slice(&0x0123_4567u32.to_le_bytes()); // m_OwnerID
        let (head, claim_id) = PosHead::decode_char_npc(&buf).unwrap();
        assert_eq!(head.unique_no, 0xAABB_CCDD);
        assert_eq!(head.act_index, 0x07F0);
        assert_eq!(claim_id, 0x0123_4567);
    }

    #[test]
    fn decode_char_npc_unclaimed_yields_zero_claim() {
        // Position-only body (40 bytes) — pre-claim layout / not a status
        // update. claim_id reads as 0, matching unclaimed semantics.
        let buf = vec![0u8; PosHead::SIZE];
        let (_, claim_id) = PosHead::decode_char_npc(&buf).unwrap();
        assert_eq!(claim_id, 0);
    }

    #[test]
    fn pos_head_legacy_40_byte_body_yields_zero_bt_target() {
        // PS2-era layout ended at Flags3. We accept these but bt_target_id is 0.
        let buf = vec![0u8; PosHead::SIZE];
        let h = PosHead::decode(&buf).unwrap();
        assert_eq!(h.bt_target_id, 0);
    }

    #[test]
    fn party_attrs_group_attr_decodes() {
        let mut buf = vec![0u8; 36];
        buf[0..4].copy_from_slice(&0x0001_0042u32.to_le_bytes()); // UniqueNo
        buf[4..8].copy_from_slice(&1500u32.to_le_bytes()); // Hp
        buf[8..12].copy_from_slice(&500u32.to_le_bytes()); // Mp
        buf[12..16].copy_from_slice(&1234u32.to_le_bytes()); // Tp
        buf[16..18].copy_from_slice(&0x0042u16.to_le_bytes()); // ActIndex
        buf[18] = 75; // Hpp
        buf[19] = 50; // Mpp
        buf[20] = 0; // Kind = PC
        buf[21] = 1; // MoghouseFlg = inside mog house
        buf[22..24].copy_from_slice(&234u16.to_le_bytes()); // ZoneNo
        buf[28] = 6; // mjob_no = WHM
        buf[29] = 75; // mjob_lv
        buf[30] = 1; // sjob_no = WAR
        buf[31] = 37; // sjob_lv

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
        buf[0..4].copy_from_slice(&0x0010_0001u32.to_le_bytes()); // UniqueNo
        buf[4..8].copy_from_slice(&2000u32.to_le_bytes()); // Hp
        buf[8..12].copy_from_slice(&100u32.to_le_bytes()); // Mp
        buf[12..16].copy_from_slice(&0u32.to_le_bytes()); // Tp
                                                          // GAttr bitfield: PartyNo:2 (=1), PartyLeaderFlg:1 (=1), AllianceLeaderFlg:1 (=0)
                                                          // → low 4 bits = 0b0101 = 5
        buf[16..20].copy_from_slice(&0x0000_0005u32.to_le_bytes());
        buf[20..22].copy_from_slice(&0x0007u16.to_le_bytes()); // ActIndex
        buf[22] = 1; // MemberNumber
        buf[23] = 1; // MoghouseFlg = inside mog house
        buf[24] = 0; // Kind
        buf[25] = 100; // Hpp
        buf[26] = 100; // Mpp
        buf[28..30].copy_from_slice(&230u16.to_le_bytes()); // ZoneNo (Bastok Markets)
        buf[30] = 1; // mjob WAR
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
        // Phoenix writes `1 + size` / `1 + buff` to the wire (see
        // `0x01c_item_max.cpp`), so the decoder must subtract 1.
        // 18 legacy u8 caps, 14 padding, 18 wide u16 caps, 28 padding.
        let mut buf = vec![0u8; ItemMax::SIZE];
        // Inventory (id=0): legacy 81, wide 0 → resolves to legacy → 80.
        buf[0] = 81;
        // Mog Safe (id=1): legacy 81, wide 201 → wide wins → 200.
        buf[1] = 81;
        let wide_off = 18 + 14 + 1 * 2;
        buf[wide_off..wide_off + 2].copy_from_slice(&201u16.to_le_bytes());
        // Wardrobe2 (id=10): legacy 0, wide 81 → wide-only → 80.
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
        // Phoenix sets `ItemNum2 = 0` to disable a container (e.g.
        // moglocker without access). A naive `- 1` would underflow to
        // u16::MAX; the saturating sub keeps it at 0.
        let mut buf = vec![0u8; ItemMax::SIZE];
        // Moglocker (id=4): legacy 1+size=21, but disabled wide=0.
        // Falls back to legacy and decodes the real size.
        buf[4] = 21;
        // Mog Safe (id=1): both zero → both disabled → 0.
        // (Pure sentinel; nothing to assert beyond the absence of
        // underflow.)
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
        // StillLoading
        let mut buf = vec![0u8; ItemSame::SIZE];
        buf[0] = 0;
        buf[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
        let s = ItemSame::decode(&buf).unwrap();
        assert_eq!(s.state, ItemSameState::StillLoading);
        assert_eq!(s.flags, 0xCAFE);

        // AllLoaded
        buf[0] = 1;
        let s = ItemSame::decode(&buf).unwrap();
        assert_eq!(s.state, ItemSameState::AllLoaded);
    }

    #[test]
    fn item_num_decodes() {
        let mut buf = vec![0u8; ItemNum::SIZE];
        buf[0..4].copy_from_slice(&12345u32.to_le_bytes()); // ItemNum (quantity)
        buf[4] = 0; // Category = LOC_INVENTORY
        buf[5] = 7; // ItemIndex
        buf[6] = 1; // LockFlg
        let n = ItemNum::decode(&buf).unwrap();
        assert_eq!(n.quantity, 12345);
        assert_eq!(n.category, 0);
        assert_eq!(n.index, 7);
        assert_eq!(n.lock_flg, 1);
    }

    #[test]
    fn item_list_decodes() {
        let mut buf = vec![0u8; ItemList::SIZE];
        buf[0..4].copy_from_slice(&1u32.to_le_bytes()); // qty
        buf[4..6].copy_from_slice(&4112u16.to_le_bytes()); // ItemNo (Hi-Potion)
        buf[6] = 5; // LOC_MOGSATCHEL
        buf[7] = 12; // slot
        buf[8] = 0; // LockFlg
        let l = ItemList::decode(&buf).unwrap();
        assert_eq!(l.quantity, 1);
        assert_eq!(l.item_no, 4112);
        assert_eq!(l.category, 5);
        assert_eq!(l.index, 12);
    }

    #[test]
    fn item_attr_decodes_with_extdata() {
        let mut buf = vec![0u8; ItemAttr::SIZE];
        buf[0..4].copy_from_slice(&1u32.to_le_bytes()); // qty
        buf[4..8].copy_from_slice(&500_000u32.to_le_bytes()); // Price
        buf[8..10].copy_from_slice(&8000u16.to_le_bytes()); // ItemNo
        buf[10] = 0; // LOC_INVENTORY
        buf[11] = 3; // slot
        buf[12] = 0; // LockFlg
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
        // CHAR_NPC body with UPDATE_NAME set (body[6] & 0x08) and the
        // name slot at fixed offset 0x30.
        let mut buf = vec![0u8; 64];
        buf[6] = 0x08; // updatemask: UPDATE_NAME
        buf[0x30..0x30 + 9].copy_from_slice(b"Sigli-Sea");
        let name = PosHead::try_extract_name(s2c::CHAR_NPC, &buf);
        assert_eq!(name.as_deref(), Some("Sigli-Sea"));
    }

    #[test]
    fn try_extract_name_returns_none_without_update_name() {
        use crate::map::s2c;
        // body[6] = 0 → Name bit clear → no name even if name bytes
        // happen to sit in the slot.
        let mut buf = vec![0u8; 64];
        buf[0x30..0x30 + 5].copy_from_slice(b"Junk!");
        assert!(PosHead::try_extract_name(s2c::CHAR_NPC, &buf).is_none());
    }

    #[test]
    fn try_extract_name_char_npc_renamed_low_targid_shift() {
        use crate::map::s2c;
        // Renamed entity with targid < 1024: server writes a 0x01 marker
        // at body[0x30] and shifts the name to body[0x31].
        // Server reference: entity_update.cpp:575-578.
        let mut buf = vec![0u8; 68];
        buf[6] = 0x08; // UPDATE_NAME
        buf[0x30] = 0x01; // marker
        buf[0x31..0x31 + 12].copy_from_slice(b"Big Bad Bee\0");
        let name = PosHead::try_extract_name(s2c::CHAR_NPC, &buf);
        assert_eq!(name.as_deref(), Some("Big Bad Bee"));
    }

    #[test]
    fn try_extract_name_char_pc_uses_fixed_offset_with_send_flag() {
        use crate::map::s2c;
        // CHAR_PC: SendFlg.Name bit set, name at body offset 0x56.
        // Offset verified empirically against a live LSB packet where the
        // PC was named "Cleric" — see `debug://name_misses` capture.
        let mut buf = vec![0u8; 0x60];
        buf[6] = 0x08; // SendFlg.Name
        buf[0x56..0x56 + 6].copy_from_slice(b"Cleric");
        let name = PosHead::try_extract_name(s2c::CHAR_PC, &buf);
        assert_eq!(name.as_deref(), Some("Cleric"));
    }

    #[test]
    fn try_extract_name_char_pc_rejects_when_send_flag_clear() {
        use crate::map::s2c;
        // Bogus byte at body offset 0x56 would have been read by the old
        // trailing-16 trick; with SendFlg.Name clear, we now return None.
        let mut buf = vec![0u8; 0x60];
        buf[6] = 0x01; // Position only — Name not set
        buf[0x56..0x56 + 6].copy_from_slice(b"Junked");
        assert!(PosHead::try_extract_name(s2c::CHAR_PC, &buf).is_none());
    }

    #[test]
    fn entity_set_name_decodes_trust_name() {
        // Fabricate the LSB layout for CEntitySetNamePacket (0x067 sub-type
        // 0x03). Body offsets = LSB offsets minus the 4-byte sub-packet
        // header.
        let mut buf = vec![0u8; 0x28];
        buf[0] = 0x03; // sub-type
        buf[1] = 0x05;
        buf[2..4].copy_from_slice(&0x07F2u16.to_le_bytes()); // targid
        buf[4..8].copy_from_slice(&0x0123_45F2u32.to_le_bytes()); // id
        buf[8..10].copy_from_slice(&0x0042u16.to_le_bytes()); // master targid
        buf[0x14..0x14 + 13].copy_from_slice(b"Mihli Aliapoh");

        let ent = EntitySetName::decode(&buf).unwrap();
        assert_eq!(ent.targid, 0x07F2);
        assert_eq!(ent.id, 0x0123_45F2);
        assert_eq!(ent.master_targid, 0x0042);
        assert_eq!(ent.name.as_deref(), Some("Mihli Aliapoh"));
    }

    #[test]
    fn entity_set_name_short_name_rejected() {
        // body[0x14..] = "Mi\0" — only 2 chars before NUL; below the
        // 3-char floor we use to filter false positives.
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
        // Active-pet variant: full pet header + name.
        let mut buf = vec![0u8; 0x28];
        buf[0] = 0x04; // message-type bit
        buf[2..4].copy_from_slice(&0x0001u16.to_le_bytes()); // owner targid
        buf[4..8].copy_from_slice(&0x0010_0001u32.to_le_bytes()); // owner id
        buf[8..10].copy_from_slice(&0x07A5u16.to_le_bytes()); // pet targid
        buf[0x0A] = 87; // HP%
        buf[0x0B] = 60; // MP%
        buf[0x0C..0x0E].copy_from_slice(&1234u16.to_le_bytes()); // TP
        buf[0x10..0x14].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes()); // bt target
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
        // Despawn: only the owner header is present (sub.size 0x1C → body
        // 0x18). All pet fields should read as zeros / None — no panic.
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

    /// `0x057 WEATHER` body is `u32 StartTime, u16 WeatherNumber,
    /// u16 WeatherOffsetTime`. Confirm little-endian decode of each field.
    #[test]
    fn weather_packet_decodes_fields() {
        let mut buf = [0u8; WeatherPacket::SIZE];
        buf[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes()); // StartTime
        buf[4..6].copy_from_slice(&6u16.to_le_bytes()); // WeatherNumber = Rain
        buf[6..8].copy_from_slice(&0x0123u16.to_le_bytes()); // WeatherOffsetTime
        let w = WeatherPacket::decode(&buf).unwrap();
        assert_eq!(w.start_time, 0xDEAD_BEEF);
        assert_eq!(w.weather_number, 6);
        assert_eq!(w.offset_time, 0x0123);
    }

    /// Truncated body (one byte short of the 8-byte fixed layout) returns
    /// `DecodeError::Truncated` instead of panicking.
    #[test]
    fn weather_packet_truncated_returns_err() {
        let buf = [0u8; WeatherPacket::SIZE - 1];
        assert!(matches!(
            WeatherPacket::decode(&buf),
            Err(DecodeError::Truncated(WeatherPacket::SIZE, n)) if n == WeatherPacket::SIZE - 1
        ));
    }

    /// `EQUIP_LIST` is byte-laid as
    /// `PropertyItemIndex EquipKind Category padding00`.
    /// Pin the field order so a field swap in a future edit can't
    /// silently mis-attribute container vs slot vs index.
    #[test]
    fn equip_list_decodes_field_order() {
        // Slot index 5 in container 8 (Wardrobe1) for equip slot 4 (Head).
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

    /// `MagicData` bitmap layout: bit N of body[N>>3] == spell id N is
    /// known. Pin the bit-order so a refactor that swaps the shift
    /// direction (a common bug) gets caught.
    #[test]
    fn magic_data_known_ids_picks_set_bits() {
        let mut buf = [0u8; MagicData::SIZE];
        // Set spell ids 0, 7, 8, 17, and 1023 (the high bit of the
        // last byte). Each comes from a different byte+bit pair so a
        // shift-direction or bit-order regression surfaces here.
        buf[0] = 0b1000_0001; // ids 0 + 7
        buf[1] = 0b0000_0001; // id 8
        buf[2] = 0b0000_0010; // id 17
        buf[127] = 0b1000_0000; // id 1023
        let m = MagicData::decode(&buf).unwrap();
        assert_eq!(m.known_ids(), vec![0, 7, 8, 17, 1023]);
        assert!(m.is_known(0));
        assert!(m.is_known(7));
        assert!(m.is_known(1023));
        assert!(!m.is_known(1));
        // Out-of-range queries return false rather than panicking
        // (caller might pass an SQL-derived id that exceeds 1024 bits).
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

    /// `CommandData` slices: the four sub-bitsets must come out at
    /// the right offsets and lengths so the HUD doesn't display
    /// WeaponSkills as JobAbilities (or vice versa).
    #[test]
    fn command_data_splits_into_four_bitsets() {
        let mut buf = [0u8; CommandData::SIZE];
        // Plant a unique byte at the start of each sub-bitset so a
        // wrong offset surfaces as a swap.
        buf[0] = 0xA1; // weapon_skills[0]
        buf[64] = 0xA2; // job_abilities[0]
        buf[128] = 0xA3; // pet_abilities[0]
        buf[192] = 0xA4; // traits[0]
        let c = CommandData::decode(&buf).unwrap();
        assert_eq!(c.weapon_skills[0], 0xA1);
        assert_eq!(c.job_abilities[0], 0xA2);
        assert_eq!(c.pet_abilities[0], 0xA3);
        assert_eq!(c.traits[0], 0xA4);
        // Sizes pinned for the same reason — a future "let's grow
        // PetAbilities" would otherwise silently shift Traits.
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
}
