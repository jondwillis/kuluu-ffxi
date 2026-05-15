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

mod attach;

use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{
        Annotated, CallToolResult, Content, ListResourcesResult, PaginatedRequestParams,
        ProtocolVersion, RawResource, ReadResourceRequestParams, ReadResourceResult, Resource,
        ResourceContents, ResourceUpdatedNotificationParam, ResourcesCapability,
        ServerCapabilities, ServerInfo, SubscribeRequestParams, ToolsCapability,
        UnsubscribeRequestParams,
    },
    service::{serve_server, Peer, RequestContext, RoleServer},
    tool, tool_handler, tool_router,
    transport::io::stdio,
    ErrorData as McpError, ServerHandler,
};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

use ffxi_client::{
    goal_store::GoalStore,
    reactor::ReactorConfig,
    relay,
    scene::SceneSummary,
    session,
    state::{
        process_monotonic_ms, ActionKind, AgentCommand, AgentEvent, LlmDecision, LlmDecisionKind,
        SessionState,
    },
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

#[derive(Debug, Deserialize, JsonSchema)]
struct RaiseMenuParams {
    /// `true` accepts the raise; `false` declines (return to homepoint).
    accept: bool,
    /// Your own `UniqueNo` (from `diagnostics://session.char_id` or
    /// `scene://entities.self.char_id`).
    target_id: u32,
    /// Your own per-zone `ActIndex`.
    target_index: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TractorMenuParams {
    /// `true` accepts the tractor; `false` declines.
    accept: bool,
    target_id: u32,
    target_index: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct HomepointMenuParams {
    /// 0=Accept (warp), 1=MonstrosityCancel, 2=MonstrosityRetry. For
    /// the post-KO homepoint warp, pass 0.
    status_id: u32,
    target_id: u32,
    target_index: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ReadResourceParams {
    /// Resource URI to read (e.g. `scene://current`, `party://members`).
    uri: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WaitForEventParams {
    /// Event kinds to wait for. Snake-case names matching `AgentEvent`'s
    /// serde tag: `low_hp`, `engaged_by`, `tell_received`,
    /// `party_member_low_hp`, `scene_summary`, `reconnected`,
    /// `zone_changed`, `inventory_ready`, `disconnected`. An empty list
    /// matches every high-signal event.
    #[serde(default)]
    kinds: Vec<String>,
    /// Maximum time to wait, in milliseconds. Capped at 60_000 server-side
    /// to keep MCP transport timeouts honest. On timeout the tool returns
    /// `{"matched": false, "waited_ms": <elapsed>}` rather than erroring.
    timeout_ms: u64,
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
    /// Dashboard observability sink: when wired (combined-binary mode),
    /// every tool dispatch emits `LlmDecision::ToolDispatched` so the
    /// view's badge can pulse and the latency sparkline can plot. None
    /// in headless harness mode — the harness reads its own latency from
    /// MCP transport instead.
    decision_tx: Option<broadcast::Sender<AgentEvent>>,
    /// Broadcast sender used by `wait_for_event` to spawn fresh
    /// subscribers per call. Always wired in `main` — kept Optional so
    /// unit tests can construct a server without an event channel.
    event_tx: Option<broadcast::Sender<AgentEvent>>,
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
            decision_tx: None,
            event_tx: None,
        }
    }

    /// Builder: wire a broadcast sender so `LlmDecision` events flow into
    /// the same stream the view consumes. Combined-binary mode only.
    fn with_decision_tx(mut self, tx: broadcast::Sender<AgentEvent>) -> Self {
        self.decision_tx = Some(tx);
        self
    }

    /// Builder: wire the same broadcast sender for `wait_for_event` to
    /// subscribe to. Distinct field from `decision_tx` so the test
    /// surface that exercises one doesn't have to wire the other.
    fn with_event_tx(mut self, tx: broadcast::Sender<AgentEvent>) -> Self {
        self.event_tx = Some(tx);
        self
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
        let elapsed_us = started.elapsed().as_micros() as u64;
        tracing::debug!(kind, elapsed_us, ok = result.is_ok(), "mcp.tool_dispatch");
        // Emit a ToolDispatched decision regardless of dispatch outcome —
        // a closed channel still occupied the LLM's attention. Lossy by
        // design (try_send): if the broadcast is full or has no live
        // receivers we drop silently rather than block the tool path.
        if let Some(tx) = &self.decision_tx {
            let _ = tx.send(AgentEvent::LlmDecision {
                decision: LlmDecision {
                    kind: LlmDecisionKind::ToolDispatched {
                        tool: kind.to_string(),
                    },
                    latency_us: elapsed_us,
                    at_monotonic_ms: process_monotonic_ms(),
                },
            });
        }
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

    #[tool(
        description = "Respond to a /raise menu (after KO). `accept: true` raises in place; `false` returns to homepoint. Self-targeted: pass your own UniqueNo + ActIndex. 0x01A action wire (ActionID 0x0D)."
    )]
    async fn raise_menu(
        &self,
        Parameters(p): Parameters<RaiseMenuParams>,
    ) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::Action {
            target_id: p.target_id,
            target_index: p.target_index,
            kind: ActionKind::RaiseMenu { accept: p.accept },
        })
        .await
    }

    #[tool(
        description = "Respond to a /tractor menu. `accept: true` warps to the caster's location; `false` declines. Self-targeted. 0x01A action wire (ActionID 0x13)."
    )]
    async fn tractor_menu(
        &self,
        Parameters(p): Parameters<TractorMenuParams>,
    ) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::Action {
            target_id: p.target_id,
            target_index: p.target_index,
            kind: ActionKind::TractorMenu { accept: p.accept },
        })
        .await
    }

    #[tool(
        description = "Respond to the homepoint warp menu (post-KO when you decline a raise, or any homepoint NPC). status_id: 0=Accept warp, 1=MonstrosityCancel, 2=MonstrosityRetry. Self-targeted. 0x01A action wire (ActionID 0x0B)."
    )]
    async fn homepoint_menu(
        &self,
        Parameters(p): Parameters<HomepointMenuParams>,
    ) -> Result<CallToolResult, McpError> {
        self.send(AgentCommand::Action {
            target_id: p.target_id,
            target_index: p.target_index,
            kind: ActionKind::HomepointMenu {
                status_id: p.status_id,
            },
        })
        .await
    }

    #[tool(
        description = "Block until one of the named events fires (or `timeout_ms` elapses). Use this instead of polling scene://current — it collapses 3-5 idle wakes into one. Returns JSON: `{matched: true, kind, payload, waited_ms}` on event, `{matched: false, waited_ms}` on timeout. Latency contract: wakes within ~1 reactor tick (~200ms) of the event."
    )]
    async fn wait_for_event(
        &self,
        Parameters(p): Parameters<WaitForEventParams>,
    ) -> Result<CallToolResult, McpError> {
        let Some(tx) = self.event_tx.as_ref() else {
            return Err(McpError::internal_error(
                "wait_for_event: event broadcaster not wired",
                None,
            ));
        };
        let timeout = std::time::Duration::from_millis(p.timeout_ms.min(60_000));
        let mut rx = tx.subscribe();
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(timeout, async {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let kind = event_kind_label(&ev);
                        if p.kinds.is_empty() || p.kinds.iter().any(|k| k == kind) {
                            return Ok::<_, ()>(Some((kind, ev)));
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return Ok(None),
                }
            }
        })
        .await;
        let waited_ms = started.elapsed().as_millis() as u64;
        let body = match result {
            Ok(Ok(Some((kind, ev)))) => serde_json::json!({
                "matched": true,
                "kind": kind,
                "payload": ev,
                "waited_ms": waited_ms,
            }),
            Ok(Ok(None)) | Ok(Err(_)) => serde_json::json!({
                "matched": false,
                "reason": "channel_closed",
                "waited_ms": waited_ms,
            }),
            Err(_) => serde_json::json!({
                "matched": false,
                "reason": "timeout",
                "waited_ms": waited_ms,
            }),
        };
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    /// Fallback for MCP clients that do not support the `resources`
    /// capability. Mirrors `resources/read` — returns the same content
    /// as a tool result so clients without resource support can still
    /// read `scene://current`, `party://members`, etc.
    #[tool(
        description = "Fallback for clients that do not support MCP `resources/read`. Read a resource by URI: `scene://current`, `party://members`, `diagnostics://session`, `goal://current`, `inventory://current`."
    )]
    async fn read_resource(
        &self,
        Parameters(p): Parameters<ReadResourceParams>,
    ) -> Result<CallToolResult, McpError> {
        let uri = p.uri.as_str();
        let started = std::time::Instant::now();
        let state = self.state.read().await;
        let content = read_resource(&state, &*self.goal_store, uri)
            .await
            .map_err(|e| McpError::internal_error(e, None))?;
        let elapsed_us = started.elapsed().as_micros() as u64;
        tracing::debug!(uri, elapsed_us, "mcp.tool_read_resource");
        Ok(CallToolResult::success(vec![Content::text(content)]))
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
        info.protocol_version = ProtocolVersion::V_2025_11_25;
        info.capabilities = caps;
        info.instructions = Some(
            "FFXI agent harness. Use tools (follow, engage, path_to, …) for \
              actions. Read resources for state: `scene://current` (compact prose \
              summary), `party://members`, `diagnostics://session`, `goal://current`, \
              `inventory://current`. Clients that do not support the MCP `resources` \
              capability should use the `read_resource { uri }` tool instead — it \
              returns identical content."
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
            Annotated {
                raw,
                annotations: None,
            }
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
            mk(
                "scene://entities",
                "entities",
                "application/json",
                "Structured nearest-N entities (id, act_index, kind, name, distance, hp_pct, claimed_by, pos) plus self pos+heading+zone. Use this when scene://current's prose lacks the IDs/coords you need to engage/follow/path_to.",
            ),
            mk(
                "debug://name_misses",
                "name_misses",
                "application/json",
                "Diagnostic ring buffer (cap 64) of CHAR_PC/CHAR_NPC packets where `PosHead::try_extract_name` returned None. Each entry has opcode, unique_no, act_index, the SendFlg byte (body[6]), body length, hex dump of the leading 96 bytes, and a classification of the failure reason. Use this to audit why specific entities show as `?` in scene://entities.",
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
        let started = std::time::Instant::now();
        let state = self.state.read().await;
        let body = read_resource(&state, &*self.goal_store, uri)
            .await
            .map_err(|e| McpError::internal_error(e, None))?;
        let elapsed_us = started.elapsed().as_micros() as u64;
        tracing::debug!(uri, elapsed_us, "mcp.resource_read");
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            body, uri,
        )]))
    }
}

/// Shared helper: read a resource by URI from the current state.
/// Used by both `ServerHandler::read_resource` (MCP protocol) and
/// the `read_resource` tool (fallback for clients without resource
/// support).
async fn read_resource(
    state: &SessionState,
    goal_store: &tokio::sync::Mutex<GoalStore>,
    uri: &str,
) -> Result<String, String> {
    match uri {
        "scene://current" => {
            let summary = SceneSummary::from_state(state);
            Ok(summary.text)
        }
        "party://members" => {
            serde_json::to_string_pretty(&state.party).map_err(|e| format!("serialize party: {e}"))
        }
        "diagnostics://session" => serde_json::to_string_pretty(&state.diagnostics)
            .map_err(|e| format!("serialize diagnostics: {e}")),
        "inventory://current" => serde_json::to_string_pretty(&state.inventory)
            .map_err(|e| format!("serialize inventory: {e}")),
        "scene://entities" => serde_json::to_string_pretty(&entities_view(state))
            .map_err(|e| format!("serialize entities: {e}")),
        "debug://name_misses" => serde_json::to_string_pretty(&name_misses_view(state))
            .map_err(|e| format!("serialize name_misses: {e}")),
        "goal://current" => {
            // Prefer the in-memory live mirror over the disk-backed
            // `goal_store`. In attach mode no producer is writing
            // `goal_store` (the running native client has its own
            // session/reactor — no MCP-side supervisor). The disk file
            // can be stale by days from a previous headless session,
            // while `state.current_goal` is folded live from
            // `AgentEvent::ReactorGoalChanged` (see state.rs apply_event).
            //
            // Fall back to disk only when the live mirror is empty,
            // which is the headless cross-process-restart case the
            // disk file was actually designed for.
            use ffxi_client::state::ReactorGoalSnapshot;
            match &state.current_goal {
                Some(ReactorGoalSnapshot::Idle) | None => {
                    // Live mirror says idle (or has no signal yet). Fall
                    // back to disk for the headless-restart case.
                    let store = goal_store.lock().await;
                    match store.load() {
                        Ok(Some(g)) => serde_json::to_string_pretty(&serde_json::json!({
                            "goal": "active",
                            "command": g.command,
                            "set_at_unix": g.set_at_unix,
                            "source": "disk",
                        }))
                        .map_err(|e| format!("serialize goal: {e}")),
                        Ok(None) => Ok("{\n  \"goal\": \"idle\"\n}".to_string()),
                        Err(e) => Err(format!("read goal store: {e}")),
                    }
                }
                Some(snap) => serde_json::to_string_pretty(&serde_json::json!({
                    "goal": "active",
                    "snapshot": snap,
                    "source": "live",
                }))
                .map_err(|e| format!("serialize goal: {e}")),
            }
        }
        other => Err(format!("unknown resource: {other}")),
    }
}

/// Cap on entities returned by `scene://entities`. The LLM cares about
/// the nearest few targets; dumping a full 200-entity zone roster would
/// just burn tokens. 30 covers a tight farming pull plus the camp PCs.
const SCENE_ENTITIES_CAP: usize = 30;

/// Build the `scene://entities` payload: nearest-N entities by 2D
/// distance, plus self pos/heading/zone. Distance is xy-plane only —
/// matches `next_target_by_distance` so the agent's "what's nearest"
/// is consistent with the renderers' Tab cycle.
fn entities_view(state: &SessionState) -> serde_json::Value {
    // Self position is now derived from the entity list — `self_position()`
    // reads the entry whose `id == state.char_id`. Returns origin before
    // LOGIN seeds the self entity; the distances-from-origin in that
    // brief window are still valid for nearest-N sorting.
    let self_pos_p = state.self_position().unwrap_or_default();
    let from = self_pos_p.pos;
    // Exclude self from the nearby-entities list — the agent already
    // gets `self.pos` via the dedicated `self` field, and "nearest to me"
    // by definition doesn't include me. Pre-Stage-5 the self entity
    // simply wasn't in `state.entities` (it lived on `state.self_pos`),
    // so callers expect this view to be other-entities-only.
    let self_id = state.char_id;
    let mut scored: Vec<(&ffxi_client::state::Entity, f32)> = state
        .entities
        .iter()
        .filter(|e| Some(e.id) != self_id)
        .map(|e| {
            let dx = e.pos.x - from.x;
            let dy = e.pos.y - from.y;
            (e, (dx * dx + dy * dy).sqrt())
        })
        .collect();
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let entities: Vec<serde_json::Value> = scored
        .into_iter()
        .take(SCENE_ENTITIES_CAP)
        .map(|(e, dist)| {
            serde_json::json!({
                "id": e.id,
                "act_index": e.act_index,
                "kind": e.kind,
                "name": e.name,
                "distance": dist,
                "hp_pct": e.hp_pct,
                "claimed_by": if e.bt_target_id == 0 { None } else { Some(e.bt_target_id) },
                "pos": { "x": e.pos.x, "y": e.pos.y, "z": e.pos.z },
                "heading": e.heading,
            })
        })
        .collect();
    serde_json::json!({
        "self": {
            "pos": { "x": from.x, "y": from.y, "z": from.z },
            "heading": self_pos_p.heading,
            "zone_id": state.zone_id,
            "char_id": state.char_id,
        },
        "entities": entities,
        "total_known": state.entities.iter().filter(|e| Some(e.id) != self_id).count(),
        "cap": SCENE_ENTITIES_CAP,
    })
}

/// Build the `debug://name_misses` payload — surfaces the per-entity raw
/// packet samples captured when `PosHead::try_extract_name` returned
/// `None`. Used for forensic debugging of "?" entities without rebuilding
/// the client. Returns an empty `entries` array when the ring buffer
/// is empty.
///
/// Each entry carries enough context to audit the packet byte-by-byte:
/// the SendFlg byte (`body[6]`), the body length, a hex dump of the
/// leading 96 bytes (covers the CHAR_PC name slot at `body[0x5A]` and
/// the CHAR_NPC slot at `body[0x30]`), and a classification of *why*
/// the extraction failed.
fn name_misses_view(state: &SessionState) -> serde_json::Value {
    let entries: Vec<serde_json::Value> = state
        .name_misses
        .iter()
        .map(|m| {
            serde_json::json!({
                "opcode": format!("0x{:03x}", m.opcode),
                "unique_no": format!("0x{:08x}", m.unique_no),
                "act_index": format!("0x{:04x}", m.act_index),
                "send_flag": format!("0x{:02x}", m.send_flag),
                "name_bit_set": m.send_flag & 0x08 != 0,
                "body_len": m.body_len,
                "body_hex": m.body_hex,
                "miss_kind": m.miss_kind,
                "at_unix_ms": m.at_unix_ms,
            })
        })
        .collect();
    serde_json::json!({
        "entries": entries,
        "count": state.name_misses.len(),
        "notes": "Ring buffer of the most recent name-extraction misses. \
                  `miss_kind: name_bit_clear` means LSB did not include the name \
                  this packet (expected when the spawn was missed); \
                  `name_bit_set_extraction_failed` means LSB sent UPDATE_NAME \
                  but the decoder rejected the slot — that's the signature of \
                  a remaining offset or validation bug. \
                  Body slot offsets: CHAR_PC = body[0x5A..], CHAR_NPC = body[0x30..] \
                  (or body[0x31..] if body[0x30] == 0x01, the renamed-low-targid marker).",
    })
}

/// Stable, snake_case label per `AgentEvent` variant. Matches the
/// serde tag the events serialize with, so callers can filter
/// `wait_for_event` by the same string they'd see in JSON.
fn event_kind_label(ev: &AgentEvent) -> &'static str {
    match ev {
        AgentEvent::Connected { .. } => "connected",
        AgentEvent::StageChanged { .. } => "stage_changed",
        AgentEvent::ZoneChanged { .. } => "zone_changed",
        AgentEvent::PositionChanged { .. } => "position_changed",
        AgentEvent::EntityUpserted { .. } => "entity_upserted",
        AgentEvent::EntityRemoved { .. } => "entity_removed",
        AgentEvent::EntityPatched { .. } => "entity_patched",
        AgentEvent::NameExtractionMiss { .. } => "name_extraction_miss",
        AgentEvent::ChatLine { .. } => "chat_line",
        AgentEvent::EventStart { .. } => "event_start",
        AgentEvent::EventDialog { .. } => "event_dialog",
        AgentEvent::ShopUpdated { .. } => "shop_updated",
        AgentEvent::StatusIconsUpdated { .. } => "status_icons_updated",
        AgentEvent::EventEnded => "event_ended",
        AgentEvent::KeyRotated { .. } => "key_rotated",
        AgentEvent::Disconnected { .. } => "disconnected",
        AgentEvent::Error { .. } => "error",
        AgentEvent::Diagnostics { .. } => "diagnostics",
        AgentEvent::PartyMemberUpdated { .. } => "party_member_updated",
        AgentEvent::LowHp { .. } => "low_hp",
        AgentEvent::PartyMemberLowHp { .. } => "party_member_low_hp",
        AgentEvent::EngagedBy { .. } => "engaged_by",
        AgentEvent::TellReceived { .. } => "tell_received",
        AgentEvent::Reconnected { .. } => "reconnected",
        AgentEvent::SceneSummary { .. } => "scene_summary",
        AgentEvent::InventoryUpdated { .. } => "inventory_updated",
        AgentEvent::InventoryReady => "inventory_ready",
        AgentEvent::ReactorGoalChanged { .. } => "reactor_goal_changed",
        AgentEvent::LlmDecision { .. } => "llm_decision",
        AgentEvent::HumanInControl { .. } => "human_in_control",
        AgentEvent::HumanReleased => "human_released",
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
        EndEventChoice { .. } => "end_event_choice",
        Snapshot => "snapshot",
        Disconnect => "disconnect",
        Follow { .. } => "follow",
        Engage { .. } => "engage",
        PathTo { .. } => "path_to",
        Cancel => "cancel",
        RequestZoneChange { .. } => "request_zone_change",
        MogHouseExit { .. } => "mog_house_exit",
        UseItem { .. } => "use_item",
        BankWhenFull { .. } => "bank_when_full",
        CheckTarget { .. } => "check_target",
        ShopBuy { .. } => "shop_buy",
        ReqLogout { .. } => "req_logout",
        ReturnToHomePoint => "return_to_home_point",
        Heal { .. } => "heal",
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
    decision_tx: Option<broadcast::Sender<AgentEvent>>,
) {
    loop {
        match event_rx.recv().await {
            Ok(ev) => {
                for uri in uris_for_event(&ev) {
                    let started = std::time::Instant::now();
                    let params = ResourceUpdatedNotificationParam { uri: (*uri).into() };
                    let send_result = peer.notify_resource_updated(params).await;
                    let latency_us = started.elapsed().as_micros() as u64;
                    if let Err(e) = send_result {
                        tracing::warn!(error = %e, uri = uri, "notify_resource_updated failed");
                    }
                    // Emit a NotificationFired even on send failure: the
                    // attempt counts as a wake signal the LLM might miss.
                    // Lossy try-send via broadcast::Sender.send semantics.
                    if let Some(tx) = &decision_tx {
                        let _ = tx.send(AgentEvent::LlmDecision {
                            decision: LlmDecision {
                                kind: LlmDecisionKind::NotificationFired {
                                    uri: (*uri).to_string(),
                                },
                                latency_us,
                                at_monotonic_ms: process_monotonic_ms(),
                            },
                        });
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

/// Background task: write high-signal events to `<dir>/last-event.json`
/// atomically, one event at a time. Read by the Claude Code Stop hook
/// to decide whether to keep the agent looping (recent event = pending
/// work) or sit idle. Same atomic tmp+rename pattern as `GoalStore`.
async fn run_event_sidecar(
    path: std::path::PathBuf,
    mut event_rx: broadcast::Receiver<AgentEvent>,
) {
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(error = %e, dir = %parent.display(), "create event sidecar dir");
            return;
        }
    }
    loop {
        match event_rx.recv().await {
            Ok(ev) => {
                if !is_high_signal(&ev) {
                    continue;
                }
                let kind = event_kind_label(&ev);
                let at_unix_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let body = serde_json::json!({
                    "kind": kind,
                    "at_unix_ms": at_unix_ms,
                    "payload": ev,
                });
                let bytes = match serde_json::to_vec_pretty(&body) {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(error = %e, "serialize last-event sidecar");
                        continue;
                    }
                };
                let tmp = path.with_extension("json.tmp");
                if let Err(e) = std::fs::write(&tmp, &bytes) {
                    tracing::warn!(error = %e, "write last-event tmp");
                    continue;
                }
                if let Err(e) = std::fs::rename(&tmp, &path) {
                    tracing::warn!(error = %e, "rename last-event sidecar");
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "event sidecar lagged");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Subset of events that are worth waking the LLM for — same set the
/// notifier uses for `scene://current`-class invalidations, plus the
/// inventory ready/zone change signals that gate banking.
fn is_high_signal(ev: &AgentEvent) -> bool {
    matches!(
        ev,
        AgentEvent::LowHp { .. }
            | AgentEvent::PartyMemberLowHp { .. }
            | AgentEvent::EngagedBy { .. }
            | AgentEvent::TellReceived { .. }
            | AgentEvent::Reconnected { .. }
            | AgentEvent::ZoneChanged { .. }
            | AgentEvent::InventoryReady
            | AgentEvent::Disconnected { .. }
    )
}

/// Default sidecar location: same parent dir as `goal.json`. Honored by
/// the FFXI_MCP_EVENT_PATH override so tests can redirect.
fn default_event_sidecar_path() -> Option<std::path::PathBuf> {
    GoalStore::default_path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("last-event.json")))
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
        AgentEvent::ZoneChanged { .. } => &[
            "scene://current",
            "party://members",
            "diagnostics://session",
        ],

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

    // Attach mode means the credentials/character live in the running
    // `ffxi-client native` peer, not here — skip FFXI_USER/PASS/CHAR
    // reads so attach works with a clean MCP environment.
    let attach_arg = std::env::var("FFXI_ATTACH").ok();
    let cfg = if attach_arg.is_some() {
        None
    } else {
        let char_id_env = std::env::var("FFXI_CHAR_ID").ok();
        let char_name_env = std::env::var("FFXI_CHAR").ok();
        let char_selection = match (char_id_env, char_name_env) {
            (Some(id_str), _) => {
                let id = id_str
                    .parse::<u32>()
                    .context("FFXI_CHAR_ID must be a u32")?;
                session::CharSelection::Id(id)
            }
            (None, Some(name)) => session::CharSelection::Name(name),
            (None, None) => anyhow::bail!("set either FFXI_CHAR_ID or FFXI_CHAR in .env"),
        };
        // Soft-degrade: when no FFXI install is reachable, static NPC
        // names render as "?" but the session still runs. MCP doesn't
        // expose a `--require-dat` switch — headless agent harnesses
        // run in containers that often lack the install, and a hard
        // failure would block the autonomous-agent path entirely.
        let dat_root = match ffxi_dat::DatRoot::from_env_or_default() {
            Ok(root) => {
                tracing::info!(
                    source = %root.root().display(),
                    "loaded FFXI DAT install for NPC name lookup"
                );
                Some(std::sync::Arc::new(root))
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "no FFXI DAT install reachable; static NPC names will render as '?'"
                );
                None
            }
        };
        Some(session::Config {
            server: read_env("FFXI_SERVER").unwrap_or_else(|_| "127.0.0.1".into()),
            map_host_override: std::env::var("FFXI_MAP_HOST_OVERRIDE").ok(),
            auth_port: parse_port("FFXI_AUTH_PORT", 54231)?,
            data_port: parse_port("FFXI_DATA_PORT", 54230)?,
            view_port: parse_port("FFXI_VIEW_PORT", 54001)?,
            user: read_env("FFXI_USER")?,
            password: read_env("FFXI_PASS")?,
            char_selection,
            initial_state: None,
            // MCP is the headless agent path — auto-dismiss events so an
            // unattended LLM session doesn't get stuck if an NPC trigger fires.
            user_driven_events: false,
            dat_root,
        })
    };

    let goal_path = match std::env::var("FFXI_MCP_GOAL_PATH") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => GoalStore::default_path()
            .context("resolve goal_store path; set FFXI_MCP_GOAL_PATH to override")?,
    };
    let goal_store = GoalStore::new(goal_path);

    // Optional WebSocket viewer relay. `FFXI_RELAY_LISTEN=auto` is the
    // recommended setting for harness use — the OS picks an ephemeral
    // port and the chosen address is printed to stderr by relay::serve.
    // Pre-flight here so a misconfigured port fails fast, before we take
    // over stdin/stdout for the MCP transport.
    let relay_addr: Option<std::net::SocketAddr> = match std::env::var("FFXI_RELAY_LISTEN") {
        Ok(s) if !s.is_empty() => match relay::parse_relay_listen(&s) {
            Ok(addr) => {
                if let Err(err) = relay::preflight_bind(addr) {
                    eprintln!("error: {err:#}");
                    eprintln!("hint: set FFXI_RELAY_LISTEN=auto to let the OS assign a free port,",);
                    eprintln!("      or pick a different host:port.");
                    std::process::exit(2);
                }
                Some(addr)
            }
            Err(msg) => {
                eprintln!("error: FFXI_RELAY_LISTEN={s:?}: {msg}");
                std::process::exit(2);
            }
        },
        _ => None,
    };

    let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>(64);
    let (event_tx, event_rx) = broadcast::channel::<AgentEvent>(256);
    let state = Arc::new(RwLock::new(SessionState::default()));

    // Producer task: either the in-process supervisor (default headless
    // mode) or an attach-mode bridge to a long-lived `ffxi-client native
    // --agent-listen` peer. `FFXI_ATTACH=<path|auto>` (read earlier into
    // `attach_arg`) selects the latter.
    let event_tx_for_producer = event_tx.clone();
    let supervisor_handle = if let Some(arg) = attach_arg {
        let sock = match attach::resolve_attach(&arg) {
            Ok(p) => p,
            Err(err) => {
                eprintln!("error: FFXI_ATTACH={arg:?}: {err:#}");
                std::process::exit(2);
            }
        };
        // In attach mode the running client owns the session, supervisor,
        // and goal_store on disk — `cfg` is None here by construction.
        tokio::spawn(async move {
            if let Err(e) = attach::run(sock, cmd_rx, event_tx_for_producer).await {
                tracing::error!(error = %e, "attach bridge exited with error");
            }
        })
    } else {
        // Headless mode: spawn the supervisor (which spawns reactor →
        // session inside).
        let cfg = cfg.expect("non-attach mode constructs cfg above");
        let sup_cfg = SupervisorConfig {
            goal_store: Some(goal_store.clone()),
            ..SupervisorConfig::default()
        };
        let reactor_cfg = ReactorConfig::default();
        tokio::spawn(async move {
            if let Err(e) =
                supervisor::run(cfg, cmd_rx, event_tx_for_producer, sup_cfg, reactor_cfg).await
            {
                tracing::error!(error = %e, "supervisor exited with error");
            }
        })
    };

    // Mirror task — folds events into `state` for resource reads.
    let mirror_handle = tokio::spawn(run_state_mirror(state.clone(), event_rx));

    // Optional viewer relay. Two tasks: a `run_event_folder` that turns
    // the broadcast event stream into a watch::Receiver<SessionState>
    // (the relay's snapshot source — independent of `run_state_mirror`,
    // which is keyed off `Arc<RwLock<SessionState>>` for resource reads),
    // and the WebSocket listener itself. The bound address is logged to
    // stderr by relay::serve so the user sees the URL the browser viewer
    // should connect to.
    let relay_handles = if let Some(addr) = relay_addr {
        let (state_tx, state_rx) = tokio::sync::watch::channel(SessionState::default());
        let folder_rx = event_tx.subscribe();
        let folder_h = tokio::spawn(session::run_event_folder(folder_rx, state_tx));
        let relay_event_tx = event_tx.clone();
        let relay_cmd_tx = cmd_tx.clone();
        let serve_h = tokio::spawn(async move {
            if let Err(err) = relay::serve(addr, state_rx, relay_event_tx, relay_cmd_tx).await {
                tracing::warn!(error = %err, "relay listener exited");
            }
        });
        Some((folder_h, serve_h))
    } else {
        None
    };

    // Sidecar task — writes high-signal events to ~/.config/ffxi-mcp/last-event.json
    // so the Claude Code Stop hook can decide whether the agent should keep
    // looping (recent event = pending work) or quietly idle.
    let event_sidecar_path = match std::env::var("FFXI_MCP_EVENT_PATH") {
        Ok(p) => Some(std::path::PathBuf::from(p)),
        Err(_) => default_event_sidecar_path(),
    };
    let sidecar_handle = event_sidecar_path.map(|path| {
        let rx = event_tx.subscribe();
        tokio::spawn(run_event_sidecar(path, rx))
    });

    // Subscribe BEFORE serve_server so we don't miss early events emitted
    // while the MCP handshake is still in progress.
    let notifier_event_rx = event_tx.subscribe();

    // Build the MCP server and serve over stdio. Wire the same event_tx
    // for decisions so `LlmDecision::ToolDispatched` and
    // `LlmDecision::NotificationFired` fold into `state.recent_decisions`
    // alongside session events. The notifier doesn't fan these out to MCP
    // clients (uris_for_event returns empty for LlmDecision), so there's
    // no recursive notification loop. A future combined-binary dashboard
    // can subscribe to the same broadcast and render the chrome badge
    // without changing this wiring.
    let server = FfxiServer::new(cmd_tx, state, goal_store)
        .with_decision_tx(event_tx.clone())
        .with_event_tx(event_tx.clone());
    let running = serve_server(server, stdio())
        .await
        .context("serve MCP server")?;

    // Now that the server is running we can grab its peer and start
    // forwarding broadcast events as ResourceUpdated notifications.
    let peer = running.peer().clone();
    let notifier_handle = tokio::spawn(run_notifier(
        peer,
        notifier_event_rx,
        Some(event_tx.clone()),
    ));

    running.waiting().await.context("MCP server crashed")?;

    // Once the MCP transport closes, shut down background tasks.
    supervisor_handle.abort();
    mirror_handle.abort();
    notifier_handle.abort();
    if let Some(h) = sidecar_handle {
        h.abort();
    }
    if let Some((folder_h, serve_h)) = relay_handles {
        folder_h.abort();
        serve_h.abort();
    }
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
    use ffxi_client::state::{ActionKind, Entity, EntityKind, PartyMember, Position, Stage, Vec3};

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
            &[
                "scene://current",
                "party://members",
                "diagnostics://session"
            ]
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
                in_mog_house: false,
            },
        };
        assert_eq!(uris_for_event(&ev), &["party://members", "scene://current"]);
    }

    #[test]
    fn stage_and_diagnostics_only_invalidate_diagnostics() {
        assert_eq!(
            uris_for_event(&AgentEvent::StageChanged {
                stage: Stage::InZone
            }),
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
                    speed: 0,
                    speed_base: 0,
                },
            },
            AgentEvent::EntityRemoved { id: 99 },
            AgentEvent::EventStart { event_id: 1 },
            AgentEvent::EventEnded,
            AgentEvent::KeyRotated {
                previous_status: ffxi_client::state::BlowfishStatus::Accepted,
            },
            AgentEvent::Error {
                message: "x".into(),
            },
        ];
        for ev in cases {
            assert!(
                uris_for_event(&ev).is_empty(),
                "should not notify for {ev:?}"
            );
        }
    }

    #[test]
    fn entities_view_sorts_by_distance_and_caps() {
        let mut s = SessionState::default();
        s.zone_id = Some(230);
        s.char_id = Some(7);
        // Self position now lives on the self entity in the entity list
        // (looked up by `id == char_id`). Seed it at the origin so the
        // distance ordering below is identical to the original test.
        s.entities.push(Entity {
            id: 7,
            act_index: 0,
            kind: EntityKind::Pc,
            name: Some("Self".into()),
            pos: Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            heading: 64,
            hp_pct: Some(100),
            bt_target_id: 0,
            claim_id: 0,
            speed: 25,
            speed_base: 25,
        });
        // Seed 35 entities at increasing distances; only nearest 30 should
        // appear, sorted ascending.
        for i in 0..35u32 {
            s.entities.push(Entity {
                id: 1000 + i,
                act_index: i as u16,
                kind: EntityKind::Mob,
                name: Some(format!("Mob{i}")),
                pos: Vec3 {
                    x: i as f32,
                    y: 0.0,
                    z: 0.0,
                },
                heading: 0,
                hp_pct: Some(100),
                bt_target_id: 0,
                claim_id: 0,
                speed: 0,
                speed_base: 0,
            });
        }
        let v = entities_view(&s);
        let arr = v["entities"].as_array().expect("entities array");
        assert_eq!(arr.len(), SCENE_ENTITIES_CAP);
        assert_eq!(arr[0]["id"], 1000, "nearest is id 1000 (distance 0)");
        assert_eq!(arr[29]["id"], 1029, "30th is id 1029");
        assert_eq!(v["self"]["zone_id"], 230);
        assert_eq!(v["self"]["char_id"], 7);
        assert_eq!(v["total_known"], 35);
    }

    #[test]
    fn entities_view_marks_claimed_targets() {
        let mut s = SessionState::default();
        s.entities.push(Entity {
            id: 99,
            act_index: 1,
            kind: EntityKind::Mob,
            name: Some("Bee".into()),
            pos: Vec3::default(),
            heading: 0,
            hp_pct: Some(60),
            bt_target_id: 4242,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
        });
        s.entities.push(Entity {
            id: 100,
            act_index: 2,
            kind: EntityKind::Mob,
            name: Some("Worm".into()),
            pos: Vec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
            heading: 0,
            hp_pct: Some(100),
            bt_target_id: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
        });
        let v = entities_view(&s);
        assert_eq!(v["entities"][0]["claimed_by"], 4242);
        assert!(v["entities"][1]["claimed_by"].is_null());
    }

    #[test]
    fn event_kind_label_round_trips_to_serde_tag() {
        // Sanity: a few labels match the serde tag downstream consumers
        // (the sidecar JSON, wait_for_event filter strings) rely on.
        assert_eq!(event_kind_label(&AgentEvent::LowHp { pct: 25 }), "low_hp");
        assert_eq!(
            event_kind_label(&AgentEvent::EngagedBy { entity_id: 1 }),
            "engaged_by"
        );
        assert_eq!(
            event_kind_label(&AgentEvent::TellReceived {
                from: "x".into(),
                text: "y".into()
            }),
            "tell_received"
        );
        assert_eq!(
            event_kind_label(&AgentEvent::InventoryReady),
            "inventory_ready"
        );
    }

    #[tokio::test]
    async fn wait_for_event_returns_matched_payload() {
        let (cmd_tx, _cmd_rx) = mpsc::channel::<AgentCommand>(8);
        let (event_tx, _) = broadcast::channel::<AgentEvent>(8);
        let state = Arc::new(RwLock::new(SessionState::default()));
        let goal_store = GoalStore::new(std::env::temp_dir().join("ffxi-mcp-test-wait-goal.json"));
        let server = FfxiServer::new(cmd_tx, state, goal_store).with_event_tx(event_tx.clone());

        let server_for_call = server.clone();
        let handle = tokio::spawn(async move {
            server_for_call
                .wait_for_event(Parameters(WaitForEventParams {
                    kinds: vec!["low_hp".into()],
                    timeout_ms: 2_000,
                }))
                .await
        });
        // Give wait_for_event a moment to subscribe; without this the
        // event can race ahead of the subscription and fail the test.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // First, an event we DON'T want — should be ignored.
        let _ = event_tx.send(AgentEvent::PositionChanged {
            pos: Position::default(),
        });
        let _ = event_tx.send(AgentEvent::LowHp { pct: 17 });
        let result = handle.await.unwrap().unwrap();
        let text = match result.content.first().unwrap().raw {
            rmcp::model::RawContent::Text(ref t) => t.text.clone(),
            _ => panic!("expected text content"),
        };
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["matched"], true);
        assert_eq!(v["kind"], "low_hp");
        assert_eq!(v["payload"]["pct"], 17);
    }

    #[tokio::test]
    async fn wait_for_event_times_out_cleanly() {
        let (cmd_tx, _cmd_rx) = mpsc::channel::<AgentCommand>(8);
        let (event_tx, _) = broadcast::channel::<AgentEvent>(8);
        let state = Arc::new(RwLock::new(SessionState::default()));
        let goal_store =
            GoalStore::new(std::env::temp_dir().join("ffxi-mcp-test-wait-timeout-goal.json"));
        let server = FfxiServer::new(cmd_tx, state, goal_store).with_event_tx(event_tx);
        let result = server
            .wait_for_event(Parameters(WaitForEventParams {
                kinds: vec!["low_hp".into()],
                timeout_ms: 100,
            }))
            .await
            .unwrap();
        let text = match result.content.first().unwrap().raw {
            rmcp::model::RawContent::Text(ref t) => t.text.clone(),
            _ => panic!("expected text content"),
        };
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["matched"], false);
        assert_eq!(v["reason"], "timeout");
    }

    /// V4a contract: when `decision_tx` is wired, a tool dispatch fires
    /// exactly one `LlmDecision::ToolDispatched` whose `tool` matches the
    /// command's `cmd_kind_label`. Headless mode (no decision_tx) emits
    /// nothing — that path is exercised by the absence of a panic when
    /// `with_decision_tx` is not called in `main()`.
    #[tokio::test]
    async fn tool_dispatch_emits_llm_decision_when_wired() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<AgentCommand>(8);
        let (decision_tx, mut decision_rx) = broadcast::channel::<AgentEvent>(8);
        let state = Arc::new(RwLock::new(SessionState::default()));
        let goal_store = GoalStore::new(std::env::temp_dir().join("ffxi-mcp-test-goal.json"));
        let server = FfxiServer::new(cmd_tx, state, goal_store).with_decision_tx(decision_tx);

        // Dispatch an Engage command and assert it both lands on cmd_rx
        // and emits a single LlmDecision event with kind=ToolDispatched
        // and tool="engage" (matches cmd_kind_label).
        let dispatched = server
            .send(AgentCommand::Engage { target_id: 42 })
            .await
            .expect("dispatch ok");
        assert!(matches!(
            dispatched,
            CallToolResult {
                is_error: Some(false) | None,
                ..
            }
        ));
        let received = cmd_rx.try_recv().expect("cmd landed in channel");
        assert!(matches!(received, AgentCommand::Engage { target_id: 42 }));
        let ev = decision_rx.try_recv().expect("decision event fired");
        match ev {
            AgentEvent::LlmDecision { decision } => match &decision.kind {
                LlmDecisionKind::ToolDispatched { tool } => assert_eq!(tool, "engage"),
                other => panic!("expected ToolDispatched, got {other:?}"),
            },
            other => panic!("expected LlmDecision, got {other:?}"),
        }
        // Channel should be empty now — exactly one decision per dispatch.
        assert!(decision_rx.try_recv().is_err());
    }

    /// Regression for the attach-mode "stale `goal://current`" bug.
    ///
    /// In attach mode no MCP-side supervisor writes the disk-backed
    /// `goal_store` — only the running native client's reactor knows
    /// about goal mutations, and it broadcasts them as
    /// `AgentEvent::ReactorGoalChanged`. The MCP's state mirror folds
    /// those events into `state.current_goal` (state.rs apply_event),
    /// but the original `read_resource` implementation read from disk
    /// instead, so a 12-day-old `path_to (5,5,0)` from a previous
    /// headless run kept showing as "active" even after the agent
    /// successfully sent `cancel`.
    ///
    /// Property pinned here: when `state.current_goal` is `Some(non-Idle)`,
    /// the live mirror wins. When it's `None`/`Idle`, the disk falls
    /// back into play (preserving the headless cross-restart use case).
    #[tokio::test]
    async fn goal_resource_prefers_in_memory_over_stale_disk() {
        use ffxi_client::state::ReactorGoalSnapshot;

        let goal_path = std::env::temp_dir().join(format!(
            "ffxi-mcp-test-goal-prefers-live-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&goal_path);
        let store = GoalStore::new(goal_path.clone());

        // Write a stale "active" goal to disk (simulates a previous
        // headless session's persisted PathTo).
        store
            .save(&AgentCommand::PathTo {
                x: 5.0,
                y: 5.0,
                z: 0.0,
            })
            .expect("save stale disk goal");

        let mutex = tokio::sync::Mutex::new(store);

        // Case 1: live mirror is in `Following`. The live goal wins
        // over the stale disk PathTo.
        let mut state = SessionState::default();
        state.current_goal = Some(ReactorGoalSnapshot::Following {
            target_id: 42,
            distance: 3.0,
        });
        let body = read_resource(&state, &mutex, "goal://current")
            .await
            .expect("read live goal");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["goal"], "active");
        assert_eq!(v["source"], "live");
        assert_eq!(v["snapshot"]["kind"], "following");
        assert_eq!(v["snapshot"]["target_id"], 42);

        // Case 2: live mirror says Idle. Disk falls back into play —
        // this preserves the headless-restart use case where the
        // persisted file is the only source.
        let mut state = SessionState::default();
        state.current_goal = Some(ReactorGoalSnapshot::Idle);
        let body = read_resource(&state, &mutex, "goal://current")
            .await
            .expect("read disk fallback");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["goal"], "active");
        assert_eq!(v["source"], "disk");

        // Case 3: live mirror is None and disk is empty → idle.
        let _ = std::fs::remove_file(&goal_path);
        let state = SessionState::default(); // current_goal: None
        let body = read_resource(&state, &mutex, "goal://current")
            .await
            .expect("read empty");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["goal"], "idle");
    }
}
