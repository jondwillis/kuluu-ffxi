use kuluu_render::{MenuKind, Preset};
use kuluu_snapshot::{Entity as WireEntity, Vec3 as WireVec3};

use kuluu_session::state::{ActionKind, AgentCommand, CheckKind, HealMode, ReqLogoutKind};

use crate::view_native::command_surface::{
    self, CommandSet, CommandSurface, Surface, EXTENSION_HELP_NAMES, EXTENSION_PREFIX,
    FIRST_PARTY_OWNER,
};

const MAX_ZONE_ID: u16 = 600;

/// Retail wraps an action name containing spaces in these (`/ma "Cure II"
/// <me>`), so an argument list cannot simply split on whitespace.
const ARG_QUOTE: char = '"';
const TARGET_TOKEN_OPEN: char = '<';
const TARGET_TOKEN_CLOSE: char = '>';
const SELF_TARGET_TOKEN: &str = "<me>";
const CURRENT_TARGET_TOKEN: &str = "<t>";

/// Party and alliance target tokens, in the one index space the client keeps
/// them in: `<p0>`..`<p5>` are the player's own party, then the two alliance
/// parties. FFXiMain.dll carries the token list in .data as 8-byte records
/// (char* name, u32 packed kind and slot index), and the slot index runs
/// straight through all eighteen without restarting per party (KNOWN_CLIENTS
/// retail-2026-09, RVA 0x00357860).
const PARTY_TARGET_TOKENS: &[&str] = &[
    "<p0>", "<p1>", "<p2>", "<p3>", "<p4>", "<p5>", "<a10>", "<a11>", "<a12>", "<a13>", "<a14>",
    "<a15>", "<a20>", "<a21>", "<a22>", "<a23>", "<a24>", "<a25>",
];

/// vendor/server/src/map/packets/s2c/0x0dd_group_list.cpp — an alliance is
/// three parties of this many.
const PARTY_SLOTS: usize = 6;

/// Retail's sub-target tokens: the bare form opens the cursor on the action's
/// own TARGETTYPE mask, the suffixed forms narrow the candidate set.
const SUB_TARGET_TOKENS: &[(&str, Option<u16>)] = &[
    ("<st>", None),
    (
        "<stpc>",
        Some(
            ffxi_vocab::valid_target::TargetFlags::SELF
                | ffxi_vocab::valid_target::TargetFlags::PLAYER_PARTY
                | ffxi_vocab::valid_target::TargetFlags::PLAYER_ALLIANCE
                | ffxi_vocab::valid_target::TargetFlags::PLAYER,
        ),
    ),
    (
        "<stnpc>",
        Some(
            ffxi_vocab::valid_target::TargetFlags::NPC
                | ffxi_vocab::valid_target::TargetFlags::ENEMY,
        ),
    ),
    (
        "<stpt>",
        Some(
            ffxi_vocab::valid_target::TargetFlags::SELF
                | ffxi_vocab::valid_target::TargetFlags::PLAYER_PARTY,
        ),
    ),
    (
        "<stal>",
        Some(
            ffxi_vocab::valid_target::TargetFlags::SELF
                | ffxi_vocab::valid_target::TargetFlags::PLAYER_PARTY
                | ffxi_vocab::valid_target::TargetFlags::PLAYER_ALLIANCE,
        ),
    ),
];
const BATTLE_TARGET_TOKEN: &str = "<bt>";
const PET_TARGET_TOKEN: &str = "<pet>";

/// Retail target tokens Kuluu parses but has no state to answer with yet, kept
/// apart from a typo so the two report differently.
const UNRESOLVED_TARGET_TOKENS: &[&str] =
    &["<ft>", "<ht>", "<r>", "<scan>", "<lastst>", "<focust>"];

struct SlashCtx<'a> {
    cmd: &'a str,
    surface: &'a CommandSurface,

    rest: &'a str,
    entities: &'a [WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
    zone_id: Option<u16>,
    self_char_id: Option<u32>,
    party: &'a [kuluu_snapshot::PartyMember],
    /// Retail's client-side fishing gate, evaluated by the renderer against the
    /// loaded zone collision (`kuluu_render::fishing_spot`).
    fishing: kuluu_render::fishing_spot::FishingGate,
    /// The reactor's engaged target, retail's `<bt>`.
    battle_target: Option<u32>,
    /// The player's own pet by act_index, retail's `<pet>`.
    self_pet_targid: Option<u16>,
}

struct Command {
    /// A [`CommandSet::Retail`] entry names the long form of every retail
    /// command it answers, and the install's table supplies their aliases.
    /// Every other set owns its whole list, because nothing else names them.
    names: &'static [&'static str],
    set: CommandSet,
    usage: &'static str,
    summary: &'static str,
    handler: fn(&SlashCtx) -> SlashOutcome,
}

impl Command {
    fn prefix(&self) -> &'static str {
        if self.set.is_retail() {
            "/"
        } else {
            EXTENSION_PREFIX
        }
    }
}

fn commands() -> impl Iterator<Item = &'static Command> {
    COMMANDS.iter().flat_map(|(_, cmds)| cmds.iter())
}

fn cycle_npc(c: &SlashCtx, reverse: bool) -> SlashOutcome {
    let kinds = [
        kuluu_snapshot::EntityKind::Npc,
        kuluu_snapshot::EntityKind::Mob,
        kuluu_snapshot::EntityKind::Pet,
    ];
    match cycle_kind_filtered(c.entities, c.self_pos, c.current_target, &kinds, reverse) {
        Some(id) => SlashOutcome::SetTarget(Some(id)),
        None => SlashOutcome::SystemMessage(format!("/{}: no NPC nearby", c.cmd)),
    }
}

fn unknown_command(cmd: &str) -> SlashOutcome {
    SlashOutcome::SystemMessage(format!("unknown command: /{cmd}"))
}

const COMMANDS: &[(&str, &[Command])] = &[
    (
        "Help",
        &[
            Command {
                names: &["help", "?"],
                set: CommandSet::Retail,
                usage: "",
                summary: "list the retail commands this client answers",
                handler: |c| {
                    SlashOutcome::SystemMessage(render_help(c.surface, Surface::Retail))
                },
            },
            Command {
                names: EXTENSION_HELP_NAMES,
                set: CommandSet::Core,
                usage: "",
                summary: "list Kuluu's own commands",
                handler: |c| {
                    SlashOutcome::SystemMessage(render_help(c.surface, Surface::Extension))
                },
            },
        ],
    ),
    (
        "Movement & Navigation",
        &[
            Command {
                names: &["follow"],
                set: CommandSet::Retail,
                usage: "[name]",
                summary: "follow target or current selection",
                handler: |c| match resolve_target_or_current(
                    c.rest,
                    c.entities,
                    c.self_pos,
                    c.current_target,
                ) {
                    Some(id) => SlashOutcome::Command(AgentCommand::Follow {
                        target_id: id,

                        distance: 0.0,
                    }),
                    None => SlashOutcome::SystemMessage("/follow: no target".into()),
                },
            },
            Command {
                names: &["pathto"],
                set: CommandSet::Dev,
                usage: "<x> <y> [z] | <name> | target",
                summary: "pathfind (navmesh, stays on mesh): coords (z optional), fuzzy zone-line/entity, or current target",
                handler: |c| {
                    parse_pathto(
                        c.rest,
                        c.entities,
                        c.self_pos,
                        c.current_target,
                        c.zone_id,
                        false,
                    )
                },
            },
            Command {
                names: &["pathtoforce", "pathtof"],
                set: CommandSet::Dev,
                usage: "<x> <y> [z] | <name> | target",
                summary: "pathfind ignoring collision (straight-lines through walls when no route -- stuck-recovery)",
                handler: |c| {
                    parse_pathto(
                        c.rest,
                        c.entities,
                        c.self_pos,
                        c.current_target,
                        c.zone_id,
                        true,
                    )
                },
            },
            Command {
                names: &["warp"],
                set: CommandSet::Dev,
                usage: "<x> <y> [z] | <name> | target",
                summary: "debug teleport (Move): coords (z optional), fuzzy zone-line/entity, or target",
                handler: |c| {
                    parse_warp(c.rest, c.entities, c.self_pos, c.current_target, c.zone_id)
                },
            },
            Command {
                names: &["zones"],
                set: CommandSet::Dev,
                usage: "",
                summary: "list zone-line destinations from current zone",
                handler: |c| parse_zones(c.zone_id),
            },
            Command {
                names: &["navmesh"],
                set: CommandSet::Dev,
                usage: "[on|off]",
                summary: "toggle the navmesh debug overlay",
                handler: |c| parse_navmesh(c.rest),
            },
            Command {
                names: &["navinfo"],
                set: CommandSet::Dev,
                usage: "",
                summary: "report navmesh snap status at current position",
                handler: |_| SlashOutcome::NavInfo,
            },
            Command {
                names: &["whereami", "pos"],
                set: CommandSet::Dev,
                usage: "",
                summary: "print self position and zone id",
                handler: |c| {
                    SlashOutcome::SystemMessage(format!(
                        "self_pos: x={:.2} y={:.2} z={:.2}  zone={}",
                        c.self_pos.x,
                        c.self_pos.y,
                        c.self_pos.z,
                        c.zone_id.map_or("?".to_string(), |z| z.to_string()),
                    ))
                },
            },
        ],
    ),
    (
        "Combat & Targeting",
        &[
            Command {
                names: &["attack"],
                set: CommandSet::Retail,
                usage: "[name]",
                summary: "engage target (reactor goal)",

                handler: |c| match resolve_action_target(
                    c.rest,
                    c.entities,
                    c.self_pos,
                    c.current_target,
                ) {
                    Some((id, _idx)) => {
                        let Some(ent) = c.entities.iter().find(|e| e.id == id) else {
                            return SlashOutcome::SystemMessage(format!("/{}: no target", c.cmd));
                        };
                        if let Some(line) = crate::view_native::engage::rejection_line(
                            ent,
                            c.self_pos,
                            c.self_char_id,
                            c.party,
                        ) {
                            return SlashOutcome::SystemMessage(line);
                        }
                        SlashOutcome::Command(AgentCommand::Engage { target_id: id })
                    }
                    None => SlashOutcome::SystemMessage(format!("/{}: no target", c.cmd)),
                },
            },
            Command {
                names: &["autoattack"],
                set: CommandSet::Retail,
                usage: "[on|off]",
                summary: "auto-retarget a mob hitting self when the target dies",
                handler: |c| parse_autoattack(c.rest),
            },
            Command {
                names: &["attackoff"],
                set: CommandSet::Retail,
                usage: "",
                summary: "one-shot attack-off packet on current target",
                handler: |c| match c.current_target {
                    Some(id) => match c.entities.iter().find(|e| e.id == id) {
                        Some(ent) => SlashOutcome::Command(AgentCommand::Action {
                            target_id: ent.id,
                            target_index: ent.act_index,
                            kind: ActionKind::AttackOff,
                        }),
                        None => {
                            SlashOutcome::SystemMessage(format!("/{}: target not in zone", c.cmd))
                        }
                    },
                    None => SlashOutcome::SystemMessage(format!("/{}: no target", c.cmd)),
                },
            },
            Command {
                names: &["dig"],
                set: CommandSet::Retail,
                usage: "",
                summary: "chocobo dig at current position (must be mounted on a chocobo)",
                handler: |c| {
                    let self_id = c.self_char_id.unwrap_or(0);
                    let self_index = c
                        .entities
                        .iter()
                        .find(|e| e.id == self_id)
                        .map(|e| e.act_index)
                        .unwrap_or(0);
                    SlashOutcome::Command(AgentCommand::Action {
                        target_id: self_id,
                        target_index: self_index,
                        kind: ActionKind::ChocoboDig,
                    })
                },
            },
            Command {
                names: &["assist"],
                set: CommandSet::Retail,
                usage: "[name]",
                summary: "assist target (inherit their target)",
                handler: |c| match resolve_action_target(
                    c.rest,
                    c.entities,
                    c.self_pos,
                    c.current_target,
                ) {
                    Some((id, idx)) => SlashOutcome::Command(AgentCommand::Action {
                        target_id: id,
                        target_index: idx,
                        kind: ActionKind::Assist,
                    }),
                    None => SlashOutcome::SystemMessage("/assist: no target".into()),
                },
            },
            Command {
                names: &["target"],
                set: CommandSet::Retail,
                usage: "[name]",
                summary: "set or clear current target",
                handler: |c| {
                    if c.rest.is_empty() {
                        SlashOutcome::SetTarget(None)
                    } else {
                        match resolve_name(c.rest, c.entities, c.self_pos) {
                            Some(ent) => SlashOutcome::SetTarget(Some(ent.id)),
                            None => SlashOutcome::SystemMessage(format!(
                                "/target: no entity '{}'",
                                c.rest
                            )),
                        }
                    }
                },
            },
            Command {
                names: &["targetnpc"],
                set: CommandSet::Retail,
                usage: "",
                summary: "cycle nearest NPC/mob/pet",
                handler: |c| cycle_npc(c, false),
            },
            Command {
                names: &["targetbnpc"],
                set: CommandSet::Retail,
                usage: "",
                summary: "cycle nearest enemy (mobs only)",

                handler: |c| {
                    let kinds = [kuluu_snapshot::EntityKind::Mob];
                    match cycle_kind_filtered(
                        c.entities,
                        c.self_pos,
                        c.current_target,
                        &kinds,
                        false,
                    ) {
                        Some(id) => SlashOutcome::SetTarget(Some(id)),
                        None => SlashOutcome::SystemMessage("/targetenemy: no enemy nearby".into()),
                    }
                },
            },
            Command {
                names: &["debug", "dbg", "nearby", "entities"],
                set: CommandSet::Dev,
                usage: "[name|id|heights]",
                summary: "dump current target + nearby entities (or one entity in detail)",
                handler: |c| parse_debug(c.rest, c.entities, c.self_pos, c.current_target),
            },
            Command {
                names: &["check", "checkname", "checkparam"],
                set: CommandSet::Retail,
                usage: "[name]",
                summary: "check target -- strength / name / parameters",
                handler: |c| match resolve_action_target(
                    c.rest,
                    c.entities,
                    c.self_pos,
                    c.current_target,
                ) {
                    Some((id, idx)) => SlashOutcome::Command(AgentCommand::CheckTarget {
                        target_id: id,
                        target_index: idx,
                        kind: match c.cmd {
                            "checkname" => CheckKind::CheckName,
                            "checkparam" => CheckKind::CheckParam,
                            _ => CheckKind::Check,
                        },
                    }),
                    None => SlashOutcome::SystemMessage(format!("/{}: no target", c.cmd)),
                },
            },
            Command {
                names: &["magic"],
                set: CommandSet::Retail,
                usage: "<spell> [target]",
                summary: "cast a spell",
                handler: parse_cast,
            },
            Command {
                names: &["weaponskill"],
                set: CommandSet::Retail,
                usage: "<name> [target]",
                summary: "weapon skill",
                handler: parse_weaponskill,
            },
            Command {
                names: &["jobability"],
                set: CommandSet::Retail,
                usage: "<name> [target]",
                summary: "job ability",
                handler: parse_job_ability,
            },
            Command {
                names: &["shoot"],
                set: CommandSet::Retail,
                usage: "[target]",
                summary: "ranged attack",
                handler: parse_ranged_attack,
            },
            Command {
                names: &["item"],
                set: CommandSet::Retail,
                usage: "<name> [target]",
                summary: "use an item",
                handler: parse_use_item,
            },
            Command {
                names: &["equip"],
                set: CommandSet::Retail,
                usage: "[slot item]",
                summary: "no-arg form opens Equipment menu; <slot> <item> equips directly (Stage 4)",

                handler: |c| {
                    if c.rest.is_empty() {
                        SlashOutcome::OpenMenu(MenuKind::Equipment)
                    } else if c.cmd == "equip" {
                        SlashOutcome::SystemMessage(
                            "/equip <slot> <item>: not yet wired (Stage 4); /equip with no args opens the Equipment menu".into(),
                        )
                    } else {
                        unknown_command(c.cmd)
                    }
                },
            },
            Command {
                names: &["cancel"],
                set: CommandSet::Agent,
                usage: "",
                summary: "cancel current reactor goal / action",
                handler: |_| SlashOutcome::Command(AgentCommand::Cancel),
            },
            Command {
                names: &["raw"],
                set: CommandSet::Agent,
                usage: "<attack|attackoff> [name]",
                summary: "low-level Action packet (bypasses reactor)",

                handler: |c| parse_raw(c.rest, c.entities, c.self_pos, c.current_target),
            },
        ],
    ),
    (
        "Chat",
        &[
            Command {
                names: &["say"],
                set: CommandSet::Retail,
                usage: "<text>",
                summary: "say (local chat)",
                handler: |c| chat_or_empty(c.rest, 0, "/s"),
            },
            Command {
                names: &["party"],
                set: CommandSet::Retail,
                usage: "<text>",
                summary: "party chat",
                handler: |c| chat_or_empty(c.rest, 4, "/p"),
            },
            Command {
                names: &["shout"],
                set: CommandSet::Retail,
                usage: "<text>",
                summary: "shout chat",
                handler: |c| chat_or_empty(c.rest, 1, "/sh"),
            },
            Command {
                names: &["linkshell"],
                set: CommandSet::Retail,
                usage: "<text>",
                summary: "linkshell chat",
                handler: |c| chat_or_empty(c.rest, 5, "/l"),
            },
            Command {
                names: &["tell"],
                set: CommandSet::Retail,
                usage: "<name> <text>",
                summary: "tell another player",
                handler: |c| parse_tell(c.rest),
            },
        ],
    ),
    (
        "Emotes",
        &[
            Command {
                names: &["emote"],
                set: CommandSet::Retail,
                usage: "<text>",
                summary: "free-form custom emote text (zone chat channel 8)",
                handler: |c| chat_or_empty(c.rest, ffxi_proto::map::chat_kind::EMOTION, "/emote"),
            },
            Command {
                names: &["jobemote"],
                set: CommandSet::Retail,
                usage: "[war|mnk|...] [motion|text]",
                summary: "job gesture (defaults to current main job; needs its JOB_GESTURE key item)",
                handler: |c| parse_jobemote(c.rest, c),
            },
            Command {
                names: &["bell"],
                set: CommandSet::Retail,
                usage: "<c4..c6|6..30> [motion|text]",
                summary: "ring an equipped bell at a note (two octaves from c4)",
                handler: |c| parse_bell(c.rest, c),
            },
            Command {
                names: &["emotelist"],
                set: CommandSet::Dev,
                usage: "",
                summary: "request job-emote/chair unlock flags (c2s 0x119)",
                handler: |_| SlashOutcome::Command(AgentCommand::RequestEmoteList),
            },
        ],
    ),
    (
        "Status & Menus",
        &[
            // "kneel" is deliberately NOT an alias here: retail /kneel is the
            // canned emote (id 3), resolved via the emote fallback.
            Command {
                names: &["sit"],
                set: CommandSet::Retail,
                usage: "[on|off]",
                summary: "sit (locks movement; any movement key stands)",

                handler: |c| parse_sit(c.rest),
            },
            Command {
                names: &["heal"],
                set: CommandSet::Retail,
                usage: "[on|off]",
                summary: "toggle resting (CAMP)",

                handler: |c| parse_heal(c.rest),
            },
            Command {
                names: &["dismount"],
                set: CommandSet::Retail,
                usage: "",
                summary: "dismount the chocobo (must be mounted)",
                handler: |c| {
                    let self_id = c.self_char_id.unwrap_or(0);
                    let self_index = c
                        .entities
                        .iter()
                        .find(|e| e.id == self_id)
                        .map(|e| e.act_index)
                        .unwrap_or(0);
                    SlashOutcome::Command(AgentCommand::Action {
                        target_id: self_id,
                        target_index: self_index,
                        kind: ActionKind::Dismount,
                    })
                },
            },
            Command {
                names: &["endevent", "endevt", "clearevent", "clearevt"],
                set: CommandSet::Dev,
                usage: "",
                summary: "flush pending NPC events (unblock /logout)",

                handler: |_| SlashOutcome::Command(AgentCommand::EndEvent),
            },
            Command {
                names: &["endcutscene", "endcs", "skipcutscene", "skipcs"],
                set: CommandSet::Dev,
                usage: "[csid]",
                summary: "end a forced cutscene (new-char intro, etc.)",

                handler: |c| parse_endcutscene(c.rest),
            },
            Command {
                names: &["bank"],
                set: CommandSet::Retail,
                usage: "<subcommand>",
                summary: "gil-bank operations",
                handler: |c| parse_bank(c.rest),
            },
            Command {
                names: &["minimap", "mm"],
                set: CommandSet::Core,
                usage: "[show|hide|toggle|mode <top|retail|auto>|cull <N>|zoom ...]",
                summary: "drive the minimap HUD (visibility, backend, cull, zoom)",
                handler: |c| parse_minimap(c.rest),
            },
            Command {
                names: &["map"],
                set: CommandSet::Retail,
                usage: "",
                summary: "open the full-screen Map + Widescan menu",
                handler: |c| {
                    if c.rest.is_empty() {
                        SlashOutcome::OpenMenu(MenuKind::Map)
                    } else {
                        unknown_command(c.cmd)
                    }
                },
            },
            #[cfg(debug_assertions)]
            Command {
                names: &["widescan", "wscan"],
                set: CommandSet::Dev,
                usage: "",
                summary: "(dev) request the server wide-scan tracking list and echo it to chat",
                handler: |_| SlashOutcome::Widescan,
            },
            Command {
                names: &["clock"],
                set: CommandSet::Retail,
                usage: "[show|hide|toggle]",
                summary: "toggle the Vana'diel clock widget (same state as the Current Time menu entry)",
                handler: |c| parse_clock(c.rest),
            },
            Command {
                names: &["mutebgm"],
                set: CommandSet::Retail,
                usage: "[on|off]",
                summary: "mute background music; survives logout",
                handler: |c| parse_mute(c.rest, SoundOp::SetBgm),
            },
            Command {
                names: &["mutese"],
                set: CommandSet::Retail,
                usage: "[on|off]",
                summary: "mute sound effects; survives logout",
                handler: |c| parse_mute(c.rest, SoundOp::SetSfx),
            },
        ],
    ),
    (
        "Fishing",
        &[
            Command {
                names: &["fish"],
                set: CommandSet::Retail,
                usage: "",
                summary: "cast a line (drives the fishing mini-game)",
                handler: |c| match c.fishing.refusal() {
                    // Retail refuses locally rather than asking the server, so
                    // the rod/bait/water checks never leave the client
                    // (research/xim FishingStartEvent.kt).
                    Some(msg) => SlashOutcome::SystemMessage(msg.to_string()),
                    None => SlashOutcome::Command(AgentCommand::Fish),
                },
            },
        ],
    ),
    (
        "Session",
        &[
            Command {
                names: &["logout"],
                set: CommandSet::Retail,
                usage: "[on|off]",
                summary: "request logout (30s LeaveGame timer)",

                handler: |c| parse_reqlogout(c.rest,  false),
            },
            Command {
                names: &["shutdown"],
                set: CommandSet::Retail,
                usage: "[on|off]",
                summary: "request shutdown (LeaveGame, then close)",
                handler: |c| parse_reqlogout(c.rest,  true),
            },
            Command {
                names: &["exit"],
                set: CommandSet::Core,
                usage: "",
                summary: "request shutdown and close the window now",
                handler: |_| SlashOutcome::Quit,
            },
        ],
    ),
    (
        "Debug & Tooling",
        &[
            Command {
                names: &["overlay"],
                set: CommandSet::Dev,
                usage: "[list|add <dir>|remove <n>|clear|reset]",
                summary: "inspect and override the DAT overlay search path",
                handler: |c| parse_overlay(c.rest),
            },
            Command {
                names: &["actordiag"],
                set: CommandSet::Dev,
                usage: "[target]",
                summary: "diagnose missing PC body parts (head/face) against the install",
                handler: |c| parse_actordiag(c.rest),
            },
            Command {
                names: &["snapshot"],
                set: CommandSet::Agent,
                usage: "",
                summary: "emit a one-shot scene snapshot",
                handler: |_| SlashOutcome::Command(AgentCommand::Snapshot),
            },
            Command {
                names: &["zonechange", "rzc"],
                set: CommandSet::Agent,
                usage: "<id>",
                summary: "request zone change (debug)",
                handler: |c| parse_zone_change(c.rest),
            },
            Command {
                names: &["agent"],
                set: CommandSet::Agent,
                usage: "<pause|resume|status>",
                summary: "human-in-control flag for agent commands",
                handler: |c| parse_agent(c.rest),
            },
            Command {
                names: &["keybinds", "keybind", "binds"],
                set: CommandSet::Dev,
                usage: "<preset|list|reset>",
                summary: "manage keybind presets",
                handler: |c| parse_keybinds(c.rest),
            },
            Command {
                names: &["load_mmb", "loadmmb"],
                set: CommandSet::Dev,
                usage: "<file_id> <chunk_idx>",
                summary: "spawn MMB model at self_pos (debug overlay)",
                handler: |c| parse_load_mmb(c.rest, c.self_pos),
            },
            Command {
                names: &["load_mmb_on", "loadmmbon"],
                set: CommandSet::Dev,
                usage: "<entity_id> <file_id> <chunk_idx>",
                summary: "attach MMB model under a tracked entity (debug)",
                handler: |c| parse_load_mmb_on(c.rest),
            },
            Command {
                names: &["load_mzb", "loadmzb"],
                set: CommandSet::Dev,
                usage: "<file_id> [chunk_idx]",
                summary: "load MZB mesh-library at self_pos (debug overlay)",
                handler: |c| parse_load_mzb(c.rest, c.self_pos),
            },
            Command {
                names: &["subarea", "subareas"],
                set: CommandSet::Dev,
                usage: "[<sub_area_id>|here]",
                summary: "list this zone's building interiors, or load one as an overlay (debug)",
                handler: |c| parse_sub_area(c.rest, c.self_pos),
            },
            Command {
                names: &["fps"],
                set: CommandSet::Dev,
                usage: "<max>",
                summary: "set target frame rate",
                handler: |c| parse_fps(c.rest),
            },
            Command {
                names: &["capture"],
                set: CommandSet::Dev,
                usage: "[on|off|toggle]",
                summary: "screen-capture-friendly mode (disables framepace; avoids QuickTime lockup)",
                handler: |c| parse_capture(c.rest),
            },
            Command {
                names: &["screenshot", "ss"],
                set: CommandSet::Dev,
                usage: "[path.png]",
                summary: "capture primary window to PNG (default: screenshot-N.png in CWD)",
                handler: |c| parse_screenshot(c.rest),
            },
            Command {
                names: &["drawdistance", "dd"],
                set: CommandSet::Dev,
                usage: "[setworld|setmob] [N]",
                summary: "set draw distance",
                handler: |c| parse_drawdistance(c.rest),
            },
            Command {
                names: &["copy"],
                set: CommandSet::Dev,
                usage: "[n]",
                summary: "copy the last n system-toast lines to the clipboard (default 1)",

                handler: |c| {
                    if c.rest.is_empty() {
                        SlashOutcome::CopyToasts { n: 1 }
                    } else {
                        match c.rest.parse::<usize>() {
                            Ok(n) if n > 0 => SlashOutcome::CopyToasts { n },
                            _ => SlashOutcome::SystemMessage(format!(
                                "/copy: expected a positive integer, got `{}`",
                                c.rest
                            )),
                        }
                    }
                },
            },
            Command {
                names: &["bgm"],
                set: CommandSet::Dev,
                usage: "<track_id>",
                summary: "audition a BGM track id (synthetic 0x05F slot 0)",
                handler: |c| match c.rest.parse::<u16>() {
                    Ok(id) => SlashOutcome::PlayBgm { track_id: id },
                    Err(_) => SlashOutcome::SystemMessage("/bgm <track_id>".into()),
                },
            },
            Command {
                names: &["sfx"],
                set: CommandSet::Dev,
                usage: "<se_id>",
                summary: "fire a one-shot SE by numeric id",
                handler: |c| match c.rest.parse::<u32>() {
                    Ok(id) => SlashOutcome::PlaySfx { se_id: id },
                    Err(_) => SlashOutcome::SystemMessage("/sfx <se_id>".into()),
                },
            },
            Command {
                names: &["look"],
                set: CommandSet::Dev,
                usage: "[name|act_index]",
                summary: "print decoded LookData (race/gear) for an entity",
                handler: |c| parse_look(c.rest, c.entities, c.self_pos, c.current_target),
            },
            Command {
                names: &["zonegeom"],
                set: CommandSet::Dev,
                usage: "[off|collision|all|toggle]",
                summary: "MZB overlay visibility (collision-only vs decorative)",
                handler: |c| parse_zonegeom(c.rest),
            },
            Command {
                names: &["zoneline", "zonelines"],
                set: CommandSet::Dev,
                usage: "[off|pillar|gate|toggle]",
                summary: "zone-line trigger markers -- off (retail-faithful, default), pillar (debug column), or gate (real oriented footprint)",
                handler: |c| parse_zoneline(c.rest),
            },
            Command {
                names: &["weather"],
                set: CommandSet::Dev,
                usage: "<id|name>",
                summary: "client-side weather override (e.g. `rain`, `none`, `12`); lasts until the next server WEATHER packet",

                handler: |c| parse_weather(c.rest),
            },
            Command {
                names: &["debugchat"],
                set: CommandSet::Dev,
                usage: "[on|off|toggle]",
                summary: "show or hide the debug chat window",
                handler: |c| parse_debugchat(c.rest),
            },
            Command {
                names: &["devhud"],
                set: CommandSet::Dev,
                usage: "[on|off|toggle]",
                summary: "stage + diagnostics bars (top/bottom telemetry). Per-panel overlays (perf, target cycle, mesh, netstat) live in the in-game Debug menu",
                handler: |c| parse_devhud(c.rest),
            },
            Command {
                names: &["netstat", "network"],
                set: CommandSet::Dev,
                usage: "[on|off|toggle]",
                summary: "network health indicator (S/R baud, connection %, send/recv arrows)",
                handler: |c| parse_netstat(c.rest),
            },
            Command {
                names: &["noclip"],
                set: CommandSet::Dev,
                usage: "[on|off|toggle]",
                summary: "debug: bypass client-side wall collision (grounding stays on); same state as the Debug menu NoClip row",
                handler: |c| parse_noclip(c.rest),
            },
            Command {
                names: &["renderscale", "rscale"],
                set: CommandSet::Dev,
                usage: "[25-200 | 0.25-2.0]",
                summary: "3D render scale: <100% renders the world at lower res and upscales (perf); >100% supersamples. HUD stays native. Bare `\u{002F}\u{002F}renderscale` reports it.",
                handler: |c| parse_renderscale(c.rest),
            },
            Command {
                names: &["lights", "lanterns"],
                set: CommandSet::Dev,
                usage: "[on|off | shadowed N | flicker on|off]",
                summary: "Enhanced dynamic lights: shadow maps from the N nearest DAT lamps; bare `\u{002F}\u{002F}lights` lists state",
                handler: |c| parse_lights(c.rest),
            },
        ],
    ),
];

/// The two surfaces list separately: `/?` answers about the client the player
/// installed, and says where the rest lives. Retail entries also list their
/// other spellings, which come from the install's table: a retail command's
/// other spellings are the install's answer, not ours.
fn render_help(surface: &CommandSurface, which: Surface) -> String {
    let mut out = String::from(match which {
        Surface::Retail => "=== Retail commands ===",
        Surface::Extension => "=== Kuluu commands ===",
    });
    for (category, entries) in COMMANDS {
        let listed: Vec<&Command> = entries
            .iter()
            .filter(|e| listed_on(e, surface, which))
            .collect();
        if listed.is_empty() {
            continue;
        }
        out.push_str("\n[");
        out.push_str(category);
        out.push(']');
        for entry in listed {
            out.push_str("\n  ");
            for (i, name) in entry.names.iter().enumerate() {
                if i > 0 {
                    out.push_str(" | ");
                }
                out.push_str(entry.prefix());
                out.push_str(name);
                if !entry.set.is_retail() {
                    continue;
                }
                for alias in surface.alias_group(name).into_iter().filter(|a| a != name) {
                    out.push_str(" | /");
                    out.push_str(alias);
                }
            }
            if !entry.usage.is_empty() {
                out.push(' ');
                out.push_str(entry.usage);
            }
            out.push_str(" -- ");
            out.push_str(entry.summary);
        }
    }
    if which == Surface::Retail {
        out.push_str("\nKuluu's own commands are typed with ");
        out.push_str(EXTENSION_PREFIX);
        out.push_str(" -- see ");
        out.push_str(EXTENSION_PREFIX);
        out.push_str(EXTENSION_HELP_NAMES[0]);
    }
    out
}

fn listed_on(entry: &Command, surface: &CommandSurface, which: Surface) -> bool {
    match which {
        Surface::Retail => entry.set.is_retail(),
        Surface::Extension => {
            !entry.set.is_retail()
                && (is_extension_help(entry) || surface.enabled.is_enabled(entry.set))
        }
    }
}

fn is_extension_help(entry: &Command) -> bool {
    !entry.set.is_retail() && entry.names == EXTENSION_HELP_NAMES
}

#[derive(Debug, Clone)]
pub enum SlashOutcome {
    Command(AgentCommand),
    Commands(Vec<AgentCommand>),

    SetTarget(Option<u32>),

    Quit,
    SystemMessage(String),
    PlayBgm {
        track_id: u16,
    },

    PlaySfx {
        se_id: u32,
    },

    ToggleNavmesh(Option<bool>),

    SetSitStance(SitToggle),

    ApplyKeybinds(KeybindUpdate),

    OpenMenu(MenuKind),

    /// The action wants retail's sub-target cursor: `<st>` and its narrowed
    /// forms, or an action typed with no target argument.
    OpenSubTarget {
        action: kuluu_render::input_mode::SubTargetAction,
        /// A token filter (`<stpc>` etc.) applied alongside the action's TARGETTYPE mask.
        narrow: Option<ffxi_vocab::valid_target::TargetFlags>,
    },

    /// Dev-only (the `/widescan` command is gated to debug builds); the
    /// retail-faithful path is the Map screen's Wide Scan submenu.
    #[cfg(debug_assertions)]
    Widescan,

    NavInfo,

    AgentControl(AgentControlOp),

    LoadMmb {
        file_id: u32,
        chunk_idx: usize,
        world_pos: WireVec3,

        entity_id: Option<u32>,
    },

    LoadMzb {
        file_id: u32,
        chunk_idx: Option<usize>,
        world_pos: WireVec3,
    },

    SubArea {
        op: SubAreaOp,
        self_pos: WireVec3,
    },

    SetDrawDistance(DrawDistanceOp),

    SetZoneGeom(Option<kuluu_render::dat_mzb::ZoneGeomMode>),

    SetCameraCollisionSource(Option<kuluu_render::dat_mzb::CameraCollisionSource>),

    SetDevHud(Option<bool>),

    SetDebugChat(Option<bool>),

    SetNetStatus(Option<bool>),

    SetNoClip(Option<bool>),

    SetAutoAttack(Option<bool>),

    SetVanaClock(Option<bool>),

    /// `Some(scale)` sets the 3D render scale (0.25-2.0); `None` reports it.
    SetRenderScale(Option<f32>),

    SetZoneLines(ZoneLineOp),

    SetLights(LightsOp),

    SetMinimap(MinimapOp),

    SetSound(SoundOp),

    Overlay(OverlayOp),

    SetTargetFps(Option<u32>),

    SetCaptureMode(Option<bool>),

    DebugHeights,

    /// Chat-report the look -> file-id -> DAT -> mesh -> texture chain for
    /// self (or the current target), so a field "no head" report is
    /// diagnosable from a screenshot (kuluu-39fi).
    ActorDiag {
        use_target: bool,
    },

    Screenshot {
        path: Option<String>,
    },

    EndCutscene {
        event_num: Option<u16>,
    },

    SetWeatherClient(kuluu_snapshot::Weather),

    CopyToasts {
        n: usize,
    },
}

const START_ZONE_CUTSCENE: &[(u16, u16)] = &[
    (235, 0),
    (234, 1),
    (236, 1),
    (231, 535),
    (230, 503),
    (232, 500),
    (238, 531),
    (241, 367),
    (240, 305),
];

pub(crate) fn start_zone_cutscene(zone_id: u16) -> Option<u16> {
    START_ZONE_CUTSCENE
        .iter()
        .find_map(|&(z, csid)| (z == zone_id).then_some(csid))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawDistanceOp {
    Show,
    SetWorld(f32),
    SetMob(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MinimapOp {
    Status,

    Show,

    Hide,

    Toggle,

    ModeTopDown,

    ModeRetail,

    ModeAuto,

    SetCull(f32),

    ZoomIn,

    ZoomOut,

    ZoomFit,

    ZoomSet(f32),

    ZoomReset,
}

/// `/overlay` verbs. The list is the DAT search path private servers ship their
/// client changes in; see `ffxi_dat::discover_overlays`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayOp {
    /// Report the active list and where it came from.
    List,
    /// Append a directory and persist the result as an override.
    Add(std::path::PathBuf),
    /// Drop the 1-based entry `n` from the active list and persist.
    Remove(usize),
    /// Persist an empty list -- the way to run a private-server install with the
    /// server's overlays off, which is distinct from `Reset`.
    Clear,
    /// Forget the override and go back to what discovery finds.
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundOp {
    SetBgm(Option<bool>),

    SetSfx(Option<bool>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SitToggle {
    Toggle,

    On,

    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentControlOp {
    Pause,

    Resume,

    Status,
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeybindUpdate {
    Preset(Preset),

    Reset,

    List,
}

/// Parses a typed slash command and runs it. Retail dispatches on the
/// command id, so a handler is reached by the long form whichever alias was
/// typed; with no table the typed word stands in, which still reaches every
/// long form.
pub fn parse_slash(
    buffer: &str,
    surface: &CommandSurface,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
    zone_id: Option<u16>,
    self_char_id: Option<u32>,
    party: &[kuluu_snapshot::PartyMember],
    fishing: kuluu_render::fishing_spot::FishingGate,
    battle_target: Option<u32>,
    self_pet_targid: Option<u16>,
) -> SlashOutcome {
    let Some(typed) = command_surface::classify(buffer) else {
        return SlashOutcome::SystemMessage("empty command".into());
    };

    let word = match typed.surface {
        Surface::Retail => surface.canonical(&typed.word).to_owned(),
        Surface::Extension => typed.word.clone(),
    };

    let ctx = SlashCtx {
        cmd: &word,
        surface,
        rest: typed.rest,
        entities,
        self_pos,
        current_target,
        zone_id,
        self_char_id,
        party,
        fishing,
        battle_target,
        self_pet_targid,
    };

    match typed.surface {
        Surface::Retail => dispatch_retail(surface, &word, &ctx),
        Surface::Extension => dispatch_extension(surface, typed.owner, &word, &ctx),
    }
}

fn dispatch_retail(surface: &CommandSurface, word: &str, ctx: &SlashCtx) -> SlashOutcome {
    if let Some(command) = commands().find(|c| c.set.is_retail() && c.names.contains(&word)) {
        return (command.handler)(ctx);
    }
    // Every emote is a command of its own (/wave, /bow, ...). The scraped names
    // come from LSB's enum, which spells some of them differently from the
    // client (Yes for /nod, Goodbye for /farewell), so the whole alias group is
    // tried rather than just the word.
    if let Some(outcome) = canned_emote(surface, word, ctx) {
        return outcome;
    }
    retail_miss(surface, word)
}

/// A `/word` no handler claimed. Saying which of the three reasons it was is
/// the difference between "this client has no such command", "Kuluu has not
/// built it yet" and "you wanted the extension surface".
fn retail_miss(surface: &CommandSurface, word: &str) -> SlashOutcome {
    if surface.id_for(word).is_some() {
        return SlashOutcome::SystemMessage(format!("/{word}: not supported yet"));
    }
    match commands().find(|c| !c.set.is_retail() && c.names.contains(&word)) {
        Some(_) => SlashOutcome::SystemMessage(format!(
            "unknown command: /{word} -- did you mean {EXTENSION_PREFIX}{word}?"
        )),
        None => unknown_command(word),
    }
}

fn dispatch_extension(
    surface: &CommandSurface,
    owner: Option<&str>,
    word: &str,
    ctx: &SlashCtx,
) -> SlashOutcome {
    if let Some(owner) = owner.filter(|o| !o.eq_ignore_ascii_case(FIRST_PARTY_OWNER)) {
        return SlashOutcome::SystemMessage(format!("{EXTENSION_PREFIX}{owner}: no such owner"));
    }
    match commands().find(|c| !c.set.is_retail() && c.names.contains(&word)) {
        Some(command) if is_extension_help(command) || surface.enabled.is_enabled(command.set) => {
            (command.handler)(ctx)
        }
        Some(command) => SlashOutcome::SystemMessage(format!(
            "{EXTENSION_PREFIX}{word}: the {} command set is off",
            command.set.word()
        )),
        None => SlashOutcome::SystemMessage(format!("unknown command: {EXTENSION_PREFIX}{word}")),
    }
}

/// Resolve the emote target: the currently selected entity, or untargeted.
fn emote_target(ctx: &SlashCtx) -> (Option<u32>, Option<u16>) {
    let ent = ctx
        .current_target
        .and_then(|id| ctx.entities.iter().find(|e| e.id == id));
    (ent.map(|e| e.id), ent.map(|e| e.act_index))
}

/// `[motion|text]` trailing argument -> EmoteMode (default All; XiPackets
/// client 0x005D: 'motion' -> 2, 'text' -> 1).
fn parse_emote_mode(arg: &str) -> Option<u8> {
    use ffxi_proto::map::emote::mode;
    match arg {
        "" => Some(mode::ALL),
        "motion" => Some(mode::MOTION),
        "text" => Some(mode::TEXT),
        _ => None,
    }
}

fn emote_outcome(ctx: &SlashCtx, emote_id: u8, mode_arg: &str, param: u16) -> SlashOutcome {
    let Some(mode) = parse_emote_mode(mode_arg) else {
        return SlashOutcome::SystemMessage(format!(
            "/{}: expected `motion` or `text`, got `{mode_arg}`",
            ctx.cmd
        ));
    };
    let (target_id, target_index) = emote_target(ctx);
    SlashOutcome::Command(AgentCommand::Emote {
        emote_id,
        mode,
        param,
        target_id,
        target_index,
    })
}

/// A bare `/wave`-style command: the word is a scraped emote name. `None` when
/// it isn't one (falls through to unknown-command).
fn parse_canned_emote(cmd: &str, rest: &str, ctx: &SlashCtx) -> Option<SlashOutcome> {
    use ffxi_proto::map::emote;
    let id = ffxi_vocab::emote_names::id_for_command(cmd)?;
    if emote::HELM_ONLY.contains(&id) || id == emote::BELL || id == emote::JOB {
        // HELM ids are server-initiated; Bell/Job have their own commands.
        return None;
    }
    Some(emote_outcome(ctx, id, rest, 0))
}

/// The same, reached through the install's alias group so a name LSB spells
/// differently still lands: `/nod` and `/yes` are both emote 7, but the enum
/// only calls it `Yes`.
fn canned_emote(surface: &CommandSurface, word: &str, ctx: &SlashCtx) -> Option<SlashOutcome> {
    std::iter::once(word)
        .chain(surface.alias_group(word))
        .find_map(|name| parse_canned_emote(name, ctx.rest, ctx))
}

fn parse_jobemote(rest: &str, ctx: &SlashCtx) -> SlashOutcome {
    use ffxi_proto::map::emote;
    let mut parts = rest.split_whitespace();
    let job_arg = parts.next().unwrap_or("");
    let mode_arg = parts.next().unwrap_or("");
    let job_id = if job_arg.is_empty() {
        ctx.self_char_id
            .and_then(|id| ctx.party.iter().find(|m| m.id == id))
            .map(|m| m.main_job as u16)
            .unwrap_or(0)
    } else {
        resolve_job_id(job_arg).unwrap_or(0)
    };
    if job_id == 0 {
        return SlashOutcome::SystemMessage(format!(
            "/jobemote: unknown job `{job_arg}` (use WAR/MNK/... or omit for main job)"
        ));
    }
    emote_outcome(
        ctx,
        emote::JOB,
        mode_arg,
        emote::JOB_PARAM_BASE + (job_id - 1),
    )
}

fn resolve_job_id(arg: &str) -> Option<u16> {
    if let Ok(id) = arg.parse::<u16>() {
        return ffxi_vocab::job_names::lookup(id).map(|_| id);
    }
    (1..=u8::MAX as u16).find(|&id| {
        ffxi_vocab::job_names::abbrev(id).is_some_and(|a| a.eq_ignore_ascii_case(arg))
            || ffxi_vocab::job_names::lookup(id).is_some_and(|n| n.eq_ignore_ascii_case(arg))
    })
}

fn parse_bell(rest: &str, ctx: &SlashCtx) -> SlashOutcome {
    use ffxi_proto::map::emote;
    let mut parts = rest.split_whitespace();
    let note_arg = parts.next().unwrap_or("");
    let mode_arg = parts.next().unwrap_or("");
    let Some(param) = parse_bell_note(note_arg) else {
        return SlashOutcome::SystemMessage(format!(
            "/bell: bad note `{note_arg}` (c4..c6, e.g. c4 d#4 eb5, or {}..{})",
            emote::BELL_NOTE_MIN,
            emote::BELL_NOTE_MAX
        ));
    };
    emote_outcome(ctx, emote::BELL, mode_arg, param)
}

/// A bell note as its wire Param: raw 6..=30, or a note name over the
/// two-octave c4..c6 range (c4 = 6, chromatic; the retail parser's exact
/// syntax is a retail unknown, bead kuluu-d4u).
fn parse_bell_note(arg: &str) -> Option<u16> {
    use ffxi_proto::map::emote::{BELL_NOTE_MAX, BELL_NOTE_MIN};
    let in_range = |p: u16| (BELL_NOTE_MIN..=BELL_NOTE_MAX).contains(&p).then_some(p);
    if let Ok(raw) = arg.parse::<u16>() {
        return in_range(raw);
    }
    let lower = arg.to_ascii_lowercase();
    let mut chars = lower.chars();
    let letter = chars.next()?;
    let semitone: i16 = match letter {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let mut next = chars.next()?;
    let accidental = match next {
        '#' => {
            next = chars.next()?;
            1
        }
        'b' => {
            next = chars.next()?;
            -1
        }
        _ => 0,
    };
    if chars.next().is_some() {
        return None;
    }
    let octave = next.to_digit(10)? as i16;
    const BASE_OCTAVE: i16 = 4;
    const SEMITONES_PER_OCTAVE: i16 = 12;
    let param = BELL_NOTE_MIN as i16
        + (octave - BASE_OCTAVE) * SEMITONES_PER_OCTAVE
        + semitone
        + accidental;
    u16::try_from(param).ok().and_then(in_range)
}

pub use kuluu_render::snapshot::system_chat_line;

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

fn parse_autoattack(rest: &str) -> SlashOutcome {
    let arg = rest.trim().to_ascii_lowercase();
    match arg.as_str() {
        "" | "toggle" => SlashOutcome::SetAutoAttack(None),
        "on" => SlashOutcome::SetAutoAttack(Some(true)),
        "off" => SlashOutcome::SetAutoAttack(Some(false)),
        other => SlashOutcome::SystemMessage(format!(
            "/autoattack: usage `/autoattack [on|off]` (got `{other}`)"
        )),
    }
}

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

fn parse_pathto(
    rest: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
    zone_id: Option<u16>,
    force: bool,
) -> SlashOutcome {
    let cmd_label = if force { "/pathtoforce" } else { "/pathto" };
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return SlashOutcome::SystemMessage(format!(
            "{cmd_label}: usage `{cmd_label} <x> <y> [z]` | `{cmd_label} <name>` | `{cmd_label} target`"
        ));
    }
    match parse_goto_target(
        trimmed,
        entities,
        self_pos,
        current_target,
        zone_id,
        cmd_label,
    ) {
        Ok(pos) => SlashOutcome::Command(AgentCommand::PathTo {
            x: pos.x,
            y: pos.y,
            z: pos.z,
            force,
        }),
        Err(msg) => SlashOutcome::SystemMessage(msg),
    }
}

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

    if (parts.len() == 2 || parts.len() == 3) && parts.iter().all(|p| p.parse::<f32>().is_ok()) {
        let v: Vec<f32> = parts.iter().map(|p| p.parse::<f32>().unwrap()).collect();
        let z = if v.len() == 3 { v[2] } else { self_pos.z };
        return Ok(WireVec3 {
            x: v[0],
            y: v[1],
            z,
        });
    }

    resolve_position_needle(trimmed, entities, self_pos, zone_id)
        .map(|(pos, _label)| pos)
        .ok_or_else(|| format!("{cmd_label}: no match for `{trimmed}` (try `/zones` or `/debug`)"))
}

fn self_heading(entities: &[WireEntity], self_pos: WireVec3) -> u8 {
    entities
        .iter()
        .find(|e| e.pos == self_pos)
        .map(|e| e.heading)
        .unwrap_or(0)
}

fn resolve_position_needle(
    needle: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    zone_id: Option<u16>,
) -> Option<(WireVec3, String)> {
    if let Some(z) = zone_id {
        let lines = kuluu_nav::zone_lines_for(z);
        if !lines.is_empty() {
            let by_id = needle.parse::<u16>().ok().filter(|n| *n <= MAX_ZONE_ID);
            let needle_lower = needle.to_ascii_lowercase();
            let hit = lines.iter().find(|line| {
                if Some(line.to_zone) == by_id {
                    return true;
                }
                kuluu_nav::zone_name(line.to_zone)
                    .map(|n| n.to_ascii_lowercase().starts_with(&needle_lower))
                    .unwrap_or(false)
            });
            if let Some(line) = hit {
                let pos = WireVec3 {
                    x: line.from_pos[0],
                    y: line.from_pos[1],
                    z: line.from_pos[2],
                };
                let label = kuluu_nav::zone_name(line.to_zone)
                    .map(|n| format!("zone-line -> {n} ({})", line.to_zone))
                    .unwrap_or_else(|| format!("zone-line -> zone {}", line.to_zone));
                return Some((pos, label));
            }
        }
    }
    let ent = resolve_name(needle, entities, self_pos)?;
    let kind = kind_tag(ent.kind);
    let name = ent.name.as_deref().unwrap_or("?");
    Some((ent.pos, format!("{kind} {name}")))
}

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

/// Split a retail command's arguments, honouring the double quotes that wrap a
/// name containing spaces.
fn split_command_args(rest: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut open = false;
    let mut started = false;
    for ch in rest.chars() {
        match ch {
            ARG_QUOTE => {
                open = !open;
                started = true;
            }
            _ if ch.is_whitespace() && !open => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            _ => {
                current.push(ch);
                started = true;
            }
        }
    }
    if started {
        args.push(current);
    }
    args
}

/// A spell/ability/weaponskill argument: retail takes the name, and Kuluu keeps
/// accepting the raw id the menus and the agent socket speak in.
fn action_id(word: &str, by_name: fn(&str) -> Option<u16>) -> Option<u32> {
    word.parse::<u32>()
        .ok()
        .or_else(|| by_name(word).map(u32::from))
}

/// The party/alliance member in one of retail's eighteen `<pN>`/`<aNN>` slots.
fn party_slot_target(slot: usize, party: &[kuluu_snapshot::PartyMember]) -> Option<(u32, u16)> {
    let party_no = (slot / PARTY_SLOTS) as u8;
    party
        .iter()
        .filter(|m| m.party_no == party_no)
        .nth(slot % PARTY_SLOTS)
        .map(|m| (m.id, m.act_index))
}

fn resolve_target_token(token: &str, ctx: &SlashCtx) -> Result<(u32, u16), String> {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        SELF_TARGET_TOKEN => {
            let id = ctx
                .self_char_id
                .ok_or_else(|| format!("{token}: self not resolved yet"))?;
            let index = ctx
                .entities
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.act_index)
                .unwrap_or(0);
            Ok((id, index))
        }
        CURRENT_TARGET_TOKEN => {
            resolve_action_target("", ctx.entities, ctx.self_pos, ctx.current_target)
                .ok_or_else(|| format!("{token}: no target"))
        }
        BATTLE_TARGET_TOKEN => {
            let id = ctx
                .battle_target
                .ok_or_else(|| format!("{token}: not engaged"))?;
            let index = ctx
                .entities
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.act_index)
                .unwrap_or(0);
            Ok((id, index))
        }
        PET_TARGET_TOKEN => {
            let targid = ctx
                .self_pet_targid
                .ok_or_else(|| format!("{token}: no pet"))?;
            ctx.entities
                .iter()
                .find(|e| e.act_index == targid)
                .map(|e| (e.id, e.act_index))
                .ok_or_else(|| format!("{token}: pet not in view"))
        }
        _ => match PARTY_TARGET_TOKENS.iter().position(|t| *t == lower) {
            Some(slot) => party_slot_target(slot, ctx.party)
                .ok_or_else(|| format!("{token}: nobody in that slot")),
            None if UNRESOLVED_TARGET_TOKENS.contains(&lower.as_str()) => {
                Err(format!("{token}: not supported yet"))
            }
            None => Err(format!("unknown target token `{token}`")),
        },
    }
}

/// What an action command's target argument resolved to.
enum TargetArg {
    Resolved((u32, u16), usize),
    /// Retail's sub-target cursor takes it from here.
    Picker {
        narrow: Option<ffxi_vocab::valid_target::TargetFlags>,
        used: usize,
    },
}

/// Resolve the target argument of an action command. No argument, or a
/// sub-target token, hands the choice to retail's cursor; `<me>`, `<t>`,
/// `<bt>`, `<pet>`, party slots, a name, or a raw `id [index]` pair resolve
/// here and the action fires without the cursor.
fn resolve_command_target(args: &[String], ctx: &SlashCtx) -> Result<TargetArg, String> {
    let Some(first) = args.first() else {
        return Ok(TargetArg::Picker {
            narrow: None,
            used: 0,
        });
    };
    if let Ok(id) = first.parse::<u32>() {
        return match args.get(1).map(|t| t.parse::<u16>()) {
            Some(Ok(index)) => Ok(TargetArg::Resolved((id, index), 2)),
            Some(Err(_)) | None => {
                let index = ctx
                    .entities
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.act_index)
                    .unwrap_or(0);
                Ok(TargetArg::Resolved((id, index), 1))
            }
        };
    }
    if first.starts_with(TARGET_TOKEN_OPEN) && first.ends_with(TARGET_TOKEN_CLOSE) {
        let lower = first.to_ascii_lowercase();
        if let Some((_, narrow)) = SUB_TARGET_TOKENS.iter().find(|(t, _)| *t == lower) {
            return Ok(TargetArg::Picker {
                narrow: narrow.map(ffxi_vocab::valid_target::TargetFlags),
                used: 1,
            });
        }
        return resolve_target_token(first, ctx).map(|pair| TargetArg::Resolved(pair, 1));
    }
    resolve_action_target(first, ctx.entities, ctx.self_pos, ctx.current_target)
        .map(|pair| TargetArg::Resolved(pair, 1))
        .ok_or_else(|| format!("no one named `{first}` nearby"))
}

fn parse_ground_target(args: &[String]) -> Result<[f32; 3], String> {
    match args {
        [] => Ok([0.0, 0.0, 0.0]),
        [x, y, z] => {
            let xyz: Result<Vec<f32>, _> = [x, y, z].iter().map(|p| p.parse::<f32>()).collect();
            xyz.map(|v| [v[0], v[1], v[2]])
                .map_err(|_| "bad ground-target coords (expected three floats)".to_string())
        }
        _ => Err("bad ground-target coords (expected three floats)".to_string()),
    }
}

fn parse_cast(ctx: &SlashCtx) -> SlashOutcome {
    let cmd = ctx.cmd;
    let args = split_command_args(ctx.rest);
    let Some(name) = args.first() else {
        return SlashOutcome::SystemMessage(format!("/{cmd}: usage `/{cmd} <spell> [target]`"));
    };
    let Some(spell_id) = action_id(name, ffxi_vocab::spell_names::id_for) else {
        return SlashOutcome::SystemMessage(format!("/{cmd}: unknown spell `{name}`"));
    };
    let ((target_id, target_index), used) = match resolve_command_target(&args[1..], ctx) {
        Ok(TargetArg::Resolved(pair, used)) => (pair, used),
        // A self-only spell typed with no target fires at the player without
        // the cursor: there is no other target for retail to ask about.
        Ok(TargetArg::Picker {
            narrow: None,
            used: 0,
        }) if u16::try_from(spell_id)
            .ok()
            .and_then(ffxi_vocab::valid_target::spell)
            .is_some_and(|f| f.is_self_only()) =>
        {
            let pair = resolve_target_token(SELF_TARGET_TOKEN, ctx)
                .map_err(|msg| SlashOutcome::SystemMessage(format!("/{cmd}: {msg}")));
            match pair {
                Ok(pair) => (pair, 0),
                Err(out) => return out,
            }
        }
        Ok(TargetArg::Picker { narrow, .. }) => {
            return SlashOutcome::OpenSubTarget {
                action: kuluu_render::input_mode::SubTargetAction::Spell(
                    u16::try_from(spell_id).unwrap_or(u16::MAX),
                ),
                narrow,
            };
        }
        Err(msg) => return SlashOutcome::SystemMessage(format!("/{cmd}: {msg}")),
    };
    let coords = match parse_ground_target(&args[1 + used..]) {
        Ok(c) => c,
        Err(msg) => return SlashOutcome::SystemMessage(format!("/{cmd}: {msg}")),
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

fn parse_weaponskill(ctx: &SlashCtx) -> SlashOutcome {
    let cmd = ctx.cmd;
    let args = split_command_args(ctx.rest);
    let Some(name) = args.first() else {
        return SlashOutcome::SystemMessage(format!("/{cmd}: usage `/{cmd} <skill> [target]`"));
    };
    let Some(skill_id) = action_id(name, ffxi_vocab::weapon_skill_names::id_for) else {
        return SlashOutcome::SystemMessage(format!("/{cmd}: unknown weapon skill `{name}`"));
    };
    let ((target_id, target_index), _) = match resolve_command_target(&args[1..], ctx) {
        Ok(TargetArg::Resolved(pair, _)) => (pair, 0),
        Ok(TargetArg::Picker { narrow, .. }) => {
            return SlashOutcome::OpenSubTarget {
                action: kuluu_render::input_mode::SubTargetAction::WeaponSkill(
                    u16::try_from(skill_id).unwrap_or(u16::MAX),
                ),
                narrow,
            };
        }
        Err(msg) => return SlashOutcome::SystemMessage(format!("/{cmd}: {msg}")),
    };
    SlashOutcome::Command(AgentCommand::Action {
        target_id,
        target_index,
        kind: ActionKind::Weaponskill { skill_id },
    })
}

/// `/ra [target]` -- ranged attack (the Shoot action). Takes no id, only a
/// target (defaults to the current target).
fn parse_ranged_attack(ctx: &SlashCtx) -> SlashOutcome {
    let cmd = ctx.cmd;
    let args = split_command_args(ctx.rest);
    let ((target_id, target_index), _) = match resolve_command_target(&args, ctx) {
        Ok(TargetArg::Resolved(pair, _)) => (pair, 0),
        Ok(TargetArg::Picker { narrow, .. }) => {
            return SlashOutcome::OpenSubTarget {
                action: kuluu_render::input_mode::SubTargetAction::Ranged,
                narrow,
            };
        }
        Err(msg) => return SlashOutcome::SystemMessage(format!("/{cmd}: {msg}")),
    };
    SlashOutcome::Command(AgentCommand::Action {
        target_id,
        target_index,
        kind: ActionKind::Shoot,
    })
}

fn parse_job_ability(ctx: &SlashCtx) -> SlashOutcome {
    let cmd = ctx.cmd;
    let args = split_command_args(ctx.rest);
    let Some(name) = args.first() else {
        return SlashOutcome::SystemMessage(format!("/{cmd}: usage `/{cmd} <ability> [target]`"));
    };
    let Some(ability_id) = action_id(name, ffxi_vocab::ability_names::id_for) else {
        return SlashOutcome::SystemMessage(format!("/{cmd}: unknown ability `{name}`"));
    };
    let ((target_id, target_index), _) = match resolve_command_target(&args[1..], ctx) {
        Ok(TargetArg::Resolved(pair, _)) => (pair, 0),
        // A self-only ability typed with no target fires at the player without
        // the cursor: there is no other target for retail to ask about
        // (vendor/server/sql/abilities.sql validTarget).
        Ok(TargetArg::Picker {
            narrow: None,
            used: 0,
        }) if u16::try_from(ability_id)
            .ok()
            .and_then(ffxi_vocab::valid_target::ability)
            .is_some_and(|f| f.is_self_only()) =>
        {
            let pair = resolve_target_token(SELF_TARGET_TOKEN, ctx)
                .map_err(|msg| SlashOutcome::SystemMessage(format!("/{cmd}: {msg}")));
            match pair {
                Ok(pair) => (pair, 0),
                Err(out) => return out,
            }
        }
        Ok(TargetArg::Picker { narrow, .. }) => {
            return SlashOutcome::OpenSubTarget {
                action: kuluu_render::input_mode::SubTargetAction::Ability(
                    u16::try_from(ability_id).unwrap_or(u16::MAX),
                ),
                narrow,
            };
        }
        Err(msg) => return SlashOutcome::SystemMessage(format!("/{cmd}: {msg}")),
    };
    SlashOutcome::Command(AgentCommand::Action {
        target_id,
        target_index,
        kind: ActionKind::JobAbility { ability_id },
    })
}

fn parse_use_item(ctx: &SlashCtx) -> SlashOutcome {
    let cmd = ctx.cmd;
    let parts = split_command_args(ctx.rest);
    if parts.len() < 2 {
        return SlashOutcome::SystemMessage(format!(
            "/{cmd}: usage `/{cmd} <container> <slot> [item_no] [target]`"
        ));
    }
    let container: u8 = match parts[0].parse() {
        Ok(n) => n,
        Err(_) => {
            return SlashOutcome::SystemMessage(format!("/{cmd}: bad container `{}`", parts[0]));
        }
    };
    let slot: u8 = match parts[1].parse() {
        Ok(n) => n,
        Err(_) => return SlashOutcome::SystemMessage(format!("/{cmd}: bad slot `{}`", parts[1])),
    };
    let item_no: u32 = match parts.get(2) {
        Some(s) => match s.parse() {
            Ok(n) => n,
            Err(_) => return SlashOutcome::SystemMessage(format!("/{cmd}: bad item_no `{s}`")),
        },
        None => 0,
    };
    let ((target_id, target_index), _) =
        match resolve_command_target(&parts[3.min(parts.len())..], ctx) {
            Ok(TargetArg::Resolved(pair, _)) => (pair, 0),
            Ok(TargetArg::Picker { narrow, .. }) => {
                return SlashOutcome::OpenSubTarget {
                    action: kuluu_render::input_mode::SubTargetAction::Item {
                        container,
                        index: slot,
                        item_no: u16::try_from(item_no).unwrap_or(u16::MAX),
                    },
                    narrow,
                };
            }
            Err(msg) => return SlashOutcome::SystemMessage(format!("/{cmd}: {msg}")),
        };
    SlashOutcome::Command(AgentCommand::UseItem {
        container,
        slot,
        item_no,
        target_id,
        target_index,
    })
}

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

fn parse_overlay(rest: &str) -> SlashOutcome {
    let trimmed = rest.trim();
    let (verb, arg) = match trimmed.split_once(char::is_whitespace) {
        Some((v, a)) => (v, a.trim()),
        None => (trimmed, ""),
    };
    let op = match verb.to_ascii_lowercase().as_str() {
        "" | "list" | "status" => OverlayOp::List,
        // A path is taken verbatim: overlay directories routinely contain
        // spaces, so splitting further would break more than it parsed.
        "add" if !arg.is_empty() => OverlayOp::Add(std::path::PathBuf::from(arg)),
        "add" => return SlashOutcome::SystemMessage("/overlay add: usage `add <dir>`".into()),
        "remove" | "rm" => match arg.parse::<usize>() {
            Ok(n) if n >= 1 => OverlayOp::Remove(n),
            _ => {
                return SlashOutcome::SystemMessage(format!(
                    "/overlay remove: want a 1-based index, got `{arg}`"
                ))
            }
        },
        "clear" | "off" => OverlayOp::Clear,
        "reset" | "auto" => OverlayOp::Reset,
        other => {
            return SlashOutcome::SystemMessage(format!(
                "/overlay: unknown `{other}` -- try list|add <dir>|remove <n>|clear|reset"
            ))
        }
    };
    SlashOutcome::Overlay(op)
}

fn parse_actordiag(rest: &str) -> SlashOutcome {
    match rest.trim().to_ascii_lowercase().as_str() {
        "" | "self" => SlashOutcome::ActorDiag { use_target: false },
        "t" | "target" => SlashOutcome::ActorDiag { use_target: true },
        other => SlashOutcome::SystemMessage(format!(
            "/actordiag: unknown `{other}` -- usage: /actordiag [target]"
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

fn parse_zones(zone_id: Option<u16>) -> SlashOutcome {
    let Some(z) = zone_id else {
        return SlashOutcome::SystemMessage("/zones: not in a zone yet".into());
    };
    let lines = kuluu_nav::zone_lines_for(z);
    if lines.is_empty() {
        return SlashOutcome::SystemMessage(format!(
            "/zones: no zone-lines from zone {z} (instance / GM zone?)"
        ));
    }
    let mut msg = format!(
        "/zones from {} ({z}):",
        kuluu_nav::zone_name(z).unwrap_or("?")
    );
    for line in lines {
        let to_name = kuluu_nav::zone_name(line.to_zone).unwrap_or("?");
        msg.push_str(&format!(
            "\n  -> {} ({}) at ({:.0}, {:.0}, {:.0})",
            to_name, line.to_zone, line.from_pos[0], line.from_pos[1], line.from_pos[2],
        ));
    }
    SlashOutcome::SystemMessage(msg)
}

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

        _ => SlashOutcome::SystemMessage(render_debug_entity(trimmed, entities, self_pos)),
    }
}

fn look_tag(look: Option<&kuluu_snapshot::EntityLook>) -> &'static str {
    use kuluu_snapshot::EntityLook;
    match look {
        None => "--",
        Some(EntityLook::Standard { .. }) => "std",
        Some(EntityLook::Equipped { .. }) => "eq",
        Some(EntityLook::Door { .. }) => "door",
        Some(EntityLook::Transport { .. }) => "tx",
    }
}

fn kind_tag(kind: kuluu_snapshot::EntityKind) -> &'static str {
    use kuluu_snapshot::EntityKind;
    match kind {
        EntityKind::Pc => "pc",
        EntityKind::Npc => "npc",
        EntityKind::Mob => "mob",
        EntityKind::Pet => "pet",
        EntityKind::Other => "?",
    }
}

fn render_debug_nearby(
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
) -> String {
    let mut out = String::new();

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

/// Renders one wire entity for /debug. `n/a` means no General-block update
/// has carried the name-visibility byte yet (it rides UPDATE_HP).
fn render_debug_entity(arg: &str, entities: &[WireEntity], self_pos: WireVec3) -> String {
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
    use kuluu_snapshot::EntityLook;
    match &e.look {
        None => s.push_str(" (none decoded -- no look-bearing tick yet)"),
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
        Some(EntityLook::Door { size, .. }) => s.push_str(&format!(" door size={size}")),
        Some(EntityLook::Transport { size, .. }) => s.push_str(&format!(" transport size={size}")),
    }
    let namevis = e
        .name_vis
        .map_or_else(|| "n/a".to_string(), |v| v.to_string());
    s.push('\n');
    s.push_str(&format!(
        "  anim={} animsub={} status={} namevis={namevis}{}",
        e.animation,
        e.animationsub,
        e.status,
        if e.animationsub != 0 { "  EFFECT" } else { "" }
    ));
    s
}

fn nearby_entities(
    entities: &[WireEntity],
    from: WireVec3,
    limit: usize,
) -> Vec<(&WireEntity, f32)> {
    let mut scored: Vec<(&WireEntity, f32)> =
        entities.iter().map(|e| (e, sq_dist(e.pos, from))).collect();
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored
}

/// `/mutebgm` / `/mutese`. Bare toggles, matching the switch the Config menu
/// shows; `on` is muted, the state the command is named for.
fn parse_mute(rest: &str, op: fn(Option<bool>) -> SoundOp) -> SlashOutcome {
    let muted = match rest.trim().to_ascii_lowercase().as_str() {
        "" | "toggle" => None,
        "on" | "mute" | "1" => Some(true),
        "off" | "unmute" | "0" => Some(false),
        other => {
            return SlashOutcome::SystemMessage(format!("bad arg `{other}` (use on|off)"));
        }
    };
    SlashOutcome::SetSound(op(muted))
}

fn parse_weather(rest: &str) -> SlashOutcome {
    use kuluu_snapshot::Weather;
    let arg = rest.trim();
    if arg.is_empty() {
        return SlashOutcome::SystemMessage(
            "/weather: usage `/weather <id|name>` -- 0..=19, or names like \
             none, sunshine, clouds, fog, rain, snow, thunderstorms, sand_storm, \
             auroras, gloom, darkness (see vendor/server/data/enums/weather.yaml)"
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
        // The four-char `weat/<tag>` DAT names, which is what the zone tree and
        // every log line actually call these. Resolved from the id table rather
        // than a second hand-written list, so the two cannot drift.
        _ => match key.as_bytes().try_into().ok().and_then(|tag: [u8; 4]| {
            (0u16..=19).find(|&n| ffxi_dat::weather::weather_type_id(n) == tag)
        }) {
            Some(n) => Weather::from_lsb(n),
            None => {
                return SlashOutcome::SystemMessage(format!(
                    "/weather: unknown weather `{arg}` (try a number 0..=19, a name like \
                     `rain`, or a DAT tag like `clod`)"
                ));
            }
        },
    };
    SlashOutcome::SetWeatherClient(w)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneLineOp {
    Status,
    Set(kuluu_render::ZoneLineDisplay),
    Toggle,
}

fn parse_zoneline(rest: &str) -> SlashOutcome {
    use kuluu_render::ZoneLineDisplay;
    let op = match rest.trim().to_ascii_lowercase().as_str() {
        "" => ZoneLineOp::Status,
        "toggle" => ZoneLineOp::Toggle,
        "off" | "hide" | "none" => ZoneLineOp::Set(ZoneLineDisplay::Off),
        "pillar" | "column" | "on" => ZoneLineOp::Set(ZoneLineDisplay::Pillar),
        "gate" | "box" | "footprint" => ZoneLineOp::Set(ZoneLineDisplay::Gate),
        other => {
            return SlashOutcome::SystemMessage(format!(
                "/zoneline: unknown mode `{other}` (use off | pillar | gate | toggle)"
            ));
        }
    };
    SlashOutcome::SetZoneLines(op)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LightsOp {
    Status,

    Enable(Option<bool>),

    Shadowed(u32),

    Flicker(Option<bool>),
}

fn parse_lights(rest: &str) -> SlashOutcome {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return SlashOutcome::SetLights(LightsOp::Status);
    }
    let mut parts = trimmed.split_ascii_whitespace();
    let verb = parts.next().unwrap_or("").to_ascii_lowercase();
    let arg = parts.next().unwrap_or("");

    let toggle = |a: &str| -> Result<Option<bool>, ()> {
        match a.to_ascii_lowercase().as_str() {
            "" | "toggle" => Ok(None),
            "on" | "true" | "1" => Ok(Some(true)),
            "off" | "false" | "0" => Ok(Some(false)),
            _ => Err(()),
        }
    };

    match verb.as_str() {
        "on" | "off" | "toggle" => match toggle(&verb) {
            Ok(v) => SlashOutcome::SetLights(LightsOp::Enable(v)),
            Err(()) => unreachable!(),
        },
        "shadowed" | "shadows" => match arg.parse::<u32>() {
            Ok(v) => SlashOutcome::SetLights(LightsOp::Shadowed(v)),
            Err(_) => SlashOutcome::SystemMessage(format!("/lights shadowed: bad value `{arg}`")),
        },
        "flicker" => match toggle(arg) {
            Ok(v) => SlashOutcome::SetLights(LightsOp::Flicker(v)),
            Err(()) => SlashOutcome::SystemMessage(format!("/lights flicker: bad value `{arg}`")),
        },
        other => SlashOutcome::SystemMessage(format!(
            "/lights: unknown `{other}` (use on|off|shadowed N|flicker on|off)"
        )),
    }
}

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

fn parse_debugchat(rest: &str) -> SlashOutcome {
    let arg = rest.trim().to_ascii_lowercase();
    let setting = match arg.as_str() {
        "" | "toggle" => None,
        "on" | "true" | "1" => Some(true),
        "off" | "false" | "0" => Some(false),
        other => {
            return SlashOutcome::SystemMessage(format!(
                "/debugchat: bad arg `{other}` (use on|off|toggle)"
            ));
        }
    };
    SlashOutcome::SetDebugChat(setting)
}

fn parse_noclip(rest: &str) -> SlashOutcome {
    let arg = rest.trim().to_ascii_lowercase();
    let setting = match arg.as_str() {
        "" | "toggle" => None,
        "on" | "true" | "1" => Some(true),
        "off" | "false" | "0" => Some(false),
        other => {
            return SlashOutcome::SystemMessage(format!(
                "/noclip: bad arg `{other}` (use on|off|toggle)"
            ));
        }
    };
    SlashOutcome::SetNoClip(setting)
}

fn parse_netstat(rest: &str) -> SlashOutcome {
    let arg = rest.trim().to_ascii_lowercase();
    let setting = match arg.as_str() {
        "" | "toggle" => None,
        "on" | "true" | "1" => Some(true),
        "off" | "false" | "0" => Some(false),
        other => {
            return SlashOutcome::SystemMessage(format!(
                "/netstat: bad arg `{other}` (use on|off|toggle)"
            ));
        }
    };
    SlashOutcome::SetNetStatus(setting)
}

fn parse_clock(rest: &str) -> SlashOutcome {
    let arg = rest.trim().to_ascii_lowercase();
    let setting = match arg.as_str() {
        "" | "toggle" => None,
        "on" | "show" => Some(true),
        "off" | "hide" => Some(false),
        other => {
            return SlashOutcome::SystemMessage(format!(
                "/clock: bad arg `{other}` (use show|hide|toggle)"
            ));
        }
    };
    SlashOutcome::SetVanaClock(setting)
}

fn parse_renderscale(rest: &str) -> SlashOutcome {
    let a = rest.trim();
    if a.is_empty() {
        return SlashOutcome::SetRenderScale(None);
    }
    let raw = a.trim_end_matches('%').trim();
    match raw.parse::<f32>() {
        Ok(mut v) if v.is_finite() => {
            let entered_as_percent = v > 4.0;
            if entered_as_percent {
                v /= 100.0;
            }
            if (0.25..=2.0).contains(&v) {
                SlashOutcome::SetRenderScale(Some(v))
            } else {
                SlashOutcome::SystemMessage(format!(
                    "/renderscale: {:.0}% out of range (25-200%)",
                    v * 100.0
                ))
            }
        }
        _ => SlashOutcome::SystemMessage(format!(
            "/renderscale: bad value `{a}` (e.g. 75, 75%, 0.75, 1.0, 200)"
        )),
    }
}

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

fn parse_zonegeom(rest: &str) -> SlashOutcome {
    use kuluu_render::dat_mzb::{CameraCollisionSource, ZoneGeomMode};
    let arg = rest.trim().to_ascii_lowercase();
    let mut tokens = arg.split_whitespace();
    let first = tokens.next().unwrap_or("");

    if matches!(first, "source" | "src" | "camsrc" | "camsource") {
        let src = match tokens.next().unwrap_or("") {
            "" | "toggle" => None,
            "mzb" => Some(CameraCollisionSource::Mzb),
            "mmb" => Some(CameraCollisionSource::Mmb),
            "both" => Some(CameraCollisionSource::Both),
            other => {
                return SlashOutcome::SystemMessage(format!(
                    "/zonegeom source: bad arg `{other}` (use mzb|mmb|both|toggle)"
                ));
            }
        };
        return SlashOutcome::SetCameraCollisionSource(src);
    }

    let setting = match first {
        "" | "toggle" => None,
        "off" | "false" | "0" => Some(ZoneGeomMode::Off),
        "collision" | "coll" => Some(ZoneGeomMode::Collision),
        "all" | "on" | "true" | "1" => Some(ZoneGeomMode::All),
        "camera" | "cam" => Some(ZoneGeomMode::Camera),
        other => {
            return SlashOutcome::SystemMessage(format!(
                "/zonegeom: bad arg `{other}` (use off|collision|all|camera|source|toggle)"
            ));
        }
    };
    SlashOutcome::SetZoneGeom(setting)
}

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
            ));
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

fn parse_look(
    rest: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
) -> SlashOutcome {
    use kuluu_snapshot::EntityLook;

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
        Some(EntityLook::Door { size, .. }) => format!("look: DOOR (size={size})"),
        Some(EntityLook::Transport { size, .. }) => format!("look: TRANSPORT (size={size})"),
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
            return SlashOutcome::SystemMessage(format!("/load_mmb: bad file_id `{file_str}`"));
        }
    };
    let chunk_idx: usize = match chunk_str.parse() {
        Ok(n) => n,
        Err(_) => {
            return SlashOutcome::SystemMessage(format!("/load_mmb: bad chunk_idx `{chunk_str}`"));
        }
    };
    SlashOutcome::LoadMmb {
        file_id,
        chunk_idx,
        world_pos: self_pos,
        entity_id: None,
    }
}

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
            ));
        }
    };
    let file_id: u32 = match file_str.parse() {
        Ok(n) => n,
        Err(_) => {
            return SlashOutcome::SystemMessage(format!("/load_mmb_on: bad file_id `{file_str}`"));
        }
    };
    let chunk_idx: usize = match chunk_str.parse() {
        Ok(n) => n,
        Err(_) => {
            return SlashOutcome::SystemMessage(format!(
                "/load_mmb_on: bad chunk_idx `{chunk_str}`"
            ));
        }
    };
    SlashOutcome::LoadMmb {
        file_id,
        chunk_idx,

        world_pos: WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        entity_id: Some(entity_id),
    }
}

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
            return SlashOutcome::SystemMessage(format!("/load_mzb: bad file_id `{file_str}`"));
        }
    };
    let chunk_idx = match parts.next() {
        None => None,
        Some(s) => match s.parse::<usize>() {
            Ok(n) => Some(n),
            Err(_) => {
                return SlashOutcome::SystemMessage(format!("/load_mzb: bad chunk_idx `{s}`"));
            }
        },
    };
    SlashOutcome::LoadMzb {
        file_id,
        chunk_idx,
        world_pos: self_pos,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAreaOp {
    List,
    Load(u32),
    /// Whichever sub-area's trigger volume holds the player right now.
    Here,
}

fn parse_sub_area(rest: &str, self_pos: WireVec3) -> SlashOutcome {
    let arg = rest.trim();
    let op = match arg {
        "" => SubAreaOp::List,
        "here" => SubAreaOp::Here,
        _ => match parse_u32_auto_radix(arg) {
            Some(id) => SubAreaOp::Load(id),
            None => {
                return SlashOutcome::SystemMessage(format!(
                    "/subarea: bad sub-area id `{arg}` -- usage `/subarea [<sub_area_id>|here]`"
                ))
            }
        },
    };
    SlashOutcome::SubArea { op, self_pos }
}

/// Sub-area ids are quoted in hex by every upstream reference
/// (research/xi-tools/docs/zone/subareas.md), so `/subarea 0x1CE` has to work as well
/// as `/subarea 462`.
fn parse_u32_auto_radix(s: &str) -> Option<u32> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => s.parse().ok(),
    }
}

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
                "/keybinds: unknown preset `{arg}` -- try compact1, compact2, or standard"
            )),
        },
        "list" => SlashOutcome::ApplyKeybinds(KeybindUpdate::List),
        "reset" => SlashOutcome::ApplyKeybinds(KeybindUpdate::Reset),
        other => SlashOutcome::SystemMessage(format!(
            "/keybinds: unknown verb `{other}` -- try preset, list, or reset"
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
        let pc_rank = |e: &WireEntity| matches!(e.kind, kuluu_snapshot::EntityKind::Pc) as u8;

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

fn cycle_kind_filtered(
    entities: &[WireEntity],
    self_pos: WireVec3,
    current: Option<u32>,
    kinds: &[kuluu_snapshot::EntityKind],
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

fn sq_dist(a: WireVec3, b: WireVec3) -> f32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = a.z - b.z;
    dx * dx + dy * dy + dz * dz
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_snapshot::{Entity as WireEntity, EntityKind, Vec3 as WireVec3};

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
            name_vis: None,
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            mount: None,
            status: 0,
            char_flags: Default::default(),
            monstrosity: false,
        }
    }

    fn empty_entities() -> Vec<WireEntity> {
        Vec::new()
    }

    #[test]
    fn debug_chat_command_controls_its_own_visibility() {
        for (command, expected) in [
            ("\u{002F}\u{002F}debugchat", None),
            ("\u{002F}\u{002F}debugchat toggle", None),
            ("\u{002F}\u{002F}debugchat on", Some(true)),
            ("\u{002F}\u{002F}debugchat off", Some(false)),
        ] {
            assert!(matches!(
                parse_slash_t(command, &empty_entities(), origin(), None, None),
                SlashOutcome::SetDebugChat(value) if value == expected
            ));
        }
        assert!(matches!(
            parse_slash_t(
                "\u{002F}\u{002F}debugchat invalid",
                &empty_entities(),
                origin(),
                None,
                None
            ),
            SlashOutcome::SystemMessage(_)
        ));
    }

    fn origin() -> WireVec3 {
        WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }

    /// A table standing in for an install's, carrying only the alias groups
    /// the tests exercise. Built rather than read so the suite pins alias
    /// resolution on a machine with no game files.
    fn test_surface() -> CommandSurface {
        use ffxi_dat::main_dll::{ClientCommand, CommandTable};
        let rows: &[(&[&str], u16)] = &[
            (&["say", "s"], 0x0001),
            (&["shout", "sh"], 0x0002),
            (&["tell", "t"], 0x0004),
            (&["party", "p"], 0x0005),
            (&["linkshell", "l"], 0x0006),
            (&["emote", "em"], 0x000b),
            (&["attack", "a"], 0x001d),
            (&["attackoff"], 0x001e),
            (&["target", "ta"], 0x0017),
            (&["targetnpc"], 0x0019),
            (&["targetbnpc"], 0x001a),
            (&["assist", "as"], 0x0020),
            (&["item"], 0x0022),
            (&["equip"], 0x0024),
            (&["magic", "ma"], 0x0025),
            (&["weaponskill", "ws"], 0x0026),
            (&["jobability", "ja"], 0x002b),
            (&["shoot", "range", "ra", "throw"], 0x001c),
            (&["heal"], 0x0032),
            (&["sit"], 0x0033),
            (&["fish"], 0x0036),
            (&["dig"], 0x0037),
            (&["help", "h"], 0x0021),
            (&["check", "c"], 0x0045),
            (&["checkname", "cn"], 0x0046),
            (&["checkparam"], 0x0047),
            (&["logout"], 0x0043),
            (&["shutdown"], 0x0044),
            (&["clock"], 0x003f),
            (&["map"], 0x004f),
            (&["bank"], 0x006c),
            (&["follow"], 0x005b),
            (&["nod", "yes"], 0x00a9),
            (&["goodbye", "farewell"], 0x00ab),
            (&["disgusted", "upset"], 0x00bc),
            (&["wave"], 0x00aa),
            (&["kneel"], 0x00a5),
            (&["bell"], 0x00eb),
            (&["jobemote"], 0x00ec),
            (&["?"], 0x0103),
            (&["recast"], 0x002e),
        ];
        let entries = rows
            .iter()
            .flat_map(|(names, id)| {
                names.iter().map(move |name| ClientCommand {
                    name: (*name).to_owned(),
                    id: *id,
                    flags: 0,
                })
            })
            .collect();
        CommandSurface::new(CommandTable::from_entries(entries))
    }

    fn parse_slash_t(
        buffer: &str,
        entities: &[WireEntity],
        self_pos: WireVec3,
        current_target: Option<u32>,
        zone_id: Option<u16>,
    ) -> SlashOutcome {
        parse_slash(
            buffer,
            &test_surface(),
            entities,
            self_pos,
            current_target,
            zone_id,
            None,
            &[],
            kuluu_render::fishing_spot::FishingGate::Ready,
            None,
            None,
        )
    }
    #[test]
    fn targetnpc_cycles_non_pc_forward() {
        let entities = vec![
            ent(1, "Goblin A", EntityKind::Mob, 3.0, 0.0),
            ent(2, "Vendor", EntityKind::Npc, 5.0, 0.0),
            ent(3, "Goblin B", EntityKind::Mob, 7.0, 0.0),
            ent(4, "Bob", EntityKind::Pc, 1.0, 0.0),
        ];
        assert!(matches!(
            parse_slash_t("/targetnpc", &entities, origin(), None, None),
            SlashOutcome::SetTarget(Some(1))
        ));

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
    fn dig_targets_self() {
        let mut me = ent(42, "Me", EntityKind::Pc, 0.0, 0.0);
        me.act_index = 7;
        let entities = vec![me, ent(1, "Goblin", EntityKind::Mob, 3.0, 0.0)];
        let outcome = parse_slash(
            "/dig",
            &test_surface(),
            &entities,
            origin(),
            Some(1),
            None,
            Some(42),
            &[],
            kuluu_render::fishing_spot::FishingGate::Ready,
            None,
            None,
        );
        assert!(matches!(
            outcome,
            SlashOutcome::Command(AgentCommand::Action {
                target_id: 42,
                target_index: 7,
                kind: ActionKind::ChocoboDig,
            })
        ));
    }
    #[test]
    fn dismount_targets_self() {
        let mut me = ent(42, "Me", EntityKind::Pc, 0.0, 0.0);
        me.act_index = 7;
        let entities = vec![me, ent(1, "Chocobo", EntityKind::Mob, 3.0, 0.0)];
        let outcome = parse_slash(
            "/dismount",
            &test_surface(),
            &entities,
            origin(),
            Some(1),
            None,
            Some(42),
            &[],
            kuluu_render::fishing_spot::FishingGate::Ready,
            None,
            None,
        );
        assert!(matches!(
            outcome,
            SlashOutcome::Command(AgentCommand::Action {
                target_id: 42,
                target_index: 7,
                kind: ActionKind::Dismount,
            })
        ));
    }

    #[test]
    fn targetenemy_skips_npcs() {
        let entities = vec![
            ent(1, "Vendor", EntityKind::Npc, 2.0, 0.0),
            ent(2, "Goblin", EntityKind::Mob, 8.0, 0.0),
        ];
        assert!(matches!(
            parse_slash_t("/targetbnpc", &entities, origin(), None, None),
            SlashOutcome::SetTarget(Some(2))
        ));
    }
    #[test]
    fn sq_dist_is_3d_euclidean() {
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
    fn map_and_alias_open_map_menu() {
        for slash in ["/map", "/map"] {
            assert!(
                matches!(
                    parse_slash_t(slash, &empty_entities(), origin(), None, None),
                    SlashOutcome::OpenMenu(MenuKind::Map)
                ),
                "{slash} should open the Map menu"
            );
        }
        assert!(matches!(
            parse_slash_t("/map foo", &empty_entities(), origin(), None, None),
            SlashOutcome::SystemMessage(_)
        ));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn widescan_requests_list() {
        for slash in ["\u{002F}\u{002F}widescan", "\u{002F}\u{002F}wscan"] {
            assert!(
                matches!(
                    parse_slash_t(slash, &empty_entities(), origin(), None, None),
                    SlashOutcome::Widescan
                ),
                "{slash} should fire a wide-scan request"
            );
        }
    }

    /// The Widescan variant only exists under debug_assertions (the /widescan
    /// command is dev-only), so this guard compiles in the same profile.
    #[cfg(debug_assertions)]
    #[test]
    fn ws_alias_stays_weaponskill() {
        assert!(
            !matches!(
                parse_slash_t(
                    "/weaponskill Fast Blade",
                    &empty_entities(),
                    origin(),
                    None,
                    None
                ),
                SlashOutcome::Widescan
            ),
            "/ws must remain the weaponskill command, not widescan"
        );
    }

    #[test]
    fn party_chat_with_text() {
        let out = parse_slash_t(
            "/party hello world",
            &empty_entities(),
            origin(),
            None,
            None,
        );
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
        let out = parse_slash_t("/party", &empty_entities(), origin(), None, None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn tell_requires_name_and_text() {
        let out = parse_slash_t(
            "/tell Bob hi there",
            &empty_entities(),
            origin(),
            None,
            None,
        );
        match out {
            SlashOutcome::Command(AgentCommand::Tell { to, text }) => {
                assert_eq!(to, "Bob");
                assert_eq!(text, "hi there");
            }
            other => panic!("expected Tell, got {other:?}"),
        }

        let out = parse_slash_t("/tell Bob", &empty_entities(), origin(), None, None);
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
                assert_eq!(target_id, 101);
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
        let out = parse_slash_t(
            "\u{002F}\u{002F}debug",
            &entities,
            origin(),
            Some(202),
            None,
        );
        match out {
            SlashOutcome::SystemMessage(s) => {
                assert!(s.contains("target:"), "no target line: {s}");
                assert!(s.contains("NearMob"), "target name not in output: {s}");
                assert!(s.contains("nearby"), "nearby header missing: {s}");

                assert!(s.contains("(self)"), "self marker missing: {s}");

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
        let out = parse_slash_t(
            "\u{002F}\u{002F}debug Goblin",
            &entities,
            origin(),
            None,
            None,
        );
        match out {
            SlashOutcome::SystemMessage(s) => {
                assert!(s.contains("Goblin"), "name missing: {s}");
                assert!(s.contains("id=202"), "id missing: {s}");
                assert!(s.contains("42%"), "hp missing: {s}");

                assert!(s.contains("dist=5.00y"), "distance wrong: {s}");
            }
            other => panic!("expected SystemMessage, got {other:?}"),
        }
    }

    #[test]
    fn debug_heights_subcommand_still_works() {
        let out = parse_slash_t(
            "\u{002F}\u{002F}debug heights",
            &empty_entities(),
            origin(),
            None,
            None,
        );
        assert!(matches!(out, SlashOutcome::DebugHeights));
        let out = parse_slash_t(
            "\u{002F}\u{002F}dbg h",
            &empty_entities(),
            origin(),
            None,
            None,
        );
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
    fn logout_no_arg_toggles_and_chains_heal_on() {
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

        let (id, ..) = expect_emote(parse_slash_t(
            "/kneel",
            &empty_entities(),
            origin(),
            None,
            None,
        ));
        assert_eq!(id, 3);
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

    #[test]
    fn load_mmb_parses_file_id_chunk_idx_and_captures_self_pos() {
        let pos = WireVec3 {
            x: 12.5,
            y: -7.0,
            z: 3.25,
        };
        match parse_slash_t(
            "\u{002F}\u{002F}load_mmb 115 18",
            &empty_entities(),
            pos,
            None,
            None,
        ) {
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

    #[test]
    fn load_mmb_on_parses_entity_id() {
        match parse_slash_t(
            "\u{002F}\u{002F}load_mmb_on 1234 115 18",
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

        assert!(matches!(
            parse_slash_t(
                "\u{002F}\u{002F}loadmmbon 99 7 0",
                &empty_entities(),
                origin(),
                None,
                None
            ),
            SlashOutcome::LoadMmb {
                entity_id: Some(99),
                ..
            }
        ));
        for s in [
            "//load_mmb_on",
            "//load_mmb_on 1234",
            "//load_mmb_on 1234 115",
            "//load_mmb_on foo 115 18",
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

    #[test]
    fn load_mmb_alias_and_bad_args() {
        assert!(matches!(
            parse_slash_t("//loadmmb 115 18", &empty_entities(), origin(), None, None),
            SlashOutcome::LoadMmb {
                file_id: 115,
                chunk_idx: 18,
                entity_id: None,
                ..
            }
        ));

        for s in [
            "//load_mmb",
            "//load_mmb 115",
            "//load_mmb foo 18",
            "//load_mmb 115 bar",
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

    #[test]
    fn load_mzb_parses_optional_chunk_idx() {
        let pos = WireVec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        };

        match parse_slash_t("//load_mzb 7368", &empty_entities(), pos, None, None) {
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

        match parse_slash_t("//load_mzb 7368 2", &empty_entities(), pos, None, None) {
            SlashOutcome::LoadMzb {
                chunk_idx: Some(2), ..
            } => {}
            other => panic!("expected LoadMzb chunk_idx=Some(2), got {other:?}"),
        }

        assert!(matches!(
            parse_slash_t("//loadmzb 7368", &empty_entities(), pos, None, None),
            SlashOutcome::LoadMzb {
                chunk_idx: None,
                ..
            }
        ));

        for s in ["//load_mzb", "//load_mzb foo", "//load_mzb 7368 bar"] {
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
    fn subarea_takes_decimal_or_hex_ids() {
        let pos = WireVec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        };

        match parse_slash_t("//subarea", &empty_entities(), pos, None, None) {
            SlashOutcome::SubArea {
                op: SubAreaOp::List,
                self_pos,
            } => assert_eq!(self_pos, pos),
            other => panic!("expected SubArea List, got {other:?}"),
        }

        // 0x1CE is the Lower Jeuno food-shop interior in
        // research/xi-tools/docs/zone/subareas.md "Worked example -- Lower Jeuno (`ROM/1/41`, zone 245)".
        for s in ["//subarea 462", "//subarea 0x1CE", "//subareas 0x1ce"] {
            match parse_slash_t(s, &empty_entities(), pos, None, None) {
                SlashOutcome::SubArea {
                    op: SubAreaOp::Load(id),
                    ..
                } => assert_eq!(id, 0x1CE, "{s}"),
                other => panic!("expected SubArea Load for {s}, got {other:?}"),
            }
        }

        assert!(matches!(
            parse_slash_t("//subarea here", &empty_entities(), pos, None, None),
            SlashOutcome::SubArea {
                op: SubAreaOp::Here,
                ..
            }
        ));

        for s in ["//subarea foo", "//subarea 0xzz", "//subarea -1"] {
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
        match parse_slash_t("//navmesh", &empty_entities(), origin(), None, None) {
            SlashOutcome::ToggleNavmesh(None) => {}
            other => panic!("expected ToggleNavmesh(None), got {other:?}"),
        }
    }

    #[test]
    fn navmesh_on_and_off_select_explicit_modes() {
        for (cmd, expected) in [("//navmesh on", Some(true)), ("//navmesh off", Some(false))] {
            match parse_slash_t(cmd, &empty_entities(), origin(), None, None) {
                SlashOutcome::ToggleNavmesh(setting) => assert_eq!(setting, expected),
                other => panic!("expected ToggleNavmesh({expected:?}), got {other:?}"),
            }
        }
    }

    #[test]
    fn navmesh_rejects_unknown_arg() {
        for s in ["//navmesh maybe", "//navmesh 1", "//navmesh ON!"] {
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
            "//pathto 1.5 2 -3.25",
            &empty_entities(),
            origin(),
            None,
            None,
        ) {
            SlashOutcome::Command(AgentCommand::PathTo { x, y, z, .. }) => {
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
        match parse_slash_t("//pathto target", &[entity], origin(), Some(42), None) {
            SlashOutcome::Command(AgentCommand::PathTo { x, y, z, .. }) => {
                assert_eq!((x, y, z), (7.0, 8.0, 9.0));
            }
            other => panic!("expected PathTo from target, got {other:?}"),
        }
    }

    #[test]
    fn pathto_rejects_bad_input() {
        for s in [
            "//pathto",
            "//pathto 1 2 3 4",
            "//pathto x y z",
            "//pathto target",
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
        let mut self_pos = origin();
        self_pos.z = 17.5;
        match parse_slash_t("//pathto 10 20", &empty_entities(), self_pos, None, None) {
            SlashOutcome::Command(AgentCommand::PathTo { x, y, z, .. }) => {
                assert_eq!((x, y, z), (10.0, 20.0, 17.5));
            }
            other => panic!("expected PathTo, got {other:?}"),
        }
    }

    #[test]
    fn pathto_fuzzy_name_picks_entity() {
        let entity = ent(42, "Bob", EntityKind::Pc, 7.0, 8.0);
        match parse_slash_t("//pathto bob", &[entity], origin(), None, None) {
            SlashOutcome::Command(AgentCommand::PathTo { x, y, .. }) => {
                assert_eq!((x, y), (7.0, 8.0));
            }
            other => panic!("expected PathTo, got {other:?}"),
        }
    }

    #[test]
    fn warp_numeric_three_args_emits_move() {
        match parse_slash_t(
            "\u{002F}\u{002F}warp 1.5 2 -3.25",
            &empty_entities(),
            origin(),
            None,
            None,
        ) {
            SlashOutcome::Command(AgentCommand::Move { x, y, z, heading }) => {
                assert_eq!((x, y, z), (1.5, 2.0, -3.25));

                assert_eq!(heading, 0);
            }
            other => panic!("expected Move, got {other:?}"),
        }
    }

    #[test]
    fn warp_two_arg_form_uses_self_z() {
        let mut self_pos = origin();
        self_pos.z = -42.0;
        match parse_slash_t(
            "\u{002F}\u{002F}warp 1 2",
            &empty_entities(),
            self_pos,
            None,
            None,
        ) {
            SlashOutcome::Command(AgentCommand::Move { x, y, z, .. }) => {
                assert_eq!((x, y, z), (1.0, 2.0, -42.0));
            }
            other => panic!("expected Move, got {other:?}"),
        }
    }

    #[test]
    fn warp_preserves_self_heading() {
        let self_pos = origin();
        let mut me = ent(1, "Me", EntityKind::Pc, self_pos.x, self_pos.y);
        me.pos.z = self_pos.z;
        me.heading = 64;
        match parse_slash_t(
            "\u{002F}\u{002F}warp 100 200 5",
            &[me],
            self_pos,
            None,
            None,
        ) {
            SlashOutcome::Command(AgentCommand::Move { heading, .. }) => {
                assert_eq!(heading, 64);
            }
            other => panic!("expected Move, got {other:?}"),
        }
    }

    #[test]
    fn warp_target_form_emits_move_to_target() {
        let entity = ent(42, "Mob", EntityKind::Mob, 11.0, 22.0);
        match parse_slash_t(
            "\u{002F}\u{002F}warp target",
            &[entity],
            origin(),
            Some(42),
            None,
        ) {
            SlashOutcome::Command(AgentCommand::Move { x, y, .. }) => {
                assert_eq!((x, y), (11.0, 22.0));
            }
            other => panic!("expected Move, got {other:?}"),
        }
    }

    #[test]
    fn warp_fuzzy_entity_match() {
        let entity = ent(42, "Bob", EntityKind::Pc, 7.0, 8.0);
        match parse_slash_t("\u{002F}\u{002F}warp bo", &[entity], origin(), None, None) {
            SlashOutcome::Command(AgentCommand::Move { x, y, .. }) => {
                assert_eq!((x, y), (7.0, 8.0));
            }
            other => panic!("expected Move, got {other:?}"),
        }
    }

    #[test]
    fn warp_rejects_empty_and_unmatched() {
        for s in ["\u{002F}\u{002F}warp", "\u{002F}\u{002F}warp nosuchname"] {
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
            parse_slash_t(
                "\u{002F}\u{002F}cancel",
                &empty_entities(),
                origin(),
                None,
                None
            ),
            SlashOutcome::Command(AgentCommand::Cancel)
        ));
    }

    #[test]
    fn attack_uses_current_target_and_engage_goal() {
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
        match parse_slash_t("/attack", &entities, origin(), Some(7), None) {
            SlashOutcome::Command(AgentCommand::Engage { target_id }) => {
                assert_eq!(target_id, 7);
            }
            other => panic!("expected Engage, got {other:?}"),
        }
    }

    #[test]
    fn attack_out_of_engage_range_stays_local() {
        let entities = vec![ent(42, "Bao Bat", EntityKind::Mob, 40.0, 0.0)];
        match parse_slash_t("/attack", &entities, origin(), Some(42), None) {
            SlashOutcome::SystemMessage(msg) => assert_eq!(msg, "Bao Bat is too far away."),
            other => panic!("expected the range rejection, got {other:?}"),
        }
    }

    #[test]
    fn attack_on_a_strangers_claim_stays_local() {
        let mut e = ent(42, "Bao Bat", EntityKind::Mob, 10.0, 0.0);
        e.claim_id = 0x0100_0002;
        let entities = vec![e];
        match parse_slash_t("/attack", &entities, origin(), Some(42), None) {
            SlashOutcome::SystemMessage(msg) => {
                assert_eq!(msg, "Cannot attack. Your target is already claimed.")
            }
            other => panic!("expected the claim rejection, got {other:?}"),
        }
    }

    #[test]
    fn attack_on_own_claim_still_engages() {
        let mut e = ent(42, "Bao Bat", EntityKind::Mob, 10.0, 0.0);
        e.claim_id = 0x0100_0001;
        let entities = vec![e];
        let out = parse_slash(
            "/attack",
            &test_surface(),
            &entities,
            origin(),
            Some(42),
            None,
            Some(0x0100_0001),
            &[],
            kuluu_render::fishing_spot::FishingGate::Ready,
            None,
            None,
        );
        assert!(matches!(
            out,
            SlashOutcome::Command(AgentCommand::Engage { target_id: 42 })
        ));
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
    fn engage_dispatches_reactor_goal() {
        let entities = vec![ent(99, "Bee", EntityKind::Mob, 1.0, 0.0)];
        match parse_slash_t("/attack", &entities, origin(), Some(99), None) {
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
    fn raw_attack_preserves_direct_action() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash_t(
            "\u{002F}\u{002F}raw attack",
            &entities,
            origin(),
            Some(7),
            None,
        ) {
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
        match parse_slash_t(
            "\u{002F}\u{002F}raw attackoff",
            &entities,
            origin(),
            Some(7),
            None,
        ) {
            SlashOutcome::Command(AgentCommand::Action { kind, .. }) => {
                assert!(matches!(kind, ActionKind::AttackOff));
            }
            other => panic!("expected Action{{AttackOff}}, got {other:?}"),
        }
    }

    #[test]
    fn cast_with_explicit_target_and_ground_coords() {
        match parse_slash_t(
            "/magic 257 99 7 1.0 0.0 2.0",
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
    fn cast_resolves_a_raw_id_target_without_the_picker() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash_t("/magic 1 7", &entities, origin(), Some(7), None) {
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
        match parse_slash_t("/weaponskill 16 7", &entities, origin(), Some(7), None) {
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
    fn job_ability_with_no_target_opens_the_picker() {
        // Assault (88) is enemy-targeted, so a missing target argument hands
        // the choice to the sub-target cursor instead of defaulting to zero.
        assert!(matches!(
            parse_slash_t("/jobability 88", &empty_entities(), origin(), None, None),
            SlashOutcome::OpenSubTarget { .. }
        ));
    }

    fn parse_slash_as(
        buffer: &str,
        entities: &[WireEntity],
        current_target: Option<u32>,
        self_char_id: Option<u32>,
        party: &[kuluu_snapshot::PartyMember],
        battle_target: Option<u32>,
        self_pet_targid: Option<u16>,
    ) -> SlashOutcome {
        parse_slash(
            buffer,
            &test_surface(),
            entities,
            origin(),
            current_target,
            None,
            self_char_id,
            party,
            kuluu_render::fishing_spot::FishingGate::Ready,
            battle_target,
            self_pet_targid,
        )
    }

    fn party_member(id: u32, act_index: u16, party_no: u8) -> kuluu_snapshot::PartyMember {
        kuluu_snapshot::PartyMember {
            id,
            act_index,
            name: None,
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
            party_no,
            in_mog_house: false,
        }
    }

    fn ability_action(out: SlashOutcome) -> (u32, u32, u16) {
        match out {
            SlashOutcome::Command(AgentCommand::Action {
                target_id,
                target_index,
                kind: ActionKind::JobAbility { ability_id },
            }) => (ability_id, target_id, target_index),
            other => panic!("expected JobAbility, got {other:?}"),
        }
    }

    /// The form retail macros are written in, and the one a player types.
    #[test]
    fn job_ability_takes_a_quoted_name_and_the_self_token() {
        let flee = u32::from(ffxi_vocab::ability_names::id_for("Flee").expect("Flee present"));
        let entities = vec![ent(9, "Me", EntityKind::Pc, 0.0, 0.0)];
        let (ability_id, target_id, target_index) = ability_action(parse_slash_as(
            "/ja \"Flee\" <me>",
            &entities,
            None,
            Some(9),
            &[],
            None,
            None,
        ));
        assert_eq!((ability_id, target_id), (flee, 9));
        assert_eq!(target_index, entities[0].act_index);
        assert_eq!(
            ability_action(parse_slash_as(
                "/ja Flee",
                &entities,
                None,
                Some(9),
                &[],
                None,
                None
            ))
            .0,
            flee
        );
    }

    /// A multi-word name only survives the split inside quotes.
    #[test]
    fn action_names_with_spaces_need_their_quotes() {
        let entities = vec![ent(9, "Me", EntityKind::Pc, 0.0, 0.0)];
        let strikes = u32::from(
            ffxi_vocab::ability_names::id_for("Mighty Strikes").expect("Mighty Strikes present"),
        );
        assert_eq!(
            ability_action(parse_slash_as(
                "/ja \"Mighty Strikes\"",
                &entities,
                None,
                Some(9),
                &[],
                None,
                None
            ))
            .0,
            strikes
        );
        assert!(matches!(
            parse_slash_as(
                "/ja Mighty Strikes",
                &entities,
                None,
                Some(9),
                &[],
                None,
                None
            ),
            SlashOutcome::SystemMessage(_)
        ));
    }

    /// A self-only ability needs no target argument, as in retail and as the
    /// menus already dispatch it.
    #[test]
    fn a_self_only_ability_targets_self_without_a_token() {
        let entities = vec![ent(9, "Me", EntityKind::Pc, 0.0, 0.0)];
        let flee = ffxi_vocab::ability_names::id_for("Flee").expect("Flee present");
        assert!(
            ffxi_vocab::valid_target::ability(flee).is_some_and(|f| f.is_self_only()),
            "Flee is the self-only case this test rests on"
        );
        let (_, target_id, _) = ability_action(parse_slash_as(
            "/ja Flee",
            &entities,
            None,
            Some(9),
            &[],
            None,
            None,
        ));
        assert_eq!(target_id, 9);
    }

    #[test]
    fn party_and_alliance_tokens_index_one_slot_space() {
        let party = vec![
            party_member(1, 10, 0),
            party_member(2, 20, 0),
            party_member(3, 30, 1),
            party_member(4, 40, 2),
        ];
        let cure = u32::from(ffxi_vocab::spell_names::id_for("Cure").expect("Cure present"));
        for (token, want) in [("<p1>", (2, 20)), ("<a10>", (3, 30)), ("<a20>", (4, 40))] {
            match parse_slash_as(
                &format!("/ma \"Cure\" {token}"),
                &empty_entities(),
                None,
                None,
                &party,
                None,
                None,
            ) {
                SlashOutcome::Command(AgentCommand::Action {
                    target_id,
                    target_index,
                    kind: ActionKind::CastMagic { spell_id, .. },
                }) => {
                    assert_eq!(spell_id, cure);
                    assert_eq!((target_id, target_index), want, "{token}");
                }
                other => panic!("{token}: expected CastMagic, got {other:?}"),
            }
        }
        assert!(matches!(
            parse_slash_as(
                "/ma Cure <p5>",
                &empty_entities(),
                None,
                None,
                &party,
                None,
                None
            ),
            SlashOutcome::SystemMessage(_)
        ));
    }

    /// `<st>` and its suffixed forms open retail's sub-target cursor instead of
    /// resolving a target here.
    #[test]
    fn st_token_opens_the_picker_on_the_spell_mask() {
        let cure = u32::from(ffxi_vocab::spell_names::id_for("Cure").expect("Cure present"));
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash_as("/ma Cure <st>", &entities, Some(7), None, &[], None, None) {
            SlashOutcome::OpenSubTarget { action, narrow } => {
                assert_eq!(
                    action,
                    kuluu_render::input_mode::SubTargetAction::Spell(
                        u16::try_from(cure).expect("spell id fits")
                    )
                );
                assert_eq!(narrow, None);
            }
            other => panic!("expected the sub-target picker, got {other:?}"),
        }
    }

    #[test]
    fn stpt_token_narrows_to_party() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash_as("/ma Cure <stpt>", &entities, Some(7), None, &[], None, None) {
            SlashOutcome::OpenSubTarget { narrow, .. } => {
                assert_eq!(
                    narrow,
                    Some(ffxi_vocab::valid_target::TargetFlags(
                        ffxi_vocab::valid_target::TargetFlags::SELF
                            | ffxi_vocab::valid_target::TargetFlags::PLAYER_PARTY
                    ))
                );
            }
            other => panic!("expected the sub-target picker, got {other:?}"),
        }
    }

    #[test]
    fn no_target_argument_prompts_the_cursor() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        assert!(matches!(
            parse_slash_as("/ma Cure", &entities, Some(7), None, &[], None, None),
            SlashOutcome::OpenSubTarget { .. }
        ));
        // A self-only ability typed with no target fires at the player instead.
        let strikes = u32::from(
            ffxi_vocab::ability_names::id_for("Mighty Strikes").expect("Mighty Strikes present"),
        );
        let me = ent(9, "Me", EntityKind::Pc, 0.0, 0.0);
        let (ability_id, target_id, _) = ability_action(parse_slash_as(
            "/ja \"Mighty Strikes\"",
            &[me],
            None,
            Some(9),
            &[],
            None,
            None,
        ));
        assert_eq!((ability_id, target_id), (strikes, 9));
    }

    #[test]
    fn bt_resolves_the_engaged_target() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        let dia = u32::from(ffxi_vocab::spell_names::id_for("Dia").expect("Dia present"));
        match parse_slash_as("/ma Dia <bt>", &entities, Some(7), None, &[], Some(7), None) {
            SlashOutcome::Command(AgentCommand::Action {
                target_id,
                target_index,
                kind: ActionKind::CastMagic { spell_id, .. },
            }) => {
                assert_eq!(spell_id, dia);
                assert_eq!(target_id, 7);
                assert_eq!(target_index, 7);
            }
            other => panic!("expected CastMagic, got {other:?}"),
        }
        let not_engaged =
            match parse_slash_as("/ma Dia <bt>", &entities, Some(7), None, &[], None, None) {
                SlashOutcome::SystemMessage(m) => m,
                other => panic!("expected a message, got {other:?}"),
            };
        assert!(not_engaged.contains("not engaged"), "{not_engaged}");
    }

    #[test]
    fn pet_resolves_the_own_pet() {
        let pet = ent(0x120, "Petite Cactuar", EntityKind::Pet, 0.0, 0.0);
        let entities = vec![pet];
        let cure = u32::from(ffxi_vocab::spell_names::id_for("Cure").expect("Cure present"));
        match parse_slash_as(
            "/ma Cure <pet>",
            &entities,
            None,
            None,
            &[],
            None,
            Some(0x120),
        ) {
            SlashOutcome::Command(AgentCommand::Action {
                target_id,
                target_index,
                kind: ActionKind::CastMagic { spell_id, .. },
            }) => {
                assert_eq!(spell_id, cure);
                assert_eq!(target_id, 0x120);
                assert_eq!(target_index, 0x120);
            }
            other => panic!("expected CastMagic, got {other:?}"),
        }
        let no_pet = match parse_slash_as("/ma Cure <pet>", &entities, None, None, &[], None, None)
        {
            SlashOutcome::SystemMessage(m) => m,
            other => panic!("expected a message, got {other:?}"),
        };
        assert!(no_pet.contains("no pet"), "{no_pet}");
    }

    /// A token the client has but Kuluu cannot answer yet must not read as a
    /// typo, and neither may silently target something else.
    #[test]
    fn unresolved_and_unknown_target_tokens_report_differently() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        let unresolved = match parse_slash_as(
            "/ws \"Fast Blade\" <ft>",
            &entities,
            Some(7),
            None,
            &[],
            None,
            None,
        ) {
            SlashOutcome::SystemMessage(m) => m,
            other => panic!("expected a message, got {other:?}"),
        };
        assert!(unresolved.contains("not supported yet"), "{unresolved}");
        let unknown = match parse_slash_as(
            "/ws \"Fast Blade\" <nope>",
            &entities,
            Some(7),
            None,
            &[],
            None,
            None,
        ) {
            SlashOutcome::SystemMessage(m) => m,
            other => panic!("expected a message, got {other:?}"),
        };
        assert!(unknown.contains("unknown target token"), "{unknown}");
    }

    /// /ws takes a skill name and the current-target token. A monster-only TP
    /// move shares no name space with the command, so it reports as unknown.
    #[test]
    fn weaponskill_takes_a_name_and_the_current_target_token() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        let fast_blade = u32::from(
            ffxi_vocab::weapon_skill_names::id_for("Fast Blade").expect("Fast Blade present"),
        );
        match parse_slash_as(
            "/ws \"Fast Blade\" <t>",
            &entities,
            Some(7),
            None,
            &[],
            None,
            None,
        ) {
            SlashOutcome::Command(AgentCommand::Action {
                target_id,
                kind: ActionKind::Weaponskill { skill_id },
                ..
            }) => {
                assert_eq!(skill_id, fast_blade);
                assert_eq!(target_id, 7);
            }
            other => panic!("expected Weaponskill, got {other:?}"),
        }
        assert!(matches!(
            parse_slash_as(
                "/ws \"Uppercut\" <t>",
                &entities,
                Some(7),
                None,
                &[],
                None,
                None
            ),
            SlashOutcome::SystemMessage(_)
        ));
    }

    /// The ground-target triple still lands after a target given as a token
    /// rather than as the raw id/index pair.
    #[test]
    fn ground_target_coords_follow_whatever_form_the_target_took() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        for slash in ["/ma Cure <t> 1 2 3", "/ma Cure 7 0 1 2 3"] {
            match parse_slash_as(slash, &entities, Some(7), None, &[], None, None) {
                SlashOutcome::Command(AgentCommand::Action {
                    kind:
                        ActionKind::CastMagic {
                            pos_x,
                            pos_y,
                            pos_z,
                            ..
                        },
                    ..
                }) => assert_eq!((pos_x, pos_y, pos_z), (1.0, 2.0, 3.0), "{slash}"),
                other => panic!("{slash}: expected CastMagic, got {other:?}"),
            }
        }
    }

    #[test]
    fn useitem_basic() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash_t("/item 0 4 4112 7", &entities, origin(), Some(7), None) {
            SlashOutcome::Command(AgentCommand::UseItem {
                container,
                slot,
                item_no,
                target_id,
                ..
            }) => {
                assert_eq!(container, 0);
                assert_eq!(slot, 4);
                assert_eq!(item_no, 4112);
                assert_eq!(target_id, 7);
            }
            other => panic!("expected UseItem, got {other:?}"),
        }
        // No target argument hands the choice to the sub-target cursor.
        assert!(matches!(
            parse_slash_t("/item 0 4 4112", &empty_entities(), origin(), None, None),
            SlashOutcome::OpenSubTarget { .. }
        ));
    }

    #[test]
    fn endevent_aliases_dispatch_end_event() {
        for input in [
            "\u{002F}\u{002F}endevent",
            "\u{002F}\u{002F}endevt",
            "\u{002F}\u{002F}clearevent",
            "\u{002F}\u{002F}clearevt",
        ] {
            match parse_slash_t(input, &empty_entities(), origin(), None, None) {
                SlashOutcome::Command(AgentCommand::EndEvent) => {}
                other => panic!("input {input:?}: expected EndEvent, got {other:?}"),
            }
        }
    }

    #[test]
    fn endcutscene_no_arg_returns_none() {
        match parse_slash_t(
            "\u{002F}\u{002F}endcutscene",
            &empty_entities(),
            origin(),
            None,
            Some(231),
        ) {
            SlashOutcome::EndCutscene { event_num } => assert_eq!(event_num, None),
            other => panic!("expected EndCutscene{{ None }}, got {other:?}"),
        }
    }

    #[test]
    fn endcutscene_with_explicit_csid_overrides_zone_lookup() {
        match parse_slash_t(
            "\u{002F}\u{002F}endcutscene 7",
            &empty_entities(),
            origin(),
            None,
            Some(235),
        ) {
            SlashOutcome::EndCutscene { event_num } => assert_eq!(event_num, Some(7)),
            other => panic!("expected EndCutscene{{ Some(7) }}, got {other:?}"),
        }
    }

    #[test]
    fn endcutscene_bad_csid_errors() {
        match parse_slash_t(
            "\u{002F}\u{002F}endcutscene abc",
            &empty_entities(),
            origin(),
            None,
            Some(231),
        ) {
            SlashOutcome::SystemMessage(msg) => assert!(msg.to_lowercase().contains("bad csid")),
            other => panic!("expected SystemMessage, got {other:?}"),
        }
    }
    #[test]
    fn endcutscene_aliases_all_work() {
        for input in [
            "\u{002F}\u{002F}endcutscene",
            "\u{002F}\u{002F}endcs",
            "\u{002F}\u{002F}skipcutscene",
            "\u{002F}\u{002F}skipcs",
        ] {
            match parse_slash_t(input, &empty_entities(), origin(), None, Some(231)) {
                SlashOutcome::EndCutscene { event_num } => assert_eq!(event_num, None),
                other => panic!("input {input:?}: expected EndCutscene, got {other:?}"),
            }
        }
    }
    #[test]
    fn snapshot_is_direct() {
        assert!(matches!(
            parse_slash_t(
                "\u{002F}\u{002F}snapshot",
                &empty_entities(),
                origin(),
                None,
                None
            ),
            SlashOutcome::Command(AgentCommand::Snapshot)
        ));
    }

    #[test]
    fn bank_parses_threshold_and_zoneline() {
        if let SlashOutcome::SystemMessage(_) = parse_slash_t(
            "/bank 60 0xDEADBEEF",
            &empty_entities(),
            origin(),
            None,
            None,
        ) {}
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
        match parse_slash_t(
            "\u{002F}\u{002F}zonechange 42",
            &empty_entities(),
            origin(),
            None,
            None,
        ) {
            SlashOutcome::Command(AgentCommand::RequestZoneChange { line_id }) => {
                assert_eq!(line_id, 42);
            }
            other => panic!("expected RequestZoneChange, got {other:?}"),
        }
    }
    #[test]
    fn agent_pause_resume_status_parse() {
        for (input, expected) in &[
            ("\u{002F}\u{002F}agent pause", AgentControlOp::Pause),
            ("\u{002F}\u{002F}agent resume", AgentControlOp::Resume),
            ("\u{002F}\u{002F}agent unpause", AgentControlOp::Resume),
            ("\u{002F}\u{002F}agent status", AgentControlOp::Status),
            ("\u{002F}\u{002F}agent", AgentControlOp::Status),
        ] {
            match parse_slash_t(input, &empty_entities(), origin(), None, None) {
                SlashOutcome::AgentControl(op) => assert_eq!(&op, expected, "input: {input}"),
                other => panic!("expected AgentControl for `{input}`, got {other:?}"),
            }
        }
    }

    #[test]
    fn agent_unknown_subcommand_is_system_message() {
        match parse_slash_t(
            "\u{002F}\u{002F}agent wat",
            &empty_entities(),
            origin(),
            None,
            None,
        ) {
            SlashOutcome::SystemMessage(s) => assert!(s.contains("wat")),
            other => panic!("expected SystemMessage, got {other:?}"),
        }
    }
    #[test]
    fn help_command_returns_multiline_listing() {
        for slash in ["/help", "/?", "/h"] {
            let out = parse_slash_t(slash, &empty_entities(), origin(), None, None);
            match out {
                SlashOutcome::SystemMessage(s) => {
                    assert!(s.starts_with("=== Retail commands"), "{slash}: {s}");
                    assert!(s.contains("/follow"), "{slash} missing /follow");
                    assert!(s.contains("/help"), "{slash} missing /help self-reference");
                }
                other => panic!("expected SystemMessage from {slash}, got {other:?}"),
            }
        }
    }

    #[test]
    fn between_them_the_two_listings_name_every_category() {
        let surface = test_surface();
        let both = format!(
            "{}\n{}",
            render_help(&surface, Surface::Retail),
            render_help(&surface, Surface::Extension)
        );
        for (category, _) in COMMANDS {
            assert!(both.contains(category), "no listing names `{category}`");
        }
    }

    #[test]
    fn help_listing_fits_in_local_toast_cap() {
        let cap = kuluu_render::snapshot::LOCAL_TOAST_CAP;
        for (which, typed) in [
            (Surface::Retail, "/?"),
            (Surface::Extension, "\u{002F}\u{002F}?"),
        ] {
            let lines = render_help(&test_surface(), which).split('\n').count();
            assert!(
                lines <= cap,
                "`{typed}` renders {lines} lines but the chat buffer retains only {cap}; \
                 the top {} lines would be evicted unreadable",
                lines.saturating_sub(cap),
            );
        }
    }

    #[test]
    fn retail_help_lists_no_extension_command_and_points_at_the_other_surface() {
        let text = render_help(&test_surface(), Surface::Retail);
        assert!(
            !text.contains(EXTENSION_PREFIX.to_string().as_str())
                || text.contains("\u{002F}\u{002F}?"),
            "the only \u{002F}\u{002F} in the retail listing is the hint"
        );
        for line in text.split('\n').filter(|l| l.starts_with("  ")) {
            assert!(
                !line.trim_start().starts_with(EXTENSION_PREFIX),
                "retail listing named an extension command: {line}"
            );
        }
        assert!(text.contains("/attack"), "retail listing names its own");
        assert!(
            text.ends_with(&format!("{EXTENSION_PREFIX}{}", EXTENSION_HELP_NAMES[0])),
            "the last line points at the extension help: {text}"
        );
    }

    #[test]
    fn extension_help_lists_no_retail_command() {
        let text = render_help(&test_surface(), Surface::Extension);
        for line in text.split('\n').filter(|l| l.starts_with("  ")) {
            assert!(
                line.trim_start().starts_with(EXTENSION_PREFIX),
                "extension listing named a retail command: {line}"
            );
        }
        assert!(text.contains("\u{002F}\u{002F}exit"));
        assert!(text.contains("\u{002F}\u{002F}minimap"));
    }

    #[test]
    fn a_disabled_set_drops_out_of_the_listing_but_help_stays() {
        let mut surface = test_surface();
        surface.enabled.set_enabled(CommandSet::Dev, false);
        let text = render_help(&surface, Surface::Extension);
        assert!(
            !text.contains("\u{002F}\u{002F}pathto"),
            "dev is off: {text}"
        );
        assert!(
            text.contains("\u{002F}\u{002F}?"),
            "help lists itself whatever is off"
        );
    }

    #[test]
    fn extension_help_answers_with_its_own_set_switched_off() {
        let mut surface = test_surface();
        surface.enabled.set_enabled(CommandSet::Core, false);
        let out = parse_slash(
            "\u{002F}\u{002F}?",
            &surface,
            &empty_entities(),
            origin(),
            None,
            None,
            None,
            &[],
            kuluu_render::fishing_spot::FishingGate::Ready,
            None,
            None,
        );
        match out {
            SlashOutcome::SystemMessage(s) => {
                assert!(s.starts_with("=== Kuluu commands"), "{s}")
            }
            other => panic!("\u{002F}\u{002F}? did not answer: {other:?}"),
        }
    }

    /// The doubled-slash exit quits; retail has no single-slash exit, so that
    /// form is a miss with a hint.
    #[test]
    fn exit_is_an_extension_command_and_quits() {
        assert!(matches!(
            parse_slash_t(
                "\u{002F}\u{002F}exit",
                &empty_entities(),
                origin(),
                None,
                None
            ),
            SlashOutcome::Quit
        ));
        match parse_slash_t("/exit", &empty_entities(), origin(), None, None) {
            SlashOutcome::SystemMessage(s) => assert!(
                s.contains(&format!("{EXTENSION_PREFIX}exit")),
                "expected a did-you-mean: {s}"
            ),
            other => panic!("/exit answered: {other:?}"),
        }
    }

    #[test]
    fn every_registered_name_dispatches_on_its_own_surface() {
        for (_, cmds) in COMMANDS {
            for cmd in *cmds {
                for name in cmd.names {
                    let slash = format!("{}{name}", cmd.prefix());
                    let out = parse_slash_t(&slash, &empty_entities(), origin(), None, None);
                    if let SlashOutcome::SystemMessage(ref s) = out {
                        assert!(
                            !s.starts_with("unknown command:"),
                            "registered `{slash}` dispatched to the unknown-command fallthrough"
                        );
                    }
                }
            }
        }
    }

    /// The prefix split is the whole point: a Kuluu command must not answer on
    /// the retail slash, and a retail command must not answer on the doubled
    /// one. An emote is a retail command of its own, reached through the
    /// scraped table rather than through COMMANDS, so it is skipped. A word
    /// may name one command on each surface (magic casts as retail does while
    /// its doubled form opens the menu retail has no command for) — neither is
    /// "wrong", so those are skipped too.
    #[test]
    fn a_name_answers_only_on_its_own_surface() {
        for (_, cmds) in COMMANDS {
            for cmd in *cmds {
                for name in cmd.names {
                    let wrong = if cmd.set.is_retail() {
                        format!("{EXTENSION_PREFIX}{name}")
                    } else {
                        format!("/{name}")
                    };
                    if !cmd.set.is_retail()
                        && ffxi_vocab::emote_names::id_for_command(name).is_some()
                    {
                        continue;
                    }
                    if commands()
                        .any(|o| o.set.is_retail() != cmd.set.is_retail() && o.names.contains(name))
                    {
                        continue;
                    }
                    match parse_slash_t(&wrong, &empty_entities(), origin(), None, None) {
                        SlashOutcome::SystemMessage(s) => assert!(
                            s.starts_with("unknown command:"),
                            "`{wrong}` answered on the wrong surface: {s}"
                        ),
                        other => {
                            panic!("`{wrong}` answered on the wrong surface: {other:?}")
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_moved_command_says_where_it_went() {
        match parse_slash_t("/pathto target", &empty_entities(), origin(), None, None) {
            SlashOutcome::SystemMessage(s) => {
                assert!(
                    s.contains("\u{002F}\u{002F}pathto"),
                    "no forwarding hint: {s}"
                );
            }
            other => panic!("expected the moved-command hint, got {other:?}"),
        }
    }

    /// A retail command with no handler reads differently from a word that is
    /// not a command at all, so a player can tell "not built yet" from "no
    /// such command".
    #[test]
    fn a_retail_command_kuluu_lacks_is_not_reported_as_unknown() {
        match parse_slash_t("/recast Cure", &empty_entities(), origin(), None, None) {
            SlashOutcome::SystemMessage(s) => {
                assert!(s.contains("not supported yet"), "{s}");
                assert!(!s.starts_with("unknown command:"), "{s}");
            }
            other => panic!("expected a not-supported message, got {other:?}"),
        }
        match parse_slash_t("/blargh", &empty_entities(), origin(), None, None) {
            SlashOutcome::SystemMessage(s) => assert!(s.starts_with("unknown command:"), "{s}"),
            other => panic!("expected unknown-command, got {other:?}"),
        }
    }

    /// Aliases come from the install's table, so a command answers to every
    /// spelling the client accepts without Kuluu listing any of them.
    #[test]
    fn an_install_alias_reaches_the_canonical_handler() {
        for (alias, long) in [
            ("/a", "/attack"),
            ("/ws 1", "/weaponskill 1"),
            ("/c", "/check"),
        ] {
            let by_alias = parse_slash_t(alias, &empty_entities(), origin(), Some(7), None);
            let by_long = parse_slash_t(long, &empty_entities(), origin(), Some(7), None);
            assert_eq!(
                format!("{by_alias:?}"),
                format!("{by_long:?}"),
                "`{alias}` did not reach the same handler as `{long}`"
            );
        }
    }

    /// LSB names emote 7 `Yes` and emote 26 `Disgusted`; the client also
    /// accepts `/nod` and `/upset`, which only the install's table knows.
    #[test]
    fn an_emote_alias_the_scrape_does_not_name_still_resolves() {
        for (alias, long) in [
            ("/nod", "/yes"),
            ("/upset", "/disgusted"),
            ("/farewell", "/goodbye"),
        ] {
            let by_alias = parse_slash_t(alias, &empty_entities(), origin(), None, None);
            let by_long = parse_slash_t(long, &empty_entities(), origin(), None, None);
            assert_eq!(
                format!("{by_alias:?}"),
                format!("{by_long:?}"),
                "`{alias}` did not resolve to the same emote as `{long}`"
            );
            assert!(
                matches!(by_alias, SlashOutcome::Command(AgentCommand::Emote { .. })),
                "`{alias}` did not emote: {by_alias:?}"
            );
        }
    }

    /// With no table, long forms still answer -- the degradation that keeps
    /// an unrecognised build usable instead of dark; an alias does not,
    /// which is the cost of the missing table.
    #[test]
    fn long_forms_answer_without_an_install_table() {
        let bare = CommandSurface::default();
        assert!(!bare.table_loaded());
        let entities = vec![ent(7, "Goblin", EntityKind::Mob, 3.0, 0.0)];
        let out = parse_slash(
            "/attack",
            &bare,
            &entities,
            origin(),
            Some(7),
            None,
            None,
            &[],
            kuluu_render::fishing_spot::FishingGate::Ready,
            None,
            None,
        );
        assert!(
            matches!(out, SlashOutcome::Command(AgentCommand::Engage { .. })),
            "{out:?}"
        );
        let aliased = parse_slash(
            "/a",
            &bare,
            &entities,
            origin(),
            Some(7),
            None,
            None,
            &[],
            kuluu_render::fishing_spot::FishingGate::Ready,
            None,
            None,
        );
        assert!(
            matches!(aliased, SlashOutcome::SystemMessage(ref s) if s.starts_with("unknown command:")),
            "{aliased:?}"
        );
    }

    #[test]
    fn a_disabled_set_says_so_rather_than_unknown() {
        let mut surface = test_surface();
        surface.enabled.set_enabled(CommandSet::Dev, false);
        let out = parse_slash(
            "\u{002F}\u{002F}noclip",
            &surface,
            &empty_entities(),
            origin(),
            None,
            None,
            None,
            &[],
            kuluu_render::fishing_spot::FishingGate::Ready,
            None,
            None,
        );
        match out {
            SlashOutcome::SystemMessage(s) => {
                assert!(s.contains("dev"), "{s}");
                assert!(!s.starts_with("unknown command:"), "{s}");
            }
            other => panic!("expected a disabled-set message, got {other:?}"),
        }
    }

    fn expect_emote(outcome: SlashOutcome) -> (u8, u8, u16, Option<u32>, Option<u16>) {
        match outcome {
            SlashOutcome::Command(AgentCommand::Emote {
                emote_id,
                mode,
                param,
                target_id,
                target_index,
            }) => (emote_id, mode, param, target_id, target_index),
            other => panic!("expected an Emote command, got {other:?}"),
        }
    }

    #[test]
    fn canned_emote_names_are_commands() {
        use ffxi_proto::map::emote::mode;
        let rabbit = ent(9, "Wild Rabbit", EntityKind::Mob, 1.0, 1.0);
        let (id, mode, param, tid, tidx) = expect_emote(parse_slash_t(
            "/wave",
            std::slice::from_ref(&rabbit),
            WireVec3::default(),
            Some(9),
            None,
        ));
        assert_eq!((id, mode, param), (8, mode::ALL, 0));
        assert_eq!((tid, tidx), (Some(9), Some(9)));

        let (_, mode, _, tid, _) = expect_emote(parse_slash_t(
            "/wave motion",
            &[],
            WireVec3::default(),
            None,
            None,
        ));
        assert_eq!(mode, mode::MOTION);
        assert_eq!(tid, None, "no selection -> untargeted");

        assert!(matches!(
            parse_slash_t("/wave sideways", &[], WireVec3::default(), None, None),
            SlashOutcome::SystemMessage(_)
        ));
        // /kneel is the emote, not a sit alias.
        let (id, ..) = expect_emote(parse_slash_t(
            "/kneel",
            &[],
            WireVec3::default(),
            None,
            None,
        ));
        assert_eq!(id, 3);
        // HELM emotes are server-initiated only.
        assert!(matches!(
            parse_slash_t("/logging", &[], WireVec3::default(), None, None),
            SlashOutcome::SystemMessage(_)
        ));
    }

    #[test]
    fn jobemote_maps_job_to_param_base() {
        use ffxi_proto::map::emote;
        let (id, _, param, ..) = expect_emote(parse_slash_t(
            "/jobemote war",
            &[],
            WireVec3::default(),
            None,
            None,
        ));
        assert_eq!(id, emote::JOB);
        assert_eq!(param, emote::JOB_PARAM_BASE, "WAR(1) -> 0x1F");
        let (_, _, param, ..) = expect_emote(parse_slash_t(
            "/jobemote RUN",
            &[],
            WireVec3::default(),
            None,
            None,
        ));
        assert_eq!(param, emote::JOB_PARAM_BASE + 21, "RUN(22) -> 0x34");
        assert!(matches!(
            parse_slash_t("/jobemote xyz", &[], WireVec3::default(), None, None),
            SlashOutcome::SystemMessage(_)
        ));
    }

    #[test]
    fn bell_notes_span_the_two_octave_wire_range() {
        use ffxi_proto::map::emote;
        assert_eq!(parse_bell_note("c4"), Some(emote::BELL_NOTE_MIN));
        assert_eq!(parse_bell_note("c#4"), Some(emote::BELL_NOTE_MIN + 1));
        assert_eq!(parse_bell_note("db4"), Some(emote::BELL_NOTE_MIN + 1));
        assert_eq!(parse_bell_note("c5"), Some(emote::BELL_NOTE_MIN + 12));
        assert_eq!(parse_bell_note("c6"), Some(emote::BELL_NOTE_MAX));
        assert_eq!(parse_bell_note("6"), Some(emote::BELL_NOTE_MIN));
        assert_eq!(parse_bell_note("30"), Some(emote::BELL_NOTE_MAX));
        assert_eq!(parse_bell_note("c#6"), None, "past the top octave");
        assert_eq!(parse_bell_note("31"), None);
        assert_eq!(parse_bell_note("h4"), None);

        let (id, _, param, ..) = expect_emote(parse_slash_t(
            "/bell e4",
            &[],
            WireVec3::default(),
            None,
            None,
        ));
        assert_eq!(id, emote::BELL);
        assert_eq!(param, emote::BELL_NOTE_MIN + 4);
    }

    /// The emote fallback runs after the COMMANDS lookup, so an alias equal to
    /// a scraped emote name would silently shadow the emote -- forbid it
    /// (Bell/Job keep dedicated commands with required args).
    #[test]
    fn no_alias_shadows_a_scraped_emote_name() {
        use ffxi_proto::map::emote;
        for &(id, name) in ffxi_vocab::emote_names::EMOTES {
            if emote::HELM_ONLY.contains(&id) || id == emote::BELL || id == emote::JOB {
                continue;
            }
            let lower = name.to_lowercase();
            for (category, cmds) in COMMANDS {
                for cmd in *cmds {
                    assert!(
                        !(cmd.set.is_retail() && cmd.names.contains(&lower.as_str())),
                        "`/{lower}` in `{category}` shadows emote {id}"
                    );
                }
            }
        }
    }

    fn overlay_op(rest: &str) -> OverlayOp {
        match parse_overlay(rest) {
            SlashOutcome::Overlay(op) => op,
            other => panic!("expected an overlay op for `{rest}`, got {other:?}"),
        }
    }

    #[test]
    fn overlay_defaults_to_listing() {
        assert_eq!(overlay_op(""), OverlayOp::List);
        assert_eq!(overlay_op("  "), OverlayOp::List);
        assert_eq!(overlay_op("list"), OverlayOp::List);
    }

    // Overlay directories routinely sit under paths with spaces, so the
    // argument is taken verbatim rather than split into words.
    #[test]
    fn overlay_add_keeps_a_path_with_spaces_intact() {
        assert_eq!(
            overlay_op("add /Users/x/Game/polplugins/DATs/xi view"),
            OverlayOp::Add(std::path::PathBuf::from(
                "/Users/x/Game/polplugins/DATs/xi view"
            ))
        );
    }

    // Clearing (run with no overlays) and resetting (go back to discovery) are
    // different outcomes, and the display list is 1-based.
    #[test]
    fn overlay_clear_reset_and_remove_are_distinct() {
        assert_eq!(overlay_op("clear"), OverlayOp::Clear);
        assert_eq!(overlay_op("reset"), OverlayOp::Reset);
        assert_eq!(overlay_op("remove 1"), OverlayOp::Remove(1));
        assert_eq!(overlay_op("rm 3"), OverlayOp::Remove(3));
    }

    #[test]
    fn overlay_rejects_bad_input_instead_of_guessing() {
        for bad in ["add", "remove", "remove 0", "remove x", "wat"] {
            assert!(
                matches!(parse_overlay(bad), SlashOutcome::SystemMessage(_)),
                "`/overlay {bad}` must explain itself, not act"
            );
        }
    }

    #[test]
    fn actordiag_targets_self_by_default_and_the_target_on_request() {
        for me in ["", "  ", "self"] {
            assert!(matches!(
                parse_actordiag(me),
                SlashOutcome::ActorDiag { use_target: false }
            ));
        }
        for tgt in ["t", "target", "TARGET"] {
            assert!(matches!(
                parse_actordiag(tgt),
                SlashOutcome::ActorDiag { use_target: true }
            ));
        }
        assert!(matches!(
            parse_actordiag("wat"),
            SlashOutcome::SystemMessage(_)
        ));
    }

    #[test]
    fn lights_shadowed_takes_a_count_and_rejects_junk() {
        assert!(matches!(
            parse_lights("shadowed 3"),
            SlashOutcome::SetLights(LightsOp::Shadowed(3))
        ));
        assert!(matches!(
            parse_lights("shadowed lots"),
            SlashOutcome::SystemMessage(_)
        ));
        assert!(matches!(
            parse_lights("threshold 1.2"),
            SlashOutcome::SystemMessage(_)
        ));
    }

    /// Unique per surface. The same word may name a retail command and a Kuluu
    /// one -- `/emote` sends free-form text as retail does, `//emote` plays a
    /// named one -- but not two on the same surface.

    #[test]
    fn every_agent_command_on_the_chat_surface_reaches_its_handler() {
        let entities = vec![ent(42, "Mob", EntityKind::Mob, 0.0, 0.0)];
        let pos = origin();
        let cur = Some(42);

        let cases: Vec<(&str, fn(&AgentCommand) -> bool)> = vec![
            ("/follow Mob", |c| {
                matches!(c, AgentCommand::Follow { target_id: 42, .. })
            }),
            ("/attack", |c| {
                matches!(c, AgentCommand::Engage { target_id: 42 })
            }),
            ("\u{002F}\u{002F}pathto 1 2 3", |c| {
                matches!(c, AgentCommand::PathTo { .. })
            }),
            ("\u{002F}\u{002F}cancel", |c| {
                matches!(c, AgentCommand::Cancel)
            }),
            ("/bank 60 12345", |c| {
                matches!(
                    c,
                    AgentCommand::BankWhenFull {
                        threshold: 60,
                        mog_house_zoneline: 12345
                    }
                )
            }),
            ("/say hello", |c| {
                matches!(c, AgentCommand::Chat { kind: 0, .. })
            }),
            ("/party hello", |c| {
                matches!(c, AgentCommand::Chat { kind: 4, .. })
            }),
            ("/tell Bob hi", |c| matches!(c, AgentCommand::Tell { .. })),
            ("\u{002F}\u{002F}zonechange 42", |c| {
                matches!(c, AgentCommand::RequestZoneChange { line_id: 42 })
            }),
            ("\u{002F}\u{002F}snapshot", |c| {
                matches!(c, AgentCommand::Snapshot)
            }),
            ("/magic 1 42", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::CastMagic { .. },
                        ..
                    }
                )
            }),
            ("/weaponskill 1 42", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::Weaponskill { .. },
                        ..
                    }
                )
            }),
            ("/jobability 1 42", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::JobAbility { .. },
                        ..
                    }
                )
            }),
            ("/item 0 4 4112 42", |c| {
                matches!(c, AgentCommand::UseItem { .. })
            }),
        ];
        for (slash, pred) in &cases {
            let out = parse_slash_t(slash, &entities, pos, cur, None);
            match out {
                SlashOutcome::Command(ref cmd) => assert!(
                    pred(cmd),
                    "slash `{slash}` dispatched the wrong variant: {cmd:?}"
                ),
                SlashOutcome::Quit => {}
                other => panic!("slash `{slash}` did not yield Command: {other:?}"),
            }
        }
    }

    #[test]
    fn sit_on_and_off() {
        match parse_slash_t("/sit on", &empty_entities(), origin(), None, None) {
            SlashOutcome::SetSitStance(t) => assert_eq!(t, SitToggle::On),
            other => panic!("expected SetSitStance(On), got {other:?}"),
        }
        match parse_slash_t("/sit off", &empty_entities(), origin(), None, None) {
            SlashOutcome::SetSitStance(t) => assert_eq!(t, SitToggle::Off),
            other => panic!("expected SetSitStance(Off), got {other:?}"),
        }
    }

    #[test]
    fn names_are_unique_within_a_surface() {
        let mut seen = std::collections::HashMap::new();
        for (category, cmds) in COMMANDS {
            for cmd in *cmds {
                for name in cmd.names {
                    let key = (cmd.set.is_retail(), *name);
                    if let Some(prev) = seen.insert(key, *category) {
                        panic!(
                            "`{}{name}` is registered twice (in `{prev}` and `{category}`)",
                            cmd.prefix()
                        );
                    }
                }
            }
        }
    }

    /// A Retail entry must name a command this client actually has, or Kuluu
    /// has invented a name and called it vanilla. The stand-in table carries
    /// only what the suite exercises; the install-gated check is in ffxi-dat,
    /// so names the stand-in lacks are skipped.
    #[test]
    fn every_retail_name_is_a_canonical_in_the_install_table() {
        let surface = test_surface();
        for (category, cmds) in COMMANDS {
            for cmd in cmds.iter().filter(|c| c.set.is_retail()) {
                for name in cmd.names {
                    let Some(id) = surface.id_for(name) else {
                        continue;
                    };
                    assert_eq!(
                        surface.canonical(name),
                        *name,
                        "`/{name}` in `{category}` is an alias of command {id:#06x}, not its \
                         long form"
                    );
                }
            }
        }
    }
}
