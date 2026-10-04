use super::*;
use std::path::PathBuf;

use bevy::app::ScheduleRunnerPlugin;
use bevy::camera::RenderTarget;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{save_to_disk, Capturing, Screenshot};
use bevy::winit::WinitPlugin;

const CAPTURE_DIR_ENV: &str = "KULUU_PARTICLE_CLEANUP_CAPTURE_DIR";
const BEFORE_FILE: &str = "before-cleanup.png";
const RETAINED_FILE: &str = "after-unrelated-cleanup.png";
const REMOVED_FILE: &str = "after-parent-despawn.png";
const SIZE: u32 = 512;
const BASELINE_FRAME: u32 = 60;
const EXPIRE_FRAME: u32 = 90;
const RETAINED_FRAME: u32 = 120;
const DESPAWN_FRAME: u32 = 150;
const REMOVED_FRAME: u32 = 180;
const EXIT_FRAME: u32 = 210;
const MARKER_OFFSET: f32 = 1.0;
const MARKER_RADIUS: f32 = 0.5;
const CAMERA_DISTANCE: f32 = 6.0;
const ONE_FRAME: f32 = 1.0;

#[derive(Resource)]
struct CaptureTarget {
    image: Handle<Image>,
    output: PathBuf,
    expired: Entity,
    parent: Entity,
    child: Entity,
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FfxiParticleMaterial>>,
    mut sim: ResMut<ParticleSimulator>,
) {
    let mut image = Image::new_fill(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC | TextureUsages::RENDER_ATTACHMENT;
    let target = images.add(image);
    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        RenderTarget::Image(target.clone().into()),
        Transform::from_xyz(0.0, 0.0, CAMERA_DISTANCE).looking_at(Vec3::ZERO, Vec3::Y),
        OperatorCamera,
        Msaa::Off,
    ));

    let mut definition = def(f32::INFINITY, ROUTINE_FPS, 0);
    definition.base_position = Vec3::ZERO.to_array();
    definition.init_velocity = Vec3::ZERO.to_array();
    definition.init_scale = Vec3::ONE.to_array();
    definition.init_color = Vec4::ONE.to_array();
    definition.fog_enabled = false;
    let mut expired = live(definition, 0.0);
    expired.auto_run = true;
    expired.emit_culled = true;
    let mut child = live(definition, 0.0);
    let mut parent = live(definition, 0.0);
    parent.def.init_color = Vec4::new(1.0, 0.0, 0.0, 1.0).to_array();
    parent.origin = Vec3::X * MARKER_OFFSET;
    for g in [&mut child, &mut parent] {
        prime(g);
        advance_generator(g, ONE_FRAME);
        g.stopped = true;
    }
    child.particles[0].pos = Vec3::NEG_X * MARKER_OFFSET * 2.0;
    child.parent = Some((2, parent.particles[0].id));
    parent.particles[0].child_gens.push(1);
    for g in [&mut expired, &mut child, &mut parent] {
        g.template.positions = vec![
            Vec3::new(-MARKER_RADIUS, -MARKER_RADIUS, 0.0),
            Vec3::new(MARKER_RADIUS, -MARKER_RADIUS, 0.0),
            Vec3::new(0.0, MARKER_RADIUS, 0.0),
        ];
        g.orientation = Some(Quat::IDENTITY);
        g.bound_radius = MARKER_RADIUS;
        g.draw_path = D3mDrawPath::Untextured;
        g.mesh = meshes.add(empty_mesh());
        let material = materials.add(FfxiParticleMaterial::for_def(
            &g.def,
            None,
            NO_DAT_ORDER,
            g.draw_path,
        ));
        g.entity = commands
            .spawn((
                InGameEntity,
                Mesh3d(g.mesh.clone()),
                MeshMaterial3d(material),
                Transform::IDENTITY,
                Visibility::Inherited,
                bevy::camera::visibility::NoFrustumCulling,
                bevy::light::NotShadowCaster,
                bevy::light::NotShadowReceiver,
            ))
            .id();
    }
    commands.insert_resource(CaptureTarget {
        image: target,
        output: std::env::var_os(CAPTURE_DIR_ENV).unwrap().into(),
        expired: expired.entity,
        parent: parent.entity,
        child: child.entity,
    });
    sim.generators = vec![expired, child, parent];
}

fn capture(
    mut commands: Commands,
    mut frame: Local<u32>,
    target: Res<CaptureTarget>,
    mut sim: ResMut<ParticleSimulator>,
    capturing: Query<(), With<Capturing>>,
    mut exit: MessageWriter<bevy::app::AppExit>,
) {
    *frame += 1;
    if *frame == EXPIRE_FRAME {
        sim.generators
            .iter_mut()
            .find(|g| g.entity == target.expired)
            .unwrap()
            .stopped = true;
    }
    if *frame == DESPAWN_FRAME {
        commands.entity(target.parent).despawn();
    }
    let filename = match *frame {
        BASELINE_FRAME => Some(BEFORE_FILE),
        RETAINED_FRAME => Some(RETAINED_FILE),
        REMOVED_FRAME => Some(REMOVED_FILE),
        _ => None,
    };
    if let Some(filename) = filename {
        println!(
            "{filename}: generators={} child_alive={}",
            sim.generators.len(),
            sim.generators.iter().any(|g| g.entity == target.child)
        );
        commands
            .spawn(Screenshot::image(target.image.clone()))
            .observe(save_to_disk(target.output.join(filename)));
    }
    if *frame >= EXIT_FRAME && capturing.is_empty() {
        exit.write(bevy::app::AppExit::Success);
    }
}

#[test]
#[ignore = "requires a graphics adapter and an explicit capture output directory"]
fn captures_child_lifetime_through_production_rendering() {
    let output = PathBuf::from(std::env::var_os(CAPTURE_DIR_ENV).expect("set capture directory"));
    std::fs::create_dir_all(&output).unwrap();
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    ..default()
                })
                .disable::<WinitPlugin>(),
        )
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
            1.0 / f64::from(ROUTINE_FPS),
        )))
        .add_plugins(crate::ffxi_particle_material::FfxiParticleMaterialPlugin)
        .init_resource::<ParticleSimulator>()
        .init_resource::<crate::graphics_settings::GraphicsSettings>()
        .add_message::<crate::audio::SfxEvent>()
        .add_message::<crate::scheduler_runtime::ParticleSpawnTrace>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (capture, tick_particle_simulator, sync_particle_meshes).chain(),
        )
        .run();
    for filename in [BEFORE_FILE, RETAINED_FILE, REMOVED_FILE] {
        assert!(output.join(filename).metadata().unwrap().len() > 0);
    }
}
