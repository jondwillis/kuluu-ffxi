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

    /// Retail sub-target confirm step: an action was chosen from a menu and
    /// the flashing sub-target cursor is asking "on whom?". Esc returns to
    /// `return_to`; confirm fires the action at `candidate`.
    SubTarget(SubTargetState),

    /// The dedicated delivery box screen is open and modal. Focus/selector
    /// state lives in `hud::delivery::DeliveryScreenState`; this variant just
    /// suppresses world movement/camera and routes keys to the delivery handler.
    DeliveryBox,
}

/// The action pending behind a sub-target cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubTargetAction {
    Spell(u16),
    Ability(u16),
    WeaponSkill(u16),
    Item {
        container: u8,
        index: u8,
        item_no: u16,
    },
}

#[derive(Debug, Clone)]
pub struct SubTargetState {
    pub action: SubTargetAction,

    /// TARGETTYPE bitmask for the pending action (ffxi-proto valid_target).
    pub flags: u16,

    /// Entity currently under the sub-target cursor. None when no valid
    /// candidate exists in range (cursor parks on self only if SELF is valid).
    pub candidate: Option<u32>,

    /// Mode to restore on Esc (retail: back to the menu, cursor preserved).
    pub return_to: Box<InputMode>,
}

impl SubTargetState {
    pub fn open(action: SubTargetAction, flags: u16, return_to: InputMode) -> Self {
        Self {
            action,
            flags,
            candidate: None,
            return_to: Box::new(return_to),
        }
    }
}

pub const DIALOG_MAX_CHOICE: u32 = 7;

#[derive(Debug, Clone, Default)]
pub struct DialogCursor {
    pub cursor: u32,
    /// Line buffer for a free-text dialog frame (`DialogState::text_entry`,
    /// e.g. the delivery-box recipient prompt). `None` for choice/speech
    /// frames; initialized when the text-entry frame is first handled.
    pub entry: Option<String>,
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

    KeyItems,

    /// Retail Command Menu "Items": only items that can actually be used
    /// right now (LSB 0x037 semantics — see `hud::menu::item_usable_now`),
    /// each row firing Use directly instead of opening the full bag.
    UsableItems,

    /// Per-item context menu pushed from the Items window (retail's item
    /// submenu): Use / Take Out / Put in <bag> rows for the focused slot.
    ItemAction {
        container: u8,
        index: u8,
        item_no: u16,
    },

    Equipment,

    Status,

    EquipSlot(u8),

    Communication,

    /// Browsable canned-emote list under Communication; rows come from the
    /// scraped LSB emote table, Job gated on the s2c 0x11A bits.
    EmoteList,

    /// Full-screen Map screen: a bespoke two-pane layout (map + wide-scan list)
    /// rendered by `hud::map_screen`, so the generic menu is suppressed while it
    /// is on top. `active_pane` selects map-vs-list focus; the top level's
    /// `cursor` indexes the wide-scan list.
    Map,
}

#[derive(Debug, Clone)]
pub struct MenuLevel {
    pub kind: MenuKind,
    pub cursor: usize,
}

/// Retail's Command Menu draws two columns: the left holds the parent level
/// (the Root command list at depth 1), the right the current/top level (a
/// preview of the highlighted category at depth 1). `active_pane` says which
/// column the cursor keys drive.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum Pane {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Default)]
pub struct MenuStack {
    pub levels: Vec<MenuLevel>,
    pub active_pane: Pane,
}

impl MenuStack {
    pub fn root() -> Self {
        Self {
            levels: vec![MenuLevel {
                kind: MenuKind::Root,
                cursor: 0,
            }],
            active_pane: Pane::Right,
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
        self.active_pane = Pane::Right;
    }

    pub fn pop(&mut self) -> bool {
        if self.levels.len() > 1 {
            self.levels.pop();
            self.active_pane = Pane::Right;
            true
        } else {
            false
        }
    }

    pub fn toggle_pane(&mut self) {
        self.active_pane = match self.active_pane {
            Pane::Left => Pane::Right,
            Pane::Right => Pane::Left,
        };
    }

    /// Level index a pane renders. Right = the top level; Left = its parent
    /// (or, at depth 1, the Root list itself). Both collapse to level 0 for a
    /// single-level stack, so Root navigation acts on the same cursor either
    /// way while the right column shows a non-interactive preview.
    pub fn pane_level_index(&self, pane: Pane) -> usize {
        match pane {
            Pane::Right => self.levels.len().saturating_sub(1),
            Pane::Left => self.levels.len().saturating_sub(2),
        }
    }

    pub fn active_level_index(&self) -> usize {
        self.pane_level_index(self.active_pane)
    }

    pub fn active_level(&self) -> Option<&MenuLevel> {
        self.levels.get(self.active_level_index())
    }

    pub fn active_level_mut(&mut self) -> Option<&mut MenuLevel> {
        let idx = self.active_level_index();
        self.levels.get_mut(idx)
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

/// Which on-screen window the "active window" cursor (retail's Select-active-
/// window / F key) is focused on. The cycle order below matches how F steps
/// through the windows; World (unfocused) is the wrap-around resting state.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum PassiveCursorFocus {
    #[default]
    Chat,
    StatusIcons,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PassiveCursorState {
    pub focus: PassiveCursorFocus,

    /// Selection index into the status-icon ribbon while `focus == StatusIcons`.
    pub status_cursor: usize,

    /// Chat log expanded to the full-screen scrollback window (retail: confirm
    /// on the focused log window expands it, cancel contracts).
    pub chat_expanded: bool,
}

impl PassiveCursorState {
    pub fn fresh_chat() -> Self {
        Self {
            focus: PassiveCursorFocus::Chat,
            status_cursor: 0,
            chat_expanded: false,
        }
    }

    pub fn fresh_status() -> Self {
        Self {
            focus: PassiveCursorFocus::StatusIcons,
            status_cursor: 0,
            chat_expanded: false,
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
