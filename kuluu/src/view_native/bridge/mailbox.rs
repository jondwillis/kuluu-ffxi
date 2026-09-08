use super::{wire, PoisonError, ReadyFrame, SnapshotMailbox, TranslatedFrame};
use kuluu_render::snapshot::apply_delta;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn publish(mailbox: &SnapshotMailbox, mut next: ReadyFrame) {
    let mut slot = mailbox.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(previous) = slot.take() {
        next.frame = match (previous.frame, next.frame) {
            (_, snapshot @ TranslatedFrame::Snapshot(_)) => snapshot,
            (TranslatedFrame::Snapshot(mut snapshot), TranslatedFrame::Delta(delta)) => {
                apply_delta(&mut snapshot, &delta);
                TranslatedFrame::Snapshot(snapshot)
            }
            (TranslatedFrame::Delta(previous), TranslatedFrame::Delta(next)) => {
                TranslatedFrame::Delta(merge(previous, next))
            }
        };
    }
    *slot = Some(next);
}

fn merge(mut previous: wire::SceneDelta, mut next: wire::SceneDelta) -> wire::SceneDelta {
    let mut entities = BTreeMap::new();
    let mut removed = BTreeSet::new();
    for delta in [&mut previous, &mut next] {
        for entity in delta.entities_upserted.drain(..) {
            removed.remove(&entity.id);
            entities.insert(entity.id, entity);
        }
        for id in delta.entities_removed.drain(..) {
            entities.remove(&id);
            removed.insert(id);
        }
    }
    wire::SceneDelta {
        stage: next.stage.or(previous.stage),
        zone_id: next.zone_id.or(previous.zone_id),
        self_pos: next.self_pos.or(previous.self_pos),
        entities_upserted: entities.into_values().collect(),
        entities_removed: removed.into_iter().collect(),
        party_upserted: previous
            .party_upserted
            .into_iter()
            .chain(next.party_upserted)
            .collect(),
        chat_appended: previous
            .chat_appended
            .into_iter()
            .chain(next.chat_appended)
            .collect(),
        diagnostics: next.diagnostics.or(previous.diagnostics),
        myroom: next.myroom.or(previous.myroom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view_native::bridge::tests::{mob_entity, normalized, populated_state};
    use kuluu_session::wire_translate::{entity_to_wire, state_to_snapshot};

    fn publish_frame(mailbox: &SnapshotMailbox, frame: TranslatedFrame) {
        publish(
            mailbox,
            ReadyFrame {
                frame,
                rebuild_us: 0,
                entity_count: 0,
            },
        );
    }

    #[test]
    fn stalled_consumer_keeps_snapshot_and_every_entity_change() {
        let mailbox = SnapshotMailbox::default();
        let mut expected = state_to_snapshot(&populated_state());
        publish_frame(
            &mailbox,
            TranslatedFrame::Snapshot(Box::new(expected.clone())),
        );
        for id in [11, 22, 11, 33] {
            let delta = wire::SceneDelta {
                entities_upserted: vec![entity_to_wire(&mob_entity(id))],
                ..Default::default()
            };
            apply_delta(&mut expected, &delta);
            publish_frame(&mailbox, TranslatedFrame::Delta(delta));
        }
        let TranslatedFrame::Snapshot(actual) = mailbox.lock().unwrap().take().unwrap().frame
        else {
            panic!("initial snapshot must survive until consumed");
        };
        assert_eq!(normalized(*actual), normalized(expected));
    }

    #[test]
    fn coalesced_deltas_keep_last_write_and_bound_repeated_updates() {
        let mailbox = SnapshotMailbox::default();
        for _ in 0..100 {
            publish_frame(
                &mailbox,
                TranslatedFrame::Delta(wire::SceneDelta {
                    entities_removed: vec![11],
                    ..Default::default()
                }),
            );
            publish_frame(
                &mailbox,
                TranslatedFrame::Delta(wire::SceneDelta {
                    entities_upserted: vec![
                        entity_to_wire(&mob_entity(11)),
                        entity_to_wire(&mob_entity(22)),
                    ],
                    ..Default::default()
                }),
            );
        }
        publish_frame(
            &mailbox,
            TranslatedFrame::Delta(wire::SceneDelta {
                entities_removed: vec![22],
                ..Default::default()
            }),
        );
        let TranslatedFrame::Delta(actual) = mailbox.lock().unwrap().take().unwrap().frame else {
            panic!("expected delta");
        };
        assert_eq!(
            actual
                .entities_upserted
                .iter()
                .map(|entity| entity.id)
                .collect::<Vec<_>>(),
            [11]
        );
        assert_eq!(actual.entities_removed, [22]);
    }

    #[test]
    fn resync_replaces_unread_old_zone_changes() {
        let mailbox = SnapshotMailbox::default();
        publish_frame(
            &mailbox,
            TranslatedFrame::Delta(wire::SceneDelta {
                entities_removed: vec![11],
                ..Default::default()
            }),
        );
        let expected = state_to_snapshot(&populated_state());
        publish_frame(
            &mailbox,
            TranslatedFrame::Snapshot(Box::new(expected.clone())),
        );
        let TranslatedFrame::Snapshot(actual) = mailbox.lock().unwrap().take().unwrap().frame
        else {
            panic!("expected resync");
        };
        assert_eq!(normalized(*actual), normalized(expected));
    }
}
