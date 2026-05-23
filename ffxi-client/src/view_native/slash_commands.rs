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

use ffxi_viewer_core::{MenuKind, Preset};
use ffxi_viewer_wire::{Entity as WireEntity, Vec3 as WireVec3};

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
        "Help",
        &[HelpEntry {
            aliases: &["help", "?"],
            usage: "",
            summary: "show this slash-command reference",
        }],
    ),
    (
        "Movement & Navigation",
        &[
            HelpEntry {
                aliases: &["follow"],
                usage: "[name]",
                summary: "follow target or current selection",
            },
            HelpEntry {
                aliases: &["pathto"],
                usage: "<x> <y> [z] | <name> | target",
                summary: "pathfind: coords (z optional), fuzzy zone-line/entity, or current target",
            },
            HelpEntry {
                aliases: &["warp"],
                usage: "<x> <y> [z] | <name> | target",
                summary:
                    "debug teleport (Move): coords (z optional), fuzzy zone-line/entity, or target",
            },
            HelpEntry {
                aliases: &["zones"],
                usage: "",
                summary: "list zone-line destinations from current zone",
            },
            HelpEntry {
                aliases: &["zoneto"],
                usage: "<name|id>",
                summary: "pathfind to a zone-line (alias of `/pathto <name>`)",
            },
            HelpEntry {
                aliases: &["navmesh"],
                usage: "[on|off]",
                summary: "toggle the navmesh debug overlay",
            },
            HelpEntry {
                aliases: &["navinfo"],
                usage: "",
                summary: "report navmesh snap status at current position",
            },
            HelpEntry {
                aliases: &["whereami", "pos"],
                usage: "",
                summary: "print self position and zone id",
            },
            HelpEntry {
                aliases: &["return", "homepoint", "hp"],
                usage: "",
                summary: "warp to home point (alive or dead)",
            },
        ],
    ),
    (
        "Combat & Targeting",
        &[
            HelpEntry {
                aliases: &["attack", "engage"],
                usage: "[name]",
                summary: "engage target (reactor goal)",
            },
            HelpEntry {
                aliases: &["disengage"],
                usage: "",
                summary: "clear active reactor goal",
            },
            HelpEntry {
                aliases: &["attackoff"],
                usage: "",
                summary: "one-shot attack-off packet on current target",
            },
            HelpEntry {
                aliases: &["assist"],
                usage: "[name]",
                summary: "assist target (inherit their target)",
            },
            HelpEntry {
                aliases: &["target"],
                usage: "[name]",
                summary: "set or clear current target",
            },
            HelpEntry {
                aliases: &["debug", "dbg", "nearby", "entities"],
                usage: "[name|id|heights]",
                summary: "dump current target + nearby entities (or one entity in detail)",
            },
            HelpEntry {
                aliases: &["check", "checkname", "checkparam"],
                usage: "[name]",
                summary: "check target — strength / name / parameters",
            },
            HelpEntry {
                aliases: &["cast"],
                usage: "<spell> [target]",
                summary: "cast a spell",
            },
            HelpEntry {
                aliases: &["ws", "weaponskill"],
                usage: "<name> [target]",
                summary: "weapon skill",
            },
            HelpEntry {
                aliases: &["ja", "jobability"],
                usage: "<name> [target]",
                summary: "job ability",
            },
            HelpEntry {
                aliases: &["useitem", "use"],
                usage: "<name> [target]",
                summary: "use an item",
            },
            HelpEntry {
                aliases: &["magic"],
                usage: "",
                summary: "open the Magic menu (no-arg form; with args use /ma)",
            },
            HelpEntry {
                aliases: &["abilities", "abi"],
                usage: "",
                summary: "open the Abilities menu (no-arg form; with args use /ja)",
            },
            HelpEntry {
                aliases: &["items"],
                usage: "",
                summary: "open the Items menu (no-arg form; with args use /useitem)",
            },
            HelpEntry {
                aliases: &["equipment", "equip"],
                usage: "[slot item]",
                summary: "no-arg form opens Equipment menu; <slot> <item> equips directly (Stage 4)",
            },
            HelpEntry {
                aliases: &["cancel"],
                usage: "",
                summary: "cancel current reactor goal / action",
            },
            HelpEntry {
                aliases: &["raw"],
                usage: "<attack|attackoff> [name]",
                summary: "low-level Action packet (bypasses reactor)",
            },
        ],
    ),
    (
        "Chat",
        &[
            HelpEntry {
                aliases: &["s", "say"],
                usage: "<text>",
                summary: "say (local chat)",
            },
            HelpEntry {
                aliases: &["p", "party"],
                usage: "<text>",
                summary: "party chat",
            },
            HelpEntry {
                aliases: &["sh", "shout"],
                usage: "<text>",
                summary: "shout chat",
            },
            HelpEntry {
                aliases: &["l", "ls", "linkshell"],
                usage: "<text>",
                summary: "linkshell chat",
            },
            HelpEntry {
                aliases: &["t", "tell"],
                usage: "<name> <text>",
                summary: "tell another player",
            },
        ],
    ),
    (
        "Status & Menus",
        &[
            HelpEntry {
                aliases: &["sit", "kneel"],
                usage: "[on|off]",
                summary: "sit (locks movement; any movement key stands)",
            },
            HelpEntry {
                aliases: &["stand"],
                usage: "",
                summary: "stand (clear any rest stance)",
            },
            HelpEntry {
                aliases: &["heal"],
                usage: "[on|off]",
                summary: "toggle resting (CAMP)",
            },
            HelpEntry {
                aliases: &["raisemenu"],
                usage: "<option>",
                summary: "respond to raise dialog",
            },
            HelpEntry {
                aliases: &["tractormenu"],
                usage: "<option>",
                summary: "respond to tractor dialog",
            },
            HelpEntry {
                aliases: &["homepointmenu"],
                usage: "<option>",
                summary: "respond to homepoint dialog",
            },
            HelpEntry {
                aliases: &["endevent", "endevt", "clearevent", "clearevt"],
                usage: "",
                summary: "flush pending NPC events (unblock /logout)",
            },
            HelpEntry {
                aliases: &["endcutscene", "endcs", "skipcutscene", "skipcs"],
                usage: "[csid]",
                summary: "end a forced cutscene (new-char intro, etc.)",
            },
            HelpEntry {
                aliases: &["release", "unwedge"],
                usage: "",
                summary: "server-side !release: forcibly end any pinned event (gmlevel>=1)",
            },
            HelpEntry {
                aliases: &["buy"],
                usage: "<row> [qty]",
                summary: "buy from open shop by row index",
            },
            HelpEntry {
                aliases: &["bank"],
                usage: "<subcommand>",
                summary: "gil-bank operations",
            },
            HelpEntry {
                aliases: &["minimap", "mm"],
                usage: "[show|hide|toggle|mode <top|retail|auto>|cull <N>|zoom ...]",
                summary: "drive the minimap HUD (visibility, backend, cull, zoom)",
            },
            HelpEntry {
                aliases: &["sound", "audio", "mute"],
                usage: "[on|off|toggle|status] [bgm|sfx]",
                summary: "bare /sound toggles all audio; survives logout",
            },
        ],
    ),
    (
        "Session",
        &[
            HelpEntry {
                aliases: &["logout"],
                usage: "[on|off]",
                summary: "request logout (30s LeaveGame timer)",
            },
            HelpEntry {
                aliases: &["shutdown"],
                usage: "[on|off]",
                summary: "request shutdown (LeaveGame, then close)",
            },
            HelpEntry {
                aliases: &["exit"],
                usage: "",
                summary: "polite logout + close window",
            },
            HelpEntry {
                aliases: &["disconnect", "quit"],
                usage: "",
                summary: "drop the connection immediately",
            },
        ],
    ),
    (
        "Debug & Tooling",
        &[
            HelpEntry {
                aliases: &["snapshot"],
                usage: "",
                summary: "emit a one-shot scene snapshot",
            },
            HelpEntry {
                aliases: &["zonechange", "rzc"],
                usage: "<id>",
                summary: "request zone change (debug)",
            },
            HelpEntry {
                aliases: &["mhexit"],
                usage: "[home|1f|2f|garden|<region> [slot]]",
                summary: "leave the current Mog House (sends 0x05E zmrq)",
            },
            HelpEntry {
                aliases: &["agent"],
                usage: "<pause|resume|status>",
                summary: "human-in-control flag for agent commands",
            },
            HelpEntry {
                aliases: &["keybinds", "keybind", "binds"],
                usage: "<preset|list|reset>",
                summary: "manage keybind presets",
            },
            HelpEntry {
                aliases: &["load_mmb", "loadmmb"],
                usage: "<file_id> <chunk_idx>",
                summary: "spawn MMB model at self_pos (debug overlay)",
            },
            HelpEntry {
                aliases: &["load_mmb_on", "loadmmbon"],
                usage: "<entity_id> <file_id> <chunk_idx>",
                summary: "attach MMB model under a tracked entity (debug)",
            },
            HelpEntry {
                aliases: &["load_mzb", "loadmzb"],
                usage: "<file_id> [chunk_idx]",
                summary: "load MZB mesh-library at self_pos (debug overlay)",
            },
            HelpEntry {
                aliases: &["fps"],
                usage: "<max>",
                summary: "set target frame rate",
            },
            HelpEntry {
                aliases: &["capture"],
                usage: "[on|off|toggle]",
                summary:
                    "screen-capture-friendly mode (disables framepace; avoids QuickTime lockup)",
            },
            HelpEntry {
                aliases: &["screenshot", "ss"],
                usage: "[path.png]",
                summary: "capture primary window to PNG (default: screenshot-N.png in CWD)",
            },
            HelpEntry {
                aliases: &["drawdistance", "dd"],
                usage: "[setworld|setmob] [N]",
                summary: "set draw distance",
            },
            HelpEntry {
                aliases: &["copy"],
                usage: "[n]",
                summary: "copy the last n system-toast lines to the clipboard (default 1)",
            },
            HelpEntry {
                aliases: &["bgm"],
                usage: "<track_id>",
                summary: "audition a BGM track id (synthetic 0x05F slot 0)",
            },
            HelpEntry {
                aliases: &["sfx"],
                usage: "<se_id>",
                summary: "fire a one-shot SE by numeric id",
            },
            HelpEntry {
                aliases: &["look"],
                usage: "[name|act_index]",
                summary: "print decoded LookData (race/gear) for an entity",
            },
            HelpEntry {
                aliases: &["zonegeom"],
                usage: "[off|collision|all|toggle]",
                summary: "MZB overlay visibility (collision-only vs decorative)",
            },
            HelpEntry {
                aliases: &["weather"],
                usage: "<id|name>",
                summary:
                    "client-side weather override (e.g. `rain`, `none`, `12`); lasts until the next server WEATHER packet",
            },
            HelpEntry {
                aliases: &["devhud"],
                usage: "[on|off|toggle]",
                summary: "developer telemetry overlays (stage bar, agent goal, etc.)",
            },
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
    /// `/bgm <track_id>` — manually trigger BGM playback (useful for
    /// testing without a server-pushed 0x05F). Slot defaults to
    /// `ZoneDay` (0), which the priority ladder will play unless
    /// the user has already engaged combat.
    PlayBgm { track_id: u16 },
    /// `/sfx <se_id>` — fire a one-shot sound effect by numeric id.
    /// IDs come from the `dat-scan-sounds` example or the LSB
    /// SE-table research; values must resolve to a `.spw` under
    /// `sound{,2..15}/win/se/seNNN/seNNNNNN.spw`.
    PlaySfx { se_id: u32 },
    /// `/navmesh [on|off]` — flip the debug navmesh overlay. `None`
    /// means toggle (no arg given); `Some(b)` is an explicit set.
    /// Mirrors the `SetTarget(Option<u32>)` shape — a client-side
    /// state mutation, no agent/wire side effect.
    ToggleNavmesh(Option<bool>),
    /// `/sit` / `/kneel` — set or toggle the local
    /// [`RestStance`][rs] resource. Pure client-side affordance; the
    /// retail server has no "is sitting" packet, so the dispatcher
    /// just mutates the resource and the avatar animation system
    /// picks up the new pose next frame.
    ///
    /// `kind = Some(RestStanceKind::Sit)` enters sit, `Some(None)`
    /// stands, `None` (toggle) flips between the current `Sit`
    /// stance and `None` (any other stance becomes `Sit`). The
    /// `/heal` slash command does NOT route through this — it stays
    /// on `AgentCommand::Heal` so the server-side CAMP machinery
    /// runs, and the dispatcher mirrors `RestStance::Heal` on the
    /// outbound side.
    ///
    /// [rs]: ffxi_viewer_core::combat_stance::RestStance
    SetSitStance(SitToggle),
    /// `/keybinds preset|list|reset` — switch keybind preset, list the
    /// active map, or drop overrides back to the active preset's
    /// defaults. The dispatcher applies the change to the `Bindings`
    /// resource and persists via `keybinds_store`.
    ApplyKeybinds(KeybindUpdate),
    /// `/magic`, `/abilities`, `/items`, `/equipment` (no-arg forms) —
    /// open the corresponding retail-style action submenu. Mutates
    /// `InputMode` to `Menu(stack)` with `kind` pushed; the dispatcher
    /// in `apply_chat_action` overrides the default post-submit
    /// "back to World" with this menu push.
    ///
    /// The action-dispatch slash commands `/ma`, `/ja`, `/useitem`,
    /// `/equip <slot> <item>` (with args) are NOT this variant — they
    /// stay on the existing `Command(AgentCommand::Action { ... })`
    /// path so the agent / MCP twin behavior is unchanged.
    OpenMenu(MenuKind),
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
        /// When `Some`, parent the spawned mesh under the wire entity
        /// with this id rather than at `world_pos`. Set by
        /// `/load_mmb_on <entity_id> <file_id> <chunk_idx>`.
        entity_id: Option<u32>,
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
    /// `/zonegeom off|collision|all|toggle` — set MZB overlay visibility.
    /// `None` means cycle (toggle). The three render states are split so
    /// operators can hide the visually-noisy non-collision (decorative)
    /// meshes while keeping the LoS-blocking collision mesh visible.
    SetZoneGeom(Option<ffxi_viewer_core::dat_mzb::ZoneGeomMode>),
    /// `/devhud on|off|toggle` — show/hide the dev-only HUD widgets
    /// (stage bar, agent goal panel, MMB hover info, LLM badge,
    /// bf/sync/last/map/fps strip, [dbg] chat pane). Default off so the
    /// idle UI matches vanilla FFXI / Ashita / Windower-addon style;
    /// `on` reveals the operator telemetry for debugging.
    /// `None` means toggle.
    SetDevHud(Option<bool>),
    /// `/minimap [show|hide|toggle|mode <top|retail|auto>|cull <N>]`
    /// — drive the minimap HUD's visibility, image-source backend,
    /// and ceiling-cull height. See [`MinimapOp`].
    SetMinimap(MinimapOp),
    /// `/sound [on|off|toggle] [bgm|sfx]` — set or toggle audio
    /// mute. With no category, applies to both BGM and SFX. With a
    /// category, applies only to that side. Naked `/sound` reports
    /// status. See [`SoundOp`] for the full parsed shape.
    SetSound(SoundOp),
    /// `/fps <max>` — cap the render-loop framerate via `bevy_framepace`.
    /// `Some(n)` sets a target FPS (clamped >0); `None` (`/fps 0` or
    /// `/fps off`) disables the limiter. Pure client-side knob — no
    /// network side-effect — so the dispatcher mutates
    /// `bevy_framepace::FramepaceSettings` directly rather than
    /// routing through `cmd_tx`.
    SetTargetFps(Option<u32>),
    /// `/capture on|off|toggle` — opt into a screen-capture-friendly
    /// presentation profile. On macOS, QuickTime's legacy capture
    /// pipeline (and to a lesser extent other recorders) can deadlock a
    /// Bevy/wgpu Metal surface when the app's render loop is parked by
    /// `bevy_framepace` while the window server holds the surface for
    /// capture. Capture-on disables the framepace limiter and pins the
    /// primary window to `PresentMode::Fifo`; capture-off restores the
    /// prior limiter. `None` means toggle.
    SetCaptureMode(Option<bool>),
    /// `/debug heights` — diagnostic. Dumps player Bevy y, navmesh
    /// height, MZB-collision-mesh height (downward raycast), and the
    /// server-snapshot self pos. Dispatcher writes a `DebugHeightsRequest`
    /// event; the consumer system reads the navmesh + collision mesh
    /// resources and pushes a multi-line system toast.
    DebugHeights,
    /// `/screenshot [path]` — capture the primary window to a PNG.
    /// `None` selects an auto-numbered default (`screenshot-N.png` in
    /// CWD). Dispatcher writes a `ScreenshotRequest`; consumer system
    /// spawns Bevy's `Screenshot::primary_window()` with a `save_to_disk`
    /// observer.
    Screenshot { path: Option<String> },
    /// `/endcutscene [csid]` — send a 0x05B `EVENT_END` for a *forced*
    /// cutscene (the kind `player:startEvent` fires from `onZoneIn` —
    /// new-character openings, area-entry cinematics) that bypassed the
    /// normal client-side dialog flow and so isn't in
    /// `pending_event_end`. The dispatcher fills `unique_no`/`act_index`
    /// from the player's own `self_char_id` and `act_index`, since
    /// `player:startEvent` builds the EVENTSTART with those as the
    /// initiator.
    ///
    /// `event_num` is the cutscene id (CSID). `Some(n)` is an
    /// operator-supplied explicit CSID; `None` tells the dispatcher to
    /// resolve from session state (preferring the live `DialogState`,
    /// falling back to [`start_zone_cutscene`] for the start-nation
    /// forced cinematics). Punting CSID resolution to the dispatcher
    /// means the parser doesn't need access to `SceneState`.
    EndCutscene { event_num: Option<u16> },
    /// `/weather <id|name>` — client-side override. Dispatcher writes
    /// `Some(w)` into `SceneState.snapshot.weather`; the existing
    /// `sync_current_weather_from_snapshot` system propagates it to
    /// `CurrentWeather` next frame and the FX/HUD update from there.
    /// The next server-pushed WEATHER packet will overwrite it.
    SetWeatherClient(ffxi_viewer_wire::Weather),
    /// `/copy [n]` — copy the last `n` `[system]` chat toasts (i.e.
    /// responses to slash commands) to the OS clipboard, newline-joined.
    /// `n` defaults to 1 so a bare `/copy` grabs just the most recent
    /// response (the common case: read a result, copy it). Clamped at
    /// dispatch time to the number of toasts that actually exist —
    /// asking for more than we have isn't an error, it just copies what
    /// we've got.
    CopyToasts { n: usize },
}

/// New-character forced-cutscene CSIDs, keyed by start-zone id. Scraped
/// from `vendor/server/scripts/quests/hiddenQuests/New_Character_Cutscenes.lua`.
/// `xi.zone.*` ids match `ffxi_proto::zone_id`. Bastok Markets fires two
/// chained cutscenes (CSID 0 → 7); the operator picks one with the
/// explicit-arg form when the first end leaves the second wedged.
const START_ZONE_CUTSCENE: &[(u16, u16)] = &[
    // Bastok Markets — chain: 0 then 7. Default to 0; if still stuck,
    // `/endcutscene 7` clears the second.
    (235, 0),
    // Bastok Mines, Port Bastok
    (234, 1),
    (236, 1),
    // San d'Oria: Northern / Southern / Port
    (231, 535),
    (230, 503),
    (232, 500),
    // Windurst Waters / Woods / Port
    (238, 531),
    (241, 367),
    (240, 305),
];

/// Resolve a starting zone's forced-cutscene CSID, if known.
pub(crate) fn start_zone_cutscene(zone_id: u16) -> Option<u16> {
    START_ZONE_CUTSCENE
        .iter()
        .find_map(|&(z, csid)| (z == zone_id).then_some(csid))
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

/// `/minimap` subcommand variants. Drives the minimap HUD —
/// visibility, background-image backend, and the top-down ceiling-cull
/// height. See `ffxi_viewer_core::minimap` for what each field maps to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MinimapOp {
    /// `/minimap` (no arg) — print current mode + visibility + cull.
    Status,
    /// `/minimap show` — force visible.
    Show,
    /// `/minimap hide` — force hidden.
    Hide,
    /// `/minimap toggle` — flip visibility.
    Toggle,
    /// `/minimap mode top` — pin the TopDown bake as the background.
    ModeTopDown,
    /// `/minimap mode retail` — pin the retail stylized image (no-op
    /// until the parser lands; the dispatcher emits a system message
    /// when no retail image is loaded for the current zone).
    ModeRetail,
    /// `/minimap mode auto` — let the resolved-mode picker choose
    /// (retail when available, else top-down).
    ModeAuto,
    /// `/minimap cull <N>` — set TopdownCullPolicy.top_cull_yalms.
    /// Triggers an immediate re-bake.
    SetCull(f32),
    /// `/minimap zoom in` — discrete zoom-in tick (same as `.` over
    /// minimap).
    ZoomIn,
    /// `/minimap zoom out` — discrete zoom-out tick.
    ZoomOut,
    /// `/minimap zoom fit` — show the whole zone (max zoom-out).
    ZoomFit,
    /// `/minimap zoom <N>` — set radius to N yalms (clamped to the
    /// allowed range).
    ZoomSet(f32),
    /// `/minimap zoom reset` — back to defaults (50 yalm radius,
    /// pan cleared).
    ZoomReset,
}

/// `/sound` subcommand variants. Operator-facing toggle for the
/// audio mute state — both BGM and SFX, or one independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundOp {
    /// `/sound status` (or `/sound ?`) — report current mute state
    /// for both categories. No mutation. Bare `/sound` is the toggle
    /// shortcut and dispatches `SetBoth(None)`, not this variant.
    Status,
    /// `/sound on` / `/sound off` / `/sound toggle` — apply to both
    /// BGM and SFX. The boolean is the target value (`true` =
    /// muted, `false` = audible); `None` means flip whatever the
    /// current state is.
    SetBoth(Option<bool>),
    /// `/sound bgm [on|off|toggle]` — apply only to BGM. Same
    /// `Option<bool>` semantics as [`SetBoth`].
    SetBgm(Option<bool>),
    /// `/sound sfx [on|off|toggle]` — apply only to SFX.
    SetSfx(Option<bool>),
}

/// `/sit` / `/kneel` argument variants. The parser turns the user's
/// text into one of these; the dispatcher applies it to the
/// [`RestStance`][rs] resource.
///
/// [rs]: ffxi_viewer_core::combat_stance::RestStance
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SitToggle {
    /// Bare `/sit` or `/kneel` — flip Sit on/off. (`Heal` → `Sit`
    /// when toggled, mirroring how retail's `/sit` while healing
    /// stands you up out of CAMP and then sits.)
    Toggle,
    /// `/sit on` — explicitly enter Sit.
    On,
    /// `/sit off` or `/stand` — explicitly clear any rest stance.
    Off,
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
    self_char_id: Option<u32>,
    party: &[ffxi_viewer_wire::PartyMember],
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
                Some((id, _idx)) => SlashOutcome::Command(AgentCommand::Engage { target_id: id }),
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
        // Retail `/targetnpc`: cycle the nearest non-PC by 3D distance.
        // Pet entities included (trusts/jugs/avatars are addressable as
        // "NPCs" in the retail sense). `/targetnpc2` is the reverse-
        // direction variant — same pool, walked backward.
        "targetnpc" | "targetnpc2" => {
            let reverse = cmd == "targetnpc2";
            let kinds = [
                ffxi_viewer_wire::EntityKind::Npc,
                ffxi_viewer_wire::EntityKind::Mob,
                ffxi_viewer_wire::EntityKind::Pet,
            ];
            match cycle_kind_filtered(entities, self_pos, current_target, &kinds, reverse) {
                Some(id) => SlashOutcome::SetTarget(Some(id)),
                None => SlashOutcome::SystemMessage(format!("/{cmd}: no NPC nearby")),
            }
        }
        // Retail `/targetenemy`: mobs only. Skips friendly NPCs even if
        // closer, so the operator can punch through a vendor crowd to
        // grab the aggro target.
        "targetenemy" => {
            let kinds = [ffxi_viewer_wire::EntityKind::Mob];
            match cycle_kind_filtered(entities, self_pos, current_target, &kinds, false) {
                Some(id) => SlashOutcome::SetTarget(Some(id)),
                None => SlashOutcome::SystemMessage("/targetenemy: no enemy nearby".into()),
            }
        }
        // Retail `/targetnpcparty`: trusts/fellows the player owns.
        // Best-effort filter — claim_id == self_char_id picks up pets
        // and trusts the server attributes to the player. If LSB uses
        // a different signal for fellow NPCs, this gets refined later.
        "targetnpcparty" => {
            let owner = self_char_id.unwrap_or(0);
            let owned: Vec<&WireEntity> = entities
                .iter()
                .filter(|e| {
                    matches!(e.kind, ffxi_viewer_wire::EntityKind::Pet) && e.claim_id == owner
                })
                .collect();
            // Reuse the same nearest+cycle pattern, but on a pre-filtered list.
            let ids: Vec<u32> = {
                let mut sorted = owned.clone();
                sorted.sort_by(|a, b| {
                    let da = sq_dist(a.pos, self_pos);
                    let db = sq_dist(b.pos, self_pos);
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                });
                sorted.into_iter().map(|e| e.id).collect()
            };
            if ids.is_empty() {
                SlashOutcome::SystemMessage("/targetnpcparty: no party NPCs".into())
            } else {
                let next = match current_target.and_then(|id| ids.iter().position(|x| *x == id)) {
                    Some(i) => ids[(i + 1) % ids.len()],
                    None => ids[0],
                };
                SlashOutcome::SetTarget(Some(next))
            }
        }
        // Retail `/targetparty1..6`: slot 1 = self, slots 2..6 = party
        // members in insertion order (the best the wire gives us — the
        // server's actual slot index isn't broadcast separately, so a
        // freshly-joined member may "shift" pre-existing ones until the
        // next party refresh).
        "targetparty1" | "targetparty2" | "targetparty3" | "targetparty4" | "targetparty5"
        | "targetparty6" => {
            let slot = cmd
                .strip_prefix("targetparty")
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or(0);
            match resolve_party_slot(slot, self_char_id, party) {
                Some(id) => SlashOutcome::SetTarget(Some(id)),
                None => SlashOutcome::SystemMessage(format!("/{cmd}: slot empty")),
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
        // `/sit` / `/kneel` — retail aliases for the sit rest pose. Pure
        // client-side affordance: lock the local self avatar in the
        // sit MO2 and refuse to emit outbound `Move` packets until a
        // movement key clears the stance (retail-style stand-on-first-
        // input). `/sit on` / `/sit off` are explicit setters; bare
        // `/sit` toggles.
        "sit" | "kneel" => parse_sit(rest),
        "stand" => SlashOutcome::SetSitStance(SitToggle::Off),
        "bgm" => match rest.parse::<u16>() {
            Ok(id) => SlashOutcome::PlayBgm { track_id: id },
            Err(_) => SlashOutcome::SystemMessage("/bgm <track_id>".into()),
        },
        "sfx" => match rest.parse::<u32>() {
            Ok(id) => SlashOutcome::PlaySfx { se_id: id },
            Err(_) => SlashOutcome::SystemMessage("/sfx <se_id>".into()),
        },
        "cancel" => SlashOutcome::Command(AgentCommand::Cancel),
        // ---- Stage 4 lockstep: every MCP tool has a slash twin -----------
        // The MCP `cast` / `weaponskill` / `job_ability` tools wrap the same
        // `Action { ActionKind::* }` shape — slash twins dispatch the
        // identical AgentCommand. `slash_mcp_lockstep.rs` pins this.
        "cast" => parse_cast(rest, entities, self_pos, current_target),
        "ws" | "weaponskill" => parse_weaponskill(rest, entities, self_pos, current_target),
        "ja" | "jobability" => parse_job_ability(rest, entities, self_pos, current_target),
        "useitem" | "use" => parse_use_item(rest, entities, self_pos, current_target),
        // Retail-style "open the X menu" no-arg slashes. The action
        // forms (cast / ja / useitem above) already cover the
        // <name> [target] use case; these just push the visible menu.
        "magic" if rest.is_empty() => SlashOutcome::OpenMenu(MenuKind::Magic),
        "abilities" | "abi" if rest.is_empty() => SlashOutcome::OpenMenu(MenuKind::Abilities),
        "items" if rest.is_empty() => SlashOutcome::OpenMenu(MenuKind::Items),
        // `/equip` (no args) opens the Equipment menu. `/equip <slot> <item>`
        // is reserved for the direct-equip action wired in Stage 4; until
        // then, equip-with-args falls through to "not yet wired".
        "equipment" if rest.is_empty() => SlashOutcome::OpenMenu(MenuKind::Equipment),
        "equip" if rest.is_empty() => SlashOutcome::OpenMenu(MenuKind::Equipment),
        "equip" => SlashOutcome::SystemMessage(
            "/equip <slot> <item>: not yet wired (Stage 4); /equip with no args opens the Equipment menu".into(),
        ),
        "raisemenu" => parse_raise_menu(rest),
        "tractormenu" => parse_tractor_menu(rest),
        "homepointmenu" => parse_homepoint_menu(rest),
        "snapshot" => SlashOutcome::Command(AgentCommand::Snapshot),
        // `/endevent` — flush every `pending_event_end` entry as a 0x05B
        // EVENT_END subpacket. Session loop drains the list
        // (`session.rs::AgentCommand::EndEvent`); we just provide a slash
        // surface. Primary use: clear `BlockedState::InEvent` so /logout
        // can succeed. No-op when nothing is pending.
        "endevent" | "endevt" | "clearevent" | "clearevt" => {
            SlashOutcome::Command(AgentCommand::EndEvent)
        }
        // `/endcutscene [csid]` — escape a forced cutscene the client
        // never registered in `pending_event_end` (new-character
        // openings, `onZoneIn` cinematics). Without an arg the parser
        // tries to look up the CSID from the current zone; with one it
        // trusts the operator's value.
        "endcutscene" | "endcs" | "skipcutscene" | "skipcs" => parse_endcutscene(rest),
        // `/release` (alias `/unwedge`) — emit the LSB GM command
        // `!release` as chat. Server-side `release.lua` calls
        // `player:release()` which runs `endCurrentEvent()` and clears
        // `currentEvent`, unblocking `BlockedState::InEvent`. Requires
        // `gmlevel >= 1`. Use as the heavy hammer when `/endcutscene`
        // can't resolve the right CSID (e.g. in-session events the
        // client never saw because of a packet drop). The server
        // intercepts `!`-prefixed chat *before* it routes to Say, so
        // this doesn't leak the command into the chat channel.
        "release" | "unwedge" => SlashOutcome::Command(AgentCommand::Chat {
            kind: 0,
            text: "!release".into(),
        }),
        // `/weather <id|name>` — client-side weather override. Writes
        // directly to the local snapshot/`CurrentWeather` resource so
        // the visual FX (particles, fog, sun modulation) flip
        // immediately without a server round-trip. No GM requirement,
        // and `none` actually clears (the LSB `!setweather` early-
        // returns when the zone's already at the requested weather).
        // Naturally reverts on the next server 0x057 WEATHER packet —
        // same lifetime as the GM command had server-side.
        "weather" => parse_weather(rest),
        "bank" => parse_bank(rest),
        "zonechange" | "rzc" => parse_zone_change(rest),
        "mhexit" => parse_mhexit(rest, zone_id),
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
        "load_mmb_on" | "loadmmbon" => parse_load_mmb_on(rest),
        "load_mzb" | "loadmzb" => parse_load_mzb(rest, self_pos),
        "look" => parse_look(rest, entities, self_pos, current_target),
        "fps" => parse_fps(rest),
        "capture" => parse_capture(rest),
        "screenshot" | "ss" => parse_screenshot(rest),
        "drawdistance" | "dd" => parse_drawdistance(rest),
        "zonegeom" => parse_zonegeom(rest),
        "devhud" => parse_devhud(rest),
        "minimap" | "mm" => parse_minimap(rest),
        "sound" | "audio" | "mute" => parse_sound(rest),
        "debug" | "dbg" | "nearby" | "entities" => {
            parse_debug(rest, entities, self_pos, current_target)
        }
        "keybinds" | "keybind" | "binds" => parse_keybinds(rest),
        "pathto" => parse_pathto(rest, entities, self_pos, current_target, zone_id),
        "warp" => parse_warp(rest, entities, self_pos, current_target, zone_id),
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
        "copy" => {
            // `/copy` => n=1; `/copy N` => n=N (positive integer).
            // Anything else falls through to a parse-error toast rather
            // than silently copying the wrong amount.
            if rest.is_empty() {
                SlashOutcome::CopyToasts { n: 1 }
            } else {
                match rest.parse::<usize>() {
                    Ok(n) if n > 0 => SlashOutcome::CopyToasts { n },
                    _ => SlashOutcome::SystemMessage(format!(
                        "/copy: expected a positive integer, got `{rest}`"
                    )),
                }
            }
        }
        "help" | "?" => SlashOutcome::SystemMessage(render_help()),
        "" => SlashOutcome::SystemMessage("empty command".into()),
        unknown => SlashOutcome::SystemMessage(format!("unknown command: /{unknown}")),
    }
}

// Chat-line constructors live in `ffxi-viewer-core` so both client-side
// slash commands and viewer-core engine systems (audio, skybox, sun/moon,
// weather, vana clock) can emit them. Re-exported here so existing callers
// (`text_input.rs`, `screenshot.rs`) keep importing from this module.
pub use ffxi_viewer_core::snapshot::{debug_chat_line, system_chat_line};

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
/// `/sit [on|off|toggle]` — parses the SitToggle variant for the
/// rest-stance dispatcher. Bare `/sit` (or `/kneel`) toggles; unknown
/// args produce a system message.
fn parse_sit(rest: &str) -> SlashOutcome {
    let arg = rest.trim().to_ascii_lowercase();
    let toggle = match arg.as_str() {
        "" | "toggle" => SitToggle::Toggle,
        "on" => SitToggle::On,
        "off" => SitToggle::Off,
        other => {
            return SlashOutcome::SystemMessage(format!(
                "/sit: usage `/sit [on|off]` (got `{other}`)"
            ));
        }
    };
    SlashOutcome::SetSitStance(toggle)
}

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

/// `/pathto` — navmesh-aware `AgentCommand::PathTo`. Accepts:
///   - `/pathto <x> <y> [z]` — explicit coords. `z` is the FFXI-native
///     vertical axis and defaults to the player's current `self_pos.z`
///     when omitted (operator typically wants to stay on the same
///     vertical plane and let the pathfinder snap as it walks).
///   - `/pathto target` — pull coords from the current target.
///   - `/pathto <name>` — fuzzy match against zone-line destinations
///     in the current zone, then entity names. Subsumes the old
///     `/zoneto`, which is kept as an alias.
fn parse_pathto(
    rest: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
    zone_id: Option<u16>,
) -> SlashOutcome {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return SlashOutcome::SystemMessage(
            "/pathto: usage `/pathto <x> <y> [z]` | `/pathto <name>` | `/pathto target`".into(),
        );
    }
    match parse_goto_target(
        trimmed,
        entities,
        self_pos,
        current_target,
        zone_id,
        "/pathto",
    ) {
        Ok(pos) => SlashOutcome::Command(AgentCommand::PathTo {
            x: pos.x,
            y: pos.y,
            z: pos.z,
        }),
        Err(msg) => SlashOutcome::SystemMessage(msg),
    }
}

/// `/warp` — debug teleport. Emits `AgentCommand::Move`, which rewrites
/// the next keepalive's reported position (and heading) directly.
/// Bypasses navmesh and pathing entirely: useful for poking at edge
/// cases like out-of-mesh spots, scripted positions, or stuck-recovery.
///
/// The server's anti-speedhack guard will reject or cap obvious jumps
/// (see `[[reactor_speed_safety]]` — outbound Move suppressed at
/// speed=0, scaled by speed ratio); that's the operator-visible
/// signal that a warp didn't take.
///
/// Accepts the same surface as [`parse_pathto`]: coords (z optional),
/// `target`, or a fuzzy `<name>` matched against zone-lines + entities.
/// Heading is carried over from the player's current entity row so
/// the warp doesn't spin the camera north.
fn parse_warp(
    rest: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
    zone_id: Option<u16>,
) -> SlashOutcome {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return SlashOutcome::SystemMessage(
            "/warp: usage `/warp <x> <y> [z]` | `/warp <name>` | `/warp target`".into(),
        );
    }
    match parse_goto_target(
        trimmed,
        entities,
        self_pos,
        current_target,
        zone_id,
        "/warp",
    ) {
        Ok(pos) => SlashOutcome::Command(AgentCommand::Move {
            x: pos.x,
            y: pos.y,
            z: pos.z,
            heading: self_heading(entities, self_pos),
        }),
        Err(msg) => SlashOutcome::SystemMessage(msg),
    }
}

/// Shared parser for the `/pathto` / `/warp` argument surface. Returns
/// the resolved FFXI-world position, or a usage/error string keyed to
/// the caller's command name (so the toast says `/warp: …` vs
/// `/pathto: …`).
///
/// Resolution order:
///   1. `target` — current target's pos (fails if no target).
///   2. 2-or-3 numeric tokens — explicit coords. 2 args means
///      `<x> <y>` with `z` defaulting to `self_pos.z`.
///   3. Anything else — fuzzy name. See [`resolve_position_needle`].
fn parse_goto_target(
    trimmed: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
    zone_id: Option<u16>,
    cmd_label: &str,
) -> Result<WireVec3, String> {
    if trimmed.eq_ignore_ascii_case("target") {
        let id = current_target.ok_or_else(|| format!("{cmd_label}: no target"))?;
        let ent = entities
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| format!("{cmd_label}: target despawned"))?;
        return Ok(ent.pos);
    }
    let parts: Vec<&str> = trimmed.split_ascii_whitespace().collect();
    // Numeric form: 2 or 3 floats. We only treat it as numeric if every
    // token parses cleanly — otherwise fall through to fuzzy match.
    // (Zone / entity names are single tokens, so a token-count guard
    // would mis-trigger here on multi-word names — parse-everything
    // is the cleaner pivot.)
    if (parts.len() == 2 || parts.len() == 3) && parts.iter().all(|p| p.parse::<f32>().is_ok()) {
        let v: Vec<f32> = parts.iter().map(|p| p.parse::<f32>().unwrap()).collect();
        let z = if v.len() == 3 { v[2] } else { self_pos.z };
        return Ok(WireVec3 {
            x: v[0],
            y: v[1],
            z,
        });
    }
    // Fuzzy name resolution (zone-line + entity).
    resolve_position_needle(trimmed, entities, self_pos, zone_id)
        .map(|(pos, _label)| pos)
        .ok_or_else(|| format!("{cmd_label}: no match for `{trimmed}` (try `/zones` or `/debug`)"))
}

/// Player's current heading, looked up from the self entity. Used by
/// [`parse_warp`] to avoid forcing a heading-reset on teleport. We
/// identify "self" by `pos == self_pos` (the same convention as
/// `render_debug_nearby` — see `state.rs::self_position`). Falls back
/// to `0` if the self entity isn't yet in the snapshot (very early
/// boot only).
fn self_heading(entities: &[WireEntity], self_pos: WireVec3) -> u8 {
    entities
        .iter()
        .find(|e| e.pos == self_pos)
        .map(|e| e.heading)
        .unwrap_or(0)
}

/// Fuzzy-match a needle against zone-line destinations (current zone)
/// AND entity names, in that priority order. Returns `(pos, label)`
/// where `label` describes what matched — currently unused by callers
/// (toast support is deferred) but kept so logging and future
/// toasting have a stable hook.
///
/// Priority: zone-line first. Rationale: `/zoneto` is the established
/// "by destination" command and we want a strict superset of its
/// behavior. Entity match is the additive surface this resolver
/// introduces. Operators can target a specific entity with the
/// `target` form when they want to skip the zone-line path.
fn resolve_position_needle(
    needle: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    zone_id: Option<u16>,
) -> Option<(WireVec3, String)> {
    if let Some(z) = zone_id {
        let lines = ffxi_nav::zone_lines_for(z);
        if !lines.is_empty() {
            let by_id = needle.parse::<u16>().ok().filter(|n| *n <= MAX_ZONE_ID);
            let needle_lower = needle.to_ascii_lowercase();
            let hit = lines.iter().find(|line| {
                if Some(line.to_zone) == by_id {
                    return true;
                }
                ffxi_nav::zone_name(line.to_zone)
                    .map(|n| n.to_ascii_lowercase().starts_with(&needle_lower))
                    .unwrap_or(false)
            });
            if let Some(line) = hit {
                let pos = WireVec3 {
                    x: line.from_pos[0],
                    y: line.from_pos[1],
                    z: line.from_pos[2],
                };
                let label = ffxi_nav::zone_name(line.to_zone)
                    .map(|n| format!("zone-line → {n} ({})", line.to_zone))
                    .unwrap_or_else(|| format!("zone-line → zone {}", line.to_zone));
                return Some((pos, label));
            }
        }
    }
    let ent = resolve_name(needle, entities, self_pos)?;
    let kind = kind_tag(ent.kind);
    let name = ent.name.as_deref().unwrap_or("?");
    Some((ent.pos, format!("{kind} {name}")))
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
        "" => SlashOutcome::SystemMessage("/raw: usage `/raw attack|attackoff [target]`".into()),
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
            return SlashOutcome::SystemMessage(format!("/cast: bad spell_id `{}`", parts[0]));
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
        Err(_) => {
            return SlashOutcome::SystemMessage(format!("/ja: bad ability_id `{}`", parts[0]))
        }
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
        Err(_) => {
            return SlashOutcome::SystemMessage(format!("/useitem: bad container `{}`", parts[0]))
        }
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
        "" => {
            SlashOutcome::SystemMessage("/tractormenu: usage `/tractormenu accept|decline`".into())
        }
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
        Err(_) => SlashOutcome::SystemMessage(format!("/homepointmenu: bad status_id `{trimmed}`")),
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
            return SlashOutcome::SystemMessage(format!("/bank: bad threshold `{}`", parts[0]));
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
/// `/endcutscene [csid]` — resolve the CSID and produce a
/// `SlashOutcome::EndCutscene`. The dispatcher in `text_input.rs` fills
/// in the player's own `unique_no`/`act_index` since those aren't in
/// the parser's args, and resolves a `None` `event_num` against the
/// live `DialogState` / start-zone fallback before sending.
///
/// - With no arg: emits `event_num = None`. The dispatcher prefers the
///   live `DialogState.event_para` (the CSID the server is currently
///   pinning) and falls back to [`start_zone_cutscene`] for the
///   forced new-character cinematics. This avoids the EventPara=0
///   mismatch class the parser used to produce when the zone-table
///   guess didn't match the actual pinned event (e.g. CSID 11002
///   Synergy Engineer on top of a start zone).
/// - With an arg: parse it as `u16` and trust the operator. Useful for
///   the Bastok Markets chain (CSID 0 → 7; default clears 0, then
///   `/endcutscene 7` clears the second one).
fn parse_endcutscene(rest: &str) -> SlashOutcome {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return SlashOutcome::EndCutscene { event_num: None };
    }
    match trimmed.parse::<u16>() {
        Ok(n) => SlashOutcome::EndCutscene { event_num: Some(n) },
        Err(_) => SlashOutcome::SystemMessage(format!(
            "/endcutscene: bad CSID `{trimmed}` (expected u16)"
        )),
    }
}

fn parse_zone_change(rest: &str) -> SlashOutcome {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return SlashOutcome::SystemMessage("/zonechange: usage `/zonechange <line_id>`".into());
    }
    match trimmed.parse::<u32>() {
        Ok(line_id) => SlashOutcome::Command(AgentCommand::RequestZoneChange { line_id }),
        Err(_) => SlashOutcome::SystemMessage(format!("/zonechange: bad line_id `{trimmed}`")),
    }
}

/// `/mhexit [home|1f|2f|garden|<region> [slot]]` — send the universal
/// Mog House exit packet (`0x05E MAPRECT` with `RectID="zmrq"`).
///
/// No-arg defaults to `MogHouseExit::Home`, which is the safe "step back
/// out the door" exit (matches `MyRoomExitMode::AreaEnteredFrom` on the
/// server; bit is ignored). Other forms:
///   - `home` — same as no-arg
///   - `1f` / `2f` — relocate inside the Mog House
///   - `garden` — zone to Mog Garden
///   - `<region> [slot]` — alternate-city exit (requires the matching
///     quest flag). Regions: `sandoria`, `bastok`, `windurst`, `jeuno`,
///     `whitegate`, `adoulin`. `slot` defaults to 1 (the home zone in
///     that region).
fn parse_mhexit(rest: &str, zone_id: Option<u16>) -> SlashOutcome {
    let trimmed = rest.trim().to_ascii_lowercase();
    let mut parts = trimmed.split_whitespace();
    let first = parts.next();
    let slot: u8 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);

    let kind = match first {
        None | Some("home") | Some("") => crate::state::MogHouseExit::Home,
        Some("1f") | Some("mog1f") => crate::state::MogHouseExit::Mog1F,
        Some("2f") | Some("mog2f") => crate::state::MogHouseExit::Mog2F,
        Some("garden") | Some("moggarden") => crate::state::MogHouseExit::MogGarden,
        Some("sandoria") | Some("sandy") => crate::state::MogHouseExit::Sandoria { slot },
        Some("bastok") => crate::state::MogHouseExit::Bastok { slot },
        Some("windurst") | Some("windy") => crate::state::MogHouseExit::Windurst { slot },
        Some("jeuno") => crate::state::MogHouseExit::Jeuno { slot },
        Some("whitegate") | Some("aht_urhgan") => crate::state::MogHouseExit::Whitegate { slot },
        Some("adoulin") => crate::state::MogHouseExit::Adoulin { slot },
        Some("auto") => {
            // `/mhexit auto` — look up which region's Mog House we're in
            // from `zone_id` and dispatch the matching `Option1` slot
            // (= the city we entered the mog house from). Useful as a
            // default when the operator doesn't remember the region
            // name; falls back to `home` if the zone isn't in the table.
            match zone_id.and_then(home_region_bit_for_zone) {
                Some(bit) => bit_and_slot_to_exit(bit, 1),
                None => {
                    return SlashOutcome::SystemMessage(format!(
                        "/mhexit auto: zone {} isn't in the home-region table — \
                         use `/mhexit home` or pass a region name explicitly",
                        zone_id.map_or("unknown".into(), |z| z.to_string()),
                    ));
                }
            }
        }
        Some(other) => {
            return SlashOutcome::SystemMessage(format!(
                "/mhexit: unknown form `{other}` — try home|1f|2f|garden|\
                 sandoria|bastok|windurst|jeuno|whitegate|adoulin|auto"
            ));
        }
    };
    SlashOutcome::Command(AgentCommand::MogHouseExit { kind })
}

/// Map a `MyRoomExitBit` value (1=Sandoria, 2=Bastok, …, 9=Adoulin) +
/// `slot` (1..4) to the corresponding [`MogHouseExit`] variant. Kept
/// separate from `home_region_bit_for_zone` so the auto-detect can stay
/// tiny — the heavy lifting (which zones are in which region) lives in
/// that single table.
fn bit_and_slot_to_exit(bit: u8, slot: u8) -> crate::state::MogHouseExit {
    use crate::state::MogHouseExit;
    match bit {
        1 => MogHouseExit::Sandoria { slot },
        2 => MogHouseExit::Bastok { slot },
        3 => MogHouseExit::Windurst { slot },
        4 => MogHouseExit::Jeuno { slot },
        5 => MogHouseExit::Whitegate { slot },
        9 => MogHouseExit::Adoulin { slot },
        _ => MogHouseExit::Home,
    }
}

/// Map a current `zone_id` to the `MyRoomExitBit` whose region contains it.
///
/// Numeric ids come from `vendor/server/src/map/zone.h` (the LSB `ZONETYPE`
/// enum); the region partition mirrors
/// `vendor/server/src/map/utils/zoneutils.cpp::GetCurrentRegion`; the
/// `MyRoomExitBit` codes are
/// `vendor/server/src/map/packets/c2s/0x05e_maprect.h::GP_CLI_COMMAND_MAPRECT_MYROOMEXITBIT`.
///
/// Only the six regions that have Mog Houses appear here; everything else
/// returns `None` (so `/mhexit auto` errors out cleanly). `Home` (no-arg
/// `/mhexit`) works from any Mog House without consulting this table —
/// the server treats `MyRoomExitMode::AreaEnteredFrom` as "walk back out
/// the way I came in," which doesn't need a region.
///
/// Note the Whitegate row: vendor `zone.h:48,50` gives Al Zahbi=48 and
/// Aht Urhgan Whitegate=50 (id 49 is `ZONE_HALL_OF_BINDING`, which is
/// *not* in the West-Aht-Urhgan region).
fn home_region_bit_for_zone(zone_id: u16) -> Option<u8> {
    match zone_id {
        230..=233 => Some(1), // Sandoria: S.Sandy/N.Sandy/Port/Chateau
        234..=237 => Some(2), // Bastok: Mines/Markets/Port/Metalworks
        238..=242 => Some(3), // Windurst: Waters/Walls/Port/Woods/Heavens Tower
        243..=246 => Some(4), // Jeuno: Ru'Lude/Upper/Lower/Port
        48 | 50 => Some(5),   // West Aht Urhgan: Al Zahbi, Whitegate
        256 | 257 => Some(9), // Adoulin: Western, Eastern
        _ => None,
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
            Some(t) => t.parse().map_err(|_| format!("bad target_index `{t}`"))?,
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
            to_name, line.to_zone, line.from_pos[0], line.from_pos[1], line.from_pos[2],
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
/// `/debug <subcommand>` — namespaced diagnostics. Currently:
///   - `heights` — dump player/navmesh/MZB collision heights at the
///     player's XZ. Used to diagnose the navmesh-vs-MZB vertical gap.
/// `/debug` family — diagnostic dumps for the entity/target/look surface.
///
/// Subcommands:
///   - `/debug heights` (alias `h`) — height-stack overlay (Bevy y vs
///     navmesh vs MZB ground); existing zonegeom-adjacent debug.
///   - `/debug` (no args) — dump the current target plus the 10 nearest
///     entities. Use to diagnose target/look/model issues without
///     leaving the world view.
///   - `/debug <name|id|act_idx>` — single-entity detail dump. Same
///     resolution rules as `/look` / `/target`.
fn parse_debug(
    rest: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
) -> SlashOutcome {
    let trimmed = rest.trim();
    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        "heights" | "h" => SlashOutcome::DebugHeights,
        "" => SlashOutcome::SystemMessage(render_debug_nearby(entities, self_pos, current_target)),
        // Argument that isn't a known subcommand → treat as entity lookup
        // (name prefix, decimal id, or act_index). Detail dump.
        _ => SlashOutcome::SystemMessage(render_debug_entity(trimmed, entities, self_pos)),
    }
}

/// One-letter summary of an `EntityLook` for the wide nearby table.
fn look_tag(look: Option<&ffxi_viewer_wire::EntityLook>) -> &'static str {
    use ffxi_viewer_wire::EntityLook;
    match look {
        None => "--",
        Some(EntityLook::Standard { .. }) => "std",
        Some(EntityLook::Equipped { .. }) => "eq",
        Some(EntityLook::Door { .. }) => "door",
        Some(EntityLook::Transport { .. }) => "tx",
    }
}

fn kind_tag(kind: ffxi_viewer_wire::EntityKind) -> &'static str {
    use ffxi_viewer_wire::EntityKind;
    match kind {
        EntityKind::Pc => "pc",
        EntityKind::Npc => "npc",
        EntityKind::Mob => "mob",
        EntityKind::Pet => "pet",
        EntityKind::Other => "?",
    }
}

/// Render the `/debug` (no args) report: one line for the current
/// target, then a header + 10 nearest entities sorted by 2D distance.
///
/// The self entity is flagged with `(self)` by matching `pos == self_pos`
/// — `state.rs::self_position` derives `self_pos` from the self entity in
/// the snapshot, so the equality is exact (no epsilon needed) when self
/// is present in the entity list. Pre-CHAR_PC ticks where self isn't in
/// the list yet just show no `(self)` marker, which is the honest
/// answer.
fn render_debug_nearby(
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
) -> String {
    let mut out = String::new();

    // Target line.
    match current_target.and_then(|id| entities.iter().find(|e| e.id == id)) {
        Some(t) => {
            let name = t.name.as_deref().unwrap_or("?");
            let d = sq_dist(t.pos, self_pos).sqrt();
            let hp = t
                .hp_pct
                .map(|p| format!("{p}%"))
                .unwrap_or_else(|| "?".into());
            out.push_str(&format!(
                "target: id={} idx={} {} {} dist={:.1}y hp={} look={}",
                t.id,
                t.act_index,
                kind_tag(t.kind),
                name,
                d,
                hp,
                look_tag(t.look.as_ref()),
            ));
        }
        None => out.push_str("target: none"),
    }
    out.push('\n');

    // Header + nearby rows.
    out.push_str("nearby (top 10 by dist):");
    let nearby = nearby_entities(entities, self_pos, 10);
    if nearby.is_empty() {
        out.push_str(" (none)");
        return out;
    }
    for (e, sq) in nearby {
        let d = sq.sqrt();
        let name = e.name.as_deref().unwrap_or("?");
        let hp = e
            .hp_pct
            .map(|p| format!("{p}%"))
            .unwrap_or_else(|| "?".into());
        let self_tag = if e.pos == self_pos { " (self)" } else { "" };
        out.push('\n');
        out.push_str(&format!(
            "  id={} idx={} {} dist={:.1}y hp={} look={} {}{}",
            e.id,
            e.act_index,
            kind_tag(e.kind),
            d,
            hp,
            look_tag(e.look.as_ref()),
            name,
            self_tag,
        ));
    }
    out
}

/// Resolve a single entity (name prefix, decimal id, or act_index) and
/// render its full detail block.
fn render_debug_entity(arg: &str, entities: &[WireEntity], self_pos: WireVec3) -> String {
    // Resolution order: numeric id → numeric act_index → name prefix.
    // u32 first because ids easily exceed u16.
    let ent: Option<&WireEntity> = if let Ok(id) = arg.parse::<u32>() {
        entities.iter().find(|e| e.id == id).or_else(|| {
            u16::try_from(id)
                .ok()
                .and_then(|idx| entities.iter().find(|e| e.act_index == idx))
        })
    } else {
        resolve_name(arg, entities, self_pos)
    };
    let Some(e) = ent else {
        return format!("/debug: no entity `{arg}`");
    };
    let d = sq_dist(e.pos, self_pos).sqrt();
    let name = e.name.as_deref().unwrap_or("?");
    let hp = e
        .hp_pct
        .map(|p| format!("{p}%"))
        .unwrap_or_else(|| "?".into());
    let mut s = String::new();
    s.push_str(&format!("/debug [{name}] id={} idx={}", e.id, e.act_index));
    s.push('\n');
    s.push_str(&format!(
        "  kind={} hp={} dist={:.2}y heading={} speed={}/{}",
        kind_tag(e.kind),
        hp,
        d,
        e.heading,
        e.speed,
        e.speed_base,
    ));
    s.push('\n');
    s.push_str(&format!(
        "  pos=({:.2}, {:.2}, {:.2})",
        e.pos.x, e.pos.y, e.pos.z
    ));
    s.push('\n');
    s.push_str(&format!(
        "  bt_target={} claim={}",
        e.bt_target_id, e.claim_id
    ));
    s.push('\n');
    s.push_str(&format!("  look_tag={}", look_tag(e.look.as_ref())));
    use ffxi_viewer_wire::EntityLook;
    match &e.look {
        None => s.push_str(" (none decoded — no look-bearing tick yet)"),
        Some(EntityLook::Standard { modelid }) => {
            s.push_str(&format!(" modelid={modelid} (0x{modelid:04X})"));
        }
        Some(EntityLook::Equipped {
            face,
            race,
            head,
            body,
            hands,
            legs,
            feet,
            main,
            sub,
            ranged,
        }) => {
            s.push('\n');
            s.push_str(&format!(
                "  race={race} face={face} head=0x{head:04X} body=0x{body:04X} hands=0x{hands:04X}"
            ));
            s.push('\n');
            s.push_str(&format!(
                "  legs=0x{legs:04X} feet=0x{feet:04X} main=0x{main:04X} sub=0x{sub:04X} ranged=0x{ranged:04X}"
            ));
        }
        Some(EntityLook::Door { size }) => s.push_str(&format!(" door size={size}")),
        Some(EntityLook::Transport { size }) => s.push_str(&format!(" transport size={size}")),
    }
    s
}

/// Top-`limit` entities by squared 2D distance from `from`. Self entity
/// (the one whose `pos == from`) is *included* — `/debug` callers want to
/// see it explicitly tagged. Returns `(entity, sq_dist)` pairs so callers
/// can reuse the squared distance for sorting/rendering without redoing
/// the math.
fn nearby_entities<'a>(
    entities: &'a [WireEntity],
    from: WireVec3,
    limit: usize,
) -> Vec<(&'a WireEntity, f32)> {
    let mut scored: Vec<(&WireEntity, f32)> =
        entities.iter().map(|e| (e, sq_dist(e.pos, from))).collect();
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored
}

/// `/sound [on|off|toggle] [bgm|sfx]` — set or toggle audio mute.
/// Argument order is flexible: `/sound bgm off`, `/sound off bgm`,
/// `/sound off` (both), `/sound toggle bgm`, `/sound` (toggle both).
///
/// Returns one of `SetBoth` / `SetBgm` / `SetSfx`. The dispatcher in
/// `text_input` applies the op to the `AudioMuteState` resource and
/// emits a status toast.
///
/// Bare `/sound` toggles both categories — same convention as
/// `/devhud` / `/heal` / `/capture` (no arg = toggle). The status
/// query is accessible via the explicit `/sound status` or
/// `/sound ?` form.
fn parse_sound(rest: &str) -> SlashOutcome {
    let tokens: Vec<String> = rest
        .split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect();
    if tokens.is_empty() {
        return SlashOutcome::SetSound(SoundOp::SetBoth(None));
    }
    // Split tokens into verb (on/off/toggle) + category (bgm/sfx).
    // Either order is accepted; missing category defaults to "both".
    let mut verb: Option<Option<bool>> = None;
    let mut category: Option<&str> = None;
    for tok in &tokens {
        match tok.as_str() {
            "on" | "unmute" | "true" | "1" => {
                // `/sound on` reads naturally as "turn sound ON" =
                // unmute. Flip so the underlying boolean is "muted".
                verb = Some(Some(false));
            }
            "off" | "mute" | "false" | "0" => {
                verb = Some(Some(true));
            }
            "toggle" | "flip" => {
                verb = Some(None);
            }
            "bgm" | "music" => category = Some("bgm"),
            "sfx" | "se" | "fx" | "effects" => category = Some("sfx"),
            "status" | "?" => return SlashOutcome::SetSound(SoundOp::Status),
            other => {
                return SlashOutcome::SystemMessage(format!(
                    "/sound: bad arg `{other}` \
                     (use on|off|toggle and/or bgm|sfx)"
                ));
            }
        }
    }
    let verb = match verb {
        Some(v) => v,
        None => {
            // A bare category (`/sound bgm`) with no verb reads as
            // toggle — same convention `/devhud` uses when called
            // with no arg.
            None
        }
    };
    let op = match category {
        Some("bgm") => SoundOp::SetBgm(verb),
        Some("sfx") => SoundOp::SetSfx(verb),
        _ => SoundOp::SetBoth(verb),
    };
    SlashOutcome::SetSound(op)
}

/// `/weather <id|name>` — resolve the arg into a [`Weather`] variant
/// and emit [`SlashOutcome::SetWeatherClient`]. Names are matched
/// case-insensitively after stripping `_`/`-`/spaces, so `rain`,
/// `RAIN`, `hot_spell`, `HotSpell`, and `hot spell` all resolve. Ids
/// are 0..=19; values outside the range get a usage toast.
fn parse_weather(rest: &str) -> SlashOutcome {
    use ffxi_viewer_wire::Weather;
    let arg = rest.trim();
    if arg.is_empty() {
        return SlashOutcome::SystemMessage(
            "/weather: usage `/weather <id|name>` — 0..=19, or names like \
             none, sunshine, clouds, fog, rain, snow, thunderstorms, sand_storm, \
             auroras, gloom, darkness (see vendor/server/scripts/enum/weather.lua)"
                .into(),
        );
    }
    if let Ok(n) = arg.parse::<u16>() {
        if n > 19 {
            return SlashOutcome::SystemMessage(format!("/weather: id {n} out of range (0..=19)"));
        }
        return SlashOutcome::SetWeatherClient(Weather::from_lsb(n));
    }
    let key: String = arg
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .flat_map(char::to_lowercase)
        .collect();
    let w = match key.as_str() {
        "none" | "clear" | "off" => Weather::None,
        "sunshine" | "sun" | "sunny" => Weather::Sunshine,
        "clouds" | "cloudy" | "cloud" => Weather::Clouds,
        "fog" | "foggy" => Weather::Fog,
        "hotspell" => Weather::HotSpell,
        "heatwave" => Weather::HeatWave,
        "rain" | "rainy" => Weather::Rain,
        "squall" => Weather::Squall,
        "duststorm" | "dust" => Weather::DustStorm,
        "sandstorm" | "sand" => Weather::SandStorm,
        "wind" | "windy" => Weather::Wind,
        "gales" | "gale" => Weather::Gales,
        "snow" | "snowy" => Weather::Snow,
        "blizzards" | "blizzard" => Weather::Blizzards,
        "thunder" => Weather::Thunder,
        "thunderstorms" | "thunderstorm" | "storm" => Weather::Thunderstorms,
        "auroras" | "aurora" => Weather::Auroras,
        "stellarglare" | "stellar" => Weather::StellarGlare,
        "gloom" => Weather::Gloom,
        "darkness" | "dark" => Weather::Darkness,
        _ => {
            return SlashOutcome::SystemMessage(format!(
                "/weather: unknown weather `{arg}` (try a number 0..=19 or a name like `rain`)"
            ));
        }
    };
    SlashOutcome::SetWeatherClient(w)
}

/// `/devhud on|off|toggle` — show/hide developer telemetry overlays.
/// Empty argument toggles, matching the convention of `/zonegeom` /
/// `/heal` / `/capture`. Off by default (vanilla FFXI / Ashita /
/// Windower-addon style).
fn parse_devhud(rest: &str) -> SlashOutcome {
    let arg = rest.trim().to_ascii_lowercase();
    let setting = match arg.as_str() {
        "" | "toggle" => None,
        "on" | "true" | "1" => Some(true),
        "off" | "false" | "0" => Some(false),
        other => {
            return SlashOutcome::SystemMessage(format!(
                "/devhud: bad arg `{other}` (use on|off|toggle)"
            ));
        }
    };
    SlashOutcome::SetDevHud(setting)
}

/// `/minimap [show|hide|toggle|mode <top|retail|auto>|cull <N>]`
///
/// Bare `/minimap` reports the current mode + visibility + cull height
/// as a system chat line (the dispatcher reads the live resources to
/// format it). `mode` and `cull` take a sub-argument.
fn parse_minimap(rest: &str) -> SlashOutcome {
    let mut parts = rest.trim().splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let arg = parts.next().unwrap_or("").trim();
    let op = match verb.as_str() {
        "" => MinimapOp::Status,
        "show" | "on" => MinimapOp::Show,
        "hide" | "off" => MinimapOp::Hide,
        "toggle" => MinimapOp::Toggle,
        "mode" => match arg.to_ascii_lowercase().as_str() {
            "top" | "topdown" => MinimapOp::ModeTopDown,
            "retail" => MinimapOp::ModeRetail,
            "auto" | "" => MinimapOp::ModeAuto,
            other => {
                return SlashOutcome::SystemMessage(format!(
                    "/minimap mode: bad arg `{other}` (use top|retail|auto)"
                ));
            }
        },
        "cull" => match arg.parse::<f32>() {
            Ok(v) if v.is_finite() && v >= 0.0 => MinimapOp::SetCull(v),
            _ => {
                return SlashOutcome::SystemMessage(format!(
                    "/minimap cull: bad value `{arg}` (expected non-negative number)"
                ));
            }
        },
        "zoom" => match arg.to_ascii_lowercase().as_str() {
            "in" => MinimapOp::ZoomIn,
            "out" => MinimapOp::ZoomOut,
            "fit" | "max" | "zone" => MinimapOp::ZoomFit,
            "reset" | "default" => MinimapOp::ZoomReset,
            "" => {
                return SlashOutcome::SystemMessage(
                    "/minimap zoom: missing arg (in|out|fit|reset|<radius>)".to_string(),
                );
            }
            num => match num.parse::<f32>() {
                Ok(v) if v.is_finite() && v > 0.0 => MinimapOp::ZoomSet(v),
                _ => {
                    return SlashOutcome::SystemMessage(format!(
                        "/minimap zoom: bad arg `{num}` (expected in|out|fit|reset|<radius>)"
                    ));
                }
            },
        },
        other => {
            return SlashOutcome::SystemMessage(format!(
                "/minimap: unknown sub `{other}` (use show|hide|toggle|mode|cull|zoom)"
            ));
        }
    };
    SlashOutcome::SetMinimap(op)
}

/// `/zonegeom off|collision|all|camera|toggle` — set MZB overlay visibility.
/// `on` is an alias for `all` (back-compat with the old bool toggle).
/// `camera`/`cam` activates the camera-collision debug overlay (MZB collision
/// + BVH AABBs + active raycast gizmos). `toggle`/empty cycles
/// Collision → All → Camera → Off → Collision.
fn parse_zonegeom(rest: &str) -> SlashOutcome {
    use ffxi_viewer_core::dat_mzb::ZoneGeomMode;
    let arg = rest.trim().to_ascii_lowercase();
    let setting = match arg.as_str() {
        "" | "toggle" => None,
        "off" | "false" | "0" => Some(ZoneGeomMode::Off),
        "collision" | "coll" => Some(ZoneGeomMode::Collision),
        "all" | "on" | "true" | "1" => Some(ZoneGeomMode::All),
        "camera" | "cam" => Some(ZoneGeomMode::Camera),
        other => {
            return SlashOutcome::SystemMessage(format!(
                "/zonegeom: bad arg `{other}` (use off|collision|all|camera|toggle)"
            ));
        }
    };
    SlashOutcome::SetZoneGeom(setting)
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

fn parse_capture(rest: &str) -> SlashOutcome {
    let arg = rest.split_whitespace().next();
    match arg.map(str::to_ascii_lowercase).as_deref() {
        None | Some("toggle") => SlashOutcome::SetCaptureMode(None),
        Some("on") | Some("1") | Some("true") => SlashOutcome::SetCaptureMode(Some(true)),
        Some("off") | Some("0") | Some("false") => SlashOutcome::SetCaptureMode(Some(false)),
        Some(other) => SlashOutcome::SystemMessage(format!(
            "/capture: unknown arg `{other}` (use `on`, `off`, or `toggle`)"
        )),
    }
}

fn parse_screenshot(rest: &str) -> SlashOutcome {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        SlashOutcome::Screenshot { path: None }
    } else {
        SlashOutcome::Screenshot {
            path: Some(trimmed.to_string()),
        }
    }
}

fn parse_fps(rest: &str) -> SlashOutcome {
    let mut parts = rest.split_whitespace();
    let Some(arg) = parts.next() else {
        return SlashOutcome::SystemMessage(
            "/fps: usage `/fps <max>` (0 or `off` disables the cap)".into(),
        );
    };
    if arg.eq_ignore_ascii_case("off") {
        return SlashOutcome::SetTargetFps(None);
    }
    match arg.parse::<u32>() {
        Ok(0) => SlashOutcome::SetTargetFps(None),
        Ok(n) => SlashOutcome::SetTargetFps(Some(n)),
        Err(_) => SlashOutcome::SystemMessage(format!(
            "/fps: `{arg}` is not a number (use `/fps <max>` or `/fps off`)"
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
        None => {
            "look: none decoded yet (entity hasn't sent a CHAR_NPC look-bearing tick)".to_string()
        }
        Some(EntityLook::Standard { modelid }) => {
            format!("look: STANDARD modelid={modelid} (0x{modelid:04X})")
        }
        Some(EntityLook::Equipped {
            face,
            race,
            head,
            body,
            hands,
            legs,
            feet,
            main,
            sub,
            ranged,
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
        entity_id: None,
    }
}

/// `/load_mmb_on <entity_id> <file_id> <chunk_idx>` — debug variant of
/// `/load_mmb` that parents the spawned mesh under a tracked wire
/// entity instead of placing it at the operator's position. Lets
/// operators dry-run the look-driven spawn pipeline (Stages 2+) with
/// a manually chosen MMB before the resolver knows how to pick one.
fn parse_load_mmb_on(rest: &str) -> SlashOutcome {
    let mut parts = rest.split_whitespace();
    let entity_str = parts.next().unwrap_or("");
    let file_str = parts.next().unwrap_or("");
    let chunk_str = parts.next().unwrap_or("");
    if entity_str.is_empty() || file_str.is_empty() || chunk_str.is_empty() {
        return SlashOutcome::SystemMessage(
            "/load_mmb_on: usage `/load_mmb_on <entity_id> <file_id> <chunk_idx>`".into(),
        );
    }
    let entity_id: u32 = match entity_str.parse() {
        Ok(n) => n,
        Err(_) => {
            return SlashOutcome::SystemMessage(format!(
                "/load_mmb_on: bad entity_id `{entity_str}`"
            ))
        }
    };
    let file_id: u32 = match file_str.parse() {
        Ok(n) => n,
        Err(_) => {
            return SlashOutcome::SystemMessage(format!("/load_mmb_on: bad file_id `{file_str}`"))
        }
    };
    let chunk_idx: usize = match chunk_str.parse() {
        Ok(n) => n,
        Err(_) => {
            return SlashOutcome::SystemMessage(format!(
                "/load_mmb_on: bad chunk_idx `{chunk_str}`"
            ))
        }
    };
    SlashOutcome::LoadMmb {
        file_id,
        chunk_idx,
        // `world_pos` is unused when `entity_id` is `Some`, but the
        // outcome shape is shared with `/load_mmb`. Pass a sentinel.
        world_pos: WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        entity_id: Some(entity_id),
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
        let pc_rank = |e: &WireEntity| matches!(e.kind, ffxi_viewer_wire::EntityKind::Pc) as u8;
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

/// Cycle through entities matching one of `kinds`, sorted by 3D
/// distance from `self_pos`. If `current` is in the filtered set, step
/// to the next entry (forward or reverse, with wrap); otherwise pick
/// the nearest. Returns `None` when no entity matches the kind filter.
///
/// Used by `/targetnpc[2]`, `/targetenemy`, and `/targetnpcparty`.
/// Not viewport-constrained — these commands intentionally bypass the
/// Tab cycle's frustum filter so the operator can grab a target they
/// know is nearby even when the camera is pointed elsewhere.
fn cycle_kind_filtered(
    entities: &[WireEntity],
    self_pos: WireVec3,
    current: Option<u32>,
    kinds: &[ffxi_viewer_wire::EntityKind],
    reverse: bool,
) -> Option<u32> {
    let mut pool: Vec<&WireEntity> = entities
        .iter()
        .filter(|e| kinds.contains(&e.kind))
        .collect();
    pool.sort_by(|a, b| {
        let da = sq_dist(a.pos, self_pos);
        let db = sq_dist(b.pos, self_pos);
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    });
    if pool.is_empty() {
        return None;
    }
    match current.and_then(|id| pool.iter().position(|e| e.id == id)) {
        Some(idx) => {
            let n = pool.len();
            let next = if reverse {
                (idx + n - 1) % n
            } else {
                (idx + 1) % n
            };
            Some(pool[next].id)
        }
        None => Some(pool[0].id),
    }
}

/// Resolve a 1-based party slot to an entity id. Slot 1 is self
/// (`self_char_id`); slots 2..=6 index `party[1..=5]` in insertion
/// order. Returns `None` if the slot is empty or `self_char_id` is
/// unknown.
fn resolve_party_slot(
    slot_1based: u8,
    self_char_id: Option<u32>,
    party: &[ffxi_viewer_wire::PartyMember],
) -> Option<u32> {
    if slot_1based == 0 {
        return None;
    }
    let idx = (slot_1based as usize).saturating_sub(1);
    if idx == 0 {
        self_char_id
    } else {
        party.get(idx).map(|p| p.id)
    }
}

fn sq_dist(a: WireVec3, b: WireVec3) -> f32 {
    // 3D Euclidean — the server's range checks are 3D, the overhead
    // nameplate is 3D, so every callsite that sorts/filters "by distance
    // for an action" must match. A 2D variant would silently undercount
    // altitude differences on sloped terrain and let `/copy` / Tab / Enter
    // pick a target the server then rejects as out of range.
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = a.z - b.z;
    dx * dx + dy * dy + dz * dz
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
            look: None,
        }
    }

    fn empty_entities() -> Vec<WireEntity> {
        Vec::new()
    }

    fn origin() -> WireVec3 {
        WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }

    /// Test wrapper that defaults `self_char_id` and `party` to the
    /// "unknown / empty" values most tests want. Tests that exercise
    /// `/targetparty*` or `/targetnpcparty` should call `parse_slash`
    /// directly with the full signature.
    fn parse_slash_t(
        buffer: &str,
        entities: &[WireEntity],
        self_pos: WireVec3,
        current_target: Option<u32>,
        zone_id: Option<u16>,
    ) -> SlashOutcome {
        parse_slash(
            buffer,
            entities,
            self_pos,
            current_target,
            zone_id,
            None,
            &[],
        )
    }

    fn party_member(id: u32, name: &str) -> ffxi_viewer_wire::PartyMember {
        ffxi_viewer_wire::PartyMember {
            id,
            act_index: id as u16,
            name: Some(name.into()),
            hp: 0,
            mp: 0,
            tp: 0,
            hp_pct: 100,
            mp_pct: 100,
            zone_no: 0,
            main_job: 0,
            main_job_lv: 0,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: false,
            is_alliance_leader: false,
            in_mog_house: false,
        }
    }

    #[test]
    fn targetnpc_cycles_non_pc_forward() {
        // Two mobs and an NPC; PC ignored. Without a current target, the
        // nearest one wins. Subsequent press cycles to the next.
        let entities = vec![
            ent(1, "Goblin A", EntityKind::Mob, 3.0, 0.0),
            ent(2, "Vendor", EntityKind::Npc, 5.0, 0.0),
            ent(3, "Goblin B", EntityKind::Mob, 7.0, 0.0),
            ent(4, "Bob", EntityKind::Pc, 1.0, 0.0), // closer but excluded
        ];
        assert!(matches!(
            parse_slash_t("/targetnpc", &entities, origin(), None, None),
            SlashOutcome::SetTarget(Some(1))
        ));
        // Cycle from id=1 → id=2 → id=3 → wrap to id=1.
        assert!(matches!(
            parse_slash_t("/targetnpc", &entities, origin(), Some(1), None),
            SlashOutcome::SetTarget(Some(2))
        ));
        assert!(matches!(
            parse_slash_t("/targetnpc", &entities, origin(), Some(3), None),
            SlashOutcome::SetTarget(Some(1))
        ));
    }

    #[test]
    fn targetnpc2_cycles_reverse() {
        // Same pool as above; "2" walks the cycle backward.
        let entities = vec![
            ent(1, "Goblin A", EntityKind::Mob, 3.0, 0.0),
            ent(2, "Vendor", EntityKind::Npc, 5.0, 0.0),
            ent(3, "Goblin B", EntityKind::Mob, 7.0, 0.0),
        ];
        // From id=2, reverse goes to id=1.
        assert!(matches!(
            parse_slash_t("/targetnpc2", &entities, origin(), Some(2), None),
            SlashOutcome::SetTarget(Some(1))
        ));
        // From id=1, reverse wraps to id=3.
        assert!(matches!(
            parse_slash_t("/targetnpc2", &entities, origin(), Some(1), None),
            SlashOutcome::SetTarget(Some(3))
        ));
    }

    #[test]
    fn targetenemy_skips_npcs() {
        // A vendor sits closer than the mob; /targetenemy must skip it.
        let entities = vec![
            ent(1, "Vendor", EntityKind::Npc, 2.0, 0.0),
            ent(2, "Goblin", EntityKind::Mob, 8.0, 0.0),
        ];
        assert!(matches!(
            parse_slash_t("/targetenemy", &entities, origin(), None, None),
            SlashOutcome::SetTarget(Some(2))
        ));
    }

    #[test]
    fn targetparty_resolves_self_and_slots() {
        let party = vec![
            party_member(100, "Self"),
            party_member(200, "Member2"),
            party_member(300, "Member3"),
        ];
        // /targetparty1 = self.
        assert!(matches!(
            parse_slash(
                "/targetparty1",
                &[],
                origin(),
                None,
                None,
                Some(100),
                &party
            ),
            SlashOutcome::SetTarget(Some(100))
        ));
        // /targetparty3 = third slot.
        assert!(matches!(
            parse_slash(
                "/targetparty3",
                &[],
                origin(),
                None,
                None,
                Some(100),
                &party
            ),
            SlashOutcome::SetTarget(Some(300))
        ));
        // Empty slot returns a system message, not a target change.
        assert!(matches!(
            parse_slash(
                "/targetparty5",
                &[],
                origin(),
                None,
                None,
                Some(100),
                &party
            ),
            SlashOutcome::SystemMessage(_)
        ));
    }

    #[test]
    fn sq_dist_is_3d_euclidean() {
        // 3-4-12 Pythagorean quadruple. A 2D implementation would
        // return 25 (3² + 4²); 3D returns 169 (3² + 4² + 12²).
        let a = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let b = WireVec3 {
            x: 3.0,
            y: 4.0,
            z: 12.0,
        };
        assert_eq!(sq_dist(a, b), 169.0);
    }

    #[test]
    fn empty_command_is_system_message() {
        let out = parse_slash_t("/", &empty_entities(), origin(), None, None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn unknown_command_is_system_message() {
        let out = parse_slash_t("/blarg", &empty_entities(), origin(), None, None);
        match out {
            SlashOutcome::SystemMessage(s) => assert!(s.contains("/blarg")),
            _ => panic!("expected SystemMessage"),
        }
    }

    #[test]
    fn party_chat_with_text() {
        let out = parse_slash_t("/p hello world", &empty_entities(), origin(), None, None);
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
        let out = parse_slash_t("/p", &empty_entities(), origin(), None, None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn tell_requires_name_and_text() {
        let out = parse_slash_t("/t Bob hi there", &empty_entities(), origin(), None, None);
        match out {
            SlashOutcome::Command(AgentCommand::Tell { to, text }) => {
                assert_eq!(to, "Bob");
                assert_eq!(text, "hi there");
            }
            other => panic!("expected Tell, got {other:?}"),
        }

        let out = parse_slash_t("/t Bob", &empty_entities(), origin(), None, None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn follow_with_name_resolves_to_id() {
        let entities = vec![
            ent(101, "Bob", EntityKind::Pc, 0.0, 0.0),
            ent(102, "Bobble", EntityKind::Npc, 5.0, 5.0),
        ];
        let out = parse_slash_t("/follow Bob", &entities, origin(), None, None);
        match out {
            SlashOutcome::Command(AgentCommand::Follow { target_id, .. }) => {
                assert_eq!(target_id, 101); // PC wins over NPC on prefix tie
            }
            other => panic!("expected Follow, got {other:?}"),
        }
    }

    #[test]
    fn debug_no_args_dumps_target_and_nearby() {
        let entities = vec![
            ent(101, "Self", EntityKind::Pc, 0.0, 0.0),
            ent(202, "NearMob", EntityKind::Mob, 2.0, 0.0),
            ent(303, "FarNpc", EntityKind::Npc, 50.0, 50.0),
        ];
        let out = parse_slash_t("/debug", &entities, origin(), Some(202), None);
        match out {
            SlashOutcome::SystemMessage(s) => {
                assert!(s.contains("target:"), "no target line: {s}");
                assert!(s.contains("NearMob"), "target name not in output: {s}");
                assert!(s.contains("nearby"), "nearby header missing: {s}");
                // Self is at origin and so is `self_pos` — must be tagged.
                assert!(s.contains("(self)"), "self marker missing: {s}");
                // Both other entities should appear in the nearby list.
                assert!(s.contains("FarNpc"), "far entity missing: {s}");
            }
            other => panic!("expected SystemMessage, got {other:?}"),
        }
    }

    #[test]
    fn debug_with_name_dumps_single_entity_detail() {
        let mut e = ent(202, "Goblin", EntityKind::Mob, 3.0, 4.0);
        e.hp_pct = Some(42);
        let entities = vec![e];
        let out = parse_slash_t("/debug Goblin", &entities, origin(), None, None);
        match out {
            SlashOutcome::SystemMessage(s) => {
                assert!(s.contains("Goblin"), "name missing: {s}");
                assert!(s.contains("id=202"), "id missing: {s}");
                assert!(s.contains("42%"), "hp missing: {s}");
                // 2D distance from (0,0) to (3,4) = 5.
                assert!(s.contains("dist=5.00y"), "distance wrong: {s}");
            }
            other => panic!("expected SystemMessage, got {other:?}"),
        }
    }

    #[test]
    fn debug_heights_subcommand_still_works() {
        let out = parse_slash_t("/debug heights", &empty_entities(), origin(), None, None);
        assert!(matches!(out, SlashOutcome::DebugHeights));
        let out = parse_slash_t("/dbg h", &empty_entities(), origin(), None, None);
        assert!(matches!(out, SlashOutcome::DebugHeights));
    }

    #[test]
    fn follow_no_name_uses_current_target() {
        let out = parse_slash_t("/follow", &empty_entities(), origin(), Some(42), None);
        match out {
            SlashOutcome::Command(AgentCommand::Follow { target_id, .. }) => {
                assert_eq!(target_id, 42);
            }
            other => panic!("expected Follow, got {other:?}"),
        }
    }

    #[test]
    fn follow_no_name_no_target_is_system_message() {
        let out = parse_slash_t("/follow", &empty_entities(), origin(), None, None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn target_clears_with_no_arg() {
        let out = parse_slash_t("/target", &empty_entities(), origin(), Some(7), None);
        assert!(matches!(out, SlashOutcome::SetTarget(None)));
    }

    #[test]
    fn quit_aliases() {
        // `/quit` and `/disconnect` are the "drop the session" pair.
        // `/logout` is *not* in this group — it goes through the
        // server's LeaveGame flow; see `logout_*` tests below.
        for s in ["/quit", "/disconnect"] {
            assert!(matches!(
                parse_slash_t(s, &empty_entities(), origin(), None, None),
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
        match parse_slash_t("/logout", &empty_entities(), origin(), None, None) {
            SlashOutcome::Commands(cmds) => {
                assert_eq!(cmds.len(), 2, "expected [ReqLogout, Heal], got {cmds:?}");
                assert!(
                    matches!(
                        cmds[0],
                        AgentCommand::ReqLogout {
                            kind: ReqLogoutKind::LogoutToggle
                        }
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
        match parse_slash_t("/logout on", &empty_entities(), origin(), None, None) {
            SlashOutcome::Commands(cmds) => {
                assert_eq!(cmds.len(), 2);
                assert!(matches!(
                    cmds[0],
                    AgentCommand::ReqLogout {
                        kind: ReqLogoutKind::LogoutOn
                    }
                ));
                assert!(matches!(cmds[1], AgentCommand::Heal { mode: HealMode::On }));
            }
            other => panic!("expected Commands([ReqLogout(On), Heal]), got {other:?}"),
        }
        // Cancelling via `off` is single-command — cancelling the
        // logout shouldn't separately try to start resting.
        match parse_slash_t("/logout off", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::ReqLogout { kind }) => {
                assert_eq!(kind, ReqLogoutKind::LogoutOff);
            }
            other => panic!("expected single Command(ReqLogout(Off)), got {other:?}"),
        }
    }

    #[test]
    fn shutdown_no_arg_toggles_and_chains_heal_on() {
        match parse_slash_t("/shutdown", &empty_entities(), origin(), None, None) {
            SlashOutcome::Commands(cmds) => {
                assert_eq!(cmds.len(), 2);
                assert!(matches!(
                    cmds[0],
                    AgentCommand::ReqLogout {
                        kind: ReqLogoutKind::ShutdownToggle
                    }
                ));
                assert!(matches!(cmds[1], AgentCommand::Heal { mode: HealMode::On }));
            }
            other => panic!("expected Commands, got {other:?}"),
        }
    }

    #[test]
    fn shutdown_on_chains_heal_shutdown_off_does_not() {
        match parse_slash_t("/shutdown on", &empty_entities(), origin(), None, None) {
            SlashOutcome::Commands(cmds) => {
                assert_eq!(cmds.len(), 2);
                assert!(matches!(
                    cmds[0],
                    AgentCommand::ReqLogout {
                        kind: ReqLogoutKind::ShutdownOn
                    }
                ));
                assert!(matches!(cmds[1], AgentCommand::Heal { mode: HealMode::On }));
            }
            other => panic!("expected Commands, got {other:?}"),
        }
        match parse_slash_t("/shutdown off", &empty_entities(), origin(), None, None) {
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
        match parse_slash_t("/exit", &empty_entities(), origin(), None, None) {
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
                    parse_slash_t(s, &empty_entities(), origin(), None, None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}"
            );
        }
    }

    #[test]
    fn sit_no_arg_toggles() {
        match parse_slash_t("/sit", &empty_entities(), origin(), None, None) {
            SlashOutcome::SetSitStance(t) => assert_eq!(t, SitToggle::Toggle),
            other => panic!("expected SetSitStance(Toggle), got {other:?}"),
        }
        // `/kneel` is the retail alias for `/sit`.
        match parse_slash_t("/kneel", &empty_entities(), origin(), None, None) {
            SlashOutcome::SetSitStance(t) => assert_eq!(t, SitToggle::Toggle),
            other => panic!("expected SetSitStance(Toggle), got {other:?}"),
        }
    }

    #[test]
    fn sit_on_off_and_stand() {
        match parse_slash_t("/sit on", &empty_entities(), origin(), None, None) {
            SlashOutcome::SetSitStance(t) => assert_eq!(t, SitToggle::On),
            other => panic!("expected SetSitStance(On), got {other:?}"),
        }
        match parse_slash_t("/sit off", &empty_entities(), origin(), None, None) {
            SlashOutcome::SetSitStance(t) => assert_eq!(t, SitToggle::Off),
            other => panic!("expected SetSitStance(Off), got {other:?}"),
        }
        // `/stand` is the retail standalone-form of `/sit off`.
        match parse_slash_t("/stand", &empty_entities(), origin(), None, None) {
            SlashOutcome::SetSitStance(t) => assert_eq!(t, SitToggle::Off),
            other => panic!("expected SetSitStance(Off), got {other:?}"),
        }
    }

    #[test]
    fn sit_rejects_unknown_arg() {
        match parse_slash_t("/sit bogus", &empty_entities(), origin(), None, None) {
            SlashOutcome::SystemMessage(s) => assert!(s.contains("/sit:"), "{s}"),
            other => panic!("expected SystemMessage, got {other:?}"),
        }
    }

    #[test]
    fn heal_no_arg_toggles() {
        match parse_slash_t("/heal", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::Heal { mode }) => {
                assert_eq!(mode, HealMode::Toggle);
            }
            other => panic!("expected Command(Heal(Toggle)), got {other:?}"),
        }
    }

    #[test]
    fn heal_on_and_off_select_explicit_modes() {
        match parse_slash_t("/heal on", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::Heal { mode }) => {
                assert_eq!(mode, HealMode::On);
            }
            other => panic!("expected Command(Heal(On)), got {other:?}"),
        }
        match parse_slash_t("/heal off", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::Heal { mode }) => {
                assert_eq!(mode, HealMode::Off);
            }
            other => panic!("expected Command(Heal(Off)), got {other:?}"),
        }
        // `/heal toggle` is an alias for the no-arg form. Tested
        // separately so a future reader doesn't assume "toggle" was
        // never a valid arg literal.
        match parse_slash_t("/heal toggle", &empty_entities(), origin(), None, None) {
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
                    parse_slash_t(s, &empty_entities(), origin(), None, None),
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
        let pos = WireVec3 {
            x: 12.5,
            y: -7.0,
            z: 3.25,
        };
        match parse_slash_t("/load_mmb 115 18", &empty_entities(), pos, None, None) {
            SlashOutcome::LoadMmb {
                file_id,
                chunk_idx,
                world_pos,
                entity_id,
            } => {
                assert_eq!(file_id, 115);
                assert_eq!(chunk_idx, 18);
                assert_eq!(world_pos, pos);
                assert_eq!(entity_id, None);
            }
            other => panic!("expected LoadMmb, got {other:?}"),
        }
    }

    /// `/load_mmb_on` carries the entity_id through unchanged. The
    /// world_pos is sentinel — the consumer ignores it when entity_id
    /// is `Some` — but we still verify the field is populated.
    #[test]
    fn load_mmb_on_parses_entity_id() {
        match parse_slash_t(
            "/load_mmb_on 1234 115 18",
            &empty_entities(),
            origin(),
            None,
            None,
        ) {
            SlashOutcome::LoadMmb {
                file_id,
                chunk_idx,
                entity_id,
                ..
            } => {
                assert_eq!(file_id, 115);
                assert_eq!(chunk_idx, 18);
                assert_eq!(entity_id, Some(1234));
            }
            other => panic!("expected LoadMmb with entity_id, got {other:?}"),
        }
        // Alias and bad-args paths surface as SystemMessage.
        assert!(matches!(
            parse_slash_t("/loadmmbon 99 7 0", &empty_entities(), origin(), None, None),
            SlashOutcome::LoadMmb {
                entity_id: Some(99),
                ..
            }
        ));
        for s in [
            "/load_mmb_on",
            "/load_mmb_on 1234",
            "/load_mmb_on 1234 115",
            "/load_mmb_on foo 115 18",
        ] {
            assert!(
                matches!(
                    parse_slash_t(s, &empty_entities(), origin(), None, None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}",
            );
        }
    }

    /// Both aliases (`load_mmb` and `loadmmb`) and bad args fall to
    /// `SystemMessage` so the operator sees a usage line instead of a
    /// silent no-op.
    #[test]
    fn load_mmb_alias_and_bad_args() {
        // Alias `/loadmmb` (no underscore) still routes.
        assert!(matches!(
            parse_slash_t("/loadmmb 115 18", &empty_entities(), origin(), None, None),
            SlashOutcome::LoadMmb {
                file_id: 115,
                chunk_idx: 18,
                entity_id: None,
                ..
            }
        ));
        // Missing chunk_idx, non-numeric file_id, non-numeric chunk_idx.
        for s in [
            "/load_mmb",
            "/load_mmb 115",
            "/load_mmb foo 18",
            "/load_mmb 115 bar",
        ] {
            assert!(
                matches!(
                    parse_slash_t(s, &empty_entities(), origin(), None, None),
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
        let pos = WireVec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        };
        // No chunk_idx: `None`.
        match parse_slash_t("/load_mzb 7368", &empty_entities(), pos, None, None) {
            SlashOutcome::LoadMzb {
                file_id,
                chunk_idx,
                world_pos,
            } => {
                assert_eq!(file_id, 7368);
                assert_eq!(chunk_idx, None);
                assert_eq!(world_pos, pos);
            }
            other => panic!("expected LoadMzb, got {other:?}"),
        }
        // Explicit chunk_idx.
        match parse_slash_t("/load_mzb 7368 2", &empty_entities(), pos, None, None) {
            SlashOutcome::LoadMzb {
                chunk_idx: Some(2), ..
            } => {}
            other => panic!("expected LoadMzb chunk_idx=Some(2), got {other:?}"),
        }
        // Alias `/loadmzb`.
        assert!(matches!(
            parse_slash_t("/loadmzb 7368", &empty_entities(), pos, None, None),
            SlashOutcome::LoadMzb {
                chunk_idx: None,
                ..
            }
        ));
        // Bad args fall to SystemMessage.
        for s in ["/load_mzb", "/load_mzb foo", "/load_mzb 7368 bar"] {
            assert!(
                matches!(
                    parse_slash_t(s, &empty_entities(), origin(), None, None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}",
            );
        }
    }

    #[test]
    fn navmesh_no_arg_toggles() {
        match parse_slash_t("/navmesh", &empty_entities(), origin(), None, None) {
            SlashOutcome::ToggleNavmesh(None) => {}
            other => panic!("expected ToggleNavmesh(None), got {other:?}"),
        }
    }

    #[test]
    fn navmesh_on_and_off_select_explicit_modes() {
        for (cmd, expected) in [("/navmesh on", Some(true)), ("/navmesh off", Some(false))] {
            match parse_slash_t(cmd, &empty_entities(), origin(), None, None) {
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
                    parse_slash_t(s, &empty_entities(), origin(), None, None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}"
            );
        }
    }

    #[test]
    fn pathto_numeric_three_args_dispatches() {
        match parse_slash_t(
            "/pathto 1.5 2 -3.25",
            &empty_entities(),
            origin(),
            None,
            None,
        ) {
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
        match parse_slash_t("/pathto target", &[entity], origin(), Some(42), None) {
            SlashOutcome::Command(AgentCommand::PathTo { x, y, z }) => {
                assert_eq!((x, y, z), (7.0, 8.0, 9.0));
            }
            other => panic!("expected PathTo from target, got {other:?}"),
        }
    }

    #[test]
    fn pathto_rejects_bad_input() {
        // `/pathto target` with no current target rejects. `/pathto x y z`
        // is a fuzzy lookup with no entities to match → rejects. `/pathto`
        // bare rejects with the usage line. `/pathto 1 2 3 4` (too many
        // numeric args) falls into fuzzy and finds no name → rejects.
        //
        // Note `/pathto 1 2` is NOT in this list — it is now valid as
        // the 2-arg coord form (z defaults to self_pos.z).
        for s in [
            "/pathto",
            "/pathto 1 2 3 4",
            "/pathto x y z",
            "/pathto target",
        ] {
            assert!(
                matches!(
                    parse_slash_t(s, &empty_entities(), origin(), None, None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}"
            );
        }
    }

    #[test]
    fn pathto_two_arg_form_uses_self_z() {
        // `/pathto x y` — z defaults to current self_pos.z. Operator
        // is on the same vertical plane and just wants to walk
        // somewhere on this floor.
        let mut self_pos = origin();
        self_pos.z = 17.5;
        match parse_slash_t("/pathto 10 20", &empty_entities(), self_pos, None, None) {
            SlashOutcome::Command(AgentCommand::PathTo { x, y, z }) => {
                assert_eq!((x, y, z), (10.0, 20.0, 17.5));
            }
            other => panic!("expected PathTo, got {other:?}"),
        }
    }

    #[test]
    fn pathto_fuzzy_name_picks_entity() {
        // `/pathto Bob` — no zone_id (so zone-lines skipped); falls
        // through to entity prefix match.
        let entity = ent(42, "Bob", EntityKind::Pc, 7.0, 8.0);
        match parse_slash_t("/pathto bob", &[entity], origin(), None, None) {
            SlashOutcome::Command(AgentCommand::PathTo { x, y, .. }) => {
                assert_eq!((x, y), (7.0, 8.0));
            }
            other => panic!("expected PathTo, got {other:?}"),
        }
    }

    #[test]
    fn warp_numeric_three_args_emits_move() {
        match parse_slash_t("/warp 1.5 2 -3.25", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::Move { x, y, z, heading }) => {
                assert_eq!((x, y, z), (1.5, 2.0, -3.25));
                // No self entity in the snapshot → heading defaults to 0.
                assert_eq!(heading, 0);
            }
            other => panic!("expected Move, got {other:?}"),
        }
    }

    #[test]
    fn warp_two_arg_form_uses_self_z() {
        let mut self_pos = origin();
        self_pos.z = -42.0;
        match parse_slash_t("/warp 1 2", &empty_entities(), self_pos, None, None) {
            SlashOutcome::Command(AgentCommand::Move { x, y, z, .. }) => {
                assert_eq!((x, y, z), (1.0, 2.0, -42.0));
            }
            other => panic!("expected Move, got {other:?}"),
        }
    }

    #[test]
    fn warp_preserves_self_heading() {
        // Self entity carries heading=64. Warp must not zero it out
        // (which would force the player to face north on every /warp).
        let self_pos = origin();
        let mut me = ent(1, "Me", EntityKind::Pc, self_pos.x, self_pos.y);
        me.pos.z = self_pos.z;
        me.heading = 64;
        match parse_slash_t("/warp 100 200 5", &[me], self_pos, None, None) {
            SlashOutcome::Command(AgentCommand::Move { heading, .. }) => {
                assert_eq!(heading, 64);
            }
            other => panic!("expected Move, got {other:?}"),
        }
    }

    #[test]
    fn warp_target_form_emits_move_to_target() {
        let entity = ent(42, "Mob", EntityKind::Mob, 11.0, 22.0);
        match parse_slash_t("/warp target", &[entity], origin(), Some(42), None) {
            SlashOutcome::Command(AgentCommand::Move { x, y, .. }) => {
                assert_eq!((x, y), (11.0, 22.0));
            }
            other => panic!("expected Move, got {other:?}"),
        }
    }

    #[test]
    fn warp_fuzzy_entity_match() {
        let entity = ent(42, "Bob", EntityKind::Pc, 7.0, 8.0);
        match parse_slash_t("/warp bo", &[entity], origin(), None, None) {
            SlashOutcome::Command(AgentCommand::Move { x, y, .. }) => {
                assert_eq!((x, y), (7.0, 8.0));
            }
            other => panic!("expected Move, got {other:?}"),
        }
    }

    #[test]
    fn warp_rejects_empty_and_unmatched() {
        for s in ["/warp", "/warp nosuchname"] {
            assert!(
                matches!(
                    parse_slash_t(s, &empty_entities(), origin(), None, None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}"
            );
        }
    }

    #[test]
    fn cancel_emits_cancel_command() {
        assert!(matches!(
            parse_slash_t("/cancel", &empty_entities(), origin(), None, None),
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
        let out = parse_slash_t("/attack", &entities, origin(), Some(42), None);
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
        match parse_slash_t("/engage", &entities, origin(), Some(7), None) {
            SlashOutcome::Command(AgentCommand::Engage { target_id }) => {
                assert_eq!(target_id, 7);
            }
            other => panic!("expected Engage, got {other:?}"),
        }
    }

    #[test]
    fn attackoff_emits_attack_off_action() {
        let entities = vec![ent(9, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash_t("/attackoff", &entities, origin(), Some(9), None) {
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
        let out = parse_slash_t("/check", &entities, origin(), Some(7), None);
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
        match parse_slash_t("/checkname", &entities, origin(), Some(7), None) {
            SlashOutcome::Command(AgentCommand::CheckTarget { kind, .. }) => {
                assert_eq!(kind, CheckKind::CheckName);
            }
            other => panic!("expected CheckTarget, got {other:?}"),
        }
        match parse_slash_t("/checkparam", &entities, origin(), Some(7), None) {
            SlashOutcome::Command(AgentCommand::CheckTarget { kind, .. }) => {
                assert_eq!(kind, CheckKind::CheckParam);
            }
            other => panic!("expected CheckTarget, got {other:?}"),
        }
    }

    #[test]
    fn check_no_target_is_system_message() {
        let out = parse_slash_t("/check", &empty_entities(), origin(), None, None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn buy_with_row_uses_qty_one_by_default() {
        let out = parse_slash_t("/buy 3", &empty_entities(), origin(), None, None);
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
        let out = parse_slash_t("/buy 0 12", &empty_entities(), origin(), None, None);
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
                    parse_slash_t(s, &empty_entities(), origin(), None, None),
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
        match parse_slash_t("/engage", &entities, origin(), Some(99), None) {
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
            parse_slash_t("/attack", &entities, origin(), Some(99), None),
            SlashOutcome::Command(AgentCommand::Engage { target_id: 99 })
        ));
    }

    #[test]
    fn disengage_dispatches_cancel() {
        // MCP has no `disengage` tool — agents call `cancel` to stop the
        // engage goal. The slash command matches.
        assert!(matches!(
            parse_slash_t("/disengage", &empty_entities(), origin(), None, None),
            SlashOutcome::Command(AgentCommand::Cancel)
        ));
    }

    #[test]
    fn raw_attack_preserves_direct_action() {
        // Escape hatch for one-shot wire-level Attack.
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash_t("/raw attack", &entities, origin(), Some(7), None) {
            SlashOutcome::Command(AgentCommand::Action {
                kind, target_id, ..
            }) => {
                assert_eq!(target_id, 7);
                assert!(matches!(kind, ActionKind::Attack));
            }
            other => panic!("expected Action{{Attack}}, got {other:?}"),
        }
    }

    #[test]
    fn raw_attackoff_preserves_direct_action() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash_t("/raw attackoff", &entities, origin(), Some(7), None) {
            SlashOutcome::Command(AgentCommand::Action { kind, .. }) => {
                assert!(matches!(kind, ActionKind::AttackOff));
            }
            other => panic!("expected Action{{AttackOff}}, got {other:?}"),
        }
    }

    #[test]
    fn cast_with_explicit_target_and_ground_coords() {
        // `/cast 257 99 7 1.0 0.0 2.0` → Tractor (id=257) ground-targeted at (1,0,2).
        match parse_slash_t(
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
        match parse_slash_t("/cast 1", &entities, origin(), Some(7), None) {
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
        match parse_slash_t("/ws 16 7", &entities, origin(), Some(7), None) {
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
        match parse_slash_t("/ja 88", &empty_entities(), origin(), None, None) {
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
        match parse_slash_t("/useitem 0 4 4112", &empty_entities(), origin(), None, None) {
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

    /// `/endevent` aliases all dispatch the same `EndEvent` — the
    /// session loop drains `pending_event_end` for it. Pins the parser
    /// surface so a rename can't silently break the help entry.
    #[test]
    fn endevent_aliases_dispatch_end_event() {
        for input in ["/endevent", "/endevt", "/clearevent", "/clearevt"] {
            match parse_slash_t(input, &empty_entities(), origin(), None, None) {
                SlashOutcome::Command(AgentCommand::EndEvent) => {}
                other => panic!("input {input:?}: expected EndEvent, got {other:?}"),
            }
        }
    }

    /// `/endcutscene` with no arg emits `event_num = None`. The
    /// dispatcher resolves it against the live `DialogState` /
    /// start-zone fallback at send time, not at parse time.
    #[test]
    fn endcutscene_no_arg_returns_none() {
        match parse_slash_t("/endcutscene", &empty_entities(), origin(), None, Some(231)) {
            SlashOutcome::EndCutscene { event_num } => assert_eq!(event_num, None),
            other => panic!("expected EndCutscene{{ None }}, got {other:?}"),
        }
    }

    /// `/endcutscene <csid>` trusts the operator's value — for Bastok
    /// Markets' 0→7 chain the second cutscene needs an explicit
    /// `/endcutscene 7`.
    #[test]
    fn endcutscene_with_explicit_csid_overrides_zone_lookup() {
        match parse_slash_t(
            "/endcutscene 7",
            &empty_entities(),
            origin(),
            None,
            Some(235), // Bastok Markets — zone lookup would return 0
        ) {
            SlashOutcome::EndCutscene { event_num } => assert_eq!(event_num, Some(7)),
            other => panic!("expected EndCutscene{{ Some(7) }}, got {other:?}"),
        }
    }

    /// Bad numeric arg → operator-facing error, not silent.
    #[test]
    fn endcutscene_bad_csid_errors() {
        match parse_slash_t(
            "/endcutscene abc",
            &empty_entities(),
            origin(),
            None,
            Some(231),
        ) {
            SlashOutcome::SystemMessage(msg) => assert!(msg.to_lowercase().contains("bad csid")),
            other => panic!("expected SystemMessage, got {other:?}"),
        }
    }

    /// `/release` and `/unwedge` both emit a chat-routed `!release` so
    /// LSB's `release.lua` runs `player:release()` →
    /// `endCurrentEvent()`. Pins the wire shape and the alias list.
    #[test]
    fn release_aliases_emit_bang_release_chat() {
        for input in ["/release", "/unwedge"] {
            match parse_slash_t(input, &empty_entities(), origin(), None, None) {
                SlashOutcome::Command(AgentCommand::Chat { kind, text }) => {
                    assert_eq!(kind, 0, "input {input:?}: expected Say (kind=0)");
                    assert_eq!(text, "!release", "input {input:?}: unexpected text");
                }
                other => panic!("input {input:?}: expected Chat(!release), got {other:?}"),
            }
        }
    }

    /// Alias coverage: every documented spelling produces the same outcome.
    #[test]
    fn endcutscene_aliases_all_work() {
        for input in ["/endcutscene", "/endcs", "/skipcutscene", "/skipcs"] {
            match parse_slash_t(input, &empty_entities(), origin(), None, Some(231)) {
                SlashOutcome::EndCutscene { event_num } => assert_eq!(event_num, None),
                other => panic!("input {input:?}: expected EndCutscene, got {other:?}"),
            }
        }
    }

    #[test]
    fn raisemenu_accept_and_decline() {
        for (input, expected) in &[("/raisemenu accept", true), ("/raisemenu decline", false)] {
            match parse_slash_t(input, &empty_entities(), origin(), None, None) {
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
            match parse_slash_t(input, &empty_entities(), origin(), None, None) {
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
        match parse_slash_t("/homepointmenu 0", &empty_entities(), origin(), None, None) {
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
            parse_slash_t("/snapshot", &empty_entities(), origin(), None, None),
            SlashOutcome::Command(AgentCommand::Snapshot)
        ));
    }

    #[test]
    fn bank_parses_threshold_and_zoneline() {
        match parse_slash_t(
            "/bank 60 0xDEADBEEF",
            &empty_entities(),
            origin(),
            None,
            None,
        ) {
            SlashOutcome::SystemMessage(_) => {
                // 0xDEADBEEF doesn't parse as plain u32; decimal works.
            }
            _ => {}
        }
        match parse_slash_t("/bank 60 12345", &empty_entities(), origin(), None, None) {
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
        match parse_slash_t("/zonechange 42", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::RequestZoneChange { line_id }) => {
                assert_eq!(line_id, 42);
            }
            other => panic!("expected RequestZoneChange, got {other:?}"),
        }
    }

    #[test]
    fn mhexit_defaults_to_home() {
        match parse_slash_t("/mhexit", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::MogHouseExit { kind }) => {
                assert!(matches!(kind, crate::state::MogHouseExit::Home));
                assert_eq!(kind.wire_pair(), (1, 0));
            }
            other => panic!("expected MogHouseExit::Home, got {other:?}"),
        }
    }

    #[test]
    fn mhexit_region_with_slot() {
        match parse_slash_t("/mhexit bastok 2", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::MogHouseExit { kind }) => {
                assert!(matches!(
                    kind,
                    crate::state::MogHouseExit::Bastok { slot: 2 }
                ));
                assert_eq!(kind.wire_pair(), (2, 2));
            }
            other => panic!("expected MogHouseExit::Bastok, got {other:?}"),
        }
    }

    #[test]
    fn mhexit_special_modes() {
        for (input, expected_mode) in &[
            ("/mhexit 1f", 126u8),
            ("/mhexit 2f", 125),
            ("/mhexit garden", 127),
        ] {
            match parse_slash_t(input, &empty_entities(), origin(), None, None) {
                SlashOutcome::Command(AgentCommand::MogHouseExit { kind }) => {
                    let (_, mode) = kind.wire_pair();
                    assert_eq!(mode, *expected_mode, "input={input}");
                }
                other => panic!("expected MogHouseExit, got {other:?}"),
            }
        }
    }

    #[test]
    fn mhexit_auto_resolves_known_zone() {
        // S. San d'Oria (zone 230) is in the Sandoria region (bit 1); auto
        // picks Option1 = S.Sandy. wire_pair → (1, 1).
        match parse_slash_t("/mhexit auto", &empty_entities(), origin(), None, Some(230)) {
            SlashOutcome::Command(AgentCommand::MogHouseExit { kind }) => {
                assert!(matches!(
                    kind,
                    crate::state::MogHouseExit::Sandoria { slot: 1 }
                ));
                assert_eq!(kind.wire_pair(), (1, 1));
            }
            other => panic!("expected MogHouseExit::Sandoria, got {other:?}"),
        }
    }

    #[test]
    fn mhexit_auto_errors_for_unknown_zone() {
        // Open-world zone (e.g. Ronfaure = 100) isn't in a Mog House region,
        // so `auto` returns a usage error rather than guessing.
        match parse_slash_t("/mhexit auto", &empty_entities(), origin(), None, Some(100)) {
            SlashOutcome::SystemMessage(msg) => {
                assert!(msg.contains("auto"), "got: {msg}");
            }
            other => panic!("expected error SystemMessage, got {other:?}"),
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
            match parse_slash_t(input, &empty_entities(), origin(), None, None) {
                SlashOutcome::AgentControl(op) => assert_eq!(&op, expected, "input: {input}"),
                other => panic!("expected AgentControl for `{input}`, got {other:?}"),
            }
        }
    }

    #[test]
    fn agent_unknown_subcommand_is_system_message() {
        match parse_slash_t("/agent wat", &empty_entities(), origin(), None, None) {
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
            ("/follow Mob", |c| {
                matches!(c, AgentCommand::Follow { target_id: 42, .. })
            }),
            // engage {target_id}
            ("/engage", |c| {
                matches!(c, AgentCommand::Engage { target_id: 42 })
            }),
            // path_to {x, y, z}
            ("/pathto 1 2 3", |c| {
                matches!(c, AgentCommand::PathTo { .. })
            }),
            // cancel
            ("/cancel", |c| matches!(c, AgentCommand::Cancel)),
            // bank_when_full {threshold, mog_house_zoneline}
            ("/bank 60 12345", |c| {
                matches!(
                    c,
                    AgentCommand::BankWhenFull {
                        threshold: 60,
                        mog_house_zoneline: 12345
                    }
                )
            }),
            // chat {kind, text} — covered by per-channel slashes (/s /p etc.)
            ("/s hello", |c| {
                matches!(c, AgentCommand::Chat { kind: 0, .. })
            }),
            ("/p hello", |c| {
                matches!(c, AgentCommand::Chat { kind: 4, .. })
            }),
            // tell {to, text}
            ("/tell Bob hi", |c| matches!(c, AgentCommand::Tell { .. })),
            // request_zone_change {line_id}
            ("/zonechange 42", |c| {
                matches!(c, AgentCommand::RequestZoneChange { line_id: 42 })
            }),
            // snapshot
            ("/snapshot", |c| matches!(c, AgentCommand::Snapshot)),
            // cast (Action::CastMagic)
            ("/cast 1", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::CastMagic { .. },
                        ..
                    }
                )
            }),
            // weaponskill (Action::Weaponskill)
            ("/ws 1", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::Weaponskill { .. },
                        ..
                    }
                )
            }),
            // job_ability (Action::JobAbility)
            ("/ja 1", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::JobAbility { .. },
                        ..
                    }
                )
            }),
            // use_item
            ("/useitem 0 4", |c| {
                matches!(c, AgentCommand::UseItem { .. })
            }),
            // raise_menu (Action::RaiseMenu)
            ("/raisemenu accept", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::RaiseMenu { .. },
                        ..
                    }
                )
            }),
            // tractor_menu (Action::TractorMenu)
            ("/tractormenu accept", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::TractorMenu { .. },
                        ..
                    }
                )
            }),
            // homepoint_menu (Action::HomepointMenu)
            ("/homepointmenu 0", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::HomepointMenu { .. },
                        ..
                    }
                )
            }),
            // disconnect — slash form fires Quit (drops sockets); the
            // MCP tool dispatches AgentCommand::Disconnect through the
            // session loop. Both reach the same exit; the slash carries
            // the GUI side effect of closing the window.
        ];
        for (slash, pred) in &cases {
            let out = parse_slash_t(slash, &entities, pos, cur, None);
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
            let out = parse_slash_t(slash, &empty_entities(), origin(), None, None);
            match out {
                SlashOutcome::SystemMessage(s) => {
                    assert!(
                        s.contains("Slash command reference"),
                        "{slash} missing header"
                    );
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
                    let out = parse_slash_t(&slash, &empty_entities(), origin(), None, None);
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
