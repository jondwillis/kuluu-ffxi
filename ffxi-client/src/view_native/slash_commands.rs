//! Parser for `/`-prefixed commands typed in the chat input bar.
//!
//! Translates a raw buffer like `"/follow Bob"` into a typed [`SlashOutcome`]
//! the input router can act on. The parser is deliberately pure (no Bevy
//! types, no I/O) so it's easy to unit-test and so `text_input_system`
//! stays focused on dispatch.
//!
//! # Name resolution
//!
//! Several commands accept an optional name (`/follow [Name]`,
//! `/attack [Name]`, `/target Name`). When a name is given we resolve it
//! against the current [`SceneSnapshot`]'s entities with a
//! case-insensitive prefix match; if multiple match, PCs win over other
//! kinds, then closer wins over farther. When no name is given we fall
//! back to the currently-selected target (mirroring vanilla FFXI's "use
//! current target" semantics).
//!
//! # Why a typed outcome enum
//!
//! The dispatcher needs to react in different ways: an [`AgentCommand`]
//! goes out the mpsc channel; a target change mutates a Bevy resource; a
//! quit emits `AppExit`; an unknown command shows a `[system]` chat line.
//! Returning a typed outcome keeps the parser side-effect-free and lets
//! the caller compose the right side effects in one place.

use ffxi_viewer_core::Preset;
use ffxi_viewer_wire::{ChatChannel, ChatLine, Entity as WireEntity, Vec3 as WireVec3};

use crate::state::{ActionKind, AgentCommand, CheckKind, HealMode, ReqLogoutKind};

/// Maximum zone-id value used for sanity-checking numeric `/zoneto`
/// args. The LSB zone table goes well past 300 (Adoulin, Voidwatch,
/// etc.) but never reaches 65535; this catches obvious typos.
const MAX_ZONE_ID: u16 = 600;

/// One row in the `/help` listing. `aliases[0]` is the canonical name;
/// the rest are accepted spellings (mirroring the `match` arms in
/// `parse_slash`). `usage` is the brief arg shape; `summary` is the
/// one-line description rendered in chat.
struct HelpEntry {
    aliases: &'static [&'static str],
    usage: &'static str,
    summary: &'static str,
}

/// Categorized listing rendered by `/help` and `/?`. Single source of
/// truth for the help screen — the `help_entries_dispatch_known`
/// test below pins each canonical alias against `parse_slash` so a
/// new command added to the match without a help entry (or vice
/// versa) trips a test failure.
const HELP_CATEGORIES: &[(&str, &[HelpEntry])] = &[
    (
        "Movement & Navigation",
        &[
            HelpEntry { aliases: &["follow"], usage: "[name]", summary: "follow target or current selection" },
            HelpEntry { aliases: &["pathto"], usage: "<x> <y> <z>", summary: "pathfind to world coordinates" },
            HelpEntry { aliases: &["zones"], usage: "", summary: "list zone-line destinations from current zone" },
            HelpEntry { aliases: &["zoneto"], usage: "<name|id>", summary: "pathfind to a zone-line by destination" },
            HelpEntry { aliases: &["navmesh"], usage: "[on|off]", summary: "toggle the navmesh debug overlay" },
            HelpEntry { aliases: &["navinfo"], usage: "", summary: "report navmesh snap status at current position" },
            HelpEntry { aliases: &["whereami", "pos"], usage: "", summary: "print self position and zone id" },
            HelpEntry { aliases: &["return", "homepoint", "hp"], usage: "", summary: "warp to home point (alive or dead)" },
        ],
    ),
    (
        "Combat & Targeting",
        &[
            HelpEntry { aliases: &["attack", "engage"], usage: "[name]", summary: "engage target (reactor goal)" },
            HelpEntry { aliases: &["disengage"], usage: "", summary: "clear active reactor goal" },
            HelpEntry { aliases: &["attackoff"], usage: "", summary: "one-shot attack-off packet on current target" },
            HelpEntry { aliases: &["assist"], usage: "[name]", summary: "assist target (inherit their target)" },
            HelpEntry { aliases: &["target"], usage: "[name]", summary: "set or clear current target" },
            HelpEntry { aliases: &["check", "checkname", "checkparam"], usage: "[name]", summary: "check target — strength / name / parameters" },
            HelpEntry { aliases: &["cast"], usage: "<spell> [target]", summary: "cast a spell" },
            HelpEntry { aliases: &["ws", "weaponskill"], usage: "<name> [target]", summary: "weapon skill" },
            HelpEntry { aliases: &["ja", "jobability"], usage: "<name> [target]", summary: "job ability" },
            HelpEntry { aliases: &["useitem", "use"], usage: "<name> [target]", summary: "use an item" },
            HelpEntry { aliases: &["cancel"], usage: "", summary: "cancel current reactor goal / action" },
            HelpEntry { aliases: &["raw"], usage: "<attack|attackoff> [name]", summary: "low-level Action packet (bypasses reactor)" },
        ],
    ),
    (
        "Chat",
        &[
            HelpEntry { aliases: &["s", "say"], usage: "<text>", summary: "say (local chat)" },
            HelpEntry { aliases: &["p", "party"], usage: "<text>", summary: "party chat" },
            HelpEntry { aliases: &["sh", "shout"], usage: "<text>", summary: "shout chat" },
            HelpEntry { aliases: &["l", "ls", "linkshell"], usage: "<text>", summary: "linkshell chat" },
            HelpEntry { aliases: &["t", "tell"], usage: "<name> <text>", summary: "tell another player" },
        ],
    ),
    (
        "Status & Menus",
        &[
            HelpEntry { aliases: &["sit"], usage: "", summary: "sit (not yet wired)" },
            HelpEntry { aliases: &["stand"], usage: "", summary: "stand (not yet wired)" },
            HelpEntry { aliases: &["heal"], usage: "[on|off]", summary: "toggle resting (CAMP)" },
            HelpEntry { aliases: &["raisemenu"], usage: "<option>", summary: "respond to raise dialog" },
            HelpEntry { aliases: &["tractormenu"], usage: "<option>", summary: "respond to tractor dialog" },
            HelpEntry { aliases: &["homepointmenu"], usage: "<option>", summary: "respond to homepoint dialog" },
            HelpEntry { aliases: &["buy"], usage: "<row> [qty]", summary: "buy from open shop by row index" },
            HelpEntry { aliases: &["bank"], usage: "<subcommand>", summary: "gil-bank operations" },
        ],
    ),
    (
        "Session",
        &[
            HelpEntry { aliases: &["logout"], usage: "[on|off]", summary: "request logout (30s LeaveGame timer)" },
            HelpEntry { aliases: &["shutdown"], usage: "[on|off]", summary: "request shutdown (LeaveGame, then close)" },
            HelpEntry { aliases: &["exit"], usage: "", summary: "polite logout + close window" },
            HelpEntry { aliases: &["disconnect", "quit"], usage: "", summary: "drop the connection immediately" },
        ],
    ),
    (
        "Debug & Tooling",
        &[
            HelpEntry { aliases: &["snapshot"], usage: "", summary: "emit a one-shot scene snapshot" },
            HelpEntry { aliases: &["zonechange", "rzc"], usage: "<id>", summary: "request zone change (debug)" },
            HelpEntry { aliases: &["agent"], usage: "<pause|resume|status>", summary: "human-in-control flag for agent commands" },
            HelpEntry { aliases: &["keybinds", "keybind", "binds"], usage: "<preset|list|reset>", summary: "manage keybind presets" },
            HelpEntry { aliases: &["load_mmb", "loadmmb"], usage: "<file_id> <chunk_idx>", summary: "spawn MMB model at self_pos (debug overlay)" },
            HelpEntry { aliases: &["load_mzb", "loadmzb"], usage: "<file_id> [chunk_idx]", summary: "load MZB mesh-library at self_pos (debug overlay)" },
            HelpEntry { aliases: &["help", "?"], usage: "", summary: "show this listing" },
        ],
    ),
];

/// Render the categorized `/help` text as a single multi-line string,
/// suitable for `SlashOutcome::SystemMessage`. The chat panel wraps
/// long lines at render time so no column-padding is needed.
fn render_help() -> String {
    let mut out = String::from("=== Slash command reference ===");
    for (category, entries) in HELP_CATEGORIES {
        out.push_str("\n[");
        out.push_str(category);
        out.push(']');
        for entry in *entries {
            // Format: `/alias1 | /alias2 <usage> -- summary`
            out.push_str("\n  ");
            for (i, name) in entry.aliases.iter().enumerate() {
                if i > 0 {
                    out.push_str(" | ");
                }
                out.push('/');
                out.push_str(name);
            }
            if !entry.usage.is_empty() {
                out.push(' ');
                out.push_str(entry.usage);
            }
            out.push_str(" -- ");
            out.push_str(entry.summary);
        }
    }
    out
}

/// What the input router should do after parsing a `/`-prefixed buffer.
#[derive(Debug, Clone)]
pub enum SlashOutcome {
    /// Dispatch this command on the agent channel.
    Command(AgentCommand),
    /// Dispatch a sequence of commands in order. Used when one slash maps
    /// to multiple wire packets — `/logout` is the canonical example
    /// (REQLOGOUT + CAMP-On so the player both arms the logout timer
    /// and starts resting during it). Each command is `try_send` in
    /// list order; failure of one doesn't short-circuit the rest, so
    /// the user still gets the logout if heal silently drops. Empty
    /// list is a no-op.
    Commands(Vec<AgentCommand>),
    /// Mutate the `Target` resource. `None` clears the target.
    SetTarget(Option<u32>),
    /// Same as [`Command(Disconnect)`](AgentCommand::Disconnect) but the
    /// caller should also fire `AppExit` so the window closes.
    Quit,
    /// `/exit` — fire-and-forget logout. Send a 0x0E7 `ReqLogout` so the
    /// server starts its proper LeaveGame flow (status-effect cleanup,
    /// charselect routing for non-GMs), then immediately disconnect and
    /// fire `AppExit`. The dispatcher must enqueue `ReqLogout` before
    /// `Disconnect` so the wire packet flushes ahead of the tear-down.
    /// `LogoutOn` (not `LogoutToggle`) so a second /exit while a logout
    /// is already armed re-arms rather than accidentally cancelling.
    QuitWithLogout(ReqLogoutKind),
    /// Append a `[system]` chat line. Used for unknown commands and for
    /// stubbed commands (like `/check`) we haven't fully wired yet.
    SystemMessage(String),
    /// `/buy <row> [qty]` — issued by the parser, resolved by the
    /// dispatcher into a full `AgentCommand::ShopBuy` once the live
    /// `SceneSnapshot.shop` is in scope (the parser can't see the shop
    /// state without taking on more context). The dispatcher fills in
    /// `shop_no` from the snapshot's `offset_index`.
    ShopBuyRow { shop_index: u8, qty: u32 },
    /// `/navmesh [on|off]` — flip the debug navmesh overlay. `None`
    /// means toggle (no arg given); `Some(b)` is an explicit set.
    /// Mirrors the `SetTarget(Option<u32>)` shape — a client-side
    /// state mutation, no agent/wire side effect.
    ToggleNavmesh(Option<bool>),
    /// `/keybinds preset|list|reset` — switch keybind preset, list the
    /// active map, or drop overrides back to the active preset's
    /// defaults. The dispatcher applies the change to the `Bindings`
    /// resource and persists via `keybinds_store`.
    ApplyKeybinds(KeybindUpdate),
    /// `/navinfo` — diagnostic: report whether the current self_pos
    /// snaps cleanly to a walkable navmesh polygon, and how far away
    /// the nearest zone-line origin is. Dispatcher resolves this by
    /// querying the live `NavmeshState` resource. Operator-facing
    /// chat output, no agent/wire side effect.
    NavInfo,
    /// `/agent pause|resume|status` — toggle the human-in-control
    /// flag. Dispatcher reads/writes the `AgentPaused` resource and
    /// emits the corresponding `AgentEvent::HumanInControl` /
    /// `HumanReleased` event on transitions. Parser is pure; the
    /// dispatcher does the I/O.
    AgentControl(AgentControlOp),
    /// `/load_mmb <file_id> <chunk_idx>` — debug-overlay: load and
    /// spawn an FFXI MMB entity model at `world_pos` (the parser
    /// pre-applies `ffxi_to_bevy` to self_pos so the dispatcher and
    /// downstream system stay axis-agnostic). The actual file I/O
    /// happens inside the `dat_mmb` system that consumes the
    /// resulting `LoadMmbRequest` event — keeping it out of the
    /// pure parser and out of `text_input_system`'s param list.
    LoadMmb {
        file_id: u32,
        chunk_idx: usize,
        world_pos: WireVec3,
    },
    /// `/load_mzb <file_id> [chunk_idx]` — debug-overlay: load a zone
    /// mesh-library. Optional chunk_idx because zone-bundle DATs
    /// usually have exactly one MZB; omitting it scans for the first
    /// kind=0x1C chunk (matches `dat-mzb-probe` behavior).
    LoadMzb {
        file_id: u32,
        chunk_idx: Option<usize>,
        world_pos: WireVec3,
    },
    /// `/drawdistance setworld N` / `/drawdistance setmob N` — mirrors
    /// the Ashita/Windower addon. `World` controls MZB overlay cull;
    /// `Mob` controls non-PC entity capsule cull. Naked `/drawdistance`
    /// shows the current values (caller dispatches a SystemMessage).
    SetDrawDistance(DrawDistanceOp),
    /// `/zonegeom on|off` — flip MZB overlay visibility. Diagnostic
    /// tool for perf — toggle off to verify whether the MZB merged
    /// mesh is the bottleneck.
    ToggleZoneGeom(Option<bool>),
}

/// `/drawdistance` subcommand variants. `Show` is a no-arg query for
/// the current values; `SetWorld(n)` and `SetMob(n)` set the cull
/// radii to `n` yalms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawDistanceOp {
    Show,
    SetWorld(f32),
    SetMob(f32),
}

/// `/agent` subcommand variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentControlOp {
    /// `/agent pause` — set the human-in-control flag. Subsequent
    /// agent-originated commands are dropped at the codec; the
    /// transition event `AgentEvent::HumanInControl` fires once.
    Pause,
    /// `/agent resume` — clear the flag and fire
    /// `AgentEvent::HumanReleased`.
    Resume,
    /// `/agent status` — operator-facing chat line reporting the
    /// current paused state. No event fired.
    Status,
}

/// `/keybinds` subcommand variants.
#[derive(Debug, Clone, PartialEq)]
pub enum KeybindUpdate {
    /// `preset <name>` — replace the active bindings with the named
    /// preset's defaults and persist.
    Preset(Preset),
    /// `reset` — drop overrides, return to the persisted preset's
    /// defaults. (Equivalent to `Preset(currently_persisted_preset)`
    /// from the operator's POV; the dispatcher resolves it.)
    Reset,
    /// `list` — print the current Action → key map as system chat lines.
    List,
}

/// Parse a `/`-prefixed buffer. The leading `/` must be present.
///
/// `entities` is the current snapshot entity list and `self_pos` is the
/// player's position — used for name → id resolution and tie-breaking
/// when several entities share a prefix. `current_target` falls in for
/// commands that accept "use current target" when no name is given.
/// `zone_id` is the current zone (used by `/zones` and `/zoneto`); pass
/// `None` if not yet known.
pub fn parse_slash(
    buffer: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
    zone_id: Option<u16>,
) -> SlashOutcome {
    let trimmed = buffer.trim_start();
    let body = trimmed.strip_prefix('/').unwrap_or(trimmed);

    // Split into command + rest. `splitn(2, ...)` gives us up to two
    // chunks; the rest may be empty for commands like `/cancel`.
    let mut parts = body.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("").to_ascii_lowercase();
    let rest = parts.next().unwrap_or("").trim();

    match cmd.as_str() {
        "follow" => match resolve_target_or_current(rest, entities, self_pos, current_target) {
            Some(id) => SlashOutcome::Command(AgentCommand::Follow {
                target_id: id,
                distance: 3.0,
            }),
            None => SlashOutcome::SystemMessage("/follow: no target".into()),
        },
        "attack" | "engage" => {
            // Reactor goal — matches the MCP `engage` tool's wire shape.
            // `spawn_session_with_reactor` (`lib.rs:92`) wires the reactor
            // in front of `cmd_rx` for the native viewer, so goal commands
            // are absorbed by the per-tick state machine. For the legacy
            // one-shot `ActionKind::Attack` semantics, see `/raw attack`.
            match resolve_action_target(rest, entities, self_pos, current_target) {
                Some((id, _idx)) => {
                    SlashOutcome::Command(AgentCommand::Engage { target_id: id })
                }
                None => SlashOutcome::SystemMessage(format!("/{cmd}: no target")),
            }
        }
        // `/disengage` clears the active reactor goal (matching the lack
        // of a dedicated `disengage` MCP tool — the agent uses `cancel`
        // to stop attacking). The low-level wire `Action::AttackOff` is
        // available as `/raw attackoff` when the operator specifically
        // wants the one-shot packet.
        "disengage" => SlashOutcome::Command(AgentCommand::Cancel),
        "attackoff" => match current_target {
            Some(id) => match entities.iter().find(|e| e.id == id) {
                Some(ent) => SlashOutcome::Command(AgentCommand::Action {
                    target_id: ent.id,
                    target_index: ent.act_index,
                    kind: ActionKind::AttackOff,
                }),
                None => SlashOutcome::SystemMessage(format!("/{cmd}: target not in zone")),
            },
            None => SlashOutcome::SystemMessage(format!("/{cmd}: no target")),
        },
        // Low-level escape hatch: `/raw <action>` sends the underlying
        // one-shot `Action` packet, bypassing the reactor. Use for
        // wire-level debugging when the operator specifically wants the
        // pre-Stage-4 behavior. Currently supports `attack` and
        // `attackoff`; `move` would also belong here but `/move` is
        // already keybind-driven so there's no slash-form pressure yet.
        "raw" => parse_raw(rest, entities, self_pos, current_target),
        "target" => {
            if rest.is_empty() {
                SlashOutcome::SetTarget(None)
            } else {
                match resolve_name(rest, entities, self_pos) {
                    Some(ent) => SlashOutcome::SetTarget(Some(ent.id)),
                    None => SlashOutcome::SystemMessage(format!("/target: no entity '{rest}'")),
                }
            }
        }
        "assist" => match resolve_action_target(rest, entities, self_pos, current_target) {
            Some((id, idx)) => SlashOutcome::Command(AgentCommand::Action {
                target_id: id,
                target_index: idx,
                kind: ActionKind::Assist,
            }),
            None => SlashOutcome::SystemMessage("/assist: no target".into()),
        },
        "check" | "checkname" | "checkparam" => {
            match resolve_action_target(rest, entities, self_pos, current_target) {
                Some((id, idx)) => SlashOutcome::Command(AgentCommand::CheckTarget {
                    target_id: id,
                    target_index: idx,
                    kind: match cmd.as_str() {
                        "checkname" => CheckKind::CheckName,
                        "checkparam" => CheckKind::CheckParam,
                        _ => CheckKind::Check,
                    },
                }),
                None => SlashOutcome::SystemMessage(format!("/{cmd}: no target")),
            }
        }
        "buy" => parse_buy(rest),
        "sit" => SlashOutcome::SystemMessage("/sit: not yet wired".into()),
        "stand" => SlashOutcome::SystemMessage("/stand: not yet wired".into()),
        "cancel" => SlashOutcome::Command(AgentCommand::Cancel),
        // ---- Stage 4 lockstep: every MCP tool has a slash twin -----------
        // The MCP `cast` / `weaponskill` / `job_ability` tools wrap the same
        // `Action { ActionKind::* }` shape — slash twins dispatch the
        // identical AgentCommand. `slash_mcp_lockstep.rs` pins this.
        "cast" => parse_cast(rest, entities, self_pos, current_target),
        "ws" | "weaponskill" => parse_weaponskill(rest, entities, self_pos, current_target),
        "ja" | "jobability" => parse_job_ability(rest, entities, self_pos, current_target),
        "useitem" | "use" => parse_use_item(rest, entities, self_pos, current_target),
        "raisemenu" => parse_raise_menu(rest),
        "tractormenu" => parse_tractor_menu(rest),
        "homepointmenu" => parse_homepoint_menu(rest),
        "snapshot" => SlashOutcome::Command(AgentCommand::Snapshot),
        "bank" => parse_bank(rest),
        "zonechange" | "rzc" => parse_zone_change(rest),
        "agent" => parse_agent(rest),
        // `/disconnect` and `/quit` skip the in-world LeaveGame timer
        // and just drop the TCP/UDP sockets. Distinct from `/logout` —
        // the operator chose to abandon the session, not to politely
        // return to char-select.
        "disconnect" | "quit" => SlashOutcome::Quit,
        // `/exit` is the courteous variant: tell the server we're
        // logging out (so it runs its normal status-effect cleanup /
        // session-end paths) AND close the window immediately. Distinct
        // from `/logout` (which leaves the window open during the 30s
        // countdown) and from `/quit` (which doesn't notify the server).
        "exit" => SlashOutcome::QuitWithLogout(ReqLogoutKind::LogoutOn),
        // `/logout` and `/shutdown`: in-world request that arms the
        // server's LeaveGame effect (30s countdown for normal players,
        // immediate for GMs and inside Mog Houses). Toggles by default;
        // `on`/`off` arguments arm or cancel explicitly.
        "logout" => parse_reqlogout(rest, /* shutdown = */ false),
        // `/heal` — toggle resting (`EFFECT_HEALING`). Default arg is
        // Toggle, the always-safe form; explicit `on`/`off` give the
        // operator deterministic control but the server rejects the
        // packet when the mode mismatches the current state ("Requested
        // healing when already healing" / "Requested stop healing when
        // not healing"). Movement implicitly cancels — the session-loop
        // keepalive interceptor prepends `0x0E8 Mode::Off` when the
        // next tick's `self_pos` differs from the last keepalived one.
        "heal" => parse_heal(rest),
        "navmesh" => parse_navmesh(rest),
        "load_mmb" | "loadmmb" => parse_load_mmb(rest, self_pos),
        "load_mzb" | "loadmzb" => parse_load_mzb(rest, self_pos),
        "look" => parse_look(rest, entities, self_pos, current_target),
        "drawdistance" | "dd" => parse_drawdistance(rest),
        "zonegeom" => parse_zonegeom(rest),
        "keybinds" | "keybind" | "binds" => parse_keybinds(rest),
        "pathto" => parse_pathto(rest, entities, current_target),
        "zones" => parse_zones(zone_id),
        "zoneto" => parse_zoneto(rest, zone_id),
        "navinfo" => SlashOutcome::NavInfo,
        "whereami" | "pos" => SlashOutcome::SystemMessage(format!(
            "self_pos: x={:.2} y={:.2} z={:.2}  zone={}",
            self_pos.x,
            self_pos.y,
            self_pos.z,
            zone_id.map_or("?".to_string(), |z| z.to_string()),
        )),
        "shutdown" => parse_reqlogout(rest, /* shutdown = */ true),
        // Accept the homepoint warp menu — what the official FFXI client
        // sends when the player picks "Return to home point" after dying.
        // Phoenix sets `requestedWarp = true` and the zone-tick processes
        // it via `charutils::HomePoint`. The warp fires regardless of
        // death state — typing `/return` while alive will still HP-warp
        // (with the dead-or-alive flavor passed by `PChar->isDead()`).
        // We don't gate locally because the agent harness wants exactly
        // the wire-protocol contract; gating would diverge from retail.
        "return" | "homepoint" | "hp" => SlashOutcome::Command(AgentCommand::ReturnToHomePoint),
        "p" | "party" => chat_or_empty(rest, 4, "/p"),
        "sh" | "shout" => chat_or_empty(rest, 1, "/sh"),
        "l" | "linkshell" | "ls" => chat_or_empty(rest, 5, "/l"),
        "s" | "say" => chat_or_empty(rest, 0, "/s"),
        "t" | "tell" => parse_tell(rest),
        "help" | "?" => SlashOutcome::SystemMessage(render_help()),
        "" => SlashOutcome::SystemMessage("empty command".into()),
        unknown => SlashOutcome::SystemMessage(format!("unknown command: /{unknown}")),
    }
}

/// Build a `[system]` chat line from a system message. Lets the caller
/// append this directly into the snapshot's chat buffer for display.
pub fn system_chat_line(text: String) -> ChatLine {
    ChatLine {
        channel: ChatChannel::System,
        sender: "client".into(),
        text,
        server_ts: 0,
    }
}

fn chat_or_empty(rest: &str, kind: u8, label: &str) -> SlashOutcome {
    if rest.is_empty() {
        SlashOutcome::SystemMessage(format!("{label}: empty message"))
    } else {
        SlashOutcome::Command(AgentCommand::Chat {
            kind,
            text: rest.to_string(),
        })
    }
}

/// `/buy <row> [qty]` — purchase the given shop row, defaulting to qty=1.
/// `row` is the `ShopIndex` from a prior 0x03C `SHOP_LIST` (matches the
/// number shown in the shop HUD's left column). The dispatcher pairs this
/// with the live shop's `offset_index` to build a full `ShopBuy` command.
fn parse_buy(rest: &str) -> SlashOutcome {
    let mut parts = rest.split_whitespace();
    let row_str = parts.next().unwrap_or("");
    if row_str.is_empty() {
        return SlashOutcome::SystemMessage("/buy: usage `/buy <row> [qty]`".into());
    }
    let shop_index: u8 = match row_str.parse() {
        Ok(n) => n,
        Err(_) => return SlashOutcome::SystemMessage(format!("/buy: bad row `{row_str}`")),
    };
    let qty: u32 = match parts.next() {
        Some(q) => match q.parse() {
            Ok(n) if n >= 1 => n,
            _ => return SlashOutcome::SystemMessage(format!("/buy: bad qty `{q}`")),
        },
        None => 1,
    };
    SlashOutcome::ShopBuyRow { shop_index, qty }
}

/// `/logout [on|off]` and `/shutdown [on|off]` — issue a 0x0E7 ReqLogout.
/// Empty arg toggles. Anything other than `on`/`off` is rejected with a
/// system chat line (matching how retail clients display "Unable to do
/// that" rather than silently sending the toggle).
///
/// **Heal coupling**: when the logout *arms* (Toggle without context, or
/// On), we also dispatch `Heal { Mode::On }` so the player sits during
/// the 30-second countdown — matching retail behavior. The server's
/// `healing.lua::onEffectLose` also calls `delStatusEffectSilent(LEAVEGAME)`,
/// so cancelling heal during the countdown *also* cancels the logout
/// — symmetric. `Off` variants don't chain Heal: cancelling logout
/// shouldn't separately try to start resting. The dispatcher sends
/// `ReqLogout` first so the wire flushes it even if the channel is
/// near-capacity.
fn parse_reqlogout(rest: &str, shutdown: bool) -> SlashOutcome {
    let label = if shutdown { "/shutdown" } else { "/logout" };
    let arg = rest.trim().to_ascii_lowercase();
    let kind = match (arg.as_str(), shutdown) {
        ("", false) => ReqLogoutKind::LogoutToggle,
        ("on", false) => ReqLogoutKind::LogoutOn,
        ("off", false) => ReqLogoutKind::LogoutOff,
        ("", true) => ReqLogoutKind::ShutdownToggle,
        ("on", true) => ReqLogoutKind::ShutdownOn,
        ("off", true) => ReqLogoutKind::ShutdownOff,
        (other, _) => {
            return SlashOutcome::SystemMessage(format!(
                "{label}: usage `{label} [on|off]` (got `{other}`)"
            ));
        }
    };
    let arms = matches!(
        kind,
        ReqLogoutKind::LogoutToggle
            | ReqLogoutKind::LogoutOn
            | ReqLogoutKind::ShutdownToggle
            | ReqLogoutKind::ShutdownOn,
    );
    if arms {
        SlashOutcome::Commands(vec![
            AgentCommand::ReqLogout { kind },
            AgentCommand::Heal { mode: HealMode::On },
        ])
    } else {
        SlashOutcome::Command(AgentCommand::ReqLogout { kind })
    }
}

/// `/heal [on|off]` — toggle/arm/cancel resting (0x0E8 CAMP). Empty arg
/// is the always-safe Toggle form. Unknown arg → system message,
/// mirroring `parse_reqlogout`'s usage-line shape.
fn parse_heal(rest: &str) -> SlashOutcome {
    let arg = rest.trim().to_ascii_lowercase();
    let mode = match arg.as_str() {
        "" | "toggle" => HealMode::Toggle,
        "on" => HealMode::On,
        "off" => HealMode::Off,
        other => {
            return SlashOutcome::SystemMessage(format!(
                "/heal: usage `/heal [on|off]` (got `{other}`)"
            ));
        }
    };
    SlashOutcome::Command(AgentCommand::Heal { mode })
}

/// `/pathto <x> <y> <z>` or `/pathto target` — issue a navmesh-aware
/// `AgentCommand::PathTo`. The numeric form takes three FFXI-world
/// floats; the `target` form pulls the current target's position so
/// you can chase without copying coords.
///
/// We don't add a `/pathto <name>` form because that's what `/follow`
/// already does (with a continuous re-issue rather than a single
/// path). Two commands with the same surface would just confuse the
/// operator.
fn parse_pathto(
    rest: &str,
    entities: &[WireEntity],
    current_target: Option<u32>,
) -> SlashOutcome {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return SlashOutcome::SystemMessage(
            "/pathto: usage `/pathto <x> <y> <z>` or `/pathto target`".into(),
        );
    }
    if trimmed.eq_ignore_ascii_case("target") {
        let Some(id) = current_target else {
            return SlashOutcome::SystemMessage("/pathto: no target".into());
        };
        let Some(ent) = entities.iter().find(|e| e.id == id) else {
            return SlashOutcome::SystemMessage("/pathto: target despawned".into());
        };
        return SlashOutcome::Command(AgentCommand::PathTo {
            x: ent.pos.x,
            y: ent.pos.y,
            z: ent.pos.z,
        });
    }
    let parts: Vec<&str> = trimmed.split_ascii_whitespace().collect();
    if parts.len() != 3 {
        return SlashOutcome::SystemMessage(format!(
            "/pathto: expected 3 coords (got {}); usage `/pathto <x> <y> <z>`",
            parts.len()
        ));
    }
    let coords: Result<Vec<f32>, _> = parts.iter().map(|p| p.parse::<f32>()).collect();
    match coords {
        Ok(v) => SlashOutcome::Command(AgentCommand::PathTo {
            x: v[0],
            y: v[1],
            z: v[2],
        }),
        Err(_) => SlashOutcome::SystemMessage(format!(
            "/pathto: bad coord in `{trimmed}` (expected three floats)"
        )),
    }
}

// ----- Stage 4 lockstep parsers ---------------------------------------------
//
// Each function dispatches the exact `AgentCommand` variant the matching
// MCP tool would emit. The lockstep test in
// `ffxi-client/tests/slash_mcp_lockstep.rs` pins this correspondence.

/// `/raw <subcommand>` — escape hatch for the wire-level one-shot
/// `Action` packets that the goal-level reactor swallows. Currently
/// supports `attack` and `attackoff`.
fn parse_raw(
    rest: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
) -> SlashOutcome {
    let mut parts = rest.trim().splitn(2, char::is_whitespace);
    let sub = parts.next().unwrap_or("").to_ascii_lowercase();
    let arg = parts.next().unwrap_or("").trim();
    match sub.as_str() {
        "attack" => match resolve_action_target(arg, entities, self_pos, current_target) {
            Some((id, idx)) => SlashOutcome::Command(AgentCommand::Action {
                target_id: id,
                target_index: idx,
                kind: ActionKind::Attack,
            }),
            None => SlashOutcome::SystemMessage("/raw attack: no target".into()),
        },
        "attackoff" => match current_target {
            Some(id) => match entities.iter().find(|e| e.id == id) {
                Some(ent) => SlashOutcome::Command(AgentCommand::Action {
                    target_id: ent.id,
                    target_index: ent.act_index,
                    kind: ActionKind::AttackOff,
                }),
                None => SlashOutcome::SystemMessage("/raw attackoff: target not in zone".into()),
            },
            None => SlashOutcome::SystemMessage("/raw attackoff: no target".into()),
        },
        "" => SlashOutcome::SystemMessage(
            "/raw: usage `/raw attack|attackoff [target]`".into(),
        ),
        other => SlashOutcome::SystemMessage(format!("/raw: unknown subcommand `{other}`")),
    }
}

/// `/cast <spell_id> [target_id] [target_index] [x y z]`
///
/// Wraps `Action { ActionKind::CastMagic { spell_id, pos_* } }`.
/// `target_id`/`target_index` default to the current target;
/// `pos_x/y/z` default to (0, 0, 0) for single-target casts. The
/// ground-target coords matter for AoE spells like Tractor — pass
/// all three.
fn parse_cast(
    rest: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
) -> SlashOutcome {
    let parts: Vec<&str> = rest.split_ascii_whitespace().collect();
    if parts.is_empty() {
        return SlashOutcome::SystemMessage(
            "/cast: usage `/cast <spell_id> [target_id] [target_index] [x y z]`".into(),
        );
    }
    let spell_id: u32 = match parts[0].parse() {
        Ok(n) => n,
        Err(_) => {
            return SlashOutcome::SystemMessage(format!(
                "/cast: bad spell_id `{}`",
                parts[0]
            ));
        }
    };
    let (target_id, target_index) = match parts.get(1) {
        Some(s) => {
            let id: u32 = match s.parse() {
                Ok(n) => n,
                Err(_) => {
                    return SlashOutcome::SystemMessage(format!("/cast: bad target_id `{s}`"));
                }
            };
            let idx: u16 = match parts.get(2) {
                Some(t) => match t.parse() {
                    Ok(n) => n,
                    Err(_) => {
                        return SlashOutcome::SystemMessage(format!(
                            "/cast: bad target_index `{t}`"
                        ));
                    }
                },
                None => entities
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.act_index)
                    .unwrap_or(0),
            };
            (id, idx)
        }
        None => match resolve_action_target("", entities, self_pos, current_target) {
            Some((id, idx)) => (id, idx),
            None => return SlashOutcome::SystemMessage("/cast: no target".into()),
        },
    };
    let coords: [f32; 3] = match parts.len() {
        n if n >= 6 => {
            let xyz: Result<Vec<f32>, _> = parts[3..6].iter().map(|p| p.parse()).collect();
            match xyz {
                Ok(v) => [v[0], v[1], v[2]],
                Err(_) => {
                    return SlashOutcome::SystemMessage(
                        "/cast: bad ground-target coords (expected three floats)".into(),
                    );
                }
            }
        }
        _ => [0.0, 0.0, 0.0],
    };
    SlashOutcome::Command(AgentCommand::Action {
        target_id,
        target_index,
        kind: ActionKind::CastMagic {
            spell_id,
            pos_x: coords[0],
            pos_y: coords[1],
            pos_z: coords[2],
        },
    })
}

/// `/ws <skill_id> [target_id] [target_index]`
fn parse_weaponskill(
    rest: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
) -> SlashOutcome {
    let parts: Vec<&str> = rest.split_ascii_whitespace().collect();
    if parts.is_empty() {
        return SlashOutcome::SystemMessage(
            "/ws: usage `/ws <skill_id> [target_id] [target_index]`".into(),
        );
    }
    let skill_id: u32 = match parts[0].parse() {
        Ok(n) => n,
        Err(_) => return SlashOutcome::SystemMessage(format!("/ws: bad skill_id `{}`", parts[0])),
    };
    let (target_id, target_index) =
        match resolve_target_args(&parts[1..], entities, self_pos, current_target) {
            Ok(pair) => pair,
            Err(msg) => return SlashOutcome::SystemMessage(format!("/ws: {msg}")),
        };
    SlashOutcome::Command(AgentCommand::Action {
        target_id,
        target_index,
        kind: ActionKind::Weaponskill { skill_id },
    })
}

/// `/ja <ability_id> [target_id] [target_index]` — self-target by default.
fn parse_job_ability(
    rest: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
) -> SlashOutcome {
    let parts: Vec<&str> = rest.split_ascii_whitespace().collect();
    if parts.is_empty() {
        return SlashOutcome::SystemMessage(
            "/ja: usage `/ja <ability_id> [target_id] [target_index]`".into(),
        );
    }
    let ability_id: u32 = match parts[0].parse() {
        Ok(n) => n,
        Err(_) => return SlashOutcome::SystemMessage(format!("/ja: bad ability_id `{}`", parts[0])),
    };
    // JAs default to self-target when no explicit target — but
    // self_id isn't in scope here. Caller passes 0/0 which the
    // server interprets as self for self-target abilities.
    let (target_id, target_index) =
        match resolve_target_args(&parts[1..], entities, self_pos, current_target) {
            Ok(pair) => pair,
            Err(_) => (0, 0),
        };
    SlashOutcome::Command(AgentCommand::Action {
        target_id,
        target_index,
        kind: ActionKind::JobAbility { ability_id },
    })
}

/// `/useitem <container> <slot> [item_no] [target_id] [target_index]`
///
/// Container ids match Phoenix's storage codes (0=Inventory, 1=Safe, 8=Wardrobe).
/// `item_no` is the LLM's bookkeeping hint; the wire packet sends 0
/// per `0x037_item_use.cpp::validate`'s `mustEqual(ItemNum, 0)`.
fn parse_use_item(
    rest: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
) -> SlashOutcome {
    let parts: Vec<&str> = rest.split_ascii_whitespace().collect();
    if parts.len() < 2 {
        return SlashOutcome::SystemMessage(
            "/useitem: usage `/useitem <container> <slot> [item_no] [target_id] [target_index]`"
                .into(),
        );
    }
    let container: u8 = match parts[0].parse() {
        Ok(n) => n,
        Err(_) => return SlashOutcome::SystemMessage(format!("/useitem: bad container `{}`", parts[0])),
    };
    let slot: u8 = match parts[1].parse() {
        Ok(n) => n,
        Err(_) => return SlashOutcome::SystemMessage(format!("/useitem: bad slot `{}`", parts[1])),
    };
    let item_no: u32 = match parts.get(2) {
        Some(s) => match s.parse() {
            Ok(n) => n,
            Err(_) => return SlashOutcome::SystemMessage(format!("/useitem: bad item_no `{s}`")),
        },
        None => 0,
    };
    let tail: &[&str] = parts.get(3..).unwrap_or(&[]);
    let (target_id, target_index) =
        match resolve_target_args(tail, entities, self_pos, current_target) {
            Ok(pair) => pair,
            Err(_) => (0, 0),
        };
    SlashOutcome::Command(AgentCommand::UseItem {
        container,
        slot,
        item_no,
        target_id,
        target_index,
    })
}

/// `/raisemenu accept|decline` — respond to the in-game raise prompt.
fn parse_raise_menu(rest: &str) -> SlashOutcome {
    match rest.trim().to_ascii_lowercase().as_str() {
        "accept" | "yes" | "y" => SlashOutcome::Command(AgentCommand::Action {
            target_id: 0,
            target_index: 0,
            kind: ActionKind::RaiseMenu { accept: true },
        }),
        "decline" | "no" | "n" => SlashOutcome::Command(AgentCommand::Action {
            target_id: 0,
            target_index: 0,
            kind: ActionKind::RaiseMenu { accept: false },
        }),
        "" => SlashOutcome::SystemMessage("/raisemenu: usage `/raisemenu accept|decline`".into()),
        other => SlashOutcome::SystemMessage(format!("/raisemenu: bad choice `{other}`")),
    }
}

/// `/tractormenu accept|decline` — respond to the in-game tractor prompt.
fn parse_tractor_menu(rest: &str) -> SlashOutcome {
    match rest.trim().to_ascii_lowercase().as_str() {
        "accept" | "yes" | "y" => SlashOutcome::Command(AgentCommand::Action {
            target_id: 0,
            target_index: 0,
            kind: ActionKind::TractorMenu { accept: true },
        }),
        "decline" | "no" | "n" => SlashOutcome::Command(AgentCommand::Action {
            target_id: 0,
            target_index: 0,
            kind: ActionKind::TractorMenu { accept: false },
        }),
        "" => SlashOutcome::SystemMessage(
            "/tractormenu: usage `/tractormenu accept|decline`".into(),
        ),
        other => SlashOutcome::SystemMessage(format!("/tractormenu: bad choice `{other}`")),
    }
}

/// `/homepointmenu <status_id>` — respond to the homepoint warp prompt.
/// `status_id` is the server's `WarpStatus` (0=accept, 1=cancel monstrosity,
/// 2=retry). Mirrors the MCP `homepoint_menu` tool argument.
fn parse_homepoint_menu(rest: &str) -> SlashOutcome {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return SlashOutcome::SystemMessage(
            "/homepointmenu: usage `/homepointmenu <status_id>` (0=accept,1=cancel,2=retry)".into(),
        );
    }
    match trimmed.parse::<u32>() {
        Ok(status_id) => SlashOutcome::Command(AgentCommand::Action {
            target_id: 0,
            target_index: 0,
            kind: ActionKind::HomepointMenu { status_id },
        }),
        Err(_) => {
            SlashOutcome::SystemMessage(format!("/homepointmenu: bad status_id `{trimmed}`"))
        }
    }
}

/// `/bank <threshold> <mog_house_zoneline>` — start the BankWhenFull
/// reactor goal.
fn parse_bank(rest: &str) -> SlashOutcome {
    let parts: Vec<&str> = rest.split_ascii_whitespace().collect();
    if parts.len() != 2 {
        return SlashOutcome::SystemMessage(
            "/bank: usage `/bank <threshold> <mog_house_zoneline>`".into(),
        );
    }
    let threshold: u8 = match parts[0].parse() {
        Ok(n) => n,
        Err(_) => {
            return SlashOutcome::SystemMessage(format!(
                "/bank: bad threshold `{}`",
                parts[0]
            ));
        }
    };
    let mog_house_zoneline: u32 = match parts[1].parse() {
        Ok(n) => n,
        Err(_) => {
            return SlashOutcome::SystemMessage(format!(
                "/bank: bad mog_house_zoneline `{}`",
                parts[1]
            ));
        }
    };
    SlashOutcome::Command(AgentCommand::BankWhenFull {
        threshold,
        mog_house_zoneline,
    })
}

/// `/agent <pause|resume|status>` — toggle the human-in-control
/// pause flag and emit the matching transition event. The dispatcher
/// resolves the parsed `AgentControlOp` against the `AgentPaused`
/// Bevy resource — when the resource is absent (no `--agent-listen`
/// configured) the dispatcher emits a system chat line instead of
/// flipping anything.
fn parse_agent(rest: &str) -> SlashOutcome {
    let trimmed = rest.trim().to_ascii_lowercase();
    match trimmed.as_str() {
        "pause" => SlashOutcome::AgentControl(AgentControlOp::Pause),
        "resume" | "unpause" => SlashOutcome::AgentControl(AgentControlOp::Resume),
        "status" | "" => SlashOutcome::AgentControl(AgentControlOp::Status),
        other => SlashOutcome::SystemMessage(format!(
            "/agent: unknown subcommand `{other}` (use pause|resume|status)"
        )),
    }
}

/// `/zonechange <line_id>` — fire a `RequestZoneChange` (the MCP
/// `request_zone_change` tool's wire shape). The character must already
/// be standing in the zoneline rect for the server to honor it.
fn parse_zone_change(rest: &str) -> SlashOutcome {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return SlashOutcome::SystemMessage(
            "/zonechange: usage `/zonechange <line_id>`".into(),
        );
    }
    match trimmed.parse::<u32>() {
        Ok(line_id) => SlashOutcome::Command(AgentCommand::RequestZoneChange { line_id }),
        Err(_) => SlashOutcome::SystemMessage(format!("/zonechange: bad line_id `{trimmed}`")),
    }
}

/// Helper: resolve `[target_id] [target_index]` from raw parts, falling
/// back to the current target's entity lookup. Returns `(id, idx)` or a
/// short error message suitable for embedding in a `/<cmd>: …` reply.
fn resolve_target_args(
    parts: &[&str],
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
) -> Result<(u32, u16), String> {
    if let Some(s) = parts.first() {
        let id: u32 = s.parse().map_err(|_| format!("bad target_id `{s}`"))?;
        let idx: u16 = match parts.get(1) {
            Some(t) => t
                .parse()
                .map_err(|_| format!("bad target_index `{t}`"))?,
            None => entities
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.act_index)
                .unwrap_or(0),
        };
        Ok((id, idx))
    } else {
        resolve_action_target("", entities, self_pos, current_target)
            .ok_or_else(|| "no target".to_string())
    }
}

/// `/zones` — list zone-line destinations from the current zone in
/// the chat panel. Each line shows `→ Zone_Name (id)` and the FFXI
/// trigger position the operator can `/pathto` to. Read-only;
/// produces a single multi-line `SystemMessage`.
fn parse_zones(zone_id: Option<u16>) -> SlashOutcome {
    let Some(z) = zone_id else {
        return SlashOutcome::SystemMessage("/zones: not in a zone yet".into());
    };
    let lines = ffxi_nav::zone_lines_for(z);
    if lines.is_empty() {
        return SlashOutcome::SystemMessage(format!(
            "/zones: no zone-lines from zone {z} (instance / GM zone?)"
        ));
    }
    let mut msg = format!(
        "/zones from {} ({z}):",
        ffxi_nav::zone_name(z).unwrap_or("?")
    );
    for line in lines {
        let to_name = ffxi_nav::zone_name(line.to_zone).unwrap_or("?");
        msg.push_str(&format!(
            "\n  -> {} ({}) at ({:.0}, {:.0}, {:.0})",
            to_name,
            line.to_zone,
            line.from_pos[0],
            line.from_pos[1],
            line.from_pos[2],
        ));
    }
    SlashOutcome::SystemMessage(msg)
}

/// `/zoneto <name|id>` — pathfind to the zone-line in the current
/// zone whose destination matches. Name match is case-insensitive
/// prefix (so `/zoneto south` finds Southern San d'Oria). Numeric
/// args match the destination zone-id directly.
///
/// Multiple matches: pick the first one (the operator can disambiguate
/// by typing more of the name). Helpful but not exhaustive — for
/// edge cases, `/zones` shows the full list and `/pathto x y z` is
/// always available.
fn parse_zoneto(rest: &str, zone_id: Option<u16>) -> SlashOutcome {
    let needle = rest.trim();
    if needle.is_empty() {
        return SlashOutcome::SystemMessage(
            "/zoneto: usage `/zoneto <zone_name|id>`; try `/zones` to list".into(),
        );
    }
    let Some(z) = zone_id else {
        return SlashOutcome::SystemMessage("/zoneto: not in a zone yet".into());
    };
    let lines = ffxi_nav::zone_lines_for(z);
    if lines.is_empty() {
        return SlashOutcome::SystemMessage(format!("/zoneto: no zone-lines from zone {z}"));
    }
    // Numeric form: parse as u16 (rejects bogus values).
    let by_id = needle.parse::<u16>().ok().filter(|n| *n <= MAX_ZONE_ID);
    let needle_lower = needle.to_ascii_lowercase();
    let chosen = lines.iter().find(|line| {
        if Some(line.to_zone) == by_id {
            return true;
        }
        ffxi_nav::zone_name(line.to_zone)
            .map(|n| n.to_ascii_lowercase().starts_with(&needle_lower))
            .unwrap_or(false)
    });
    match chosen {
        Some(line) => SlashOutcome::Command(AgentCommand::PathTo {
            x: line.from_pos[0],
            y: line.from_pos[1],
            z: line.from_pos[2],
        }),
        None => SlashOutcome::SystemMessage(format!(
            "/zoneto: no zone-line matches `{needle}` (try `/zones` to list)"
        )),
    }
}

/// `/load_mmb <file_id> <chunk_idx>` — debug-overlay command for the
/// Phase 10a DAT pipeline. Spawns the MMB at the player's current
/// world position. Both args are decimal integers; bad parses get a
/// usage line in chat rather than a silent no-op.
///
/// The parser captures `self_pos` *at command time* (it's the snapshot
/// passed into `parse_slash`). The overlay does NOT track the player —
/// it stays where the operator stood when they typed the command, so
/// they can walk around the spawned model. `ffxi_to_bevy` is applied
/// here at the parser boundary, matching the convention used by every
/// other place we cross the FFXI→Bevy axis flip.
/// `/zonegeom on|off|toggle` — flip MZB overlay visibility.
fn parse_zonegeom(rest: &str) -> SlashOutcome {
    let arg = rest.trim().to_ascii_lowercase();
    let setting = match arg.as_str() {
        "" | "toggle" => None,
        "on" | "true" | "1" => Some(true),
        "off" | "false" | "0" => Some(false),
        other => {
            return SlashOutcome::SystemMessage(format!(
                "/zonegeom: bad arg `{other}` (use on|off|toggle)"
            ));
        }
    };
    SlashOutcome::ToggleZoneGeom(setting)
}

/// `/drawdistance` family. Matches Ashita/Windower conventions:
///   - `/drawdistance` or `/dd` — show current
///   - `/drawdistance setworld N` — MZB overlay cull distance
///   - `/drawdistance setmob N` — entity capsule cull distance
fn parse_drawdistance(rest: &str) -> SlashOutcome {
    let mut parts = rest.split_whitespace();
    let sub = parts.next().unwrap_or("").to_ascii_lowercase();
    if sub.is_empty() {
        return SlashOutcome::SetDrawDistance(DrawDistanceOp::Show);
    }
    let value_str = parts.next().unwrap_or("");
    let value: f32 = match value_str.parse() {
        Ok(v) if v > 0.0 => v,
        _ => {
            return SlashOutcome::SystemMessage(format!(
                "/drawdistance: bad value `{value_str}` (expected positive number)"
            ))
        }
    };
    match sub.as_str() {
        "setworld" | "world" => SlashOutcome::SetDrawDistance(DrawDistanceOp::SetWorld(value)),
        "setmob" | "mob" => SlashOutcome::SetDrawDistance(DrawDistanceOp::SetMob(value)),
        other => SlashOutcome::SystemMessage(format!(
            "/drawdistance: unknown sub `{other}` (use setworld | setmob)"
        )),
    }
}

/// `/look [name|act_index]` — print the decoded LookData for an entity.
/// Default: the current target. Diagnostic for hand-bootstrapping a
/// `modelid → MMB file_id` mapping by observation. Format is one line
/// keyed on the entity's display name + decoded look variant.
fn parse_look(
    rest: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
) -> SlashOutcome {
    use ffxi_viewer_wire::EntityLook;

    let ent: Option<&WireEntity> = if rest.is_empty() {
        current_target.and_then(|id| entities.iter().find(|e| e.id == id))
    } else if let Ok(idx) = rest.parse::<u16>() {
        entities.iter().find(|e| e.act_index == idx)
    } else {
        resolve_name(rest, entities, self_pos)
    };

    let Some(ent) = ent else {
        return SlashOutcome::SystemMessage(if rest.is_empty() {
            "/look: no target".into()
        } else {
            format!("/look: no entity '{rest}'")
        });
    };

    let name = ent.name.as_deref().unwrap_or("?");
    let body = match &ent.look {
        None => "look: none decoded yet (entity hasn't sent a CHAR_NPC look-bearing tick)".to_string(),
        Some(EntityLook::Standard { modelid }) => {
            format!("look: STANDARD modelid={modelid} (0x{modelid:04X})")
        }
        Some(EntityLook::Equipped {
            face, race, head, body, hands, legs, feet, main, sub, ranged,
        }) => format!(
            "look: EQUIPPED race={race} face={face} head=0x{head:04X} body=0x{body:04X} \
             hands=0x{hands:04X} legs=0x{legs:04X} feet=0x{feet:04X} \
             main=0x{main:04X} sub=0x{sub:04X} ranged=0x{ranged:04X}"
        ),
        Some(EntityLook::Door { size }) => format!("look: DOOR (size={size})"),
        Some(EntityLook::Transport { size }) => format!("look: TRANSPORT (size={size})"),
    };
    SlashOutcome::SystemMessage(format!("/look [{name}] {body}"))
}

fn parse_load_mmb(rest: &str, self_pos: WireVec3) -> SlashOutcome {
    let mut parts = rest.split_whitespace();
    let file_str = parts.next().unwrap_or("");
    let chunk_str = parts.next().unwrap_or("");
    if file_str.is_empty() || chunk_str.is_empty() {
        return SlashOutcome::SystemMessage(
            "/load_mmb: usage `/load_mmb <file_id> <chunk_idx>`".into(),
        );
    }
    let file_id: u32 = match file_str.parse() {
        Ok(n) => n,
        Err(_) => {
            return SlashOutcome::SystemMessage(format!("/load_mmb: bad file_id `{file_str}`"))
        }
    };
    let chunk_idx: usize = match chunk_str.parse() {
        Ok(n) => n,
        Err(_) => {
            return SlashOutcome::SystemMessage(format!("/load_mmb: bad chunk_idx `{chunk_str}`"))
        }
    };
    SlashOutcome::LoadMmb {
        file_id,
        chunk_idx,
        world_pos: self_pos,
    }
}

/// `/load_mzb <file_id> [chunk_idx]` — Phase 11a debug-overlay
/// companion to `/load_mmb`. Optional chunk_idx — zone DATs usually
/// only have one MZB so we auto-scan when it's omitted.
fn parse_load_mzb(rest: &str, self_pos: WireVec3) -> SlashOutcome {
    let mut parts = rest.split_whitespace();
    let file_str = parts.next().unwrap_or("");
    if file_str.is_empty() {
        return SlashOutcome::SystemMessage(
            "/load_mzb: usage `/load_mzb <file_id> [chunk_idx]`".into(),
        );
    }
    let file_id: u32 = match file_str.parse() {
        Ok(n) => n,
        Err(_) => {
            return SlashOutcome::SystemMessage(format!("/load_mzb: bad file_id `{file_str}`"))
        }
    };
    let chunk_idx = match parts.next() {
        None => None,
        Some(s) => match s.parse::<usize>() {
            Ok(n) => Some(n),
            Err(_) => {
                return SlashOutcome::SystemMessage(format!("/load_mzb: bad chunk_idx `{s}`"))
            }
        },
    };
    SlashOutcome::LoadMzb {
        file_id,
        chunk_idx,
        world_pos: self_pos,
    }
}

/// `/navmesh [on|off]` — empty arg toggles, `on` / `off` are explicit.
/// Same shape as `/logout`. Unknown args show the usage line rather
/// than silently flipping (operators sometimes typo "om" / "ofd").
fn parse_navmesh(rest: &str) -> SlashOutcome {
    match rest.trim().to_ascii_lowercase().as_str() {
        "" => SlashOutcome::ToggleNavmesh(None),
        "on" => SlashOutcome::ToggleNavmesh(Some(true)),
        "off" => SlashOutcome::ToggleNavmesh(Some(false)),
        other => SlashOutcome::SystemMessage(format!(
            "/navmesh: usage `/navmesh [on|off]` (got `{other}`)"
        )),
    }
}

/// `/keybinds [preset <name> | list | reset]` — switch preset, print the
/// active map, or drop overrides. Empty arg shows the usage line. We
/// invent this command (no retail equivalent — keybinding lives in the
/// retail Config menu) but match retail's slash style: single root
/// command with a subcommand verb, like `/equip head <item>`.
fn parse_keybinds(rest: &str) -> SlashOutcome {
    let mut parts = rest.splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let arg = parts.next().unwrap_or("").trim();
    match verb.as_str() {
        "" => SlashOutcome::SystemMessage(
            "/keybinds: usage `/keybinds preset <compact1|compact2|standard> | list | reset`"
                .into(),
        ),
        "preset" => match Preset::from_slug(arg) {
            Some(preset) => SlashOutcome::ApplyKeybinds(KeybindUpdate::Preset(preset)),
            None => SlashOutcome::SystemMessage(format!(
                "/keybinds: unknown preset `{arg}` — try compact1, compact2, or standard"
            )),
        },
        "list" => SlashOutcome::ApplyKeybinds(KeybindUpdate::List),
        "reset" => SlashOutcome::ApplyKeybinds(KeybindUpdate::Reset),
        other => SlashOutcome::SystemMessage(format!(
            "/keybinds: unknown verb `{other}` — try preset, list, or reset"
        )),
    }
}

fn parse_tell(rest: &str) -> SlashOutcome {
    let mut parts = rest.splitn(2, char::is_whitespace);
    let to = parts.next().unwrap_or("").trim();
    let text = parts.next().unwrap_or("").trim();
    if to.is_empty() || text.is_empty() {
        SlashOutcome::SystemMessage("/t: usage `/t Name message`".into())
    } else {
        SlashOutcome::Command(AgentCommand::Tell {
            to: to.to_string(),
            text: text.to_string(),
        })
    }
}

/// Resolve `name → wire entity`. Case-insensitive prefix match. Ties
/// broken by: PC kind first, then nearer wins.
fn resolve_name<'a>(
    name: &str,
    entities: &'a [WireEntity],
    self_pos: WireVec3,
) -> Option<&'a WireEntity> {
    let needle = name.to_ascii_lowercase();
    let mut matches: Vec<&WireEntity> = entities
        .iter()
        .filter(|e| {
            e.name
                .as_deref()
                .map(|n| n.to_ascii_lowercase().starts_with(&needle))
                .unwrap_or(false)
        })
        .collect();
    if matches.is_empty() {
        return None;
    }
    matches.sort_by(|a, b| {
        let pc_rank = |e: &WireEntity| {
            matches!(e.kind, ffxi_viewer_wire::EntityKind::Pc) as u8
        };
        // Higher pc_rank first → reverse compare.
        pc_rank(b).cmp(&pc_rank(a)).then_with(|| {
            let da = sq_dist(a.pos, self_pos);
            let db = sq_dist(b.pos, self_pos);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    matches.into_iter().next()
}

fn resolve_target_or_current(
    name: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
) -> Option<u32> {
    if name.is_empty() {
        current_target
    } else {
        resolve_name(name, entities, self_pos).map(|e| e.id)
    }
}

fn resolve_action_target(
    name: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
) -> Option<(u32, u16)> {
    if name.is_empty() {
        let id = current_target?;
        let ent = entities.iter().find(|e| e.id == id)?;
        Some((ent.id, ent.act_index))
    } else {
        resolve_name(name, entities, self_pos).map(|e| (e.id, e.act_index))
    }
}

fn sq_dist(a: WireVec3, b: WireVec3) -> f32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_wire::{Entity as WireEntity, EntityKind, Vec3 as WireVec3};

    fn ent(id: u32, name: &str, kind: EntityKind, x: f32, y: f32) -> WireEntity {
        WireEntity {
            id,
            act_index: id as u16,
            kind,
            name: Some(name.into()),
            pos: WireVec3 { x, y, z: 0.0 },
            heading: 0,
            hp_pct: None,
            bt_target_id: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
        }
    }

    fn empty_entities() -> Vec<WireEntity> {
        Vec::new()
    }

    fn origin() -> WireVec3 {
        WireVec3 { x: 0.0, y: 0.0, z: 0.0 }
    }

    #[test]
    fn empty_command_is_system_message() {
        let out = parse_slash("/", &empty_entities(), origin(), None, None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn unknown_command_is_system_message() {
        let out = parse_slash("/blarg", &empty_entities(), origin(), None, None);
        match out {
            SlashOutcome::SystemMessage(s) => assert!(s.contains("/blarg")),
            _ => panic!("expected SystemMessage"),
        }
    }

    #[test]
    fn party_chat_with_text() {
        let out = parse_slash("/p hello world", &empty_entities(), origin(), None, None);
        match out {
            SlashOutcome::Command(AgentCommand::Chat { kind, text }) => {
                assert_eq!(kind, 4);
                assert_eq!(text, "hello world");
            }
            other => panic!("expected /p Chat, got {other:?}"),
        }
    }

    #[test]
    fn party_chat_empty_text_is_system_message() {
        let out = parse_slash("/p", &empty_entities(), origin(), None, None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn tell_requires_name_and_text() {
        let out = parse_slash("/t Bob hi there", &empty_entities(), origin(), None, None);
        match out {
            SlashOutcome::Command(AgentCommand::Tell { to, text }) => {
                assert_eq!(to, "Bob");
                assert_eq!(text, "hi there");
            }
            other => panic!("expected Tell, got {other:?}"),
        }

        let out = parse_slash("/t Bob", &empty_entities(), origin(), None, None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn follow_with_name_resolves_to_id() {
        let entities = vec![
            ent(101, "Bob", EntityKind::Pc, 0.0, 0.0),
            ent(102, "Bobble", EntityKind::Npc, 5.0, 5.0),
        ];
        let out = parse_slash("/follow Bob", &entities, origin(), None, None);
        match out {
            SlashOutcome::Command(AgentCommand::Follow { target_id, .. }) => {
                assert_eq!(target_id, 101); // PC wins over NPC on prefix tie
            }
            other => panic!("expected Follow, got {other:?}"),
        }
    }

    #[test]
    fn follow_no_name_uses_current_target() {
        let out = parse_slash("/follow", &empty_entities(), origin(), Some(42), None);
        match out {
            SlashOutcome::Command(AgentCommand::Follow { target_id, .. }) => {
                assert_eq!(target_id, 42);
            }
            other => panic!("expected Follow, got {other:?}"),
        }
    }

    #[test]
    fn follow_no_name_no_target_is_system_message() {
        let out = parse_slash("/follow", &empty_entities(), origin(), None, None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn target_clears_with_no_arg() {
        let out = parse_slash("/target", &empty_entities(), origin(), Some(7), None);
        assert!(matches!(out, SlashOutcome::SetTarget(None)));
    }

    #[test]
    fn quit_aliases() {
        // `/quit` and `/disconnect` are the "drop the session" pair.
        // `/logout` is *not* in this group — it goes through the
        // server's LeaveGame flow; see `logout_*` tests below.
        for s in ["/quit", "/disconnect"] {
            assert!(matches!(
                parse_slash(s, &empty_entities(), origin(), None, None),
                SlashOutcome::Quit
            ));
        }
    }

    #[test]
    fn logout_no_arg_toggles_and_chains_heal_on() {
        // `/logout` toggles the LeaveGame effect and, because it arms,
        // also enqueues Heal::On so the player sits during the 30s
        // countdown (matches retail). Order matters: ReqLogout must be
        // first so the wire flush of 0x0E7 lands before 0x0E8.
        match parse_slash("/logout", &empty_entities(), origin(), None, None) {
            SlashOutcome::Commands(cmds) => {
                assert_eq!(cmds.len(), 2, "expected [ReqLogout, Heal], got {cmds:?}");
                assert!(
                    matches!(
                        cmds[0],
                        AgentCommand::ReqLogout { kind: ReqLogoutKind::LogoutToggle }
                    ),
                    "first cmd must be ReqLogout(LogoutToggle), got {:?}",
                    cmds[0]
                );
                assert!(
                    matches!(cmds[1], AgentCommand::Heal { mode: HealMode::On }),
                    "second cmd must be Heal(On), got {:?}",
                    cmds[1]
                );
            }
            other => panic!("expected Commands([ReqLogout, Heal]), got {other:?}"),
        }
    }

    #[test]
    fn logout_on_chains_heal_logout_off_does_not() {
        // Arming via explicit `on` chains Heal::On.
        match parse_slash("/logout on", &empty_entities(), origin(), None, None) {
            SlashOutcome::Commands(cmds) => {
                assert_eq!(cmds.len(), 2);
                assert!(matches!(
                    cmds[0],
                    AgentCommand::ReqLogout { kind: ReqLogoutKind::LogoutOn }
                ));
                assert!(matches!(cmds[1], AgentCommand::Heal { mode: HealMode::On }));
            }
            other => panic!("expected Commands([ReqLogout(On), Heal]), got {other:?}"),
        }
        // Cancelling via `off` is single-command — cancelling the
        // logout shouldn't separately try to start resting.
        match parse_slash("/logout off", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::ReqLogout { kind }) => {
                assert_eq!(kind, ReqLogoutKind::LogoutOff);
            }
            other => panic!("expected single Command(ReqLogout(Off)), got {other:?}"),
        }
    }

    #[test]
    fn shutdown_no_arg_toggles_and_chains_heal_on() {
        match parse_slash("/shutdown", &empty_entities(), origin(), None, None) {
            SlashOutcome::Commands(cmds) => {
                assert_eq!(cmds.len(), 2);
                assert!(matches!(
                    cmds[0],
                    AgentCommand::ReqLogout { kind: ReqLogoutKind::ShutdownToggle }
                ));
                assert!(matches!(cmds[1], AgentCommand::Heal { mode: HealMode::On }));
            }
            other => panic!("expected Commands, got {other:?}"),
        }
    }

    #[test]
    fn shutdown_on_chains_heal_shutdown_off_does_not() {
        match parse_slash("/shutdown on", &empty_entities(), origin(), None, None) {
            SlashOutcome::Commands(cmds) => {
                assert_eq!(cmds.len(), 2);
                assert!(matches!(
                    cmds[0],
                    AgentCommand::ReqLogout { kind: ReqLogoutKind::ShutdownOn }
                ));
                assert!(matches!(cmds[1], AgentCommand::Heal { mode: HealMode::On }));
            }
            other => panic!("expected Commands, got {other:?}"),
        }
        match parse_slash("/shutdown off", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::ReqLogout { kind }) => {
                assert_eq!(kind, ReqLogoutKind::ShutdownOff);
            }
            other => panic!("expected single Command(ReqLogout(ShutdownOff)), got {other:?}"),
        }
    }

    #[test]
    fn exit_emits_quit_with_logout_on() {
        // /exit must arm with `LogoutOn` (not Toggle) so it can't
        // accidentally cancel an in-flight logout from a prior /logout.
        match parse_slash("/exit", &empty_entities(), origin(), None, None) {
            SlashOutcome::QuitWithLogout(kind) => {
                assert_eq!(kind, ReqLogoutKind::LogoutOn);
            }
            other => panic!("expected QuitWithLogout(LogoutOn), got {other:?}"),
        }
    }

    #[test]
    fn logout_rejects_unknown_arg() {
        for s in ["/logout please", "/logout 1", "/shutdown maybe"] {
            assert!(
                matches!(
                    parse_slash(s, &empty_entities(), origin(), None, None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}"
            );
        }
    }

    #[test]
    fn heal_no_arg_toggles() {
        match parse_slash("/heal", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::Heal { mode }) => {
                assert_eq!(mode, HealMode::Toggle);
            }
            other => panic!("expected Command(Heal(Toggle)), got {other:?}"),
        }
    }

    #[test]
    fn heal_on_and_off_select_explicit_modes() {
        match parse_slash("/heal on", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::Heal { mode }) => {
                assert_eq!(mode, HealMode::On);
            }
            other => panic!("expected Command(Heal(On)), got {other:?}"),
        }
        match parse_slash("/heal off", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::Heal { mode }) => {
                assert_eq!(mode, HealMode::Off);
            }
            other => panic!("expected Command(Heal(Off)), got {other:?}"),
        }
        // `/heal toggle` is an alias for the no-arg form. Tested
        // separately so a future reader doesn't assume "toggle" was
        // never a valid arg literal.
        match parse_slash("/heal toggle", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::Heal { mode }) => {
                assert_eq!(mode, HealMode::Toggle);
            }
            other => panic!("expected Command(Heal(Toggle)), got {other:?}"),
        }
    }

    #[test]
    fn heal_rejects_unknown_arg() {
        for s in ["/heal please", "/heal 1", "/heal nope"] {
            assert!(
                matches!(
                    parse_slash(s, &empty_entities(), origin(), None, None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}"
            );
        }
    }

    /// `/load_mmb` requires both args (file_id + chunk_idx). Verify
    /// happy path captures `self_pos` as the parser's snapshot — the
    /// model spawns where the operator stood, not where they later
    /// walk to.
    #[test]
    fn load_mmb_parses_file_id_chunk_idx_and_captures_self_pos() {
        let pos = WireVec3 { x: 12.5, y: -7.0, z: 3.25 };
        match parse_slash("/load_mmb 115 18", &empty_entities(), pos, None, None) {
            SlashOutcome::LoadMmb { file_id, chunk_idx, world_pos } => {
                assert_eq!(file_id, 115);
                assert_eq!(chunk_idx, 18);
                assert_eq!(world_pos, pos);
            }
            other => panic!("expected LoadMmb, got {other:?}"),
        }
    }

    /// Both aliases (`load_mmb` and `loadmmb`) and bad args fall to
    /// `SystemMessage` so the operator sees a usage line instead of a
    /// silent no-op.
    #[test]
    fn load_mmb_alias_and_bad_args() {
        // Alias `/loadmmb` (no underscore) still routes.
        assert!(matches!(
            parse_slash("/loadmmb 115 18", &empty_entities(), origin(), None, None),
            SlashOutcome::LoadMmb { file_id: 115, chunk_idx: 18, .. }
        ));
        // Missing chunk_idx, non-numeric file_id, non-numeric chunk_idx.
        for s in ["/load_mmb", "/load_mmb 115", "/load_mmb foo 18", "/load_mmb 115 bar"] {
            assert!(
                matches!(
                    parse_slash(s, &empty_entities(), origin(), None, None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}",
            );
        }
    }

    /// `/load_mzb` allows omitting chunk_idx (the consumer scans for
    /// the first kind=0x1C). Captures `self_pos` like `/load_mmb`.
    #[test]
    fn load_mzb_parses_optional_chunk_idx() {
        let pos = WireVec3 { x: 1.0, y: 2.0, z: 3.0 };
        // No chunk_idx: `None`.
        match parse_slash("/load_mzb 7368", &empty_entities(), pos, None, None) {
            SlashOutcome::LoadMzb { file_id, chunk_idx, world_pos } => {
                assert_eq!(file_id, 7368);
                assert_eq!(chunk_idx, None);
                assert_eq!(world_pos, pos);
            }
            other => panic!("expected LoadMzb, got {other:?}"),
        }
        // Explicit chunk_idx.
        match parse_slash("/load_mzb 7368 2", &empty_entities(), pos, None, None) {
            SlashOutcome::LoadMzb { chunk_idx: Some(2), .. } => {}
            other => panic!("expected LoadMzb chunk_idx=Some(2), got {other:?}"),
        }
        // Alias `/loadmzb`.
        assert!(matches!(
            parse_slash("/loadmzb 7368", &empty_entities(), pos, None, None),
            SlashOutcome::LoadMzb { chunk_idx: None, .. }
        ));
        // Bad args fall to SystemMessage.
        for s in ["/load_mzb", "/load_mzb foo", "/load_mzb 7368 bar"] {
            assert!(
                matches!(
                    parse_slash(s, &empty_entities(), origin(), None, None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}",
            );
        }
    }

    #[test]
    fn navmesh_no_arg_toggles() {
        match parse_slash("/navmesh", &empty_entities(), origin(), None, None) {
            SlashOutcome::ToggleNavmesh(None) => {}
            other => panic!("expected ToggleNavmesh(None), got {other:?}"),
        }
    }

    #[test]
    fn navmesh_on_and_off_select_explicit_modes() {
        for (cmd, expected) in [("/navmesh on", Some(true)), ("/navmesh off", Some(false))] {
            match parse_slash(cmd, &empty_entities(), origin(), None, None) {
                SlashOutcome::ToggleNavmesh(setting) => assert_eq!(setting, expected),
                other => panic!("expected ToggleNavmesh({expected:?}), got {other:?}"),
            }
        }
    }

    #[test]
    fn navmesh_rejects_unknown_arg() {
        for s in ["/navmesh maybe", "/navmesh 1", "/navmesh ON!"] {
            assert!(
                matches!(
                    parse_slash(s, &empty_entities(), origin(), None, None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}"
            );
        }
    }

    #[test]
    fn pathto_numeric_three_args_dispatches() {
        match parse_slash("/pathto 1.5 2 -3.25", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::PathTo { x, y, z }) => {
                assert_eq!(x, 1.5);
                assert_eq!(y, 2.0);
                assert_eq!(z, -3.25);
            }
            other => panic!("expected PathTo, got {other:?}"),
        }
    }

    #[test]
    fn pathto_target_uses_current_target_pos() {
        let mut entity = ent(42, "Bob", EntityKind::Pc, 7.0, 8.0);
        entity.pos.z = 9.0;
        match parse_slash("/pathto target", &[entity], origin(), Some(42), None) {
            SlashOutcome::Command(AgentCommand::PathTo { x, y, z }) => {
                assert_eq!((x, y, z), (7.0, 8.0, 9.0));
            }
            other => panic!("expected PathTo from target, got {other:?}"),
        }
    }

    #[test]
    fn pathto_rejects_bad_input() {
        // "/pathto target" with no current target also rejects.
        for s in ["/pathto", "/pathto 1 2", "/pathto 1 2 3 4", "/pathto x y z", "/pathto target"] {
            assert!(
                matches!(
                    parse_slash(s, &empty_entities(), origin(), None, None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}"
            );
        }
    }

    #[test]
    fn cancel_emits_cancel_command() {
        assert!(matches!(
            parse_slash("/cancel", &empty_entities(), origin(), None, None),
            SlashOutcome::Command(AgentCommand::Cancel)
        ));
    }

    #[test]
    fn attack_uses_current_target_and_engage_goal() {
        // Stage 4a normalized `/attack` from `Action::Attack` (one-shot
        // wire packet) to `AgentCommand::Engage` (reactor goal). The
        // pre-normalization semantics live under `/raw attack` (see
        // `raw_attack_preserves_direct_action`).
        let entities = vec![ent(42, "Mob", EntityKind::Mob, 0.0, 0.0)];
        let out = parse_slash("/attack", &entities, origin(), Some(42), None);
        match out {
            SlashOutcome::Command(AgentCommand::Engage { target_id }) => {
                assert_eq!(target_id, 42);
            }
            other => panic!("expected Engage, got {other:?}"),
        }
    }

    #[test]
    fn engage_alias_matches_attack() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash("/engage", &entities, origin(), Some(7), None) {
            SlashOutcome::Command(AgentCommand::Engage { target_id }) => {
                assert_eq!(target_id, 7);
            }
            other => panic!("expected Engage, got {other:?}"),
        }
    }

    #[test]
    fn attackoff_emits_attack_off_action() {
        let entities = vec![ent(9, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash("/attackoff", &entities, origin(), Some(9), None) {
            SlashOutcome::Command(AgentCommand::Action {
                target_id, kind, ..
            }) => {
                assert_eq!(target_id, 9);
                assert!(matches!(kind, ActionKind::AttackOff));
            }
            other => panic!("expected Action(AttackOff), got {other:?}"),
        }
    }

    #[test]
    fn check_uses_current_target_with_kind_check() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        let out = parse_slash("/check", &entities, origin(), Some(7), None);
        match out {
            SlashOutcome::Command(AgentCommand::CheckTarget {
                target_id,
                target_index,
                kind,
            }) => {
                assert_eq!(target_id, 7);
                assert_eq!(target_index, 7);
                assert_eq!(kind, CheckKind::Check);
            }
            other => panic!("expected CheckTarget, got {other:?}"),
        }
    }

    #[test]
    fn checkname_and_checkparam_select_correct_kind() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash("/checkname", &entities, origin(), Some(7), None) {
            SlashOutcome::Command(AgentCommand::CheckTarget { kind, .. }) => {
                assert_eq!(kind, CheckKind::CheckName);
            }
            other => panic!("expected CheckTarget, got {other:?}"),
        }
        match parse_slash("/checkparam", &entities, origin(), Some(7), None) {
            SlashOutcome::Command(AgentCommand::CheckTarget { kind, .. }) => {
                assert_eq!(kind, CheckKind::CheckParam);
            }
            other => panic!("expected CheckTarget, got {other:?}"),
        }
    }

    #[test]
    fn check_no_target_is_system_message() {
        let out = parse_slash("/check", &empty_entities(), origin(), None, None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn buy_with_row_uses_qty_one_by_default() {
        let out = parse_slash("/buy 3", &empty_entities(), origin(), None, None);
        match out {
            SlashOutcome::ShopBuyRow { shop_index, qty } => {
                assert_eq!(shop_index, 3);
                assert_eq!(qty, 1);
            }
            other => panic!("expected ShopBuyRow, got {other:?}"),
        }
    }

    #[test]
    fn buy_with_qty_passes_it_through() {
        let out = parse_slash("/buy 0 12", &empty_entities(), origin(), None, None);
        match out {
            SlashOutcome::ShopBuyRow { shop_index, qty } => {
                assert_eq!(shop_index, 0);
                assert_eq!(qty, 12);
            }
            other => panic!("expected ShopBuyRow, got {other:?}"),
        }
    }

    #[test]
    fn buy_rejects_zero_qty_and_bad_input() {
        for s in ["/buy", "/buy abc", "/buy 1 0", "/buy 1 xyz"] {
            assert!(
                matches!(
                    parse_slash(s, &empty_entities(), origin(), None, None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}"
            );
        }
    }

    // ---- Stage 4 lockstep tests --------------------------------------------
    //
    // For each MCP tool, verify the matching slash command dispatches the
    // same `AgentCommand` variant. The MCP tool surface is enumerated in
    // `ffxi-mcp/src/main.rs`; if a new tool ships there, add a slash twin
    // here and a corresponding assertion in `tests/slash_mcp_lockstep.rs`.

    #[test]
    fn engage_dispatches_reactor_goal() {
        // Bug fix for the pre-Stage-4 drift: `/engage` used to send a
        // direct `Action { Attack }` action. The MCP `engage` tool sends
        // `AgentCommand::Engage` (reactor goal). They now match.
        let entities = vec![ent(99, "Bee", EntityKind::Mob, 1.0, 0.0)];
        match parse_slash("/engage", &entities, origin(), Some(99), None) {
            SlashOutcome::Command(AgentCommand::Engage { target_id }) => {
                assert_eq!(target_id, 99);
            }
            other => panic!("expected Engage, got {other:?}"),
        }
    }

    #[test]
    fn attack_is_alias_for_engage() {
        let entities = vec![ent(99, "Bee", EntityKind::Mob, 1.0, 0.0)];
        assert!(matches!(
            parse_slash("/attack", &entities, origin(), Some(99), None),
            SlashOutcome::Command(AgentCommand::Engage { target_id: 99 })
        ));
    }

    #[test]
    fn disengage_dispatches_cancel() {
        // MCP has no `disengage` tool — agents call `cancel` to stop the
        // engage goal. The slash command matches.
        assert!(matches!(
            parse_slash("/disengage", &empty_entities(), origin(), None, None),
            SlashOutcome::Command(AgentCommand::Cancel)
        ));
    }

    #[test]
    fn raw_attack_preserves_direct_action() {
        // Escape hatch for one-shot wire-level Attack.
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash("/raw attack", &entities, origin(), Some(7), None) {
            SlashOutcome::Command(AgentCommand::Action { kind, target_id, .. }) => {
                assert_eq!(target_id, 7);
                assert!(matches!(kind, ActionKind::Attack));
            }
            other => panic!("expected Action{{Attack}}, got {other:?}"),
        }
    }

    #[test]
    fn raw_attackoff_preserves_direct_action() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash("/raw attackoff", &entities, origin(), Some(7), None) {
            SlashOutcome::Command(AgentCommand::Action { kind, .. }) => {
                assert!(matches!(kind, ActionKind::AttackOff));
            }
            other => panic!("expected Action{{AttackOff}}, got {other:?}"),
        }
    }

    #[test]
    fn cast_with_explicit_target_and_ground_coords() {
        // `/cast 257 99 7 1.0 0.0 2.0` → Tractor (id=257) ground-targeted at (1,0,2).
        match parse_slash(
            "/cast 257 99 7 1.0 0.0 2.0",
            &empty_entities(),
            origin(),
            None,
            None,
        ) {
            SlashOutcome::Command(AgentCommand::Action {
                target_id,
                target_index,
                kind:
                    ActionKind::CastMagic {
                        spell_id,
                        pos_x,
                        pos_y,
                        pos_z,
                    },
            }) => {
                assert_eq!(spell_id, 257);
                assert_eq!(target_id, 99);
                assert_eq!(target_index, 7);
                assert_eq!(pos_x, 1.0);
                assert_eq!(pos_y, 0.0);
                assert_eq!(pos_z, 2.0);
            }
            other => panic!("expected CastMagic, got {other:?}"),
        }
    }

    #[test]
    fn cast_defaults_target_to_current() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash("/cast 1", &entities, origin(), Some(7), None) {
            SlashOutcome::Command(AgentCommand::Action {
                target_id,
                kind: ActionKind::CastMagic { spell_id, .. },
                ..
            }) => {
                assert_eq!(spell_id, 1);
                assert_eq!(target_id, 7);
            }
            other => panic!("expected CastMagic, got {other:?}"),
        }
    }

    #[test]
    fn weaponskill_basic() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash("/ws 16 7", &entities, origin(), Some(7), None) {
            SlashOutcome::Command(AgentCommand::Action {
                kind: ActionKind::Weaponskill { skill_id },
                target_id,
                ..
            }) => {
                assert_eq!(skill_id, 16);
                assert_eq!(target_id, 7);
            }
            other => panic!("expected Weaponskill, got {other:?}"),
        }
    }

    #[test]
    fn job_ability_defaults_to_zero_target() {
        match parse_slash("/ja 88", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::Action {
                target_id,
                kind: ActionKind::JobAbility { ability_id },
                ..
            }) => {
                assert_eq!(ability_id, 88);
                assert_eq!(target_id, 0);
            }
            other => panic!("expected JobAbility, got {other:?}"),
        }
    }

    #[test]
    fn useitem_basic() {
        match parse_slash("/useitem 0 4 4112", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::UseItem {
                container,
                slot,
                item_no,
                ..
            }) => {
                assert_eq!(container, 0);
                assert_eq!(slot, 4);
                assert_eq!(item_no, 4112);
            }
            other => panic!("expected UseItem, got {other:?}"),
        }
    }

    #[test]
    fn raisemenu_accept_and_decline() {
        for (input, expected) in &[("/raisemenu accept", true), ("/raisemenu decline", false)] {
            match parse_slash(input, &empty_entities(), origin(), None, None) {
                SlashOutcome::Command(AgentCommand::Action {
                    kind: ActionKind::RaiseMenu { accept },
                    ..
                }) => assert_eq!(accept, *expected, "input: {input}"),
                other => panic!("expected RaiseMenu, got {other:?}"),
            }
        }
    }

    #[test]
    fn tractormenu_accept_and_decline() {
        for (input, expected) in &[("/tractormenu y", true), ("/tractormenu n", false)] {
            match parse_slash(input, &empty_entities(), origin(), None, None) {
                SlashOutcome::Command(AgentCommand::Action {
                    kind: ActionKind::TractorMenu { accept },
                    ..
                }) => assert_eq!(accept, *expected, "input: {input}"),
                other => panic!("expected TractorMenu, got {other:?}"),
            }
        }
    }

    #[test]
    fn homepointmenu_parses_status_id() {
        match parse_slash("/homepointmenu 0", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::Action {
                kind: ActionKind::HomepointMenu { status_id },
                ..
            }) => assert_eq!(status_id, 0),
            other => panic!("expected HomepointMenu, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_is_direct() {
        assert!(matches!(
            parse_slash("/snapshot", &empty_entities(), origin(), None, None),
            SlashOutcome::Command(AgentCommand::Snapshot)
        ));
    }

    #[test]
    fn bank_parses_threshold_and_zoneline() {
        match parse_slash("/bank 60 0xDEADBEEF", &empty_entities(), origin(), None, None) {
            SlashOutcome::SystemMessage(_) => {
                // 0xDEADBEEF doesn't parse as plain u32; decimal works.
            }
            _ => {}
        }
        match parse_slash("/bank 60 12345", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::BankWhenFull {
                threshold,
                mog_house_zoneline,
            }) => {
                assert_eq!(threshold, 60);
                assert_eq!(mog_house_zoneline, 12345);
            }
            other => panic!("expected BankWhenFull, got {other:?}"),
        }
    }

    #[test]
    fn zonechange_parses_line_id() {
        match parse_slash("/zonechange 42", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::RequestZoneChange { line_id }) => {
                assert_eq!(line_id, 42);
            }
            other => panic!("expected RequestZoneChange, got {other:?}"),
        }
    }

    #[test]
    fn agent_pause_resume_status_parse() {
        for (input, expected) in &[
            ("/agent pause", AgentControlOp::Pause),
            ("/agent resume", AgentControlOp::Resume),
            ("/agent unpause", AgentControlOp::Resume),
            ("/agent status", AgentControlOp::Status),
            ("/agent", AgentControlOp::Status),
        ] {
            match parse_slash(input, &empty_entities(), origin(), None, None) {
                SlashOutcome::AgentControl(op) => assert_eq!(&op, expected, "input: {input}"),
                other => panic!("expected AgentControl for `{input}`, got {other:?}"),
            }
        }
    }

    #[test]
    fn agent_unknown_subcommand_is_system_message() {
        match parse_slash("/agent wat", &empty_entities(), origin(), None, None) {
            SlashOutcome::SystemMessage(s) => assert!(s.contains("wat")),
            other => panic!("expected SystemMessage, got {other:?}"),
        }
    }

    /// Consolidated lockstep verifier: for every MCP tool exposed by
    /// `ffxi-mcp/src/main.rs`, assert there is a slash twin that
    /// dispatches the right `AgentCommand` variant. When you add a new
    /// MCP tool, add a row here. If this test fails because of an
    /// extraneous row, remove the row. If a variant assertion fails,
    /// the slash parser drifted from the MCP tool.
    #[test]
    fn every_mcp_tool_has_a_slash_twin() {
        let entities = vec![ent(42, "Mob", EntityKind::Mob, 0.0, 0.0)];
        let pos = origin();
        let cur = Some(42);
        // (slash invocation, predicate that the dispatched command
        // matches the MCP tool's wire shape).
        let cases: Vec<(&str, fn(&AgentCommand) -> bool)> = vec![
            // follow {target_id, distance} — slash resolves names (or
            // current target); the entity at id=42 named "Mob" matches.
            ("/follow Mob", |c| matches!(c, AgentCommand::Follow { target_id: 42, .. })),
            // engage {target_id}
            ("/engage", |c| matches!(c, AgentCommand::Engage { target_id: 42 })),
            // path_to {x, y, z}
            ("/pathto 1 2 3", |c| matches!(c, AgentCommand::PathTo { .. })),
            // cancel
            ("/cancel", |c| matches!(c, AgentCommand::Cancel)),
            // bank_when_full {threshold, mog_house_zoneline}
            ("/bank 60 12345", |c| matches!(c, AgentCommand::BankWhenFull { threshold: 60, mog_house_zoneline: 12345 })),
            // chat {kind, text} — covered by per-channel slashes (/s /p etc.)
            ("/s hello", |c| matches!(c, AgentCommand::Chat { kind: 0, .. })),
            ("/p hello", |c| matches!(c, AgentCommand::Chat { kind: 4, .. })),
            // tell {to, text}
            ("/tell Bob hi", |c| matches!(c, AgentCommand::Tell { .. })),
            // request_zone_change {line_id}
            ("/zonechange 42", |c| matches!(c, AgentCommand::RequestZoneChange { line_id: 42 })),
            // snapshot
            ("/snapshot", |c| matches!(c, AgentCommand::Snapshot)),
            // cast (Action::CastMagic)
            ("/cast 1", |c| matches!(c,
                AgentCommand::Action { kind: ActionKind::CastMagic { .. }, .. })),
            // weaponskill (Action::Weaponskill)
            ("/ws 1", |c| matches!(c,
                AgentCommand::Action { kind: ActionKind::Weaponskill { .. }, .. })),
            // job_ability (Action::JobAbility)
            ("/ja 1", |c| matches!(c,
                AgentCommand::Action { kind: ActionKind::JobAbility { .. }, .. })),
            // use_item
            ("/useitem 0 4", |c| matches!(c, AgentCommand::UseItem { .. })),
            // raise_menu (Action::RaiseMenu)
            ("/raisemenu accept", |c| matches!(c,
                AgentCommand::Action { kind: ActionKind::RaiseMenu { .. }, .. })),
            // tractor_menu (Action::TractorMenu)
            ("/tractormenu accept", |c| matches!(c,
                AgentCommand::Action { kind: ActionKind::TractorMenu { .. }, .. })),
            // homepoint_menu (Action::HomepointMenu)
            ("/homepointmenu 0", |c| matches!(c,
                AgentCommand::Action { kind: ActionKind::HomepointMenu { .. }, .. })),
            // disconnect — slash form fires Quit (drops sockets); the
            // MCP tool dispatches AgentCommand::Disconnect through the
            // session loop. Both reach the same exit; the slash carries
            // the GUI side effect of closing the window.
        ];
        for (slash, pred) in &cases {
            let out = parse_slash(slash, &entities, pos, cur, None);
            match out {
                SlashOutcome::Command(ref cmd) => assert!(
                    pred(cmd),
                    "slash `{slash}` dispatched the wrong variant: {cmd:?}"
                ),
                SlashOutcome::Quit => {} // /disconnect path; intentional
                other => panic!("slash `{slash}` did not yield Command: {other:?}"),
            }
        }
    }

    #[test]
    fn help_command_returns_multiline_listing() {
        for slash in ["/help", "/?"] {
            let out = parse_slash(slash, &empty_entities(), origin(), None, None);
            match out {
                SlashOutcome::SystemMessage(s) => {
                    assert!(s.contains("Slash command reference"), "{slash} missing header");
                    // Each category header should be present.
                    for (category, _) in HELP_CATEGORIES {
                        assert!(
                            s.contains(category),
                            "{slash} output missing category `{category}`"
                        );
                    }
                    // Spot-check a couple of canonical commands.
                    assert!(s.contains("/follow"), "{slash} missing /follow");
                    assert!(s.contains("/help"), "{slash} missing /help self-reference");
                }
                other => panic!("expected SystemMessage from {slash}, got {other:?}"),
            }
        }
    }

    /// Drift guard: every alias listed in HELP_CATEGORIES must be a
    /// command the parser actually accepts. If someone removes a
    /// command from the match without updating the help table (or
    /// adds an entry with a typo), this fails. We check by asserting
    /// the parser does NOT return the "unknown command" SystemMessage
    /// — any other outcome (Command, Commands, SystemMessage with a
    /// usage line, ToggleNavmesh, etc.) counts as "known".
    #[test]
    fn help_entries_dispatch_known() {
        for (_, entries) in HELP_CATEGORIES {
            for entry in *entries {
                for alias in entry.aliases {
                    let slash = format!("/{alias}");
                    let out = parse_slash(&slash, &empty_entities(), origin(), None, None);
                    if let SlashOutcome::SystemMessage(ref s) = out {
                        assert!(
                            !s.starts_with("unknown command:"),
                            "help entry `/{alias}` is not accepted by parse_slash (drift)"
                        );
                    }
                }
            }
        }
    }
}
