use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bevy::prelude::Resource;
use kuluu_render::SceneSource;
use kuluu_snapshot as wire;
use tokio::runtime::Handle as RtHandle;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

use kuluu_session::state::{AgentEvent, SessionState};
use kuluu_session::wire_translate::{event_to_viewer_event, state_to_snapshot};

// The session watch signals per folded packet event — far above frame rate in a
// crowd — so the off-main-thread translator caps itself near the 120 Hz display
// ceiling. On slower displays this allows up to ~2x the old once-per-frame
// translate rate (and its watch read-lock pressure on the session folder);
// accepted because the work left the render thread, and the cap still bounds
// folder contention (audit of kuluu-4mef).
const TRANSLATE_MIN_PERIOD: Duration = Duration::from_millis(8);

struct TranslatedSnapshot {
    snap: Box<wire::SceneSnapshot>,
    rebuild_us: u64,
}

// Mutex<Option> rather than a watch: poll_snapshot takes ownership of the Box,
// so the render thread never clones a snapshot; overwriting the single slot
// keeps only the newest, which makes out-of-order delivery impossible.
type SnapshotMailbox = Arc<Mutex<Option<TranslatedSnapshot>>>;

fn translate_current(state_rx: &mut watch::Receiver<SessionState>) -> TranslatedSnapshot {
    let started = std::time::Instant::now();
    let snap = {
        let guard = state_rx.borrow_and_update();
        state_to_snapshot(&guard)
    };
    TranslatedSnapshot {
        rebuild_us: started.elapsed().as_micros() as u64,
        snap: Box::new(snap),
    }
}

async fn run_translator(mut state_rx: watch::Receiver<SessionState>, mailbox: SnapshotMailbox) {
    loop {
        let started = tokio::time::Instant::now();
        let translated = translate_current(&mut state_rx);
        // Take the overwritten snapshot out before dropping it: its Vec frees
        // must not run inside the lock the render thread polls every frame.
        let prev = mailbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(translated);
        drop(prev);
        tokio::time::sleep_until(started + TRANSLATE_MIN_PERIOD).await;
        if state_rx.changed().await.is_err() {
            break;
        }
    }
}

#[derive(Resource)]
pub struct NativeSource {
    mailbox: SnapshotMailbox,
    translator: JoinHandle<()>,
    event_rx: broadcast::Receiver<AgentEvent>,
    warned_translator_gone: bool,

    pub last_rebuild_us: u64,
    pub last_entity_count: usize,
    pub rebuilds_total: u64,
}

impl NativeSource {
    pub fn new(
        runtime: &RtHandle,
        state_rx: watch::Receiver<SessionState>,
        event_rx: broadcast::Receiver<AgentEvent>,
    ) -> Self {
        let mailbox = SnapshotMailbox::default();
        let translator = runtime.spawn(run_translator(state_rx, Arc::clone(&mailbox)));
        Self {
            mailbox,
            translator,
            event_rx,
            warned_translator_gone: false,
            last_rebuild_us: 0,
            last_entity_count: 0,
            rebuilds_total: 0,
        }
    }
}

impl Drop for NativeSource {
    fn drop(&mut self) {
        self.translator.abort();
    }
}

impl SceneSource for NativeSource {
    fn poll_snapshot(&mut self) -> Option<Box<wire::SceneSnapshot>> {
        let ready = self
            .mailbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(ready) = ready else {
            // A dead translator (panic in state_to_snapshot, or session end)
            // otherwise freezes the viewer on the last scene with no diagnostic.
            if !self.warned_translator_gone && self.translator.is_finished() {
                self.warned_translator_gone = true;
                bevy::log::warn!(
                    "snapshot translator exited; no further scene updates until reconnect"
                );
            }
            return None;
        };
        self.last_rebuild_us = ready.rebuild_us;
        self.last_entity_count = ready.snap.entities.len();
        self.rebuilds_total = self.rebuilds_total.wrapping_add(1);
        Some(ready.snap)
    }

    fn drain_deltas(&mut self) -> Vec<wire::SceneDelta> {
        Vec::new()
    }

    fn drain_events(&mut self) -> Vec<wire::ViewerEvent> {
        let mut out = Vec::new();
        loop {
            match self.event_rx.try_recv() {
                Ok(ev) => {
                    if let Some(translated) = event_to_viewer_event(ev) {
                        out.push(translated);
                    }
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_session::state::{
        ChatChannel, ChatLine, ContainerInfo, Entity, EntityKind, EquippedRef, ItemSlot,
        PartyMember, ReactorGoalSnapshot, ReconnectInfo, Stage, Vec3,
    };

    fn populated_state() -> SessionState {
        let mut s = SessionState {
            stage: Stage::InZone,
            account_id: Some(1),
            char_id: Some(0x1000_0001),
            character: Some("Sylvie".into()),
            zone_id: Some(230),
            current_goal: Some(ReactorGoalSnapshot::Engaged {
                target_id: 0x1000_0102,
                attack_issued: true,
            }),
            last_reconnect: Some(ReconnectInfo {
                downtime_ms: 800,
                at_unix_ms: 1_700_000_002_000,
            }),
            current_weather: Some(4),
            status_icons: vec![13, 33],
            status_icon_expiries: vec![600, 0],
            ability_recasts: vec![(16, 45), (0, 0)],
            spells_known: vec![1, 2, 17],
            job_abilities_known: vec![16],
            weaponskills_known: vec![32],
            key_items: vec![342],
            key_items_seen: vec![342],
            ..Default::default()
        };

        s.entities = vec![
            Entity {
                id: 0x1000_0001,
                act_index: 0x001,
                kind: EntityKind::Pc,
                name: Some("Sylvie".into()),
                pos: Vec3 {
                    x: 12.5,
                    y: -1.0,
                    z: 34.0,
                },
                heading: 96,
                hp_pct: Some(100),
                bt_target_id: 0,
                name_vis: None,
                face_target: 0x102,
                claim_id: 0,
                speed: 40,
                speed_base: 40,
                look: Some(ffxi_proto::decode::LookData::Equipped {
                    face: 2,
                    race: 1,
                    head: 0x1000,
                    body: 0x2001,
                    hands: 0x3002,
                    legs: 0x4003,
                    feet: 0x5004,
                    main: 0x6005,
                    sub: 0,
                    ranged: 0,
                }),
                npc_state: None,
                status: 1,
                char_flags: Default::default(),
                mount_id: None,
            },
            Entity {
                id: 0x1000_0102,
                act_index: 0x102,
                kind: EntityKind::Mob,
                name: Some("Wild Rabbit".into()),
                pos: Vec3 {
                    x: 15.0,
                    y: -1.2,
                    z: 30.0,
                },
                heading: 12,
                hp_pct: Some(72),
                bt_target_id: 0x1000_0001,
                name_vis: None,
                face_target: 0x001,
                claim_id: 0x1000_0001,
                speed: 40,
                speed_base: 40,
                look: Some(ffxi_proto::decode::LookData::Standard { modelid: 0x0119 }),
                npc_state: Some(ffxi_proto::decode::NpcState {
                    animation: 1,
                    animationsub: 0,
                    status: 1,
                }),
                status: 1,
                char_flags: Default::default(),
                mount_id: None,
            },
        ];

        s.party = vec![PartyMember {
            id: 0x1000_0001,
            act_index: 0x001,
            name: Some("Sylvie".into()),
            hp: 512,
            mp: 128,
            tp: 1000,
            hp_pct: 100,
            mp_pct: 90,
            zone_no: 230,
            main_job: 1,
            main_job_lv: 12,
            sub_job: 5,
            sub_job_lv: 6,
            is_party_leader: true,
            is_alliance_leader: false,
            in_mog_house: false,
            party_no: 0,
        }];

        s.chat = (0..8)
            .map(|i| ChatLine {
                spans: Vec::new(),
                channel: if i % 2 == 0 {
                    ChatChannel::Say
                } else {
                    ChatChannel::Battle
                },
                sender: format!("Speaker{i}"),
                text: format!("line {i}"),
                server_ts: 1000 + i,
            })
            .collect();

        let mut inv0 = ContainerInfo {
            capacity: 30,
            slots: Vec::new(),
        };
        inv0.slots.push(ItemSlot {
            index: 3,
            item_no: 16448,
            quantity: 1,
            locked: true,
            price: 0,
            charges_remaining: None,
            next_use_vana_ts: None,
        });
        s.inventory.containers.insert(0, inv0);
        s.equipment[0] = Some(EquippedRef {
            container: 0,
            container_index: 3,
        });

        s
    }

    fn normalized(mut snap: wire::SceneSnapshot) -> serde_json::Value {
        snap.producer_monotonic_ms = 0;
        serde_json::to_value(&snap).expect("SceneSnapshot serializes")
    }

    #[test]
    fn translator_task_matches_synchronous_translate() {
        let state = populated_state();
        let expected = state_to_snapshot(&state);
        assert!(!expected.entities.is_empty() && !expected.chat.is_empty());

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        let (state_tx, state_rx) = watch::channel(state);
        let mailbox = SnapshotMailbox::default();

        let translated = rt.block_on(async {
            let task = tokio::spawn(run_translator(state_rx, Arc::clone(&mailbox)));
            let got = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(t) = mailbox
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take()
                    {
                        break t;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("translator publishes within timeout");
            task.abort();
            got
        });
        drop(state_tx);

        assert_eq!(translated.snap.entities.len(), expected.entities.len());
        assert_eq!(normalized(*translated.snap), normalized(expected));
    }

    #[test]
    fn final_state_is_delivered_after_sender_drops() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        let (state_tx, state_rx) = watch::channel(populated_state());
        let mailbox = SnapshotMailbox::default();

        rt.block_on(async {
            let task = tokio::spawn(run_translator(state_rx, Arc::clone(&mailbox)));
            state_tx.send_modify(|s| s.zone_id = Some(999));
            drop(state_tx);
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .expect("translator exits after sender drop")
                .expect("translator did not panic");
        });

        let last = mailbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .expect("final snapshot published");
        assert_eq!(last.snap.zone_id, Some(999), "last unseen state delivered");
    }

    /// The 0x5D master volume is fanned across the music slots, so the count
    /// here and the one the renderer indexes have to be the same number.
    /// Lives here (not in kuluu-session) because it is the one assertion that
    /// needs both sides of the session/renderer boundary in scope.
    #[test]
    fn the_music_slot_count_matches_the_renderer_mixer() {
        assert_eq!(
            kuluu_session::state::MUSIC_SLOT_COUNT as usize,
            kuluu_render::audio::SLOT_COUNT
        );
    }
}
