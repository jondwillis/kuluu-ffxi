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

use crate::state::{ActionKind, AgentCommand};

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
    /// Append a `[system]` chat line. Used for unknown commands and for
    /// stubbed commands (like `/check`) we haven't fully wired yet.
    SystemMessage(String),
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
            match resolve_target_or_current(rest, entities, self_pos, current_target) {
                Some(id) => SlashOutcome::Command(AgentCommand::Engage { target_id: id }),
                None => SlashOutcome::SystemMessage(format!("/{cmd}: no target")),
            }
        }
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
        "check" => SlashOutcome::SystemMessage(
            "/check: not yet wired (needs ChangeTarget action + response parse)".into(),
        ),
        "sit" => SlashOutcome::SystemMessage("/sit: not yet wired".into()),
        "stand" => SlashOutcome::SystemMessage("/stand: not yet wired".into()),
        "cancel" => SlashOutcome::Command(AgentCommand::Cancel),
        "disconnect" | "quit" | "logout" => SlashOutcome::Quit,
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
        for s in ["/quit", "/disconnect", "/logout"] {
            assert!(matches!(
                parse_slash(s, &empty_entities(), origin(), None),
                SlashOutcome::Quit
            ));
        }
    }

    #[test]
    fn cancel_emits_cancel_command() {
        assert!(matches!(
            parse_slash("/cancel", &empty_entities(), origin(), None),
            SlashOutcome::Command(AgentCommand::Cancel)
        ));
    }
}
