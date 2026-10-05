use super::*;

const SOURCE: [u8; 4] = *b"src1";
const LINK: [u8; 4] = *b"lnk1";
const SOURCE_CHILD: [u8; 4] = *b"src2";
const LINK_CHILD: [u8; 4] = *b"lnk2";
const MESH: [u8; 4] = *b"tri1";
const SOURCE_TRACKS: [[u8; 4]; 3] = [*b"px  ", *b"py  ", *b"pz  "];
const LINK_TRACKS: [[u8; 4]; 3] = [*b"lx  ", *b"ly  ", *b"lz  "];
const SOURCE_DAMPING: [u8; 4] = *b"pd  ";
const LINK_DAMPING: [u8; 4] = *b"ld  ";
const SOURCE_END: Vec3 = Vec3::new(-6.0, -8.0, -10.0);
const LINK_END: Vec3 = Vec3::new(2.0, 4.0, 6.0);
const SOURCE_FACTOR: f32 = 0.25;
const LINK_FACTOR: f32 = 0.75;
const LIFE: f32 = ROUTINE_FPS * 4.0;
const HALF_LIFE: f32 = LIFE / 2.0;
const ONE_FRAME: f32 = 1.0;
const SOURCE_AND_LINK: usize = 2;

mod capture;

fn assets(link_bindings: bool) -> ActionAssets {
    let mut assets = ActionAssets::default();
    assets.d3ms.insert(
        MESH,
        ffxi_dat::d3m::D3m {
            name: MESH,
            num_triangles: 1,
            texture_name: [0; 16],
            vertices: [Vec3::NEG_X, Vec3::X, Vec3::Y]
                .into_iter()
                .map(|pos| ffxi_dat::d3m::D3mVertex {
                    pos: pos.to_array(),
                    normal: Vec3::Z.to_array(),
                    color: Vec4::ONE.to_array(),
                    uv: Vec2::ZERO.to_array(),
                })
                .collect(),
        },
    );
    let mut source = def(LIFE, ROUTINE_FPS, 0);
    source.mesh_id = MESH;
    source.base_position = Vec3::ZERO.to_array();
    source.init_velocity = Vec3::X.to_array();
    source.immediate_generator = Some(LINK);
    source.position_x_track = Some(SOURCE_TRACKS[0]);
    source.position_y_track = Some(SOURCE_TRACKS[1]);
    source.position_z_track = Some(SOURCE_TRACKS[2]);
    source.velocity_dampener_track = Some(SOURCE_DAMPING);
    source.dampening_factor_applier = true;
    source.velocity_dampener = Some([ONE_FRAME, 0.0]);
    source.child_generator = Some(SOURCE_CHILD);
    let mut linked = source;
    linked.immediate_generator = None;
    linked.position_x_track = link_bindings.then_some(LINK_TRACKS[0]);
    linked.position_y_track = link_bindings.then_some(LINK_TRACKS[1]);
    linked.position_z_track = link_bindings.then_some(LINK_TRACKS[2]);
    linked.velocity_dampener_track = link_bindings.then_some(LINK_DAMPING);
    linked.dampening_factor_applier = link_bindings;
    linked.child_generator = link_bindings.then_some(LINK_CHILD);
    let mut descendant = source;
    descendant.immediate_generator = None;
    descendant.child_generator = None;
    for (id, definition) in [
        (SOURCE, source),
        (LINK, linked),
        (SOURCE_CHILD, descendant),
        (LINK_CHILD, descendant),
    ] {
        assets.particle_defs.insert(id, definition);
    }
    for (ids, end) in [(SOURCE_TRACKS, SOURCE_END), (LINK_TRACKS, LINK_END)] {
        for (id, value) in ids.into_iter().zip(end.to_array()) {
            assets.keyframes.insert(
                id,
                KeyFrameTrack {
                    points: vec![(0.0, 0.0), (ONE_FRAME, value)],
                },
            );
        }
    }
    for (id, value) in [(SOURCE_DAMPING, SOURCE_FACTOR), (LINK_DAMPING, LINK_FACTOR)] {
        assets.keyframes.insert(
            id,
            KeyFrameTrack {
                points: vec![(0.0, value), (ONE_FRAME, value)],
            },
        );
    }
    assets
}

fn spawn(link_bindings: bool) -> App {
    let mut app = App::new();
    app.add_plugins(bevy::asset::AssetPlugin::default())
        .init_asset::<Mesh>()
        .init_asset::<Image>()
        .init_asset::<FfxiParticleMaterial>()
        .init_resource::<ParticleSimulator>()
        .add_message::<SchedulerStageEvent>()
        .add_message::<crate::scheduler_runtime::ParticleSpawnTrace>()
        .add_message::<crate::audio::SfxEvent>()
        .add_systems(Update, spawn_particle_generators);
    let actor = app
        .world_mut()
        .spawn((assets(link_bindings), Transform::IDENTITY))
        .id();
    app.world_mut().write_message(SchedulerStageEvent {
        actor,
        target: Some(actor),
        stage: particle_stage(SOURCE),
        scheduler: SOURCE,
        cutscene_motion: false,
        scheduler_instance: None,
    });
    app.update();
    app
}

#[test]
fn immediate_link_uses_its_own_tracks_and_child_bindings() {
    for bindings in [true, false] {
        let mut app = spawn(bindings);
        let mut sim = app.world_mut().resource_mut::<ParticleSimulator>();
        assert_eq!(sim.generators.len(), SOURCE_AND_LINK);
        assert!(
            sim.generators[0]
                .child_factories
                .iter()
                .all(|f| f.name != LINK),
            "a sec2 0x3C link is one linked generator, not a per-particle child"
        );
        advance_simulator(&mut sim, ONE_FRAME);
        let linked = &mut sim.generators[1];
        assert_eq!(linked.particles.len(), 1);
        linked.particles[0].age_frames = HALF_LIFE;
        let expected = if bindings {
            LINK_END * WORLD_PARTICLE_VEL_BASIS / 2.0
        } else {
            Vec3::ZERO
        };
        assert_eq!(
            particle_draw(linked, &linked.particles[0], &CelestialClock::default()).world,
            expected,
            "the linked definition owns its position channels"
        );
        advance_generator(linked, ONE_FRAME);
        let factor = if bindings { LINK_FACTOR } else { ONE_FRAME };
        assert_eq!(linked.particles[0].vel, Vec3::X * factor);
        let descendants: Vec<_> = linked.child_factories.iter().map(|f| f.name).collect();
        let expected_descendants = if bindings { vec![LINK_CHILD] } else { vec![] };
        assert_eq!(descendants, expected_descendants);
    }
}

#[test]
fn textureless_action_meshes_take_the_untextured_d3m_table() {
    let app = spawn(true);
    let sim = app.world().resource::<ParticleSimulator>();
    assert_eq!(sim.generators.len(), SOURCE_AND_LINK);
    for g in &sim.generators {
        assert_eq!(
            g.draw_path,
            D3mDrawPath::D3mUntextured,
            "{:?}",
            g.def.mesh_id
        );
        assert!(!g.child_factories.is_empty());
        for f in &g.child_factories {
            let ChildPayload::Draw(child) = &f.payload else {
                panic!("{:?} binds a drawable child", f.name);
            };
            assert_eq!(child.draw_path, D3mDrawPath::D3mUntextured, "{:?}", f.name);
        }
    }
}
