//! Change-password screen — three masked fields (old/new/confirm). Submit
//! dispatches the same async-work pattern as login; success returns to
//! Login. Reached from Login via Ctrl-P.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;

use super::{ChangePasswordField, ChangePasswordForm, LauncherState};

#[derive(Component)]
pub(super) struct ChangePasswordRoot;

#[derive(Component)]
pub(super) struct ChangePasswordBody;

pub(super) fn spawn_ui(mut commands: Commands, form: Res<ChangePasswordForm>) {
    commands
        .spawn((
            ChangePasswordRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(10.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.04, 0.05)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("Change password"),
                TextFont {
                    font_size: 22.0,
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
            ));
            parent.spawn((
                ChangePasswordBody,
                Text::new(format_body(&form)),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.95, 0.95)),
            ));
            parent.spawn((
                Text::new("Tab: next field   Enter: submit   Esc: cancel"),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.5, 0.5, 0.5)),
            ));
        });
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<ChangePasswordRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut form: ResMut<ChangePasswordForm>,
    mut next_state: ResMut<NextState<LauncherState>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::Escape => {
                form.old.clear();
                form.new_pw.clear();
                form.confirm.clear();
                form.error.clear();
                next_state.set(LauncherState::Login);
                return;
            }
            Key::Tab => form.focus = form.focus.next(),
            Key::Enter => {
                if form.new_pw.is_empty() || form.old.is_empty() {
                    form.error = "Fill all fields.".into();
                    continue;
                }
                if form.new_pw != form.confirm {
                    form.error = "Confirmation doesn't match.".into();
                    continue;
                }
                form.error.clear();
                next_state.set(LauncherState::ChangePasswordInFlight);
                return;
            }
            Key::Backspace => match form.focus {
                ChangePasswordField::Old => {
                    form.old.pop();
                }
                ChangePasswordField::New => {
                    form.new_pw.pop();
                }
                ChangePasswordField::Confirm => {
                    form.confirm.pop();
                }
            },
            Key::Character(s) => {
                for c in s.chars() {
                    if c.is_control() {
                        continue;
                    }
                    match form.focus {
                        ChangePasswordField::Old => form.old.push(c),
                        ChangePasswordField::New => form.new_pw.push(c),
                        ChangePasswordField::Confirm => form.confirm.push(c),
                    }
                }
            }
            _ => {}
        }
    }
}

pub(super) fn redraw_system(
    form: Res<ChangePasswordForm>,
    mut q: Query<&mut Text, With<ChangePasswordBody>>,
) {
    if !form.is_changed() {
        return;
    }
    for mut t in q.iter_mut() {
        **t = format_body(&form);
    }
}

fn format_body(form: &ChangePasswordForm) -> String {
    let mark = |f: ChangePasswordField| if form.focus == f { ">" } else { " " };
    let mask = |s: &str| "*".repeat(s.chars().count());
    let mut out = format!(
        "{o} Old:     {mo}\n{n} New:     {mn}\n{c} Confirm: {mc}",
        o = mark(ChangePasswordField::Old),
        mo = mask(&form.old),
        n = mark(ChangePasswordField::New),
        mn = mask(&form.new_pw),
        c = mark(ChangePasswordField::Confirm),
        mc = mask(&form.confirm),
    );
    if !form.error.is_empty() {
        out.push_str(&format!("\n\n{}", form.error));
    }
    out
}
