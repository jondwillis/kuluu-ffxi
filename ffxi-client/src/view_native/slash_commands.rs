use ffxi_viewer_core::{MenuKind, Preset};
use ffxi_viewer_wire::{Entity as WireEntity, Vec3 as WireVec3};

use crate::state::{ActionKind, AgentCommand, CheckKind, HealMode, ReqLogoutKind};

const MAX_ZONE_ID: u16 = 600;

struct SlashCtx<'a> {
    cmd: &'a str,

    rest: &'a str,
    entities: &'a [WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
    zone_id: Option<u16>,
    self_char_id: Option<u32>,
    party: &'a [ffxi_viewer_wire::PartyMember],
    myroom: Option<ffxi_viewer_wire::MyRoom>,
}

struct Command {
    aliases: &'static [&'static str],
    usage: &'static str,
    summary: &'static str,
    handler: fn(&SlashCtx) -> SlashOutcome,
}

fn unknown_command(cmd: &str) -> SlashOutcome {
    SlashOutcome::SystemMessage(format!("unknown command: /{cmd}"))
}

const COMMANDS: &[(&str, &[Command])] = &[
    (
        "Help",
        &[Command {
            aliases: &["help", "?"],
            usage: "",
            summary: "show this slash-command reference",
            handler: |_| SlashOutcome::SystemMessage(render_help()),
        }],
    ),
    (
        "Movement & Navigation",
        &[
            Command {
                aliases: &["follow"],
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
                aliases: &["pathto"],
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
                aliases: &["pathtoforce", "pathtof"],
                usage: "<x> <y> [z] | <name> | target",
                summary: "pathfind ignoring collision (straight-lines through walls when no route — stuck-recovery)",
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
                aliases: &["warp"],
                usage: "<x> <y> [z] | <name> | target",
                summary: "debug teleport (Move): coords (z optional), fuzzy zone-line/entity, or target",
                handler: |c| {
                    parse_warp(c.rest, c.entities, c.self_pos, c.current_target, c.zone_id)
                },
            },
            Command {
                aliases: &["zones"],
                usage: "",
                summary: "list zone-line destinations from current zone",
                handler: |c| parse_zones(c.zone_id),
            },
            Command {
                aliases: &["zoneto"],
                usage: "<name|id>",
                summary: "pathfind to a zone-line (alias of `/pathto <name>`)",
                handler: |c| parse_zoneto(c.rest, c.zone_id),
            },
            Command {
                aliases: &["navmesh"],
                usage: "[on|off]",
                summary: "toggle the navmesh debug overlay",
                handler: |c| parse_navmesh(c.rest),
            },
            Command {
                aliases: &["navinfo"],
                usage: "",
                summary: "report navmesh snap status at current position",
                handler: |_| SlashOutcome::NavInfo,
            },
            Command {
                aliases: &["whereami", "pos"],
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
            Command {
                aliases: &["return", "homepoint", "hp"],
                usage: "",
                summary: "warp to home point (alive or dead)",

                handler: |_| SlashOutcome::Command(AgentCommand::ReturnToHomePoint),
            },
        ],
    ),
    (
        "Combat & Targeting",
        &[
            Command {
                aliases: &["attack", "engage"],
                usage: "[name]",
                summary: "engage target (reactor goal)",

                handler: |c| match resolve_action_target(
                    c.rest,
                    c.entities,
                    c.self_pos,
                    c.current_target,
                ) {
                    Some((id, _idx)) => {
                        SlashOutcome::Command(AgentCommand::Engage { target_id: id })
                    }
                    None => SlashOutcome::SystemMessage(format!("/{}: no target", c.cmd)),
                },
            },
            Command {
                aliases: &["disengage"],
                usage: "",
                summary: "clear active reactor goal",

                handler: |_| SlashOutcome::Command(AgentCommand::Cancel),
            },
            Command {
                aliases: &["attackoff"],
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
                aliases: &["dig"],
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
                aliases: &["assist"],
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
                aliases: &["target"],
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
                aliases: &["targetnpc", "targetnpc2"],
                usage: "",
                summary: "cycle nearest NPC/mob/pet (targetnpc2 cycles in reverse)",

                handler: |c| {
                    let reverse = c.cmd == "targetnpc2";
                    let kinds = [
                        ffxi_viewer_wire::EntityKind::Npc,
                        ffxi_viewer_wire::EntityKind::Mob,
                        ffxi_viewer_wire::EntityKind::Pet,
                    ];
                    match cycle_kind_filtered(
                        c.entities,
                        c.self_pos,
                        c.current_target,
                        &kinds,
                        reverse,
                    ) {
                        Some(id) => SlashOutcome::SetTarget(Some(id)),
                        None => SlashOutcome::SystemMessage(format!("/{}: no NPC nearby", c.cmd)),
                    }
                },
            },
            Command {
                aliases: &["targetenemy"],
                usage: "",
                summary: "cycle nearest enemy (mobs only)",

                handler: |c| {
                    let kinds = [ffxi_viewer_wire::EntityKind::Mob];
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
                aliases: &["targetnpcparty"],
                usage: "",
                summary: "cycle owned party NPCs (trusts/fellows/pets)",

                handler: |c| {
                    let owner = c.self_char_id.unwrap_or(0);
                    let owned: Vec<&WireEntity> = c
                        .entities
                        .iter()
                        .filter(|e| {
                            matches!(e.kind, ffxi_viewer_wire::EntityKind::Pet)
                                && e.claim_id == owner
                        })
                        .collect();
                    let ids: Vec<u32> = {
                        let mut sorted = owned.clone();
                        sorted.sort_by(|a, b| {
                            let da = sq_dist(a.pos, c.self_pos);
                            let db = sq_dist(b.pos, c.self_pos);
                            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        sorted.into_iter().map(|e| e.id).collect()
                    };
                    if ids.is_empty() {
                        SlashOutcome::SystemMessage("/targetnpcparty: no party NPCs".into())
                    } else {
                        let next = match c
                            .current_target
                            .and_then(|id| ids.iter().position(|x| *x == id))
                        {
                            Some(i) => ids[(i + 1) % ids.len()],
                            None => ids[0],
                        };
                        SlashOutcome::SetTarget(Some(next))
                    }
                },
            },
            Command {
                aliases: &[
                    "targetparty1",
                    "targetparty2",
                    "targetparty3",
                    "targetparty4",
                    "targetparty5",
                    "targetparty6",
                ],
                usage: "",
                summary: "target party slot 1-6 (slot 1 = self)",

                handler: |c| {
                    let slot = c
                        .cmd
                        .strip_prefix("targetparty")
                        .and_then(|s| s.parse::<u8>().ok())
                        .unwrap_or(0);
                    match resolve_party_slot(slot, c.self_char_id, c.party) {
                        Some(id) => SlashOutcome::SetTarget(Some(id)),
                        None => SlashOutcome::SystemMessage(format!("/{}: slot empty", c.cmd)),
                    }
                },
            },
            Command {
                aliases: &["debug", "dbg", "nearby", "entities"],
                usage: "[name|id|heights]",
                summary: "dump current target + nearby entities (or one entity in detail)",
                handler: |c| parse_debug(c.rest, c.entities, c.self_pos, c.current_target),
            },
            Command {
                aliases: &["check", "checkname", "checkparam"],
                usage: "[name]",
                summary: "check target — strength / name / parameters",
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
                aliases: &["cast"],
                usage: "<spell> [target]",
                summary: "cast a spell",
                handler: |c| parse_cast(c.rest, c.entities, c.self_pos, c.current_target),
            },
            Command {
                aliases: &["ws", "weaponskill"],
                usage: "<name> [target]",
                summary: "weapon skill",
                handler: |c| parse_weaponskill(c.rest, c.entities, c.self_pos, c.current_target),
            },
            Command {
                aliases: &["ja", "jobability"],
                usage: "<name> [target]",
                summary: "job ability",
                handler: |c| parse_job_ability(c.rest, c.entities, c.self_pos, c.current_target),
            },
            Command {
                aliases: &["ra", "shoot", "rangedattack"],
                usage: "[target]",
                summary: "ranged attack",
                handler: |c| parse_ranged_attack(c.rest, c.entities, c.self_pos, c.current_target),
            },
            Command {
                aliases: &["useitem", "use"],
                usage: "<name> [target]",
                summary: "use an item",
                handler: |c| parse_use_item(c.rest, c.entities, c.self_pos, c.current_target),
            },
            Command {
                aliases: &["magic"],
                usage: "",
                summary: "open the Magic menu (no-arg form; with args use /ma)",

                handler: |c| {
                    if c.rest.is_empty() {
                        SlashOutcome::OpenMenu(MenuKind::Magic)
                    } else {
                        unknown_command(c.cmd)
                    }
                },
            },
            Command {
                aliases: &["abilities", "abi"],
                usage: "",
                summary: "open the Abilities menu (no-arg form; with args use /ja)",
                handler: |c| {
                    if c.rest.is_empty() {
                        SlashOutcome::OpenMenu(MenuKind::Abilities)
                    } else {
                        unknown_command(c.cmd)
                    }
                },
            },
            Command {
                aliases: &["items"],
                usage: "",
                summary: "open the Items menu (no-arg form; with args use /useitem)",
                handler: |c| {
                    if c.rest.is_empty() {
                        SlashOutcome::OpenMenu(MenuKind::Items)
                    } else {
                        unknown_command(c.cmd)
                    }
                },
            },
            Command {
                aliases: &["equipment", "equip"],
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
                aliases: &["cancel"],
                usage: "",
                summary: "cancel current reactor goal / action",
                handler: |_| SlashOutcome::Command(AgentCommand::Cancel),
            },
            Command {
                aliases: &["raw"],
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
                aliases: &["s", "say"],
                usage: "<text>",
                summary: "say (local chat)",
                handler: |c| chat_or_empty(c.rest, 0, "/s"),
            },
            Command {
                aliases: &["p", "party"],
                usage: "<text>",
                summary: "party chat",
                handler: |c| chat_or_empty(c.rest, 4, "/p"),
            },
            Command {
                aliases: &["sh", "shout"],
                usage: "<text>",
                summary: "shout chat",
                handler: |c| chat_or_empty(c.rest, 1, "/sh"),
            },
            Command {
                aliases: &["l", "ls", "linkshell"],
                usage: "<text>",
                summary: "linkshell chat",
                handler: |c| chat_or_empty(c.rest, 5, "/l"),
            },
            Command {
                aliases: &["t", "tell"],
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
                aliases: &["emote"],
                usage: "<name> [motion|text]",
                summary: "canned emote by name — every table emote also works directly (/wave, /bow, /dance1…)",
                handler: |c| parse_named_emote_args(c.rest, c),
            },
            Command {
                aliases: &["em"],
                usage: "<text>",
                summary: "free-form custom emote (zone chat channel 8)",
                handler: |c| chat_or_empty(c.rest, ffxi_proto::map::chat_kind::EMOTION, "/em"),
            },
            Command {
                aliases: &["jobemote"],
                usage: "[war|mnk|…] [motion|text]",
                summary: "job gesture (defaults to current main job; needs its JOB_GESTURE key item)",
                handler: |c| parse_jobemote(c.rest, c),
            },
            Command {
                aliases: &["bell"],
                usage: "<c4..c6|6..30> [motion|text]",
                summary: "ring an equipped bell at a note (two octaves from c4)",
                handler: |c| parse_bell(c.rest, c),
            },
            Command {
                aliases: &["emotelist"],
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
                aliases: &["sit"],
                usage: "[on|off]",
                summary: "sit (locks movement; any movement key stands)",

                handler: |c| parse_sit(c.rest),
            },
            Command {
                aliases: &["stand"],
                usage: "",
                summary: "stand (clear any rest stance)",
                handler: |_| SlashOutcome::SetSitStance(SitToggle::Off),
            },
            Command {
                aliases: &["heal"],
                usage: "[on|off]",
                summary: "toggle resting (CAMP)",

                handler: |c| parse_heal(c.rest),
            },
            Command {
                aliases: &["raisemenu"],
                usage: "<option>",
                summary: "respond to raise dialog",
                handler: |c| parse_raise_menu(c.rest),
            },
            Command {
                aliases: &["tractormenu"],
                usage: "<option>",
                summary: "respond to tractor dialog",
                handler: |c| parse_tractor_menu(c.rest),
            },
            Command {
                aliases: &["homepointmenu"],
                usage: "<option>",
                summary: "respond to homepoint dialog",
                handler: |c| parse_homepoint_menu(c.rest),
            },
            Command {
                aliases: &["jobchange", "jc"],
                usage: "[<main> [sub]]",
                summary: "no args: open the Mog Menu; with jobs: change main/sub (names, WAR/MNK codes, or ids)",
                handler: |c| parse_jobchange(c.rest, c.myroom.is_some()),
            },
            Command {
                aliases: &["endevent", "endevt", "clearevent", "clearevt"],
                usage: "",
                summary: "flush pending NPC events (unblock /logout)",

                handler: |_| SlashOutcome::Command(AgentCommand::EndEvent),
            },
            Command {
                aliases: &["endcutscene", "endcs", "skipcutscene", "skipcs"],
                usage: "[csid]",
                summary: "end a forced cutscene (new-char intro, etc.)",

                handler: |c| parse_endcutscene(c.rest),
            },
            Command {
                aliases: &["release", "unwedge"],
                usage: "",
                summary: "server-side !release: forcibly end any pinned event (gmlevel>=1)",

                handler: |_| {
                    SlashOutcome::Command(AgentCommand::Chat {
                        kind: 0,
                        text: "!release".into(),
                    })
                },
            },
            Command {
                aliases: &["buy"],
                usage: "<row> [qty]",
                summary: "buy from open shop by row index",
                handler: |c| parse_buy(c.rest),
            },
            Command {
                aliases: &["sell"],
                usage: "<slot> [qty] | confirm",
                summary: "sell to open shop from inventory slot",
                handler: |c| parse_sell(c.rest),
            },
            Command {
                aliases: &["bank"],
                usage: "<subcommand>",
                summary: "gil-bank operations",
                handler: |c| parse_bank(c.rest),
            },
            Command {
                aliases: &["minimap", "mm"],
                usage: "[show|hide|toggle|mode <top|retail|auto>|cull <N>|zoom ...]",
                summary: "drive the minimap HUD (visibility, backend, cull, zoom)",
                handler: |c| parse_minimap(c.rest),
            },
            Command {
                aliases: &["map", "m"],
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
            // Not a retail command — a dev/diagnostic convenience (echoes the raw
            // 0x0F4 tracking list to chat for copying). Debug builds only; the
            // retail-faithful path is the Map screen's Wide Scan submenu.
            #[cfg(debug_assertions)]
            Command {
                // /ws is the weaponskill command; widescan takes /wscan to avoid
                // shadowing it.
                aliases: &["widescan", "wscan"],
                usage: "",
                summary: "(dev) request the server wide-scan tracking list and echo it to chat",
                handler: |_| SlashOutcome::Widescan,
            },
            Command {
                aliases: &["clock"],
                usage: "[show|hide|toggle]",
                summary: "toggle the Vana'diel clock widget (same state as the Current Time menu entry)",
                handler: |c| parse_clock(c.rest),
            },
            Command {
                aliases: &["sound", "audio", "mute"],
                usage: "[on|off|toggle|status] [bgm|sfx]",
                summary: "bare /sound toggles all audio; survives logout",
                handler: |c| parse_sound(c.rest),
            },
        ],
    ),
    (
        "Fishing",
        &[
            Command {
                aliases: &["fish"],
                usage: "",
                summary: "cast a line (drives the fishing mini-game)",
                handler: |_| SlashOutcome::Command(AgentCommand::Fish),
            },
            Command {
                aliases: &["hook"],
                usage: "",
                summary: "set the hook once a fish bites",
                handler: |_| SlashOutcome::Command(AgentCommand::FishingInput {
                    input: crate::state::FishingInput::Hook,
                }),
            },
            Command {
                aliases: &["reelleft", "rl"],
                usage: "",
                summary: "react to a left fishing arrow",
                handler: |_| SlashOutcome::Command(AgentCommand::FishingInput {
                    input: crate::state::FishingInput::Left,
                }),
            },
            Command {
                aliases: &["reelright", "rr"],
                usage: "",
                summary: "react to a right fishing arrow",
                handler: |_| SlashOutcome::Command(AgentCommand::FishingInput {
                    input: crate::state::FishingInput::Right,
                }),
            },
            Command {
                aliases: &["reelstop", "fishcancel"],
                usage: "",
                summary: "abandon the cast / mini-game",
                handler: |_| SlashOutcome::Command(AgentCommand::FishingInput {
                    input: crate::state::FishingInput::Cancel,
                }),
            },
        ],
    ),
    (
        "Session",
        &[
            Command {
                aliases: &["logout"],
                usage: "[on|off]",
                summary: "request logout (30s LeaveGame timer)",

                handler: |c| parse_reqlogout(c.rest,  false),
            },
            Command {
                aliases: &["shutdown"],
                usage: "[on|off]",
                summary: "request shutdown (LeaveGame, then close)",
                handler: |c| parse_reqlogout(c.rest,  true),
            },
            Command {
                aliases: &["exit"],
                usage: "",
                summary: "polite logout + close window",

                handler: |_| SlashOutcome::QuitWithLogout(ReqLogoutKind::LogoutOn),
            },
            Command {
                aliases: &["disconnect", "quit"],
                usage: "",
                summary: "drop the connection immediately",

                handler: |_| SlashOutcome::Quit,
            },
        ],
    ),
    (
        "Debug & Tooling",
        &[
            Command {
                aliases: &["snapshot"],
                usage: "",
                summary: "emit a one-shot scene snapshot",
                handler: |_| SlashOutcome::Command(AgentCommand::Snapshot),
            },
            Command {
                aliases: &["zonechange", "rzc"],
                usage: "<id>",
                summary: "request zone change (debug)",
                handler: |c| parse_zone_change(c.rest),
            },
            Command {
                aliases: &["mhexit"],
                usage: "[home|1f|2f|garden|<region> [slot]]",
                summary: "leave the current Mog House (sends 0x05E zmrq; the Exit door offers the same Where to? menu)",
                handler: |c| parse_mhexit(c.rest, c.zone_id),
            },
            Command {
                aliases: &["agent"],
                usage: "<pause|resume|status>",
                summary: "human-in-control flag for agent commands",
                handler: |c| parse_agent(c.rest),
            },
            Command {
                aliases: &["keybinds", "keybind", "binds"],
                usage: "<preset|list|reset>",
                summary: "manage keybind presets",
                handler: |c| parse_keybinds(c.rest),
            },
            Command {
                aliases: &["load_mmb", "loadmmb"],
                usage: "<file_id> <chunk_idx>",
                summary: "spawn MMB model at self_pos (debug overlay)",
                handler: |c| parse_load_mmb(c.rest, c.self_pos),
            },
            Command {
                aliases: &["load_mmb_on", "loadmmbon"],
                usage: "<entity_id> <file_id> <chunk_idx>",
                summary: "attach MMB model under a tracked entity (debug)",
                handler: |c| parse_load_mmb_on(c.rest),
            },
            Command {
                aliases: &["load_mzb", "loadmzb"],
                usage: "<file_id> [chunk_idx]",
                summary: "load MZB mesh-library at self_pos (debug overlay)",
                handler: |c| parse_load_mzb(c.rest, c.self_pos),
            },
            Command {
                aliases: &["fps"],
                usage: "<max>",
                summary: "set target frame rate",
                handler: |c| parse_fps(c.rest),
            },
            Command {
                aliases: &["capture"],
                usage: "[on|off|toggle]",
                summary: "screen-capture-friendly mode (disables framepace; avoids QuickTime lockup)",
                handler: |c| parse_capture(c.rest),
            },
            Command {
                aliases: &["screenshot", "ss"],
                usage: "[path.png]",
                summary: "capture primary window to PNG (default: screenshot-N.png in CWD)",
                handler: |c| parse_screenshot(c.rest),
            },
            Command {
                aliases: &["drawdistance", "dd"],
                usage: "[setworld|setmob] [N]",
                summary: "set draw distance",
                handler: |c| parse_drawdistance(c.rest),
            },
            Command {
                aliases: &["copy"],
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
                aliases: &["bgm"],
                usage: "<track_id>",
                summary: "audition a BGM track id (synthetic 0x05F slot 0)",
                handler: |c| match c.rest.parse::<u16>() {
                    Ok(id) => SlashOutcome::PlayBgm { track_id: id },
                    Err(_) => SlashOutcome::SystemMessage("/bgm <track_id>".into()),
                },
            },
            Command {
                aliases: &["sfx"],
                usage: "<se_id>",
                summary: "fire a one-shot SE by numeric id",
                handler: |c| match c.rest.parse::<u32>() {
                    Ok(id) => SlashOutcome::PlaySfx { se_id: id },
                    Err(_) => SlashOutcome::SystemMessage("/sfx <se_id>".into()),
                },
            },
            Command {
                aliases: &["look"],
                usage: "[name|act_index]",
                summary: "print decoded LookData (race/gear) for an entity",
                handler: |c| parse_look(c.rest, c.entities, c.self_pos, c.current_target),
            },
            Command {
                aliases: &["zonegeom"],
                usage: "[off|collision|all|toggle]",
                summary: "MZB overlay visibility (collision-only vs decorative)",
                handler: |c| parse_zonegeom(c.rest),
            },
            Command {
                aliases: &["zoneline", "zonelines"],
                usage: "[off|pillar|gate|toggle]",
                summary: "zone-line trigger markers — off (retail-faithful, default), pillar (debug column), or gate (real oriented footprint)",
                handler: |c| parse_zoneline(c.rest),
            },
            Command {
                aliases: &["weather"],
                usage: "<id|name>",
                summary: "client-side weather override (e.g. `rain`, `none`, `12`); lasts until the next server WEATHER packet",

                handler: |c| parse_weather(c.rest),
            },
            Command {
                aliases: &["devhud"],
                usage: "[on|off|toggle]",
                summary: "stage + diagnostics bars (top/bottom telemetry) + [dbg] chat lines. Per-panel overlays (perf, target cycle, mesh, netstat) live in the in-game Debug menu",
                handler: |c| parse_devhud(c.rest),
            },
            Command {
                aliases: &["netstat", "network"],
                usage: "[on|off|toggle]",
                summary: "network health indicator (S/R baud, connection %, send/recv arrows)",
                handler: |c| parse_netstat(c.rest),
            },
            Command {
                aliases: &["renderscale", "rscale"],
                usage: "[25-200 | 0.25-2.0]",
                summary: "3D render scale: <100% renders the world at lower res and upscales (perf); >100% supersamples. HUD stays native. Bare `/renderscale` reports it.",
                handler: |c| parse_renderscale(c.rest),
            },
            Command {
                aliases: &["sky"],
                usage: "[vanilla|enhanced|toggle]",
                summary: "switch the sky mode — vanilla (retail-faithful) or enhanced (atmosphere + bloom glare); bare `/sky` shows the current mode",
                handler: |c| parse_sky(c.rest),
            },
            Command {
                aliases: &["lights", "lanterns"],
                usage: "[on|off | threshold N | intensity N | range N | flicker on|off]",
                summary: "tune dynamic lantern/fire lights (from over-bright vertices); bare `/lights` lists state",
                handler: |c| parse_lights(c.rest),
            },
        ],
    ),
];

fn render_help() -> String {
    let mut out = String::from("=== Slash command reference ===");
    for (category, entries) in COMMANDS {
        out.push_str("\n[");
        out.push_str(category);
        out.push(']');
        for entry in *entries {
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

#[derive(Debug, Clone)]
pub enum SlashOutcome {
    Command(AgentCommand),

    /// A command that still goes out, prefaced by an advisory chat line
    /// (warn-don't-block, e.g. /jobchange outside a Mog House).
    CommandWithNotice {
        cmd: AgentCommand,
        notice: String,
    },

    Commands(Vec<AgentCommand>),

    SetTarget(Option<u32>),

    Quit,

    QuitWithLogout(ReqLogoutKind),

    SystemMessage(String),

    ShopBuyRow {
        shop_index: u8,
        qty: u32,
    },

    ShopSellSlot {
        inv_slot: u8,
        qty: u32,
    },

    ShopSellConfirm,

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

    SetDrawDistance(DrawDistanceOp),

    SetZoneGeom(Option<ffxi_viewer_core::dat_mzb::ZoneGeomMode>),

    SetCameraCollisionSource(Option<ffxi_viewer_core::dat_mzb::CameraCollisionSource>),

    SetDevHud(Option<bool>),

    SetNetStatus(Option<bool>),

    SetVanaClock(Option<bool>),

    /// `Some(scale)` sets the 3D render scale (0.25–2.0); `None` reports it.
    SetRenderScale(Option<f32>),

    SetSky(SkyOp),

    SetZoneLines(ZoneLineOp),

    SetLights(LightsOp),

    SetMinimap(MinimapOp),

    SetSound(SoundOp),

    SetTargetFps(Option<u32>),

    SetCaptureMode(Option<bool>),

    DebugHeights,

    Screenshot {
        path: Option<String>,
    },

    EndCutscene {
        event_num: Option<u16>,
    },

    SetWeatherClient(ffxi_viewer_wire::Weather),

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundOp {
    Status,

    SetBoth(Option<bool>),

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

pub fn parse_slash(
    buffer: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
    zone_id: Option<u16>,
    self_char_id: Option<u32>,
    party: &[ffxi_viewer_wire::PartyMember],
    myroom: Option<ffxi_viewer_wire::MyRoom>,
) -> SlashOutcome {
    let trimmed = buffer.trim_start();
    let body = trimmed.strip_prefix('/').unwrap_or(trimmed);

    let mut parts = body.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("").to_ascii_lowercase();
    let rest = parts.next().unwrap_or("").trim();

    if cmd.is_empty() {
        return SlashOutcome::SystemMessage("empty command".into());
    }

    let ctx = SlashCtx {
        cmd: &cmd,
        rest,
        entities,
        self_pos,
        current_target,
        zone_id,
        self_char_id,
        party,
        myroom,
    };

    match COMMANDS
        .iter()
        .flat_map(|(_, cmds)| cmds.iter())
        .find(|c| c.aliases.contains(&cmd.as_str()))
    {
        Some(command) => (command.handler)(&ctx),
        // Every scraped emote name is its own command (/wave, /bow, …); the
        // table lookup runs after the explicit commands so it can never shadow
        // one.
        None => match parse_canned_emote(&cmd, rest, &ctx) {
            Some(outcome) => outcome,
            None => unknown_command(&cmd),
        },
    }
}

/// Resolve the emote target: the currently selected entity, or untargeted.
fn emote_target(ctx: &SlashCtx) -> (Option<u32>, Option<u16>) {
    let ent = ctx
        .current_target
        .and_then(|id| ctx.entities.iter().find(|e| e.id == id));
    (ent.map(|e| e.id), ent.map(|e| e.act_index))
}

/// `[motion|text]` trailing argument → EmoteMode (default All; XiPackets
/// client 0x005D: 'motion' → 2, 'text' → 1).
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
    let id = ffxi_proto::emote_names::id_for_command(cmd)?;
    if emote::HELM_ONLY.contains(&id) || id == emote::BELL || id == emote::JOB {
        // HELM ids are server-initiated; Bell/Job have their own commands.
        return None;
    }
    Some(emote_outcome(ctx, id, rest, 0))
}

fn parse_named_emote_args(rest: &str, ctx: &SlashCtx) -> SlashOutcome {
    let mut parts = rest.split_whitespace();
    let Some(name) = parts.next() else {
        return SlashOutcome::SystemMessage("/emote: usage `/emote <name> [motion|text]`".into());
    };
    match parse_canned_emote(name, parts.next().unwrap_or(""), ctx) {
        Some(outcome) => outcome,
        None => SlashOutcome::SystemMessage(format!("/emote: unknown emote `{name}`")),
    }
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
            "/jobemote: unknown job `{job_arg}` (use WAR/MNK/… or omit for main job)"
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
        return ffxi_proto::job_names::lookup(id).map(|_| id);
    }
    (1..=u8::MAX as u16).find(|&id| {
        ffxi_proto::job_names::abbrev(id).is_some_and(|a| a.eq_ignore_ascii_case(arg))
            || ffxi_proto::job_names::lookup(id).is_some_and(|n| n.eq_ignore_ascii_case(arg))
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

pub use ffxi_viewer_core::snapshot::system_chat_line;

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

fn parse_sell(rest: &str) -> SlashOutcome {
    let mut parts = rest.split_whitespace();
    let slot_str = parts.next().unwrap_or("");
    if slot_str.is_empty() {
        return SlashOutcome::SystemMessage(
            "/sell: usage `/sell <slot> [qty]` or `/sell confirm`".into(),
        );
    }
    if slot_str.eq_ignore_ascii_case("confirm") {
        return SlashOutcome::ShopSellConfirm;
    }
    let inv_slot: u8 = match slot_str.parse() {
        Ok(n) => n,
        Err(_) => return SlashOutcome::SystemMessage(format!("/sell: bad slot `{slot_str}`")),
    };
    let qty: u32 = match parts.next() {
        Some(q) => match q.parse() {
            Ok(n) if n >= 1 => n,
            _ => return SlashOutcome::SystemMessage(format!("/sell: bad qty `{q}`")),
        },
        None => 1,
    };
    SlashOutcome::ShopSellSlot { inv_slot, qty }
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

/// `/ra [target]` — ranged attack (c2s action 0x10). Takes no id, only a target
/// (defaults to the current target).
fn parse_ranged_attack(
    rest: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
) -> SlashOutcome {
    let parts: Vec<&str> = rest.split_ascii_whitespace().collect();
    let (target_id, target_index) =
        match resolve_target_args(&parts, entities, self_pos, current_target) {
            Ok(pair) => pair,
            Err(msg) => return SlashOutcome::SystemMessage(format!("/ra: {msg}")),
        };
    SlashOutcome::Command(AgentCommand::Action {
        target_id,
        target_index,
        kind: ActionKind::Shoot,
    })
}

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
            return SlashOutcome::SystemMessage(format!("/ja: bad ability_id `{}`", parts[0]));
        }
    };

    let (target_id, target_index) =
        resolve_target_args(&parts[1..], entities, self_pos, current_target).unwrap_or_default();
    SlashOutcome::Command(AgentCommand::Action {
        target_id,
        target_index,
        kind: ActionKind::JobAbility { ability_id },
    })
}

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
            return SlashOutcome::SystemMessage(format!("/useitem: bad container `{}`", parts[0]));
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
        resolve_target_args(tail, entities, self_pos, current_target).unwrap_or_default();
    SlashOutcome::Command(AgentCommand::UseItem {
        container,
        slot,
        item_no,
        target_id,
        target_index,
    })
}

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

fn parse_mhexit(rest: &str, zone_id: Option<u16>) -> SlashOutcome {
    let trimmed = rest.trim().to_ascii_lowercase();
    let mut parts = trimmed.split_whitespace();
    let first = parts.next();
    let slot: u8 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);

    let kind = match first {
        None | Some("home") | Some("") => crate::state::MogHouseExit::Home {
            exit_bit: zone_id.and_then(home_region_bit_for_zone).unwrap_or(0),
        },
        Some("1f") | Some("mog1f") => crate::state::MogHouseExit::Mog1F,
        Some("2f") | Some("mog2f") => crate::state::MogHouseExit::Mog2F,
        Some("garden") | Some("moggarden") => crate::state::MogHouseExit::MogGarden,
        Some("sandoria") | Some("sandy") => crate::state::MogHouseExit::Sandoria { slot },
        Some("bastok") => crate::state::MogHouseExit::Bastok { slot },
        Some("windurst") | Some("windy") => crate::state::MogHouseExit::Windurst { slot },
        Some("jeuno") => crate::state::MogHouseExit::Jeuno { slot },
        Some("whitegate") | Some("aht_urhgan") => crate::state::MogHouseExit::Whitegate { slot },
        Some("adoulin") => crate::state::MogHouseExit::Adoulin { slot },
        Some("auto") => match zone_id.and_then(home_region_bit_for_zone) {
            Some(bit) => crate::state::MogHouseExit::from_bit_slot(bit, 1),
            None => {
                return SlashOutcome::SystemMessage(format!(
                    "/mhexit auto: zone {} isn't in the home-region table — \
                         use `/mhexit home` or pass a region name explicitly",
                    zone_id.map_or("unknown".into(), |z| z.to_string()),
                ));
            }
        },
        Some(other) => {
            return SlashOutcome::SystemMessage(format!(
                "/mhexit: unknown form `{other}` — try home|1f|2f|garden|\
                 sandoria|bastok|windurst|jeuno|whitegate|adoulin|auto"
            ));
        }
    };
    SlashOutcome::Command(AgentCommand::MogHouseExit { kind })
}

// Job tokens accept the LSB-scraped full name ("Warrior"), the canonical
// three-letter code ("WAR"), or the numeric JOBTYPE id — all case-insensitive.
fn parse_job_token(token: &str) -> Option<u8> {
    if let Ok(id) = token.parse::<u16>() {
        return (id > 0 && ffxi_proto::job_names::lookup(id).is_some()).then_some(id as u8);
    }
    ffxi_proto::job_names::JOB_ABBREVS
        .iter()
        .chain(ffxi_proto::job_names::JOB_NAMES.iter())
        .find(|(id, name)| *id > 0 && name.eq_ignore_ascii_case(token))
        .map(|(id, _)| *id as u8)
}

fn parse_jobchange(rest: &str, in_mog_house: bool) -> SlashOutcome {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return SlashOutcome::Command(AgentCommand::OpenMogMenu);
    }

    let mut parts = trimmed.split_whitespace();
    let main_token = parts.next().unwrap_or("");
    let sub_token = parts.next();
    if parts.next().is_some() {
        return SlashOutcome::SystemMessage(
            "/jobchange: usage `/jobchange [<main> [sub]]` (e.g. `/jc war mnk`)".into(),
        );
    }

    let Some(main_job) = parse_job_token(main_token) else {
        return SlashOutcome::SystemMessage(format!(
            "/jobchange: unknown job `{main_token}` (use a name or code like WAR, MNK, WHM)"
        ));
    };
    let sub_job = match sub_token {
        Some(token) => match parse_job_token(token) {
            Some(job) => Some(job),
            None => {
                return SlashOutcome::SystemMessage(format!(
                    "/jobchange: unknown support job `{token}` (use a name or code like WAR, MNK, WHM)"
                ));
            }
        },
        None => None,
    };

    let cmd = AgentCommand::ChangeJob {
        main_job: Some(main_job),
        sub_job,
    };
    if in_mog_house {
        SlashOutcome::Command(cmd)
    } else {
        SlashOutcome::CommandWithNotice {
            cmd,
            notice:
                "/jobchange: you don't appear to be in a Mog House — the server may reject this"
                    .into(),
        }
    }
}

fn home_region_bit_for_zone(zone_id: u16) -> Option<u8> {
    match zone_id {
        230..=233 => Some(1),
        234..=237 => Some(2),
        238..=242 => Some(3),
        243..=246 => Some(4),
        48 | 50 => Some(5),
        256 | 257 => Some(9),
        _ => None,
    }
}

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
            force: false,
        }),
        None => SlashOutcome::SystemMessage(format!(
            "/zoneto: no zone-line matches `{needle}` (try `/zones` to list)"
        )),
    }
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
    s.push('\n');
    s.push_str(&format!(
        "  anim={} animsub={} status={}{}",
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

fn parse_sound(rest: &str) -> SlashOutcome {
    let tokens: Vec<String> = rest
        .split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect();
    if tokens.is_empty() {
        return SlashOutcome::SetSound(SoundOp::SetBoth(None));
    }

    let mut verb: Option<Option<bool>> = None;
    let mut category: Option<&str> = None;
    for tok in &tokens {
        match tok.as_str() {
            "on" | "unmute" | "true" | "1" => {
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

    let verb = verb.unwrap_or_default();
    let op = match category {
        Some("bgm") => SoundOp::SetBgm(verb),
        Some("sfx") => SoundOp::SetSfx(verb),
        _ => SoundOp::SetBoth(verb),
    };
    SlashOutcome::SetSound(op)
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkyOp {
    Status,
    Set(ffxi_viewer_core::SkyStyle),
    Toggle,
}

fn parse_sky(rest: &str) -> SlashOutcome {
    use ffxi_viewer_core::SkyStyle;
    let op = match rest.trim().to_ascii_lowercase().as_str() {
        "" => SkyOp::Status,
        "toggle" => SkyOp::Toggle,
        "enhanced" | "physical" => SkyOp::Set(SkyStyle::Enhanced),
        "vanilla" | "retail" => SkyOp::Set(SkyStyle::Vanilla),
        other => {
            return SlashOutcome::SystemMessage(format!(
                "/sky: unknown mode `{other}` (use vanilla | enhanced | toggle)"
            ));
        }
    };
    SlashOutcome::SetSky(op)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneLineOp {
    Status,
    Set(ffxi_viewer_core::ZoneLineDisplay),
    Toggle,
}

fn parse_zoneline(rest: &str) -> SlashOutcome {
    use ffxi_viewer_core::ZoneLineDisplay;
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

    Threshold(f32),

    Intensity(f32),

    Range(f32),

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
    let num = |a: &str| a.parse::<f32>().ok().filter(|v| v.is_finite() && *v >= 0.0);

    match verb.as_str() {
        "on" | "off" | "toggle" => match toggle(&verb) {
            Ok(v) => SlashOutcome::SetLights(LightsOp::Enable(v)),
            Err(()) => unreachable!(),
        },
        "threshold" | "thresh" => match num(arg) {
            Some(v) => SlashOutcome::SetLights(LightsOp::Threshold(v)),
            None => SlashOutcome::SystemMessage(format!("/lights threshold: bad value `{arg}`")),
        },
        "intensity" | "int" => match num(arg) {
            Some(v) => SlashOutcome::SetLights(LightsOp::Intensity(v)),
            None => SlashOutcome::SystemMessage(format!("/lights intensity: bad value `{arg}`")),
        },
        "range" => match num(arg) {
            Some(v) => SlashOutcome::SetLights(LightsOp::Range(v)),
            None => SlashOutcome::SystemMessage(format!("/lights range: bad value `{arg}`")),
        },
        "flicker" => match toggle(arg) {
            Ok(v) => SlashOutcome::SetLights(LightsOp::Flicker(v)),
            Err(()) => SlashOutcome::SystemMessage(format!("/lights flicker: bad value `{arg}`")),
        },
        other => SlashOutcome::SystemMessage(format!(
            "/lights: unknown `{other}` (use on|off|threshold N|intensity N|range N|flicker on|off)"
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
                    "/renderscale: {:.0}% out of range (25–200%)",
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
    use ffxi_viewer_core::dat_mzb::{CameraCollisionSource, ZoneGeomMode};
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
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            status: 0,
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
            None,
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
            &entities,
            origin(),
            Some(1),
            None,
            Some(42),
            &[],
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
    fn targetnpc2_cycles_reverse() {
        let entities = vec![
            ent(1, "Goblin A", EntityKind::Mob, 3.0, 0.0),
            ent(2, "Vendor", EntityKind::Npc, 5.0, 0.0),
            ent(3, "Goblin B", EntityKind::Mob, 7.0, 0.0),
        ];

        assert!(matches!(
            parse_slash_t("/targetnpc2", &entities, origin(), Some(2), None),
            SlashOutcome::SetTarget(Some(1))
        ));

        assert!(matches!(
            parse_slash_t("/targetnpc2", &entities, origin(), Some(1), None),
            SlashOutcome::SetTarget(Some(3))
        ));
    }

    #[test]
    fn targetenemy_skips_npcs() {
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

        assert!(matches!(
            parse_slash(
                "/targetparty1",
                &[],
                origin(),
                None,
                None,
                Some(100),
                &party,
                None
            ),
            SlashOutcome::SetTarget(Some(100))
        ));

        assert!(matches!(
            parse_slash(
                "/targetparty3",
                &[],
                origin(),
                None,
                None,
                Some(100),
                &party,
                None
            ),
            SlashOutcome::SetTarget(Some(300))
        ));

        assert!(matches!(
            parse_slash(
                "/targetparty5",
                &[],
                origin(),
                None,
                None,
                Some(100),
                &party,
                None
            ),
            SlashOutcome::SystemMessage(_)
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
        for slash in ["/map", "/m"] {
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
        for slash in ["/widescan", "/wscan"] {
            assert!(
                matches!(
                    parse_slash_t(slash, &empty_entities(), origin(), None, None),
                    SlashOutcome::Widescan
                ),
                "{slash} should fire a wide-scan request"
            );
        }
    }

    #[test]
    fn ws_alias_stays_weaponskill() {
        assert!(
            !matches!(
                parse_slash_t("/ws Fast Blade", &empty_entities(), origin(), None, None),
                SlashOutcome::Widescan
            ),
            "/ws must remain the weaponskill command, not widescan"
        );
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
        let out = parse_slash_t("/debug", &entities, origin(), Some(202), None);
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
        let out = parse_slash_t("/debug Goblin", &entities, origin(), None, None);
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
        for s in ["/quit", "/disconnect"] {
            assert!(matches!(
                parse_slash_t(s, &empty_entities(), origin(), None, None),
                SlashOutcome::Quit
            ));
        }
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
    fn exit_emits_quit_with_logout_on() {
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

        // /kneel is the canned emote (id 3) — no longer a /sit alias.
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
    fn sit_on_off_and_stand() {
        match parse_slash_t("/sit on", &empty_entities(), origin(), None, None) {
            SlashOutcome::SetSitStance(t) => assert_eq!(t, SitToggle::On),
            other => panic!("expected SetSitStance(On), got {other:?}"),
        }
        match parse_slash_t("/sit off", &empty_entities(), origin(), None, None) {
            SlashOutcome::SetSitStance(t) => assert_eq!(t, SitToggle::Off),
            other => panic!("expected SetSitStance(Off), got {other:?}"),
        }

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

    #[test]
    fn load_mmb_alias_and_bad_args() {
        assert!(matches!(
            parse_slash_t("/loadmmb 115 18", &empty_entities(), origin(), None, None),
            SlashOutcome::LoadMmb {
                file_id: 115,
                chunk_idx: 18,
                entity_id: None,
                ..
            }
        ));

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

    #[test]
    fn load_mzb_parses_optional_chunk_idx() {
        let pos = WireVec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        };

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

        match parse_slash_t("/load_mzb 7368 2", &empty_entities(), pos, None, None) {
            SlashOutcome::LoadMzb {
                chunk_idx: Some(2), ..
            } => {}
            other => panic!("expected LoadMzb chunk_idx=Some(2), got {other:?}"),
        }

        assert!(matches!(
            parse_slash_t("/loadmzb 7368", &empty_entities(), pos, None, None),
            SlashOutcome::LoadMzb {
                chunk_idx: None,
                ..
            }
        ));

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
        match parse_slash_t("/pathto target", &[entity], origin(), Some(42), None) {
            SlashOutcome::Command(AgentCommand::PathTo { x, y, z, .. }) => {
                assert_eq!((x, y, z), (7.0, 8.0, 9.0));
            }
            other => panic!("expected PathTo from target, got {other:?}"),
        }
    }

    #[test]
    fn pathto_rejects_bad_input() {
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
        let mut self_pos = origin();
        self_pos.z = 17.5;
        match parse_slash_t("/pathto 10 20", &empty_entities(), self_pos, None, None) {
            SlashOutcome::Command(AgentCommand::PathTo { x, y, z, .. }) => {
                assert_eq!((x, y, z), (10.0, 20.0, 17.5));
            }
            other => panic!("expected PathTo, got {other:?}"),
        }
    }

    #[test]
    fn pathto_fuzzy_name_picks_entity() {
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
    fn sell_with_slot_uses_qty_one_by_default() {
        let out = parse_slash_t("/sell 5", &empty_entities(), origin(), None, None);
        match out {
            SlashOutcome::ShopSellSlot { inv_slot, qty } => {
                assert_eq!(inv_slot, 5);
                assert_eq!(qty, 1);
            }
            other => panic!("expected ShopSellSlot, got {other:?}"),
        }
    }

    #[test]
    fn sell_with_qty_passes_it_through() {
        let out = parse_slash_t("/sell 2 12", &empty_entities(), origin(), None, None);
        match out {
            SlashOutcome::ShopSellSlot { inv_slot, qty } => {
                assert_eq!(inv_slot, 2);
                assert_eq!(qty, 12);
            }
            other => panic!("expected ShopSellSlot, got {other:?}"),
        }
    }

    #[test]
    fn sell_confirm_maps_to_confirm_outcome() {
        let out = parse_slash_t("/sell confirm", &empty_entities(), origin(), None, None);
        assert!(matches!(out, SlashOutcome::ShopSellConfirm));
    }

    #[test]
    fn sell_rejects_zero_qty_and_bad_input() {
        for bad in ["/sell", "/sell x", "/sell 1 0", "/sell 1 x"] {
            let out = parse_slash_t(bad, &empty_entities(), origin(), None, None);
            assert!(
                matches!(out, SlashOutcome::SystemMessage(_)),
                "`{bad}` should be rejected"
            );
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

    #[test]
    fn engage_dispatches_reactor_goal() {
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
        assert!(matches!(
            parse_slash_t("/disengage", &empty_entities(), origin(), None, None),
            SlashOutcome::Command(AgentCommand::Cancel)
        ));
    }

    #[test]
    fn raw_attack_preserves_direct_action() {
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

    #[test]
    fn endevent_aliases_dispatch_end_event() {
        for input in ["/endevent", "/endevt", "/clearevent", "/clearevt"] {
            match parse_slash_t(input, &empty_entities(), origin(), None, None) {
                SlashOutcome::Command(AgentCommand::EndEvent) => {}
                other => panic!("input {input:?}: expected EndEvent, got {other:?}"),
            }
        }
    }

    #[test]
    fn endcutscene_no_arg_returns_none() {
        match parse_slash_t("/endcutscene", &empty_entities(), origin(), None, Some(231)) {
            SlashOutcome::EndCutscene { event_num } => assert_eq!(event_num, None),
            other => panic!("expected EndCutscene{{ None }}, got {other:?}"),
        }
    }

    #[test]
    fn endcutscene_with_explicit_csid_overrides_zone_lookup() {
        match parse_slash_t(
            "/endcutscene 7",
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
                assert!(matches!(
                    kind,
                    crate::state::MogHouseExit::Home { exit_bit: 0 }
                ));
                assert_eq!(kind.wire_pair(), (0, 0));
            }
            other => panic!("expected MogHouseExit::Home, got {other:?}"),
        }

        // With a known city zone, Home echoes the zone-derived bit like retail
        // (research/XiPackets/world/client/0x005E).
        match parse_slash_t("/mhexit home", &empty_entities(), origin(), None, Some(235)) {
            SlashOutcome::Command(AgentCommand::MogHouseExit { kind }) => {
                assert_eq!(kind.wire_pair(), (2, 0));
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
        match parse_slash_t("/mhexit auto", &empty_entities(), origin(), None, Some(100)) {
            SlashOutcome::SystemMessage(msg) => {
                assert!(msg.contains("auto"), "got: {msg}");
            }
            other => panic!("expected error SystemMessage, got {other:?}"),
        }
    }

    #[test]
    fn jobchange_no_args_opens_mog_menu() {
        match parse_slash_t("/jc", &empty_entities(), origin(), None, None) {
            SlashOutcome::Command(AgentCommand::OpenMogMenu) => {}
            other => panic!("expected OpenMogMenu, got {other:?}"),
        }
    }

    #[test]
    fn jobchange_outside_mog_house_warns_but_sends() {
        match parse_slash_t(
            "/jobchange WAR mnk",
            &empty_entities(),
            origin(),
            None,
            None,
        ) {
            SlashOutcome::CommandWithNotice { cmd, notice } => {
                assert!(matches!(
                    cmd,
                    AgentCommand::ChangeJob {
                        main_job: Some(1),
                        sub_job: Some(2)
                    }
                ));
                assert!(notice.contains("Mog House"), "got: {notice}");
            }
            other => panic!("expected CommandWithNotice, got {other:?}"),
        }
    }

    #[test]
    fn jobchange_in_mog_house_sends_without_notice() {
        let outcome = parse_slash(
            "/jc warrior",
            &empty_entities(),
            origin(),
            None,
            Some(230),
            None,
            &[],
            Some(ffxi_viewer_wire::MyRoom {
                model: 257,
                sub_map: 0,
            }),
        );
        match outcome {
            SlashOutcome::Command(AgentCommand::ChangeJob {
                main_job: Some(1),
                sub_job: None,
            }) => {}
            other => panic!("expected ChangeJob, got {other:?}"),
        }
    }

    #[test]
    fn jobchange_accepts_names_codes_and_ids_case_insensitively() {
        assert_eq!(parse_job_token("WAR"), Some(1));
        assert_eq!(parse_job_token("war"), Some(1));
        assert_eq!(parse_job_token("Warrior"), Some(1));
        assert_eq!(parse_job_token("mnk"), Some(2));
        assert_eq!(parse_job_token("13"), Some(13));
        assert_eq!(parse_job_token("0"), None, "0 = keep-current, not a job");
        assert_eq!(parse_job_token("none"), None);
        assert_eq!(parse_job_token("moogle"), None);
    }

    #[test]
    fn jobchange_unknown_job_is_system_message() {
        match parse_slash_t("/jc moogle", &empty_entities(), origin(), None, None) {
            SlashOutcome::SystemMessage(msg) => assert!(msg.contains("moogle"), "got: {msg}"),
            other => panic!("expected SystemMessage, got {other:?}"),
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

    #[test]
    fn every_mcp_tool_has_a_slash_twin() {
        let entities = vec![ent(42, "Mob", EntityKind::Mob, 0.0, 0.0)];
        let pos = origin();
        let cur = Some(42);

        let cases: Vec<(&str, fn(&AgentCommand) -> bool)> = vec![
            ("/follow Mob", |c| {
                matches!(c, AgentCommand::Follow { target_id: 42, .. })
            }),
            ("/engage", |c| {
                matches!(c, AgentCommand::Engage { target_id: 42 })
            }),
            ("/pathto 1 2 3", |c| {
                matches!(c, AgentCommand::PathTo { .. })
            }),
            ("/cancel", |c| matches!(c, AgentCommand::Cancel)),
            ("/bank 60 12345", |c| {
                matches!(
                    c,
                    AgentCommand::BankWhenFull {
                        threshold: 60,
                        mog_house_zoneline: 12345
                    }
                )
            }),
            ("/s hello", |c| {
                matches!(c, AgentCommand::Chat { kind: 0, .. })
            }),
            ("/p hello", |c| {
                matches!(c, AgentCommand::Chat { kind: 4, .. })
            }),
            ("/tell Bob hi", |c| matches!(c, AgentCommand::Tell { .. })),
            ("/zonechange 42", |c| {
                matches!(c, AgentCommand::RequestZoneChange { line_id: 42 })
            }),
            ("/snapshot", |c| matches!(c, AgentCommand::Snapshot)),
            ("/cast 1", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::CastMagic { .. },
                        ..
                    }
                )
            }),
            ("/ws 1", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::Weaponskill { .. },
                        ..
                    }
                )
            }),
            ("/ja 1", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::JobAbility { .. },
                        ..
                    }
                )
            }),
            ("/useitem 0 4", |c| {
                matches!(c, AgentCommand::UseItem { .. })
            }),
            ("/raisemenu accept", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::RaiseMenu { .. },
                        ..
                    }
                )
            }),
            ("/tractormenu accept", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::TractorMenu { .. },
                        ..
                    }
                )
            }),
            ("/homepointmenu 0", |c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::HomepointMenu { .. },
                        ..
                    }
                )
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
    fn help_command_returns_multiline_listing() {
        for slash in ["/help", "/?"] {
            let out = parse_slash_t(slash, &empty_entities(), origin(), None, None);
            match out {
                SlashOutcome::SystemMessage(s) => {
                    assert!(
                        s.contains("Slash command reference"),
                        "{slash} missing header"
                    );

                    for (category, _) in COMMANDS {
                        assert!(
                            s.contains(category),
                            "{slash} output missing category `{category}`"
                        );
                    }

                    assert!(s.contains("/follow"), "{slash} missing /follow");
                    assert!(s.contains("/help"), "{slash} missing /help self-reference");
                }
                other => panic!("expected SystemMessage from {slash}, got {other:?}"),
            }
        }
    }

    #[test]
    fn help_listing_fits_in_local_toast_cap() {
        let lines = render_help().split('\n').count();
        let cap = ffxi_viewer_core::snapshot::LOCAL_TOAST_CAP;
        assert!(
            lines <= cap,
            "/help renders {lines} lines but the chat buffer retains only {cap}; \
             the top {} lines would be evicted unreadable",
            lines.saturating_sub(cap),
        );
    }

    #[test]
    fn every_alias_dispatches() {
        for (_, cmds) in COMMANDS {
            for cmd in *cmds {
                for alias in cmd.aliases {
                    let slash = format!("/{alias}");
                    let out = parse_slash_t(&slash, &empty_entities(), origin(), None, None);
                    if let SlashOutcome::SystemMessage(ref s) = out {
                        assert!(
                            !s.starts_with("unknown command:"),
                            "registered alias `/{alias}` dispatched to the unknown-command \
                             fallthrough"
                        );
                    }
                }
            }
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
        assert_eq!(tid, None, "no selection → untargeted");

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
        assert_eq!(param, emote::JOB_PARAM_BASE, "WAR(1) → 0x1F");
        let (_, _, param, ..) = expect_emote(parse_slash_t(
            "/jobemote RUN",
            &[],
            WireVec3::default(),
            None,
            None,
        ));
        assert_eq!(param, emote::JOB_PARAM_BASE + 21, "RUN(22) → 0x34");
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
    /// a scraped emote name would silently shadow the emote — forbid it
    /// (Bell/Job keep dedicated commands with required args).
    #[test]
    fn no_alias_shadows_a_scraped_emote_name() {
        use ffxi_proto::map::emote;
        for &(id, name) in ffxi_proto::emote_names::EMOTES {
            if emote::HELM_ONLY.contains(&id) || id == emote::BELL || id == emote::JOB {
                continue;
            }
            let lower = name.to_lowercase();
            for (category, cmds) in COMMANDS {
                for cmd in *cmds {
                    assert!(
                        !cmd.aliases.contains(&lower.as_str()),
                        "alias `/{lower}` in `{category}` shadows emote {id}"
                    );
                }
            }
        }
    }

    #[test]
    fn aliases_are_unique() {
        let mut seen = std::collections::HashMap::new();
        for (category, cmds) in COMMANDS {
            for cmd in *cmds {
                for alias in cmd.aliases {
                    if let Some(prev) = seen.insert(*alias, *category) {
                        panic!(
                            "alias `/{alias}` is registered twice (in `{prev}` and `{category}`)"
                        );
                    }
                }
            }
        }
    }
}
