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
    state::{ActionKind, AgentCommand, AgentEvent, SessionState},
    supervisor::{self, SupervisorConfig},
};

#[derive(Debug, Deserialize, JsonSchema)]
struct FollowParams {
    target_id: u32,

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
    force: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ChatParams {
    kind: u8,
    text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TellParams {
    to: String,
    text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ZoneChangeParams {
    line_id: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CastParams {
    spell_id: u32,

    target_id: u32,

    target_index: u16,

    pos_x: Option<f32>,
    pos_y: Option<f32>,
    pos_z: Option<f32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WeaponskillParams {
    skill_id: u32,
    target_id: u32,
    target_index: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct JobAbilityParams {
    ability_id: u32,
    target_id: u32,
    target_index: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BankWhenFullParams {
    threshold: u8,

    mog_house_zoneline: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct UseItemParams {
    container: u8,

    slot: u8,

    item_no: u32,

    target_id: u32,

    target_index: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RaiseMenuParams {
    accept: bool,

    target_id: u32,

    target_index: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TractorMenuParams {
    accept: bool,
    target_id: u32,
    target_index: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct HomepointMenuParams {
    status_id: u32,
    target_id: u32,
    target_index: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ReadResourceParams {
    uri: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WaitForEventParams {
    #[serde(default)]
    kinds: Vec<String>,

    timeout_ms: u64,
}

#[derive(Clone)]
struct FfxiServer {
    cmd_tx: mpsc::Sender<AgentCommand>,

    state: Arc<RwLock<SessionState>>,

    goal_store: Arc<Mutex<GoalStore>>,

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
            event_tx: None,
        }
    }

    fn with_event_tx(mut self, tx: broadcast::Sender<AgentEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    async fn send(&self, cmd: AgentCommand) -> Result<CallToolResult, McpError> {
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
            force: p.force,
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
        let content = read_resource(&state, &self.goal_store, uri)
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
        let result = ListResourcesResult {
            resources: vec![
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
            ],
            ..Default::default()
        };
        Ok(result)
    }

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
        let body = read_resource(&state, &self.goal_store, uri)
            .await
            .map_err(|e| McpError::internal_error(e, None))?;
        let elapsed_us = started.elapsed().as_micros() as u64;
        tracing::debug!(uri, elapsed_us, "mcp.resource_read");
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            body, uri,
        )]))
    }
}

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
            use ffxi_client::state::ReactorGoalSnapshot;
            match &state.current_goal {
                Some(ReactorGoalSnapshot::Idle) | None => {
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

const SCENE_ENTITIES_CAP: usize = 30;

fn entities_view(state: &SessionState) -> serde_json::Value {
    let self_pos_p = state.self_position().unwrap_or_default();
    let from = self_pos_p.pos;

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
        AgentEvent::AbilityRecastsUpdated { .. } => "ability_recasts_updated",
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
        AgentEvent::EquipUpdated { .. } => "equip_updated",
        AgentEvent::EquipCleared => "equip_cleared",
        AgentEvent::SpellsKnownUpdated { .. } => "spells_known_updated",
        AgentEvent::CommandDataUpdated { .. } => "command_data_updated",
        AgentEvent::ReactorGoalChanged { .. } => "reactor_goal_changed",
        AgentEvent::HumanInControl { .. } => "human_in_control",
        AgentEvent::HumanReleased => "human_released",
        AgentEvent::ForcedMove { .. } => "forced_move",
        AgentEvent::MusicChanged { .. } => "music_changed",
        AgentEvent::MusicVolumeChanged { .. } => "music_volume_changed",
        AgentEvent::DeathTimerUpdated { .. } => "death_timer_updated",
        AgentEvent::WeatherUpdated { .. } => "weather_updated",
        AgentEvent::LogoutCountdown { .. } => "logout_countdown",
        AgentEvent::SetFps { .. } => "set_fps",
        AgentEvent::LevelUp { .. } => "level_up",
        AgentEvent::SkillLevelUp { .. } => "skill_level_up",
        AgentEvent::ActionStarted { .. } => "action_started",
        AgentEvent::VanaTimeSynced { .. } => "vana_time_synced",
    }
}

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
        Equip { .. } => "equip",
        BankWhenFull { .. } => "bank_when_full",
        CheckTarget { .. } => "check_target",
        ShopBuy { .. } => "shop_buy",
        ReqLogout { .. } => "req_logout",
        ReturnToHomePoint => "return_to_home_point",
        Heal { .. } => "heal",
        SetFps { .. } => "set_fps",
    }
}

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

async fn run_notifier(peer: Peer<RoleServer>, mut event_rx: broadcast::Receiver<AgentEvent>) {
    loop {
        match event_rx.recv().await {
            Ok(ev) => {
                for uri in uris_for_event(&ev) {
                    let params = ResourceUpdatedNotificationParam { uri: (*uri).into() };
                    let send_result = peer.notify_resource_updated(params).await;
                    if let Err(e) = send_result {
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

fn default_event_sidecar_path() -> Option<std::path::PathBuf> {
    GoalStore::default_path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("last-event.json")))
}

fn uris_for_event(ev: &AgentEvent) -> &'static [&'static str] {
    match ev {
        AgentEvent::LowHp { .. }
        | AgentEvent::PartyMemberLowHp { .. }
        | AgentEvent::EngagedBy { .. }
        | AgentEvent::TellReceived { .. }
        | AgentEvent::Reconnected { .. }
        | AgentEvent::SceneSummary { .. } => &["scene://current"],

        AgentEvent::ZoneChanged { .. } => &[
            "scene://current",
            "party://members",
            "diagnostics://session",
        ],

        AgentEvent::PartyMemberUpdated { .. } => &["party://members", "scene://current"],

        AgentEvent::InventoryUpdated { .. } => &["inventory://current"],
        AgentEvent::InventoryReady => &["inventory://current", "scene://current"],

        AgentEvent::StageChanged { .. } | AgentEvent::Diagnostics { .. } => {
            &["diagnostics://session"]
        }

        AgentEvent::Disconnected { .. } => &["scene://current", "diagnostics://session"],

        _ => &[],
    }
}

fn read_env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("env var {name} required"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

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

    let event_tx_for_producer = event_tx.clone();
    let supervisor_handle = if let Some(arg) = attach_arg {
        let sock = match attach::resolve_attach(&arg) {
            Ok(p) => p,
            Err(err) => {
                eprintln!("error: FFXI_ATTACH={arg:?}: {err:#}");
                std::process::exit(2);
            }
        };

        tokio::spawn(async move {
            if let Err(e) = attach::run(sock, cmd_rx, event_tx_for_producer).await {
                tracing::error!(error = %e, "attach bridge exited with error");
            }
        })
    } else {
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

    let mirror_handle = tokio::spawn(run_state_mirror(state.clone(), event_rx));

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

    let event_sidecar_path = match std::env::var("FFXI_MCP_EVENT_PATH") {
        Ok(p) => Some(std::path::PathBuf::from(p)),
        Err(_) => default_event_sidecar_path(),
    };
    let sidecar_handle = event_sidecar_path.map(|path| {
        let rx = event_tx.subscribe();
        tokio::spawn(run_event_sidecar(path, rx))
    });

    let notifier_event_rx = event_tx.subscribe();

    let server = FfxiServer::new(cmd_tx, state, goal_store).with_event_tx(event_tx.clone());
    let running = serve_server(server, stdio())
        .await
        .context("serve MCP server")?;

    let peer = running.peer().clone();
    let notifier_handle = tokio::spawn(run_notifier(peer, notifier_event_rx));

    running.waiting().await.context("MCP server crashed")?;

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

        assert_eq!(
            uris_for_event(&AgentEvent::InventoryReady),
            &["inventory://current", "scene://current"]
        );
    }

    #[test]
    fn tactical_events_do_not_notify() {
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
        let mut s = SessionState {
            zone_id: Some(230),
            char_id: Some(7),
            ..Default::default()
        };

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
            look: None,
            npc_state: None,
            status: 0,
        });

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
                look: None,
                npc_state: None,
                status: 0,
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
            look: None,
            npc_state: None,
            status: 0,
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
            look: None,
            npc_state: None,
            status: 0,
        });
        let v = entities_view(&s);
        assert_eq!(v["entities"][0]["claimed_by"], 4242);
        assert!(v["entities"][1]["claimed_by"].is_null());
    }

    #[test]
    fn event_kind_label_round_trips_to_serde_tag() {
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

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

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

    #[tokio::test]
    async fn goal_resource_prefers_in_memory_over_stale_disk() {
        use ffxi_client::state::ReactorGoalSnapshot;

        let goal_path = std::env::temp_dir().join(format!(
            "ffxi-mcp-test-goal-prefers-live-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&goal_path);
        let store = GoalStore::new(goal_path.clone());

        store
            .save(&AgentCommand::PathTo {
                x: 5.0,
                y: 5.0,
                z: 0.0,
                force: false,
            })
            .expect("save stale disk goal");

        let mutex = tokio::sync::Mutex::new(store);

        let state = SessionState {
            current_goal: Some(ReactorGoalSnapshot::Following {
                target_id: 42,
                distance: 3.0,
            }),
            ..Default::default()
        };
        let body = read_resource(&state, &mutex, "goal://current")
            .await
            .expect("read live goal");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["goal"], "active");
        assert_eq!(v["source"], "live");
        assert_eq!(v["snapshot"]["kind"], "following");
        assert_eq!(v["snapshot"]["target_id"], 42);

        let state = SessionState {
            current_goal: Some(ReactorGoalSnapshot::Idle),
            ..Default::default()
        };
        let body = read_resource(&state, &mutex, "goal://current")
            .await
            .expect("read disk fallback");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["goal"], "active");
        assert_eq!(v["source"], "disk");

        let _ = std::fs::remove_file(&goal_path);
        let state = SessionState::default();
        let body = read_resource(&state, &mutex, "goal://current")
            .await
            .expect("read empty");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["goal"], "idle");
    }
}
