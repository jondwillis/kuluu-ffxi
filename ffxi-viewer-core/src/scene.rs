//! 3D scene: spawns ground + light at startup, syncs primitive meshes with
//! the wire entity list each frame.
//!
//! Axis convention: FFXI uses (x, y horizontal, z vertical/up). Bevy is
//! y-up. The mapping is therefore:
//!
//! ```text
//! Bevy (x, y, z) = FFXI (x, z, -y)
//! ```
//!
//! That is: FFXI's vertical axis (`z`) becomes Bevy's vertical (`y`), and
//! FFXI's `y` (north-ish) becomes Bevy's `-z` (camera-forward in the
//! default pose). See [`ffxi_to_bevy`].

use std::collections::HashMap;

use bevy::light::FogVolume;
use bevy::picking::Pickable;
use bevy::prelude::*;
use ffxi_viewer_wire::{EntityKind, Vec3 as WireVec3};

use crate::components::{IsSelf, LookComp, Nameplate, WorldEntity};
use crate::graphics_settings::GraphicsSettings;
use crate::snapshot::SceneState;

/// Map a wire-side FFXI position to a Bevy world position.
///
/// FFXI is Y-down (height grows toward negative Z when laid out in
/// the client's native frame). Bevy is Y-up. The transform is
/// therefore `Bevy = (x, -z, -y)`: negate Z for the up-axis sign,
/// negate Y for Z-handedness. Empirically the previous `(x, z, -y)`
/// rendered the whole world upside-down — buildings, navmesh and
/// entities all share this convention so the fix applies in lockstep
/// at all three coordinate-conversion sites (here, `dat_mzb.rs`,
/// `navmesh_overlay.rs::detour_to_bevy`).
#[inline]
pub fn ffxi_to_bevy(p: WireVec3) -> Vec3 {
    Vec3::new(p.x, -p.z, -p.y)
}

/// Vertical distance from an entity's `transform.y` (the mesh center)
/// down to the bottom of its visual mesh — i.e., its "feet."
///
/// Mirrors the meshes built by [`setup_world`]:
///   - `Capsule3d::new(radius, half_length)` → bottom at center - (half_length + radius)
///   - `Cuboid::new(_, side, _)` → bottom at center - side/2
///
/// Used by:
///   - The navmesh gravity-snap (so feet sit on the navmesh, not the
///     center sinks below it).
///   - The target ring (so the ring sits at the entity's *ground*
///     level, not at a hardcoded y=0).
///
/// If the mesh constructors in `setup_world` change, update here.
#[inline]
pub fn feet_offset(kind: EntityKind) -> f32 {
    match kind {
        EntityKind::Pc => 1.9 + 0.35, // pc capsule (radius=0.35, half_length=1.9)
        EntityKind::Pet => 0.6 + 0.4, // pet capsule (radius=0.4, half_length=0.6)
        EntityKind::Mob => 1.1 / 2.0, // mob cuboid (1.1 cube)
        _ => 1.4 + 0.5,               // default capsule (radius=0.5, half_length=1.4)
    }
}

/// Marker inserted on an entity once its baked PC/NPC mesh has been
/// spawned (see `dat_vos2::spawn_equipped`). The snap and target-ring
/// systems consult this to switch their visual offset model: a baked
/// mesh's origin is the **skeleton root (hip)**, not the mesh center
/// the legacy capsule assumed.
///
/// `min_mesh_y` is the **lowest** Y of any baked vertex in Bevy frame —
/// i.e., the actual hip-to-foot distance for this specific actor.
/// Populated by the bake-extent walk in
/// [`crate::dat_vos2::spawn_skinned_actor`] /
/// `spawn_vos2_meshes_with_skeleton` and surfaced by `/debug heights`,
/// so the operator can spot when [`visual_root_offset`]'s default `0.9`
/// disagrees with reality (Taru shorter, Galka taller).
#[derive(Component, Clone, Copy, Debug)]
pub struct BakedActor {
    pub min_mesh_y: f32,
}

/// Distance from the entity transform's y down to the "ground"
/// reference plane the snap should land on.
///
/// - Legacy capsule visual: returns [`feet_offset`] (capsule center →
///   capsule bottom).
/// - Baked-mesh visual: returns ~hip-to-foot distance. The bake's
///   mesh-y=0 is the **skeleton root (hip)**, not feet: the head-slot
///   bake extent (y=[0.84..1.59] post-rotation) is *neck-to-top-of-
///   head*, and feet are at negative y (legs/feet slot extends below
///   the hip). To put the mesh's feet at ground, the entity transform
///   needs to be at `ground + hip_height`.
#[inline]
pub fn visual_root_offset(kind: EntityKind, has_baked_mesh: bool) -> f32 {
    if has_baked_mesh {
        // ~0.9 yalm — typical PC hip-to-foot distance derived from
        // the bake-extent analysis. Per-race tuning (Taru shorter,
        // Galka taller) is a follow-up; this constant is acceptable
        // until a character of the wrong race shows visible float/sink.
        return 0.9;
    }
    feet_offset(kind)
}

/// Per-frame visual smoothing for *non-self* entity transforms.
///
/// Self position is updated 60 Hz from `dispatch_movement_system`'s
/// FixedUpdate, so it's snapped directly — any smoothing on self compounds
/// with the chase-camera lerp into perceptible input lag. Other entities
/// (mobs, PCs) come from server packets at variable cadence (often below
/// 60 Hz), so smoothing hides that stair-step.
///
/// Snaps when the gap exceeds the threshold so zone transitions / warps
/// don't interpolate through walls — anything ≥ 2 yalms is a discontinuity.
const VISUAL_SMOOTH: f32 = 0.4;
const SNAP_DIST_SQ: f32 = 4.0;

#[inline]
fn apply_visual_smoothing(current: Vec3, target: Vec3) -> Vec3 {
    if current.distance_squared(target) >= SNAP_DIST_SQ {
        target
    } else {
        current.lerp(target, VISUAL_SMOOTH)
    }
}

/// Cached materials per entity kind. Spawned once at startup.
#[derive(Resource)]
pub struct EntityMaterials {
    pub pc: Handle<StandardMaterial>,
    pub self_pc: Handle<StandardMaterial>,
    pub npc: Handle<StandardMaterial>,
    pub mob: Handle<StandardMaterial>,
    pub pet: Handle<StandardMaterial>,
    pub other: Handle<StandardMaterial>,
    /// Bright yellow + emissive so the targeted entity stands out.
    pub target: Handle<StandardMaterial>,
    /// Aggro override: saturated red + emissive for mobs targeting the player.
    pub aggro: Handle<StandardMaterial>,
    /// Mob claimed by the player: bright white. Canonical FFXI cue that
    /// "this mob is mine — fight it, don't worry about claim contention".
    pub mob_claimed_self: Handle<StandardMaterial>,
    /// Mob claimed by another player: muted red. Distinct from the
    /// brighter, emissive `aggro` red so the operator can tell at a glance
    /// "someone else is fighting this" vs. "this is fighting me".
    pub mob_claimed_other: Handle<StandardMaterial>,
}

/// Marker for entities currently aggroing the player. Inserted by
/// `sync_aggro_system`.
#[derive(Component)]
pub struct Aggroing;

/// Cached per-kind entity meshes. Distinct silhouettes give the operator a
/// cheap visual differentiator before nameplates load. PC = tall slim
/// capsule (humanoid); Mob = boxy cuboid; Pet = short capsule; everything
/// else (NPC, Other) shares a "default" mid-sized capsule.
#[derive(Resource)]
pub struct EntityMesh {
    pub default: Handle<Mesh>,
    pub pc: Handle<Mesh>,
    pub mob: Handle<Mesh>,
    pub pet: Handle<Mesh>,
}

/// Currently-targeted FFXI entity id. `None` when no target is selected.
#[derive(Resource, Default)]
pub struct Target {
    pub id: Option<u32>,
}

/// Tracks which Bevy entity represents each wire entity id, so we can
/// move/despawn it across frames without scanning the world.
#[derive(Resource, Default)]
pub struct TrackedEntities {
    pub by_id: HashMap<u32, Entity>,
}

/// Startup system: ground plane, key light, and the cached materials.
///
/// Reads `GraphicsSettings` for the initial cascade config so a player
/// with a persisted non-default preset doesn't see a one-frame pop on
/// zone-in (the reactor systems in
/// `crate::graphics_settings` re-apply on the next change, but spawning
/// straight to the right config avoids that initial mismatch).
pub fn setup_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    settings: Res<GraphicsSettings>,
) {
    let mk = |c: Color, m: &mut Assets<StandardMaterial>| {
        m.add(StandardMaterial {
            base_color: c,
            perceptual_roughness: 1.0,
            metallic: 0.0,
            ..default()
        })
    };
    commands.insert_resource(EntityMaterials {
        pc: mk(Color::srgb(0.40, 0.85, 1.00), &mut materials),
        self_pc: mk(Color::srgb(0.20, 1.00, 1.00), &mut materials),
        npc: mk(Color::srgb(0.95, 0.85, 0.30), &mut materials),
        mob: mk(Color::srgb(0.95, 0.40, 0.40), &mut materials),
        pet: mk(Color::srgb(0.40, 0.85, 0.50), &mut materials),
        other: mk(Color::srgb(0.60, 0.60, 0.60), &mut materials),
        target: materials.add(StandardMaterial {
            base_color: Color::srgb(1.00, 0.95, 0.20),
            emissive: LinearRgba::new(1.0, 0.95, 0.0, 1.0) * 0.6,
            perceptual_roughness: 0.4,
            ..default()
        }),
        aggro: materials.add(StandardMaterial {
            base_color: Color::srgb(1.00, 0.10, 0.10),
            emissive: LinearRgba::new(1.5, 0.0, 0.0, 1.0),
            perceptual_roughness: 0.4,
            ..default()
        }),
        // Self-claimed mob: bright matte white. No emissive — the visual
        // weight of pure white against the dim ground reads as "ours" without
        // competing with the aggro material.
        mob_claimed_self: mk(Color::srgb(0.96, 0.96, 0.96), &mut materials),
        // Other-claimed mob: deep, slightly desaturated red. Calibrated to
        // sit between the unclaimed mob's pinkish red (0.95, 0.40, 0.40)
        // and the aggro material's saturated emissive red — same hue
        // family, distinguishable side-by-side.
        mob_claimed_other: mk(Color::srgb(0.65, 0.10, 0.10), &mut materials),
    });
    commands.insert_resource(EntityMesh {
        default: meshes.add(Capsule3d::new(0.5, 1.4)),
        // PCs: noticeably taller and thinner than NPCs so player characters
        // pop visually in the world.
        pc: meshes.add(Capsule3d::new(0.35, 1.9)),
        // Mobs: boxy. Distinct silhouette from anything humanoid.
        mob: meshes.add(Cuboid::new(1.1, 1.1, 1.1)),
        // Pets: small capsule, hugs the ground.
        pet: meshes.add(Capsule3d::new(0.4, 0.6)),
    });

    // No placeholder ground plane: the navmesh wireframe overlay
    // (`ffxi-client::view_native::navmesh_overlay`) provides terrain
    // visualization, and the gravity-snap system anchors entities to
    // navmesh height — both work better without the flat plane
    // depth-fighting them.

    // Sun + moon directional lights + visible emissive discs. Both
    // are tagged and updated each frame by `sun_moon::sun_moon_system`
    // from Vana'diel time (sun arcs east→west across the V-day; moon
    // is anti-phase; moon brightness follows the 84-day phase cycle).
    crate::sun_moon::spawn_sun_and_moon(&mut commands, &mut meshes, &mut materials, &settings);

    // Zone-scale fog volume. `FogVolume`'s bounds come from its
    // Transform scale (default 1m³); we make it a ~2km cube so the
    // entire FFXI zone sits inside. Density tuned for "heavy
    // atmosphere": air itself visibly scatters but mid-ground stays
    // readable. Pair with the camera's `VolumetricFog` and the
    // directional light's `VolumetricLight` marker above.
    commands.spawn((
        crate::components::InGameEntity,
        FogVolume {
            fog_color: Color::srgb(0.65, 0.72, 0.82),
            density_factor: 0.06,
            absorption: 0.25,
            scattering: 0.35,
            // High asymmetry: light shafts pop only when looking
            // toward the sun. Looking away the air is just hazy.
            scattering_asymmetry: 0.7,
            light_tint: Color::srgb(1.0, 0.96, 0.88),
            light_intensity: 1.0,
            ..default()
        },
        Transform::from_xyz(0.0, 200.0, 0.0).with_scale(Vec3::splat(2000.0)),
    ));
    // Ambient fill. With cascaded shadows enabled the shaded side of
    // walls gets only ambient light, so 120 lux (the original lower
    // bound chosen for shadow contrast) ends up rendering interior
    // floors near-black. 500 lux is a compromise: shadows still read
    // as shadows (~20:1 contrast vs 10k-lux sun) but the texture on
    // back-facing walls stays visible.
    commands.insert_resource(AmbientLight {
        color: Color::srgb(0.85, 0.88, 1.0),
        brightness: 500.0,
        ..default()
    });
}

/// Sync wire-side entities with the Bevy world. Spawns new entities,
/// updates transforms for existing ones, despawns missing ones. Also
/// applies target highlight and manages HP bars.
///
/// Heuristic for "self": the snapshot's `self_pos` is the truth, but it
/// doesn't carry an id. We mirror it as a *synthetic* tracked entity at
/// id=0 so the camera and any future systems treat it uniformly with
/// other world entities.
pub fn sync_entities_system(
    state: Res<SceneState>,
    target: Res<Target>,
    mesh: Res<EntityMesh>,
    mats: Res<EntityMaterials>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    billboard_font: Res<crate::nameplate_billboard::BillboardFont>,
    mut tracked: ResMut<TrackedEntities>,
    mut commands: Commands,
    mut q_xform: Query<&mut Transform, With<WorldEntity>>,
    mut q_mat: Query<&mut MeshMaterial3d<StandardMaterial>, With<WorldEntity>>,
    q_nameplates: Query<&Nameplate>,
) {
    if !state.dirty && !target.is_changed() {
        return;
    }

    let snap = &state.snapshot;

    // Set of entity ids that already own a nameplate. We mutate this as we
    // spawn new ones below so a single tick can't double-spawn for the
    // same id (the ECS Commands queue won't materialize until after this
    // system finishes, so a `Query<&Nameplate>` lookup mid-system would
    // miss anything spawned earlier this tick).
    let mut nameplated: std::collections::HashSet<u32> =
        q_nameplates.iter().map(|n| n.entity_id).collect();

    // Wire entities first.
    let mut seen: std::collections::HashSet<u32> =
        std::collections::HashSet::with_capacity(snap.entities.len() + 1);
    let mut hp_by_id: HashMap<u32, Option<u8>> = HashMap::new();

    // Player's own UniqueNo, used to recognize self-claimed mobs. `None` /
    // 0 until the lobby resolves the player id; falls through to "any
    // non-zero claim is other-claim" in the picker.
    let self_char_id = snap.self_char_id.unwrap_or(0);
    for wire in &snap.entities {
        seen.insert(wire.id);
        hp_by_id.insert(wire.id, wire.hp_pct);
        let world_pos = ffxi_to_bevy(wire.pos);
        let is_target = target.id == Some(wire.id);
        let is_self = self_char_id != 0 && wire.id == self_char_id;
        // Material selection:
        //   - Self: dedicated `self_pc` material (camera-anchored capsule).
        //   - Mobs: claim-aware (white = self-claim, red = other-claim) so
        //     ownership is visible regardless of target state.
        //   - Other PCs/NPCs/pets: target overrides everything else.
        // `sync_aggro_system` runs after this and rewrites materials for
        // entities that should glow red — that's why `is_aggro = false`
        // here.
        let mat = if is_self {
            mats.self_pc.clone()
        } else {
            match wire.kind {
                EntityKind::Mob => {
                    pick_mob_material(&mats, wire.claim_id, self_char_id, is_target, false).clone()
                }
                _ if is_target => mats.target.clone(),
                _ => pick_material(&mats, wire.kind, false),
            }
        };

        match tracked.by_id.get(&wire.id).copied() {
            Some(existing) => {
                if let Ok(mut t) = q_xform.get_mut(existing) {
                    // Self snaps directly to the wire position because the
                    // 60 Hz `dispatch_movement_system` already does its own
                    // integration — extra smoothing here only adds input
                    // lag. Other entities arrive at server tick rate, so
                    // visual smoothing fills the gaps.
                    t.translation = if is_self {
                        world_pos
                    } else {
                        apply_visual_smoothing(t.translation, world_pos)
                    };
                    t.rotation = heading_to_quat(wire.heading);
                }
                if let Ok(mut m) = q_mat.get_mut(existing) {
                    m.0 = mat;
                }
            }
            None => {
                // Self capsule uses `Pickable::IGNORE` because it sits under
                // the chase camera and would intercept every click aimed
                // past it; click-to-target (C4) on self is intentionally
                // unreachable.
                let pickable = if is_self {
                    Pickable::IGNORE
                } else {
                    Pickable::default()
                };
                let mut spawn = commands.spawn((
                    crate::components::InGameEntity,
                    WorldEntity {
                        id: wire.id,
                        act_index: wire.act_index,
                        kind: wire.kind,
                    },
                    pickable,
                    Mesh3d(pick_mesh(&mesh, wire.kind)),
                    MeshMaterial3d(mat),
                    Transform {
                        translation: world_pos,
                        rotation: heading_to_quat(wire.heading),
                        ..default()
                    },
                ));
                if is_self {
                    spawn.insert(IsSelf);
                }
                let bevy_e = spawn.id();
                tracked.by_id.insert(wire.id, bevy_e);

                // HP bar for kinds that have HP — but not for self (the
                // operator HUD already shows self HP/MP, and a floating
                // bar under the chase camera is visual noise).
                if !is_self
                    && matches!(
                        wire.kind,
                        EntityKind::Mob | EntityKind::Pc | EntityKind::Pet
                    )
                {
                    // HP indicator is rendered as filled rectangles
                    // inside the nameplate texture (see
                    // `nameplate_billboard.rs`). No separate 3D entity:
                    // the prior `HpBar` quad parented to the WorldEntity
                    // followed the entity's heading rotation, so it
                    // appeared horizontally-across-the-chest at any
                    // camera angle that wasn't dead-aligned with the
                    // entity's facing direction. Folding the bar into
                    // the nameplate texture lets it inherit the same
                    // Y-locked billboard rotation and stay perpendicular
                    // to the camera for free.
                    let _ = bevy_e;
                }
            }
        }

        // Reconcile nameplate independently of the spawn-vs-update branch:
        // a PC that first appeared with `name = None` (common — names
        // resolve a frame after the entity does) must still get a label
        // once the name fills in. For self, fall back to `snap.char_name`
        // when the LOGIN-seed Entity has `name = None` (CHAR_PC hasn't
        // arrived yet) so the nameplate doesn't briefly disappear after
        // zone-in.
        let name = wire.name.as_deref().or_else(|| {
            if is_self {
                snap.char_name.as_deref()
            } else {
                None
            }
        });
        if let Some(name) = name.filter(|s| !s.is_empty()) {
            if !nameplated.contains(&wire.id) {
                crate::nameplate_billboard::spawn_nameplate_billboard(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &mut images,
                    &billboard_font.0,
                    wire.id,
                    wire.kind,
                    name,
                    // Spawn-time color is the kind-only default with no
                    // combat context. The update system re-derives the
                    // engagement-aware color next tick (mob: aggro vs.
                    // wandering, etc.) and re-rasterizes if needed.
                    crate::nameplate_billboard::nameplate_color(wire.kind, false, false),
                );
                nameplated.insert(wire.id);
            }
        }
    }

    // Self is no longer a synthetic id=0 entity — it now flows through
    // the main loop above as the entity with `wire.id == self_char_id`
    // (seeded from `0x00A LOGIN`'s `PosHead` in `session.rs` before any
    // CHAR_PC arrives). Before this refactor, the synthetic id=0 self
    // capsule lagged the real server-authoritative position on every
    // zone-in because `state.self_pos` updated only from `Move`
    // commands, not from CHAR_PC for self — landing the camera "in the
    // sky / strange place" on zone transition.

    // Despawn entities no longer in the snapshot.
    let stale: Vec<u32> = tracked
        .by_id
        .keys()
        .copied()
        .filter(|id| !seen.contains(id))
        .collect();
    for id in stale {
        if let Some(bevy_e) = tracked.by_id.remove(&id) {
            commands.entity(bevy_e).despawn();
        }
    }

    // HP update path moved into `nameplate_billboard.rs`: the per-
    // frame `update_nameplate_billboards_system` re-rasterizes the
    // nameplate texture (which embeds the HP bar) only when the
    // integer percentage actually changes, gated on a new `last_hp`
    // field on `NameplateBillboard`.
}

/// Per-tick: reconcile the `Aggroing` marker on each ECS entity and
/// override its material. Runs in `Update` after `sync_entities_system`
/// so any kind/target material write from that pass is overwritten on
/// the same frame for entities that just became aggro.
///
/// Uses `bt_target_id` from the snapshot — any mob whose bt_target_id
/// matches the player's UniqueNo is considered aggroing.
pub fn sync_aggro_system(
    mut commands: Commands,
    state: Res<SceneState>,
    target: Res<Target>,
    mats: Res<EntityMaterials>,
    // Drop the `Without<WorldEntity>` filter that used to live here: the
    // synthetic-id=0 self capsule has been removed, and the real self
    // entity (spawned by `sync_entities_system` from the `id ==
    // self_char_id` wire entry) carries *both* `IsSelf` and `WorldEntity`.
    // The disjoint mutable borrow in `q` is still safe — that query is
    // `Without<IsSelf>`, so the two queries never overlap.
    self_q: Query<&Transform, With<IsSelf>>,
    mut q: Query<
        (
            Entity,
            Ref<WorldEntity>,
            &mut Transform,
            &mut MeshMaterial3d<StandardMaterial>,
            Option<&Aggroing>,
        ),
        Without<IsSelf>,
    >,
    mut gizmos: Gizmos,
) {
    let snap = &state.snapshot;
    let self_id = snap.diagnostics.sync_in;
    let Some(self_uid) = self_id else { return };
    // Full u32 player id for claim coloring. Falls back to 0 (unknown)
    // when the lobby hasn't resolved it yet — the picker treats 0 as
    // "can't distinguish self vs. other" and routes any non-zero claim
    // to the "other-claim" branch.
    let self_char_id = snap.self_char_id.unwrap_or(0);
    // Map per-id claim_id so the restore branch below can pick the
    // right "not-aggroing-anymore" material without re-scanning entities.
    let mut claim_by_id: HashMap<u32, u32> = HashMap::new();

    let mut aggroing: HashMap<u32, bool> = HashMap::new();
    for ent in &snap.entities {
        if ent.bt_target_id as u16 == self_uid
            && matches!(ent.kind, EntityKind::Mob | EntityKind::Pet)
        {
            aggroing.insert(ent.id, true);
        }
        if matches!(ent.kind, EntityKind::Mob) {
            claim_by_id.insert(ent.id, ent.claim_id);
        }
    }

    let self_pos = self_q.single().ok().map(|t| t.translation);

    for (e, w, t, mut m, has_aggro) in q.iter_mut() {
        let should_aggro = aggroing.get(&w.id).copied().unwrap_or(false);
        let is_target = Some(w.id) == target.id;
        match (should_aggro, has_aggro.is_some()) {
            (true, false) => {
                commands.entity(e).insert(Aggroing);
                m.0 = mats.aggro.clone();
            }
            (true, true) => {
                m.0 = mats.aggro.clone();
            }
            (false, true) => {
                commands.entity(e).remove::<Aggroing>();
                // Restore through the picker so claim-color survives the
                // aggro→clear transition (white/red mob stays white/red
                // after the player breaks aggro).
                let restore = if matches!(w.kind, EntityKind::Mob) {
                    let claim = claim_by_id.get(&w.id).copied().unwrap_or(0);
                    pick_mob_material(&mats, claim, self_char_id, is_target, false).clone()
                } else if is_target {
                    mats.target.clone()
                } else {
                    pick_material(&mats, w.kind, false)
                };
                m.0 = restore;
            }
            (false, false) => {}
        }

        if should_aggro {
            if let Some(sp) = self_pos {
                gizmos.line(sp, t.translation, Color::srgb(1.0, 0.15, 0.15));
            }
        }
    }
}

fn pick_mesh(m: &EntityMesh, kind: EntityKind) -> Handle<Mesh> {
    match kind {
        EntityKind::Pc => m.pc.clone(),
        EntityKind::Mob => m.mob.clone(),
        EntityKind::Pet => m.pet.clone(),
        EntityKind::Npc | EntityKind::Other => m.default.clone(),
    }
}

fn pick_material(m: &EntityMaterials, kind: EntityKind, is_self: bool) -> Handle<StandardMaterial> {
    if is_self {
        return m.self_pc.clone();
    }
    match kind {
        EntityKind::Pc => m.pc.clone(),
        EntityKind::Npc => m.npc.clone(),
        EntityKind::Mob => m.mob.clone(),
        EntityKind::Pet => m.pet.clone(),
        EntityKind::Other => m.other.clone(),
    }
}

/// Pure decision: which material should a mob capsule use?
///
/// Priority (high → low):
///   1. **Aggro** — the mob is targeting the player. Always wins; the
///      operator needs to see "this is fighting me" regardless of who
///      claimed it (especially the case of a kited claimed mob aggroing
///      a passerby).
///   2. **Target ring** — the operator's selected target. Yellow.
///   3. **Self-claim** — `claim_id == self_id`, both non-zero. White.
///   4. **Other-claim** — `claim_id != 0 && claim_id != self_id`. Muted red.
///   5. **Unclaimed** — `claim_id == 0`. Default mob material (yellow tint).
///
/// `self_id == 0` means "we don't know our own UniqueNo yet"; in that
/// state we can't distinguish self-claim from other-claim, so any
/// non-zero `claim_id` falls through to "other".
pub fn pick_mob_material<'a>(
    mats: &'a EntityMaterials,
    claim_id: u32,
    self_id: u32,
    is_target: bool,
    is_aggro: bool,
) -> &'a Handle<StandardMaterial> {
    if is_aggro {
        return &mats.aggro;
    }
    if is_target {
        return &mats.target;
    }
    if claim_id == 0 {
        return &mats.mob;
    }
    if self_id != 0 && claim_id == self_id {
        &mats.mob_claimed_self
    } else {
        &mats.mob_claimed_other
    }
}

/// FFXI heading 0..=255 maps to 0..2π. Heading 0 = +y in FFXI = -z in Bevy
/// = "camera-forward in default pose". Rotation axis is Bevy's Y-up.
///
/// Sign: FFXI heading increases clockwise from above (0=N, 64=E, 128=S,
/// 192=W). Bevy `Quat::from_rotation_y(+θ)` rotates counterclockwise from
/// above. So heading→yaw needs a sign flip; matches the convention in
/// `camera::yaw_for_heading`.
fn heading_to_quat(heading: u8) -> Quat {
    let angle = (heading as f32) * std::f32::consts::TAU / 256.0;
    Quat::from_rotation_y(-angle)
}

/// Copy each wire entity's `look` field onto its Bevy `WorldEntity` as
/// a [`LookComp`], but only when the value actually changed. The
/// inserted/removed `LookComp` is what downstream look-driven systems
/// (model spawning in Stage 3+) hang off via `Changed<LookComp>`
/// queries.
///
/// Why "only when changed": `commands.entity(e).insert(...)` is cheap,
/// but each insert *touches* the component and a downstream
/// `Changed<LookComp>` filter would then fire every snapshot tick —
/// many times per second per entity — even when the value is
/// byte-identical to last tick. The explicit compare-then-insert here
/// is what makes Bevy's change-detection meaningful for this surface.
/// Launcher → game look bridge. The retail FFXI server zeros the
/// self GrapIDTbl in CHAR_PC because the retail client rebuilds
/// appearance from local equipment state — but we don't have that
/// state on the wire (`session.rs:678` documents the empty slot).
/// The launcher knows the player's appearance (`CharSlot`); it
/// writes it into this resource before connecting, and
/// [`ensure_self_lookcomp_system`] applies it to the self
/// `WorldEntity` whenever the wire's `look` is empty.
///
/// `None` (the default) means no self override — entities use their
/// wire look as-is. This is the launcher pre-login state, the wasm
/// browser flow, and headless / MCP sessions.
#[derive(Resource, Default, Debug, Clone)]
pub struct SelfAppearance {
    pub look: Option<ffxi_viewer_wire::EntityLook>,
}

/// Ensure the self entity has a `LookComp` whenever a
/// [`SelfAppearance`] override is set. Runs *after*
/// `sync_entity_looks_system` so a real wire look (if it ever
/// arrives) takes precedence — we only fill in when wire-side is
/// absent.
pub fn ensure_self_lookcomp_system(
    appearance: Res<SelfAppearance>,
    q_self: Query<(Entity, Option<&LookComp>), With<IsSelf>>,
    mut commands: Commands,
) {
    let Some(look) = appearance.look.as_ref() else {
        return;
    };
    for (e, current) in q_self.iter() {
        let needs = match current {
            None => true,
            Some(LookComp(existing)) => existing != look,
        };
        if needs {
            commands.entity(e).insert(LookComp(look.clone()));
        }
    }
}

pub fn sync_entity_looks_system(
    state: Res<SceneState>,
    tracked: Res<TrackedEntities>,
    q_look: Query<&LookComp>,
    mut commands: Commands,
) {
    if !state.dirty {
        return;
    }
    for wire in &state.snapshot.entities {
        let Some(&bevy_e) = tracked.by_id.get(&wire.id) else {
            continue;
        };
        let current = q_look.get(bevy_e).ok();
        match (&wire.look, current) {
            (Some(new), Some(LookComp(old))) if new == old => {}
            (Some(new), _) => {
                commands.entity(bevy_e).insert(LookComp(new.clone()));
            }
            (None, Some(_)) => {
                commands.entity(bevy_e).remove::<LookComp>();
            }
            (None, None) => {}
        }
    }
}

/// React to look changes for spawned entities — Stage 3+ fills this in
/// with `LoadMmbRequest` dispatch. Today it's a hook that just logs the
/// transition (at debug level) so we can verify change-detect plumbing.
///
/// Uses Bevy's `Changed<LookComp>` rather than re-comparing snapshot
/// state because [`sync_entity_looks_system`] already absorbed the
/// "snapshot says X, world says Y" reconciliation upstream.
///
/// Note: empirically `Changed<LookComp>` fires every frame on the local
/// PC during movement, even when the look bytes don't actually change.
/// Demoted from `info!` to `debug!` so it stays opt-in via RUST_LOG;
/// the underlying false-positive is a separate fix.
pub fn process_entity_look_changes(q_changed: Query<(&WorldEntity, &LookComp), Changed<LookComp>>) {
    for (we, look) in q_changed.iter() {
        debug!(
            "look changed for entity {} ({:?}): {:?}",
            we.id, we.kind, look.0
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Small deltas (normal movement) lerp toward the target so the
    /// rendered position catches up over a few frames; large deltas (zone
    /// transitions, warps) snap so the avatar doesn't slide across the map.
    #[test]
    fn visual_smoothing_lerps_short_then_snaps_long() {
        // 0.25-yalm tick (= one server-cadence step at speed=25).
        let near = apply_visual_smoothing(Vec3::ZERO, Vec3::new(0.25, 0.0, 0.0));
        assert!(near.x > 0.0 && near.x < 0.25, "lerp partial: {}", near.x);
        assert!(
            (near.x - 0.1).abs() < 1e-6,
            "VISUAL_SMOOTH=0.4 → 0.25 * 0.4 = 0.1, got {}",
            near.x
        );

        // 50-yalm jump (zone change). Snap.
        let far = apply_visual_smoothing(Vec3::ZERO, Vec3::new(50.0, 0.0, 0.0));
        assert_eq!(far, Vec3::new(50.0, 0.0, 0.0));
    }

    /// Build an `EntityMaterials` whose handles are all `Handle::default()`
    /// — *distinct values* aren't required because the tests compare
    /// `&Handle` references for pointer equality, not the underlying
    /// `AssetId`. Using `Handle::default()` keeps the fixture tiny and
    /// avoids dragging in `App` / `MinimalPlugins` for a pure decision-
    /// logic test.
    fn dummy_materials() -> EntityMaterials {
        EntityMaterials {
            pc: Handle::default(),
            self_pc: Handle::default(),
            npc: Handle::default(),
            mob: Handle::default(),
            pet: Handle::default(),
            other: Handle::default(),
            target: Handle::default(),
            aggro: Handle::default(),
            mob_claimed_self: Handle::default(),
            mob_claimed_other: Handle::default(),
        }
    }

    /// Mob with no owner gets the default mob material — preserves the
    /// pre-claim behavior for unclaimed mobs.
    #[test]
    fn pick_mob_material_unclaimed_uses_default_mob() {
        let mats = dummy_materials();
        let h = pick_mob_material(&mats, 0, 0xCAFE, false, false);
        assert!(std::ptr::eq(h, &mats.mob), "unclaimed mob → mats.mob");
    }

    /// `claim_id == self_id` (both non-zero) → self-claim white.
    #[test]
    fn pick_mob_material_self_claim_uses_white() {
        let mats = dummy_materials();
        let h = pick_mob_material(&mats, 0xCAFE, 0xCAFE, false, false);
        assert!(std::ptr::eq(h, &mats.mob_claimed_self));
    }

    /// `claim_id != 0 && claim_id != self_id` → other-claim red.
    /// Also exercises the `self_id == 0` (unknown player id) path: any
    /// non-zero claim falls through to "other".
    #[test]
    fn pick_mob_material_other_claim_uses_muted_red() {
        let mats = dummy_materials();
        let h = pick_mob_material(&mats, 0x4242, 0xCAFE, false, false);
        assert!(
            std::ptr::eq(h, &mats.mob_claimed_other),
            "other player's claim"
        );
        let h_unknown_self = pick_mob_material(&mats, 0x4242, 0, false, false);
        assert!(
            std::ptr::eq(h_unknown_self, &mats.mob_claimed_other),
            "unknown self_id falls through to other-claim",
        );
    }

    /// Aggro must override every claim state — even a self-claimed mob
    /// shows aggro red when it's targeting the player. The operator
    /// needs the "this is fighting me" cue to dominate.
    #[test]
    fn pick_mob_material_aggro_overrides_claim() {
        let mats = dummy_materials();
        let h_self = pick_mob_material(&mats, 0xCAFE, 0xCAFE, false, true);
        assert!(std::ptr::eq(h_self, &mats.aggro), "aggro > self-claim");
        let h_other = pick_mob_material(&mats, 0x4242, 0xCAFE, false, true);
        assert!(std::ptr::eq(h_other, &mats.aggro), "aggro > other-claim");
        let h_unclaimed = pick_mob_material(&mats, 0, 0xCAFE, false, true);
        assert!(std::ptr::eq(h_unclaimed, &mats.aggro), "aggro > unclaimed");
        // Aggro also beats target highlight — the existing system already
        // had this implicit ordering; the helper makes it explicit.
        let h_target_too = pick_mob_material(&mats, 0xCAFE, 0xCAFE, true, true);
        assert!(std::ptr::eq(h_target_too, &mats.aggro), "aggro > target");
    }

    /// Target ring beats claim coloring (when not aggroing): the operator
    /// needs the "this is what I selected" cue regardless of who claimed
    /// it. Aggro still beats target — the priority chain is aggro >
    /// target > claim > unclaimed.
    #[test]
    fn pick_mob_material_target_overrides_claim_when_not_aggro() {
        let mats = dummy_materials();
        let h = pick_mob_material(&mats, 0xCAFE, 0xCAFE, true, false);
        assert!(std::ptr::eq(h, &mats.target));
    }

    /// At exactly the snap threshold, snap (not lerp). Below threshold,
    /// lerp. The boundary catches the off-by-one error of using a strict
    /// inequality the wrong way.
    #[test]
    fn visual_smoothing_snap_threshold_boundary() {
        let just_under = (SNAP_DIST_SQ - 1e-3).sqrt();
        let result = apply_visual_smoothing(Vec3::ZERO, Vec3::new(just_under, 0.0, 0.0));
        // Lerp would give result.x = just_under * VISUAL_SMOOTH ≈ 0.8.
        assert!(
            result.x < just_under,
            "below threshold should lerp, got {}",
            result.x
        );

        let at_threshold = SNAP_DIST_SQ.sqrt();
        let result = apply_visual_smoothing(Vec3::ZERO, Vec3::new(at_threshold, 0.0, 0.0));
        assert_eq!(result.x, at_threshold, "at threshold should snap");
    }
}
