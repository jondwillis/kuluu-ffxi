#![cfg(not(target_arch = "wasm32"))]

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::graphics_settings::{GraphicsSettings, MinimapRadar};
use crate::hud::style::theme;

pub mod input;
pub mod overlay;
pub mod retail;
pub mod topdown;

pub const MINIMAP_TEX_SIZE: u32 = 256;

pub const MINIMAP_UI_SIZE_PX: f32 = 192.0;

#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MinimapMode {
    #[default]
    Auto,
    TopDown,
    Retail,
}

#[derive(Resource, Debug, Clone, Copy)]
pub struct MinimapVisible(pub bool);

impl Default for MinimapVisible {
    fn default() -> Self {
        Self(MinimapRadar::default().panel_visible())
    }
}

pub const ZOOM_DEFAULT_RADIUS: f32 = 50.0;

pub const ZOOM_MIN_RADIUS: f32 = 25.0;

pub const ZOOM_STEP_FACTOR: f32 = 1.25;

pub const RECENTER_IDLE_FRAMES: u32 = 180;

pub const RECENTER_LERP_FRAMES: u32 = 30;

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
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn zoom_by(&mut self, factor: f32, zone_half_span: Option<f32>) {
        let current = match self.radius_yalms {
            Some(r) => r,
            None => {
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

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct MinimapView {
    pub pan_offset_xz: Vec2,

    pub idle_frames: u32,

    pub center_world_xz: Option<Vec2>,

    pub visible_aabb: Option<MinimapAabb>,
}

impl MinimapView {
    pub fn clear_for_logout(&mut self) {
        *self = Self::default();
    }

    pub fn pan_is_zero(&self) -> bool {
        self.pan_offset_xz == Vec2::ZERO
    }
}

// research/xim/src/jsMain/kotlin/xim/poc/ui/MapDrawer.kt getPlayerMapCoordinates mapPosX indexes a 512-px map by floor(15f * pos / 512f),
// i.e. a 16×16 grid whose last cell index is 15.
const MAP_GRID_LAST_INDEX: f32 = 15.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinimapAabb {
    pub min: Vec2,

    pub max: Vec2,
}

impl MinimapAabb {
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

    /// Inverse of `world_to_uv` on the XZ plane: a UV in 0..1 back to world
    /// `(x, z)`. Height is not recoverable from a 2D map, so callers supply their
    /// own Y. Used to place map markers at the crosshair.
    pub fn uv_to_world(&self, uv: Vec2) -> Vec2 {
        let span = self.max - self.min;
        Vec2::new(self.min.x + uv.x * span.x, self.min.y + uv.y * span.y)
    }

    pub fn world_to_grid(&self, world: Vec3) -> (char, u8) {
        let uv = self.world_to_uv(world);
        let col = (MAP_GRID_LAST_INDEX * uv.x).floor() as u8;
        let row = (MAP_GRID_LAST_INDEX * uv.y).floor() as u8;
        ((b'A' + col) as char, row + 1)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RetailStatus {
    #[default]
    Idle,
    Loaded,
    Failed(String),
}

#[derive(Resource, Default)]
pub struct MinimapState {
    /// Resolved zone-DAT file id the current top-down bake mirrors (the Mog House
    /// interior and the surrounding city share a zone_id but not a file id).
    pub baked_file_id: Option<u32>,

    /// Quantized time-of-day lighting the current top-down bake was rendered
    /// under; a change re-bakes so the lit map tracks Vana'diel time.
    pub baked_lighting_bucket: Option<u64>,

    pub topdown_image: Option<Handle<Image>>,

    pub aabb: Option<MinimapAabb>,

    pub retail_image: Option<Handle<Image>>,

    pub retail_aabb: Option<MinimapAabb>,

    pub retail_zone: Option<u16>,

    pub retail_status: RetailStatus,

    pub retail_failed_zones: std::collections::HashSet<u16>,
}

impl MinimapState {
    pub fn clear_for_logout(&mut self) {
        *self = Self::default();
    }

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

    pub fn active_aabb(&self, mode: MinimapMode) -> Option<MinimapAabb> {
        match self.resolved_mode(mode) {
            MinimapMode::Retail => self.retail_aabb,
            MinimapMode::TopDown => self.aabb,
            MinimapMode::Auto => unreachable!("resolved_mode never returns Auto"),
        }
    }
}

#[derive(Component)]
pub struct MinimapRoot;

#[derive(Component)]
pub struct MinimapImage;

#[derive(Component)]
pub struct MinimapOverlayLayer;

#[derive(Component)]
pub struct MinimapResetButton;

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
            .init_resource::<overlay::MarkerFilters>()
            .add_plugins((topdown::TopdownBackendPlugin, retail::RetailBackendPlugin))
            .add_systems(
                Update,
                (
                    input::update_minimap_hover_gate,
                    input::handle_minimap_zoom_input,
                    input::handle_minimap_drag_input,
                    input::recenter_minimap_view,
                    update_minimap_view,
                    apply_minimap_radar_setting,
                    (
                        update_minimap_image_source,
                        update_minimap_image_placement,
                        update_minimap_visibility,
                        update_reset_button_visibility,
                        handle_reset_button_click,
                        overlay::update_minimap_overlay,
                        overlay::update_minimap_placed_markers,
                    ),
                )
                    .chain()
                    .before(crate::mouse::mouse_camera_system),
            );
    }
}

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

pub fn zone_half_span(aabb: Option<MinimapAabb>) -> Option<f32> {
    let aabb = aabb?;
    let span = aabb.max - aabb.min;
    Some(span.x.abs().max(span.y.abs()) * 0.5)
}

pub fn spawn_minimap_as_child(p: &mut ChildSpawnerCommands, images: &mut Assets<Image>) {
    let placeholder = images.add(transparent_placeholder_image());
    p.spawn((
        MinimapRoot,
        Node {
            flex_shrink: 0.0,
            width: Val::Px(MINIMAP_UI_SIZE_PX),
            height: Val::Px(MINIMAP_UI_SIZE_PX),
            border: UiRect::all(Val::Px(1.0)),
            // The map image is sized and offset to put the visible world window
            // on this box, so at any zoom but fit it overhangs the widget.
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(theme::MAP_BACKING),
        BorderColor::all(theme::FRAME_EDGE),
        bevy::ui::RelativeCursorPosition::default(),
    ))
    .with_children(|p| {
        p.spawn((
            MinimapImage,
            // Stretch, not the Auto default: Bevy 0.19 draws Auto images
            // aspect-fit centered inside the node, which detaches the picture
            // from the percent-placed marker overlay (kuluu-y4ye).
            ImageNode::new(placeholder).with_mode(NodeImageMode::Stretch),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
        ));

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
        ))
        .with_children(overlay::spawn_minimap_placed_markers);

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
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::CURSOR),
        ))
        .with_children(|btn| {
            btn.spawn((
                Text::new("\u{2715}".to_string()),
                TextFont {
                    font_size: 11.0.into(),
                    ..default()
                },
                TextColor(theme::CURSOR),
            ));
        });
    });
}

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

pub fn update_minimap_image_source(
    state: Res<MinimapState>,
    mode: Res<MinimapMode>,
    mut q: Query<&mut ImageNode, With<MinimapImage>>,
) {
    // Re-evaluated every frame, NOT gated on is_changed(): the MinimapImage node spawns on
    // OnEnter(InGame) and the retail/top-down handle can settle on the same frame it's cleared
    // during a zone change, so a single change edge gets consumed (node absent / handle briefly
    // None) and never re-fires until the next bake — leaving the map blank. The `!=` assign below
    // keeps this idempotent and churn-free.
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

pub fn update_minimap_image_placement(
    state: Res<MinimapState>,
    mode: Res<MinimapMode>,
    view: Res<MinimapView>,
    mut q: Query<&mut Node, With<MinimapImage>>,
) {
    let Ok(mut node) = q.single_mut() else {
        return;
    };
    let full_aabb = match state.resolved_mode(*mode) {
        MinimapMode::Retail => state.retail_aabb,
        MinimapMode::TopDown => state.aabb,
        MinimapMode::Auto => return,
    };
    let placement = view
        .visible_aabb
        .zip(full_aabb)
        .and_then(|(visible, full)| MapImagePlacement::of(full, visible))
        .unwrap_or(MapImagePlacement::FILL);
    placement.apply(&mut node);
}

/// Where a full-zone map image sits inside a map surface, as percentages of that
/// surface's own box.
///
/// The image is *placed*, not cropped: a crop has to clamp at the image edge
/// while the marker overlay maps world→UV against the unclamped visible window,
/// so the two disagree by however far the window overhangs the map — which is
/// exactly what happens near a zone border at low zoom (kuluu-5al7). Scaling and
/// offsetting the whole image cannot drift, and the overhang clips against the
/// surface instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapImagePlacement {
    pub left_pct: f32,
    pub top_pct: f32,
    pub width_pct: f32,
    pub height_pct: f32,
}

impl MapImagePlacement {
    /// The whole image stretched over the whole surface — what a fit-to-zone
    /// view wants, and the safe answer when there is no calibrated AABB to
    /// place against.
    pub const FILL: Self = Self {
        left_pct: 0.0,
        top_pct: 0.0,
        width_pct: 100.0,
        height_pct: 100.0,
    };

    pub fn of(full: MinimapAabb, visible: MinimapAabb) -> Option<Self> {
        let visible_span = visible.max - visible.min;
        if visible_span.x.abs() < f32::EPSILON || visible_span.y.abs() < f32::EPSILON {
            return None;
        }
        let full_span = full.max - full.min;
        let offset = (full.min - visible.min) / visible_span;
        let scale = full_span / visible_span;
        Some(Self {
            left_pct: offset.x * 100.0,
            top_pct: offset.y * 100.0,
            width_pct: scale.x * 100.0,
            height_pct: scale.y * 100.0,
        })
    }

    pub fn apply(self, node: &mut Node) {
        let left = Val::Percent(self.left_pct);
        let top = Val::Percent(self.top_pct);
        let width = Val::Percent(self.width_pct);
        let height = Val::Percent(self.height_pct);
        if node.left != left {
            node.left = left;
        }
        if node.top != top {
            node.top = top;
        }
        if node.width != width {
            node.width = width;
        }
        if node.height != height {
            node.height = height;
        }
    }
}

/// The persisted [`MinimapRadar`] mode owns the widget's open/closed state and
/// the marker categories, but only when it *changes* (startup included): a
/// later `/minimap` toggle is the player's own override and must survive every
/// unrelated graphics-settings write (kuluu-7cqw). `MarkerFilters` has no other
/// writer yet, so this is its sole owner until a per-category UI exists.
pub fn apply_minimap_radar_setting(
    settings: Res<GraphicsSettings>,
    mut applied: Local<Option<MinimapRadar>>,
    mut visible: ResMut<MinimapVisible>,
    mut filters: ResMut<overlay::MarkerFilters>,
) {
    let radar = settings.minimap_radar;
    if *applied == Some(radar) {
        return;
    }
    *applied = Some(radar);
    if visible.0 != radar.panel_visible() {
        visible.0 = radar.panel_visible();
    }
    let want = overlay::MarkerFilters::for_radar(radar);
    if *filters != want {
        *filters = want;
    }
}

pub fn update_minimap_visibility(
    visible: Res<MinimapVisible>,
    mut q: Query<
        (&mut Node, Has<MinimapRoot>),
        Or<(With<MinimapRoot>, With<crate::hud::compass::CompassPanel>)>,
    >,
) {
    for (mut node, is_terrain_map) in &mut q {
        let want = if is_terrain_map == visible.0 {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn uv_to_world_inverts_world_to_uv() {
        let aabb = MinimapAabb {
            min: Vec2::new(-120.0, 40.0),
            max: Vec2::new(80.0, 300.0),
        };
        let world = Vec3::new(-30.0, 12.0, 210.0);
        let uv = aabb.world_to_uv(world);
        let back = aabb.uv_to_world(uv);
        assert!((back.x - world.x).abs() < 1e-3, "x round-trips");
        assert!((back.y - world.z).abs() < 1e-3, "z round-trips into y");
    }

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

    #[test]
    fn resolved_mode_auto_prefers_retail_when_available() {
        let mut state = MinimapState::default();
        assert_eq!(state.resolved_mode(MinimapMode::Auto), MinimapMode::TopDown);
        state.retail_image = Some(Handle::default());
        assert_eq!(state.resolved_mode(MinimapMode::Auto), MinimapMode::Retail);
    }

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

        let inside = aabb.world_to_uv_or_offscreen(Vec3::new(5.0, 0.0, 5.0));
        assert_eq!(inside, Some(Vec2::new(0.5, 0.5)));
    }

    /// The whole point of placing rather than cropping: wherever the image sits,
    /// a world point lands on the same fraction of the surface as the marker
    /// overlay puts its dot (kuluu-5al7).
    #[test]
    fn a_placed_image_agrees_with_the_marker_mapping_over_the_map_edge() {
        let full = MinimapAabb {
            min: Vec2::new(-300.0, -300.0),
            max: Vec2::new(300.0, 300.0),
        };
        // Player hard against the south-west corner, zoomed out far enough that
        // the window overhangs the map on two sides.
        let visible = MinimapAabb {
            min: Vec2::new(-480.0, -420.0),
            max: Vec2::new(-80.0, -20.0),
        };
        let placement = MapImagePlacement::of(full, visible).expect("non-degenerate window");

        for world in [
            Vec3::new(-290.0, 0.0, -290.0),
            Vec3::new(-120.0, 0.0, -300.0),
            Vec3::new(-200.0, 0.0, -100.0),
        ] {
            let marker_uv = visible.world_to_uv(world);
            let in_image = full.world_to_uv(world);
            let on_surface = Vec2::new(
                (placement.left_pct + in_image.x * placement.width_pct) / 100.0,
                (placement.top_pct + in_image.y * placement.height_pct) / 100.0,
            );
            assert!(
                (marker_uv - on_surface).length() < 1e-4,
                "{world:?}: marker at {marker_uv:?} but the map feature at {on_surface:?}"
            );
        }
    }

    #[test]
    fn a_fit_to_zone_view_places_the_image_over_the_whole_surface() {
        let full = MinimapAabb {
            min: Vec2::new(-300.0, -100.0),
            max: Vec2::new(300.0, 500.0),
        };
        assert_eq!(
            MapImagePlacement::of(full, full),
            Some(MapImagePlacement::FILL)
        );
    }

    #[test]
    fn a_degenerate_window_has_no_placement() {
        let full = MinimapAabb {
            min: Vec2::ZERO,
            max: Vec2::splat(10.0),
        };
        let degenerate = MinimapAabb {
            min: Vec2::ZERO,
            max: Vec2::new(0.0, 10.0),
        };
        assert_eq!(MapImagePlacement::of(full, degenerate), None);
    }

    #[test]
    fn zooming_in_scales_the_image_past_the_surface() {
        let full = MinimapAabb {
            min: Vec2::splat(-100.0),
            max: Vec2::splat(100.0),
        };
        let visible = MinimapAabb {
            min: Vec2::splat(-25.0),
            max: Vec2::splat(25.0),
        };
        let placement = MapImagePlacement::of(full, visible).unwrap();
        assert!((placement.width_pct - 400.0).abs() < 1e-3);
        assert!((placement.left_pct + 150.0).abs() < 1e-3);
    }

    #[test]
    fn zoom_by_clamps_to_min_radius() {
        let mut z = MinimapZoom::default();
        for _ in 0..50 {
            z.zoom_by(1.0 / ZOOM_STEP_FACTOR, Some(1000.0));
        }
        assert_eq!(z.radius_yalms, Some(ZOOM_MIN_RADIUS));
    }

    #[test]
    fn zoom_by_switches_to_fit_when_passing_half_span() {
        let mut z = MinimapZoom::default();
        for _ in 0..20 {
            z.zoom_by(ZOOM_STEP_FACTOR, Some(200.0));
        }
        assert_eq!(z.radius_yalms, None);
    }

    #[test]
    fn zoom_is_default_flips_on_change() {
        let mut z = MinimapZoom::default();
        assert!(z.is_default());
        z.zoom_by(1.0 / ZOOM_STEP_FACTOR, Some(1000.0));
        assert!(!z.is_default());
    }

    #[test]
    fn zone_half_span_uses_larger_axis() {
        let aabb = MinimapAabb {
            min: Vec2::new(-50.0, -200.0),
            max: Vec2::new(50.0, 200.0),
        };

        assert_eq!(zone_half_span(Some(aabb)), Some(200.0));
        assert_eq!(zone_half_span(None), None);
    }

    #[test]
    fn world_to_grid_spans_a_to_p_and_1_to_16() {
        let aabb = MinimapAabb {
            min: Vec2::ZERO,
            max: Vec2::splat(512.0),
        };

        assert_eq!(aabb.world_to_grid(Vec3::new(0.0, 0.0, 0.0)), ('A', 1));
        assert_eq!(aabb.world_to_grid(Vec3::new(256.0, 0.0, 256.0)), ('H', 8));
        assert_eq!(aabb.world_to_grid(Vec3::new(512.0, 0.0, 512.0)), ('P', 16));
        // Off-map clamps to the edge cell rather than overflowing past 'P'/16.
        assert_eq!(
            aabb.world_to_grid(Vec3::new(9999.0, 0.0, 9999.0)),
            ('P', 16)
        );
    }

    #[test]
    fn radar_setting_owns_the_defaults_but_not_the_manual_toggle() {
        let mut world = World::new();
        world.init_resource::<GraphicsSettings>();
        world.init_resource::<MinimapVisible>();
        world.init_resource::<overlay::MarkerFilters>();
        let apply = world.register_system(apply_minimap_radar_setting);

        world.run_system(apply).unwrap();
        assert!(!world.resource::<MinimapVisible>().0, "vanilla is closed");
        assert_eq!(
            *world.resource::<overlay::MarkerFilters>(),
            overlay::MarkerFilters::for_radar(MinimapRadar::Vanilla)
        );

        world.resource_mut::<GraphicsSettings>().minimap_radar = MinimapRadar::Enhanced;
        world.run_system(apply).unwrap();
        assert!(world.resource::<MinimapVisible>().0, "enhanced opens it");
        assert_eq!(
            *world.resource::<overlay::MarkerFilters>(),
            overlay::MarkerFilters::for_radar(MinimapRadar::Enhanced)
        );

        world.resource_mut::<MinimapVisible>().0 = false;
        world
            .resource_mut::<overlay::MarkerFilters>()
            .set(overlay::MarkerCategory::Mob, false);
        world.resource_mut::<GraphicsSettings>().fov_deg += 1.0;
        world.run_system(apply).unwrap();
        assert!(
            !world.resource::<MinimapVisible>().0,
            "an unrelated settings write must not reopen the widget"
        );
        assert!(
            !world
                .resource::<overlay::MarkerFilters>()
                .is_visible(overlay::MarkerCategory::Mob),
            "nor clobber the category bitset"
        );

        world.resource_mut::<GraphicsSettings>().minimap_radar = MinimapRadar::Vanilla;
        world.run_system(apply).unwrap();
        assert_eq!(
            *world.resource::<overlay::MarkerFilters>(),
            overlay::MarkerFilters::for_radar(MinimapRadar::Vanilla),
            "switching back re-applies the mode"
        );
    }

    #[test]
    fn radar_presentations_are_exclusive_after_settings_and_manual_toggles() {
        use crate::hud::compass::CompassPanel;
        let mut world = World::new();
        world.init_resource::<GraphicsSettings>();
        world.init_resource::<MinimapVisible>();
        world.init_resource::<overlay::MarkerFilters>();
        let apply = world.register_system(apply_minimap_radar_setting);
        let show = world.register_system(update_minimap_visibility);
        world.run_system(apply).unwrap();
        world.run_system(show).unwrap();
        world.clear_trackers();
        let map = world.spawn((MinimapRoot, Node::default())).id();
        let compass = world.spawn((CompassPanel, Node::default())).id();
        for terrain in [false, true, false] {
            world.resource_mut::<GraphicsSettings>().minimap_radar = if terrain {
                MinimapRadar::Enhanced
            } else {
                MinimapRadar::Vanilla
            };
            world.run_system(apply).unwrap();
            world.run_system(show).unwrap();
            assert_eq!(
                world.get::<Node>(map).unwrap().display == Display::Flex,
                terrain
            );
            assert_eq!(
                world.get::<Node>(compass).unwrap().display == Display::Flex,
                !terrain
            );
        }
        for terrain in [true, false] {
            world.resource_mut::<MinimapVisible>().0 = terrain;
            world.run_system(show).unwrap();
            assert_eq!(
                world.get::<Node>(map).unwrap().display == Display::Flex,
                terrain
            );
            assert_eq!(
                world.get::<Node>(compass).unwrap().display == Display::Flex,
                !terrain
            );
        }
    }

    /// The widget node spawns Display::Flex on zone-in, after the startup
    /// MinimapVisible edge is gone; the vanilla default only sticks because
    /// this system reads the resource unconditionally (kuluu-7cqw).
    #[test]
    fn minimap_root_closes_after_the_visibility_change_edge_is_consumed() {
        let mut world = World::new();
        world.insert_resource(MinimapVisible(false));
        let system = world.register_system(update_minimap_visibility);

        world.run_system(system).unwrap();
        world.clear_trackers();

        let root = world
            .spawn((
                MinimapRoot,
                Node {
                    display: Display::Flex,
                    ..default()
                },
            ))
            .id();
        world.clear_trackers();
        world.run_system(system).unwrap();

        assert_eq!(
            world.get::<Node>(root).unwrap().display,
            Display::None,
            "a node spawned after the change edge still honours the setting"
        );
    }
}
