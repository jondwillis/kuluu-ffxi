//! Single-authority entity table (piece 2 of the entity-table refactor).
//!
//! The ingest system mirrors every snapshot/delta into this resource so later
//! pieces can read one fact store instead of re-scanning
//! `SceneSnapshot.entities`: piece 4's sync pass iterates
//! [`EntityTable::changed_ids`] only, and piece 7's nameplate pass reads live
//! records per frame. Additive for now: nothing outside ingest reads the table
//! yet — `SceneSnapshot.entities` keeps being populated until piece 8.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use kuluu_snapshot::{Entity, SceneDelta, SceneSnapshot};

/// One wire entity plus change bookkeeping. `dirty` is set on every upsert and
/// cleared when [`EntityTable::changed_ids`] reports the id.
#[derive(Debug, Clone)]
pub struct EntityRecord {
    pub entity: Entity,
    pub dirty: bool,
}

impl EntityRecord {
    /// Pass-throughs so consumers read facts through one type: piece 4 stamps
    /// Visibility from `is_invisible`/`name_hidden`, piece 5's self-dead check
    /// uses `is_dead`.
    pub fn is_invisible(&self) -> bool {
        self.entity.is_invisible()
    }

    /// `Flags1.InvisFlag` (bit 29), PCs only — see [`Entity::invis_flag`] on the
    /// record's entity.
    pub fn invis_flag(&self) -> bool {
        self.entity.invis_flag()
    }

    pub fn name_hidden(&self) -> bool {
        self.entity.name_hidden()
    }

    pub fn is_dead(&self) -> bool {
        self.entity.is_dead()
    }
}

/// Every live wire entity keyed by id. `live` keeps insertion order for stable
/// full-table iteration; `removed_since_drain` carries despawns to the next
/// [`Self::changed_ids`] call so removals reach consumers exactly once.
#[derive(Debug, Default, Resource)]
pub struct EntityTable {
    records: HashMap<u32, EntityRecord>,
    live: Vec<u32>,
    self_id: Option<u32>,
    removed_since_drain: HashSet<u32>,
}

impl EntityTable {
    /// Insert or replace one entity. A re-upsert voids a same-window removal.
    pub fn upsert(&mut self, entity: &Entity) {
        let id = entity.id;
        match self.records.get_mut(&id) {
            Some(rec) => {
                rec.entity = entity.clone();
                rec.dirty = true;
            }
            None => {
                self.live.push(id);
                self.records.insert(
                    id,
                    EntityRecord {
                        entity: entity.clone(),
                        dirty: true,
                    },
                );
            }
        }
        self.removed_since_drain.remove(&id);
    }

    /// Drop one entity. Returns whether it was live; a no-op removal reports
    /// nothing on the next drain.
    pub fn remove(&mut self, id: u32) -> bool {
        if self.records.remove(&id).is_some() {
            self.live.retain(|x| *x != id);
            self.removed_since_drain.insert(id);
            true
        } else {
            false
        }
    }

    pub fn get(&self, id: u32) -> Option<&EntityRecord> {
        self.records.get(&id)
    }

    /// All live records in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &EntityRecord> + '_ {
        self.live.iter().filter_map(|id| self.records.get(id))
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Ids upserted or removed since the last call; clears that state.
    /// Upserts report in table insertion order. A same-window
    /// remove-then-upsert reports once, as an upsert (the record exists); an
    /// upsert-then-remove reports once, as a removal (`get` is None for it).
    /// Removal order within one drain is unspecified.
    pub fn changed_ids(&mut self) -> Vec<u32> {
        let mut out: Vec<u32> = Vec::new();
        for &id in &self.live {
            if let Some(rec) = self.records.get_mut(&id) {
                if rec.dirty {
                    rec.dirty = false;
                    out.push(id);
                }
            }
        }
        out.extend(self.removed_since_drain.drain());
        out
    }

    /// Full replace from a snapshot (resync / zone change). Every record is
    /// dirty; ids that were live but are absent from the snapshot become
    /// pending removals. Ids still present void any pending removal of theirs.
    pub fn apply_snapshot(&mut self, snap: &SceneSnapshot) {
        let mut records = HashMap::with_capacity(snap.entities.len());
        let mut live = Vec::with_capacity(snap.entities.len());
        for e in &snap.entities {
            records.insert(
                e.id,
                EntityRecord {
                    entity: e.clone(),
                    dirty: true,
                },
            );
            live.push(e.id);
        }
        self.removed_since_drain
            .retain(|id| !records.contains_key(id));
        for id in self.live.iter().copied() {
            if !records.contains_key(&id) {
                self.removed_since_drain.insert(id);
            }
        }
        self.records = records;
        self.live = live;
    }

    /// Apply one delta: upserts first, then removals — the same order as
    /// `snapshot::apply_delta`, so a remove-then-upsert of one id in a single
    /// delta nets to an upsert.
    pub fn apply_delta(&mut self, delta: &SceneDelta) {
        for e in &delta.entities_upserted {
            self.upsert(e);
        }
        for &id in &delta.entities_removed {
            self.remove(id);
        }
    }

    /// Piece 3 stamps this at zone entry; until then `is_self` is false.
    pub fn set_self_id(&mut self, id: Option<u32>) {
        self.self_id = id;
    }

    pub fn self_id(&self) -> Option<u32> {
        self.self_id
    }

    /// Piece 3 replaces the scattered `self_char_id == wire.id` checks with this.
    pub fn is_self(&self, id: u32) -> bool {
        self.self_id.is_some_and(|s| s == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_snapshot::{EntityKind, Vec3};

    fn ent(id: u32, x: f32) -> Entity {
        Entity {
            id,
            act_index: 1,
            kind: EntityKind::Mob,
            name: Some(format!("e{id}")),
            pos: Vec3 { x, y: 0.0, z: 0.0 },
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
            monstrosity: false,
            name_vis: None,
        }
    }

    fn snap_of(ids: &[u32]) -> SceneSnapshot {
        SceneSnapshot {
            entities: ids.iter().map(|id| ent(*id, 0.0)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn upsert_inserts_then_updates_without_duplicating() {
        let mut t = EntityTable::default();
        t.upsert(&ent(1, 0.0));
        t.upsert(&ent(2, 5.0));
        assert_eq!(t.len(), 2);

        t.upsert(&ent(1, 99.0));
        assert_eq!(t.len(), 2, "same id must update in place");
        let rec = t.get(1).expect("id=1 live");
        assert_eq!(rec.entity.pos.x, 99.0);

        // Insertion order is stable for full-table iteration.
        let ids: Vec<u32> = t.iter().map(|r| r.entity.id).collect();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn changed_ids_reports_upserts_once_and_clears() {
        let mut t = EntityTable::default();
        t.upsert(&ent(1, 0.0));
        t.upsert(&ent(1, 1.0)); // same window: still one report

        assert_eq!(t.changed_ids(), vec![1]);
        assert!(t.changed_ids().is_empty(), "drain clears the dirty flags");

        t.upsert(&ent(2, 0.0));
        assert_eq!(t.changed_ids(), vec![2], "only new changes report");
    }

    #[test]
    fn removal_reports_once_and_get_is_none() {
        let mut t = EntityTable::default();
        t.upsert(&ent(1, 0.0));
        assert_eq!(t.changed_ids(), vec![1]);

        assert!(t.remove(1), "live id removes");
        assert!(!t.remove(1), "second remove is a no-op");
        assert!(t.get(1).is_none());
        assert_eq!(t.len(), 0);

        let changed = t.changed_ids();
        assert_eq!(changed, vec![1], "removal reaches the drain exactly once");
        assert!(t.changed_ids().is_empty());
    }

    #[test]
    fn same_window_upsert_then_remove_nets_to_removal() {
        let mut t = EntityTable::default();
        t.upsert(&ent(7, 0.0));
        t.remove(7);

        let changed = t.changed_ids();
        assert_eq!(changed, vec![7]);
        assert!(t.get(7).is_none(), "record must be gone");
    }

    #[test]
    fn same_window_remove_then_upsert_nets_to_upsert() {
        let mut t = EntityTable::default();
        t.upsert(&ent(7, 0.0));
        assert_eq!(t.changed_ids(), vec![7]);
        t.remove(7);
        t.upsert(&ent(7, 42.0));

        let changed = t.changed_ids();
        assert_eq!(changed, vec![7], "reported once, as an upsert");
        assert_eq!(t.get(7).expect("record back").entity.pos.x, 42.0);
    }

    #[test]
    fn apply_snapshot_replaces_and_reports_gone_ids() {
        let mut t = EntityTable::default();
        t.upsert(&ent(1, 0.0));
        t.upsert(&ent(2, 0.0));
        assert_eq!(t.changed_ids(), vec![1, 2]);

        t.apply_snapshot(&snap_of(&[2, 3]));
        let changed = t.changed_ids();
        // All snapshot records are dirty; id=1 was live but is gone.
        assert_eq!(changed.len(), 3);
        for id in [1u32, 2, 3] {
            assert!(changed.contains(&id), "missing {id} from drain");
        }
        assert!(t.get(1).is_none());
        assert!(t.get(2).is_some());
        assert!(t.get(3).is_some());

        // A second identical snapshot reports nothing new.
        t.apply_snapshot(&snap_of(&[2, 3]));
        let changed = t.changed_ids();
        assert_eq!(changed.len(), 2, "full replace marks every record dirty");
        for id in [2u32, 3] {
            assert!(changed.contains(&id));
        }
    }

    #[test]
    fn apply_snapshot_voids_pending_removal_of_returned_id() {
        let mut t = EntityTable::default();
        t.upsert(&ent(1, 0.0));
        assert_eq!(t.changed_ids(), vec![1]);
        t.remove(1); // pending removal, not yet drained

        // The snapshot brings id=1 back: it must report as an upsert only.
        t.apply_snapshot(&snap_of(&[1]));
        let changed = t.changed_ids();
        assert_eq!(changed, vec![1]);
        assert!(t.get(1).is_some());
    }

    #[test]
    fn apply_delta_mirrors_upserts_and_removals() {
        let mut t = EntityTable::default();
        t.apply_snapshot(&snap_of(&[1, 2]));
        assert_eq!(t.changed_ids().len(), 2);

        t.apply_delta(&SceneDelta {
            entities_upserted: vec![ent(1, 99.0), ent(3, 7.0)],
            entities_removed: vec![2],
            ..Default::default()
        });

        let changed = t.changed_ids();
        assert_eq!(changed.len(), 3);
        for id in [1u32, 2, 3] {
            assert!(changed.contains(&id), "missing {id} from drain");
        }
        assert_eq!(t.get(1).expect("updated").entity.pos.x, 99.0);
        assert!(t.get(2).is_none());
        assert!(t.get(3).is_some());
    }

    #[test]
    fn self_id_gates_is_self() {
        let mut t = EntityTable::default();
        t.upsert(&ent(1, 0.0));
        assert!(!t.is_self(1), "no self stamped yet");

        t.set_self_id(Some(1));
        assert!(t.is_self(1));
        assert!(!t.is_self(2));

        t.set_self_id(None);
        assert!(!t.is_self(1));
    }
}
