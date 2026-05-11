//! Flat ground ring drawn under the currently-targeted entity.
//!
//! The kind/target material swap in `scene::sync_entities_system` already
//! highlights the targeted body, but in busy scenes the colour change is
//! easy to miss. A bright yellow ring on the ground gives the operator a
//! cheap, unmistakable visual cue.
//!
//! Implementation: re-emitted every frame via [`Gizmos`] (no mesh-asset
//! bookkeeping). Placed slightly above `y=0` so it sits *on* the ground
//! plane spawned by `scene::setup_world`. Skips entirely when no target
//! is set.

use std::f32::consts::PI;

use bevy::prelude::*;

use crate::components::WorldEntity;
use crate::scene::{feet_offset, Target};

/// Bevy world units. Tuned so the ring reads clearly around the default
/// PC capsule footprint without overlapping neighbours in tight clusters.
const RING_RADIUS: f32 = 1.5;

/// Lift above the entity's ground level to avoid z-fighting with
/// the navmesh / floor. Applied to the per-entity foot position, not
/// to a hardcoded y=0 — entities now sit at navmesh-height (variable)
/// rather than on a flat plane.
const RING_Y_LIFT: f32 = 0.05;

/// Bright yellow matching `EntityMaterials::target` so the ring colour
/// reads as "the same kind of attention" as the body emissive.
const RING_COLOR: Color = Color::srgb(1.0, 0.95, 0.20);

/// Draw a flat ring at the targeted entity's xz, every frame.
///
/// Runs in `Update` after `sync_entities_system` so the `Target` resource
/// and any newly-spawned `WorldEntity` transforms have been reconciled.
pub fn draw_target_ring_system(
    target: Res<Target>,
    world_q: Query<(&Transform, &WorldEntity)>,
    mut gizmos: Gizmos,
) {
    let Some(target_id) = target.id else {
        return;
    };

    for (t, w) in &world_q {
        if w.id == target_id {
            // Entity's center is at navmesh_h + feet_offset; subtract
            // feet_offset to land back at the navmesh-ground level
            // for this entity, then lift slightly to avoid z-fight.
            let ground_y = t.translation.y - feet_offset(w.kind) + RING_Y_LIFT;
            let pos = Vec3::new(t.translation.x, ground_y, t.translation.z);
            // Default circle is in the xy plane; rotate -90° around X so it
            // lies flat on xz.
            gizmos.circle(
                Isometry3d::new(pos, Quat::from_rotation_x(-PI / 2.0)),
                RING_RADIUS,
                RING_COLOR,
            );
            break;
        }
    }
}
