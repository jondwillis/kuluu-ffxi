//! Sub-target confirm step (retail's flashing "on whom?" cursor).
//!
//! Pure candidate-selection logic; the client owns entity gathering, key
//! handling, and firing the action. Semantics captured from retail
//! observation (task #3): choosing a spell/ability/WS/item from a menu does
//! NOT cast immediately — a flashing sub-target cursor appears over an
//! initial candidate, Tab/arrows cycle valid targets, Enter confirms, Esc
//! returns to the menu with cursor preserved.

use crate::input_mode::SubTargetAction;
use ffxi_proto::valid_target::TargetFlags;

/// Snapshot of one targetable entity, gathered by the client per frame.
#[derive(Debug, Clone, Copy)]
pub struct SubTargetEntity {
    pub id: u32,
    pub is_self: bool,
    pub is_pc: bool,
    pub is_party: bool,
    pub is_alliance: bool,
    pub is_enemy: bool,
    pub is_npc: bool,
    pub is_dead: bool,
    /// Squared distance from the player, used for initial pick + cycling order.
    pub dist_sq: f32,
}

/// Retail cap: sub-target cursor only considers entities within ~50 yalms.
pub const SUB_TARGET_RANGE: f32 = 50.0;

/// Does `flags` permit targeting `e`? Mirrors LSB TARGETTYPE checks
/// (vendor/server/src/map/entities/battleentity.h semantics).
pub fn entity_valid(flags: TargetFlags, e: &SubTargetEntity) -> bool {
    if e.dist_sq > SUB_TARGET_RANGE * SUB_TARGET_RANGE {
        return false;
    }
    if e.is_dead {
        // Only PLAYER_DEAD reaches corpses (Raise etc.).
        return e.is_pc && flags.contains(TargetFlags::PLAYER_DEAD);
    }
    if e.is_self {
        return flags.contains(TargetFlags::SELF)
            || flags.contains(TargetFlags::PLAYER)
            || flags.contains(TargetFlags::PLAYER_PARTY)
            || flags.contains(TargetFlags::PLAYER_ALLIANCE);
    }
    if e.is_enemy {
        return flags.contains(TargetFlags::ENEMY);
    }
    if e.is_pc {
        return flags.contains(TargetFlags::PLAYER)
            || (e.is_party && flags.contains(TargetFlags::PLAYER_PARTY))
            || (e.is_alliance && flags.contains(TargetFlags::PLAYER_ALLIANCE));
    }
    if e.is_npc {
        return flags.contains(TargetFlags::NPC);
    }
    false
}

/// Initial cursor position, matching retail order of preference:
/// 1. the currently locked/selected target, if still valid for this action;
/// 2. self, if SELF is a valid type (cures default to self);
/// 3. nearest valid entity.
pub fn initial_candidate(
    flags: TargetFlags,
    current_target: Option<u32>,
    entities: &[SubTargetEntity],
) -> Option<u32> {
    if let Some(cur) = current_target {
        if entities
            .iter()
            .any(|e| e.id == cur && entity_valid(flags, e))
        {
            return Some(cur);
        }
    }
    if flags.contains(TargetFlags::SELF) {
        if let Some(me) = entities.iter().find(|e| e.is_self) {
            if entity_valid(flags, me) {
                return Some(me.id);
            }
        }
    }
    entities
        .iter()
        .filter(|e| entity_valid(flags, e))
        .min_by(|a, b| a.dist_sq.total_cmp(&b.dist_sq))
        .map(|e| e.id)
}

/// Next/previous candidate in distance order (Tab / Shift-Tab, arrow keys).
/// Wraps around; returns `from` unchanged if it is the only valid entity.
pub fn cycle_candidate(
    flags: TargetFlags,
    from: Option<u32>,
    entities: &[SubTargetEntity],
    reverse: bool,
) -> Option<u32> {
    let mut valid: Vec<&SubTargetEntity> =
        entities.iter().filter(|e| entity_valid(flags, e)).collect();
    if valid.is_empty() {
        return None;
    }
    valid.sort_by(|a, b| a.dist_sq.total_cmp(&b.dist_sq).then(a.id.cmp(&b.id)));
    let cur_idx = from.and_then(|id| valid.iter().position(|e| e.id == id));
    let n = valid.len();
    let next = match cur_idx {
        Some(i) if reverse => (i + n - 1) % n,
        Some(i) => (i + 1) % n,
        None => 0,
    };
    Some(valid[next].id)
}

/// TARGETTYPE bitmask for the pending action. Weapon skills are always
/// enemy-targeted (LSB has no WS valid-target table); items fall back to
/// SELF when unknown, matching retail's default for usable items.
pub fn action_flags(action: SubTargetAction) -> TargetFlags {
    match action {
        SubTargetAction::Spell(id) => {
            ffxi_proto::valid_target::spell(id).unwrap_or(TargetFlags(TargetFlags::SELF))
        }
        SubTargetAction::Ability(id) => {
            ffxi_proto::valid_target::ability(id).unwrap_or(TargetFlags(TargetFlags::SELF))
        }
        SubTargetAction::WeaponSkill(_) | SubTargetAction::Ranged => {
            TargetFlags(TargetFlags::ENEMY)
        }
        SubTargetAction::Item { .. } => TargetFlags(TargetFlags::SELF),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(id: u32, dist: f32) -> SubTargetEntity {
        SubTargetEntity {
            id,
            is_self: false,
            is_pc: false,
            is_party: false,
            is_alliance: false,
            is_enemy: false,
            is_npc: false,
            is_dead: false,
            dist_sq: dist * dist,
        }
    }

    fn me() -> SubTargetEntity {
        SubTargetEntity {
            id: 1,
            is_self: true,
            is_pc: true,
            is_party: true,
            is_alliance: true,
            ..ent(1, 0.0)
        }
    }

    fn mob(id: u32, dist: f32) -> SubTargetEntity {
        SubTargetEntity {
            is_enemy: true,
            ..ent(id, dist)
        }
    }

    #[test]
    fn cure_defaults_to_self_without_target() {
        let flags = TargetFlags(TargetFlags::SELF | TargetFlags::PLAYER_PARTY);
        let ents = [me(), mob(10, 5.0)];
        assert_eq!(initial_candidate(flags, None, &ents), Some(1));
    }

    #[test]
    fn enemy_action_keeps_current_target() {
        let flags = TargetFlags(TargetFlags::ENEMY);
        let ents = [me(), mob(10, 20.0), mob(11, 5.0)];
        assert_eq!(initial_candidate(flags, Some(10), &ents), Some(10));
    }

    #[test]
    fn enemy_action_falls_back_to_nearest_mob() {
        let flags = TargetFlags(TargetFlags::ENEMY);
        let ents = [me(), mob(10, 20.0), mob(11, 5.0)];
        assert_eq!(initial_candidate(flags, None, &ents), Some(11));
        // Self is never a candidate for enemy-only actions.
        assert!(!entity_valid(flags, &me()));
    }

    #[test]
    fn out_of_range_mob_is_invalid() {
        let flags = TargetFlags(TargetFlags::ENEMY);
        assert!(!entity_valid(flags, &mob(10, 51.0)));
        assert!(entity_valid(flags, &mob(10, 49.0)));
    }

    #[test]
    fn dead_pc_only_valid_for_raise_like_flags() {
        let corpse = SubTargetEntity {
            is_pc: true,
            is_dead: true,
            ..ent(7, 3.0)
        };
        assert!(entity_valid(TargetFlags(TargetFlags::PLAYER_DEAD), &corpse));
        assert!(!entity_valid(
            TargetFlags(TargetFlags::PLAYER_PARTY),
            &corpse
        ));
    }

    #[test]
    fn cycle_wraps_in_distance_order() {
        let flags = TargetFlags(TargetFlags::ENEMY);
        let ents = [me(), mob(10, 20.0), mob(11, 5.0), mob(12, 30.0)];
        // order: 11, 10, 12
        assert_eq!(cycle_candidate(flags, Some(11), &ents, false), Some(10));
        assert_eq!(cycle_candidate(flags, Some(12), &ents, false), Some(11));
        assert_eq!(cycle_candidate(flags, Some(11), &ents, true), Some(12));
        assert_eq!(cycle_candidate(flags, None, &ents, false), Some(11));
    }

    #[test]
    fn cycle_with_no_valid_entities_is_none() {
        let flags = TargetFlags(TargetFlags::ENEMY);
        let ents = [me()];
        assert_eq!(cycle_candidate(flags, None, &ents, false), None);
    }

    #[test]
    fn ws_is_enemy_only_and_unknown_item_self_only() {
        assert!(action_flags(SubTargetAction::WeaponSkill(1)).can_target_enemy());
        assert!(action_flags(SubTargetAction::Item {
            container: 0,
            index: 0,
            item_no: 4096
        })
        .is_self_only());
    }
}
