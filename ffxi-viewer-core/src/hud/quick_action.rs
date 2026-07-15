use bevy::prelude::*;

use crate::hud::style::{self, theme};
use crate::input_mode::InputMode;

const ENTRIES_TARGETED: &[&str] = &["Attack", "Check", "Talk"];

const ENTRIES_UNTARGETED: &[&str] = &["Magic", "Abilities", "Items", "Macros"];

const MAX_ROWS: usize = 4;

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

#[derive(Message, Debug, Clone, Copy)]
pub struct QuickActionActivated {
    pub slot: usize,
}

pub fn spawn_quick_action(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            QuickActionPanel,
            Node {
                position_type: PositionType::Absolute,

                bottom: Val::Px(250.0),
                left: Val::Percent(50.0),
                margin: UiRect {
                    left: Val::Px(-80.0),
                    ..default()
                },
                width: Val::Px(160.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                display: Display::None,
                ..default()
            },
            ZIndex(20),
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
        ))
        .with_children(|p| {
            for slot in 0..MAX_ROWS {
                p.spawn((
                    QuickActionRow { slot },
                    Button,
                    Node::default(),
                    Text::new(""),
                    style::text_font(13.0),
                    TextColor(theme::MUTED),
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
                            theme::CURSOR
                        } else {
                            theme::MUTED
                        };
                        if color.0 != want_color {
                            color.0 = want_color;
                        }
                    }
                    None => {
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

pub fn quick_action_mouse_hover_system(
    mut mode: ResMut<InputMode>,
    rows: Query<(&Interaction, &QuickActionRow), Changed<Interaction>>,
) {
    let InputMode::QuickAction(state) = &mut *mode else {
        return;
    };
    let limit = entry_count(state.has_target);
    for (interaction, row) in &rows {
        if matches!(interaction, Interaction::Hovered | Interaction::Pressed)
            && row.slot < limit
            && state.cursor != row.slot
        {
            state.cursor = row.slot;
        }
    }
}

pub fn quick_action_mouse_click_system(
    mode: Res<InputMode>,
    rows: Query<(&Interaction, &QuickActionRow), Changed<Interaction>>,
    mut out: MessageWriter<QuickActionActivated>,
) {
    let InputMode::QuickAction(state) = &*mode else {
        return;
    };
    let limit = entry_count(state.has_target);
    for (interaction, row) in &rows {
        if *interaction == Interaction::Pressed && row.slot < limit {
            out.write(QuickActionActivated { slot: row.slot });
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
        let labels: Vec<&str> = entries_for(true).to_vec();
        assert!(labels.contains(&"Check"));
        assert!(labels.contains(&"Talk"));
    }

    #[test]
    fn untargeted_excludes_attack() {
        let labels: Vec<&str> = entries_for(false).to_vec();
        assert!(!labels.contains(&"Attack"));
        assert!(!labels.contains(&"Check"));
    }

    #[test]
    fn out_of_range_returns_sentinel() {
        assert_eq!(entry_label(true, 999), "<unknown>");
    }
}
