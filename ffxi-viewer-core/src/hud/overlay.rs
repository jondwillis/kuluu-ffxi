//! Client-profile *overlay* layer for the menu suite.
//!
//! The parity base is always **retail / LSB**: [`RETAIL`] is the canonical
//! ground truth and an empty overlay is, by construction, identical to it.
//! A [`ClientOverlay`] is a *sparse diff* applied on top of that base —
//! absent fields fall through to retail, so a server-specific profile
//! (e.g. [`HORIZON`]) is a thin tailoring layer rather than a fork.
//!
//! Everything here is pure data + pure functions. No Bevy systems live in
//! this file; the only Bevy surface is [`ActiveOverlay`], a `Resource`
//! pointer to the currently-selected compiled-in profile (defaulting to
//! [`RETAIL`]). Selection is switched at runtime via `/overlay <name>`
//! (wired by a later feature agent) and seeded in the plugin's
//! `init_resource::<ActiveOverlay>()`.
//!
//! Foundation scope: this file defines the shared *types* the menu /
//! magic / action-model feature agents build on. The resolver bodies are
//! intentionally minimal (correct for the empty-overlay / retail case)
//! so the crate compiles and the parallel agents have a stable contract.

use bevy::prelude::Resource;

use super::action_model::{ActionEntry, TargetActionContext, TargetActionId};

/// Stable identity of a single entry in the root "Commands" menu or in a
/// target-action context menu. Used as the join key between the retail
/// base order ([`RETAIL_COMMANDS`]) and an overlay's sparse per-entry
/// tweaks ([`MenuEntrySpec`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuEntryId {
    // Root "Commands" verbs.
    Magic,
    Abilities,
    Items,
    Equipment,
    Status,
    Party,
    Search,
    Config,
    // Target-action contextual verbs — mirrors
    // [`super::action_model::TargetActionId`] so an overlay can reorder /
    // relabel / hide the contextual menu through the same machinery as
    // the root menu.
    TargetChat,
    TargetMagic,
    TargetAbilities,
    TargetTrust,
    TargetItems,
    TargetTrade,
    TargetCheck,
}

impl MenuEntryId {
    /// Map a contextual [`TargetActionId`] onto its [`MenuEntryId`] so the
    /// overlay can tweak target-action entries with the same `entries`
    /// diff list it uses for the root menu.
    pub fn from_target_action(id: TargetActionId) -> Self {
        match id {
            TargetActionId::Chat => MenuEntryId::TargetChat,
            TargetActionId::Magic => MenuEntryId::TargetMagic,
            TargetActionId::Abilities => MenuEntryId::TargetAbilities,
            TargetActionId::Trust => MenuEntryId::TargetTrust,
            TargetActionId::Items => MenuEntryId::TargetItems,
            TargetActionId::Trade => MenuEntryId::TargetTrade,
            TargetActionId::Check => MenuEntryId::TargetCheck,
        }
    }
}

/// Retail base order of the root "Commands" menu. An overlay's
/// [`MenuOverlay::entries`] diff is layered over this; absent ids keep
/// their base position. This `const` is the literal "parity base".
pub const RETAIL_COMMANDS: &[MenuEntryId] = &[
    MenuEntryId::Magic,
    MenuEntryId::Abilities,
    MenuEntryId::Items,
    MenuEntryId::Equipment,
    MenuEntryId::Status,
    MenuEntryId::Party,
    MenuEntryId::Search,
    MenuEntryId::Config,
];

/// One sparse tweak to a menu entry. `label: None` keeps the base label;
/// `visible: false` hides the entry; `order` is the sort key applied when
/// the overlay reorders entries (lower sorts first). Only entries an
/// overlay actually wants to change need a spec — everything else falls
/// through to the retail base.
#[derive(Debug, Clone)]
pub struct MenuEntrySpec {
    pub id: MenuEntryId,
    pub label: Option<String>,
    pub visible: bool,
    pub order: u16,
}

/// Overlay diff for the root "Commands" menu and the target-action
/// contextual menu. Empty = retail.
#[derive(Debug, Clone, Default)]
pub struct MenuOverlay {
    /// Sparse per-entry tweaks. Entries not present here keep their retail
    /// label / visibility / position.
    pub entries: &'static [MenuEntrySpec],
}

/// Classic FFXI spell categories. HorizonXI enables exactly these five
/// (no newer post-classic categories). A category with no learned spells
/// renders "No spells available." in the leaf view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpellCategory {
    White,
    Black,
    Songs,
    Summon,
    Blue,
}

impl SpellCategory {
    /// All five classic categories, in the retail display order.
    pub const ALL: [SpellCategory; 5] = [
        SpellCategory::White,
        SpellCategory::Black,
        SpellCategory::Songs,
        SpellCategory::Summon,
        SpellCategory::Blue,
    ];

    /// Human-readable label for the category tab.
    pub fn label(self) -> &'static str {
        match self {
            SpellCategory::White => "White Magic",
            SpellCategory::Black => "Black Magic",
            SpellCategory::Songs => "Songs",
            SpellCategory::Summon => "Summoning",
            SpellCategory::Blue => "Blue Magic",
        }
    }
}

/// Overlay diff for the magic menu. `enabled` lists the categories this
/// profile exposes (retail = all five today). Empty = retail default.
#[derive(Debug, Clone, Default)]
pub struct MagicOverlay {
    /// Categories this profile shows, in display order. Empty falls
    /// through to [`SpellCategory::ALL`].
    pub enabled: &'static [SpellCategory],
}

/// Overlay diff for job availability. `allowed_jobs` filters job lists and
/// ability / magic availability; ids match LSB `job_name.lua`. Empty =
/// retail (all jobs).
#[derive(Debug, Clone, Default)]
pub struct JobOverlay {
    /// LSB job ids this profile permits. Empty = no restriction (retail).
    pub allowed_jobs: &'static [u8],
}

/// A compiled-in client profile. The retail base is [`ClientOverlay::retail_base`];
/// any other profile is a sparse diff over it.
#[derive(Debug, Clone)]
pub struct ClientOverlay {
    /// Stable machine id, e.g. `"retail"` or `"horizon"`. Used by
    /// `/overlay <name>` selection.
    pub id: &'static str,
    /// Human-readable name for the HUD.
    pub display_name: &'static str,
    pub menu: MenuOverlay,
    pub magic: MagicOverlay,
    pub jobs: JobOverlay,
}

impl ClientOverlay {
    /// The empty overlay == retail. Every diff field is empty, so every
    /// lookup falls through to the retail base behavior.
    pub const fn retail_base() -> ClientOverlay {
        ClientOverlay {
            id: "retail",
            display_name: "Retail / LSB",
            menu: MenuOverlay { entries: &[] },
            magic: MagicOverlay { enabled: &[] },
            jobs: JobOverlay { allowed_jobs: &[] },
        }
    }

    /// Effective ordered list of root "Commands" entries after applying
    /// this overlay's diff over [`RETAIL_COMMANDS`]. Hidden entries are
    /// dropped; relabels are applied later at render time via
    /// [`Self::label_for`].
    ///
    /// Foundation impl: handles the empty-overlay (retail) case exactly,
    /// and applies `visible: false` hides + `order` sorting when specs
    /// are present. Feature agents extend rendering on top of this.
    pub fn resolve_commands(&self) -> Vec<MenuEntryId> {
        let mut out: Vec<MenuEntryId> = RETAIL_COMMANDS
            .iter()
            .copied()
            .filter(|id| self.is_visible(*id))
            .collect();
        if !self.menu.entries.is_empty() {
            out.sort_by_key(|id| self.order_of(*id));
        }
        out
    }

    /// Effective ordered list of target-action entries for `ctx`, after
    /// applying the overlay diff over the base entries produced by
    /// [`super::action_model::build_target_action_entries`].
    pub fn resolve_target_actions(&self, ctx: &TargetActionContext) -> Vec<ActionEntry> {
        let mut entries = super::action_model::build_target_action_entries(ctx, self);
        if !self.menu.entries.is_empty() {
            entries.retain(|e| self.is_visible(MenuEntryId::from_target_action(e.id)));
            entries.sort_by_key(|e| self.order_of(MenuEntryId::from_target_action(e.id)));
        }
        entries
    }

    /// Classify a spell id into one of the five classic categories, gated
    /// by which categories this overlay enables. Returns `None` when the
    /// id is unknown or its category isn't enabled by this profile.
    ///
    /// Foundation impl returns `None` (the classifier id-range table is
    /// populated by the magic feature agent). The signature is the
    /// contract; callers must treat `None` as "uncategorized".
    pub fn category_of(&self, _spell_id: u16) -> Option<SpellCategory> {
        None
    }

    /// Categories this overlay exposes, in display order. Falls through to
    /// [`SpellCategory::ALL`] for the retail base.
    pub fn enabled_categories(&self) -> &[SpellCategory] {
        if self.magic.enabled.is_empty() {
            &SpellCategory::ALL
        } else {
            self.magic.enabled
        }
    }

    /// Whether `job_id` (LSB id) is permitted by this profile. Retail
    /// (empty `allowed_jobs`) permits everything.
    pub fn job_allowed(&self, job_id: u8) -> bool {
        self.jobs.allowed_jobs.is_empty() || self.jobs.allowed_jobs.contains(&job_id)
    }

    /// Effective label for an entry: the overlay's relabel if present,
    /// else `base`.
    pub fn label_for(&self, id: MenuEntryId, base: &'static str) -> String {
        self.spec(id)
            .and_then(|s| s.label.clone())
            .unwrap_or_else(|| base.to_string())
    }

    fn spec(&self, id: MenuEntryId) -> Option<&MenuEntrySpec> {
        self.menu.entries.iter().find(|s| s.id == id)
    }

    fn is_visible(&self, id: MenuEntryId) -> bool {
        self.spec(id).map(|s| s.visible).unwrap_or(true)
    }

    fn order_of(&self, id: MenuEntryId) -> u16 {
        self.spec(id).map(|s| s.order).unwrap_or(u16::MAX)
    }
}

/// The canonical retail / LSB profile. Empty overlay == base.
pub const RETAIL: ClientOverlay = ClientOverlay::retail_base();

/// HorizonXI profile: classic-only tailoring. Foundation stub — it
/// declares the five classic magic categories and otherwise inherits
/// retail. The job allow-list / menu relabels are populated by the
/// feature agents that own those leaf views.
pub const HORIZON: ClientOverlay = ClientOverlay {
    id: "horizon",
    display_name: "HorizonXI",
    menu: MenuOverlay { entries: &[] },
    magic: MagicOverlay {
        enabled: &SpellCategory::ALL,
    },
    jobs: JobOverlay { allowed_jobs: &[] },
};

/// All compiled-in profiles, for `/overlay list` and name resolution.
pub const PROFILES: &[&ClientOverlay] = &[&RETAIL, &HORIZON];

/// Resolve a profile by its machine `id`. Returns `None` for unknown
/// names so the `/overlay` handler can report the error.
pub fn profile_by_id(id: &str) -> Option<&'static ClientOverlay> {
    PROFILES.iter().copied().find(|p| p.id == id)
}

/// The currently-selected client profile. Defaults to [`RETAIL`] so the
/// parity base is in force until `/overlay <name>` selects otherwise.
#[derive(Resource, Debug, Clone, Copy)]
pub struct ActiveOverlay(pub &'static ClientOverlay);

impl Default for ActiveOverlay {
    fn default() -> Self {
        ActiveOverlay(&RETAIL)
    }
}
