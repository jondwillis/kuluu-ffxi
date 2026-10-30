use bevy::picking::events::PointerPress;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::Button;

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
                    Hovered::default(),
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
                        let want = format!("{}{label}", style::cursor_prefix(is_cursor));
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
    rows: Query<(&Hovered, &QuickActionRow), Changed<Hovered>>,
) {
    let InputMode::QuickAction(state) = &mut *mode else {
        return;
    };
    let limit = entry_count(state.has_target);
    for (interaction, row) in &rows {
        if interaction.0 && row.slot < limit && state.cursor != row.slot {
            state.cursor = row.slot;
        }
    }
}

pub fn quick_action_mouse_click_system(
    mode: Res<InputMode>,
    rows: Query<&QuickActionRow>,
    mut out: MessageWriter<QuickActionActivated>,
    mut presses: MessageReader<PointerPress>,
    parents: Query<&ChildOf>,
) {
    let pressed_rows = super::primary_pressed_rows(&mut presses, &rows, &parents);
    let InputMode::QuickAction(state) = &*mode else {
        return;
    };
    let limit = entry_count(state.has_target);
    for row in pressed_rows {
        if row.slot < limit {
            out.write(QuickActionActivated { slot: row.slot });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_press_activates_child_hit_once_and_secondary_press_does_not() {
        use crate::input_mode::QuickActionState;
        use bevy::camera::NormalizedRenderTarget;
        use bevy::picking::{
            backend::HitData,
            events::Pointer,
            pointer::{Location, PointerButton, PointerId},
        };

        let mut app = App::new();
        app.add_message::<PointerPress>()
            .add_message::<QuickActionActivated>()
            .insert_resource(InputMode::QuickAction(QuickActionState::default()))
            .add_systems(Update, quick_action_mouse_click_system);
        let row = app.world_mut().spawn(QuickActionRow { slot: 0 }).id();
        let label = app.world_mut().spawn(ChildOf(row)).id();
        let press = |button| PointerPress {
            entity: label,
            pointer: Pointer::new(
                PointerId::Mouse,
                Location {
                    target: NormalizedRenderTarget::None {
                        width: 1,
                        height: 1,
                    },
                    position: Vec2::ZERO,
                },
            ),
            button,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            count: 1,
        };
        app.world_mut()
            .write_message(press(PointerButton::Secondary));
        app.update();
        assert!(app
            .world_mut()
            .resource_mut::<Messages<QuickActionActivated>>()
            .drain()
            .next()
            .is_none());
        app.world_mut().write_message(press(PointerButton::Primary));
        app.world_mut().write_message(press(PointerButton::Primary));
        app.update();
        let activated: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<QuickActionActivated>>()
            .drain()
            .collect();
        assert_eq!(activated.len(), 1);
        assert_eq!(activated[0].slot, 0);
        app.update();
        assert!(app
            .world_mut()
            .resource_mut::<Messages<QuickActionActivated>>()
            .drain()
            .next()
            .is_none());

        *app.world_mut().resource_mut::<InputMode>() = InputMode::default();
        app.world_mut().write_message(press(PointerButton::Primary));
        app.update();
        *app.world_mut().resource_mut::<InputMode>() =
            InputMode::QuickAction(QuickActionState::default());
        app.update();
        assert!(app
            .world_mut()
            .resource_mut::<Messages<QuickActionActivated>>()
            .drain()
            .next()
            .is_none());
    }

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
