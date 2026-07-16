//! Repro for kuluu-y3o0: `bevy_light::check_dir_light_mesh_visibility` index-OOB
//! panic when the directional-light cascade count grows at runtime.
//!
//! Upstream bug (bevy_light 0.19.0, src/lib.rs:477): per-thread scratch queues are
//! only resized on threads that receive a batch for the current view; the
//! aggregation loop then indexes ALL thread-locals by cascade index, hitting
//! stale queues sized by an earlier (smaller) cascade count.
//!
//! Expected: panics on stock bevy_light 0.19.0, prints REPRO-OK with the
//! vendored patch in vendor/bevy_light (see [patch.crates-io] in Cargo.toml).
//!
//! Run: cargo run --example zz-cascade-panic-repro

use bevy::app::AppExit;
use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder};
use bevy::prelude::*;

const FRAMES: u32 = 120;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_systems(Startup, setup)
        .add_systems(Update, cycle_cascades)
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 40.0, 120.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder {
            num_cascades: 2,
            ..default()
        }
        .build(),
        Transform::from_xyz(50.0, 100.0, 50.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    // Enough shadow-casting meshes that visibility checking fans out across
    // many task-pool batches, populating per-thread scratch queues.
    let mesh = meshes.add(Cuboid::default());
    let mat = materials.add(StandardMaterial::default());
    for i in 0..4000u32 {
        let x = (i % 64) as f32 * 3.0 - 96.0;
        let z = (i / 64) as f32 * 3.0 - 96.0;
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mat.clone()),
            Transform::from_xyz(x, ((i * 7) % 13) as f32, z),
        ));
    }
}

/// Cycle cascade count 2 -> 3 -> 4 every few frames — the growth transitions
/// (2->3, 3->4) are what trip the unpatched aggregation loop.
fn cycle_cascades(
    mut frames: Local<u32>,
    mut q_light: Query<&mut CascadeShadowConfig, With<DirectionalLight>>,
    mut exit: MessageWriter<AppExit>,
) {
    *frames += 1;
    let n = match (*frames / 5) % 3 {
        0 => 2usize,
        1 => 3,
        _ => 4,
    };
    for mut cfg in &mut q_light {
        *cfg = CascadeShadowConfigBuilder {
            num_cascades: n,
            ..default()
        }
        .build();
    }
    if *frames >= FRAMES {
        println!("REPRO-OK: no panic after {FRAMES} frames of cascade cycling 2->3->4");
        exit.write(AppExit::Success);
    }
}
