//! Live relay probe for verification sessions (.agents/skills/verify).
//!
//! Attaches to a running kuluu client's WebSocket relay (launch the client
//! with `--relay-listen auto` or a fixed port; it prints
//! `relay listening on ws://...`) and either streams the frame feed as compact
//! per-entity update lines, or sends one viewer command.
//!
//! The stream is the session state's own view of what the wire delivered:
//! every relay snapshot is diffed against the previous one, so `ADD`/`DEL`/`UP`
//! lines are exactly which entities changed and how (status byte, hp,
//! nameplate-driving flags) — the per-entity "update list" for diagnosing
//! stale-table bugs without touching game state. Cross-platform: unlike the
//! unix-socket agent probe, this works on Windows hosts too.
//!
//! Usage:
//!   cargo run -p kuluu-session --features relay --example relay-probe \
//!       -- watch [--url=ws://127.0.0.1:PORT] [--filter SUBSTR] [--verbose]
//!   ... -- chat "!zone Davoi"
//!   ... -- engage <entity_id>
//!   ... -- screenshot [path.png]
//!
//! `--url` may be omitted when `FFXI_RELAY_URL` is set.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use kuluu_snapshot::{CharFlags, ClientFrame, Entity, Frame, ViewerCommand, ViewerEvent};
use tokio_tungstenite::{connect_async, tungstenite::Message};

fn relay_url(args: &[String]) -> String {
    let from_arg = args.iter().find_map(|a| a.strip_prefix("--url="));
    let url = from_arg
        .map(str::to_string)
        .or_else(|| std::env::var("FFXI_RELAY_URL").ok())
        .unwrap_or_else(|| {
            panic!(
                "no relay URL: pass --url=ws://127.0.0.1:<port> or set FFXI_RELAY_URL \
                 (the client prints `relay listening on ws://...` at startup)"
            )
        });
    // The relay defaults to postcard binary; ask for JSON text frames.
    if url.contains('?') {
        url
    } else {
        format!("{url}?format=json")
    }
}

/// The fields a stale-table diagnosis actually reads; position is excluded so
/// movement never spams the diff. `claim` drives the white→claimed plate colour.
#[derive(Clone, Copy, PartialEq)]
struct Key {
    status: u8,
    hp: Option<u8>,
    flags: CharFlags,
    name_vis: Option<u8>,
    claim: u32,
}

fn key_of(e: &Entity) -> Key {
    Key {
        status: e.status,
        hp: e.hp_pct,
        flags: e.char_flags,
        name_vis: e.name_vis,
        claim: e.claim_id,
    }
}

fn name_of(e: &Entity) -> String {
    e.name.clone().unwrap_or_else(|| "?".into())
}

/// One compact line per entity upsert. `nc` = new-adventurer "?" marker,
/// `inv` = invis flag, `unt` = untargetable.
fn up_line(id: u32, name: &str, old: Option<&Key>, new: &Entity) -> String {
    let st = match old {
        Some(o) if o.status != new.status => format!("{}->{}", o.status, new.status),
        _ => format!("{}", new.status),
    };
    let cl = match old {
        Some(o) if o.claim != new.claim_id => format!("{:#X}->{:#X}", o.claim, new.claim_id),
        _ if new.claim_id != 0 => format!("{:#X}", new.claim_id),
        _ => "-".into(),
    };
    format!(
        "UP  id={id:08X} {name} st={st} hp={:?} cl={cl} nc={} inv={} unt={}",
        new.hp_pct, new.char_flags.new_character, new.char_flags.invis, new.char_flags.untargetable,
    )
}

fn add_line(e: &Entity) -> String {
    format!(
        "ADD id={:08X} {:?} {} st={} hp={:?} cl={:#X} nc={} inv={} unt={} pos=({:.1},{:.1})",
        e.id,
        e.kind,
        name_of(e),
        e.status,
        e.hp_pct,
        e.claim_id,
        e.char_flags.new_character,
        e.char_flags.invis,
        e.char_flags.untargetable,
        e.pos.x,
        e.pos.y,
    )
}

fn del_line(id: u32, name: &str) -> String {
    format!("DEL id={id:08X} {name}")
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(|s| s.as_str()).unwrap_or("watch");
    let url = relay_url(&args);

    match mode {
        "chat" => {
            let text = args
                .get(1)
                .filter(|a| !a.starts_with("--url="))
                .context("usage: relay-probe chat <text> [--url=ws://host:port]")?;
            send_one(
                &url,
                &ViewerCommand::Chat {
                    kind: 0,
                    text: text.clone(),
                },
            )
            .await?;
            println!("sent chat: {text}");
        }
        "engage" => {
            let raw = args
                .get(1)
                .filter(|a| !a.starts_with("--url="))
                .context("usage: relay-probe engage <entity_id> (hex or decimal)")?;
            // Watch lines print ids as 08X hex; accept both.
            let id = if let Some(hex) = raw.strip_prefix("0x") {
                u32::from_str_radix(hex, 16).with_context(|| format!("bad hex id: {raw}"))?
            } else {
                raw.parse::<u32>()
                    .with_context(|| format!("bad entity id: {raw}"))?
            };
            send_one(&url, &ViewerCommand::Engage { target_id: id }).await?;
        }
        "screenshot" => {
            let path = args
                .iter()
                .find(|a| !a.starts_with("--url="))
                .filter(|a| a.as_str() != "screenshot")
                .cloned();
            send_one(&url, &ViewerCommand::Screenshot { path }).await?;
        }
        _ => {
            let mut filter: Option<String> = None;
            let mut verbose = false;
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--filter" => {
                        filter = args.get(i + 1).cloned();
                        i += 2;
                    }
                    "--verbose" => {
                        verbose = true;
                        i += 1;
                    }
                    _ => i += 1, // --url=... handled by relay_url()
                }
            }
            watch(&url, filter.as_deref(), verbose).await?;
        }
    }
    Ok(())
}

/// Send one viewer command (postcard binary frame) and drain ~2s of replies.
async fn send_one(url: &str, cmd: &ViewerCommand) -> Result<()> {
    let (ws, _resp) = connect_async(url)
        .await
        .with_context(|| format!("connecting to {url}"))?;
    let (mut sink, mut stream) = ws.split();
    let frame = ClientFrame::Command(cmd.clone());
    let bytes = postcard::to_allocvec(&frame).context("postcard encoding ClientFrame")?;
    sink.send(Message::Binary(bytes))
        .await
        .context("sending command")?;

    // Give the server a moment to answer (events, or nothing for screenshots —
    // the PNG lands in the client's CWD instead).
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let f: Frame = serde_json::from_str(&t)
                    .unwrap_or_else(|_| panic!("undecodable relay frame: {t}"));
                println!("reply: {f:?}");
            }
            Ok(Some(_)) => {}
            _ => break,
        }
    }

    if matches!(cmd, ViewerCommand::Screenshot { .. }) {
        let where_to = match &cmd {
            ViewerCommand::Screenshot { path: Some(p) } => p.clone(),
            _ => "screenshot-N.png (client CWD)".into(),
        };
        println!("screenshot requested -> {where_to}");
    }
    Ok(())
}

async fn watch(url: &str, filter: Option<&str>, verbose: bool) -> Result<()> {
    let (ws, _resp) = connect_async(url)
        .await
        .with_context(|| format!("connecting to {url}"))?;
    eprintln!("relay-probe: attached to {url}");
    let (_sink, mut stream) = ws.split();

    // id -> (name, last key); the diff source for every incoming snapshot.
    let mut prev: HashMap<u32, (String, Key)> = HashMap::new();
    let mut zone: Option<u16> = None;
    let mut stage: Option<kuluu_snapshot::Stage> = None;
    let mut snaps: u64 = 0;
    let mut last_hb = Instant::now();

    fn keep(filter: Option<&str>, line: &str) -> bool {
        match filter {
            Some(f) => line.to_lowercase().contains(&f.to_lowercase()),
            None => true,
        }
    }

    while let Some(msg) = stream.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => bail!("reading relay: {e}"),
        };
        let frame = match &msg {
            Message::Text(t) => match serde_json::from_str::<Frame>(t) {
                Ok(f) => f,
                Err(_) => {
                    eprintln!("relay-probe: undecodable frame: {t}");
                    continue;
                }
            },
            _ => {
                eprintln!("relay-probe: expected text (json) frame, got binary/other");
                continue;
            }
        };

        match &frame {
            Frame::Hello { protocol_version } => {
                println!("HELLO proto={protocol_version}");
            }
            Frame::Snapshot(snap) => {
                snaps += 1;
                if snap.zone_id != zone {
                    println!(
                        "ZONE {} -> {:?} ({} ents)",
                        zone.map(|z| z.to_string()).unwrap_or_else(|| "-".into()),
                        snap.zone_id,
                        snap.entities.len()
                    );
                    zone = snap.zone_id;
                }
                if stage.as_ref() != Some(&snap.stage) {
                    println!("STAGE -> {:?}", snap.stage);
                    stage = Some(snap.stage);
                }

                let mut now: HashMap<u32, (String, Key)> =
                    HashMap::with_capacity(snap.entities.len());
                for e in &snap.entities {
                    now.insert(e.id, (name_of(e), key_of(e)));
                }

                // Additions and changes.
                for e in &snap.entities {
                    match prev.get(&e.id) {
                        None => {
                            let line = add_line(e);
                            if keep(filter, &line) {
                                println!("{line}");
                            }
                        }
                        Some((name, old)) => {
                            if *old != key_of(e) {
                                let line = up_line(e.id, name, Some(old), e);
                                if keep(filter, &line) {
                                    println!("{line}");
                                }
                            } else if verbose {
                                // Key unchanged but present: movement/heading only.
                                eprintln!(
                                    "    (move id={:08X} {} -> ({:.1},{:.1}))",
                                    e.id,
                                    name_of(e),
                                    e.pos.x,
                                    e.pos.y
                                );
                            }
                        }
                    }
                }
                // Removals.
                for (id, (name, _)) in prev.iter().filter(|(id, _)| !now.contains_key(id)) {
                    let line = del_line(*id, name);
                    if keep(filter, &line) {
                        println!("{line}");
                    }
                }

                prev = now;

                if last_hb.elapsed() >= Duration::from_secs(5) {
                    last_hb = Instant::now();
                    eprintln!(
                        "HB snaps={snaps} ents={} zone={:?}",
                        snap.entities.len(),
                        snap.zone_id
                    );
                }
            }
            Frame::Delta(_) => {
                // The relay only emits full snapshots; a delta here would be a
                // protocol surprise worth surfacing.
                eprintln!("relay-probe: unexpected Delta frame");
            }
            Frame::Event(ev) => {
                let line = match ev {
                    ViewerEvent::ZoneChanged { from, to } => {
                        format!("EV zone {from:?} -> {to}")
                    }
                    ViewerEvent::EntityRemoved { id } => format!("EV removed id={id:08X}"),
                    ViewerEvent::Disconnected { reason } => {
                        format!("EV disconnected: {reason}")
                    }
                    ViewerEvent::Reconnected { downtime_ms } => {
                        format!("EV reconnected after {downtime_ms}ms")
                    }
                    ViewerEvent::EngagedBy { entity_id } => {
                        format!("EV engaged by id={entity_id:08X}")
                    }
                    _ if verbose => {
                        let v = serde_json::to_value(ev).unwrap_or_default();
                        eprintln!("  {v}");
                        "EV other".into()
                    }
                    _ => continue,
                };
                if keep(filter, &line) {
                    println!("{line}");
                }
            }
        }
    }

    bail!("relay connection closed")
}
