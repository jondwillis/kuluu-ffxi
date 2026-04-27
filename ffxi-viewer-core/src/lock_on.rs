use bevy::prelude::*;

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct LockOn {
    pub target_id: Option<u32>,
}

impl LockOn {
    pub fn is_active(&self) -> bool {
        self.target_id.is_some()
    }

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

pub fn auto_lock_transition(
    engaged: Option<u32>,
    prev_engaged: Option<u32>,
    current_lock: Option<u32>,
) -> Option<Option<u32>> {
    if engaged == prev_engaged {
        return None;
    }
    match engaged {
        Some(t) => Some(Some(t)),
        None if current_lock == prev_engaged => Some(None),
        None => None,
    }
}

pub fn auto_lock_on_when_engaged(
    scene: Res<crate::snapshot::SceneState>,
    mut lock_on: ResMut<LockOn>,
    mut prev_engaged: Local<Option<u32>>,
) {
    let engaged = match scene.snapshot.current_goal {
        Some(ffxi_viewer_wire::ReactorGoal::Engaged { target_id, .. }) => Some(target_id),
        _ => None,
    };
    if engaged == *prev_engaged {
        return;
    }
    if let Some(new_lock) = auto_lock_transition(engaged, *prev_engaged, lock_on.target_id) {
        lock_on.target_id = new_lock;
    }
    *prev_engaged = engaged;
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

    #[test]
    fn engaging_auto_locks_the_engaged_target() {
        assert_eq!(auto_lock_transition(Some(42), None, None), Some(Some(42)));
    }

    #[test]
    fn re_engaging_a_new_target_relocks_onto_it() {
        assert_eq!(
            auto_lock_transition(Some(7), Some(42), Some(42)),
            Some(Some(7))
        );
    }

    #[test]
    fn disengaging_releases_the_auto_lock() {
        assert_eq!(auto_lock_transition(None, Some(42), Some(42)), Some(None));
    }

    #[test]
    fn disengaging_leaves_a_manual_relock_untouched() {
        assert_eq!(auto_lock_transition(None, Some(42), Some(99)), None);
    }

    #[test]
    fn no_engage_transition_is_a_noop() {
        assert_eq!(auto_lock_transition(Some(42), Some(42), Some(42)), None);
        assert_eq!(auto_lock_transition(None, None, Some(99)), None);
    }
}
