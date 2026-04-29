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

pub mod camera;
pub mod components;
pub mod hud;
pub mod nameplate;
pub mod scene;
pub mod snapshot;
pub mod source;
pub mod target_ring;

pub use camera::{
    chase_camera_system, heading_for_yaw, spawn_camera, yaw_for_heading, ChaseCamera,
    OperatorCamera,
};
pub use components::{HpIndicator, IsSelf, Nameplate, WorldEntity};
pub use hud::HudPlugin;
pub use scene::{
    ffxi_to_bevy, setup_world, sync_entities_system, sync_aggro_system,
    Aggroing, EntityMaterials, EntityMesh, HpBar, HpBarMesh, Target, TrackedEntities,
};
pub use snapshot::{apply_delta, ingest_system, EventLog, SceneState, CHAT_HISTORY_CAP};
pub use source::SceneSource;

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
        app.init_resource::<SceneState>()
            .init_resource::<EventLog>()
            .init_resource::<TrackedEntities>()
            .init_resource::<Target>()
            .add_systems(PreUpdate, ingest_system::<S>)
            .add_systems(Startup, (setup_world, spawn_camera))
            .add_systems(
                Update,
                (
                    sync_entities_system,
                    chase_camera_system,
                    sync_aggro_system,
                    nameplate::update_nameplates_system,
                    target_ring::draw_target_ring_system,
                )
                    .chain(),
            )
            .add_plugins(HudPlugin);
    }
}
