//! Lock-on: pin the player's heading to face a chosen target.
//!
//! When [`LockOn::target_id`] is `Some(id)` and the entity is in the
//! current snapshot, the input layer overrides the heading derived from
//! WASD/A-D so the player's facing tracks the target every tick. The
//! camera follows naturally because chase-mode `yaw` is locked-stepped
//! to heading by `dispatch_movement_system`'s rotate path. In FFXI
//! retail this is the "L" key; we use `H` since the laptop layout
//! we're testing on already binds L for camera mode toggle in some
//! external configs.
//!
//! No server-side packet is involved — lock-on is a client-side
//! input-layer behavior. The server just sees position + heading
//! updates as usual.
//!
//! Toggle semantics:
//! - First press with a target → lock on to current target.
//! - Press again → unlock.
//! - Press with no target → no-op (toast in the front-end).
//! - Lock-on auto-clears when the target leaves the snapshot (despawn,
//!   zone change, out-of-range).

use bevy::prelude::Resource;

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct LockOn {
    pub target_id: Option<u32>,
}

impl LockOn {
    pub fn is_active(&self) -> bool {
        self.target_id.is_some()
    }

    /// Toggle: clear if currently locked, otherwise lock to `target_id`
    /// (a no-op if `target_id` is `None`). Returns the new state for
    /// caller logging / UX feedback.
    pub fn toggle(&mut self, target_id: Option<u32>) -> ToggleResult {
        match (self.target_id, target_id) {
            (Some(_), _) => {
                self.target_id = None;
                ToggleResult::Cleared
            }
            (None, Some(id)) => {
                self.target_id = Some(id);
                ToggleResult::Locked(id)
            }
            (None, None) => ToggleResult::NoTarget,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToggleResult {
    Locked(u32),
    Cleared,
    NoTarget,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_locks_then_clears() {
        let mut lo = LockOn::default();
        assert_eq!(lo.toggle(Some(42)), ToggleResult::Locked(42));
        assert!(lo.is_active());
        assert_eq!(lo.toggle(Some(42)), ToggleResult::Cleared);
        assert!(!lo.is_active());
    }

    #[test]
    fn toggle_with_no_target_when_unlocked_is_noop() {
        let mut lo = LockOn::default();
        assert_eq!(lo.toggle(None), ToggleResult::NoTarget);
        assert!(!lo.is_active());
    }

    #[test]
    fn toggle_with_active_lock_always_clears_even_without_target_arg() {
        let mut lo = LockOn { target_id: Some(7) };
        assert_eq!(lo.toggle(None), ToggleResult::Cleared);
        assert!(!lo.is_active());
    }
}
