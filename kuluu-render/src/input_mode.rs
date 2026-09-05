use std::collections::VecDeque;

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

    /// The /check window owns input: arrows move its equipment grid, confirm on
    /// View Wares opens the target's bazaar, Esc closes. Cursor state lives in
    /// `hud::check_view::CheckTarget`.
    Check,

    /// A browsed bazaar's wares list. Cursor/quantity state lives in
    /// `hud::bazaar_view::BazaarScreenState`; Esc leaves the bazaar (c2s 0x104)
    /// and returns to the Check window.
    Bazaar,

    /// The Auction House counter UI is open and modal. Screen/cursor state
    /// lives in `hud::auction::AuctionScreenState`; the native input layer
    /// drives it and emits the Ah* `AgentCommand`s.
    Auction,
}

/// The action pending behind a sub-target cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubTargetAction {
    Spell(u16),
    Ability(u16),
    WeaponSkill(u16),
    /// Ranged attack — targets an enemy, carries no id.
    Ranged,
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

    /// How far back in [`ChatHistory`] Up/Down has paged. `None` is the fresh
    /// line being typed; `Some(0)` is the most recent submission.
    pub history_pos: Option<usize>,

    /// The line that was being typed when paging into history started, restored
    /// when Down walks back past the newest entry.
    pub draft: Option<String>,
}

impl ChatBuffer {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with_prefix(prefix: &str) -> Self {
        Self {
            text: prefix.to_string(),
            ..Self::default()
        }
    }

    /// Page one entry further back. No-op at the oldest entry or on an empty
    /// history.
    pub fn recall_older(&mut self, history: &ChatHistory) {
        let next = match self.history_pos {
            None => 0,
            Some(pos) => pos + 1,
        };
        let Some(entry) = history.get(next) else {
            return;
        };
        if self.history_pos.is_none() {
            self.draft = Some(std::mem::take(&mut self.text));
        }
        self.text = entry.to_string();
        self.history_pos = Some(next);
    }

    /// Page one entry forward, landing back on the stashed draft past the
    /// newest entry. No-op when not paging.
    pub fn recall_newer(&mut self, history: &ChatHistory) {
        let Some(pos) = self.history_pos else {
            return;
        };
        match pos
            .checked_sub(1)
            .and_then(|next| history.get(next).map(|entry| (next, entry.to_string())))
        {
            Some((next, entry)) => {
                self.text = entry;
                self.history_pos = Some(next);
            }
            None => {
                self.text = self.draft.take().unwrap_or_default();
                self.history_pos = None;
            }
        }
    }
}

/// Lines the player has submitted from the chat bar, newest first. Kept as a
/// resource so it outlives the per-open [`ChatBuffer`].
#[derive(Resource, Debug, Clone, Default)]
pub struct ChatHistory {
    entries: VecDeque<String>,
}

/// Retail's text input recalls a bounded number of past lines rather than the
/// whole session; 32 covers a fight's worth of commands without unbounded growth.
pub const CHAT_HISTORY_MAX: usize = 32;

impl ChatHistory {
    /// `index` 0 is the most recent submission.
    pub fn get(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Records a submitted line. Blank lines and an immediate repeat of the
    /// newest entry are dropped so paging stays useful when a command is spammed.
    pub fn push(&mut self, line: &str) {
        if line.trim().is_empty() || self.entries.front().is_some_and(|prev| prev == line) {
            return;
        }
        self.entries.push_front(line.to_string());
        self.entries.truncate(CHAT_HISTORY_MAX);
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MenuKind {
    Root,

    Config,

    Debug,

    Graphics,

    /// DLSS Config submenu pushed from the Graphics list's "DLSS Config" row
    /// (hud::menu::GRAPHICS_DLSS_CONFIG_SLOT): the quality tier plus the inert
    /// RenoDX-parity placeholder rows.
    GraphicsDlss,

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

    /// Full-screen Map screen rendered by `hud::map_screen`: a full-screen map
    /// with a top-right command submenu (Markers / Wide Scan / Change Map). The
    /// stack only carries this level; the submode + cursor live in the bespoke
    /// `hud::map_screen::MapScreenState`, so the generic menu is suppressed while
    /// it is on top.
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
    /// The `-` keypress that opens the menu (Action::OpenMenu, handled in the
    /// input system that runs just before the text handler) also reaches the
    /// menu's `-` page-flip on the same frame. This absorbs exactly that opening
    /// press so the Commands menu lands on page 1, not page 2 (kuluu-bi1s.2).
    pub absorb_open_minus: bool,
}

impl MenuStack {
    pub fn root() -> Self {
        Self {
            levels: vec![MenuLevel {
                kind: MenuKind::Root,
                cursor: 0,
            }],
            active_pane: Pane::Right,
            absorb_open_minus: true,
        }
    }

    /// Consume the one-shot open-`-` absorb flag; returns true the first time
    /// (the opening frame) and false thereafter.
    pub fn take_absorb_open_minus(&mut self) -> bool {
        std::mem::take(&mut self.absorb_open_minus)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absorb_open_minus_fires_once_then_clears() {
        let mut stack = MenuStack::root();
        assert!(
            stack.take_absorb_open_minus(),
            "opening frame absorbs the '-'"
        );
        assert!(
            !stack.take_absorb_open_minus(),
            "subsequent '-' presses flip pages normally"
        );
    }

    #[test]
    fn pushed_stack_does_not_absorb() {
        let mut stack = MenuStack::default();
        assert!(!stack.take_absorb_open_minus());
    }

    fn history_of(lines: &[&str]) -> ChatHistory {
        let mut history = ChatHistory::default();
        for line in lines {
            history.push(line);
        }
        history
    }

    #[test]
    fn history_pages_newest_first_and_clamps_at_the_oldest() {
        let history = history_of(&["/heal", "hello", "/tell Zilart hi"]);
        let mut buffer = ChatBuffer::empty();

        buffer.recall_older(&history);
        assert_eq!(buffer.text, "/tell Zilart hi");
        buffer.recall_older(&history);
        assert_eq!(buffer.text, "hello");
        buffer.recall_older(&history);
        assert_eq!(buffer.text, "/heal");

        buffer.recall_older(&history);
        assert_eq!(buffer.text, "/heal", "oldest entry holds");
    }

    #[test]
    fn paging_forward_restores_the_stashed_draft() {
        let history = history_of(&["/heal"]);
        let mut buffer = ChatBuffer::empty();
        buffer.text = "half typed".into();

        buffer.recall_older(&history);
        assert_eq!(buffer.text, "/heal");

        buffer.recall_newer(&history);
        assert_eq!(buffer.text, "half typed");
        assert_eq!(buffer.history_pos, None);

        buffer.recall_newer(&history);
        assert_eq!(buffer.text, "half typed", "already at the fresh line");
    }

    #[test]
    fn empty_history_leaves_the_line_alone() {
        let history = ChatHistory::default();
        let mut buffer = ChatBuffer::empty();
        buffer.text = "typing".into();

        buffer.recall_older(&history);
        buffer.recall_newer(&history);

        assert_eq!(buffer.text, "typing");
        assert_eq!(buffer.history_pos, None);
        assert_eq!(buffer.draft, None);
    }

    #[test]
    fn blank_and_repeated_lines_are_not_recorded() {
        let history = history_of(&["/heal", "/heal", "   ", ""]);
        assert_eq!(history.len(), 1);
        assert_eq!(history.get(0), Some("/heal"));
    }

    #[test]
    fn history_is_capped_at_the_max() {
        let lines: Vec<String> = (0..CHAT_HISTORY_MAX + 8)
            .map(|i| format!("/l {i}"))
            .collect();
        let mut history = ChatHistory::default();
        for line in &lines {
            history.push(line);
        }

        assert_eq!(history.len(), CHAT_HISTORY_MAX);
        assert_eq!(history.get(0).map(str::to_string), lines.last().cloned());
        assert_eq!(history.get(CHAT_HISTORY_MAX), None);
    }
}
