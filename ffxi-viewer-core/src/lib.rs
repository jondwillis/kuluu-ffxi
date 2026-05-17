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
pub mod camera;
pub mod components;
#[cfg(not(target_arch = "wasm32"))]
pub mod dat_mmb;
#[cfg(not(target_arch = "wasm32"))]
pub mod dat_mzb;
#[cfg(not(target_arch = "wasm32"))]
pub mod dat_vos2;
pub mod hud;
pub mod input_mode;
pub mod keybinds;
pub mod lock_on;
#[cfg(not(target_arch = "wasm32"))]
pub mod look_resolver;
pub mod mouse;
pub mod nameplate;
pub mod picking;
pub mod scene;
pub mod snapshot;
pub mod source;
pub mod sun_moon;
pub mod target_ring;
pub mod weather_fx;
pub mod zone_lines;

pub use camera::{
    chase_camera_system, firstperson_camera_system, heading_for_yaw, spawn_camera,
    toggle_camera_mode, yaw_for_heading, CameraMode, ChaseCamera, OperatorCamera,
};
pub use components::{EntityModel, HpIndicator, IsSelf, LookComp, Nameplate, WorldEntity};
pub use hud::{add_hud_spawners, HudPlugin};
pub use input_mode::{
    ChatBuffer, DialogCursor, InputMode, MenuKind, MenuLevel, MenuStack, PassiveCursorFocus,
    PassiveCursorState, QuickActionState, DIALOG_MAX_CHOICE,
};
pub use keybinds::{Action, Bindings, KeyBind, Modifiers, Preset};
pub use lock_on::{LockOn, ToggleResult as LockOnToggle};
pub use mouse::{CursorLockRequest, MousePlugin, MousePointer};
pub use picking::{click_to_target_system, resolve_click_target, ClickResolution, PickingPlugin};
pub use scene::{
    feet_offset, ffxi_to_bevy, process_entity_look_changes, setup_world, sync_aggro_system,
    sync_entities_system, sync_entity_looks_system, Aggroing, EntityMaterials, EntityMesh, HpBar,
    HpBarMesh, Target, TrackedEntities,
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
        app.init_resource::<SceneState>()
            .init_resource::<EventLog>()
            .init_resource::<TrackedEntities>()
            .init_resource::<Target>()
            .init_resource::<InputMode>()
            .init_resource::<Bindings>()
            .init_resource::<CameraMode>()
            .init_resource::<LockOn>()
            .init_resource::<ZoneLineState>()
            .init_resource::<atmosphere::ZoneAtmosphereProvider>()
            .init_resource::<atmosphere::LastAtmosphereZone>()
            .init_resource::<sun_moon::VanaSky>()
            .init_resource::<weather_fx::ActiveWeatherModifier>()
            .init_resource::<weather_fx::ParticleAssets>()
            .init_resource::<weather_fx::LightningState>()
            .init_resource::<weather_fx::CurrentWeather>()
            .init_resource::<hud::chat_panel::ChatScroll>()
            .init_resource::<hud::chat_panel::BattleScroll>()
            .init_resource::<hud::chat_panel::ChatScrollAccum>()
            .init_resource::<hud::chat_panel::BattleScrollAccum>()
            // PickingPlugin owns the mesh raycast backend + the click→target
            // reader. `DefaultPickingPlugins` (input/hover/interaction) is
            // already added by `DefaultPlugins` on both front-ends.
            .add_plugins(PickingPlugin)
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
                    process_entity_look_changes,
                    chase_camera_system,
                    firstperson_camera_system,
                    sync_aggro_system,
                    nameplate::update_nameplates_system,
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
    }
}
