//! Live delivery-box smoke test against a running LSB stack.
//!
//! Drives the server-side delivery-box flow directly with
//! `AgentCommand::DeliveryBox` ops (no local menu / dialog navigation):
//!
//!   DeliOpen → Opened(Outgoing) → Query(bogus name) → RecipientCheck{ok:false}
//!   → Query(own name) → RecipientCheck{ok:true, same_account:true}
//!   → PostClose → Closed
//!
//! Skips (passes) when no stack is reachable, matching play_lifecycle.rs.

mod common;

use std::time::Duration;

use ffxi_client::{
    session::{self, CharSelection, Config},
    state::{
        AgentCommand, AgentEvent, DeliveryBoxNo, DeliveryBoxOp, DeliveryBoxUpdate, EntityKind,
        Stage,
    },
};
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc},
    time::timeout,
};

use common::EphemeralChar;

#[derive(Debug, Default)]
struct BoxTally {
    in_zone: bool,
    pc_entity_seen: bool,
    opened: Option<DeliveryBoxNo>,
    closed: bool,
    bogus_check: Option<(bool, bool)>,
    self_check: Option<(bool, bool)>,
    failures: Vec<(u8, u8)>,
}

/// Steps the test walks through after zone-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    AwaitZoneIn,
    AwaitOpen,
    AwaitBogusCheck,
    AwaitSelfCheck,
    AwaitClose,
    Done,
}

#[tokio::test]
async fn delivery_box_against_live_lsb() {
    let server_host = std::env::var("SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let map_host_override = std::env::var("MAP_HOST_OVERRIDE").ok();
    let auth_port = std::env::var("AUTH_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(54231);
    let data_port = 54230;
    let view_port = 54001;

    if !is_reachable(&server_host, auth_port).await {
        eprintln!(
            "skipping: no LSB stack reachable at {server_host}:{auth_port}. \
             To run this test, start the dev stack and re-run with SERVER_HOST set."
        );
        return;
    }

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,ffxi_client=debug")),
        )
        .with_test_writer()
        .try_init();

    let fixture = EphemeralChar::create(&server_host, auth_port)
        .await
        .expect("provisioning ephemeral LSB account+char");
    eprintln!(
        "fixture: user={} accid={} charid={} charname={}",
        fixture.username, fixture.accid, fixture.charid, fixture.charname,
    );

    let cfg = Config {
        server: server_host.clone(),
        map_host_override,
        auth_port,
        data_port,
        view_port,
        user: fixture.username.clone(),
        password: fixture.password.clone(),
        char_selection: CharSelection::Name(fixture.charname.clone()),
        initial_state: None,
        dat_root: None,
        user_driven_events: false,
    };

    let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>(32);
    let (event_tx, mut event_rx) = broadcast::channel::<AgentEvent>(512);
    let session_task = tokio::spawn(session::run(cfg, cmd_rx, event_tx));

    let mut tally = BoxTally::default();
    let mut step = Step::AwaitZoneIn;

    // Generous window: auth + lobby + zone-in flood + 5 box round-trips.
    let observe_until = std::time::Instant::now() + Duration::from_secs(45);

    loop {
        let now = std::time::Instant::now();
        if now >= observe_until || step == Step::Done {
            break;
        }
        let ev = match timeout(observe_until - now, event_rx.recv()).await {
            Ok(Ok(ev)) => ev,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) => break,
            Err(_) => break,
        };
        process_event(&ev, &mut tally);

        match step {
            Step::AwaitZoneIn if tally.in_zone && tally.pc_entity_seen => {
                let _ = cmd_tx
                    .send(AgentCommand::DeliveryBox {
                        op: DeliveryBoxOp::DeliOpen,
                    })
                    .await;
                step = Step::AwaitOpen;
            }
            Step::AwaitOpen if tally.opened == Some(DeliveryBoxNo::Outgoing) => {
                let _ = cmd_tx
                    .send(AgentCommand::DeliveryBox {
                        op: DeliveryBoxOp::Query {
                            recipient: "Zzqvxjnope".to_string(),
                        },
                    })
                    .await;
                step = Step::AwaitBogusCheck;
            }
            Step::AwaitBogusCheck if tally.bogus_check.is_some() => {
                let _ = cmd_tx
                    .send(AgentCommand::DeliveryBox {
                        op: DeliveryBoxOp::Query {
                            recipient: fixture.charname.clone(),
                        },
                    })
                    .await;
                step = Step::AwaitSelfCheck;
            }
            Step::AwaitSelfCheck if tally.self_check.is_some() => {
                let _ = cmd_tx
                    .send(AgentCommand::DeliveryBox {
                        op: DeliveryBoxOp::PostClose {
                            box_no: DeliveryBoxNo::Outgoing,
                        },
                    })
                    .await;
                step = Step::AwaitClose;
            }
            Step::AwaitClose if tally.closed => {
                let _ = cmd_tx.send(AgentCommand::Disconnect).await;
                step = Step::Done;
            }
            _ => {}
        }

        // Route each RecipientCheck answer to whichever query is in flight.
        if let AgentEvent::DeliveryBoxUpdated {
            update: DeliveryBoxUpdate::RecipientCheck { ok, same_account },
            ..
        } = &ev
        {
            match step {
                Step::AwaitBogusCheck if tally.bogus_check.is_none() => {
                    tally.bogus_check = Some((*ok, *same_account));
                }
                Step::AwaitSelfCheck if tally.self_check.is_none() => {
                    tally.self_check = Some((*ok, *same_account));
                }
                _ => {}
            }
        }
    }

    drop(cmd_tx);
    match timeout(Duration::from_secs(5), session_task).await {
        Ok(Ok(Ok(()))) => eprintln!("session task ended cleanly"),
        Ok(Ok(Err(e))) => eprintln!("session task returned Err: {e:#}"),
        Ok(Err(join_err)) => eprintln!("session task panicked: {join_err}"),
        Err(_) => eprintln!("session task did not finish within 5s after cmd_tx drop"),
    }

    if let Err(e) = fixture.cleanup().await {
        eprintln!("fixture cleanup failed (non-fatal for this test): {e:#}");
    }

    eprintln!("tally: {tally:?}, final step: {step:?}");

    assert!(tally.in_zone, "session never reached InZone");
    assert_eq!(
        tally.opened,
        Some(DeliveryBoxNo::Outgoing),
        "DeliOpen did not open the outbox (failures: {:?})",
        tally.failures,
    );
    let (bogus_ok, _) = tally
        .bogus_check
        .expect("no RecipientCheck for the nonexistent-name Query");
    assert!(
        !bogus_ok,
        "server claimed the nonexistent recipient name resolves",
    );
    let (self_ok, self_same_account) = tally
        .self_check
        .expect("no RecipientCheck for the own-name Query");
    assert!(self_ok, "own charname failed the recipient Query");
    assert!(
        self_same_account,
        "LSB should flag our own char as same-account (ConfirmNameBeforeSending ResParam1)",
    );
    assert!(
        tally.closed,
        "PostClose never answered with Closed (failures: {:?})",
        tally.failures,
    );
    assert_eq!(step, Step::Done, "did not complete the full flow");
}

fn process_event(ev: &AgentEvent, tally: &mut BoxTally) {
    match ev {
        AgentEvent::StageChanged { stage } => {
            if *stage == Stage::InZone {
                tally.in_zone = true;
            }
        }
        AgentEvent::EntityUpserted { entity, .. } => {
            if entity.kind == EntityKind::Pc {
                tally.pc_entity_seen = true;
            }
        }
        AgentEvent::DeliveryBoxUpdated { box_no, update } => match update {
            DeliveryBoxUpdate::Opened => tally.opened = Some(*box_no),
            DeliveryBoxUpdate::Closed => tally.closed = true,
            DeliveryBoxUpdate::Failed { command, result } => {
                tally.failures.push((*command, *result));
            }
            _ => {}
        },
        _ => {}
    }
}

async fn is_reachable(host: &str, port: u16) -> bool {
    timeout(Duration::from_millis(750), TcpStream::connect((host, port)))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}
