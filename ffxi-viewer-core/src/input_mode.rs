use bevy::prelude::Resource;

#[derive(Resource, Debug, Clone, Default)]
pub enum InputMode {
    #[default]
    World,

    Chat(ChatBuffer),

    Menu(MenuStack),

    QuickAction(QuickActionState),

    TargetAction(TargetActionState),

    Dialog(DialogCursor),

    PassiveCursor(PassiveCursorState),
}

pub const DIALOG_MAX_CHOICE: u32 = 7;

#[derive(Debug, Clone, Copy, Default)]
pub struct DialogCursor {
    pub cursor: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ChatBuffer {
    pub text: String,
}

impl ChatBuffer {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with_prefix(prefix: &str) -> Self {
        Self {
            text: prefix.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MenuKind {
    Root,

    Config,

    Debug,

    Graphics,

    Magic,

    Abilities,

    Items,

    Equipment,

    Status,

    EquipSlot(u8),
}

/// One entry in a `NavStack`: which sub-menu/sub-mode `K` is showing, and the
/// list cursor it had the last time it was on top (restored on pop back to
/// it, so backing out of a drilled-down level doesn't reset your place).
#[derive(Debug, Clone)]
pub struct NavLevel<K> {
    pub kind: K,
    pub cursor: usize,
}

/// A generic push/pop stack of navigable menu levels, shared by every
/// drill-down menu in the HUD (the root pause menu keyed by `MenuKind`, the
/// target-action menu keyed by `TargetLevel`) so scroll-window/list-cursor
/// behavior — including per-level cursor memory across push/pop — only needs
/// implementing once.
#[derive(Debug, Clone)]
pub struct NavStack<K> {
    pub levels: Vec<NavLevel<K>>,
}

impl<K> NavStack<K> {
    pub fn single(kind: K) -> Self {
        Self {
            levels: vec![NavLevel { kind, cursor: 0 }],
        }
    }

    pub fn current(&self) -> Option<&NavLevel<K>> {
        self.levels.last()
    }

    pub fn current_mut(&mut self) -> Option<&mut NavLevel<K>> {
        self.levels.last_mut()
    }

    pub fn push(&mut self, kind: K) {
        self.levels.push(NavLevel { kind, cursor: 0 });
    }

    pub fn pop(&mut self) -> bool {
        if self.levels.len() > 1 {
            self.levels.pop();
            true
        } else {
            false
        }
    }
}

pub type MenuLevel = NavLevel<MenuKind>;
pub type MenuStack = NavStack<MenuKind>;

impl MenuStack {
    pub fn root() -> Self {
        Self::single(MenuKind::Root)
    }
}

#[derive(Debug, Clone, Default)]
pub struct QuickActionState {
    pub cursor: usize,
    pub has_target: bool,
}

impl QuickActionState {
    pub fn for_target(has_target: bool) -> Self {
        Self {
            cursor: 0,
            has_target,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum PassiveCursorFocus {
    #[default]
    Chat,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PassiveCursorState {
    pub focus: PassiveCursorFocus,
}

impl PassiveCursorState {
    pub fn fresh_chat() -> Self {
        Self {
            focus: PassiveCursorFocus::Chat,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAction {
    MagicCategory(crate::hud::overlay::SpellCategory),

    AbilitiesGroup(crate::hud::action_model::AbilityGroup),

    Items,

    ChatCompose,
}

/// A level in the target-action menu's `NavStack`: either the base list of
/// actions for the current target, or a drilled-down `SubAction` (e.g. an
/// ability group's own list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetLevel {
    Root,
    Sub(SubAction),
}

#[derive(Debug, Clone)]
pub struct TargetActionState {
    pub stack: NavStack<TargetLevel>,
    pub ctx: crate::hud::action_model::TargetActionContext,

    pub chat_mode_idx: usize,

    pub abilities_group_idx: usize,
}

impl TargetActionState {
    pub fn open(ctx: crate::hud::action_model::TargetActionContext) -> Self {
        Self {
            stack: NavStack::single(TargetLevel::Root),
            ctx,
            chat_mode_idx: 0,
            abilities_group_idx: 0,
        }
    }
}
