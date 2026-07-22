use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::ui::UiTransform;
use ffxi_viewer_wire::EntityKind;

use crate::camera::ChaseCamera;
use crate::components::{InGameEntity, IsSelf, WorldEntity};
use crate::lock_on::LockOn;
use crate::scene::Target;
use crate::snapshot::SceneState;

use super::{MinimapAabb, MinimapOverlayLayer, MinimapView};

const DOT_DIAMETER_PX: f32 = 6.0;

const SELF_MARKER_PX: f32 = 10.0;

/// Nose protruding past the marker's top edge; without an asymmetric shape the
/// heading rotation of a plain square is invisible at every 90-degree step.
const SELF_MARKER_NOSE_PX: f32 = 4.0;

const SELF_MARKER_COLOR: Color = Color::srgb(0.2, 1.0, 1.0);

const PARTY_MARKER_COLOR: Color = Color::srgb(0.30, 1.00, 0.55);

const LOCKED_MARKER_COLOR: Color = Color::srgb(1.0, 0.4, 0.8);

const TARGET_MARKER_COLOR: Color = Color::srgb(1.0, 0.95, 0.2);

const TARGET_RING_PX: f32 = 2.0;

/// Marker categories. `SelfMarker` and `Target` are role overlays that win
/// over kind; `Party` is snapshot party-list membership; the rest are per
/// `EntityKind`. The `MarkerFilters` bitset and the legend both key off this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkerCategory {
    SelfMarker,
    Party,
    Pc,
    Npc,
    Mob,
    Pet,
    Target,
}

impl MarkerCategory {
    pub const ALL: [MarkerCategory; 7] = [
        MarkerCategory::SelfMarker,
        MarkerCategory::Party,
        MarkerCategory::Pc,
        MarkerCategory::Npc,
        MarkerCategory::Mob,
        MarkerCategory::Pet,
        MarkerCategory::Target,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MarkerCategory::SelfMarker => "Self",
            MarkerCategory::Party => "Party",
            MarkerCategory::Pc => "PC",
            MarkerCategory::Npc => "NPC",
            MarkerCategory::Mob => "Mob",
            MarkerCategory::Pet => "Pet",
            MarkerCategory::Target => "Target",
        }
    }

    fn bit(self) -> u8 {
        let idx = match self {
            MarkerCategory::SelfMarker => 0,
            MarkerCategory::Party => 1,
            MarkerCategory::Pc => 2,
            MarkerCategory::Npc => 3,
            MarkerCategory::Mob => 4,
            MarkerCategory::Pet => 5,
            MarkerCategory::Target => 6,
        };
        1 << idx
    }

    /// Legend swatch drawn from the same palette the dots use, so the key reads
    /// as the map itself.
    pub fn swatch_color(self) -> Color {
        match self {
            MarkerCategory::SelfMarker => SELF_MARKER_COLOR,
            MarkerCategory::Party => PARTY_MARKER_COLOR,
            MarkerCategory::Target => TARGET_MARKER_COLOR,
            MarkerCategory::Pc => dot_color(EntityKind::Pc, false, false, false),
            MarkerCategory::Npc => dot_color(EntityKind::Npc, false, false, false),
            MarkerCategory::Mob => dot_color(EntityKind::Mob, false, false, false),
            MarkerCategory::Pet => dot_color(EntityKind::Pet, false, false, false),
        }
    }
}

const ALL_CATEGORIES_MASK: u8 = (1 << MarkerCategory::ALL.len()) - 1;

/// Session-persistent per-category visibility bitset; a cleared bit hides that
/// category on BOTH the minimap and the Map screen through the shared
/// `sync_marker_layer`. Every category starts visible.
#[derive(Resource, Debug, Clone, Copy)]
pub struct MarkerFilters {
    bits: u8,
}

impl Default for MarkerFilters {
    fn default() -> Self {
        Self {
            bits: ALL_CATEGORIES_MASK,
        }
    }
}

impl MarkerFilters {
    pub fn is_visible(&self, category: MarkerCategory) -> bool {
        self.bits & category.bit() != 0
    }

    pub fn toggle(&mut self, category: MarkerCategory) {
        self.bits ^= category.bit();
    }

    pub fn set(&mut self, category: MarkerCategory, visible: bool) {
        if visible {
            self.bits |= category.bit();
        } else {
            self.bits &= !category.bit();
        }
    }
}

/// Which legend/filter bucket a world dot belongs to. Role overlays win over
/// kind so a locked party mob still filters and colors as Target.
fn marker_category(kind: EntityKind, is_party: bool, is_role_target: bool) -> MarkerCategory {
    if is_role_target {
        return MarkerCategory::Target;
    }
    if is_party {
        return MarkerCategory::Party;
    }
    match kind {
        EntityKind::Pc => MarkerCategory::Pc,
        EntityKind::Npc | EntityKind::Other => MarkerCategory::Npc,
        EntityKind::Mob => MarkerCategory::Mob,
        EntityKind::Pet => MarkerCategory::Pet,
    }
}

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

pub const SELF_MARKER_ID: u32 = u32::MAX;

/// Placed user markers drawn on the live minimap (same source as the Map
/// screen), so a dropped marker shows on both surfaces (kuluu-qfmx).
const MINIMAP_PLACED_PX: f32 = 6.0;
const MINIMAP_PLACED_POOL: usize = 32;
const MINIMAP_PLACED_COLOR: Color = Color::srgb(1.0, 0.55, 0.10);

#[derive(Component, Clone, Copy)]
pub struct MinimapPlacedMarker {
    pub slot: usize,
}

pub fn spawn_minimap_placed_markers(layer: &mut ChildSpawnerCommands) {
    let half = MINIMAP_PLACED_PX * 0.5;
    for slot in 0..MINIMAP_PLACED_POOL {
        layer.spawn((
            MinimapPlacedMarker { slot },
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(MINIMAP_PLACED_PX),
                height: Val::Px(MINIMAP_PLACED_PX),
                margin: UiRect {
                    left: Val::Px(-half),
                    top: Val::Px(-half),
                    ..default()
                },
                display: Display::None,
                ..default()
            },
            BackgroundColor(MINIMAP_PLACED_COLOR),
        ));
    }
}

pub fn update_minimap_placed_markers(
    view: Res<MinimapView>,
    scene_state: Res<SceneState>,
    markers: Res<crate::hud::map_screen::MapMarkers>,
    mut q: Query<(&MinimapPlacedMarker, &mut Node)>,
) {
    let zone = scene_state.snapshot.zone_id.unwrap_or(0);
    let placed = markers.for_zone(zone);
    let aabb = view.visible_aabb;
    for (marker, mut node) in q.iter_mut() {
        let uv = placed.get(marker.slot).zip(aabb).and_then(|(m, a)| {
            a.world_to_uv_or_offscreen(bevy::math::Vec3::new(m.world.x, m.world.y, m.world.z))
        });
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
}

pub fn update_minimap_overlay(
    view: Res<MinimapView>,
    scene_state: Res<SceneState>,
    target: Res<Target>,
    lock_on: Res<LockOn>,
    chase: Res<ChaseCamera>,
    filters: Res<MarkerFilters>,
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
    let self_char_id = scene_state.snapshot.self_char_id.unwrap_or(0);
    let party_ids = party_id_set(&scene_state);
    sync_marker_layer(
        aabb,
        overlay_layer,
        self_char_id,
        &target,
        &lock_on,
        chase.yaw,
        &party_ids,
        &filters,
        &q_self,
        &q_transform,
        &mut dots.by_id,
        &mut commands,
        &mut q_dot_node,
        &mut q_marker_transform,
    );
}

/// Party-member entity ids from the snapshot, the party-color / Party-filter
/// membership source (party ids already live in `SessionState`).
pub fn party_id_set(scene_state: &SceneState) -> HashSet<u32> {
    scene_state
        .snapshot
        .party
        .iter()
        .map(|m| m.id)
        .filter(|id| *id != 0)
        .collect()
}

/// Repaint one marker layer (the minimap widget or the full Map screen) from the
/// live world: per-entity kind/target/lock dots, the rotating self marker, and
/// stale-dot cleanup — the single marker code path both surfaces share. `by_id`
/// is the caller's own dot store (disjoint entity sets); `aabb` is whatever
/// world→UV window that surface renders.
#[allow(clippy::too_many_arguments)]
pub fn sync_marker_layer<FD, FT>(
    aabb: MinimapAabb,
    overlay_layer: Entity,
    self_char_id: u32,
    target: &Target,
    lock_on: &LockOn,
    chase_yaw: f32,
    party_ids: &HashSet<u32>,
    filters: &MarkerFilters,
    q_self: &Query<&Transform, With<IsSelf>>,
    q_transform: &Query<(&Transform, &WorldEntity), Without<IsSelf>>,
    by_id: &mut HashMap<u32, Entity>,
    commands: &mut Commands,
    q_dot_node: &mut Query<(&mut Node, &mut BackgroundColor), FD>,
    q_marker_transform: &mut Query<&mut UiTransform, FT>,
) where
    FD: bevy::ecs::query::QueryFilter,
    FT: bevy::ecs::query::QueryFilter,
{
    let mut seen: HashSet<u32> = HashSet::with_capacity(by_id.len() + 1);

    for (transform, world_entity) in q_transform.iter() {
        if self_char_id != 0 && world_entity.id == self_char_id {
            continue;
        }

        let Some(uv) = aabb.world_to_uv_or_offscreen(transform.translation) else {
            continue;
        };
        let is_target = target.id == Some(world_entity.id);
        let is_locked = lock_on.target_id == Some(world_entity.id);
        let is_party = party_ids.contains(&world_entity.id);
        let category = marker_category(world_entity.kind, is_party, is_target || is_locked);
        if !filters.is_visible(category) {
            continue;
        }
        let color = dot_color(world_entity.kind, is_target, is_locked, is_party);
        upsert_dot(
            by_id,
            commands,
            overlay_layer,
            world_entity.id,
            uv,
            color,
            DOT_DIAMETER_PX,
            q_dot_node,
            is_target || is_locked,
        );
        seen.insert(world_entity.id);
    }

    if filters.is_visible(MarkerCategory::SelfMarker) {
        if let Ok(self_t) = q_self.single() {
            let uv = aabb
                .world_to_uv_or_offscreen(self_t.translation)
                .unwrap_or_else(|| aabb.world_to_uv(self_t.translation));
            upsert_dot(
                by_id,
                commands,
                overlay_layer,
                SELF_MARKER_ID,
                uv,
                SELF_MARKER_COLOR,
                SELF_MARKER_PX,
                q_dot_node,
                false,
            );
            seen.insert(SELF_MARKER_ID);

            if let Some(&marker) = by_id.get(&SELF_MARKER_ID) {
                let rotation = self_marker_rotation(chase_yaw);
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
    }

    let stale: Vec<u32> = by_id
        .keys()
        .copied()
        .filter(|id| !seen.contains(id))
        .collect();
    for id in stale {
        if let Some(dot_entity) = by_id.remove(&id) {
            if let Ok(mut ec) = commands.get_entity(dot_entity) {
                ec.despawn();
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn upsert_dot<FD>(
    by_id: &mut HashMap<u32, Entity>,
    commands: &mut Commands,
    overlay_layer: Entity,
    entity_id: u32,
    uv: Vec2,
    color: Color,
    diameter_px: f32,
    q_dot_node: &mut Query<(&mut Node, &mut BackgroundColor), FD>,
    with_ring: bool,
) where
    FD: bevy::ecs::query::QueryFilter,
{
    let half = diameter_px * 0.5;
    let left_pct = uv.x * 100.0;
    let top_pct = uv.y * 100.0;

    if let Some(&dot_entity) = by_id.get(&entity_id) {
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
    by_id.insert(entity_id, dot_entity);
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

/// Wide-scan marker/list color for the packed `Type` byte (0x0f4_tracking_list:
/// 0 = char, 1 = npc, 2 = mob), reusing the minimap's per-kind palette so a
/// tracked entity reads the same on the list and the map.
pub fn widescan_color(kind: u8) -> Color {
    let entity_kind = match kind {
        0 => EntityKind::Pc,
        1 => EntityKind::Npc,
        2 => EntityKind::Mob,
        _ => EntityKind::Other,
    };
    dot_color(entity_kind, false, false, false)
}

pub fn dot_color(kind: EntityKind, is_target: bool, is_locked: bool, is_party: bool) -> Color {
    if is_locked {
        return LOCKED_MARKER_COLOR;
    }
    if is_target {
        return TARGET_MARKER_COLOR;
    }
    if is_party {
        return PARTY_MARKER_COLOR;
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
        let locked = dot_color(EntityKind::Mob, true, true, false);
        let targeted = dot_color(EntityKind::Mob, true, false, false);
        let mob = dot_color(EntityKind::Mob, false, false, false);

        assert_ne!(locked, targeted);
        assert_ne!(targeted, mob);
        assert_ne!(locked, mob);
    }

    #[test]
    fn dot_color_locked_overrides_pc_kind() {
        let locked_pc = dot_color(EntityKind::Pc, false, true, false);
        let pc = dot_color(EntityKind::Pc, false, false, false);
        assert_ne!(locked_pc, pc);
    }

    #[test]
    fn dot_color_party_distinct_from_pc_and_overridden_by_role() {
        let pc = dot_color(EntityKind::Pc, false, false, false);
        let party = dot_color(EntityKind::Pc, false, false, true);
        assert_ne!(party, pc, "party members get their own color");
        assert_eq!(party, PARTY_MARKER_COLOR);

        // Target/lock role still wins over party membership.
        assert_eq!(
            dot_color(EntityKind::Pc, true, false, true),
            TARGET_MARKER_COLOR
        );
        assert_eq!(
            dot_color(EntityKind::Pc, false, true, true),
            LOCKED_MARKER_COLOR
        );
    }

    #[test]
    fn marker_category_role_beats_party_beats_kind() {
        assert_eq!(
            marker_category(EntityKind::Mob, true, true),
            MarkerCategory::Target
        );
        assert_eq!(
            marker_category(EntityKind::Pc, true, false),
            MarkerCategory::Party
        );
        assert_eq!(
            marker_category(EntityKind::Mob, false, false),
            MarkerCategory::Mob
        );
        assert_eq!(
            marker_category(EntityKind::Other, false, false),
            MarkerCategory::Npc
        );
    }

    #[test]
    fn marker_filters_default_all_visible_and_toggle_hides_one() {
        let mut filters = MarkerFilters::default();
        for category in MarkerCategory::ALL {
            assert!(filters.is_visible(category), "{category:?} starts visible");
        }
        filters.toggle(MarkerCategory::Mob);
        assert!(!filters.is_visible(MarkerCategory::Mob));
        // Toggling one category leaves the rest untouched.
        for category in MarkerCategory::ALL {
            if category != MarkerCategory::Mob {
                assert!(filters.is_visible(category));
            }
        }
        filters.toggle(MarkerCategory::Mob);
        assert!(filters.is_visible(MarkerCategory::Mob));
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
    fn sync_marker_layer_skips_filtered_category() {
        use bevy::ecs::system::RunSystemOnce;

        #[derive(Component)]
        struct TestLayer;

        #[derive(Resource, Default)]
        struct TestStore(HashMap<u32, Entity>);

        fn run_layer(
            filters: Res<MarkerFilters>,
            q_layer: Query<Entity, With<TestLayer>>,
            q_self: Query<&Transform, With<IsSelf>>,
            q_transform: Query<(&Transform, &WorldEntity), Without<IsSelf>>,
            mut store: ResMut<TestStore>,
            mut commands: Commands,
            mut q_dot_node: Query<(&mut Node, &mut BackgroundColor), With<MinimapDot>>,
            mut q_marker_transform: Query<&mut UiTransform, With<MinimapDot>>,
        ) {
            let layer = q_layer.single().unwrap();
            let aabb = MinimapAabb {
                min: Vec2::splat(-100.0),
                max: Vec2::splat(100.0),
            };
            let party = HashSet::new();
            sync_marker_layer(
                aabb,
                layer,
                0,
                &Target::default(),
                &LockOn::default(),
                0.0,
                &party,
                &filters,
                &q_self,
                &q_transform,
                &mut store.0,
                &mut commands,
                &mut q_dot_node,
                &mut q_marker_transform,
            );
        }

        let mut world = World::new();
        world.insert_resource(MarkerFilters::default());
        world.init_resource::<TestStore>();
        world.spawn(TestLayer);
        world.spawn((
            Transform::from_xyz(10.0, 0.0, 10.0),
            WorldEntity {
                id: 42,
                act_index: 1,
                kind: EntityKind::Mob,
            },
        ));

        world.run_system_once(run_layer).unwrap();
        assert_eq!(
            world.resource::<TestStore>().0.len(),
            1,
            "the mob dot exists while its category is visible"
        );

        world
            .resource_mut::<MarkerFilters>()
            .set(MarkerCategory::Mob, false);
        world.run_system_once(run_layer).unwrap();
        assert!(
            world.resource::<TestStore>().0.is_empty(),
            "filtering Mob off skips the dot in the shared helper (stale-cleaned)"
        );
    }

    #[test]
    fn dot_color_kinds_are_distinct() {
        let pc = dot_color(EntityKind::Pc, false, false, false);
        let npc = dot_color(EntityKind::Npc, false, false, false);
        let mob = dot_color(EntityKind::Mob, false, false, false);
        let pet = dot_color(EntityKind::Pet, false, false, false);
        assert_ne!(pc, npc);
        assert_ne!(npc, mob);
        assert_ne!(mob, pet);
        assert_ne!(pc, mob);
    }
}
