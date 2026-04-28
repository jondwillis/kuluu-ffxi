//! Keyboard → `AgentCommand` for the native viewer.
//!
//! Bevy's `ButtonInput<KeyCode>` gives proper press/hold/release from the OS,
//! so we don't need the kitty-protocol fallback that view3d's terminal-driven
//! input has — held-key polling Just Works.
//!
//! Bindings (mirroring the diagnostics-bar hint):
//!   W/Up    forward       S/Down   back
//!   A/Left  rotate left   D/Right  rotate right
//!   Tab     cycle target by 2D distance from self
//!   Esc     disconnect cleanly + exit

use bevy::input::ButtonInput;
use bevy::prelude::*;
use ffxi_viewer_core::SceneState;
use ffxi_viewer_wire::{Entity as WireEntity, Vec3 as WireVec3};
use tokio::sync::mpsc;

use crate::state::AgentCommand;

/// 20 Hz movement: distance per tick (5 u/s — FFXI normal run speed).
const MOVE_STEP_HELD: f32 = 0.25;
/// 20 Hz rotation: heading delta per tick (~56 °/s).
const ROTATE_STEP_HELD: u8 = 2;

#[derive(Resource, Clone)]
pub struct CommandTx(pub mpsc::Sender<AgentCommand>);

/// Currently-selected target. Tab cycles by 2D distance from self.
#[derive(Resource, Default)]
pub struct Target {
    pub id: Option<u32>,
}

/// Esc → disconnect + exit, Tab → cycle target. Press-edge events only;
/// movement is handled by [`dispatch_movement_system`] on FixedUpdate.
pub fn handle_input_system(
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<SceneState>,
    cmd_tx: Res<CommandTx>,
    mut target: ResMut<Target>,
    mut exit: MessageWriter<AppExit>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        let _ = cmd_tx.0.try_send(AgentCommand::Disconnect);
        exit.write_default();
        return;
    }
    if keys.just_pressed(KeyCode::Tab) {
        target.id = next_target_by_distance(
            &state.snapshot.entities,
            state.snapshot.self_pos.pos,
            target.id,
        );
    }
}

/// 20 Hz movement dispatch. Reads the latest server-echoed self position
/// from the snapshot, applies one tick of held-key motion, fires
/// `AgentCommand::Move`. Forward+Back cancel; Left+Right cancel.
pub fn dispatch_movement_system(
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<SceneState>,
    cmd_tx: Res<CommandTx>,
) {
    let mut forward: i32 = 0;
    let mut rotate: i32 = 0;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        forward += 1;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        forward -= 1;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        rotate -= 1;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        rotate += 1;
    }
    if forward == 0 && rotate == 0 {
        return;
    }

    let self_pos = state.snapshot.self_pos;
    let mut heading = self_pos.heading;
    if rotate != 0 {
        let delta = (ROTATE_STEP_HELD as i32 * rotate).rem_euclid(256) as u8;
        heading = self_pos.heading.wrapping_add(delta);
    }
    let (mut x, mut y) = (self_pos.pos.x, self_pos.pos.y);
    if forward != 0 {
        let (fx, fy) = heading_to_forward(heading);
        let dist = MOVE_STEP_HELD * forward as f32;
        x += fx * dist;
        y += fy * dist;
    }

    let _ = cmd_tx.0.try_send(AgentCommand::Move {
        x,
        y,
        z: self_pos.pos.z,
        heading,
    });
}

/// FFXI heading 0..=255 → (forward.x, forward.y) unit vector. Mirrors
/// `state::heading_to_forward` for wire-type inputs.
fn heading_to_forward(heading: u8) -> (f32, f32) {
    let angle = (heading as f32) * std::f32::consts::TAU / 256.0;
    (angle.sin(), angle.cos())
}

/// Cycle target by 2D distance — wire-types version of
/// `state::next_target_by_distance`.
fn next_target_by_distance(
    entities: &[WireEntity],
    from: WireVec3,
    current: Option<u32>,
) -> Option<u32> {
    if entities.is_empty() {
        return None;
    }
    let mut order: Vec<(&WireEntity, f32)> = entities
        .iter()
        .map(|e| {
            let dx = e.pos.x - from.x;
            let dy = e.pos.y - from.y;
            (e, dx * dx + dy * dy)
        })
        .collect();
    order.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let ids: Vec<u32> = order.iter().map(|(e, _)| e.id).collect();
    match current.and_then(|id| ids.iter().position(|&i| i == id)) {
        Some(p) => Some(ids[(p + 1) % ids.len()]),
        None => Some(ids[0]),
    }
}
