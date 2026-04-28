//! Integration test for the supervisor's disconnect-recovery path.
//!
//! Forces a hard map-server crash by `docker restart`-ing the map server
//! container, then asserts the ffxi-mcp supervisor re-authenticates,
//! re-zones, and reaches `InZone` again within budget. Reports the
//! observed downtime to stderr so the operator can compare against the
//! Stage 7 latency target (≤8 s p95 on transient drops, ≤60 s on full
//! container restart).
//!
//! **Destructive**: restarting the map server affects any other test
//! that's using the same stack. Opt-in via `RESTART_MAP_SERVER=1`.
//! Skips when:
//!   - `RESTART_MAP_SERVER` is not set
//!   - the LSB stack isn't reachable
//!   - `docker --version` doesn't run

mod common;

use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use serde_json::json;
use tokio::{process::Command, time::timeout};

use common::EphemeralChar;
use common::mcp_client::{McpClient, ffxi_mcp_bin, is_reachable, read_text};

/// Override with `MAP_SERVER_CONTAINER` if your stack uses a different name.
fn map_server_container() -> String {
    std::env::var("MAP_SERVER_CONTAINER").unwrap_or_else(|_| "server-map-1".into())
}

#[tokio::test]
async fn disconnect_recovery_reconnects_after_map_restart() {
    if std::env::var("RESTART_MAP_SERVER").ok().as_deref() != Some("1") {
        eprintln!(
            "skipping: set RESTART_MAP_SERVER=1 to enable. This test \
             restarts the map server container, which can disrupt other \
             concurrent tests using the same dev stack."
        );
        return;
    }

    let server_host = std::env::var("SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let auth_port: u16 = std::env::var("AUTH_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(54231);

    if !is_reachable(&server_host, auth_port).await {
        eprintln!("skipping: LSB stack not reachable at {server_host}:{auth_port}");
        return;
    }

    if !docker_available().await {
        eprintln!("skipping: `docker` not invokable from PATH");
        return;
    }

    let bin = ffxi_mcp_bin();
    if !bin.exists() {
        panic!(
            "ffxi-mcp binary not found at {}.\n\
             Build it first: `cargo build -p ffxi-mcp`",
            bin.display()
        );
    }

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,ffxi_client=debug,ffxi_mcp=debug")
            }),
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

    let goal_path = std::env::temp_dir().join(format!(
        "ffxi-mcp-recover-{}.json",
        fixture.charid
    ));
    let _ = std::fs::remove_file(&goal_path);

    let mut child = Command::new(&bin)
        .env("FFXI_USER", &fixture.username)
        .env("FFXI_PASS", &fixture.password)
        .env("FFXI_CHAR_ID", fixture.charid.to_string())
        .env("FFXI_CHAR", &fixture.charname)
        .env("FFXI_SERVER", &server_host)
        .env("FFXI_AUTH_PORT", auth_port.to_string())
        .env("FFXI_MCP_GOAL_PATH", &goal_path)
        .env("RUST_LOG", "info,ffxi_client=info,ffxi_mcp=info")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn ffxi-mcp");

    let stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let mut client = McpClient::new(stdin, stdout);

    let outcome = run_protocol(&mut client).await;

    drop(client);
    match timeout(Duration::from_secs(5), child.wait()).await {
        Ok(Ok(status)) => eprintln!("ffxi-mcp exited: {status:?}"),
        Ok(Err(e)) => eprintln!("ffxi-mcp wait err: {e:#}"),
        Err(_) => {
            let _ = child.start_kill();
            let _ = timeout(Duration::from_secs(2), child.wait()).await;
        }
    }

    let _ = std::fs::remove_file(&goal_path);
    if let Err(e) = fixture.cleanup().await {
        eprintln!("fixture cleanup failed (non-fatal): {e:#}");
    }

    let downtime = outcome.expect("disconnect recovery protocol assertions");
    eprintln!(
        "reconnect_downtime_ms={} (target: ≤60000, plan p95 for warm drops: ≤8000)",
        downtime.as_millis()
    );
}

async fn run_protocol(client: &mut McpClient) -> Result<Duration> {
    client.handshake().await.context("MCP handshake")?;

    wait_for_in_zone(client, Duration::from_secs(60))
        .await
        .context("initial wait for InZone before crash")?;
    eprintln!("agent reached InZone — about to crash the map server");

    let container = map_server_container();
    let crash_at = Instant::now();
    let status = Command::new("docker")
        .args(["restart", "-t", "0", &container])
        .status()
        .await
        .with_context(|| format!("invoking `docker restart {container}`"))?;
    if !status.success() {
        return Err(anyhow!("docker restart {container} exited {status}"));
    }
    eprintln!(
        "docker restart finished after {} ms; awaiting supervisor reconnect",
        crash_at.elapsed().as_millis()
    );

    // Phase 1: supervisor must notice the connection died. Stage drops out
    // of in_zone (typically to disconnected/authenticating).
    wait_for_stage(client, Duration::from_secs(30), |stage| stage != "in_zone")
        .await
        .context("supervisor never noticed the disconnect within 30s")?;
    eprintln!("supervisor noticed disconnect");

    // Phase 2: supervisor must re-auth → re-zone-in → reach in_zone again.
    let recovered_at = wait_for_in_zone(client, Duration::from_secs(60))
        .await
        .context("supervisor failed to recover to InZone within 60s")?;
    let downtime = recovered_at.duration_since(crash_at);

    // Sanity-check: the goal store should still be readable. We didn't set a
    // goal so it's "idle", but the resource itself surviving the supervisor
    // re-spawn is the structural assertion.
    let goal = client
        .request("resources/read", json!({"uri": "goal://current"}))
        .await
        .context("read goal://current after recovery")?;
    let body = read_text(&goal).unwrap_or("");
    if body.is_empty() {
        return Err(anyhow!("goal://current returned empty body after recovery"));
    }

    client
        .request("tools/call", json!({"name": "disconnect", "arguments": {}}))
        .await
        .context("clean disconnect after recovery")?;

    Ok(downtime)
}

async fn wait_for_in_zone(client: &mut McpClient, deadline: Duration) -> Result<Instant> {
    let until = Instant::now() + deadline;
    while Instant::now() < until {
        let res = client
            .request("resources/read", json!({"uri": "diagnostics://session"}))
            .await?;
        let body = read_text(&res).unwrap_or("");
        if let Some(stage) = parse_stage(body) {
            if stage == "in_zone" {
                return Ok(Instant::now());
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(anyhow!("did not reach in_zone within {deadline:?}"))
}

async fn wait_for_stage<F: Fn(&str) -> bool>(
    client: &mut McpClient,
    deadline: Duration,
    pred: F,
) -> Result<String> {
    let until = Instant::now() + deadline;
    while Instant::now() < until {
        let res = client
            .request("resources/read", json!({"uri": "diagnostics://session"}))
            .await?;
        let body = read_text(&res).unwrap_or("");
        if let Some(stage) = parse_stage(body) {
            if pred(&stage) {
                return Ok(stage);
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(anyhow!("predicate not satisfied within {deadline:?}"))
}

/// Pull the `stage` field out of the diagnostics JSON body. Parses with
/// serde_json::Value so it doesn't have to ABI-link against the
/// `Diagnostics` struct (which would re-export the entire ffxi-client
/// dep tree into the test).
fn parse_stage(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("stage")?.as_str().map(String::from)
}

async fn docker_available() -> bool {
    Command::new("docker")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}
