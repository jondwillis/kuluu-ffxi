use std::f32::consts::PI;

use bevy::prelude::*;

use crate::ffxi_actor_render::FfxiRenderActor;
use crate::scene::Target;
use crate::skinned_ffxi_material::FfxiSkinnedMaterial;

const STROBE_DURATION: f32 = 2.1;

const STROBE_PULSES: f32 = 3.0;

const STROBE_PEAK: f32 = 0.6;

#[derive(Resource, Default)]
pub struct StrobeState {
    last_target: Option<u32>,
    active: Option<u32>,
    elapsed: f32,
}

pub fn strobe_intensity(elapsed: f32) -> f32 {
    let t = (elapsed / STROBE_DURATION).clamp(0.0, 1.0);
    (t * STROBE_PULSES * PI).sin().abs() * STROBE_PEAK
}

pub fn target_strobe_system(
    time: Res<Time>,
    target: Res<Target>,
    mut state: ResMut<StrobeState>,
    q_actors: Query<&FfxiRenderActor>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
) {
    let cur = target.id;
    if state.last_target != cur {
        if let Some(old) = state.active.take() {
            set_actor_highlight(old, 0.0, &q_actors, &mut materials);
        }
        state.last_target = cur;
        state.active = cur;
        state.elapsed = 0.0;
    }

    let Some(id) = state.active else {
        return;
    };

    state.elapsed += time.delta_secs();
    if state.elapsed >= STROBE_DURATION {
        set_actor_highlight(id, 0.0, &q_actors, &mut materials);
        state.active = None;
        return;
    }

    set_actor_highlight(
        id,
        strobe_intensity(state.elapsed),
        &q_actors,
        &mut materials,
    );
}

fn set_actor_highlight(
    world_id: u32,
    value: f32,
    q_actors: &Query<&FfxiRenderActor>,
    materials: &mut Assets<FfxiSkinnedMaterial>,
) {
    for actor in q_actors {
        if actor.world_id != world_id {
            continue;
        }
        for handle in actor.material_handles() {
            if let Some(m) = materials.get_mut(handle) {
                m.material_flags.flags.w = value;
            }
        }
        break;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intensity_zero_at_ends() {
        assert!(strobe_intensity(0.0).abs() < 1e-6);
        assert!(strobe_intensity(STROBE_DURATION).abs() < 1e-6);
    }

    #[test]
    fn intensity_clamps_after_end() {
        assert!(strobe_intensity(STROBE_DURATION * 2.0).abs() < 1e-6);
    }

    #[test]
    fn three_peaks_reach_configured_peak() {
        for k in [1.0_f32, 3.0, 5.0] {
            let t = k / (2.0 * STROBE_PULSES) * STROBE_DURATION;
            assert!(
                (strobe_intensity(t) - STROBE_PEAK).abs() < 1e-4,
                "peak at t={t} should reach STROBE_PEAK",
            );
        }
    }

    #[test]
    fn intensity_bounded() {
        for i in 0..=120 {
            let t = i as f32 / 120.0 * STROBE_DURATION;
            let v = strobe_intensity(t);
            assert!((0.0..=STROBE_PEAK + 1e-6).contains(&v), "t={t} v={v}");
        }
    }
}
