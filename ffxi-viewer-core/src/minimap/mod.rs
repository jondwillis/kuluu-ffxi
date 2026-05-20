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
            if span.x.abs() < f32::EPSILON { 1.0 } else { span.x },
            if span.y.abs() < f32::EPSILON { 1.0 } else { span.y },
        );
        Vec2::new(
            ((world.x - self.min.x) / safe.x).clamp(0.0, 1.0),
            ((world.z - self.min.y) / safe.y).clamp(0.0, 1.0),
        )
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
            .init_resource::<MinimapState>()
            .add_plugins((topdown::TopdownBackendPlugin, retail::RetailBackendPlugin))
            .add_systems(
                Update,
                (
                    update_minimap_image_source,
                    update_minimap_visibility,
                    overlay::update_minimap_overlay,
                ),
            );
    }
}

/// Spawn the minimap UI root + children. Front-ends call this via
/// `add_hud_spawners` so it lands on the same schedule as the rest of
/// the HUD (today: `OnEnter(AppPhase::InGame)` for native, `Startup`
/// for wasm — but wasm is gated out at the module level).
///
/// Image starts as a 1×1 transparent pixel; the swap system will point
/// it at the real render target / retail texture as soon as a backend
/// produces one.
pub fn spawn_minimap(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = images.add(transparent_placeholder_image());
    commands
        .spawn((
            crate::components::InGameEntity,
            MinimapRoot,
            Node {
                position_type: PositionType::Absolute,
                // Top-right corner. Vana-clock occupies the top-right
                // most slot today (top: 8, right: 8), so the minimap
                // sits beneath it. 56px gap below clears the clock's
                // ~44px height plus a small margin.
                top: Val::Px(56.0),
                right: Val::Px(8.0),
                width: Val::Px(MINIMAP_UI_SIZE_PX),
                height: Val::Px(MINIMAP_UI_SIZE_PX),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::BORDER),
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
        });
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
        let mut state = MinimapState::default();
        state.retail_image = Some(Handle::default());
        assert_eq!(state.resolved_mode(MinimapMode::TopDown), MinimapMode::TopDown);
        assert_eq!(state.resolved_mode(MinimapMode::Retail), MinimapMode::Retail);
    }
}
