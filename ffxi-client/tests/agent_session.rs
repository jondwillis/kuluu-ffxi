mod common;

use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::{process::Command, time::timeout};

use common::mcp_client::{build_ffxi_mcp, ffxi_mcp_bin, is_reachable, read_text, McpClient};
use common::EphemeralChar;

#[tokio::test]
async fn agent_session_drives_mcp_end_to_end() {
    let server_host = std::env::var("SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let auth_port: u16 = std::env::var("AUTH_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(54231);

    if !is_reachable(&server_host, auth_port).await {
        eprintln!("skipping: LSB stack not reachable at {server_host}:{auth_port}");
        return;
    }

    // ffxi-mcp is a separate crate, so CARGO_BIN_EXE_ isn't available here;
    // rebuild it so a stale target/debug/ffxi-mcp is never spawned.
    build_ffxi_mcp().expect("cargo build -p ffxi-mcp");
    let bin = ffxi_mcp_bin();
    assert!(
        bin.exists(),
        "ffxi-mcp binary not found at {} even after `cargo build -p ffxi-mcp`",
        bin.display()
    );

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

    let goal_path = std::env::temp_dir().join(format!("ffxi-mcp-it-goal-{}.json", fixture.charid));
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

    let outcome = run_protocol(&mut client, &fixture.charname).await;

    drop(client);
    match timeout(Duration::from_secs(5), child.wait()).await {
        Ok(Ok(status)) => eprintln!("ffxi-mcp exited: {status:?}"),
        Ok(Err(e)) => eprintln!("ffxi-mcp wait err: {e:#}"),
        Err(_) => {
            eprintln!("ffxi-mcp didn't exit in 5s after stdin close, killing");
            let _ = child.start_kill();
            let _ = timeout(Duration::from_secs(2), child.wait()).await;
        }
    }

    let _ = std::fs::remove_file(&goal_path);
    if let Err(e) = fixture.cleanup().await {
        eprintln!("fixture cleanup failed (non-fatal): {e:#}");
    }

    outcome.expect("agent session protocol assertions");
}

async fn run_protocol(client: &mut McpClient, charname: &str) -> Result<()> {
    assert!(
        !charname.is_empty(),
        "fixture must supply a non-empty charname"
    );

    let init = client.handshake().await?;
    let server_info = init
        .get("serverInfo")
        .ok_or_else(|| anyhow!("no serverInfo"))?;
    eprintln!("server: {server_info}");

    let tools = client.request("tools/list", json!({})).await?;
    let tool_names = list_field_strings(&tools, "tools", "name")?;
    let required_tools = [
        "follow",
        "engage",
        "path_to",
        "cancel",
        "chat",
        "tell",
        "request_zone_change",
        "end_event",
        "snapshot",
        "cast",
        "weaponskill",
        "job_ability",
        "use_item",
        "disconnect",
    ];
    for need in required_tools {
        if !tool_names.iter().any(|n| n == need) {
            return Err(anyhow!("tools/list missing {need}; got {tool_names:?}"));
        }
    }

    let resources = client.request("resources/list", json!({})).await?;
    let uris = list_field_strings(&resources, "resources", "uri")?;
    let required_resources = [
        "scene://current",
        "party://members",
        "diagnostics://session",
        "goal://current",
    ];
    for need in required_resources {
        if !uris.iter().any(|u| u == need) {
            return Err(anyhow!("resources/list missing {need}; got {uris:?}"));
        }
    }

    client
        .request("resources/subscribe", json!({"uri": "scene://current"}))
        .await
        .context("resources/subscribe scene://current")?;

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut reached_in_zone = false;
    while Instant::now() < deadline {
        let res = client
            .request("resources/read", json!({"uri": "diagnostics://session"}))
            .await
            .context("resources/read diagnostics://session")?;
        let body = read_text(&res).unwrap_or("");

        if body.contains("InZone") || body.contains("in_zone") {
            reached_in_zone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if !reached_in_zone {
        return Err(anyhow!("session never reached InZone within 60s"));
    }

    let scene = client
        .request("resources/read", json!({"uri": "scene://current"}))
        .await
        .context("resources/read scene://current")?;
    let scene_text = read_text(&scene).unwrap_or("").trim().to_string();
    if scene_text.is_empty() {
        return Err(anyhow!("scene://current returned empty text"));
    }
    eprintln!("scene://current → {scene_text}");
    if !scene_text.contains(charname) {
        return Err(anyhow!(
            "scene://current did not mention character {charname:?}: {scene_text}"
        ));
    }

    client
        .request("tools/call", json!({"name": "snapshot", "arguments": {}}))
        .await
        .context("tools/call snapshot")?;
    let updated = client
        .wait_for_notification(Duration::from_secs(5), |n| {
            if n.get("method")?.as_str()? != "notifications/resources/updated" {
                return None;
            }
            let uri = n.get("params")?.get("uri")?.as_str()?;
            (uri == "scene://current").then(|| uri.to_string())
        })
        .await;
    if updated.is_none() {
        return Err(anyhow!(
            "no resources/updated for scene://current within 5s after snapshot"
        ));
    }

    client
        .request("tools/call", json!({"name": "disconnect", "arguments": {}}))
        .await
        .context("tools/call disconnect")?;

    Ok(())
}

fn list_field_strings(result: &Value, list_key: &str, item_key: &str) -> Result<Vec<String>> {
    Ok(result
        .get(list_key)
        .and_then(|t| t.as_array())
        .ok_or_else(|| anyhow!("missing {list_key}[]"))?
        .iter()
        .filter_map(|t| t.get(item_key).and_then(|n| n.as_str().map(String::from)))
        .collect())
}
