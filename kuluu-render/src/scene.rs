use std::collections::HashMap;

use bevy::ecs::system::SystemParam;
use bevy::light::FogVolume;
use bevy::picking::Pickable;
use bevy::prelude::*;
use kuluu_snapshot::{EntityKind, EntityLook, Vec3 as WireVec3};

use crate::components::{CurrRenderPos, IsSelf, LookComp, MorphIn, PrevRenderPos, WorldEntity};
use crate::entity_table::EntityTable;
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

    /// The skeleton's authored height span (`ffxi_dat::skel::Skeleton::height_span`), when the actor was
    /// built from one that carries it.
    pub skeleton_span: Option<f32>,
}

#[derive(Component, Clone, Copy, Debug)]
pub struct NameplateLocator {
    pub offset: Option<Vec3>,
    pub root_attached: bool,
    pub model_scale: f32,
}

impl NameplateLocator {
    pub fn from_skeleton(skeleton: &ffxi_dat::skel::Skeleton, model_scale: f32) -> Self {
        let offset = ffxi_actor::skeleton_instance::nameplate_locator_offset(
            skeleton,
            Vec3::splat(model_scale),
        )
        .map(|p| Vec3::new(p.x, -p.y, -p.z));
        Self {
            offset,
            root_attached: skeleton
                .reference_at(ffxi_dat::skel::standard_position::ABOVE_HEAD)
                .is_some_and(|r| r.index == 0),
            model_scale,
        }
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

    /// Fully transparent stand-in for a placeholder orb whose owner must not be
    /// drawn (Flags1.InvisFlag). Shared, so blanking an orb is a handle swap,
    /// not a per-entity asset.
    pub invis_orb: Handle<StandardMaterial>,
}

#[derive(Component)]
pub struct Aggroing;

#[derive(Resource)]
pub struct EntityMesh {
    pub default: Handle<Mesh>,
    pub pc: Handle<Mesh>,
    pub mob: Handle<Mesh>,
    pub pet: Handle<Mesh>,
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

/// Snapshot frames a server retarget waits for its entity to appear. The 0x058
/// (0x058_assist.cpp) reaches the viewer on the event channel while the entity
/// reaches it in the snapshot, so a retarget onto a mob spawning into view can
/// beat its own entity by a frame; expiring keeps a retarget whose entity does
/// not arrive from firing much later.
const RETARGET_ENTITY_WAIT_FRAMES: u8 = 3;

#[derive(Clone, Copy)]
pub struct PendingRetarget {
    id: u32,
    frames_left: u8,
}

/// Applies the server's retarget push (s2c 0x058 ASSIST, reaching us as
/// [`kuluu_snapshot::ViewerEvent::TargetChanged`]). LSB sends it whenever it
/// moves our battle target - `/assist`, engaging, and the
/// auto-target-after-kill scan in `CAttackState::UpdateTarget` - so it is the
/// only channel that can move the target cursor for those.
///
/// A `None` target_id (wire `AssistNo` 0, which LSB emits from
/// `CCharEntity::OnChangeTarget` when the target went away) is left alone:
/// [`auto_clear_target_system`] already drops a target whose entity is gone,
/// and retail's `RecvAssist` behaviour for a zero id is not established.
///
/// A held lock rides along with the target rather than pinning it: xim keeps
/// the lock as a flag over the one target slot (research/xim/src/jsMain/kotlin/
/// xim/poc/game/PlayerTargetSelector.kt isTargetLocked reads
/// `state.targetState.locked` beside `state.targetState.targetId`), so only
/// player targeting input is gated on it
/// ([`crate::lock_on::suppresses_retarget`]) and a server-side target change
/// carries the lock with it.
///
/// The `Target` write deliberately reaches `dispatch_target_change_system` and
/// goes back out as c2s 0x01A ChangeTarget: `battleutils::assistTarget` pushes
/// 0x058 without touching `m_battleTarget`
/// (vendor/server/src/map/utils/battleutils.cpp assistTarget), so that echo is what
/// actually moves the server's battle target for `/assist`
/// (vendor/server/src/map/ai/ai_container.cpp CAIContainer::Internal_ChangeTarget `SetBattleTargetID`).
pub fn apply_server_retarget_system(
    events: Res<crate::snapshot::EventLog>,
    state: Res<SceneState>,
    mut cursor: Local<u64>,
    mut pending: Local<Option<PendingRetarget>>,
    mut target: ResMut<Target>,
    mut lock_on: ResMut<crate::lock_on::LockOn>,
) {
    let total = events.pushed_total;
    let first_global = total.saturating_sub(events.recent.len() as u64);
    for g in (*cursor).max(first_global)..total {
        if let kuluu_snapshot::ViewerEvent::TargetChanged {
            target_id: Some(id),
        } = events.recent[(g - first_global) as usize]
        {
            *pending = Some(PendingRetarget {
                id,
                frames_left: RETARGET_ENTITY_WAIT_FRAMES,
            });
        }
    }
    *cursor = total;

    let Some(p) = *pending else {
        return;
    };

    let landed = state
        .snapshot
        .entities
        .iter()
        .any(|e| e.id == p.id && e.is_targetable());
    if !landed {
        if state.dirty {
            let frames_left = p.frames_left.saturating_sub(1);
            *pending = (frames_left > 0).then_some(PendingRetarget { frames_left, ..p });
        }
        return;
    }

    *pending = None;
    if target.id != Some(p.id) {
        target.id = Some(p.id);
    }
    if lock_on.target_id.is_some() && lock_on.target_id != Some(p.id) {
        lock_on.target_id = Some(p.id);
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

/// Actors the client makes for placed Mog House furniture (NPC-look pieces and mannequins), one id per seat:
/// above every LSB unique_no and clear of [`MOUNT_ACTOR_ID_BIT`]. The snapshot never carries them, so the
/// entity sync leaves them to the furniture module.
pub const FURNITURE_ACTOR_IDS: std::ops::Range<u32> = 0x6000_0000..0x6001_0000;

pub fn furniture_actor_id(seat: usize) -> u32 {
    FURNITURE_ACTOR_IDS.start + seat as u32
}

pub fn is_furniture_actor_id(world_id: u32) -> bool {
    FURNITURE_ACTOR_IDS.contains(&world_id)
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

        invis_orb: {
            let mut m = StandardMaterial {
                unlit: true,
                ..default()
            };
            m.base_color = Color::srgba(0.0, 0.0, 0.0, 0.0);
            m.alpha_mode = AlphaMode::Blend;
            materials.add(m)
        },
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
    vis: Query<'w, 's, &'static mut Visibility, (With<WorldEntity>, Without<IsSelf>)>,
    self_tagged: Query<'w, 's, Has<IsSelf>, With<WorldEntity>>,
    mounted: Query<'w, 's, Has<crate::components::MountedRider>, With<WorldEntity>>,
    departing: Query<'w, 's, &'static crate::components::MountDeparting>,
}

#[derive(SystemParam)]
pub struct ZoneFloorGate<'w> {
    snapshot: Res<'w, SceneState>,
    #[cfg(not(target_arch = "wasm32"))]
    last_auto: Res<'w, crate::dat_mzb::LastAutoLoadedZone>,
    #[cfg(not(target_arch = "wasm32"))]
    in_flight: Res<'w, crate::dat_mzb::LoadMzbInFlight>,
}

impl ZoneFloorGate<'_> {
    pub fn ready(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return crate::dat_mzb::main_zone_floor_ready(
            &self.snapshot.snapshot,
            &self.last_auto,
            &self.in_flight,
        );
        #[cfg(target_arch = "wasm32")]
        true
    }

    fn changed(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self.last_auto.is_changed() || self.in_flight.is_changed();
        #[cfg(target_arch = "wasm32")]
        self.snapshot.is_changed()
    }
}

/// Spawns, moves and re-skins the wire entities from the snapshot. Self's
/// position is owned by the native fixed-tick prediction, so the wire position
/// is applied only on a zone change or on the relay viewer, which consumes
/// snapshots. The Other kind is not server-moved — doors are static (their
/// open/close is client-side MMB/DAT rotation) and transports play their
/// authored FollowPoints path in pose_transports, which owns the whole
/// transform — so the wire position applies directly, with Y left to its own
/// owner. Nameplate billboards are spawned by
/// update_nameplate_billboards_system's ensure pass, not here.
pub fn sync_entities_system(
    state: Res<SceneState>,
    table: Res<EntityTable>,
    mesh: Res<EntityMesh>,
    mats: Res<EntityMaterials>,
    mut tracked: ResMut<TrackedEntities>,
    mut prediction: ResMut<crate::combat_stance::EntityPrediction>,
    mut motion: ResMut<crate::combat_stance::EntityMotion>,
    mut blends: ResMut<crate::combat_stance::AnimationBlends>,
    mut commands: Commands,
    mut queries: EntitySyncQueries,
    floor_gate: ZoneFloorGate,
    mut prev_zone: Local<Option<Option<u32>>>,
) {
    if !state.dirty && !floor_gate.changed() {
        return;
    }

    let snap = &state.snapshot;

    let floor_ready = floor_gate.ready();

    // Keyed on the resolved DAT file id, not zone_id: Mog House entry/exit keeps
    // the city zone_id but teleports the player into a different interior.
    let zone_key = crate::snapshot::effective_zone_file_id(snap);
    let zone_changed = matches!(*prev_zone, Some(p) if p != zone_key);
    *prev_zone = Some(zone_key);

    let mut seen: std::collections::HashSet<u32> =
        std::collections::HashSet::with_capacity(snap.entities.len() + 1);
    let mut hp_by_id: HashMap<u32, Option<u8>> = HashMap::new();

    let self_char_id = table.self_id().unwrap_or(0);
    for wire in &snap.entities {
        seen.insert(wire.id);
        hp_by_id.insert(wire.id, wire.hp_pct);
        let world_pos = ffxi_to_bevy(wire.pos);
        let is_self = table.is_self(wire.id);

        if !is_self
            && matches!(
                wire.kind,
                EntityKind::Mob | EntityKind::Pc | EntityKind::Pet | EntityKind::Npc
            )
        {
            let mover = if wire.kind == EntityKind::Pc {
                crate::combat_stance::Mover::Client
            } else {
                crate::combat_stance::Mover::Server
            };
            prediction.observe_moved_by(
                wire.id,
                world_pos,
                wire.heading,
                wire.speed,
                wire.speed_base,
                mover,
            );
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
                        if zone_changed || cfg!(target_arch = "wasm32") {
                            t.translation = world_pos;
                        }
                    } else if matches!(wire.kind, EntityKind::Other) {
                        t.translation = Vec3::new(world_pos.x, t.translation.y, world_pos.z);
                        t.rotation = heading_to_quat(wire.heading);
                    }
                }
                if let Ok(mut m) = queries.mat.get_mut(existing) {
                    if m.0 != mat {
                        m.0 = mat;
                    }
                }
                // LSB STATUS_TYPE::INVISIBLE hides the model entirely (worms
                // between dive and surface); hiding the root takes the skinned
                // model child and the placeholder orb with it. The nameplate is
                // culled on the same signals by update_nameplate_billboards_system.
                // Self is exempt — the server never sets INVISIBLE on players,
                // and a stray byte must not delete our own model.
                if let Ok(mut v) = queries.vis.get_mut(existing) {
                    let want = if !is_self && wire.is_invisible() {
                        Visibility::Hidden
                    } else {
                        Visibility::default()
                    };
                    if *v != want {
                        *v = want;
                    }
                }
                // The spawn arm can only tag self once the id is known, and the
                // player's own entity routinely arrives before it — every reader
                // of this marker (camera, first-person, the self plate) would
                // then treat the player as somebody else for the whole session.
                if is_self && !queries.self_tagged.get(existing).unwrap_or(false) {
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
                let spawn_vis = if !is_self && wire.is_invisible() {
                    Visibility::Hidden
                } else {
                    Visibility::default()
                };
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
                    spawn_vis,
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
        let mounted = queries.mounted.get(rider_e).unwrap_or(false);
        if snap.mount_of(wire).is_none() {
            if mounted {
                commands
                    .entity(rider_e)
                    .remove::<crate::components::MountedRider>();
                // The wire forgets the mount entirely at a dismount, but retail keeps it
                // long enough to play its authored get-off gallop (`coff` on the mount's own
                // model - buck, two gallop loops; census cited on `MountDeparting`). Hand the
                // entity over to `drive_mount_departure_system` instead of letting this
                // frame's sweep reap it.
                let mid = mount_actor_id(wire.id);
                if let Some(&mount_e) = tracked.by_id.get(&mid) {
                    if queries.departing.get(mount_e).is_err() {
                        commands
                            .entity(mount_e)
                            .insert(crate::components::MountDeparting {
                                started: false,
                                elapsed_frames: 0,
                                done: false,
                            });
                        seen.insert(mid);
                        // Every get-off is one INFO line from wire edge to sweep (the two
                        // transitions in `drive_mount_departure_system` are the other halves),
                        // so "the chocobo just stands there" reports name which leg stalled.
                        tracing::info!(
                            target: "kuluu_render::scene",
                            mount_id = wire.id,
                            rider_id = %rider_e,
                            "mount departed the wire; handing the actor to its get-off"
                        );
                    }
                }
            }
            continue;
        }
        if !mounted {
            commands
                .entity(rider_e)
                .insert(crate::components::MountedRider);
        }
        let id = mount_actor_id(wire.id);
        seen.insert(id);
        if tracked.by_id.contains_key(&id) {
            // A rider who climbed back on mid-gallop cancels the departure state; a running
            // one-shot pose clears itself when its authored windows run out.
            if let Some(&mount_e) = tracked.by_id.get(&id) {
                if queries.departing.get(mount_e).is_ok() {
                    commands
                        .entity(mount_e)
                        .remove::<crate::components::MountDeparting>();
                }
            }
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
        .filter(|id| !seen.contains(id) && !is_furniture_actor_id(*id))
        .collect();
    for id in stale {
        // A mount still playing its get-off gallop (or waiting for its model to load so it
        // can) waits out the sweep with its tracking entry intact; `done` hands it back.
        if tracked
            .by_id
            .get(&id)
            .is_some_and(|bevy_e| queries.departing.get(*bevy_e).is_ok_and(|dep| !dep.done))
        {
            continue;
        }
        if let Some(bevy_e) = tracked.by_id.remove(&id) {
            commands.entity(bevy_e).try_despawn();
        }

        prediction.by_id.remove(&id);
        motion.by_id.remove(&id);
        blends.by_id.remove(&id);
    }
}

/// LSB `Flags1.InvisFlag` (bit 29): player-invisibility — a GM hiding themselves or an
/// EFFECTFLAG_INVISIBLE status effect. The server sets it for PCs only
/// (vendor/server/src/map/packets/char_update.cpp CCharUpdatePacket::updateWith). Retail keeps such players
/// targetable but draws nothing: no model, no nameplate (the plate gate lives in
/// `update_nameplate_billboards_system`).
///
/// Unlike STATUS_TYPE::INVISIBLE mobs — where sync_entities_system hides the wire root and
/// takes the hitbox child with it (correct there: those entities are untargetable) — an
/// invisible PC's root must stay visible, or its transparent EntityHitbox child stops being
/// mouse-pickable. So this hides the actor root and blanks the placeholder orb instead;
/// targeting is untouched.
///
/// Runs every frame; it owns nodes nothing else writes (the actor root's Visibility is set
/// only at spawn and here), so there is no fight. The
/// orb material restore is owned by sync_entities_system: the same UPDATE_HP delta that
/// clears the bit marks state dirty and resets it to the kind handle.
pub fn apply_invis_flag_system(
    table: Res<EntityTable>,
    materials: Res<EntityMaterials>,
    mut q_roots: Query<(
        Entity,
        &WorldEntity,
        Option<&mut MeshMaterial3d<StandardMaterial>>,
    )>,
    #[cfg(not(target_arch = "wasm32"))] model_roots: Query<
        &crate::ffxi_actor_render::FfxiRenderRoot,
    >,
    #[cfg(not(target_arch = "wasm32"))] mut other_vis: Query<&mut Visibility, Without<WorldEntity>>,
) {
    for (_bevy_entity, ent, orb_mat) in &mut q_roots {
        let hide = table.get(ent.id).is_some_and(|r| r.invis_flag());

        #[cfg(not(target_arch = "wasm32"))]
        if let Ok(rr) = model_roots.get(_bevy_entity) {
            if let Ok(mut v) = other_vis.get_mut(rr.0) {
                let want = if hide {
                    Visibility::Hidden
                } else {
                    Visibility::default()
                };
                if *v != want {
                    *v = want;
                }
            }
        }

        if let Some(mut mm) = orb_mat {
            if hide && mm.0 != materials.invis_orb {
                mm.0 = materials.invis_orb.clone();
            }
        }
    }
}

pub fn pin_mount_actors_system(
    tracked: Res<TrackedEntities>,
    mut q_xform: Query<&mut Transform, With<WorldEntity>>,
    departing: Query<(), With<crate::components::MountDeparting>>,
) {
    for (&id, &bevy_e) in tracked.by_id.iter() {
        let Some(rider_id) = mount_actor_rider(id) else {
            continue;
        };
        // A departing mount stands where it stopped being ridden and plays its authored
        // get-off gallop there; pinning it back onto the now-foot rider would glue the
        // animation to the player's feet.
        if departing.contains(bevy_e) {
            continue;
        }
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

/// How long a departing mount waits for its model to load before the sweep reaps it
/// unread (5 s of 60 Hz frames) - and the hard ceiling that hands any stuck departure back.
/// Gates `drive_mount_departure_system`'s clock, which is where it is read.
#[cfg(not(target_arch = "wasm32"))]
const MOUNT_DEPARTURE_MAX_FRAMES: u32 = 300;

/// Drive one authored get-off gallop per departing mount: enqueue `coff` from the mount's own
/// model onto the scheduler spine (the same ActiveScheduler + Motion-stage dispatch every other
/// routine runs on - buck `ch1?`, then `run?` with its authored two loops; census cited on
/// [`crate::components::MountDeparting`], law in research/xim poc/game/event/ActorDismountEvent.kt),
/// and hand the entity back to the sweep once the scheduler has no entry left. Retail also fades
/// it over its last 12 frames (the ActorFadeOnCaster stage) - kuluu draws no per-actor fade, so
/// it pops; recorded in lieu of an invented ramp.
///
/// Non-wasm only: the get-off plays off a live `FfxiRenderActor` model, and that module (and
/// `scheduler_runtime::enqueue_routine`) is not compiled into the browser viewer at all.
#[cfg(not(target_arch = "wasm32"))]
pub fn drive_mount_departure_system(
    mut commands: Commands,
    mut q_dep: Query<(Entity, &WorldEntity, &mut crate::components::MountDeparting)>,
    actors: Query<&crate::ffxi_actor_render::FfxiRenderActor>,
    scheds: Query<&crate::scheduler_runtime::ActiveSchedulers>,
) {
    for (bevy_e, we, mut dep) in q_dep.iter_mut() {
        if dep.done {
            continue;
        }
        dep.elapsed_frames += 1;
        let Some(actor) = actors.iter().find(|a| a.world_id == we.id) else {
            if dep.elapsed_frames > MOUNT_DEPARTURE_MAX_FRAMES {
                tracing::warn!(
                    target: "kuluu_render::scene",
                    mount_id = we.id,
                    elapsed_frames = dep.elapsed_frames,
                    "departing mount's model never loaded; the sweep takes it back unread"
                );
                dep.done = true;
            }
            continue;
        };
        if !dep.started {
            // Enqueue the mount's own get-off timeline. A loaded model without the routine has
            // nothing to play: end it now rather than after the grace.
            let lookup =
                crate::scheduler_runtime::RoutineLookup::new().with_actor(actor.routines());
            match crate::scheduler_runtime::ActiveScheduler::from_routine(
                &lookup,
                &ffxi_actor::actor_state::DISMOUNT_ROUTINE,
            ) {
                Some(active) => {
                    // The same component set the special-pose arm gives a wire entity: the
                    // Motion-stage dispatch resolves clip assets and a (self) target off these.
                    tracing::info!(
                        target: "kuluu_render::scene",
                        mount_id = we.id,
                        stages = active.stages.len(),
                        end_frame = active.end_frame(),
                        "departing mount's get-off routine enqueued"
                    );
                    crate::scheduler_runtime::enqueue_routine(
                        &mut commands,
                        bevy_e,
                        active.with_target(Some(bevy_e)),
                    );
                    commands
                        .entity(bevy_e)
                        .try_insert_if_new(actor.action_assets().clone());
                    dep.started = true;
                }
                None => {
                    tracing::info!(
                        target: "kuluu_render::scene",
                        mount_id = we.id,
                        "departing mount's model carries no get-off routine; ending now"
                    );
                    dep.done = true;
                }
            }
            continue;
        }
        // Done when nothing runs the get-off any more: its stages (and their post-finish
        // linger) have played out, which is where retail despawns the mount. Absent counts as
        // finished - `tick_active_schedulers` strips the component with the last entry's TTL,
        // so requiring the query to succeed would strand the entity behind its own sweep
        // protection forever (the reported "stationary chocobo next to the rider"). The frame
        // cap covers a scheduler wedged mid-routine, capping it from the started edge too.
        let get_off_over = match scheds.get(bevy_e) {
            Ok(s) => !s.is_running(ffxi_actor::actor_state::DISMOUNT_ROUTINE),
            Err(_) => true,
        };
        if get_off_over || dep.elapsed_frames > MOUNT_DEPARTURE_MAX_FRAMES {
            tracing::info!(
                target: "kuluu_render::scene",
                mount_id = we.id,
                elapsed_frames = dep.elapsed_frames,
                scheds_present = scheds.get(bevy_e).is_ok(),
                capped = !get_off_over,
                "departing mount's get-off ran out; the sweep takes it back"
            );
            dep.done = true;
        }
    }
}

pub fn sync_aggro_system(
    mut commands: Commands,
    state: Res<SceneState>,
    table: Res<EntityTable>,
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

    let self_char_id = table.self_id().unwrap_or(0);

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
    if cfg!(target_arch = "wasm32") {
        return;
    }
    for (e, t) in &q {
        commands
            .entity(e)
            .insert((PrevRenderPos(t.translation), CurrRenderPos(t.translation)));
    }
}

/// The self avatar's drawn height rides a visual follower toward the lerped
/// physics Y with the same two-stage law the camera rig cascade uses (ease one
/// half once per rendered update, then chase nought-point-two-five per movement
/// tick - the same constants the chase rig cascade carries, cited from
/// `FFXiMain.dll retail-2026-10` sites 0x1001F50F/0x1001F5D3; see
/// kuluu/src/view_native/camera_collision.rs).
/// Why this exists: physics commits a stair riser in one tick (record law - rises up to the
/// follow word are accepted whole, collision record "Step up"), so without a drawn-side follower
/// every tread arrives as a full-height pop: the avatar sawtooths, the camera pivots against that
/// sawtooth, and walking stairs jackhammers the screen instead of sloping up it. Retail's own drawn
/// feed is eased upstream (the mover commits through the state block its probe measurements feed,
/// collision record "Terrain height and gravity, per frame"), while its committed/wired numbers stay
/// stepped - this follower reproduces that separation at kuluu's render seam.
/// The follower is kuluu-authored placement: physics, wire reports, and every collision query keep
/// using the exact logical feet. It never moves XZ; it snaps-seeds on warps and bounds its lag to
/// [`SELF_VISUAL_Y_MAX_LAG_YAMLS`], so a long fall cannot float the avatar further above its logical
/// feet than that budget.
const SELF_VISUAL_Y_EASE_ALPHA: f32 = 0.5;
const SELF_VISUAL_Y_SPRING_ALPHA: f32 = 0.25;
/// Movement-tick measure for the spring substeps - shared basis with the rig cascade.
const SELF_VISUAL_Y_TICK_SECS: f32 = 1.0 / 60.0;
/// Most the drawn height may trail its logical one (yalm). Riser-sized commits (the shipped stair
/// class measures half a yalm, `kuluu-render/tests/mzb_shipped_step_classes.rs`) stay inside the
/// budget and get the full slope; falls and landings self-limit to this float.
const SELF_VISUAL_Y_MAX_LAG_YAMLS: f32 = 1.0;
/// Spring substeps a single rendered frame may simulate, bounding catch-up after a pause or hitch -
/// the same guard the rig cascade carries.
const SELF_VISUAL_Y_MAX_SUBSTEPS: usize = 8;

/// Render-seam state for the drawn-height follower (public because
/// [`interpolate_self_transform_system`] carries it as a `Local`; its fields stay private).
#[derive(Default)]
pub struct SelfVisualY {
    seeded: bool,
    eased: f32,
    sprung: f32,
    carry_ticks: f32,
}

impl SelfVisualY {
    /// One rendered update toward `target_y` (the overstep-lerped physics height).
    fn advance(&mut self, target_y: f32, dt_secs: f32) -> f32 {
        if !self.seeded || (target_y - self.sprung).abs() > SELF_VISUAL_Y_MAX_LAG_YAMLS * 4.0 {
            // First frame and warps ride exact; a jump beyond four lag budgets cannot be a slope.
            self.seeded = true;
            self.eased = target_y;
            self.sprung = target_y;
            self.carry_ticks = 0.0;
            return target_y;
        }
        self.eased += (target_y - self.eased) * SELF_VISUAL_Y_EASE_ALPHA;
        // Fractional share, same fix the chase cascade carries: at any framerate above the tick rate
        // an integer substep count fires spring on some frames and none on their neighbours, which is
        // felt as the drawn body (and every nameplate hung off it) shivering along a flat run at full
        // speed - USER 2026-10-28, "nameplates jitter on a flat surface while running". Whole tick
        // counts are unchanged.
        let elapsed = (self.carry_ticks + dt_secs / SELF_VISUAL_Y_TICK_SECS)
            .clamp(0.0, SELF_VISUAL_Y_MAX_SUBSTEPS as f32);
        let share = 1.0 - (1.0 - SELF_VISUAL_Y_SPRING_ALPHA).powf(elapsed);
        self.sprung += (self.eased - self.sprung) * share;
        self.carry_ticks = 0.0;
        // Lag budget: never let the drawn height float further above (or sink below) its logical one
        // than the cap, so falls track at physics rate past the first frames.
        if (self.sprung - target_y).abs() > SELF_VISUAL_Y_MAX_LAG_YAMLS {
            self.sprung =
                target_y + SELF_VISUAL_Y_MAX_LAG_YAMLS * (target_y - self.sprung).signum();
            self.eased = self.eased.clamp(target_y - 2.0, target_y + 2.0);
        }
        if (self.sprung - target_y).abs() < 1e-4 {
            // Settled: no asymptotic crawl left to accumulate.
            self.sprung = target_y;
            self.eased = target_y;
        }
        self.sprung
    }
}

/// Runs every rendered frame in `RunFixedMainLoopSystems::AfterFixedMainLoop`.
/// Lerps the visual Transform between the last two authoritative render
/// positions (produced by `apply_self_prediction_system` at 60Hz) using the
/// fixed-timestep overstep fraction, then rides the drawn height through the
/// [`SelfVisualY`] follower - XZ stays exact, only Y is followed. This decouples
/// the visible character motion from the fixed-tick cadence and slopes stair
/// risers into view instead of popping them at the logical commit.
pub fn interpolate_self_transform_system(
    fixed_time: Res<Time<Fixed>>,
    time: Res<Time>,
    mut visual_y: Local<SelfVisualY>,
    mut q: Query<(&mut Transform, &PrevRenderPos, &CurrRenderPos), With<IsSelf>>,
) {
    let Ok((mut t, prev, curr)) = q.single_mut() else {
        return;
    };
    let alpha = fixed_time.overstep_fraction();
    let lerped = prev.0.lerp(curr.0, alpha);
    let y = visual_y.advance(lerped.y, time.delta_secs());
    t.translation = Vec3::new(lerped.x, y, lerped.z);
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

    /// Drive the follower at a fixed render dt until it settles or `frames` elapse; return every
    /// produced drawn height, first to last.
    fn drive(mut vis: SelfVisualY, targets: &[f32], dt: f32) -> (SelfVisualY, Vec<f32>) {
        let mut out = Vec::with_capacity(targets.len());
        for &t in targets {
            let y = vis.advance(t, dt);
            out.push(y);
        }
        (vis, out)
    }

    #[test]
    fn a_riser_commit_reaches_drawn_height_sloped_not_whole() {
        // The regression this fixes: physics commits a half-yalm riser in one tick (the shipped stair
        // class), and the pre-follower transform took the full 0.5 yalm in a single rendered frame -
        // the avatar sawtoothed and the camera jackhammered. Drawn height must arrive as a slope.
        let dt = 1.0 / 60.0;
        let (mut vis, flat) = drive(SelfVisualY::default(), &[0.0; 8], dt);
        assert!(flat.iter().all(|&y| y == 0.0), "warm-up drifted: {flat:?}");
        // One commit of +0.5 yalm, then hold (a walker standing on the new tread).
        let targets = vec![0.5; 35];
        let mut prev_y = 0.0;
        let mut max_step = 0.0f32;
        for t in &targets {
            let y = vis.advance(*t, dt);
            max_step = max_step.max(y - prev_y);
            assert!(
                y >= prev_y - 1e-6,
                "drawn height dipped mid-climb: {y} < {prev_y}"
            );
            prev_y = y;
        }
        assert!(
            max_step <= SELF_VISUAL_Y_MAX_LAG_YAMLS * 0.4,
            "a single rendered frame carried {max_step} yalm - riser popped, not sloped"
        );
        // Settles onto the tread (never overshoots above it) within a few tenths of a second.
        assert!(
            (prev_y - 0.5).abs() < 1e-3,
            "drawn height never reached the tread: {prev_y}"
        );
    }

    /// The follower is frame-rate honest: one second of wall clock over a riser commit lands on the
    /// same drawn height whether it is rendered at 30, 60 or 144 Hz, and no frame in the middle of the
    /// climb stands still while most of the riser is still to go. The former integer substep count did
    /// exactly that above 60 Hz - which is how a plate hanging off the drawn body came to shiver along a
    /// flat run (USER 2026-10-28: "nameplates jitter on a flat surface while running").
    #[test]
    fn visual_y_follower_is_frame_rate_honest_across_refresh_rates() {
        let mut ends = Vec::new();
        for fps in [30.0f32, 60.0, 144.0] {
            let dt = 1.0 / fps;
            let mut vis = SelfVisualY::default();
            assert_eq!(vis.advance(0.0, dt), 0.0);
            let mut prev = 0.0f32;
            let mut stalled_while_climbing = 0usize;
            let mut climbing_frames = 0usize;
            for _ in 0..(fps as usize) {
                let y = vis.advance(0.5, dt);
                if 0.5 - prev > 0.25 * 0.5 && y <= prev + 1e-9 {
                    stalled_while_climbing += 1;
                }
                climbing_frames += 1;
                assert!(
                    y >= prev - 1e-6,
                    "drawn height dipped mid-climb at {fps} fps"
                );
                prev = y;
            }
            println!(
                "DIAG visual_y {fps:>5.0} fps after 1 s: drawn {:.9}, stalled frames while climbing {stalled_while_climbing}/{climbing_frames}",
                prev
            );
            assert_eq!(
                stalled_while_climbing, 0,
                "{fps} fps follower stood still on {stalled_while_climbing} of {climbing_frames} frames mid-riser"
            );
            ends.push(prev);
        }
        let spread = ends
            .iter()
            .fold(0.0f32, |acc, v| acc.max((v - ends[0]).abs()));
        println!("DIAG visual_y endpoint spread across 30/60/144 fps: {spread:.3e}");
        assert!(
            spread < 1e-2,
            "refresh rates disagree about the drawn height: {ends:?}"
        );
    }

    #[test]
    fn visual_y_lag_stays_bounded_during_a_long_fall() {
        // A fall commits -1.04 yalm per movement tick (the recorded fall step at the 0.8 rescale).
        // The follower may slope the first frames but must then track: the drawn height never floats
        // more than the lag budget above its logical one.
        let dt = 1.0 / 60.0;
        let mut vis = SelfVisualY::default();
        vis.advance(0.0, dt);
        for i in 1..=90 {
            let target = -(i as f32) * 1.04;
            let y = vis.advance(target, dt);
            assert!(
                (y - target).abs() <= SELF_VISUAL_Y_MAX_LAG_YAMLS + 1e-5,
                "tick {i}: drawn {y} left logical {target} by more than the lag budget"
            );
        }
    }

    #[test]
    fn a_warp_seeds_exact_and_risers_still_slope_afterwards() {
        let dt = 1.0 / 60.0;
        let mut vis = SelfVisualY::default();
        assert_eq!(vis.advance(0.0, dt), 0.0);
        assert_eq!(
            vis.advance(40.0, dt),
            40.0,
            "a warp beyond four lag budgets must ride exact"
        );
        let mut prev_y = 40.0;
        for _ in 0..25 {
            let y = vis.advance(40.5, dt);
            assert!(y - prev_y <= SELF_VISUAL_Y_MAX_LAG_YAMLS * 0.4);
            prev_y = y;
        }
    }

    #[test]
    fn entity_sync_preserves_first_person_self_visibility() {
        let mut app = App::new();
        app.init_resource::<SceneState>()
            .init_resource::<EntityTable>()
            .init_resource::<TrackedEntities>()
            .init_resource::<crate::combat_stance::EntityPrediction>()
            .init_resource::<crate::combat_stance::EntityMotion>()
            .init_resource::<crate::combat_stance::AnimationBlends>()
            .insert_resource(EntityMesh {
                default: Handle::default(),
                pc: Handle::default(),
                mob: Handle::default(),
                pet: Handle::default(),
            })
            .insert_resource(dummy_materials())
            .add_systems(Update, sync_entities_system);
        #[cfg(not(target_arch = "wasm32"))]
        app.init_resource::<crate::dat_mzb::LastAutoLoadedZone>()
            .init_resource::<crate::dat_mzb::LoadMzbInFlight>();
        let player = app
            .world_mut()
            .spawn((
                WorldEntity {
                    id: 7,
                    act_index: 0,
                    kind: EntityKind::Pc,
                },
                IsSelf,
                Transform::default(),
                Visibility::Hidden,
            ))
            .id();
        app.world_mut()
            .resource_mut::<TrackedEntities>()
            .by_id
            .insert(7, player);
        app.world_mut()
            .resource_mut::<EntityTable>()
            .set_self_id(Some(7));
        for x in [0.0, 1.0, 2.0] {
            let mut entity = pc_entity(7, false);
            entity.pos.x = x;
            let mut scene = app.world_mut().resource_mut::<SceneState>();
            scene.snapshot.entities = vec![entity];
            scene.dirty = true;
            app.update();
            assert_eq!(
                app.world().get::<Visibility>(player),
                Some(&Visibility::Hidden)
            );
        }
    }

    #[test]
    fn mount_actor_id_is_reversible_and_disjoint_from_server_ids() {
        // Largest unique_no LSB can build: (4<<28) | (zone<<12) | targid.
        const SERVER_ID_TYPE_SHIFT: u32 = 28;
        const SERVER_ID_ZONE_SHIFT: u32 = 12;
        const SERVER_ID_MAX_TYPE: u32 = 4;
        const SERVER_ID_MAX_ZONE: u32 = 0xFFFF;
        const SERVER_ID_MAX_TARGID: u32 = 0xFFF;
        let max_server_id = (SERVER_ID_MAX_TYPE << SERVER_ID_TYPE_SHIFT)
            | (SERVER_ID_MAX_ZONE << SERVER_ID_ZONE_SHIFT)
            | SERVER_ID_MAX_TARGID;
        assert_eq!(max_server_id & MOUNT_ACTOR_ID_BIT, 0);
        assert_eq!(mount_actor_rider(max_server_id), None);

        for rider in [0x0100_0001u32, 0x1700_0123, max_server_id] {
            let mount = mount_actor_id(rider);
            assert_ne!(mount, rider);
            assert_eq!(mount_actor_rider(mount), Some(rider));
        }
    }

    #[test]
    fn furniture_actor_ids_stay_clear_of_server_and_mount_ids() {
        const SERVER_ID_TYPE_SHIFT: u32 = 28;
        const SERVER_ID_ZONE_SHIFT: u32 = 12;
        const SERVER_ID_MAX_TYPE: u32 = 4;
        const SERVER_ID_MAX_ZONE: u32 = 0xFFFF;
        const SERVER_ID_MAX_TARGID: u32 = 0xFFF;
        let max_server_id = (SERVER_ID_MAX_TYPE << SERVER_ID_TYPE_SHIFT)
            | (SERVER_ID_MAX_ZONE << SERVER_ID_ZONE_SHIFT)
            | SERVER_ID_MAX_TARGID;
        assert!(FURNITURE_ACTOR_IDS.start > max_server_id);
        assert!(!is_furniture_actor_id(max_server_id));
        let last_seat = 2 * ffxi_dat::furniture::SEATS_PER_SHELF;
        for seat in [0, last_seat] {
            let id = furniture_actor_id(seat);
            assert!(is_furniture_actor_id(id));
            assert_eq!(mount_actor_rider(id), None);
        }
        let mounted = mount_actor_id(furniture_actor_id(0));
        assert!(!is_furniture_actor_id(mounted));
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
            invis_orb: Handle::default(),
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

    fn pc_entity(id: u32, invis: bool) -> kuluu_snapshot::Entity {
        kuluu_snapshot::Entity {
            id,
            act_index: 0,
            kind: EntityKind::Pc,
            name: None,
            pos: WireVec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            heading: 0,
            hp_pct: Some(100),
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
            char_flags: kuluu_snapshot::CharFlags {
                invis,
                ..Default::default()
            },
            monstrosity: false,
        }
    }

    /// Flags1.InvisFlag (bit 29) hides the actor root and blanks the placeholder orb,
    /// but must NOT hide the wire root — its transparent EntityHitbox child is what keeps
    /// an invisible player targetable. Kind-gated: a stray bit on a mob changes nothing.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn invis_flag_hides_actor_root_but_keeps_wire_root_pickable() {
        let mut app = App::new();
        app.init_resource::<EntityTable>();
        app.insert_resource(Assets::<StandardMaterial>::default());
        app.add_systems(Update, apply_invis_flag_system);

        let (pc_handle, invis_handle) = {
            let mut assets = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
            (
                assets.add(StandardMaterial::default()),
                assets.add(StandardMaterial::default()),
            )
        };
        app.insert_resource(EntityMaterials {
            pc: pc_handle.clone(),
            invis_orb: invis_handle.clone(),
            ..dummy_materials()
        });

        // Another player's wire root, shaped like scene.rs spawns it: visible root,
        // placeholder orb material, transparent hitbox child with explicit Visible.
        let root = app
            .world_mut()
            .spawn((
                WorldEntity {
                    id: 7,
                    act_index: 0,
                    kind: EntityKind::Pc,
                },
                Visibility::default(),
                MeshMaterial3d(pc_handle.clone()),
            ))
            .id();
        let hitbox = app
            .world_mut()
            .spawn((Visibility::Visible, ChildOf(root)))
            .id();

        let actor_root = app.world_mut().spawn(Visibility::default()).id();
        app.world_mut()
            .entity_mut(root)
            .insert(crate::ffxi_actor_render::FfxiRenderRoot(actor_root));

        // A mob carrying the same bit must be untouched (LSB sets InvisFlag for PCs only).
        let mob_root = app
            .world_mut()
            .spawn((
                WorldEntity {
                    id: 8,
                    act_index: 0,
                    kind: EntityKind::Mob,
                },
                Visibility::default(),
            ))
            .id();
        let mob_actor = app.world_mut().spawn(Visibility::default()).id();
        app.world_mut()
            .entity_mut(mob_root)
            .insert(crate::ffxi_actor_render::FfxiRenderRoot(mob_actor));

        {
            let mut table = app.world_mut().resource_mut::<EntityTable>();
            table.upsert(&pc_entity(7, true));
            table.upsert(&kuluu_snapshot::Entity {
                kind: EntityKind::Mob,
                char_flags: kuluu_snapshot::CharFlags {
                    invis: true,
                    ..Default::default()
                },
                ..pc_entity(8, false)
            });
        }

        app.update();

        assert_eq!(
            *app.world().get::<Visibility>(actor_root).unwrap(),
            Visibility::Hidden,
            "InvisFlag PC: the actor root must be hidden"
        );
        assert_eq!(
            *app.world().get::<Visibility>(root).unwrap(),
            Visibility::default(),
            "InvisFlag PC: the wire root stays visible so its hitbox child remains pickable"
        );
        assert_eq!(
            *app.world().get::<Visibility>(hitbox).unwrap(),
            Visibility::Visible,
            "the transparent hitbox must keep its explicit Visible"
        );
        let orb_mat = app
            .world()
            .get::<MeshMaterial3d<StandardMaterial>>(root)
            .unwrap();
        assert_eq!(
            orb_mat.0.id(),
            invis_handle.id(),
            "placeholder orb is blanked to the shared transparent handle"
        );
        assert_eq!(
            *app.world().get::<Visibility>(mob_actor).unwrap(),
            Visibility::default(),
            "a stray InvisFlag bit on a mob must not hide it (PCs only)"
        );

        app.world_mut()
            .resource_mut::<EntityTable>()
            .upsert(&pc_entity(7, false));
        app.update();

        assert_eq!(
            *app.world().get::<Visibility>(actor_root).unwrap(),
            Visibility::default(),
            "flag cleared: the actor root comes back"
        );
    }

    fn retarget_app() -> App {
        let mut app = App::new();
        app.init_resource::<crate::snapshot::EventLog>()
            .init_resource::<SceneState>()
            .init_resource::<Target>()
            .init_resource::<crate::lock_on::LockOn>()
            .add_systems(Update, apply_server_retarget_system);
        app
    }

    /// A fresh authoritative frame carrying `ids`, as ingest would leave it.
    fn push_snapshot(app: &mut App, ids: &[u32]) {
        let mut state = app.world_mut().resource_mut::<SceneState>();
        state.snapshot.entities = ids
            .iter()
            .map(|&id| entity_with_hp(id, Some(100)))
            .collect();
        state.dirty = true;
    }

    fn push_event(app: &mut App, ev: kuluu_snapshot::ViewerEvent) {
        app.world_mut()
            .resource_mut::<crate::snapshot::EventLog>()
            .push(ev);
    }

    fn retarget_to(id: u32) -> kuluu_snapshot::ViewerEvent {
        kuluu_snapshot::ViewerEvent::TargetChanged {
            target_id: Some(id),
        }
    }

    /// The server-pushed retarget (s2c 0x058 ASSIST, 0x058_assist.cpp) is what
    /// makes `/assist` and auto-target-after-kill move the cursor.
    #[test]
    fn server_retarget_moves_the_target() {
        let mut app = retarget_app();
        push_snapshot(&mut app, &[11, 22]);
        app.world_mut().resource_mut::<Target>().id = Some(11);
        push_event(&mut app, retarget_to(22));
        app.update();
        assert_eq!(app.world().resource::<Target>().id, Some(22));
    }

    /// LSB emits `AssistNo` 0 from `CCharEntity::OnChangeTarget` when the
    /// target went away; dropping the cursor there is not established
    /// behaviour, and `auto_clear_target_system` already handles the entity
    /// actually leaving the scene.
    #[test]
    fn zero_assist_no_leaves_the_target_alone() {
        let mut app = retarget_app();
        push_snapshot(&mut app, &[11]);
        app.world_mut().resource_mut::<Target>().id = Some(11);
        push_event(
            &mut app,
            kuluu_snapshot::ViewerEvent::TargetChanged { target_id: None },
        );
        app.update();
        assert_eq!(app.world().resource::<Target>().id, Some(11));
    }

    /// The cursor is a global index, so an event the ring already evicted must
    /// not be replayed and a later local retarget must not be undone.
    #[test]
    fn retarget_is_edge_triggered_not_reapplied() {
        let mut app = retarget_app();
        push_snapshot(&mut app, &[22, 33]);
        push_event(&mut app, retarget_to(22));
        app.update();
        assert_eq!(app.world().resource::<Target>().id, Some(22));

        app.world_mut().resource_mut::<Target>().id = Some(33);
        app.update();
        assert_eq!(
            app.world().resource::<Target>().id,
            Some(33),
            "an already-drained event must not re-fire"
        );
    }

    #[derive(Resource, Default)]
    struct TargetChanges(u32);

    fn count_target_changes(target: Res<Target>, mut changes: ResMut<TargetChanges>) {
        if target.is_changed() {
            changes.0 += 1;
        }
    }

    /// `ResMut` marks the resource changed on any deref, so a repeat 0x058 for
    /// the target we already have must not touch `Target` at all - LSB pushes
    /// one from `CPlayerController::Engage` and one per `CAttackState::
    /// UpdateTarget` auto-target, and every touch costs a c2s ChangeTarget.
    #[test]
    fn a_repeat_retarget_does_not_touch_the_target() {
        let mut app = retarget_app();
        app.init_resource::<TargetChanges>().add_systems(
            Update,
            count_target_changes.after(apply_server_retarget_system),
        );
        push_snapshot(&mut app, &[22]);
        app.world_mut().resource_mut::<Target>().id = Some(22);
        app.update();
        let before = app.world().resource::<TargetChanges>().0;

        push_event(&mut app, retarget_to(22));
        app.update();
        assert_eq!(
            app.world().resource::<TargetChanges>().0,
            before,
            "the same target re-pushed must not tick change detection"
        );
    }

    /// A held lock is a flag over the one target slot in retail's model
    /// (research/xim Actor.kt createFrom), so a server-side target change carries
    /// the lock with it instead of leaving camera and target panel on
    /// different mobs.
    #[test]
    fn a_held_lock_follows_the_server_retarget() {
        let mut app = retarget_app();
        push_snapshot(&mut app, &[11, 22]);
        app.world_mut().resource_mut::<Target>().id = Some(11);
        app.world_mut()
            .resource_mut::<crate::lock_on::LockOn>()
            .target_id = Some(11);
        push_event(&mut app, retarget_to(22));
        app.update();
        assert_eq!(
            app.world().resource::<crate::lock_on::LockOn>().target_id,
            Some(22)
        );
    }

    #[test]
    fn an_unlocked_camera_stays_unlocked_through_a_retarget() {
        let mut app = retarget_app();
        push_snapshot(&mut app, &[22]);
        push_event(&mut app, retarget_to(22));
        app.update();
        assert_eq!(
            app.world().resource::<crate::lock_on::LockOn>().target_id,
            None
        );
    }

    /// The 0x058 (0x058_assist.cpp) rides the event channel while the entity
    /// rides the snapshot, so the retarget can arrive first. Applying it
    /// straight away would hand
    /// `auto_clear_target_system` a target with no entity, which drops it for
    /// good.
    #[test]
    fn a_retarget_waits_for_its_entity_to_arrive() {
        let mut app = retarget_app();
        push_snapshot(&mut app, &[11]);
        app.world_mut().resource_mut::<Target>().id = Some(11);
        push_event(&mut app, retarget_to(22));
        app.update();
        assert_eq!(app.world().resource::<Target>().id, Some(11));

        push_snapshot(&mut app, &[11, 22]);
        app.update();
        assert_eq!(app.world().resource::<Target>().id, Some(22));
    }

    #[test]
    fn a_retarget_whose_entity_never_arrives_expires() {
        let mut app = retarget_app();
        push_event(&mut app, retarget_to(22));
        for _ in 0..RETARGET_ENTITY_WAIT_FRAMES {
            push_snapshot(&mut app, &[11]);
            app.update();
        }

        push_snapshot(&mut app, &[11, 22]);
        app.update();
        assert_eq!(
            app.world().resource::<Target>().id,
            None,
            "a stale retarget must not fire once its entity finally shows up"
        );
    }

    /// An entity present but not targetable is one `auto_clear_target_system`
    /// would clear right after, so it does not count as arrived.
    #[test]
    fn an_untargetable_entity_does_not_satisfy_a_retarget() {
        let mut app = retarget_app();
        push_event(&mut app, retarget_to(22));
        let mut state = app.world_mut().resource_mut::<SceneState>();
        state.snapshot.entities = vec![entity_with_hp(22, Some(0))];
        state.dirty = true;
        app.update();
        assert_eq!(app.world().resource::<Target>().id, None);
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
            monstrosity: false,
        }
    }

    /// The reported "stationary chocobo next to the rider": `tick_active_schedulers` strips
    /// `(ActiveSchedulers, ActionAssets, ActionTarget)` with the last entry's TTL, and the old
    /// done-test demanded that query succeed - a retired get-off therefore stranded the entity
    // behind its own sweep protection forever. Absence now counts as finished.
    #[test]
    fn a_departing_mount_is_handed_back_when_its_get_off_retired() {
        let mut app = App::new();
        app.add_message::<crate::scheduler_runtime::SchedulerStageEvent>();
        app.add_message::<crate::scheduler_runtime::CutsceneMotionDone>();
        app.init_resource::<Time>();
        app.add_systems(
            Update,
            (
                crate::scheduler_runtime::tick_active_schedulers,
                drive_mount_departure_system,
            )
                .chain(),
        );

        let sched = ffxi_dat::scheduler::Scheduler {
            name: *b"coff",
            stages: Vec::new(),
        };
        let mut active = crate::scheduler_runtime::ActiveScheduler::from_scheduler(&sched);
        // Past the authored end and the post-finish TTL: the very next tick retires the
        // entry and strips the component with it.
        active.elapsed = 60.0;

        let mount_id = mount_actor_id(7);
        let mount = app
            .world_mut()
            .spawn((
                crate::components::InGameEntity,
                WorldEntity {
                    id: mount_id,
                    act_index: 12,
                    kind: EntityKind::Pet,
                },
                Transform::default(),
                crate::ffxi_actor_render::render_actor_stub(mount_id),
                crate::components::MountDeparting {
                    started: true,
                    elapsed_frames: 30,
                    done: false,
                },
                crate::scheduler_runtime::ActiveSchedulers::one(active),
            ))
            .id();

        // One update lets the tick retire the entry; the commands stripping the component
        // apply before `drive_mount_departure_system` runs behind it.
        app.update();
        assert!(
            !app.world()
                .entity(mount)
                .contains::<crate::scheduler_runtime::ActiveSchedulers>(),
            "the retired get-off strips its scheduler component",
        );
        let dep = app.world().get::<crate::components::MountDeparting>(mount).expect("departure marker survives the tick that retires the routine - the sweep exemption reads it");
        assert!(dep.done, "a retired get-off must hand the departing mount back to the sweep (the stationary-chocobo bug)");
    }

    /// A get-off wedged mid-routine cannot outlive the frame cap either: with a running
    /// `coff` entry still listed and the departure clock past the ceiling, the entity is
    // released anyway.
    #[test]
    fn a_wedged_get_off_hits_the_departure_frame_cap() {
        let mut app = App::new();
        app.add_message::<crate::scheduler_runtime::SchedulerStageEvent>();
        app.add_message::<crate::scheduler_runtime::CutsceneMotionDone>();
        app.init_resource::<Time>();
        app.add_systems(
            Update,
            (
                crate::scheduler_runtime::tick_active_schedulers,
                drive_mount_departure_system,
            )
                .chain(),
        );

        let sched = ffxi_dat::scheduler::Scheduler {
            name: *b"coff",
            // A stage authored far past the routine's start: `tick_active_schedulers` can
            // never retire it, so only `MOUNT_DEPARTURE_MAX_FRAMES` can. Full payload per
            // scheduler_runtime's own test constructor (an argument-less Unknown kind fires
            // nothing on the way there).
            stages: vec![ffxi_dat::scheduler::TimedStage {
                frame: u32::MAX / 2,
                stage: ffxi_dat::scheduler::SchedulerStage {
                    animation_mode: None,
                    hand: None,
                    kind: ffxi_dat::scheduler::StageKind::Unknown,
                    raw_type: 0,
                    stage_words: ffxi_dat::scheduler::SYNTHESIZED_STAGE_WORDS,
                    delay_frames: 0,
                    duration_frames: 0,
                    id: [0; 4],
                    max_loops: 0,
                    transition_in: 0,
                    transition_out: 0,
                    random_group: None,
                    local_dir: ffxi_dat::scheduler::NO_LOCAL_DIR,
                    model_transform: None,
                    follow_points: None,
                    screen_color: None,
                    actor_fade: None,
                    idle_transition_time: None,
                    flinch_duration: None,
                    model_visibility: None,
                    actor_rotation: None,
                    turn_toward_step_degrees: None,
                    spell_effect: None,
                    sound_range: None,
                    control_flow: None,
                },
            }],
        };
        let active = crate::scheduler_runtime::ActiveScheduler::from_scheduler(&sched);

        let mount_id = mount_actor_id(8);
        let mount = app
            .world_mut()
            .spawn((
                crate::components::InGameEntity,
                WorldEntity {
                    id: mount_id,
                    act_index: 12,
                    kind: EntityKind::Pet,
                },
                Transform::default(),
                crate::ffxi_actor_render::render_actor_stub(mount_id),
                crate::components::MountDeparting {
                    started: true,
                    elapsed_frames: MOUNT_DEPARTURE_MAX_FRAMES + 1,
                    done: false,
                },
                crate::scheduler_runtime::ActiveSchedulers::one(active),
            ))
            .id();

        app.update();
        let dep = app
            .world()
            .get::<crate::components::MountDeparting>(mount)
            .expect("entity survives to be capped, not swept blind");
        assert!(dep.done, "the frame cap releases a wedged get-off");
    }
}
