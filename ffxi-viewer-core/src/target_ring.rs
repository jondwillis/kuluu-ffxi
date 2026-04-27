use std::f32::consts::{PI, TAU};

use bevy::prelude::*;

use crate::camera::{nameplate_anchor_y, OperatorCamera};
use crate::components::WorldEntity;
use crate::scene::{BakedActor, Target};
use crate::snapshot::SceneState;

const ARROW_COLOR: Color = Color::srgb(1.00, 0.96, 0.60);

const ARROW_ENGAGED_COLOR: Color = Color::srgb(1.00, 0.22, 0.26);

const ARROW_BORDER_COLOR: Color = Color::srgb(0.02, 0.02, 0.03);

const ARROW_WIDTH: f32 = 0.55;
const ARROW_HEIGHT: f32 = 0.42;

const ARROW_TIP_ABOVE_ANCHOR: f32 = 0.30;

const ARROW_BOB_AMPLITUDE: f32 = 0.08;
const ARROW_BOB_FREQUENCY: f32 = 3.0;

const ARROW_FILL_SCANLINES: u32 = 18;

const ARROW_BORDER_LIFT: f32 = 0.02;
const ARROW_BORDER_THICK: f32 = 0.02;

const RING_Y_LIFT: f32 = 0.08;

const TARGET_RING_RADIUS: f32 = 1.4;

const RING_NEUTRAL_RGB: [f32; 3] = [1.00, 0.82, 0.30];
const RING_ENGAGED_RGB: [f32; 3] = [1.00, 0.16, 0.12];

const RING_GLOW_GAIN: f32 = 5.0;

const RING_PULSE_HZ: f32 = 0.85;
const RING_PULSE_DEPTH: f32 = 0.35;
const RING_BREATH_DEPTH: f32 = 0.04;

const RING_THICKNESS: [f32; 3] = [-0.045, 0.0, 0.045];

const RING_TICKS: usize = 12;
const RING_TICK_LEN: f32 = 0.18;
const RING_SPIN_RATE: f32 = 0.7;

pub fn target_ring_color(engaged_on_target: bool) -> Color {
    if engaged_on_target {
        ARROW_ENGAGED_COLOR
    } else {
        ARROW_COLOR
    }
}

pub fn arrow_bob_offset(seconds: f32) -> f32 {
    (seconds * ARROW_BOB_FREQUENCY).sin() * ARROW_BOB_AMPLITUDE
}

fn engaged_on(state: &SceneState, target_id: u32) -> bool {
    state
        .snapshot
        .self_char_id
        .and_then(|sid| state.snapshot.entities.iter().find(|e| e.id == sid))
        .map(|self_pc| self_pc.bt_target_id == target_id)
        .unwrap_or(false)
}

pub fn draw_target_arrow_system(
    target: Res<Target>,
    state: Res<SceneState>,
    time: Res<Time>,
    cam_q: Query<&Transform, With<OperatorCamera>>,
    world_q: Query<(&Transform, &WorldEntity, Option<&BakedActor>)>,
    mut gizmos: Gizmos,
) {
    let Some(target_id) = target.id else {
        return;
    };
    let Ok(cam_t) = cam_q.single() else {
        return;
    };
    let cam_pos = cam_t.translation;

    let fill = target_ring_color(engaged_on(&state, target_id));

    for (t, w, baked) in &world_q {
        if w.id != target_id {
            continue;
        }

        let tip_y = t.translation.y
            + nameplate_anchor_y(baked)
            + ARROW_TIP_ABOVE_ANCHOR
            + arrow_bob_offset(time.elapsed_secs());
        let apex = Vec3::new(t.translation.x, tip_y, t.translation.z);
        draw_camera_facing_arrow(&mut gizmos, apex, cam_pos, fill, ARROW_BORDER_COLOR);
        break;
    }
}

fn draw_camera_facing_arrow(
    gizmos: &mut Gizmos,
    apex: Vec3,
    cam_pos: Vec3,
    fill: Color,
    border: Color,
) {
    let up = Vec3::Y;

    let to_cam = (cam_pos - apex).try_normalize().unwrap_or(Vec3::Z);
    let right = up.cross(to_cam).try_normalize().unwrap_or(Vec3::X);

    let top_center = apex + up * ARROW_HEIGHT;
    let half = right * (ARROW_WIDTH * 0.5);
    let top_left = top_center - half;
    let top_right = top_center + half;

    for i in 0..=ARROW_FILL_SCANLINES {
        let f = i as f32 / ARROW_FILL_SCANLINES as f32;
        let l = apex.lerp(top_left, f);
        let r = apex.lerp(top_right, f);
        gizmos.line(l, r, fill);
    }

    let toward = to_cam * ARROW_BORDER_LIFT;
    let offsets = [
        Vec3::ZERO,
        right * ARROW_BORDER_THICK,
        -right * ARROW_BORDER_THICK,
        up * ARROW_BORDER_THICK,
    ];
    for off in offsets {
        let a = apex + toward + off;
        let tl = top_left + toward + off;
        let tr = top_right + toward + off;
        gizmos.line(a, tl, border);
        gizmos.line(a, tr, border);
        gizmos.line(tl, tr, border);
    }
}

fn ring_pulse(seconds: f32) -> (f32, f32) {
    let phase = (seconds * RING_PULSE_HZ * TAU).sin();
    let brightness = RING_GLOW_GAIN * (1.0 - RING_PULSE_DEPTH * (0.5 - 0.5 * phase));
    let radius_scale = 1.0 + RING_BREATH_DEPTH * phase;
    (brightness, radius_scale)
}

pub fn draw_target_ring_system(
    target: Res<Target>,
    state: Res<SceneState>,
    time: Res<Time>,
    world_q: Query<(&Transform, &WorldEntity)>,
    mut gizmos: Gizmos,
) {
    let Some(target_id) = target.id else {
        return;
    };
    let base = if engaged_on(&state, target_id) {
        RING_ENGAGED_RGB
    } else {
        RING_NEUTRAL_RGB
    };

    let t = time.elapsed_secs();
    let (brightness, radius_scale) = ring_pulse(t);
    let color = LinearRgba::rgb(
        base[0] * brightness,
        base[1] * brightness,
        base[2] * brightness,
    );
    let radius = TARGET_RING_RADIUS * radius_scale;

    for (tr, w) in &world_q {
        if w.id != target_id {
            continue;
        }
        let pos = Vec3::new(
            tr.translation.x,
            tr.translation.y + RING_Y_LIFT,
            tr.translation.z,
        );
        let iso = Isometry3d::new(pos, Quat::from_rotation_x(-PI / 2.0));

        for dr in RING_THICKNESS {
            gizmos.circle(iso, radius + dr, color);
        }

        let spin = t * RING_SPIN_RATE;
        for i in 0..RING_TICKS {
            let a = spin + (i as f32 / RING_TICKS as f32) * TAU;
            let dir = Vec3::new(a.cos(), 0.0, a.sin());
            let inner = pos + dir * (radius - RING_TICK_LEN);
            let outer = pos + dir * (radius + RING_TICK_LEN);
            gizmos.line(inner, outer, color);
        }
        break;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engaged_target_uses_red() {
        assert_eq!(target_ring_color(true), ARROW_ENGAGED_COLOR);
    }

    #[test]
    fn unengaged_target_uses_neutral() {
        assert_eq!(target_ring_color(false), ARROW_COLOR);
    }

    #[test]
    fn engaged_and_unengaged_colors_differ() {
        assert_ne!(ARROW_COLOR, ARROW_ENGAGED_COLOR);
    }

    #[test]
    fn border_contrasts_with_both_fills() {
        assert_ne!(ARROW_BORDER_COLOR, ARROW_COLOR);
        assert_ne!(ARROW_BORDER_COLOR, ARROW_ENGAGED_COLOR);
    }

    #[test]
    fn bob_is_bounded_by_amplitude() {
        for i in 0..64 {
            let s = i as f32 * 0.1;
            assert!(arrow_bob_offset(s).abs() <= ARROW_BOB_AMPLITUDE + 1e-6);
        }
    }

    #[test]
    fn bob_starts_at_rest() {
        assert!(arrow_bob_offset(0.0).abs() < 1e-6);
    }

    #[test]
    fn ring_pulse_brightness_is_hdr_and_bounded() {
        let lo = RING_GLOW_GAIN * (1.0 - RING_PULSE_DEPTH);
        for i in 0..240 {
            let (b, _) = ring_pulse(i as f32 * 0.05);
            assert!(b > 1.0, "ring glow must stay HDR to bloom (b={b})");
            assert!(
                (lo - 1e-4..=RING_GLOW_GAIN + 1e-4).contains(&b),
                "brightness {b} out of [{lo}, {RING_GLOW_GAIN}]",
            );
        }
    }

    #[test]
    fn ring_pulse_radius_breathes_within_depth() {
        for i in 0..240 {
            let (_, r) = ring_pulse(i as f32 * 0.05);
            assert!((1.0 - RING_BREATH_DEPTH - 1e-4..=1.0 + RING_BREATH_DEPTH + 1e-4).contains(&r));
        }
    }
}
