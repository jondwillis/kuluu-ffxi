include!(concat!(env!("OUT_DIR"), "/spell_skill_table.rs"));

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

    #[test]
    fn table_is_nonempty_and_sorted() {
        assert!(SPELL_MAGIC_SKILL.len() >= 400);
        assert!(SPELL_MAGIC_SKILL.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
