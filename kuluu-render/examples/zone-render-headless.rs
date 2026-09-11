use bevy::app::AppExit;
use bevy::camera::ScalingMode;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Capturing, Screenshot};
use ffxi_dat::weather::collect_zone_weather_sets;
use kuluu_render::camera::OperatorCamera;
use kuluu_render::dat_mmb::{
    process_load_mmb_requests, LoadMmbRequest, MmbHandleCache, MmbLoadInFlight, MmbLoadQueue,
    MmbParseCache, MmbTexPools,
};
use kuluu_render::dat_mzb::{
    kick_load_mzb_tasks, poll_load_mzb_tasks, scroll_water_uv, spawn_zone_water, DrawDistance,
    LoadMzbInFlight, LoadMzbRequest, MzbCollisionGeometry, PendingWaterSpawns, ZoneGeomCache,
    ZoneGeomMode, ZoneWaterMaterial,
};
use kuluu_render::ffxi_zone_material::FfxiZoneMaterialPlugin;
use kuluu_render::scene::TrackedEntities;
use kuluu_render::snapshot::ToastEvent;
use kuluu_render::sun_moon::IsSun;
use kuluu_render::vana_time::VanaClock;
use kuluu_render::weather::{apply_zone_weather, sample_zone_weather, ZoneWeather};
use kuluu_render::SceneState;
use std::env;

// --sky reuses the REAL client sky pipeline (no reimplementation): the skybox
// gradient dome, the sun/moon discs + directional lights, and the lens flare,
// all driven by sun_moon_system exactly as ViewerCorePlugin wires them. Lets us
// tune the daytime sun glow + warm tone deterministically at a fixed --hour.
use bevy::post_process::bloom::{Bloom, BloomPrefilter};
use kuluu_render::graphics::settings::GraphicsSettings;
use kuluu_render::lens_flare::LensFlarePlugin;
use kuluu_render::moon_material::{MoonMaterial, MoonMaterialPlugin};
use kuluu_render::skybox::SkyboxPlugin;
use kuluu_render::sun_moon::{spawn_sun_and_moon, sun_moon_system, VanaSky};
use kuluu_render::weather::ZoneDirectionalLighting;

#[derive(Resource, Clone)]
struct P {
    file_id: u32,
    out: String,
    cy: f32,
    cap: u32,
    mode: ZoneGeomMode,
    amb: f32,
    sun: f32,
    cam: Option<Vec3>,
    tgt: Vec3,
    fog: bool,
    sky: bool,
    /// Draw-distance override (the graphics-menu value, not the far plane), so a
    /// verification run can exercise the sky at more than one preset.
    far: Option<f32>,
    hour: f32,
    // Reproduce the live client's sun exactly: sun_moon.rs bias values,
    // cascade_config_from_settings(High preset), 4096 shadow map, and the
    // hour-driven sun_direction() instead of the fixed debug sun position.
    client_sun: bool,
    // LSB weather id driving the weat/<tag> canopy under --sky. None = whatever
    // an unset CurrentWeather resolves to, i.e. the client's own zone-in default.
    weather: Option<u16>,
    zone_particles: bool,
    // Some(n): Enhanced Dynamic Lights with n shadowed lamps, zone geometry casting.
    enhanced_lights: Option<u32>,
    // Skinned NPC actors placed in the zone (needs --sky), for reading how the
    // lamps light and shadow a character standing in their pool.
    npcs: Vec<NpcPlacement>,
    // Model Shadow Receiving off: the A/B against a default run isolates what the
    // shadow maps change on the placed actors, since the zone reads identically.
    no_receive: bool,
}
#[derive(Clone, Copy)]
enum ActorSubject {
    Npc(u32),
    Pc(u8),
}
#[derive(Clone, Copy)]
struct NpcPlacement {
    subject: ActorSubject,
    pos: Vec3,
    yaw: f32,
}
#[derive(Component)]
struct NpcGrounded(bool);
#[derive(Resource, Default)]
struct FC(u32);
#[derive(Resource, Default)]
struct CS(bool);
/// Off-screen render target; `Screenshot::primary_window()` returns black on
/// macOS when the window is never presented, so we capture this image instead.
#[derive(Resource)]
struct CapTarget(Handle<Image>);
fn main() {
    let a: Vec<String> = env::args().collect();
    let mut p = P {
        file_id: 216,
        out: "/tmp/zone_fix.png".into(),
        cy: 250.0,
        cap: 200,
        mode: ZoneGeomMode::Off,
        amb: 600.0,
        sun: 8000.0,
        cam: None,
        tgt: Vec3::ZERO,
        fog: false,
        sky: false,
        far: None,
        hour: 12.0,
        client_sun: false,
        weather: None,
        zone_particles: true,
        enhanced_lights: None,
        npcs: Vec::new(),
        no_receive: false,
    };
    let f3 = |a: &[String], i: usize| {
        Vec3::new(
            a[i + 1].parse().unwrap(),
            a[i + 2].parse().unwrap(),
            a[i + 3].parse().unwrap(),
        )
    };
    let mut i = 1;
    while i < a.len() {
        match a[i].as_str() {
            "--file" => {
                p.file_id = a[i + 1].parse().unwrap();
                i += 2;
            }
            "--out" => {
                p.out = a[i + 1].clone();
                i += 2;
            }
            "--cy" => {
                p.cy = a[i + 1].parse().unwrap();
                i += 2;
            }
            "--amb" => {
                p.amb = a[i + 1].parse().unwrap();
                i += 2;
            }
            "--sun" => {
                p.sun = a[i + 1].parse().unwrap();
                i += 2;
            }
            "--cam" => {
                p.cam = Some(f3(&a, i));
                i += 4;
            }
            "--tgt" => {
                p.tgt = f3(&a, i);
                i += 4;
            }
            "--cap" => {
                p.cap = a[i + 1].parse().unwrap();
                i += 2;
            }
            "--all" => {
                p.mode = ZoneGeomMode::All;
                i += 1;
            }
            "--fog" => {
                p.fog = true;
                i += 1;
            }
            "--sky" => {
                p.sky = true;
                i += 1;
            }
            "--far" => {
                p.far = Some(a[i + 1].parse().unwrap());
                i += 2;
            }
            "--hour" => {
                p.hour = a[i + 1].parse().unwrap();
                i += 2;
            }
            "--client-sun" => {
                p.client_sun = true;
                i += 1;
            }
            "--weather" => {
                p.weather = Some(a[i + 1].parse().unwrap());
                i += 2;
            }
            "--no-zone-particles" => {
                p.zone_particles = false;
                i += 1;
            }
            "--npc" | "--pc" => {
                let subject = if a[i] == "--npc" {
                    ActorSubject::Npc(a[i + 1].parse().unwrap())
                } else {
                    ActorSubject::Pc(a[i + 1].parse().unwrap())
                };
                p.npcs.push(NpcPlacement {
                    subject,
                    pos: f3(&a, i + 1),
                    yaw: a[i + 5].parse().unwrap(),
                });
                i += 6;
            }
            "--no-receive" => {
                p.no_receive = true;
                i += 1;
            }
            "--enhanced-lights" => {
                p.enhanced_lights = Some(a[i + 1].parse().unwrap());
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }
    let zone_particles = p.zone_particles;
    let enhanced_lights = p.enhanced_lights;
    let mut app = App::new();
    app.insert_resource(VanaClock::anchored_at_hour(p.hour))
        .insert_resource(p)
        .init_resource::<FC>()
        .init_resource::<CS>()
        .insert_resource(ClearColor(Color::BLACK))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                resolution: (1280u32, 1280u32).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FfxiZoneMaterialPlugin)
        .add_message::<LoadMzbRequest>()
        .add_message::<LoadMmbRequest>()
        .add_message::<ToastEvent>()
        .init_resource::<DrawDistance>()
        .init_resource::<MzbCollisionGeometry>()
        .init_resource::<kuluu_render::sub_area_activation::SubAreaActivation>()
        .init_resource::<kuluu_render::dat_mzb::ZoneAreaMap>()
        .init_resource::<kuluu_render::dat_mzb::ZoneChunkLightMap>()
        .init_resource::<PendingWaterSpawns>()
        .init_resource::<ZoneWaterMaterial>()
        .init_resource::<kuluu_render::zone_point_lights::ActiveSceneLights>()
        .init_resource::<kuluu_render::graphics::settings::GraphicsSettings>()
        .init_resource::<kuluu_render::dat_mmb::MmbLoadInFlight>()
        .init_resource::<LoadMzbInFlight>()
        .init_resource::<ZoneGeomCache>()
        .init_resource::<MmbHandleCache>()
        .init_resource::<MmbLoadInFlight>()
        .init_resource::<MmbLoadQueue>()
        .init_resource::<MmbParseCache>()
        .init_resource::<MmbTexPools>()
        .init_resource::<TrackedEntities>()
        .init_resource::<SceneState>()
        .init_resource::<ZoneWeather>()
        .init_resource::<kuluu_render::weather_fx::CurrentWeather>()
        .init_resource::<kuluu_render::weather_fx::ActiveWeatherModifier>()
        .init_resource::<kuluu_render::weather::DefaultClearColor>()
        .add_systems(
            Update,
            (sample_zone_weather, apply_zone_weather)
                .chain()
                .run_if(|p: Res<P>| p.fog || p.sky),
        )
        .add_systems(Startup, (setup, fire, load_weather))
        .add_systems(
            Update,
            (
                drain_toasts,
                kick_load_mzb_tasks,
                poll_load_mzb_tasks,
                spawn_zone_water,
                scroll_water_uv,
                process_load_mmb_requests,
                print_toasts,
                cap,
            )
                .chain(),
        );
    // --sky pulls in the real client sky pipeline. spawn_sun_and_moon runs in
    // setup (below); sun_moon_system drives the discs/lights each frame after
    // the weather record is sampled, and LensFlarePlugin chains its own system
    // after sun_moon_system, exactly as ViewerCorePlugin orders them.
    let sky = app.world().resource::<P>().sky;
    if sky {
        let mut gfx = GraphicsSettings {
            faithful_shadow_receive: !app.world().resource::<P>().no_receive,
            ..GraphicsSettings::default()
        };
        if let Some(n) = enhanced_lights {
            gfx.dynamic_lights = kuluu_render::graphics::settings::DynamicLights::Enhanced;
            gfx.shadowed_lights = n;
            gfx.zone_shadow_cast = true;
        }
        app.insert_resource(gfx)
            // The DAT's 0x47 point lights, and under --enhanced-lights their shadow maps.
            .add_plugins(kuluu_render::zone_point_lights::ZonePointLightsPlugin)
            .add_plugins(SkyboxPlugin)
            .add_plugins(MoonMaterialPlugin)
            .add_plugins(LensFlarePlugin)
            // The retail sun/moon: the zone DAT's own Sun/Moon-attached generators, drawn by
            // the particle simulator, so the hand-authored discs stand down here exactly as
            // they do in the client. Only the simulator's two systems are pulled in — the
            // rest of SchedulerRuntimePlugin wants a live session's resources.
            .init_resource::<kuluu_render::particle_sim::ParticleSimulator>()
            .add_systems(
                Update,
                (
                    kuluu_render::particle_sim::tick_particle_simulator,
                    kuluu_render::particle_sim::sync_particle_meshes,
                )
                    .chain(),
            )
            // Owns Assets<FfxiParticleMaterial>, which both particle plugins below write
            // every frame; without it they panic on the missing resource (kuluu-render/src/lib.rs KuluuRenderPlugin adds
            // it ahead of them for the same reason).
            .add_plugins(kuluu_render::ffxi_particle_material::FfxiParticleMaterialPlugin)
            .add_plugins(kuluu_render::celestial_particles::CelestialParticlesPlugin)
            // The weat/<tag> precipitation set, drawn by the same simulator. Its sync system
            // reads the HUD's mesh-debug flag, which only the full viewer inserts.
            .init_resource::<kuluu_render::hud::HudPanels>()
            .add_plugins(kuluu_render::weather_particles::WeatherParticlesPlugin)
            // The zone's own timed auto-run emitters (lantern flames/glows, chimney smoke);
            // --no-zone-particles leaves them out for an A/B frame-time read.
            .add_plugins(ZoneParticlesGate(zone_particles))
            .init_resource::<VanaSky>()
            .init_resource::<ZoneDirectionalLighting>()
            // The weat/<tag> cloud canopy + star dome, the layers whose per-generator
            // fog bit this harness exists to eyeball (kuluu-grbo).
            .add_plugins(kuluu_render::zone_clouds::ZoneCloudsPlugin)
            .add_systems(Startup, spawn_celestials)
            .add_systems(Update, sun_moon_system.after(sample_zone_weather));
        let weather = app.world().resource::<P>().weather;
        app.insert_resource(kuluu_render::weather_fx::CurrentWeather(
            weather.map(kuluu_snapshot::Weather::from_lsb),
        ));
    }
    let npcs = app.world().resource::<P>().npcs.clone();
    if !npcs.is_empty() {
        assert!(
            sky,
            "--npc needs --sky: actor lighting reads the zone's 0x2F records"
        );
        app.add_plugins(kuluu_render::skinned_ffxi_material::FfxiMaterialPlugin)
            .add_systems(Startup, spawn_npcs)
            .add_systems(
                Update,
                (
                    ground_npcs,
                    kuluu_render::ffxi_actor_render::update_ffxi_render_actor_lighting,
                    kuluu_render::ffxi_actor_render::update_ffxi_actor_point_lights
                        .after(kuluu_render::zone_point_lights::build_active_scene_lights),
                    kuluu_render::ffxi_actor_render::tick_ffxi_render_actors,
                )
                    .chain(),
            );
    }
    app.run();
}
fn spawn_npcs(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<kuluu_render::skinned_ffxi_material::FfxiSkinnedMaterial>>,
    mut material_cache: ResMut<kuluu_render::skinned_ffxi_material::FfxiSkinnedMaterialCache>,
    mut registry: ResMut<kuluu_render::skinned_ffxi_material::FfxiSkinRegistry>,
    mut images: ResMut<Assets<Image>>,
    p: Res<P>,
) {
    for npc in &p.npcs {
        let loaded = match npc.subject {
            ActorSubject::Npc(id) => kuluu_render::ffxi_actor_render::load_npc(id),
            ActorSubject::Pc(race) => {
                kuluu_render::ffxi_actor_render::load_pc(race, false, &[], None, None, None)
            }
        };
        let loaded = match loaded {
            Ok(loaded) => loaded,
            Err(e) => {
                eprintln!("actor: {e}");
                continue;
            }
        };
        let entity = kuluu_render::ffxi_actor_render::spawn_loaded_actor(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut material_cache,
            &mut registry,
            &mut images,
            &loaded,
            npc.pos,
            npc.yaw,
            1.0,
            kuluu_render::zone_texture::TextureQuality {
                mipmaps: true,
                anisotropy: 8,
            },
        );
        commands.entity(entity).insert(NpcGrounded(false));
    }
}
// The harness has no player grounding, so each actor snaps to the MZB floor once
// the collision blocks have streamed in. The nearest DAT lamps are printed with it,
// which is what places a character in a chosen lantern pool.
fn ground_npcs(
    collision: Res<MzbCollisionGeometry>,
    lamps: Res<kuluu_render::zone_point_lights::ZonePointLights>,
    mut q: Query<(&mut Transform, &mut NpcGrounded)>,
) {
    const NEAREST_LAMPS_REPORTED: usize = 3;
    for (mut t, mut grounded) in &mut q {
        if grounded.0 {
            continue;
        }
        let Some(y) = collision.ground_nearest(t.translation.xz(), t.translation.y) else {
            continue;
        };
        t.translation.y = y;
        grounded.0 = true;
        let mut nearest: Vec<(f32, Vec3, f32)> = lamps
            .lights
            .iter()
            .map(|l| (l.world_pos.distance(t.translation), l.world_pos, l.range))
            .collect();
        nearest.sort_by(|a, b| a.0.total_cmp(&b.0));
        eprintln!(
            "npc grounded at ({:.2}, {:.2}, {:.2}); nearest lamps: {:?}",
            t.translation.x,
            y,
            t.translation.z,
            nearest
                .iter()
                .take(NEAREST_LAMPS_REPORTED)
                .map(|(d, p, r)| format!(
                    "d={d:.1} at ({:.1},{:.1},{:.1}) range={r:.1}",
                    p.x, p.y, p.z
                ))
                .collect::<Vec<_>>()
        );
    }
}
fn print_toasts(mut rx: MessageReader<ToastEvent>) {
    for t in rx.read() {
        println!("[toast] {}", t.line.text);
    }
}
fn setup(
    mut c: Commands,
    p: Res<P>,
    mut d: ResMut<DrawDistance>,
    mut images: ResMut<Assets<Image>>,
    settings: Res<GraphicsSettings>,
) {
    d.zone_geom_mode = p.mode;
    d.world = 1e5;
    d.mob = 1e5;
    // Off-screen render target (see CapTarget).
    let size = bevy::render::render_resource::Extent3d {
        width: 1280,
        height: 1280,
        depth_or_array_layers: 1,
    };
    let mut target = Image::new_fill(
        size,
        bevy::render::render_resource::TextureDimension::D2,
        &[0, 0, 0, 255],
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    );
    target.texture_descriptor.usage = bevy::render::render_resource::TextureUsages::TEXTURE_BINDING
        | bevy::render::render_resource::TextureUsages::COPY_SRC
        | bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT;
    let target = images.add(target);
    c.insert_resource(CapTarget(target.clone()));
    let cam_target = bevy::camera::RenderTarget::Image(target.into());
    let cam = match p.cam {
        Some(pos) => c
            .spawn((
                Camera3d::default(),
                cam_target.clone(),
                Transform::from_translation(pos).looking_at(p.tgt, Vec3::Y),
            ))
            .id(),
        None => {
            let mut proj = OrthographicProjection::default_3d();
            proj.scaling_mode = ScalingMode::FixedVertical {
                viewport_height: 360.0,
            };
            c.spawn((
                Camera3d::default(),
                cam_target.clone(),
                Projection::Orthographic(proj),
                Transform::from_xyz(0., p.cy, 0.).looking_at(Vec3::new(0., 0., -0.001), Vec3::Z),
            ))
            .id()
        }
    };
    if p.fog {
        c.entity(cam).insert((
            OperatorCamera,
            bevy::camera::Hdr,
            bevy::light::VolumetricFog {
                step_count: 64,
                ambient_intensity: 0.03,
                ambient_color: Color::srgb(0.85, 0.88, 1.0),
                jitter: 0.0,
            },
        ));
        // Mirrors the client's zone fog volume (scene.rs); apply_zone_weather
        // retunes color/density from the zone weather record each frame.
        c.spawn((
            bevy::light::FogVolume {
                fog_color: Color::srgb(0.65, 0.72, 0.82),
                density_factor: 0.06,
                absorption: 0.25,
                scattering: 0.35,
                scattering_asymmetry: 0.7,
                light_tint: Color::srgb(1.0, 0.96, 0.88),
                light_intensity: 1.0,
                // Ground haze with vertical falloff so the sky stays visible;
                // see height_fog_density_texture in weather.rs.
                density_texture: Some(kuluu_render::weather::height_fog_density_texture(
                    &mut images,
                )),
                ..default()
            },
            Transform::from_xyz(0.0, kuluu_render::weather::FOG_VOLUME_CENTER_Y, 0.0)
                .with_scale(kuluu_render::weather::FOG_VOLUME_SCALE),
        ));
    }
    if p.sky {
        // Same post-processing the client camera carries (camera.rs
        // build_operator_camera): HDR + Bloom give the sun disc its bloom halo,
        // tonemapping matches the active sky style, and camera_far derives the far
        // plane from the draw distance exactly as the client does.
        // OperatorCamera is what sun_moon_system tracks to place the discs and
        // what apply_zone_weather attaches DistanceFog to.
        c.entity(cam).insert((
            OperatorCamera,
            bevy::camera::Hdr,
            settings.tonemapping(),
            Bloom {
                intensity: settings.bloom_intensity,
                prefilter: BloomPrefilter {
                    threshold: 1.0,
                    threshold_softness: 0.4,
                },
                ..Bloom::NATURAL
            },
            Projection::Perspective(PerspectiveProjection {
                far: p
                    .far
                    .map(kuluu_render::skybox::camera_far)
                    .unwrap_or_else(|| kuluu_render::skybox::camera_far(settings.view_distance)),
                fov: settings.fov_deg.to_radians(),
                ..default()
            }),
        ));
    }
    c.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: p.amb,
        ..default()
    });

    // --sky spawns the real sun/moon/disc rig via spawn_celestials; skip the
    // debug single-light sun so there is exactly one IsSun in the scene.
    if p.sky {
        return;
    }

    let sun = if p.client_sun {
        // Mirror sun_moon.rs exactly: same bias values, same cascade config
        // (High preset = default), 4096 map, hour-driven direction.
        let gfx = kuluu_render::graphics::settings::GraphicsSettings::default();
        c.insert_resource(bevy::light::DirectionalLightShadowMap {
            size: gfx.shadow_map_size as usize,
        });
        let sun_dir = kuluu_render::sun_moon::sun_direction(p.hour);
        c.spawn((
            IsSun,
            DirectionalLight {
                illuminance: p.sun,
                shadow_maps_enabled: true,
                shadow_depth_bias: 0.2,
                shadow_normal_bias: 0.6,
                ..default()
            },
            kuluu_render::graphics::settings::cascade_config_from_settings(&gfx),
            Transform::from_translation(sun_dir * 1000.0).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id()
    } else {
        c.spawn((
            IsSun,
            DirectionalLight {
                illuminance: p.sun,
                shadow_maps_enabled: true,
                ..default()
            },
            Transform::from_xyz(300., 220., 120.).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id()
    };
    if p.fog {
        c.entity(sun).insert(bevy::light::VolumetricLight);
    }
}
fn fire(mut tx: MessageWriter<LoadMzbRequest>, p: Res<P>) {
    tx.write(LoadMzbRequest {
        file_id: p.file_id,
        chunk_idx: None,
        world_pos: Vec3::ZERO,
        auto_loaded: false,
        slot: kuluu_render::dat_mzb::ZONE_SLOT_MAIN,
        active_sub_area: None,
    });
}

// The real client spawner (sun_moon.rs), used verbatim so the harness renders
// the same sun/moon directional lights + emissive discs the client does.
// Only registered under --sky, so Assets<MoonMaterial> (from MoonMaterialPlugin)
// is guaranteed present.
fn spawn_celestials(
    mut c: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut std_mats: ResMut<Assets<StandardMaterial>>,
    mut moon_mats: ResMut<Assets<MoonMaterial>>,
    settings: Res<GraphicsSettings>,
) {
    c.insert_resource(bevy::light::DirectionalLightShadowMap {
        size: settings.shadow_map_size as usize,
    });
    spawn_sun_and_moon(
        &mut c,
        &mut meshes,
        &mut std_mats,
        &mut moon_mats,
        &settings,
    );
}
// Headless mirror of weather::load_zone_weather: that system derives the zone
// from the live SceneState snapshot, but this example has no login flow — the
// zone comes straight from `P.file_id`, so read the same DAT bytes directly.
fn load_weather(
    mut c: Commands,
    p: Res<P>,
    mut zone_weather: ResMut<ZoneWeather>,
    mut scene_state: ResMut<SceneState>,
) {
    // The zone-DAT consumers (celestial_particles, zone_particles, moon_material) resolve
    // their file from SceneState's zone_id, not from --file, so seed the zone whose DAT this
    // is. Reverse scan over the public mapping; the harness names a file, the client a zone.
    scene_state.snapshot.zone_id =
        (0u16..=0x1FF).find(|z| ffxi_dat::zone_dat::zone_id_to_mzb_file_id(*z) == Some(p.file_id));

    let Ok(root) = ffxi_dat::DatRoot::from_env_or_default().map(std::sync::Arc::new) else {
        return;
    };
    // The client hands this to every DAT consumer through view_native's insert_dat_roots;
    // without it load_moon_sprite_sheet and load_lens_flare_sheet bail on the first line and
    // the harness silently renders the no-sprite fallbacks instead of the retail assets.
    c.insert_resource(kuluu_render::moon_material::MoonDatRoot(Some(root.clone())));
    // The client loads this off-thread (scheduler_runtime load_global_effect_dir); the harness
    // reads it inline so zone generators whose mesh ships in syst/effe/ resolve.
    if let Some(global) = root
        .resolve(kuluu_render::scheduler_runtime::GLOBAL_EFFECT_DIR_FILE_ID)
        .ok()
        .and_then(|l| std::fs::read(l.path_under(&root)).ok())
    {
        let (schedulers, assets) = kuluu_render::scheduler_runtime::parse_action_bytes(&global);
        c.insert_resource(kuluu_render::scheduler_runtime::GlobalEffectDir { schedulers, assets });
    }
    let Ok(location) = root.resolve(p.file_id) else {
        return;
    };
    let path = location.path_under(&root);
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    zone_weather.file_id = Some(p.file_id);
    zone_weather.sets = collect_zone_weather_sets(&bytes);
}
fn drain_toasts(mut rx: MessageReader<ToastEvent>) {
    for t in rx.read() {
        eprintln!("[toast] {}", t.line.text);
    }
}
#[allow(clippy::too_many_arguments)]
fn cap(
    mut c: Commands,
    mut f: ResMut<FC>,
    mut s: ResMut<CS>,
    p: Res<P>,
    q: Query<Entity, With<Capturing>>,
    mut e: MessageWriter<AppExit>,
    queue: Res<MmbLoadQueue>,
    water: Res<PendingWaterSpawns>,
    target: Res<CapTarget>,
    time: Res<Time>,
    mut frame_secs: Local<f32>,
) {
    f.0 += 1;
    *frame_secs += time.delta_secs();
    if f.0.is_multiple_of(40) {
        eprintln!(
            "frame {} pending={} water_pending={} avg_frame_ms={:.2}",
            f.0,
            queue.pending.len(),
            water.specs.len(),
            *frame_secs / 40.0 * 1000.0
        );
        *frame_secs = 0.0;
    }
    if !s.0 && f.0 >= p.cap {
        c.spawn(Screenshot::image(target.0.clone()))
            .observe(save_to_disk(p.out.clone()));
        s.0 = true;
        eprintln!("captured -> {}", p.out);
    }
    if s.0 && q.is_empty() && f.0 >= p.cap + 5 {
        e.write(AppExit::Success);
    }
}

struct ZoneParticlesGate(bool);
impl Plugin for ZoneParticlesGate {
    fn build(&self, app: &mut App) {
        if self.0 {
            app.add_plugins(kuluu_render::zone_particles::ZoneParticlesPlugin);
        }
    }
}
