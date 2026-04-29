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
use crate::snapshot::SceneState;

/// Marker on the inner `Text` child of a nameplate. Lets the per-frame
/// updater target the right child without ambiguity if extra siblings
/// (e.g. background sprites) get added later.
#[derive(Component)]
pub struct NameplateLabel {
    /// The owning entity id, copied from the parent `Nameplate` so the
    /// label-refresh path doesn't have to re-walk parents on every text
    /// write.
    pub entity_id: u32,
    /// The base name (without HP suffix) so we can rebuild the displayed
    /// string each frame without losing the original.
    pub base_name: String,
}

/// Spawn a UI nameplate for a wire entity. Returns the spawned UI entity so
/// callers can keep a handle if they want; ignoring the return is fine —
/// `update_nameplates_system` reconciles via `entity_id`.
pub fn spawn_nameplate(commands: &mut Commands, entity_id: u32, name: &str, color: Color) -> Entity {
    let owned = name.to_string();
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
                NameplateLabel {
                    entity_id,
                    base_name: owned.clone(),
                },
                Text::new(owned),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(color),
            ));
        })
        .id()
}

/// Build the label text for an entity. PCs/mobs/pets have HP and get a
/// `"Name 73%"` suffix; NPCs/Other show only the name. `hp_pct == None`
/// also falls back to the bare name (the wire flagged HP as unavailable
/// for that entity this frame).
pub fn format_label(base_name: &str, hp_pct: Option<u8>) -> String {
    match hp_pct {
        Some(pct) => format!("{base_name} {pct}%"),
        None => base_name.to_string(),
    }
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
    state: Res<SceneState>,
    cam_q: Query<(&Camera, &GlobalTransform), (With<OperatorCamera>, Without<WorldEntity>)>,
    world_q: Query<(&Transform, &WorldEntity), Without<Nameplate>>,
    mut nameplate_q: Query<(Entity, &Nameplate, &mut Node, &Children)>,
    mut label_q: Query<(&NameplateLabel, &mut Text)>,
    mut commands: Commands,
) {
    let Ok((camera, cam_global)) = cam_q.single() else {
        return;
    };

    let mut pos_by_id: HashMap<u32, Vec3> = HashMap::new();
    for (t, w) in &world_q {
        pos_by_id.insert(w.id, t.translation);
    }

    // HP lookup keyed by wire id. Only wire entities with HP are listed —
    // the synthetic self at id=0 isn't here, so its label stays bare.
    let mut hp_by_id: HashMap<u32, Option<u8>> = HashMap::new();
    for ent in &state.snapshot.entities {
        hp_by_id.insert(ent.id, ent.hp_pct);
    }

    for (ui_entity, np, mut node, children) in &mut nameplate_q {
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

                // Refresh the inner Text label. Skip the write when the
                // composed string hasn't changed so we don't trigger
                // Bevy's change-detection on every nameplate every frame.
                let hp_pct = hp_by_id.get(&np.entity_id).copied().flatten();
                for child in children.iter() {
                    if let Ok((label, mut text)) = label_q.get_mut(child) {
                        let want = format_label(&label.base_name, hp_pct);
                        if **text != want {
                            **text = want;
                        }
                    }
                }
            }
            None => {
                commands.entity(ui_entity).despawn();
            }
        }
    }
}
