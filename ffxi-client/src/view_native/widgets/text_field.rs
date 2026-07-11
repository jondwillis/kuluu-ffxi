use bevy::feathers::theme::ThemeBackgroundColor;
use bevy::feathers::tokens;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::input::ButtonState;
use bevy::input_focus::tab_navigation::TabIndex;
use bevy::input_focus::{FocusedInput, InputFocus};
use bevy::prelude::*;
use bevy::ui_widgets::ValueChange;
use bevy::window::Ime;

#[derive(Component, Default, Debug, Clone)]
pub struct TextField {
    pub value: String,
    pub placeholder: String,

    pub mask: bool,

    pub cursor: usize,

    pub submit_on_enter: bool,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct TextFieldDisplay {
    pub owner: Entity,
}

#[derive(EntityEvent, Debug, Clone)]
pub struct TextFieldSubmitted {
    #[event_target]
    pub entity: Entity,
}

#[derive(Default)]
pub struct TextFieldProps {
    pub initial: String,
    pub placeholder: String,
    pub mask: bool,
    pub submit_on_enter: bool,

    pub width: Option<Val>,
}

pub fn text_field(props: TextFieldProps) -> impl Bundle {
    let width = props.width.unwrap_or(Val::Percent(100.0));
    let cursor = props.initial.len();
    (
        TextField {
            value: props.initial,
            placeholder: props.placeholder,
            mask: props.mask,
            cursor,
            submit_on_enter: props.submit_on_enter,
        },
        Node {
            width,
            height: Val::Px(28.0),
            align_items: AlignItems::Center,
            padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(3.0)),
            ..default()
        },
        BorderColor::all(Color::srgb(0.25, 0.25, 0.28)),
        ThemeBackgroundColor(tokens::BUTTON_BG),
        TabIndex(0),
    )
}

pub struct TextFieldPlugin;

impl Plugin for TextFieldPlugin {
    fn build(&self, app: &mut App) {
        // Gamescope's OSK paste gesture synthesizes Ctrl-down and V-down
        // within the same PreUpdate tick, unlike a physical keyboard where
        // they always land frames apart. Without this ordering, dispatch
        // (which triggers text_field_on_key) can run before
        // keyboard_input_system has recorded the Ctrl-down in
        // `ButtonInput<KeyCode>`, so the paste's Ctrl+V reads as an
        // unmodified 'V' and inserts a literal "v" instead of pasting.
        app.configure_sets(
            PreUpdate,
            bevy::input_focus::InputFocusSystems::Dispatch.after(bevy::input::InputSystems),
        );
        app.add_observer(text_field_on_key)
            .add_systems(Update, (sync_display, sync_focus_border, ime_commit_system));
    }
}

/// Gamescope's on-screen keyboard (and other IME-driven virtual keyboards)
/// inserts and pastes text via `Ime::Commit` rather than synthesizing
/// `Ctrl+V`/per-character `KeyboardInput`, so it bypasses `text_field_on_key`
/// entirely unless handled here too.
fn ime_commit_system(
    mut ime_events: MessageReader<Ime>,
    focus: Option<Res<InputFocus>>,
    mut q: Query<&mut TextField>,
    mut commands: Commands,
) {
    let Some(focused) = focus.and_then(|f| f.0) else {
        return;
    };
    for ev in ime_events.read() {
        let Ime::Commit { value, .. } = ev else {
            continue;
        };
        let Ok(mut field) = q.get_mut(focused) else {
            continue;
        };
        let sanitized: String = value.chars().filter(|c| !c.is_control()).collect();
        if sanitized.is_empty() {
            continue;
        }
        let cur = field.cursor;
        field.value.insert_str(cur, &sanitized);
        field.cursor = cur + sanitized.len();
        let value = field.value.clone();
        commands.trigger(ValueChange {
            source: focused,
            value,
        });
    }
}

/// Tracks Ctrl/Super held-state from the `FocusedInput<KeyboardInput>` stream
/// itself rather than `Res<ButtonInput<KeyCode>>`. bevy_input_focus's
/// `dispatch_focused_input` fires observers via a *deferred* `Commands::trigger`,
/// so this observer only runs after `keyboard_input_system` has already
/// processed the whole frame's raw key events. Gamescope's OSK paste
/// synthesizes a full Ctrl+V chord (down/down/up/up) within a single frame, so
/// by the time the deferred observer runs — for any of those four events —
/// `ButtonInput<KeyCode>` already reflects Ctrl as released. Reconstructing
/// modifier state from the events as this observer sees them, in their
/// original per-event order, sidesteps that entirely.
#[derive(Default)]
struct KeyModifierTracker {
    ctrl: bool,
    super_: bool,
}

fn text_field_on_key(
    mut ev: On<FocusedInput<KeyboardInput>>,
    mut q: Query<&mut TextField>,
    mut modifiers: Local<KeyModifierTracker>,
    mut commands: Commands,
) {
    let input = &ev.event().input;
    match input.key_code {
        KeyCode::ControlLeft | KeyCode::ControlRight => {
            modifiers.ctrl = input.state == ButtonState::Pressed;
        }
        KeyCode::SuperLeft | KeyCode::SuperRight => {
            modifiers.super_ = input.state == ButtonState::Pressed;
        }
        _ => {}
    }

    let Ok(mut field) = q.get_mut(ev.focused_entity) else {
        return;
    };
    if input.state != ButtonState::Pressed {
        return;
    }

    if matches!(input.key_code, KeyCode::Tab) {
        return;
    }

    let cmd_or_ctrl = modifiers.ctrl || modifiers.super_;
    if cmd_or_ctrl {
        match input.key_code {
            KeyCode::KeyV => {
                let pasted = match arboard::Clipboard::new().and_then(|mut cb| cb.get_text()) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, "text_field: clipboard read failed");
                        ev.propagate(false);
                        return;
                    }
                };
                let sanitized: String = pasted.chars().filter(|c| !c.is_control()).collect();
                if !sanitized.is_empty() {
                    let cur = field.cursor;
                    field.value.insert_str(cur, &sanitized);
                    field.cursor = cur + sanitized.len();
                    let value = field.value.clone();
                    commands.trigger(ValueChange {
                        source: ev.focused_entity,
                        value,
                    });
                }
                ev.propagate(false);
                return;
            }
            KeyCode::KeyC => {
                let payload = field.value.clone();
                if let Err(e) = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(payload)) {
                    tracing::warn!(error = %e, "text_field: clipboard write failed");
                }
                ev.propagate(false);
                return;
            }
            _ => {}
        }
    }

    let mut mutated = false;
    match input.key_code {
        KeyCode::ArrowLeft => {
            field.cursor = prev_grapheme(&field.value, field.cursor);
            ev.propagate(false);
        }
        KeyCode::ArrowRight => {
            field.cursor = next_grapheme(&field.value, field.cursor);
            ev.propagate(false);
        }
        KeyCode::Home => {
            field.cursor = 0;
            ev.propagate(false);
        }
        KeyCode::End => {
            field.cursor = field.value.len();
            ev.propagate(false);
        }
        KeyCode::Backspace => {
            if field.cursor > 0 {
                let cur = field.cursor;
                let new_cursor = prev_grapheme(&field.value, cur);
                field.value.drain(new_cursor..cur);
                field.cursor = new_cursor;
                mutated = true;
            }
            ev.propagate(false);
        }
        KeyCode::Delete => {
            let cur = field.cursor;
            if cur < field.value.len() {
                let next = next_grapheme(&field.value, cur);
                field.value.drain(cur..next);
                mutated = true;
            }
            ev.propagate(false);
        }
        KeyCode::Enter | KeyCode::NumpadEnter => {
            if field.submit_on_enter {
                commands.trigger(TextFieldSubmitted {
                    entity: ev.focused_entity,
                });
            }
            ev.propagate(false);
        }
        _ => {
            if let Key::Character(ref s) = input.logical_key {
                if s.chars().all(|c| !c.is_control()) {
                    let s = s.to_string();
                    let cur = field.cursor;
                    field.value.insert_str(cur, &s);
                    field.cursor = cur + s.len();
                    mutated = true;
                    ev.propagate(false);
                }
            } else if matches!(input.logical_key, Key::Space) {
                let cur = field.cursor;
                field.value.insert(cur, ' ');
                field.cursor = cur + 1;
                mutated = true;
                ev.propagate(false);
            }
        }
    }

    if mutated {
        let value = field.value.clone();
        commands.trigger(ValueChange {
            source: ev.focused_entity,
            value,
        });
    }
}

fn sync_display(
    time: Res<Time<Real>>,
    focus: Option<Res<InputFocus>>,
    q_fields: Query<&TextField>,
    mut q_display: Query<(&TextFieldDisplay, &mut Text, &mut TextColor)>,
) {
    let caret_on = time.elapsed_secs().rem_euclid(1.0) < 0.5;
    let focused = focus.and_then(|f| f.0);
    for (display, mut text, mut color) in q_display.iter_mut() {
        let Ok(field) = q_fields.get(display.owner) else {
            continue;
        };
        let is_focused = focused == Some(display.owner);
        let raw = if field.value.is_empty() && !is_focused {
            color.0 = Color::srgb(0.5, 0.5, 0.55);
            field.placeholder.clone()
        } else {
            color.0 = Color::srgb(0.92, 0.92, 0.95);
            if field.mask {
                "*".repeat(field.value.chars().count())
            } else {
                field.value.clone()
            }
        };

        if is_focused && caret_on {
            let pos = if field.mask {
                field.value[..field.cursor.min(field.value.len())]
                    .chars()
                    .count()
            } else {
                field.cursor.min(raw.len())
            };
            let mut s = String::with_capacity(raw.len() + 1);

            let mut byte_pos = 0usize;
            for (i, (b, _)) in raw.char_indices().enumerate() {
                if i == pos {
                    byte_pos = b;
                    break;
                }
                byte_pos = b + raw[b..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
            }
            s.push_str(&raw[..byte_pos]);
            s.push('|');
            s.push_str(&raw[byte_pos..]);
            text.0 = s;
        } else {
            text.0 = raw;
        }
    }
}

fn sync_focus_border(
    focus: Option<Res<InputFocus>>,
    mut q: Query<(Entity, &mut BorderColor), With<TextField>>,
) {
    let focused = focus.and_then(|f| f.0);
    for (e, mut bc) in q.iter_mut() {
        let target = if Some(e) == focused {
            Color::srgb(0.36, 0.62, 1.0)
        } else {
            Color::srgb(0.25, 0.25, 0.28)
        };
        *bc = BorderColor::all(target);
    }
}

fn prev_grapheme(s: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    s[..cursor]
        .char_indices()
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn next_grapheme(s: &str, cursor: usize) -> usize {
    if cursor >= s.len() {
        return s.len();
    }
    let c = s[cursor..].chars().next().unwrap();
    cursor + c.len_utf8()
}
