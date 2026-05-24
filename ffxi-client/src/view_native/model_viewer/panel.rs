//! Right-side form panel for the model viewer.
//!
//! Plain bevy_ui — same widget vocabulary as the launcher screens
//! (`view_native/launcher_ui/login.rs`), one TextField per scalar input,
//! and an `[<] clip-name [>]` row driven by [`ClipList`]. The panel is
//! spawned once at startup and never rebuilt; only the `Text` and
//! `Resource` values update as the user edits.

use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input_focus::tab_navigation::TabGroup;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, ValueChange};
use ffxi_viewer_core::combat_stance::ModelViewerClipOverride;

use crate::view_native::widgets::text_field::{text_field, TextFieldDisplay};
use crate::view_native::widgets::TextFieldProps;

use super::{ClipList, NpcForm, PcForm, ViewerMode};

#[derive(Component)]
struct PanelRoot;

/// Marker for the text node displaying the current clip name. The
/// `update_clip_label` system keeps its content in sync with
/// [`ClipList`]; without this we'd need to respawn the row on every
/// rebake.
#[derive(Component)]
struct ClipNameLabel;

/// Marker on the mode-toggle buttons. Carried so a future "highlight
/// active mode" reactor can find them without a re-query against
/// `ChildOf` chains.
#[derive(Component, Clone, Copy)]
struct ModeButton(#[allow(dead_code)] ViewerMode);

/// PC-form field identity, mirrors the `LoginField` enum in login.rs.
/// Used by the per-field `ValueChange<String>` observers to know which
/// `PcForm` slot to write into.
#[derive(Clone, Copy)]
enum PcField {
    Race,
    Face,
    Head,
    Body,
    Hands,
    Legs,
    Feet,
    Main,
    Sub,
    Ranged,
}

pub fn register(app: &mut App) {
    app.add_systems(Startup, spawn_panel)
        .add_systems(Update, (update_clip_label, sync_clip_override));
}

fn spawn_panel(mut commands: Commands) {
    commands
        .spawn((
            PanelRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(16.0),
                right: Val::Px(16.0),
                width: Val::Px(360.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(8.0),
                padding: UiRect::all(Val::Px(16.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.04, 0.05, 0.88)),
            BorderColor::all(Color::srgb(0.20, 0.20, 0.24)),
            BorderRadius::all(Val::Px(6.0)),
            TabGroup::default(),
        ))
        .with_children(|panel| {
            panel.spawn((
                Text::new("ffxi model viewer"),
                TextFont {
                    font_size: 18.0,
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
                ThemedText,
            ));

            // Mode toggle row.
            panel
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(8.0),
                    ..default()
                })
                .with_children(|row| {
                    spawn_mode_button(row, ViewerMode::Pc, "PC");
                    spawn_mode_button(row, ViewerMode::Npc, "NPC");
                });

            // ---- PC inputs ----------------------------------------------------
            panel.spawn((
                Text::new("PC inputs"),
                TextColor(Color::srgb(0.7, 0.85, 1.0)),
                ThemedText,
            ));
            spawn_pc_field(panel, "race",   "1",       PcField::Race);
            spawn_pc_field(panel, "face",   "0",       PcField::Face);
            spawn_pc_field(panel, "head",   "0",       PcField::Head);
            spawn_pc_field(panel, "body",   "0",       PcField::Body);
            spawn_pc_field(panel, "hands",  "0",       PcField::Hands);
            spawn_pc_field(panel, "legs",   "0",       PcField::Legs);
            spawn_pc_field(panel, "feet",   "0",       PcField::Feet);
            spawn_pc_field(panel, "main",   "0",       PcField::Main);
            spawn_pc_field(panel, "sub",    "0",       PcField::Sub);
            spawn_pc_field(panel, "ranged", "0",       PcField::Ranged);

            // ---- NPC inputs ---------------------------------------------------
            panel.spawn((
                Text::new("NPC inputs"),
                TextColor(Color::srgb(0.7, 0.85, 1.0)),
                ThemedText,
            ));
            spawn_npc_modelid(panel);

            // ---- Animation cycler --------------------------------------------
            panel.spawn((
                Text::new("Animation"),
                TextColor(Color::srgb(0.7, 0.85, 1.0)),
                ThemedText,
            ));
            panel
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(8.0),
                    align_items: AlignItems::Center,
                    ..default()
                })
                .with_children(|row| {
                    row.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("<"), ThemedText)),
                    ))
                    .observe(|_ev: On<Activate>, mut list: ResMut<ClipList>| {
                        if list.names.is_empty() {
                            return;
                        }
                        list.index = (list.index + list.names.len() - 1) % list.names.len();
                    });
                    row.spawn((
                        ClipNameLabel,
                        Node {
                            flex_grow: 1.0,
                            ..default()
                        },
                        Text::new("(none)"),
                        TextColor(Color::srgb(0.92, 0.92, 0.95)),
                        ThemedText,
                    ));
                    row.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new(">"), ThemedText)),
                    ))
                    .observe(|_ev: On<Activate>, mut list: ResMut<ClipList>| {
                        if list.names.is_empty() {
                            return;
                        }
                        list.index = (list.index + 1) % list.names.len();
                    });
                });

            panel.spawn((
                Text::new(
                    "PC mode reads race/face + 8 equipment ids. \
                     NPC mode reads model_id (u16). Hex (0x…) or decimal. \
                     Apply happens automatically ~150ms after edits.",
                ),
                TextColor(Color::srgb(0.65, 0.65, 0.7)),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                ThemedText,
            ));
        });
}

fn spawn_mode_button(parent: &mut ChildSpawnerCommands, mode: ViewerMode, label: &str) {
    parent
        .spawn((
            ModeButton(mode),
            button(
                ButtonProps {
                    variant: ButtonVariant::Primary,
                    ..default()
                },
                (),
                Spawn((Text::new(label.to_string()), ThemedText)),
            ),
        ))
        .observe(move |_ev: On<Activate>, mut viewer_mode: ResMut<ViewerMode>| {
            *viewer_mode = mode;
        });
}

fn spawn_pc_field(parent: &mut ChildSpawnerCommands, label: &str, initial: &str, field: PcField) {
    let label = label.to_string();
    let initial = initial.to_string();
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(8.0),
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|row| {
            row.spawn((
                Node {
                    width: Val::Px(70.0),
                    ..default()
                },
                Text::new(label),
                ThemedText,
            ));
            row.spawn(text_field(TextFieldProps {
                initial: initial.clone(),
                ..default()
            }))
            .with_children(|tf| {
                tf.spawn((
                    Node {
                        flex_grow: 1.0,
                        ..default()
                    },
                    Text::new(String::new()),
                    TextColor(Color::srgb(0.92, 0.92, 0.95)),
                    TextFieldDisplay {
                        owner: Entity::PLACEHOLDER,
                    },
                    ThemedText,
                ));
            })
            .observe(move |ev: On<ValueChange<String>>, mut form: ResMut<PcForm>| {
                apply_pc_field(&mut form, field, &ev.value);
            });
        });
}

fn spawn_npc_modelid(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(8.0),
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|row| {
            row.spawn((
                Node {
                    width: Val::Px(70.0),
                    ..default()
                },
                Text::new("model_id"),
                ThemedText,
            ));
            row.spawn(text_field(TextFieldProps {
                initial: "0".to_string(),
                ..default()
            }))
            .with_children(|tf| {
                tf.spawn((
                    Node {
                        flex_grow: 1.0,
                        ..default()
                    },
                    Text::new(String::new()),
                    TextColor(Color::srgb(0.92, 0.92, 0.95)),
                    TextFieldDisplay {
                        owner: Entity::PLACEHOLDER,
                    },
                    ThemedText,
                ));
            })
            .observe(|ev: On<ValueChange<String>>, mut form: ResMut<NpcForm>| {
                if let Some(v) = parse_u16_lenient(&ev.value) {
                    form.model_id = v;
                }
            });
        });
}

fn apply_pc_field(form: &mut PcForm, field: PcField, value: &str) {
    match field {
        PcField::Race => {
            if let Some(v) = parse_u8_lenient(value) {
                form.race = v;
            }
        }
        PcField::Face => {
            if let Some(v) = parse_u8_lenient(value) {
                form.face = v;
            }
        }
        PcField::Head => {
            if let Some(v) = parse_u16_lenient(value) {
                form.head = v;
            }
        }
        PcField::Body => {
            if let Some(v) = parse_u16_lenient(value) {
                form.body = v;
            }
        }
        PcField::Hands => {
            if let Some(v) = parse_u16_lenient(value) {
                form.hands = v;
            }
        }
        PcField::Legs => {
            if let Some(v) = parse_u16_lenient(value) {
                form.legs = v;
            }
        }
        PcField::Feet => {
            if let Some(v) = parse_u16_lenient(value) {
                form.feet = v;
            }
        }
        PcField::Main => {
            if let Some(v) = parse_u16_lenient(value) {
                form.main = v;
            }
        }
        PcField::Sub => {
            if let Some(v) = parse_u16_lenient(value) {
                form.sub = v;
            }
        }
        PcField::Ranged => {
            if let Some(v) = parse_u16_lenient(value) {
                form.ranged = v;
            }
        }
    }
}

/// Accept `0x…` or `0X…` hex, otherwise parse as decimal. Returns
/// `None` (silently) on empty or malformed input so the user can
/// backspace through a field without trashing the form.
fn parse_u16_lenient(s: &str) -> Option<u16> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u16>().ok()
    }
}

fn parse_u8_lenient(s: &str) -> Option<u8> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u8::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u8>().ok()
    }
}

/// Keep the clip-name label in sync with [`ClipList::current`]. Runs
/// every frame but the `Text::set_if_neq` no-ops when the string didn't
/// change, so the write is cheap on stable selections.
fn update_clip_label(
    list: Res<ClipList>,
    mut q: Query<&mut Text, With<ClipNameLabel>>,
) {
    if !list.is_changed() {
        return;
    }
    let next = list.current().unwrap_or("(none)").to_string();
    for mut text in q.iter_mut() {
        if text.0 != next {
            text.0 = next.clone();
        }
    }
}

/// When the selected clip index changes, update the override resource so
/// `tick_skinned_actors` switches clips on the next tick. We update the
/// resource itself rather than mutate-in-place so `Res::is_changed()`
/// gates downstream reactors cleanly.
fn sync_clip_override(
    list: Res<ClipList>,
    mut commands: Commands,
    mut last: Local<Option<String>>,
) {
    if !list.is_changed() {
        return;
    }
    let now = list.current().map(str::to_string);
    if *last == now {
        return;
    }
    *last = now.clone();
    if let Some(name) = now {
        commands.insert_resource(ModelViewerClipOverride::new(name));
    }
}
