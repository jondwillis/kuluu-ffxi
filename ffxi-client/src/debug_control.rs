//! Focus-less GUI driving bridge (kuluu-0pof). The agent socket is a command
//! *source* into the session; it cannot otherwise reach the Bevy-side input
//! path (`view_native::input::dispatch_movement_system`) where WASD movement and
//! re-grounding live. This shared handle is written by the socket command
//! decoder ([`crate::agent_codec`]) and read by the GUI each frame, so a remote
//! driver can inject movement input and trigger `/debug heights` without OS
//! keystrokes.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type SharedDebugControl = Arc<Mutex<DebugControl>>;

#[derive(Default)]
pub struct DebugControl {
    drive: Option<DebugDrive>,
    heights_seq: u64,
}

#[derive(Clone, Copy)]
struct DebugDrive {
    forward: i32,
    strafe: i32,
    until: Instant,
}

impl DebugControl {
    pub fn new_shared() -> SharedDebugControl {
        Arc::new(Mutex::new(Self::default()))
    }

    /// Hold a simulated movement input for `duration_ms`. `forward`/`strafe` are
    /// the same {-1,0,1} axes `resolve_move_inputs` produces from held keys.
    pub fn set_drive(&mut self, forward: i32, strafe: i32, duration_ms: u64) {
        self.drive = Some(DebugDrive {
            forward: forward.clamp(-1, 1),
            strafe: strafe.clamp(-1, 1),
            until: Instant::now() + Duration::from_millis(duration_ms),
        });
    }

    /// Live movement override, or `None` once the hold expires.
    pub fn active_drive(&self, now: Instant) -> Option<(i32, i32)> {
        self.drive
            .filter(|d| now < d.until)
            .map(|d| (d.forward, d.strafe))
    }

    pub fn request_heights(&mut self) {
        self.heights_seq = self.heights_seq.wrapping_add(1);
    }

    pub fn heights_seq(&self) -> u64 {
        self.heights_seq
    }
}
