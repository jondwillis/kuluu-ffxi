//! Avian-backed character sweep (kuluu <-> avian3d 0.7 bridge).
//!
//! The zone MZB triangle soup is mirrored into one static avian trimesh
//! collider; dispatch_movement_system routes the per-tick wall clamp through
//! avian move-and-slide (horizontal) plus raw shape casts (vertical: the
//! classic up-forward-down swept stair step, then a ground snap). The height
//! returned is the SWEPT capsule height — it emerges from the motion; there is
//! no post-hoc floor snap left to warp.

use avian3d::prelude::*;
use bevy::prelude::*;
use std::time::Duration;

use kuluu_render::components::{IsSelf, WorldEntity};
use kuluu_render::dat_mzb::{MzbCollisionGeometry, WallClipResult, MAX_GROUND_STEP_UP};

/// Collision classes in the unified avian world. Every collider carries
/// exactly one membership so the position resolver can ask "what did I hit"
/// and branch: walls and doors both block-and-slide (a door is a wall you
/// can't pass, not a hard freeze), mobs soft-block with push-through. The
/// resolver casts per-layer or reads the hit entity's layer to decide.
#[derive(PhysicsLayer, Default)]
pub enum GameLayer {
    /// Unclassified / default. Nothing meaningful should land here.
    #[default]
    Default,
    /// Static zone geometry (MZB walls, floors, ramps, stairs). Block + slide.
    Wall,
    /// MMB placements that block movement (doors, gates, solid furniture).
    /// Block + slide exactly like Wall, but a distinct class so it can never
    /// be treated as a mob (no push-through) and so doors can also count as
    /// FLOORS in the vertical pass (stand on a closed drawbridge).
    Door,
    /// Per-entity obstacle capsules (mobs/NPCs/players). Soft block: sustained
    /// forward pressure past a threshold excludes that one entity and you pass.
    Mob,
}

/// Membership helpers so collider spawns read clearly and stay consistent.
fn wall_layers() -> CollisionLayers {
    CollisionLayers::new(GameLayer::Wall, LayerMask::ALL)
}
fn mob_layers() -> CollisionLayers {
    CollisionLayers::new(GameLayer::Mob, LayerMask::ALL)
}

/// Capsule dimensions (bevy units = yalms). Radius matches the hand-rolled
/// walker's PLAYER_WALL_RADIUS; total height = 2*RADIUS + SEG_LEN.
pub const RADIUS: f32 = 0.4;
pub const SEG_LEN: f32 = 1.0;
/// Feet -> capsule center.
pub const HALF: f32 = RADIUS + SEG_LEN * 0.5;
/// Max riser a swept step may clear (MAX_GROUND_STEP_UP + slack).
pub const MAX_STEP: f32 = 0.45;
/// Steepest surface treated as walkable ground. 60deg: normal.y >= cos(60)=0.5.
pub const SLOPE_MAX_ANGLE: f32 = std::f32::consts::PI / 3.0;
/// Radius of the thin walkability/ground probe. The 0.8-wide walker capsule
/// grazes riser faces and misreads them as walls; a small sphere sees only
/// what's actually underfoot.
const THIN_R: f32 = 0.05;
/// Seconds of sustained forward pressure into the SAME mob before it stops
/// blocking (excluded from the sweep) and the player passes through. Retail
/// FFXI soft body-block.
pub const PUSH_THROUGH_SECS: f32 = 0.8;

pub struct AvianBridgePlugin;

impl Plugin for AvianBridgePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(PhysicsPlugins::default())
            .insert_resource(Gravity(Vec3::ZERO))
            .init_resource::<ZoneAvianCollider>()
            .add_observer(despawn_door_leaf_collider);
        // The three collider-sync systems (sync_zone_collider,
        // sync_mob_collider_radius, sync_mob_colliders) are scheduled in mod.rs
        // into FixedUpdate .before(dispatch_movement_system), NOT here in Update.
        // Reason: avian runs its physics + spatial-query pipeline in
        // FixedPostUpdate (which is BEFORE Update in the frame). The walker
        // (dispatch_movement_system) sweeps in FixedUpdate. If the colliders
        // synced in Update they'd land a full frame after the walker already
        // cast, so a just-spawned mob/door would be walk-through-able its first
        // tick. Ordering them before the walker in the same FixedUpdate makes
        // each collider present and positioned before the sweep.
    }
}

/// Schedules the three collider-sync systems into FixedUpdate before the walker.
/// Called from mod.rs where `dispatch_movement_system` is in scope.
pub fn add_collider_sync_systems(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        (
            sync_zone_collider,
            sync_door_leaf_colliders,
            sync_mob_collider_radius,
            sync_mob_colliders,
        )
            .before(super::input::dispatch_movement_system),
    );
}

/// The one static trimesh entity mirroring the currently loaded zone blocks.
#[derive(Resource, Default)]
pub struct ZoneAvianCollider {
    pub entity: Option<Entity>,
    pub tris: usize,
}

fn sync_zone_collider(
    geom: Res<MzbCollisionGeometry>,
    mut zc: ResMut<ZoneAvianCollider>,
    mut commands: Commands,
) {
    if !geom.is_changed() {
        return;
    }
    if let Some(e) = zc.entity.take() {
        commands.entity(e).try_despawn();
    }
    let (positions, tris) = geom.trimesh_data();
    zc.tris = tris.len();
    if tris.is_empty() {
        return;
    }
    zc.entity = Some(
        commands
            .spawn((
                RigidBody::Static,
                Collider::trimesh(positions, tris),
                wall_layers(),
                Transform::default(),
            ))
            .id(),
    );
}

// ---------------------------------------------------------------------------
// REMOVED: sync_door_colliders (the MMB-visual -> Door-layer collider mirror).
//
// Why it's gone (Bastok Mines ROM/1/34.DAT, probed 2026-08-26): the MZB
// collision section is retail's authored truth of what blocks. 855 of 1525
// visual placements have an index-parallel collision object at the identical
// origin (walls, kabe-atariyou invisible collision helpers, door1/door2/
// doorstop); the other 670 are visual-only decor that retail NEVER collides
// (cho_door among them -- the chocobo door has NO collision object by
// authorial intent). dat_mzb's build_collision_geometry already instances all
// collision objects into the Wall-layer trimesh, so mirroring visual MMB
// meshes into physics (a) duplicated every real wall -- avian reported the
// MZB copy one tick ("wall-REAL") and the MMB copy the next ("door-REAL"),
// the coin-toss flip-flop -- and (b) invented blockers for the 670
// visual-only placements, which is where every "door in the wall" and the
// original chocobo-doorway phantom came from. Single collision authority =
// the MZB collision section, like retail.
//
// Openable-door state (door1/door2 etc., which ARE in the collision section)
// is future work: per-object suppression in MzbCollisionGeometry (the
// sub_area suppression machinery is the template), keyed by the
// collision-object index that is parallel to the named visual placement.
// ---------------------------------------------------------------------------

/// Links a door-leaf collider back to a named visual mesh (a submesh child
/// carrying MmbDebugInfo), so input.rs's debug lookup can print which door
/// blocked. Inserted by `sync_door_leaf_colliders`.
#[derive(Component)]
pub struct DoorColliderSource(pub Entity);

// ---------------------------------------------------------------------------
// Door-leaf colliders: the one place door SOLIDITY lives.
//
// A door in this engine is three things with three owners: the MESH (a `_`/`@`
// FourCC placement group; each drawn leaf carries ZoneDoorLeaf and is re-posed
// by apply_zone_door_stages as it swings), the STATE (the server door entity:
// kind Other + EntityLook::Door, whose door_id FourCC == the group's BlockID
// and whose animation byte drives the open/clos routines), and the COLLISION,
// which before this system was owned by NOBODY: the MZB gives door groups no
// collision by authorial intent (retail door solidity is dynamic), the old
// blanket MMB collider mirror is deleted, and the door entity itself is
// kind-Other so the mob path ignores it. Result: doors rendered, animated,
// and walk-through in every state.
//
// This system closes the gap: one standalone Door-layer trimesh per
// door-routine leaf, verts baked through the AUTHORED (closed) pose --
// mirror-correct via the full matrix, independent of the current swing --
// then toggled by the leaf's live pose: authored pose = closed = solid;
// any displacement (open or mid-swing) = ColliderDisabled = passable.
// Only groups with door open/clos routines qualify (doors.dir); other
// underscore families stay MZB-only.
// ---------------------------------------------------------------------------

/// On the leaf placement: its standalone collider entity, for streaming
/// teardown (see `despawn_door_leaf_collider`).
#[derive(Component)]
struct DoorLeafCollider(Entity);

fn sync_door_leaf_colliders(
    mut commands: Commands,
    doors: Res<kuluu_render::zone_doors::ZoneDoors>,
    meshes: Res<Assets<Mesh>>,
    to_build: Query<
        (Entity, &kuluu_render::zone_doors::ZoneDoorLeaf, &Children),
        Without<DoorLeafCollider>,
    >,
    mesh_children: Query<&Mesh3d>,
    built: Query<(&DoorLeafCollider, &kuluu_render::zone_doors::ZoneDoorLeaf)>,
    disabled_q: Query<&ColliderDisabled>,
) {
    // Build pass.
    for (leaf_ent, leaf, kids) in to_build.iter() {
        if doors.dir(leaf.four_cc).is_none() {
            continue; // not a door-routine group
        }
        let xform = leaf.posed_transform(kuluu_render::zone_doors::DoorPose::default());
        let mut verts: Vec<Vec3> = Vec::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();
        let mut name_src: Option<Entity> = None;
        let mut ready = true;
        for child in kids.iter() {
            let Ok(m3) = mesh_children.get(child) else {
                continue;
            };
            let Some(mesh) = meshes.get(m3.0.id()) else {
                ready = false; // asset still loading: retry next tick
                break;
            };
            let Some(positions) = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .and_then(|a| a.as_float3())
            else {
                continue;
            };
            let Some(indices) = mesh.indices() else {
                continue;
            };
            if name_src.is_none() {
                name_src = Some(child);
            }
            let base = verts.len() as u32;
            verts.extend(
                positions
                    .iter()
                    .map(|v| xform.transform_point3(Vec3::from_array(*v))),
            );
            let mut it = indices.iter();
            while let (Some(a), Some(b), Some(c)) = (it.next(), it.next(), it.next()) {
                tris.push([base + a as u32, base + b as u32, base + c as u32]);
            }
        }
        if !ready || tris.is_empty() {
            continue;
        }
        let collider = commands
            .spawn((
                RigidBody::Static,
                Collider::trimesh(verts, tris),
                CollisionLayers::new(GameLayer::Door, LayerMask::ALL),
                Transform::default(),
                DoorColliderSource(name_src.unwrap_or(leaf_ent)),
            ))
            .id();
        commands.entity(leaf_ent).insert(DoorLeafCollider(collider));
    }

    // Toggle pass: authored pose = closed = solid; any displacement = open.
    for (dc, leaf) in built.iter() {
        let pose = doors.pose(leaf.key());
        let closed = pose.rotation == Vec3::ZERO && pose.translation == Vec3::ZERO;
        let is_disabled = disabled_q.get(dc.0).is_ok();
        if closed == is_disabled {
            if let Ok(mut e) = commands.get_entity(dc.0) {
                if closed {
                    e.remove::<ColliderDisabled>();
                } else {
                    e.insert(ColliderDisabled);
                }
            }
        }
    }
}

/// Frees the standalone leaf collider when its leaf placement streams out
/// (a root entity is not caught by the placement's recursive despawn).
fn despawn_door_leaf_collider(
    trigger: On<Remove, DoorLeafCollider>,
    q: Query<&DoorLeafCollider>,
    mut commands: Commands,
) {
    if let Ok(dc) = q.get(trigger.event().event_target()) {
        // try_despawn: the zone sweep may have taken the collider already;
        // already-gone is a legitimate state here, not an error to log.
        commands.entity(dc.0).try_despawn();
    }
}

/// The texture-matched horizontal radius for this entity's mob obstacle
/// capsule, captured once from the live model AABB. The AABB lives on a mesh
/// DESCENDANT of the WorldEntity (WorldEntity -> actor_root -> mesh child),
/// updated every frame by `update_actor_mesh_aabbs`; we snapshot it once the
/// child exists rather than re-reading each frame (a walk cycle barely moves
/// the horizontal extent, and re-sizing a collider every tick thrashes the
/// broadphase).
#[derive(Component, Clone, Copy)]
struct MobColliderRadius {
    /// Horizontal capsule radius (wider of the two ground-plane half-extents).
    radius: f32,
    /// Vertical half-extent, for capsule segment length.
    half_height: f32,
}

/// Snapshot each non-self entity's model AABB into a `MobColliderRadius` once
/// its mesh descendant carrying the live `Aabb` exists. Separated from collider
/// spawn so the timing hazard (the Aabb child may not exist for the first
/// frames after the entity spawns) is handled by simply retrying until it does,
/// without repeatedly rebuilding a collider.
fn sync_mob_collider_radius(
    mut commands: Commands,
    entities: Query<
        (Entity, &WorldEntity, Option<&Children>),
        (Without<IsSelf>, Without<MobColliderRadius>),
    >,
    children_q: Query<&Children>,
    aabb_q: Query<&bevy::camera::primitives::Aabb>,
) {
    for (entity, _we, kids) in entities.iter() {
        let Some(kids) = kids else { continue };
        // Search descendants (actor_root -> mesh child) for the live Aabb.
        if let Some(aabb) = find_descendant_aabb(kids, &children_q, &aabb_q) {
            let he = aabb.half_extents;
            let radius = he.x.max(he.z);
            // Ignore degenerate/not-yet-posed bounds; retry next frame.
            if radius > 1e-3 && he.y > 1e-3 {
                commands.entity(entity).insert(MobColliderRadius {
                    radius,
                    half_height: he.y,
                });
            }
        }
    }
}

/// Depth-first search of an entity's descendants for the first `Aabb`.
fn find_descendant_aabb(
    kids: &Children,
    children_q: &Query<&Children>,
    aabb_q: &Query<&bevy::camera::primitives::Aabb>,
) -> Option<bevy::camera::primitives::Aabb> {
    for child in kids.iter() {
        if let Ok(aabb) = aabb_q.get(child) {
            return Some(*aabb);
        }
        if let Ok(grandkids) = children_q.get(child) {
            if let Some(found) = find_descendant_aabb(grandkids, children_q, aabb_q) {
                return Some(found);
            }
        }
    }
    None
}

/// Links a visual entity to its separate Mob-collider entity. The collider is
/// NOT a component on the visual: the visual's Transform is written every frame
/// by the scene sync from server data, and avian's PhysicsTransformPlugin also
/// wants to own the Transform of any body — two writers, one Transform, the
/// exact conflict this whole unification exists to kill. So the obstacle
/// collider lives on its own entity that only avian + this system touch, parked
/// each frame at the visual's position.
#[derive(Component)]
struct MobColliderLink(Entity);

/// Marks a spawned collider entity's owner so it can be despawned when the
/// visual goes away.
#[derive(Component)]
pub struct MobColliderOwner(Entity);

/// Spawn a KINEMATIC Mob-layer capsule on a SEPARATE entity for each non-self
/// visual with a known model radius, and keep it parked at the visual's current
/// position each frame by writing the collider's Transform (avian's
/// PhysicsTransformPlugin syncs Position/Rotation from it). Kinematic because
/// the server owns mob position; the mob is a pure obstacle for the player's
/// sweep, never simulated. IsSelf is excluded. When the visual despawns, its
/// collider entity is despawned too (owner link).
fn sync_mob_colliders(
    mut commands: Commands,
    to_build: Query<
        (Entity, &Transform, &MobColliderRadius),
        (
            Without<IsSelf>,
            Without<MobColliderLink>,
            Without<MobColliderOwner>,
        ),
    >,
    visuals: Query<
        &Transform,
        (
            With<MobColliderLink>,
            Without<IsSelf>,
            Without<MobColliderOwner>,
        ),
    >,
    links: Query<(Entity, &MobColliderLink)>,
    mut collider_tf: Query<
        &mut Transform,
        (
            With<MobColliderOwner>,
            Without<MobColliderLink>,
            Without<IsSelf>,
        ),
    >,
    owners: Query<(Entity, &MobColliderOwner)>,
) {
    // Spawn a separate collider entity for each newly-sized visual.
    for (visual, t, r) in to_build.iter() {
        let seg = (r.half_height * 2.0 - r.radius * 2.0).max(0.05);
        let collider = commands
            .spawn((
                RigidBody::Kinematic,
                Collider::capsule(r.radius, seg),
                mob_layers(),
                Transform::from_translation(t.translation),
                MobColliderOwner(visual),
            ))
            .id();
        commands.entity(visual).insert(MobColliderLink(collider));
    }
    // Park each collider on its visual's current position.
    for (visual, link) in links.iter() {
        let Ok(vt) = visuals.get(visual) else {
            continue;
        };
        if let Ok(mut ct) = collider_tf.get_mut(link.0) {
            ct.translation = vt.translation;
        }
    }
    // Despawn colliders whose visual is gone.
    for (collider, owner) in owners.iter() {
        if visuals.get(owner.0).is_err() && links.get(owner.0).is_err() {
            // try_despawn: at zone boundaries the teardown can race this
            // sweep for the same collider; second-in-line must be silent.
            commands.entity(collider).try_despawn();
        }
    }
}

/// MoveAndSlide + SpatialQuery bundled so dispatch grows by one param only.
#[derive(bevy::ecs::system::SystemParam)]
pub struct AvianMoveParams<'w, 's> {
    pub mas: MoveAndSlide<'w, 's>,
    pub sq: SpatialQuery<'w, 's>,
    pub geom: Res<'w, MzbCollisionGeometry>,
    /// Mob collider -> actor link, for the body-block test.
    pub mob_owner: Query<'w, 's, &'static MobColliderOwner>,
    /// Actor kind ([obj] never body-blocks), for the body-block test.
    pub world_ents: Query<'w, 's, &'static WorldEntity>,
    /// Descendant walk for the drawn test (same walk the radius snapshot does).
    pub children: Query<'w, 's, &'static Children>,
    /// "Is the texture drawn": a rendered mesh in the actor's subtree.
    pub mesh_vis: Query<'w, 's, &'static InheritedVisibility, With<Mesh3d>>,
}

/// Should this mob collider body-block the walker at all? Rule, in order:
///   1. EntityKind::Other -- the HUD's "[obj]": door objects, "???" points,
///      event triggers. These NEVER body-block, whatever mesh they carry;
///      retail does not collide with object entities. (The invisible
///      "? [obj]" plaza blocker is this class: real entity, real placeholder
///      mesh, draws nothing.)
///   2. Character kinds block only when their texture is actually drawn: a
///      rendered mesh in the actor's subtree (same descendant walk the radius
///      snapshot does; InheritedVisibility so a real mob doesn't turn
///      walk-through when the camera looks away). Undrawn actor = invisible
///      entity = walk through.
fn mob_body_blocks(av: &AvianMoveParams, collider_ent: Entity) -> bool {
    let Ok(owner) = av.mob_owner.get(collider_ent) else {
        return false;
    };
    if let Ok(we) = av.world_ents.get(owner.0) {
        if matches!(we.kind, kuluu_snapshot::EntityKind::Other) {
            return false;
        }
    }
    let Ok(kids) = av.children.get(owner.0) else {
        return false;
    };
    drawn_mesh_in(kids, av)
}

fn drawn_mesh_in(kids: &Children, av: &AvianMoveParams) -> bool {
    for child in kids.iter() {
        if let Ok(vis) = av.mesh_vis.get(child) {
            if vis.get() {
                return true;
            }
        }
        if let Ok(k) = av.children.get(child) {
            if drawn_mesh_in(k, av) {
                return true;
            }
        }
    }
    false
}

fn capsule() -> Collider {
    Collider::capsule(RADIUS, SEG_LEN)
}

/// Vertical probe: distance the capsule travels along `dir` before contact,
/// capped at `max`, restricted to the given layer mask. Raw shape cast.
#[allow(dead_code)]
fn probe(
    sq: &SpatialQuery,
    col: &Collider,
    from: Vec3,
    dir: Dir3,
    max: f32,
    mask: LayerMask,
) -> f32 {
    match sq.cast_shape(
        col,
        from,
        Quat::IDENTITY,
        dir,
        &ShapeCastConfig::from_max_distance(max),
        &SpatialQueryFilter::from_mask(mask),
    ) {
        Some(hit) => hit.distance,
        None => max,
    }
}

/// A thin sphere collider for walkability sampling (see THIN_R).
#[allow(dead_code)]
fn thin_probe() -> Collider {
    Collider::sphere(THIN_R)
}

/// True when a surface normal is close enough to straight up to be walkable.
#[allow(dead_code)]
fn is_walkable(normal: Vec3) -> bool {
    normal.y >= SLOPE_MAX_ANGLE.cos()
}

/// Ground normal under `center`, sampled with a thin sphere against WALL+DOOR
/// (doors are floors too). None if nothing within `max`. The wide walker
/// capsule grazes riser faces and misreads them as walls; the thin sphere sees
/// only what's underfoot.
#[allow(dead_code)]
fn ground_normal(sq: &SpatialQuery, center: Vec3, max: f32) -> Option<Vec3> {
    let hit = sq.cast_shape(
        &thin_probe(),
        center,
        Quat::IDENTITY,
        Dir3::NEG_Y,
        &ShapeCastConfig::from_max_distance(max),
        &SpatialQueryFilter::from_mask(ground_mask()),
    )?;
    Some(hit.normal1.into())
}

/// Multi-sampled walkability for a swept-step landing: center and +/- along
/// travel. Walkable if ANY sample passes (a tread edge always has one probe
/// mid-tread); a uniform steep wall fails all three. Permissive on all-miss.
#[allow(dead_code)]
fn landing_walkable(sq: &SpatialQuery, at: Vec3, dir_xz: Vec3) -> bool {
    const SPREAD: f32 = 0.15;
    let max = HALF + MAX_STEP;
    let mut any_hit = false;
    for off in [Vec3::ZERO, dir_xz * SPREAD, dir_xz * -SPREAD] {
        if let Some(n) = ground_normal(sq, at + off, max) {
            any_hit = true;
            if is_walkable(n) {
                return true;
            }
        }
    }
    !any_hit
}

/// Layer mask for ground/floor: walls and doors (a closed drawbridge is floor).
fn ground_mask() -> LayerMask {
    LayerMask::from([GameLayer::Wall, GameLayer::Door])
}

/// Layer mask for the camera boom: walls and doors block the camera; mobs
/// never do (you always see through/past creatures). Same solid world the
/// walker collides against — one collision authority for movement AND camera.
pub fn camera_mask() -> LayerMask {
    LayerMask::from([GameLayer::Wall, GameLayer::Door])
}
/// Layer mask for obstacle bodies: mobs only.
fn mob_mask() -> LayerMask {
    LayerMask::from([GameLayer::Mob])
}

/// Layer mask for doors only.
fn door_mask() -> LayerMask {
    LayerMask::from([GameLayer::Door])
}

/// True if a capsule sweep from `start` along `want` hits anything in `mask`.
/// (Superseded by entity_in_layer for stop classification; kept for reference.)
#[allow(dead_code)]
fn layer_ahead(
    sq: &SpatialQuery,
    col: &Collider,
    start: Vec3,
    want: Vec3,
    mask: LayerMask,
) -> bool {
    let len = want.length();
    if len < 1e-6 {
        return false;
    }
    let Ok(dir) = Dir3::new(want / len) else {
        return false;
    };
    sq.cast_shape(
        col,
        start,
        Quat::IDENTITY,
        dir,
        &ShapeCastConfig::from_max_distance(len),
        &SpatialQueryFilter::from_mask(mask),
    )
    .is_some()
}

/// Does `ent` belong to `mask`? Casts mask-only along the move and checks the
/// hit entity IS `ent`. This classifies the SPECIFIC entity avian stopped us on
/// -- not any door/mob somewhere in the path -- killing false positives.
fn entity_in_layer(
    sq: &SpatialQuery,
    col: &Collider,
    start: Vec3,
    want: Vec3,
    mask: LayerMask,
    ent: Entity,
) -> bool {
    let len = want.length();
    if len < 1e-6 {
        return false;
    }
    let Ok(dir) = Dir3::new(want / len) else {
        return false;
    };
    sq.cast_shape(
        col,
        start,
        Quat::IDENTITY,
        dir,
        &ShapeCastConfig::from_max_distance(len),
        &SpatialQueryFilter::from_mask(mask),
    )
    .map(|h| h.entity == ent)
    .unwrap_or(false)
}

/// The single horizontal-obstacle question: cast the capsule along `want` from
/// `start` and report the FIRST thing hit and how far. Doors and walls are one
/// slide class; mobs are their own. Returns (hit distance, hit entity, is_mob).
/// None = clear path.
fn horizontal_obstacle(
    sq: &SpatialQuery,
    col: &Collider,
    start: Vec3,
    want: Vec3,
    excluded: Option<Entity>,
) -> Option<(f32, Entity, bool)> {
    let len = want.length();
    if len < 1e-6 {
        return None;
    }
    let dir = Dir3::new(want / len).ok()?;
    // Cast against walls+doors+mobs together; nearest hit wins.
    let mut filter = SpatialQueryFilter::from_mask(LayerMask::from([
        GameLayer::Wall,
        GameLayer::Door,
        GameLayer::Mob,
    ]));
    if let Some(e) = excluded {
        filter = filter.with_excluded_entities([e]);
    }
    let hit = sq.cast_shape(
        col,
        start,
        Quat::IDENTITY,
        dir,
        &ShapeCastConfig::from_max_distance(len),
        &filter,
    )?;
    // Is the hit entity a mob? Test its membership by re-casting mob-only to the
    // same distance and seeing if the same entity is the mob-layer hit. Simpler:
    // record the entity and let the caller classify via a mob-only probe.
    // Here we approximate: a hit within a mob-only cast at <= this distance and
    // same entity means mob.
    let mob_here = {
        let mut mf = SpatialQueryFilter::from_mask(mob_mask());
        if let Some(e) = excluded {
            mf = mf.with_excluded_entities([e]);
        }
        sq.cast_shape(
            col,
            start,
            Quat::IDENTITY,
            dir,
            &ShapeCastConfig::from_max_distance(len),
            &mf,
        )
        .map(|mh| mh.entity == hit.entity)
        .unwrap_or(false)
    };
    Some((hit.distance, hit.entity, mob_here))
}

/// Per-entity push-through accrual: which mob the player is currently shoving,
/// and for how long. Lives in `dispatch_movement_system` as a Local and is
/// passed into the resolver.
#[derive(Default)]
pub struct PushThrough {
    pub target: Option<Entity>,
    pub secs: f32,
}

impl PushThrough {
    /// Register a block against `mob` this tick; returns true if the mob should
    /// now be pushed through (excluded from the sweep).
    fn press(&mut self, mob: Entity, dt: f32) -> bool {
        if self.target == Some(mob) {
            self.secs += dt;
        } else {
            self.target = Some(mob);
            self.secs = dt;
        }
        self.secs >= PUSH_THROUGH_SECS
    }
    fn release(&mut self) {
        self.target = None;
        self.secs = 0.0;
    }
}

/// THE resolver. One authority, two passes, fixed priority. Replaces the old
/// `wall_clip_avian`. Wire contract unchanged: ffxi x/y horizontal, z vertical
/// (grows DOWN); bevy x=x, z=-y, y=-z (up).
///
/// Pass A (horizontal, never touches Y): slide on walls+doors (no height gain
/// -> no goat), soft-block on mobs (push-through after PUSH_THROUGH_SECS).
/// Pass B (vertical, never touches XZ): settle on walls+doors within a step,
/// swept-step climb gated by climb-slope, else fall to real ground/door-floor.
pub fn resolve_position(
    av: &AvianMoveParams,
    push: &mut PushThrough,
    x: f32,
    y: f32,
    z: f32,
    dx: f32,
    dy: f32,
    dt: f32,
    // OUT: the one detect_stairs result this tick, for asp + HUD to read (dedup).
    det_out: &mut Option<super::input::StairDetection>,
    // OUT: when the block was classified as a door, the door entity, so the
    // caller can resolve its mesh/texture name for debug.
    door_ent_out: &mut Option<Entity>,
) -> WallClipResult {
    let col = capsule();
    let feet0 = -z;
    let start = Vec3::new(x, feet0 + HALF, -y);
    let want = Vec3::new(dx, 0.0, -dy);
    let want_len = want.length();
    let dt = dt.max(1e-4);

    // Ceiling to cast ground rays down from: a bit above the head.
    // The floor PLANE: cast down from body-center (feet + 1.0) to the floor at
    // the player's XZ. Body-relative origin, so it is stable whether the player
    // is grounded OR in the air -- the plane does not move with the (possibly
    // airborne) live Y. Everything below references THIS, not the live position,
    // so the height output can't feed back into its own input.
    let body_center_y = feet0 + 1.0;
    let plane_y = av
        .geom
        .ground_raycast(Vec2::new(x, -y), body_center_y)
        .unwrap_or(feet0);

    // THE ONE detect_stairs call this tick (word of god). Runs from the stable
    // plane at the CURRENT xz. apply_self_prediction_system and the HUD read
    // this same result via LastStairDetection -- no duplicate raycasting.
    let det = super::input::detect_stairs(Vec3::new(x, plane_y, -y), &av.geom);
    *det_out = Some(det);
    *door_ent_out = None;

    // ---- STOPPED (no input): settle onto the actual tread ----
    // While moving we ride the smooth footprint ramp (below); the instant input
    // stops we drop onto the real stepped surface underneath. The 0.2 up/down
    // guard: ground_step accepts a floor up to MAX_GROUND_STEP_UP above the feet
    // (rise onto a tread you are wedged just below) and any distance below
    // (fall to the tread). This is what "fall to the tread when you stop" means,
    // and the up-accept keeps you from sinking through the stairs.
    if want_len < 1e-6 {
        push.release();
        let floor = av
            .geom
            .ground_step(Vec2::new(x, -y), feet0, MAX_GROUND_STEP_UP)
            .or_else(|| av.geom.ground_nearest(Vec2::new(x, -y), feet0));
        return WallClipResult {
            dx: 0.0,
            dy: 0.0,
            landed_floor: floor.map(|f| -f),
            dbg_is_a_stop: false,
            dbg_stop_slope: false,
            dbg_slope_angle: 0.0,
            dbg_stop_steps: false,
            dbg_tall_wall: false,
            dbg_step_slope: 0.0,
            dbg_step_height: 0.0,
            dbg_stop_wall: false,
            dbg_wall_height: 0.0,
            dbg_stop_door: false,
            dbg_stop_mob: false,
            dbg_soft_timer: 0.0,
            dbg_block_nx: 0.0,
            dbg_block_ny: 0.0,
            dbg_block_nz: 0.0,
            dbg_reason: "stopped-input",
            dbg_hit_x: 0.0,
            dbg_hit_y: 0.0,
            dbg_hit_z: 0.0,
        };
    }

    // =====================================================================
    // ORCHESTRATION (word of god). One ordered sequence, one authority. The
    // slide is STEP ONE; its result flows into ONE priority-ordered
    // classification that makes ONE decision; step three assembles the result.
    // Nothing overrides anything after the fact. The debug flags are set by the
    // SAME classification, so the HUD can never disagree with what moved us.
    // =====================================================================
    const STEP_HEIGHT: f32 = 0.4; // max auto-climb
    const SNAP_DOWN: f32 = 0.4; // ground-snap reach below feet
    let move_dir = Vec2::new(want.x, want.z).normalize_or_zero();
    let here = Vec2::new(x, -y);

    // debug accumulators (set by the single classification below)
    let mut dbg_is_a_stop = false;
    let mut dbg_stop_slope = false;
    let mut dbg_slope_angle = 0.0f32;
    let mut dbg_stop_steps = false;
    let mut dbg_step_slope = 0.0f32;
    let mut dbg_step_height = 0.0f32;
    let mut dbg_stop_wall = false;
    let mut dbg_wall_height = 0.0f32;
    let mut dbg_stop_door = false;
    let mut dbg_stop_mob = false;
    let dbg_soft_timer = (PUSH_THROUGH_SECS - push.secs).max(0.0);
    let mut dbg_reason: &'static str = "moving-free";

    // ---- STEP 1: SLIDE (the orchestration runs avian, once) ----------------
    // Mob push-through accrual decides whether one mob entity is excluded, then
    // we build the filter and slide. slide_walls_only returns the moved position
    // AND the first blocking (non-walkable) contact normal (None = not stopped).
    let mut excluded_mob: Option<Entity> = None;
    if let Some((_d, ent, is_mob)) = horizontal_obstacle(&av.sq, &col, start, want, None) {
        if is_mob {
            if !mob_body_blocks(av, ent) {
                // No drawn texture on the actor = invisible server entity
                // (event trigger, door object). Those never body-block:
                // exclude instantly, no push-through timer. Walls behind
                // still apply -- the slide runs with only this one entity
                // excluded from the filter.
                excluded_mob = Some(ent);
                push.release();
            } else if push.press(ent, dt) {
                excluded_mob = Some(ent);
            }
        } else {
            push.release();
        }
    } else {
        push.release();
    }
    let mut hfilter = SpatialQueryFilter::from_mask(LayerMask::from([
        GameLayer::Wall,
        GameLayer::Door,
        GameLayer::Mob,
    ]));
    if let Some(e) = excluded_mob {
        hfilter = hfilter.with_excluded_entities([e]);
    }
    let mut block_normal: Option<Vec3> = None;
    let mut block_entity: Option<Entity> = None;
    let mut block_point: Option<Vec3> = None;
    let p1 = slide_walls_only(
        &av.mas,
        &col,
        start,
        want / dt,
        dt,
        &hfilter,
        &mut block_normal,
        &mut block_entity,
        &mut block_point,
    );
    let slide_xz = Vec2::new(p1.x, p1.z);

    // ---- STEP 2: CLASSIFY (ONE decision, priority order) -------------------
    // Inputs: the slide result (slide_xz, block_normal) + the detector (det).
    // We pick exactly ONE outcome and set (move_xz, final_feet, debug) from it.
    // Priority: slope-ride(pre-triggered) > stairs-ahead > blocked(door>mob>wall)
    // > free-walk. The ride's result is collected separately (ride_result) and
    // applied AFTER the chain, so no branch ever reassigns a decided tick —
    // one decision per tick, and definite-assignment stays provable.
    let mut move_xz;
    let mut final_feet;

    // Avian absorbed most of the requested move without necessarily capturing
    // a block: a rounded-bottom contact on a low sill reads walkable -> Ignore,
    // the capsule embeds, and the slide comes back truncated with no evidence.
    // This flag is what lets the lip ride below rescue that silent stop.
    let want_len = Vec2::new(want.x, want.z).length();
    let slide_len = (slide_xz - Vec2::new(start.x, start.z)).length();
    let slide_truncated = want_len > 1e-4 && slide_len < want_len * 0.6;

    // (a) Walkable stairs OR a small lip ahead. Two detector signals ride:
    //     - banded risers (band != 0): real staircases. Avian sees each riser
    //       as a vertical wall every tick; letting the block win = stop/go.
    //     - sub-band lips (band == 0, small positive rise): door sills and
    //       thresholds below the detector's riser quantum -- the HUD's GRAY
    //       orbs. Avian has NO step height, so a 0.1-yalm sill hard-stops the
    //       capsule silently ("moving-free" with a zero delta). Retail walks
    //       straight over these. Lips ride ONLY when the slide was actually
    //       truncated, so this rescues a stuck capsule and never bypasses
    //       normal wall sliding on gently uneven ground.
    //     The detector is the authority on "is this walkable" for both.
    let mut lip_h: f32 = 0.0;
    let stairs_ahead = {
        let mut found = false;
        for &(oxz, oy, _g, band) in det.sample_data.iter() {
            if oy.is_nan() {
                continue; // invalid sample
            }
            let along = (oxz - here).dot(move_dir);
            if band == 0 {
                // Sub-band lip: UP only (drops are ground-snap's job), CLOSE
                // ahead only (step when we reach it, not from a yalm out).
                let rise = oy - plane_y;
                if along >= -0.2 && along <= 0.9 && rise > 0.02 && rise <= STEP_HEIGHT {
                    lip_h = lip_h.max(rise);
                }
                continue;
            }
            let rise = (oy - plane_y).abs(); // up OR down both walkable
            if along >= -0.2 && rise > 0.02 && rise <= STEP_HEIGHT * 3.0 {
                found = true;
                break;
            }
        }
        found
    };
    let lip_ride = !stairs_ahead && lip_h > 0.0 && slide_truncated;

    // TALL-WALL VETO: a stair riser tops out at STEP_HEIGHT, so a ray fired
    // JUST above that height, a hand's width ahead, can only hit something
    // taller than a step -- a wall. Without this, the detector seeing treads
    // on the far side of a thin side wall rode the walker straight through
    // it. The reach is tight (RADIUS + 0.15) so the SECOND riser of a real
    // staircase -- one tread deeper, the first thing tall enough to cross
    // this ray -- stays out of range and never vetoes legitimate climbing.
    // `det.ramp_locked` included too: the pre-triggered slope-ride engages BEFORE a
    // riser is close enough for ring samples, so the veto must fire then as well —
    // otherwise we'd ride the walker straight through a wall standing between us and
    // the staircase.
    let tall_wall_before_step = (stairs_ahead || det.ramp_locked || lip_h > 0.0)
        && Dir3::new(Vec3::new(move_dir.x, 0.0, move_dir.y))
            .ok()
            .is_some_and(|d| {
                av.sq
                    .cast_ray(
                        Vec3::new(start.x, plane_y + STEP_HEIGHT + 0.01, start.z),
                        d,
                        RADIUS + 0.15,
                        true,
                        &SpatialQueryFilter::from_mask(LayerMask::from([
                            GameLayer::Wall,
                            GameLayer::Door,
                        ])),
                    )
                    .is_some()
            });

    // (a0) SLOPE-RIDE — continuous stair follow, pre-triggered. When the detector
    // holds a locked ramp line (march-measured or fit), feet ride THAT LINE instead
    // of snapping to det.ramp_near.1 per tick: wire Y advances at slope × progress,
    // so treads are one continuous incline on the WIRE (collision + c2s 0x015), not
    // just in render — ground_step never gets a turn mid-stair, which is what used to
    // force us down onto the slab under a buried staircase. The rise is anchored at
    // the FIRST RISER (march_first_riser_rel): the flat approach before it stays at
    // current foot level exactly, so the follow engages while we are still walking UP
    // TO the steps (no float over the last flat strip).
    // HOLE DETECTOR: a down-ray at the destination xz must find floor within
    // STAIR_HOLE_DROP of the ride line; a missing tread / open hole drops us through
    // via ground-snap — exactly what plain walking did before slope-ride existed.
    const STAIR_HOLE_DROP: f32 = 0.5;
    // Some(destination, feet) when the ride decides this tick; resolved after
    // the legacy chain below (it outranks every arm).
    let mut ride_result: Option<(Vec2, f32)> = None;
    let mut ride_hole_fall = false;
    if det.ramp_locked && !tall_wall_before_step {
        let target_xz = Vec2::new(start.x + want.x, start.z + want.z);
        if let Some(pred) = slope_ride_feet(&det, plane_y, here, target_xz) {
            if av
                .geom
                .ground_raycast(target_xz, pred.max(plane_y) + 0.5)
                .is_some_and(|actual| actual >= pred - STAIR_HOLE_DROP)
            {
                dbg_reason = "slope-ride";
                dbg_is_a_stop = true;
                dbg_stop_steps = true;
                // Continuous-ride tick: no discrete step happened, so dispatch must not
                // re-arm the stair-settle dip clamp — it would swallow our small per-tick
                // descent deltas (pre-ride code produced per-riser jumps that cleared the
                // 0.08 gate).
                dbg_stop_slope = true;
                dbg_step_height = pred - plane_y;
                dbg_step_slope = det.best_slope;
                dbg_slope_angle = det.best_slope.atan().to_degrees();
                ride_result = Some((target_xz, pred)); // resolved after the chain below
            } else {
                // Hole in the stair: no real floor under the ride line at the
                // destination. Disengage; ground-snap (below) drops us through.
                dbg_reason = "stair-hole-fall";
                ride_hole_fall = true;
            }
        }
    }

    // Hole-fall ticks skip the hold arm A as well: its ramp_near flat-hold would
    // float us across a missing tread instead of letting ground-snap drop us.
    if ride_result.is_none()
        && !ride_hole_fall
        && (stairs_ahead || lip_ride)
        && !tall_wall_before_step
    {
        if stairs_ahead {
            dbg_reason = "stairs-ahead";
            dbg_step_height = det.ramp_near.1 - plane_y;
            dbg_step_slope = det.best_slope;
            dbg_slope_angle = det.best_slope.atan().to_degrees();
            final_feet = det.ramp_near.1;
        } else {
            // Lip-step: full forward move, feet lifted onto the sill this
            // tick so the capsule clears the edge (no embed, no truncation).
            dbg_reason = "lip-step";
            dbg_step_height = lip_h;
            final_feet = plane_y + lip_h;
        }
        dbg_is_a_stop = true;
        dbg_stop_steps = true;
        move_xz = Vec2::new(start.x + want.x, start.z + want.z);
    } else if let Some(n) = block_normal {
        // (b) BLOCKED by something that is not a walkable staircase. Classify the
        //     SPECIFIC entity avian stopped us on (block_entity).
        dbg_is_a_stop = true;

        // GROUND TRUTH: cast a clean forward ray a short distance against
        // walls+doors. If NOTHING is really in front of us, avian's move_and_slide
        // fabricated the contact (depenetration artifact) -- we should NOT block.
        let probe_from = Vec3::new(start.x, start.y, start.z);
        let clean_hit = Dir3::new(want / want.length().max(1e-6))
            .ok()
            .and_then(|d| {
                av.sq.cast_ray(
                    probe_from,
                    d,
                    RADIUS + 0.6, // just in front (capsule radius + a little)
                    true,
                    &SpatialQueryFilter::from_mask(LayerMask::from([
                        GameLayer::Wall,
                        GameLayer::Door,
                    ])),
                )
            });
        // Record for debug: did the clean forward ray actually find a face?
        let real = clean_hit.is_some();

        let ent = block_entity;
        let door_hit =
            ent.is_some_and(|e| entity_in_layer(&av.sq, &col, start, want, door_mask(), e));
        let mob_hit = excluded_mob.is_none()
            && ent.is_some_and(|e| entity_in_layer(&av.sq, &col, start, want, mob_mask(), e));

        if door_hit {
            dbg_reason = if real { "door-REAL" } else { "door-PHANTOM" };
            dbg_stop_door = true;
            *door_ent_out = ent;
            move_xz = slide_xz;
            final_feet = det.center_y;
        } else if mob_hit {
            // REAL/PHANTOM by the drawn test, NOT the forward ray: the ray only
            // sees Wall+Door, so a mob in open space always read "PHANTOM",
            // visible or not. Drawn actor = real body = soft block. Undrawn =
            // invisible entity = walk through. (This arm normally only fires
            // for a second undrawn mob behind one already excluded pre-slide.)
            if ent.is_some_and(|e| mob_body_blocks(av, e)) {
                dbg_reason = "mob-REAL";
                dbg_stop_mob = true;
                move_xz = slide_xz;
                final_feet = det.center_y;
            } else {
                dbg_reason = "mob-PHANTOM-pass";
                dbg_is_a_stop = false;
                let dest = Vec2::new(start.x + want.x, start.z + want.z);
                move_xz = dest;
                final_feet = av
                    .geom
                    .ground_step(dest, plane_y, SNAP_DOWN)
                    .unwrap_or(det.center_y);
            }
        } else {
            dbg_reason = if real { "wall-REAL" } else { "wall-PHANTOM" };
            let angle = n.y.clamp(-1.0, 1.0).acos();
            dbg_stop_wall = true;
            dbg_slope_angle = angle.to_degrees();
            dbg_wall_height = 1.0;
            move_xz = slide_xz;
            final_feet = det.center_y;
        }
    } else {
        // (c) FREE WALK: not stopped, no stairs. Take avian's slide result and
        //     snap to the ground under the new position.
        move_xz = slide_xz;
        // A hole-fall disengage skips the locked-ramp flat hold: the ground-snap
        // below is what drops us through the gap.
        if det.ramp_locked && !ride_hole_fall {
            dbg_stop_slope = true; // informational: walking a locked ramp
            dbg_slope_angle = det.best_slope.atan().to_degrees();
            final_feet = det.ramp_near.1;
        } else if let Some(g) = av.geom.ground_step(slide_xz, plane_y, SNAP_DOWN) {
            final_feet = g;
        } else {
            final_feet = det.center_y;
        }
    }

    // SLOPE-RIDE outranks every legacy arm above (one decision per tick): when
    // it fired, its result replaces whatever the chain assigned. Without this a
    // successful ascent tick fell into C's flat hold, which froze wire Y at
    // current foot level — the climb would only ever happen in render.
    if let Some((r_xz, r_feet)) = ride_result {
        move_xz = r_xz;
        final_feet = r_feet;
    }

    // TEMP (stair diagnosis): console trace of decision flips — a flickering
    // ramp lock shows up as slope-ride/stairs-ahead alternating line by line.
    // Remove once the stair work is verified in-game.
    {
        static LAST_REASON: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(usize::MAX);
        let cur = dbg_reason.as_bytes().as_ptr() as usize;
        if LAST_REASON.swap(cur, std::sync::atomic::Ordering::Relaxed) != cur {
            tracing::info!(
                reason = dbg_reason,
                slope_deg = dbg_slope_angle,
                step_h = dbg_step_height,
                "stair decision change",
            );
        }
    }

    // ---- STEP 3: ASSEMBLE --------------------------------------------------
    WallClipResult {
        dx: move_xz.x - start.x,
        dy: -(move_xz.y - start.z),
        landed_floor: Some(final_feet),
        dbg_is_a_stop,
        dbg_stop_slope,
        dbg_slope_angle,
        dbg_stop_steps,
        dbg_tall_wall: tall_wall_before_step,
        dbg_step_slope,
        dbg_step_height,
        dbg_stop_wall,
        dbg_wall_height,
        dbg_stop_door,
        dbg_stop_mob,
        dbg_soft_timer,
        dbg_block_nx: block_normal.map(|n| n.x).unwrap_or(0.0),
        dbg_block_ny: block_normal.map(|n| n.y).unwrap_or(0.0),
        dbg_block_nz: block_normal.map(|n| n.z).unwrap_or(0.0),
        dbg_reason,
        dbg_hit_x: block_point.map(|p| p.x).unwrap_or(0.0),
        dbg_hit_y: block_point.map(|p| p.y).unwrap_or(0.0),
        dbg_hit_z: block_point.map(|p| p.z).unwrap_or(0.0),
    }
}

/// Continuous stair-ride height at this tick's destination xz: the detector's measured
/// ramp line, anchored so the flat approach before the first riser stays at current foot
/// level and rise accumulates only past it (see SLOPE-RIDE in [`resolve_position`]).
/// Returns `None` when the ride doesn't apply this tick. All xz are bevy space; `plane_y`
/// is current foot level (bevy up).
fn slope_ride_feet(
    det: &super::input::StairDetection,
    plane_y: f32,
    here: Vec2,
    target: Vec2,
) -> Option<f32> {
    // One riser + margin — a larger single-tick delta means the fit is lying.
    const MAX_TICK_DELTA: f32 = 0.45;

    let slope = det.best_slope;
    if !slope.is_finite() || slope.abs() < 0.02 {
        return None; // not a stair-grade surface
    }
    // Ramp direction from the detector's gizmo endpoints (near = player-side anchor).
    let line = det.ramp_far.0 - det.ramp_near.0;
    let len = line.length();
    if len < 1e-4 {
        return None; // degenerate line — not actually on/beside a locked ramp
    }
    let dir = line / len;
    let here_t = ((here - det.ramp_near.0).dot(dir)).max(0.0);
    let target_t = (target - det.ramp_near.0).dot(dir);
    if target_t <= here_t {
        return None; // no progress along the ramp this tick — stay flat, ground-snap handles it
    }
    // The march measured where the first riser sits relative to the player; before it,
    // the surface is flat at foot level. (Pink-fit lock with no march data: near-zero approach.)
    let d0 = det.march_first_riser_rel.unwrap_or(0.3);
    let feet = plane_y + slope * ((target_t - (here_t + d0)).max(0.0));
    if (feet - plane_y).abs() > MAX_TICK_DELTA {
        return None;
    }
    Some(feet)
}

/// Like `probe` but returns (distance, normal) for a masked down/any cast.
#[allow(dead_code)]
fn probe_hit(
    sq: &SpatialQuery,
    col: &Collider,
    from: Vec3,
    dir: Dir3,
    max: f32,
    mask: LayerMask,
) -> Option<(f32, Vec3)> {
    let hit = sq.cast_shape(
        col,
        from,
        Quat::IDENTITY,
        dir,
        &ShapeCastConfig::from_max_distance(max),
        &SpatialQueryFilter::from_mask(mask),
    )?;
    Some((hit.distance, hit.normal1.into()))
}

/// move_and_slide that only treats WALLS as blocking. Any contact whose surface
/// is walkable (normal within SLOPE_MAX_ANGLE of straight up: floor, ramps up to
/// 60deg, stair treads) returns `Ignore` -- the slide does not stop or deflect on
/// it, so walkable ground never blocks horizontal travel (fixes "stuck on flat
/// floor" and removes any surface the slide could ride up = no goat). Steeper
/// faces (>60deg = true walls) return `Accept` and block/slide as normal.
fn slide_walls_only(
    mas: &MoveAndSlide,
    col: &Collider,
    from: Vec3,
    vel: Vec3,
    dt: f32,
    filter: &SpatialQueryFilter,
    // OUT: the normal of the first blocking (non-walkable) contact, if any.
    // Some(normal) => the slide was stopped by a wall/steep face this tick.
    block_normal: &mut Option<Vec3>,
    // OUT: the entity of the first blocking contact, for layer classification.
    block_entity: &mut Option<Entity>,
    // OUT: the world contact POINT of the block (where collision happened).
    block_point: &mut Option<Vec3>,
) -> Vec3 {
    if vel.length_squared() < 1e-12 || dt <= 0.0 {
        return from;
    }
    let mut captured: Option<Vec3> = None;
    let mut captured_ent: Option<Entity> = None;
    let mut captured_pt: Option<Vec3> = None;
    let pos = mas
        .move_and_slide(
            col,
            from,
            Quat::IDENTITY,
            vel,
            Duration::from_secs_f32(dt),
            &MoveAndSlideConfig::default(),
            filter,
            |hit| {
                // hit.normal is a Dir3 pointing away from the character. Up-y
                // >= cos(60deg) => walkable => ignore (not a wall). Otherwise
                // it's a blocking face: capture its normal for classification.
                let n: Vec3 = (*hit.normal).into();
                if n.y >= SLOPE_MAX_ANGLE.cos() {
                    MoveAndSlideHitResponse::Ignore
                } else {
                    // SANITY GATE: a real block is within the capsule's reach. If
                    // avian reports a contact point far from the player (a
                    // depenetration artifact or degenerate trimesh contact
                    // returning garbage coords), it is NOT in front of us --
                    // ignore it instead of treating distant geometry as a wall.
                    let pt: Vec3 =
                        Vec3::new(hit.point.x as f32, hit.point.y as f32, hit.point.z as f32);
                    let reach = RADIUS + HALF + 0.5; // capsule reach + margin
                    let horiz = Vec2::new(pt.x - from.x, pt.z - from.z).length();
                    if horiz > reach {
                        // Contact is not actually in front of us -> phantom.
                        MoveAndSlideHitResponse::Ignore
                    } else {
                        if captured.is_none() {
                            captured = Some(n);
                            captured_ent = Some(hit.entity);
                            captured_pt = Some(pt);
                        }
                        MoveAndSlideHitResponse::Accept
                    }
                }
            },
        )
        .position;
    *block_normal = captured;
    *block_entity = captured_ent;
    *block_point = captured_pt;
    pos
}
