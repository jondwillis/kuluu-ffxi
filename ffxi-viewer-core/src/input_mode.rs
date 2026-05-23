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
///
/// # Per-mode behavior matrix
///
/// |              | Movement | Camera arrows | Click→target | Esc            |
/// |--------------|----------|---------------|--------------|----------------|
/// | World        | Yes      | Yes           | Yes          | Clear target   |
/// | Chat         | Paused   | Paused        | No           | Clear/exit     |
/// | Menu         | Walks    | Suppressed    | No           | Pop level/exit |
/// | QuickAction  | Walks    | Suppressed    | No           | Exit to World  |
/// | Dialog       | Paused   | Paused        | No           | Skip/exit      |
/// | PassiveCursor| Walks    | Suppressed    | No           | Exit to World  |
///
/// PassiveCursor matches Menu/QuickAction's "stay walkable" shape on
/// purpose: retail FFXI lets you autorun while scrolling chat, and
/// pausing here would kill autorun on every chat scroll.
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
    /// An NPC event dialog is on screen. Up/Down moves the choice cursor
    /// `0..=DIALOG_MAX_CHOICE`; Enter dispatches `EndEventChoice` with the
    /// chosen `EndPara`; Esc sends `EndEvent` (which uses choice 0). The
    /// router pushes this mode whenever `SceneState.snapshot.dialog` is
    /// `Some` and pops back to `World` when it clears.
    Dialog(DialogCursor),
    /// FFXI-style passive cursor: a HUD panel (currently chat) has focus
    /// and arrow keys scroll/navigate it instead of driving the camera.
    /// Movement still works — operator can autorun while scrolling chat
    /// log. Toggled in/out via `Action::TogglePassiveCursor` (default
    /// Insert) or `NavCancel` (Esc).
    PassiveCursor(PassiveCursorState),
}

/// Cap on the dialog choice cursor. FFXI events that take a numeric
/// EndPara almost always operate in 0..7 (`/buy /sell /trade /quit` shop,
/// 0..3 for Mog House menus, etc.) — eight rows is plenty without
/// scrolling the picker. Operators picking an out-of-range value can
/// always retry; the server treats out-of-bound EndPara as
/// "cancel/default" anyway.
pub const DIALOG_MAX_CHOICE: u32 = 7;

/// Currently-selected event choice. Lives inside `InputMode::Dialog` so
/// the cursor and the "dialog is open" flag can never desync.
#[derive(Debug, Clone, Copy, Default)]
pub struct DialogCursor {
    pub cursor: u32,
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

/// Identifies which screen of the menu tree we're currently on. The set
/// grows as submenus are wired; today's submenus are `Config` (keybind
/// presets), `Graphics` (quality knobs), and the four retail-style
/// action menus (Magic / Abilities / Items / Equipment) — see plan
/// `let-s-work-on-hooking-dynamic-backus.md` for staging.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MenuKind {
    Root,
    /// The keybind-config submenu: lists presets + a "show current
    /// bindings" entry. Selecting a preset is equivalent to running
    /// `/keybinds preset <name>`; selecting Reset is `/keybinds reset`;
    /// selecting List is `/keybinds list`. In-game keypress-capture
    /// rebinding is intentionally NOT here yet — slash-only for now.
    Config,
    /// Graphics quality knobs. Rows are individual fields (shadow size,
    /// AA mode, etc.); Up/Down moves the cursor, Left/Right cycles the
    /// highlighted row's value. The mapping from row index → field +
    /// the cycle dispatcher both live in `text_input::handle_menu_key`
    /// / `resolve_menu_entry`.
    Graphics,
    /// Retail "Magic" submenu — lists spells the character has learned.
    /// Stage 0: placeholder; Stage 2 populates from a decoded
    /// `spells_learned` bitmap and Enter dispatches `ActionKind::CastMagic`.
    Magic,
    /// Retail "Abilities" submenu — lists job abilities currently
    /// available (intersected with the s2c 0x119 recast snapshot).
    /// Stage 0: placeholder; Stage 2 wires data + `ActionKind::JobAbility`.
    Abilities,
    /// Retail "Items" submenu — lists usable items from the main
    /// Inventory bag. Stage 0: placeholder; Stage 3 populates from
    /// `SessionState.inventory` and dispatches `ActionKind::UseItem`.
    Items,
    /// Retail "Equipment" submenu — shows the 16 equipped slots.
    /// Stage 0: placeholder; Stage 1 wires a new s2c 0x050 decoder so
    /// rows reflect actual equipped items; Stage 4 turns each slot row
    /// into a "pick from inventory" sub-submenu.
    Equipment,
    /// Stage-4 sub-submenu pushed when an operator presses Enter on a
    /// row in the Equipment menu. The contained byte is the SLOTTYPE
    /// id (0=Main..15=Back) that's being filled — `refresh_dynamic_menu_rows`
    /// filters the inventory bag by `equip_info::fits_slot` + job +
    /// level so the rows only show items the operator can actually
    /// equip there. Selecting a row dispatches `AgentCommand::Equip`;
    /// Esc pops back to the Equipment menu.
    EquipSlot(u8),
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

/// Quick-action picker state.
///
/// `has_target` is captured at the moment the picker opens; it decides
/// whether the entry list shows target-relevant verbs (Attack/Check/Talk)
/// or no-target stubs (Magic/Items/Macros). Capturing it once at open
/// time means the entry list won't shift mid-navigation if a server
/// snapshot drops the target between keypresses.
#[derive(Debug, Clone, Default)]
pub struct QuickActionState {
    pub cursor: usize,
    pub has_target: bool,
}

impl QuickActionState {
    /// Open the picker with knowledge of whether a target is selected.
    pub fn for_target(has_target: bool) -> Self {
        Self {
            cursor: 0,
            has_target,
        }
    }
}

/// Which HUD panel currently has the passive cursor's focus. The first
/// (and only) entry is Chat; future variants (Party, Inventory, …) can
/// be added when the corresponding HUD grows arrow-key-navigable
/// elements.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum PassiveCursorFocus {
    #[default]
    Chat,
}

/// Passive-cursor state. The chat-scroll offset itself lives in the
/// stand-alone [`crate::hud::chat_panel::ChatScroll`] resource so that
/// mouse-wheel scrolling can drive it from any mode — not just while
/// PassiveCursor is active. This struct is now just a focus marker.
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
