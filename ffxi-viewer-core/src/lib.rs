#![forbid(unsafe_code)]
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

pub mod atmosphere;
#[cfg(not(target_arch = "wasm32"))]
pub mod audio;
pub mod camera;
pub mod combat_stance;
pub mod components;
pub mod cursor;
#[cfg(not(target_arch = "wasm32"))]
pub mod dat_d3m;
#[cfg(not(target_arch = "wasm32"))]
pub mod dat_mmb;
#[cfg(not(target_arch = "wasm32"))]
pub mod dat_mzb;
#[cfg(not(target_arch = "wasm32"))]
pub mod dat_vos2;
pub mod debug_chat;
#[cfg(not(target_arch = "wasm32"))]
pub mod ffxi_actor_render;
#[cfg(not(target_arch = "wasm32"))]
pub mod ffxi_zone_material;
pub mod graphics;
pub use graphics::settings as graphics_settings;
pub mod hud;
pub mod input_mode;
pub mod keybinds;
pub mod lens_flare;
pub mod lock_on;
#[cfg(not(target_arch = "wasm32"))]
pub mod look_resolver;
#[cfg(not(target_arch = "wasm32"))]
pub mod minimap;
pub mod moon_material;
pub mod mouse;
pub mod nameplate;
pub mod nameplate_billboard;
#[cfg(not(target_arch = "wasm32"))]
pub mod particle_sim;
pub mod perf_probe;
pub mod picking;
pub mod scene;
pub mod scheduler_runtime;
#[cfg(not(target_arch = "wasm32"))]
pub mod skeleton_instance;
#[cfg(not(target_arch = "wasm32"))]
pub mod skinned_ffxi_material;
pub mod sky_realism;
#[cfg(not(target_arch = "wasm32"))]
pub mod skybox;
pub mod snapshot;
pub mod source;
pub mod sun_moon;
pub mod target_ring;
#[cfg(not(target_arch = "wasm32"))]
pub mod target_strobe;
pub mod ui_font;
pub mod vana_time;
#[cfg(feature = "enhanced-water")]
pub mod water_enhanced;
#[cfg(not(target_arch = "wasm32"))]
pub mod weather;
pub mod weather_fx;
#[cfg(not(target_arch = "wasm32"))]
pub mod zone_clouds;
pub mod zone_lights;
pub mod zone_lines;
#[cfg(not(target_arch = "wasm32"))]
pub mod zone_point_lights;
#[cfg(not(target_arch = "wasm32"))]
pub mod zone_texture;

pub use camera::{
    camera_transition_system, chase_camera_system, configure_gizmo_render_layer,
    first_person_eye_y, firstperson_camera_system, heading_for_yaw, nameplate_anchor_y,
    self_visibility_for_camera_mode_system, spawn_camera, third_person_anchor_y,
    toggle_camera_mode, yaw_for_heading, CameraMode, CameraTransition, ChaseCamera, OperatorCamera,
    WORLD_GIZMO_LAYER,
};
pub use components::{
    EntityModel, HpIndicator, InGameEntity, IsSelf, LookComp, Nameplate, WorldEntity,
};
pub use cursor::{system_cursor_icon, CursorPlugin, CursorRequests, CursorStyle};
pub use graphics_settings::{
    AaMode, CharacterRenderPath, DynamicLights, GraphicsField, GraphicsSettings, QualityPreset,
    SkyStyle, TextureFiltering, ZoneLineDisplay, GRAPHICS_FIELDS,
};
pub use hud::{add_hud_spawners, HudPlugin};
pub use input_mode::{
    ChatBuffer, DialogCursor, InputMode, MenuKind, MenuLevel, MenuStack, PassiveCursorFocus,
    PassiveCursorState, QuickActionState, DIALOG_MAX_CHOICE,
};
pub use keybinds::{Action, Bindings, KeyBind, Modifiers, Preset};
pub use lock_on::{LockOn, ToggleResult as LockOnToggle};
pub use mouse::{CursorLockRequest, MousePlugin, MousePointer};
pub use picking::{
    click_to_target_system, resolve_click_target, ClickResolution, HoveredEntity, PickingPlugin,
};
pub use scene::{
    entity_visual_height, ffxi_to_bevy, process_entity_look_changes, setup_world,
    sync_aggro_system, sync_entities_system, sync_entity_looks_system, Aggroing, BakedActor,
    EntityMaterials, EntityMesh, Target, TrackedEntities,
};
pub use snapshot::{
    apply_delta, drain_toast_events, ingest_system, EventLog, SceneState, ToastEvent,
    CHAT_HISTORY_CAP,
};
pub use source::SceneSource;
pub use zone_lines::{
    setup_zone_line_assets, sync_zone_lines_system, ZoneLineAssets, ZoneLineDescriptor,
    ZoneLineMarker, ZoneLineResolver, ZoneLineState,
};

use std::marker::PhantomData;

use bevy::prelude::*;

pub struct ViewerCorePlugin<S> {
    _marker: PhantomData<fn() -> S>,
}

impl<S> Default for ViewerCorePlugin<S> {
    fn default() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

fn tolerate_command_entity_despawn(
    error: bevy::ecs::error::BevyError,
    ctx: bevy::ecs::error::ErrorContext,
) {
    use bevy::ecs::error::ErrorContext;
    use bevy::ecs::world::error::EntityMutableFetchError;

    let is_despawn_race = error
        .downcast_ref::<EntityMutableFetchError>()
        .is_some_and(|e| matches!(e, EntityMutableFetchError::NotSpawned(_)))
        || error
            .downcast_ref::<bevy::ecs::entity::EntityNotSpawnedError>()
            .is_some();

    if is_despawn_race && matches!(ctx, ErrorContext::Command { .. }) {
        warn!("command skipped: target entity despawned before flush ({error})");
        return;
    }

    bevy::ecs::error::panic(error, ctx);
}

impl<S: SceneSource + Resource> Plugin for ViewerCorePlugin<S> {
    fn build(&self, app: &mut App) {
        app.set_error_handler(tolerate_command_entity_despawn);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(dat_mmb::DatOverlayPlugin);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(audio::AudioPlugin);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(weather::WeatherPlugin);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(minimap::MinimapPlugin);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(graphics::render_scale::RenderScalePlugin);

        app.add_plugins(scheduler_runtime::SchedulerRuntimePlugin);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(skybox::SkyboxPlugin);
        app.add_plugins(moon_material::MoonMaterialPlugin);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(skinned_ffxi_material::FfxiMaterialPlugin);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(ffxi_zone_material::FfxiZoneMaterialPlugin);

        #[cfg(feature = "enhanced-water")]
        app.add_plugins(water_enhanced::EnhancedWaterPlugin);

        app.add_plugins(lens_flare::LensFlarePlugin);

        app.add_plugins(zone_lights::ZoneLightsPlugin);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(zone_point_lights::ZonePointLightsPlugin);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(zone_clouds::ZoneCloudsPlugin);

        app.add_plugins(debug_chat::DebugChatPlugin);
        app.init_resource::<SceneState>()
            .init_resource::<EventLog>()
            .init_resource::<TrackedEntities>()
            .init_resource::<Target>()
            .init_resource::<InputMode>()
            .init_resource::<Bindings>()
            .init_resource::<CameraMode>()
            .init_resource::<LockOn>()
            .init_resource::<ZoneLineState>()
            .init_resource::<GraphicsSettings>()
            .init_resource::<graphics_settings::MsaaCaps>()
            .add_systems(PreStartup, graphics_settings::init_msaa_caps_system)
            .init_resource::<atmosphere::ZoneAtmosphereProvider>()
            .init_resource::<atmosphere::LastAtmosphereZone>()
            .init_resource::<sun_moon::VanaSky>()
            .init_resource::<weather::ZoneDirectionalLighting>()
            .init_resource::<vana_time::VanaClock>()
            .init_resource::<sky_realism::SkyRealism>()
            .init_resource::<weather_fx::ActiveWeatherModifier>()
            .init_resource::<weather_fx::ParticleAssets>()
            .init_resource::<weather_fx::LightningState>()
            .init_resource::<weather_fx::CurrentWeather>()
            .init_resource::<hud::chat_panel::ChatScroll>()
            .init_resource::<hud::chat_panel::BattleScroll>()
            .init_resource::<hud::chat_panel::DebugScroll>()
            .init_resource::<hud::chat_panel::ChatScrollAccum>()
            .init_resource::<hud::chat_panel::BattleScrollAccum>()
            .init_resource::<hud::chat_panel::DebugScrollAccum>()
            .init_resource::<scene::SelfAppearance>()
            .init_resource::<nameplate_billboard::BillboardFont>()
            .init_resource::<ui_font::UiFont>()
            .add_plugins(PickingPlugin)
            .add_plugins(CursorPlugin)
            .add_message::<ToastEvent>()
            .add_systems(Startup, ui_font::load_ui_font)
            .add_systems(Update, ui_font::apply_ui_font)
            .add_systems(PreUpdate, ingest_system::<S>.run_if(resource_exists::<S>))
            .add_systems(PostUpdate, drain_toast_events)
            .add_systems(
                PreUpdate,
                vana_time::ingest_vana_time.after(ingest_system::<S>),
            )
            .add_systems(
                Update,
                (
                    (
                        sync_entities_system,
                        sync_entity_looks_system,
                        scene::ensure_self_lookcomp_system,
                        process_entity_look_changes,
                        camera_transition_system,
                        chase_camera_system,
                        firstperson_camera_system,
                        sync_aggro_system,
                        nameplate::update_nameplates_system,
                        nameplate_billboard::update_nameplate_billboards_system,
                        target_ring::draw_target_arrow_system,
                        target_ring::draw_target_ring_system,
                        sync_zone_lines_system,
                        atmosphere::apply_zone_atmosphere_system,
                    )
                        // Chained so each camera writes its Transform before the nameplate and
                        // target-ring systems read it. Left unordered, the first-person camera and
                        // the self nameplate race on the camera Transform, so the overhead self
                        // nameplate jitters/dips against the eye frame-to-frame (kuluu-gr2).
                        .chain(),
                    (
                        weather_fx::sync_current_weather_from_snapshot,
                        weather_fx::update_weather_modifier_system,
                        weather_fx::apply_weather_to_ambient_and_fog_system,
                        sun_moon::sun_moon_system,
                        weather_fx::apply_weather_to_sun_system,
                        weather_fx::manage_weather_particles_system,
                        weather_fx::update_weather_particles_system,
                    ),
                )
                    .chain()
                    .run_if(resource_exists::<EntityMesh>),
            );

        #[cfg(not(target_arch = "wasm32"))]
        {
            app.init_resource::<target_strobe::StrobeState>();
            app.add_systems(Update, target_strobe::target_strobe_system);
        }

        #[cfg(not(target_arch = "wasm32"))]
        app.configure_sets(
            Update,
            weather::WeatherSampleSet
                .before(sun_moon::sun_moon_system)
                .after(weather_fx::sync_current_weather_from_snapshot),
        );

        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, ffxi_actor_render::update_ffxi_render_actor_lighting);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, ffxi_actor_render::apply_character_shadow_cast);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            ffxi_actor_render::update_ffxi_actor_point_lights
                .after(ffxi_actor_render::update_ffxi_render_actor_lighting)
                .after(zone_point_lights::build_active_scene_lights),
        );

        app.init_resource::<combat_stance::EntityMotion>();
        app.init_resource::<combat_stance::EntityPrediction>();
        app.init_resource::<combat_stance::RestStance>();
        app.init_resource::<combat_stance::AnimationBlends>();
        app.init_resource::<combat_stance::WalkMode>();
        app.init_resource::<camera::CameraTransition>();

        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            combat_stance::predict_entities_system
                .after(sync_entities_system)
                .before(combat_stance::track_entity_motion_system),
        );
        #[cfg(target_arch = "wasm32")]
        app.add_systems(
            Update,
            combat_stance::predict_entities_system.after(sync_entities_system),
        );
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            combat_stance::track_entity_motion_system
                .before(ffxi_actor_render::tick_live_ffxi_actors),
        );

        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, ffxi_actor_render::tick_live_ffxi_actors);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            ffxi_actor_render::dispatch_action_overlay
                .before(ffxi_actor_render::tick_live_ffxi_actors),
        );

        app.add_systems(
            Update,
            scene::auto_clear_target_system.before(sync_entities_system),
        );

        app.add_systems(Update, lock_on::auto_lock_on_when_engaged);

        app.add_systems(Update, self_visibility_for_camera_mode_system);

        app.add_systems(
            Update,
            (
                graphics_settings::apply_shadow_map_size_system,
                graphics_settings::apply_cascade_config_system,
                graphics_settings::apply_bloom_system,
                graphics_settings::apply_volumetric_fog_system,
                graphics_settings::apply_projection_system,
                graphics_settings::apply_vsync_system,
                graphics_settings::apply_anti_aliasing_system,
                graphics_settings::apply_sky_realism_system,
            )
                .chain()
                .run_if(resource_changed::<GraphicsSettings>),
        );

        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Startup, graphics_settings::init_physical_sky_medium_system);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            (
                graphics_settings::apply_depth_of_field_system,
                graphics_settings::apply_physical_sky_system,
            )
                .chain()
                .after(graphics_settings::apply_anti_aliasing_system)
                .run_if(resource_changed::<GraphicsSettings>),
        );

        // Runs every frame (not gated on settings changes): the Vanilla sun
        // flare needs the depth prepass only while the sun is up, so this tracks
        // VanaSky, not just GraphicsSettings.
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            graphics_settings::apply_camera_prepass_system
                .after(graphics_settings::apply_anti_aliasing_system),
        );

        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            graphics_settings::update_depth_of_field_focus_system,
        );
    }
}
