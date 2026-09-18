// Live end-to-end check of event 531, the Windurst Waters new-character
// opening cutscene, against a local LSB stack with retail DATs mounted.
//
// Self-skips when the auth port or xidb is unreachable, or when no FFXI
// install can be opened. 531 is a multi-entity event: ROM/21/47.DAT holds
// five exact owners (the 163-step main program, a [HIDE_SELF, END] block,
// and three more NPC blocks). The master (zone) block carries only a
// wildcard END, so retail runs every owner block in parallel from event
// start (per-entity event instances, research/XiEvents/Event VM Functions.md
// InitEvent2/XiEventInit). Kuluu mirrors that: the session spawns an
// owner-block child per non-master owner (ffxi-event EventVm::spawn_owner,
// kuluu-session event_dialog begin) and the event ends when they all drain.
//
// This test asserts the full playback: CutsceneStarted, at least one staging
// frame (CutsceneCue), and EventEnded.

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

// Windurst Waters zone id, the onZoneIn key of the 531 new-character
// cutscene trigger (vendor/server/scripts/quests/hiddenQuests/
// New_Character_Cutscenes.lua WINDURST_WATERS onZoneIn).
const WINDURST_WATERS_ZONE: u32 = 238;
// The 531 event id, low 16 bits of the agent event id.
const EVENT_531: u16 = 531;
// The char_vars row that arms the trigger (quest var 'notSeen' of hidden quest
// newCharacterCS).
const NOT_SEEN_VAR: &str = "HQuest[newCharacterCS]notSeen";

const LOGIN_DEADLINE: Duration = Duration::from_secs(90);
// The 531 cutscene plays for a while; two minutes from CutsceneStarted is
// generous.
const PLAYBACK_DEADLINE: Duration = Duration::from_secs(120);
// Grace for the event to start after zone-in.
const NO_EVENT_GRACE: Duration = Duration::from_secs(60);

fn open_dat_root() -> Option<ffxi_dat::DatRoot> {
    match ffxi_dat::DatRoot::from_env_or_default() {
        Ok(root) => Some(root),
        Err(e) => {
            eprintln!("no dat root: {e:#}");
            None
        }
    }
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
    cutscene_started_at: Option<Instant>,
    event_ended_at: Option<Instant>,
    // Staging frames the running 531 script emitted (CutsceneCue).
    cues_total: u32,
    auto_skipped_line: Option<String>,
    disconnected_reason: Option<String>,
}

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
            if *event_id & 0xFFFF == u32::from(EVENT_531) && tally.cutscene_started_at.is_none() {
                tally.cutscene_started_at = Some(now);
                eprintln!(
                    "[live] CutsceneStarted (agent id 0x{event_id:08X}) at t+{:.1}s",
                    now.elapsed().as_secs_f32()
                );
            }
        }
        AgentEvent::CutsceneCue { .. } => {
            // Count staging frames only after the 531 cutscene has started, so
            // unrelated cues around zone-in do not inflate the tally.
            if tally.cutscene_started_at.is_some() {
                tally.cues_total += 1;
            }
        }
        AgentEvent::EventEnded => {
            // EventEnded is a unit variant carrying no event id, so gate on the
            // 531 cutscene having started to ignore any unrelated event that
            // ends around zone-in. First end after start is the one we tally.
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
        AgentEvent::Disconnected { reason } => {
            tally.disconnected_reason = Some(reason.clone());
        }
        _ => {}
    }
}

#[tokio::test]
async fn event_531_full_playback_against_live_lsb() {
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
            "skipping: no FFXI install found (set FFXI_DAT_PATH, or install under \
             vendor/game-files/); without DATs the VM cannot spawn the \
             531 owner blocks and this test cannot observe the playback"
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
        EphemeralChar::create_in_zone(&server_host, auth_port, WINDURST_WATERS_ZONE)
            .await
            .expect("provisioning ephemeral LSB account+char")
    else {
        eprintln!("skipping: xidb not reachable; treating the LSB stack as absent");
        return;
    };
    fixture
        .set_char_var(NOT_SEEN_VAR, 1)
        .await
        .expect("arming the new-character CS trigger in char_vars");
    eprintln!(
        "fixture: user={} accid={} charid={} charname={} zone={WINDURST_WATERS_ZONE}",
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
        dat_root: Some(Arc::new(dat_root)),
        user_driven_events: false,
    };

    let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>(32);
    let (event_tx, mut event_rx) = broadcast::channel::<AgentEvent>(512);

    let session_task = tokio::spawn(session::run(cfg, cmd_rx, event_tx));

    let out_dir = artifact_dir();
    fs::create_dir_all(&out_dir).expect("creating artifacts/verify");
    let events_path = out_dir.join("event_531_events.jsonl");
    let mut events_log = fs::File::create(&events_path).expect("opening event_531_events.jsonl");

    let t0 = Instant::now();
    let hard_deadline = t0 + LOGIN_DEADLINE + PLAYBACK_DEADLINE;
    let mut tally = Tally::default();

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
                writeln!(events_log, "{line}").expect("writing event_531_events.jsonl");

                if tally.disconnected_reason.is_some() {
                    break Some("session disconnected".into());
                }

                // Success: the 531 cutscene played end to end — it started,
                // emitted at least one staging frame, and ended.
                if tally.event_ended_at.is_some() && tally.cues_total >= 1 {
                    break Some("event 531 played (CutsceneStarted, frame, EventEnded)".into());
                }

                if let Some(started_at) = tally.cutscene_started_at {
                    if now - started_at > PLAYBACK_DEADLINE {
                        break Some("playback deadline exceeded".into());
                    }
                } else if let Some(inzone_at) = tally.inzone_at {
                    if now - inzone_at > NO_EVENT_GRACE {
                        break Some("event 531 never started after zone-in".into());
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
        "event 531 auto-skipped with an unimplemented-opcode line instead of \
         playing: {:?}",
        tally.auto_skipped_line
    );
    assert!(
        tally.cutscene_started_at.is_some(),
        "CutsceneStarted for event 531 never observed (stop: {stop_reason})"
    );
    assert!(
        tally.cues_total >= 1,
        "no staging frame observed — the 531 owner blocks never ran (stop: \
         {stop_reason})"
    );
    assert!(
        tally.event_ended_at.is_some(),
        "event 531 never ended (stop: {stop_reason}, frames: {})",
        tally.cues_total
    );
    assert!(
        tally.disconnected_reason.is_none(),
        "session disconnected: {:?}",
        tally.disconnected_reason
    );

    eprintln!(
        "[live] PASS: event 531 played end to end — InZone at {:?}, \
         CutsceneStarted at {:?}, {} frame(s), EventEnded at {:?} — the multi-\
         entity owner blocks ran in parallel",
        secs_opt(tally.inzone_at),
        secs_opt(tally.cutscene_started_at),
        tally.cues_total,
        secs_opt(tally.event_ended_at),
    );
}

fn secs_opt(i: Option<Instant>) -> String {
    match i {
        Some(t) => format!("{:.1}s", t.elapsed().as_secs_f32()),
        None => "n/a".into(),
    }
}

fn write_summary(out_dir: &Path, tally: &Tally, stop_reason: &str) {
    let summary_path = out_dir.join("event_531_summary.txt");
    let mut s = String::new();
    push_line(&mut s, &format!("stop reason: {stop_reason}"));
    push_line(&mut s, &format!("stages: {:?}", tally.stages_seen));
    push_line(&mut s, &format!("InZone at {}", secs_opt(tally.inzone_at)));
    push_line(
        &mut s,
        &format!("CutsceneStarted at {}", secs_opt(tally.cutscene_started_at)),
    );
    push_line(
        &mut s,
        &format!("EventEnded at {}", secs_opt(tally.event_ended_at)),
    );
    push_line(&mut s, &format!("cues_total: {}", tally.cues_total));
    if let Some(a) = &tally.auto_skipped_line {
        push_line(&mut s, &format!("AUTO-SKIP LINE: {a}"));
    }
    if let Some(r) = &tally.disconnected_reason {
        push_line(&mut s, &format!("disconnected: {r}"));
    }
    fs::write(&summary_path, s).expect("writing event_531_summary.txt");
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
