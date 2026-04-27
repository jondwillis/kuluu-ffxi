use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use ffxi_viewer_wire::{self as wire, ClientFrame, Frame, PROTOCOL_VERSION};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_tungstenite::tungstenite::{
    handshake::server::{ErrorResponse, Request, Response},
    Message,
};

use crate::state::{AgentCommand, AgentEvent, SessionState};
use crate::wire_translate::{event_to_viewer_event, state_to_snapshot};

pub fn parse_relay_listen(s: &str) -> std::result::Result<SocketAddr, String> {
    if s.eq_ignore_ascii_case("auto") {
        return Ok(SocketAddr::from(([127, 0, 0, 1], 0)));
    }
    s.parse()
        .map_err(|e: std::net::AddrParseError| format!("expected `auto` or `host:port`: {e}"))
}

pub fn preflight_bind(addr: SocketAddr) -> Result<()> {
    if addr.port() == 0 {
        return Ok(());
    }
    let _l = std::net::TcpListener::bind(addr)
        .with_context(|| format!("--relay-listen {addr}: pre-flight bind failed"))?;
    Ok(())
}

pub async fn serve(
    addr: SocketAddr,
    state_rx: watch::Receiver<SessionState>,
    event_tx: broadcast::Sender<AgentEvent>,
    cmd_tx: mpsc::Sender<AgentCommand>,
) -> Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding relay listener to {addr}"))?;
    let bound = listener
        .local_addr()
        .ok()
        .map(|a| a.to_string())
        .unwrap_or_else(|| addr.to_string());

    eprintln!("relay listening on ws://{bound}/ (use `?ws=ws://{bound}` from the browser viewer)");
    tracing::info!(addr = %bound, "ffxi viewer relay listening");

    let cmd_tx = Arc::new(cmd_tx);

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(error = %err, "relay accept failed");
                continue;
            }
        };
        let state_rx = state_rx.clone();
        let event_rx = event_tx.subscribe();
        let cmd_tx = cmd_tx.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, peer, state_rx, event_rx, cmd_tx).await {
                tracing::debug!(peer = %peer, error = %err, "relay connection ended");
            }
        });
    }
}

#[allow(clippy::result_large_err)]
async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    mut state_rx: watch::Receiver<SessionState>,
    mut event_rx: broadcast::Receiver<AgentEvent>,
    cmd_tx: Arc<mpsc::Sender<AgentCommand>>,
) -> Result<()> {
    let mut want_json = false;
    let want_json_ref = &mut want_json;
    let ws_stream = tokio_tungstenite::accept_hdr_async(
        stream,
        |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
            if let Some(query) = req.uri().query() {
                if query.split('&').any(|kv| kv == "format=json") {
                    *want_json_ref = true;
                }
            }
            Ok(resp)
        },
    )
    .await
    .context("websocket handshake")?;

    let format = if want_json {
        WireFormat::Json
    } else {
        WireFormat::Postcard
    };
    tracing::debug!(peer = %peer, ?format, "relay client connected");

    let (mut sink, mut stream) = ws_stream.split();

    send_frame(
        &mut sink,
        format,
        &Frame::Hello {
            protocol_version: PROTOCOL_VERSION,
        },
    )
    .await?;

    {
        let snap = {
            let guard = state_rx.borrow();
            state_to_snapshot(&guard)
        };

        let _ = state_rx.borrow_and_update();
        send_frame(&mut sink, format, &Frame::Snapshot(Box::new(snap))).await?;
    }

    loop {
        tokio::select! {

            changed = state_rx.changed() => {
                if changed.is_err() {

                    break;
                }
                let snap = {
                    let guard = state_rx.borrow_and_update();
                    state_to_snapshot(&guard)
                };
                send_frame(&mut sink, format, &Frame::Snapshot(Box::new(snap))).await?;
            }

            ev = event_rx.recv() => match ev {
                Ok(ev) => {
                    if let Some(translated) = event_to_viewer_event(ev) {
                        send_frame(&mut sink, format, &Frame::Event(translated)).await?;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {

                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },

            msg = stream.next() => match msg {
                Some(Ok(Message::Binary(data))) => {
                    match postcard::from_bytes::<ClientFrame>(&data) {
                        Ok(ClientFrame::Command(cmd)) => {
                            if let Some(translated) = viewer_command_to_agent(cmd) {
                                if cmd_tx.send(translated).await.is_err() {

                                    break;
                                }
                            }
                        }
                        Ok(ClientFrame::Hello { .. }) => {

                        }
                        Err(err) => {
                            tracing::debug!(peer = %peer, error = %err, "decoding ClientFrame failed");
                        }
                    }
                }
                Some(Ok(Message::Text(_))) => {

                    tracing::trace!(peer = %peer, "ignoring inbound text frame");
                }
                Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {}
                Some(Err(err)) => {
                    tracing::debug!(peer = %peer, error = %err, "websocket read error");
                    break;
                }
                None => break,
            }
        }
    }

    let _ = sink.close().await;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum WireFormat {
    Postcard,
    Json,
}

async fn send_frame<S>(sink: &mut S, format: WireFormat, frame: &Frame) -> Result<()>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let msg = match format {
        WireFormat::Postcard => {
            let bytes = postcard::to_allocvec(frame).context("postcard encoding Frame")?;
            Message::Binary(bytes)
        }
        WireFormat::Json => {
            let s = serde_json::to_string(frame).context("json encoding Frame")?;
            Message::Text(s)
        }
    };
    sink.send(msg).await.context("sending websocket frame")?;
    Ok(())
}

fn viewer_command_to_agent(cmd: wire::ViewerCommand) -> Option<AgentCommand> {
    use crate::state::ActionKind;
    Some(match cmd {
        wire::ViewerCommand::Move { x, y, z, heading } => AgentCommand::Move { x, y, z, heading },
        wire::ViewerCommand::StopMove => AgentCommand::StopMove,
        wire::ViewerCommand::EndEvent => AgentCommand::EndEvent,
        wire::ViewerCommand::Snapshot => AgentCommand::Snapshot,
        wire::ViewerCommand::Chat { kind, text } => AgentCommand::Chat { kind, text },
        wire::ViewerCommand::Tell { to, text } => AgentCommand::Tell { to, text },
        wire::ViewerCommand::Follow {
            target_id,
            distance,
        } => AgentCommand::Follow {
            target_id,
            distance,
        },
        wire::ViewerCommand::Engage { target_id } => AgentCommand::Engage { target_id },

        wire::ViewerCommand::PathTo { x, y, z } => AgentCommand::PathTo {
            x,
            y,
            z,
            force: false,
        },
        wire::ViewerCommand::Cancel => AgentCommand::Cancel,
        wire::ViewerCommand::Cast {
            spell_id,
            target_id,
            target_index,
            pos_x,
            pos_y,
            pos_z,
        } => AgentCommand::Action {
            target_id,
            target_index,
            kind: ActionKind::CastMagic {
                spell_id,
                pos_x,
                pos_y,
                pos_z,
            },
        },
        wire::ViewerCommand::Weaponskill {
            skill_id,
            target_id,
            target_index,
        } => AgentCommand::Action {
            target_id,
            target_index,
            kind: ActionKind::Weaponskill { skill_id },
        },
        wire::ViewerCommand::JobAbility {
            ability_id,
            target_id,
            target_index,
        } => AgentCommand::Action {
            target_id,
            target_index,
            kind: ActionKind::JobAbility { ability_id },
        },
        wire::ViewerCommand::UseItem {
            container,
            slot,
            item_no,
            target_id,
            target_index,
        } => AgentCommand::UseItem {
            container,
            slot,
            item_no,
            target_id,
            target_index,
        },
        wire::ViewerCommand::BankWhenFull {
            threshold,
            mog_house_zoneline,
        } => AgentCommand::BankWhenFull {
            threshold,
            mog_house_zoneline,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ActionKind;

    #[test]
    fn cast_translates_to_action_castmagic() {
        let translated = viewer_command_to_agent(wire::ViewerCommand::Cast {
            spell_id: 0x101,
            target_id: 0xCAFE,
            target_index: 7,
            pos_x: 1.5,
            pos_y: 0.0,
            pos_z: -2.5,
        })
        .expect("translation");
        match translated {
            AgentCommand::Action {
                target_id,
                target_index,
                kind:
                    ActionKind::CastMagic {
                        spell_id,
                        pos_x,
                        pos_y,
                        pos_z,
                    },
            } => {
                assert_eq!(target_id, 0xCAFE);
                assert_eq!(target_index, 7);
                assert_eq!(spell_id, 0x101);
                assert_eq!(pos_x, 1.5);
                assert_eq!(pos_y, 0.0);
                assert_eq!(pos_z, -2.5);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn weaponskill_and_job_ability_share_action_envelope() {
        match viewer_command_to_agent(wire::ViewerCommand::Weaponskill {
            skill_id: 0xBEEF,
            target_id: 0xCAFE,
            target_index: 7,
        })
        .expect("translation")
        {
            AgentCommand::Action {
                kind: ActionKind::Weaponskill { skill_id },
                target_id,
                target_index,
            } => {
                assert_eq!(skill_id, 0xBEEF);
                assert_eq!(target_id, 0xCAFE);
                assert_eq!(target_index, 7);
            }
            other => panic!("ws: {other:?}"),
        }
        match viewer_command_to_agent(wire::ViewerCommand::JobAbility {
            ability_id: 0xABCD,
            target_id: 0,
            target_index: 0,
        })
        .expect("translation")
        {
            AgentCommand::Action {
                kind: ActionKind::JobAbility { ability_id },
                ..
            } => assert_eq!(ability_id, 0xABCD),
            other => panic!("ja: {other:?}"),
        }
    }

    #[test]
    fn use_item_passes_through_all_fields() {
        match viewer_command_to_agent(wire::ViewerCommand::UseItem {
            container: 8,
            slot: 4,
            item_no: 4112,
            target_id: 0xCAFE,
            target_index: 7,
        })
        .expect("translation")
        {
            AgentCommand::UseItem {
                container,
                slot,
                item_no,
                target_id,
                target_index,
            } => {
                assert_eq!(container, 8);
                assert_eq!(slot, 4);
                assert_eq!(item_no, 4112);
                assert_eq!(target_id, 0xCAFE);
                assert_eq!(target_index, 7);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn bank_when_full_passes_through() {
        match viewer_command_to_agent(wire::ViewerCommand::BankWhenFull {
            threshold: 60,
            mog_house_zoneline: 0xDEAD_BEEF,
        })
        .expect("translation")
        {
            AgentCommand::BankWhenFull {
                threshold,
                mog_house_zoneline,
            } => {
                assert_eq!(threshold, 60);
                assert_eq!(mog_house_zoneline, 0xDEAD_BEEF);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
