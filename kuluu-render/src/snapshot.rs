use bevy::prelude::*;
use kuluu_snapshot::{
    ChatChannel, ChatLine, Entity, PartyMember, SceneDelta, SceneSnapshot, ViewerEvent,
};

use crate::source::SceneSource;

pub const CHAT_HISTORY_CAP: usize = 256;

/// Single render key for zone-keyed DAT resources: inside the Mog House LSB
/// keeps `zone_id` as the surrounding city, so `zone_id` edges miss the swap.
pub fn effective_zone_file_id(snap: &SceneSnapshot) -> Option<u32> {
    ffxi_dat::zone_dat::effective_zone_dat_file_id(snap.zone_id, snap.myroom.map(|m| m.model))
}

pub fn resolve_self(party: &[PartyMember], self_char_id: Option<u32>) -> Option<&PartyMember> {
    if let Some(id) = self_char_id {
        if let Some(m) = party.iter().find(|m| m.id == id) {
            return Some(m);
        }
    }
    party.first()
}

#[derive(Resource, Default)]
pub struct SceneState {
    pub snapshot: SceneSnapshot,

    pub dirty: bool,

    pub local_toasts: Vec<ChatLine>,

    /// Session chat lines seen so far, in absolute history indices. Stamped onto
    /// each local toast as the point in the server stream it follows.
    pub server_chat_seen: u64,
}

pub const LOCAL_TOAST_CAP: usize = 256;

pub fn system_chat_line(text: String) -> ChatLine {
    ChatLine {
        spans: Vec::new(),
        channel: ChatChannel::System,
        sender: "client".into(),
        text,
        server_ts: 0,
        local_seq: 0,
    }
}

pub fn debug_chat_line(text: String) -> ChatLine {
    ChatLine {
        spans: Vec::new(),
        channel: ChatChannel::Debug,
        sender: "client".into(),
        text,
        server_ts: 0,
        local_seq: 0,
    }
}

impl SceneState {
    pub fn push_local_toast(&mut self, mut line: ChatLine) {
        line.local_seq = self.server_chat_seen;
        self.local_toasts.push(line);
        if self.local_toasts.len() > LOCAL_TOAST_CAP {
            let drop_n = self.local_toasts.len() - LOCAL_TOAST_CAP;
            self.local_toasts.drain(0..drop_n);
        }
        self.dirty = true;
    }

    fn observe_server_chat(&mut self) {
        self.server_chat_seen = self.snapshot.chat_base_seq + self.snapshot.chat.len() as u64;
    }
}

#[derive(Resource, Default)]
pub struct EventLog {
    pub recent: std::collections::VecDeque<ViewerEvent>,

    pub pushed_total: u64,
}

const EVENT_LOG_CAP: usize = 64;

impl EventLog {
    // `pushed_total` is the global index every consumer's drain cursor is expressed in, so it
    // must advance for events the ring has already dropped. Pushing straight into `recent`
    // makes a reader skip the event entirely.
    pub fn push(&mut self, ev: ViewerEvent) {
        if self.recent.len() >= EVENT_LOG_CAP {
            self.recent.pop_front();
        }
        self.recent.push_back(ev);
        self.pushed_total += 1;
    }
}

#[derive(Message, Debug, Clone)]
pub struct ToastEvent {
    pub line: ChatLine,
}

impl ToastEvent {
    pub fn system(text: String) -> Self {
        Self {
            line: system_chat_line(text),
        }
    }

    pub fn debug(text: String) -> Self {
        Self {
            line: debug_chat_line(text),
        }
    }
}

pub fn chat_line_visible(channel: ChatChannel, dev_hud: bool) -> bool {
    dev_hud || channel != ChatChannel::Debug
}

pub fn drain_toast_events(mut state: ResMut<SceneState>, mut events: MessageReader<ToastEvent>) {
    for ev in events.read() {
        state.push_local_toast(ev.line.clone());
    }
}

// 0.19's resources-as-components: `ResMut` needs the resource's Component
// impl to be mutable, which generic bounds must now spell out.
pub fn ingest_system<
    S: SceneSource + Resource + Component<Mutability = bevy::ecs::component::Mutable>,
>(
    mut source: ResMut<S>,
    mut state: ResMut<SceneState>,
    mut events: ResMut<EventLog>,
) {
    // Clearing through the tracked ResMut would tick SceneState every frame,
    // poisoning is_changed()/resource_changed gating for every consumer.
    state.bypass_change_detection().dirty = false;

    if let Some(snap) = source.poll_snapshot() {
        state.snapshot = *snap;
        state.observe_server_chat();
        state.dirty = true;
    }

    for delta in source.drain_deltas() {
        apply_delta(&mut state.snapshot, &delta);
        state.observe_server_chat();
        state.dirty = true;
    }

    for ev in source.drain_events() {
        events.push(ev);
    }
}

/// Interleave the two chat producers — session lines carried in the snapshot and
/// renderer-local toasts — back into arrival order. They share no clock
/// (`server_ts` is 0 on every client-authored line), so the merge runs on
/// absolute history indices: session line `i` sits at `chat_base_seq + i`, and a
/// toast carries the count of session lines that preceded it. Strict `<` puts a
/// toast after every line it followed and before the next one to arrive.
pub fn rendered_chat(state: &SceneState) -> Vec<&ChatLine> {
    let s = &state.snapshot.chat;
    let base = state.snapshot.chat_base_seq;
    let t = &state.local_toasts;
    let mut out: Vec<&ChatLine> = Vec::with_capacity(s.len() + t.len());
    let (mut i, mut j) = (0usize, 0usize);
    while i < s.len() && j < t.len() {
        if base + (i as u64) < t[j].local_seq {
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
        snap.chat_base_seq += drop_n as u64;
    }

    if let Some(d) = &delta.diagnostics {
        snap.diagnostics = d.clone();
    }
    if let Some(m) = delta.myroom {
        snap.myroom = Some(m);
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
            let preserved_party_no = if m.name.is_some() {
                m.party_no
            } else {
                existing.party_no
            };
            *existing = PartyMember {
                name: preserved_name,
                is_party_leader: preserved_leader,
                is_alliance_leader: preserved_alliance,
                party_no: preserved_party_no,
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
    use kuluu_snapshot::{ChatChannel, ChatLine, EntityKind, Position, Stage, Vec3};

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
            face_target: 0,
            name_vis: None,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            mount: None,
            status: 0,
            char_flags: Default::default(),
        }
    }

    #[test]
    fn debug_lines_hidden_unless_dev_hud() {
        assert!(!chat_line_visible(ChatChannel::Debug, false));
        assert!(chat_line_visible(ChatChannel::Debug, true));
    }

    #[test]
    fn non_debug_lines_visible_regardless_of_dev_hud() {
        for channel in [ChatChannel::System, ChatChannel::Say, ChatChannel::Battle] {
            assert!(chat_line_visible(channel, false));
            assert!(chat_line_visible(channel, true));
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
            party_no: 0,
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
            party_no: 0,
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
    fn delta_sets_myroom_and_none_is_no_change() {
        use kuluu_snapshot::MyRoom;
        let mut snap = SceneSnapshot::default();
        apply_delta(
            &mut snap,
            &SceneDelta {
                myroom: Some(MyRoom {
                    model: 256,
                    sub_map: 0,
                }),
                ..Default::default()
            },
        );
        assert_eq!(
            snap.myroom,
            Some(MyRoom {
                model: 256,
                sub_map: 0
            })
        );

        apply_delta(&mut snap, &SceneDelta::default());
        assert_eq!(
            snap.myroom,
            Some(MyRoom {
                model: 256,
                sub_map: 0
            }),
            "None delta must not clear myroom"
        );
    }

    #[test]
    fn effective_zone_file_id_prefers_myroom_over_zone() {
        use kuluu_snapshot::MyRoom;
        let mut snap = SceneSnapshot {
            zone_id: Some(230),
            ..Default::default()
        };
        assert_eq!(effective_zone_file_id(&snap), Some(330));

        snap.myroom = Some(MyRoom {
            model: 257,
            sub_map: 0,
        });
        assert_eq!(
            effective_zone_file_id(&snap),
            Some(357),
            "myroom model must resolve via the MH table, not zone_id_to_mzb_file_id"
        );

        snap.myroom = None;
        assert_eq!(
            effective_zone_file_id(&snap),
            Some(330),
            "exit restores the town key"
        );
    }

    #[test]
    fn a_toast_renders_after_the_server_lines_it_followed() {
        let mut state = SceneState::default();
        arrive_server_line(&mut state, "echo");
        state.push_local_toast(chat_line("client", "/blarg"));

        let lines = rendered_chat(&state);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].sender, "mob");
        assert_eq!(lines[1].sender, "client");
    }

    fn chat_line(sender: &str, text: &str) -> ChatLine {
        ChatLine {
            spans: Vec::new(),
            channel: ChatChannel::System,
            sender: sender.into(),
            text: text.into(),
            server_ts: 0,
            local_seq: 0,
        }
    }

    fn arrive_server_line(state: &mut SceneState, text: &str) {
        state.snapshot.chat.push(chat_line("mob", text));
        state.observe_server_chat();
    }

    fn rendered_texts(state: &SceneState) -> Vec<String> {
        rendered_chat(state)
            .iter()
            .map(|l| l.text.clone())
            .collect()
    }

    #[test]
    fn rendered_chat_interleaves_by_arrival_seq() {
        let mut state = SceneState::default();
        arrive_server_line(&mut state, "first");
        state.push_local_toast(chat_line("client", "middle"));
        arrive_server_line(&mut state, "last");

        assert_eq!(rendered_texts(&state), vec!["first", "middle", "last"]);
    }

    // The native viewer has no delta path (NativeSource::drain_deltas returns
    // empty), so every poll replaces the whole snapshot. A merge key derived from
    // array position is renumbered by that replacement and collapses to
    // all-server-then-all-toasts, which puts each newly arriving line above the
    // toast block instead of at the bottom (kuluu-zvc3).
    #[test]
    fn interleaving_survives_a_full_snapshot_resend() {
        let mut state = SceneState::default();
        arrive_server_line(&mut state, "first");
        state.push_local_toast(chat_line("client", "middle"));
        arrive_server_line(&mut state, "last");

        state.snapshot = state.snapshot.clone();
        state.observe_server_chat();
        assert_eq!(rendered_texts(&state), vec!["first", "middle", "last"]);

        arrive_server_line(&mut state, "newest");
        assert_eq!(
            rendered_texts(&state),
            vec!["first", "middle", "last", "newest"],
            "a line arriving after the toast must land at the bottom"
        );
    }

    // Absolute indices, not live positions: once the session evicts the oldest
    // lines, the survivors must keep their place relative to the toasts.
    #[test]
    fn eviction_from_session_history_does_not_reorder_survivors() {
        let mut state = SceneState::default();
        arrive_server_line(&mut state, "evicted");
        state.push_local_toast(chat_line("client", "toast"));
        arrive_server_line(&mut state, "kept");

        state.snapshot.chat.remove(0);
        state.snapshot.chat_base_seq += 1;
        state.observe_server_chat();

        assert_eq!(rendered_texts(&state), vec!["toast", "kept"]);
    }

    #[test]
    fn local_toast_cap_drops_oldest() {
        let mut state = SceneState::default();
        for i in 0..(LOCAL_TOAST_CAP + 5) {
            state.push_local_toast(ChatLine {
                spans: Vec::new(),
                channel: ChatChannel::System,
                sender: "client".into(),
                text: format!("toast {i}"),
                server_ts: 0,
                local_seq: 0,
            });
        }
        assert_eq!(state.local_toasts.len(), LOCAL_TOAST_CAP);

        assert_eq!(state.local_toasts[0].text, "toast 5");
        assert!(state.dirty, "push must mark dirty for the panel");
    }

    #[test]
    fn chat_appends_and_caps() {
        let mut snap = SceneSnapshot::default();
        let line = ChatLine {
            spans: Vec::new(),
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

        app.world_mut()
            .resource_mut::<SceneState>()
            .push_local_toast(ChatLine {
                spans: Vec::new(),
                channel: ChatChannel::System,
                sender: "client".into(),
                text: "/sound on".into(),
                server_ts: 0,
                local_seq: 0,
            });

        let mut s = SceneSnapshot::default();
        for text in ["server-a", "server-b"] {
            s.chat.push(ChatLine {
                spans: Vec::new(),
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
            vec!["/sound on", "server-a", "server-b"],
            "the toast preceded both lines, so it keeps its place ahead of them"
        );
    }

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

        app.update();
        assert!(!app.world().resource::<SceneState>().dirty);
    }

    #[test]
    fn empty_poll_frame_does_not_tick_scene_state() {
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
        #[derive(Resource, Default)]
        struct ChangedProbe(bool);
        fn probe(state: Res<SceneState>, mut p: ResMut<ChangedProbe>) {
            p.0 = state.is_changed();
        }

        let mut app = App::new();
        app.init_resource::<TestSource>();
        app.init_resource::<SceneState>();
        app.init_resource::<EventLog>();
        app.init_resource::<ChangedProbe>();
        app.add_systems(Update, (ingest_system::<TestSource>, probe).chain());

        app.world_mut().resource_mut::<TestSource>().next_snapshot =
            Some(Box::new(SceneSnapshot::default()));
        app.update();
        assert!(
            app.world().resource::<ChangedProbe>().0,
            "snapshot arrival must tick SceneState"
        );

        app.update();
        assert!(
            !app.world().resource::<ChangedProbe>().0,
            "an empty poll frame must not tick SceneState (dirty clear bypasses change detection)"
        );
    }

    fn pm(id: u32, hp: u32, hp_pct: u8) -> PartyMember {
        PartyMember {
            id,
            act_index: id as u16,
            name: Some("X".into()),
            hp,
            mp: 0,
            tp: 0,
            hp_pct,
            mp_pct: 0,
            zone_no: 0,
            main_job: 0,
            main_job_lv: 0,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: false,
            is_alliance_leader: false,
            in_mog_house: false,
            party_no: 0,
        }
    }

    #[test]
    fn resolve_self_uses_self_char_id_when_present() {
        let party = vec![pm(1, 100, 100), pm(42, 500, 80)];
        let me = resolve_self(&party, Some(42));
        assert_eq!(me.unwrap().hp, 500);
    }

    #[test]
    fn resolve_self_falls_back_to_first_when_id_unknown() {
        let party = vec![pm(1, 100, 100), pm(42, 500, 80)];
        let me = resolve_self(&party, None);
        assert_eq!(me.unwrap().hp, 100);
    }

    #[test]
    fn resolve_self_falls_back_to_first_when_id_not_in_party() {
        let party = vec![pm(1, 100, 100), pm(42, 500, 80)];
        let me = resolve_self(&party, Some(999));
        assert_eq!(me.unwrap().hp, 100);
    }

    #[test]
    fn resolve_self_returns_none_for_empty_party() {
        let party: Vec<PartyMember> = vec![];
        assert!(resolve_self(&party, Some(42)).is_none());
    }
}
