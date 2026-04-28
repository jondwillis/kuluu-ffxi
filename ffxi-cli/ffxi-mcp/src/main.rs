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
        ResourceContents, ResourceUpdatedNotificationParam, ResourcesCapability,
        ServerCapabilities, ServerInfo, SubscribeRequestParams, ToolsCapability,
        UnsubscribeRequestParams,
    },
    service::{Peer, RequestContext, RoleServer, serve_server},
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
    state::{ActionKind, AgentCommand, AgentEvent, SessionState},
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
    /// 0=say, 1=shout, 4=party, 5=linkshell. **`/tell` does not go here**
    /// — use the dedicated `tell` tool instead (different opcode).
    /// Server-side say messages beginning with `@` dispatch as GM
    /// commands when the account has `gmlevel ≥ 1`.
    kind: u8,
    text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TellParams {
    /// Recipient character name. Resolved cross-zone by the server.
    to: String,
    text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ZoneChangeParams {
    /// `RectID` of the zoneline. The agent must walk into ~40 yalms of
    /// the zoneline first; this command is "I'm here, take me through".
    line_id: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CastParams {
    /// FFXI spell id (Spells.dat index). E.g. Cure = 1, Tractor = 39.
    spell_id: u32,
    /// `UniqueNo` of the recipient (self for self-target, mob for
    /// offensive, party member for healing).
    target_id: u32,
    /// Per-zone `ActIndex` of the recipient.
    target_index: u16,
    /// Ground-target world coords for AoE-target spells (Tractor,
    /// certain blue magic). `None` → 0.0 — single-target casts ignore
    /// these fields server-side.
    pos_x: Option<f32>,
    pos_y: Option<f32>,
    pos_z: Option<f32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WeaponskillParams {
    /// FFXI weaponskill id. Must be unlocked on the active weapon.
    skill_id: u32,
    target_id: u32,
    target_index: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct JobAbilityParams {
    /// FFXI job-ability id (e.g. WAR Mighty Strikes, RDM Convert).
    ability_id: u32,
    target_id: u32,
    target_index: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BankWhenFullParams {
    /// Slot count at which any field bag (Inventory / Mog Satchel /
    /// Mog Sack / Mog Case) triggers a zone change. 30 is a safe
    /// "almost full" threshold for an 80-slot Inventory; 60 lets
    /// the agent fight longer before banking.
    threshold: u8,
    /// `RectID` of the mog-house zoneline from the agent's current
    /// home city. Typical values vary per city — caller picks the
    /// right one for where the agent is farming.
    mog_house_zoneline: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct UseItemParams {
    /// Container id (`CONTAINER_ID`): 0=Inventory, 1=Safe, 2=Storage,
    /// 8=Wardrobe, 10..16=Wardrobe2..Wardrobe8.
    container: u8,
    /// Slot index inside the container (`PropertyItemIndex`).
    slot: u8,
    /// FFXI item id. Hint for the LLM's bookkeeping; the wire `ItemNum`
    /// is server-forced to 0 and the real lookup is `(container, slot)`.
    /// Stage 9's inventory mirror will let agents read this from
    /// `SessionState.inventory`; until then, pass it inline.
    item_no: u32,
    /// `UniqueNo` of the recipient (self for potions / scrolls, mob for
    /// ranged items like Soultrapper).
    target_id: u32,
    /// Per-zone `ActIndex` of the recipient.
    target_index: u16,
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
        // The dispatch latency the LLM observes is just the channel push —
        // tools return "ok" as soon as the command lands in cmd_tx, before
        // the session actually executes it. Recording elapsed_us here gives
        // the operator the exact number to compare against the plan's
        // `MCP dispatch ≤50 ms p99` budget. Event (not span) because
        // EnteredSpan is `!Send` and we await on cmd_tx.
        let kind = cmd_kind_label(&cmd);
        let started = std::time::Instant::now();
        let result = match self.cmd_tx.send(cmd).await {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text("ok")])),
            Err(_) => Err(McpError::internal_error(
                "session command channel closed",
                None,
            )),
        };
        tracing::debug!(
            kind,
            elapsed_us = started.elapsed().as_micros() as u64,
            ok = result.is_ok(),
            "mcp.tool_dispatch"
        );
        result
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
        description = "Reactor goal: monitor inventory and zone to mog house when any field bag (Inventory / Mog Satchel / Mog Sack / Mog Case) reaches `threshold` slots filled. One-shot — clears after firing the zone change. Holds until `inventory://current` reports `all_loaded == true`. Persists across reconnects."
    )]
    async fn bank_when_full(
        &self,
        Parameters(p): Parameters<BankWhenFullParams>,
    ) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::BankWhenFull {
            threshold: p.threshold,
            mog_house_zoneline: p.mog_house_zoneline,
        })
        .await
    }

    #[tool(
        description = "Send a chat-channel message. `kind`: 0=say, 1=shout, 4=party, 5=linkshell. For /tell use the `tell` tool — it's a different opcode."
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
        description = "Send a /tell to another player by character name. Server resolves the recipient cross-zone."
    )]
    async fn tell(
        &self,
        Parameters(p): Parameters<TellParams>,
    ) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::Tell {
            to: p.to,
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

    #[tool(
        description = "Cast a spell (FFXI Spells.dat id). For self-target casts pass your own UniqueNo + ActIndex; for ground-target spells (Tractor, certain blue magic) supply pos_x/y/z. 0x01A action wire."
    )]
    async fn cast(
        &self,
        Parameters(p): Parameters<CastParams>,
    ) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::Action {
            target_id: p.target_id,
            target_index: p.target_index,
            kind: ActionKind::CastMagic {
                spell_id: p.spell_id,
                pos_x: p.pos_x.unwrap_or(0.0),
                pos_y: p.pos_y.unwrap_or(0.0),
                pos_z: p.pos_z.unwrap_or(0.0),
            },
        })
        .await
    }

    #[tool(
        description = "Use an unlocked weaponskill on a target. Requires sufficient TP (server-validated). 0x01A action wire."
    )]
    async fn weaponskill(
        &self,
        Parameters(p): Parameters<WeaponskillParams>,
    ) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::Action {
            target_id: p.target_id,
            target_index: p.target_index,
            kind: ActionKind::Weaponskill {
                skill_id: p.skill_id,
            },
        })
        .await
    }

    #[tool(
        description = "Use a job ability (e.g. WAR Mighty Strikes, RDM Convert). Server-validated for cooldown / job. 0x01A action wire."
    )]
    async fn job_ability(
        &self,
        Parameters(p): Parameters<JobAbilityParams>,
    ) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::Action {
            target_id: p.target_id,
            target_index: p.target_index,
            kind: ActionKind::JobAbility {
                ability_id: p.ability_id,
            },
        })
        .await
    }

    #[tool(
        description = "Use a consumable / scroll / charged item. (container, slot) identifies the item; target is self for potions or another entity for ranged items (Soultrapper, etc.). 0x037 wire."
    )]
    async fn use_item(
        &self,
        Parameters(p): Parameters<UseItemParams>,
    ) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::UseItem {
            container: p.container,
            slot: p.slot,
            item_no: p.item_no,
            target_id: p.target_id,
            target_index: p.target_index,
        })
        .await
    }

    #[tool(description = "Disconnect cleanly. The supervisor will not reconnect.")]
    async fn disconnect(&self) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::Disconnect).await
    }
}

#[tool_handler]
impl ServerHandler for FfxiServer {
    fn get_info(&self) -> ServerInfo {
        // Advertise resources/subscribe so MCP clients can call
        // resources/subscribe and receive ResourceUpdated notifications
        // when the underlying state changes (low_hp, party update, etc.).
        let mut caps = ServerCapabilities::default();
        caps.tools = Some(ToolsCapability { list_changed: None });
        caps.resources = Some(ResourcesCapability {
            subscribe: Some(true),
            list_changed: None,
        });

        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::V_2025_03_26;
        info.capabilities = caps;
        info.instructions = Some(
            "FFXI agent harness. Use `set_goal`-style tools (follow, engage, path_to) \
             not raw moves — the reactor handles per-tick motion. Read `scene://current` \
             for a compact prose summary; subscribe to scene://current for wake-on-event."
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
            mk(
                "inventory://current",
                "inventory",
                "application/json",
                "Container-keyed slot map: capacity + populated slots (item_no, quantity, locked, price). `all_loaded` flips true after the initial zone-in flood completes.",
            ),
        ];
        Ok(result)
    }

    /// Honour the `resources.subscribe = true` capability. The notifier task
    /// fans out `notifications/resources/updated` to every connected peer
    /// regardless of per-URI subscription state, so this handler doesn't need
    /// to maintain a subscription set — it just has to ack the call so the
    /// capability advertisement isn't a lie.
    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        tracing::debug!(uri = %request.uri, "client subscribed to resource");
        Ok(())
    }

    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        tracing::debug!(uri = %request.uri, "client unsubscribed from resource");
        Ok(())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = request.uri.as_str();
        // Event (not span) because EnteredSpan is `!Send` and we await on
        // self.state.read().await below; the elapsed_us field on the trailing
        // event captures the same latency information.
        let started = std::time::Instant::now();
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
            "inventory://current" => {
                let body = serde_json::to_string_pretty(&state.inventory).map_err(|e| {
                    McpError::internal_error(format!("serialize inventory: {e}"), None)
                })?;
                ResourceContents::text(body, uri)
            }
            "goal://current" => {
                drop(state);
                let store = self.goal_store.lock().await;
                let body = match store.load() {
                    // Active goal: return the persisted command + timestamp.
                    Ok(Some(g)) => serde_json::to_string_pretty(&serde_json::json!({
                        "goal": "active",
                        "command": g.command,
                        "set_at_unix": g.set_at_unix,
                    }))
                    .map_err(|e| {
                        McpError::internal_error(format!("serialize goal: {e}"), None)
                    })?,
                    // No active goal — explicit "idle" sentinel beats raw `null`.
                    // Indistinguishable on disk between "never set" and "canceled";
                    // both render the same to the LLM, which is the intended UX.
                    Ok(None) => "{\n  \"goal\": \"idle\"\n}".to_string(),
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
        tracing::debug!(
            uri,
            elapsed_us = started.elapsed().as_micros() as u64,
            "mcp.resource_read"
        );
        Ok(ReadResourceResult::new(vec![contents]))
    }
}

/// Stable, low-cardinality label per `AgentCommand` variant for use as a
/// span/log field. Mirrors the MCP tool names so log analysis can join
/// dispatch latency back to the tool the LLM called.
fn cmd_kind_label(cmd: &AgentCommand) -> &'static str {
    use ffxi_client::state::AgentCommand::*;
    match cmd {
        Move { .. } => "move",
        StopMove => "stop_move",
        Chat { .. } => "chat",
        Tell { .. } => "tell",
        Action { .. } => "action",
        EndEvent => "end_event",
        Snapshot => "snapshot",
        Disconnect => "disconnect",
        Follow { .. } => "follow",
        Engage { .. } => "engage",
        PathTo { .. } => "path_to",
        Cancel => "cancel",
        RequestZoneChange { .. } => "request_zone_change",
        UseItem { .. } => "use_item",
        BankWhenFull { .. } => "bank_when_full",
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

/// Background task: subscribe to the event broadcast and emit
/// `notifications/resources/updated` to the MCP client for events that
/// change resource content. The client can then re-read the affected
/// resource (e.g. `scene://current`) on its own schedule.
///
/// We *don't* notify on every position change or every chat line —
/// those would defeat the point of subscription. Filtered to events
/// the LLM should actually wake on.
async fn run_notifier(
    peer: Peer<RoleServer>,
    mut event_rx: broadcast::Receiver<AgentEvent>,
) {
    loop {
        match event_rx.recv().await {
            Ok(ev) => {
                for uri in uris_for_event(&ev) {
                    let params = ResourceUpdatedNotificationParam {
                        uri: (*uri).into(),
                    };
                    if let Err(e) = peer.notify_resource_updated(params).await {
                        tracing::warn!(error = %e, uri = uri, "notify_resource_updated failed");
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "notifier lagged");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Map an `AgentEvent` to the set of resource URIs whose content the
/// event invalidated. Empty slice = don't notify.
fn uris_for_event(ev: &AgentEvent) -> &'static [&'static str] {
    match ev {
        // High-signal events that the LLM strategy layer cares about —
        // re-read scene to get the latest prose with the updated context.
        AgentEvent::LowHp { .. }
        | AgentEvent::PartyMemberLowHp { .. }
        | AgentEvent::EngagedBy { .. }
        | AgentEvent::TellReceived { .. }
        | AgentEvent::Reconnected { .. }
        | AgentEvent::SceneSummary { .. } => &["scene://current"],

        // Zone changes invalidate scene + party + diagnostics.
        AgentEvent::ZoneChanged { .. } => {
            &["scene://current", "party://members", "diagnostics://session"]
        }

        // Party-roster changes touch party and (incidentally) scene's
        // party-size summary.
        AgentEvent::PartyMemberUpdated { .. } => &["party://members", "scene://current"],

        // Inventory: per-slot churn invalidates only the inventory
        // resource (tactical, doesn't wake the LLM via scene). The
        // initial "all loaded" signal is high-signal — that's when
        // bank_when_full can start trusting slot counts.
        AgentEvent::InventoryUpdated { .. } => &["inventory://current"],
        AgentEvent::InventoryReady => &["inventory://current", "scene://current"],

        // Stage / diagnostics updates are diagnostics only.
        AgentEvent::StageChanged { .. } | AgentEvent::Diagnostics { .. } => {
            &["diagnostics://session"]
        }

        // Disconnected blanks the scene; resource list itself is unchanged.
        AgentEvent::Disconnected { .. } => &["scene://current", "diagnostics://session"],

        // Position / entity / chat / event flow signals are tactical
        // detail. They DO change `scene://current` cosmetically (last
        // chat line) but the LLM doesn't need to wake for those — that
        // would defeat the wake-on-signal contract.
        _ => &[],
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
        initial_state: None,
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

    // Subscribe BEFORE serve_server so we don't miss early events emitted
    // while the MCP handshake is still in progress.
    let notifier_event_rx = event_tx.subscribe();

    // Build the MCP server and serve over stdio.
    let server = FfxiServer::new(cmd_tx, state, goal_store);
    let running = serve_server(server, stdio())
        .await
        .context("serve MCP server")?;

    // Now that the server is running we can grab its peer and start
    // forwarding broadcast events as ResourceUpdated notifications.
    let peer = running.peer().clone();
    let notifier_handle = tokio::spawn(run_notifier(peer, notifier_event_rx));

    running.waiting().await.context("MCP server crashed")?;

    // Once the MCP transport closes, shut down background tasks.
    supervisor_handle.abort();
    mirror_handle.abort();
    notifier_handle.abort();
    Ok(())
}

fn parse_port(name: &str, default: u16) -> Result<u16> {
    match std::env::var(name) {
        Ok(s) => s.parse().with_context(|| format!("{name} must be a u16")),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_client::state::{ActionKind, PartyMember, Position, Stage, Vec3};

    #[test]
    fn cmd_kind_label_for_action_surface_tools() {
        // The four new MCP tools all flow through existing AgentCommand
        // variants. `cast` / `weaponskill` / `job_ability` are sugar for
        // `AgentCommand::Action { kind: … }` → label "action"; `use_item`
        // is its own top-level variant → label "use_item".
        let cast = AgentCommand::Action {
            target_id: 1,
            target_index: 1,
            kind: ActionKind::CastMagic {
                spell_id: 1,
                pos_x: 0.0,
                pos_y: 0.0,
                pos_z: 0.0,
            },
        };
        assert_eq!(cmd_kind_label(&cast), "action");

        let ws = AgentCommand::Action {
            target_id: 1,
            target_index: 1,
            kind: ActionKind::Weaponskill { skill_id: 100 },
        };
        assert_eq!(cmd_kind_label(&ws), "action");

        let ja = AgentCommand::Action {
            target_id: 1,
            target_index: 1,
            kind: ActionKind::JobAbility { ability_id: 50 },
        };
        assert_eq!(cmd_kind_label(&ja), "action");

        let item = AgentCommand::UseItem {
            container: 0,
            slot: 3,
            item_no: 4112,
            target_id: 42,
            target_index: 7,
        };
        assert_eq!(cmd_kind_label(&item), "use_item");
    }

    #[test]
    fn high_signal_events_invalidate_scene() {
        let cases: Vec<AgentEvent> = vec![
            AgentEvent::LowHp { pct: 20 },
            AgentEvent::PartyMemberLowHp { id: 1, pct: 15 },
            AgentEvent::EngagedBy { entity_id: 99 },
            AgentEvent::TellReceived {
                from: "X".into(),
                text: "hi".into(),
            },
            AgentEvent::Reconnected { downtime_ms: 1000 },
            AgentEvent::SceneSummary { text: "x".into() },
        ];
        for ev in cases {
            assert_eq!(uris_for_event(&ev), &["scene://current"], "ev: {ev:?}");
        }
    }

    #[test]
    fn zone_change_invalidates_scene_party_diagnostics() {
        let ev = AgentEvent::ZoneChanged {
            from: Some(100),
            to: 230,
        };
        assert_eq!(
            uris_for_event(&ev),
            &["scene://current", "party://members", "diagnostics://session"]
        );
    }

    #[test]
    fn party_update_invalidates_party_and_scene() {
        let ev = AgentEvent::PartyMemberUpdated {
            member: PartyMember {
                id: 1,
                act_index: 1,
                name: None,
                hp: 100,
                mp: 100,
                tp: 0,
                hp_pct: 100,
                mp_pct: 100,
                zone_no: 230,
                main_job: 1,
                main_job_lv: 75,
                sub_job: 6,
                sub_job_lv: 37,
                is_party_leader: false,
                is_alliance_leader: false,
            },
        };
        assert_eq!(uris_for_event(&ev), &["party://members", "scene://current"]);
    }

    #[test]
    fn stage_and_diagnostics_only_invalidate_diagnostics() {
        assert_eq!(
            uris_for_event(&AgentEvent::StageChanged { stage: Stage::InZone }),
            &["diagnostics://session"]
        );
        assert_eq!(
            uris_for_event(&AgentEvent::Diagnostics {
                diagnostics: Default::default()
            }),
            &["diagnostics://session"]
        );
    }

    #[test]
    fn inventory_events_invalidate_inventory_resource() {
        // Per-slot churn → inventory only (tactical, not high-signal).
        let upd = AgentEvent::InventoryUpdated {
            container: 0,
            update: ffxi_client::state::InventoryUpdate::SlotChanged {
                slot: ffxi_client::state::ItemSlot {
                    index: 7,
                    item_no: 4112,
                    quantity: 1,
                    locked: false,
                    price: 0,
                },
            },
        };
        assert_eq!(uris_for_event(&upd), &["inventory://current"]);

        // InventoryReady (initial-flood-done) is high-signal; also wakes
        // scene because it changes the agent's banking-eligibility.
        assert_eq!(
            uris_for_event(&AgentEvent::InventoryReady),
            &["inventory://current", "scene://current"]
        );
    }

    #[test]
    fn tactical_events_do_not_notify() {
        // The wake-on-signal contract: tactical detail mustn't notify.
        let cases: Vec<AgentEvent> = vec![
            AgentEvent::PositionChanged {
                pos: Position {
                    pos: Vec3::default(),
                    heading: 0,
                },
            },
            AgentEvent::EntityRemoved { id: 99 },
            AgentEvent::EventStart { event_id: 1 },
            AgentEvent::EventEnded,
            AgentEvent::KeyRotated {
                previous_status: ffxi_client::state::BlowfishStatus::Accepted,
            },
            AgentEvent::Error { message: "x".into() },
        ];
        for ev in cases {
            assert!(
                uris_for_event(&ev).is_empty(),
                "should not notify for {ev:?}"
            );
        }
    }
}
