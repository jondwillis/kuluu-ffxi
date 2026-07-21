use std::collections::HashMap;

use bevy::prelude::*;
use bevy::ui::UiTransform;
use ffxi_viewer_wire::SceneSnapshot;

use crate::camera::ChaseCamera;
use crate::components::{InGameEntity, IsSelf, WorldEntity};
use crate::hud::menu::MENU_PANE_WIDTH;
use crate::hud::style::{self, theme};
use crate::input_mode::{InputMode, MenuKind, Pane};
use crate::lock_on::LockOn;
use crate::minimap::overlay::{self, MarkerFilters, MinimapDot};
use crate::minimap::{crop_pixel_rect, MinimapMode, MinimapState, MinimapView, MinimapZoom};
use crate::scene::Target;
use crate::snapshot::SceneState;

/// The map pane is the left column; the wide-scan list is the right column, so
/// the shared `Pane` toggle reads naturally as map-vs-list focus.
pub const MAP_PANE: Pane = Pane::Left;
pub const WIDESCAN_PANE: Pane = Pane::Right;

const MAP_VIEW_PX: f32 = 320.0;

const WIDESCAN_VISIBLE_ROWS: usize = 20;

const TRACKED_MARKER_PX: f32 = 12.0;

const TRACKED_MARKER_RING_PX: f32 = 2.0;

/// Distinct color for the currently tracked (0x0F5) target, kept clear of the
/// per-kind list palette and the minimap's target/lock colors.
const TRACKED_MARKER_COLOR: Color = Color::srgb(1.0, 0.30, 0.95);

/// Map-screen dot store, disjoint from the minimap widget's `MinimapDots`; both
/// drive `overlay::sync_marker_layer` over their own entity set.
#[derive(Resource, Default)]
pub struct MapScreenDots {
    pub by_id: HashMap<u32, Entity>,
}

impl MapScreenDots {
    pub fn clear_for_logout(&mut self) {
        self.by_id.clear();
    }
}

#[derive(Component)]
pub struct MapScreenRoot;

#[derive(Component)]
pub struct MapScreenImage;

#[derive(Component)]
pub struct MapScreenOverlayLayer;

#[derive(Component)]
pub struct MapScreenGridLabel;

#[derive(Component)]
pub struct MapTrackedMarker;

#[derive(Component, Clone, Copy)]
pub struct MapWidescanRow {
    pub slot: usize,
}

/// The Map screen is on top of the menu stack.
pub fn map_open(mode: &InputMode) -> bool {
    matches!(
        mode,
        InputMode::Menu(stack) if stack.current().map(|l| l.kind) == Some(MenuKind::Map)
    )
}

/// One wide-scan list row for display, sorted nearest-first. Shared by the
/// renderer and the client's confirm handler so the cursor index and the
/// `WidescanTrack(act_index)` it fires stay in lockstep.
#[derive(Debug, Clone, PartialEq)]
pub struct WidescanRow {
    pub act_index: u16,
    pub label: String,
    pub color: Color,
}

/// Build the sorted wide-scan rows from the snapshot: nearest first by the
/// server-relative offset, colored by kind, named from the server `sName` or —
/// when empty (current LSB) — the local entity keyed on `act_index`.
pub fn widescan_rows(snap: &SceneSnapshot) -> Vec<WidescanRow> {
    let mut entries: Vec<&ffxi_viewer_wire::WidescanEntry> = snap.widescan.entries.iter().collect();
    entries.sort_by_key(|e| {
        let (x, z) = (e.rel_x as i64, e.rel_z as i64);
        x * x + z * z
    });
    entries
        .into_iter()
        .map(|e| {
            let name = if !e.name.is_empty() {
                e.name.clone()
            } else {
                snap.entities
                    .iter()
                    .find(|ent| ent.act_index == e.act_index)
                    .and_then(|ent| ent.name.clone())
                    .unwrap_or_else(|| format!("#{}", e.act_index))
            };
            let label = if e.level > 0 {
                format!("{name} (Lv{})", e.level)
            } else {
                name
            };
            WidescanRow {
                act_index: e.act_index,
                label,
                color: overlay::widescan_color(e.kind),
            }
        })
        .collect()
}

pub(crate) fn spawn_map_screen(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = crate::hud::item_ui::transparent_placeholder(&mut images);

    commands
        .spawn((
            InGameEntity,
            MapScreenRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                left: Val::Px(8.0),
                column_gap: Val::Px(6.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::FlexStart,
                display: Display::None,
                ..default()
            },
            ZIndex(style::WINDOW_Z),
        ))
        .with_children(|root| {
            spawn_map_column(root, placeholder);
            spawn_widescan_column(root);
        });
}

fn spawn_map_column(root: &mut ChildSpawnerCommands, placeholder: Handle<Image>) {
    let (mut n, bg, bd) = style::window_frame();
    n.flex_direction = FlexDirection::Column;
    n.row_gap = Val::Px(4.0);
    root.spawn((n, bg, bd)).with_children(|col| {
        col.spawn((
            MapScreenGridLabel,
            Text::new("Map"),
            style::text_font(14.0),
            TextColor(theme::TITLE),
        ));
        col.spawn((
            Node {
                width: Val::Px(MAP_VIEW_PX),
                height: Val::Px(MAP_VIEW_PX),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(theme::MAP_BACKING),
        ))
        .with_children(|view| {
            view.spawn((
                MapScreenImage,
                ImageNode::new(placeholder),
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
            ));
            view.spawn((
                MapScreenOverlayLayer,
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
            ))
            .with_children(|overlay_layer| {
                let half = TRACKED_MARKER_PX * 0.5;
                overlay_layer.spawn((
                    MapTrackedMarker,
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Px(TRACKED_MARKER_PX),
                        height: Val::Px(TRACKED_MARKER_PX),
                        margin: UiRect {
                            left: Val::Px(-half),
                            top: Val::Px(-half),
                            ..default()
                        },
                        border: UiRect::all(Val::Px(TRACKED_MARKER_RING_PX)),
                        display: Display::None,
                        ..default()
                    },
                    BackgroundColor(TRACKED_MARKER_COLOR),
                    BorderColor::all(Color::WHITE),
                ));
            });
        });
    });
}

fn spawn_widescan_column(root: &mut ChildSpawnerCommands) {
    let (mut n, bg, bd) = style::window_frame();
    n.width = Val::Px(MENU_PANE_WIDTH);
    n.flex_direction = FlexDirection::Column;
    n.row_gap = Val::Px(2.0);
    root.spawn((n, bg, bd)).with_children(|col| {
        col.spawn((
            Text::new("Wide Scan"),
            style::text_font(14.0),
            TextColor(theme::TITLE),
        ));
        for slot in 0..WIDESCAN_VISIBLE_ROWS {
            col.spawn((
                MapWidescanRow { slot },
                Text::new(""),
                style::text_font(13.0),
                TextColor(theme::TEXT),
                Node {
                    display: Display::None,
                    ..default()
                },
            ));
        }
        col.spawn((Node {
            margin: UiRect::top(Val::Px(6.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            ..default()
        },))
            .with_children(overlay::spawn_marker_legend);
    });
}

pub(crate) fn update_map_screen_image(
    mode: Res<InputMode>,
    state: Res<MinimapState>,
    minimap_mode: Res<MinimapMode>,
    view: Res<MinimapView>,
    zoom: Res<MinimapZoom>,
    images: Res<Assets<Image>>,
    mut q: Query<&mut ImageNode, With<MapScreenImage>>,
) {
    if !map_open(&mode) {
        return;
    }
    let Ok(mut image_node) = q.single_mut() else {
        return;
    };
    let resolved = state.resolved_mode(*minimap_mode);
    let (handle, full_aabb) = match resolved {
        MinimapMode::Retail => (state.retail_image.clone(), state.retail_aabb),
        MinimapMode::TopDown => (state.topdown_image.clone(), state.aabb),
        MinimapMode::Auto => (None, None),
    };
    if let Some(h) = handle.clone() {
        if image_node.image != h {
            image_node.image = h;
        }
    }
    // Fit-to-zone shows the whole DAT image; a finite zoom crops it to the same
    // window the overlay markers use (MinimapView is shared with the minimap).
    let rect = match (zoom.radius_yalms, view.visible_aabb, full_aabb, handle) {
        (Some(_), Some(visible), Some(full), Some(h)) => images
            .get(&h)
            .and_then(|img| crop_pixel_rect(full, visible, img.size_f32())),
        _ => None,
    };
    if image_node.rect != rect {
        image_node.rect = rect;
    }
}

/// Read-only marker inputs bundled so `update_map_screen_markers` stays under
/// Bevy's 16-parameter system limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MarkerInputs<'w> {
    target: Res<'w, Target>,
    lock_on: Res<'w, LockOn>,
    chase: Res<'w, ChaseCamera>,
    filters: Res<'w, MarkerFilters>,
}

#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_map_screen_markers(
    mode: Res<InputMode>,
    scene_state: Res<SceneState>,
    view: Res<MinimapView>,
    markers: MarkerInputs,
    mut dots: ResMut<MapScreenDots>,
    mut commands: Commands,
    q_overlay_layer: Query<Entity, With<MapScreenOverlayLayer>>,
    q_self: Query<&Transform, With<IsSelf>>,
    q_transform: Query<(&Transform, &WorldEntity), Without<IsSelf>>,
    mut root_q: Query<
        &mut Node,
        (
            With<MapScreenRoot>,
            Without<MinimapDot>,
            Without<MapTrackedMarker>,
        ),
    >,
    mut q_dot_node: Query<
        (&mut Node, &mut BackgroundColor),
        (
            With<MinimapDot>,
            Without<MapScreenRoot>,
            Without<MapTrackedMarker>,
        ),
    >,
    mut q_marker_transform: Query<&mut UiTransform, With<MinimapDot>>,
    mut tracked_q: Query<
        &mut Node,
        (
            With<MapTrackedMarker>,
            Without<MinimapDot>,
            Without<MapScreenRoot>,
        ),
    >,
    mut grid_q: Query<&mut Text, With<MapScreenGridLabel>>,
) {
    let open = map_open(&mode);
    if let Ok(mut node) = root_q.single_mut() {
        let want = if open { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
    if !open {
        // Closing (or logout resetting the mode) drops every map dot so its
        // store never outlives the despawned overlay entities.
        for (_, dot) in dots.by_id.drain() {
            if let Ok(mut ec) = commands.get_entity(dot) {
                ec.despawn();
            }
        }
        if let Ok(mut node) = tracked_q.single_mut() {
            if node.display != Display::None {
                node.display = Display::None;
            }
        }
        return;
    }

    let snap = &scene_state.snapshot;
    let (Some(aabb), Ok(overlay_layer)) = (view.visible_aabb, q_overlay_layer.single()) else {
        return;
    };
    let self_char_id = snap.self_char_id.unwrap_or(0);
    let party_ids = overlay::party_id_set(&scene_state);

    overlay::sync_marker_layer(
        aabb,
        overlay_layer,
        self_char_id,
        &markers.target,
        &markers.lock_on,
        markers.chase.yaw,
        &party_ids,
        &markers.filters,
        &q_self,
        &q_transform,
        &mut dots.by_id,
        &mut commands,
        &mut q_dot_node,
        &mut q_marker_transform,
    );

    if let (Ok(mut label), Ok(self_t)) = (grid_q.single_mut(), q_self.single()) {
        let (col, row) = aabb.world_to_grid(self_t.translation);
        let want = format!("Map  {col}-{row}");
        if **label != want {
            **label = want;
        }
    }

    if let Ok(mut node) = tracked_q.single_mut() {
        let uv = snap.widescan.tracked.and_then(|t| {
            // Prefer the live local entity's Bevy transform (its coord transform
            // is already applied); fall back to converting the raw 0x0F5
            // server position when the tracked act_index is not locally spawned.
            let world = q_transform
                .iter()
                .find(|(_, we)| we.act_index == t.act_index)
                .map(|(tf, _)| tf.translation)
                .unwrap_or_else(|| {
                    crate::scene::ffxi_to_bevy(ffxi_viewer_wire::Vec3 {
                        x: t.x,
                        y: t.y,
                        z: t.z,
                    })
                });
            aabb.world_to_uv_or_offscreen(world)
        });
        match uv {
            Some(uv) => {
                node.left = Val::Percent(uv.x * 100.0);
                node.top = Val::Percent(uv.y * 100.0);
                if node.display != Display::Flex {
                    node.display = Display::Flex;
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

pub(crate) fn update_map_widescan_list(
    mode: Res<InputMode>,
    scene_state: Res<SceneState>,
    mut row_q: Query<(&MapWidescanRow, &mut Text, &mut TextColor, &mut Node)>,
) {
    if !map_open(&mode) {
        return;
    }
    let InputMode::Menu(stack) = &*mode else {
        return;
    };
    let cursor = stack.current().map(|l| l.cursor).unwrap_or(0);
    let list_focused = stack.active_pane == WIDESCAN_PANE;

    let rows = widescan_rows(&scene_state.snapshot);
    let total = rows.len();
    let start = cursor
        .saturating_sub(WIDESCAN_VISIBLE_ROWS / 2)
        .min(total.saturating_sub(WIDESCAN_VISIBLE_ROWS));

    for (row, mut text, mut color, mut node) in row_q.iter_mut() {
        if total == 0 {
            let (want, want_color, visible) = if row.slot == 0 {
                ("(no targets in range)".to_string(), theme::MUTED, true)
            } else {
                (String::new(), theme::TEXT, false)
            };
            set_row(&mut text, &mut color, &mut node, want, want_color, visible);
            continue;
        }
        let idx = start + row.slot;
        match rows.get(idx) {
            Some(entry) => {
                let is_cursor = idx == cursor;
                let (prefix, want_color) = if is_cursor && list_focused {
                    ("> ", theme::CURSOR)
                } else if is_cursor {
                    ("  ", theme::MUTED)
                } else {
                    ("  ", entry.color)
                };
                set_row(
                    &mut text,
                    &mut color,
                    &mut node,
                    format!("{prefix}{}", entry.label),
                    want_color,
                    true,
                );
            }
            None => set_row(
                &mut text,
                &mut color,
                &mut node,
                String::new(),
                theme::TEXT,
                false,
            ),
        }
    }
}

fn set_row(
    text: &mut Text,
    color: &mut TextColor,
    node: &mut Node,
    want: String,
    want_color: Color,
    visible: bool,
) {
    let display = if visible {
        Display::Flex
    } else {
        Display::None
    };
    if node.display != display {
        node.display = display;
    }
    if visible && text.0 != want {
        text.0 = want;
    }
    if color.0 != want_color {
        color.0 = want_color;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_wire::{Entity, EntityKind, Vec3, WidescanEntry, WidescanList};

    fn entry(act_index: u16, level: u8, kind: u8, rel_x: i16, rel_z: i16) -> WidescanEntry {
        WidescanEntry {
            act_index,
            level,
            kind,
            rel_x,
            rel_z,
            name: String::new(),
        }
    }

    #[test]
    fn widescan_rows_sort_nearest_first() {
        let snap = SceneSnapshot {
            widescan: WidescanList {
                entries: vec![
                    entry(1, 5, 2, 30, 40), // dist 50
                    entry(2, 3, 2, 3, 4),   // dist 5
                    entry(3, 9, 1, 6, 8),   // dist 10
                ],
                tracked: None,
            },
            ..Default::default()
        };
        let rows = widescan_rows(&snap);
        let order: Vec<u16> = rows.iter().map(|r| r.act_index).collect();
        assert_eq!(order, vec![2, 3, 1], "nearest by rel offset comes first");
    }

    #[test]
    fn widescan_row_name_falls_back_to_local_entity() {
        let mut snap = SceneSnapshot {
            widescan: WidescanList {
                entries: vec![entry(7, 12, 2, 1, 1)],
                tracked: None,
            },
            ..Default::default()
        };
        snap.entities.push(Entity {
            id: 0x400_0007,
            act_index: 7,
            kind: EntityKind::Mob,
            name: Some("Orcish Fodder".to_string()),
            pos: Vec3::default(),
            heading: 0,
            hp_pct: None,
            bt_target_id: 0,
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            status: 0,
        });
        let rows = widescan_rows(&snap);
        assert_eq!(rows[0].label, "Orcish Fodder (Lv12)");
    }
}
