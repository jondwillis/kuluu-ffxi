//! Quick-action picker — FFXI-classic action ring.
//!
//! Anchored bottom-left, sitting just above the chat panel. The entry
//! list is target-aware: with a target the ring shows the verbs the
//! operator is most likely to want (Attack/Check/Talk); with no target
//! it falls back to the standard quick-access stubs (Magic/Items/Macros).
//! Capturing `has_target` once at open time (see
//! [`crate::input_mode::QuickActionState::for_target`]) means the list
//! is stable through navigation.
//!
//! Cursor wraps at top/bottom — Up at row 0 jumps to the last row,
//! Down at the last row returns to row 0. Matches retail and avoids
//! the dead-zone feel of clamped cursors on a 3-entry ring.

use bevy::prelude::*;

use crate::hud::palette;
use crate::input_mode::InputMode;

/// Action verbs shown when the player has a target. Order is rough
/// retail-action-ring priority: combat verb → information verb →
/// social verb. Trade is intentionally absent — it goes through 0x036
/// TRADE_REQUEST, not the Action surface, and isn't wired yet.
const ENTRIES_TARGETED: &[&str] = &["Attack", "Check", "Talk"];

/// Stub entries shown with no target — the player still needs *some*
/// context to land on, but these are placeholders for spell/item
/// submenus that aren't built yet.
const ENTRIES_UNTARGETED: &[&str] = &["Macros", "Magic", "Items"];

/// Maximum row count across both lists. The panel spawns this many row
/// slots once and the renderer hides any extras (`Display::None`) when
/// the active list is shorter.
const MAX_ROWS: usize = 3;

/// Pick the entries list to display based on whether a target is
/// selected. Public so callers (the input router, tests) can keep
/// cursor bounds in sync.
pub fn entries_for(has_target: bool) -> &'static [&'static str] {
    if has_target {
        ENTRIES_TARGETED
    } else {
        ENTRIES_UNTARGETED
    }
}

pub fn entry_count(has_target: bool) -> usize {
    entries_for(has_target).len()
}

pub fn entry_label(has_target: bool, idx: usize) -> &'static str {
    entries_for(has_target)
        .get(idx)
        .copied()
        .unwrap_or("<unknown>")
}

#[derive(Component)]
pub struct QuickActionPanel;

#[derive(Component)]
pub struct QuickActionRow {
    pub slot: usize,
}

pub fn spawn_quick_action(mut commands: Commands) {
    commands
        .spawn((
            QuickActionPanel,
            Node {
                position_type: PositionType::Absolute,
                // Anchored bottom-left, sitting just above the chat
                // panel. Chat occupies bottom 0..214 (54 + 160). 220 px
                // leaves a small breathing gap; the operator's eye
                // tracks both panels in the same screen quadrant
                // without crossing the world view.
                bottom: Val::Px(220.0),
                left: Val::Px(0.0),
                width: Val::Px(160.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|p| {
            for slot in 0..MAX_ROWS {
                p.spawn((
                    QuickActionRow { slot },
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

pub fn update_quick_action(
    mode: Res<InputMode>,
    mut panel_q: Query<&mut Node, With<QuickActionPanel>>,
    mut row_q: Query<
        (&QuickActionRow, &mut Node, &mut Text, &mut TextColor),
        Without<QuickActionPanel>,
    >,
) {
    let Ok(mut panel) = panel_q.single_mut() else {
        return;
    };

    let active = match &*mode {
        InputMode::QuickAction(state) => Some((state.cursor, state.has_target)),
        _ => None,
    };

    match active {
        Some((cursor, has_target)) => {
            panel.display = Display::Flex;
            let entries = entries_for(has_target);
            for (row, mut node, mut text, mut color) in row_q.iter_mut() {
                let label = entries.get(row.slot).copied();
                match label {
                    Some(label) => {
                        if node.display != Display::Flex {
                            node.display = Display::Flex;
                        }
                        let is_cursor = row.slot == cursor;
                        let want = if is_cursor {
                            format!("> {label}")
                        } else {
                            format!("  {label}")
                        };
                        if **text != want {
                            **text = want;
                        }
                        let want_color = if is_cursor {
                            palette::ACCENT
                        } else {
                            palette::MUTED
                        };
                        if color.0 != want_color {
                            color.0 = want_color;
                        }
                    }
                    None => {
                        // Entry list shorter than MAX_ROWS — hide the
                        // extra slot completely (don't just blank the
                        // text — empty Text nodes still consume row_gap).
                        if node.display != Display::None {
                            node.display = Display::None;
                        }
                    }
                }
            }
        }
        None => {
            if panel.display != Display::None {
                panel.display = Display::None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targeted_list_starts_with_attack() {
        assert_eq!(entry_label(true, 0), "Attack");
    }

    #[test]
    fn targeted_includes_check_and_talk() {
        let labels: Vec<&str> = entries_for(true).iter().copied().collect();
        assert!(labels.contains(&"Check"));
        assert!(labels.contains(&"Talk"));
    }

    #[test]
    fn untargeted_excludes_attack() {
        let labels: Vec<&str> = entries_for(false).iter().copied().collect();
        assert!(!labels.contains(&"Attack"));
        assert!(!labels.contains(&"Check")); // Check requires a target.
    }

    #[test]
    fn out_of_range_returns_sentinel() {
        assert_eq!(entry_label(true, 999), "<unknown>");
    }
}
