use super::*;

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
    /// (research/XIClient .../Game/Net/Packets/s2c/0x037.cpp).
    pub(crate) const FLAGS2_OFFSET: usize = 0x30;
    /// `BallistaFlg`'s position inside the Flags2 word (bits 21..28),
    /// char_status.cpp `flags2_t`.
    pub(crate) const BALLISTA_FLG_SHIFT: u32 = 21;
    /// `field_57`, the byte the disassembly's own struct puts right before
    /// `Field58Flags` (research/XIClient .../Game/Net/Packets/s2c/0x037.h
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
            // flags0_t bitfield: hpp occupies bits 16..24.
            hpp: ((flags0 >> 16) & 0xFF) as u8,
            dead_counter1: rd(Self::DEAD_COUNTER1_OFFSET),
            dead_counter2: rd(Self::DEAD_COUNTER2_OFFSET),
            server_status: body[Self::SERVER_STATUS_OFFSET],
            fishing_timer: body.get(Self::FISHING_TIMER_OFFSET).copied().unwrap_or(0),
            speed: u16::from_le_bytes([body[Self::SPEED_OFFSET], body[Self::SPEED_OFFSET + 1]])
                & Self::SPEED_MASK,
            mount_id: body.get(Self::MOUNT_ID_OFFSET).copied().unwrap_or(0),
            allegiance: ((rd(Self::FLAGS2_OFFSET) >> Self::BALLISTA_FLG_SHIFT) & 0xFF) as u8,
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
const _: () = assert!(CharStatus::FLAGS2_OFFSET + 4 <= CharStatus::MIN_LEN);

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

#[cfg(test)]
mod cli_status_tests {
    use super::*;

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
}

#[cfg(test)]
mod job_info_tests {
    use super::*;

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

    #[test]
    fn char_status_decodes_allegiance_from_flags2_ballista_flg() {
        let mut body = vec![0u8; CharStatus::MIN_LEN];
        // BallistaFlg at bits 21..28, with a PetIndex (bits 3..18) that must
        // not leak into the byte.
        let flags2 = (5u32 << 21) | 0x1234;
        body[CharStatus::FLAGS2_OFFSET..CharStatus::FLAGS2_OFFSET + 4]
            .copy_from_slice(&flags2.to_le_bytes());
        assert_eq!(CharStatus::decode(&body).unwrap().allegiance, 5);

        // The belligerence case: base allegiance OR'd with the 0x08 bit that
        // char_status.cpp sets outside the Ferretory.
        body[CharStatus::FLAGS2_OFFSET..CharStatus::FLAGS2_OFFSET + 4]
            .copy_from_slice(&((9u32 << 21) | 0x1234).to_le_bytes());
        assert_eq!(CharStatus::decode(&body).unwrap().allegiance, 9);
    }
}
