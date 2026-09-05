use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::ui::UiTransform;
use kuluu_snapshot::EntityKind;

use crate::components::{InGameEntity, IsSelf, WorldEntity};
use crate::lock_on::LockOn;
use crate::nameplate_color::{name_color_choice, NameColorTable, SelfContext};
use crate::scene::Target;
use crate::snapshot::SceneState;

use super::{MinimapAabb, MinimapOverlayLayer, MinimapView};

const DOT_DIAMETER_PX: f32 = 9.0;

const SELF_MARKER_PX: f32 = 13.0;

/// Fills mirror the retail `ncol` nameplate rows (`crate::nameplate_color`), so
/// a dot and the plate over the same actor always agree. These stand in only
/// for the frames before that table is read out of the DAT, and for a run whose
/// DAT root never resolves.
const PC_FALLBACK_COLOR: Color = Color::srgb(1.00, 1.00, 1.00);
const PARTY_FALLBACK_COLOR: Color = Color::srgb(0.45, 0.75, 1.00);
const NPC_FALLBACK_COLOR: Color = Color::srgb(0.35, 0.95, 0.45);
const MOB_FALLBACK_COLOR: Color = Color::srgb(1.00, 0.95, 0.35);
const OTHER_FALLBACK_COLOR: Color = Color::srgb(0.70, 0.70, 0.70);

const SELF_MARKER_COLOR: Color = Color::srgb(0.20, 1.00, 1.00);

/// Role rings. Target and lock-on ride the marker's *edge*, never its fill: the
/// fill carries the retail name colour, and an unclaimed mob is already yellow.
const TARGET_RING_COLOR: Color = Color::srgb(1.00, 0.95, 0.20);
const LOCKED_RING_COLOR: Color = Color::srgb(1.00, 0.40, 0.80);
const SELF_RING_COLOR: Color = Color::WHITE;

/// Every dot carries a dark hairline so a pale marker still reads against the
/// parchment of a retail map image.
const MARKER_EDGE_COLOR: Color = Color::srgba(0.0, 0.0, 0.0, 0.65);
const MARKER_EDGE_PX: f32 = 1.0;
const MARKER_RING_PX: f32 = 2.0;

/// The marker silhouette is a square with three rounded corners and one sharp
/// one — a map pin, which points without needing a second node. The sharp
/// corner sits on the node's top-right diagonal, so the node is rotated back by
/// this much before being turned to the bearing it should indicate.
pub(crate) const PIN_TIP_BEARING: f32 = std::f32::consts::FRAC_PI_4;

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
    /// as the map itself. `Target` shows its ring rather than a fill, which is
    /// how that role is drawn.
    pub fn swatch_color(self) -> Color {
        match self {
            MarkerCategory::SelfMarker => SELF_MARKER_COLOR,
            MarkerCategory::Target => TARGET_RING_COLOR,
            MarkerCategory::Party => PARTY_FALLBACK_COLOR,
            MarkerCategory::Pc => fill_fallback(EntityKind::Pc),
            MarkerCategory::Npc => fill_fallback(EntityKind::Npc),
            MarkerCategory::Mob => fill_fallback(EntityKind::Mob),
            MarkerCategory::Pet => fill_fallback(EntityKind::Pet),
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
                border_radius: BorderRadius::MAX,
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

/// Every marker query a repaint needs, gathered once: the role resources, the
/// filter bitset, the retail name-colour table, and the snapshot rows the fill
/// colour is derived from. Built per frame by each surface and handed to
/// [`sync_marker_layer`].
pub struct MarkerContext<'a> {
    self_char_id: u32,
    target: &'a Target,
    lock_on: &'a LockOn,
    filters: &'a MarkerFilters,
    name_colors: &'a NameColorTable,
    party_ids: HashSet<u32>,
    entities: HashMap<u32, &'a kuluu_snapshot::Entity>,
    color_ctx: SelfContext<'a>,
}

impl<'a> MarkerContext<'a> {
    pub fn new(
        scene_state: &'a SceneState,
        target: &'a Target,
        lock_on: &'a LockOn,
        filters: &'a MarkerFilters,
        name_colors: &'a NameColorTable,
    ) -> Self {
        let snapshot = &scene_state.snapshot;
        Self {
            self_char_id: snapshot.self_char_id.unwrap_or(0),
            target,
            lock_on,
            filters,
            name_colors,
            party_ids: snapshot
                .party
                .iter()
                .map(|m| m.id)
                .filter(|id| *id != 0)
                .collect(),
            entities: snapshot.entities.iter().map(|e| (e.id, e)).collect(),
            color_ctx: SelfContext {
                self_id: snapshot.self_char_id,
                party: &snapshot.party,
            },
        }
    }

    /// The fill for one world dot: the same retail `ncol` colour its nameplate
    /// draws in, or the per-kind stand-in until that table loads.
    fn fill(&self, entity_id: u32, kind: EntityKind) -> Color {
        self.entities
            .get(&entity_id)
            .map(|e| name_color_choice(e, self.color_ctx))
            .and_then(|choice| choice.resolve(self.name_colors))
            .unwrap_or_else(|| fill_fallback(kind))
    }
}

/// Stand-in fill for an actor the retail colour table can't speak for yet.
fn fill_fallback(kind: EntityKind) -> Color {
    match kind {
        EntityKind::Pc => PC_FALLBACK_COLOR,
        EntityKind::Npc => NPC_FALLBACK_COLOR,
        EntityKind::Mob => MOB_FALLBACK_COLOR,
        // A player's pet takes the party row in retail (nameplate_color.rs).
        EntityKind::Pet => PARTY_FALLBACK_COLOR,
        EntityKind::Other => OTHER_FALLBACK_COLOR,
    }
}

/// Wide-scan marker/list color for the packed `Type` byte (0x0f4_tracking_list:
/// 0 = char, 1 = npc, 2 = mob), reusing the map's per-kind palette so a tracked
/// entity reads the same on the list and the map.
pub fn widescan_color(kind: u8) -> Color {
    fill_fallback(match kind {
        0 => EntityKind::Pc,
        1 => EntityKind::Npc,
        2 => EntityKind::Mob,
        _ => EntityKind::Other,
    })
}

pub fn update_minimap_overlay(
    view: Res<MinimapView>,
    scene_state: Res<SceneState>,
    target: Res<Target>,
    lock_on: Res<LockOn>,
    filters: Res<MarkerFilters>,
    name_colors: Res<NameColorTable>,
    q_overlay_layer: Query<Entity, With<MinimapOverlayLayer>>,
    q_self: Query<&Transform, With<IsSelf>>,
    q_transform: Query<(&Transform, &WorldEntity), Without<IsSelf>>,
    mut dots: ResMut<MinimapDots>,
    mut commands: Commands,
    mut q_dot: Query<MarkerNode, With<MinimapDot>>,
) {
    let Some(aabb) = view.visible_aabb else {
        return;
    };
    let Ok(overlay_layer) = q_overlay_layer.single() else {
        return;
    };
    let ctx = MarkerContext::new(&scene_state, &target, &lock_on, &filters, &name_colors);
    sync_marker_layer(
        aabb,
        overlay_layer,
        &ctx,
        &q_self,
        &q_transform,
        &mut dots.by_id,
        &mut commands,
        &mut q_dot,
    );
}

/// The mutable pieces of one marker node. Both surfaces query it under their
/// own disjointness filters, so it is spelled once here.
pub type MarkerNode = (
    &'static mut Node,
    &'static mut BackgroundColor,
    &'static mut Outline,
    &'static mut UiTransform,
);

/// Repaint one marker layer (the minimap widget or the full Map screen) from the
/// live world: per-entity dots coloured and pointed like their nameplates, the
/// self marker, and stale-dot cleanup — the single marker code path both
/// surfaces share. `by_id` is the caller's own dot store (disjoint entity sets);
/// `aabb` is whatever world→UV window that surface renders.
pub fn sync_marker_layer<F>(
    aabb: MinimapAabb,
    overlay_layer: Entity,
    ctx: &MarkerContext<'_>,
    q_self: &Query<&Transform, With<IsSelf>>,
    q_transform: &Query<(&Transform, &WorldEntity), Without<IsSelf>>,
    by_id: &mut HashMap<u32, Entity>,
    commands: &mut Commands,
    q_dot: &mut Query<MarkerNode, F>,
) where
    F: bevy::ecs::query::QueryFilter,
{
    let mut seen: HashSet<u32> = HashSet::with_capacity(by_id.len() + 1);

    for (transform, world_entity) in q_transform.iter() {
        if ctx.self_char_id != 0 && world_entity.id == ctx.self_char_id {
            continue;
        }

        let Some(uv) = aabb.world_to_uv_or_offscreen(transform.translation) else {
            continue;
        };
        let is_target = ctx.target.id == Some(world_entity.id);
        let is_locked = ctx.lock_on.target_id == Some(world_entity.id);
        let is_party = ctx.party_ids.contains(&world_entity.id);
        let category = marker_category(world_entity.kind, is_party, is_target || is_locked);
        if !ctx.filters.is_visible(category) {
            continue;
        }
        let (ring, ring_px) = ring_for(false, is_target, is_locked);
        upsert_dot(
            by_id,
            commands,
            overlay_layer,
            world_entity.id,
            DotStyle {
                uv,
                diameter_px: DOT_DIAMETER_PX,
                fill: ctx.fill(world_entity.id, world_entity.kind),
                ring,
                ring_px,
                rotation: marker_rotation(facing_xz(transform.rotation)),
            },
            q_dot,
        );
        seen.insert(world_entity.id);
    }

    if ctx.filters.is_visible(MarkerCategory::SelfMarker) {
        if let Ok(self_t) = q_self.single() {
            let uv = aabb
                .world_to_uv_or_offscreen(self_t.translation)
                .unwrap_or_else(|| aabb.world_to_uv(self_t.translation));
            let (ring, ring_px) = ring_for(true, false, false);
            upsert_dot(
                by_id,
                commands,
                overlay_layer,
                SELF_MARKER_ID,
                DotStyle {
                    uv,
                    diameter_px: SELF_MARKER_PX,
                    fill: SELF_MARKER_COLOR,
                    ring,
                    ring_px,
                    rotation: marker_rotation(facing_xz(self_t.rotation)),
                },
                q_dot,
            );
            seen.insert(SELF_MARKER_ID);
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
                ec.try_despawn();
            }
        }
    }
}

/// Everything that varies per marker per frame.
#[derive(Debug, Clone, Copy)]
struct DotStyle {
    uv: Vec2,
    diameter_px: f32,
    fill: Color,
    ring: Color,
    ring_px: f32,
    rotation: Rot2,
}

/// Lock-on outranks target, and the self marker always wears its own ring so it
/// reads at a glance in a crowd. Everything else gets the legibility hairline.
fn ring_for(is_self: bool, is_target: bool, is_locked: bool) -> (Color, f32) {
    if is_self {
        return (SELF_RING_COLOR, MARKER_EDGE_PX);
    }
    if is_locked {
        return (LOCKED_RING_COLOR, MARKER_RING_PX);
    }
    if is_target {
        return (TARGET_RING_COLOR, MARKER_RING_PX);
    }
    (MARKER_EDGE_COLOR, MARKER_EDGE_PX)
}

fn upsert_dot<F>(
    by_id: &mut HashMap<u32, Entity>,
    commands: &mut Commands,
    overlay_layer: Entity,
    entity_id: u32,
    style: DotStyle,
    q_dot: &mut Query<MarkerNode, F>,
) where
    F: bevy::ecs::query::QueryFilter,
{
    let left = Val::Percent(style.uv.x * 100.0);
    let top = Val::Percent(style.uv.y * 100.0);

    if let Some(&dot_entity) = by_id.get(&entity_id) {
        if let Ok((mut node, mut bg, mut outline, mut transform)) = q_dot.get_mut(dot_entity) {
            if node.left != left {
                node.left = left;
            }
            if node.top != top {
                node.top = top;
            }
            if bg.0 != style.fill {
                bg.0 = style.fill;
            }
            if outline.color != style.ring {
                outline.color = style.ring;
            }
            let ring_width = Val::Px(style.ring_px);
            if outline.width != ring_width {
                outline.width = ring_width;
            }
            if transform.rotation != style.rotation {
                transform.rotation = style.rotation;
            }
        }
        return;
    }

    let half = style.diameter_px * 0.5;
    let dot_entity = commands
        .spawn((
            InGameEntity,
            MinimapDot { entity_id },
            Node {
                position_type: PositionType::Absolute,
                left,
                top,
                width: Val::Px(style.diameter_px),
                height: Val::Px(style.diameter_px),
                margin: UiRect {
                    left: Val::Px(-half),
                    top: Val::Px(-half),
                    ..default()
                },
                border_radius: pin_border_radius(style.diameter_px),
                ..default()
            },
            BackgroundColor(style.fill),
            Outline::new(Val::Px(style.ring_px), Val::ZERO, style.ring),
            UiTransform::from_rotation(style.rotation),
            ChildOf(overlay_layer),
        ))
        .id();
    by_id.insert(entity_id, dot_entity);
}

/// Three rounded corners and one sharp one: a map pin whose tip is the heading
/// indicator.
pub(crate) fn pin_border_radius(diameter_px: f32) -> BorderRadius {
    let round = Val::Px(diameter_px * 0.5);
    BorderRadius::new(round, Val::ZERO, round, round)
}

/// The map-plane direction an entity faces. Entity transforms are yaw-only
/// `rotY(-heading)` (`scene::heading_to_quat`, and the smoothed equivalent in
/// `combat_stance::predict_entities_system`), which sends local +X to the world
/// direction of that heading.
pub fn facing_xz(rotation: Quat) -> Vec2 {
    let forward = rotation * Vec3::X;
    Vec2::new(forward.x, forward.z)
}

/// Clockwise UI rotation putting the pin's tip on `facing_xz`. `world_to_uv`
/// maps world +X to screen-right and world +Z to screen-down, so the facing
/// vector is already in the screen basis the rotation applies in.
fn marker_rotation(facing_xz: Vec2) -> Rot2 {
    Rot2::radians(facing_xz.x.atan2(-facing_xz.y) - PIN_TIP_BEARING)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::heading_to_quat;

    /// Where the pin's tip sits before any rotation, as a unit vector.
    fn pin_tip_local() -> Vec2 {
        Rot2::radians(PIN_TIP_BEARING) * Vec2::NEG_Y
    }

    #[test]
    fn fill_fallback_kinds_carry_retail_hues() {
        let mob = fill_fallback(EntityKind::Mob).to_srgba();
        assert!(
            mob.red > 0.9 && mob.green > 0.9 && mob.blue < 0.5,
            "an unclaimed mob is yellow"
        );
        let npc = fill_fallback(EntityKind::Npc).to_srgba();
        assert!(
            npc.green > npc.red && npc.green > npc.blue,
            "an NPC is green"
        );
        let party = fill_fallback(EntityKind::Pet).to_srgba();
        assert!(
            party.blue > party.red,
            "a pet takes the blue party row, like its plate"
        );
        assert_eq!(fill_fallback(EntityKind::Pc), PC_FALLBACK_COLOR);
    }

    #[test]
    fn widescan_colors_match_the_map_palette() {
        assert_eq!(widescan_color(0), fill_fallback(EntityKind::Pc));
        assert_eq!(widescan_color(1), fill_fallback(EntityKind::Npc));
        assert_eq!(widescan_color(2), fill_fallback(EntityKind::Mob));
    }

    #[test]
    fn role_rides_the_ring_and_never_the_fill() {
        let (plain, plain_px) = ring_for(false, false, false);
        let (target, target_px) = ring_for(false, true, false);
        let (locked, _) = ring_for(false, true, true);

        assert_eq!(plain, MARKER_EDGE_COLOR);
        assert_eq!(target, TARGET_RING_COLOR);
        assert_eq!(locked, LOCKED_RING_COLOR, "lock-on outranks target");
        assert!(
            target_px > plain_px,
            "a role ring is thicker than the legibility hairline"
        );
        assert_eq!(ring_for(true, false, false).0, SELF_RING_COLOR);
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

    /// The heading an actor's `Transform` encodes and the heading the camera
    /// resolves are the same angle in two conventions; a marker reading the
    /// transform must land where the camera's forward would.
    #[test]
    fn facing_from_a_transform_matches_the_camera_heading_convention() {
        for heading in [0u8, 32, 64, 96, 128, 160, 192, 224] {
            let yaw = crate::camera::yaw_for_heading(heading);
            let from_camera = Vec2::new(-yaw.sin(), -yaw.cos());
            let from_transform = facing_xz(heading_to_quat(heading));
            assert!(
                (from_transform - from_camera).length() < 1e-5,
                "heading {heading}: transform {from_transform:?} vs camera {from_camera:?}"
            );
        }
    }

    #[test]
    fn marker_tip_points_along_the_entity_facing() {
        // Screen space is +x right / +y down (Node left/top from world_to_uv);
        // `UiTransform.rotation` applies its Rot2 matrix in that space.
        for heading in [0u8, 32, 64, 96, 128, 160, 192, 224] {
            let facing = facing_xz(heading_to_quat(heading));
            let tip = marker_rotation(facing) * pin_tip_local();
            assert!(
                (tip - facing).length() < 1e-5,
                "heading {heading}: tip {tip:?} vs facing {facing:?}"
            );
        }
    }

    #[test]
    fn marker_tip_at_heading_zero_points_screen_right() {
        // FFXI heading 0 faces world +X (camera::yaw_for_heading), and
        // world_to_uv maps +X to screen-right.
        let tip = marker_rotation(facing_xz(heading_to_quat(0))) * pin_tip_local();
        assert!((tip - Vec2::X).length() < 1e-5, "tip {tip:?}");
    }

    /// The regression the marker rotation exists to prevent: swivelling the
    /// camera must not move a marker that stands still (kuluu-2d2c).
    #[test]
    fn marker_rotation_ignores_the_camera() {
        let facing = facing_xz(heading_to_quat(96));
        let before = marker_rotation(facing);
        // Same actor, camera swung a quarter turn — nothing the rotation reads.
        assert_eq!(marker_rotation(facing), before);
    }

    #[test]
    fn sync_marker_layer_skips_filtered_category() {
        use bevy::ecs::system::RunSystemOnce;

        #[derive(Component)]
        struct TestLayer;

        #[derive(Resource, Default)]
        struct TestStore(HashMap<u32, Entity>);

        fn run_layer(
            scene_state: Res<SceneState>,
            filters: Res<MarkerFilters>,
            name_colors: Res<NameColorTable>,
            q_layer: Query<Entity, With<TestLayer>>,
            q_self: Query<&Transform, With<IsSelf>>,
            q_transform: Query<(&Transform, &WorldEntity), Without<IsSelf>>,
            mut store: ResMut<TestStore>,
            mut commands: Commands,
            mut q_dot: Query<MarkerNode, With<MinimapDot>>,
        ) {
            let layer = q_layer.single().unwrap();
            let aabb = MinimapAabb {
                min: Vec2::splat(-100.0),
                max: Vec2::splat(100.0),
            };
            let (target, lock_on) = (Target::default(), LockOn::default());
            let ctx = MarkerContext::new(&scene_state, &target, &lock_on, &filters, &name_colors);
            sync_marker_layer(
                aabb,
                layer,
                &ctx,
                &q_self,
                &q_transform,
                &mut store.0,
                &mut commands,
                &mut q_dot,
            );
        }

        let mut world = World::new();
        world.init_resource::<SceneState>();
        world.insert_resource(MarkerFilters::default());
        world.init_resource::<NameColorTable>();
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
}
