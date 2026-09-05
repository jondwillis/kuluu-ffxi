use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::picking::hover::HoverMap;
use bevy::picking::mesh_picking::MeshPickingPlugin;
use bevy::picking::pointer::{PointerButton, PointerId};
use bevy::picking::prelude::*;
use bevy::picking::Pickable;
use bevy::prelude::*;
use kuluu_snapshot::EntityKind;

use crate::camera::CameraMode;
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
        // Opt-in picking: only entities with a Pickable marker are hit
        // candidates (entity roots + hitbox cuboids). Without this, Bevy's
        // default require_markers=false treats every visible Mesh3d — MZB zone
        // floor, MMB props, water, skinned actor bodies — as a blocking hit, so
        // nearer geometry swallows the ray and entity hitboxes never receive
        // Over/Click (kuluu-k929). Inserted after the plugin so it wins over the
        // plugin's init_resource; the OperatorCamera carries MeshPickingCamera
        // (camera.rs), mandatory once markers are required
        // (bevy_picking mesh_picking/mod.rs early-returns for unmarked cameras).
        app.add_plugins(MeshPickingPlugin)
            .insert_resource(bevy::picking::mesh_picking::MeshPickingSettings {
                require_markers: true,
                ..default()
            })
            .init_resource::<HoveredEntity>()
            .init_resource::<CameraMode>()
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

/// The self hitbox reports its hits without blocking the ray: the box is
/// deliberately oversized, so a target in front of, inside, or behind it must
/// still reach the hover map and outrank self via [`choose_hovered_id`].
const SELF_PICKABLE: Pickable = Pickable {
    should_block_lower: false,
    is_hoverable: true,
};

/// Retail selects your own character on a click like any other actor. The
/// first-person eye sits inside the hitbox, where the box would swallow every
/// world click, so the self box is a click surface in chase view only.
pub fn self_hitbox_pickable(mode: CameraMode) -> Pickable {
    if matches!(mode, CameraMode::Chase) {
        SELF_PICKABLE
    } else {
        Pickable::IGNORE
    }
}

fn sync_entity_hitboxes(
    mut commands: Commands,
    assets: Res<HitboxAssets>,
    camera_mode: Res<CameraMode>,
    q_entity: Query<(
        Entity,
        &WorldEntity,
        Option<&BakedActor>,
        Option<&HitboxChild>,
        Has<IsSelf>,
    )>,
    mut q_box: Query<(&mut Transform, &mut Pickable), With<EntityHitbox>>,
    mut q_root_pick: Query<&mut Pickable, (With<WorldEntity>, Without<EntityHitbox>)>,
) {
    for (parent_e, world, baked, child, is_self) in &q_entity {
        let (half_width, box_height, center_y) = hitbox_dims(world.kind, baked);
        let translation = Vec3::new(0.0, center_y, 0.0);
        let scale = Vec3::new(half_width * 2.0, box_height, half_width * 2.0);
        let pickable = if is_self {
            self_hitbox_pickable(*camera_mode)
        } else {
            Pickable::default()
        };
        // The placeholder ball rides the entity root with a blocking Pickable;
        // self's root gets the same non-blocking surface as its hitbox so a
        // target behind the player is reachable through it too.
        if is_self {
            if let Ok(mut root_pick) = q_root_pick.get_mut(parent_e) {
                if *root_pick != pickable {
                    *root_pick = pickable;
                }
            }
        }

        match child {
            Some(HitboxChild(box_e)) => {
                if let Ok((mut tf, mut pick)) = q_box.get_mut(*box_e) {
                    if tf.scale != scale || tf.translation != translation {
                        tf.translation = translation;
                        tf.scale = scale;
                    }
                    if *pick != pickable {
                        *pick = pickable;
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
                        pickable,
                        ChildOf(parent_e),
                    ))
                    .id();
                commands.entity(parent_e).insert(HitboxChild(box_e));
            }
        }
    }
}

pub fn update_hovered_entity_system(
    hover_map: Res<HoverMap>,
    bridge: Res<PickBridgePointer>,
    scene: Res<crate::snapshot::SceneState>,
    world_q: Query<&WorldEntity>,
    parent_q: Query<&ChildOf>,
    nameplate_q: Query<&Nameplate>,
    mut hovered: ResMut<HoveredEntity>,
) {
    let id = priority_hover_id(
        &hover_map,
        &bridge,
        &world_q,
        &parent_q,
        &nameplate_q,
        &scene.snapshot,
    );
    if hovered.id != id {
        hovered.id = id;
    }
}

/// The self hitbox reports without blocking, so several entities can sit under
/// one pointer; gather every resolved (id, depth) and let [`choose_hovered_id`]
/// rank them. Hits from the mouse and the render-scale bridge pointer count
/// alike; id 0 is the self entity before its char id arrives.
fn priority_hover_id(
    hover_map: &HoverMap,
    bridge: &PickBridgePointer,
    world_q: &Query<&WorldEntity>,
    parent_q: &Query<&ChildOf>,
    nameplate_q: &Query<&Nameplate>,
    snap: &kuluu_snapshot::SceneSnapshot,
) -> Option<u32> {
    let hits = [Some(PointerId::Mouse), bridge.0]
        .into_iter()
        .flatten()
        .filter_map(|pointer| hover_map.get(&pointer))
        .flatten()
        .filter_map(|(entity, hit)| {
            let id = resolve_hit_entity_id(*entity, world_q, parent_q, nameplate_q)?;
            (id != 0).then_some((id, hit.depth))
        })
        // Untargetable entities (invisible "[obj]" event points, hidden NPCs)
        // never hover: the mouse behaves exactly like the click path, which
        // already refuses to select them -- no hover card, no cursor swap,
        // and they can't mask a real entity behind them. Unknown ids
        // (snapshot mid-sync) stay hoverable. Doors pass: is_targetable
        // carves them out for the retail Talk flow.
        .filter(|(id, _)| {
            snap.entities
                .iter()
                .find(|e| e.id == *id)
                .map(|e| e.is_targetable())
                .unwrap_or(true)
        });
    choose_hovered_id(hits, snap.self_char_id)
}

/// Any non-self hit outranks self at any depth — the self box is oversized on
/// purpose, so a target in front of, inside, or behind it always wins; nearest
/// depth decides between non-self hits. Self is picked only when it is the
/// sole entity under the pointer.
fn choose_hovered_id(
    hits: impl IntoIterator<Item = (u32, f32)>,
    self_id: Option<u32>,
) -> Option<u32> {
    let mut nearest_other: Option<(u32, f32)> = None;
    let mut self_hit = false;
    for (id, depth) in hits {
        if Some(id) == self_id {
            self_hit = true;
        } else if nearest_other.is_none_or(|(_, d)| depth < d) {
            nearest_other = Some((id, depth));
        }
    }
    nearest_other
        .map(|(id, _)| id)
        .or(if self_hit { self_id } else { None })
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

pub fn resolve_click_target(
    hit_id: Option<u32>,
    current_target: Option<u32>,
    locked: bool,
) -> ClickResolution {
    let resolution = match hit_id {
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
    };
    match resolution {
        // Clicking the locked target still opens its menu — that is not a
        // de-select, and it is the mouse route to Switch Target/Disengage.
        ClickResolution::Set(_) | ClickResolution::Clear if locked => ClickResolution::Ignored,
        other => other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickResolution {
    Set(u32),

    Clear,

    OpenContextMenu,

    Ignored,
}

pub fn click_to_target_system(
    mut clicks: MessageReader<Pointer<Click>>,
    q_world: Query<&WorldEntity>,
    q_parent: Query<&ChildOf>,
    q_nameplate: Query<&Nameplate>,
    pointer: Res<crate::mouse::MousePointer>,
    scene: Res<crate::snapshot::SceneState>,
    enabled: Res<WorldPickingEnabled>,
    lock_on: Res<crate::lock_on::LockOn>,
    fishing_spot: Res<crate::fishing_spot::FishingSpot>,
    hover_map: Res<HoverMap>,
    bridge: Res<PickBridgePointer>,
    mut target: ResMut<Target>,
    mut input_mode: ResMut<InputMode>,
) {
    if !enabled.0 {
        clicks.clear();
        return;
    }
    let winner = priority_hover_id(
        &hover_map,
        &bridge,
        &q_world,
        &q_parent,
        &q_nameplate,
        &scene.snapshot,
    );
    // One physical click emits one Click per hovered entity — the priority
    // winner's surface, self's non-blocking box beside it, and the full-window
    // surface under everything that resolves to no entity (and is what makes
    // click-on-empty-ground reach us at all). Resolve the click exactly once:
    // the winner's world hit if present, else the background passthrough.
    let mut world_hit: Option<u32> = None;
    let mut background_hit = false;
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
        match resolve_hit_entity_id(ev.entity, &q_world, &q_parent, &q_nameplate) {
            Some(id) if winner == Some(id) => world_hit = Some(id),
            Some(_) => {}
            None => background_hit = true,
        }
    }
    let hit_id = match (world_hit, background_hit) {
        (Some(id), _) => Some(id),
        (None, true) => None,
        (None, false) => return,
    };
    if let Some(id) = hit_id {
        if id != 0
            && scene
                .snapshot
                .entities
                .iter()
                .any(|e| e.id == id && !e.is_targetable())
        {
            return;
        }
    }
    let locked = crate::lock_on::suppresses_retarget(&lock_on, false);
    match resolve_click_target(hit_id, target.id, locked) {
        ClickResolution::Ignored => {}
        ClickResolution::Set(id) => target.id = Some(id),
        ClickResolution::Clear => target.id = None,
        ClickResolution::OpenContextMenu => {
            use crate::hud::action_model;
            let engaged = matches!(
                scene.snapshot.current_goal,
                Some(kuluu_snapshot::ReactorGoal::Engaged { .. })
            );
            let ctx = action_model::context_for_target(
                target.id,
                &scene.snapshot.entities,
                scene.snapshot.self_pos.pos,
                scene.snapshot.self_char_id,
                engaged,
                crate::hud::menu::any_usable_item(&scene.snapshot),
                fishing_spot.0.is_ready(),
            );
            if !action_model::build_target_action_entries(&ctx, &crate::hud::overlay::RETAIL)
                .is_empty()
            {
                *input_mode = InputMode::TargetAction(TargetActionState::open(ctx));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bevy::camera::NormalizedRenderTarget;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::picking::backend::HitData;
    use bevy::picking::pointer::Location;

    fn click_world() -> World {
        let mut world = World::new();
        world.insert_resource(Messages::<Pointer<Click>>::default());
        world.insert_resource(crate::mouse::MousePointer::default());
        let mut scene = crate::snapshot::SceneState::default();
        scene.snapshot.self_char_id = Some(7);
        world.insert_resource(scene);
        world.insert_resource(WorldPickingEnabled(true));
        world.insert_resource(crate::lock_on::LockOn::default());
        world.insert_resource(crate::fishing_spot::FishingSpot::default());
        world.insert_resource(HoverMap::default());
        world.insert_resource(PickBridgePointer::default());
        world.insert_resource(Target::default());
        world.insert_resource(InputMode::default());
        world
    }

    fn spawn_hit_entity(world: &mut World, id: u32) -> (Entity, Entity) {
        let root = world
            .spawn(WorldEntity {
                id,
                act_index: 1,
                kind: EntityKind::Pc,
            })
            .id();
        let hitbox = world
            .spawn((EntityHitbox { entity_id: id }, ChildOf(root)))
            .id();
        (root, hitbox)
    }

    fn hover_hit(depth: f32) -> HitData {
        HitData::new(Entity::PLACEHOLDER, depth, None, None)
    }

    fn set_hover(world: &mut World, hits: &[(Entity, f32)]) {
        let mut map = HoverMap::default();
        let entry = map.0.entry(PointerId::Mouse).or_default();
        for (entity, depth) in hits {
            entry.insert(*entity, hover_hit(*depth));
        }
        world.insert_resource(map);
    }

    fn send_click(world: &mut World, entity: Entity) {
        let msg = Pointer::new(
            PointerId::Mouse,
            Location {
                target: NormalizedRenderTarget::None {
                    width: 1,
                    height: 1,
                },
                position: Vec2::ZERO,
            },
            Click {
                button: PointerButton::Primary,
                hit: hover_hit(1.0),
                duration: std::time::Duration::ZERO,
                count: 1,
            },
            entity,
        );
        world.resource_mut::<Messages<Pointer<Click>>>().write(msg);
    }

    #[test]
    fn click_on_self_hitbox_sets_self_target() {
        let mut world = click_world();
        let (_root, hitbox) = spawn_hit_entity(&mut world, 7);
        set_hover(&mut world, &[(hitbox, 1.0)]);
        send_click(&mut world, hitbox);
        world.run_system_once(click_to_target_system).unwrap();
        assert_eq!(world.resource::<Target>().id, Some(7));
    }

    #[test]
    fn click_with_target_behind_self_hits_the_target() {
        let mut world = click_world();
        let (_self_root, self_box) = spawn_hit_entity(&mut world, 7);
        let (_other_root, other_box) = spawn_hit_entity(&mut world, 42);
        // Self's box is the nearer hit; the target behind it still wins, and
        // bevy emits one Click per hovered entity for the single press.
        set_hover(&mut world, &[(self_box, 1.0), (other_box, 5.0)]);
        send_click(&mut world, self_box);
        send_click(&mut world, other_box);
        world.run_system_once(click_to_target_system).unwrap();
        assert_eq!(world.resource::<Target>().id, Some(42));
        assert!(matches!(world.resource::<InputMode>(), InputMode::World));
    }

    #[test]
    fn click_with_background_surface_still_targets_self() {
        // The observed regression: one physical click on self emitted a Click
        // for the self hitbox AND for an unresolvable full-window surface;
        // processed in the wrong order, the None resolution cleared the
        // just-set target.
        for background_first in [true, false] {
            let mut world = click_world();
            let (_root, hitbox) = spawn_hit_entity(&mut world, 7);
            let background = world.spawn_empty().id();
            set_hover(&mut world, &[(hitbox, 1.0), (background, 0.0)]);
            if background_first {
                send_click(&mut world, background);
                send_click(&mut world, hitbox);
            } else {
                send_click(&mut world, hitbox);
                send_click(&mut world, background);
            }
            world.run_system_once(click_to_target_system).unwrap();
            assert_eq!(
                world.resource::<Target>().id,
                Some(7),
                "background_first={background_first}"
            );
            assert!(matches!(world.resource::<InputMode>(), InputMode::World));
        }
    }

    #[test]
    fn background_only_click_clears_target() {
        let mut world = click_world();
        world.insert_resource(Target { id: Some(42) });
        let background = world.spawn_empty().id();
        set_hover(&mut world, &[(background, 0.0)]);
        send_click(&mut world, background);
        world.run_system_once(click_to_target_system).unwrap();
        assert_eq!(world.resource::<Target>().id, None);
    }

    #[test]
    fn click_on_new_entity_retargets() {
        assert_eq!(
            resolve_click_target(Some(17), Some(99), false),
            ClickResolution::Set(17),
        );
    }

    #[test]
    fn click_on_entity_with_no_target_sets_target() {
        assert_eq!(
            resolve_click_target(Some(17), None, false),
            ClickResolution::Set(17),
        );
    }

    #[test]
    fn click_on_already_selected_opens_menu() {
        assert_eq!(
            resolve_click_target(Some(17), Some(17), false),
            ClickResolution::OpenContextMenu,
        );
    }

    #[test]
    fn click_on_id_zero_with_target_clears() {
        assert_eq!(
            resolve_click_target(Some(0), Some(17), false),
            ClickResolution::Clear,
        );
    }

    #[test]
    fn click_on_id_zero_without_target_opens_menu() {
        assert_eq!(
            resolve_click_target(Some(0), None, false),
            ClickResolution::OpenContextMenu,
        );
    }

    #[test]
    fn self_is_clickable_in_third_person_only() {
        assert_eq!(self_hitbox_pickable(CameraMode::Chase), SELF_PICKABLE);
        assert_eq!(
            self_hitbox_pickable(CameraMode::FirstPerson),
            Pickable::IGNORE
        );
    }

    #[test]
    fn self_hitbox_reports_without_blocking_other_targets() {
        let pickable = self_hitbox_pickable(CameraMode::Chase);
        assert!(pickable.is_hoverable);
        assert!(!pickable.should_block_lower);
    }

    #[test]
    fn other_targets_outrank_self_at_any_depth() {
        // Target behind the player (self box is the nearer hit): it still wins.
        assert_eq!(choose_hovered_id([(7, 1.0), (42, 5.0)], Some(7)), Some(42));
        // Target in front of the player: nearest hit wins as usual.
        assert_eq!(choose_hovered_id([(7, 5.0), (42, 1.0)], Some(7)), Some(42));
        // Nearest of several non-self targets wins, self ignored.
        assert_eq!(
            choose_hovered_id([(42, 3.0), (43, 2.0), (7, 1.0)], Some(7)),
            Some(43)
        );
        // Self alone under the pointer stays selectable.
        assert_eq!(choose_hovered_id([(7, 1.0)], Some(7)), Some(7));
        assert_eq!(choose_hovered_id(std::iter::empty(), Some(7)), None);
    }

    #[test]
    fn self_gets_a_hitbox_that_follows_the_camera_mode() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(HitboxAssets {
            mesh: Handle::default(),
            material: Handle::default(),
        });
        world.insert_resource(CameraMode::Chase);
        let self_e = world
            .spawn((
                IsSelf,
                WorldEntity {
                    id: 7,
                    act_index: 1,
                    kind: EntityKind::Pc,
                },
                Transform::default(),
                Visibility::default(),
            ))
            .id();

        world.run_system_once(sync_entity_hitboxes).unwrap();
        let box_e = world
            .entity(self_e)
            .get::<HitboxChild>()
            .expect("self is a click target too")
            .0;
        assert_eq!(
            world.entity(box_e).get::<Pickable>().copied(),
            Some(SELF_PICKABLE),
        );

        world.insert_resource(CameraMode::FirstPerson);
        world.run_system_once(sync_entity_hitboxes).unwrap();
        assert_eq!(
            world.entity(box_e).get::<Pickable>().copied(),
            Some(Pickable::IGNORE),
        );
    }

    #[test]
    fn click_on_empty_with_target_clears() {
        assert_eq!(
            resolve_click_target(None, Some(17), false),
            ClickResolution::Clear,
        );
    }

    #[test]
    fn click_on_empty_without_target_opens_menu() {
        assert_eq!(
            resolve_click_target(None, None, false),
            ClickResolution::OpenContextMenu,
        );
    }

    #[test]
    fn locked_click_neither_retargets_nor_clears() {
        assert_eq!(
            resolve_click_target(Some(17), Some(99), true),
            ClickResolution::Ignored,
        );
        assert_eq!(
            resolve_click_target(None, Some(99), true),
            ClickResolution::Ignored,
        );
        assert_eq!(
            resolve_click_target(Some(0), Some(99), true),
            ClickResolution::Ignored,
        );
    }

    #[test]
    fn locked_click_on_the_target_still_opens_its_menu() {
        assert_eq!(
            resolve_click_target(Some(17), Some(17), true),
            ClickResolution::OpenContextMenu,
        );
    }
}
