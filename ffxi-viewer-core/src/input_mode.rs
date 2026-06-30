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

#[derive(Debug, Clone)]
pub struct MenuLevel {
    pub kind: MenuKind,
    pub cursor: usize,
}

#[derive(Debug, Clone, Default)]
pub struct MenuStack {
    pub levels: Vec<MenuLevel>,
}

impl MenuStack {
    pub fn root() -> Self {
        Self {
            levels: vec![MenuLevel {
                kind: MenuKind::Root,
                cursor: 0,
            }],
        }
    }

    pub fn current(&self) -> Option<&MenuLevel> {
        self.levels.last()
    }

    pub fn current_mut(&mut self) -> Option<&mut MenuLevel> {
        self.levels.last_mut()
    }

    pub fn push(&mut self, kind: MenuKind) {
        self.levels.push(MenuLevel { kind, cursor: 0 });
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

#[derive(Debug, Clone, Default)]
pub struct TargetActionState {
    pub cursor: usize,
    pub ctx: crate::hud::action_model::TargetActionContext,
    pub sub: Option<SubActionStack>,

    pub chat_mode_idx: usize,

    pub abilities_group_idx: usize,
}

impl TargetActionState {
    pub fn open(ctx: crate::hud::action_model::TargetActionContext) -> Self {
        Self {
            cursor: 0,
            ctx,
            sub: None,
            chat_mode_idx: 0,
            abilities_group_idx: 0,
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

#[derive(Debug, Clone, Default)]
pub struct SubActionStack {
    pub frames: Vec<SubAction>,
    pub cursor: usize,
}

impl SubActionStack {
    pub fn with(frame: SubAction) -> Self {
        Self {
            frames: vec![frame],
            cursor: 0,
        }
    }

    pub fn current(&self) -> Option<SubAction> {
        self.frames.last().copied()
    }

    pub fn push(&mut self, frame: SubAction) {
        self.frames.push(frame);
        self.cursor = 0;
    }

    pub fn pop(&mut self) -> bool {
        if self.frames.pop().is_some() {
            self.cursor = 0;
            !self.frames.is_empty()
        } else {
            false
        }
    }
}
