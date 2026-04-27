#![allow(dead_code)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    process::{ChildStdin, ChildStdout},
    time::timeout,
};

pub fn ffxi_mcp_bin() -> PathBuf {
    let mut p = std::env::current_exe().expect("test current_exe");
    p.pop();
    p.pop();
    p.push(if cfg!(windows) {
        "ffxi-mcp.exe"
    } else {
        "ffxi-mcp"
    });
    p
}

pub async fn is_reachable(host: &str, port: u16) -> bool {
    timeout(Duration::from_millis(750), TcpStream::connect((host, port)))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}

pub struct McpClient {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    buffered_notifications: Vec<Value>,
}

impl McpClient {
    pub fn new(stdin: ChildStdin, stdout: ChildStdout) -> Self {
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
        serde_json::from_str(buf.trim()).with_context(|| format!("parse JSON-RPC frame: {buf:?}"))
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
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
        }
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await
    }

    pub async fn wait_for_notification<F, T>(&mut self, wait: Duration, mut pred: F) -> Option<T>
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

    pub async fn handshake(&mut self) -> Result<Value> {
        let init = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": { "name": "ffxi-it", "version": "0.0.0" },
                }),
            )
            .await
            .context("initialize")?;
        self.notify("notifications/initialized", json!({}))
            .await
            .context("notifications/initialized")?;
        Ok(init)
    }
}

pub fn read_text(result: &Value) -> Option<&str> {
    result
        .get("contents")?
        .as_array()?
        .first()?
        .get("text")?
        .as_str()
}
