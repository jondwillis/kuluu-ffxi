use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::picking::events::{Out, Over};
use bevy::picking::mesh_picking::MeshPickingPlugin;
use bevy::picking::pointer::{PointerButton, PointerId};
use bevy::picking::prelude::*;
use bevy::picking::Pickable;
use bevy::prelude::*;
use ffxi_viewer_wire::EntityKind;

use crate::components::{IsSelf, Nameplate, WorldEntity};
use crate::input_mode::{InputMode, TargetActionState};
use crate::scene::{BakedActor, Target};

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct HoveredEntity {
    pub id: Option<u32>,
}

/// The synthetic picking pointer that `graphics::render_scale` drives over the
/// off-screen 3D target while render scale is active. `None` at native scale
/// (and always on wasm). Hover/target systems treat its hits like the mouse's.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct PickBridgePointer(pub Option<PointerId>);

pub struct PickingPlugin;

/// Gate for world-click targeting. When false, `click_to_target_system`
/// ignores pointer clicks so UI clicks outside the game world (launcher /
/// character-select buttons) can't leak into world targeting and spuriously
/// open the target-action menu. Defaults true for the launcher-less wasm
/// viewer; the native client toggles it per `AppPhase::InGame`.
#[derive(Resource, Debug, Clone, Copy)]
pub struct WorldPickingEnabled(pub bool);

impl Default for WorldPickingEnabled {
    fn default() -> Self {
        Self(true)
    }
}

impl Plugin for PickingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MeshPickingPlugin)
            .init_resource::<HoveredEntity>()
            .init_resource::<PickBridgePointer>()
            .init_resource::<WorldPickingEnabled>()
            .add_systems(
                Update,
                (
                    click_to_target_system,
                    update_hovered_entity_system,
                    sync_entity_hitboxes.run_if(resource_exists::<HitboxAssets>),
                ),
            );
    }
}

#[derive(Resource)]
pub struct HitboxAssets {
    pub mesh: Handle<Mesh>,
    pub material: Handle<StandardMaterial>,
}

impl HitboxAssets {
    pub fn new(meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) -> Self {
        Self {
            mesh: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
            material: materials.add(StandardMaterial {
                base_color: Color::srgba(0.0, 0.0, 0.0, 0.0),
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                ..default()
            }),
        }
    }
}

#[derive(Component)]
pub struct EntityHitbox {
    pub entity_id: u32,
}

#[derive(Component)]
struct HitboxChild(Entity);

fn fallback_hitbox_height(kind: EntityKind) -> f32 {
    match kind {
        EntityKind::Pet => 1.2,
        EntityKind::Mob => 2.0,
        _ => 2.2,
    }
}

const HITBOX_VERTICAL_PAD: f32 = 0.2;

fn hitbox_dims(kind: EntityKind, baked: Option<&BakedActor>) -> (f32, f32, f32) {
    let model_height = baked
        .map(|b| b.actor_height)
        .unwrap_or_else(|| fallback_hitbox_height(kind))
        .max(0.3);
    let half_width = (model_height * 0.35).clamp(0.6, 1.7);
    let box_height = model_height + 2.0 * HITBOX_VERTICAL_PAD;
    let center_y = model_height * 0.5;
    (half_width, box_height, center_y)
}

fn sync_entity_hitboxes(
    mut commands: Commands,
    assets: Res<HitboxAssets>,
    q_entity: Query<
        (
            Entity,
            &WorldEntity,
            Option<&BakedActor>,
            Option<&HitboxChild>,
        ),
        Without<IsSelf>,
    >,
    mut q_box: Query<&mut Transform, With<EntityHitbox>>,
) {
    for (parent_e, world, baked, child) in &q_entity {
        let (half_width, box_height, center_y) = hitbox_dims(world.kind, baked);
        let translation = Vec3::new(0.0, center_y, 0.0);
        let scale = Vec3::new(half_width * 2.0, box_height, half_width * 2.0);

        match child {
            Some(HitboxChild(box_e)) => {
                if let Ok(mut tf) = q_box.get_mut(*box_e) {
                    if tf.scale != scale || tf.translation != translation {
                        tf.translation = translation;
                        tf.scale = scale;
                    }
                }
            }
            None => {
                let box_e = commands
                    .spawn((
                        EntityHitbox {
                            entity_id: world.id,
                        },
                        Mesh3d(assets.mesh.clone()),
                        MeshMaterial3d(assets.material.clone()),
                        Transform {
                            translation,
                            scale,
                            ..default()
                        },
                        Visibility::Visible,
                        NotShadowCaster,
                        NotShadowReceiver,
                        Pickable::default(),
                        ChildOf(parent_e),
                    ))
                    .id();
                commands.entity(parent_e).insert(HitboxChild(box_e));
            }
        }
    }
}

pub fn update_hovered_entity_system(
    mut over_events: MessageReader<Pointer<Over>>,
    mut out_events: MessageReader<Pointer<Out>>,
    world_q: Query<&WorldEntity>,
    parent_q: Query<&ChildOf>,
    nameplate_q: Query<&Nameplate>,
    bridge: Res<PickBridgePointer>,
    mut hovered: ResMut<HoveredEntity>,
) {
    let accept = |id: PointerId| id == PointerId::Mouse || Some(id) == bridge.0;
    for ev in out_events.read() {
        if !accept(ev.pointer_id) {
            continue;
        }
        if let Some(id) = resolve_hit_entity_id(ev.entity, &world_q, &parent_q, &nameplate_q) {
            if hovered.id == Some(id) {
                hovered.id = None;
            }
        }
    }
    for ev in over_events.read() {
        if !accept(ev.pointer_id) {
            continue;
        }
        if let Some(id) = resolve_hit_entity_id(ev.entity, &world_q, &parent_q, &nameplate_q) {
            if id == 0 {
                continue;
            }
            hovered.id = Some(id);
        }
    }
}

fn find_world_entity<'q>(
    mut entity: Entity,
    world_q: &'q Query<&WorldEntity>,
    parent_q: &Query<&ChildOf>,
) -> Option<&'q WorldEntity> {
    for _ in 0..8 {
        if let Ok(w) = world_q.get(entity) {
            return Some(w);
        }
        match parent_q.get(entity) {
            Ok(parent) => entity = parent.0,
            Err(_) => return None,
        }
    }
    None
}

fn resolve_hit_entity_id(
    hit: Entity,
    world_q: &Query<&WorldEntity>,
    parent_q: &Query<&ChildOf>,
    nameplate_q: &Query<&Nameplate>,
) -> Option<u32> {
    if let Some(w) = find_world_entity(hit, world_q, parent_q) {
        return Some(w.id);
    }
    nameplate_q.get(hit).ok().map(|np| np.entity_id)
}

pub fn resolve_click_target(hit_id: Option<u32>, current_target: Option<u32>) -> ClickResolution {
    match hit_id {
        Some(0) => match current_target {
            Some(_) => ClickResolution::Clear,
            None => ClickResolution::OpenContextMenu,
        },

        Some(id) if Some(id) == current_target => ClickResolution::OpenContextMenu,

        Some(id) => ClickResolution::Set(id),

        None => match current_target {
            Some(_) => ClickResolution::Clear,
            None => ClickResolution::OpenContextMenu,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickResolution {
    Set(u32),

    Clear,

    OpenContextMenu,
}

pub fn click_to_target_system(
    mut clicks: MessageReader<Pointer<Click>>,
    q_world: Query<&WorldEntity>,
    q_parent: Query<&ChildOf>,
    q_nameplate: Query<&Nameplate>,
    pointer: Res<crate::mouse::MousePointer>,
    scene: Res<crate::snapshot::SceneState>,
    enabled: Res<WorldPickingEnabled>,
    mut target: ResMut<Target>,
    mut input_mode: ResMut<InputMode>,
) {
    if !enabled.0 {
        clicks.clear();
        return;
    }
    for ev in clicks.read() {
        if ev.button != PointerButton::Primary {
            continue;
        }
        if !matches!(*input_mode, InputMode::World) {
            continue;
        }

        if pointer.left_dragged {
            continue;
        }
        let hit_id = resolve_hit_entity_id(ev.entity, &q_world, &q_parent, &q_nameplate);
        if let Some(id) = hit_id {
            if id != 0
                && scene
                    .snapshot
                    .entities
                    .iter()
                    .any(|e| e.id == id && !e.is_targetable())
            {
                continue;
            }
        }
        match resolve_click_target(hit_id, target.id) {
            ClickResolution::Set(id) => target.id = Some(id),
            ClickResolution::Clear => target.id = None,
            ClickResolution::OpenContextMenu => {
                use crate::hud::action_model;
                let engaged = matches!(
                    scene.snapshot.current_goal,
                    Some(ffxi_viewer_wire::ReactorGoal::Engaged { .. })
                );
                let ctx = action_model::context_for_target(
                    target.id,
                    &scene.snapshot.entities,
                    scene.snapshot.self_pos.pos,
                    scene.snapshot.self_char_id,
                    engaged,
                    crate::hud::menu::any_usable_item(&scene.snapshot),
                );
                if !action_model::build_target_action_entries(&ctx, &crate::hud::overlay::RETAIL)
                    .is_empty()
                {
                    *input_mode = InputMode::TargetAction(TargetActionState::open(ctx));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_on_new_entity_retargets() {
        assert_eq!(
            resolve_click_target(Some(17), Some(99)),
            ClickResolution::Set(17),
        );
    }

    #[test]
    fn click_on_entity_with_no_target_sets_target() {
        assert_eq!(
            resolve_click_target(Some(17), None),
            ClickResolution::Set(17),
        );
    }

    #[test]
    fn click_on_already_selected_opens_menu() {
        assert_eq!(
            resolve_click_target(Some(17), Some(17)),
            ClickResolution::OpenContextMenu,
        );
    }

    #[test]
    fn click_on_self_capsule_with_target_clears() {
        assert_eq!(
            resolve_click_target(Some(0), Some(17)),
            ClickResolution::Clear,
        );
    }

    #[test]
    fn click_on_self_capsule_without_target_opens_menu() {
        assert_eq!(
            resolve_click_target(Some(0), None),
            ClickResolution::OpenContextMenu,
        );
    }

    #[test]
    fn click_on_empty_with_target_clears() {
        assert_eq!(resolve_click_target(None, Some(17)), ClickResolution::Clear,);
    }

    #[test]
    fn click_on_empty_without_target_opens_menu() {
        assert_eq!(
            resolve_click_target(None, None),
            ClickResolution::OpenContextMenu,
        );
    }
}
