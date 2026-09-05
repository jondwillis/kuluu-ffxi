use std::collections::HashMap;

use bevy::prelude::*;
use kuluu_snapshot::SceneSnapshot;

use crate::components::{InGameEntity, IsSelf, WorldEntity};
use crate::hud::style::{self, theme};
use crate::hud::zone_flash::ZoneNameResolver;
use crate::input_mode::{InputMode, MenuKind};
use crate::lock_on::LockOn;
use crate::minimap::overlay::{self, MarkerContext, MarkerFilters, MarkerNode, MinimapDot};
use crate::minimap::{MapImagePlacement, MinimapMode, MinimapState};
use crate::nameplate_color::NameColorTable;
use crate::scene::Target;
use crate::snapshot::SceneState;

/// Retail's Map is full-screen: the DAT map image fills the viewport with the 3D
/// world faintly visible behind it, drawn at this alpha (retail composites the
/// map semi-transparently over the scene).
const MAP_IMAGE_ALPHA: f32 = 0.90;

/// Black scrim between the scene and the map image. Without it the parchment's
/// contrast tracks whatever the camera happens to face — a sunlit dune or PC
/// silhouette bleeding through at (1 - MAP_IMAGE_ALPHA) washes the map out —
/// so damp the scene uniformly first; through scrim + image it contributes a
/// steady ~6% instead of a scene-dependent 14% (kuluu-kshw).
const MAP_BACKDROP_ALPHA: f32 = 0.40;

/// Top-right command/submode panel geometry.
const PANEL_WIDTH_PX: f32 = 190.0;
const PANEL_TOP_PX: f32 = 48.0;
const PANEL_RIGHT_PX: f32 = 8.0;

/// Retail anchors the Wide Scan roster on the left screen edge; the other
/// submodes keep the panel top-right (vanilla reference, kuluu-lf42).
const PANEL_LEFT_PX: f32 = 8.0;

/// Retail's list box is a fixed-size frame; a content-sized panel jumps in
/// height whenever the scroll window slides past rows that wrap to two lines
/// (kuluu-xavp). The command submenu stays content-sized like retail's small
/// button stack.
const PANEL_LIST_HEIGHT_PCT: f32 = 55.0;

/// Rows in the reusable panel pool — sized for the wide-scan roster and the
/// Change Map zone list, which are the longest submode lists.
pub const PANEL_ROWS: usize = 24;

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
pub(crate) const TRACKED_MARKER_COLOR: Color = Color::srgb(1.0, 0.30, 0.95);

/// Wide-scan result dot size, slightly under the live entity dots so a scan hit
/// that walks into spawn range reads as a promotion, not a duplicate.
const WIDESCAN_DOT_PX: f32 = 7.0;

/// Retail marks the wide-scan list's selected entry with a large orange
/// crosshair centered on the entity (vanilla Wide Scan reference, kuluu-lf42).
const WIDESCAN_CURSOR_CROSS_SPAN_PX: f32 = 56.0;
const WIDESCAN_CURSOR_CROSS_THICKNESS_PX: f32 = 2.0;
const WIDESCAN_CURSOR_COLOR: Color = Color::srgba(1.0, 0.35, 0.10, 0.90);

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
    pub world: kuluu_snapshot::Vec3,
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
    /// The command submenu's cursor, parked while a submode owns `cursor` and
    /// restored on back-out so leaving Wide Scan lands back on the Wide Scan
    /// row (the generic `MenuStack` gets this per-level; this bespoke state
    /// mirrors it, kuluu-xavp).
    pub command_cursor: usize,
    /// Markers placement crosshair, in map UV (0..1). `None` until Markers opens.
    pub map_cursor: Option<Vec2>,
    /// The (zone, map_index) the image shows. `None` = the live zone, index 0;
    /// `Some` = a Change Map override.
    pub viewed: Option<(u16, u8)>,
    /// Active text-entry buffer while naming a new marker.
    pub marker_entry: Option<String>,
    /// Map zoom radius in yalms. `None` = fit the whole zone (the default, so
    /// the Map opens showing the full map, not a close crop); `Some(r)` = a
    /// player-centered window of half-span `r`. Independent of the minimap zoom.
    pub zoom_radius: Option<f32>,
}

impl MapScreenState {
    /// Reset to the default command submenu (Map open / logout).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Step the map zoom by `factor` (>1 zooms out), snapping to fit when it
    /// reaches the zone span — mirrors `MinimapZoom::zoom_by`.
    pub fn zoom_by(&mut self, factor: f32, zone_half_span: Option<f32>) {
        let full = zone_half_span.unwrap_or(crate::minimap::ZOOM_DEFAULT_RADIUS);
        let current = self.zoom_radius.unwrap_or(full);
        let next = current * factor;
        if next >= full {
            self.zoom_radius = None;
        } else {
            self.zoom_radius = Some(next.max(crate::minimap::ZOOM_MIN_RADIUS));
        }
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

/// The Map screen's own visible AABB, computed from `MapScreenState.zoom_radius`
/// independent of the minimap widget (which keeps its own zoom). Fit-to-zone by
/// default so the Map opens showing the whole map, zoomable via the map's zoom.
#[derive(Resource, Default)]
pub struct MapView {
    pub visible_aabb: Option<crate::minimap::MinimapAabb>,
}

/// Recompute the Map screen's visible AABB each frame it's open, mirroring
/// `update_minimap_view` but driven by the map's own zoom (kuluu-bi1s.3).
pub(crate) fn update_map_view(
    mode: Res<InputMode>,
    map_state: Res<MapScreenState>,
    minimap_state: Res<MinimapState>,
    minimap_mode: Res<MinimapMode>,
    viewed: Res<ViewedMap>,
    scene_state: Res<SceneState>,
    q_self: Query<&Transform, With<IsSelf>>,
    mut map_view: ResMut<MapView>,
) {
    if !map_open(&mode) {
        return;
    }
    let live_zone = scene_state.snapshot.zone_id.unwrap_or(0);
    if map_state.viewed_override(live_zone).is_some() {
        // Change Map preview: show the viewed map whole against its own AABB.
        map_view.visible_aabb = viewed.aabb;
        return;
    }
    let Some(full) = minimap_state.active_aabb(*minimap_mode) else {
        map_view.visible_aabb = None;
        return;
    };
    let visible = match map_state.zoom_radius {
        None => full,
        Some(r) => {
            let center = q_self
                .single()
                .ok()
                .map(|t| Vec2::new(t.translation.x, t.translation.z))
                .unwrap_or_else(|| (full.min + full.max) * 0.5);
            crate::minimap::MinimapAabb {
                min: center - Vec2::splat(r),
                max: center + Vec2::splat(r),
            }
        }
    };
    map_view.visible_aabb = Some(visible);
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

/// Wide-scan result dots keyed by the entry's `act_index`, disjoint from
/// `MapScreenDots` (live entities, keyed by unique_no) so the two layers never
/// collide over keys or stale-sweep each other's nodes.
#[derive(Resource, Default)]
pub struct MapWidescanDots {
    pub by_index: HashMap<u16, Entity>,
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
pub struct MapWidescanDot;

#[derive(Component)]
pub struct MapWidescanCursorMarker;

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
/// server-relative offset, colored by kind. Names come from the entry itself
/// (server `sName` or the session's zone NPC-name DAT enrichment), then the
/// local entity keyed on `act_index`, then a generic kind label — except CHAR
/// entries, which retail hides entirely when unnamed (research/XiPackets
/// world/server/0x00F4; `widescan_entry_hidden`).
pub fn widescan_rows(snap: &SceneSnapshot) -> Vec<WidescanRow> {
    let mut entries: Vec<&kuluu_snapshot::WidescanEntry> = snap.widescan.entries.iter().collect();
    entries.sort_by_key(|e| {
        let (x, z) = (e.rel_x as i64, e.rel_z as i64);
        x * x + z * z
    });
    entries
        .into_iter()
        .filter(|e| !widescan_entry_hidden(e, snap))
        .map(|e| {
            let name = if !e.name.is_empty() {
                e.name.clone()
            } else {
                snap.entities
                    .iter()
                    .find(|ent| ent.act_index == e.act_index)
                    .and_then(|ent| ent.name.clone())
                    .unwrap_or_else(|| widescan_kind_label(e.kind).to_string())
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

/// Retail hides a CHAR-typed entry with no name — no list row, no map dot
/// (research/XiPackets world/server/0x00F4 `Type` note). Shared by the row
/// builder and the dot layer so the two surfaces never disagree.
fn widescan_entry_hidden(e: &kuluu_snapshot::WidescanEntry, snap: &SceneSnapshot) -> bool {
    e.kind == ffxi_proto::map::tracking::kind::CHAR
        && e.name.is_empty()
        && !snap
            .entities
            .iter()
            .any(|ent| ent.act_index == e.act_index && ent.name.is_some())
}

/// Generic row label for an entry the server didn't name and the client hasn't
/// spawned, by the packed `Type` (0x0f4_tracking_list: 0 char, 1 npc, 2 mob;
/// CHAR never reaches here — unnamed CHAR entries are hidden).
fn widescan_kind_label(kind: u8) -> &'static str {
    match kind {
        1 => "NPC",
        2 => "Mob",
        _ => "Unknown",
    }
}

/// The tracked (0x0F5) entity's world position: the live entity transform when
/// spawned, else the raw stream coords. 0x0F5 carries raw LSB values (y =
/// height, z = the second horizontal axis; ffxi-proto decode/widescan.rs
/// WidescanPos), so they swap into the wire convention `ffxi_to_bevy` expects.
/// Shared by the map's tracked marker and the compass track pointer.
pub(crate) fn tracked_world(
    tracked: Option<kuluu_snapshot::WidescanTracked>,
    lookup_local: impl Fn(u16) -> Option<Vec3>,
) -> Option<Vec3> {
    let t = tracked?;
    Some(lookup_local(t.act_index).unwrap_or_else(|| {
        crate::scene::ffxi_to_bevy(kuluu_snapshot::Vec3 {
            x: t.x,
            y: t.z,
            z: t.y,
        })
    }))
}

/// World position of a wide-scan entry. `rel_x`/`rel_z` are LSB's two
/// horizontal deltas (internal x/z; wire x/y after the movement-decode axis
/// swap, see `ffxi-proto decode/movement.rs`), applied to the self transform.
pub fn widescan_dot_world(self_world: Vec3, rel_x: i16, rel_z: i16) -> Vec3 {
    self_world
        + crate::scene::ffxi_to_bevy(kuluu_snapshot::Vec3 {
            x: rel_x as f32,
            y: rel_z as f32,
            z: 0.0,
        })
}

/// First pooled row index of the scroll window: the cursor never sits lower
/// than the window's middle row, so it cannot scroll under the fixed panel's
/// clipped bottom edge even when wrapped two-line rows shrink how many of the
/// `PANEL_ROWS` actually fit (kuluu-pfpt). The list end shows fewer trailing
/// rows instead of pinning a full window.
fn panel_scroll_start(cursor: usize) -> usize {
    cursor.saturating_sub(PANEL_ROWS / 2)
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
            // While naming a marker, the panel is the live text field so the
            // player sees what they type (retail shows an inline entry box).
            if let Some(entry) = &state.marker_entry {
                return vec![PanelRow {
                    text: format!("Name: {entry}_"),
                    color: theme::CURSOR,
                    is_cursor: false,
                }];
            }
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
                // The map image is sized and offset to put the visible world
                // window on this box, so at any zoom but fit it overhangs it.
                overflow: Overflow::clip(),
                ..default()
            },
            ZIndex(style::WINDOW_Z - 2),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, MAP_BACKDROP_ALPHA)),
            ));
            root.spawn((
                MapScreenImage,
                ImageNode {
                    image: placeholder,
                    color: Color::srgba(1.0, 1.0, 1.0, MAP_IMAGE_ALPHA),
                    // Stretch, not the Auto default: Bevy 0.19 draws Auto images
                    // aspect-fit centered inside the node, which detaches the
                    // picture from the percent-placed marker overlay (kuluu-y4ye).
                    image_mode: NodeImageMode::Stretch,
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
                        border_radius: BorderRadius::MAX,
                        ..default()
                    },
                    BackgroundColor(TRACKED_MARKER_COLOR),
                    BorderColor::all(Color::WHITE),
                ));
                let whalf = WIDESCAN_CURSOR_CROSS_SPAN_PX * 0.5;
                let thalf = WIDESCAN_CURSOR_CROSS_THICKNESS_PX * 0.5;
                overlay_layer
                    .spawn((
                        MapWidescanCursorMarker,
                        Node {
                            position_type: PositionType::Absolute,
                            width: Val::Px(WIDESCAN_CURSOR_CROSS_SPAN_PX),
                            height: Val::Px(WIDESCAN_CURSOR_CROSS_SPAN_PX),
                            margin: UiRect {
                                left: Val::Px(-whalf),
                                top: Val::Px(-whalf),
                                ..default()
                            },
                            display: Display::None,
                            ..default()
                        },
                    ))
                    .with_children(|cross| {
                        cross.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(0.0),
                                top: Val::Percent(50.0),
                                width: Val::Percent(100.0),
                                height: Val::Px(WIDESCAN_CURSOR_CROSS_THICKNESS_PX),
                                margin: UiRect {
                                    top: Val::Px(-thalf),
                                    ..default()
                                },
                                ..default()
                            },
                            BackgroundColor(WIDESCAN_CURSOR_COLOR),
                        ));
                        cross.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                top: Val::Px(0.0),
                                left: Val::Percent(50.0),
                                height: Val::Percent(100.0),
                                width: Val::Px(WIDESCAN_CURSOR_CROSS_THICKNESS_PX),
                                margin: UiRect {
                                    left: Val::Px(-thalf),
                                    ..default()
                                },
                                ..default()
                            },
                            BackgroundColor(WIDESCAN_CURSOR_COLOR),
                        ));
                    });
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
                                border_radius: BorderRadius::MAX,
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
    n.overflow = Overflow::clip_y();
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
    let Some(dll) = calib.ensure_dll(root.root()) else {
        *viewed = ViewedMap::default();
        return;
    };
    // `idx` is the row's ordinal in the Change Map list, which indexes the zone's
    // maps in table order — `sub_zone_id` is not a dense index (kuluu-bqm5).
    let Some(record) = dll.zone_maps(zone).get(usize::from(idx)).copied() else {
        *viewed = ViewedMap::default();
        return;
    };
    match crate::minimap::retail::load_zone_map_image(root, &record, &mut images) {
        Some((image, aabb)) => {
            *viewed = ViewedMap {
                key: Some((zone, idx)),
                image: Some(image),
                aabb: Some(aabb),
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
    map_view: Res<MapView>,
    mut q: Query<(&mut ImageNode, &mut Node), With<MapScreenImage>>,
) {
    if !map_open(&mode) {
        return;
    }
    let Ok((mut image_node, mut node)) = q.single_mut() else {
        return;
    };
    let live_zone = scene_state.snapshot.zone_id.unwrap_or(0);
    // Change Map preview: show the whole foreign map, unzoomed.
    if map_state.viewed_override(live_zone).is_some() {
        if let Some(h) = viewed.image.clone() {
            if image_node.image != h {
                image_node.image = h;
            }
        }
        MapImagePlacement::FILL.apply(&mut node);
        return;
    }
    let (handle, full_aabb) = match state.resolved_mode(*minimap_mode) {
        MinimapMode::Retail => (state.retail_image.clone(), state.retail_aabb),
        MinimapMode::TopDown => (state.topdown_image.clone(), state.aabb),
        MinimapMode::Auto => (None, None),
    };
    if let Some(h) = handle {
        if image_node.image != h {
            image_node.image = h;
        }
    }
    let placement = map_view
        .visible_aabb
        .zip(full_aabb)
        .and_then(|(visible, full)| MapImagePlacement::of(full, visible))
        .unwrap_or(MapImagePlacement::FILL);
    placement.apply(&mut node);
}

/// Read-only marker inputs bundled so `update_map_screen_markers` stays under
/// Bevy's 16-parameter system limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MarkerInputs<'w> {
    target: Res<'w, Target>,
    lock_on: Res<'w, LockOn>,
    filters: Res<'w, MarkerFilters>,
    name_colors: Res<'w, NameColorTable>,
    map_state: Res<'w, MapScreenState>,
}

#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_map_screen_markers(
    mode: Res<InputMode>,
    scene_state: Res<SceneState>,
    map_view: Res<MapView>,
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
    mut q_dot: Query<
        MarkerNode,
        (
            With<MinimapDot>,
            Without<MapScreenRoot>,
            Without<MapTrackedMarker>,
            Without<MapPlaceCursor>,
        ),
    >,
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
                ec.try_despawn();
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
                ec.try_despawn();
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
    let (Some(aabb), Ok(overlay_layer)) = (map_view.visible_aabb, q_overlay_layer.single()) else {
        return;
    };
    let ctx = MarkerContext::new(
        &scene_state,
        &markers.target,
        &markers.lock_on,
        &markers.filters,
        &markers.name_colors,
    );

    overlay::sync_marker_layer(
        aabb,
        overlay_layer,
        &ctx,
        &q_self,
        &q_transform,
        &mut dots.by_id,
        &mut commands,
        &mut q_dot,
    );

    if let (Ok(mut label), Ok(self_t)) = (grid_q.single_mut(), q_self.single()) {
        let (col, row) = aabb.world_to_grid(self_t.translation);
        let want = format!("({col}-{row})");
        if **label != want {
            **label = want;
        }
    }

    if let Ok(mut node) = tracked_q.single_mut() {
        let uv = tracked_world(snap.widescan.tracked, |act_index| {
            q_transform
                .iter()
                .find(|(_, we)| we.act_index == act_index)
                .map(|(tf, _)| tf.translation)
        })
        .and_then(|world| aabb.world_to_uv_or_offscreen(world));
        set_overlay_marker(&mut node, uv);
    }

    if let Ok(mut node) = place_q.single_mut() {
        let uv = (markers.map_state.mode == MapSubMode::Markers)
            .then_some(markers.map_state.map_cursor)
            .flatten();
        set_overlay_marker(&mut node, uv);
    }
}

/// Plot the server's wide-scan hits (0x0F4 entries) as dots while the Wide Scan
/// submode is up, positioned by their self-relative offsets so entries far
/// beyond the local spawn range still land on the map (kuluu-iw58). Entries
/// with a locally spawned entity are skipped: the live layer already draws them
/// with their nameplate color and facing. The list's selected row additionally
/// gets a cursor ring over its entity (kuluu-zbve).
pub(crate) fn update_map_widescan_dots(
    mode: Res<InputMode>,
    scene_state: Res<SceneState>,
    map_view: Res<MapView>,
    map_state: Res<MapScreenState>,
    mut dots: ResMut<MapWidescanDots>,
    mut commands: Commands,
    q_overlay_layer: Query<Entity, With<MapScreenOverlayLayer>>,
    q_self: Query<&Transform, With<IsSelf>>,
    q_local: Query<(&Transform, &WorldEntity), Without<IsSelf>>,
    mut q_dot: Query<&mut Node, (With<MapWidescanDot>, Without<MapWidescanCursorMarker>)>,
    mut cursor_q: Query<&mut Node, (With<MapWidescanCursorMarker>, Without<MapWidescanDot>)>,
) {
    let snap = &scene_state.snapshot;
    let live_zone = snap.zone_id.unwrap_or(0);
    let active = map_open(&mode)
        && map_state.mode == MapSubMode::WideScan
        && map_state.viewed_override(live_zone).is_none();
    let inputs = map_view
        .visible_aabb
        .zip(q_self.single().ok())
        .zip(q_overlay_layer.single().ok())
        .filter(|_| active);
    let Some(((aabb, self_t), overlay_layer)) = inputs else {
        for (_, dot) in dots.by_index.drain() {
            if let Ok(mut ec) = commands.get_entity(dot) {
                ec.try_despawn();
            }
        }
        if let Ok(mut node) = cursor_q.single_mut() {
            if node.display != Display::None {
                node.display = Display::None;
            }
        }
        return;
    };

    let selected_uv = {
        let rows = widescan_rows(snap);
        rows.get(map_state.cursor).and_then(|row| {
            let world = q_local
                .iter()
                .find(|(_, we)| we.act_index == row.act_index)
                .map(|(t, _)| t.translation)
                .or_else(|| {
                    snap.widescan
                        .entries
                        .iter()
                        .find(|e| e.act_index == row.act_index)
                        .map(|e| widescan_dot_world(self_t.translation, e.rel_x, e.rel_z))
                })?;
            aabb.world_to_uv_or_offscreen(world)
        })
    };
    if let Ok(mut node) = cursor_q.single_mut() {
        set_overlay_marker(&mut node, selected_uv);
    }

    let local: std::collections::HashSet<u16> =
        q_local.iter().map(|(_, we)| we.act_index).collect();
    let mut seen: std::collections::HashSet<u16> =
        std::collections::HashSet::with_capacity(snap.widescan.entries.len());
    let half = WIDESCAN_DOT_PX * 0.5;
    for e in &snap.widescan.entries {
        if local.contains(&e.act_index) || widescan_entry_hidden(e, snap) {
            continue;
        }
        let world = widescan_dot_world(self_t.translation, e.rel_x, e.rel_z);
        let Some(uv) = aabb.world_to_uv_or_offscreen(world) else {
            continue;
        };
        seen.insert(e.act_index);
        match dots.by_index.get(&e.act_index) {
            Some(&dot) => {
                if let Ok(mut node) = q_dot.get_mut(dot) {
                    set_overlay_marker(&mut node, Some(uv));
                }
            }
            None => {
                let dot = commands
                    .spawn((
                        InGameEntity,
                        MapWidescanDot,
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Percent(uv.x * 100.0),
                            top: Val::Percent(uv.y * 100.0),
                            width: Val::Px(WIDESCAN_DOT_PX),
                            height: Val::Px(WIDESCAN_DOT_PX),
                            margin: UiRect {
                                left: Val::Px(-half),
                                top: Val::Px(-half),
                                ..default()
                            },
                            border_radius: BorderRadius::MAX,
                            ..default()
                        },
                        BackgroundColor(overlay::widescan_color(e.kind)),
                        ChildOf(overlay_layer),
                    ))
                    .id();
                dots.by_index.insert(e.act_index, dot);
            }
        }
    }

    let stale: Vec<u16> = dots
        .by_index
        .keys()
        .copied()
        .filter(|idx| !seen.contains(idx))
        .collect();
    for idx in stale {
        if let Some(dot) = dots.by_index.remove(&idx) {
            if let Ok(mut ec) = commands.get_entity(dot) {
                ec.try_despawn();
            }
        }
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
    map_view: Res<MapView>,
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
        None => (live_zone, map_view.visible_aabb),
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
        let (left, right) = if map_state.mode == MapSubMode::WideScan {
            (Val::Px(PANEL_LEFT_PX), Val::Auto)
        } else {
            (Val::Auto, Val::Px(PANEL_RIGHT_PX))
        };
        if node.left != left {
            node.left = left;
        }
        if node.right != right {
            node.right = right;
        }
        let height = if map_state.mode == MapSubMode::Command {
            Val::Auto
        } else {
            Val::Percent(PANEL_LIST_HEIGHT_PCT)
        };
        if node.height != height {
            node.height = height;
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

    let start = panel_scroll_start(map_state.cursor);

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
    use kuluu_snapshot::{Entity, EntityKind, Vec3, WidescanEntry, WidescanList};

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

    fn named_entry(act_index: u16, level: u8, kind: u8, rel_x: i16, rel_z: i16) -> WidescanEntry {
        WidescanEntry {
            name: format!("E{act_index}"),
            ..entry(act_index, level, kind, rel_x, rel_z)
        }
    }

    #[test]
    fn widescan_rows_sort_nearest_first() {
        let snap = SceneSnapshot {
            widescan: WidescanList {
                entries: vec![
                    named_entry(1, 5, 2, 30, 40), // dist 50
                    named_entry(2, 3, 2, 3, 4),   // dist 5
                    named_entry(3, 9, 1, 6, 8),   // dist 10
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
    fn widescan_unnamed_entries_get_generic_kind_labels() {
        // No name and no local entity (beyond spawn range): npc/mob entries
        // must still be listed and trackable (kuluu-iw58), while an unnamed
        // CHAR entry is hidden like retail (XiPackets 0x00F4 Type note).
        let snap = SceneSnapshot {
            widescan: WidescanList {
                entries: vec![
                    entry(48, 24, 2, 1, 1),
                    entry(190, 0, 1, 2, 2),
                    entry(300, 0, 0, 3, 3),
                ],
                tracked: None,
            },
            ..Default::default()
        };
        let rows = widescan_rows(&snap);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["Mob (Lv24)", "NPC"], "unnamed CHAR is hidden");
        assert_eq!(rows[0].act_index, 48, "generic rows keep their act_index");
    }

    #[test]
    fn widescan_named_entry_keeps_server_name() {
        // The session enriches empty sName from the zone NPC-name DAT before
        // the entry reaches the snapshot; a named entry must never fall back
        // to the generic kind label.
        let snap = SceneSnapshot {
            widescan: WidescanList {
                entries: vec![named_entry(9, 12, 2, 1, 1)],
                tracked: None,
            },
            ..Default::default()
        };
        assert_eq!(widescan_rows(&snap)[0].label, "E9 (Lv12)");
    }

    #[test]
    fn panel_scroll_keeps_the_cursor_at_or_above_the_window_middle() {
        // A cursor at the very end of a long list must stay at visual row
        // PANEL_ROWS/2, never drift toward the clipped bottom (kuluu-pfpt).
        let last = 40;
        assert_eq!(last - panel_scroll_start(last), PANEL_ROWS / 2);
        assert_eq!(panel_scroll_start(3), 0, "short lists start at the top");
    }

    #[test]
    fn widescan_dot_world_maps_rel_offsets_onto_the_map_plane() {
        // rel_x/rel_z are LSB's horizontal deltas; wire y = lsb_z, so Bevy
        // X += rel_x and Z -= rel_z while height is untouched (scene::ffxi_to_bevy).
        let world = widescan_dot_world(bevy::math::Vec3::new(10.0, 5.0, -20.0), 3, 4);
        assert_eq!(world, bevy::math::Vec3::new(13.0, 5.0, -24.0));
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
            name_vis: None,
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            mount: None,
            status: 0,
            char_flags: Default::default(),
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
    fn zoom_defaults_to_fit_and_snaps_back_out() {
        let mut s = MapScreenState::default();
        assert_eq!(s.zoom_radius, None, "opens fit-to-zone");
        s.zoom_by(1.0 / crate::minimap::ZOOM_STEP_FACTOR, Some(100.0));
        assert!(s.zoom_radius.is_some(), "zoom in leaves fit");
        // Zooming back out past the zone span snaps to fit (None) again.
        for _ in 0..12 {
            s.zoom_by(crate::minimap::ZOOM_STEP_FACTOR, Some(100.0));
        }
        assert_eq!(s.zoom_radius, None, "zoom out past the zone span refits");
    }

    #[test]
    fn marker_entry_renders_as_live_text_field() {
        let state = MapScreenState {
            mode: MapSubMode::Markers,
            marker_entry: Some("Camp".to_string()),
            ..Default::default()
        };
        let snap = SceneSnapshot::default();
        let rows = panel_rows(&state, &snap, &[], &|_| None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "Name: Camp_");
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
