use super::*;
use crate::map_client::MapClient;
use crate::state::ActionKind;
use ffxi_dat::event_dat::{EventBlock, EventDat, ZONE_PLAYER_ACTOR};
use ffxi_proto::{decode::PosMode, framing, map};

const PLAYER: u32 = 17_455_719;
/// The retail NPC these fixtures key off; other session tests name the same
/// actor, so the id is importable rather than re-typed.
pub(crate) const NPC: u32 = 17_793_078;
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
/// The position fixture's heading, in event units; the value coincides with
/// ffxi-event's motion band edge, which is unrelated.
const HEADING_EVENT_UNITS_PINNED: u32 = 3072;
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
        vec![
            33_762,
            (-31_432i32) as u32,
            (-2_558i32) as u32,
            HEADING_EVENT_UNITS_PINNED,
        ],
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
    map: MapClient,
}
impl Host {
    /// Offline fixture socket: an ephemeral UDP bind with no server behind
    /// it; a tag send from Begin::AwaitServerAck just drops.
    async fn new(dat: EventDat, gil: i32) -> Self {
        let (events, receiver) = broadcast::channel(64);
        let map = MapClient::connect_with_local_sync(
            std::net::SocketAddr::from(([127, 0, 0, 1], 9)),
            [0u8; 20],
            "0.0.0.0:0",
        )
        .unwrap();
        let mut host = Self {
            dialog: crate::event_dialog::tests::contract_session(dat, ZONE, TEXT_ZONE),
            pending: vec![],
            sequence: u16::MAX,
            position: INITIAL,
            events,
            receiver,
            map,
        };
        host.begin(gil).await;
        host
    }
    async fn begin(&mut self, gil: i32) {
        self.dialog
            .set_player_position(event_position(self.position));
        let mut automatic = vec![];
        super::super::begin_server_event(
            &mut self.map,
            &mut self.sequence,
            0,
            ZONE,
            &mut self.dialog,
            trigger(gil),
            &self.events,
            &mut crate::event_dialog::CutsceneScope::default(),
            &mut self.pending,
            &mut automatic,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        )
        .await;
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

async fn numeric_contract() {
    for gil in [0, FARE - 1, FARE, 1_300_000, i32::MAX] {
        let mut host = Host::new(affordability_dat(), gil).await;
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
async fn acknowledgement_contract() {
    for child in [false, true] {
        for position_first in [false, true] {
            for mode in [PosMode::Event, PosMode::Clear] {
                let mut host = Host::new(position_dat(child), 1_300_000).await;
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
async fn abort_contract() {
    let mut replaced = Host::new(position_dat(false), FARE).await;
    replaced.begin(FARE).await;
    replaced.request();

    for queued_ack in [false, true] {
        let mut host = Host::new(position_dat(true), FARE).await;
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
    let mut host = Host::new(position_dat(false), FARE).await;
    host.request();
    host.position_ack(PLAYER, PosMode::Event);
    host.packet(
        map::s2c::EVENTUCOFF,
        &map::eventucoff_mode::CANCEL_EVENT.to_le_bytes(),
    );
    assert!(host.dialog.active_end().is_none());
    assert!(host.dialog.drain_scene_actions(&DrivePermit(())).is_empty());
    host.begin(FARE).await;
    host.request();
    assert_eq!(host.position, INITIAL);
}

/// Every [`ActionKind`] paired with the answer
/// vendor/server/src/map/packets/c2s/0x01a_action.cpp
/// GP_CLI_COMMAND_ACTION::validate gives it under BlockedState::InEvent.
fn action_kinds() -> Vec<(ActionKind, bool)> {
    vec![
        (ActionKind::Attack, true),
        (
            ActionKind::CastMagic {
                spell_id: 1,
                pos_x: 0.0,
                pos_y: 0.0,
                pos_z: 0.0,
            },
            true,
        ),
        (ActionKind::JobAbility { ability_id: 1 }, true),
        (ActionKind::Shoot, true),
        (ActionKind::Weaponskill { skill_id: 1 }, true),
        (ActionKind::MonsterSkill { skill_id: 1 }, true),
        (ActionKind::Fish, true),
        (ActionKind::Mount { mount_id: 0 }, true),
        (ActionKind::Talk, false),
        (ActionKind::AttackOff, false),
        (ActionKind::Help, false),
        (ActionKind::HomepointMenu { status_id: 0 }, false),
        (ActionKind::Assist, false),
        (ActionKind::RaiseMenu { accept: true }, false),
        (ActionKind::ChangeTarget, false),
        (ActionKind::ChocoboDig, false),
        (ActionKind::Dismount, false),
        (ActionKind::TractorMenu { accept: true }, false),
        (ActionKind::SendResRdy, false),
        (ActionKind::Quarry, false),
        (ActionKind::Sprint, false),
        (ActionKind::Scout, false),
        (ActionKind::Blockaid { status_id: 0 }, false),
    ]
}

/// Drive a fixture event to its end, asserting it emits 0x05B EVENT_END and
/// releases the in-event state.
/// vendor/server/src/map/packets/c2s/0x05b_eventend.cpp
fn end_event(host: &mut Host) {
    let step = host.step(Drive::Cancel);
    assert!(matches!(step.advance, Advance::Ended { .. }));
    assert_eq!(
        packets(&step).iter().map(|p| p.opcode).collect::<Vec<_>>(),
        [map::c2s::EVENT_END]
    );
    assert!(
        !super::super::in_event(&host.dialog, &host.pending),
        "EVENT_END released the event"
    );
}

/// vendor/server/src/map/packets/c2s/0x01a_action.cpp
/// GP_CLI_COMMAND_ACTION::validate refuses Attack, CastMagic, JobAbility,
/// Shoot, Weaponskill, MonsterSkill, Fish and Mount while the character is
/// InEvent, so the client must not spend a 0x01A on one until its 0x05B
/// EVENT_END has gone out.
async fn action_event_gate_contract() {
    const ACTION_ID: std::ops::Range<usize> = 10..12;

    let mut host = Host::new(position_dat(true), FARE).await;
    let open = super::super::in_event(&host.dialog, &host.pending);
    assert!(open, "the fixture event is open before its EVENT_END");
    for (kind, blocked) in action_kinds() {
        let encoded = super::super::build_subpacket_action(0, NPC, INDEX, &kind, open);
        assert_eq!(encoded.is_err(), blocked, "{kind:?} while InEvent");
        if let Ok(packet) = encoded {
            assert_eq!(
                u16::from_le_bytes(packet[ACTION_ID].try_into().unwrap()),
                kind.action_id()
            );
        }
    }

    end_event(&mut host);
    let released = super::super::in_event(&host.dialog, &host.pending);
    for (kind, _) in action_kinds() {
        let packet = super::super::build_subpacket_action(0, NPC, INDEX, &kind, released)
            .unwrap_or_else(|reason| panic!("{kind:?} after EVENT_END: {reason}"));
        assert_eq!(
            framing::walk_sub_packets(&packet)
                .next()
                .unwrap()
                .unwrap()
                .opcode,
            map::c2s::ACTION
        );
    }
}

/// vendor/server/src/map/packets/c2s/0x03a_item_stack.cpp
/// GP_CLI_COMMAND_ITEM_STACK::validate refuses the sort while InEvent and
/// accepts only a container id PacketValidator::isValidContainer admits.
async fn item_stack_gate_contract() {
    use ffxi_proto::map::container;
    const CATEGORY: std::ops::Range<usize> = 4..8;

    let mut host = Host::new(position_dat(true), FARE).await;
    let open = super::super::in_event(&host.dialog, &host.pending);
    assert!(open, "the fixture event is open before its EVENT_END");
    for container in 0..=container::MAX_CONTAINER_ID {
        assert!(
            super::super::build_subpacket_item_stack(0, container, open).is_err(),
            "container {container} while InEvent"
        );
    }

    end_event(&mut host);
    let released = super::super::in_event(&host.dialog, &host.pending);
    for container in 0..container::MAX_CONTAINER_ID {
        let packet = super::super::build_subpacket_item_stack(0, container, released)
            .unwrap_or_else(|reason| panic!("container {container}: {reason}"));
        assert_eq!(
            u32::from_le_bytes(packet[CATEGORY].try_into().unwrap()),
            u32::from(container)
        );
    }
    for container in [container::MAX_CONTAINER_ID, u8::MAX] {
        assert!(
            super::super::build_subpacket_item_stack(0, container, released).is_err(),
            "container {container} is not a storage the server owns"
        );
    }
}

/// vendor/server/src/map/packets/c2s/0x015_pos.cpp GP_CLI_COMMAND_POS::process
/// discards the update when x, y or z is not finite.
fn pos_finite_contract() {
    const HEADING: u8 = 192;
    const FINITE: [f32; 3] = [33.762, -31.432, -2.558];

    let packet = build_subpacket_pos(0, FINITE[0], FINITE[1], FINITE[2], HEADING, 0)
        .expect("a finite position is sendable");
    assert_eq!(
        [float(&packet, 4), float(&packet, 12), float(&packet, 8)],
        FINITE
    );

    for poison in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for axis in 0..FINITE.len() {
            let mut coords = FINITE;
            coords[axis] = poison;
            assert!(
                build_subpacket_pos(0, coords[0], coords[1], coords[2], HEADING, 0).is_none(),
                "{poison} on axis {axis} must not reach the wire"
            );
        }
    }
}

/// bootstrap_acceptance_contract blocks on its own current-thread runtime,
/// so it must run outside an active tokio context.
#[test]
fn ferry_and_bootstrap_contracts_hold() {
    super::super::tests::ferry_packet_state_contract();
    super::super::tests::bootstrap_acceptance_contract();
    super::super::tests::bootstrap_enterzone_contract();
}

#[tokio::test]
async fn event_state_contract() {
    numeric_contract().await;
    acknowledgement_contract().await;
    abort_contract().await;
    action_event_gate_contract().await;
    item_stack_gate_contract().await;
    pos_finite_contract();
}
