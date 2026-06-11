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
// Insurmountable for a Bevy ECS crate: system signatures are dictated by the
// framework — a system's parameter list IS its dependency set (often >7 Res /
// Query params), and `Query<...>` filter/data tuples are inherently deep. The
// idiomatic alternatives (SystemParam bundles, query type aliases) don't reduce
// the real complexity, they only move it. Scoped to exactly these two lints.
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
pub mod graphics_settings;
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
pub mod target_strobe;
pub mod vana_time;
#[cfg(not(target_arch = "wasm32"))]
pub mod weather;
pub mod weather_fx;
pub mod zone_clouds;
pub mod zone_lights;
pub mod zone_lines;

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
pub use cursor::{CursorAssets, CursorPlugin, CursorRequests, CursorStyle};
pub use graphics_settings::{
    AaMode, CharacterRenderPath, DynamicLights, GraphicsField, GraphicsSettings, QualityPreset,
    SkyStyle, GRAPHICS_FIELDS,
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

/// App-wide error handler that downgrades the "command targeted an entity
/// that despawned before the command flushed" race from a panic to a
/// warning, while letting every other error keep Bevy's default panic
/// behavior so genuine bugs still surface.
///
/// Why this is needed: in Bevy 0.18 `EntityCommands::remove` / `despawn`
/// already swallow this race — both queue with the `warn` handler — but
/// `insert` queues with the *default* handler, which panics
/// (`bevy_ecs::error::handler::panic`). In a server-synced client,
/// snapshot churn despawns actors constantly (mob death, zone-out, logout
/// teardown); any system that captured an actor's `Entity` and then
/// `insert`s a component on it after the actor despawned — same-frame, or
/// a stale handle held across a despawn+slot-reuse — would otherwise take
/// down the whole session. Per-site `try_insert` covers the known hot
/// paths (scene/look/dat_vos2 actor sync); this handler is the safety net
/// for every other present and future `insert` site.
///
/// Scope is deliberately narrow: only `ErrorContext::Command` errors whose
/// payload is an entity-not-spawned/despawned error are tolerated. A
/// fallible system returning `Err`, an aliased-mutability command bug, or
/// any other error still routes to `panic`.
fn tolerate_command_entity_despawn(
    error: bevy::ecs::error::BevyError,
    ctx: bevy::ecs::error::ErrorContext,
) {
    use bevy::ecs::error::ErrorContext;
    use bevy::ecs::world::error::EntityMutableFetchError;

    // Entity commands fail with `EntityMutableFetchError::NotSpawned(..)`
    // (the `get_entity_mut` path); a few commands surface the inner
    // `EntityNotSpawnedError` directly. Match both, but never the
    // `AliasedMutability` variant — that's a real double-borrow bug.
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
        // Install the despawn-race-tolerant command error handler before
        // anything else so it's in force for every system this plugin (and
        // its sub-plugins) registers. `set_error_handler` asserts it's
        // called at most once per `App`; this plugin is added exactly once
        // and no front-end sets its own, so the assert holds. See
        // [`tolerate_command_entity_despawn`] for the rationale.
        app.set_error_handler(tolerate_command_entity_despawn);

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
        app.add_plugins(moon_material::MoonMaterialPlugin);
        // FFXI-faithful skinned-character material (ported from FFXI /
        // research-xim). Coexists with StandardMaterial; the character
        // render path is selected by `GraphicsSettings::character_path`.
        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(skinned_ffxi_material::FfxiMaterialPlugin);
        // Screen-space lens flare — Enhanced sky style only; the system
        // gates itself on `SkyStyle` and sun visibility.
        app.add_plugins(lens_flare::LensFlarePlugin);
        // Dynamic local lights (lanterns/fires) from baked over-bright
        // MMB vertices. PointLights are Enhanced-only; flame sprites
        // show in both styles.
        app.add_plugins(zone_lights::ZoneLightsPlugin);
        // Faithful cloud mesh (the zone's "clod" MMB) — shown + drifted
        // in Retail style; hidden in Enhanced (procedural dome wins).
        app.add_plugins(zone_clouds::ZoneCloudsPlugin);
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
            // MSAA capability probe — replaced at Startup with the
            // real adapter caps; default is the WebGPU spec floor so
            // apply_anti_aliasing_system has something safe to clamp
            // against on frame 0.
            .init_resource::<graphics_settings::MsaaCaps>()
            // PreStartup: must run before `spawn_camera` (front-end
            // Startup system) which reads `settings.msaa()` and puts
            // the result on the camera entity. If we ran in Startup we
            // could end up after the camera spawn, and the render
            // world's first extract would already see the unsupported
            // sample count → panic in `prepare_view_targets`.
            .add_systems(PreStartup, graphics_settings::init_msaa_caps_system)
            .init_resource::<atmosphere::ZoneAtmosphereProvider>()
            .init_resource::<atmosphere::LastAtmosphereZone>()
            .init_resource::<sun_moon::VanaSky>()
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
            .add_message::<ToastEvent>()
            .add_systems(PreUpdate, ingest_system::<S>.run_if(resource_exists::<S>))
            // Single mutator of SceneState.local_toasts outside of
            // ingest_system. PostUpdate so all Update-stage toast
            // emissions land before next frame's chat-panel render.
            .add_systems(PostUpdate, drain_toast_events)
            // Drain VanaTimeSynced events out of the EventLog into the
            // VanaClock resource. Runs after ingest_system so the same
            // frame's events are visible.
            .add_systems(
                PreUpdate,
                vana_time::ingest_vana_time.after(ingest_system::<S>),
            )
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
                // Split into two nested tuples — Bevy 0.17's
                // `IntoSystemConfigs` tuple impls top out at 20
                // elements, and this chain currently has 21. `.chain()`
                // flattens nested tuples while preserving order, so the
                // execution semantics are identical to a flat 21-tuple.
                (
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
                        camera_transition_system,
                        chase_camera_system,
                        firstperson_camera_system,
                        sync_aggro_system,
                        nameplate::update_nameplates_system,
                        nameplate_billboard::update_nameplate_billboards_system,
                        target_strobe::target_strobe_system,
                        target_ring::draw_target_arrow_system,
                        target_ring::draw_engaged_ring_system,
                        sync_zone_lines_system,
                        atmosphere::apply_zone_atmosphere_system,
                    ),
                    (
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
                    ),
                )
                    .chain()
                    .run_if(resource_exists::<EntityMesh>),
            );

        // Phase 4a: the live character tick is now the faithful
        // `ffxi_actor_render::tick_live_ffxi_actors` (registered below,
        // after `track_entity_motion_system`). The legacy
        // `dat_vos2::tick_skinned_actors` (Bevy CPU-bake path) and
        // `dat_vos2::tick_ffxi_actors` (old GPU path) are left defined but
        // no longer registered — a later pass removes them once the new
        // path is confirmed in-game.

        // Faithful character lighting: pull the live zone sun / moon / ambient
        // into each faithful render-actor material's light uniform every frame,
        // and stamp the realistic-lighting toggle. Targets the live
        // `FfxiRenderActor` materials (the legacy
        // `dat_vos2::update_ffxi_lighting_system` queried the retired
        // `FfxiActor` component and never reached them).
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, ffxi_actor_render::update_ffxi_render_actor_lighting);

        // Per-entity motion tracker for combat-stance / locomotion
        // animation selection. Runs *before* `tick_skinned_actors`
        // (which reads `EntityMotion`) so the locomotion decision
        // sees same-frame movement. Wire `Entity.speed` is the
        // player's *capability* to move (40 = base run, 0 = bound),
        // NOT whether they're currently moving — see [`EntityMotion`]
        // module docs.
        app.init_resource::<combat_stance::EntityMotion>();
        app.init_resource::<combat_stance::RestStance>();
        app.init_resource::<combat_stance::AnimationBlends>();
        app.init_resource::<combat_stance::WalkMode>();
        app.init_resource::<camera::CameraTransition>();
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            combat_stance::track_entity_motion_system
                .before(ffxi_actor_render::tick_live_ffxi_actors),
        );
        // Faithful live character animation tick. Runs *after*
        // `track_entity_motion_system` (above) so its locomotion decision
        // sees same-frame motion state.
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, ffxi_actor_render::tick_live_ffxi_actors);

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
        // the next change. Chained so AA runs last — it despawns +
        // respawns the OperatorCamera entity to dodge Bevy's
        // pipeline-cache vs view-target sample-count race, which would
        // panic any sibling reactor that still holds the old entity id.
        // `build_operator_camera` re-reads bloom/fog/projection from
        // settings, so the earlier reactors' work on the old entity is
        // harmless when AA does respawn.
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
    }
}
