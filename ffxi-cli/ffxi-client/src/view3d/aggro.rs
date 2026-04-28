//! Aggro chain indicator: any mob whose `bt_target_id` points at the
//! player gets a red+emissive material override and a gizmo line back
//! to the player. The marker `Aggroing` lives on the ECS entity for
//! the aggroing mob; precedence over `Target` is enforced by running
//! this system *after* `sync_entities_system`, so we have the last
//! word on material assignment.
//!
//! Spike outcome on the gizmo line: `bevy_ratatui_camera` 0.16 reads
//! the same image-target framebuffer the standard render pipeline
//! writes to (`camera_readback.rs` upstream), so gizmos are picked up
//! the same way capsule meshes are. At halfblock resolution thin
//! lines may subpixel-render to invisibility — if that happens we'll
//! swap to a thin Cuboid mesh, but gizmos stay the default because
//! they're allocation-free and a single configuration constant away
//! from the fallback.

use std::collections::HashSet;

use bevy::color::Color;
use bevy::prelude::*;

use crate::state::{EntityKind, SessionState};

use super::bridge::SessionStateSnapshot;
use super::scene::{EntityPalette, IsSelf, Target, WorldEntity};

/// Marker for entities currently aggroing the player. Inserted by
/// `sync_aggro_system` and removed when `bt_target_id` no longer
/// matches `state.char_id`. The presence of this marker is the
/// authoritative signal that an entity should render with the aggro
/// material; never write to `palette.aggro` without inserting it,
/// otherwise the next snapshot tick will cheerfully overwrite the
/// material back to the kind/target one.
#[derive(Component)]
pub struct Aggroing;

/// Pure aggro selector: which entity ids in the snapshot are
/// currently aggroing the player. Returns empty if `char_id` is
/// unknown (login isn't complete yet) — there's no way to be aggroed
/// before we know our own UniqueNo.
///
/// PCs and NPCs are filtered out: a PC's `bt_target_id` happens to
/// point at us during PvP and an NPC's during escort flagging, but
/// neither generates the threat-display semantics this overlay is
/// trying to surface.
pub fn aggroing_ids(state: &SessionState) -> HashSet<u32> {
    let mut out = HashSet::new();
    let Some(self_id) = state.char_id else { return out };
    for ent in &state.entities {
        if ent.bt_target_id != self_id {
            continue;
        }
        if matches!(ent.kind, EntityKind::Pc | EntityKind::Npc) {
            continue;
        }
        out.insert(ent.id);
    }
    out
}

/// Per-tick: reconcile the `Aggroing` marker on each ECS entity and
/// override its material. Runs in `Update` after `sync_entities_system`
/// so any kind/target material write from that pass is overwritten on
/// the same frame for entities that just became aggro.
pub fn sync_aggro_system(
    mut commands: Commands,
    snapshot: Res<SessionStateSnapshot>,
    target: Res<Target>,
    palette: Option<Res<EntityPalette>>,
    self_q: Query<&Transform, (With<IsSelf>, Without<WorldEntity>)>,
    mut entity_q: Query<
        (
            bevy::prelude::Entity,
            &WorldEntity,
            &Transform,
            &mut MeshMaterial3d<StandardMaterial>,
            Option<&Aggroing>,
        ),
        Without<IsSelf>,
    >,
    mut gizmos: Gizmos,
) {
    let Some(palette) = palette else { return };
    let state = &snapshot.0;
    let aggroing = aggroing_ids(state);
    let self_pos: Option<Vec3> = self_q.single().ok().map(|t| t.translation);

    for (e, we, t, mut m, has_aggro) in entity_q.iter_mut() {
        let should_aggro = aggroing.contains(&we.id);
        match (should_aggro, has_aggro.is_some()) {
            (true, false) => {
                commands.entity(e).insert(Aggroing);
                m.0 = palette.aggro.clone();
            }
            (true, true) => {
                if m.0 != palette.aggro {
                    m.0 = palette.aggro.clone();
                }
            }
            (false, true) => {
                commands.entity(e).remove::<Aggroing>();
                // Restore the non-aggro material immediately. We
                // can't rely on `sync_entities_system` running on a
                // frame where the snapshot is otherwise unchanged —
                // the marker change alone wouldn't dirty its skip
                // check, and we'd see a frozen-red mob until the
                // player moved or zoned.
                let restore = if Some(we.id) == target.id {
                    palette.target.clone()
                } else {
                    palette.material_for(we.kind)
                };
                m.0 = restore;
            }
            (false, false) => {}
        }

        if should_aggro {
            if let Some(sp) = self_pos {
                gizmos.line(sp, t.translation, Color::srgb(1.0, 0.15, 0.15));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Entity, EntityKind, Vec3 as ProtoVec3};

    fn ent(id: u32, kind: EntityKind, bt: u32) -> Entity {
        Entity {
            id,
            act_index: 0,
            kind,
            name: None,
            pos: ProtoVec3 { x: 0.0, y: 0.0, z: 0.0 },
            heading: 0,
            hp_pct: None,
            bt_target_id: bt,
        }
    }

    #[test]
    fn selector_returns_empty_when_no_self_id() {
        let mut s = SessionState::default();
        s.char_id = None;
        s.entities.push(ent(101, EntityKind::Mob, 999));
        assert!(aggroing_ids(&s).is_empty());
    }

    #[test]
    fn selector_picks_mobs_targeting_self() {
        let mut s = SessionState::default();
        s.char_id = Some(42);
        s.entities.push(ent(101, EntityKind::Mob, 42));
        s.entities.push(ent(102, EntityKind::Mob, 999));
        s.entities.push(ent(103, EntityKind::Pet, 42));
        let ids = aggroing_ids(&s);
        assert!(ids.contains(&101), "mob targeting self → aggro");
        assert!(!ids.contains(&102), "mob targeting someone else → not aggro");
        assert!(ids.contains(&103), "pet targeting self also counts (rare but possible)");
    }

    #[test]
    fn selector_skips_pcs_and_npcs() {
        let mut s = SessionState::default();
        s.char_id = Some(42);
        s.entities.push(ent(101, EntityKind::Pc, 42));
        s.entities.push(ent(102, EntityKind::Npc, 42));
        s.entities.push(ent(103, EntityKind::Mob, 42));
        let ids = aggroing_ids(&s);
        assert!(!ids.contains(&101), "PCs filtered (PvP not aggro semantics)");
        assert!(!ids.contains(&102), "NPCs filtered (escort flag not aggro)");
        assert!(ids.contains(&103), "real mob still aggroing");
    }
}
