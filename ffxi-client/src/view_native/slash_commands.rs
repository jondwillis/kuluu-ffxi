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

use ffxi_viewer_wire::{ChatChannel, ChatLine, Entity as WireEntity, Vec3 as WireVec3};

use crate::state::{ActionKind, AgentCommand, CheckKind, ReqLogoutKind};

/// What the input router should do after parsing a `/`-prefixed buffer.
#[derive(Debug, Clone)]
pub enum SlashOutcome {
    /// Dispatch this command on the agent channel.
    Command(AgentCommand),
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
}

/// Parse a `/`-prefixed buffer. The leading `/` must be present.
///
/// `entities` is the current snapshot entity list and `self_pos` is the
/// player's position — used for name → id resolution and tie-breaking
/// when several entities share a prefix. `current_target` falls in for
/// commands that accept "use current target" when no name is given.
pub fn parse_slash(
    buffer: &str,
    entities: &[WireEntity],
    self_pos: WireVec3,
    current_target: Option<u32>,
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
            // Wire-level Action with `ActionKind::Attack` (action_id 0x02) —
            // not the goal-level `AgentCommand::Engage`. The native viewer
            // doesn't put a reactor in front of `cmd_rx` (see session.rs:704
            // "reactor goal command reached session loop"), so goal-level
            // commands silently fail. The /attack slash needs a packet to
            // go out, so we issue the action directly.
            match resolve_action_target(rest, entities, self_pos, current_target) {
                Some((id, idx)) => SlashOutcome::Command(AgentCommand::Action {
                    target_id: id,
                    target_index: idx,
                    kind: ActionKind::Attack,
                }),
                None => SlashOutcome::SystemMessage(format!("/{cmd}: no target")),
            }
        }
        "attackoff" | "disengage" => match current_target {
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
        "navmesh" => parse_navmesh(rest),
        "pathto" => parse_pathto(rest, entities, current_target),
        "shutdown" => parse_reqlogout(rest, /* shutdown = */ true),
        "p" | "party" => chat_or_empty(rest, 4, "/p"),
        "sh" | "shout" => chat_or_empty(rest, 1, "/sh"),
        "l" | "linkshell" | "ls" => chat_or_empty(rest, 5, "/l"),
        "s" | "say" => chat_or_empty(rest, 0, "/s"),
        "t" | "tell" => parse_tell(rest),
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
    SlashOutcome::Command(AgentCommand::ReqLogout { kind })
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
        let out = parse_slash("/", &empty_entities(), origin(), None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn unknown_command_is_system_message() {
        let out = parse_slash("/blarg", &empty_entities(), origin(), None);
        match out {
            SlashOutcome::SystemMessage(s) => assert!(s.contains("/blarg")),
            _ => panic!("expected SystemMessage"),
        }
    }

    #[test]
    fn party_chat_with_text() {
        let out = parse_slash("/p hello world", &empty_entities(), origin(), None);
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
        let out = parse_slash("/p", &empty_entities(), origin(), None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn tell_requires_name_and_text() {
        let out = parse_slash("/t Bob hi there", &empty_entities(), origin(), None);
        match out {
            SlashOutcome::Command(AgentCommand::Tell { to, text }) => {
                assert_eq!(to, "Bob");
                assert_eq!(text, "hi there");
            }
            other => panic!("expected Tell, got {other:?}"),
        }

        let out = parse_slash("/t Bob", &empty_entities(), origin(), None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn follow_with_name_resolves_to_id() {
        let entities = vec![
            ent(101, "Bob", EntityKind::Pc, 0.0, 0.0),
            ent(102, "Bobble", EntityKind::Npc, 5.0, 5.0),
        ];
        let out = parse_slash("/follow Bob", &entities, origin(), None);
        match out {
            SlashOutcome::Command(AgentCommand::Follow { target_id, .. }) => {
                assert_eq!(target_id, 101); // PC wins over NPC on prefix tie
            }
            other => panic!("expected Follow, got {other:?}"),
        }
    }

    #[test]
    fn follow_no_name_uses_current_target() {
        let out = parse_slash("/follow", &empty_entities(), origin(), Some(42));
        match out {
            SlashOutcome::Command(AgentCommand::Follow { target_id, .. }) => {
                assert_eq!(target_id, 42);
            }
            other => panic!("expected Follow, got {other:?}"),
        }
    }

    #[test]
    fn follow_no_name_no_target_is_system_message() {
        let out = parse_slash("/follow", &empty_entities(), origin(), None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn target_clears_with_no_arg() {
        let out = parse_slash("/target", &empty_entities(), origin(), Some(7));
        assert!(matches!(out, SlashOutcome::SetTarget(None)));
    }

    #[test]
    fn quit_aliases() {
        // `/quit` and `/disconnect` are the "drop the session" pair.
        // `/logout` is *not* in this group — it goes through the
        // server's LeaveGame flow; see `logout_*` tests below.
        for s in ["/quit", "/disconnect"] {
            assert!(matches!(
                parse_slash(s, &empty_entities(), origin(), None),
                SlashOutcome::Quit
            ));
        }
    }

    #[test]
    fn logout_no_arg_toggles() {
        match parse_slash("/logout", &empty_entities(), origin(), None) {
            SlashOutcome::Command(AgentCommand::ReqLogout { kind }) => {
                assert_eq!(kind, ReqLogoutKind::LogoutToggle);
            }
            other => panic!("expected ReqLogout(LogoutToggle), got {other:?}"),
        }
    }

    #[test]
    fn logout_on_and_off_select_explicit_modes() {
        match parse_slash("/logout on", &empty_entities(), origin(), None) {
            SlashOutcome::Command(AgentCommand::ReqLogout { kind }) => {
                assert_eq!(kind, ReqLogoutKind::LogoutOn);
            }
            other => panic!("expected ReqLogout(LogoutOn), got {other:?}"),
        }
        match parse_slash("/logout off", &empty_entities(), origin(), None) {
            SlashOutcome::Command(AgentCommand::ReqLogout { kind }) => {
                assert_eq!(kind, ReqLogoutKind::LogoutOff);
            }
            other => panic!("expected ReqLogout(LogoutOff), got {other:?}"),
        }
    }

    #[test]
    fn shutdown_no_arg_toggles() {
        match parse_slash("/shutdown", &empty_entities(), origin(), None) {
            SlashOutcome::Command(AgentCommand::ReqLogout { kind }) => {
                assert_eq!(kind, ReqLogoutKind::ShutdownToggle);
            }
            other => panic!("expected ReqLogout(ShutdownToggle), got {other:?}"),
        }
    }

    #[test]
    fn shutdown_on_and_off_select_explicit_modes() {
        match parse_slash("/shutdown on", &empty_entities(), origin(), None) {
            SlashOutcome::Command(AgentCommand::ReqLogout { kind }) => {
                assert_eq!(kind, ReqLogoutKind::ShutdownOn);
            }
            other => panic!("expected ReqLogout(ShutdownOn), got {other:?}"),
        }
        match parse_slash("/shutdown off", &empty_entities(), origin(), None) {
            SlashOutcome::Command(AgentCommand::ReqLogout { kind }) => {
                assert_eq!(kind, ReqLogoutKind::ShutdownOff);
            }
            other => panic!("expected ReqLogout(ShutdownOff), got {other:?}"),
        }
    }

    #[test]
    fn exit_emits_quit_with_logout_on() {
        // /exit must arm with `LogoutOn` (not Toggle) so it can't
        // accidentally cancel an in-flight logout from a prior /logout.
        match parse_slash("/exit", &empty_entities(), origin(), None) {
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
                    parse_slash(s, &empty_entities(), origin(), None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}"
            );
        }
    }

    #[test]
    fn navmesh_no_arg_toggles() {
        match parse_slash("/navmesh", &empty_entities(), origin(), None) {
            SlashOutcome::ToggleNavmesh(None) => {}
            other => panic!("expected ToggleNavmesh(None), got {other:?}"),
        }
    }

    #[test]
    fn navmesh_on_and_off_select_explicit_modes() {
        for (cmd, expected) in [("/navmesh on", Some(true)), ("/navmesh off", Some(false))] {
            match parse_slash(cmd, &empty_entities(), origin(), None) {
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
                    parse_slash(s, &empty_entities(), origin(), None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}"
            );
        }
    }

    #[test]
    fn pathto_numeric_three_args_dispatches() {
        match parse_slash("/pathto 1.5 2 -3.25", &empty_entities(), origin(), None) {
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
        match parse_slash("/pathto target", &[entity], origin(), Some(42)) {
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
                    parse_slash(s, &empty_entities(), origin(), None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}"
            );
        }
    }

    #[test]
    fn cancel_emits_cancel_command() {
        assert!(matches!(
            parse_slash("/cancel", &empty_entities(), origin(), None),
            SlashOutcome::Command(AgentCommand::Cancel)
        ));
    }

    #[test]
    fn attack_uses_current_target_and_attack_kind() {
        let entities = vec![ent(42, "Mob", EntityKind::Mob, 0.0, 0.0)];
        let out = parse_slash("/attack", &entities, origin(), Some(42));
        match out {
            SlashOutcome::Command(AgentCommand::Action {
                target_id,
                target_index,
                kind,
            }) => {
                assert_eq!(target_id, 42);
                assert_eq!(target_index, 42);
                assert!(matches!(kind, ActionKind::Attack));
            }
            other => panic!("expected Action(Attack), got {other:?}"),
        }
    }

    #[test]
    fn engage_alias_matches_attack() {
        let entities = vec![ent(7, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash("/engage", &entities, origin(), Some(7)) {
            SlashOutcome::Command(AgentCommand::Action { kind, .. }) => {
                assert!(matches!(kind, ActionKind::Attack));
            }
            other => panic!("expected Action(Attack), got {other:?}"),
        }
    }

    #[test]
    fn attackoff_emits_attack_off_action() {
        let entities = vec![ent(9, "Mob", EntityKind::Mob, 0.0, 0.0)];
        match parse_slash("/attackoff", &entities, origin(), Some(9)) {
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
        let out = parse_slash("/check", &entities, origin(), Some(7));
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
        match parse_slash("/checkname", &entities, origin(), Some(7)) {
            SlashOutcome::Command(AgentCommand::CheckTarget { kind, .. }) => {
                assert_eq!(kind, CheckKind::CheckName);
            }
            other => panic!("expected CheckTarget, got {other:?}"),
        }
        match parse_slash("/checkparam", &entities, origin(), Some(7)) {
            SlashOutcome::Command(AgentCommand::CheckTarget { kind, .. }) => {
                assert_eq!(kind, CheckKind::CheckParam);
            }
            other => panic!("expected CheckTarget, got {other:?}"),
        }
    }

    #[test]
    fn check_no_target_is_system_message() {
        let out = parse_slash("/check", &empty_entities(), origin(), None);
        assert!(matches!(out, SlashOutcome::SystemMessage(_)));
    }

    #[test]
    fn buy_with_row_uses_qty_one_by_default() {
        let out = parse_slash("/buy 3", &empty_entities(), origin(), None);
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
        let out = parse_slash("/buy 0 12", &empty_entities(), origin(), None);
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
                    parse_slash(s, &empty_entities(), origin(), None),
                    SlashOutcome::SystemMessage(_)
                ),
                "expected SystemMessage for {s}"
            );
        }
    }
}
