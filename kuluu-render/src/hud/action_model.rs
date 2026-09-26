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
    Fish,
    Dig,
    Dismount,
}

impl TargetActionId {
    /// Whether confirming this entry only addresses, opens or inspects the
    /// target — it commits no resource and starts no fight, so there is nothing
    /// for the player to undo if it fires unasked.
    pub fn is_non_destructive(self) -> bool {
        match self {
            TargetActionId::Open
            | TargetActionId::Check
            | TargetActionId::Chat
            | TargetActionId::Dig
            | TargetActionId::Dismount
            | TargetActionId::SwitchTarget => true,
            TargetActionId::Attack
            | TargetActionId::Magic
            | TargetActionId::Abilities
            | TargetActionId::Trust
            | TargetActionId::Items
            | TargetActionId::Trade
            | TargetActionId::Disengage
            | TargetActionId::Fish => false,
        }
    }
}

/// The entry a target's Command Menu may confirm on the player's behalf instead
/// of drawing a one-line box to press Enter in again.
///
/// Retail never reaches the menu for a target whose whole vocabulary is one
/// interaction: a static NPC — doors included — interacts straight off the
/// confirm key (research/xim UiState.kt `handleDefaultEnter`).
pub fn sole_auto_confirm_entry(entries: &[ActionEntry]) -> Option<&ActionEntry> {
    match entries {
        [only] if only.enabled && only.id.is_non_destructive() => Some(only),
        _ => None,
    }
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

    /// Retail's client-side fishing gate: idle, a fishing rod in the ranged
    /// slot, and water within casting reach ahead. All three must hold before
    /// "Fish" is offered at all — retail omits the entry rather than greying it
    /// (research/xim UiState.kt `getCurrentActions`).
    pub can_fish: bool,

    /// The player is riding a mount: the self menu becomes Chat / Dig /
    /// Dismount and Dismount is appended to every other menu
    /// (.agents/skills/retail-observe/references/2026-09-24-chocobo-mounted-menu.md,
    /// research/xim UiState.kt `getCurrentActions`).
    pub mounted: bool,
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

fn applies_to(id: TargetActionId, kind: TargetKindLite, engaged: bool, mounted: bool) -> bool {
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

        // Fishing needs no target; retail keeps the entry available when one
        // happens to be selected (research/xim UiState.kt `getCurrentActions`
        // appends it independently of the target).
        TargetActionId::Fish => true,

        // Chocobo commands: Dig only on the mounted self menu, Dismount
        // appended to every menu while mounted (the same XIM `getCurrentActions`
        // append, the self-menu replacement from the mounted-menu capture).
        TargetActionId::Dig => kind == SelfPc && mounted,
        TargetActionId::Dismount => mounted,
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
        TargetActionId::Dismount,
        TargetActionId::Trade,
        TargetActionId::Disengage,
        TargetActionId::Fish,
        TargetActionId::Check,
    ];

    // While riding, retail replaces the self menu with Chat / Dig / Dismount
    // (.agents/skills/retail-observe/references/2026-09-24-chocobo-mounted-menu.md;
    // the item ids are retail's own, research/xim ActionMenu.kt Dig(38) /
    // Dismount(39)). The command menu opens on the self target, which is the
    // no-target menu (`has_target` false) as well as an explicit self target;
    // a Pet/Other target still reads `None` kind with `has_target` true and
    // keeps the appended-Dismount menu.
    if ctx.mounted && (ctx.target_kind == TargetKindLite::SelfPc || !ctx.has_target) {
        return [
            TargetActionId::Chat,
            TargetActionId::Dig,
            TargetActionId::Dismount,
        ]
        .iter()
        .map(|&id| entry_for(id, ctx))
        .collect();
    }

    let mut out = Vec::new();
    for &id in ORDER {
        if id == TargetActionId::Trust && !ctx.trusts_available {
            continue;
        }
        if id == TargetActionId::Fish && !ctx.can_fish {
            continue;
        }
        if !applies_to(id, ctx.target_kind, ctx.engaged, ctx.mounted) {
            continue;
        }
        out.push(entry_for(id, ctx));
    }
    out
}

fn entry_for(id: TargetActionId, ctx: &TargetActionContext) -> ActionEntry {
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
        TargetActionId::Fish => (ActionEntryKind::Plain, "Fish".to_string()),
        TargetActionId::Dig => (ActionEntryKind::Plain, "Dig".to_string()),
        TargetActionId::Dismount => (ActionEntryKind::Plain, "Dismount".to_string()),
    };

    let hint = if out_of_range {
        Some("Target out of range.".to_string())
    } else if no_usable_items {
        Some("No usable items.".to_string())
    } else {
        None
    };

    ActionEntry {
        id,
        label,
        kind,
        enabled: !out_of_range && !no_usable_items,
        hint,
    }
}

pub fn context_for_target(
    target_id: Option<u32>,
    entities: &[kuluu_snapshot::Entity],
    self_pos: kuluu_snapshot::Vec3,
    self_id: Option<u32>,
    engaged: bool,
    usable_items_available: bool,
    can_fish: bool,
    mounted: bool,
) -> TargetActionContext {
    use kuluu_snapshot::EntityKind;

    let ent = target_id.and_then(|id| entities.iter().find(|e| e.id == id));
    let (target_kind, in_range) = match ent {
        Some(e) => {
            let kind = if matches!(e.look, Some(kuluu_snapshot::EntityLook::Door { .. })) {
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
        can_fish,
        mounted,
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
            can_fish: false,
            mounted: false,
        }
    }

    fn mounted_ctx(kind: TargetKindLite) -> TargetActionContext {
        TargetActionContext {
            mounted: true,
            ..ctx(kind, true)
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
    fn door_menu_is_one_entry_and_confirms_itself() {
        let entries = build_target_action_entries(&ctx(TargetKindLite::Door, true), &RETAIL);
        let ids: Vec<_> = entries.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![TargetActionId::Open]);
        assert_eq!(
            sole_auto_confirm_entry(&entries).map(|e| e.id),
            Some(TargetActionId::Open),
            "a door must open off the first confirm, not the second"
        );
    }

    #[test]
    fn an_out_of_range_door_still_shows_its_menu() {
        let entries = build_target_action_entries(&ctx(TargetKindLite::Door, false), &RETAIL);
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].enabled);
        assert!(
            sole_auto_confirm_entry(&entries).is_none(),
            "the greyed entry carries the reason, so the player has to see it"
        );
    }

    #[test]
    fn a_door_within_casting_reach_keeps_its_menu() {
        let fishing = TargetActionContext {
            can_fish: true,
            ..ctx(TargetKindLite::Door, true)
        };
        let entries = build_target_action_entries(&fishing, &RETAIL);
        assert_eq!(entries.len(), 2);
        assert!(sole_auto_confirm_entry(&entries).is_none());
    }

    #[test]
    fn multi_entry_menus_are_never_auto_confirmed() {
        for kind in [
            TargetKindLite::Mob,
            TargetKindLite::Pc,
            TargetKindLite::SelfPc,
            TargetKindLite::None,
        ] {
            let entries = build_target_action_entries(&ctx(kind, true), &RETAIL);
            assert!(
                entries.len() > 1,
                "{kind:?} is expected to offer more than one command"
            );
            assert!(sole_auto_confirm_entry(&entries).is_none());
        }
    }

    /// The mounted no-target context: the command menu opens on the self
    /// target, which is the no-target menu, not an explicit self target.
    fn mounted_no_target() -> TargetActionContext {
        TargetActionContext {
            has_target: false,
            target_kind: TargetKindLite::None,
            mounted: true,
            ..Default::default()
        }
    }

    #[test]
    fn mounted_self_menu_is_chat_dig_dismount() {
        let entries = build_target_action_entries(&mounted_ctx(TargetKindLite::SelfPc), &RETAIL);
        let ids: Vec<_> = entries.iter().map(|e| e.id).collect();
        assert_eq!(
            ids,
            vec![
                TargetActionId::Chat,
                TargetActionId::Dig,
                TargetActionId::Dismount
            ]
        );
        assert!(entries.iter().all(|e| e.enabled));
    }

    #[test]
    fn mounted_no_target_menu_is_chat_dig_dismount() {
        let entries = build_target_action_entries(&mounted_no_target(), &RETAIL);
        let ids: Vec<_> = entries.iter().map(|e| e.id).collect();
        assert_eq!(
            ids,
            vec![
                TargetActionId::Chat,
                TargetActionId::Dig,
                TargetActionId::Dismount
            ]
        );
        assert!(entries.iter().all(|e| e.enabled));
    }

    #[test]
    fn mounted_other_menus_gain_dismount_but_not_dig() {
        for kind in [
            TargetKindLite::Mob,
            TargetKindLite::Pc,
            TargetKindLite::None,
        ] {
            let entries = build_target_action_entries(&mounted_ctx(kind), &RETAIL);
            let ids: Vec<_> = entries.iter().map(|e| e.id).collect();
            assert!(
                ids.contains(&TargetActionId::Dismount),
                "{kind:?} loses Dismount"
            );
            assert!(!ids.contains(&TargetActionId::Dig), "{kind:?} gains Dig");
        }
    }

    #[test]
    fn dismounted_menus_have_no_chocobo_rows() {
        for kind in [
            TargetKindLite::SelfPc,
            TargetKindLite::Pc,
            TargetKindLite::None,
        ] {
            let entries = build_target_action_entries(&ctx(kind, true), &RETAIL);
            let ids: Vec<_> = entries.iter().map(|e| e.id).collect();
            assert!(!ids.contains(&TargetActionId::Dig), "{kind:?} gains Dig");
            assert!(
                !ids.contains(&TargetActionId::Dismount),
                "{kind:?} gains Dismount"
            );
        }
    }

    #[test]
    fn a_lone_committing_entry_is_not_confirmed_for_the_player() {
        for id in [
            TargetActionId::Attack,
            TargetActionId::Trade,
            TargetActionId::Items,
            TargetActionId::Fish,
            TargetActionId::Disengage,
        ] {
            let lone = vec![ActionEntry {
                id,
                label: String::new(),
                kind: ActionEntryKind::Plain,
                enabled: true,
                hint: None,
            }];
            assert!(
                sole_auto_confirm_entry(&lone).is_none(),
                "{id:?} commits something and must keep its confirm"
            );
        }
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
