//! Entity name labels rendered as Bevy UI nodes whose screen positions are
//! recomputed each frame from the owning `WorldEntity`'s 3D position.
//!
//! UI overlay (instead of `Text2d` in 3D space or sprite-based billboards)
//! was chosen because:
//!   - text stays readable at any camera angle (no Z-flicker, no perspective
//!     shrinkage when the entity is far away),
//!   - no additional textures or font atlases required beyond the default,
//!   - cheap: one `Camera::world_to_viewport` call + a `Node` write per label.
//!
//! # Lifecycle
//!
//! - Spawn: `sync_entities_system` calls [`spawn_nameplate`] when a wire
//!   entity with a non-empty `name` is first seen.
//! - Update: [`update_nameplates_system`] runs each frame; projects the
//!   owning entity's world position to screen and writes `Node.left`/`top`.
//! - Despawn: same system despawns any nameplate whose owner is gone, so
//!   we don't have to plumb cleanup through every despawn site.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::camera::OperatorCamera;
use crate::components::{Nameplate, WorldEntity};

/// Spawn a UI nameplate for a wire entity. Returns the spawned UI entity so
/// callers can keep a handle if they want; ignoring the return is fine —
/// `update_nameplates_system` reconciles via `entity_id`.
pub fn spawn_nameplate(commands: &mut Commands, entity_id: u32, name: &str, color: Color) -> Entity {
    commands
        .spawn((
            Nameplate { entity_id },
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(-1000.0),
                left: Val::Px(-1000.0),
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Text::new(name.to_string()),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(color),
            ));
        })
        .id()
}

/// Per-frame: project each nameplate owner's world position to viewport
/// coords, write into the UI node. Despawn orphaned nameplates whose
/// owning `WorldEntity` is gone.
///
/// The 2.4-unit Y offset places the label roughly above the head of the
/// default-sized capsule. Off-screen labels are pushed far off-canvas
/// (`-9999`) rather than hidden, so we don't have to manage `Visibility`
/// on each node — the cheap path stays cheap.
pub fn update_nameplates_system(
    cam_q: Query<(&Camera, &GlobalTransform), (With<OperatorCamera>, Without<WorldEntity>)>,
    world_q: Query<(&Transform, &WorldEntity), Without<Nameplate>>,
    mut nameplate_q: Query<(Entity, &Nameplate, &mut Node)>,
    mut commands: Commands,
) {
    let Ok((camera, cam_global)) = cam_q.single() else {
        return;
    };

    let mut pos_by_id: HashMap<u32, Vec3> = HashMap::new();
    for (t, w) in &world_q {
        pos_by_id.insert(w.id, t.translation);
    }

    for (ui_entity, np, mut node) in &mut nameplate_q {
        match pos_by_id.get(&np.entity_id) {
            Some(&world_pos) => {
                let head = world_pos + Vec3::Y * 2.4;
                match camera.world_to_viewport(cam_global, head) {
                    Ok(screen) => {
                        // Approximate horizontal centering: assume ~7 px per
                        // glyph and a typical name <= 16 chars. Refining
                        // would require text-bounds measurement; this is
                        // close enough that names sit visually centered.
                        node.left = Val::Px(screen.x - 40.0);
                        node.top = Val::Px(screen.y - 16.0);
                    }
                    Err(_) => {
                        node.left = Val::Px(-9999.0);
                        node.top = Val::Px(-9999.0);
                    }
                }
            }
            None => {
                commands.entity(ui_entity).despawn();
            }
        }
    }
}
