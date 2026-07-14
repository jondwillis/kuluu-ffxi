use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button_bundle, ButtonBundleProps, ButtonVariant};
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

#[derive(Component)]
struct ClipNameLabel;

#[derive(Component, Clone, Copy)]
struct ModeButton(#[allow(dead_code)] ViewerMode);

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

fn spawn_panel(mut commands: Commands, pc: Res<PcForm>, npc: Res<NpcForm>) {
    let race = pc.race.to_string();
    let face = pc.face.to_string();
    let fmt_id = |v: u16| {
        if v == 0 {
            "0".to_string()
        } else {
            format!("0x{v:04X}")
        }
    };
    let head = fmt_id(pc.head);
    let body = fmt_id(pc.body);
    let hands = fmt_id(pc.hands);
    let legs = fmt_id(pc.legs);
    let feet = fmt_id(pc.feet);
    let main = fmt_id(pc.main);
    let sub = fmt_id(pc.sub);
    let ranged = fmt_id(pc.ranged);
    let model_id = fmt_id(npc.model_id);

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
                border_radius: BorderRadius::all(Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.04, 0.05, 0.88)),
            BorderColor::all(Color::srgb(0.20, 0.20, 0.24)),
            TabGroup::default(),
        ))
        .with_children(|panel| {
            panel.spawn((
                Text::new("ffxi model viewer"),
                TextFont {
                    font_size: 18.0.into(),
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
                ThemedText,
            ));

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

            panel.spawn((
                Text::new("PC inputs"),
                TextColor(Color::srgb(0.7, 0.85, 1.0)),
                ThemedText,
            ));
            spawn_pc_field(panel, "race", &race, PcField::Race);
            spawn_pc_field(panel, "face", &face, PcField::Face);
            spawn_pc_field(panel, "head", &head, PcField::Head);
            spawn_pc_field(panel, "body", &body, PcField::Body);
            spawn_pc_field(panel, "hands", &hands, PcField::Hands);
            spawn_pc_field(panel, "legs", &legs, PcField::Legs);
            spawn_pc_field(panel, "feet", &feet, PcField::Feet);
            spawn_pc_field(panel, "main", &main, PcField::Main);
            spawn_pc_field(panel, "sub", &sub, PcField::Sub);
            spawn_pc_field(panel, "ranged", &ranged, PcField::Ranged);

            panel.spawn((
                Text::new("NPC inputs"),
                TextColor(Color::srgb(0.7, 0.85, 1.0)),
                ThemedText,
            ));
            spawn_npc_modelid(panel, &model_id);

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
                    row.spawn(button_bundle(
                        ButtonBundleProps::default(),
                        (),
                        Spawn((Text::new("<"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut list: ResMut<ClipList>| {
                            if list.names.is_empty() {
                                return;
                            }
                            list.index = (list.index + list.names.len() - 1) % list.names.len();
                        },
                    );
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
                    row.spawn(button_bundle(
                        ButtonBundleProps::default(),
                        (),
                        Spawn((Text::new(">"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut list: ResMut<ClipList>| {
                            if list.names.is_empty() {
                                return;
                            }
                            list.index = (list.index + 1) % list.names.len();
                        },
                    );
                });

            panel.spawn((
                Text::new(
                    "PC mode reads race/face + 8 equipment ids. \
                     NPC mode reads model_id (u16). Hex (0x…) or decimal. \
                     Apply happens automatically ~150ms after edits.",
                ),
                TextColor(Color::srgb(0.65, 0.65, 0.7)),
                TextFont {
                    font_size: 11.0.into(),
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
            button_bundle(
                ButtonBundleProps {
                    variant: ButtonVariant::Primary,
                    ..default()
                },
                (),
                Spawn((Text::new(label.to_string()), ThemedText)),
            ),
        ))
        .observe(
            move |_ev: On<Activate>, mut viewer_mode: ResMut<ViewerMode>| {
                *viewer_mode = mode;
            },
        );
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
            .observe(
                move |ev: On<ValueChange<String>>, mut form: ResMut<PcForm>| {
                    apply_pc_field(&mut form, field, &ev.value);
                },
            );
        });
}

fn spawn_npc_modelid(parent: &mut ChildSpawnerCommands, initial: &str) {
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
                Text::new("model_id"),
                ThemedText,
            ));
            row.spawn(text_field(TextFieldProps {
                initial,
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

fn update_clip_label(list: Res<ClipList>, mut q: Query<&mut Text, With<ClipNameLabel>>) {
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
