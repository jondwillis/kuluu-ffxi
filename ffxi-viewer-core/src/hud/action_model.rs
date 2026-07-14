use super::overlay::ClientOverlay;

pub const NPC_INTERACT_YALMS: f32 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetActionId {
    Attack,
    SwitchTarget,
    Chat,
    Magic,
    Abilities,
    Trust,
    Items,
    Trade,
    Disengage,
    Check,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetKindLite {
    SelfPc,

    Pc,

    Npc,

    Mob,

    Door,

    #[default]
    None,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TargetActionContext {
    pub has_target: bool,
    pub target_kind: TargetKindLite,

    pub in_range: bool,

    pub trusts_available: bool,

    pub engaged: bool,

    /// Whether any item currently passes the LSB 0x037 use gate
    /// (`hud::menu::any_usable_item`); when false the "Items" entry is
    /// greyed out (kuluu-268h).
    pub usable_items_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionEntryKind {
    Plain,

    Select {
        modes: Vec<&'static str>,
        mode_idx: usize,
    },
}

#[derive(Debug, Clone)]
pub struct ActionEntry {
    pub id: TargetActionId,
    pub label: String,
    pub kind: ActionEntryKind,
    pub enabled: bool,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbilityGroup {
    JobAbilities,
    WeaponSkill,
    RangedAttack,
    Mount,
    PetCommand,
}

impl AbilityGroup {
    pub const ALL: [AbilityGroup; 5] = [
        AbilityGroup::JobAbilities,
        AbilityGroup::WeaponSkill,
        AbilityGroup::RangedAttack,
        AbilityGroup::Mount,
        AbilityGroup::PetCommand,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AbilityGroup::JobAbilities => "Job Abilities",
            AbilityGroup::WeaponSkill => "Weapon Skill",
            AbilityGroup::RangedAttack => "Ranged Attack",
            AbilityGroup::Mount => "Mount",
            AbilityGroup::PetCommand => "Pet Commands",
        }
    }
}

fn applies_to(id: TargetActionId, kind: TargetKindLite, engaged: bool) -> bool {
    use TargetKindLite::*;
    match id {
        TargetActionId::Attack => matches!(kind, Mob) && !engaged,
        TargetActionId::SwitchTarget | TargetActionId::Disengage => matches!(kind, Mob) && engaged,

        TargetActionId::Magic | TargetActionId::Abilities => {
            matches!(kind, None | Mob | Pc | SelfPc)
        }

        TargetActionId::Check => matches!(kind, Mob | Pc | SelfPc),

        TargetActionId::Chat => matches!(kind, Pc | SelfPc),

        TargetActionId::Trade => matches!(kind, Pc),

        TargetActionId::Items => matches!(kind, None | Mob | Pc | SelfPc),

        TargetActionId::Trust => matches!(kind, None | Pc | SelfPc),

        TargetActionId::Open => matches!(kind, Door),
    }
}

pub fn build_target_action_entries(
    ctx: &TargetActionContext,
    _overlay: &ClientOverlay,
) -> Vec<ActionEntry> {
    const ORDER: &[TargetActionId] = &[
        TargetActionId::SwitchTarget,
        TargetActionId::Attack,
        TargetActionId::Open,
        TargetActionId::Chat,
        TargetActionId::Magic,
        TargetActionId::Abilities,
        TargetActionId::Trust,
        TargetActionId::Items,
        TargetActionId::Trade,
        TargetActionId::Disengage,
        TargetActionId::Check,
    ];

    let mut out = Vec::new();
    for &id in ORDER {
        if id == TargetActionId::Trust && !ctx.trusts_available {
            continue;
        }
        if !applies_to(id, ctx.target_kind, ctx.engaged) {
            continue;
        }

        let needs_range = matches!(
            id,
            TargetActionId::Chat | TargetActionId::Trade | TargetActionId::Open
        );
        let out_of_range = needs_range && ctx.has_target && !ctx.in_range;
        // Retail greys the Command Menu "Items" entry when nothing in the
        // bags would pass the 0x037 use gate (kuluu-268h).
        let no_usable_items = id == TargetActionId::Items && !ctx.usable_items_available;

        let (kind, label) = match id {
            TargetActionId::Attack => (ActionEntryKind::Plain, "Attack".to_string()),
            TargetActionId::SwitchTarget => (ActionEntryKind::Plain, "Switch Target".to_string()),
            TargetActionId::Disengage => (ActionEntryKind::Plain, "Disengage".to_string()),
            TargetActionId::Chat => (
                ActionEntryKind::Select {
                    modes: vec!["Say", "Tell", "Party", "Linkshell", "Unity", "Shout"],
                    mode_idx: 0,
                },
                "Chat".to_string(),
            ),
            TargetActionId::Magic => (
                ActionEntryKind::Select {
                    modes: vec!["Category", "Flat"],
                    mode_idx: 0,
                },
                "Magic".to_string(),
            ),
            TargetActionId::Abilities => (
                ActionEntryKind::Select {
                    modes: AbilityGroup::ALL.iter().map(|g| g.label()).collect(),
                    mode_idx: 0,
                },
                "Abilities".to_string(),
            ),
            TargetActionId::Trust => (ActionEntryKind::Plain, "Trust".to_string()),
            TargetActionId::Items => (ActionEntryKind::Plain, "Items".to_string()),
            TargetActionId::Trade => (ActionEntryKind::Plain, "Trade".to_string()),
            TargetActionId::Check => (ActionEntryKind::Plain, "Check".to_string()),
            TargetActionId::Open => (ActionEntryKind::Plain, "Open".to_string()),
        };

        let hint = if out_of_range {
            Some("Target out of range.".to_string())
        } else if no_usable_items {
            Some("No usable items.".to_string())
        } else {
            None
        };

        out.push(ActionEntry {
            id,
            label,
            kind,
            enabled: !out_of_range && !no_usable_items,
            hint,
        });
    }
    out
}

pub fn context_for_target(
    target_id: Option<u32>,
    entities: &[ffxi_viewer_wire::Entity],
    self_pos: ffxi_viewer_wire::Vec3,
    self_id: Option<u32>,
    engaged: bool,
    usable_items_available: bool,
) -> TargetActionContext {
    use ffxi_viewer_wire::EntityKind;

    let ent = target_id.and_then(|id| entities.iter().find(|e| e.id == id));
    let (target_kind, in_range) = match ent {
        Some(e) => {
            let kind = if matches!(e.look, Some(ffxi_viewer_wire::EntityLook::Door { .. })) {
                TargetKindLite::Door
            } else {
                match e.kind {
                    EntityKind::Pc if Some(e.id) == self_id => TargetKindLite::SelfPc,
                    EntityKind::Pc => TargetKindLite::Pc,
                    EntityKind::Npc => TargetKindLite::Npc,
                    EntityKind::Mob => TargetKindLite::Mob,
                    EntityKind::Pet | EntityKind::Other => TargetKindLite::None,
                }
            };
            let dx = e.pos.x - self_pos.x;
            let dy = e.pos.y - self_pos.y;
            let dz = e.pos.z - self_pos.z;
            let in_range = dx * dx + dy * dy + dz * dz <= NPC_INTERACT_YALMS * NPC_INTERACT_YALMS;
            (kind, in_range)
        }
        None => (TargetKindLite::None, false),
    };

    TargetActionContext {
        has_target: ent.is_some(),
        target_kind,
        in_range,
        trusts_available: false,
        engaged,
        usable_items_available,
    }
}

#[derive(Debug, Clone)]
pub struct RingSlot {
    pub id: TargetActionId,
    pub label: String,
    pub enabled: bool,

    pub slot_index: usize,
}

pub fn ring_skin_from(entries: &[ActionEntry]) -> Vec<RingSlot> {
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| RingSlot {
            id: e.id,
            label: e.label.clone(),
            enabled: e.enabled,
            slot_index: i,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::overlay::RETAIL;

    fn ctx(kind: TargetKindLite, in_range: bool) -> TargetActionContext {
        TargetActionContext {
            has_target: true,
            target_kind: kind,
            in_range,
            trusts_available: false,
            engaged: false,
            usable_items_available: true,
        }
    }

    fn ctx_engaged(kind: TargetKindLite, in_range: bool) -> TargetActionContext {
        TargetActionContext {
            engaged: true,
            ..ctx(kind, in_range)
        }
    }

    #[test]
    fn mob_menu_leads_with_attack_and_offers_check() {
        let entries = build_target_action_entries(&ctx(TargetKindLite::Mob, true), &RETAIL);
        let ids: Vec<_> = entries.iter().map(|e| e.id).collect();
        assert_eq!(ids.first(), Some(&TargetActionId::Attack));
        assert!(ids.contains(&TargetActionId::Check));
    }

    #[test]
    fn mob_menu_has_no_chat_or_trade() {
        let entries = build_target_action_entries(&ctx(TargetKindLite::Mob, true), &RETAIL);
        for e in &entries {
            assert!(!matches!(
                e.id,
                TargetActionId::Chat | TargetActionId::Trade
            ));
        }
    }

    #[test]
    fn unengaged_mob_menu_is_attack_magic_abilities_items_check() {
        let entries = build_target_action_entries(&ctx(TargetKindLite::Mob, true), &RETAIL);
        let ids: Vec<_> = entries.iter().map(|e| e.id).collect();
        assert_eq!(
            ids,
            vec![
                TargetActionId::Attack,
                TargetActionId::Magic,
                TargetActionId::Abilities,
                TargetActionId::Items,
                TargetActionId::Check,
            ]
        );
    }

    #[test]
    fn engaged_mob_menu_swaps_attack_for_switch_target_and_adds_disengage() {
        let entries = build_target_action_entries(&ctx_engaged(TargetKindLite::Mob, true), &RETAIL);
        let ids: Vec<_> = entries.iter().map(|e| e.id).collect();
        assert_eq!(
            ids,
            vec![
                TargetActionId::SwitchTarget,
                TargetActionId::Magic,
                TargetActionId::Abilities,
                TargetActionId::Items,
                TargetActionId::Disengage,
                TargetActionId::Check,
            ]
        );
        assert!(!ids.contains(&TargetActionId::Attack));
    }

    #[test]
    fn items_entry_greyed_when_no_usable_items() {
        let no_items = TargetActionContext {
            usable_items_available: false,
            ..ctx(TargetKindLite::Mob, true)
        };
        let entries = build_target_action_entries(&no_items, &RETAIL);
        let items = entries
            .iter()
            .find(|e| e.id == TargetActionId::Items)
            .expect("items entry still listed");
        assert!(!items.enabled);
        assert_eq!(items.hint.as_deref(), Some("No usable items."));
        // Other entries stay enabled.
        assert!(entries
            .iter()
            .filter(|e| e.id != TargetActionId::Items)
            .all(|e| e.enabled));
    }

    #[test]
    fn pc_menu_has_no_attack() {
        let entries = build_target_action_entries(&ctx(TargetKindLite::Pc, true), &RETAIL);
        assert!(entries.iter().all(|e| e.id != TargetActionId::Attack));
        assert!(entries.iter().any(|e| e.id == TargetActionId::Check));
    }

    #[test]
    fn npc_has_no_menu() {
        let entries = build_target_action_entries(&ctx(TargetKindLite::Npc, true), &RETAIL);
        assert!(entries.is_empty());
    }

    #[test]
    fn mob_attack_is_never_range_gated() {
        let far = build_target_action_entries(&ctx(TargetKindLite::Mob, false), &RETAIL);
        let attack = far.iter().find(|e| e.id == TargetActionId::Attack).unwrap();
        assert!(
            attack.enabled,
            "Attack must stay enabled out of melee range"
        );
    }
}
