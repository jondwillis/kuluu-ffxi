use std::collections::HashMap;

use bevy::prelude::*;
use bevy::ui::UiTransform;
use ffxi_viewer_wire::EntityKind;

use crate::camera::ChaseCamera;
use crate::components::{InGameEntity, IsSelf, WorldEntity};
use crate::lock_on::LockOn;
use crate::scene::{Target, TrackedEntities};
use crate::snapshot::SceneState;

use super::{MinimapOverlayLayer, MinimapView};

const DOT_DIAMETER_PX: f32 = 6.0;

const SELF_MARKER_PX: f32 = 10.0;

/// Nose protruding past the marker's top edge; without an asymmetric shape the
/// heading rotation of a plain square is invisible at every 90-degree step.
const SELF_MARKER_NOSE_PX: f32 = 4.0;

const SELF_MARKER_COLOR: Color = Color::srgb(0.2, 1.0, 1.0);

const TARGET_RING_PX: f32 = 2.0;

#[derive(Resource, Default)]
pub struct MinimapDots {
    pub by_id: HashMap<u32, Entity>,
}

impl MinimapDots {
    pub fn clear_for_logout(&mut self) {
        self.by_id.clear();
    }
}

#[derive(Component)]
pub struct MinimapDot {
    pub entity_id: u32,
}

const SELF_MARKER_ID: u32 = u32::MAX;

pub fn update_minimap_overlay(
    view: Res<MinimapView>,
    scene_state: Res<SceneState>,
    target: Res<Target>,
    lock_on: Res<LockOn>,
    chase: Res<ChaseCamera>,
    tracked: Res<TrackedEntities>,
    q_overlay_layer: Query<Entity, With<MinimapOverlayLayer>>,
    q_self: Query<&Transform, With<IsSelf>>,
    q_transform: Query<(&Transform, &WorldEntity), Without<IsSelf>>,
    mut dots: ResMut<MinimapDots>,
    mut commands: Commands,
    mut q_dot_node: Query<(&mut Node, &mut BackgroundColor), With<MinimapDot>>,
    mut q_marker_transform: Query<&mut UiTransform, With<MinimapDot>>,
) {
    let Some(aabb) = view.visible_aabb else {
        return;
    };
    let Ok(overlay_layer) = q_overlay_layer.single() else {
        return;
    };

    let snap = &scene_state.snapshot;
    let self_char_id = snap.self_char_id.unwrap_or(0);

    let mut seen: std::collections::HashSet<u32> =
        std::collections::HashSet::with_capacity(tracked.by_id.len() + 1);

    for (transform, world_entity) in q_transform.iter() {
        if self_char_id != 0 && world_entity.id == self_char_id {
            continue;
        }

        let Some(uv) = aabb.world_to_uv_or_offscreen(transform.translation) else {
            continue;
        };
        let is_target = target.id == Some(world_entity.id);
        let is_locked = lock_on.target_id == Some(world_entity.id);
        let color = dot_color(world_entity.kind, is_target, is_locked);
        upsert_dot(
            &mut dots,
            &mut commands,
            overlay_layer,
            world_entity.id,
            uv,
            color,
            DOT_DIAMETER_PX,
            &mut q_dot_node,
            is_target || is_locked,
        );
        seen.insert(world_entity.id);
    }

    if let Ok(self_t) = q_self.single() {
        let uv = aabb
            .world_to_uv_or_offscreen(self_t.translation)
            .unwrap_or_else(|| aabb.world_to_uv(self_t.translation));
        upsert_dot(
            &mut dots,
            &mut commands,
            overlay_layer,
            SELF_MARKER_ID,
            uv,
            SELF_MARKER_COLOR,
            SELF_MARKER_PX,
            &mut q_dot_node,
            false,
        );
        seen.insert(SELF_MARKER_ID);

        if let Some(&marker) = dots.by_id.get(&SELF_MARKER_ID) {
            let rotation = self_marker_rotation(chase.yaw);
            if let Ok(mut ui_transform) = q_marker_transform.get_mut(marker) {
                if ui_transform.rotation != rotation {
                    ui_transform.rotation = rotation;
                }
            } else if let Ok(mut ec) = commands.get_entity(marker) {
                ec.insert(UiTransform::from_rotation(rotation))
                    .with_children(spawn_self_marker_nose);
            }
        }
    }

    let stale: Vec<u32> = dots
        .by_id
        .keys()
        .copied()
        .filter(|id| !seen.contains(id))
        .collect();
    for id in stale {
        if let Some(dot_entity) = dots.by_id.remove(&id) {
            if let Ok(mut ec) = commands.get_entity(dot_entity) {
                ec.despawn();
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn upsert_dot(
    dots: &mut MinimapDots,
    commands: &mut Commands,
    overlay_layer: Entity,
    entity_id: u32,
    uv: Vec2,
    color: Color,
    diameter_px: f32,
    q_dot_node: &mut Query<(&mut Node, &mut BackgroundColor), With<MinimapDot>>,
    with_ring: bool,
) {
    let half = diameter_px * 0.5;
    let left_pct = uv.x * 100.0;
    let top_pct = uv.y * 100.0;

    if let Some(&dot_entity) = dots.by_id.get(&entity_id) {
        if let Ok((mut node, mut bg)) = q_dot_node.get_mut(dot_entity) {
            let want_left = Val::Percent(left_pct);
            let want_top = Val::Percent(top_pct);
            if node.left != want_left {
                node.left = want_left;
            }
            if node.top != want_top {
                node.top = want_top;
            }
            let want_w = Val::Px(diameter_px);
            let want_h = Val::Px(diameter_px);
            if node.width != want_w {
                node.width = want_w;
            }
            if node.height != want_h {
                node.height = want_h;
            }
            let want_margin = UiRect {
                left: Val::Px(-half),
                top: Val::Px(-half),
                ..default()
            };
            if node.margin != want_margin {
                node.margin = want_margin;
            }
            if bg.0 != color {
                bg.0 = color;
            }
        }
        return;
    }

    let border = if with_ring {
        UiRect::all(Val::Px(TARGET_RING_PX))
    } else {
        UiRect::all(Val::Px(0.0))
    };
    let dot_entity = commands
        .spawn((
            InGameEntity,
            MinimapDot { entity_id },
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(left_pct),
                top: Val::Percent(top_pct),
                width: Val::Px(diameter_px),
                height: Val::Px(diameter_px),
                margin: UiRect {
                    left: Val::Px(-half),
                    top: Val::Px(-half),
                    ..default()
                },
                border,
                ..default()
            },
            BackgroundColor(color),
            BorderColor::all(Color::srgb(1.0, 0.95, 0.2)),
            ChildOf(overlay_layer),
        ))
        .id();
    dots.by_id.insert(entity_id, dot_entity);
}

/// Clockwise UI rotation aligning the marker nose (screen-up when unrotated)
/// with the player's forward direction. `world_to_uv` maps world +X to
/// screen-right and +Z to screen-down, and the chase camera sits at
/// +(sin yaw, cos yaw) behind the player (camera.rs chase placement), so
/// forward on the map plane is (-sin yaw, -cos yaw) — screen-up rotated
/// clockwise by -yaw.
fn self_marker_rotation(yaw: f32) -> Rot2 {
    Rot2::radians(-yaw)
}

fn spawn_self_marker_nose(parent: &mut ChildSpawnerCommands) {
    parent.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px((SELF_MARKER_PX - SELF_MARKER_NOSE_PX) * 0.5),
            top: Val::Px(-SELF_MARKER_NOSE_PX),
            width: Val::Px(SELF_MARKER_NOSE_PX),
            height: Val::Px(SELF_MARKER_NOSE_PX),
            ..default()
        },
        BackgroundColor(SELF_MARKER_COLOR),
    ));
}

fn dot_color(kind: EntityKind, is_target: bool, is_locked: bool) -> Color {
    if is_locked {
        return Color::srgb(1.0, 0.4, 0.8);
    }
    if is_target {
        return Color::srgb(1.0, 0.95, 0.2);
    }
    match kind {
        EntityKind::Pc => Color::srgb(0.40, 0.85, 1.00),
        EntityKind::Npc => Color::srgb(0.95, 0.85, 0.30),
        EntityKind::Mob => Color::srgb(0.95, 0.40, 0.40),
        EntityKind::Pet => Color::srgb(0.40, 0.85, 0.50),
        EntityKind::Other => Color::srgb(0.60, 0.60, 0.60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_color_priority_locked_over_target_over_kind() {
        let locked = dot_color(EntityKind::Mob, true, true);
        let targeted = dot_color(EntityKind::Mob, true, false);
        let mob = dot_color(EntityKind::Mob, false, false);

        assert_ne!(locked, targeted);
        assert_ne!(targeted, mob);
        assert_ne!(locked, mob);
    }

    #[test]
    fn dot_color_locked_overrides_pc_kind() {
        let locked_pc = dot_color(EntityKind::Pc, false, true);
        let pc = dot_color(EntityKind::Pc, false, false);
        assert_ne!(locked_pc, pc);
    }

    #[test]
    fn self_marker_rotation_aligns_nose_with_forward() {
        // Screen space is +x right / +y down (Node left/top from world_to_uv);
        // `UiTransform.rotation` applies its Rot2 matrix in that space.
        for heading in [0u8, 32, 64, 96, 128, 160, 192, 224] {
            let yaw = crate::camera::yaw_for_heading(heading);
            let forward_screen = Vec2::new(-yaw.sin(), -yaw.cos());
            let nose_screen = self_marker_rotation(yaw) * Vec2::new(0.0, -1.0);
            assert!(
                (nose_screen - forward_screen).length() < 1e-5,
                "heading {heading}: nose {nose_screen:?} vs forward {forward_screen:?}"
            );
        }
    }

    #[test]
    fn self_marker_rotation_heading_zero_points_screen_right() {
        // FFXI heading 0 faces world +X (camera::yaw_for_heading), and
        // world_to_uv maps +X to screen-right.
        let rot = self_marker_rotation(crate::camera::yaw_for_heading(0));
        let nose = rot * Vec2::new(0.0, -1.0);
        assert!(
            (nose - Vec2::new(1.0, 0.0)).length() < 1e-5,
            "nose {nose:?}"
        );
    }

    #[test]
    fn dot_color_kinds_are_distinct() {
        let pc = dot_color(EntityKind::Pc, false, false);
        let npc = dot_color(EntityKind::Npc, false, false);
        let mob = dot_color(EntityKind::Mob, false, false);
        let pet = dot_color(EntityKind::Pet, false, false);
        assert_ne!(pc, npc);
        assert_ne!(npc, mob);
        assert_ne!(mob, pet);
        assert_ne!(pc, mob);
    }
}
