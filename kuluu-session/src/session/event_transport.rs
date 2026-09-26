use ffxi_event::vm::scene::{EventPosition, SceneAction, EVENT_COORD_UNITS, EVENT_HEADING_UNITS};
use tokio::sync::broadcast;

use super::codec::{build_subpacket_event_position, build_subpacket_pos};
use crate::event_dialog::{Advance, DialogSession, ResolvedCue};
use crate::map_client::MapClient;
use crate::state::{AgentEvent, Position, Vec3};

pub(crate) enum Drive {
    Cancel,
    Choice(u32),
    Tick(f32),
}

pub(crate) struct DrivePermit(());

#[must_use = "an event step must transmit its complete packet before exposing its outcome"]
pub(super) struct PreparedStep {
    advance: Advance,
    cues: Vec<ResolvedCue>,
    payload: Vec<u8>,
    datagram_id: u16,
}

impl PreparedStep {
    pub(super) async fn send(
        self,
        map: &mut MapClient,
        server_last_seq: u16,
    ) -> anyhow::Result<(Advance, Vec<ResolvedCue>)> {
        if !self.payload.is_empty() {
            map.send_encrypted(&self.payload, self.datagram_id, server_last_seq)
                .await?;
        }
        Ok((self.advance, self.cues))
    }
}

pub(super) fn prepare(
    dialog: &mut DialogSession,
    drive: Drive,
    zone: u16,
    pending: &mut Vec<(u32, u16, u16)>,
    sequence: &mut u16,
    position: &mut Position,
    events: &broadcast::Sender<AgentEvent>,
) -> Option<PreparedStep> {
    let (actor, index, event) = dialog.active_end()?;
    let advance = dialog.step(drive, &DrivePermit(()));
    let cues = dialog.take_cues();
    let mut actions = dialog.drain_scene_actions(&DrivePermit(()));
    if let Advance::Ended {
        final_position: Some(final_position),
        ..
    } = &advance
    {
        if !matches!(actions.last(), Some(SceneAction::PlayerPosition(last)) if last == final_position)
        {
            actions.push(SceneAction::PlayerPosition(*final_position));
        }
    }
    let mut payload = encode_scene_actions(
        actions,
        (actor, index, event),
        zone,
        sequence,
        position,
        events,
    );
    if let Advance::Ended { end_para, .. } = &advance {
        if super::take_pending_event_end(pending, actor, event) {
            // vendor/server/src/map/map_networking.cpp MapNetworking::parse dispatches in payload order.
            payload.extend(super::build_subpacket_event_end(
                *sequence,
                actor,
                index,
                zone,
                event,
                *end_para,
                ffxi_proto::map::c2s::event_end_mode::END,
            ));
            *sequence = sequence.wrapping_add(1);
        }
    }
    Some(PreparedStep {
        advance,
        cues,
        payload,
        datagram_id: super::datagram_header_id(*sequence),
    })
}

/// Fold one in-event sub-packet into the dialog: WPOS2 confirms or rejects
/// a scripted position, EVENTUCOFF acknowledges or cancels, and the
/// CHAR_PC/CHAR_NPC placement is the source for MOVE hold lengths when a
/// scene walks its actors.
pub(super) fn receive(
    dialog: &mut DialogSession,
    sub: &ffxi_proto::framing::SubPacket<'_>,
    player: u32,
    position: Position,
) {
    use ffxi_proto::{decode, map};
    dialog.note_player_id(player);
    match sub.opcode {
        map::s2c::WPOS2 => {
            if let Ok(movement) = decode::ForcedMove::decode(sub.data) {
                if movement.unique_no == player
                    && matches!(
                        movement.mode,
                        decode::PosMode::Event | decode::PosMode::Clear
                    )
                {
                    if movement.mode == decode::PosMode::Event {
                        let accepted = Position {
                            pos: Vec3 {
                                x: movement.x,
                                y: movement.y,
                                z: movement.z,
                            },
                            heading: movement.heading,
                            ..position
                        };
                        dialog.acknowledge_position(event_position(accepted));
                    } else {
                        dialog.reject_position();
                    }
                }
            }
        }
        map::s2c::CHAR_PC | map::s2c::CHAR_NPC => {
            if let Ok(head) = decode::PosHead::decode(sub.data) {
                dialog.note_entity_position(
                    head.unique_no,
                    event_position(Position {
                        pos: Vec3 {
                            x: head.x,
                            y: head.y,
                            z: head.z,
                        },
                        heading: head.dir,
                        ..Default::default()
                    }),
                );
                // The 0x5B/0x66 gate's input: the entity Type byte this 0x0E's
                // SubKind dispatch writes (decode::LookData::retail_type).
                // research/XiEvents/OpCodes/0x005B.md
                if let Some(t) = decode::LookData::retail_type(sub.opcode, sub.data) {
                    dialog.note_entity_type(head.unique_no, head.act_index, t);
                }
            }
        }
        map::s2c::EVENTUCOFF => match super::eventucoff_mode_of(sub.data) {
            Some(map::event_position_wire::EVENT_RECV_PENDING) => dialog.acknowledge_event(),
            Some(map::eventucoff_mode::CANCEL_EVENT) => dialog.clear(),
            _ => {}
        },
        _ => {}
    }
}

const WIRE_HEADING_UNITS: f32 = (u8::MAX as u16 + 1) as f32;

// The wire and the event VM use DIFFERENT vertical axes, so this pair is a
// y<->z swap, not an identity:
//   * the wire (and the session Position it carries) is FFXI's native
//     (x, y = ground, z = height) — y is the horizontal plane coordinate,
//     z the vertical. Proven at login: a char at wire (x, 109.08, 8.0) in
//     Upper Jeuno has y=109.08 on the ground plane and z=8.0 of height.
//   * the retail event VM (XiEvent) is (x, y = height, z = ground): CodeMOVE
//     (research/XiEvents/OpCodes/0x001F.md) walks EventPos[0]/EventPos[2]
//     (x, z) and snaps EventPos[1] (y) to the floor, and SET_EVENT_POS
//     (0x0037.md) VCalibrate-snaps EventPos[1] to the ground height.
// So wire z (height) is event y, and wire y (ground) is event z.
pub(super) fn event_position(position: Position) -> EventPosition {
    EventPosition {
        x: (position.pos.x * EVENT_COORD_UNITS) as i32,
        y: (position.pos.z * EVENT_COORD_UNITS) as i32,
        z: (position.pos.y * EVENT_COORD_UNITS) as i32,
        heading: (f32::from(position.heading) * EVENT_HEADING_UNITS / WIRE_HEADING_UNITS) as i32,
    }
}

pub(super) fn session_position(position: EventPosition, previous: Position) -> Position {
    Position {
        pos: Vec3 {
            x: position.x as f32 / EVENT_COORD_UNITS,
            y: position.z as f32 / EVENT_COORD_UNITS,
            z: position.y as f32 / EVENT_COORD_UNITS,
        },
        heading: (position.heading as f32 / EVENT_HEADING_UNITS * WIRE_HEADING_UNITS) as i32 as u8,
        ..previous
    }
}

fn encode_scene_actions(
    actions: Vec<SceneAction>,
    identity: (u32, u16, u16),
    zone: u16,
    sequence: &mut u16,
    position: &mut Position,
    events: &broadcast::Sender<AgentEvent>,
) -> Vec<u8> {
    let mut payload = Vec::new();
    for action in actions {
        let encoded = match action {
            SceneAction::PlayerPosition(next) => {
                *position = session_position(next, *position);
                let _ = events.send(AgentEvent::PositionChanged { pos: *position });
                // vendor/server/src/map/packets/c2s/0x015_pos.cpp GP_CLI_COMMAND_POS::process
                // accepts scripted walking in-event; the final POS must precede EVENTEND.
                build_subpacket_pos(
                    *sequence,
                    position.pos.x,
                    position.pos.y,
                    position.pos.z,
                    position.heading,
                    0,
                )
            }
            SceneAction::PositionUpdate {
                position: next,
                end_para,
            } => Some(build_subpacket_event_position(
                *sequence,
                identity,
                zone,
                end_para,
                session_position(next, *position),
            )),
        };
        if let Some(sub_packet) = encoded {
            payload.extend(sub_packet);
            *sequence = sequence.wrapping_add(1);
        }
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::contracts::NPC;
    use super::*;

    /// The fixture heading, in event units; the value coincides with
    /// ffxi-event's motion band edge, which is unrelated.
    const HEADING_EVENT_UNITS_PINNED: i32 = 3072;

    #[test]
    fn event_coordinates_and_heading_roundtrip_through_session_axes() {
        // Event VM space (x, y = height, z = ground): the char sits at
        // height -2.558, ground -31.432. The session (wire) space is
        // (x, y = ground, z = height), so the conversion swaps y and z.
        let authored = EventPosition {
            x: 33_762,
            y: -2_558,  // height
            z: -31_432, // ground
            heading: HEADING_EVENT_UNITS_PINNED,
        };
        let converted = session_position(authored, Position::default());
        assert!((converted.pos.x - 33.762).abs() < 0.001);
        assert!(
            (converted.pos.y + 31.432).abs() < 0.001,
            "wire y is the ground"
        );
        assert!(
            (converted.pos.z + 2.558).abs() < 0.001,
            "wire z is the height"
        );
        assert_eq!(converted.heading, 192);
        assert_eq!(event_position(converted), authored);
    }

    #[test]
    fn position_update_packet_matches_lsb_eventendxzy_layout() {
        let position = Position {
            pos: Vec3 {
                x: 33.762,
                y: -31.432,
                z: -2.558,
            },
            heading: 192,
            ..Position::default()
        };
        let packet = build_subpacket_event_position(19, (NPC, 54, 221), 248, 7, position);
        assert_eq!(packet.len(), 32);
        assert_eq!(u16::from_le_bytes(packet[2..4].try_into().unwrap()), 19);
        for (offset, expected) in [
            (4, position.pos.x),
            (8, position.pos.z),
            (12, position.pos.y),
        ] {
            assert_eq!(
                f32::from_le_bytes(packet[offset..offset + 4].try_into().unwrap()),
                expected
            );
        }
        assert_eq!(u32::from_le_bytes(packet[16..20].try_into().unwrap()), NPC);
        assert_eq!(u32::from_le_bytes(packet[20..24].try_into().unwrap()), 7);
        assert_eq!(u16::from_le_bytes(packet[24..26].try_into().unwrap()), 248);
        assert_eq!(u16::from_le_bytes(packet[26..28].try_into().unwrap()), 221);
        assert_eq!(u16::from_le_bytes(packet[28..30].try_into().unwrap()), 54);
        assert_eq!(
            packet[30],
            ffxi_proto::map::event_position_wire::UPDATE_PENDING as u8
        );
        assert_eq!(packet[31], 192);
    }
}

#[cfg(test)]
pub(crate) mod contracts;
