use bevy::prelude::Resource;

use super::action_model::{ActionEntry, TargetActionContext, TargetActionId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuEntryId {
    Magic,
    Abilities,
    Items,
    Equipment,
    Status,
    Party,
    Search,
    Config,

    TargetAttack,
    TargetSwitchTarget,
    TargetChat,
    TargetMagic,
    TargetAbilities,
    TargetTrust,
    TargetItems,
    TargetTrade,
    TargetDisengage,
    TargetCheck,
}

impl MenuEntryId {
    pub fn from_target_action(id: TargetActionId) -> Self {
        match id {
            TargetActionId::Attack => MenuEntryId::TargetAttack,
            TargetActionId::SwitchTarget => MenuEntryId::TargetSwitchTarget,
            TargetActionId::Chat => MenuEntryId::TargetChat,
            TargetActionId::Magic => MenuEntryId::TargetMagic,
            TargetActionId::Abilities => MenuEntryId::TargetAbilities,
            TargetActionId::Trust => MenuEntryId::TargetTrust,
            TargetActionId::Items => MenuEntryId::TargetItems,
            TargetActionId::Trade => MenuEntryId::TargetTrade,
            TargetActionId::Disengage => MenuEntryId::TargetDisengage,
            TargetActionId::Check => MenuEntryId::TargetCheck,
        }
    }
}

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

#[derive(Debug, Clone)]
pub struct MenuEntrySpec {
    pub id: MenuEntryId,
    pub label: Option<String>,
    pub visible: bool,
    pub order: u16,
}

#[derive(Debug, Clone, Default)]
pub struct MenuOverlay {
    pub entries: &'static [MenuEntrySpec],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpellCategory {
    White,
    Black,
    Songs,
    Summon,
    Blue,
}

impl SpellCategory {
    pub const ALL: [SpellCategory; 5] = [
        SpellCategory::White,
        SpellCategory::Black,
        SpellCategory::Songs,
        SpellCategory::Summon,
        SpellCategory::Blue,
    ];

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

#[derive(Debug, Clone, Default)]
pub struct MagicOverlay {
    pub enabled: &'static [SpellCategory],
}

#[derive(Debug, Clone, Default)]
pub struct JobOverlay {
    pub allowed_jobs: &'static [u8],
}

#[derive(Debug, Clone)]
pub struct ClientOverlay {
    pub id: &'static str,

    pub display_name: &'static str,
    pub menu: MenuOverlay,
    pub magic: MagicOverlay,
    pub jobs: JobOverlay,
}

impl ClientOverlay {
    pub const fn retail_base() -> ClientOverlay {
        ClientOverlay {
            id: "retail",
            display_name: "Retail / LSB",
            menu: MenuOverlay { entries: &[] },
            magic: MagicOverlay { enabled: &[] },
            jobs: JobOverlay { allowed_jobs: &[] },
        }
    }

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

    pub fn resolve_target_actions(&self, ctx: &TargetActionContext) -> Vec<ActionEntry> {
        let mut entries = super::action_model::build_target_action_entries(ctx, self);
        if !self.menu.entries.is_empty() {
            entries.retain(|e| self.is_visible(MenuEntryId::from_target_action(e.id)));
            entries.sort_by_key(|e| self.order_of(MenuEntryId::from_target_action(e.id)));
        }
        entries
    }

    pub fn category_of(&self, _spell_id: u16) -> Option<SpellCategory> {
        None
    }

    pub fn enabled_categories(&self) -> &[SpellCategory] {
        if self.magic.enabled.is_empty() {
            &SpellCategory::ALL
        } else {
            self.magic.enabled
        }
    }

    pub fn job_allowed(&self, job_id: u8) -> bool {
        self.jobs.allowed_jobs.is_empty() || self.jobs.allowed_jobs.contains(&job_id)
    }

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

pub const RETAIL: ClientOverlay = ClientOverlay::retail_base();

pub const HORIZON: ClientOverlay = ClientOverlay {
    id: "horizon",
    display_name: "HorizonXI",
    menu: MenuOverlay { entries: &[] },
    magic: MagicOverlay {
        enabled: &SpellCategory::ALL,
    },
    jobs: JobOverlay { allowed_jobs: &[] },
};

pub const PROFILES: &[&ClientOverlay] = &[&RETAIL, &HORIZON];

pub fn profile_by_id(id: &str) -> Option<&'static ClientOverlay> {
    PROFILES.iter().copied().find(|p| p.id == id)
}

#[derive(Resource, Debug, Clone, Copy)]
pub struct ActiveOverlay(pub &'static ClientOverlay);

impl Default for ActiveOverlay {
    fn default() -> Self {
        ActiveOverlay(&RETAIL)
    }
}
