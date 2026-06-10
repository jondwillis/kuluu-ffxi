//! Minimap HUD: corner widget that shows an overhead view of the current
//! zone with live entity dots.
//!
//! Two pluggable image backends live behind a single [`MinimapMode`]
//! selector so the same overlay (self arrow, PCs/NPCs, mobs, target ring)
//! works over either:
//!
//!   * [`MinimapMode::TopDown`] — secondary `Camera3d` baked to a render
//!     target once per zone-enter (see [`topdown`]). Hides ceilings/roofs
//!     via a Y-slab cull. Available for every zone we can load MZB for.
//!   * [`MinimapMode::Retail`] — FFXI's stylized in-game map texture (see
//!     [`retail`]). Authentic look but only for zones the retail-map DAT
//!     parser has decoded.
//!   * [`MinimapMode::Auto`] — `Retail` when [`MinimapState::retail_image`]
//!     is populated for the current zone, else `TopDown`.
//!
//! Coordinate convention follows `scene::ffxi_to_bevy` — minimap UVs are
//! computed off the **Bevy XZ** plane (the world floor), with U increasing
//! east (+X) and V increasing south (+Z). The same [`MinimapAabb`]
//! drives both image-source selection (orthographic camera framing) and
//! the entity dot mapper, so a dot always lands at the same UV regardless
//! of which backend is rendering the background.
//!
//! Native-only (`cfg(not(target_arch = "wasm32"))`) because the top-down
//! backend reads [`crate::dat_mzb::MzbCollisionGeometry`], which itself is
//! native-only (sync `fs::read` for DAT bytes).

#![cfg(not(target_arch = "wasm32"))]

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::hud::palette;

pub mod input;
pub mod overlay;
pub mod retail;
pub mod topdown;

/// Pixel size of the minimap UI surface and the top-down render target.
/// 256 is a good compromise: large enough that a city zone reads
/// legibly, small enough that the per-frame orthographic re-render
/// (when enabled) is well under a millisecond on a modern GPU.
pub const MINIMAP_TEX_SIZE: u32 = 256;

/// Edge length of the minimap UI box in CSS pixels. Distinct from
/// [`MINIMAP_TEX_SIZE`] so the texture can be supersampled or
/// undersampled relative to the on-screen footprint without touching the
/// layout.
pub const MINIMAP_UI_SIZE_PX: f32 = 192.0;

/// Which backend supplies the minimap background image.
///
/// `Auto` defers to whichever backend has data for the current zone:
/// retail map if `MinimapState::retail_image` is populated, else the
/// top-down bake.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MinimapMode {
    #[default]
    Auto,
    TopDown,
    Retail,
}

/// Operator visibility toggle. `/minimap hide` flips to `false`; the UI
/// node's `Display` follows.
#[derive(Resource, Debug, Clone, Copy)]
pub struct MinimapVisible(pub bool);

impl Default for MinimapVisible {
    fn default() -> Self {
        Self(true)
    }
}

/// Default zoom radius. Matches the server's ~50yd entity-update
/// radius — the area we actually have live information about. Below
/// this we mostly see ourselves; above it dots represent stale state.
pub const ZOOM_DEFAULT_RADIUS: f32 = 50.0;
/// Tightest zoom-in. Below this the background pixelates badly and
/// dots start overlapping each other.
pub const ZOOM_MIN_RADIUS: f32 = 25.0;
/// Wheel/key step multiplier. 1.25× per tick — ~4 ticks to span
/// 25 → 50 yalms.
pub const ZOOM_STEP_FACTOR: f32 = 1.25;
/// Frames of no pan input before the view auto-recenters on the
/// player. ~3 s at 60 fps.
pub const RECENTER_IDLE_FRAMES: u32 = 180;
/// Frames over which auto-recenter lerps the pan offset back to zero
/// once the idle threshold trips. Half a second.
pub const RECENTER_LERP_FRAMES: u32 = 30;

/// Visible window of the minimap.
///
/// * `radius_yalms = None` → fit the whole zone (the original
///   behavior, useful for orienting yourself relative to landmarks).
/// * `radius_yalms = Some(r)` → show a `2r × 2r` world-XZ window
///   centered on the player (plus any operator pan offset).
///
/// Default is [`ZOOM_DEFAULT_RADIUS`] yalms so the minimap matches
/// the area the server actually streams entity updates for.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct MinimapZoom {
    pub radius_yalms: Option<f32>,
}

impl Default for MinimapZoom {
    fn default() -> Self {
        Self {
            radius_yalms: Some(ZOOM_DEFAULT_RADIUS),
        }
    }
}

impl MinimapZoom {
    /// True iff zoom is at the spawn-time default — gates the
    /// reset-zoom button's visibility.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Multiply current radius by `factor`, clamped to
    /// `[ZOOM_MIN_RADIUS, fit-zone]`. Fit-zone is represented as
    /// `radius_yalms = None`; once a zoom-out push exceeds the
    /// zone's half-span, we switch to `None` so the math doesn't
    /// keep crossing larger and larger numbers.
    pub fn zoom_by(&mut self, factor: f32, zone_half_span: Option<f32>) {
        let current = match self.radius_yalms {
            Some(r) => r,
            None => {
                // Already fit-to-zone; zooming in starts from the
                // zone's half-span (so the first tick is meaningful)
                // and zooming out is a no-op.
                if factor < 1.0 {
                    zone_half_span.unwrap_or(ZOOM_DEFAULT_RADIUS)
                } else {
                    return;
                }
            }
        };
        let next = current * factor;
        if let Some(half) = zone_half_span {
            if next >= half {
                self.radius_yalms = None;
                return;
            }
        }
        self.radius_yalms = Some(next.max(ZOOM_MIN_RADIUS));
    }
}

/// Per-frame view state. Pan offset is owned by the input layer
/// (drag-pan, auto-recenter); `center_world_xz` and `visible_aabb`
/// are derived by [`update_minimap_view`] so the image cropper and
/// the entity overlay agree on what's visible.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct MinimapView {
    /// Drag-induced world-XZ offset from the auto-center (player).
    /// Yalms.
    pub pan_offset_xz: Vec2,
    /// Frames since the user last dragged the minimap. Reset to 0
    /// on any pan input; counted up by the recenter system.
    pub idle_frames: u32,
    /// Center of the visible window in world XZ this frame. `None`
    /// until the first frame after zone-enter.
    pub center_world_xz: Option<Vec2>,
    /// The actual visible AABB this frame. Both the image cropper
    /// and the overlay read this — single source of truth.
    pub visible_aabb: Option<MinimapAabb>,
}

impl MinimapView {
    pub fn clear_for_logout(&mut self) {
        *self = Self::default();
    }
    /// True iff there's no operator-applied pan offset — gates the
    /// reset-zoom button alongside [`MinimapZoom::is_default`].
    pub fn pan_is_zero(&self) -> bool {
        self.pan_offset_xz == Vec2::ZERO
    }
}

/// Axis-aligned bounding box in the Bevy **XZ** plane. The minimap
/// flattens the world by ignoring Y; the AABB tells overlay + camera
/// systems how to map world XZ into the [0, 1] UV space of the minimap
/// texture.
///
/// `min` / `max` use Bevy convention: `min.x` is the western edge, `max.x`
/// is the east, `min.y` (the Vec2 y component, but Bevy world Z) is the
/// north edge, `max.y` is the south. See [`Self::world_to_uv`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinimapAabb {
    /// Bevy world (x, z) of the southwest corner.
    pub min: Vec2,
    /// Bevy world (x, z) of the northeast corner.
    pub max: Vec2,
}

impl MinimapAabb {
    /// Map a Bevy world position to UV space in `[0, 1]²`. Out-of-AABB
    /// positions are clamped — callers that need to detect "off-map"
    /// should check the unclamped fraction themselves. The Y axis is
    /// flipped so that **+Z (south) → V=1**: matches Bevy UI's
    /// top-left-origin coordinate system without an extra flip in the
    /// overlay system.
    pub fn world_to_uv(&self, world: Vec3) -> Vec2 {
        let span = self.max - self.min;
        let safe = Vec2::new(
            if span.x.abs() < f32::EPSILON {
                1.0
            } else {
                span.x
            },
            if span.y.abs() < f32::EPSILON {
                1.0
            } else {
                span.y
            },
        );
        Vec2::new(
            ((world.x - self.min.x) / safe.x).clamp(0.0, 1.0),
            ((world.z - self.min.y) / safe.y).clamp(0.0, 1.0),
        )
    }

    /// Like [`Self::world_to_uv`] but returns `None` when the world
    /// position lies outside the AABB. Used by the entity overlay so
    /// dots can be *culled* when zoomed in (the clamp behavior would
    /// pile every off-screen dot onto the minimap's edges).
    pub fn world_to_uv_or_offscreen(&self, world: Vec3) -> Option<Vec2> {
        let span = self.max - self.min;
        if span.x.abs() < f32::EPSILON || span.y.abs() < f32::EPSILON {
            return None;
        }
        let u = (world.x - self.min.x) / span.x;
        let v = (world.z - self.min.y) / span.y;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        Some(Vec2::new(u, v))
    }
}

/// Shared minimap state — image handles and per-zone AABBs for both
/// backends, plus the active zone id used to gate re-bakes.
///
/// Populated by the per-backend systems:
///   * [`topdown::bake_topdown_system`] fills `topdown_image` + `aabb`
///     on zone-enter.
///   * [`retail::load_retail_for_zone_system`] fills `retail_image` +
///     `retail_aabb` when the retail-map DAT parser has a hit.
///
/// Cleared on `OnExit(InGame)` by the front-end's
/// `despawn_ingame_entities` path? No — the entities go away but
/// this is a `Resource`. Front-ends should call
/// [`MinimapState::clear_for_logout`] from their own session-exit
/// system to drop the per-zone handles. (Per the bevy-lifecycle-symmetry
/// note in MEMORY.md: every cache-holding `Resource` needs an explicit
/// drain.)
#[derive(Resource, Default)]
pub struct MinimapState {
    /// Zone-id of the currently baked / loaded minimap. `None` while
    /// pre-zone-in; updated by the backend systems on bake/load.
    pub zone_id: Option<u16>,
    /// Render target written by the top-down bake. `None` until the
    /// first successful bake for this zone.
    pub topdown_image: Option<Handle<Image>>,
    /// World-XZ AABB used by both the top-down camera framing and the
    /// overlay's `world_to_uv` mapping. Set together with
    /// `topdown_image`.
    pub aabb: Option<MinimapAabb>,
    /// Retail stylized map texture for the current zone, when the
    /// parser has decoded one. `None` is the common case until the
    /// retail-map DAT format work lands.
    pub retail_image: Option<Handle<Image>>,
    /// AABB that maps Bevy world XZ into retail-map UV space. May
    /// differ from `aabb` because retail maps crop / pad differently
    /// from the zone's geometric extent.
    pub retail_aabb: Option<MinimapAabb>,
    /// Negative cache: zones whose retail-map DAT load failed (no
    /// Graphic chunks, unresolved file_id, IO error). Without this,
    /// the auto-loader would re-queue every frame because the success
    /// gate only checks `retail_image.is_some()`. Cleared on logout.
    pub retail_failed_zones: std::collections::HashSet<u16>,
}

impl MinimapState {
    /// Reset all per-zone state. Front-ends call this on session exit
    /// (logout, disconnect) to satisfy the lifecycle-symmetry invariant
    /// — see the type-level docs.
    pub fn clear_for_logout(&mut self) {
        *self = Self::default();
    }

    /// Which backend should drive the visible image right now, resolving
    /// `Auto` against current state.
    pub fn resolved_mode(&self, mode: MinimapMode) -> MinimapMode {
        match mode {
            MinimapMode::Auto => {
                if self.retail_image.is_some() {
                    MinimapMode::Retail
                } else {
                    MinimapMode::TopDown
                }
            }
            explicit => explicit,
        }
    }

    /// AABB associated with whichever backend `resolved_mode` picks.
    /// Used by the overlay so dots align with the visible image.
    pub fn active_aabb(&self, mode: MinimapMode) -> Option<MinimapAabb> {
        match self.resolved_mode(mode) {
            MinimapMode::Retail => self.retail_aabb,
            MinimapMode::TopDown => self.aabb,
            MinimapMode::Auto => unreachable!("resolved_mode never returns Auto"),
        }
    }
}

/// Marker on the corner UI root. The overlay layer is a child of this
/// entity so dots inherit the same absolute-positioned bounds.
#[derive(Component)]
pub struct MinimapRoot;

/// Marker on the inner `ImageNode` whose `image` handle is swapped by
/// [`update_minimap_image_source`].
#[derive(Component)]
pub struct MinimapImage;

/// Marker on the empty child node that the overlay system uses as the
/// dot container. Spawned as a sibling of [`MinimapImage`] so dots
/// render on top via stacking order rather than z-index gymnastics.
#[derive(Component)]
pub struct MinimapOverlayLayer;

/// Marker on the reset-zoom button (corner of the minimap). Visible
/// only when [`MinimapZoom`] differs from its default *or* the user
/// has dragged the view away from the player.
#[derive(Component)]
pub struct MinimapResetButton;

/// Plugin entry. Registers resources + the systems that swap the image
/// source and toggle visibility. Backend bake/load systems live in
/// [`topdown`] / [`retail`] and are added independently so a front-end
/// that wanted only one backend could mix and match (today both are
/// always added).
pub struct MinimapPlugin;

impl Plugin for MinimapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MinimapMode>()
            .init_resource::<MinimapVisible>()
            .init_resource::<MinimapZoom>()
            .init_resource::<MinimapView>()
            .init_resource::<MinimapState>()
            .init_resource::<input::MinimapHoverGate>()
            .init_resource::<input::MinimapDrag>()
            .init_resource::<overlay::MinimapDots>()
            .add_plugins((topdown::TopdownBackendPlugin, retail::RetailBackendPlugin))
            .add_systems(
                Update,
                (
                    // Hover gate must precede the zoom handler (and
                    // the client's camera-zoom handler reads it too).
                    input::update_minimap_hover_gate,
                    // Zoom input runs before the view derives the
                    // visible AABB so a key/wheel tick lands in the
                    // same frame.
                    input::handle_minimap_zoom_input,
                    // Drag-pan + idle-recenter mutate pan_offset_xz,
                    // which update_minimap_view consumes next.
                    input::handle_minimap_drag_input,
                    input::recenter_minimap_view,
                    // View must update before the overlay + image
                    // cropper read it, so they all agree on the
                    // visible AABB for this frame.
                    update_minimap_view,
                    (
                        update_minimap_image_source,
                        update_minimap_crop_rect,
                        update_minimap_visibility,
                        update_reset_button_visibility,
                        handle_reset_button_click,
                        overlay::update_minimap_overlay,
                    ),
                )
                    .chain()
                    // `handle_minimap_zoom_input` zeros `MousePointer::wheel`
                    // when the cursor is over the minimap, to stop a hovered
                    // scroll from also zooming the chase camera. That write
                    // must land *before* `mouse_camera_system` reads the
                    // wheel — both run in `Update`, so without this edge the
                    // scheduler is free to run the camera first and the
                    // suppression no-ops (the camera zooms anyway). Chat gets
                    // the same guarantee for free by consuming in `PreUpdate`;
                    // the minimap zoom handler lives in `Update` (it also
                    // reads per-frame UI hover state), so it states the order
                    // explicitly. Constraining the head of the chain
                    // transitively orders the whole chain before the camera,
                    // which is harmless — none of these read camera state.
                    .before(crate::mouse::mouse_camera_system),
            );
    }
}

/// Resolve [`MinimapZoom`] + [`MinimapView::pan_offset_xz`] + player
/// position against the active backend's full AABB. Writes the
/// resulting `center_world_xz` and `visible_aabb` to [`MinimapView`]
/// so downstream image-crop and overlay systems read a consistent
/// snapshot.
///
/// Behavior:
///   * No AABB available (pre-zone-in) → clear `visible_aabb` to None.
///   * `radius = None` (fit-to-zone) → visible = full AABB, ignore pan.
///   * `radius = Some(r)` + player present → visible is a `2r × 2r`
///     window centered on `player + pan_offset`.
///   * `radius = Some(r)` + no player transform → keep the previous
///     center if any, else fall back to the AABB center. (Brief
///     pre-snapshot frames.)
pub fn update_minimap_view(
    state: Res<MinimapState>,
    mode: Res<MinimapMode>,
    zoom: Res<MinimapZoom>,
    q_self: Query<&Transform, With<crate::components::IsSelf>>,
    mut view: ResMut<MinimapView>,
) {
    let Some(full_aabb) = state.active_aabb(*mode) else {
        if view.visible_aabb.is_some() || view.center_world_xz.is_some() {
            view.visible_aabb = None;
            view.center_world_xz = None;
        }
        return;
    };
    let player_xz = q_self
        .single()
        .ok()
        .map(|t| Vec2::new(t.translation.x, t.translation.z));
    let aabb_center = (full_aabb.min + full_aabb.max) * 0.5;
    let center = match (zoom.radius_yalms, player_xz) {
        (None, _) => aabb_center,
        (Some(_), Some(p)) => p + view.pan_offset_xz,
        (Some(_), None) => view.center_world_xz.unwrap_or(aabb_center),
    };
    let visible = match zoom.radius_yalms {
        None => full_aabb,
        Some(r) => MinimapAabb {
            min: center - Vec2::splat(r),
            max: center + Vec2::splat(r),
        },
    };
    view.center_world_xz = Some(center);
    view.visible_aabb = Some(visible);
}

/// Half the larger zone-axis extent, used as the zoom-out upper
/// bound. Returns `None` when no AABB is loaded yet.
pub fn zone_half_span(aabb: Option<MinimapAabb>) -> Option<f32> {
    let aabb = aabb?;
    let span = aabb.max - aabb.min;
    Some(span.x.abs().max(span.y.abs()) * 0.5)
}

/// Spawn the minimap UI root as a child of an existing parent (the
/// `BottomLeftStack` flex container in `hud::mod`). The parent owns
/// positioning + flex flow; this function only contributes the
/// minimap's own size + background-and-overlay child structure.
///
/// `Image` starts as a 1×1 transparent pixel; the swap system points
/// it at the real render target / retail texture as soon as a backend
/// produces one.
pub fn spawn_minimap_as_child(p: &mut ChildSpawnerCommands, images: &mut Assets<Image>) {
    let placeholder = images.add(transparent_placeholder_image());
    p.spawn((
        // The parent flex container handles position/anchoring. We
        // only declare size + chrome here.
        MinimapRoot,
        Node {
            // `flex_shrink: 0` so when the chat panel grows to its
            // PANEL_MAX_HEIGHT_PX the flex layout doesn't squeeze the
            // minimap below its intended 192×192 footprint.
            flex_shrink: 0.0,
            width: Val::Px(MINIMAP_UI_SIZE_PX),
            height: Val::Px(MINIMAP_UI_SIZE_PX),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(palette::BACKGROUND),
        BorderColor::all(palette::BORDER),
        // Bevy populates `cursor_over()` every frame — the same
        // mechanism `chat_wheel_scroll_system` uses to detect hover.
        bevy::ui::RelativeCursorPosition::default(),
    ))
    .with_children(|p| {
        // Background image (top-down bake or retail texture). Fills
        // the parent; `ImageNode`'s default mode stretches to fit.
        p.spawn((
            MinimapImage,
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
        // Overlay layer for entity dots. Same bounds as the image,
        // higher in the child list so dots render on top. The
        // overlay system spawns/clears its dot nodes as children of
        // this node every frame.
        p.spawn((
            MinimapOverlayLayer,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
        ));
        // Reset-zoom button in the top-right corner. Starts hidden;
        // `update_reset_button_visibility` flips Display::Flex when
        // the user has zoomed or panned away from defaults.
        p.spawn((
            Button,
            MinimapResetButton,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(4.0),
                right: Val::Px(4.0),
                padding: UiRect::axes(Val::Px(5.0), Val::Px(1.0)),
                border: UiRect::all(Val::Px(1.0)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|btn| {
            btn.spawn((
                Text::new("\u{2715}".to_string()), // ✕
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(palette::ACCENT),
            ));
        });
    });
}

/// Toggle the reset-button's `Display` based on whether zoom + pan
/// have diverged from defaults. Hidden when at defaults — no point
/// offering a "reset" affordance with nothing to reset.
pub fn update_reset_button_visibility(
    zoom: Res<MinimapZoom>,
    view: Res<MinimapView>,
    mut q: Query<&mut Node, With<MinimapResetButton>>,
) {
    let want_visible = !zoom.is_default() || !view.pan_is_zero();
    let want = if want_visible {
        Display::Flex
    } else {
        Display::None
    };
    if let Ok(mut node) = q.single_mut() {
        if node.display != want {
            node.display = want;
        }
    }
}

/// React to clicks on [`MinimapResetButton`]: snap zoom + pan back
/// to defaults. Same `Changed<Interaction>` pattern as the chat-tab
/// buttons so the system stays O(buttons-that-just-changed).
pub fn handle_reset_button_click(
    interactions: Query<&Interaction, (With<MinimapResetButton>, Changed<Interaction>)>,
    mut zoom: ResMut<MinimapZoom>,
    mut view: ResMut<MinimapView>,
) {
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
            *zoom = MinimapZoom::default();
            view.pan_offset_xz = Vec2::ZERO;
            view.idle_frames = 0;
        }
    }
}

/// 1×1 fully-transparent RGBA8 image. Used as the initial `ImageNode`
/// source so the UI tree has a valid handle to display before any
/// backend has produced one. Swapped out in
/// [`update_minimap_image_source`] as soon as `MinimapState` has data.
fn transparent_placeholder_image() -> Image {
    Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0u8, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

/// Reactor: when the active mode changes or the backend that owns the
/// current mode publishes a new image handle, repoint the `ImageNode`.
/// Skips the write when the handle already matches so Bevy's
/// `Changed<ImageNode>` filter doesn't churn.
pub fn update_minimap_image_source(
    state: Res<MinimapState>,
    mode: Res<MinimapMode>,
    mut q: Query<&mut ImageNode, With<MinimapImage>>,
) {
    if !state.is_changed() && !mode.is_changed() {
        return;
    }
    let Ok(mut image_node) = q.single_mut() else {
        return;
    };
    let new_handle = match state.resolved_mode(*mode) {
        MinimapMode::Retail => state.retail_image.clone(),
        MinimapMode::TopDown => state.topdown_image.clone(),
        MinimapMode::Auto => None,
    };
    let Some(h) = new_handle else {
        return;
    };
    if image_node.image != h {
        image_node.image = h;
    }
}

/// Set [`ImageNode::rect`] to a sub-region of the source image so the
/// background image shows only the zoomed-in window. Reads the
/// resolved backend's full-zone AABB and computes the
/// visible-AABB-in-full-AABB UV → source-pixel rect.
///
/// When zoom is `None` (fit-to-zone), clears `rect` to `None` so the
/// whole image renders.
///
/// Runs every frame — the visible AABB shifts as the player moves,
/// so the rect needs to follow without waiting on `Changed<...>`.
pub fn update_minimap_crop_rect(
    state: Res<MinimapState>,
    mode: Res<MinimapMode>,
    view: Res<MinimapView>,
    zoom: Res<MinimapZoom>,
    images: Res<Assets<Image>>,
    mut q: Query<&mut ImageNode, With<MinimapImage>>,
) {
    let Ok(mut image_node) = q.single_mut() else {
        return;
    };
    // Fit-to-zone: drop any prior crop so the full image renders.
    if zoom.radius_yalms.is_none() {
        if image_node.rect.is_some() {
            image_node.rect = None;
        }
        return;
    }
    let Some(visible) = view.visible_aabb else {
        return;
    };
    let resolved = state.resolved_mode(*mode);
    let (handle, full_aabb) = match resolved {
        MinimapMode::Retail => (state.retail_image.as_ref(), state.retail_aabb),
        MinimapMode::TopDown => (state.topdown_image.as_ref(), state.aabb),
        MinimapMode::Auto => return,
    };
    let (Some(handle), Some(full)) = (handle, full_aabb) else {
        return;
    };
    let Some(image) = images.get(handle) else {
        // Image hasn't loaded into Assets yet — try again next frame.
        return;
    };
    let size = image.size_f32();
    let full_span = full.max - full.min;
    if full_span.x.abs() < f32::EPSILON || full_span.y.abs() < f32::EPSILON {
        return;
    }
    let uv_min = Vec2::new(
        (visible.min.x - full.min.x) / full_span.x,
        (visible.min.y - full.min.y) / full_span.y,
    );
    let uv_max = Vec2::new(
        (visible.max.x - full.min.x) / full_span.x,
        (visible.max.y - full.min.y) / full_span.y,
    );
    // Clamp to image bounds so a partly-off-edge view (player near
    // the zone boundary) still produces a valid rect — the clamped
    // edge will just show that side of the map as the UI clamps the
    // visible window's overshoot.
    let pixel_rect = Rect {
        min: (uv_min * size).max(Vec2::ZERO),
        max: (uv_max * size).min(size),
    };
    // Skip the write when nothing changed so Bevy's change filters
    // don't churn every frame for a stationary player.
    if image_node.rect != Some(pixel_rect) {
        image_node.rect = Some(pixel_rect);
    }
}

/// Reactor: mirror [`MinimapVisible`] onto the UI root's `Display`.
/// `Display::None` is a layout-level hide (no space reserved) — matches
/// the convention other HUD modules use for `/<panel> hide`.
pub fn update_minimap_visibility(
    visible: Res<MinimapVisible>,
    mut q: Query<&mut Node, With<MinimapRoot>>,
) {
    if !visible.is_changed() {
        return;
    }
    let Ok(mut node) = q.single_mut() else {
        return;
    };
    let want = if visible.0 {
        Display::Flex
    } else {
        Display::None
    };
    if node.display != want {
        node.display = want;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Center of the AABB maps to (0.5, 0.5). Sanity check the
    /// world→UV math before any downstream code relies on it.
    #[test]
    fn world_to_uv_center_is_half_half() {
        let aabb = MinimapAabb {
            min: Vec2::new(-100.0, -100.0),
            max: Vec2::new(100.0, 100.0),
        };
        let uv = aabb.world_to_uv(Vec3::new(0.0, 5.0, 0.0));
        assert!((uv.x - 0.5).abs() < 1e-6);
        assert!((uv.y - 0.5).abs() < 1e-6);
    }

    /// Northwest corner (Bevy -X, -Z) maps to (0, 0). Catches a sign
    /// flip in either axis. Note "north" here is +Z is south per the
    /// docstring on `world_to_uv` — `min.y` is the Vec2 y component,
    /// which holds Bevy world Z.
    #[test]
    fn world_to_uv_min_corner_is_origin() {
        let aabb = MinimapAabb {
            min: Vec2::new(-100.0, -100.0),
            max: Vec2::new(100.0, 100.0),
        };
        let uv = aabb.world_to_uv(Vec3::new(-100.0, 0.0, -100.0));
        assert_eq!(uv, Vec2::new(0.0, 0.0));
    }

    #[test]
    fn world_to_uv_clamps_out_of_bounds() {
        let aabb = MinimapAabb {
            min: Vec2::ZERO,
            max: Vec2::new(10.0, 10.0),
        };
        let uv = aabb.world_to_uv(Vec3::new(50.0, 0.0, -5.0));
        assert_eq!(uv, Vec2::new(1.0, 0.0));
    }

    /// Auto mode promotes to Retail iff the retail image is loaded,
    /// otherwise falls back to TopDown.
    #[test]
    fn resolved_mode_auto_prefers_retail_when_available() {
        let mut state = MinimapState::default();
        assert_eq!(state.resolved_mode(MinimapMode::Auto), MinimapMode::TopDown);
        state.retail_image = Some(Handle::default());
        assert_eq!(state.resolved_mode(MinimapMode::Auto), MinimapMode::Retail);
    }

    /// Explicit modes pass through unchanged regardless of state.
    #[test]
    fn resolved_mode_explicit_passes_through() {
        let state = MinimapState {
            retail_image: Some(Handle::default()),
            ..Default::default()
        };
        assert_eq!(
            state.resolved_mode(MinimapMode::TopDown),
            MinimapMode::TopDown
        );
        assert_eq!(
            state.resolved_mode(MinimapMode::Retail),
            MinimapMode::Retail
        );
    }

    /// Out-of-bounds positions return `None` (used to cull dots at
    /// high zoom). Compare against the clamping variant which
    /// produces (1, 0) for the same input.
    #[test]
    fn world_to_uv_or_offscreen_returns_none_when_outside() {
        let aabb = MinimapAabb {
            min: Vec2::ZERO,
            max: Vec2::new(10.0, 10.0),
        };
        assert_eq!(
            aabb.world_to_uv_or_offscreen(Vec3::new(50.0, 0.0, -5.0)),
            None
        );
        // Inside still returns Some.
        let inside = aabb.world_to_uv_or_offscreen(Vec3::new(5.0, 0.0, 5.0));
        assert_eq!(inside, Some(Vec2::new(0.5, 0.5)));
    }

    /// Zooming in repeatedly clamps to `ZOOM_MIN_RADIUS` — operators
    /// can't scroll past the readable floor.
    #[test]
    fn zoom_by_clamps_to_min_radius() {
        let mut z = MinimapZoom::default();
        for _ in 0..50 {
            z.zoom_by(1.0 / ZOOM_STEP_FACTOR, Some(1000.0));
        }
        assert_eq!(z.radius_yalms, Some(ZOOM_MIN_RADIUS));
    }

    /// Zooming out past the zone half-span switches to fit-to-zone
    /// (`None`) rather than running away to infinity.
    #[test]
    fn zoom_by_switches_to_fit_when_passing_half_span() {
        let mut z = MinimapZoom::default();
        for _ in 0..20 {
            z.zoom_by(ZOOM_STEP_FACTOR, Some(200.0));
        }
        assert_eq!(z.radius_yalms, None);
    }

    /// `is_default` flips false once the operator zooms — gates the
    /// reset-button visibility predicate.
    #[test]
    fn zoom_is_default_flips_on_change() {
        let mut z = MinimapZoom::default();
        assert!(z.is_default());
        z.zoom_by(1.0 / ZOOM_STEP_FACTOR, Some(1000.0));
        assert!(!z.is_default());
    }

    /// `zone_half_span` returns the larger axis to avoid rectangular
    /// zones flipping into fit-mode prematurely on the short axis.
    #[test]
    fn zone_half_span_uses_larger_axis() {
        let aabb = MinimapAabb {
            min: Vec2::new(-50.0, -200.0),
            max: Vec2::new(50.0, 200.0),
        };
        // X span = 100 (half 50), Z span = 400 (half 200) → max is 200.
        assert_eq!(zone_half_span(Some(aabb)), Some(200.0));
        assert_eq!(zone_half_span(None), None);
    }
}
