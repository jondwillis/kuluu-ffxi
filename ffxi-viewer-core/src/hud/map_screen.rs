use std::collections::HashMap;

use bevy::prelude::*;
use bevy::ui::UiTransform;
use ffxi_viewer_wire::SceneSnapshot;

use crate::camera::ChaseCamera;
use crate::components::{InGameEntity, IsSelf, WorldEntity};
use crate::hud::style::{self, theme};
use crate::hud::zone_flash::ZoneNameResolver;
use crate::input_mode::{InputMode, MenuKind};
use crate::lock_on::LockOn;
use crate::minimap::overlay::{self, MarkerFilters, MinimapDot};
use crate::minimap::{crop_pixel_rect, MinimapMode, MinimapState, MinimapView, MinimapZoom};
use crate::scene::Target;
use crate::snapshot::SceneState;

/// Retail's Map is full-screen: the DAT map image fills the viewport with the 3D
/// world faintly visible behind it, drawn at this alpha (retail composites the
/// map semi-transparently over the scene).
const MAP_IMAGE_ALPHA: f32 = 0.86;

/// Top-right command/submode panel geometry.
const PANEL_WIDTH_PX: f32 = 190.0;
const PANEL_TOP_PX: f32 = 48.0;
const PANEL_RIGHT_PX: f32 = 8.0;

/// Rows in the reusable panel pool — sized for the wide-scan roster and the
/// Change Map zone list, which are the longest submode lists.
const PANEL_ROWS: usize = 24;

const TRACKED_MARKER_PX: f32 = 14.0;
const TRACKED_MARKER_RING_PX: f32 = 2.0;

/// Marker placement crosshair size (Markers submode).
const PLACE_CURSOR_PX: f32 = 16.0;

/// Placed-marker dot size and pool cap (per-zone user markers drawn on the map).
const PLACED_MARKER_PX: f32 = 9.0;
const PLACED_MARKER_POOL: usize = 32;

/// Placed-marker fill, distinct from entity dots and the tracked-target color.
const PLACED_MARKER_COLOR: Color = Color::srgb(1.0, 0.55, 0.10);

/// Distinct color for the currently tracked (0x0F5) target, kept clear of the
/// per-kind list palette and the minimap's target/lock colors.
const TRACKED_MARKER_COLOR: Color = Color::srgb(1.0, 0.30, 0.95);

/// The Map screen's four sub-modes. Retail opens on a floating command submenu
/// (Markers / Wide Scan / Change Map); selecting a row drills into that mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MapSubMode {
    #[default]
    Command,
    WideScan,
    Markers,
    ChangeMap,
}

impl MapSubMode {
    /// Display name shown in the top-left title and the top-right panel header.
    pub fn title(self) -> &'static str {
        match self {
            MapSubMode::Command => "Map",
            MapSubMode::WideScan => "Wide Scan",
            MapSubMode::Markers => "Markers",
            MapSubMode::ChangeMap => "Change Map",
        }
    }
}

/// The command submenu rows, in retail order. Confirm on each drills into the
/// matching submode.
pub const COMMAND_ROWS: [(&str, MapSubMode); 3] = [
    ("Markers", MapSubMode::Markers),
    ("Wide Scan", MapSubMode::WideScan),
    ("Change Map", MapSubMode::ChangeMap),
];

/// A user-placed map marker. Rendered on both the full-screen map and the
/// minimap; persisted per character + zone by the client's marker store.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MapMarker {
    pub world: ffxi_viewer_wire::Vec3,
    pub label: String,
}

/// Placed markers keyed by zone id. Owned here (viewer-core) so both surfaces
/// render them; the client's `marker_store` loads/saves this to disk.
#[derive(Resource, Default)]
pub struct MapMarkers {
    pub by_zone: HashMap<u16, Vec<MapMarker>>,
}

impl MapMarkers {
    pub fn for_zone(&self, zone: u16) -> &[MapMarker] {
        self.by_zone.get(&zone).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// Bespoke Map-screen state, disjoint from the generic `MenuStack` (the Map is a
/// gated full-screen surface like the item/equipment screens). The stack only
/// carries `MenuKind::Map`; the submode + per-submode cursor live here.
#[derive(Resource, Default)]
pub struct MapScreenState {
    pub mode: MapSubMode,
    pub cursor: usize,
    /// Markers placement crosshair, in map UV (0..1). `None` until Markers opens.
    pub map_cursor: Option<Vec2>,
    /// The (zone, map_index) the image shows. `None` = the live zone, index 0;
    /// `Some` = a Change Map override.
    pub viewed: Option<(u16, u8)>,
    /// Active text-entry buffer while naming a new marker.
    pub marker_entry: Option<String>,
}

impl MapScreenState {
    /// Reset to the default command submenu (Map open / logout).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The `(zone, index)` being previewed via Change Map, or `None` when the
    /// map shows the live zone's floor 0 (the minimap's own image). Used to gate
    /// the on-demand viewed-map loader and to suppress live entity markers.
    pub fn viewed_override(&self, live_zone: u16) -> Option<(u16, u8)> {
        self.viewed.filter(|&(z, i)| (z, i) != (live_zone, 0))
    }
}

/// The Change Map preview image + calibration for a non-live `(zone, index)`,
/// loaded on demand so the live `MinimapState` is never disturbed (kuluu-ziru).
#[derive(Resource, Default)]
pub struct ViewedMap {
    pub key: Option<(u16, u8)>,
    pub image: Option<Handle<Image>>,
    pub aabb: Option<crate::minimap::MinimapAabb>,
}

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
pub struct MapTitleLabel;

#[derive(Component)]
pub struct MapGridLabel;

#[derive(Component)]
pub struct MapTrackedMarker;

#[derive(Component)]
pub struct MapPlaceCursor;

#[derive(Component, Clone, Copy)]
pub struct MapPlacedMarker {
    pub slot: usize,
}

#[derive(Component, Clone, Copy)]
pub struct MapPlacedLabel {
    pub slot: usize,
}

#[derive(Component)]
pub struct MapPanelRoot;

#[derive(Component)]
pub struct MapPanelTitle;

#[derive(Component, Clone, Copy)]
pub struct MapPanelRow {
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

/// A rendered panel row: text, color, and whether the cursor is on it.
struct PanelRow {
    text: String,
    color: Color,
    is_cursor: bool,
}

/// Build the top-right panel's title and rows for the current submode. Kept
/// pure (no ECS) so the row count / cursor logic can be unit-tested.
fn panel_rows(
    state: &MapScreenState,
    snap: &SceneSnapshot,
    markers: &[MapMarker],
    zone_name: &dyn Fn(u16) -> Option<String>,
) -> Vec<PanelRow> {
    let cursor = state.cursor;
    match state.mode {
        MapSubMode::Command => COMMAND_ROWS
            .iter()
            .enumerate()
            .map(|(i, (label, _))| PanelRow {
                text: label.to_string(),
                color: theme::TEXT,
                is_cursor: i == cursor,
            })
            .collect(),
        MapSubMode::WideScan => {
            let rows = widescan_rows(snap);
            if rows.is_empty() {
                return vec![PanelRow {
                    text: "(no targets in range)".to_string(),
                    color: theme::MUTED,
                    is_cursor: false,
                }];
            }
            rows.into_iter()
                .enumerate()
                .map(|(i, r)| PanelRow {
                    text: r.label,
                    color: r.color,
                    is_cursor: i == cursor,
                })
                .collect()
        }
        MapSubMode::Markers => {
            if markers.is_empty() {
                return vec![PanelRow {
                    text: "(no markers — Confirm to place)".to_string(),
                    color: theme::MUTED,
                    is_cursor: false,
                }];
            }
            markers
                .iter()
                .enumerate()
                .map(|(i, m)| PanelRow {
                    text: m.label.clone(),
                    color: theme::TEXT,
                    is_cursor: i == cursor,
                })
                .collect()
        }
        MapSubMode::ChangeMap => change_map_rows(state, snap, zone_name)
            .into_iter()
            .enumerate()
            .map(|(i, (text, _))| PanelRow {
                text,
                color: theme::TEXT,
                is_cursor: i == cursor,
            })
            .collect(),
    }
}

/// The `(zone, map_index)` each Change Map row selects, in display order: this
/// zone's floors first, then every other zone that ships a map (index 0). The
/// display builder (`change_map_rows`) and the client's confirm handler both
/// index this, so the visible list and the dispatched target stay in lockstep.
pub fn change_map_targets(state: &MapScreenState, snap: &SceneSnapshot) -> Vec<(u16, u8)> {
    let live_zone = snap.zone_id.unwrap_or(0);
    let (viewed_zone, _) = state.viewed.unwrap_or((live_zone, 0));
    let mut targets = Vec::new();

    let floors = ffxi_dat::map_image::map_count_for_zone(viewed_zone);
    if floors > 1 {
        for idx in 0..floors {
            targets.push((viewed_zone, idx as u8));
        }
    }
    for zone in ffxi_dat::map_image::zones_with_maps() {
        if zone != viewed_zone {
            targets.push((zone, 0));
        }
    }
    targets
}

/// Labelled Change Map rows built from `change_map_targets`, naming floors of the
/// viewed zone and other zones via the resolver.
pub fn change_map_rows(
    state: &MapScreenState,
    snap: &SceneSnapshot,
    zone_name: &dyn Fn(u16) -> Option<String>,
) -> Vec<(String, (u16, u8))> {
    let live_zone = snap.zone_id.unwrap_or(0);
    let (viewed_zone, viewed_idx) = state.viewed.unwrap_or((live_zone, 0));
    change_map_targets(state, snap)
        .into_iter()
        .map(|(zone, idx)| {
            let label = if zone == viewed_zone {
                let mark = if idx == viewed_idx { "* " } else { "  " };
                format!("{mark}Floor {}", idx + 1)
            } else {
                zone_name(zone).unwrap_or_else(|| format!("Zone #{zone}"))
            };
            (label, (zone, idx))
        })
        .collect()
}

pub(crate) fn spawn_map_screen(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = crate::hud::item_ui::transparent_placeholder(&mut images);

    // Full-screen map surface (image + marker overlay + title), below the HUD
    // panels so chat and the command submenu draw over it.
    commands
        .spawn((
            InGameEntity,
            MapScreenRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                display: Display::None,
                ..default()
            },
            ZIndex(style::WINDOW_Z - 2),
        ))
        .with_children(|root| {
            root.spawn((
                MapScreenImage,
                ImageNode {
                    image: placeholder,
                    color: Color::srgba(1.0, 1.0, 1.0, MAP_IMAGE_ALPHA),
                    ..default()
                },
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
            ));
            root.spawn((
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
                let phalf = PLACE_CURSOR_PX * 0.5;
                overlay_layer.spawn((
                    MapPlaceCursor,
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Px(PLACE_CURSOR_PX),
                        height: Val::Px(PLACE_CURSOR_PX),
                        margin: UiRect {
                            left: Val::Px(-phalf),
                            top: Val::Px(-phalf),
                            ..default()
                        },
                        border: UiRect::all(Val::Px(2.0)),
                        display: Display::None,
                        ..default()
                    },
                    BorderColor::all(theme::CURSOR),
                ));
                let mhalf = PLACED_MARKER_PX * 0.5;
                for slot in 0..PLACED_MARKER_POOL {
                    overlay_layer
                        .spawn((
                            MapPlacedMarker { slot },
                            Node {
                                position_type: PositionType::Absolute,
                                width: Val::Px(PLACED_MARKER_PX),
                                height: Val::Px(PLACED_MARKER_PX),
                                margin: UiRect {
                                    left: Val::Px(-mhalf),
                                    top: Val::Px(-mhalf),
                                    ..default()
                                },
                                border: UiRect::all(Val::Px(1.0)),
                                flex_direction: FlexDirection::Column,
                                display: Display::None,
                                ..default()
                            },
                            BackgroundColor(PLACED_MARKER_COLOR),
                            BorderColor::all(Color::WHITE),
                        ))
                        .with_children(|dot| {
                            dot.spawn((
                                MapPlacedLabel { slot },
                                Text::new(""),
                                style::text_font(11.0),
                                TextColor(theme::TITLE),
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(PLACED_MARKER_PX),
                                    top: Val::Px(-2.0),
                                    ..default()
                                },
                            ));
                        });
                }
            });
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(8.0),
                    left: Val::Px(10.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(2.0),
                    ..default()
                },
                BackgroundColor(theme::FRAME_BG),
            ))
            .with_children(|title| {
                title.spawn((
                    MapTitleLabel,
                    Text::new("Map"),
                    style::text_font(15.0),
                    TextColor(theme::TITLE),
                ));
                title.spawn((
                    MapGridLabel,
                    Text::new(""),
                    style::text_font(13.0),
                    TextColor(theme::MUTED),
                ));
            });
        });

    // Top-right command / submode panel, above the map.
    let (mut n, bg, bd) = style::window_frame();
    n.position_type = PositionType::Absolute;
    n.top = Val::Px(PANEL_TOP_PX);
    n.right = Val::Px(PANEL_RIGHT_PX);
    n.width = Val::Px(PANEL_WIDTH_PX);
    n.display = Display::None;
    commands
        .spawn((
            InGameEntity,
            MapPanelRoot,
            n,
            bg,
            bd,
            ZIndex(style::WINDOW_Z),
        ))
        .with_children(|col| {
            col.spawn((
                MapPanelTitle,
                Text::new("Map"),
                style::text_font(14.0),
                TextColor(theme::TITLE),
            ));
            for slot in 0..PANEL_ROWS {
                col.spawn((
                    MapPanelRow { slot },
                    Text::new(""),
                    style::text_font(13.0),
                    TextColor(theme::TEXT),
                    Node {
                        display: Display::None,
                        ..default()
                    },
                ));
            }
        });
}

/// Reset the submode to the command submenu on the rising edge of the Map
/// screen opening, so a fresh open always lands on Markers/Wide Scan/Change Map
/// rather than whatever submode the previous session left behind.
pub(crate) fn reset_map_screen_on_open(
    mode: Res<InputMode>,
    mut state: ResMut<MapScreenState>,
    mut was_open: Local<bool>,
) {
    let open = map_open(&mode);
    if open && !*was_open {
        state.reset();
    }
    *was_open = open;
}

/// Load the Change Map preview image when `MapScreenState.viewed` points at a
/// non-live zone/floor, decoding it off to the side of `MinimapState`.
pub(crate) fn load_viewed_map(
    mode: Res<InputMode>,
    map_state: Res<MapScreenState>,
    scene_state: Res<SceneState>,
    dat_root: Res<crate::minimap::retail::MinimapDatRoot>,
    mut calib: ResMut<crate::minimap::retail::MapCalibration>,
    mut viewed: ResMut<ViewedMap>,
    mut images: ResMut<Assets<Image>>,
) {
    if !map_open(&mode) {
        return;
    }
    let live_zone = scene_state.snapshot.zone_id.unwrap_or(0);
    let want = map_state.viewed_override(live_zone);
    if want == viewed.key {
        return;
    }
    let Some((zone, idx)) = want else {
        *viewed = ViewedMap::default();
        return;
    };
    let Some(root) = dat_root.0.as_ref() else {
        return;
    };
    let dll = calib.ensure_dll(root.root());
    match crate::minimap::retail::load_zone_map_image(root, dll.as_deref(), zone, idx, &mut images)
    {
        Some((image, aabb)) => {
            *viewed = ViewedMap {
                key: Some((zone, idx)),
                image: Some(image),
                aabb,
            };
        }
        None => *viewed = ViewedMap::default(),
    }
}

pub(crate) fn update_map_screen_image(
    mode: Res<InputMode>,
    map_state: Res<MapScreenState>,
    scene_state: Res<SceneState>,
    viewed: Res<ViewedMap>,
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
    let live_zone = scene_state.snapshot.zone_id.unwrap_or(0);
    // Change Map preview: show the whole foreign map (no live-minimap crop).
    if map_state.viewed_override(live_zone).is_some() {
        if let Some(h) = viewed.image.clone() {
            if image_node.image != h {
                image_node.image = h;
            }
        }
        if image_node.rect.is_some() {
            image_node.rect = None;
        }
        return;
    }
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
    map_state: Res<'w, MapScreenState>,
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
            Without<MapPlaceCursor>,
        ),
    >,
    mut q_dot_node: Query<
        (&mut Node, &mut BackgroundColor),
        (
            With<MinimapDot>,
            Without<MapScreenRoot>,
            Without<MapTrackedMarker>,
            Without<MapPlaceCursor>,
        ),
    >,
    mut q_marker_transform: Query<&mut UiTransform, With<MinimapDot>>,
    mut tracked_q: Query<
        &mut Node,
        (
            With<MapTrackedMarker>,
            Without<MinimapDot>,
            Without<MapScreenRoot>,
            Without<MapPlaceCursor>,
        ),
    >,
    mut place_q: Query<
        &mut Node,
        (
            With<MapPlaceCursor>,
            Without<MinimapDot>,
            Without<MapScreenRoot>,
            Without<MapTrackedMarker>,
        ),
    >,
    mut grid_q: Query<&mut Text, With<MapGridLabel>>,
) {
    let open = map_open(&mode);
    if let Ok(mut node) = root_q.single_mut() {
        let want = if open { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
    if !open {
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
        if let Ok(mut node) = place_q.single_mut() {
            if node.display != Display::None {
                node.display = Display::None;
            }
        }
        return;
    }

    let snap = &scene_state.snapshot;
    // Change Map preview of a foreign zone: its entities aren't ours, so drop the
    // live entity dots, tracked target, and placement crosshair (the viewed
    // zone's placed markers still render via `update_map_placed_markers`).
    if markers
        .map_state
        .viewed_override(snap.zone_id.unwrap_or(0))
        .is_some()
    {
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
        if let Ok(mut node) = place_q.single_mut() {
            if node.display != Display::None {
                node.display = Display::None;
            }
        }
        return;
    }
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
        let want = format!("({col}-{row})");
        if **label != want {
            **label = want;
        }
    }

    if let Ok(mut node) = tracked_q.single_mut() {
        let uv = snap.widescan.tracked.and_then(|t| {
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
        set_overlay_marker(&mut node, uv);
    }

    if let Ok(mut node) = place_q.single_mut() {
        let uv = (markers.map_state.mode == MapSubMode::Markers)
            .then_some(markers.map_state.map_cursor)
            .flatten();
        set_overlay_marker(&mut node, uv);
    }
}

fn set_overlay_marker(node: &mut Node, uv: Option<Vec2>) {
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

/// Position the placed-marker dots (and their labels) from `MapMarkers` for the
/// live zone, using the same visible AABB as the entity overlay so both align.
#[allow(clippy::type_complexity)]
pub(crate) fn update_map_placed_markers(
    mode: Res<InputMode>,
    map_state: Res<MapScreenState>,
    scene_state: Res<SceneState>,
    map_markers: Res<MapMarkers>,
    viewed: Res<ViewedMap>,
    view: Res<MinimapView>,
    mut dot_q: Query<(&MapPlacedMarker, &mut Node), Without<MapPlacedLabel>>,
    mut label_q: Query<(&MapPlacedLabel, &mut Text)>,
) {
    if !map_open(&mode) {
        for (_, mut node) in dot_q.iter_mut() {
            if node.display != Display::None {
                node.display = Display::None;
            }
        }
        return;
    }
    let live_zone = scene_state.snapshot.zone_id.unwrap_or(0);
    // Change Map preview shows the viewed zone's own markers against its AABB.
    let (zone, aabb) = match map_state.viewed_override(live_zone) {
        Some((z, _)) => (z, viewed.aabb),
        None => (live_zone, view.visible_aabb),
    };
    let markers = map_markers.for_zone(zone);

    for (dot, mut node) in dot_q.iter_mut() {
        let uv = markers.get(dot.slot).zip(aabb).and_then(|(m, a)| {
            a.world_to_uv_or_offscreen(Vec3::new(m.world.x, m.world.y, m.world.z))
        });
        set_overlay_marker(&mut node, uv);
    }
    for (label, mut text) in label_q.iter_mut() {
        let want = markers
            .get(label.slot)
            .map(|m| m.label.as_str())
            .unwrap_or("");
        if text.as_str() != want {
            **text = want.to_string();
        }
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn update_map_panel(
    mode: Res<InputMode>,
    map_state: Res<MapScreenState>,
    scene_state: Res<SceneState>,
    map_markers: Res<MapMarkers>,
    resolver: Option<Res<ZoneNameResolver>>,
    mut panel_root_q: Query<&mut Node, (With<MapPanelRoot>, Without<MapPanelRow>)>,
    mut title_q: Query<
        &mut Text,
        (
            With<MapPanelTitle>,
            Without<MapGridLabel>,
            Without<MapTitleLabel>,
            Without<MapPanelRow>,
        ),
    >,
    mut screen_title_q: Query<
        &mut Text,
        (
            With<MapTitleLabel>,
            Without<MapPanelTitle>,
            Without<MapGridLabel>,
            Without<MapPanelRow>,
        ),
    >,
    mut row_q: Query<
        (&MapPanelRow, &mut Text, &mut TextColor, &mut Node),
        (
            Without<MapPanelRoot>,
            Without<MapPanelTitle>,
            Without<MapTitleLabel>,
            Without<MapGridLabel>,
        ),
    >,
) {
    let open = map_open(&mode);
    if let Ok(mut node) = panel_root_q.single_mut() {
        let want = if open { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
    if !open {
        return;
    }

    let snap = &scene_state.snapshot;
    let zone = snap.zone_id.unwrap_or(0);
    let zone_name = |z: u16| -> Option<String> {
        resolver
            .as_ref()
            .and_then(|r| (r.0)(z))
            .map(|s| s.replace('_', " "))
    };
    let submode_name = map_state.mode.title();

    if let Ok(mut t) = title_q.single_mut() {
        if **t != *submode_name {
            **t = submode_name.to_string();
        }
    }
    if let Ok(mut t) = screen_title_q.single_mut() {
        let want = match zone_name(zone) {
            Some(name) => format!("{submode_name}   {name}"),
            None => submode_name.to_string(),
        };
        if **t != want {
            **t = want;
        }
    }

    let markers = map_markers.for_zone(zone);
    let rows = panel_rows(&map_state, snap, markers, &zone_name);

    // Scroll the pool so the cursor stays visible in long lists.
    let total = rows.len();
    let start = map_state
        .cursor
        .saturating_sub(PANEL_ROWS / 2)
        .min(total.saturating_sub(PANEL_ROWS));

    for (row, mut text, mut color, mut node) in row_q.iter_mut() {
        let idx = start + row.slot;
        match rows.get(idx) {
            Some(entry) => {
                let (prefix, want_color) = if entry.is_cursor {
                    ("> ", theme::CURSOR)
                } else {
                    ("  ", entry.color)
                };
                let want = format!("{prefix}{}", entry.text);
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
                if text.0 != want {
                    text.0 = want;
                }
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

    #[test]
    fn command_submode_lists_three_rows_with_cursor() {
        let state = MapScreenState {
            cursor: 1,
            ..Default::default()
        };
        let snap = SceneSnapshot::default();
        let rows = panel_rows(&state, &snap, &[], &|_| None);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].text, "Markers");
        assert!(rows[1].is_cursor, "cursor on Wide Scan");
        assert!(!rows[0].is_cursor);
    }

    #[test]
    fn empty_markers_shows_placement_hint() {
        let state = MapScreenState {
            mode: MapSubMode::Markers,
            ..Default::default()
        };
        let snap = SceneSnapshot::default();
        let rows = panel_rows(&state, &snap, &[], &|_| None);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].is_cursor);
    }
}
