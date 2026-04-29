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

use crate::components::{IsSelf, Nameplate, WorldEntity};
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
    /// Bright yellow + emissive so the targeted entity stands out.
    pub target: Handle<StandardMaterial>,
    /// Aggro override: saturated red + emissive for mobs targeting the player.
    pub aggro: Handle<StandardMaterial>,
}

/// Marker for entities currently aggroing the player. Inserted by
/// `sync_aggro_system`.
#[derive(Component)]
pub struct Aggroing;

/// HP bar quad parented to a `WorldEntity`. Width rescaled per tick from
/// entity hp_pct; color lerps red↔green by HP fraction.
#[derive(Component)]
pub struct HpBar {
    pub owner_id: u32,
}

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

/// HP bar mesh — a horizontal cuboid used for all HP indicators.
#[derive(Resource)]
pub struct HpBarMesh(pub Handle<Mesh>);

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
pub fn setup_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mk = |c: Color, m: &mut Assets<StandardMaterial>| {
        m.add(StandardMaterial {
            base_color: c,
            perceptual_roughness: 0.7,
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
    commands.insert_resource(HpBarMesh(
        meshes.add(Cuboid::new(1.0, 0.12, 0.12)),
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
    hp_bar_mesh: Res<HpBarMesh>,
    mats: Res<EntityMaterials>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut tracked: ResMut<TrackedEntities>,
    mut commands: Commands,
    mut q_xform: Query<&mut Transform, With<WorldEntity>>,
    mut q_mat: Query<&mut MeshMaterial3d<StandardMaterial>, With<WorldEntity>>,
    mut q_hp: Query<(&HpBar, &mut Transform, &mut MeshMaterial3d<StandardMaterial>), Without<WorldEntity>>,
    q_nameplates: Query<&Nameplate>,
) {
    if !state.dirty && !target.is_changed() {
        return;
    }

    let snap = &state.snapshot;
    let self_id: u32 = 0;

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

    for wire in &snap.entities {
        seen.insert(wire.id);
        hp_by_id.insert(wire.id, wire.hp_pct);
        let world_pos = ffxi_to_bevy(wire.pos);
        let is_target = target.id == Some(wire.id);
        let mat = if is_target {
            mats.target.clone()
        } else {
            pick_material(&mats, wire.kind, false)
        };

        match tracked.by_id.get(&wire.id).copied() {
            Some(existing) => {
                if let Ok(mut t) = q_xform.get_mut(existing) {
                    t.translation = world_pos;
                    t.rotation = heading_to_quat(wire.heading);
                }
                if let Ok(mut m) = q_mat.get_mut(existing) {
                    m.0 = mat;
                }
            }
            None => {
                let bevy_e = commands
                    .spawn((
                        WorldEntity {
                            id: wire.id,
                            act_index: wire.act_index,
                            kind: wire.kind,
                        },
                        Mesh3d(pick_mesh(&mesh, wire.kind)),
                        MeshMaterial3d(mat),
                        Transform {
                            translation: world_pos,
                            rotation: heading_to_quat(wire.heading),
                            ..default()
                        },
                    ))
                    .id();
                tracked.by_id.insert(wire.id, bevy_e);

                // Spawn HP bar for kinds that have HP.
                if matches!(wire.kind, EntityKind::Mob | EntityKind::Pc | EntityKind::Pet) {
                    let bar_color = hp_color(wire.hp_pct);
                    commands.spawn((
                        HpBar { owner_id: wire.id },
                        Mesh3d(hp_bar_mesh.0.clone()),
                        MeshMaterial3d(materials.add(StandardMaterial {
                            base_color: bar_color,
                            perceptual_roughness: 0.5,
                            ..default()
                        })),
                        Transform::from_xyz(0.0, 1.5, 0.0),
                        ChildOf(bevy_e),
                    ));
                }
            }
        }

        // Reconcile nameplate independently of the spawn-vs-update branch:
        // a PC that first appeared with `name = None` (common — names
        // resolve a frame after the entity does) must still get a label
        // once the name fills in. Skip empty/missing names (untargetable
        // scenery NPCs come through this way).
        if let Some(name) = wire.name.as_deref().filter(|s| !s.is_empty()) {
            if !nameplated.contains(&wire.id) {
                crate::nameplate::spawn_nameplate(
                    &mut commands,
                    wire.id,
                    wire.kind,
                    name,
                    nameplate_color(wire.kind),
                );
                nameplated.insert(wire.id);
            }
        }
    }

    // Synthetic self at id=0.
    seen.insert(self_id);
    let self_pos = ffxi_to_bevy(snap.self_pos.pos);
    let self_rot = heading_to_quat(snap.self_pos.heading);
    match tracked.by_id.get(&self_id).copied() {
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
                        id: self_id,
                        act_index: 0,
                        kind: EntityKind::Pc,
                    },
                    IsSelf,
                    Mesh3d(mesh.pc.clone()),
                    MeshMaterial3d(mats.self_pc.clone()),
                    Transform {
                        translation: self_pos,
                        rotation: self_rot,
                        ..default()
                    },
                ))
                .id();
            tracked.by_id.insert(self_id, bevy_e);
        }
    }

    // Self nameplate reconciliation: same pattern as wire entities.
    // `char_name` is `None` on the very first frame (before the lobby
    // reply lands), so the first-spawn-only path used to permanently
    // miss it.
    if let Some(name) = snap.char_name.as_deref().filter(|s| !s.is_empty()) {
        if !nameplated.contains(&self_id) {
            crate::nameplate::spawn_nameplate(
                &mut commands,
                self_id,
                EntityKind::Pc,
                name,
                nameplate_color(EntityKind::Pc),
            );
            nameplated.insert(self_id);
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

    // Update HP bars.
    for (bar, mut t, mut hm) in q_hp.iter_mut() {
        if let Some(Some(pct)) = hp_by_id.get(&bar.owner_id).copied() {
            let frac = (pct as f32 / 100.0).clamp(0.0, 1.0);
            t.scale.x = frac;
            t.translation.x = -(1.0 - frac) * 0.5;
            hm.0 = materials.add(StandardMaterial {
                base_color: hp_color(Some(pct)),
                perceptual_roughness: 0.5,
                ..default()
            });
        } else {
            t.scale.x = 0.0;
        }
    }
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
    self_q: Query<&Transform, (With<IsSelf>, Without<WorldEntity>)>,
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

    let mut aggroing: HashMap<u32, bool> = HashMap::new();
    for ent in &snap.entities {
        if ent.bt_target_id as u16 == self_uid
            && matches!(ent.kind, EntityKind::Mob | EntityKind::Pet)
        {
            aggroing.insert(ent.id, true);
        }
    }

    let self_pos = self_q.single().ok().map(|t| t.translation);

    for (e, w, t, mut m, has_aggro) in q.iter_mut() {
        let should_aggro = aggroing.get(&w.id).copied().unwrap_or(false);
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
                let restore = if Some(w.id) == target.id {
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

/// Nameplate text color per entity kind. Matches the body-material palette
/// roughly, brightened so the label is legible against the dark scene.
fn nameplate_color(kind: EntityKind) -> Color {
    match kind {
        EntityKind::Pc => Color::srgb(0.55, 0.95, 1.0),
        EntityKind::Npc => Color::srgb(1.0, 0.92, 0.55),
        EntityKind::Mob => Color::srgb(1.0, 0.55, 0.55),
        EntityKind::Pet => Color::srgb(0.55, 0.95, 0.65),
        EntityKind::Other => Color::srgb(0.85, 0.85, 0.85),
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

/// HP bar color: green at 100%, yellow at 50%, red at 0%.
fn hp_color(pct: Option<u8>) -> Color {
    let frac = pct.unwrap_or(100) as f32 / 100.0;
    let r = frac.min(1.0);
    let g = (1.0 - frac).min(1.0);
    Color::srgb(r, g, 0.0)
}
