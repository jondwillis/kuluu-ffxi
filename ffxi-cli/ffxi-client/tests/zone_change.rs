//! Integration test for the zone-change + reconnect + Blowfish-key-rotation
//! flow.
//!
//! Stamps out a fresh ephemeral account+char with `gmlevel = 5` (sufficient
//! to issue `!zone N`) via `tests/common/mod.rs::EphemeralChar`, runs the
//! flow, then deletes the rows it created. The 60-second LSB
//! `accounts_sessions` lockout is sidestepped because each run uses a brand
//! new accid.
//!
//! Skips automatically if no LSB stack is reachable.

mod common;

use std::time::Duration;

use ffxi_client::{
    session::{self, Config},
    state::{AgentCommand, AgentEvent, Stage},
};
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc},
    time::timeout,
};

use common::EphemeralChar;

/// West Ronfaure — a real zone the server recognises. Picked for
/// connectivity, not lore.
const TARGET_ZONE_ID: u16 = 100;

#[tokio::test]
async fn zone_change_reconnects_with_rotated_key() {
    let server_host = std::env::var("SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let map_host_override = std::env::var("MAP_HOST_OVERRIDE").ok();
    let auth_port = std::env::var("AUTH_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(54231);

    if !is_reachable(&server_host, auth_port).await {
        eprintln!("skipping: LSB stack not reachable at {server_host}:{auth_port}");
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
        "fixture: user={} accid={} charid={} charname={} gmlevel=5",
        fixture.username, fixture.accid, fixture.charid, fixture.charname,
    );

    let cfg = Config {
        server: server_host.clone(),
        map_host_override,
        auth_port,
        data_port: 54230,
        view_port: 54001,
        user: fixture.username.clone(),
        password: fixture.password.clone(),
        char_id: fixture.charid,
        char_name: fixture.charname.clone(),
    };

    let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>(32);
    let (event_tx, mut event_rx) = broadcast::channel::<AgentEvent>(1024);
    let session_task = tokio::spawn(session::run(cfg, cmd_rx, event_tx));

    let mut in_zone_count = 0u32;
    let mut key_rotated = false;
    let mut zone_changed = false;
    let mut sent_zone_cmd = false;

    // Window: long enough for two full bootstraps + zone-in floods.
    let observe_until = std::time::Instant::now() + Duration::from_secs(45);

    loop {
        let now = std::time::Instant::now();
        if now >= observe_until {
            break;
        }
        match timeout(observe_until - now, event_rx.recv()).await {
            Ok(Ok(ev)) => {
                match &ev {
                    AgentEvent::StageChanged {
                        stage: Stage::InZone,
                    } => {
                        in_zone_count += 1;
                        if in_zone_count == 1 && !sent_zone_cmd {
                            // First time in-zone: send the GM zone command.
                            // !zone is LSB's GM teleport command; see
                            // `server/scripts/commands/zone.lua`. Requires
                            // `chars.gmlevel >= 1`.
                            let _ = cmd_tx
                                .send(AgentCommand::Chat {
                                    kind: 0,
                                    text: format!("!zone {TARGET_ZONE_ID}"),
                                })
                                .await;
                            sent_zone_cmd = true;
                        }
                    }
                    AgentEvent::ZoneChanged { .. } => zone_changed = true,
                    AgentEvent::KeyRotated { .. } => key_rotated = true,
                    AgentEvent::Disconnected { .. } => break,
                    _ => {}
                }
                if in_zone_count >= 2 && key_rotated {
                    // We've reconnected and zoned in to the new map.
                    let _ = cmd_tx.send(AgentCommand::Disconnect).await;
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
             (in_zone_count={in_zone_count}, key_rotated={key_rotated}, \
              zone_changed={zone_changed}, sent_zone_cmd={sent_zone_cmd})",
        ),
    }

    if let Err(e) = fixture.cleanup().await {
        eprintln!("fixture cleanup failed (non-fatal for this test): {e:#}");
    }

    assert!(
        sent_zone_cmd,
        "never reached InZone, so !zone command was never issued",
    );
    assert!(
        zone_changed,
        "no ZoneChanged event seen — server's 0x00B never decoded",
    );
    assert!(
        key_rotated,
        "no KeyRotated event seen — reconnect path didn't fire",
    );
    assert!(
        in_zone_count >= 2,
        "expected InZone twice (old zone + new zone), saw {in_zone_count}",
    );
}

async fn is_reachable(host: &str, port: u16) -> bool {
    timeout(Duration::from_millis(750), TcpStream::connect((host, port)))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}
