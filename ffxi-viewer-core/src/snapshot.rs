//! Snapshot ingest: pulls fresh `SceneSnapshot`s and `SceneDelta`s from the
//! `SceneSource` and folds them into a Bevy resource.
//!
//! Why a resource and not direct trait access? Bevy systems that read
//! viewer state would otherwise each call `poll_snapshot`, racing each
//! other and confusing the source's "since last poll" semantics. The
//! single `ingest_system` is the only caller of `poll_*` per frame; every
//! other system reads `SceneState` instead.

use bevy::prelude::*;
use ffxi_viewer_wire::{
    ChatChannel, ChatLine, Entity, PartyMember, SceneDelta, SceneSnapshot, ViewerEvent,
};

use crate::source::SceneSource;

/// Cap on retained chat lines mirrored on the renderer side. Matches the
/// producer-side cap (`state::CHAT_HISTORY_CAP`) so a long session doesn't
/// grow unbounded if the relay sends snapshot+delta sequences without ever
/// re-baselining via a fresh full snapshot.
pub const CHAT_HISTORY_CAP: usize = 256;

/// Latest viewer-side scene state. Folded each frame from the source.
/// Systems read this; nothing writes except `ingest_system` and the
/// local-toast helpers below.
#[derive(Resource, Default)]
pub struct SceneState {
    pub snapshot: SceneSnapshot,
    /// Set on every frame where the snapshot changed (full or delta).
    /// Sync-driven systems (entity sync, HUD) check this and skip work
    /// when nothing changed.
    pub dirty: bool,
    /// UI-local chat toasts that survive snapshot replacement. The session
    /// owns `snapshot.chat` and overwrites it on every poll, so a UI
    /// notification (`unknown command`, `command dropped`, `no shop is
    /// open`) pushed straight into the snapshot would flash for one frame
    /// and vanish. Toasts live here, get appended to the panel each frame,
    /// and survive until either evicted by the cap or the user explicitly
    /// clears them.
    pub local_toasts: Vec<ChatLine>,
    /// Monotonically-increasing counter stamped on every chat line as it
    /// enters viewer state. Drives the merge in [`rendered_chat`] so a
    /// debug toast pushed between two server-pushed battle lines lands
    /// between them in render order, not at the tail of the toast bucket.
    /// Field rather than `AtomicU64` because every mutation is from
    /// `ingest_system` / `push_local_toast`, which run on the Bevy main
    /// thread and hold `&mut SceneState`.
    pub next_chat_seq: u64,
}

/// Cap on retained local toasts. Matches `CHAT_HISTORY_CAP` (256) so a
/// single multi-line client output is retained in full and stays
/// scrollable in the chat panel — the chat scroll already walks the
/// whole buffer (`buffer_len - 1` rows), so the only thing that can hide
/// lines is eviction from this buffer. `/help` is the largest such
/// output (~80 lines and growing with the command set); at the old cap
/// of 32 its top categories were evicted before the operator could read
/// them. A test in `ffxi-client`'s slash-command suite keeps the
/// `render_help` reference under this bound.
pub const LOCAL_TOAST_CAP: usize = 256;

/// Build a system-channel chat line. Routes to `ChatChannel::System` so
/// the message shows in the Battle pane (Chat 2) regardless of `/devhud`
/// state — used both for slash-command responses and for engine event
/// surfacing that a normal player would want to see (zone change, BGM
/// track change, sunrise, low-HP, aggro).
///
/// Counterpart to [`debug_chat_line`] for noisier per-event diagnostics
/// that should stay hidden by default.
pub fn system_chat_line(text: String) -> ChatLine {
    ChatLine {
        channel: ChatChannel::System,
        sender: "client".into(),
        text,
        server_ts: 0,
        local_seq: 0,
    }
}

/// Build a debug-channel chat line for client-internal chatter that
/// should stay hidden by default and only surface when `/devhud` is
/// enabled. Used for high-frequency engine diagnostics (per-SFX fires,
/// BGM loop boundaries, skybox keyframe crossings, unknown opcodes,
/// individual inventory deltas). Counterpart to [`system_chat_line`].
pub fn debug_chat_line(text: String) -> ChatLine {
    ChatLine {
        channel: ChatChannel::Debug,
        sender: "client".into(),
        text,
        server_ts: 0,
        local_seq: 0,
    }
}

impl SceneState {
    /// Push a UI-local chat line that survives snapshot replacement.
    /// Trims to `LOCAL_TOAST_CAP` and marks the state dirty so the chat
    /// panel re-renders this frame.
    pub fn push_local_toast(&mut self, mut line: ChatLine) {
        line.local_seq = self.next_chat_seq;
        self.next_chat_seq += 1;
        self.local_toasts.push(line);
        if self.local_toasts.len() > LOCAL_TOAST_CAP {
            let drop_n = self.local_toasts.len() - LOCAL_TOAST_CAP;
            self.local_toasts.drain(0..drop_n);
        }
        self.dirty = true;
    }

    /// Stamp `local_seq` on each newly-arrived server chat line. Called by
    /// `ingest_system` after a poll/delta merges into `snapshot.chat`.
    /// Walks the tail of `chat` and assigns seqs to entries that still
    /// carry the default 0 — server-pushed lines arrive with seq=0 and
    /// get a fresh number here in arrival order.
    fn stamp_new_server_chat(&mut self, prev_len: usize) {
        let n = self.snapshot.chat.len();
        for i in prev_len..n {
            self.snapshot.chat[i].local_seq = self.next_chat_seq;
            self.next_chat_seq += 1;
        }
    }
}

/// Ring buffer of recent `ViewerEvent`s. HUD systems drain it for
/// notifications (Tell toasts, aggro flashes); `ingest_system` appends.
#[derive(Resource, Default)]
pub struct EventLog {
    pub recent: std::collections::VecDeque<ViewerEvent>,
}

const EVENT_LOG_CAP: usize = 64;

/// Write-only channel for client-side toast messages. Producers use
/// `EventWriter<ToastEvent>` so they can keep `Res<SceneState>` instead
/// of needing `ResMut<SceneState>` just to call `push_local_toast`.
/// Drained each frame by [`drain_toast_events`] (the single mutator).
///
/// Why this exists: before this event, ~17 systems took
/// `ResMut<SceneState>` solely to emit a toast, which forced Bevy's
/// parallel executor to serialize them all on a single core. Moving
/// toast emission to a write-only event lets those systems run in
/// parallel against each other and against the heavy
/// `tick_skinned_actors` / `track_entity_motion_system` work.
#[derive(Message, Debug, Clone)]
pub struct ToastEvent {
    pub line: ChatLine,
}

impl ToastEvent {
    /// System-channel toast (default chat surface).
    pub fn system(text: String) -> Self {
        Self {
            line: system_chat_line(text),
        }
    }
    /// Debug-channel toast (only visible with `/devhud`).
    pub fn debug(text: String) -> Self {
        Self {
            line: debug_chat_line(text),
        }
    }
}

/// Single-writer drain: reads `ToastEvent`s and folds them into
/// `SceneState.local_toasts`. All other systems become read-only or
/// write-only against `SceneState`, unlocking parallel scheduling.
///
/// Runs in `PostUpdate` so toasts emitted anywhere in `Update` land
/// in `local_toasts` before the chat panel renders next frame's
/// `rendered_chat`.
pub fn drain_toast_events(mut state: ResMut<SceneState>, mut events: MessageReader<ToastEvent>) {
    for ev in events.read() {
        state.push_local_toast(ev.line.clone());
    }
}

/// Pull fresh state from the `SceneSource` into the Bevy resource. Runs in
/// `PreUpdate` so downstream systems see a coherent view for the rest of
/// the frame.
pub fn ingest_system<S: SceneSource + Resource>(
    mut source: ResMut<S>,
    mut state: ResMut<SceneState>,
    mut events: ResMut<EventLog>,
) {
    state.dirty = false;

    if let Some(snap) = source.poll_snapshot() {
        state.snapshot = *snap;
        // Re-stamp chat seqs from 0 so server lines render in their
        // canonical arrival order, then re-stamp local_toasts at the
        // tail (seqs chat_n..chat_n+toast_n) so the seq-merge in
        // `rendered_chat` pins them after the most-recent server line.
        //
        // Why not clear local_toasts: in the native bridge,
        // `poll_snapshot` returns Some on every `state_rx.has_changed`
        // tick — i.e. every frame the session mutates anything
        // (position, HP, entities). Clearing on each poll wiped
        // slash-command echos, DAT-load notices, and debug messages
        // after a single frame. We treat toasts as part of the chat
        // log (per the comment at `text_input.rs::push_system_chat_line`)
        // and let `LOCAL_TOAST_CAP` bound retention.
        let chat_n = state.snapshot.chat.len();
        for i in 0..chat_n {
            state.snapshot.chat[i].local_seq = i as u64;
        }
        let toast_n = state.local_toasts.len();
        for i in 0..toast_n {
            state.local_toasts[i].local_seq = (chat_n + i) as u64;
        }
        state.next_chat_seq = (chat_n + toast_n) as u64;
        state.dirty = true;
    }

    for delta in source.drain_deltas() {
        let prev_len = state.snapshot.chat.len();
        apply_delta(&mut state.snapshot, &delta);
        state.stamp_new_server_chat(prev_len);
        state.dirty = true;
    }

    for ev in source.drain_events() {
        if events.recent.len() >= EVENT_LOG_CAP {
            events.recent.pop_front();
        }
        events.recent.push_back(ev);
    }
}

/// Combined chat for rendering: server-pushed `snapshot.chat` followed by
/// UI-local `local_toasts`. The chat panel calls this each frame so the
/// rendering order matches the user's mental model — server messages
/// arrive in the past, the toasts they triggered show below them.
pub fn rendered_chat(state: &SceneState) -> Vec<&ChatLine> {
    // Merge the two stamped streams by `local_seq` so a debug toast
    // pushed at seq=42 lands between server lines stamped 41 and 43,
    // not at the tail of the toast bucket. Both inputs are already
    // sorted ascending by seq (ingest stamps in insertion order), so
    // a linear two-way merge is sufficient and avoids an alloc-sort.
    let s = &state.snapshot.chat;
    let t = &state.local_toasts;
    let mut out: Vec<&ChatLine> = Vec::with_capacity(s.len() + t.len());
    let (mut i, mut j) = (0usize, 0usize);
    while i < s.len() && j < t.len() {
        if s[i].local_seq <= t[j].local_seq {
            out.push(&s[i]);
            i += 1;
        } else {
            out.push(&t[j]);
            j += 1;
        }
    }
    out.extend(s[i..].iter());
    out.extend(t[j..].iter());
    out
}

/// Pure fold: merge a delta into a snapshot. Mirrors the apply rules from
/// `state::SessionState::apply_event` for the same wire signals — in
/// particular the party-member name preservation across attr-only updates
/// (`0x0DF GROUP_ATTR` payloads carry HP/MP/TP but not name/leader flags).
pub fn apply_delta(snap: &mut SceneSnapshot, delta: &SceneDelta) {
    if let Some(stage) = delta.stage {
        snap.stage = stage;
    }
    if let Some(zone) = delta.zone_id {
        snap.zone_id = Some(zone);
    }
    if let Some(pos) = delta.self_pos {
        snap.self_pos = pos;
    }

    upsert_entities(&mut snap.entities, &delta.entities_upserted);
    for &id in &delta.entities_removed {
        snap.entities.retain(|e| e.id != id);
    }

    upsert_party(&mut snap.party, &delta.party_upserted);

    for line in &delta.chat_appended {
        snap.chat.push(line.clone());
    }
    if snap.chat.len() > CHAT_HISTORY_CAP {
        let drop_n = snap.chat.len() - CHAT_HISTORY_CAP;
        snap.chat.drain(0..drop_n);
    }

    if let Some(d) = &delta.diagnostics {
        snap.diagnostics = d.clone();
    }
}

fn upsert_entities(list: &mut Vec<Entity>, ups: &[Entity]) {
    for e in ups {
        if let Some(existing) = list.iter_mut().find(|x| x.id == e.id) {
            *existing = e.clone();
        } else {
            list.push(e.clone());
        }
    }
}

/// Party upsert preserves `name` and `is_party_leader` across attr-only
/// refreshes (where `name == None` indicates a `0x0DF GROUP_ATTR` payload).
/// `in_mog_house` is *not* preserved — both 0x0DD and 0x0DF carry it, and
/// the moghouse-entry rezone delivers a fresh `0x0DF GROUP_ATTR` whose
/// flag value is the new truth.
fn upsert_party(list: &mut Vec<PartyMember>, ups: &[PartyMember]) {
    for m in ups {
        if let Some(existing) = list.iter_mut().find(|x| x.id == m.id) {
            let preserved_name = if m.name.is_some() {
                m.name.clone()
            } else {
                existing.name.clone()
            };
            let preserved_leader = if m.name.is_some() {
                m.is_party_leader
            } else {
                existing.is_party_leader
            };
            let preserved_alliance = if m.name.is_some() {
                m.is_alliance_leader
            } else {
                existing.is_alliance_leader
            };
            *existing = PartyMember {
                name: preserved_name,
                is_party_leader: preserved_leader,
                is_alliance_leader: preserved_alliance,
                ..m.clone()
            };
        } else {
            list.push(m.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_wire::{ChatChannel, ChatLine, EntityKind, Position, Stage, Vec3};

    fn ent(id: u32, x: f32) -> Entity {
        Entity {
            id,
            act_index: 1,
            kind: EntityKind::Pc,
            name: Some(format!("e{id}")),
            pos: Vec3 { x, y: 0.0, z: 0.0 },
            heading: 0,
            hp_pct: Some(100),
            bt_target_id: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
        }
    }

    #[test]
    fn delta_upserts_and_removes_entities() {
        let mut snap = SceneSnapshot::default();
        snap.entities.push(ent(1, 0.0));
        snap.entities.push(ent(2, 5.0));

        let delta = SceneDelta {
            entities_upserted: vec![ent(1, 99.0), ent(3, 7.0)],
            entities_removed: vec![2],
            ..Default::default()
        };
        apply_delta(&mut snap, &delta);

        assert_eq!(snap.entities.len(), 2);
        let e1 = snap.entities.iter().find(|e| e.id == 1).unwrap();
        assert_eq!(e1.pos.x, 99.0, "id=1 must be updated, not duplicated");
        assert!(
            snap.entities.iter().any(|e| e.id == 3),
            "id=3 must be inserted"
        );
        assert!(
            !snap.entities.iter().any(|e| e.id == 2),
            "id=2 must be removed"
        );
    }

    #[test]
    fn delta_replaces_self_pos_and_stage() {
        let mut snap = SceneSnapshot::default();
        let delta = SceneDelta {
            stage: Some(Stage::InZone),
            self_pos: Some(Position {
                pos: Vec3 {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                },
                heading: 64,
                speed: 25,
                speed_base: 25,
            }),
            ..Default::default()
        };
        apply_delta(&mut snap, &delta);
        assert_eq!(snap.stage, Stage::InZone);
        assert_eq!(snap.self_pos.heading, 64);
        assert_eq!(snap.self_pos.pos.y, 2.0);
    }

    #[test]
    fn party_upsert_preserves_name_across_attr_only_update() {
        let mut snap = SceneSnapshot::default();
        let from_list = PartyMember {
            id: 42,
            act_index: 7,
            name: Some("Vanari".into()),
            hp: 2000,
            mp: 100,
            tp: 0,
            hp_pct: 100,
            mp_pct: 100,
            zone_no: 230,
            main_job: 1,
            main_job_lv: 75,
            sub_job: 6,
            sub_job_lv: 37,
            is_party_leader: true,
            is_alliance_leader: false,
            in_mog_house: false,
        };
        apply_delta(
            &mut snap,
            &SceneDelta {
                party_upserted: vec![from_list],
                ..Default::default()
            },
        );
        assert_eq!(snap.party.len(), 1);
        assert!(snap.party[0].is_party_leader);

        // Attr-only update: name=None, leader=false. Must NOT clobber.
        let from_attr = PartyMember {
            id: 42,
            act_index: 7,
            name: None,
            hp: 1500,
            mp: 100,
            tp: 1234,
            hp_pct: 75,
            mp_pct: 100,
            zone_no: 230,
            main_job: 1,
            main_job_lv: 75,
            sub_job: 6,
            sub_job_lv: 37,
            is_party_leader: false,
            is_alliance_leader: false,
            in_mog_house: false,
        };
        apply_delta(
            &mut snap,
            &SceneDelta {
                party_upserted: vec![from_attr],
                ..Default::default()
            },
        );
        assert_eq!(snap.party.len(), 1, "upsert by id");
        assert_eq!(snap.party[0].name.as_deref(), Some("Vanari"));
        assert!(snap.party[0].is_party_leader);
        assert_eq!(snap.party[0].hp, 1500, "HP overwritten");
    }

    #[test]
    fn rendered_chat_concatenates_server_then_toasts() {
        // Server-pushed lines are oldest-first; local toasts append after.
        // The chat panel renders the full chain; `rendered_chat` is the
        // single source of truth for that ordering. Regression guard
        // against the dirty-flag race that hid local toasts from view.
        let mut state = SceneState::default();
        state.snapshot.chat.push(ChatLine {
            channel: ChatChannel::Say,
            sender: "Server".into(),
            text: "echo".into(),
            server_ts: 0,
            local_seq: 0,
        });
        state.push_local_toast(ChatLine {
            channel: ChatChannel::System,
            sender: "client".into(),
            text: "/blarg".into(),
            server_ts: 0,
            local_seq: 0,
        });
        let lines = rendered_chat(&state);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].sender, "Server");
        assert_eq!(lines[1].sender, "client");
    }

    #[test]
    fn rendered_chat_interleaves_by_arrival_seq() {
        // Stamp two server lines and one toast in arrival order
        //   server seq=0, toast seq=1, server seq=2
        // The merge must preserve that order even though the toast
        // physically sits in `local_toasts` and the server lines in
        // `snapshot.chat`.
        let mut state = SceneState::default();
        state.snapshot.chat.push(ChatLine {
            channel: ChatChannel::Battle,
            sender: "mob".into(),
            text: "first".into(),
            server_ts: 0,
            local_seq: 0,
        });
        state.next_chat_seq = 1;
        state.push_local_toast(ChatLine {
            channel: ChatChannel::System,
            sender: "client".into(),
            text: "middle".into(),
            server_ts: 0,
            local_seq: 0,
        });
        // push_local_toast bumped next_chat_seq to 2.
        state.snapshot.chat.push(ChatLine {
            channel: ChatChannel::Battle,
            sender: "mob".into(),
            text: "last".into(),
            server_ts: 0,
            local_seq: state.next_chat_seq,
        });
        let lines = rendered_chat(&state);
        assert_eq!(
            lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            vec!["first", "middle", "last"]
        );
    }

    #[test]
    fn local_toast_cap_drops_oldest() {
        let mut state = SceneState::default();
        for i in 0..(LOCAL_TOAST_CAP + 5) {
            state.push_local_toast(ChatLine {
                channel: ChatChannel::System,
                sender: "client".into(),
                text: format!("toast {i}"),
                server_ts: 0,
                local_seq: 0,
            });
        }
        assert_eq!(state.local_toasts.len(), LOCAL_TOAST_CAP);
        // First retained toast should be the (LOCAL_TOAST_CAP+5 - LOCAL_TOAST_CAP)
        // = 5th one ("toast 5") — older ones evicted.
        assert_eq!(state.local_toasts[0].text, "toast 5");
        assert!(state.dirty, "push must mark dirty for the panel");
    }

    #[test]
    fn chat_appends_and_caps() {
        let mut snap = SceneSnapshot::default();
        let line = ChatLine {
            channel: ChatChannel::Say,
            sender: "x".into(),
            text: "hi".into(),
            server_ts: 0,
            local_seq: 0,
        };
        let delta = SceneDelta {
            chat_appended: vec![line; CHAT_HISTORY_CAP + 5],
            ..Default::default()
        };
        apply_delta(&mut snap, &delta);
        assert_eq!(snap.chat.len(), CHAT_HISTORY_CAP);
    }

    /// Regression: local toasts must survive a snapshot replacement and
    /// land *after* the snapshot's chat in the merged stream. The native
    /// bridge produces a fresh full snapshot every frame the session
    /// mutates anything, so clearing toasts on each poll wiped slash-
    /// command echos / DAT-load notices after a single frame ("<1s flash"
    /// bug). The contract is now: toasts persist, get re-stamped at the
    /// tail of the new chat, and remain visible until `LOCAL_TOAST_CAP`
    /// evicts them.
    #[test]
    fn toasts_persist_through_snapshot_replacement() {
        #[derive(Resource, Default)]
        struct TestSource {
            next_snapshot: Option<Box<SceneSnapshot>>,
        }
        impl SceneSource for TestSource {
            fn poll_snapshot(&mut self) -> Option<Box<SceneSnapshot>> {
                self.next_snapshot.take()
            }
            fn drain_deltas(&mut self) -> Vec<SceneDelta> {
                vec![]
            }
            fn drain_events(&mut self) -> Vec<ViewerEvent> {
                vec![]
            }
        }
        let mut app = App::new();
        app.init_resource::<TestSource>();
        app.init_resource::<SceneState>();
        app.init_resource::<EventLog>();
        app.add_systems(Update, ingest_system::<TestSource>);

        // Push a toast first ("/sound on" echo).
        app.world_mut()
            .resource_mut::<SceneState>()
            .push_local_toast(ChatLine {
                channel: ChatChannel::System,
                sender: "client".into(),
                text: "/sound on".into(),
                server_ts: 0,
                local_seq: 0,
            });

        // Then a fresh snapshot lands carrying two server chat lines —
        // simulates the native bridge handing off updated session state
        // (positions changed, etc.) on the very next frame.
        let mut s = SceneSnapshot::default();
        for text in ["server-a", "server-b"] {
            s.chat.push(ChatLine {
                channel: ChatChannel::Battle,
                sender: "mob".into(),
                text: text.into(),
                server_ts: 0,
                local_seq: 0,
            });
        }
        app.world_mut().resource_mut::<TestSource>().next_snapshot = Some(Box::new(s));
        app.update();

        let state = app.world().resource::<SceneState>();
        assert_eq!(
            state.local_toasts.len(),
            1,
            "toast must survive snapshot replacement"
        );
        let lines = rendered_chat(state);
        assert_eq!(
            lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            vec!["server-a", "server-b", "/sound on"],
            "toast re-stamped at the tail, after the new chat lines"
        );
    }

    /// Confirm `ingest_system::<S>` compiles when S is a Resource +
    /// SceneSource — the generic-bound shape that the plugin uses.
    #[test]
    fn ingest_system_compiles_with_test_source() {
        #[derive(Resource, Default)]
        struct TestSource {
            next_snapshot: Option<Box<SceneSnapshot>>,
        }
        impl SceneSource for TestSource {
            fn poll_snapshot(&mut self) -> Option<Box<SceneSnapshot>> {
                self.next_snapshot.take()
            }
            fn drain_deltas(&mut self) -> Vec<SceneDelta> {
                vec![]
            }
            fn drain_events(&mut self) -> Vec<ViewerEvent> {
                vec![]
            }
        }
        let mut app = App::new();
        app.init_resource::<TestSource>();
        app.init_resource::<SceneState>();
        app.init_resource::<EventLog>();
        app.add_systems(Update, ingest_system::<TestSource>);

        // Hand the source a snapshot, run one update, verify it lands.
        let s = SceneSnapshot {
            stage: Stage::InZone,
            ..Default::default()
        };
        app.world_mut().resource_mut::<TestSource>().next_snapshot = Some(Box::new(s));
        app.update();
        assert_eq!(
            app.world().resource::<SceneState>().snapshot.stage,
            Stage::InZone
        );
        assert!(app.world().resource::<SceneState>().dirty);

        // Next frame with no new snapshot — dirty must clear.
        app.update();
        assert!(!app.world().resource::<SceneState>().dirty);
    }
}
