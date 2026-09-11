use bevy::app::{AppExit, ScheduleRunnerPlugin};
use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::light::CascadeShadowConfigBuilder;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::batching::gpu_preprocessing::{GpuPreprocessingMode, GpuPreprocessingSupport};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{save_to_disk, Capturing, Screenshot};
use kuluu_render::ffxi_zone_material::{
    FfxiZoneMaterial, FfxiZoneMaterialKey, FfxiZoneMaterialPlugin,
};
use kuluu_render::skinned_ffxi_material::*;
use kuluu_render::weather::ZoneDirectionalLighting;
use std::time::Duration;

const IMAGE_SIDE: u32 = 512;
const CAMERA_DISTANCE: f32 = 7.0;
const CAMERA_SPAN: f32 = 5.0;
const RECEIVER_SIDE: f32 = 4.0;
const RECEIVER_THICKNESS: f32 = 0.2;
const BLOCKER_SIDE: f32 = 0.7;
const BLOCKER_DEPTH: f32 = 0.15;
const BLOCKER_DISTANCE: f32 = 1.0;
const BLOCKER_OFFSET: f32 = 0.5;
const LIGHT_DIRECTION: Vec3 = Vec3::new(1.0, 1.0, 3.0);
const LIGHT_LUX: f32 = 12000.0;
const SHADOW_DEPTH_BIAS: f32 = 0.02;
const SHADOW_NORMAL_BIAS: f32 = 0.1;
const SHADOW_DISTANCE: f32 = 20.0;
const AMBIENT: f32 = 0.08;
const DIFFUSE: f32 = 0.25;
// The point modes light the plate from a single lamp on the camera side of the blocker.
const POINT_LIGHT_DISTANCE: f32 = 2.5;
const POINT_LIGHT_RANGE: f32 = 10.0;
// FAITHFUL_LIGHT_INTENSITY (zone_point_lights.rs) x DIFFUSE, so the --zone plate's
// clustered feed lands at the same brightness as the skinned plate's uniform slot.
const POINT_LIGHT_INTENSITY: f32 = 6250.0;
// Flat falloff (const term only) keeps the lit/shadowed contrast a single step.
const POINT_LIGHT_CONST_ATTEN: f32 = 1.0;
const ALBEDO: f32 = 0.6;
const CAPTURE_FRAME: u32 = 80;
const EXIT_DELAY: u32 = 10;
const FRAME_MILLIS: u64 = 16;

#[derive(Resource)]
struct Run {
    mode: String,
    out: String,
    frame: u32,
    zone: bool,
    secondary: bool,
    realistic: bool,
    target: Handle<Image>,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args
        .next()
        .expect("mode: off, active, inactive, opposing, valid, valid-off, point, point-off, point-unshadowed");
    let out = args.next().expect("output PNG path");
    let flags: Vec<String> = args.collect();
    let mut app = App::new();
    // The fixture owns its entities and GPU buffers until this bounded process exits.
    app.insert_resource(Run {
        mode,
        out,
        frame: 0,
        zone: flags.iter().any(|flag| flag == "--zone"),
        secondary: flags.iter().any(|flag| flag == "--dir1"),
        realistic: flags.iter().any(|flag| flag == "--realistic"),
        target: Handle::default(),
    })
    .insert_resource(ClearColor(Color::BLACK))
    .add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>(),
    )
    .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(
        FRAME_MILLIS,
    )))
    .add_plugins((FfxiMaterialPlugin, FfxiZoneMaterialPlugin))
    .add_systems(Startup, setup)
    .add_systems(Update, capture);
    if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
        render_app.insert_resource(GpuPreprocessingSupport {
            max_supported_mode: GpuPreprocessingMode::None,
        });
        render_app.add_systems(
            bevy::render::RenderStartup,
            (|mut support: ResMut<GpuPreprocessingSupport>| {
                support.max_supported_mode = GpuPreprocessingMode::None;
            })
            .after(bevy::render::init_gpu_resource::<GpuPreprocessingSupport>),
        );
    }
    app.run();
}

fn skinned_cuboid(size: Vec3) -> Mesh {
    let mut mesh = Mesh::from(Cuboid::from_size(size));
    let positions = mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap().clone();
    let normals = mesh.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap().clone();
    let count = mesh.count_vertices();
    mesh.insert_attribute(ATTR_POSITION0, positions);
    mesh.insert_attribute(ATTR_POSITION1, vec![[0.0; 3]; count]);
    mesh.insert_attribute(ATTR_NORMAL0, normals);
    mesh.insert_attribute(ATTR_NORMAL1, vec![[0.0; 3]; count]);
    mesh.insert_attribute(ATTR_JOINT_WEIGHT, vec![1.0; count]);
    mesh.insert_attribute(ATTR_JOINT0, vec![0u32; count]);
    mesh.insert_attribute(ATTR_JOINT1, vec![0u32; count]);
    mesh.insert_attribute(ATTR_COLOR, vec![[1.0; 4]; count]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0; 4]; count]);
    mesh
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    mut run: ResMut<Run>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut registry: ResMut<FfxiSkinRegistry>,
    mut zone_materials: ResMut<Assets<FfxiZoneMaterial>>,
    mut zone_lighting: ResMut<ZoneDirectionalLighting>,
) {
    let mut target = Image::new_fill(
        Extent3d {
            width: IMAGE_SIDE,
            height: IMAGE_SIDE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    target.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC | TextureUsages::RENDER_ATTACHMENT;
    run.target = images.add(target);
    commands.spawn((
        Camera3d::default(),
        RenderTarget::Image(run.target.clone().into()),
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: bevy::camera::ScalingMode::FixedVertical {
                viewport_height: CAMERA_SPAN,
            },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, 0.0, CAMERA_DISTANCE).looking_at(Vec3::ZERO, Vec3::Y),
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Msaa::Off,
    ));

    let to_light = LIGHT_DIRECTION.normalize();
    let point = run.mode.starts_with("point");
    let point_pos = Vec3::new(BLOCKER_OFFSET, BLOCKER_OFFSET, POINT_LIGHT_DISTANCE);
    if point {
        commands.spawn((
            PointLight {
                intensity: POINT_LIGHT_INTENSITY,
                range: POINT_LIGHT_RANGE,
                shadow_maps_enabled: run.mode != "point-unshadowed",
                shadow_depth_bias: SHADOW_DEPTH_BIAS,
                shadow_normal_bias: SHADOW_NORMAL_BIAS,
                ..default()
            },
            Transform::from_translation(point_pos),
        ));
    }
    let primary = DirectionalLight {
        illuminance: LIGHT_LUX,
        shadow_maps_enabled: true,
        shadow_depth_bias: SHADOW_DEPTH_BIAS,
        shadow_normal_bias: SHADOW_NORMAL_BIAS,
        ..default()
    };
    let cascade = CascadeShadowConfigBuilder {
        maximum_distance: SHADOW_DISTANCE,
        ..default()
    }
    .build();
    if !point {
        commands.spawn((
            primary,
            Transform::from_translation(to_light).looking_at(Vec3::ZERO, Vec3::Y),
            cascade.clone(),
        ));
    }
    if matches!(run.mode.as_str(), "inactive" | "opposing") {
        commands.spawn((
            DirectionalLight {
                illuminance: if run.mode == "inactive" {
                    0.0
                } else {
                    LIGHT_LUX
                },
                ..primary
            },
            Transform::from_translation(-to_light).looking_at(Vec3::ZERO, Vec3::Y),
            cascade,
        ));
    }
    let skin = registry.alloc_skin();
    registry.skin_mut(skin).lighting = FfxiLightingUniform {
        ambient: Vec4::splat(AMBIENT),
        dir0_dir: (-to_light).extend(0.0),
        dir0_color: if point {
            Vec4::ZERO
        } else {
            Vec4::new(DIFFUSE, DIFFUSE, DIFFUSE, 1.0)
        },
        dir1_dir: Vec4::ZERO,
        dir1_color: Vec4::ZERO,
        ..default()
    };
    if point {
        let lighting = &mut registry.skin_mut(skin).lighting;
        lighting.point_pos[0] = point_pos.extend(0.0);
        lighting.point_color[0] = Vec4::new(DIFFUSE, DIFFUSE, DIFFUSE, POINT_LIGHT_RANGE);
        lighting.point_atten[0] = Vec4::new(POINT_LIGHT_CONST_ATTEN, 0.0, 0.0, 0.0);
    }
    if run.secondary {
        let lighting = &mut registry.skin_mut(skin).lighting;
        lighting.dir1_dir = lighting.dir0_dir;
        lighting.dir1_color = lighting.dir0_color;
        lighting.dir0_dir = Vec4::ZERO;
        lighting.dir0_color = Vec4::ZERO;
    }
    *zone_lighting = ZoneDirectionalLighting {
        valid: true,
        ambient_landscape: Vec3::splat(AMBIENT),
        sun_dir: if run.secondary { Vec3::ZERO } else { to_light },
        sun_color: Vec3::splat(DIFFUSE),
        sun_k: if run.secondary || point { 0.0 } else { 1.0 },
        moon_dir: if run.secondary { to_light } else { Vec3::ZERO },
        moon_color: Vec3::splat(DIFFUSE),
        moon_k: if run.secondary { 1.0 } else { 0.0 },
        ..default()
    };
    let zone_material = zone_materials.add(FfxiZoneMaterial::new(
        None,
        FfxiMaterialFlags { flags: Vec4::ZERO },
        Vec4::splat(ALBEDO),
        Vec4::ZERO,
        AlphaMode::Opaque,
        FfxiZoneMaterialKey::LEGACY,
    ));
    let material = materials.add(FfxiSkinnedMaterial {
        base_color_texture: None,
    });
    let receive = !matches!(run.mode.as_str(), "off" | "valid-off" | "point-off");
    let front = run.mode.starts_with("valid") || point;
    for (size, position) in [
        (
            Vec3::new(RECEIVER_SIDE, RECEIVER_SIDE, RECEIVER_THICKNESS),
            Vec3::ZERO,
        ),
        (
            Vec3::new(BLOCKER_SIDE, BLOCKER_SIDE, BLOCKER_DEPTH),
            Vec3::new(
                BLOCKER_OFFSET,
                BLOCKER_OFFSET,
                if front {
                    BLOCKER_DISTANCE
                } else {
                    -BLOCKER_DISTANCE
                },
            ),
        ),
    ] {
        if run.zone {
            commands.spawn((
                Mesh3d(meshes.add(skinned_cuboid(size))),
                MeshMaterial3d(zone_material.clone()),
                Transform::from_translation(position),
            ));
            continue;
        }
        let slot = registry.alloc_instance(FfxiInstance {
            flags: Vec4::new(
                0.0,
                if run.realistic { 1.0 } else { 0.0 },
                if receive { 1.0 } else { 0.0 },
                0.0,
            ),
            tint: Vec4::splat(ALBEDO),
            skin_slot: skin,
        });
        commands.spawn((
            Mesh3d(meshes.add(skinned_cuboid(size))),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(position),
            MeshTag(slot),
            FfxiInstanceSlot(slot),
        ));
    }
}

fn capture(
    mut commands: Commands,
    mut run: ResMut<Run>,
    pending: Query<Entity, With<Capturing>>,
    mut exit: MessageWriter<AppExit>,
) {
    run.frame += 1;
    if run.frame == CAPTURE_FRAME {
        commands
            .spawn(Screenshot::image(run.target.clone()))
            .observe(save_to_disk(run.out.clone()));
    }
    if run.frame > CAPTURE_FRAME + EXIT_DELAY && pending.is_empty() {
        exit.write(AppExit::Success);
    }
}
