#![cfg(not(target_arch = "wasm32"))]

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

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
        Self(true)
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

// research/xim/.../ui/MapDrawer.kt:59-60 indexes a 512-px map by floor(15f * pos / 512f),
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
            .add_plugins((topdown::TopdownBackendPlugin, retail::RetailBackendPlugin))
            .add_systems(
                Update,
                (
                    input::update_minimap_hover_gate,
                    input::handle_minimap_zoom_input,
                    input::handle_minimap_drag_input,
                    input::recenter_minimap_view,
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
            ..default()
        },
        BackgroundColor(theme::FRAME_BG),
        BorderColor::all(theme::FRAME_EDGE),
        bevy::ui::RelativeCursorPosition::default(),
    ))
    .with_children(|p| {
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

        crate::hud::compass::spawn_compass_overlay_as_child(p);

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

    let pixel_rect = Rect {
        min: (uv_min * size).max(Vec2::ZERO),
        max: (uv_max * size).min(size),
    };

    if image_node.rect != Some(pixel_rect) {
        image_node.rect = Some(pixel_rect);
    }
}

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
}
