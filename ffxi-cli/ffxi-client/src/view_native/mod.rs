//! Native windowed Bevy viewer.
//!
//! Sibling to `view3d/` (which projects 3D into a terminal). This module
//! opens a real OS window via `bevy_winit`, runs the viewer-core plugin,
//! and reads the same in-process `watch::Receiver<SessionState>` /
//! `broadcast::Receiver<AgentEvent>` that the TUI uses.
//!
//! # Threading
//!
//! Bevy's winit-driven event loop must run on the **main thread on macOS**
//! (Cocoa requires it). The caller — `main.rs::run_native` — handles this:
//! it builds an explicit tokio Runtime, runs auth/lobby preflight via
//! `block_on`, spawns the session task, then invokes `view_native::run`
//! synchronously on the main thread. We do NOT use `spawn_blocking` here.

pub mod bridge;
pub mod input;

use anyhow::Result;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use ffxi_viewer_core::{ViewerCorePlugin, WorldEntity};
use tokio::sync::{broadcast, mpsc, watch};

use crate::state::{AgentCommand, AgentEvent, SessionState};

use self::bridge::NativeSource;
use self::input::{CommandTx, Target};

pub fn run(
    state_rx: watch::Receiver<SessionState>,
    event_rx: broadcast::Receiver<AgentEvent>,
    cmd_tx: mpsc::Sender<AgentCommand>,
) -> Result<()> {
    let mut app = App::new();

    // `LogPlugin` would install its own tracing subscriber and clobber the
    // one main.rs sets up. Remove it; let main.rs's stderr-routed logger
    // own logging.
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "ffxi-client (native viewer)".into(),
                    resolution: (1280u32, 800u32).into(),
                    ..default()
                }),
                ..default()
            })
            .build()
            .disable::<LogPlugin>(),
    );

    // 20 Hz movement dispatch — match the wire-side keepalive cadence used
    // by view3d's input.rs. Without this override, FixedUpdate runs at
    // Bevy's default 64 Hz, which floods the session with `Move` commands.
    app.insert_resource(Time::<Fixed>::from_hz(20.0))
        .insert_resource(NativeSource::new(state_rx, event_rx))
        .insert_resource(CommandTx(cmd_tx))
        .init_resource::<Target>()
        .add_plugins(ViewerCorePlugin::<NativeSource>::default())
        .add_systems(Update, (input::handle_input_system, target_visual_system))
        .add_systems(FixedUpdate, input::dispatch_movement_system);

    app.run();
    Ok(())
}

/// Minimal target highlight — scale the selected entity 1.4x so the user
/// can see Tab is working. Proper outline/ring visualization lands with
/// Stage 0e (nameplate billboards + roster). Cheap: only writes Transform
/// for entities whose state changed since last frame.
fn target_visual_system(
    target: Res<Target>,
    mut last_target: Local<Option<u32>>,
    mut q: Query<(&WorldEntity, &mut Transform)>,
) {
    if target.id == *last_target {
        return;
    }
    let prev = *last_target;
    let next = target.id;
    for (we, mut xform) in &mut q {
        if Some(we.id) == prev {
            xform.scale = Vec3::ONE;
        }
        if Some(we.id) == next {
            xform.scale = Vec3::splat(1.4);
        }
    }
    *last_target = target.id;
}
