//! Gamepad vibration for particle generators that carry the sec2 0x82 / sec3 0x5F pair
//! (research/xim ParticleInitializers.kt CameraShakeSetup, ParticleUpdaters.kt
//! CameraShakeUpdater). xim labels the pair "camera shake"; no retail observation record yet
//! settles whether retail drives the pad or the screen from it. The envelope track is PS2-rescaled
//! at spawn (ffxi-dat/src/particle_gen.rs ps2_float_rescale); intensity per frame is
//! `envelope(progress) × falloff(camera distance)` sent through bevy's rumble pipeline —
//! bevy_gilrs' PostUpdate handler turns the requests into gilrs force feedback.

use std::time::{Duration, Instant};

use bevy::ecs::message::MessageWriter;
use bevy::input::gamepad::{Gamepad, GamepadRumbleIntensity, GamepadRumbleRequest};
use bevy::prelude::*;

use crate::camera::OperatorCamera;
use crate::graphics_settings::GraphicsSettings;
use crate::scheduler_runtime::ROUTINE_FPS;
use ffxi_dat::particle_gen::KeyFrameTrack;

/// Full inside `near`, linear to zero at `far` — the camera-distance law both the sec3 0x5F
/// rumble falloff and the sec3 0x2E draw-distance alpha fade apply (research/XIClient/src/
/// XIClient/source/World/Generator/CYyGenerator.cpp CYyGenerator::ElemIdle cases 0x2E/0x48;
/// research/xim Utils.kt fallOff).
pub(crate) fn distance_falloff(dist: f32, near: f32, far: f32) -> f32 {
    if dist <= near || far <= near {
        1.0
    } else if dist >= far {
        0.0
    } else {
        (far - dist) / (far - near)
    }
}

/// One Add request outlives a frame by this much, so the pad never goes quiet between
/// 30 fps ticks while the generator is alive; each frame's Stop+Add replaces it.
const RUMBLE_REQUEST_DURATION_MS: u64 = 100;

/// Below this intensity the motors are indistinguishable from off; skip the request pair.
const RUMBLE_MIN_INTENSITY: f32 = 0.01;

/// One live rumble generator: the sec2 0x82 envelope track and the sec3 0x5F
/// camera-distance falloff, over the particle's authored life.
#[derive(Component)]
pub struct RumbleSource {
    pub envelope: KeyFrameTrack,
    /// Full intensity inside this camera-to-particle distance (sec3 0x5F near).
    pub near: f32,
    /// Zero intensity beyond this camera-to-particle distance (sec3 0x5F far).
    pub far: f32,
    started_at: Instant,
    duration_secs: f32,
    was_active: bool,
}

impl RumbleSource {
    pub fn new(envelope: KeyFrameTrack, near: f32, far: f32, life_frames: f32) -> Self {
        Self {
            envelope,
            near,
            far,
            started_at: Instant::now(),
            duration_secs: (life_frames / ROUTINE_FPS).max(1.0 / ROUTINE_FPS),
            was_active: false,
        }
    }

    /// Full inside `near`, linear to zero at `far` — the sec3 0x5F law.
    fn falloff(&self, dist: f32) -> f32 {
        distance_falloff(dist, self.near, self.far)
    }

    fn stop(
        &mut self,
        pads: &Query<Entity, With<Gamepad>>,
        requests: &mut MessageWriter<GamepadRumbleRequest>,
    ) {
        if !self.was_active {
            return;
        }
        for pad in pads.iter() {
            requests.write(GamepadRumbleRequest::Stop { gamepad: pad });
        }
        self.was_active = false;
    }
}

/// Per-frame rumble drive. No camera, no vibration setting, or no connected pad makes this
/// a silent no-op (headless and keyboard-only sessions never touch the message queue).
pub fn update_rumble_system(
    settings: Res<GraphicsSettings>,
    q_cam: Query<&Transform, With<OperatorCamera>>,
    mut q_rumble: Query<(Entity, &Transform, &mut RumbleSource)>,
    q_pads: Query<Entity, With<Gamepad>>,
    mut requests: MessageWriter<GamepadRumbleRequest>,
    mut commands: Commands,
) {
    let Ok(cam) = q_cam.single() else {
        return;
    };
    for (entity, xf, mut src) in q_rumble.iter_mut() {
        let progress = src.started_at.elapsed().as_secs_f32() / src.duration_secs;
        if progress >= 1.0 {
            src.stop(&q_pads, &mut requests);
            commands.entity(entity).despawn();
            continue;
        }
        let dist = cam.translation.distance(xf.translation);
        let intensity = (src.envelope.sample(progress) * src.falloff(dist)).clamp(0.0, 1.0);
        if settings.vibration && intensity >= RUMBLE_MIN_INTENSITY {
            for pad in q_pads.iter() {
                requests.write(GamepadRumbleRequest::Stop { gamepad: pad });
                requests.write(GamepadRumbleRequest::Add {
                    duration: Duration::from_millis(RUMBLE_REQUEST_DURATION_MS),
                    intensity: GamepadRumbleIntensity {
                        strong_motor: intensity,
                        weak_motor: intensity,
                    },
                    gamepad: pad,
                });
            }
            src.was_active = true;
        } else {
            src.stop(&q_pads, &mut requests);
        }
    }
}

pub struct RumblePlugin;

impl Plugin for RumblePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, update_rumble_system);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falloff_is_full_inside_near_and_zero_beyond_far() {
        let src = RumbleSource::new(KeyFrameTrack { points: vec![] }, 2.0, 10.0, 60.0);
        assert_eq!(src.falloff(0.0), 1.0);
        assert_eq!(src.falloff(2.0), 1.0);
        assert!((src.falloff(6.0) - 0.5).abs() < f32::EPSILON);
        assert_eq!(src.falloff(10.0), 0.0);
        assert_eq!(src.falloff(99.0), 0.0);
    }

    #[test]
    fn falloff_degenerates_to_full_when_far_does_not_exceed_near() {
        let src = RumbleSource::new(KeyFrameTrack { points: vec![] }, 5.0, 2.0, 60.0);
        assert_eq!(src.falloff(100.0), 1.0);
    }

    #[test]
    fn intensity_is_envelope_times_falloff() {
        let src = RumbleSource::new(
            KeyFrameTrack {
                points: vec![(0.0, 1.0), (1.0, 0.0)],
            },
            2.0,
            10.0,
            60.0,
        );
        // Mid-life envelope ≈ 0.5 at half progress; camera 4 yalms out → falloff 0.75.
        let mid = src.envelope.sample(0.5) * src.falloff(4.0);
        assert!((mid - 0.375).abs() < 1e-6);
    }
}
