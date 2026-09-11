use super::*;
use crate::components::InGameEntity;
use bevy::ecs::system::RunSystemOnce;

const SELF_ID: u32 = 1;
const NPC_ID: u32 = 2;
const MOB_ID: u32 = 3;
const PC_ID: u32 = 4;
const PARTY_ID: u32 = 5;
const PET_ID: u32 = 6;
const EPSILON: f32 = 1e-5;

fn member(id: u32) -> PartyMember {
    PartyMember {
        id,
        act_index: 1,
        name: None,
        hp: 1,
        mp: 0,
        tp: 0,
        hp_pct: 100,
        mp_pct: 0,
        zone_no: 1,
        main_job: job_id("WAR"),
        main_job_lv: 1,
        sub_job: 0,
        sub_job_lv: 0,
        is_party_leader: false,
        is_alliance_leader: false,
        party_no: 0,
        in_mog_house: false,
    }
}

fn job_id(code: &str) -> u8 {
    ffxi_vocab::job_names::JOB_ABBREVS
        .iter()
        .find(|(_, name)| *name == code)
        .unwrap()
        .0 as u8
}

fn wire_entity(id: u32, kind: EntityKind) -> kuluu_snapshot::Entity {
    kuluu_snapshot::Entity {
        id,
        act_index: 1,
        kind,
        name: Some("Radar fixture".into()),
        pos: kuluu_snapshot::Vec3::default(),
        heading: 0,
        hp_pct: Some(100),
        bt_target_id: 0,
        face_target: 0,
        claim_id: 0,
        speed: 0,
        speed_base: 0,
        look: None,
        animation: 0,
        animationsub: 0,
        mount: None,
        status: 0,
        char_flags: default(),
        monstrosity: false,
        name_vis: None,
    }
}

fn camera_transform(forward: Vec3) -> GlobalTransform {
    GlobalTransform::from(Transform::IDENTITY.looking_to(forward, Vec3::Y))
}

fn fixture() -> (World, Entity) {
    let mut world = World::new();
    world.init_resource::<Assets<Image>>();
    world.init_resource::<SceneState>();
    let mut snapshot = world.resource::<SceneState>().snapshot.clone();
    snapshot.self_char_id = Some(SELF_ID);
    snapshot.party = vec![member(SELF_ID), member(PARTY_ID)];
    let mut table = EntityTable::default();
    table.apply_snapshot(&snapshot);
    table.set_self_id(Some(SELF_ID));
    world.resource_mut::<SceneState>().snapshot = snapshot;
    world.insert_resource(table);
    world.spawn((IsSelf, Transform::IDENTITY));
    let camera = world
        .spawn((OperatorCamera, camera_transform(Vec3::NEG_Z)))
        .id();
    world
        .run_system_once(crate::hud::spawn_bottom_left_stack)
        .unwrap();
    (world, camera)
}

fn add_actor(world: &mut World, id: u32, kind: EntityKind, position: Vec3) -> Entity {
    world
        .resource_mut::<EntityTable>()
        .upsert(&wire_entity(id, kind));
    world
        .spawn((
            WorldEntity {
                id,
                act_index: 1,
                kind,
            },
            Transform::from_translation(position),
        ))
        .id()
}

fn dot(world: &mut World, id: u32) -> Option<(Entity, Node, Color)> {
    world
        .query::<(Entity, &CompassDot, &Node, &BackgroundColor)>()
        .iter(world)
        .find(|(_, dot, _, _)| dot.entity_id == id)
        .map(|(entity, _, node, color)| (entity, node.clone(), color.0))
}

fn dot_offset(node: &Node) -> Vec2 {
    let (Val::Px(left), Val::Px(top)) = (node.left, node.top) else {
        panic!("pixel position")
    };
    Vec2::new(
        left + DOT_DIAMETER_PX * 0.5 - PANEL_WIDTH_PX * 0.5,
        top + DOT_DIAMETER_PX * 0.5 - PANEL_HEIGHT_PX * 0.5,
    )
}

#[test]
fn camera_rotation_moves_cardinals_and_dots_together_without_turning_glyphs() {
    let (mut world, camera) = fixture();
    add_actor(
        &mut world,
        NPC_ID,
        EntityKind::Npc,
        Vec3::NEG_Z * RADAR_RANGE_YALMS * 0.5,
    );
    for (facing, north_screen) in [
        (Vec3::NEG_Z, Vec2::new(0.0, -RADAR_FLATTENING)),
        (Vec3::X, Vec2::NEG_X),
        (Vec3::Z, Vec2::new(0.0, RADAR_FLATTENING)),
        (Vec3::NEG_X, Vec2::X),
    ] {
        *world.get_mut::<GlobalTransform>(camera).unwrap() = camera_transform(facing);
        world.run_system_once(update_compass).unwrap();
        world.run_system_once(update_compass_dots).unwrap();
        let (_, node, _) = dot(&mut world, NPC_ID).unwrap();
        assert!((dot_offset(&node) - north_screen * RADAR_RADIUS_PX * 0.5).length() < EPSILON);
        let mut labels = world.query::<(&CompassLabel, &Node, Option<&UiTransform>)>();
        for (label, node, rotation) in labels.iter(&world) {
            assert_eq!(
                rotation.unwrap().rotation,
                Rot2::IDENTITY,
                "cardinal glyph stays upright"
            );
            if label.direction == Vec2::NEG_Y {
                let position = centered_node(
                    north_screen * CARDINAL_RADIUS_PX,
                    Vec2::splat(CARDINAL_SIZE_PX),
                );
                let (Val::Px(actual), Val::Px(expected)) = (node.left, position.left) else {
                    panic!()
                };
                assert!((actual - expected).abs() < EPSILON);
                let (Val::Px(actual), Val::Px(expected)) = (node.top, position.top) else {
                    panic!()
                };
                assert!((actual - expected).abs() < EPSILON);
            }
        }
    }
}

#[test]
fn vanilla_colors_and_enemy_job_gate_are_independent_of_terrain_minimap() {
    let (mut world, _) = fixture();
    world.init_resource::<crate::graphics_settings::GraphicsSettings>();
    world.init_resource::<crate::minimap::overlay::MarkerFilters>();
    world.init_resource::<crate::minimap::MinimapVisible>();
    for (id, kind) in [
        (NPC_ID, EntityKind::Npc),
        (MOB_ID, EntityKind::Mob),
        (PC_ID, EntityKind::Pc),
        (PARTY_ID, EntityKind::Pc),
        (PET_ID, EntityKind::Pet),
    ] {
        add_actor(&mut world, id, kind, Vec3::X);
    }
    world.run_system_once(update_compass_dots).unwrap();
    assert!(!world.resource::<crate::minimap::MinimapVisible>().0);
    assert!(
        dot(&mut world, MOB_ID).is_none(),
        "WAR cannot see enemy dots"
    );
    for (id, color) in [
        (NPC_ID, NPC_DOT_COLOR),
        (PC_ID, PC_DOT_COLOR),
        (PARTY_ID, PARTY_DOT_COLOR),
        (PET_ID, PET_DOT_COLOR),
    ] {
        assert_eq!(dot(&mut world, id).unwrap().2, color);
    }
    for code in ["THF", "BST", "RNG", "NIN", "SMN", "BLU"] {
        for support in [false, true] {
            {
                let mut scene = world.resource_mut::<SceneState>();
                let member = &mut scene.snapshot.party[0];
                member.main_job = if support { job_id("WAR") } else { job_id(code) };
                member.sub_job = if support { job_id(code) } else { 0 };
            }
            world.run_system_once(update_compass_dots).unwrap();
            assert_eq!(dot(&mut world, MOB_ID).unwrap().2, MOB_DOT_COLOR);
        }
    }
    world.resource_mut::<SceneState>().snapshot.party[0].sub_job = 0;
    world.run_system_once(update_compass_dots).unwrap();
    assert!(
        dot(&mut world, MOB_ID).is_none(),
        "losing eligible support job removes enemy dot"
    );
}

#[test]
fn radar_removes_stale_out_of_range_hidden_and_missing_self_dots() {
    let (mut world, _) = fixture();
    let actor = add_actor(&mut world, NPC_ID, EntityKind::Npc, Vec3::X);
    world.run_system_once(update_compass_dots).unwrap();
    assert!(dot(&mut world, NPC_ID).is_some());
    world.get_mut::<Transform>(actor).unwrap().translation = Vec3::X * RADAR_RANGE_YALMS * 2.0;
    world.run_system_once(update_compass_dots).unwrap();
    assert!(dot(&mut world, NPC_ID).is_none());
    world.get_mut::<Transform>(actor).unwrap().translation = Vec3::X;
    let mut hidden = wire_entity(NPC_ID, EntityKind::Npc);
    hidden.status = kuluu_snapshot::status_type::INVISIBLE;
    world.resource_mut::<EntityTable>().upsert(&hidden);
    world.run_system_once(update_compass_dots).unwrap();
    assert!(dot(&mut world, NPC_ID).is_none());
    world
        .resource_mut::<EntityTable>()
        .upsert(&wire_entity(NPC_ID, EntityKind::Npc));
    world.run_system_once(update_compass_dots).unwrap();
    assert!(dot(&mut world, NPC_ID).is_some());
    world.resource_mut::<EntityTable>().remove(NPC_ID);
    world.run_system_once(update_compass_dots).unwrap();
    assert!(dot(&mut world, NPC_ID).is_none());
    world
        .resource_mut::<EntityTable>()
        .upsert(&wire_entity(NPC_ID, EntityKind::Npc));
    world.run_system_once(update_compass_dots).unwrap();
    let self_entity = world
        .query_filtered::<Entity, With<IsSelf>>()
        .single(&world)
        .unwrap();
    world.despawn(self_entity);
    world.run_system_once(update_compass_dots).unwrap();
    assert!(dot(&mut world, NPC_ID).is_none());
}

#[test]
fn logout_despawns_radar_children_and_next_login_recreates_them() {
    let (mut world, _) = fixture();
    add_actor(&mut world, NPC_ID, EntityKind::Npc, Vec3::X);
    world.run_system_once(update_compass_dots).unwrap();
    let first_dot = dot(&mut world, NPC_ID).unwrap().0;
    let roots: Vec<_> = world
        .query_filtered::<Entity, With<InGameEntity>>()
        .iter(&world)
        .collect();
    for root in roots {
        world.despawn(root);
    }
    assert_eq!(world.query::<&CompassPanel>().iter(&world).count(), 0);
    assert_eq!(world.query::<&CompassDot>().iter(&world).count(), 0);
    world
        .run_system_once(crate::hud::spawn_bottom_left_stack)
        .unwrap();
    world.run_system_once(update_compass_dots).unwrap();
    assert_ne!(dot(&mut world, NPC_ID).unwrap().0, first_dot);
}

#[test]
fn compass_art_uses_all_dat_quadrants_and_north_tint_when_available() {
    let Ok(root) = ffxi_dat::DatRoot::from_env_or_default() else {
        return;
    };
    let (mut world, _) = fixture();
    world.insert_resource(UiElementDatRoot(Some(std::sync::Arc::new(root))));
    world.init_resource::<UiElementAtlas>();
    world.run_system_once(update_compass_art).unwrap();
    let (_, children) = world
        .query_filtered::<(Entity, &Children), With<CompassRose>>()
        .single(&world)
        .unwrap();
    assert_eq!(
        children.len(),
        4,
        "dial contains all four mirrored quadrants"
    );
    let mut positions = Vec::new();
    for child in children {
        let node = world.get::<Node>(*child).unwrap();
        assert_eq!(node.width, Val::Percent(50.0));
        assert_eq!(node.height, Val::Percent(50.0));
        positions.push((node.left, node.top));
    }
    for x in [0.0, 50.0] {
        for y in [0.0, 50.0] {
            assert!(positions.contains(&(Val::Percent(x), Val::Percent(y))));
        }
    }
    let mut labels = world.query::<(&CompassLabel, &Children)>();
    for (label, children) in labels.iter(&world) {
        let image = world.get::<ImageNode>(children[0]).unwrap();
        let color = image.color.to_srgba();
        assert_eq!(color.alpha, 1.0, "DAT opacity is baked into the DXT3 image");
        if label.direction == Vec2::NEG_Y {
            assert!(color.red > color.green && color.red > color.blue);
        } else {
            assert_eq!(color.red, color.green);
            assert_eq!(color.red, color.blue);
        }
    }
    world.run_system_once(update_compass_art).unwrap();
    assert_eq!(
        world
            .query_filtered::<&Children, With<CompassRose>>()
            .single(&world)
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn track_pointer_and_cardinals_share_camera_relative_bearings() {
    for facing in [Vec2::NEG_Y, Vec2::X, Vec2::Y, Vec2::NEG_X] {
        let right = Vec2::new(-facing.y, facing.x);
        assert!(track_pointer_theta(facing, facing).abs() < EPSILON);
        assert!((track_pointer_theta(facing, right) - std::f32::consts::FRAC_PI_2).abs() < EPSILON);
        assert!(
            (track_pointer_theta(facing, -facing).abs() - std::f32::consts::PI).abs() < EPSILON
        );
    }
}
