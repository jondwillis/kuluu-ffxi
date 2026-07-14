use bevy::prelude::*;
use ffxi_viewer_core::ffxi_actor_render::{
    inputs_for_pose, load_npc, load_pc, spawn_loaded_actor, tick_ffxi_render_actors,
    FfxiRenderActor, PoseState,
};
use ffxi_viewer_core::skinned_ffxi_material::FfxiMaterialPlugin;
use std::env;

#[derive(Resource, Clone)]
enum Subject {
    Npc(u32),
    Pc(u8, Vec<u32>),
}

#[derive(Resource)]
struct ViewState {
    pose: PoseState,
    engaged: bool,
}

#[derive(Component)]
struct StatusText;

#[derive(Component)]
struct OrbitCam {
    distance: f32,
    yaw: f32,
    pitch: f32,
    target_y: f32,
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let subject = parse_subject(&args)
        .unwrap_or(Subject::Npc(ffxi_viewer_core::look_resolver::npc_dat_id(2)));

    App::new()
        .insert_resource(subject)
        .insert_resource(ViewState {
            pose: PoseState::Idle,
            engaged: false,
        })
        .insert_resource(ClearColor(Color::srgb(0.10, 0.10, 0.13)))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "ffxi actor-view (faithful)".into(),
                resolution: (1280u32, 960u32).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FfxiMaterialPlugin)
        .add_systems(Startup, (setup_scene, spawn_subject, spawn_ui))
        .add_systems(
            Update,
            (
                orbit_camera,
                handle_input,
                tick_ffxi_render_actors,
                update_status,
            ),
        )
        .run();
}

fn parse_subject(args: &[String]) -> Option<Subject> {
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "npc" => {
                let id: u32 = args.get(i + 1)?.parse().ok()?;
                return Some(Subject::Npc(id));
            }
            "pc" => {
                let race: u8 = args.get(i + 1)?.parse().ok()?;
                let equip: Vec<u32> = args[i + 2..]
                    .iter()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                return Some(Subject::Pc(race, equip));
            }
            _ => i += 1,
        }
    }
    None
}

fn setup_scene(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 1.0, 4.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        OrbitCam {
            distance: 4.0,
            yaw: 0.0,
            pitch: 0.15,
            target_y: 1.0,
        },
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 9000.0,
            ..default()
        },
        Transform::from_xyz(3.0, 6.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 600.0,
        ..default()
    });
}

fn spawn_subject(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ffxi_viewer_core::skinned_ffxi_material::FfxiSkinnedMaterial>>,
    mut images: ResMut<Assets<Image>>,
    subject: Res<Subject>,
) {
    let loaded = match &*subject {
        Subject::Npc(id) => load_npc(*id),
        Subject::Pc(race, equip) => load_pc(0, *race, equip, None, None),
    };
    match loaded {
        Ok(loaded) => {
            eprintln!(
                "loaded: skeleton joints={} skel_meshes={}",
                loaded.skeleton.joints.len(),
                loaded.skel_meshes.len()
            );
            spawn_loaded_actor(
                &mut commands,
                &mut meshes,
                &mut materials,
                &mut images,
                &loaded,
                Vec3::ZERO,
                0.0,
                1.0,
                ffxi_viewer_core::zone_texture::TextureQuality {
                    mipmaps: true,
                    anisotropy: 8,
                },
            );
        }
        Err(e) => eprintln!("load failed: {e}"),
    }
}

fn spawn_ui(mut commands: Commands) {
    commands.spawn((
        Text::new("loading..."),
        TextFont {
            font_size: 18.0,
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(8.0),
            ..default()
        },
        StatusText,
    ));
}

fn orbit_camera(time: Res<Time>, mut q: Query<(&mut Transform, &mut OrbitCam)>) {
    for (mut t, mut cam) in &mut q {
        cam.yaw += time.delta_secs() * 0.4;
        let (sy, cy) = cam.yaw.sin_cos();
        let (sp, cp) = cam.pitch.sin_cos();
        let target = Vec3::new(0.0, cam.target_y, 0.0);
        t.translation = target
            + Vec3::new(
                cam.distance * cp * sy,
                cam.distance * sp,
                cam.distance * cp * cy,
            );
        t.look_at(target, Vec3::Y);
    }
}

fn handle_input(keys: Res<ButtonInput<KeyCode>>, mut view: ResMut<ViewState>) {
    let map = [
        (KeyCode::Digit1, PoseState::Idle),
        (KeyCode::Digit2, PoseState::Walk),
        (KeyCode::Digit3, PoseState::Run),
        (KeyCode::Digit4, PoseState::StrafeLeft),
        (KeyCode::Digit5, PoseState::StrafeRight),
        (KeyCode::Digit6, PoseState::Back),
        (KeyCode::Digit7, PoseState::Sit),
        (KeyCode::Digit8, PoseState::Kneel),
        (KeyCode::Digit9, PoseState::Heal),
        (KeyCode::Digit0, PoseState::Dead),
    ];
    for (key, pose) in map {
        if keys.just_pressed(key) {
            view.pose = pose;
        }
    }
    if keys.just_pressed(KeyCode::KeyE) {
        view.engaged = !view.engaged;
    }
}

fn update_status(
    view: Res<ViewState>,
    mut q_actor: Query<&mut FfxiRenderActor>,
    mut q_text: Query<&mut Text, With<StatusText>>,
) {
    let inputs = inputs_for_pose(view.pose, view.engaged);
    let mut clip = String::from("-");
    let mut frame = 0.0;
    for mut actor in &mut q_actor {
        actor.inputs = inputs;
        clip = actor
            .last_clip
            .map(|c| c.as_str())
            .unwrap_or_else(|| "-".into());
        frame = actor.last_frame;
    }
    for mut text in &mut q_text {
        **text = format!(
            "pose: {}  engaged: {}  clip: {}  frame: {:.1}\n[1]idle [2]walk [3]run [4]strafeL [5]strafeR [6]back [7]sit [8]kneel [9]heal [0]dead [E]engage",
            view.pose.label(),
            view.engaged,
            clip,
            frame,
        );
    }
}
