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

// A held lock pins the target: it must be released before ordinary targeting
// input can move or drop it (research/xim PlayerTargetSelector.kt:62,74,92,225
// — clear, party-slot, tab-cycle and click-target all return early while
// isTargetLocked(), with a sub-target carve-out at :74,:92). Losing the entity
// and zoning still clear it; those are not player targeting input.
pub fn suppresses_retarget(lock: &LockOn, sub_target_flow: bool) -> bool {
    lock.is_active() && !sub_target_flow
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
    use bevy::ecs::system::RunSystemOnce;

    fn targetable_entity(id: u32) -> kuluu_snapshot::Entity {
        kuluu_snapshot::Entity {
            id,
            act_index: 0,
            kind: kuluu_snapshot::EntityKind::Mob,
            name: None,
            pos: kuluu_snapshot::Vec3::default(),
            heading: 0,
            hp_pct: Some(100),
            bt_target_id: 0,
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            mount: None,
            status: 0,
            char_flags: Default::default(),
            name_vis: None,
        }
    }

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
    fn an_active_lock_suppresses_ordinary_retargeting() {
        let locked = LockOn { target_id: Some(7) };
        assert!(suppresses_retarget(&locked, false));
        assert!(!suppresses_retarget(&LockOn::default(), false));
    }

    #[test]
    fn the_sub_target_flow_retargets_through_an_active_lock() {
        let locked = LockOn { target_id: Some(7) };
        assert!(!suppresses_retarget(&locked, true));
    }

    #[test]
    fn engaged_goal_does_not_create_or_replace_a_camera_lock() {
        let mut scene = crate::snapshot::SceneState::default();
        scene.snapshot.current_goal = Some(kuluu_snapshot::ReactorGoal::Engaged {
            target_id: 42,
            attack_issued: true,
        });
        scene.snapshot.entities = vec![targetable_entity(7), targetable_entity(42)];

        let mut world = World::new();
        world.insert_resource(scene);
        world.insert_resource(crate::scene::Target::default());
        world.insert_resource(LockOn::default());
        world
            .run_system_once(crate::scene::auto_clear_target_system)
            .unwrap();
        assert_eq!(world.resource::<LockOn>().target_id, None);

        world.resource_mut::<LockOn>().target_id = Some(7);
        world
            .run_system_once(crate::scene::auto_clear_target_system)
            .unwrap();
        assert_eq!(world.resource::<LockOn>().target_id, Some(7));
    }
}
