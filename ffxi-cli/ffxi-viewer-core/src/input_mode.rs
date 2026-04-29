//! Single source of truth for "where is keyboard input going right now?"
//!
//! The native viewer's existing input pipeline ([`crate::scene`] /
//! `view_native::input`) hard-codes camera-driven movement: every key press
//! goes straight to character control. Once chat, menu, or quick-action UIs
//! exist, those same keys need to route to text buffers / cursors instead —
//! and movement must pause so the player doesn't walk into a wall while
//! typing "hello".
//!
//! [`InputMode`] is the central enum every input-consuming system reads.
//! `World` is the default and preserves today's behavior. The other
//! variants both *carry* their UI state (chat buffer, menu cursor stack)
//! and *gate* the world systems (which early-return when not in `World`).
//!
//! Bundling the UI state into the variant means every reader sees a
//! consistent snapshot — there's no chance the menu cursor and the
//! "menu is open" flag desync.

use bevy::prelude::Resource;

/// Top-level input focus. `World` is the default; the other variants own
/// the UI state for their respective overlays.
#[derive(Resource, Debug, Clone, Default)]
pub enum InputMode {
    /// Camera and movement keys drive the character. The chat / menu / QA
    /// HUDs are hidden.
    #[default]
    World,
    /// A chat input bar is open at the bottom of the chat panel. Keystrokes
    /// append to the buffer; Enter submits, Esc clears-or-closes.
    Chat(ChatBuffer),
    /// The minus-key main menu is open. Up/Down moves the cursor on the
    /// current level; Enter selects (push a submenu or fire an action);
    /// Esc pops a level (or exits to `World` on the root).
    Menu(MenuStack),
    /// The Enter-from-rest quick-action picker is open. Up/Down moves the
    /// cursor; Enter selects; Esc returns to `World`.
    QuickAction(QuickActionState),
}

/// In-progress chat text. Submission semantics live in the keyboard handler
/// (a `/`-prefix is parsed as a slash command; otherwise the buffer is sent
/// as a Say chat line).
#[derive(Debug, Clone, Default)]
pub struct ChatBuffer {
    /// The raw text the user has typed so far, including any leading `/`.
    pub text: String,
}

impl ChatBuffer {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Start a buffer pre-seeded with `prefix`. Used to enter chat mode via
    /// the `/` key, where the prefix is itself the first character of the
    /// command the user is about to type.
    pub fn with_prefix(prefix: &str) -> Self {
        Self {
            text: prefix.to_string(),
        }
    }
}

/// Identifies which screen of the menu tree we're currently on. The set is
/// intentionally tiny right now — this stage scaffolds the input plumbing,
/// not the full menu content. Submenu variants land alongside the data
/// mirrors that populate them.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MenuKind {
    Root,
}

/// One frame of the menu navigation stack. `cursor` is the row currently
/// highlighted on this level; preserved when a submenu is pushed so popping
/// back restores where the user was.
#[derive(Debug, Clone)]
pub struct MenuLevel {
    pub kind: MenuKind,
    pub cursor: usize,
}

/// The full navigation stack. The bottom of the stack is the root menu;
/// pushing means "open a submenu", popping means "back". Popping the root
/// exits menu mode entirely (handled by the keyboard layer, not here).
#[derive(Debug, Clone, Default)]
pub struct MenuStack {
    pub levels: Vec<MenuLevel>,
}

impl MenuStack {
    /// Fresh stack at the root.
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

    /// Pop one level. Returns `true` if a level was popped, `false` if we
    /// were already on the root (in which case the caller should exit to
    /// `InputMode::World`).
    pub fn pop(&mut self) -> bool {
        if self.levels.len() > 1 {
            self.levels.pop();
            true
        } else {
            false
        }
    }
}

/// Quick-action picker state — just a cursor index. The entry list is
/// fixed (defined in `hud::quick_action`) so we don't need to carry it.
#[derive(Debug, Clone, Default)]
pub struct QuickActionState {
    pub cursor: usize,
}
