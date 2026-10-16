include!(concat!(env!("OUT_DIR"), "/spell_group_table.rs"));
include!(concat!(env!("OUT_DIR"), "/spell_skill_table.rs"));

/// `vendor/server/scripts/enum/magic.lua` xi.magic.spellGroup — the values stored in
/// [`SPELL_GROUP`] (scraped from vendor/server/sql/spell_list.sql field 3).
pub const GROUP_SONG: u8 = 1;
pub const GROUP_BLACK: u8 = 2;
pub const GROUP_BLUE: u8 = 3;
pub const GROUP_NINJUTSU: u8 = 4;
pub const GROUP_SUMMONING: u8 = 5;
pub const GROUP_WHITE: u8 = 6;
pub const GROUP_GEOMANCY: u8 = 7;
pub const GROUP_TRUST: u8 = 8;

/// The spell's school per the LSB server data, on xi.magic.spellGroup numbering.
pub fn group(spell_id: u32) -> Option<u8> {
    let id = u16::try_from(spell_id).ok()?;
    SPELL_GROUP
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| SPELL_GROUP[i].1)
}

const SKILL_DIVINE: u8 = 32;
const SKILL_HEALING: u8 = 33;
const SKILL_ENHANCING: u8 = 34;
const SKILL_ENFEEBLING: u8 = 35;
const SKILL_ELEMENTAL: u8 = 36;
const SKILL_DARK: u8 = 37;
const SKILL_SUMMONING: u8 = 38;
const SKILL_NINJUTSU: u8 = 39;
const SKILL_SINGING: u8 = 40;
const SKILL_BLUE: u8 = 43;
const SKILL_GEOMANCY: u8 = 44;

fn skill_to_suffix(skill: u8) -> Option<&'static str> {
    Some(match skill {
        SKILL_DIVINE | SKILL_HEALING | SKILL_ENHANCING | SKILL_ENFEEBLING => "wh",
        SKILL_ELEMENTAL | SKILL_DARK => "bk",
        SKILL_SUMMONING => "sm",
        SKILL_NINJUTSU => "nj",
        SKILL_SINGING => "so",
        SKILL_BLUE => "bl",
        SKILL_GEOMANCY => "ge",
        _ => return None,
    })
}

pub fn cast_suffix(spell_id: u32) -> Option<&'static str> {
    let id = u16::try_from(spell_id).ok()?;
    let i = SPELL_MAGIC_SKILL
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()?;
    skill_to_suffix(SPELL_MAGIC_SKILL[i].1)
}

// vendor/server/src/map/ai/states/magic_state.cpp CMagicState::CMagicState packs the spell's FourCC — not its id —
// into BATTLE2 cmd_arg for ActionCategory::MagicStart; the spell id rides in the first result's
// `param` (:109). vendor/server/src/map/action/interrupts.cpp MagicInterrupt reuses the SAME category
// for an interrupt, distinguished only by the FourCC. vendor/server/src/map/enums/four_cc.h FourCC BlackMagicCast
// names both families: casts are "ca"+suffix, interrupts "sp"+suffix.
const MAGIC_CAST_PREFIX: [u8; 2] = *b"ca";
const MAGIC_INTERRUPT_PREFIX: [u8; 2] = *b"sp";

// vendor/server/src/map/enums/action/category.h ActionCategory ItemFinish — `action.cmd_no`, 4 bits.
pub const CATEGORY_MAGIC_FINISH: u8 = 4;
pub const CATEGORY_MAGIC_START: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MagicRoutine {
    pub id: [u8; 4],
    pub interrupt: bool,
}

pub fn magic_start_routine(cmd_arg: u32) -> Option<MagicRoutine> {
    let id = cmd_arg.to_le_bytes();
    let interrupt = match [id[0], id[1]] {
        MAGIC_CAST_PREFIX => false,
        MAGIC_INTERRUPT_PREFIX => true,
        _ => return None,
    };
    id[2..]
        .iter()
        .all(|b| b.is_ascii_lowercase())
        .then_some(MagicRoutine { id, interrupt })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_spells_map_to_schools() {
        assert_eq!(cast_suffix(1), Some("wh"));

        assert_eq!(cast_suffix(144), Some("bk"));
    }

    #[test]
    fn unknown_spell_is_none() {
        assert_eq!(cast_suffix(0xFFFF), None);
    }

    // xi.magic.spellGroup (vendor/server/scripts/enum/magic.lua) via the scraped `group` column.
    #[test]
    fn spell_groups_follow_the_lsb_enum() {
        assert_eq!(group(1), Some(GROUP_WHITE)); // cure
        assert_eq!(group(144), Some(GROUP_BLACK)); // fire
        assert_eq!(group(368), Some(GROUP_SONG)); // foe requiem
        assert_eq!(group(320), Some(GROUP_NINJUTSU)); // katon ichi
        assert_eq!(group(288), Some(GROUP_SUMMONING)); // fire spirit
        assert_eq!(group(603), Some(GROUP_BLUE)); // wild oats
        assert_eq!(group(768), Some(GROUP_GEOMANCY)); // indi-regen
        assert_eq!(group(896), Some(GROUP_TRUST)); // shantotto
    }

    #[test]
    fn unknown_spell_group_is_none() {
        assert_eq!(group(0xFFFF), None);
    }

    #[test]
    fn group_table_is_nonempty_and_sorted() {
        assert!(SPELL_GROUP.len() >= 891);
        assert!(SPELL_GROUP.windows(2).all(|w| w[0].0 < w[1].0));
    }

    // vendor/server/src/map/enums/four_cc.h FourCC BlueMagicCast — the literal constants LSB sends.
    #[test]
    fn magic_start_fourcc_decodes_to_its_routine_dat_id() {
        let black = magic_start_routine(0x6B626163).unwrap();
        assert_eq!(&black.id, b"cabk");
        assert!(!black.interrupt);

        assert_eq!(&magic_start_routine(0x68776163).unwrap().id, b"cawh");
        assert_eq!(&magic_start_routine(0x6D736163).unwrap().id, b"casm");

        let interrupted = magic_start_routine(0x6B627073).unwrap();
        assert_eq!(&interrupted.id, b"spbk");
        assert!(interrupted.interrupt);

        assert!(magic_start_routine(0x68777073).unwrap().interrupt);
    }

    // A spell id in cmd_arg would decode as garbage; reject it so the caller can tell the
    // FourCC convention from a stale/other-category payload instead of resolving a junk DatId.
    #[test]
    fn non_magic_fourcc_is_rejected() {
        assert_eq!(magic_start_routine(220), None);
        assert_eq!(magic_start_routine(0x306B7461), None);
        assert_eq!(magic_start_routine(0), None);
    }

    #[test]
    fn table_is_nonempty_and_sorted() {
        assert!(SPELL_MAGIC_SKILL.len() >= 400);
        assert!(SPELL_MAGIC_SKILL.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
