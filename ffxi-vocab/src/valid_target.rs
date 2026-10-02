include!(concat!(env!("OUT_DIR"), "/ability_valid_target_table.rs"));
include!(concat!(env!("OUT_DIR"), "/spell_valid_target_table.rs"));

// vendor/server/src/map/entities/battle_entity.h — TARGETTYPE bitmask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetFlags(pub u16);

impl TargetFlags {
    pub const SELF: u16 = 0x01;
    pub const PLAYER_PARTY: u16 = 0x02;
    pub const ENEMY: u16 = 0x04;
    pub const PLAYER_ALLIANCE: u16 = 0x08;
    pub const PLAYER: u16 = 0x10;
    pub const PLAYER_DEAD: u16 = 0x20;
    pub const NPC: u16 = 0x40;
    pub const PET: u16 = 0x0100;

    pub fn contains(self, flag: u16) -> bool {
        self.0 & flag != 0
    }

    pub fn can_target_enemy(self) -> bool {
        self.contains(Self::ENEMY)
    }

    pub fn is_self_only(self) -> bool {
        self.contains(Self::SELF) && !self.contains(Self::ENEMY) && !self.contains(Self::NPC)
    }
}

fn lookup(table: &[(u16, u16)], id: u16) -> Option<TargetFlags> {
    table
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| TargetFlags(table[i].1))
}

pub fn ability(id: u16) -> Option<TargetFlags> {
    lookup(ABILITY_VALID_TARGET, id)
}

pub fn spell(id: u16) -> Option<TargetFlags> {
    lookup(SPELL_VALID_TARGET, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mighty_strikes_is_self_only() {
        let flags = ability(16).expect("Mighty Strikes present");
        assert!(flags.is_self_only());
        assert!(!flags.can_target_enemy());
    }

    #[test]
    fn provoke_targets_enemy_not_self() {
        let flags = ability(35).expect("Provoke present");
        assert!(flags.can_target_enemy());
        assert!(!flags.is_self_only());
    }

    #[test]
    fn cure_targets_party_and_undead_not_self_only() {
        let flags = spell(1).expect("Cure present");
        assert!(flags.contains(TargetFlags::PLAYER_PARTY));
        assert!(!flags.is_self_only());
    }

    #[test]
    fn cure_valid_target_is_the_six_base_flags() {
        // vendor/server/sql/spell_list.sql spell 1: ENEMY included, so mobs
        // are valid Cure targets — no special-casing list.
        assert_eq!(
            spell(1).expect("Cure present"),
            TargetFlags(
                TargetFlags::SELF
                    | TargetFlags::PLAYER_PARTY
                    | TargetFlags::ENEMY
                    | TargetFlags::PLAYER_ALLIANCE
                    | TargetFlags::PLAYER
                    | TargetFlags::NPC
            )
        );
    }

    #[test]
    fn pet_command_abilities_carry_the_pet_bit() {
        // Sic (72) is a pet command: the server redirects it at the caster's
        // own pet (vendor/server/src/map/ai/helpers/targetfind.cpp
        // CTargetFind::getValidTarget TARGET_PET).
        let flags = ability(72).expect("Sic present");
        assert!(flags.contains(TargetFlags::PET));
        // Blood Rage (267) is self-only: its message1 value (319) carries the
        // PET bit, so a scrape drifting onto the wrong column would light it —
        // pin the column mapping against re-scrape drift
        // (vendor/server/sql/abilities.sql).
        assert!(!ability(267)
            .expect("Blood Rage present")
            .contains(TargetFlags::PET));
        assert!(!spell(1).expect("Cure present").contains(TargetFlags::PET));
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(ability(0xFFFF).is_none());
        assert!(spell(0xFFFF).is_none());
    }
}
