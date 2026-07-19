use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button_bundle, ButtonBundleProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, Overflow, ScrollPosition};
use bevy::ui_widgets::{Activate, ControlOrientation, Scrollbar, ScrollbarThumb};

use ffxi_viewer_core::{GraphicsField, GraphicsSettings, GRAPHICS_FIELDS};

use super::common::{
    hint, panel_node_capped, row, screen_root, spawn_breadcrumb, title, Crumb, ScrollRegion,
};
use super::{LauncherState, ServerInfo};

/// Compact metrics for the graphics list. [`LABEL_WIDTH`]/[`ROW_FONT_SIZE`] are
/// sized so the longest label ("Model Shadow Receiving") stays on one line —
/// wrapped labels were what made the list overflow the window.
const ROW_FONT_SIZE: f32 = 13.0;
const LABEL_WIDTH: f32 = 172.0;
const VALUE_WIDTH: f32 = 110.0;
const ROW_COLUMN_GAP: f32 = 6.0;
const LIST_ROW_GAP: f32 = 4.0;
const PANEL_WIDTH: f32 = 432.0;

/// Cap the panel at a fraction of the viewport so a tall settings list scrolls
/// inside it instead of spilling off the top and bottom of the window.
const PANEL_MAX_VH: f32 = 90.0;

/// Width of the scrollbar track/thumb reserved to the right of the list.
const SCROLLBAR_WIDTH: f32 = 6.0;
/// Shortest the thumb shrinks to on a long list, so it stays grabbable.
const SCROLLBAR_MIN_THUMB: f32 = 28.0;
const SCROLLBAR_TRACK_COLOR: Color = Color::srgba(1.0, 1.0, 1.0, 0.06);
const SCROLLBAR_THUMB_COLOR: Color = Color::srgba(0.62, 0.62, 0.68, 0.9);

#[derive(Component)]
pub(super) struct GraphicsRoot;

/// The `Overflow::scroll_y` region holding the settings rows; scrolled by
/// the shared [`ScrollRegion`] wheel system while the title/hint/footer stay
/// pinned.
#[derive(Component)]
pub(super) struct GraphicsScrollList;

/// The scrollbar track paired with [`GraphicsScrollList`]; hidden by
/// [`update_scrollbar_visibility`] when the list isn't overflowing.
#[derive(Component)]
pub(super) struct GraphicsScrollbar;

#[derive(Component)]
pub(super) struct GraphicsValueText(GraphicsField);

/// A field row gated behind the "Advanced" disclosure (the indented sky/light
/// sub-knobs). Toggled visible/hidden by [`redraw_advanced_visibility`].
#[derive(Component)]
pub(super) struct AdvancedRow;

/// The disclosure row's label text, swapped between ▸ and ▾.
#[derive(Component)]
pub(super) struct AdvancedToggleLabel;

/// Whether the Advanced sub-knobs are expanded. Persists across visits to the
/// Graphics screen; collapsed by default so the screen stays short.
#[derive(Resource, Default)]
pub(super) struct GraphicsAdvancedOpen(pub bool);

const ADVANCED_COLLAPSED: &str = "▸ Advanced — light tuning";
const ADVANCED_EXPANDED: &str = "▾ Advanced — light tuning";

pub(super) fn spawn_ui(
    mut commands: Commands,
    settings: Res<GraphicsSettings>,
    server: Res<ServerInfo>,
    advanced: Res<GraphicsAdvancedOpen>,
) {
    let open = advanced.0;
    commands
        .spawn((GraphicsRoot, screen_root()))
        .with_children(|root| {
            spawn_breadcrumb(root, &server, &[Crumb::Other("Graphics".to_string())]);
            root.spawn(panel_node_capped(PANEL_WIDTH, Val::Vh(PANEL_MAX_VH)))
                .with_children(|panel| {
                    panel.spawn(title("Graphics"));
                    panel.spawn(hint(
                        "Tune the same quality settings as the in-game menu. \
                         Changes apply when you connect and are saved \
                         automatically. Scroll for more; Esc goes back.",
                    ));

                    panel
                        .spawn(Node {
                            width: Val::Percent(100.0),
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Stretch,
                            column_gap: Val::Px(SCROLLBAR_WIDTH),
                            flex_grow: 1.0,
                            min_height: Val::Px(0.0),
                            ..default()
                        })
                        .with_children(|area| {
                            let list = area
                                .spawn((
                                    GraphicsScrollList,
                                    Node {
                                        flex_grow: 1.0,
                                        min_width: Val::Px(0.0),
                                        min_height: Val::Px(0.0),
                                        flex_direction: FlexDirection::Column,
                                        align_items: AlignItems::Stretch,
                                        row_gap: Val::Px(LIST_ROW_GAP),
                                        overflow: Overflow::scroll_y(),
                                        ..default()
                                    },
                                    ScrollPosition::default(),
                                    ScrollRegion,
                                ))
                                .with_children(|list| {
                                    for &field in
                                        GRAPHICS_FIELDS.iter().filter(|f| !f.is_advanced())
                                    {
                                        spawn_field_row(list, field, &settings, false, open);
                                    }

                                    list.spawn(row()).with_children(|r| {
                                        r.spawn(button_bundle(
                                            ButtonBundleProps {
                                                variant: ButtonVariant::Normal,
                                                ..default()
                                            },
                                            (),
                                            Spawn((
                                                Text::new(if open {
                                                    ADVANCED_EXPANDED
                                                } else {
                                                    ADVANCED_COLLAPSED
                                                }),
                                                ThemedText,
                                                AdvancedToggleLabel,
                                            )),
                                        ))
                                        .observe(
                                            |_ev: On<Activate>,
                                             mut open: ResMut<GraphicsAdvancedOpen>| {
                                                open.0 = !open.0;
                                            },
                                        );
                                    });

                                    for &field in GRAPHICS_FIELDS.iter().filter(|f| f.is_advanced())
                                    {
                                        spawn_field_row(list, field, &settings, true, open);
                                    }
                                })
                                .id();

                            area.spawn((
                                GraphicsScrollbar,
                                Scrollbar::new(
                                    list,
                                    ControlOrientation::Vertical,
                                    SCROLLBAR_MIN_THUMB,
                                ),
                                Node {
                                    width: Val::Px(SCROLLBAR_WIDTH),
                                    height: Val::Percent(100.0),
                                    flex_shrink: 0.0,
                                    overflow: Overflow::clip(),
                                    border_radius: BorderRadius::all(Val::Px(
                                        SCROLLBAR_WIDTH / 2.0,
                                    )),
                                    ..default()
                                },
                                BackgroundColor(SCROLLBAR_TRACK_COLOR),
                            ))
                            .with_children(|track| {
                                track.spawn((
                                    ScrollbarThumb {
                                        border_radius: BorderRadius::all(Val::Px(
                                            SCROLLBAR_WIDTH / 2.0,
                                        )),
                                        border: UiRect::DEFAULT,
                                    },
                                    Node {
                                        position_type: PositionType::Absolute,
                                        ..default()
                                    },
                                    BackgroundColor(SCROLLBAR_THUMB_COLOR),
                                ));
                            });
                        });

                    panel.spawn(row()).with_children(|r| {
                        r.spawn(button_bundle(
                            ButtonBundleProps {
                                variant: ButtonVariant::Normal,
                                ..default()
                            },
                            (),
                            Spawn((Text::new("Reset to High"), ThemedText)),
                        ))
                        .observe(
                            |_ev: On<Activate>, mut settings: ResMut<GraphicsSettings>| {
                                settings.reset_to_default();
                            },
                        );

                        r.spawn(button_bundle(
                            ButtonBundleProps {
                                variant: ButtonVariant::Primary,
                                ..default()
                            },
                            (),
                            Spawn((Text::new("Back"), ThemedText)),
                        ))
                        .observe(
                            |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                                next.set(LauncherState::ServerSelect);
                            },
                        );
                    });
                });
        });
}

fn spawn_field_row(
    panel: &mut ChildSpawnerCommands,
    field: GraphicsField,
    settings: &GraphicsSettings,
    advanced: bool,
    advanced_open: bool,
) {
    let value_color = Color::srgb(0.92, 0.92, 0.95);
    let mut row_cmd = panel.spawn(Node {
        width: Val::Percent(100.0),
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        column_gap: Val::Px(ROW_COLUMN_GAP),
        display: if advanced && !advanced_open {
            Display::None
        } else {
            Display::Flex
        },
        ..default()
    });
    if advanced {
        row_cmd.insert(AdvancedRow);
    }
    row_cmd.with_children(|rowc| {
        rowc.spawn((
            Node {
                width: Val::Px(LABEL_WIDTH),
                flex_shrink: 0.0,
                ..default()
            },
            Text::new(field.label().to_string()),
            TextFont {
                font_size: ROW_FONT_SIZE.into(),
                ..default()
            },
            ThemedText,
        ));

        rowc.spawn(button_bundle(
            ButtonBundleProps::default(),
            (),
            Spawn((Text::new("◀"), ThemedText)),
        ))
        .observe(
            move |_ev: On<Activate>, mut settings: ResMut<GraphicsSettings>| {
                settings.cycle(field, -1);
            },
        );

        rowc.spawn((
            Node {
                width: Val::Px(VALUE_WIDTH),
                justify_content: JustifyContent::Center,
                ..default()
            },
            Text::new(settings.value_label(field)),
            TextFont {
                font_size: ROW_FONT_SIZE.into(),
                ..default()
            },
            TextColor(value_color),
            GraphicsValueText(field),
            ThemedText,
        ));

        rowc.spawn(button_bundle(
            ButtonBundleProps::default(),
            (),
            Spawn((Text::new("▶"), ThemedText)),
        ))
        .observe(
            move |_ev: On<Activate>, mut settings: ResMut<GraphicsSettings>| {
                settings.cycle(field, 1);
            },
        );
    });
}

pub(super) fn redraw_graphics_system(
    settings: Res<GraphicsSettings>,
    mut q: Query<(&GraphicsValueText, &mut Text)>,
) {
    if !settings.is_changed() {
        return;
    }
    for (cell, mut text) in q.iter_mut() {
        let want = settings.value_label(cell.0);
        if text.0 != want {
            text.0 = want;
        }
    }
}

/// Show/hide the Advanced sub-knob rows and flip the ▸/▾ disclosure label when
/// the user toggles the Advanced row.
pub(super) fn redraw_advanced_visibility(
    open: Res<GraphicsAdvancedOpen>,
    mut rows: Query<&mut Node, With<AdvancedRow>>,
    mut labels: Query<&mut Text, With<AdvancedToggleLabel>>,
) {
    if !open.is_changed() {
        return;
    }
    let display = if open.0 { Display::Flex } else { Display::None };
    for mut node in rows.iter_mut() {
        if node.display != display {
            node.display = display;
        }
    }
    let want = if open.0 {
        ADVANCED_EXPANDED
    } else {
        ADVANCED_COLLAPSED
    };
    for mut text in labels.iter_mut() {
        if text.0 != want {
            text.0 = want.to_string();
        }
    }
}

/// Hide the scrollbar track when the list fits, so a full-height thumb never
/// sits there implying scroll on a list that has none.
pub(super) fn update_scrollbar_visibility(
    lists: Query<&ComputedNode, With<GraphicsScrollList>>,
    mut bars: Query<&mut Node, With<GraphicsScrollbar>>,
) {
    let Ok(list) = lists.single() else {
        return;
    };
    let want = if list.content_size.y > list.size.y + 0.5 {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in bars.iter_mut() {
        if node.display != want {
            node.display = want;
        }
    }
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<GraphicsRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if matches!(ev.logical_key, Key::Escape) {
            next.set(LauncherState::ServerSelect);
            return;
        }
    }
}
