//! The config-flag pair: s2c 0x0B4 GP_SERV_COMMAND_CONFIG and c2s 0x0DC
//! GP_CLI_COMMAND_CONFIG. Both carry the same `SAVE_CONF` bitfield prefix — the
//! server's copy of the client's option flags (vault main_menu.md §35/§42).
//! LSB layout authorities: vendor/server/src/common/mmo.h `SAVE_CONF`,
//! vendor/server/src/map/packets/s2c/0x0b4_config.h, and
//! vendor/server/src/map/packets/c2s/0x0dc_config.{h,cpp}.

/// The four `SAVE_CONF` bitfield bytes (MSVC little-endian bitfields: the first
/// declared bits occupy the low bits of each byte). Bits beyond the ones named
/// here are carried as-is by callers that re-send untouched state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerConfigFlags {
    /// `InviteFlg` — character is looking for a party (byte 0 bit 0).
    pub invite: bool,
    /// `AwayFlg` — /away (byte 0 bit 1).
    pub away: bool,
    /// `AnonymityFlg` — /anon; name and level hidden from others (byte 0 bit 2).
    pub anonymity: bool,
    /// `Language` — the client's language id, bits 3-4. LSB never re-reads it
    /// off CONFIG (a sibling packet owns it), but it rides every save.
    pub language: u8,
    /// `SysMesFilterLevel` — system-message filter level (byte 1 bits 3-4).
    pub sysmes_filter_level: u8,
    /// `AutoTargetOffFlg` — /autotarget toggles this OFF-flag (byte 1 bit 6).
    pub auto_target_off: bool,
    /// `AutoPartyFlg` — /autogroup (byte 1 bit 7).
    pub auto_party: bool,
    /// `MentorUnlockedFlg` — read-only unlock state from the server's aman()
    /// record; a client write is ignored byte-identically by LSB. Not part of
    /// c2s GP_CLI_COMMAND_CONFIG (it only appears in s2c SAVE_CONF) [V].
    pub mentor_unlocked: bool,
    /// `MentorFlg` — /mentor; the server applies it only while unlocked
    /// (`pChar->aman().hasMentorUnlocked()` in c2s/0x0dc_config.cpp).
    pub mentor: bool,
    /// `NewAdventurerOffFlg` — drop the red question mark; LSB law: "Can only
    /// turn it off, not on." (c2s/0x0dc_config.cpp).
    pub new_adventurer_off: bool,
    /// `DisplayHeadOffFlg` — /displayhead; toggling re-blinks head gear via a
    /// GRAP_LIST push when something is equipped there.
    pub display_head_off: bool,
    /// `RecruitFlg` — /rec (byte 3 bit 5).
    pub recruit: bool,
}

/// SAVE_CONF's named prefix is four bytes; the packet carries more behind it
/// (filters, PvpFlg, AreaCode), which kuluu ignores until those rows land.
const CONF_PREFIX_LEN: usize = 4;

/// Parse the `SAVE_CONF` prefix from an s2c 0x0B4 body. The packet's PacketData
/// begins at this struct (`ConfData` is the first field).
pub fn parse_config_flags(body: &[u8]) -> Option<ServerConfigFlags> {
    let b: &[u8; CONF_PREFIX_LEN] = body.get(..CONF_PREFIX_LEN)?.try_into().ok()?;
    Some(ServerConfigFlags {
        invite: b[0] & 0x01 != 0,
        away: b[0] & 0x02 != 0,
        anonymity: b[0] & 0x04 != 0,
        language: (b[0] >> 3) & 0x3,
        sysmes_filter_level: (b[1] >> 3) & 0x3,
        auto_target_off: b[1] & 0x40 != 0,
        auto_party: b[1] & 0x80 != 0,
        mentor_unlocked: b[3] & 0x01 != 0,
        mentor: b[3] & 0x02 != 0,
        new_adventurer_off: b[3] & 0x04 != 0,
        display_head_off: b[3] & 0x08 != 0,
        recruit: b[3] & 0x20 != 0,
    })
}

/// One writable config flag of the c2s GP_CLI_COMMAND_CONFIG body. `SetFlg` then
/// says On/Off for exactly this bit; LSB's setter guards every flag with
/// `if (this->X && ...)` so untouched bits may stay zero ([V] 0x0dc_config.cpp).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ConfigBit {
    Invite,
    Away,
    Anonymity,
    AutoTargetOff,
    AutoParty,
    Mentor,
    NewAdventurerOff,
    DisplayHeadOff,
    Recruit,
}

impl ConfigBit {
    /// (byte index within the four prefix bytes, bit mask) — mirrors mmo.h.
    fn byte_and_mask(self) -> (usize, u8) {
        match self {
            Self::Invite => (0, 0x01),
            Self::Away => (0, 0x02),
            Self::Anonymity => (0, 0x04),
            Self::AutoTargetOff => (1, 0x40),
            Self::AutoParty => (1, 0x80),
            Self::Mentor => (3, 0x02),
            Self::NewAdventurerOff => (3, 0x04),
            Self::DisplayHeadOff => (3, 0x08),
            Self::Recruit => (3, 0x20),
        }
    }

    /// The flag's current value inside a folded `ServerConfigFlags`.
    pub fn get(self, flags: ServerConfigFlags) -> bool {
        match self {
            Self::Invite => flags.invite,
            Self::Away => flags.away,
            Self::Anonymity => flags.anonymity,
            Self::AutoTargetOff => flags.auto_target_off,
            Self::AutoParty => flags.auto_party,
            Self::Mentor => flags.mentor,
            Self::NewAdventurerOff => flags.new_adventurer_off,
            Self::DisplayHeadOff => flags.display_head_off,
            Self::Recruit => flags.recruit,
        }
    }
}

/// `GP_CLI_COMMAND_CONFIG_SETFLG` (c2s/0x0dc_config.h): the packet's SetFlg byte.
pub const SETFLG_ON: u8 = 1;
pub const SETFLG_OFF: u8 = 2;

/// Full c2s GP_CLI_COMMAND_CONFIG body: four bitfield mask bytes, two reserved
/// uint32s, `SetFlg`, three padding bytes (`uint8_t padding00[3]` — the packet
/// grew past its PS2 shape) → 16 bytes total. Only the named flag's bit is set
/// in the mask; LSB applies only guarded bits [V].
pub fn build_config_change(flag: ConfigBit, on: bool) -> Vec<u8> {
    let mut body = vec![0u8; 16];
    let (idx, mask) = flag.byte_and_mask();
    body[idx] |= mask;
    // unused00 at [4..8], unused01 at [8..12]: zero. SetFlg at [12].
    body[12] = if on { SETFLG_ON } else { SETFLG_OFF };
    body
}

/// Wire command of the `conf12wi` store-0xBF pair announce: the send wrapper
/// FFXiMain.dll retail-2026-09 RVA 0xFC4D0 receives this literal pushed at .text
/// 0xE5548 and is called at .text 0xE555C (bead kuluu-o9qt7.20.1.4; vault
/// main_menu.md §59 addendum 16). **vendor/server has no handler for c2s 0x088** —
/// its packets/c2s/ directory carries no file for the id, so LSB drops the frame.
pub const C2S_CONF_PAIR_ANNOUNCE_ID: u16 = 0x088;

/// The announce body as the writer FFXiMain.dll retail-2026-09 RVA 0xE5520 lays
/// it out over buffer bytes +4..+7: byte0 from `mov byte ptr [eax + 4], 0`
/// (.text 0xE5553), byte1 the mode handed in as its first argument (`[eax + 5]`,
/// .text 0xE553E), and byte2 bit 0 from the writer's second argument — which
/// every `conf12wi` row site passes as zero (the arms at .text FFXiMain.dll
/// retail-2026-09 RVA 0x19BA43/0x19BA51/0x19BA5D all push it so), and the
/// character-flag re-sharer wrapper at .text 0x952D1 likewise passes zero. The
/// writer draws its buffer from a per-command template cache (FC3C0 call with
/// literal `push 0x53` at .text 0xE5524/0xE5526); the body is padded to a dword
/// and no LSB-side consumer exists for those bits.
pub fn build_conf_pair_announce(mode: u8) -> Vec<u8> {
    vec![0x00, mode, 0x00, 0x00]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_save_conf_prefix_bit_law() {
        // byte0: invite|anonymity set, language=1 (bit3) -> 0x09 | 0x08 = 0x0D? no:
        // invite(0x01)+anon(0x04)+language bit3 set (=0x08) => 0x0D. byte1 filter
        // level 1 at bits3-4 (0x08) + autotarget-off (0x40). byte2 opaque.
        // byte3 mentor-unlocked(0x01)+mentor(0x02)+recruit(0x20).
        let f = parse_config_flags(&[0x0D, 0x48, 0xA5, 0x23]).unwrap();
        assert!(f.invite && !f.away && f.anonymity);
        assert_eq!(f.language, 1);
        assert_eq!(f.sysmes_filter_level, 1);
        assert!(f.auto_target_off && !f.auto_party);
        assert!(f.mentor_unlocked && f.mentor && !f.new_adventurer_off && f.recruit);
    }

    #[test]
    fn rejects_short_bodies() {
        assert!(parse_config_flags(&[0x00; 3]).is_none());
    }

    #[test]
    fn change_frames_mask_exactly_one_bit_and_setflg() {
        let on = build_config_change(ConfigBit::AutoTargetOff, true);
        assert_eq!(on.len(), 16);
        assert_eq!((on[0], on[1], on[2], on[3]), (0x00, 0x40, 0x00, 0x00));
        assert_eq!(&on[4..12], &[0u8; 8]);
        assert_eq!(on[12], SETFLG_ON);

        let off = build_config_change(ConfigBit::Recruit, false);
        assert_eq!((off[0], off[1], off[2], off[3]), (0x00, 0x00, 0x00, 0x20));
        assert_eq!(off[12], SETFLG_OFF);
    }

    #[test]
    fn accessor_matches_bit_law() {
        let f = parse_config_flags(&[0x02, 0x80, 0, 0x04]).unwrap();
        assert!(ConfigBit::Away.get(f));
        assert!(ConfigBit::AutoParty.get(f));
        assert!(ConfigBit::NewAdventurerOff.get(f));
        assert!(!ConfigBit::Invite.get(f));
    }

    #[test]
    fn bf_pair_announce_body_pins_the_writer_bytes() {
        // Buffer writes [eax+4]=0, [eax+5]=mode, bit 0 of [eax+6] ← arg2 (zero at
        // every conf site) — FFXiMain.dll retail-2026-09 RVA 0xE553E–0xE5559.
        for mode in [0x00u8, 0x01, 0x04] {
            let body = build_conf_pair_announce(mode);
            assert_eq!(body.as_slice(), &[0x00, mode, 0x00, 0x00]);
        }
    }
}
