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
use ffxi_viewer_wire::EntityKind;

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

/// Marker on the secondary coord line under a nameplate. Drawn one line
/// below the name in a smaller, dimmer style — `123.4 / 45.1 / -67.2`.
/// Per-frame system writes the owning entity's world translation here.
#[derive(Component)]
pub struct NameplateCoord;

/// Spawn a UI nameplate for a wire entity. Returns the spawned UI entity so
/// callers can keep a handle if they want; ignoring the return is fine —
/// `update_nameplates_system` reconciles via `entity_id`.
pub fn spawn_nameplate(
    commands: &mut Commands,
    entity_id: u32,
    kind: EntityKind,
    name: &str,
    color: Color,
) -> Entity {
    let owned = name.to_string();
    commands
        .spawn((
            Nameplate { entity_id, kind },
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(-1000.0),
                left: Val::Px(-1000.0),
                // Stack the name and the coord line vertically. Bevy UI
                // defaults to `Row`, which would put the coords on the
                // right of the name and break the centering math.
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
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
            // Coord line — dimmer, smaller. Filled per-frame by
            // `update_nameplates_system`. Starts empty so a freshly spawned
            // label without a world position doesn't flash placeholder text.
            p.spawn((
                NameplateCoord,
                Text::new(""),
                TextFont {
                    font_size: 10.0,
                    ..default()
                },
                TextColor(Color::srgba(0.78, 0.78, 0.82, 0.85)),
            ));
        })
        .id()
}

/// Build the label text for an entity. Mobs and pets get the `"Name 73%"`
/// HP suffix; PCs (including the local player) and NPCs/Other always show
/// the bare name — vanilla FFXI does not surface other players' HP, and
/// most NPCs lack HP at all. `hp_pct == None` also falls back to the bare
/// name (the wire flagged HP as unavailable for that entity this frame).
pub fn format_label(base_name: &str, hp_pct: Option<u8>, kind: EntityKind) -> String {
    let show_hp = matches!(kind, EntityKind::Mob | EntityKind::Pet);
    match (show_hp, hp_pct) {
        (true, Some(pct)) => format!("{base_name} {pct}%"),
        _ => base_name.to_string(),
    }
}

/// Format a world position as the second-line coord string on a nameplate.
/// One decimal of precision keeps the line short while still being useful
/// for sub-meter navigation. Order is FFXI-conventional X / Y / Z.
pub fn format_coord(pos: Vec3) -> String {
    format!("{:.1} / {:.1} / {:.1}", pos.x, pos.y, pos.z)
}

/// Per-frame: project each nameplate owner's world position to viewport
/// coords, write into the UI node. Despawn orphaned nameplates whose
/// owning `WorldEntity` is gone.
///
/// The 2.4-unit Y offset places the label roughly above the head of the
/// default-sized capsule. Off-screen labels are pushed far off-canvas
/// (`-9999`) rather than hidden, so we don't have to manage `Visibility`
/// on each node — the cheap path stays cheap.
///
/// We read the camera's local `Transform` rather than `GlobalTransform`
/// and convert via `GlobalTransform::from(*cam_t)`. Reason: this system
/// runs in `Update`, and `GlobalTransform` propagation lives in
/// `PostUpdate::TransformSystem::TransformPropagate` — so the global
/// version we'd otherwise see is one frame stale, which produces a visible
/// nameplate-vs-capsule desync (the capsule renders against this frame's
/// global, the label against last frame's). The operator camera has no
/// parent, so `local == global` and the construction is exact.
pub fn update_nameplates_system(
    state: Res<SceneState>,
    cam_q: Query<(&Camera, &Transform), (With<OperatorCamera>, Without<WorldEntity>)>,
    world_q: Query<(&Transform, &WorldEntity), Without<Nameplate>>,
    mut nameplate_q: Query<(Entity, &Nameplate, &mut Node, &Children)>,
    mut label_q: Query<(&NameplateLabel, &mut Text), Without<NameplateCoord>>,
    mut coord_q: Query<&mut Text, (With<NameplateCoord>, Without<NameplateLabel>)>,
    mut commands: Commands,
) {
    let Ok((camera, cam_t)) = cam_q.single() else {
        return;
    };
    let cam_global = GlobalTransform::from(*cam_t);

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
                match camera.world_to_viewport(&cam_global, head) {
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
                let coord_str = format_coord(world_pos);
                for child in children.iter() {
                    if let Ok((label, mut text)) = label_q.get_mut(child) {
                        let want = format_label(&label.base_name, hp_pct, np.kind);
                        if **text != want {
                            **text = want;
                        }
                    } else if let Ok(mut text) = coord_q.get_mut(child) {
                        if **text != coord_str {
                            **text = coord_str.clone();
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
