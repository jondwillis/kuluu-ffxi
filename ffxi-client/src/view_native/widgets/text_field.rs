//! Single-line editable text widget — the one widget `bevy_feathers` 0.17
//! does **not** ship. Built on top of feathers' theming + `bevy_input_focus`
//! `InputFocus` resource so a `TextField` participates in the same Tab-cycle
//! the rest of the feathers widget kit uses (button, checkbox, slider, ...).
//!
//! # Focus model
//!
//! We piggy-back on `bevy_input_focus::tab_navigation` (auto-registered by
//! `FeathersPlugins`). Each `TextField` carries a `TabIndex(0)` component
//! and lives under a `TabGroup` ancestor; Tab/Shift-Tab cycles the
//! `InputFocus` resource through them in spawn order. Clicking a TextField
//! also focuses it — `TabNavigationPlugin::click_to_focus` observer
//! handles that for any `TabIndex`-bearing entity, so we don't add a
//! per-entity click observer.
//!
//! # Key handling
//!
//! We use the `FocusedInput<KeyboardInput>` bubble (same pattern as
//! `bevy_ui_widgets::Checkbox`) for editing keys (printable chars,
//! arrows, Backspace, etc.). Tab is intentionally *not* consumed — the
//! tab-navigation observer is registered at the same level and will
//! advance focus. Enter optionally fires `TextFieldSubmitted`
//! (gated on `submit_on_enter`); we don't insert a newline in a
//! single-line field.
//!
//! # Output
//!
//! Edits fire `ValueChange<String>` events (the same generic the rest of
//! the feathers widgets use for scalar changes). Submission fires
//! `TextFieldSubmitted`.

use bevy::feathers::theme::ThemeBackgroundColor;
use bevy::feathers::tokens;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::input::ButtonInput;
use bevy::input::ButtonState;
use bevy::input_focus::tab_navigation::TabIndex;
use bevy::input_focus::{FocusedInput, InputFocus};
use bevy::prelude::*;
use bevy::ui_widgets::ValueChange;

/// Marker + state for a single-line editable text widget.
#[derive(Component, Default, Debug, Clone)]
pub struct TextField {
    pub value: String,
    pub placeholder: String,
    /// Render every char as `*` (password). Real chars stay in `value`.
    pub mask: bool,
    /// Byte offset into `value` for the insertion caret. Always aligned
    /// to a UTF-8 boundary by every mutation path.
    pub cursor: usize,
    /// If true, Enter fires `TextFieldSubmitted` instead of being a no-op.
    pub submit_on_enter: bool,
}

/// Child marker pointing back at the owning `TextField` entity. The render
/// system updates the `Text` value on this entity (parent carries the
/// state + border + focus, child carries the actual glyphs + caret).
#[derive(Component, Debug, Clone, Copy)]
pub struct TextFieldDisplay {
    pub owner: Entity,
}

/// Fired when Enter is pressed in a focused TextField with `submit_on_enter`.
#[derive(EntityEvent, Debug, Clone)]
pub struct TextFieldSubmitted {
    /// The TextField entity that was submitted.
    #[event_target]
    pub entity: Entity,
}

/// Construction-time properties for [`text_field`].
#[derive(Default)]
pub struct TextFieldProps {
    pub initial: String,
    pub placeholder: String,
    pub mask: bool,
    pub submit_on_enter: bool,
    /// Optional fixed width. `None` → flex-grow to fill parent row.
    pub width: Option<Val>,
}

/// Spawn helper. Returns a bundle that should be passed to `commands.spawn(...)`.
///
/// Caller must spawn a child via `Children::spawn`/`children![]` with the
/// `TextFieldDisplay` marker to get the visible text rendered — or use the
/// `text_field_bundle` helper which wires that for you.
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

/// Plugin: registers focus-aware keyboard handling and the display-sync system.
pub struct TextFieldPlugin;

impl Plugin for TextFieldPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(text_field_on_key)
            .add_systems(Update, (sync_display, sync_focus_border));
    }
}

/// Keyboard handler — same observer pattern feathers' Checkbox uses.
/// Operates on `Key` (the logical key with shift/layout applied) for
/// printable insertion, and on `KeyCode` for navigation/editing keys.
fn text_field_on_key(
    mut ev: On<FocusedInput<KeyboardInput>>,
    mut q: Query<&mut TextField>,
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
) {
    let Ok(mut field) = q.get_mut(ev.focused_entity) else {
        return;
    };
    let input = &ev.event().input;
    if input.state != ButtonState::Pressed {
        return;
    }

    // Tab is owned by the tab-navigation observer; don't consume it.
    if matches!(input.key_code, KeyCode::Tab) {
        return;
    }

    // Cmd (macOS) / Ctrl shortcuts. Checked first so `Key::Character("v")`
    // arriving on the same press doesn't also get inserted as a literal 'v'.
    let cmd_or_ctrl = keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight)
        || keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight);
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
                // Copy the real value (not the masked rendering) — matches
                // every other text widget's Cmd+C behavior.
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
            // Printable insertion. `Key::Character` carries the post-layout
            // glyph(s) (already shifted, dead-key composed, etc.) — exactly
            // what we want to insert literally.
            if let Key::Character(ref s) = input.logical_key {
                // Filter control chars (Key::Character can carry e.g. "\u{1}"
                // for Ctrl-A on some platforms).
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

/// Update the child `Text` display whenever the parent `TextField` changes,
/// or every frame for the blinking caret (cheap — one string format per
/// focused field per frame).
fn sync_display(
    time: Res<Time<Real>>,
    focus: Option<Res<InputFocus>>,
    q_fields: Query<&TextField>,
    mut q_display: Query<(&TextFieldDisplay, &mut Text, &mut TextColor)>,
) {
    // Blink at 1 Hz (caret visible for first half of each second).
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
            // Insert a caret glyph at the (byte-aligned) cursor. We work in
            // the displayed string (which may be masked) so the caret lands
            // at the visual position; for masked fields each '*' is 1 byte
            // so cursor maps 1:1 to char-count, which is equivalent.
            let pos = if field.mask {
                field.value[..field.cursor.min(field.value.len())]
                    .chars()
                    .count()
            } else {
                field.cursor.min(raw.len())
            };
            let mut s = String::with_capacity(raw.len() + 1);
            // Byte-safe split at the char-boundary equivalent.
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

/// Border-color flip on focus change. Cheap: only runs when `InputFocus`
/// mutates or when a TextField is added/removed.
fn sync_focus_border(
    focus: Option<Res<InputFocus>>,
    mut q: Query<(Entity, &mut BorderColor), With<TextField>>,
) {
    let focused = focus.and_then(|f| f.0);
    for (e, mut bc) in q.iter_mut() {
        let target = if Some(e) == focused {
            // Approx tokens::FOCUS_RING — feathers' theme registry maps to a
            // bright accent; hardcode a close match so we don't reach into
            // the theme registry from here.
            Color::srgb(0.36, 0.62, 1.0)
        } else {
            Color::srgb(0.25, 0.25, 0.28)
        };
        *bc = BorderColor::all(target);
    }
}

// ---------- grapheme-ish boundary helpers ----------
//
// Real grapheme segmentation needs `unicode-segmentation`; for v1 we step
// per-`char` (Unicode scalar). This is wrong for combining marks + emoji
// ZWJ sequences but correct for ASCII + Latin-1 (every cred/login field
// we'll display). Upgrade to `unicode-segmentation` when a CJK/emoji
// field shows up.

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
