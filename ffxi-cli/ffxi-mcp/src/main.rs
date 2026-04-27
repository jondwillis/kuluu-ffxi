//! `ffxi-mcp` — MCP server bridging an `ffxi-client` session to LLM
//! harnesses (Claude Code, OpenCode, pi.dev). Communicates over stdio.
//!
//! Architecture:
//!
//! ```text
//!   harness (LLM)
//!     │  MCP/stdio
//!     ▼
//!   ffxi-mcp ──cmd_tx──▶ supervisor → reactor → session → server
//!     │
//!     └─event_rx── (mirrors SessionState into shared RwLock)
//! ```
//!
//! Tools dispatch to `cmd_tx`. Resources read from the mirror. The
//! `goal://current` resource persists across reconnects (the supervisor
//! writes `~/.config/ffxi-mcp/goal.json`); resources without disk
//! persistence rebuild from the live broadcast stream.
//!
//! Tools and resources cover the **v1** primitives — Follow / Engage /
//! PathTo / Chat / Snapshot / Disconnect plus Scene / Party / Goal /
//! Diagnostics. Adding more is additive: a tool is a method; a
//! resource is a `match` arm in `read_resource`. No protocol churn.

use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::wrapper::Parameters,
    model::{
        Annotated, CallToolResult, Content, ListResourcesResult, PaginatedRequestParams,
        ProtocolVersion, RawResource, ReadResourceRequestParams, ReadResourceResult, Resource,
        ResourceContents, ServerCapabilities, ServerInfo,
    },
    service::{RequestContext, RoleServer, serve_server},
    tool, tool_handler, tool_router,
    transport::io::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

use ffxi_client::{
    goal_store::GoalStore,
    reactor::ReactorConfig,
    scene::SceneSummary,
    session,
    state::{AgentCommand, AgentEvent, SessionState},
    supervisor::{self, SupervisorConfig},
};

// ----- Tool parameter schemas ---------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct FollowParams {
    /// `UniqueNo` of the entity to follow (party leader, mob, NPC).
    target_id: u32,
    /// Yalms to hold once close. 5.0 is melee range; 18.0 is casting range.
    distance: f32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EngageParams {
    target_id: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PathToParams {
    x: f32,
    y: f32,
    z: f32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ChatParams {
    /// 0=say, 1=shout, 4=party, 5=linkshell, 6=tell. Server-side say
    /// messages beginning with `@` dispatch as GM commands when the
    /// account has `gmlevel ≥ 1`.
    kind: u8,
    text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ZoneChangeParams {
    /// `RectID` of the zoneline. The agent must walk into ~40 yalms of
    /// the zoneline first; this command is "I'm here, take me through".
    line_id: u32,
}

// ----- Server state -------------------------------------------------

#[derive(Clone)]
struct FfxiServer {
    cmd_tx: mpsc::Sender<AgentCommand>,
    /// Live mirror of `SessionState`, updated by a background task
    /// that consumes the broadcast event stream.
    state: Arc<RwLock<SessionState>>,
    /// Persisted goal store. Resources read via `goal://current`.
    goal_store: Arc<Mutex<GoalStore>>,
}

#[tool_router]
impl FfxiServer {
    fn new(
        cmd_tx: mpsc::Sender<AgentCommand>,
        state: Arc<RwLock<SessionState>>,
        goal_store: GoalStore,
    ) -> Self {
        Self {
            cmd_tx,
            state,
            goal_store: Arc::new(Mutex::new(goal_store)),
        }
    }

    async fn send(&self, cmd: AgentCommand) -> Result<CallToolResult, McpError> {
        match self.cmd_tx.send(cmd).await {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text("ok")])),
            Err(_) => Err(McpError::internal_error(
                "session command channel closed",
                None,
            )),
        }
    }

    #[tool(
        description = "Reactor goal: step toward `target_id`, holding `distance` yalms once close. Persists across reconnects."
    )]
    async fn follow(
        &self,
        Parameters(p): Parameters<FollowParams>,
    ) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::Follow {
            target_id: p.target_id,
            distance: p.distance,
        })
        .await
    }

    #[tool(
        description = "Reactor goal: face `target_id` and engage auto-attack. Single Attack action on transition; subsequent ticks just keep facing."
    )]
    async fn engage(
        &self,
        Parameters(p): Parameters<EngageParams>,
    ) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::Engage {
            target_id: p.target_id,
        })
        .await
    }

    #[tool(
        description = "Reactor goal: walk in a straight line to (x,y,z). Server-validated; out-of-bounds steps are rejected."
    )]
    async fn path_to(
        &self,
        Parameters(p): Parameters<PathToParams>,
    ) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::PathTo {
            x: p.x,
            y: p.y,
            z: p.z,
        })
        .await
    }

    #[tool(
        description = "Clear any active reactor goal and return to Idle. Clears the persisted goal too."
    )]
    async fn cancel(&self) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::Cancel).await
    }

    #[tool(
        description = "Send a chat-channel message. `kind`: 0=say, 1=shout, 4=party, 5=linkshell."
    )]
    async fn chat(
        &self,
        Parameters(p): Parameters<ChatParams>,
    ) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::Chat {
            kind: p.kind,
            text: p.text,
        })
        .await
    }

    #[tool(
        description = "Request a zoneline transition. The character must already be standing in the zoneline rect."
    )]
    async fn request_zone_change(
        &self,
        Parameters(p): Parameters<ZoneChangeParams>,
    ) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::RequestZoneChange { line_id: p.line_id })
            .await
    }

    #[tool(
        description = "Auto-end any in-progress event/cutscene. Cheap; safe to call when no event is active."
    )]
    async fn end_event(&self) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::EndEvent).await
    }

    #[tool(
        description = "Echo current session diagnostics + a SceneSummary event. Triggers re-fetch of scene://current."
    )]
    async fn snapshot(&self) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::Snapshot).await
    }

    #[tool(description = "Disconnect cleanly. The supervisor will not reconnect.")]
    async fn disconnect(&self) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::Disconnect).await
    }
}

#[tool_handler]
impl ServerHandler for FfxiServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::V_2025_03_26;
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        info.instructions = Some(
            "FFXI agent harness. Use `set_goal`-style tools (follow, engage, path_to) \
             not raw moves — the reactor handles per-tick motion. Read `scene://current` \
             for a compact prose summary; pull `entities://nearby` only when planning."
                .into(),
        );
        info
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let mk = |uri: &str, name: &str, mime: &str, desc: &str| -> Resource {
            let raw = RawResource::new(uri, name)
                .with_description(desc)
                .with_mime_type(mime);
            Annotated { raw, annotations: None }
        };
        let mut result = ListResourcesResult::default();
        result.resources = vec![
            mk(
                "scene://current",
                "scene",
                "text/plain",
                "Compact prose summary of the live session — zone, HP/MP, nearby entities, last chat.",
            ),
            mk(
                "party://members",
                "party",
                "application/json",
                "Party roster: id, name, hp/mp/tp, job, leader flags. Includes self.",
            ),
            mk(
                "diagnostics://session",
                "diagnostics",
                "application/json",
                "Stage, blowfish status, sync_in/sync_out, server packet age, map server addr.",
            ),
            mk(
                "goal://current",
                "goal",
                "application/json",
                "Last goal command (Follow/Engage/PathTo) — persisted to ~/.config/ffxi-mcp/goal.json.",
            ),
        ];
        Ok(result)
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = request.uri.as_str();
        let state = self.state.read().await;
        let contents = match uri {
            "scene://current" => {
                let summary = SceneSummary::from_state(&state);
                ResourceContents::text(summary.text, uri)
            }
            "party://members" => {
                let body = serde_json::to_string_pretty(&state.party)
                    .map_err(|e| McpError::internal_error(format!("serialize party: {e}"), None))?;
                ResourceContents::text(body, uri)
            }
            "diagnostics://session" => {
                let body = serde_json::to_string_pretty(&state.diagnostics).map_err(|e| {
                    McpError::internal_error(format!("serialize diagnostics: {e}"), None)
                })?;
                ResourceContents::text(body, uri)
            }
            "goal://current" => {
                drop(state);
                let store = self.goal_store.lock().await;
                let body = match store.load() {
                    Ok(g) => serde_json::to_string_pretty(&g).map_err(|e| {
                        McpError::internal_error(format!("serialize goal: {e}"), None)
                    })?,
                    Err(e) => {
                        return Err(McpError::internal_error(
                            format!("read goal store: {e}"),
                            None,
                        ));
                    }
                };
                ResourceContents::text(body, uri)
            }
            other => {
                return Err(McpError::resource_not_found(
                    format!("unknown resource: {other}"),
                    None,
                ));
            }
        };
        Ok(ReadResourceResult::new(vec![contents]))
    }
}

/// Background task: consume the event broadcast and fold each event
/// into a shared `SessionState` mirror that resources read from.
async fn run_state_mirror(
    state: Arc<RwLock<SessionState>>,
    mut event_rx: broadcast::Receiver<AgentEvent>,
) {
    loop {
        match event_rx.recv().await {
            Ok(ev) => {
                let mut s = state.write().await;
                s.apply_event(&ev);
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "state mirror lagged");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

fn read_env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("env var {name} required"))
}

#[tokio::main]
async fn main() -> Result<()> {
    // Logs to stderr — stdout is the MCP transport.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cfg = session::Config {
        server: read_env("FFXI_SERVER").unwrap_or_else(|_| "127.0.0.1".into()),
        map_host_override: std::env::var("FFXI_MAP_HOST_OVERRIDE").ok(),
        auth_port: parse_port("FFXI_AUTH_PORT", 54231)?,
        data_port: parse_port("FFXI_DATA_PORT", 54230)?,
        view_port: parse_port("FFXI_VIEW_PORT", 54001)?,
        user: read_env("FFXI_USER")?,
        password: read_env("FFXI_PASS")?,
        char_id: read_env("FFXI_CHAR_ID")?
            .parse()
            .context("FFXI_CHAR_ID must be a u32")?,
        char_name: read_env("FFXI_CHAR")?,
    };

    let goal_path = match std::env::var("FFXI_MCP_GOAL_PATH") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => GoalStore::default_path()
            .context("resolve goal_store path; set FFXI_MCP_GOAL_PATH to override")?,
    };
    let goal_store = GoalStore::new(goal_path);

    let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>(64);
    let (event_tx, event_rx) = broadcast::channel::<AgentEvent>(256);
    let state = Arc::new(RwLock::new(SessionState::default()));

    // Spawn the supervisor (which spawns reactor → session inside).
    let sup_cfg = SupervisorConfig {
        goal_store: Some(goal_store.clone()),
        ..SupervisorConfig::default()
    };
    let reactor_cfg = ReactorConfig::default();
    let event_tx_for_sup = event_tx.clone();
    let supervisor_handle = tokio::spawn(async move {
        if let Err(e) =
            supervisor::run(cfg, cmd_rx, event_tx_for_sup, sup_cfg, reactor_cfg).await
        {
            tracing::error!(error = %e, "supervisor exited with error");
        }
    });

    // Mirror task — folds events into `state` for resource reads.
    let mirror_handle = tokio::spawn(run_state_mirror(state.clone(), event_rx));

    // Build the MCP server and serve over stdio.
    let server = FfxiServer::new(cmd_tx, state, goal_store);
    let running = serve_server(server, stdio())
        .await
        .context("serve MCP server")?;
    running.waiting().await.context("MCP server crashed")?;

    // Once the MCP transport closes, shut down background tasks.
    supervisor_handle.abort();
    mirror_handle.abort();
    Ok(())
}

fn parse_port(name: &str, default: u16) -> Result<u16> {
    match std::env::var(name) {
        Ok(s) => s.parse().with_context(|| format!("{name} must be a u16")),
        Err(_) => Ok(default),
    }
}
