use bevy::app::{AppExit, ScheduleRunnerPlugin};
use bevy::audio::AddAudioSource;
use bevy::prelude::*;
use bevy::render::render_resource::*;
use bevy::render::view::screenshot::{save_to_disk, Capturing, Screenshot};
use kuluu_render::camera::{third_person_anchor_y, ChaseCamera, OperatorCamera};
use kuluu_render::components::{IsSelf, WorldEntity};
use kuluu_render::ffxi_actor_render::{
    load_pc, spawn_loaded_actor, tick_ffxi_render_actors, FfxiRenderRoot,
};
use kuluu_render::ffxi_particle_material::FfxiParticleMaterialPlugin;
use kuluu_render::scene::TrackedEntities;
use kuluu_render::scheduler_runtime::{
    tick_active_schedulers, ActionDatRoot, ActiveSchedulers, GlobalEffectDir,
    SchedulerRuntimePlugin, ROUTINE_FPS,
};
use kuluu_render::skinned_ffxi_material::{
    FfxiMaterialPlugin, FfxiSkinRegistry, FfxiSkinnedMaterial, FfxiSkinnedMaterialCache,
};
use kuluu_render::snapshot::EventLog;
use kuluu_snapshot::{EntityKind, ViewerEvent};
use std::sync::Arc;
use std::time::Duration;

#[derive(Resource)]
struct Target(Handle<Image>);
#[derive(Resource)]
struct DemoOptions {
    capture: Option<String>,
}
#[derive(Resource)]
struct Hume(Entity);
#[derive(Default)]
struct Drive {
    frame: u32,
    fired_at: Option<u32>,
    started_at: Option<u32>,
    shots: usize,
}

const DEMO_BACKGROUND: Color = Color::srgb(0.04, 0.04, 0.06);
const IMAGE_WIDTH: u32 = 1000;
const IMAGE_HEIGHT: u32 = 800;
const HUME_M_RACE: u8 = 1;
const DEFAULT_GEAR_SLOTS: std::ops::RangeInclusive<u8> = 1..=5;
const DEFAULT_GEAR_MODEL: u16 = 0;
const DEMO_PLAYER_ID: u32 = 0x0100_7E57;
const ACTOR_SCALE: f32 = 1.0;
const ACTOR_FACING: f32 = 0.0;
const DEFAULT_FACE: u8 = 0;
// Facing 0 points the model down Bevy +X, so the eye sits on that axis to see its front.
const CAMERA_YAW: f32 = std::f32::consts::FRAC_PI_2;
const SETTLE_FRAMES: u32 = 30;
const REPEAT_FRAMES: u32 = 240;
const CAPTURE_SECONDS: [f32; 6] = [0.1, 0.25, 0.5, 1.0, 1.5, 2.0];
const GIVE_UP_SECONDS: u32 = 20;
const GIVE_UP_FRAMES: u32 = GIVE_UP_SECONDS * ROUTINE_FPS as u32;
const KEY_ILLUMINANCE: f32 = 9000.0;
const KEY_LIGHT_FROM: Vec3 = Vec3::new(3.0, 6.0, 4.0);
const AMBIENT_BRIGHTNESS: f32 = 700.0;

fn main() {
    let capture = std::env::args().nth(1);
    let root = ffxi_dat::DatRoot::from_env_or_default()
        .map(Arc::new)
        .expect("level-up demo needs an FFXI install (FFXI_DAT_PATH or the registry default)");
    let mut app = App::new();
    let plugins = DefaultPlugins
        .set(WindowPlugin {
            primary_window: capture.is_none().then(|| Window {
                title: "Kuluu: production level-up effect demo".into(),
                resolution: (IMAGE_WIDTH, IMAGE_HEIGHT).into(),
                ..default()
            }),
            exit_condition: if capture.is_some() {
                bevy::window::ExitCondition::DontExit
            } else {
                bevy::window::ExitCondition::OnAllClosed
            },
            ..default()
        })
        .set(AssetPlugin {
            file_path: format!("{}/../assets", env!("CARGO_MANIFEST_DIR")),
            ..default()
        });
    if capture.is_some() {
        app.add_plugins(plugins.disable::<bevy::winit::WinitPlugin>());
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs_f32(1.0 / ROUTINE_FPS),
        ));
        app.add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f32(
            1.0 / ROUTINE_FPS,
        )));
    } else {
        app.add_plugins(plugins);
    }
    app.insert_resource(ActionDatRoot(Some(root.clone())))
        .insert_resource(kuluu_render::ffxi_actor_render::ActorDatRoot(Some(root)))
        .add_plugins((FfxiMaterialPlugin, FfxiParticleMaterialPlugin))
        .add_audio_source::<kuluu_render::audio::PcmAudio>()
        .init_resource::<kuluu_render::audio::BgmSlots>()
        .init_resource::<kuluu_render::audio::AudioMuteState>()
        // The standalone demo has no session boundary; decoded audio lives until exit.
        .init_resource::<kuluu_render::audio::SfxCache>()
        .add_message::<kuluu_render::snapshot::ToastEvent>()
        .add_message::<kuluu_render::audio::SfxEvent>()
        .init_resource::<kuluu_render::camera::CameraMode>()
        .init_resource::<EventLog>()
        .init_resource::<TrackedEntities>()
        .init_resource::<kuluu_render::snapshot::SceneState>()
        .init_resource::<kuluu_render::EntityTable>()
        .init_resource::<kuluu_render::ffxi_actor_render::SpellSuffixCache>()
        .init_resource::<kuluu_render::combat_stance::EntityMotion>()
        .init_resource::<kuluu_render::combat_stance::RestStance>()
        .init_resource::<kuluu_render::combat_stance::WalkMode>()
        .init_resource::<kuluu_render::combat_stance::SelfMoveIntent>()
        .init_resource::<kuluu_render::scene::Target>()
        .add_plugins(SchedulerRuntimePlugin)
        .insert_resource(ClearColor(DEMO_BACKGROUND))
        .insert_resource(DemoOptions { capture })
        .add_systems(Startup, (setup, spawn_hume))
        .add_systems(
            Update,
            (
                tick_ffxi_render_actors.before(tick_active_schedulers),
                drive.after(kuluu_render::particle_sim::sync_particle_meshes),
                kuluu_render::audio::play_sfx_system
                    .after(kuluu_render::scheduler_runtime::dispatch_sound_stages),
            ),
        );
    app.run();
}

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>, options: Res<DemoOptions>) {
    let chase = ChaseCamera::default();
    let anchor = Vec3::Y * third_person_anchor_y(None);
    let eye = anchor
        + Quat::from_rotation_y(CAMERA_YAW)
            * Quat::from_rotation_x(-chase.pitch)
            * Vec3::Z
            * ChaseCamera::DIST_MAX;
    let camera = (
        Camera3d::default(),
        OperatorCamera,
        Projection::Perspective(PerspectiveProjection {
            fov: kuluu_render::graphics_settings::retail_default_fov_deg().to_radians(),
            ..default()
        }),
        Transform::from_translation(eye).looking_at(anchor, Vec3::Y),
    );
    commands.spawn((
        DirectionalLight {
            illuminance: KEY_ILLUMINANCE,
            ..default()
        },
        Transform::from_translation(KEY_LIGHT_FROM).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: AMBIENT_BRIGHTNESS,
        ..default()
    });
    if options.capture.is_none() {
        commands.spawn(camera);
        return;
    }
    let mut image = Image::new_fill(
        Extent3d {
            width: IMAGE_WIDTH,
            height: IMAGE_HEIGHT,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC | TextureUsages::RENDER_ATTACHMENT;
    let target = images.add(image);
    commands.insert_resource(Target(target.clone()));
    commands.spawn((camera, bevy::camera::RenderTarget::Image(target.into())));
}

// The live scene's shape (scene.rs, ffxi_actor_render.rs spawn_live_actor): a tracked wire
// entity at the actor's feet carrying the self marker, and the posed model root as its child.
#[allow(clippy::too_many_arguments)]
fn spawn_hume(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    mut material_cache: ResMut<FfxiSkinnedMaterialCache>,
    mut registry: ResMut<FfxiSkinRegistry>,
    mut images: ResMut<Assets<Image>>,
    mut tracked: ResMut<TrackedEntities>,
    root: Res<ActionDatRoot>,
) {
    let root = root.0.as_deref().expect("install wired in main");
    let face = kuluu_render::look_resolver::resolve_face(DEFAULT_FACE, HUME_M_RACE);
    let gear: Vec<u32> = face
        .into_iter()
        .chain(DEFAULT_GEAR_SLOTS.filter_map(|slot| {
            kuluu_render::look_resolver::resolve_equipment_model(
                slot,
                DEFAULT_GEAR_MODEL,
                HUME_M_RACE,
            )
        }))
        .collect();
    let loaded = load_pc(root, HUME_M_RACE, false, &gear, None, None, None)
        .unwrap_or_else(|e| panic!("Hume M failed to load: {e}"));
    let wire = commands
        .spawn((
            WorldEntity {
                id: DEMO_PLAYER_ID,
                act_index: 0,
                kind: EntityKind::Pc,
            },
            IsSelf,
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
        ))
        .id();
    let model = spawn_loaded_actor(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut material_cache,
        &mut registry,
        &mut images,
        &loaded,
        Vec3::ZERO,
        ACTOR_FACING,
        ACTOR_SCALE,
        kuluu_render::zone_texture::TextureQuality {
            mipmaps: true,
            anisotropy: kuluu_render::zone_texture::TextureQuality::default().anisotropy,
        },
    );
    commands.entity(model).insert(ChildOf(wire));
    commands.entity(wire).insert(FfxiRenderRoot(model));
    tracked.by_id.insert(DEMO_PLAYER_ID, wire);
    commands.insert_resource(Hume(wire));
}

#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    mut state: Local<Drive>,
    mut events: ResMut<EventLog>,
    hume: Option<Res<Hume>>,
    global: Option<Res<GlobalEffectDir>>,
    q_running: Query<(), With<ActiveSchedulers>>,
    capturing: Query<(), With<Capturing>>,
    target: Option<Res<Target>>,
    options: Res<DemoOptions>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    mut exit: MessageWriter<AppExit>,
) {
    state.frame += 1;
    if keys.is_some_and(|k| k.just_pressed(KeyCode::Escape)) {
        exit.write(AppExit::Success);
    }
    let (Some(hume), Some(_)) = (hume, global) else {
        assert!(
            state.frame < GIVE_UP_FRAMES,
            "global effect dir never loaded"
        );
        return;
    };
    let rearm = state
        .fired_at
        .is_some_and(|at| options.capture.is_none() && state.frame >= at + REPEAT_FRAMES);
    if (state.fired_at.is_none() && state.frame >= SETTLE_FRAMES) || rearm {
        events.push(ViewerEvent::LevelUp {
            player_id: DEMO_PLAYER_ID,
        });
        state.fired_at = Some(state.frame);
        state.started_at = None;
        eprintln!("level-up pushed at update {}", state.frame);
        return;
    }
    if state.fired_at.is_none() {
        return;
    }
    if state.started_at.is_none() {
        if q_running.get(hume.0).is_ok() {
            state.started_at = Some(state.frame);
            eprintln!("level-up routine running from update {}", state.frame);
        } else {
            assert!(
                state.frame < GIVE_UP_FRAMES,
                "the level-up routine never started"
            );
            return;
        }
    }
    let (Some(start), Some(target), Some(dir)) =
        (state.started_at, target, options.capture.as_ref())
    else {
        return;
    };
    let routine_frame = state.frame - start;
    if let Some(&seconds) = CAPTURE_SECONDS.get(state.shots) {
        if routine_frame as f32 >= seconds * ROUTINE_FPS {
            std::fs::create_dir_all(dir).unwrap();
            let path = format!("{dir}/levelup-hume-{:04}ms.png", (seconds * 1000.0) as u32);
            eprintln!("capture {path} at routine frame {routine_frame}");
            commands
                .spawn(Screenshot::image(target.0.clone()))
                .observe(save_to_disk(path));
            state.shots += 1;
        }
    } else if capturing.is_empty() {
        exit.write(AppExit::Success);
    }
}
