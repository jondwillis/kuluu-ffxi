//! `ffxi-mcp-repl` — line-oriented driver for the MCP layer.
//!
//! Spawns `ffxi-mcp` as a child process and speaks JSON-RPC over its
//! stdio. A human (or a shell script piped on stdin) types one tool
//! call per line and gets back the JSON response; useful for:
//!
//! - Headless regression sequences (`ffxi-mcp-repl < script.txt`).
//! - Poking individual tools while watching the 3D window in
//!   attach mode (`FFXI_ATTACH=auto ffxi-mcp-repl`).
//! - Debugging tool argument schemas without an LLM mediating.
//!
//! # Command grammar
//!
//! ```text
//!   <tool> [key=value ...]
//!   list             # ask the server for its tool list
//!   help             # local cheat-sheet
//!   quit | exit      # stop the REPL (does NOT call `disconnect`)
//! ```
//!
//! Values are parsed by serde_json: bare numbers become numbers,
//! `true`/`false` become booleans, otherwise the literal is wrapped
//! as a string. To force a string that looks numeric, JSON-quote it:
//! `chat kind=0 text="hello"`. Strings containing whitespace need
//! the quotes (we split args on whitespace before key=value parsing).
//!
//! # Environment
//!
//! All `FFXI_*` env vars are inherited by the child. Set `FFXI_ATTACH=auto`
//! before running the REPL to drive a long-lived `ffxi-client native
//! --agent-listen auto` instead of spawning a fresh headless session.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::time::timeout;

/// Locate the sibling `ffxi-mcp` binary by walking up from our own
/// `current_exe()`. Matches the layout cargo produces (`target/debug/`
/// or `target/release/`).
fn ffxi_mcp_bin() -> PathBuf {
    let mut p = std::env::current_exe().expect("current_exe");
    p.pop();
    p.push(if cfg!(windows) {
        "ffxi-mcp.exe"
    } else {
        "ffxi-mcp"
    });
    p
}

struct Mcp {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn new(stdin: ChildStdin, stdout: ChildStdout) -> Self {
        Self {
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
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
        serde_json::from_str(buf.trim()).with_context(|| format!("parse JSON-RPC frame: {buf:?}"))
    }

    /// Send a request and read frames until we see the matching id.
    /// Notifications encountered in between are printed (one line of
    /// `<note> ...` so the operator sees event flow live).
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;
        loop {
            let frame = timeout(Duration::from_secs(30), self.read_one())
                .await
                .with_context(|| format!("waiting for response to {method}"))??;
            if frame.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(err) = frame.get("error") {
                    return Err(anyhow!("server error: {err}"));
                }
                return Ok(frame.get("result").cloned().unwrap_or(Value::Null));
            }
            // Notification — print and continue.
            if let Some(method) = frame.get("method").and_then(|v| v.as_str()) {
                let params = frame.get("params").cloned().unwrap_or(Value::Null);
                eprintln!("<note> {method} {params}");
            }
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
}

/// Parse one input line into a `(tool_name, args_value)` pair, or one
/// of the meta commands. Returns `None` for blank/comment lines.
enum Parsed {
    Tool { name: String, args: Value },
    List,
    Help,
    Quit,
    Empty,
}

fn parse_line(line: &str) -> Result<Parsed> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(Parsed::Empty);
    }
    let mut parts = trimmed.split_ascii_whitespace();
    let first = parts.next().ok_or_else(|| anyhow!("empty line"))?;
    match first {
        "quit" | "exit" => return Ok(Parsed::Quit),
        "help" => return Ok(Parsed::Help),
        "list" | "tools" => return Ok(Parsed::List),
        _ => {}
    }
    let mut args = serde_json::Map::new();
    for kv in parts {
        let mut split = kv.splitn(2, '=');
        let key = split.next().unwrap_or("").to_string();
        let raw = split
            .next()
            .ok_or_else(|| anyhow!("argument `{kv}` is not in key=value form"))?;
        // Try JSON first (catches numbers/bools/quoted strings), fall
        // back to bare-string. `123abc` is a string, `123` is a number.
        let value: Value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.into()));
        args.insert(key, value);
    }
    Ok(Parsed::Tool {
        name: first.to_string(),
        args: Value::Object(args),
    })
}

fn print_help() {
    eprintln!(
        r#"ffxi-mcp-repl — line-oriented MCP driver

  <tool> [key=value ...]   Call a tool. Args are JSON-parsed; quote
                           strings with spaces: text="hello world".
  list                     Ask the server for the tool list.
  help                     This message.
  quit | exit              Stop the REPL.

Examples:
  list
  follow target_id=0x4242 distance=3.0
  chat kind=0 text="hello"
  cast spell_id=257 target_id=99 target_index=7
  snapshot
"#
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let bin = ffxi_mcp_bin();
    if !bin.exists() {
        return Err(anyhow!(
            "ffxi-mcp binary not found at {}. Build first: `cargo build -p ffxi-mcp`",
            bin.display()
        ));
    }

    eprintln!("ffxi-mcp-repl: spawning {}", bin.display());
    let mut child = Command::new(&bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawn {}", bin.display()))?;

    let stdin = child.stdin.take().context("child stdin")?;
    let stdout = child.stdout.take().context("child stdout")?;
    let mut mcp = Mcp::new(stdin, stdout);

    // MCP handshake: initialize + notifications/initialized. We claim
    // protocol_version 2024-11-05 — the rmcp server side negotiates.
    let init = mcp
        .request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "ffxi-mcp-repl", "version": "0.1.0" },
            }),
        )
        .await
        .context("initialize")?;
    eprintln!(
        "ffxi-mcp-repl: connected to {}",
        init.get("serverInfo")
            .and_then(|s| s.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("ffxi-mcp")
    );
    mcp.notify("notifications/initialized", json!({})).await?;
    eprintln!("ffxi-mcp-repl: ready. type `help` for usage.");

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    while let Some(line) = lines.next_line().await? {
        match parse_line(&line) {
            Ok(Parsed::Empty) => continue,
            Ok(Parsed::Quit) => break,
            Ok(Parsed::Help) => print_help(),
            Ok(Parsed::List) => match mcp.request("tools/list", json!({})).await {
                Ok(v) => {
                    let arr = v
                        .get("tools")
                        .and_then(|t| t.as_array())
                        .cloned()
                        .unwrap_or_default();
                    for t in arr {
                        let name = t.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                        let desc = t.get("description").and_then(|d| d.as_str()).unwrap_or("");
                        println!("{name}\t{desc}");
                    }
                }
                Err(e) => eprintln!("error: {e}"),
            },
            Ok(Parsed::Tool { name, args }) => {
                let req = json!({ "name": name, "arguments": args });
                match mcp.request("tools/call", req).await {
                    Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
                    Err(e) => eprintln!("error: {e}"),
                }
            }
            Err(e) => eprintln!("parse error: {e}"),
        }
    }

    eprintln!("ffxi-mcp-repl: shutting down");
    let _ = child.kill().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_blank_is_empty() {
        assert!(matches!(parse_line("").unwrap(), Parsed::Empty));
        assert!(matches!(parse_line("   ").unwrap(), Parsed::Empty));
        assert!(matches!(parse_line("# comment").unwrap(), Parsed::Empty));
    }

    #[test]
    fn parse_meta_commands() {
        assert!(matches!(parse_line("quit").unwrap(), Parsed::Quit));
        assert!(matches!(parse_line("exit").unwrap(), Parsed::Quit));
        assert!(matches!(parse_line("help").unwrap(), Parsed::Help));
        assert!(matches!(parse_line("list").unwrap(), Parsed::List));
        assert!(matches!(parse_line("tools").unwrap(), Parsed::List));
    }

    #[test]
    fn parse_tool_with_numeric_and_string_args() {
        match parse_line("follow target_id=42 distance=3.5").unwrap() {
            Parsed::Tool { name, args } => {
                assert_eq!(name, "follow");
                assert_eq!(args["target_id"], 42);
                assert_eq!(args["distance"], 3.5);
            }
            _ => panic!("expected Tool"),
        }
    }

    #[test]
    fn parse_tool_with_quoted_string_arg() {
        match parse_line(r#"chat kind=0 text="hi""#).unwrap() {
            Parsed::Tool { name, args } => {
                assert_eq!(name, "chat");
                assert_eq!(args["kind"], 0);
                assert_eq!(args["text"], "hi");
            }
            _ => panic!("expected Tool"),
        }
    }

    #[test]
    fn parse_tool_with_bare_string_falls_back() {
        // `123abc` isn't valid JSON; should become a string.
        match parse_line("foo k=123abc").unwrap() {
            Parsed::Tool { args, .. } => assert_eq!(args["k"], "123abc"),
            _ => panic!("expected Tool"),
        }
    }

    #[test]
    fn parse_rejects_bare_arg_without_eq() {
        assert!(parse_line("follow target_id").is_err());
    }
}
