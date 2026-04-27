mod common;

use std::time::Duration;

use ffxi_client::{
    agent_io,
    session::{self, CharSelection, Config},
    state::{AgentCommand, AgentEvent, EntityKind, Stage},
};
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc},
    time::timeout,
};

use common::EphemeralChar;

#[derive(Default)]
struct EventTally {
    stages_seen: Vec<Stage>,
    pc_entity_seen: bool,
    npc_entity_seen: bool,
    bundles_after_zone_in: u32,
    disconnected_reason: Option<String>,
}

#[tokio::test]
async fn play_lifecycle_against_live_lsb() {
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
    let _ = agent_io::run;

    let session_task = tokio::spawn(session::run(cfg, cmd_rx, event_tx));

    let mut tally = EventTally::default();
    let observation_window = Duration::from_secs(20);
    let observe_until = std::time::Instant::now() + observation_window;
    let mut sent_disconnect = false;

    loop {
        let elapsed = std::time::Instant::now();
        if elapsed >= observe_until {
            break;
        }
        match timeout(observe_until - elapsed, event_rx.recv()).await {
            Ok(Ok(ev)) => {
                if process_event(&ev, &mut tally) {
                    break;
                }

                if !sent_disconnect
                    && tally.stages_seen.contains(&Stage::InZone)
                    && tally.pc_entity_seen
                {
                    let _ = cmd_tx.send(AgentCommand::Disconnect).await;
                    sent_disconnect = true;
                }
            }
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) => break,
            Err(_) => break,
        }
    }

    drop(cmd_tx);

    match timeout(Duration::from_secs(5), session_task).await {
        Ok(Ok(Ok(()))) => eprintln!("session task ended cleanly"),
        Ok(Ok(Err(e))) => eprintln!("session task returned Err: {e:#}"),
        Ok(Err(join_err)) => eprintln!("session task panicked: {join_err}"),
        Err(_) => eprintln!(
            "session task did not finish within 5s after cmd_tx drop \
             (likely blocked in I/O — last stages: {:?})",
            tally.stages_seen,
        ),
    }

    if let Err(e) = fixture.cleanup().await {
        eprintln!("fixture cleanup failed (non-fatal for this test): {e:#}");
    }

    assert!(
        tally
            .stages_seen
            .windows(2)
            .all(|w| stage_order(w[0]) <= stage_order(w[1])),
        "stages must arrive in non-decreasing order, got {:?}",
        tally.stages_seen,
    );
    assert!(
        tally.stages_seen.contains(&Stage::Authenticating),
        "missing Authenticating stage",
    );
    assert!(
        tally.stages_seen.contains(&Stage::InZone),
        "session never reached InZone (stages: {:?})",
        tally.stages_seen,
    );
    assert!(
        tally.pc_entity_seen,
        "no CHAR_PC entity for our char arrived in the zone-in flood",
    );
    assert!(
        tally.bundles_after_zone_in >= 1 || tally.disconnected_reason.is_some(),
        "no in-zone bundles followed zone-in (keepalive not eliciting server replies)",
    );
}

fn process_event(ev: &AgentEvent, tally: &mut EventTally) -> bool {
    match ev {
        AgentEvent::StageChanged { stage } => {
            if !tally.stages_seen.contains(stage) {
                tally.stages_seen.push(*stage);
            }
        }
        AgentEvent::EntityUpserted { entity, .. } => {
            if entity.kind == EntityKind::Pc {
                tally.pc_entity_seen = true;
            }
            if entity.kind == EntityKind::Npc {
                tally.npc_entity_seen = true;
            }

            if tally.stages_seen.contains(&Stage::InZone) {
                tally.bundles_after_zone_in += 1;
            }
        }
        AgentEvent::Disconnected { reason } => {
            tally.disconnected_reason = Some(reason.clone());
            return true;
        }
        _ => {}
    }
    false
}

fn stage_order(s: Stage) -> u8 {
    match s {
        Stage::Idle => 0,
        Stage::Authenticating => 1,
        Stage::LobbyHandshake => 2,
        Stage::MapBootstrap => 3,
        Stage::Zoning => 4,
        Stage::InZone => 5,
        Stage::Disconnected => 6,
    }
}

async fn is_reachable(host: &str, port: u16) -> bool {
    timeout(Duration::from_millis(750), TcpStream::connect((host, port)))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}
