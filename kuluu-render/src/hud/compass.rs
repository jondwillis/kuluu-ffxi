use std::collections::HashMap;

use bevy::prelude::*;

use crate::camera::OperatorCamera;
use crate::components::{IsSelf, WorldEntity};
use crate::entity_table::EntityTable;
use crate::hud::style::{self, theme};
use crate::snapshot::SceneState;
use crate::ui_element_atlas::{UiElementAtlas, UiElementDatRoot, UiElementQuad};
use kuluu_snapshot::{EntityKind, PartyMember};

#[derive(Component)]
pub struct CompassPanel;

#[derive(Component)]
pub struct CompassLabel {
    direction: Vec2,
    sprite_index: usize,
}

#[derive(Component)]
pub(super) struct CompassRose;

#[derive(Component)]
pub(super) struct CompassArtLoaded;

#[derive(Component)]
pub(super) struct CompassDot {
    entity_id: u32,
}

#[derive(Component)]
pub struct CompassTrackPointer;

// .agents/skills/retail-observe/references/2026-09-09-compass-radar.md, visible output.
const PANEL_WIDTH_PX: f32 = 128.0;
const PANEL_HEIGHT_PX: f32 = 80.0;
const RADAR_RADIUS_PX: f32 = 48.0;
const RADAR_FLATTENING: f32 = 0.5;
const CARDINAL_RADIUS_PX: f32 = 56.0;
const CARDINAL_SIZE_PX: f32 = 16.0;
const DOT_DIAMETER_PX: f32 = 4.0;
// Provisional distance calibration; the supplied screenshot does not establish range.
const RADAR_RANGE_YALMS: f32 = 20.0;
const ROSE_FALLBACK_COLOR: Color = Color::srgba(0.65, 0.70, 0.80, 0.25);
const NORTH_FALLBACK_COLOR: Color = Color::srgb(0.95, 0.30, 0.30);
// .agents/skills/retail-observe/references/2026-09-09-compass-radar.md, installed DAT entries.
const COMPASS_GROUP: &str = "menu    compass ";
const ROSE_SPRITE_INDEX: usize = 0;
const CARDINALS: [(&str, Vec2, usize); 4] = [
    ("N", Vec2::NEG_Y, 4),
    ("S", Vec2::Y, 3),
    ("E", Vec2::X, 1),
    ("W", Vec2::NEG_X, 2),
];

#[cfg(not(target_arch = "wasm32"))]
const TRACK_POINTER_PX: f32 = 11.0;
#[cfg(not(target_arch = "wasm32"))]
const TRACK_POINTER_GAP_PX: f32 = 3.0;

fn centered_node(offset: Vec2, size: Vec2) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(PANEL_WIDTH_PX * 0.5 + offset.x - size.x * 0.5),
        top: Val::Px(PANEL_HEIGHT_PX * 0.5 + offset.y - size.y * 0.5),
        width: Val::Px(size.x),
        height: Val::Px(size.y),
        ..default()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_track_pointer_as_child(p: &mut ChildSpawnerCommands) {
    p.spawn((
        CompassTrackPointer,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(100.0),
            top: Val::Percent(50.0),
            width: Val::Px(TRACK_POINTER_PX),
            height: Val::Px(TRACK_POINTER_PX),
            margin: UiRect {
                left: Val::Px(TRACK_POINTER_GAP_PX),
                top: Val::Px(-TRACK_POINTER_PX * 0.5),
                ..default()
            },
            display: Display::None,
            border_radius: crate::minimap::overlay::pin_border_radius(TRACK_POINTER_PX),
            ..default()
        },
        BackgroundColor(crate::hud::map_screen::TRACKED_MARKER_COLOR),
        UiTransform::default(),
    ));
}

pub fn spawn_compass_as_child(p: &mut ChildSpawnerCommands) {
    p.spawn((
        CompassPanel,
        Node {
            flex_shrink: 0.0,
            width: Val::Px(PANEL_WIDTH_PX),
            height: Val::Px(PANEL_HEIGHT_PX),
            ..default()
        },
        bevy::picking::Pickable::IGNORE,
    ))
    .with_children(|p| {
        p.spawn((
            centered_node(Vec2::ZERO, Vec2::splat(RADAR_RADIUS_PX * 2.0)),
            UiTransform::from_scale(Vec2::new(1.0, RADAR_FLATTENING)),
        ))
        .with_children(|disc| {
            disc.spawn((
                CompassRose,
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::MAX,
                    ..default()
                },
                BorderColor::all(ROSE_FALLBACK_COLOR),
                UiTransform::default(),
            ));
        });
        for (label, direction, sprite_index) in CARDINALS {
            p.spawn((
                CompassLabel {
                    direction,
                    sprite_index,
                },
                centered_node(Vec2::ZERO, Vec2::splat(CARDINAL_SIZE_PX)),
            ))
            .with_children(|letter| {
                letter.spawn((
                    Text::new(label),
                    style::text_font(CARDINAL_SIZE_PX),
                    TextColor(if label == "N" {
                        NORTH_FALLBACK_COLOR
                    } else {
                        theme::TEXT
                    }),
                ));
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        spawn_track_pointer_as_child(p);
    });
}

fn spawn_art(commands: &mut Commands, entity: Entity, quads: Vec<UiElementQuad>) {
    let min = quads
        .iter()
        .map(|q| q.rect.min)
        .fold(Vec2::splat(f32::INFINITY), Vec2::min);
    let max = quads
        .iter()
        .map(|q| q.rect.max)
        .fold(Vec2::splat(f32::NEG_INFINITY), Vec2::max);
    let span = max - min;
    commands
        .entity(entity)
        .insert(CompassArtLoaded)
        .with_children(|art| {
            for quad in quads {
                let offset = (quad.rect.min - min) / span;
                let size = quad.rect.size() / span;
                art.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Percent(offset.x * 100.0),
                        top: Val::Percent(offset.y * 100.0),
                        width: Val::Percent(size.x * 100.0),
                        height: Val::Percent(size.y * 100.0),
                        ..default()
                    },
                    ImageNode {
                        color: quad.color,
                        ..ImageNode::new(quad.image)
                    },
                    bevy::picking::Pickable::IGNORE,
                ));
            }
        });
}

pub(super) fn update_compass_art(
    mut commands: Commands,
    mut atlas: ResMut<UiElementAtlas>,
    dat_root: Res<UiElementDatRoot>,
    mut images: ResMut<Assets<Image>>,
    mut rose: Query<(Entity, &mut Node), (With<CompassRose>, Without<CompassArtLoaded>)>,
    labels: Query<(Entity, &CompassLabel, &Children), Without<CompassArtLoaded>>,
) {
    if dat_root.0.is_none() {
        return;
    }
    for (entity, mut node) in &mut rose {
        if let Some(quads) =
            atlas.ensure_element(COMPASS_GROUP, ROSE_SPRITE_INDEX, &dat_root, &mut images)
        {
            node.border = UiRect::ZERO;
            spawn_art(&mut commands, entity, quads);
        }
    }
    for (entity, label, children) in &labels {
        if let Some(quads) =
            atlas.ensure_element(COMPASS_GROUP, label.sprite_index, &dat_root, &mut images)
        {
            for child in children {
                commands.entity(*child).despawn();
            }
            spawn_art(&mut commands, entity, quads);
        }
    }
}

fn camera_facing(camera: &GlobalTransform) -> Option<Vec2> {
    let forward = camera.forward();
    Vec2::new(forward.x, forward.z).try_normalize()
}

fn radar_offset(facing: Vec2, delta: Vec2) -> Vec2 {
    let right = Vec2::new(-facing.y, facing.x);
    Vec2::new(delta.dot(right), -delta.dot(facing) * RADAR_FLATTENING)
}

pub(super) fn update_compass(
    camera: Query<&GlobalTransform, With<OperatorCamera>>,
    mut labels: Query<(&CompassLabel, &mut Node)>,
    mut rose: Query<&mut UiTransform, With<CompassRose>>,
) {
    let Some(facing) = camera.single().ok().and_then(camera_facing) else {
        return;
    };
    for (label, mut node) in &mut labels {
        let offset = radar_offset(facing, label.direction * CARDINAL_RADIUS_PX);
        let position = centered_node(offset, Vec2::splat(CARDINAL_SIZE_PX));
        if node.left != position.left || node.top != position.top {
            node.left = position.left;
            node.top = position.top;
        }
    }
    for mut transform in &mut rose {
        let rotation = Rot2::radians(track_pointer_theta(facing, Vec2::NEG_Y));
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
    }
}

// Palette categories and enemy-job eligibility: Square Enix manual and moderator;
// .agents/skills/retail-observe/references/2026-09-09-compass-radar.md.
const PC_DOT_COLOR: Color = Color::srgb(0.3, 0.5, 1.0);
const NPC_DOT_COLOR: Color = Color::srgb(0.2, 1.0, 0.3);
const PARTY_DOT_COLOR: Color = Color::srgb(1.0, 0.3, 0.8);
const PET_DOT_COLOR: Color = Color::srgb(1.0, 1.0, 0.2);
const MOB_DOT_COLOR: Color = Color::srgb(1.0, 0.2, 0.2);

fn job_has_enemy_radar(job: u8) -> bool {
    matches!(
        ffxi_vocab::job_names::abbrev(u16::from(job)),
        Some("THF" | "BST" | "RNG" | "NIN" | "SMN" | "BLU")
    )
}

fn radar_color(
    kind: EntityKind,
    id: u32,
    party: &[PartyMember],
    enemies_visible: bool,
) -> Option<Color> {
    match kind {
        EntityKind::Pc if party.iter().any(|m| m.id == id) => Some(PARTY_DOT_COLOR),
        EntityKind::Pc => Some(PC_DOT_COLOR),
        EntityKind::Npc => Some(NPC_DOT_COLOR),
        EntityKind::Pet => Some(PET_DOT_COLOR),
        EntityKind::Mob if enemies_visible => Some(MOB_DOT_COLOR),
        EntityKind::Mob | EntityKind::Other => None,
    }
}

pub(super) fn update_compass_dots(
    mut commands: Commands,
    panel: Query<Entity, With<CompassPanel>>,
    camera: Query<&GlobalTransform, With<OperatorCamera>>,
    self_position: Query<&Transform, With<IsSelf>>,
    entities: Query<(&Transform, &WorldEntity), Without<IsSelf>>,
    table: Res<EntityTable>,
    scene: Res<SceneState>,
    mut dots: Query<(Entity, &CompassDot, &mut Node, &mut BackgroundColor)>,
) {
    let Ok(parent) = panel.single() else { return };
    let basis = camera
        .single()
        .ok()
        .and_then(camera_facing)
        .zip(self_position.single().ok());
    let mut pending = HashMap::new();
    if let Some((facing, self_transform)) = basis {
        let self_member = scene
            .snapshot
            .party
            .iter()
            .find(|m| Some(m.id) == table.self_id());
        let enemies_visible = self_member
            .is_some_and(|m| job_has_enemy_radar(m.main_job) || job_has_enemy_radar(m.sub_job));
        for (transform, entity) in &entities {
            let Some(record) = table.get(entity.id) else {
                continue;
            };
            if record.is_invisible()
                || record.invis_flag()
                || record.name_hidden()
                || !record.entity.is_targetable()
            {
                continue;
            }
            let delta = (transform.translation - self_transform.translation).xz();
            if !delta.is_finite() || delta.length_squared() > RADAR_RANGE_YALMS * RADAR_RANGE_YALMS
            {
                continue;
            }
            let offset = radar_offset(facing, delta) * (RADAR_RADIUS_PX / RADAR_RANGE_YALMS);
            let Some(color) = radar_color(
                entity.kind,
                entity.id,
                &scene.snapshot.party,
                enemies_visible,
            ) else {
                continue;
            };
            pending.insert(entity.id, (offset, color));
        }
    }
    for (entity, dot, mut node, mut color) in &mut dots {
        if let Some((offset, fill)) = pending.remove(&dot.entity_id) {
            let position = centered_node(offset, Vec2::splat(DOT_DIAMETER_PX));
            if node.left != position.left || node.top != position.top {
                node.left = position.left;
                node.top = position.top;
            }
            if color.0 != fill {
                color.0 = fill;
            }
        } else {
            commands.entity(entity).despawn();
        }
    }
    for (entity_id, (offset, color)) in pending {
        commands.spawn((
            CompassDot { entity_id },
            Node {
                border_radius: BorderRadius::MAX,
                ..centered_node(offset, Vec2::splat(DOT_DIAMETER_PX))
            },
            BackgroundColor(color),
            bevy::picking::Pickable::IGNORE,
            ChildOf(parent),
        ));
    }
}

pub fn track_pointer_theta(facing_xz: Vec2, to_target_xz: Vec2) -> f32 {
    let cross = facing_xz.x * to_target_xz.y - facing_xz.y * to_target_xz.x;
    let dot = facing_xz.dot(to_target_xz);
    cross.atan2(dot)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn update_compass_track_pointer(
    scene_state: Res<SceneState>,
    cam_q: Query<&GlobalTransform, With<OperatorCamera>>,
    q_self: Query<&Transform, With<IsSelf>>,
    q_entities: Query<(&Transform, &WorldEntity), Without<IsSelf>>,
    mut ptr_q: Query<(&mut Node, &mut UiTransform), With<CompassTrackPointer>>,
) {
    let theta =
        crate::hud::map_screen::tracked_world(scene_state.snapshot.widescan.tracked, |act_index| {
            q_entities
                .iter()
                .find(|(_, we)| we.act_index == act_index)
                .map(|(t, _)| t.translation)
        })
        .zip(q_self.single().ok())
        .zip(cam_q.single().ok())
        .and_then(|((target, self_t), cam)| {
            let to_target = target - self_t.translation;
            let d = Vec2::new(to_target.x, to_target.z);
            let f3 = cam.forward();
            let f = Vec2::new(f3.x, f3.z);
            (d.length_squared() > f32::EPSILON && f.length_squared() > f32::EPSILON)
                .then(|| track_pointer_theta(f, d))
        });

    for (mut node, mut transform) in ptr_q.iter_mut() {
        match theta {
            Some(theta) => {
                let rot = Rot2::radians(theta - crate::minimap::overlay::PIN_TIP_BEARING);
                if transform.rotation != rot {
                    transform.rotation = rot;
                }
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

#[cfg(test)]
mod tests;
