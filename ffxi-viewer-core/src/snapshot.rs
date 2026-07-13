use bevy::prelude::*;
use ffxi_viewer_wire::{
    ChatChannel, ChatLine, Entity, PartyMember, SceneDelta, SceneSnapshot, ViewerEvent,
};

use crate::source::SceneSource;

pub const CHAT_HISTORY_CAP: usize = 256;

/// Single render key for zone-keyed DAT resources: inside the Mog House LSB
/// keeps `zone_id` as the surrounding city, so `zone_id` edges miss the swap.
pub fn effective_zone_file_id(snap: &SceneSnapshot) -> Option<u32> {
    ffxi_dat::zone_dat::effective_zone_dat_file_id(snap.zone_id, snap.myroom.map(|m| m.model))
}

#[derive(Resource, Default)]
pub struct SceneState {
    pub snapshot: SceneSnapshot,

    pub dirty: bool,

    pub local_toasts: Vec<ChatLine>,

    pub next_chat_seq: u64,
}

pub const LOCAL_TOAST_CAP: usize = 256;

pub fn system_chat_line(text: String) -> ChatLine {
    ChatLine {
        channel: ChatChannel::System,
        sender: "client".into(),
        text,
        server_ts: 0,
        local_seq: 0,
    }
}

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

    fn stamp_new_server_chat(&mut self, prev_len: usize) {
        let n = self.snapshot.chat.len();
        for i in prev_len..n {
            self.snapshot.chat[i].local_seq = self.next_chat_seq;
            self.next_chat_seq += 1;
        }
    }
}

#[derive(Resource, Default)]
pub struct EventLog {
    pub recent: std::collections::VecDeque<ViewerEvent>,

    pub pushed_total: u64,
}

const EVENT_LOG_CAP: usize = 64;

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

pub fn drain_toast_events(mut state: ResMut<SceneState>, mut events: MessageReader<ToastEvent>) {
    for ev in events.read() {
        state.push_local_toast(ev.line.clone());
    }
}

pub fn ingest_system<S: SceneSource + Resource>(
    mut source: ResMut<S>,
    mut state: ResMut<SceneState>,
    mut events: ResMut<EventLog>,
) {
    state.dirty = false;

    if let Some(snap) = source.poll_snapshot() {
        state.snapshot = *snap;

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
        events.pushed_total += 1;
    }
}

pub fn rendered_chat(state: &SceneState) -> Vec<&ChatLine> {
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
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            status: 0,
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
    fn delta_sets_myroom_and_none_is_no_change() {
        use ffxi_viewer_wire::MyRoom;
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
        use ffxi_viewer_wire::MyRoom;
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
    fn rendered_chat_concatenates_server_then_toasts() {
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
                channel: ChatChannel::System,
                sender: "client".into(),
                text: "/sound on".into(),
                server_ts: 0,
                local_seq: 0,
            });

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
}
