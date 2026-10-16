use super::*;
use crate::s2c_layout::clistatus as lsb;

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
    /// `MOUNTTYPE` of the mount being ridden, 0 when not mounted *and* when the
    /// mount is a plain chocobo — both are `MOUNT_CHOCOBO`. Read `server_status`
    /// through [`animation::is_mounted`] to tell those apart.
    /// `packet->mount_id` in CCharStatusPacket's constructor,
    /// vendor/server/src/map/packets/char_status.cpp.
    pub mount_id: u8,
    /// The allegiance byte retail's own nameplate keys off for self:
    /// `Flags2.BallistaFlg`. LSB writes `PChar->allegiance` there and ORs in the
    /// belligerence bit (0x08) while a monstrosity is outside the Ferretory, so 8/9
    /// are MOB/PLAYER | belligerent. Self never receives its own 0x0D —
    /// `CZoneEntities::UpdateEntityPacket` skips the sender
    /// (vendor/server/src/map/zone_entities.cpp) — this packet is the only channel.
    /// vendor/server/src/map/packets/char_status.cpp.
    pub allegiance: u8,
}

impl CharStatus {
    pub(crate) const UNIQUE_NO_OFFSET: usize = 0x20;
    pub(crate) const FLAGS0_OFFSET: usize = 0x24;
    /// `flags0_t.hpp : 8` at bits 16..24 (vendor/server/src/map/packets/char_status.cpp flags0_t).
    pub(crate) const HPP_SHIFT: u32 = 16;
    pub(crate) const HPP_MASK: u32 = 0xFF;
    pub(crate) const SPEED_OFFSET: usize = 0x28;
    pub(crate) const SERVER_STATUS_OFFSET: usize = 0x2C;
    pub(crate) const DEAD_COUNTER1_OFFSET: usize = 0x38;
    pub(crate) const DEAD_COUNTER2_OFFSET: usize = 0x3C;
    pub(crate) const FISHING_TIMER_OFFSET: usize = 0x46;
    /// The `Flags2` word. Struct layout in char_status.cpp: BufStatus[32] @0x04,
    /// UniqueNo @0x24, Flags0 @0x28, Flags1 @0x2C, server_status/r/g/b
    /// @0x30..0x34, Flags2 @0x34 — the "Flags3 starts at 0x38" comment there is
    /// struct-relative. Retail's 0x037.h static_asserts the same word at body
    /// 0x30 (`MoreFlags`) and its handler reads `AUDIT_1FF = MoreFlags.dummy & 0xFF`
    /// (research/XIClient/src/XIClient/source/Game/Net/Packets/s2c/0x037.cpp).
    pub(crate) const FLAGS2_OFFSET: usize = 0x30;
    /// `BallistaFlg`'s position inside the Flags2 word (bits 21..28),
    /// char_status.cpp `flags2_t`.
    pub(crate) const BALLISTA_FLG_SHIFT: u32 = 21;
    pub(crate) const BALLISTA_FLG_MASK: u32 = 0xFF;
    /// `field_57`, the byte the disassembly's own struct puts right before
    /// `Field58Flags` (research/XIClient/src/XIClient/include/Game/Net/Packets/s2c/0x037.h
    /// static_asserts), which LSB fills from the mount effect's power.
    pub(crate) const MOUNT_ID_OFFSET: usize = 0x57;
    pub(crate) const SPEED_MASK: u16 = 0x0FFF;
    pub(crate) const MIN_LEN: usize = Self::DEAD_COUNTER2_OFFSET + 4;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        let need = Self::MIN_LEN;
        if body.len() < need {
            return Err(DecodeError::Truncated(need, body.len()));
        }
        let rd = |o: usize| u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        let flags0 = rd(Self::FLAGS0_OFFSET);
        Ok(Self {
            unique_no: rd(Self::UNIQUE_NO_OFFSET),
            hpp: ((flags0 >> Self::HPP_SHIFT) & Self::HPP_MASK) as u8,
            dead_counter1: rd(Self::DEAD_COUNTER1_OFFSET),
            dead_counter2: rd(Self::DEAD_COUNTER2_OFFSET),
            server_status: body[Self::SERVER_STATUS_OFFSET],
            fishing_timer: body.get(Self::FISHING_TIMER_OFFSET).copied().unwrap_or(0),
            speed: u16::from_le_bytes([body[Self::SPEED_OFFSET], body[Self::SPEED_OFFSET + 1]])
                & Self::SPEED_MASK,
            mount_id: body.get(Self::MOUNT_ID_OFFSET).copied().unwrap_or(0),
            allegiance: ((rd(Self::FLAGS2_OFFSET) >> Self::BALLISTA_FLG_SHIFT)
                & Self::BALLISTA_FLG_MASK) as u8,
        })
    }

    /// Seconds until the server force-warps a KO'd player home. Meaningless
    /// unless `hpp == 0`; see [`dead_counter_seconds_until_homepoint`].
    pub fn seconds_until_homepoint(&self) -> u32 {
        dead_counter_seconds_until_homepoint(self.dead_counter1)
    }
}

/// The scale LSB applies to the second count in every dead-counter carrier
/// (`60 * deadRemaining` in vendor/server/src/map/packets/char_status.cpp and
/// vendor/server/src/map/packets/s2c/0x00a_login.cpp). Retail divides straight
/// back out: `Payload.field_A4 / 60`
/// (research/XIClient/src/XIClient/source/Game/Net/Packets/s2c/0x00A.cpp S2C::RecvLogin).
pub(crate) const DEAD_COUNTER_UNITS_PER_SECOND: u32 = 60;

/// The fixed padding LSB prepends to the real remaining time (`6min +
/// PChar->GetTimeUntilDeathHomepoint()`), whose own comment records that the
/// client treats 66min as the death maximum and force-homepoints once the value
/// drops below 6min. vendor/server/src/map/packets/char_status.cpp.
pub(crate) const DEAD_COUNTER_PADDING_SECS: u32 = 6 * 60;

/// Seconds until the server force-warps a KO'd player home, from the raw counter
/// both 0x037 (`dead_counter1`) and 0x00A (`DeadCounter`) carry with the same
/// encoding. Stripping the fixed padding yields the real time left, which hits 0
/// when the server-side CDeathState completes at death + 60min
/// (vendor/server/src/map/entities/char_entity.cpp::GetTimeUntilDeathHomepoint,
/// vendor/server/src/map/ai/states/death_state.cpp).
///
/// The counter alone cannot tell a corpse from a living character: LSB computes
/// it unconditionally and `GetTimeSinceDeath()` returns 0s while alive, so a
/// living character is byte-identical to a fresh corpse. Callers must gate on the
/// carrier's `hpp == 0`.
pub fn dead_counter_seconds_until_homepoint(dead_counter: u32) -> u32 {
    (dead_counter / DEAD_COUNTER_UNITS_PER_SECOND).saturating_sub(DEAD_COUNTER_PADDING_SECS)
}

const _: () = assert!(CharStatus::SPEED_OFFSET + 2 <= CharStatus::MIN_LEN);
const _: () = assert!(CharStatus::FLAGS2_OFFSET + 4 <= CharStatus::MIN_LEN);

/// s2c 0x061 GP_SERV_COMMAND_CLISTATUS — the self-character stat block.
/// Field offsets follow the `CLISTATUS` struct in
/// vendor/server/src/map/packets/s2c/0x061_clistatus.h (mirror of
/// research/XiPackets/world/server/0x0061). `bp_base`/`bp_adj` are STR, DEX, VIT,
/// AGI, INT, MND, CHR in order; `bp_adj` is the signed gear/buff delta retail shows
/// as the "+N" beside each stat. `def_elem` is Fire, Ice, Wind, Earth, Lightning,
/// Water, Light, Dark. The struct declares `atk`/`def` as int16_t, but they are
/// sourced from the non-negative ATT()/DEF() calls in the packer, so we read them as u16.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CliStatus {
    pub hp_max: u32,
    pub mp_max: u32,
    pub mjob_no: u8,
    pub mjob_lv: u8,
    pub sjob_no: u8,
    pub sjob_lv: u8,
    /// Current- and next-level EXP thresholds carried straight from the wire so
    /// the `level` box under the persona panel answers (LSB
    /// vendor/server/src/map/packets/s2c/0x061_clistatus.h CLISTATUS exp_now/exp_next).
    pub exp_now: i16,
    pub exp_next: i16,
    pub bp_base: [u16; Self::BP_COUNT],
    pub bp_adj: [i16; Self::BP_COUNT],
    pub attack: u16,
    pub defense: u16,
    pub def_elem: [i16; Self::DEF_ELEM_COUNT],
    /// `ilvl` — the amount above 99 (0 renders as `---`). Retail's persona line is
    /// `99 + ilvl`, and it reads this byte at packet +0x55
    /// (FFXiMain.dll retail-2026-09 RVA 0x147287).
    pub ilvl: u8,
    /// `su_lv` — the character's superior level (vendor/server/src/map/packets/s2c/0x061_clistatus.h
    /// CLISTATUS su_lv, packet +0x52 in retail's view). It gates equipment
    /// (CanEquip compares an item's `su` against it) and fills the persona panel's
    /// `Su` line (FFXiMain.dll retail-2026-09 RVA 0x147287).
    pub su_lv: u8,
    /// The per-slot equip-block mask (s2c 0x061 packet +0x60; see
    /// [`Self::EQUIP_SLOT_BLOCK_OFFSET`] for the client law it feeds).
    pub equip_slot_block: u32,
    /// bit 0 of `mastery_info.flags` — master-level play unlocked for this character.
    /// Retail greys the Status window's Master Levels row unless this bit is set
    /// *and* the main job is level 99 (FFXiMain.dll retail-2026-09 `.text 0x148B20`,
    /// `cmp al,0x63` + `test byte ptr […],1`).
    pub master_level_unlocked: bool,
    /// `unity_info.Faction`: the character's Unity Concord, 0 for none
    /// (vendor/server/src/map/packets/s2c/0x061_clistatus.h unityinfo_t).
    pub unity_faction: u8,
    /// `unity_info.Points`: the 17-bit Unity accolades total LSB fills from `unity_accolades`.
    pub unity_points: u32,
    /// `unity_points1` / `unity_points2`: LSB's current and previous week accolades / 1000
    /// (vendor/server/src/map/packets/s2c/0x061_clistatus.cpp).
    pub unity_points1: u16,
    pub unity_points2: u16,
}

impl CliStatus {
    // vendor/server/src/map/packets/s2c/0x061_clistatus.h CLISTATUS
    const HP_MAX_OFFSET: usize = 0;
    const MP_MAX_OFFSET: usize = 4;
    const MJOB_NO_OFFSET: usize = 8;
    const MJOB_LV_OFFSET: usize = 9;
    const SJOB_NO_OFFSET: usize = 10;
    const SJOB_LV_OFFSET: usize = 11;
    // CLISTATUS keeps `int16_t exp_now/exp_next` between the sub-job bytes and
    // `bp_base[7]` (vendor/server/src/map/packets/s2c/0x061_clistatus.h).
    const EXP_NOW_OFFSET: usize = 12;
    const EXP_NEXT_OFFSET: usize = 14;
    const BP_BASE_OFFSET: usize = 16;
    const BP_ADJ_OFFSET: usize = 30;
    const ATTACK_OFFSET: usize = 44;
    const DEFENSE_OFFSET: usize = 46;
    const DEF_ELEM_OFFSET: usize = 48;
    const ILVL_OFFSET: usize = 81;
    // vendor/server/src/map/packets/s2c/0x061_clistatus.h CLISTATUS su_lv (packet +0x52)
    pub(crate) const SU_LV_OFFSET: usize = 0x4E;
    // ... masteryinfo_t flags byte (packet +0x66): the master-level unlock.
    pub(crate) const MASTERY_INFO_FLAGS_OFFSET: usize = 0x62;
    /// The dword at `unity_chat_color_flag` (packet +0x60). Despite that XiPackets name, its
    /// consumer is equipment law: the client copies it straight into game state (+0x40DF0) from
    /// this packet's parse (FFXiMain.dll retail-2026-09 `.text 0xF0982`) and reads it back through
    /// `.text 0xFD350`; the equip legality check refuses when an item's record slot mask intersects
    /// it (`.text 0x132111` in RVA 0x132050, and again at `.text 0x1321AC`). Per-slot bits; a
    /// zero dword blocks nothing.
    pub(crate) const EQUIP_SLOT_BLOCK_OFFSET: usize = 0x5C;
    /// MASTER_LEVEL_UNLOCK_BIT — bit 0 of `mastery_info.flags`.
    pub(crate) const MASTER_LEVEL_UNLOCK_BIT: u8 = 1;
    // vendor/server/src/map/packets/s2c/0x061_clistatus.h unityinfo_t bitfield word
    pub(crate) const UNITY_INFO_OFFSET: usize = 0x54;
    pub(crate) const UNITY_FACTION_SHIFT: u32 = 0;
    pub(crate) const UNITY_FACTION_MASK: u32 = 0x1F;
    pub(crate) const UNITY_POINTS_SHIFT: u32 = 10;
    pub(crate) const UNITY_POINTS_MASK: u32 = 0x1_FFFF;
    pub(crate) const UNITY_POINTS1_OFFSET: usize = 0x58;
    pub(crate) const UNITY_POINTS2_OFFSET: usize = 0x5A;
    const NEEDED: usize = Self::MASTERY_INFO_FLAGS_OFFSET + 1;

    /// STR, DEX, VIT, AGI, INT, MND, CHR.
    pub const BP_COUNT: usize = 7;
    /// Fire, Ice, Wind, Earth, Lightning, Water, Light, Dark.
    pub const DEF_ELEM_COUNT: usize = 8;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::NEEDED {
            return Err(DecodeError::Truncated(Self::NEEDED, body.len()));
        }
        let rd32 = |o: usize| u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        let rd16 = |o: usize| u16::from_le_bytes([body[o], body[o + 1]]);
        let rdi16 = |o: usize| i16::from_le_bytes([body[o], body[o + 1]]);
        let mut bp_base = [0u16; Self::BP_COUNT];
        let mut bp_adj = [0i16; Self::BP_COUNT];
        for i in 0..Self::BP_COUNT {
            bp_base[i] = rd16(Self::BP_BASE_OFFSET + i * 2);
            bp_adj[i] = rdi16(Self::BP_ADJ_OFFSET + i * 2);
        }
        let mut def_elem = [0i16; Self::DEF_ELEM_COUNT];
        for (i, e) in def_elem.iter_mut().enumerate() {
            *e = rdi16(Self::DEF_ELEM_OFFSET + i * 2);
        }
        let unity_info = rd32(Self::UNITY_INFO_OFFSET);
        Ok(Self {
            hp_max: rd32(Self::HP_MAX_OFFSET),
            mp_max: rd32(Self::MP_MAX_OFFSET),
            mjob_no: body[Self::MJOB_NO_OFFSET],
            mjob_lv: body[Self::MJOB_LV_OFFSET],
            sjob_no: body[Self::SJOB_NO_OFFSET],
            sjob_lv: body[Self::SJOB_LV_OFFSET],
            exp_now: rdi16(Self::EXP_NOW_OFFSET),
            exp_next: rdi16(Self::EXP_NEXT_OFFSET),
            bp_base,
            bp_adj,
            attack: rd16(Self::ATTACK_OFFSET),
            defense: rd16(Self::DEFENSE_OFFSET),
            def_elem,
            ilvl: body[Self::ILVL_OFFSET],
            su_lv: body[Self::SU_LV_OFFSET],
            equip_slot_block: rd32(Self::EQUIP_SLOT_BLOCK_OFFSET),
            master_level_unlocked: body[Self::MASTERY_INFO_FLAGS_OFFSET]
                & Self::MASTER_LEVEL_UNLOCK_BIT
                != 0,
            unity_faction: ((unity_info >> Self::UNITY_FACTION_SHIFT) & Self::UNITY_FACTION_MASK)
                as u8,
            unity_points: (unity_info >> Self::UNITY_POINTS_SHIFT) & Self::UNITY_POINTS_MASK,
            unity_points1: rd16(Self::UNITY_POINTS1_OFFSET),
            unity_points2: rd16(Self::UNITY_POINTS2_OFFSET),
        })
    }
}

pin_s2c_offset!(
    CliStatus::UNITY_INFO_OFFSET,
    crate::s2c_layout::clistatus::STATUSDATA_UNITY_INFO,
    "CLISTATUS.unity_info"
);
pin_s2c_offset!(
    CliStatus::UNITY_POINTS1_OFFSET,
    crate::s2c_layout::clistatus::STATUSDATA_UNITY_POINTS1,
    "CLISTATUS.unity_points1"
);
pin_s2c_offset!(
    CliStatus::UNITY_POINTS2_OFFSET,
    crate::s2c_layout::clistatus::STATUSDATA_UNITY_POINTS2,
    "CLISTATUS.unity_points2"
);
const _: () = {
    use crate::s2c_layout::clistatus as lsb;
    assert!(
        CliStatus::UNITY_FACTION_SHIFT == lsb::STATUSDATA_UNITY_INFO_FACTION_SHIFT
            && CliStatus::UNITY_FACTION_MASK == lsb::STATUSDATA_UNITY_INFO_FACTION_MASK,
        "LSB moved unityinfo_t.Faction: the decoder bits are stale"
    );
    assert!(
        CliStatus::UNITY_POINTS_SHIFT == lsb::STATUSDATA_UNITY_INFO_POINTS_SHIFT
            && CliStatus::UNITY_POINTS_MASK == lsb::STATUSDATA_UNITY_INFO_POINTS_MASK,
        "LSB moved unityinfo_t.Points: the decoder bits are stale"
    );
};

/// One visible skill row after the encoding is applied: a level, and whether the
/// word says it is capped at the character's ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillRow {
    pub level: u16,
    /// Bit 15 of the raw word. Retail paints such a row blue (ARGB 0x802060A0)
    /// instead of the ordinary grey — the arithmetic is `mask = 0xff9fe020` then
    /// `base + mask` over the base colour 0x80808080, which lands on exactly that
    /// value (FFXiMain.dll retail-2026-09 `.text 0x14857A..0x14858C`).
    pub capped: bool,
}

/// One visible craft row: level and rank, both decoded out of the packed word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CraftRow {
    pub level: u16,
    /// The low 5 bits — the rank index the client maps to its abbreviation.
    pub rank: u8,
    pub capped: bool,
}

/// s2c 0x062 GP_SERV_COMMAND_CLISTATUS2 — the skill block behind the Status
/// window's Combat Skill (ids 1..31), Magic Skill (32..47) and Craft Skill
/// (48..62) rows. Retail copies these 64 words straight into its character blob
/// (FFXiMain.dll retail-2026-09 skill table RVA 0x4856A4; the handler at `.text
/// 0x99400` reads them from packet +0x80) and nothing else writes that table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharSkillLevels {
    /// `CommandRecast[31]` — per-command recast timers, same order as the array
    /// LSB fills (vendor/server/src/map/packets/s2c/0x062_clistatus2.h).
    pub command_recast: [u32; Self::COMMAND_RECAST_COUNT],
    /// `skill_base[64]`, indexed by skill id. Combat ids run 1..=31, magic
    /// 32..=47 and craft 48..=62; the two families encode the word differently.
    pub skills: [u16; Self::SKILL_COUNT],
}

impl CharSkillLevels {
    // vendor/server/src/map/packets/s2c/0x062_clistatus2.h GP_SERV_COMMAND_CLISTATUS2 CommandRecast
    pub const COMMAND_RECAST_OFFSET: usize = 0x0;
    pub const COMMAND_RECAST_COUNT: usize = 31;
    // ... skill_base (packet +0x80 in retail's view).
    pub const SKILL_BASE_OFFSET: usize = 0x7C;
    pub const SKILL_COUNT: usize = 64;

    /// A combat/magic word that is *exactly* this says the character cannot have
    /// the skill at all, and the row is not drawn — a mere bit-15 set does not,
    /// which is why the check cannot be `raw & 0x8000 != 0`
    /// (FFXiMain.dll retail-2026-09 `.text 0x148564`: `and eax,0x8000`, then
    /// `test edi,0x7fff` and skip only when that is also zero).
    pub const SKILL_HIDDEN: u16 = 0x8000;
    pub const SKILL_CAP_BIT: u16 = 0x8000;
    pub const SKILL_LEVEL_MASK: u16 = 0x7FFF;

    /// Craft words hide on a different sentinel and pack level+rank in one word
    /// (FFXiMain.dll retail-2026-09 `.text 0x148930` `cmp bx,0xffff` for the skip,
    /// and the row builder at RVA 0x1488A0 for the unpacking).
    pub const CRAFT_HIDDEN: u16 = 0xFFFF;
    pub const CRAFT_LEVEL_SHIFT: u32 = 5;
    pub const CRAFT_LEVEL_MASK: u16 = 0x3FF;
    pub const CRAFT_RANK_MASK: u8 = 0x1F;

    /// Combat-skill ids shown by `btlskill`: 1..=31.
    pub const COMBAT_SKILL_RANGE: std::ops::RangeInclusive<usize> = 1..=31;
    /// Magic-skill ids shown by `mgcskill`: 32..=47.
    pub const MAGIC_SKILL_RANGE: std::ops::RangeInclusive<usize> = 32..=47;
    /// Craft-skill ids shown by `trdskill`: 48..=62.
    pub const CRAFT_SKILL_RANGE: std::ops::RangeInclusive<usize> = 48..=62;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        const MIN_LEN: usize = CharSkillLevels::SKILL_BASE_OFFSET
            + CharSkillLevels::SKILL_COUNT * std::mem::size_of::<u16>();
        if body.len() < MIN_LEN {
            return Err(DecodeError::Truncated(MIN_LEN, body.len()));
        }
        let mut command_recast = [0u32; Self::COMMAND_RECAST_COUNT];
        for (i, slot) in command_recast.iter_mut().enumerate() {
            *slot = u32::from_le_bytes(
                body[Self::COMMAND_RECAST_OFFSET + i * 4..Self::COMMAND_RECAST_OFFSET + i * 4 + 4]
                    .try_into()
                    .unwrap(),
            );
        }
        let mut skills = [0u16; Self::SKILL_COUNT];
        for (i, slot) in skills.iter_mut().enumerate() {
            *slot = u16::from_le_bytes(
                body[Self::SKILL_BASE_OFFSET + i * 2..Self::SKILL_BASE_OFFSET + i * 2 + 2]
                    .try_into()
                    .unwrap(),
            );
        }
        Ok(Self {
            command_recast,
            skills,
        })
    }

    /// The combat/magic reading of one word: `None` when the character cannot have
    /// the skill, otherwise its level and cap flag.
    pub fn noncraft_row(raw: u16) -> Option<SkillRow> {
        (raw != Self::SKILL_HIDDEN).then_some(SkillRow {
            level: raw & Self::SKILL_LEVEL_MASK,
            capped: raw & Self::SKILL_CAP_BIT != 0,
        })
    }

    /// The craft reading of one word: `None` when the row is hidden, otherwise the
    /// unpacked level/rank pair.
    pub fn craft_row(raw: u16) -> Option<CraftRow> {
        (raw != Self::CRAFT_HIDDEN).then_some(CraftRow {
            level: (raw >> Self::CRAFT_LEVEL_SHIFT) & Self::CRAFT_LEVEL_MASK,
            rank: (raw & Self::CRAFT_RANK_MASK as u16) as u8,
            capped: raw & Self::SKILL_CAP_BIT != 0,
        })
    }
}

pin_s2c_offset!(
    CharSkillLevels::COMMAND_RECAST_OFFSET,
    crate::s2c_layout::clistatus2::COMMAND_RECAST,
    "GP_SERV_COMMAND_CLISTATUS2.CommandRecast"
);
pin_s2c_offset!(
    CharSkillLevels::SKILL_BASE_OFFSET,
    crate::s2c_layout::clistatus2::SKILL_BASE,
    "GP_SERV_COMMAND_CLISTATUS2.skill_base"
);
pin_s2c_offset!(
    CliStatus::SU_LV_OFFSET,
    lsb::STATUSDATA_SU_LV,
    "CLISTATUS.su_lv"
);
pin_s2c_offset!(
    CliStatus::MASTERY_INFO_FLAGS_OFFSET,
    lsb::STATUSDATA_MASTERY_INFO_FLAGS,
    "CLISTATUS.mastery_info.flags"
);
pin_s2c_offset!(
    CliStatus::EQUIP_SLOT_BLOCK_OFFSET,
    lsb::STATUSDATA_UNITY_CHAT_COLOR_FLAG,
    "CLISTATUS.unity_chat_color_flag (the equip-block dword)"
);

/// s2c 0x01B GP_SERV_COMMAND_JOB_INFO — per-job levels + unlocked-jobs bitmask for
/// the self character. Body offsets follow the GP_MYROOM_DANCER struct in
/// vendor/server/src/map/packets/s2c/0x01b_job_info.h GP_MYROOM_DANCER (filled in 0x01b_job_info.cpp).
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
    /// MAX_JOBTYPE, vendor/server/src/map/entities/battle_entity.h (JOBTYPE 1=WAR..23=MON).
    pub const MAX_JOBTYPE: usize = 24;

    pub(crate) const MJOB_NO_OFFSET: usize = 0x04;
    pub(crate) const SJOB_NO_OFFSET: usize = 0x07;
    pub(crate) const UNLOCKED_OFFSET: usize = 0x08;
    pub(crate) const HP_MAX_OFFSET: usize = 0x38;
    pub(crate) const MP_MAX_OFFSET: usize = 0x3C;
    pub(crate) const SJOBFLG_OFFSET: usize = 0x40;
    pub(crate) const JOB_LEVELS_OFFSET: usize = 0x44;
    pub(crate) const MIN_LEN: usize = Self::JOB_LEVELS_OFFSET + Self::MAX_JOBTYPE;

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

pin_s2c_offset!(
    CliStatus::HP_MAX_OFFSET,
    lsb::STATUSDATA_HPMAX,
    "CLISTATUS.hpmax"
);
pin_s2c_offset!(
    CliStatus::MP_MAX_OFFSET,
    lsb::STATUSDATA_MPMAX,
    "CLISTATUS.mpmax"
);
pin_s2c_offset!(
    CliStatus::MJOB_NO_OFFSET,
    lsb::STATUSDATA_MJOB_NO,
    "CLISTATUS.mjob_no"
);
pin_s2c_offset!(
    CliStatus::MJOB_LV_OFFSET,
    lsb::STATUSDATA_MJOB_LV,
    "CLISTATUS.mjob_lv"
);
pin_s2c_offset!(
    CliStatus::SJOB_NO_OFFSET,
    lsb::STATUSDATA_SJOB_NO,
    "CLISTATUS.sjob_no"
);
pin_s2c_offset!(
    CliStatus::SJOB_LV_OFFSET,
    lsb::STATUSDATA_SJOB_LV,
    "CLISTATUS.sjob_lv"
);
pin_s2c_offset!(
    CliStatus::BP_BASE_OFFSET,
    lsb::STATUSDATA_BP_BASE,
    "CLISTATUS.bp_base"
);
pin_s2c_offset!(
    CliStatus::BP_COUNT,
    lsb::STATUSDATA_BP_BASE_COUNT,
    "CLISTATUS.bp_base length"
);
pin_s2c_offset!(
    CliStatus::BP_ADJ_OFFSET,
    lsb::STATUSDATA_BP_ADJ,
    "CLISTATUS.bp_adj"
);
pin_s2c_offset!(
    CliStatus::BP_COUNT,
    lsb::STATUSDATA_BP_ADJ_COUNT,
    "CLISTATUS.bp_adj length"
);
pin_s2c_offset!(
    CliStatus::ATTACK_OFFSET,
    lsb::STATUSDATA_ATK,
    "CLISTATUS.atk"
);
pin_s2c_offset!(
    CliStatus::DEFENSE_OFFSET,
    lsb::STATUSDATA_DEF,
    "CLISTATUS.def"
);
pin_s2c_offset!(
    CliStatus::DEF_ELEM_OFFSET,
    lsb::STATUSDATA_DEF_ELEM,
    "CLISTATUS.def_elem"
);
pin_s2c_offset!(
    CliStatus::DEF_ELEM_COUNT,
    lsb::STATUSDATA_DEF_ELEM_COUNT,
    "CLISTATUS.def_elem length"
);
pin_s2c_offset!(
    CliStatus::ILVL_OFFSET,
    lsb::STATUSDATA_ILVL,
    "CLISTATUS.ilvl"
);

#[cfg(test)]
mod cli_status_tests {
    use super::*;

    #[test]
    fn clistatus_reads_stat_block_offsets() {
        let mut buf = vec![0u8; CliStatus::NEEDED];
        buf[0..4].copy_from_slice(&1946u32.to_le_bytes()); // hp_max
        buf[4..8].copy_from_slice(&1295u32.to_le_bytes()); // mp_max
        buf[8] = 5; // mjob_no (RDM)
        buf[9] = 75; // mjob_lv
        buf[10] = 4; // sjob_no (BLM)
        buf[11] = 37; // sjob_lv
        buf[12..14].copy_from_slice(&806i16.to_le_bytes()); // exp_now
        buf[14..16].copy_from_slice(&1750i16.to_le_bytes()); // exp_next
        for i in 0..7 {
            buf[16 + i * 2..18 + i * 2].copy_from_slice(&((10 + i as u16) * 5).to_le_bytes());
            buf[30 + i * 2..32 + i * 2].copy_from_slice(&((i as i16 + 1) * 7).to_le_bytes());
        }
        buf[44..46].copy_from_slice(&1048u16.to_le_bytes()); // attack
        buf[46..48].copy_from_slice(&1006u16.to_le_bytes()); // defense
        buf[48..50].copy_from_slice(&(-15i16).to_le_bytes()); // fire resist
        buf[81] = 119; // ilvl
        buf[CliStatus::SU_LV_OFFSET] = 3; // su_lv (packet +0x52)
        buf[CliStatus::EQUIP_SLOT_BLOCK_OFFSET..CliStatus::EQUIP_SLOT_BLOCK_OFFSET + 4]
            .copy_from_slice(&0xDEAD_00F0u32.to_le_bytes());
        buf[CliStatus::MASTERY_INFO_FLAGS_OFFSET] = CliStatus::MASTER_LEVEL_UNLOCK_BIT;

        let cs = CliStatus::decode(&buf).expect("decodes");
        assert_eq!(cs.hp_max, 1946);
        assert_eq!(cs.mp_max, 1295);
        assert_eq!(cs.mjob_no, 5);
        assert_eq!(cs.mjob_lv, 75);
        assert_eq!(cs.sjob_no, 4);
        assert_eq!(cs.sjob_lv, 37);
        assert_eq!(cs.exp_now, 806, "EXP between levels (level box)");
        assert_eq!(cs.exp_next, 1750);
        assert_eq!(cs.bp_base[0], 50, "STR base");
        assert_eq!(cs.bp_base[6], 80, "CHR base");
        assert_eq!(cs.bp_adj[0], 7, "STR gear delta");
        assert_eq!(cs.attack, 1048);
        assert_eq!(cs.defense, 1006);
        assert_eq!(cs.def_elem[0], -15, "fire resist signed");
        assert_eq!(cs.ilvl, 119);
        assert_eq!(cs.su_lv, 3, "superior level (persona `Su` + equip gate)");
        assert_eq!(
            cs.equip_slot_block, 0xDEAD_00F0,
            "per-slot equip-block dword (fifth legality test)"
        );
        assert!(cs.master_level_unlocked, "mastery_info flags bit 0");
        assert!(
            CliStatus::decode(&buf[..CliStatus::NEEDED - 1]).is_err(),
            "truncation rejected"
        );
    }

    /// The Unity fields the `ut_point` readout and the Unity panel draw
    /// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md): Faction is
    /// the low 5 bits of `unity_info`, Points the 17 bits from bit 10.
    #[test]
    fn clistatus_reads_unity_info_bitfield_and_weekly_points() {
        const FACTION: u32 = 7;
        const RANK: u32 = 3;
        const ACCOLADES: u32 = 0x1_2345;
        const UNUSED_HIGH_BITS: u32 = 0x1F << 27;
        let word =
            FACTION | (RANK << 5) | (ACCOLADES << CliStatus::UNITY_POINTS_SHIFT) | UNUSED_HIGH_BITS;
        let mut buf = vec![0u8; CliStatus::NEEDED];
        buf[CliStatus::UNITY_INFO_OFFSET..CliStatus::UNITY_INFO_OFFSET + 4]
            .copy_from_slice(&word.to_le_bytes());
        buf[CliStatus::UNITY_POINTS1_OFFSET..CliStatus::UNITY_POINTS1_OFFSET + 2]
            .copy_from_slice(&42u16.to_le_bytes());
        buf[CliStatus::UNITY_POINTS2_OFFSET..CliStatus::UNITY_POINTS2_OFFSET + 2]
            .copy_from_slice(&17u16.to_le_bytes());
        let cs = CliStatus::decode(&buf).expect("decodes");
        assert_eq!(u32::from(cs.unity_faction), FACTION);
        assert_eq!(cs.unity_points, ACCOLADES);
        assert_eq!(cs.unity_points1, 42);
        assert_eq!(cs.unity_points2, 17);
    }
}

#[cfg(test)]
mod job_info_tests {
    use super::*;

    /// Pins JobInfo to LSB's GP_MYROOM_DANCER layout
    /// (vendor/server/src/map/packets/s2c/0x01b_job_info.h GP_MYROOM_DANCER; job_lev2, not
    /// the legacy job_lev[16] @0x0C) and MAX_JOBTYPE
    /// (vendor/server/src/map/entities/battle_entity.h MAX_JOBTYPE), since the decode
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
}

#[cfg(test)]
mod char_status_tests {
    use super::*;

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
                mount_id: 0,
                allegiance: 0,
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
    fn char_status_decodes_mount_id_and_defaults_when_truncated() {
        let mut body = vec![0u8; CharStatus::MOUNT_ID_OFFSET + 1];
        body[CharStatus::SERVER_STATUS_OFFSET] = animation::MOUNT;
        body[CharStatus::MOUNT_ID_OFFSET] = 17; // MOUNT_HIPPOGRYPH
        let cs = CharStatus::decode(&body).unwrap();
        assert_eq!(cs.mount_id, 17);
        assert!(animation::is_mounted(cs.server_status));

        // The mount byte sits past every field LSB guarantees, so a short body
        // must read 0 rather than panic.
        let short = vec![0u8; CharStatus::MIN_LEN];
        assert_eq!(CharStatus::decode(&short).unwrap().mount_id, 0);
    }

    #[test]
    fn char_status_chocobo_is_mount_id_zero_and_only_animation_tells_it_apart() {
        let mut body = vec![0u8; CharStatus::MOUNT_ID_OFFSET + 1];
        assert_eq!(CharStatus::decode(&body).unwrap().mount_id, 0);
        assert!(!animation::is_mounted(
            CharStatus::decode(&body).unwrap().server_status
        ));

        body[CharStatus::SERVER_STATUS_OFFSET] = animation::CHOCOBO;
        let cs = CharStatus::decode(&body).unwrap();
        assert_eq!(cs.mount_id, 0, "MOUNT_CHOCOBO shares the unmounted value");
        assert!(animation::is_mounted(cs.server_status));
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

    /// Pins allegiance to LSB's Flags2 word and BallistaFlg's bit position,
    /// since the decode tests build buffers through these same consts.
    #[test]
    fn char_status_allegiance_offsets_match_char_status_layout() {
        assert_eq!(CharStatus::FLAGS2_OFFSET, 0x30);
        assert_eq!(CharStatus::BALLISTA_FLG_SHIFT, 21);
    }

    /// BallistaFlg sits at bits 21..28 of Flags2, with a PetIndex (bits 3..18)
    /// that must not leak into the byte, so the fixture ORs noise into the word.
    #[test]
    fn char_status_decodes_allegiance_from_flags2_ballista_flg() {
        const PET_INDEX_NOISE: u32 = 0x1234;
        let mut body = vec![0u8; CharStatus::MIN_LEN];
        let flags2 = (5u32 << CharStatus::BALLISTA_FLG_SHIFT) | PET_INDEX_NOISE;
        body[CharStatus::FLAGS2_OFFSET..CharStatus::FLAGS2_OFFSET + 4]
            .copy_from_slice(&flags2.to_le_bytes());
        assert_eq!(CharStatus::decode(&body).unwrap().allegiance, 5);

        // The belligerence case: base allegiance OR'd with the 0x08 bit that
        // char_status.cpp sets outside the Ferretory.
        body[CharStatus::FLAGS2_OFFSET..CharStatus::FLAGS2_OFFSET + 4].copy_from_slice(
            &((9u32 << CharStatus::BALLISTA_FLG_SHIFT) | PET_INDEX_NOISE).to_le_bytes(),
        );
        assert_eq!(CharStatus::decode(&body).unwrap().allegiance, 9);
    }
}

#[cfg(test)]
mod skill_tests {
    use super::*;

    fn body_with(skills: [(usize, u16); 3], recast0: u32) -> Vec<u8> {
        let mut body =
            vec![0u8; CharSkillLevels::SKILL_BASE_OFFSET + CharSkillLevels::SKILL_COUNT * 2];
        body[0..4].copy_from_slice(&recast0.to_le_bytes());
        for (id, word) in skills {
            body[CharSkillLevels::SKILL_BASE_OFFSET + id * 2
                ..CharSkillLevels::SKILL_BASE_OFFSET + id * 2 + 2]
                .copy_from_slice(&word.to_le_bytes());
        }
        body
    }

    #[test]
    fn char_skill_levels_decodes_both_arrays() {
        const RECAST0: u32 = 4_200;
        const HAND_TO_HAND: u16 = 301;
        /// A level carried at id 32, the first id of the magic family
        /// (`mgcskill` shows ids 32..=47).
        const DIVINE_MAGIC: u16 = 300;

        // The three window families partition the array by id.
        assert_eq!(
            (
                CharSkillLevels::COMBAT_SKILL_RANGE,
                CharSkillLevels::MAGIC_SKILL_RANGE,
                CharSkillLevels::CRAFT_SKILL_RANGE,
            ),
            ((1..=31), (32..=47), (48..=62))
        );
        let body = body_with(
            [
                (1, HAND_TO_HAND),
                (*CharSkillLevels::MAGIC_SKILL_RANGE.start(), DIVINE_MAGIC),
                (48, CharSkillLevels::CRAFT_HIDDEN),
            ],
            RECAST0,
        );
        let decoded = CharSkillLevels::decode(&body).unwrap();
        assert_eq!(decoded.command_recast[0], RECAST0);
        assert_eq!(decoded.skills[1], HAND_TO_HAND);
        assert_eq!(decoded.skills[32], DIVINE_MAGIC);

        // Truncation is an error, not a silently missing array.
        let short = vec![0u8; body.len() - 1];
        assert!(CharSkillLevels::decode(&short).is_err());
    }

    /// The hidden rule is equality against the sentinel, not bit 15: a capped skill
    /// keeps its level and only changes colour (FFXiMain.dll retail-2026-09
    /// `.text 0x148564`).
    #[test]
    fn noncraft_skill_words_hide_only_at_the_exact_sentinel() {
        assert_eq!(
            CharSkillLevels::noncraft_row(CharSkillLevels::SKILL_HIDDEN),
            None
        );
        let capped = CharSkillLevels::noncraft_row(0x8000 | 125).unwrap();
        assert_eq!((capped.level, capped.capped), (125, true));
        let plain = CharSkillLevels::noncraft_row(301).unwrap();
        assert_eq!((plain.level, plain.capped), (301, false));
    }

    /// Craft words pack level and rank into one word and hide on 0xFFFF instead.
    #[test]
    fn craft_skill_words_unpack_level_and_rank() {
        assert_eq!(
            CharSkillLevels::craft_row(CharSkillLevels::CRAFT_HIDDEN),
            None
        );
        const LEVEL: u16 = 220;
        const RANK: u8 = 11;
        let raw = (LEVEL << CharSkillLevels::CRAFT_LEVEL_SHIFT) | (RANK as u16);
        let row = CharSkillLevels::craft_row(raw).unwrap();
        assert_eq!((row.level, row.rank, row.capped), (LEVEL, RANK, false));

        let capped_raw = raw | CharSkillLevels::SKILL_CAP_BIT;
        let capped = CharSkillLevels::craft_row(capped_raw).unwrap();
        assert_eq!(
            (capped.level, capped.rank, capped.capped),
            (LEVEL, RANK, true)
        );
    }

    /// `su_lv` and the master-level bit are what gate equipment legality and the
    /// Status window's Master Levels row; they must read from their own bytes.
    #[test]
    fn cli_status_reads_superior_level_and_master_unlock() {
        const SU_LV: u8 = 4;
        let mut body = vec![0u8; CliStatus::NEEDED];
        body[CliStatus::SU_LV_OFFSET] = SU_LV;
        let plain = CliStatus::decode(&body).unwrap();
        assert_eq!(plain.su_lv, SU_LV);
        assert!(!plain.master_level_unlocked);

        // Noise in the neighbouring mastery bytes must not leak into bit 0.
        body[CliStatus::MASTERY_INFO_FLAGS_OFFSET] = CliStatus::MASTER_LEVEL_UNLOCK_BIT;
        let unlocked = CliStatus::decode(&body).unwrap();
        assert!(unlocked.master_level_unlocked);
    }
}
