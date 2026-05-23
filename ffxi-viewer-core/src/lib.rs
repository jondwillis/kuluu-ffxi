//! Operator-viewer core: Bevy ECS plugins, components, scene systems, and
//! HUD shared by the native windowed viewer (`ffxi-client/src/view_native/`)
//! and the browser viewer (`ffxi-viewer-wasm/`).
//!
//! State arrives through the [`SceneSource`] trait — implementations live
//! in each front-end. This crate is `tokio`-free; nothing here knows how
//! the bytes get from a server to the source.
//!
//! # Plugin tree (Stage 0 scaffold — scene, camera, HUD land in 0c/0d)
//!
//! ```text
//! ViewerCorePlugin<S>
//!  ├─ resources: SceneState, EventLog
//!  └─ systems: ingest_system::<S>  (PreUpdate)
//! ```

#![forbid(unsafe_code)]

pub mod atmosphere;
#[cfg(not(target_arch = "wasm32"))]
pub mod audio;
pub mod camera;
pub mod combat_stance;
pub mod components;
pub mod cursor;
pub mod debug_chat;
#[cfg(not(target_arch = "wasm32"))]
pub mod dat_mmb;
#[cfg(not(target_arch = "wasm32"))]
pub mod dat_d3m;
#[cfg(not(target_arch = "wasm32"))]
pub mod dat_mzb;
#[cfg(not(target_arch = "wasm32"))]
pub mod dat_vos2;
pub mod graphics_settings;
pub mod hud;
pub mod input_mode;
pub mod keybinds;
pub mod lock_on;
#[cfg(not(target_arch = "wasm32"))]
pub mod look_resolver;
#[cfg(not(target_arch = "wasm32"))]
pub mod minimap;
pub mod mouse;
pub mod nameplate;
pub mod nameplate_billboard;
pub mod picking;
pub mod scene;
pub mod scheduler_runtime;
#[cfg(not(target_arch = "wasm32"))]
pub mod skybox;
pub mod snapshot;
pub mod source;
pub mod sun_moon;
pub mod target_ring;
#[cfg(not(target_arch = "wasm32"))]
pub mod weather;
pub mod weather_fx;
pub mod zone_lines;

pub use camera::{
    chase_camera_system, first_person_eye_y, firstperson_camera_system, heading_for_yaw,
    nameplate_anchor_y, self_visibility_for_camera_mode_system, spawn_camera,
    third_person_anchor_y, toggle_camera_mode, yaw_for_heading, CameraMode, ChaseCamera,
    OperatorCamera,
};
pub use components::{
    EntityModel, HpIndicator, InGameEntity, IsSelf, LookComp, Nameplate, WorldEntity,
};
pub use graphics_settings::{AaMode, GraphicsField, GraphicsSettings, QualityPreset};
pub use hud::{add_hud_spawners, HudPlugin};
pub use input_mode::{
    ChatBuffer, DialogCursor, InputMode, MenuKind, MenuLevel, MenuStack, PassiveCursorFocus,
    PassiveCursorState, QuickActionState, DIALOG_MAX_CHOICE,
};
pub use keybinds::{Action, Bindings, KeyBind, Modifiers, Preset};
pub use lock_on::{LockOn, ToggleResult as LockOnToggle};
pub use cursor::{CursorAssets, CursorPlugin, CursorRequests, CursorStyle};
pub use mouse::{CursorLockRequest, MousePlugin, MousePointer};
pub use picking::{
    click_to_target_system, resolve_click_target, ClickResolution, HoveredEntity, PickingPlugin,
};
pub use scene::{
    entity_visual_height, ffxi_to_bevy, process_entity_look_changes, setup_world,
    sync_aggro_system, sync_entities_system, sync_entity_looks_system, Aggroing, BakedActor,
    EntityMaterials, EntityMesh, Target, TrackedEntities,
};
pub use snapshot::{apply_delta, ingest_system, EventLog, SceneState, CHAT_HISTORY_CAP};
pub use source::SceneSource;
pub use zone_lines::{
    setup_zone_line_assets, sync_zone_lines_system, ZoneLineAssets, ZoneLineDescriptor,
    ZoneLineMarker, ZoneLineResolver, ZoneLineState,
};

use std::marker::PhantomData;

use bevy::prelude::*;

/// The plugin every viewer front-end adds to its Bevy app. Generic over the
/// `SceneSource` impl — the front-end inserts a concrete `S: Resource`
/// before adding this plugin.
///
/// ```ignore
/// app.insert_resource(NativeSource::new(state_rx, event_rx))
///    .add_plugins(ViewerCorePlugin::<NativeSource>::default());
/// ```
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

impl<S: SceneSource + Resource> Plugin for ViewerCorePlugin<S> {
    fn build(&self, app: &mut App) {
        // Native-only: `/load_mmb` reads the user's local DAT install
        // via `fs::read`. wasm can't yet — see `dat_mmb.rs` for the gate.
        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(dat_mmb::DatOverlayPlugin);
        // Native-only: BGM playback driven by LSB 0x05F music
        // events. Reads `FFXI_DAT_PATH` for the install root. No-ops
        // gracefully when the env var is unset or the sound trees
        // are missing — see `audio::apply_bgm_system`.
        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(audio::AudioPlugin);
        // Per-zone TOD-driven fog + ambient from the DAT's Weather
        // chunks (kind 0x2F). Runs every frame after the zone-change
        // atmosphere applier so the live lerp overrides the heuristic
        // baseline. Native-only (synchronous fs reads to load DAT
        // bytes; same constraint as `dat_mmb::DatOverlayPlugin`).
        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(weather::WeatherPlugin);
        // Minimap HUD plugin. Owns `MinimapMode` / `MinimapVisible` /
        // `MinimapState` and registers the swap + visibility reactors.
        // The spawn-once UI node is registered separately via
        // `add_hud_spawners` (same pattern as the rest of the HUD), so
        // a front-end that wants the resources without the UI tree can
        // still add this plugin alone. Native-only because the
        // top-down backend reads `dat_mzb::MzbCollisionGeometry`.
        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(minimap::MinimapPlugin);
        // Action-scheduler runtime: advances ActiveScheduler components
        // (one per acting entity) by elapsed real time × 30 fps and
        // fans stages out as SchedulerStageEvent messages. D2 (particle
        // spawn) and E3 (sound dispatch) will subscribe.
        app.add_plugins(scheduler_runtime::SchedulerRuntimePlugin);
        // Skybox dome: inverted sphere centered on the camera, fragment
        // shader gradient driven by ZoneWeather. Native-only because
        // it reads ZoneWeather which is itself native-only.
        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(skybox::SkyboxPlugin);
        // Debug chat surfacing: routes engine + protocol events
        // (zone change, aggro, low HP, speed suppression) to the
        // System chat pane. Cross-platform: the drain reads the same
        // EventLog the SFX systems do.
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
            // Front-ends that load a `graphics.json` from disk (native
            // client) call `insert_resource` *before* adding this plugin;
            // `init_resource` here is a no-op in that case. WASM and
            // headless tests fall back to the default (High preset).
            .init_resource::<GraphicsSettings>()
            .init_resource::<atmosphere::ZoneAtmosphereProvider>()
            .init_resource::<atmosphere::LastAtmosphereZone>()
            .init_resource::<sun_moon::VanaSky>()
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
            // Launcher-supplied appearance for the local PC. LSB zeros
            // the self GrapIDTbl in CHAR_PC packets; without this
            // override the local player would never get a LookComp
            // and `dispatch_look_driven_models` would never fire,
            // leaving the self capsule un-replaced. Populated by the
            // launcher pre-connect (`ConnectInFlight`) and consumed
            // by `ensure_self_lookcomp_system`.
            .init_resource::<scene::SelfAppearance>()
            // 3D-billboard nameplates need a shared ab_glyph font for
            // text rasterization. Initialized lazily from the embedded
            // Bevy default font (FiraMono-subset) — see
            // `nameplate_billboard::BillboardFont::from_world`.
            .init_resource::<nameplate_billboard::BillboardFont>()
            // PickingPlugin owns the mesh raycast backend + the click→target
            // reader. `DefaultPickingPlugins` (input/hover/interaction) is
            // already added by `DefaultPlugins` on both front-ends.
            .add_plugins(PickingPlugin)
            // Custom cursor sprite. Hides the OS cursor and renders a 24×24
            // in-app sprite that swaps shape based on what's under the
            // pointer (Arrow / Hand / Rotate). The OS cursor lock layer
            // (`mouse::apply_cursor_lock_system`) no longer touches
            // visibility — `CursorPlugin` is the sole owner.
            .add_plugins(CursorPlugin)
            .add_systems(PreUpdate, ingest_system::<S>.run_if(resource_exists::<S>))
            // The Update tuple needs the world resources `setup_world`
            // inserts (EntityMesh/EntityMaterials/HpBarMesh) — those
            // are `Res<>` params, which Bevy treats as hard requirements
            // and panics on at parameter validation. Native gates this
            // via OnEnter(InGame); wasm runs `setup_world` at Startup.
            // EntityMesh is the canonical "world ready" canary.
            //
            // `chase_camera_system` and `firstperson_camera_system` both
            // run every frame; each early-returns when its mode isn't
            // active, so exactly one moves the camera.
            .add_systems(
                Update,
                (
                    sync_entities_system,
                    // Stage 2 of look→MMB pipeline: copy each wire
                    // entity's `look` onto its Bevy entity (when
                    // changed) and emit a hook system that Stage 3+
                    // will hang `LoadMmbRequest` dispatch off. Order
                    // matters: must run after the spawn pass so the
                    // `TrackedEntities` map is current.
                    sync_entity_looks_system,
                    // Inject the launcher-supplied self look when
                    // the wire snapshot leaves it empty (the LSB
                    // self-CHAR_PC case). Must run *after*
                    // sync_entity_looks_system so wire-side data
                    // wins when present.
                    scene::ensure_self_lookcomp_system,
                    process_entity_look_changes,
                    chase_camera_system,
                    firstperson_camera_system,
                    sync_aggro_system,
                    nameplate::update_nameplates_system,
                    nameplate_billboard::update_nameplate_billboards_system,
                    target_ring::draw_target_ring_system,
                    target_ring::draw_engaged_ring_system,
                    sync_zone_lines_system,
                    atmosphere::apply_zone_atmosphere_system,
                    // Order matters: update_weather_modifier_system runs
                    // *after* the zone atmosphere has written fresh base
                    // ambient values. apply_weather_to_ambient_and_fog
                    // then multiplies the modifier onto that base. The
                    // sun system writes time-of-day illuminance, and
                    // apply_weather_to_sun multiplies the modifier
                    // onto that — see `weather_fx` module docs for the
                    // scheduling rationale.
                    weather_fx::sync_current_weather_from_snapshot,
                    weather_fx::update_weather_modifier_system,
                    weather_fx::apply_weather_to_ambient_and_fog_system,
                    sun_moon::sun_moon_system,
                    weather_fx::apply_weather_to_sun_system,
                    weather_fx::manage_weather_particles_system,
                    weather_fx::update_weather_particles_system,
                )
                    .chain()
                    .run_if(resource_exists::<EntityMesh>),
            );

        // Skinned-actor idle-animation tick. Runs on every `SkinnedActor`
        // (currently NPCs; PC migration tracked separately). Independent
        // of the main chained tuple above — it operates on a different
        // entity set and has no ordering constraints with the
        // sync/dispatch/camera pipeline.
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, dat_vos2::tick_skinned_actors);

        // Per-entity motion tracker for combat-stance / locomotion
        // animation selection. Runs *before* `tick_skinned_actors`
        // (which reads `EntityMotion`) so the locomotion decision
        // sees same-frame movement. Wire `Entity.speed` is the
        // player's *capability* to move (40 = base run, 0 = bound),
        // NOT whether they're currently moving — see [`EntityMotion`]
        // module docs.
        app.init_resource::<combat_stance::EntityMotion>();
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            combat_stance::track_entity_motion_system.before(dat_vos2::tick_skinned_actors),
        );

        // Server doesn't send an explicit "clear your target" signal —
        // drop Target/LockOn refs to entities that fell out of the
        // snapshot or hit 0 HP. Runs *before* `sync_entities_system`
        // so the same-frame target panel + ring see the cleared state
        // immediately rather than flashing a stale highlight on the
        // dying-mob's last frame. Pulled out of the main chained
        // tuple because it pushed us past Bevy's system-tuple
        // `chain()` limit; explicit `.before()` preserves the
        // ordering we need.
        app.add_systems(
            Update,
            scene::auto_clear_target_system.before(sync_entities_system),
        );

        // Camera-mode reactor: hide the player's own avatar in first-person
        // so the camera doesn't render the inside of the skull/equipment.
        // Standalone (not part of the chained scene tuple above) because it
        // has no ordering relationship with anything in that pipeline — it
        // only reads `CameraMode` and writes `Visibility` on the `IsSelf`
        // entity, and Bevy's visibility propagation pass picks up the
        // change downstream.
        app.add_systems(Update, self_visibility_for_camera_mode_system);

        // Graphics-settings reactors: each fires on the frame the user
        // touches a knob, applies its slice of state, then sleeps until
        // the next change. Independent of each other and of the chained
        // scene pipeline above; no ordering constraints.
        app.add_systems(
            Update,
            (
                graphics_settings::apply_shadow_map_size_system,
                graphics_settings::apply_cascade_config_system,
                graphics_settings::apply_anti_aliasing_system,
                graphics_settings::apply_bloom_system,
                graphics_settings::apply_volumetric_fog_system,
                graphics_settings::apply_projection_system,
                graphics_settings::apply_vsync_system,
            )
                .run_if(resource_changed::<GraphicsSettings>),
        );
    }
}
