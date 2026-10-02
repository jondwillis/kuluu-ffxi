// Live end-to-end playback of event 568, the Northern San d'Oria new-character
// Chateau d'Oraguille intro, against a local LSB stack with retail DATs
// mounted.
//
// Self-skips when the auth port or xidb is unreachable, or when no FFXI
// install can be opened. The 568 program (zone master block of ROM/21/40.DAT)
// is a single-beat script: cancel lock, a 16 ms wait, lookat/interactive on
// the Chateau NPC, then 0x47 EVENTPOSSET case 0, which sends the c2s 0x05C
// position tag and parks the VM at the case-1 poll until the server's s2c
// 0x052 EventRecvPending acks it (vendor/server/src/map/packets/c2s/
// 0x05c_eventendxzy.cpp luautils::OnEventUpdate; Northern San d'Oria's
// onEventUpdate is an empty function). The single message frame then
// dismisses with choice 0, 0x21 EXECEND sends the c2s 0x05B, and the
// server's endCurrentEvent answers with its own 0x052. The event ending at
// all is live proof the 0x47/0x05C onEventUpdate round trip completed: the
// case-1 hold cannot release without that ack, and the frame only appears
// after it does.

mod common;

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use kuluu_session::{
    session::{self, CharSelection, Config},
    state::{AgentCommand, AgentEvent, Stage},
};
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc},
    time::timeout,
};

use common::EphemeralChar;

// Raw event id of the Northern San d'Oria new-character cutscene
// (vendor/server/scripts/zones/Northern_San_dOria/Zone.lua onTriggerAreaEnter
// area 1: 569 for a rank-2+ nation-0 char, a nation-5-mission char, or any
// rank-3+ char; 568 otherwise — the fixture char is nation 0 rank 0).
const EVENT_568: u16 = 568;
/// Northern San d'Oria zone id.
const NORTHERN_SAN_DORIA_ZONE: u32 = 231;

// The trigger cuboid (Zone.lua registerCuboidTriggerArea(1, -7, -3, 110, 7,
// -1, 155)): x in [-7, 7], height in [-3, -1], north-south in [110, 155].
// The fixture char spawns at the schema default (0, 0, 0), outside it, so the
// test walks it in. Session/wire Position: x = east-west, y = north-south,
// z = height, so these land the server's position_t at (0, -2, 130): inside
// all three bands.
const MOVE_TARGET_X: f32 = 0.0;
const MOVE_TARGET_Y: f32 = 130.0;
const MOVE_TARGET_Z: f32 = -2.0;
const MOVE_HEADING: u8 = 64;

const LOGIN_DEADLINE: Duration = Duration::from_secs(90);
// The trigger fires ~2.5 s after zone-in (the ZoningIn window,
// vendor/server/scripts/globals/player.lua onGameIn) plus the 200 ms
// trigger-check interval (vendor/server/src/map/map_constants.h
// kTriggerAreaInterval); the 0x47 round trip and the single frame then take
// well under a second. A minute from InZone is generous.
const NO_EVENT_GRACE: Duration = Duration::from_secs(60);
/// From CutsceneStarted to EventEnded: the 0x47 round trip
/// (research/XiEvents/OpCodes/0x0047.md) plus one frame.
const PLAYBACK_DEADLINE: Duration = Duration::from_secs(2 * 60);
/// The 568 script carries exactly one message frame (0x2B + 0x23,
/// research/XiEvents/OpCodes/0x002B.md). The frame only appears after the
/// 0x05C ack (research/XiEvents/OpCodes/0x005C.md) releases the case-1 hold,
/// so at least one frame is part of the round-trip proof.
const MIN_DIALOG_FRAMES: u32 = 1;

fn open_dat_root() -> Option<ffxi_dat::DatRoot> {
    ffxi_dat::archive::open_test_install()
}

fn artifact_dir() -> PathBuf {
    std::env::var_os("VERIFY_ARTIFACT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("workspace root above kuluu-session");
            workspace_root.join("artifacts").join("verify")
        })
}

#[derive(Default)]
struct Tally {
    stages_seen: Vec<Stage>,
    inzone_at: Option<Instant>,
    move_sent_at: Option<Instant>,
    cutscene_started_at: Option<Instant>,
    event_ended_at: Option<Instant>,
    auto_skipped_line: Option<String>,
    frames_total: u32,
    frame_texts: Vec<String>,
    disconnected_reason: Option<String>,
}

/// Tallys the 568 run. The end is gated on the cutscene having started:
/// `EventEnded` is a unit variant carrying no event id, so this ignores any
/// unrelated event that ends around zone-in; the first end after start is the
/// one tallied.
fn handle_event(tally: &mut Tally, ev: &AgentEvent, now: Instant) {
    match ev {
        AgentEvent::StageChanged { stage } => {
            if !tally.stages_seen.contains(stage) {
                tally.stages_seen.push(*stage);
            }
            if *stage == Stage::InZone && tally.inzone_at.is_none() {
                tally.inzone_at = Some(now);
                eprintln!("[live] InZone at t+{:.1}s", now.elapsed().as_secs_f32());
            }
        }
        AgentEvent::CutsceneStarted { event_id } => {
            if *event_id & 65535 == u32::from(EVENT_568) && tally.cutscene_started_at.is_none() {
                tally.cutscene_started_at = Some(now);
                eprintln!(
                    "[live] CutsceneStarted (agent id 0x{event_id:08X}) at t+{:.1}s",
                    now.elapsed().as_secs_f32()
                );
            }
        }
        AgentEvent::EventEnded => {
            if tally.event_ended_at.is_none() && tally.cutscene_started_at.is_some() {
                tally.event_ended_at = Some(now);
                eprintln!("[live] EventEnded at t+{:.1}s", now.elapsed().as_secs_f32());
            }
        }
        AgentEvent::ChatLine { line, .. } => {
            if line.text.contains(session::EVENT_AUTO_SKIPPED_MARKER)
                && tally.auto_skipped_line.is_none()
            {
                tally.auto_skipped_line = Some(line.text.clone());
                eprintln!("[live] AUTO-SKIP CHAT LINE: {}", line.text);
            }
        }
        AgentEvent::EventDialog { dialog } => {
            if dialog.event_para != EVENT_568 {
                return;
            }
            tally.frames_total += 1;
            let prompt = dialog.prompt.clone().unwrap_or_default();
            let snippet: String = prompt.chars().take(90).collect();
            tally.frame_texts.push(format!("[text] {snippet}"));
        }
        AgentEvent::Disconnected { reason } => {
            tally.disconnected_reason = Some(reason.clone());
        }
        _ => {}
    }
}

#[tokio::test]
async fn event_568_full_playback_against_live_lsb() {
    let server_host = std::env::var("SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let auth_port = std::env::var("AUTH_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(ffxi_proto::login::LOGIN_AUTH_PORT);

    if !is_reachable(&server_host, auth_port).await {
        eprintln!(
            "skipping: no LSB stack reachable at {server_host}:{auth_port}. \
             To run this test, start the dev stack and re-run with SERVER_HOST set."
        );
        return;
    }

    let Some(dat_root) = open_dat_root() else {
        eprintln!(
            "skipping: no FFXI install found (register one with \
             `kuluu install`, or set FFXI_DAT_PATH); without DATs the VM cannot drive the \
             568 program and this test would only re-prove the skip-to-end \
             failure mode"
        );
        return;
    };

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,kuluu_session=debug")),
        )
        .with_test_writer()
        .try_init();

    common::pin_unique_local_port();

    let Some(fixture) =
        EphemeralChar::create_in_zone(&server_host, auth_port, NORTHERN_SAN_DORIA_ZONE)
            .await
            .expect("provisioning ephemeral LSB account+char")
    else {
        eprintln!("skipping: xidb not reachable; treating the LSB stack as absent");
        return;
    };
    eprintln!(
        "fixture: user={} accid={} charid={} charname={} zone={NORTHERN_SAN_DORIA_ZONE}",
        fixture.username, fixture.accid, fixture.charid, fixture.charname,
    );

    let cfg = Config {
        server: server_host.clone(),
        map_host_override: None,
        auth_port,
        data_port: ffxi_proto::login::LOGIN_DATA_PORT,
        view_port: ffxi_proto::login::LOGIN_VIEW_PORT,
        user: fixture.username.clone(),
        password: fixture.password.clone(),
        char_selection: CharSelection::Name(fixture.charname.clone()),
        initial_state: None,
        playonline_session: None,
        dat_root: Some(Arc::new(dat_root)),
        user_driven_events: false,
    };

    let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>(32);
    let (event_tx, mut event_rx) = broadcast::channel::<AgentEvent>(512);

    let session_task = tokio::spawn(session::run(cfg, cmd_rx, event_tx));

    let out_dir = artifact_dir();
    fs::create_dir_all(&out_dir).expect("creating artifacts/verify");
    let events_path = out_dir.join("event_568_events.jsonl");
    let mut events_log = fs::File::create(&events_path).expect("opening event_568_events.jsonl");

    let t0 = Instant::now();
    let hard_deadline = t0 + LOGIN_DEADLINE + NO_EVENT_GRACE + PLAYBACK_DEADLINE;
    let mut tally = Tally::default();
    let mut move_sent = false;

    let stop_reason: Option<String> = loop {
        if Instant::now() >= hard_deadline {
            break Some("hard deadline reached".into());
        }
        match timeout(Duration::from_millis(250), event_rx.recv()).await {
            Ok(Ok(ev)) => {
                let now = Instant::now();
                handle_event(&mut tally, &ev, now);

                let event_value = serde_json::to_value(&ev).expect("serializing AgentEvent");
                let line = serde_json::json!({
                    "t_ms": now.duration_since(t0).as_millis(),
                    "event": event_value,
                });
                writeln!(events_log, "{line}").expect("writing event_568_events.jsonl");

                // The trigger only checks once the ZoningIn window has passed
                // (vendor/server/scripts/globals/player.lua onGameIn), so the
                // walk-in can go out as soon as the zone is up; the POS lands
                // and the 200 ms interval picks it up after that.
                if !move_sent && tally.inzone_at.is_some() {
                    move_sent = true;
                    tally.move_sent_at = Some(now);
                    eprintln!(
                        "[live] Move to ({MOVE_TARGET_X}, {MOVE_TARGET_Y}, {MOVE_TARGET_Z}) \
                         heading {MOVE_HEADING} at t+{:.1}s",
                        now.elapsed().as_secs_f32()
                    );
                    let _ = cmd_tx
                        .send(AgentCommand::Move {
                            x: MOVE_TARGET_X,
                            y: MOVE_TARGET_Y,
                            z: MOVE_TARGET_Z,
                            heading: MOVE_HEADING,
                        })
                        .await;
                }

                if let AgentEvent::EventDialog { dialog } = &ev {
                    if dialog.event_para == EVENT_568 {
                        // The 568 script carries no 0x24 QUERY frames
                        // (research/XiEvents/OpCodes/0x0024.md); choice 0
                        // dismisses the single message frame.
                        eprintln!("[live] frame {}: dismissing", tally.frames_total);
                        let _ = cmd_tx
                            .send(AgentCommand::EndEventChoice {
                                event_id: dialog.event_id,
                                act_index: dialog.act_index,
                                event_num: dialog.event_num,
                                choice: 0,
                            })
                            .await;
                    }
                }

                if tally.disconnected_reason.is_some() {
                    break Some("session disconnected".into());
                }

                if tally.event_ended_at.is_some() {
                    break Some("event 568 ended".into());
                }

                if let Some(started_at) = tally.cutscene_started_at {
                    if now - started_at > PLAYBACK_DEADLINE {
                        break Some("playback deadline exceeded".into());
                    }
                } else if let Some(inzone_at) = tally.inzone_at {
                    if now - inzone_at > NO_EVENT_GRACE {
                        break Some("event 568 never started after zone-in".into());
                    }
                }
            }
            Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                eprintln!("[live] event stream lagged, dropped {n} events");
            }
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                break Some("event stream closed".into());
            }
            Err(_) => continue,
        }
    };

    if tally.disconnected_reason.is_none() {
        let _ = cmd_tx.send(AgentCommand::Disconnect).await;
    }
    drop(cmd_tx);

    let stop_reason = stop_reason.unwrap_or_else(|| "unknown (loop fell through)".into());

    match timeout(Duration::from_secs(10), session_task).await {
        Ok(Ok(Ok(()))) => eprintln!("[live] session task ended cleanly"),
        Ok(Ok(Err(e))) => eprintln!("[live] session task returned Err: {e:#}"),
        Ok(Err(join_err)) => eprintln!("[live] session task panicked: {join_err}"),
        Err(_) => eprintln!(
            "[live] session task did not finish within 10s after disconnect \
             (last stages: {:?})",
            tally.stages_seen
        ),
    }

    if let Err(e) = fixture.cleanup().await {
        eprintln!("fixture cleanup failed (non-fatal for this test): {e:#}");
    }

    write_summary(&out_dir, &tally, &stop_reason);

    assert!(
        tally.stages_seen.contains(&Stage::InZone),
        "session never reached InZone (stages: {:?}, stop: {stop_reason})",
        tally.stages_seen,
    );
    assert!(
        tally.auto_skipped_line.is_none(),
        "event 568 auto-skipped instead of playing: {:?}",
        tally.auto_skipped_line
    );
    assert!(
        tally.cutscene_started_at.is_some(),
        "CutsceneStarted for event 568 never observed (stop: {stop_reason}, \
         frames so far: {:?})",
        &tally.frame_texts[..tally.frame_texts.len().min(8)]
    );
    assert!(
        tally.frames_total >= MIN_DIALOG_FRAMES,
        "no message frame observed — the 0x47 case-1 hold never released, so the \
         0x05C -> 0x052 round trip did not complete (stop: {stop_reason})"
    );
    assert!(
        tally.event_ended_at.is_some(),
        "event 568 never ended (stop: {stop_reason}, frames: {})",
        tally.frames_total
    );
    assert!(
        tally.disconnected_reason.is_none(),
        "session disconnected: {:?}",
        tally.disconnected_reason
    );

    eprintln!(
        "[live] PASS: event 568 played end to end — InZone at {:?}, Move at {:?}, \
         CutsceneStarted at {:?}, {} frame(s), EventEnded at {:?} — the 0x47/0x05C \
         onEventUpdate round trip completed",
        secs_opt(tally.inzone_at),
        secs_opt(tally.move_sent_at),
        secs_opt(tally.cutscene_started_at),
        tally.frames_total,
        secs_opt(tally.event_ended_at),
    );
}

fn secs_opt(t: Option<Instant>) -> String {
    match t {
        Some(i) => format!("t+{:.1}s", i.elapsed().as_secs_f32()),
        None => "n/a".into(),
    }
}

fn write_summary(out_dir: &Path, tally: &Tally, stop_reason: &str) {
    let summary_path = out_dir.join("event_568_summary.txt");
    let mut s = String::new();
    let secs = |i: Option<Instant>| match i {
        Some(t) => format!("{:.1}s", t.elapsed().as_secs_f32()),
        None => "n/a".into(),
    };
    push_line(&mut s, &format!("stop reason: {stop_reason}"));
    push_line(&mut s, &format!("stages: {:?}", tally.stages_seen));
    push_line(
        &mut s,
        &format!(
            "InZone at {}, Move at {}, CutsceneStarted at {}, EventEnded at {}",
            secs(tally.inzone_at),
            secs(tally.move_sent_at),
            secs(tally.cutscene_started_at),
            secs(tally.event_ended_at)
        ),
    );
    if let Some(a) = &tally.auto_skipped_line {
        push_line(&mut s, &format!("AUTO-SKIP LINE: {a}"));
    }
    push_line(&mut s, &format!("frames={}", tally.frames_total));
    for (i, f) in tally.frame_texts.iter().enumerate() {
        push_line(&mut s, &format!("frame {i}: {f}"));
    }
    if let Some(r) = &tally.disconnected_reason {
        push_line(&mut s, &format!("disconnected: {r}"));
    }
    fs::write(&summary_path, s).expect("writing event_568_summary.txt");
    eprintln!("[live] summary written to {}", summary_path.display());
}

fn push_line(s: &mut String, line: &str) {
    s.push_str(line);
    s.push('\n');
}

async fn is_reachable(host: &str, port: u16) -> bool {
    timeout(Duration::from_millis(750), TcpStream::connect((host, port)))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}
