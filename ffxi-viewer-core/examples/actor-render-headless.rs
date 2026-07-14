use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Capturing, Screenshot};
use ffxi_viewer_core::ffxi_actor_render::{
    inputs_for_pose, load_npc, load_pc, spawn_loaded_actor, tick_ffxi_render_actors,
    FfxiRenderActor, PoseState, FRAME_RATE,
};
use ffxi_viewer_core::skinned_ffxi_material::{FfxiMaterialPlugin, FfxiSkinnedMaterial};
use std::env;

#[derive(Clone)]
enum Subject {
    Npc(u32),
    Pc(u8, Vec<u32>),
}

#[derive(Resource, Clone)]
struct Params {
    subject: Subject,
    pose: PoseState,
    engaged: bool,
    out: String,
    target_frame: f32,
    cap: u32,
    yaw: f32,
    cam_dist: f32,
    cam_height: f32,

    scale: f32,

    ground: bool,

    autoframe: bool,

    realistic: bool,

    shadowtest: bool,
}

#[derive(Resource, Default)]
struct FrameCount(u32);
#[derive(Resource, Default)]
struct Shot(bool);

#[derive(Resource)]
struct SubjectBounds {
    min: Vec3,
    max: Vec3,
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let params = parse_params(&args);

    App::new()
        .insert_resource(params)
        .init_resource::<FrameCount>()
        .init_resource::<Shot>()
        .insert_resource(ClearColor(Color::srgb(0.10, 0.10, 0.13)))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                resolution: (900u32, 1100u32).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FfxiMaterialPlugin)
        .add_systems(Startup, (setup_scene, spawn_subject))
        .add_systems(
            Update,
            (
                reframe_camera,
                set_inputs,
                apply_realistic_flag,
                tick_ffxi_render_actors,
                capture,
            )
                .chain(),
        )
        .run();
}

fn parse_params(args: &[String]) -> Params {
    let mut subject = Subject::Npc(2056);
    let mut pose = PoseState::Idle;
    let mut engaged = false;
    let mut out = "/tmp/actor.png".to_string();
    let mut target_frame = 0.0f32;
    let mut cap = 90u32;
    let mut yaw = 0.0f32;
    let mut cam_dist = 3.4f32;
    let mut cam_height = 1.1f32;
    let mut scale = 1.0f32;
    let mut ground = false;
    let mut autoframe = false;
    let mut realistic = false;
    let mut shadowtest = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "npc" => {
                if let Some(id) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                    subject = Subject::Npc(id);
                }
                i += 2;
            }
            "pc" => {
                let race: u8 = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(1);

                let mut equip = Vec::new();
                let mut j = i + 2;
                while j < args.len() && !args[j].starts_with("--") {
                    if let Ok(v) = args[j].parse() {
                        equip.push(v);
                    }
                    j += 1;
                }
                subject = Subject::Pc(race, equip);
                i = j;
            }
            "--pose" => {
                if let Some(p) = args.get(i + 1).and_then(|s| PoseState::from_name(s)) {
                    pose = p;
                }
                i += 2;
            }
            "--engaged" => {
                engaged = true;
                i += 1;
            }
            "--out" => {
                if let Some(v) = args.get(i + 1) {
                    out = v.clone();
                }
                i += 2;
            }
            "--frame" => {
                target_frame = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                i += 2;
            }
            "--cap" => {
                cap = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(90);
                i += 2;
            }
            "--yaw" => {
                yaw = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                i += 2;
            }
            "--cam" => {
                cam_dist = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(3.4);
                i += 2;
            }
            "--cy" => {
                cam_height = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(1.1);
                i += 2;
            }
            "--scale" => {
                scale = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(1.0);
                i += 2;
            }
            "--ground" => {
                ground = true;
                i += 1;
            }
            "--autoframe" => {
                autoframe = true;
                i += 1;
            }
            "--realistic" => {
                realistic = true;
                i += 1;
            }
            "--shadowtest" => {
                shadowtest = true;
                ground = true;
                i += 1;
            }
            _ => i += 1,
        }
    }

    Params {
        subject,
        pose,
        engaged,
        out,
        target_frame,
        cap,
        yaw,
        cam_dist,
        cam_height,
        scale,
        ground,
        autoframe,
        realistic,
        shadowtest,
    }
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut std_materials: ResMut<Assets<StandardMaterial>>,
    params: Res<Params>,
) {
    let look_y = params.cam_height;
    let d = params.cam_dist;

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(d * 0.45, look_y + d * 0.25, d)
            .looking_at(Vec3::new(0.0, look_y, 0.0), Vec3::Y),
    ));
    if params.shadowtest {
        commands.spawn((
            DirectionalLight {
                illuminance: 11000.0,
                shadow_maps_enabled: true,
                ..default()
            },
            Transform::from_xyz(4.0, 7.0, 2.0).looking_at(Vec3::ZERO, Vec3::Y),
        ));

        commands.insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 150.0,
            ..default()
        });
    } else {
        commands.spawn((
            DirectionalLight {
                illuminance: 9000.0,
                ..default()
            },
            Transform::from_xyz(3.0, 6.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
        ));
        commands.insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 700.0,
            ..default()
        });
    }
    if params.ground {
        let base_color = if params.shadowtest {
            Color::srgb(0.55, 0.56, 0.6)
        } else {
            Color::srgb(0.3, 0.32, 0.36)
        };
        commands.spawn((
            Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, 8.0))),
            MeshMaterial3d(std_materials.add(StandardMaterial {
                base_color,
                perceptual_roughness: 1.0,
                ..default()
            })),
            Transform::from_xyz(0.0, 0.0, 0.0),
        ));
    }
}

fn spawn_subject(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    mut images: ResMut<Assets<Image>>,
    params: Res<Params>,
) {
    let loaded = match &params.subject {
        Subject::Npc(id) => load_npc(*id),
        Subject::Pc(race, equip) => load_pc(*race, equip, None, None),
    };
    match loaded {
        Ok(loaded) => {
            eprintln!(
                "loaded: skeleton joints={} skel_meshes={}",
                loaded.skeleton.joints.len(),
                loaded.skel_meshes.len(),
            );
            if let Some((min, max)) = loaded.bind_pose_bounds(params.yaw, params.scale) {
                eprintln!(
                    "geometry bounds (bevy): min=({:.3},{:.3},{:.3}) max=({:.3},{:.3},{:.3})",
                    min.x, min.y, min.z, max.x, max.y, max.z
                );
                commands.insert_resource(SubjectBounds { min, max });
            }
            spawn_loaded_actor(
                &mut commands,
                &mut meshes,
                &mut materials,
                &mut images,
                &loaded,
                Vec3::ZERO,
                params.yaw,
                params.scale,
                ffxi_viewer_core::zone_texture::TextureQuality {
                    mipmaps: true,
                    anisotropy: 8,
                },
            );
        }
        Err(e) => eprintln!("load failed: {e}"),
    }
}

fn reframe_camera(
    mut commands: Commands,
    params: Res<Params>,
    bounds: Option<Res<SubjectBounds>>,
    mut q_cam: Query<&mut Transform, With<Camera3d>>,
) {
    if !params.autoframe {
        return;
    }
    let Some(bounds) = bounds else {
        return;
    };
    let size = bounds.max - bounds.min;
    let center = (bounds.min + bounds.max) * 0.5;
    let extent = size.max_element().max(0.2);

    let d = extent / (std::f32::consts::FRAC_PI_8).tan() * 1.4;
    let aim = center + Vec3::new(0.0, size.y * 0.5, 0.0);
    let dir = Vec3::new(0.45, 0.35, 1.0).normalize();
    for mut t in &mut q_cam {
        *t = Transform::from_translation(aim + dir * d).looking_at(aim, Vec3::Y);
    }
    commands.remove_resource::<SubjectBounds>();
}

fn set_inputs(params: Res<Params>, mut q: Query<&mut FfxiRenderActor>) {
    let inputs = inputs_for_pose(params.pose, params.engaged);
    for mut actor in &mut q {
        actor.inputs = inputs;
    }
}

fn apply_realistic_flag(params: Res<Params>, mut materials: ResMut<Assets<FfxiSkinnedMaterial>>) {
    let realistic = if params.realistic { 1.0 } else { 0.0 };
    let receive = if params.shadowtest { 1.0 } else { 0.0 };
    let ids: Vec<_> = materials.ids().collect();
    for id in ids {
        if let Some(mut m) = materials.get_mut(id) {
            if m.material_flags.flags.y != realistic {
                m.material_flags.flags.y = realistic;
            }
            if m.material_flags.flags.z != receive {
                m.material_flags.flags.z = receive;
            }
        }
    }
}

fn capture(
    mut commands: Commands,
    mut fc: ResMut<FrameCount>,
    mut shot: ResMut<Shot>,
    params: Res<Params>,
    q_cap: Query<Entity, With<Capturing>>,
    q_actor: Query<&FfxiRenderActor>,
    mut exit: MessageWriter<AppExit>,
) {
    fc.0 += 1;

    let near_target = params.target_frame <= 0.0
        || q_actor
            .iter()
            .any(|a| a.last_frame >= params.target_frame - 0.5);

    if !shot.0 && fc.0 >= params.cap && near_target {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(params.out.clone()));
        shot.0 = true;
        let joints = q_actor
            .iter()
            .next()
            .map(|_| "actor present")
            .unwrap_or("NO actor");
        eprintln!(
            "captured -> {} (frame {:.1}, {})",
            params.out,
            q_actor.iter().next().map(|a| a.last_frame).unwrap_or(0.0),
            joints,
        );
    }

    if shot.0 && q_cap.is_empty() && fc.0 >= params.cap + 5 {
        let written = std::path::Path::new(&params.out).exists();
        if written || fc.0 >= params.cap + 120 {
            exit.write(AppExit::Success);
        }
    }

    if !shot.0 && fc.0 >= params.cap + (FRAME_RATE as u32) * 20 {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(params.out.clone()));
        shot.0 = true;
    }
}
