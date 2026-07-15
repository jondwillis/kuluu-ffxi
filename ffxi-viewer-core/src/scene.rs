use std::collections::HashMap;

use bevy::light::FogVolume;
use bevy::picking::Pickable;
use bevy::prelude::*;
use ffxi_viewer_wire::{EntityKind, Vec3 as WireVec3};

use crate::components::{IsSelf, LookComp, MorphIn, Nameplate, WorldEntity};
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

pub fn should_clear_target(id: Option<u32>, entities: &[ffxi_viewer_wire::Entity]) -> bool {
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

pub fn setup_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut moon_materials: ResMut<Assets<crate::moon_material::MoonMaterial>>,
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
            ..default()
        },
        Transform::from_xyz(0.0, 200.0, 0.0).with_scale(Vec3::splat(2000.0)),
    ));

    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.85, 0.88, 1.0),
        brightness: 500.0,
        ..default()
    });
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
    mut q_xform: Query<&mut Transform, With<WorldEntity>>,
    mut q_mat: Query<&mut MeshMaterial3d<StandardMaterial>, (With<WorldEntity>, Without<MorphIn>)>,
    q_nameplates: Query<&Nameplate>,
    mut prev_zone: Local<Option<Option<u32>>>,
) {
    if !state.dirty {
        return;
    }

    let snap = &state.snapshot;

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
                EntityKind::Mob | EntityKind::Pc | EntityKind::Pet
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
                if let Ok(mut t) = q_xform.get_mut(existing) {
                    if is_self {
                        let y = if zone_changed {
                            world_pos.y
                        } else {
                            t.translation.y
                        };
                        t.translation = Vec3::new(world_pos.x, y, world_pos.z);
                        t.rotation = heading_to_quat(wire.heading);
                    } else if matches!(wire.kind, EntityKind::Npc | EntityKind::Other) {
                        let smoothed = apply_visual_smoothing(t.translation, world_pos);
                        t.translation = Vec3::new(smoothed.x, t.translation.y, smoothed.z);
                        t.rotation = heading_to_quat(wire.heading);
                    }
                }
                if let Ok(mut m) = q_mat.get_mut(existing) {
                    m.0 = mat;
                }
            }
            None => {
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

        // No self plate: retail never draws the local player's own overhead
        // name, and the self plate's overhead projection sits just above the
        // first-person eye where frame skew makes it dip/jitter (kuluu-gr2).
        if let Some(name) = wire.name.as_deref().filter(|s| !s.is_empty() && !is_self) {
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
                    crate::nameplate_billboard::nameplate_color(wire.kind, false, false),
                );
                nameplated.insert(wire.id);
            }
        }
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
        match (should_aggro, has_aggro.is_some()) {
            (true, false) => {
                commands.entity(e).try_insert(Aggroing);
                m.0 = mats.aggro.clone();
            }
            (true, true) => {
                m.0 = mats.aggro.clone();
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

fn heading_to_quat(heading: u8) -> Quat {
    let angle = (heading as f32) * std::f32::consts::TAU / 256.0;
    Quat::from_rotation_y(-angle)
}

#[derive(Resource, Default, Debug, Clone)]
pub struct SelfAppearance {
    pub look: Option<ffxi_viewer_wire::EntityLook>,
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
    // set it yet, so the model shows before the server's CHAR_PC for self lands.
    // Once a LookComp exists, sync_entity_looks_system (server-driven) owns it —
    // otherwise this would clobber the server look every frame and the self model
    // would never reflect gear changes (other PCs already update via that system).
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
        match (&wire.look, current) {
            (Some(new), Some(LookComp(old))) if new == old => {}
            (Some(new), _) => {
                commands.entity(bevy_e).try_insert(LookComp(*new));
            }
            (None, Some(_)) => {
                commands.entity(bevy_e).remove::<LookComp>();
            }
            (None, None) => {}
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

    fn entity_with_hp(id: u32, hp_pct: Option<u8>) -> ffxi_viewer_wire::Entity {
        ffxi_viewer_wire::Entity {
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
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            status: 0,
        }
    }
}
