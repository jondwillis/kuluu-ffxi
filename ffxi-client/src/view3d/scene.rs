//! Scene composition: spawn/move/despawn capsule meshes mirroring the
//! protocol-decoded entities in the `SessionStateSnapshot`. The camera
//! and floor live in their own modules; this one only knows about
//! "things that exist in the world."

use std::collections::HashMap;

use bevy::prelude::*;

use crate::state::{Entity as ProtoEntity, EntityKind, SessionState, Vec3 as ProtoVec3};

use super::aggro::Aggroing;
use super::bridge::SessionStateSnapshot;

/// Marker for the player's own avatar capsule. Exactly one expected.
#[derive(Component)]
pub struct IsSelf;

/// Identifies a Bevy entity as a mirror of a protocol entity. The chase
/// camera and any future systems use this to look up entities by their
/// FFXI id without scanning all transforms. `name` is duplicated from the
/// protocol snapshot so the nametag overlay system doesn't have to
/// re-scan `state.entities` and match by id every frame.
#[derive(Component)]
pub struct WorldEntity {
    pub id: u32,
    pub kind: EntityKind,
    pub name: Option<String>,
}

/// HP bar quad parented to a `WorldEntity`. Width is rescaled per tick
/// from `Entity::hp_pct`; color lerps red↔green by HP fraction.
#[derive(Component)]
pub struct HpBar {
    pub owner_id: u32,
}

/// Currently-targeted FFXI entity id. `None` when no target is selected.
/// Tab cycling lives in `input::handle_input_system`; the scene system
/// reads this to apply visual highlight.
#[derive(Resource, Default)]
pub struct Target {
    pub id: Option<u32>,
}

/// Cached materials so we don't allocate a new StandardMaterial per
/// spawn — a busy zone could pop dozens of entities in/out per second.
#[derive(Resource)]
pub struct EntityPalette {
    pc: Handle<StandardMaterial>,
    npc: Handle<StandardMaterial>,
    mob: Handle<StandardMaterial>,
    pet: Handle<StandardMaterial>,
    other: Handle<StandardMaterial>,
    pub self_: Handle<StandardMaterial>,
    /// Highlight material applied to whichever entity is the current
    /// `Target`. Reverted to `material_for(kind)` when target changes.
    pub target: Handle<StandardMaterial>,
    /// Aggro override applied by `view3d::aggro::sync_aggro_system` to
    /// any mob whose `bt_target_id` points at the player. Bright red
    /// + emissive so it's still legible at halfblock terminal
    /// resolution where the standard mob material can fade into shadow.
    pub aggro: Handle<StandardMaterial>,
    pub capsule_mesh: Handle<Mesh>,
    pub self_mesh: Handle<Mesh>,
    pub hp_bar_mesh: Handle<Mesh>,
}

impl EntityPalette {
    pub fn material_for(&self, kind: EntityKind) -> Handle<StandardMaterial> {
        match kind {
            EntityKind::Pc => self.pc.clone(),
            EntityKind::Npc => self.npc.clone(),
            EntityKind::Mob => self.mob.clone(),
            EntityKind::Pet => self.pet.clone(),
            EntityKind::Other => self.other.clone(),
        }
    }
}

/// Build the palette + the player capsule at the origin. Entities will
/// be spawned/moved by `sync_entities_system` once `SessionState` updates
/// arrive from the wire.
pub fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mk = |c: Color, materials: &mut Assets<StandardMaterial>| {
        materials.add(StandardMaterial {
            base_color: c,
            perceptual_roughness: 0.7,
            metallic: 0.0,
            ..default()
        })
    };
    let palette = EntityPalette {
        // Match the TUI's ratatui colors (tui.rs:310-318) so the two views
        // are visually consistent: cyan PCs, white NPCs, red mobs, yellow pets.
        pc: mk(Color::srgb(0.30, 0.85, 1.00), &mut materials),
        npc: mk(Color::srgb(0.90, 0.90, 0.90), &mut materials),
        mob: mk(Color::srgb(1.00, 0.30, 0.30), &mut materials),
        pet: mk(Color::srgb(1.00, 0.85, 0.20), &mut materials),
        other: mk(Color::srgb(0.50, 0.50, 0.50), &mut materials),
        self_: mk(Color::srgb(0.20, 1.00, 0.80), &mut materials),
        // Bright yellow + emissive so the targeted entity stands out even
        // when other entities are in shadow or behind the chase camera.
        target: materials.add(StandardMaterial {
            base_color: Color::srgb(1.00, 0.95, 0.20),
            emissive: LinearRgba::new(1.0, 0.95, 0.0, 1.0) * 0.6,
            perceptual_roughness: 0.4,
            ..default()
        }),
        // Aggro: saturated red, emissive cranked > 1.0 so the color
        // survives the halfblock cell-averaging that bevy_ratatui_camera
        // performs on readback.
        aggro: materials.add(StandardMaterial {
            base_color: Color::srgb(1.00, 0.10, 0.10),
            emissive: LinearRgba::new(1.5, 0.0, 0.0, 1.0),
            perceptual_roughness: 0.4,
            ..default()
        }),
        capsule_mesh: meshes.add(Capsule3d::new(0.4, 1.0)),
        // The player gets a slightly bigger capsule so they read clearly
        // even at low terminal resolutions where halfblocks coalesce.
        self_mesh: meshes.add(Capsule3d::new(0.5, 1.4)),
        hp_bar_mesh: meshes.add(Cuboid::new(1.0, 0.12, 0.12)),
    };

    commands.spawn((
        IsSelf,
        Mesh3d(palette.self_mesh.clone()),
        MeshMaterial3d(palette.self_.clone()),
        Transform::from_xyz(0.0, 0.7, 0.0),
    ));

    // Light needs to be high and offset so the chase camera sees both
    // shadow- and lit-side faces of the capsules.
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(20.0, 40.0, 20.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.insert_resource(palette);
}

/// FFXI world coords → Bevy world coords.
///
/// FFXI's wire convention (per `state::Position` / minimap math in
/// `tui.rs:486`): +x = east, +y = north (top-down map "up"), +z = elevation.
/// Bevy's default is right-handed Y-up where -Z is "into the screen."
///
/// We map +y_ffxi (north) → -z_bevy and +z_ffxi (height) → +y_bevy. With
/// this mapping a player at heading=0 facing FFXI-north will face
/// Bevy's -Z, which is the camera's natural forward — chase cam Just Works.
#[inline]
pub fn ffxi_to_bevy(p: ProtoVec3) -> Vec3 {
    Vec3::new(p.x, p.z, -p.y)
}

/// FFXI heading (u8, 0=+y/north, CW from above) → Bevy yaw quaternion.
///
/// FFXI rotates clockwise viewed from above; Bevy yaw around +Y is CCW
/// from above (right-handed). Hence the negation. Heading 0 yields
/// identity rotation, so the player capsule's default forward (Bevy -Z)
/// already points "north."
#[inline]
pub fn ffxi_heading_to_bevy_rotation(heading: u8) -> Quat {
    let angle = -(heading as f32) * std::f32::consts::TAU / 256.0;
    Quat::from_rotation_y(angle)
}

/// Diff the snapshot against ECS state and reconcile: spawn new entities,
/// move existing ones, despawn vanished ones; apply target highlight; and
/// scale HP bars to the latest hp_pct. Runs every Update but most frames
/// short-circuit because neither the snapshot nor the target changed.
pub fn sync_entities_system(
    mut commands: Commands,
    snapshot: Res<SessionStateSnapshot>,
    target: Res<Target>,
    palette: Option<Res<EntityPalette>>,
    mut self_q: Query<&mut Transform, (With<IsSelf>, Without<WorldEntity>, Without<HpBar>)>,
    mut entity_q: Query<
        (
            bevy::prelude::Entity,
            &mut WorldEntity,
            &mut Transform,
            &mut MeshMaterial3d<StandardMaterial>,
            Option<&Aggroing>,
        ),
        (Without<IsSelf>, Without<HpBar>),
    >,
    mut hp_q: Query<(&HpBar, &mut Transform), (Without<IsSelf>, Without<WorldEntity>)>,
) {
    let Some(palette) = palette else { return };
    let dirty = snapshot.is_changed() || target.is_changed();
    if !dirty {
        return;
    }
    let state: &SessionState = &snapshot.0;

    // 1) Self transform: position + heading-derived rotation. Encoding
    //    heading on the transform (not as a side resource) is what makes
    //    `chase_camera_system` follow turns by reading `forward()`.
    if let Ok(mut t) = self_q.single_mut() {
        let p = ffxi_to_bevy(state.self_pos.pos);
        t.translation = p + Vec3::Y * 0.7;
        t.rotation = ffxi_heading_to_bevy_rotation(state.self_pos.heading);
    }

    // 2) Index existing ECS entities by FFXI id for the diff. The
    //    `is_aggro` flag is captured at iteration time so we can
    //    decide later in the loop whether to skip the material write
    //    (precedence: aggro > target > kind, owned by sync_aggro_system).
    let mut existing: HashMap<
        u32,
        (
            bevy::prelude::Entity,
            Mut<WorldEntity>,
            Mut<Transform>,
            Mut<MeshMaterial3d<StandardMaterial>>,
            bool,
        ),
    > = HashMap::new();
    for (e, w, t, m, agg) in entity_q.iter_mut() {
        let is_aggro = agg.is_some();
        existing.insert(w.id, (e, w, t, m, is_aggro));
    }

    // 3) Update or spawn each entity in the snapshot. Material updates
    //    happen here so target highlight (re)applies even on frames where
    //    only the target changed and the snapshot didn't.
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut hp_by_id: HashMap<u32, Option<u8>> = HashMap::new();
    for ent in &state.entities {
        seen.insert(ent.id);
        hp_by_id.insert(ent.id, ent.hp_pct);
        let pos = ffxi_to_bevy(ent.pos);
        let translation = pos + Vec3::Y * 0.5;
        let want_mat = if Some(ent.id) == target.id {
            palette.target.clone()
        } else {
            palette.material_for(ent.kind)
        };
        match existing.remove(&ent.id) {
            Some((_, mut w, mut t, mut m, is_aggro)) => {
                t.translation = translation;
                // Skip material write when this entity is aggroing the
                // player — sync_aggro_system owns the material for
                // those, and overwriting here would just race it back
                // to kind/target on every snapshot tick.
                if !is_aggro && m.0 != want_mat {
                    m.0 = want_mat;
                }
                // Names occasionally backfill late (server sends them as
                // separate updates), so refresh the cached label each tick.
                if w.name != ent.name {
                    w.name = ent.name.clone();
                }
            }
            None => {
                spawn_entity(&mut commands, &palette, ent, translation, want_mat);
            }
        }
    }

    // 4) Despawn entities that disappeared from the snapshot. Children
    //    (HP bars) go with them via Bevy's hierarchy semantics.
    for (id, (entity, _, _, _, _)) in existing {
        if !seen.contains(&id) {
            commands.entity(entity).despawn();
        }
    }

    // 5) Resize HP bars from the latest hp_pct. The bar is a child quad
    //    so its transform is already relative to the parent capsule;
    //    scale.x in [0..1] gives the fill fraction.
    for (bar, mut t) in hp_q.iter_mut() {
        if let Some(Some(pct)) = hp_by_id.get(&bar.owner_id).copied() {
            let frac = (pct as f32 / 100.0).clamp(0.0, 1.0);
            t.scale.x = frac;
            // Bar slides slightly so its left edge stays anchored as it shrinks.
            t.translation.x = -(1.0 - frac) * 0.5;
        } else {
            // No HP info → hide the bar.
            t.scale.x = 0.0;
        }
    }
}

fn spawn_entity(
    commands: &mut Commands,
    palette: &EntityPalette,
    ent: &ProtoEntity,
    translation: Vec3,
    material: Handle<StandardMaterial>,
) {
    let parent = commands
        .spawn((
            WorldEntity {
                id: ent.id,
                kind: ent.kind,
                name: ent.name.clone(),
            },
            Mesh3d(palette.capsule_mesh.clone()),
            MeshMaterial3d(material),
            Transform::from_translation(translation),
        ))
        .id();

    // HP bar: a small horizontal cuboid 1.5 units above the capsule.
    // Only meaningful for kinds that have HP (mobs primarily). For PCs
    // and pets we still spawn it so the diff system stays uniform; a
    // None hp_pct just hides the bar.
    if matches!(ent.kind, EntityKind::Mob | EntityKind::Pc | EntityKind::Pet) {
        commands.spawn((
            HpBar { owner_id: ent.id },
            Mesh3d(palette.hp_bar_mesh.clone()),
            MeshMaterial3d(materials_hp_color(palette, ent.hp_pct)),
            Transform::from_xyz(0.0, 1.5, 0.0),
            ChildOf(parent),
        ));
    }
}

/// Pick a HP-bar material color from the bar palette by `hp_pct`. Stage 5
/// keeps it simple: clone the kind-specific palette entry as-is. A nicer
/// version would lerp red↔green per HP fraction; we'd then need a per-bar
/// material handle (one allocation per spawn), so we defer until it's
/// actually visible enough at terminal resolution to be worth the cost.
fn materials_hp_color(palette: &EntityPalette, _hp_pct: Option<u8>) -> Handle<StandardMaterial> {
    palette.mob.clone()
}
