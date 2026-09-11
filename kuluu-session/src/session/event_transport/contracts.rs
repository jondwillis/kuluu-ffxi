use super::*;
use ffxi_dat::event_dat::{EventBlock, EventDat, ZONE_PLAYER_ACTOR};
use ffxi_proto::{decode::PosMode, framing, map};

const PLAYER: u32 = 17_455_719;
const NPC: u32 = 17_793_078;
const INDEX: u16 = 54;
const EVENT: u16 = 221;
const ZONE: u16 = 248;
const TEXT_ZONE: u16 = 230;
const TICK: f32 = 0.2;
const FARE: i32 = 100;
const SCRIPT_FARE: i32 = 200;
const INITIAL: Position = Position {
    pos: Vec3 {
        x: 1.25,
        y: -2.5,
        z: 3.75,
    },
    heading: 32,
    speed: 0,
    speed_base: 0,
};
const ACCEPTED: Position = Position {
    pos: Vec3 {
        x: 44.125,
        y: -61.25,
        z: 8.5,
    },
    heading: 96,
    speed: 0,
    speed_base: 0,
};

// research/XiEvents/OpCodes/0x0002.md CodeIF; 0x0003.md; 0x001D.md CodeMESSAGE.
const OP_IF: u8 = 0x02;
const OP_SET: u8 = 0x03;
const OP_MESSAGE: u8 = 0x1D;
const OP_MESSAGE_WAIT: u8 = 0x23;
const OP_END: u8 = 0x21;
const OP_REQUEST_WAIT: u8 = 0x29;
const OP_POSITION: u8 = 0x47;
const COMPARE_LESS: u8 = 4;
const WORK_GIL: u16 = 0x1002;
const WORK_FARE: u16 = 0x1003;
const REFERENCE: u16 = 0x8000;

fn operand(program: &mut Vec<u8>, value: u16) {
    program.extend(value.to_le_bytes());
}
fn message(program: &mut Vec<u8>, index: u16) {
    program.push(OP_MESSAGE);
    operand(program, REFERENCE + index);
    program.push(OP_MESSAGE_WAIT);
}

fn block(program: Vec<u8>, references: Vec<u32>) -> EventBlock {
    EventBlock {
        actor: NPC,
        event_ids: vec![EVENT],
        event_offsets: vec![0],
        event_data: program,
        references,
    }
}

fn affordability_dat() -> EventDat {
    let mut program = Vec::new();
    message(&mut program, 0);
    program.push(OP_IF);
    operand(&mut program, WORK_GIL);
    operand(&mut program, WORK_FARE);
    program.push(COMPARE_LESS);
    let jump = program.len();
    operand(&mut program, 0);
    program.push(OP_SET);
    operand(&mut program, WORK_FARE);
    operand(&mut program, REFERENCE + 3);
    message(&mut program, 1);
    program.push(OP_END);
    let poor = program.len() as u16;
    program[jump..jump + 2].copy_from_slice(&poor.to_le_bytes());
    message(&mut program, 2);
    program.push(OP_END);
    EventDat {
        blocks: vec![block(program, vec![0, 1, 2, SCRIPT_FARE as u32])],
    }
}

fn position_dat(child: bool) -> EventDat {
    let mut program = vec![OP_POSITION, 0];
    for reference in 0..4 {
        operand(&mut program, REFERENCE + reference);
    }
    program.extend([OP_POSITION, 1, OP_END]);
    let movement = block(
        program,
        vec![33_762, (-31_432i32) as u32, (-2_558i32) as u32, 3072],
    );
    if !child {
        return EventDat {
            blocks: vec![movement],
        };
    }
    let mut root = vec![OP_REQUEST_WAIT, 0];
    root.extend(ffxi_event::ActorLookup::LOCAL_PLAYER.0.to_le_bytes());
    root.extend([1, OP_END]);
    let mut child = movement;
    child.actor = ZONE_PLAYER_ACTOR;
    child.event_ids = vec![0, u16::MAX];
    child.event_offsets = vec![0, 1];
    child.event_data.insert(0, OP_END);
    EventDat {
        blocks: vec![block(root, vec![]), child],
    }
}

// vendor/server/src/map/packets/s2c/0x034_eventnum.h GP_SERV_COMMAND_EVENTNUM.
fn trigger(gil: i32) -> crate::event_dialog::EventTrigger {
    let mut body = [0u8; 48];
    body[0..4].copy_from_slice(&NPC.to_le_bytes());
    body[4..8].copy_from_slice(&gil.to_le_bytes());
    body[8..12].copy_from_slice(&FARE.to_le_bytes());
    body[36..38].copy_from_slice(&INDEX.to_le_bytes());
    body[38..40].copy_from_slice(&ZONE.to_le_bytes());
    body[40..42].copy_from_slice(&EVENT.to_le_bytes());
    body[44..46].copy_from_slice(&TEXT_ZONE.to_le_bytes());
    super::super::event_trigger(&framing::SubPacket {
        opcode: map::s2c::EVENTNUM,
        sequence: 0,
        data: &body,
    })
    .unwrap()
}

struct Host {
    dialog: DialogSession,
    pending: Vec<(u32, u16, u16)>,
    sequence: u16,
    position: Position,
    events: broadcast::Sender<AgentEvent>,
    receiver: broadcast::Receiver<AgentEvent>,
}
impl Host {
    fn new(dat: EventDat, gil: i32) -> Self {
        let (events, receiver) = broadcast::channel(64);
        let mut host = Self {
            dialog: crate::event_dialog::tests::contract_session(dat, ZONE, TEXT_ZONE),
            pending: vec![],
            sequence: u16::MAX,
            position: INITIAL,
            events,
            receiver,
        };
        host.begin(gil);
        host
    }
    fn begin(&mut self, gil: i32) {
        self.dialog
            .set_player_position(event_position(self.position));
        let mut automatic = vec![];
        super::super::begin_server_event(
            &mut self.dialog,
            trigger(gil),
            &self.events,
            &mut crate::event_dialog::CutsceneScope::default(),
            &mut self.pending,
            &mut automatic,
        );
        assert!(
            automatic.is_empty(),
            "contract fixture must be driven, not auto-released"
        );
    }
    fn step(&mut self, drive: Drive) -> PreparedStep {
        prepare(
            &mut self.dialog,
            drive,
            ZONE,
            &mut self.pending,
            &mut self.sequence,
            &mut self.position,
            &self.events,
        )
        .unwrap()
    }
    fn packet(&mut self, opcode: u16, data: &[u8]) {
        receive(
            &mut self.dialog,
            &framing::SubPacket {
                opcode,
                sequence: 0,
                data,
            },
            PLAYER,
            self.position,
        );
    }
    fn event_ack(&mut self) {
        self.packet(
            map::s2c::EVENTUCOFF,
            &map::event_position_wire::EVENT_RECV_PENDING.to_le_bytes(),
        );
    }
    fn position_ack(&mut self, player: u32, mode: PosMode) {
        if player == PLAYER && mode == PosMode::Clear {
            self.position = Position {
                pos: Vec3 {
                    x: -444.0,
                    y: 333.0,
                    z: 222.0,
                },
                ..INITIAL
            };
        }

        // vendor/server/src/map/packets/s2c/0x065_wpos2.h GP_SERV_COMMAND_WPOS2.
        let mut body = [0u8; 24];
        body[0..4].copy_from_slice(&ACCEPTED.pos.x.to_le_bytes());
        body[4..8].copy_from_slice(&ACCEPTED.pos.z.to_le_bytes());
        body[8..12].copy_from_slice(&ACCEPTED.pos.y.to_le_bytes());
        body[12..16].copy_from_slice(&player.to_le_bytes());
        body[18] = mode as u8;
        body[19] = ACCEPTED.heading;
        self.packet(map::s2c::WPOS2, &body);
    }
    fn waiting(&mut self) {
        let step = self.step(Drive::Tick(TICK));
        assert!(matches!(step.advance, Advance::Waiting));
        assert!(packets(&step)
            .iter()
            .all(|p| p.opcode != map::c2s::EVENT_END));
        assert!(!self.pending.is_empty());
    }
    fn request(&mut self) {
        let step = self.step(Drive::Tick(TICK));
        assert!(matches!(step.advance, Advance::Waiting));
        let packets = packets(&step);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].opcode, map::event_position_wire::OPCODE);
        assert_eq!(
            self.position, INITIAL,
            "requested position is not accepted yet"
        );
    }
}
fn packets(step: &PreparedStep) -> Vec<framing::SubPacket<'_>> {
    framing::walk_sub_packets(&step.payload)
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}
fn float(body: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(body[offset..offset + 4].try_into().unwrap())
}

fn numeric_contract() {
    for gil in [0, FARE - 1, FARE, 1_300_000, i32::MAX] {
        let mut host = Host::new(affordability_dat(), gil);
        let initial = std::iter::from_fn(|| host.receiver.try_recv().ok())
            .find_map(|event| {
                if let AgentEvent::EventDialog { dialog } = event {
                    Some(dialog)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(initial.nums[0..2], [gil, FARE]);
        assert!(initial
            .prompt
            .as_ref()
            .unwrap()
            .contains(&format!("Balance {gil}, fare {FARE}")));
        let step = host.step(Drive::Choice(0));
        let Advance::Frame(frame) = step.advance else {
            panic!("missing decision frame")
        };
        let expected_fare = if gil < FARE { FARE } else { SCRIPT_FARE };
        assert_eq!(frame.nums[0..2], [gil, expected_fare]);
        assert!(frame.prompt.as_ref().unwrap().contains(if gil < FARE {
            "Insufficient:"
        } else {
            "Accepted:"
        }));
        assert!(frame
            .prompt
            .as_ref()
            .unwrap()
            .contains(&format!("fare {expected_fare}")));
    }
}
fn acknowledgement_contract() {
    for child in [false, true] {
        for position_first in [false, true] {
            for mode in [PosMode::Event, PosMode::Clear] {
                let mut host = Host::new(position_dat(child), 1_300_000);
                host.request();
                host.position_ack(PLAYER + 1, PosMode::Event);
                host.waiting();
                if position_first {
                    host.position_ack(PLAYER, mode);
                    host.position_ack(PLAYER, mode);
                } else {
                    host.event_ack();
                    host.event_ack();
                }
                host.waiting();
                if position_first {
                    assert_eq!(
                        host.position,
                        if mode == PosMode::Event {
                            ACCEPTED
                        } else {
                            INITIAL
                        }
                    );
                    host.position = Position {
                        pos: Vec3 {
                            x: -999.0,
                            y: 777.0,
                            z: 555.0,
                        },
                        ..INITIAL
                    };
                    host.event_ack();
                } else {
                    host.position_ack(PLAYER, mode);
                }
                let step = host.step(Drive::Tick(TICK));
                assert!(matches!(step.advance, Advance::Ended { .. }));
                let packets = packets(&step);
                assert_eq!(
                    packets.iter().map(|p| p.opcode).collect::<Vec<_>>(),
                    [map::c2s::POS, map::c2s::EVENT_END]
                );
                let expected = if mode == PosMode::Event {
                    ACCEPTED
                } else {
                    INITIAL
                };
                assert_eq!(
                    [
                        float(packets[0].data, 0),
                        float(packets[0].data, 4),
                        float(packets[0].data, 8)
                    ],
                    [expected.pos.x, expected.pos.z, expected.pos.y]
                );
                assert_eq!(packets[0].data[16], expected.heading);
                assert_eq!(host.position, expected);
                assert_eq!(packets[1].sequence, packets[0].sequence.wrapping_add(1));
                assert_eq!(step.datagram_id, packets[1].sequence);
                assert_eq!(
                    u32::from_le_bytes(packets[1].data[0..4].try_into().unwrap()),
                    NPC
                );
                assert_eq!(
                    u16::from_le_bytes(packets[1].data[8..10].try_into().unwrap()),
                    INDEX
                );
                assert_eq!(
                    u16::from_le_bytes(packets[1].data[14..16].try_into().unwrap()),
                    EVENT
                );
                assert!(host.pending.is_empty());
                assert!(host.dialog.active_end().is_none());
            }
        }
    }
}
fn abort_contract() {
    let mut replaced = Host::new(position_dat(false), FARE);
    replaced.begin(FARE);
    replaced.request();

    for queued_ack in [false, true] {
        let mut host = Host::new(position_dat(true), FARE);
        if queued_ack {
            host.request();
            host.position_ack(PLAYER, PosMode::Event);
        }
        let step = host.step(Drive::Cancel);
        assert!(matches!(step.advance, Advance::Ended { .. }));
        assert_eq!(
            packets(&step).iter().map(|p| p.opcode).collect::<Vec<_>>(),
            [map::c2s::EVENT_END]
        );
        assert_eq!(host.position, INITIAL);
        assert!(host.dialog.drain_scene_actions(&DrivePermit(())).is_empty());
    }
    let mut host = Host::new(position_dat(false), FARE);
    host.request();
    host.position_ack(PLAYER, PosMode::Event);
    host.packet(
        map::s2c::EVENTUCOFF,
        &map::eventucoff_mode::CANCEL_EVENT.to_le_bytes(),
    );
    assert!(host.dialog.active_end().is_none());
    assert!(host.dialog.drain_scene_actions(&DrivePermit(())).is_empty());
    host.begin(FARE);
    host.request();
    assert_eq!(host.position, INITIAL);
}

#[test]
fn event_state_contract() {
    super::super::tests::ferry_packet_state_contract();
    super::super::tests::bootstrap_acceptance_contract();
    numeric_contract();
    acknowledgement_contract();
    abort_contract();
}
