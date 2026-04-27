//! Integration test that drives `ffxi-mcp` end-to-end as an MCP client.
//!
//! Spawns a fresh `EphemeralChar`, launches `ffxi-mcp` as a child process
//! with `FFXI_*` env, and speaks JSON-RPC over stdio:
//!
//!   1. `initialize` + `notifications/initialized`
//!   2. `tools/list` — assert the v1 tool set is present
//!   3. `resources/list` — assert the v1 resource set is present
//!   4. `resources/subscribe scene://current`
//!   5. Poll `diagnostics://session` until the session reaches `InZone`
//!   6. `resources/read scene://current` — assert non-empty prose
//!   7. `tools/call snapshot` → expect `notifications/resources/updated`
//!      for `scene://current` (closes the wake-on-signal contract loop)
//!   8. `tools/call disconnect` — clean shutdown
//!
//! Skips automatically when no LSB stack is reachable, mirroring
//! `play_lifecycle.rs`/`zone_change.rs`. Requires `cargo build -p ffxi-mcp`
//! to have produced the binary in the workspace `target/{debug,release}`
//! directory; otherwise the test panics with an explicit instruction.

mod common;

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    process::{ChildStdin, ChildStdout, Command},
    time::timeout,
};

use common::EphemeralChar;

/// Locate the `ffxi-mcp` binary by walking up from this test's executable.
/// Cargo lays tests out as `target/{profile}/deps/<test>-<hash>`, so two
/// `pop`s land us in the profile dir alongside sibling-crate binaries.
fn ffxi_mcp_bin() -> PathBuf {
    let mut p = std::env::current_exe().expect("test current_exe");
    p.pop(); // deps/
    p.pop(); // {debug,release}/
    p.push(if cfg!(windows) { "ffxi-mcp.exe" } else { "ffxi-mcp" });
    p
}

async fn is_reachable(host: &str, port: u16) -> bool {
    timeout(Duration::from_millis(750), TcpStream::connect((host, port)))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}

/// Tiny line-delimited JSON-RPC client over a child's stdio. Holds a
/// queue of buffered notifications encountered while awaiting responses
/// so the caller can later drain them deterministically.
struct McpClient {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    buffered_notifications: Vec<Value>,
}

impl McpClient {
    fn new(stdin: ChildStdin, stdout: ChildStdout) -> Self {
        Self {
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            buffered_notifications: Vec::new(),
        }
    }

    async fn send(&mut self, msg: Value) -> Result<()> {
        let line = serde_json::to_string(&msg)?;
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn read_one(&mut self) -> Result<Value> {
        let mut buf = String::new();
        let n = self.stdout.read_line(&mut buf).await?;
        if n == 0 {
            return Err(anyhow!("ffxi-mcp closed stdout"));
        }
        serde_json::from_str(buf.trim())
            .with_context(|| format!("parse JSON-RPC frame: {buf:?}"))
    }

    /// Send a request and read messages until the matching response
    /// arrives. Notifications encountered along the way go to the
    /// `buffered_notifications` queue.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.send(req).await?;
        loop {
            let msg = self.read_one().await?;
            if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(err) = msg.get("error") {
                    return Err(anyhow!("RPC error for {method}: {err}"));
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            if msg.get("method").is_some() && msg.get("id").is_none() {
                self.buffered_notifications.push(msg);
            }
            // Other shapes (responses to in-flight requests we don't track)
            // are dropped; the test only issues one request at a time.
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await
    }

    /// Drain buffered notifications then read more from stdout for up to
    /// `wait`, returning the first one for which `pred` yields `Some(_)`.
    async fn wait_for_notification<F, T>(&mut self, wait: Duration, mut pred: F) -> Option<T>
    where
        F: FnMut(&Value) -> Option<T>,
    {
        let buffered = std::mem::take(&mut self.buffered_notifications);
        for n in buffered {
            if let Some(t) = pred(&n) {
                return Some(t);
            }
        }
        let deadline = Instant::now() + wait;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            match timeout(deadline - now, self.read_one()).await {
                Ok(Ok(msg)) => {
                    if msg.get("method").is_some() && msg.get("id").is_none() {
                        if let Some(t) = pred(&msg) {
                            return Some(t);
                        }
                    }
                }
                _ => return None,
            }
        }
    }
}

/// Pull the first `text` field out of an MCP `resources/read` result.
fn read_text<'a>(result: &'a Value) -> Option<&'a str> {
    result
        .get("contents")?
        .as_array()?
        .first()?
        .get("text")?
        .as_str()
}

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

    // Isolate goal-store writes from the operator's `~/.config/ffxi-mcp`.
    let goal_path = std::env::temp_dir().join(format!(
        "ffxi-mcp-it-goal-{}.json",
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

    let outcome = run_protocol(&mut client, &fixture.charname).await;

    // Tear down: drop client → child sees EOF on stdin → exits.
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
    // Invariant from EphemeralChar::create. Asserting it here so a future
    // regression in fixture provisioning fails loudly, instead of silently
    // skipping the scene-name assertion below.
    assert!(!charname.is_empty(), "fixture must supply a non-empty charname");
    // 1) initialize
    let init = client
        .request(
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "agent-session-it", "version": "0.0.0" },
            }),
        )
        .await
        .context("initialize")?;
    let server_info = init.get("serverInfo").ok_or_else(|| anyhow!("no serverInfo"))?;
    eprintln!("server: {server_info}");

    client
        .notify("notifications/initialized", json!({}))
        .await
        .context("notifications/initialized")?;

    // 2) tools/list
    let tools = client
        .request("tools/list", json!({}))
        .await
        .context("tools/list")?;
    let tool_names: Vec<String> = tools
        .get("tools")
        .and_then(|t| t.as_array())
        .ok_or_else(|| anyhow!("tools/list missing tools[]"))?
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str().map(String::from)))
        .collect();
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
        "disconnect",
    ];
    for need in required_tools {
        if !tool_names.iter().any(|n| n == need) {
            return Err(anyhow!(
                "tools/list missing {need}; got {tool_names:?}"
            ));
        }
    }

    // 3) resources/list
    let resources = client
        .request("resources/list", json!({}))
        .await
        .context("resources/list")?;
    let uris: Vec<String> = resources
        .get("resources")
        .and_then(|r| r.as_array())
        .ok_or_else(|| anyhow!("resources/list missing resources[]"))?
        .iter()
        .filter_map(|r| r.get("uri").and_then(|u| u.as_str().map(String::from)))
        .collect();
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

    // 4) Subscribe so step 7 can verify a notification arrives.
    let _ = client
        .request(
            "resources/subscribe",
            json!({"uri": "scene://current"}),
        )
        .await
        .context("resources/subscribe scene://current")?;

    // 5) Wait for InZone via diagnostics://session. 60s ceiling matches
    //    play_lifecycle's observation window plus headroom for warm cargo.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut reached_in_zone = false;
    while Instant::now() < deadline {
        let res = client
            .request(
                "resources/read",
                json!({"uri": "diagnostics://session"}),
            )
            .await
            .context("resources/read diagnostics://session")?;
        let body = read_text(&res).unwrap_or("");
        // Stage serializes either as "InZone" (Debug) or "in_zone" depending
        // on serde rename — accept both so the test isn't fragile to that.
        if body.contains("InZone") || body.contains("in_zone") {
            reached_in_zone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if !reached_in_zone {
        return Err(anyhow!("session never reached InZone within 60s"));
    }

    // 6) scene://current — non-empty prose.
    let scene = client
        .request("resources/read", json!({"uri": "scene://current"}))
        .await
        .context("resources/read scene://current")?;
    let scene_text = read_text(&scene).unwrap_or("").trim().to_string();
    if scene_text.is_empty() {
        return Err(anyhow!("scene://current returned empty text"));
    }
    eprintln!("scene://current → {scene_text}");
    // Stronger floor: the summarizer should reflect *our* character. The
    // `charname` here is the fixture's `EphemeralChar::charname` (passed as
    // a parameter), guaranteed non-empty by the assert at function entry —
    // so this assertion always runs and catches a regression where
    // SessionState→SceneSummary silently drops the name. The earlier
    // version of this check read `std::env::var("FFXI_CHAR")` from the test
    // process, which was empty (the env var is set on the *child* process),
    // making the check dead code.
    if !scene_text.contains(charname) {
        return Err(anyhow!(
            "scene://current did not mention character {charname:?}: {scene_text}"
        ));
    }

    // 7) snapshot triggers a SceneSummary event → notifier maps it to
    //    notifications/resources/updated for scene://current.
    let _ = client
        .request(
            "tools/call",
            json!({"name": "snapshot", "arguments": {}}),
        )
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

    // 8) Clean disconnect — supervisor will not retry.
    let _ = client
        .request(
            "tools/call",
            json!({"name": "disconnect", "arguments": {}}),
        )
        .await
        .context("tools/call disconnect")?;

    Ok(())
}
