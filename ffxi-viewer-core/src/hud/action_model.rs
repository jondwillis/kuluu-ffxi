//! Shared *action model* for the contextual target-action menu.
//!
//! Both the vanilla contextual menu (the retail-style verb list shown when
//! you confirm on a target) and the de-promoted "Enhanced" quick-action
//! ring render over the *same* data produced here. The ring is provably a
//! view: [`ring_skin_from`] projects the exact entries
//! [`build_target_action_entries`] returns into the radial layout, so the
//! two surfaces can never disagree about what a target affords.
//!
//! This file is pure logic + types — no Bevy systems. It is the single
//! source of truth the menu / overlay / quick-action feature agents build
//! on. The overlay ([`super::overlay::ClientOverlay`]) reorders / relabels
//! / hides entries via [`super::overlay::ClientOverlay::resolve_target_actions`];
//! this module produces the *base* (retail) entry set.

use super::overlay::ClientOverlay;

/// Yalms within which NPC-interaction verbs (Chat / Trade / Check) are
/// enabled. Beyond this the entries stay visible but disabled with a
/// "Target out of range." hint — matching retail's behavior of greying
/// rather than hiding.
pub const NPC_INTERACT_YALMS: f32 = 6.0;

/// Stable identity of a target-action verb. This is the foundation join
/// key between the action model, the overlay diff
/// ([`super::overlay::MenuEntryId::from_target_action`]), and the ring
/// skin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetActionId {
    Chat,
    Magic,
    Abilities,
    Trust,
    Items,
    Trade,
    Check,
}

/// Coarse, render-side classification of the current target. Drives
/// contextual visibility: Chat / Trade / Check are only meaningful for
/// PCs and (some) NPCs; combat verbs only for mobs; etc. "Lite" because
/// it carries only what the action model needs, not the full entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetKindLite {
    /// The operator's own character.
    SelfPc,
    /// Another player character.
    Pc,
    /// A non-hostile NPC (vendor, quest-giver, …).
    Npc,
    /// A hostile / claimable monster.
    Mob,
    /// A door / zone-line / interactable object.
    Door,
    /// No valid target.
    #[default]
    None,
}

/// Everything [`build_target_action_entries`] needs to decide which verbs
/// to show and whether each is enabled. Captured at the moment the menu
/// opens so the entry list won't shift mid-navigation if a server
/// snapshot changes the target between keypresses (same discipline as
/// [`crate::input_mode::QuickActionState`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct TargetActionContext {
    pub has_target: bool,
    pub target_kind: TargetKindLite,
    /// Within [`NPC_INTERACT_YALMS`] of the target.
    pub in_range: bool,
    /// Whether the operator has any Trust magic available (the Trust verb
    /// is hidden entirely until trusts exist).
    pub trusts_available: bool,
}

/// How an action entry behaves when selected / right-arrowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionEntryKind {
    /// A normal button: Enter fires it.
    Plain,
    /// A cycler: right-arrow advances `mode_idx` through `modes`
    /// (e.g. Chat's Say / Tell / Party / Linkshell / Unity / Shout, or
    /// Magic's flat-list-vs-category toggle).
    Select {
        modes: Vec<&'static str>,
        mode_idx: usize,
    },
}

/// One row of the contextual target-action menu. `hint` carries a
/// contextual error/explanation string ("Target out of range.",
/// "No spells available.") rendered as a subtitle when present.
#[derive(Debug, Clone)]
pub struct ActionEntry {
    pub id: TargetActionId,
    pub label: String,
    pub kind: ActionEntryKind,
    pub enabled: bool,
    pub hint: Option<String>,
}

/// Grouping for the Abilities sub-menu / sub-action stack. Mirrors the
/// retail "Abilities" breakdown so the input layer can push into one
/// group and back out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbilityGroup {
    JobAbilities,
    WeaponSkill,
    RangedAttack,
    Mount,
    PetCommand,
}

impl AbilityGroup {
    /// Retail group order, for the contextual menu's NavRight cycler and
    /// any ordered iteration. Mirrors `SpellCategory::ALL`.
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

/// Whether a verb is contextually applicable to the given target kind.
/// Chat / Trade / Check are PC/NPC-only; Magic / Abilities / Items /
/// Trust are self-castable and so always offered (their per-entry
/// `enabled` state is decided separately).
fn applies_to(id: TargetActionId, kind: TargetKindLite) -> bool {
    use TargetKindLite::*;
    match id {
        TargetActionId::Chat | TargetActionId::Trade | TargetActionId::Check => {
            matches!(kind, Pc | Npc | SelfPc)
        }
        // These act on self/target as the verb's own targeting allows, so
        // they're always present in the contextual list.
        TargetActionId::Magic
        | TargetActionId::Abilities
        | TargetActionId::Items
        | TargetActionId::Trust => true,
    }
}

/// Build the *base* (retail) target-action entry list for `ctx`. The
/// overlay's `resolve_target_actions` layers reorder / relabel / hide on
/// top of this; callers that don't care about overlays can use this
/// directly.
///
/// Single source of truth: the ring skin ([`ring_skin_from`]) consumes
/// the exact `Vec` this returns.
pub fn build_target_action_entries(
    ctx: &TargetActionContext,
    _overlay: &ClientOverlay,
) -> Vec<ActionEntry> {
    // Retail contextual verb order.
    const ORDER: &[TargetActionId] = &[
        TargetActionId::Chat,
        TargetActionId::Magic,
        TargetActionId::Abilities,
        TargetActionId::Trust,
        TargetActionId::Items,
        TargetActionId::Trade,
        TargetActionId::Check,
    ];

    let mut out = Vec::new();
    for &id in ORDER {
        // Trust is hidden entirely until trusts exist.
        if id == TargetActionId::Trust && !ctx.trusts_available {
            continue;
        }
        if !applies_to(id, ctx.target_kind) {
            continue;
        }

        let needs_range = matches!(
            id,
            TargetActionId::Chat | TargetActionId::Trade | TargetActionId::Check
        );
        let out_of_range = needs_range && ctx.has_target && !ctx.in_range;

        let (kind, label) = match id {
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
                // Cycler over the five ability groups; NavRight advances the
                // group that confirming the row will descend into (the input
                // router tracks the cycled index in `abilities_group_idx`,
                // since this entry is rebuilt at `mode_idx == 0` each frame).
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
        };

        let hint = if out_of_range {
            Some("Target out of range.".to_string())
        } else {
            None
        };

        out.push(ActionEntry {
            id,
            label,
            kind,
            enabled: !out_of_range,
            hint,
        });
    }
    out
}

/// One slot in the radial Enhanced ring. The ring is a *skin* over the
/// same [`ActionEntry`] data, so it carries the entry's id + label +
/// enabled state plus its angular index.
#[derive(Debug, Clone)]
pub struct RingSlot {
    pub id: TargetActionId,
    pub label: String,
    pub enabled: bool,
    /// 0-based position around the ring, clockwise from the top.
    pub slot_index: usize,
}

/// Project the contextual entry list into the radial ring layout. Proves
/// the ring is a view over the model: it reads nothing the menu doesn't.
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
