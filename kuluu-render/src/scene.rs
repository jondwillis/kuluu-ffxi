use std::collections::HashMap;

use bevy::ecs::system::SystemParam;
use bevy::light::FogVolume;
use bevy::picking::Pickable;
use bevy::prelude::*;
use kuluu_snapshot::{EntityKind, EntityLook, Vec3 as WireVec3};

use crate::components::{
    CurrRenderPos, IsSelf, LookComp, MorphIn, Nameplate, PrevRenderPos, WorldEntity,
};
use crate::graphics_settings::GraphicsSettings;
use crate::snapshot::SceneState;

#[inline]
pub fn ffxi_to_bevy(p: WireVec3) -> Vec3 {
    Vec3::new(p.x, -p.z, -p.y)
}

#[inline]
pub fn mzb_to_bevy(p: WireVec3) -> Vec3 {
    Vec3::new(p.x, -p.y, -p.z)
}

#[inline]
pub fn entity_visual_height(kind: EntityKind) -> f32 {
    match kind {
        EntityKind::Pc => 2.0 * (0.35 + 1.9),
        EntityKind::Pet => 2.0 * (0.4 + 0.6),

        EntityKind::Mob => 1.1,
        _ => 2.0 * (0.5 + 1.4),
    }
}

#[derive(Component, Clone, Copy, Debug)]
pub struct BakedActor {
    pub min_mesh_y: f32,

    pub actor_height: f32,
}

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

#[derive(Resource)]
pub struct EntityMaterials {
    pub pc: Handle<StandardMaterial>,
    pub self_pc: Handle<StandardMaterial>,
    pub npc: Handle<StandardMaterial>,
    pub mob: Handle<StandardMaterial>,
    pub pet: Handle<StandardMaterial>,
    pub other: Handle<StandardMaterial>,

    pub aggro: Handle<StandardMaterial>,

    pub mob_claimed_self: Handle<StandardMaterial>,

    pub mob_claimed_other: Handle<StandardMaterial>,
}

#[derive(Component)]
pub struct Aggroing;

#[derive(Resource)]
pub struct EntityMesh {
    pub default: Handle<Mesh>,
    pub pc: Handle<Mesh>,
    pub mob: Handle<Mesh>,
    pub pet: Handle<Mesh>,
    pub morph_orb: Handle<Mesh>,
}

#[derive(Resource, Default)]
pub struct Target {
    pub id: Option<u32>,
}

pub fn should_clear_target(id: Option<u32>, entities: &[kuluu_snapshot::Entity]) -> bool {
    let Some(id) = id else {
        return false;
    };
    match entities.iter().find(|e| e.id == id) {
        None => true,
        Some(e) => !e.is_targetable(),
    }
}

pub fn auto_clear_target_system(
    state: Res<SceneState>,
    mut target: ResMut<Target>,
    mut lock_on: ResMut<crate::lock_on::LockOn>,
) {
    let entities = &state.snapshot.entities;
    if should_clear_target(target.id, entities) {
        target.id = None;
    }
    if should_clear_target(lock_on.target_id, entities) {
        lock_on.target_id = None;
    }
}

#[derive(Resource, Default)]
pub struct TrackedEntities {
    pub by_id: HashMap<u32, Entity>,
}

/// A ridden mount is a second actor standing where its rider stands, so it needs
/// an id of its own in the actor pipeline (which is keyed on world id throughout).
/// LSB unique_no tops out at `(4<<28)|(zone<<12)|targid`
/// (vendor/server/src/map/zone_entities.cpp), so bit 31 is free to mark the
/// derived id and the mapping stays reversible.
pub const MOUNT_ACTOR_ID_BIT: u32 = 0x8000_0000;

pub fn mount_actor_id(rider_id: u32) -> u32 {
    rider_id | MOUNT_ACTOR_ID_BIT
}

/// The rider a [`mount_actor_id`] belongs to, or `None` for an ordinary world id.
pub fn mount_actor_rider(world_id: u32) -> Option<u32> {
    (world_id & MOUNT_ACTOR_ID_BIT != 0).then_some(world_id & !MOUNT_ACTOR_ID_BIT)
}

pub fn setup_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut moon_materials: ResMut<Assets<crate::moon_material::MoonMaterial>>,
    mut images: ResMut<Assets<Image>>,
    settings: Res<GraphicsSettings>,
) {
    let orb = |c: Color, glow: f32, m: &mut Assets<StandardMaterial>| {
        let l = c.to_linear();
        m.add(StandardMaterial {
            base_color: c,
            emissive: LinearRgba::new(l.red * glow, l.green * glow, l.blue * glow, 1.0),
            unlit: true,
            ..default()
        })
    };
    commands.insert_resource(EntityMaterials {
        pc: orb(Color::srgb(0.40, 0.85, 1.00), 6.0, &mut materials),
        self_pc: orb(Color::srgb(0.20, 1.00, 1.00), 6.0, &mut materials),
        npc: orb(Color::srgb(0.95, 0.85, 0.30), 6.0, &mut materials),
        mob: orb(Color::srgb(0.95, 0.40, 0.40), 6.0, &mut materials),
        pet: orb(Color::srgb(0.40, 0.85, 0.50), 6.0, &mut materials),
        other: orb(Color::srgb(0.60, 0.60, 0.60), 6.0, &mut materials),
        aggro: orb(Color::srgb(1.00, 0.12, 0.12), 9.0, &mut materials),

        mob_claimed_self: orb(Color::srgb(0.96, 0.96, 0.96), 6.0, &mut materials),

        mob_claimed_other: orb(Color::srgb(0.80, 0.18, 0.18), 7.0, &mut materials),
    });

    let orb_mesh = |radius: f32, center_y: f32, m: &mut Assets<Mesh>| {
        m.add(
            Sphere::new(radius)
                .mesh()
                .build()
                .translated_by(Vec3::Y * center_y),
        )
    };
    commands.insert_resource(EntityMesh {
        default: orb_mesh(0.28, 1.05, &mut meshes),
        pc: orb_mesh(0.28, 1.05, &mut meshes),
        mob: orb_mesh(0.36, 0.85, &mut meshes),
        pet: orb_mesh(0.22, 0.62, &mut meshes),
        morph_orb: meshes.add(Sphere::new(0.22).mesh().build()),
    });

    commands.insert_resource(crate::picking::HitboxAssets::new(
        &mut meshes,
        &mut materials,
    ));

    crate::sun_moon::spawn_sun_and_moon(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut moon_materials,
        &settings,
    );

    commands.spawn((
        crate::components::InGameEntity,
        FogVolume {
            fog_color: Color::srgb(0.65, 0.72, 0.82),
            density_factor: 0.06,
            absorption: 0.25,
            scattering: 0.35,

            scattering_asymmetry: 0.7,
            light_tint: Color::srgb(1.0, 0.96, 0.88),
            light_intensity: 1.0,
            // Vertical falloff: ground haze that clears overhead so the sky
            // stays visible (FFXI never fogs the sky dome). See weather.rs.
            density_texture: Some(crate::weather::height_fog_density_texture(&mut images)),
            ..default()
        },
        Transform::from_xyz(0.0, crate::weather::FOG_VOLUME_CENTER_Y, 0.0)
            .with_scale(crate::weather::FOG_VOLUME_SCALE),
    ));

    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.85, 0.88, 1.0),
        brightness: 500.0,
        ..default()
    });
}

/// Bundled so [`sync_entities_system`] stays under bevy's 16-param SystemParam
/// ceiling (it was already at it before the floor gate landed).
#[derive(SystemParam)]
pub struct EntitySyncQueries<'w, 's> {
    xform: Query<'w, 's, &'static mut Transform, With<WorldEntity>>,
    mat: Query<
        'w,
        's,
        &'static mut MeshMaterial3d<StandardMaterial>,
        (With<WorldEntity>, Without<MorphIn>),
    >,
}

/// The two signals that say "this zone's floor has landed" — the same pair the
/// loading overlay's `ready` reads. Bundled for the 16-param ceiling.
#[derive(SystemParam)]
pub struct ZoneFloorGate<'w> {
    last_auto: Res<'w, crate::dat_mzb::LastAutoLoadedZone>,
    in_flight: Res<'w, crate::dat_mzb::LoadMzbInFlight>,
}

pub fn sync_entities_system(
    state: Res<SceneState>,
    mesh: Res<EntityMesh>,
    mats: Res<EntityMaterials>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    billboard_font: Res<crate::nameplate_billboard::BillboardFont>,
    mut tracked: ResMut<TrackedEntities>,
    mut prediction: ResMut<crate::combat_stance::EntityPrediction>,
    mut motion: ResMut<crate::combat_stance::EntityMotion>,
    mut blends: ResMut<crate::combat_stance::AnimationBlends>,
    mut commands: Commands,
    mut queries: EntitySyncQueries,
    q_nameplates: Query<&Nameplate>,
    floor_gate: ZoneFloorGate,
    mut prev_zone: Local<Option<Option<u32>>>,
) {
    if !state.dirty {
        return;
    }

    let snap = &state.snapshot;

    // Hard load-order gate: no NPC/character visuals may exist before this
    // zone's floor has landed. The main-zone MZB streams in asynchronously
    // AFTER the first InZone snapshot, and an actor spawned ahead of it has
    // nothing to ground against — it falls at walker terminal velocity while
    // under-floor recovery is deliberately inert mid-load (and the server
    // echoes back whatever c2s 0x015 reports, so that fall sticks). The
    // overlay's `ready` reads this same pair of signals, so the gate opens
    // exactly when the loading screen lifts. Existing entities keep updating
    // and stale ones still despawn below; only NEW visuals wait.
    let floor_ready =
        crate::dat_mzb::main_zone_floor_ready(snap, &floor_gate.last_auto, &floor_gate.in_flight);

    // Keyed on the resolved DAT file id, not zone_id: Mog House entry/exit keeps
    // the city zone_id but teleports the player into a different interior.
    let zone_key = crate::snapshot::effective_zone_file_id(snap);
    let zone_changed = matches!(*prev_zone, Some(p) if p != zone_key);
    *prev_zone = Some(zone_key);

    let mut nameplated: std::collections::HashSet<u32> =
        q_nameplates.iter().map(|n| n.entity_id).collect();

    let mut seen: std::collections::HashSet<u32> =
        std::collections::HashSet::with_capacity(snap.entities.len() + 1);
    let mut hp_by_id: HashMap<u32, Option<u8>> = HashMap::new();

    let self_char_id = snap.self_char_id.unwrap_or(0);
    for wire in &snap.entities {
        seen.insert(wire.id);
        hp_by_id.insert(wire.id, wire.hp_pct);
        let world_pos = ffxi_to_bevy(wire.pos);
        let is_self = self_char_id != 0 && wire.id == self_char_id;

        if !is_self
            && matches!(
                wire.kind,
                EntityKind::Mob | EntityKind::Pc | EntityKind::Pet | EntityKind::Npc
            )
        {
            prediction.observe(wire.id, world_pos, wire.heading);
        }

        let mat = if is_self {
            mats.self_pc.clone()
        } else {
            match wire.kind {
                EntityKind::Mob => {
                    pick_mob_material(&mats, wire.claim_id, self_char_id, false).clone()
                }
                _ => pick_material(&mats, wire.kind, false),
            }
        };

        match tracked.by_id.get(&wire.id).copied() {
            Some(existing) => {
                if let Ok(mut t) = queries.xform.get_mut(existing) {
                    if is_self {
                        trace!(
                            target: "self_sync",
                            echo_x = world_pos.x,
                            echo_z = world_pos.z,
                            cur_x = t.translation.x,
                            cur_z = t.translation.z,
                            "self ingest"
                        );
                        // Self Transform is owned by apply_self_prediction_system
                        // (FixedUpdate, driven by LocalPlayerPrediction). The
                        // server just echoes our c2s 0x015, so ingesting the echo
                        // here would fight the walker and re-introduce the
                        // horizontal jiggle the smoothing was meant to hide, and
                        // stall self.y on stairs (held from prev frame).
                        //
                        // Zone change is the one case where the wire is
                        // authoritative: the walker resyncs from snapshot on
                        // init or big deltas (PREDICTION_RESYNC_YALMS), but
                        // seeding the Transform here avoids a one-frame flicker
                        // at the old zone's coords before the next FixedUpdate
                        // tick lands. Rotation is owned by
                        // self_visual_yaw_system.
                        if zone_changed {
                            t.translation = world_pos;
                        }
                    } else if matches!(wire.kind, EntityKind::Other) {
                        // Doors/transports and other non-actor entities keep the
                        // simple visual lerp; pathed NPCs are dead-reckoned by
                        // predict_entities_system alongside mobs/PCs/pets so the
                        // two systems never fight over the same Transform.
                        let smoothed = apply_visual_smoothing(t.translation, world_pos);
                        t.translation = Vec3::new(smoothed.x, t.translation.y, smoothed.z);
                        t.rotation = heading_to_quat(wire.heading);
                    }
                }
                if let Ok(mut m) = queries.mat.get_mut(existing) {
                    m.0 = mat;
                }
                // The spawn arm can only tag self once the id is known, and the
                // player's own entity routinely arrives before it — every reader
                // of this marker (camera, first-person, the self plate) would
                // then treat the player as somebody else for the whole session.
                if is_self {
                    commands.entity(existing).insert(IsSelf);
                }
            }
            None => {
                if !floor_ready {
                    continue;
                }
                // Doors/transports have no client model — their visual is the
                // zone/MMB geometry — so the placeholder orb would render as a
                // floating sphere over them (kuluu-nf56). Suppress the orb mesh
                // but keep the entity (and its Visibility node): it stays
                // mouse-pickable via the transparent EntityHitbox child spawned
                // in picking.rs.
                let suppress_orb = matches!(
                    wire.look,
                    Some(EntityLook::Door { .. } | EntityLook::Transport { .. })
                );
                let mut spawn = commands.spawn((
                    crate::components::InGameEntity,
                    WorldEntity {
                        id: wire.id,
                        act_index: wire.act_index,
                        kind: wire.kind,
                    },
                    Pickable::default(),
                    Transform {
                        translation: world_pos,
                        rotation: heading_to_quat(wire.heading),
                        ..default()
                    },
                    Visibility::default(),
                ));
                if !suppress_orb {
                    spawn.insert((Mesh3d(pick_mesh(&mesh, wire.kind)), MeshMaterial3d(mat)));
                }
                if is_self {
                    spawn.insert(IsSelf);
                }
                let bevy_e = spawn.id();
                tracked.by_id.insert(wire.id, bevy_e);

                if !is_self
                    && matches!(
                        wire.kind,
                        EntityKind::Mob | EntityKind::Pc | EntityKind::Pet
                    )
                {
                    let _ = bevy_e;
                }
            }
        }

        // Retail draws the local player's own overhead name in the same PC
        // styling as other PCs (kuluu-hof); the update system hides it in
        // first-person mode where the plate would sit at the camera eye.
        if let Some(name) = wire.name.as_deref().filter(|s| !s.is_empty()) {
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
                    crate::nameplate_billboard::NAMEPLATE_FALLBACK_COLOR,
                );
                nameplated.insert(wire.id);
            }
        }
    }

    // A mount is a second actor standing exactly where its rider stands; the
    // rider's own body is then lifted onto the saddle joint by the actor pose
    // pass. Spawned bare — no orb, no nameplate, not Pickable — because retail
    // gives a mount none of those; it is scenery bolted to the rider. Its
    // transform is pinned later, by `pin_mount_actors_system`.
    for wire in &snap.entities {
        let Some(&rider_e) = tracked.by_id.get(&wire.id) else {
            continue;
        };
        if snap.mount_of(wire).is_none() {
            commands
                .entity(rider_e)
                .remove::<crate::components::MountedRider>();
            continue;
        }
        commands
            .entity(rider_e)
            .insert(crate::components::MountedRider);
        let id = mount_actor_id(wire.id);
        seen.insert(id);
        if tracked.by_id.contains_key(&id) {
            continue;
        }
        let rider_tf = queries.xform.get(rider_e).copied().unwrap_or_default();
        let bevy_e = commands
            .spawn((
                crate::components::InGameEntity,
                WorldEntity {
                    id,
                    act_index: wire.act_index,
                    kind: wire.kind,
                },
                rider_tf,
                Visibility::default(),
            ))
            .id();
        tracked.by_id.insert(id, bevy_e);
    }

    let stale: Vec<u32> = tracked
        .by_id
        .keys()
        .copied()
        .filter(|id| !seen.contains(id))
        .collect();
    for id in stale {
        if let Some(bevy_e) = tracked.by_id.remove(&id) {
            commands.entity(bevy_e).try_despawn();
        }

        prediction.by_id.remove(&id);
        motion.by_id.remove(&id);
        blends.by_id.remove(&id);
    }
}

/// Holds each mount actor on its rider. Deliberately not part of
/// `sync_entities_system`: the rider's transform is still being written after
/// that runs (dead reckoning), and the floor snap that follows must see both
/// actors already agreeing, or the mount grounds against a stale position.
pub fn pin_mount_actors_system(
    tracked: Res<TrackedEntities>,
    mut q_xform: Query<&mut Transform, With<WorldEntity>>,
) {
    for (&id, &bevy_e) in tracked.by_id.iter() {
        let Some(rider_id) = mount_actor_rider(id) else {
            continue;
        };
        let Some(&rider_e) = tracked.by_id.get(&rider_id) else {
            continue;
        };
        let Ok(rider_tf) = q_xform.get(rider_e).copied() else {
            continue;
        };
        if let Ok(mut t) = q_xform.get_mut(bevy_e) {
            *t = rider_tf;
        }
    }
}

pub fn sync_aggro_system(
    mut commands: Commands,
    state: Res<SceneState>,
    mats: Res<EntityMaterials>,

    self_q: Query<&Transform, With<IsSelf>>,
    mut q: Query<
        (
            Entity,
            Ref<WorldEntity>,
            &mut Transform,
            &mut MeshMaterial3d<StandardMaterial>,
            Option<&Aggroing>,
        ),
        (Without<IsSelf>, Without<MorphIn>),
    >,
    mut gizmos: Gizmos,
) {
    let snap = &state.snapshot;
    let self_id = snap.diagnostics.sync_in;
    let Some(self_uid) = self_id else { return };

    let self_char_id = snap.self_char_id.unwrap_or(0);

    // Aggro state derives from the snapshot, so the map rebuild + material
    // reconciliation only run on snapshot frames; the gizmo line still draws
    // every frame from the Aggroing marker set by the last reconciliation.
    let dirty = state.dirty;

    let mut claim_by_id: HashMap<u32, u32> = HashMap::new();

    let mut aggroing: HashMap<u32, bool> = HashMap::new();
    if dirty {
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
    }

    let self_pos = self_q.single().ok().map(|t| t.translation);

    for (e, w, t, mut m, has_aggro) in q.iter_mut() {
        let has_marker = has_aggro.is_some();
        let should_aggro = if dirty {
            aggroing.get(&w.id).copied().unwrap_or(false)
        } else {
            has_marker
        };
        if dirty {
            match (should_aggro, has_marker) {
                (true, false) => {
                    commands.entity(e).try_insert(Aggroing);
                    m.0 = mats.aggro.clone();
                }
                (true, true) => {
                    if m.0 != mats.aggro {
                        m.0 = mats.aggro.clone();
                    }
                }
                (false, true) => {
                    commands.entity(e).remove::<Aggroing>();

                    let restore = if matches!(w.kind, EntityKind::Mob) {
                        let claim = claim_by_id.get(&w.id).copied().unwrap_or(0);
                        pick_mob_material(&mats, claim, self_char_id, false).clone()
                    } else {
                        pick_material(&mats, w.kind, false)
                    };
                    m.0 = restore;
                }
                (false, false) => {}
            }
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

pub fn pick_mob_material(
    mats: &EntityMaterials,
    claim_id: u32,
    self_id: u32,
    is_aggro: bool,
) -> &Handle<StandardMaterial> {
    if is_aggro {
        return &mats.aggro;
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

pub(crate) fn heading_to_quat(heading: u8) -> Quat {
    let angle = (heading as f32) * std::f32::consts::TAU / 256.0;
    Quat::from_rotation_y(-angle)
}

/// Retail snaps the *movement* heading instantly (about-face, first step from
/// standstill), but the rendered body whips around rather than teleporting.
/// This slerps only the self model's visible yaw toward the logical heading;
/// travel direction is unaffected. Rate is tuned so a 180° flip completes in
/// roughly a quarter second.
const SELF_VISUAL_YAW_RATE: f32 = 14.0;

pub fn self_visual_yaw_system(
    time: Res<Time>,
    state: Res<SceneState>,
    mut q_self: Query<&mut Transform, With<IsSelf>>,
) {
    let Ok(mut t) = q_self.single_mut() else {
        return;
    };
    let target = heading_to_quat(state.snapshot.self_pos.heading);
    let alpha = 1.0 - (-SELF_VISUAL_YAW_RATE * time.delta_secs()).exp();
    t.rotation = t.rotation.slerp(target, alpha);
}

/// Attaches [`PrevRenderPos`] and [`CurrRenderPos`] to the local player entity
/// on the frame it becomes IsSelf, seeded from its current Transform so the
/// very first interpolation lerps between two identical points (no origin
/// warp). Runs every frame; the query filter makes it a no-op once the
/// components exist. Mirrors [`ensure_self_lookcomp_system`].
pub fn ensure_self_render_pos_system(
    q: Query<(Entity, &Transform), (With<IsSelf>, Without<CurrRenderPos>)>,
    mut commands: Commands,
) {
    for (e, t) in &q {
        commands
            .entity(e)
            .insert((PrevRenderPos(t.translation), CurrRenderPos(t.translation)));
    }
}

/// Runs every rendered frame in `RunFixedMainLoopSystems::AfterFixedMainLoop`.
/// Lerps the visual Transform between the last two authoritative render
/// positions (produced by `apply_self_prediction_system` at 60Hz) using the
/// fixed-timestep overstep fraction. This decouples the visible character
/// motion from the fixed-tick cadence so the chase camera, which reads
/// Transform every render frame, no longer sees stair-step Y jitter as the
/// display frame rate races ahead of FixedUpdate.
pub fn interpolate_self_transform_system(
    fixed_time: Res<Time<Fixed>>,
    mut q: Query<(&mut Transform, &PrevRenderPos, &CurrRenderPos), With<IsSelf>>,
) {
    let Ok((mut t, prev, curr)) = q.single_mut() else {
        return;
    };
    let alpha = fixed_time.overstep_fraction();
    t.translation = prev.0.lerp(curr.0, alpha);
}

#[derive(Resource, Default, Debug, Clone)]
pub struct SelfAppearance {
    pub look: Option<kuluu_snapshot::EntityLook>,
}

pub fn ensure_self_lookcomp_system(
    appearance: Res<SelfAppearance>,
    q_self: Query<(Entity, Option<&LookComp>), With<IsSelf>>,
    mut commands: Commands,
) {
    let Some(look) = appearance.look.as_ref() else {
        return;
    };
    // Seed the self look from the launcher-time appearance ONLY when nothing has
    // set it yet, so the model shows before the server's 0x00A LOGIN / 0x051
    // GRAP_LIST appearance lands. Once a LookComp exists, sync_entity_looks_system
    // (server-driven) owns it — otherwise this would clobber the server look every
    // frame and the self model would never reflect gear changes.
    for (e, current) in q_self.iter() {
        if current.is_none() {
            if let Ok(mut ec) = commands.get_entity(e) {
                ec.insert(LookComp(*look));
            }
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
        // A look the server stops reporting is not a look that changed: clearing
        // the component on None fought ensure_self_lookcomp_system re-seeding it,
        // so dispatch_look_driven_models -- scheduled between the two -- never saw
        // self hold a LookComp and never requested the model.
        match (&wire.look, current) {
            (Some(new), Some(LookComp(old))) if new == old => {}
            (Some(new), _) => {
                commands.entity(bevy_e).try_insert(LookComp(*new));
            }
            (None, _) => {}
        }
    }
}

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

    #[test]
    fn mount_actor_id_is_reversible_and_disjoint_from_server_ids() {
        // Largest unique_no LSB can build: (4<<28) | (zone<<12) | targid.
        let max_server_id = (4u32 << 28) | (0xFFFF << 12) | 0xFFF;
        assert_eq!(max_server_id & MOUNT_ACTOR_ID_BIT, 0);
        assert_eq!(mount_actor_rider(max_server_id), None);

        for rider in [0x0100_0001u32, 0x1700_0123, max_server_id] {
            let mount = mount_actor_id(rider);
            assert_ne!(mount, rider);
            assert_eq!(mount_actor_rider(mount), Some(rider));
        }
    }

    #[test]
    fn visual_smoothing_lerps_short_then_snaps_long() {
        let near = apply_visual_smoothing(Vec3::ZERO, Vec3::new(0.25, 0.0, 0.0));
        assert!(near.x > 0.0 && near.x < 0.25, "lerp partial: {}", near.x);
        assert!(
            (near.x - 0.1).abs() < 1e-6,
            "VISUAL_SMOOTH=0.4 → 0.25 * 0.4 = 0.1, got {}",
            near.x
        );

        let far = apply_visual_smoothing(Vec3::ZERO, Vec3::new(50.0, 0.0, 0.0));
        assert_eq!(far, Vec3::new(50.0, 0.0, 0.0));
    }

    fn dummy_materials() -> EntityMaterials {
        EntityMaterials {
            pc: Handle::default(),
            self_pc: Handle::default(),
            npc: Handle::default(),
            mob: Handle::default(),
            pet: Handle::default(),
            other: Handle::default(),
            aggro: Handle::default(),
            mob_claimed_self: Handle::default(),
            mob_claimed_other: Handle::default(),
        }
    }

    #[test]
    fn pick_mob_material_unclaimed_uses_default_mob() {
        let mats = dummy_materials();
        let h = pick_mob_material(&mats, 0, 0xCAFE, false);
        assert!(std::ptr::eq(h, &mats.mob), "unclaimed mob → mats.mob");
    }

    #[test]
    fn pick_mob_material_self_claim_uses_white() {
        let mats = dummy_materials();
        let h = pick_mob_material(&mats, 0xCAFE, 0xCAFE, false);
        assert!(std::ptr::eq(h, &mats.mob_claimed_self));
    }

    #[test]
    fn pick_mob_material_other_claim_uses_muted_red() {
        let mats = dummy_materials();
        let h = pick_mob_material(&mats, 0x4242, 0xCAFE, false);
        assert!(
            std::ptr::eq(h, &mats.mob_claimed_other),
            "other player's claim"
        );
        let h_unknown_self = pick_mob_material(&mats, 0x4242, 0, false);
        assert!(
            std::ptr::eq(h_unknown_self, &mats.mob_claimed_other),
            "unknown self_id falls through to other-claim",
        );
    }

    #[test]
    fn pick_mob_material_aggro_overrides_claim() {
        let mats = dummy_materials();
        let h_self = pick_mob_material(&mats, 0xCAFE, 0xCAFE, true);
        assert!(std::ptr::eq(h_self, &mats.aggro), "aggro > self-claim");
        let h_other = pick_mob_material(&mats, 0x4242, 0xCAFE, true);
        assert!(std::ptr::eq(h_other, &mats.aggro), "aggro > other-claim");
        let h_unclaimed = pick_mob_material(&mats, 0, 0xCAFE, true);
        assert!(std::ptr::eq(h_unclaimed, &mats.aggro), "aggro > unclaimed");
    }

    #[test]
    fn visual_smoothing_snap_threshold_boundary() {
        let just_under = (SNAP_DIST_SQ - 1e-3).sqrt();
        let result = apply_visual_smoothing(Vec3::ZERO, Vec3::new(just_under, 0.0, 0.0));

        assert!(
            result.x < just_under,
            "below threshold should lerp, got {}",
            result.x
        );

        let at_threshold = SNAP_DIST_SQ.sqrt();
        let result = apply_visual_smoothing(Vec3::ZERO, Vec3::new(at_threshold, 0.0, 0.0));
        assert_eq!(result.x, at_threshold, "at threshold should snap");
    }

    #[test]
    fn auto_clear_keeps_none() {
        assert!(!should_clear_target(None, &[]));
    }

    #[test]
    fn auto_clear_keeps_live_entity() {
        let ents = vec![entity_with_hp(17, Some(75))];
        assert!(!should_clear_target(Some(17), &ents));
    }

    #[test]
    fn auto_clear_drops_when_id_absent() {
        let ents = vec![entity_with_hp(99, Some(50))];
        assert!(should_clear_target(Some(17), &ents));
    }

    #[test]
    fn auto_clear_drops_when_hp_zero() {
        let ents = vec![entity_with_hp(17, Some(0))];
        assert!(should_clear_target(Some(17), &ents));
    }

    #[test]
    fn auto_clear_keeps_when_hp_unknown() {
        let ents = vec![entity_with_hp(17, None)];
        assert!(!should_clear_target(Some(17), &ents));
    }

    #[test]
    fn auto_clear_drops_other_kind() {
        let mut e = entity_with_hp(17, Some(75));
        e.kind = EntityKind::Other;
        assert!(should_clear_target(Some(17), &[e]));
    }

    #[test]
    fn auto_clear_keeps_dead_pc_for_raise() {
        let mut e = entity_with_hp(17, Some(0));
        e.kind = EntityKind::Pc;
        assert!(!should_clear_target(Some(17), &[e]));
    }

    #[test]
    fn auto_clear_drops_hidden_status_mob() {
        let mut e = entity_with_hp(17, Some(75));
        e.status = 2;
        assert!(should_clear_target(Some(17), &[e]));
    }

    // kuluu-2sqm: self's snapshot entity reports look = None on pos-only
    // updates. Clearing LookComp there raced ensure_self_lookcomp_system's
    // re-seed, and dispatch_look_driven_models -- which only acts on entities
    // holding a LookComp -- never requested the model, so the player rendered
    // as the bare placeholder orb for the whole session.
    #[test]
    fn look_going_none_keeps_the_last_known_lookcomp() {
        let mut app = App::new();
        app.init_resource::<SceneState>()
            .init_resource::<TrackedEntities>()
            .add_systems(Update, sync_entity_looks_system);

        let look = EntityLook::Standard { modelid: 42 };
        let bevy_e = app.world_mut().spawn(LookComp(look)).id();
        app.world_mut()
            .resource_mut::<TrackedEntities>()
            .by_id
            .insert(7, bevy_e);

        let mut wire = entity_with_hp(7, None);
        wire.look = None;
        {
            let mut state = app.world_mut().resource_mut::<SceneState>();
            state.snapshot.entities = vec![wire];
            state.dirty = true;
        }
        app.update();

        assert_eq!(
            app.world().get::<LookComp>(bevy_e).map(|l| l.0),
            Some(look),
            "a look the server stopped reporting must not clear the component"
        );
    }

    fn entity_with_hp(id: u32, hp_pct: Option<u8>) -> kuluu_snapshot::Entity {
        kuluu_snapshot::Entity {
            id,
            act_index: 0,
            kind: EntityKind::Mob,
            name: None,
            pos: WireVec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            heading: 0,
            hp_pct,
            bt_target_id: 0,
            face_target: 0,
            name_vis: None,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            mount: None,
            status: 0,
            char_flags: Default::default(),
        }
    }
}
