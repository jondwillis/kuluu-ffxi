//! Vanilla contextual target-action menu — the retail-style verb list shown
//! when the operator confirms on a target.
//!
//! This is the *vanilla* surface over the shared
//! [`crate::hud::action_model`] model; the de-promoted Enhanced ring
//! ([`crate::hud::quick_action`]) is a skin over the same entries. Both
//! render exactly what [`build_target_action_entries`] +
//! [`ClientOverlay::resolve_target_actions`] produce, so the two can never
//! disagree about what a target affords.
//!
//! Shape mirrors `quick_action.rs`: a spawn-once panel
//! ([`TargetActionMenu`]) with a fixed pool of row slots
//! ([`TargetActionRow`]); an [`update_target_action_menu`] system that
//! shows/hides + relabels rows from the live [`InputMode::TargetAction`]
//! state; and hover / click systems that keep the keyboard cursor and the
//! mouse in agreement, emitting [`TargetActionActivated`] on click.
//!
//! Anchored like `quick_action` (center-bottom, above the chat panel) so
//! the operator's eye lands on the picker without crossing the world view.
//!
//! ## Select entries
//!
//! Two verbs are cyclers (`ActionEntryKind::Select`): Chat
//! (Say / Tell / Party / Linkshell / Unity / Shout) and Magic
//! (Category / Flat). The selected mode is rendered inline as
//! `Chat: Say` and right-arrow advances it. Because the entry list is
//! rebuilt from the captured `ctx` every frame, the per-entry `mode_idx`
//! the operator has cycled into is tracked separately in the input layer
//! (the contested `text_input.rs` switch owns the `Select.mode_idx`
//! mutation); this renderer reads whatever `mode_idx` the rebuilt entry
//! carries and displays it.
//!
//! ## Hints
//!
//! `ActionEntry.hint` carries contextual error/explanation strings
//! ("Target out of range.", "No spells available."). When present, the
//! row renders the hint as a dimmed suffix and the verb itself is shown
//! disabled (muted, non-cursor color) when `!entry.enabled`.
//!
//! ## Back-out
//!
//! Esc semantics live in the input layer, but the contract this renderer
//! is built against is: Esc pops `state.sub` one frame
//! ([`SubActionStack::pop`]); when the sub-stack is empty it closes the
//! menu (back to [`InputMode::World`]). When a sub-frame is active this
//! panel dims the top-level rows and surfaces a breadcrumb so the operator
//! knows a leaf list is open underneath.

use bevy::prelude::*;

use crate::hud::action_model::{ActionEntry, ActionEntryKind, TargetActionId};
use crate::hud::overlay::ActiveOverlay;
use crate::hud::palette;
use crate::input_mode::{InputMode, SubAction};

/// Maximum number of contextual verbs. The full retail set is seven
/// (Chat / Magic / Abilities / Trust / Items / Trade / Check); the panel
/// spawns this many row slots once and the renderer hides any extras
/// (`Display::None`) when the contextual list is shorter (e.g. Trust
/// hidden, or a mob target dropping the social verbs).
const MAX_ROWS: usize = 7;

/// Spawn-once panel root for the contextual target-action menu.
#[derive(Component)]
pub struct TargetActionMenu;

/// One row slot in the panel. `slot` is the fixed pool index `0..MAX_ROWS`;
/// the renderer maps it onto the current entry list each frame.
#[derive(Component)]
pub struct TargetActionRow {
    pub slot: usize,
}

/// Breadcrumb / sub-frame indicator row. Shown only while a
/// [`SubAction`] frame is active so the operator can see which leaf list
/// is open underneath the dimmed verb rows.
#[derive(Component)]
pub struct TargetActionBreadcrumb;

/// Emitted when an operator clicks (LMB-press) a target-action row. The
/// consumer in `ffxi-client/src/view_native/text_input.rs` dispatches via
/// the same path the keyboard Enter uses (push a [`SubAction`] for a leaf
/// verb, fire a `Plain` verb, or cycle a `Select` verb), so click and
/// keyboard never diverge.
#[derive(Message, Debug, Clone, Copy)]
pub struct TargetActionActivated {
    pub slot: usize,
}

/// Resolve the effective contextual entry list for the current mode +
/// active overlay. Returns `None` when the menu isn't open. Public so the
/// input router / tests can keep cursor bounds in sync with what the
/// renderer shows (same discipline as `quick_action::entries_for`).
pub fn entries_for_mode(mode: &InputMode, overlay: &ActiveOverlay) -> Option<Vec<ActionEntry>> {
    match mode {
        InputMode::TargetAction(state) => Some(overlay.0.resolve_target_actions(&state.ctx)),
        _ => None,
    }
}

/// Number of contextual entries currently shown. Mirrors
/// `quick_action::entry_count` for cursor-bound parity.
pub fn entry_count(mode: &InputMode, overlay: &ActiveOverlay) -> usize {
    entries_for_mode(mode, overlay)
        .map(|e| e.len())
        .unwrap_or(0)
}

/// Render text for one entry: the verb, plus the cycler's selected mode
/// inline for `Select` entries, plus the contextual hint as a dimmed
/// suffix. The cursor caret (`> ` / `  `) is prepended by the caller so
/// hover/keyboard share one definition of "selected".
fn entry_text(entry: &ActionEntry) -> String {
    let mut s = match &entry.kind {
        ActionEntryKind::Plain => entry.label.clone(),
        ActionEntryKind::Select { modes, mode_idx } => {
            // Show the currently-selected mode inline (`Chat: Say`). A
            // trailing chevron hints the right-arrow cycle affordance.
            let mode = modes.get(*mode_idx).copied().unwrap_or("");
            if mode.is_empty() {
                entry.label.clone()
            } else {
                format!("{}: {} >", entry.label, mode)
            }
        }
    };
    if let Some(hint) = &entry.hint {
        s.push_str("  (");
        s.push_str(hint);
        s.push(')');
    }
    s
}

/// Human-readable breadcrumb for the active sub-frame, if any.
fn breadcrumb_text(sub: Option<SubAction>) -> Option<String> {
    sub.map(|frame| match frame {
        SubAction::MagicCategory(cat) => format!("Magic / {}", cat.label()),
        SubAction::AbilitiesGroup(group) => format!("Abilities / {}", group.label()),
        SubAction::Items => "Items".to_string(),
        SubAction::ChatCompose => "Chat".to_string(),
    })
    .map(|s| format!("» {s}"))
}

/// Spawn the contextual target-action panel. Anchored like
/// [`crate::hud::quick_action::spawn_quick_action`] (center-bottom, above
/// the chat panel) but offset slightly so the two never overlap if both
/// were ever momentarily visible during the Enhanced/vanilla switch.
pub fn spawn_target_action_menu(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            TargetActionMenu,
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(250.0),
                left: Val::Percent(50.0),
                margin: UiRect {
                    left: Val::Px(-90.0),
                    ..default()
                },
                width: Val::Px(180.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                display: Display::None,
                ..default()
            },
            // Match quick_action's z so neither buries the other; both sit
            // above chat panel + minimap.
            ZIndex(20),
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|p| {
            // Breadcrumb row (hidden unless a sub-frame is active).
            p.spawn((
                TargetActionBreadcrumb,
                Node {
                    display: Display::None,
                    ..default()
                },
                Text::new(""),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(palette::MUTED),
            ));
            for slot in 0..MAX_ROWS {
                p.spawn((
                    TargetActionRow { slot },
                    // Bevy UI Interaction → cursor swap + click dispatch.
                    Button,
                    Node::default(),
                    Text::new(""),
                    TextFont {
                        font_size: 13.0,
                        ..default()
                    },
                    TextColor(palette::MUTED),
                ));
            }
        });
}

/// Show/hide + relabel the contextual rows from the live
/// [`InputMode::TargetAction`] state. Mirrors
/// [`crate::hud::quick_action::update_quick_action`]: the panel is shown
/// only while the mode is active, each in-range row renders its entry
/// (with cursor caret, Select mode, and hint), and surplus slots are
/// `Display::None`. While a sub-frame is active the top-level rows are
/// dimmed and the breadcrumb is shown.
pub fn update_target_action_menu(
    mode: Res<InputMode>,
    overlay: Res<ActiveOverlay>,
    // Needed for the `AbilitiesGroup` leaf frame, which renders rows built
    // from the live snapshot's known-ability vectors (mirrors
    // status_panel / check_view, which also take `Res<SceneState>`).
    scene: Res<crate::snapshot::SceneState>,
    mut panel_q: Query<&mut Node, With<TargetActionMenu>>,
    mut row_q: Query<
        (&TargetActionRow, &mut Node, &mut Text, &mut TextColor),
        (Without<TargetActionMenu>, Without<TargetActionBreadcrumb>),
    >,
    mut crumb_q: Query<
        (&mut Node, &mut Text, &mut TextColor),
        (With<TargetActionBreadcrumb>, Without<TargetActionMenu>),
    >,
) {
    let Ok(mut panel) = panel_q.single_mut() else {
        return;
    };

    let InputMode::TargetAction(state) = &*mode else {
        if panel.display != Display::None {
            panel.display = Display::None;
        }
        return;
    };

    panel.display = Display::Flex;

    // A sub-frame open underneath dims the verb rows (the leaf list is the
    // active surface) and shows the breadcrumb.
    let sub_active = state.sub.as_ref().and_then(|s| s.current());
    if let Ok((mut node, mut text, mut color)) = crumb_q.single_mut() {
        match breadcrumb_text(sub_active) {
            Some(crumb) => {
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
                if **text != crumb {
                    **text = crumb;
                }
                let want = palette::ACCENT;
                if color.0 != want {
                    color.0 = want;
                }
            }
            None => {
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
        }
    }

    let mut entries = overlay.0.resolve_target_actions(&state.ctx);
    // The entry list is rebuilt fresh each frame (mode_idx == 0), so mirror
    // the chat send-mode the input router has cycled into onto the Chat
    // entry — otherwise the inline `Chat: Say` text never advances.
    for entry in entries.iter_mut() {
        // Both Chat and Abilities are `Select` cyclers whose index lives in
        // the persistent state (the rebuilt entry always carries
        // `mode_idx == 0`); re-apply each so the inline `X: Y` text advances.
        let cycled = match entry.id {
            TargetActionId::Chat => Some(state.chat_mode_idx),
            TargetActionId::Abilities => Some(state.abilities_group_idx),
            _ => None,
        };
        if let Some(idx) = cycled {
            if let ActionEntryKind::Select { modes, mode_idx } = &mut entry.kind {
                if !modes.is_empty() {
                    *mode_idx = idx % modes.len();
                }
            }
        }
    }
    let cursor = state.cursor;

    // While an Abilities group is pushed, the slot pool renders that group's
    // leaf list (or its contextual error) instead of the dimmed top-level
    // verbs — the leaf is the active surface and owns the cursor.
    if let Some(SubAction::AbilitiesGroup(group)) = sub_active {
        let rows = crate::hud::menu::ability_group_rows(&scene.snapshot, group);
        let sub_cursor = state.sub.as_ref().map(|s| s.cursor).unwrap_or(0);
        for (row, mut node, mut text, mut color) in row_q.iter_mut() {
            let (want, want_color) = if rows.is_empty() {
                // Empty group: the contextual error occupies the single
                // first slot; the rest are hidden.
                if row.slot == 0 {
                    (
                        format!("  {}", crate::hud::menu::ability_group_empty_hint(group)),
                        palette::MUTED,
                    )
                } else {
                    if node.display != Display::None {
                        node.display = Display::None;
                    }
                    continue;
                }
            } else if let Some(leaf) = rows.get(row.slot) {
                let is_cursor = row.slot == sub_cursor;
                let caret = if is_cursor { "> " } else { "  " };
                let color = if is_cursor {
                    palette::ACCENT
                } else {
                    palette::TEXT
                };
                (format!("{caret}{}", leaf.label), color)
            } else {
                if node.display != Display::None {
                    node.display = Display::None;
                }
                continue;
            };
            if node.display != Display::Flex {
                node.display = Display::Flex;
            }
            if **text != want {
                **text = want;
            }
            if color.0 != want_color {
                color.0 = want_color;
            }
        }
        return;
    }

    for (row, mut node, mut text, mut color) in row_q.iter_mut() {
        match entries.get(row.slot) {
            Some(entry) => {
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
                let is_cursor = row.slot == cursor && sub_active.is_none();
                let caret = if is_cursor { "> " } else { "  " };
                let want = format!("{caret}{}", entry_text(entry));
                if **text != want {
                    **text = want;
                }
                // Color precedence: disabled/out-of-range verbs are always
                // muted (contextual hint already explains why); the cursor
                // row is accented; a sub-frame dims everything; otherwise
                // text color.
                let want_color = if !entry.enabled {
                    palette::DARK
                } else if sub_active.is_some() {
                    palette::MUTED
                } else if is_cursor {
                    palette::ACCENT
                } else {
                    palette::TEXT
                };
                if color.0 != want_color {
                    color.0 = want_color;
                }
            }
            None => {
                // Contextual list shorter than MAX_ROWS — hide the slot
                // completely (empty Text nodes still consume row_gap).
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
        }
    }
}

/// Move the cursor to follow mouse hover so keyboard-Enter and mouse-click
/// agree on which verb fires. Only runs while
/// [`InputMode::TargetAction`] is active and no sub-frame is open (the
/// leaf list owns the cursor while pushed). Mirrors
/// [`crate::hud::quick_action::quick_action_mouse_hover_system`].
pub fn target_action_mouse_hover_system(
    mut mode: ResMut<InputMode>,
    overlay: Res<ActiveOverlay>,
    rows: Query<(&Interaction, &TargetActionRow), Changed<Interaction>>,
) {
    // Read the bound before borrowing `mode` mutably (overlay + ctx).
    let limit = entry_count(&mode, &overlay);
    let InputMode::TargetAction(state) = &mut *mode else {
        return;
    };
    // While a sub-frame is open the top-level rows are inert.
    if state.sub.as_ref().and_then(|s| s.current()).is_some() {
        return;
    }
    for (interaction, row) in &rows {
        if matches!(interaction, Interaction::Hovered | Interaction::Pressed)
            && row.slot < limit
            && state.cursor != row.slot
        {
            state.cursor = row.slot;
        }
    }
}

/// Emit [`TargetActionActivated`] on row click. Filtered to in-bounds
/// slots (the spawn pool is `MAX_ROWS`, but the contextual list may be
/// shorter) and suppressed while a sub-frame is open. Mirrors
/// [`crate::hud::quick_action::quick_action_mouse_click_system`].
pub fn target_action_mouse_click_system(
    mode: Res<InputMode>,
    overlay: Res<ActiveOverlay>,
    rows: Query<(&Interaction, &TargetActionRow), Changed<Interaction>>,
    mut out: MessageWriter<TargetActionActivated>,
) {
    let limit = entry_count(&mode, &overlay);
    let InputMode::TargetAction(state) = &*mode else {
        return;
    };
    if state.sub.as_ref().and_then(|s| s.current()).is_some() {
        return;
    }
    for (interaction, row) in &rows {
        if *interaction == Interaction::Pressed && row.slot < limit {
            out.write(TargetActionActivated { slot: row.slot });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::action_model::{
        build_target_action_entries, TargetActionContext, TargetActionId, TargetKindLite,
    };
    use crate::hud::overlay::RETAIL;
    use crate::input_mode::TargetActionState;

    fn npc_ctx(in_range: bool) -> TargetActionContext {
        TargetActionContext {
            has_target: true,
            target_kind: TargetKindLite::Npc,
            in_range,
            trusts_available: false,
        }
    }

    #[test]
    fn closed_mode_yields_no_entries() {
        let overlay = ActiveOverlay(&RETAIL);
        assert!(entries_for_mode(&InputMode::World, &overlay).is_none());
        assert_eq!(entry_count(&InputMode::World, &overlay), 0);
    }

    #[test]
    fn open_mode_yields_contextual_entries() {
        let overlay = ActiveOverlay(&RETAIL);
        let mode = InputMode::TargetAction(TargetActionState::open(npc_ctx(true)));
        let entries = entries_for_mode(&mode, &overlay).expect("menu open");
        assert!(!entries.is_empty());
        // NPC in range: Chat is contextual and enabled.
        assert!(entries
            .iter()
            .any(|e| e.id == TargetActionId::Chat && e.enabled));
    }

    #[test]
    fn out_of_range_npc_disables_chat_with_hint() {
        let overlay = ActiveOverlay(&RETAIL);
        let mode = InputMode::TargetAction(TargetActionState::open(npc_ctx(false)));
        let entries = entries_for_mode(&mode, &overlay).expect("menu open");
        let chat = entries
            .iter()
            .find(|e| e.id == TargetActionId::Chat)
            .expect("chat present");
        assert!(!chat.enabled);
        assert!(chat.hint.is_some());
    }

    #[test]
    fn select_entry_renders_mode_inline() {
        // Chat is a Select cycler; its text should show the active mode.
        let overlay = ActiveOverlay(&RETAIL);
        let entries = build_target_action_entries(&npc_ctx(true), overlay.0);
        let chat = entries
            .iter()
            .find(|e| e.id == TargetActionId::Chat)
            .expect("chat present");
        let text = entry_text(chat);
        assert!(text.starts_with("Chat: "));
        // Right-arrow affordance chevron.
        assert!(text.contains('>'));
    }

    #[test]
    fn plain_entry_renders_bare_label() {
        let overlay = ActiveOverlay(&RETAIL);
        let entries = build_target_action_entries(&npc_ctx(true), overlay.0);
        let items = entries
            .iter()
            .find(|e| e.id == TargetActionId::Items)
            .expect("items present");
        assert_eq!(entry_text(items), "Items");
    }

    #[test]
    fn hint_renders_as_suffix() {
        let overlay = ActiveOverlay(&RETAIL);
        let entries = build_target_action_entries(&npc_ctx(false), overlay.0);
        let chat = entries
            .iter()
            .find(|e| e.id == TargetActionId::Chat)
            .expect("chat present");
        let text = entry_text(chat);
        assert!(text.contains("(Target out of range.)"));
    }

    #[test]
    fn breadcrumb_only_for_active_sub_frame() {
        assert!(breadcrumb_text(None).is_none());
        let crumb = breadcrumb_text(Some(SubAction::Items)).expect("crumb");
        assert!(crumb.contains("Items"));
    }
}
