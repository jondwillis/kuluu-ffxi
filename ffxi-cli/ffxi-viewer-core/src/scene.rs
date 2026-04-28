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

use bevy::prelude::*;
use ffxi_viewer_wire::{EntityKind, Vec3 as WireVec3};

use crate::components::{IsSelf, WorldEntity};
use crate::snapshot::SceneState;

/// Map a wire-side FFXI position to a Bevy world position.
#[inline]
pub fn ffxi_to_bevy(p: WireVec3) -> Vec3 {
    Vec3::new(p.x, p.z, -p.y)
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
}

/// Cached entity mesh — single capsule shared by all spawned entities.
#[derive(Resource)]
pub struct EntityMesh(pub Handle<Mesh>);

/// Tracks which Bevy entity represents each wire entity id, so we can
/// move/despawn it across frames without scanning the world.
#[derive(Resource, Default)]
pub struct TrackedEntities {
    pub by_id: HashMap<u32, Entity>,
}

/// Startup system: ground plane, key light, and the cached materials.
pub fn setup_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(EntityMaterials {
        pc: materials.add(color_material(Color::srgb(0.40, 0.85, 1.00))),
        self_pc: materials.add(color_material(Color::srgb(0.20, 1.00, 1.00))),
        npc: materials.add(color_material(Color::srgb(0.95, 0.85, 0.30))),
        mob: materials.add(color_material(Color::srgb(0.95, 0.40, 0.40))),
        pet: materials.add(color_material(Color::srgb(0.40, 0.85, 0.50))),
        other: materials.add(color_material(Color::srgb(0.60, 0.60, 0.60))),
    });
    commands.insert_resource(EntityMesh(
        meshes.add(Capsule3d::new(0.5, 1.4)),
    ));

    // Ground: a 200×200 plane at y=0 in muted dark slate.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(200.0, 200.0))),
        MeshMaterial3d(materials.add(color_material(Color::srgb(0.08, 0.08, 0.10)))),
        Transform::from_translation(Vec3::ZERO),
    ));

    // A single directional light angled from above-right.
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(50.0, 100.0, 50.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Sync wire-side entities with the Bevy world. Spawns new entities,
/// updates transforms for existing ones, despawns missing ones. Also
/// tracks the player's own avatar via `IsSelf` for the camera follow
/// system to find.
///
/// Heuristic for "self": the snapshot's `self_pos` is the truth, but it
/// doesn't carry an id. We mirror it as a *synthetic* tracked entity at
/// id=0 so the camera and any future systems treat it uniformly with
/// other world entities.
pub fn sync_entities_system(
    state: Res<SceneState>,
    mesh: Res<EntityMesh>,
    mats: Res<EntityMaterials>,
    mut tracked: ResMut<TrackedEntities>,
    mut commands: Commands,
    mut q_xform: Query<&mut Transform, With<WorldEntity>>,
) {
    if !state.dirty {
        return;
    }

    // Wire entities first.
    let mut seen: std::collections::HashSet<u32> =
        std::collections::HashSet::with_capacity(state.snapshot.entities.len() + 1);

    for wire in &state.snapshot.entities {
        seen.insert(wire.id);
        let world_pos = ffxi_to_bevy(wire.pos);
        match tracked.by_id.get(&wire.id).copied() {
            Some(existing) => {
                if let Ok(mut t) = q_xform.get_mut(existing) {
                    t.translation = world_pos;
                    t.rotation = heading_to_quat(wire.heading);
                }
            }
            None => {
                let mat = pick_material(&mats, wire.kind, false);
                let bevy_e = commands
                    .spawn((
                        WorldEntity {
                            id: wire.id,
                            act_index: wire.act_index,
                            kind: wire.kind,
                        },
                        Mesh3d(mesh.0.clone()),
                        MeshMaterial3d(mat),
                        Transform {
                            translation: world_pos,
                            rotation: heading_to_quat(wire.heading),
                            ..default()
                        },
                    ))
                    .id();
                tracked.by_id.insert(wire.id, bevy_e);
            }
        }
    }

    // Synthetic self at id=0.
    seen.insert(0);
    let self_pos = ffxi_to_bevy(state.snapshot.self_pos.pos);
    let self_rot = heading_to_quat(state.snapshot.self_pos.heading);
    match tracked.by_id.get(&0).copied() {
        Some(existing) => {
            if let Ok(mut t) = q_xform.get_mut(existing) {
                t.translation = self_pos;
                t.rotation = self_rot;
            }
        }
        None => {
            let bevy_e = commands
                .spawn((
                    WorldEntity {
                        id: 0,
                        act_index: 0,
                        kind: EntityKind::Pc,
                    },
                    IsSelf,
                    Mesh3d(mesh.0.clone()),
                    MeshMaterial3d(mats.self_pc.clone()),
                    Transform {
                        translation: self_pos,
                        rotation: self_rot,
                        ..default()
                    },
                ))
                .id();
            tracked.by_id.insert(0, bevy_e);
        }
    }

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

/// FFXI heading 0..=255 maps to 0..2π. Heading 0 = +y in FFXI = -z in Bevy
/// = "camera-forward in default pose". Rotation axis is Bevy's Y-up.
fn heading_to_quat(heading: u8) -> Quat {
    let angle = (heading as f32) * std::f32::consts::TAU / 256.0;
    Quat::from_rotation_y(angle)
}

fn color_material(color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.85,
        ..default()
    }
}
