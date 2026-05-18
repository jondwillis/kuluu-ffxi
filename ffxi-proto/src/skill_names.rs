//! Display names for `SKILLTYPE`, the id carried in `Data` for battle
//! messages like `SkillGain` (38) and `SkillLevelUp` (53).
//!
//! Source: `vendor/server/src/map/entities/battleentity.h::SKILLTYPE`.
//! Hand-curated — short, ~60 entries — so a static slice beats a build-time
//! scrape. Update when the enum gains new entries.

const SKILL_NAMES: &[(u8, &str)] = &[
    (1, "Hand-to-Hand"),
    (2, "Dagger"),
    (3, "Sword"),
    (4, "Great Sword"),
    (5, "Axe"),
    (6, "Great Axe"),
    (7, "Scythe"),
    (8, "Polearm"),
    (9, "Katana"),
    (10, "Great Katana"),
    (11, "Club"),
    (12, "Staff"),
    (22, "Automaton Melee"),
    (23, "Automaton Ranged"),
    (24, "Automaton Magic"),
    (25, "Archery"),
    (26, "Marksmanship"),
    (27, "Throwing"),
    (28, "Guard"),
    (29, "Evasion"),
    (30, "Shield"),
    (31, "Parrying"),
    (32, "Divine Magic"),
    (33, "Healing Magic"),
    (34, "Enhancing Magic"),
    (35, "Enfeebling Magic"),
    (36, "Elemental Magic"),
    (37, "Dark Magic"),
    (38, "Summoning Magic"),
    (39, "Ninjutsu"),
    (40, "Singing"),
    (41, "String Instrument"),
    (42, "Wind Instrument"),
    (43, "Blue Magic"),
    (44, "Geomancy"),
    (45, "Handbell"),
    (48, "Fishing"),
    (49, "Woodworking"),
    (50, "Smithing"),
    (51, "Goldsmithing"),
    (52, "Clothcraft"),
    (53, "Leathercraft"),
    (54, "Bonecraft"),
    (55, "Alchemy"),
    (56, "Cooking"),
    (57, "Synergy"),
    (58, "Riding"),
    (59, "Digging"),
];

pub fn lookup(id: u8) -> Option<&'static str> {
    SKILL_NAMES.iter().find_map(|&(k, v)| (k == id).then_some(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_skills_resolve() {
        assert_eq!(lookup(1), Some("Hand-to-Hand"));
        assert_eq!(lookup(48), Some("Fishing"));
        assert_eq!(lookup(33), Some("Healing Magic"));
    }

    #[test]
    fn unused_slot_is_none() {
        // Slots 13-21 are unused per the SKILLTYPE enum.
        assert!(lookup(15).is_none());
    }
}
