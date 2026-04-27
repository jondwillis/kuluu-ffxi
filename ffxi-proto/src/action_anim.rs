include!(concat!(env!("OUT_DIR"), "/spell_animation_table.rs"));
include!(concat!(env!("OUT_DIR"), "/ability_animation_table.rs"));
include!(concat!(env!("OUT_DIR"), "/weapon_skill_animation_table.rs"));

// research/xim SpellTables.kt / AbilityTable.kt: a skill's completion animation
// is a global file-table entry at base_offset + per-skill animation index, where
// the per-skill index is the `animation` column of spell_list.sql / abilities.sql.
const SPELL_FILE_TABLE_OFFSET: u32 = 0xAF0;
const ABILITY_FILE_TABLE_OFFSET: u32 = 0x113C;
const TRUST_FILE_ID: u32 = 0xE9B;
const TRUST_SPELL_ID_MIN: u16 = 896;

fn lookup(table: &[(u16, u16)], id: u16) -> Option<u16> {
    table
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| table[i].1)
}

pub fn spell_file_id(spell_id: u32) -> Option<u32> {
    let id = u16::try_from(spell_id).ok()?;
    if id >= TRUST_SPELL_ID_MIN {
        return Some(TRUST_FILE_ID);
    }
    let index = lookup(SPELL_ANIMATION, id)?;
    Some(SPELL_FILE_TABLE_OFFSET + index as u32)
}

// The completion-effect DAT for a job ability is the file-table entry at
// ABILITY_FILE_TABLE_OFFSET + abilityId — i.e. the ability id is itself the index. LSB's
// `abilities.animation` column is the packet animation value, NOT this index: for abilities where
// the two diverge (e.g. Boost id 39 / anim 7, Mighty Strikes 16 / 33) only `+ abilityId` lands on
// the effect DAT that actually carries the local D3m billboard meshes; `+ animation` points at a
// meshless or unrelated DAT. (Verified against retail DATs: 0x113C+39 = Boost's "maz" self-buff
// aura with 4 D3m chunks; 0x113C+7 has none.)
pub fn ability_file_id(ability_id: u32) -> Option<u32> {
    let id = u16::try_from(ability_id).ok()?;
    Some(ABILITY_FILE_TABLE_OFFSET + id as u32)
}

// research/xim AbilityTable.kt:103 — weapon skills add their per-skill animation index to a
// race-dependent base read from FFXiMain.dll (see ffxi_dat::main_dll). This returns only the
// per-skill index; the caller adds the race base.
pub fn weapon_skill_animation_index(weapon_skill_id: u32) -> Option<u16> {
    let id = u16::try_from(weapon_skill_id).ok()?;
    lookup(WEAPON_SKILL_ANIMATION, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_nonempty_and_sorted() {
        assert!(SPELL_ANIMATION.len() >= 400);
        assert!(ABILITY_ANIMATION.len() >= 100);
        assert!(SPELL_ANIMATION.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(ABILITY_ANIMATION.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn cure_resolves_to_offset_plus_index() {
        let index = lookup(SPELL_ANIMATION, 1).unwrap();
        assert_eq!(
            spell_file_id(1),
            Some(SPELL_FILE_TABLE_OFFSET + index as u32)
        );
    }

    #[test]
    fn trust_spells_share_one_file() {
        assert_eq!(spell_file_id(900), Some(TRUST_FILE_ID));
    }

    #[test]
    fn out_of_range_is_none() {
        assert_eq!(spell_file_id(0xF_FFFF), None);
        assert_eq!(ability_file_id(0xF_FFFF), None);
    }

    #[test]
    fn ability_effect_file_is_offset_plus_id_not_animation_column() {
        // Boost (abilityId 39) must resolve to 0x113C+39 = 0x1163 (its "maz" aura DAT, which has
        // the local billboard meshes) — NOT 0x113C+7 from the LSB animation column.
        assert_eq!(ability_file_id(39), Some(ABILITY_FILE_TABLE_OFFSET + 39));
        assert_eq!(ability_file_id(16), Some(ABILITY_FILE_TABLE_OFFSET + 16));
    }
}
