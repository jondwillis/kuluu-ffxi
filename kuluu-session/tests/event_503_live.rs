// Live end-to-end playback of event 503, the Southern San d'Oria new-character
// opening cutscene, against a local LSB stack with retail DATs mounted.
//
// Self-skips when the auth port or xidb is unreachable, or when no FFXI install
// can be opened. Proves every instruction of event 503 fires start to end:
// camera/fade/gesture cues in authored order, all input-gated frames on the
// G1 "adventuring" -> G4a "helping people" path, the H2 coupon line, then the
// server-side onEventFinish rewards — item 536 and setPos to the gate
// (vendor/server/scripts/quests/hiddenQuests/New_Character_Cutscenes.lua
// SOUTHERN_SAN_DORIA).

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
    state::{AgentCommand, AgentEvent, DialogState, InventoryUpdate, Stage},
};
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc},
    time::timeout,
};

use common::EphemeralChar;

// Raw event id of the SSD new-character cutscene (vendor/server/scripts/quests/
// hiddenQuests/New_Character_Cutscenes.lua SOUTHERN_SAN_DORIA onZoneIn).
const EVENT_503: u16 = 503;
/// xi.item.ADVENTURER_COUPON, granted by onEventFinish[503]
/// (vendor/server/scripts/quests/hiddenQuests/New_Character_Cutscenes.lua).
const COUPON_ITEM_NO: u16 = 536;
/// The char_vars row that arms the trigger (quest var 'notSeen' of hidden
/// quest newCharacterCS,
/// vendor/server/scripts/quests/hiddenQuests/New_Character_Cutscenes.lua).
const NOT_SEEN_VAR: &str = "HQuest[newCharacterCS]notSeen";

/// onEventFinish[503] setPos(-100, 1, -40, 224)
/// (vendor/server/scripts/quests/hiddenQuests/New_Character_Cutscenes.lua):
/// native (x=-100, y=+1, z=-40) arrives as wire Position pos = (-100, -40, +1);
/// the wire .y is horizontal Z and .z is vertical.
const GATE_WIRE_X: f32 = -100.0;
const GATE_WIRE_Y: f32 = -40.0;
const GATE_WIRE_Z: f32 = 1.0;
const GATE_TOLERANCE: f32 = 0.5;

/// A full playback must take at least this long from CutsceneStarted to end:
/// the authored holds alone exceed it by a wide margin (an 8 s WAIT plus every
/// camera/fade routine hold at its DAT-authored length). An event ending faster
/// than this means the holds did not run — the skip-to-end failure mode.
const MIN_PLAYBACK_SECS: f32 = 60.0;

/// The G1 -> "adventuring" -> G4a path carries ~45 input-gated frames in the
/// authored script; allow for lines that merge into one frame.
const MIN_DIALOG_FRAMES: u32 = 25;

const LOGIN_DEADLINE: Duration = Duration::from_secs(90);
const PLAYBACK_DEADLINE: Duration = Duration::from_secs(8 * 60);
/// After the event ends, onEventFinish rewards (item grant + setPos) arrive
/// as ordinary s2c traffic; allow this long before giving up on them.
const SERVER_RESPONSE_GRACE: Duration = Duration::from_secs(30);
/// The zone-in event fires on entry; no start within a minute of InZone means
/// the trigger did not fire and waiting longer changes nothing.
const NO_EVENT_GRACE: Duration = Duration::from_secs(60);

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
    cutscene_started_at: Option<Instant>,
    event_ended_at: Option<Instant>,
    auto_skipped_line: Option<String>,
    frames_total: u32,
    menu_frames: u32,
    frame_texts: Vec<String>,
    choices_made: Vec<(u32, String)>,
    coupon_line_seen: bool,
    map_opened: bool,
    marker_label: Option<String>,
    map_closed: bool,
    camera_lock_take: bool,
    camera_lock_release: bool,
    scheduler_cues: Vec<(u32, String)>,
    gesture_keys: Vec<String>,
    actor_moves: u32,
    item_536_slot: Option<u8>,
    forced_move_target: Option<[f32; 3]>,
    disconnected_reason: Option<String>,
}

fn fourcc_to_string(cc: [u8; 4]) -> String {
    String::from_utf8_lossy(&cc).into_owned()
}

/// G1 main menu: take the "adventuring" branch so the nested G4a menu runs;
/// G4a sub-menu: "Helping people". Text frames (no choices) dismiss with 0.
fn pick_choice(dialog: &DialogState) -> u32 {
    for (i, label) in dialog.choices.iter().enumerate() {
        if label.contains("adventuring") {
            return i as u32;
        }
    }
    for (i, label) in dialog.choices.iter().enumerate() {
        if label.to_lowercase().contains("helping people") {
            return i as u32;
        }
    }
    0
}

/// Tallys the 503 run. The end is gated on the cutscene having started:
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
            if *event_id & 65535 == u32::from(EVENT_503) && tally.cutscene_started_at.is_none() {
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
            if dialog.event_para != EVENT_503 {
                return;
            }
            tally.frames_total += 1;
            let is_menu = !dialog.choices.is_empty();
            if is_menu {
                tally.menu_frames += 1;
            }
            let prompt = dialog.prompt.clone().unwrap_or_default();
            if prompt.contains("Ailevia") {
                tally.coupon_line_seen = true;
            }
            let snippet: String = prompt.chars().take(90).collect();
            tally.frame_texts.push(if is_menu {
                format!("[menu] {} | choices={:?}", snippet, dialog.choices)
            } else {
                format!("[text] {snippet}")
            });
        }
        AgentEvent::CutsceneCue { cue } => match cue {
            kuluu_session::state::CutsceneCue::CameraLock { lock } => {
                if *lock {
                    tally.camera_lock_take = true;
                } else {
                    tally.camera_lock_release = true;
                }
            }
            kuluu_session::state::CutsceneCue::Scheduler { dat_id, tag, .. } => {
                tally.scheduler_cues.push((*dat_id, fourcc_to_string(*tag)));
            }
            kuluu_session::state::CutsceneCue::ExtScheduler { motion, key, .. } => {
                // The 0x66 LOADEXTSCHEDULER2 gesture cues
                // (research/XiEvents/OpCodes/0x0066.md): container A is 32732.
                if matches!(
                    motion,
                    Some(kuluu_snapshot::ExtSchedulerMotion::Tpc { a: 32_732, .. })
                ) {
                    tally.gesture_keys.push(fourcc_to_string(*key));
                }
            }
            kuluu_session::state::CutsceneCue::ActorMove { .. } => {
                tally.actor_moves += 1;
            }
            _ => {}
        },
        AgentEvent::MapOpen { .. } => tally.map_opened = true,
        AgentEvent::MapMarkerPlaced { label, .. } => {
            tally.marker_label = Some(label.clone());
        }
        AgentEvent::MapClosed => tally.map_closed = true,
        AgentEvent::InventoryUpdated {
            update: InventoryUpdate::SlotChanged { slot },
            ..
        } => {
            if slot.item_no == COUPON_ITEM_NO && tally.item_536_slot.is_none() {
                tally.item_536_slot = Some(slot.index);
                eprintln!(
                    "[live] item {COUPON_ITEM_NO} granted to inventory slot {} at t+{:.1}s",
                    slot.index,
                    now.elapsed().as_secs_f32()
                );
            }
        }
        AgentEvent::ForcedMove { target, .. } => {
            if tally.forced_move_target.is_none() {
                tally.forced_move_target = Some([target.pos.x, target.pos.y, target.pos.z]);
                eprintln!(
                    "[live] ForcedMove to ({:.1}, {:.1}, {:.1}) at t+{:.1}s",
                    target.pos.x,
                    target.pos.y,
                    target.pos.z,
                    now.elapsed().as_secs_f32()
                );
            }
        }
        AgentEvent::Disconnected { reason } => {
            tally.disconnected_reason = Some(reason.clone());
        }
        _ => {}
    }
}

#[tokio::test]
async fn event_503_full_playback_against_live_lsb() {
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
             `kuluu install`, or set FFXI_DAT_PATH); without DATs the holds cannot arm and this \
             test would only re-prove the skip-to-end failure mode"
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

    let Some(fixture) = EphemeralChar::create(&server_host, auth_port)
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
        "fixture: user={} accid={} charid={} charname={}",
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
    let events_path = out_dir.join("events.jsonl");
    let mut events_log = fs::File::create(&events_path).expect("opening events.jsonl");

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
                writeln!(events_log, "{line}").expect("writing events.jsonl");

                if let AgentEvent::EventDialog { dialog } = &ev {
                    if dialog.event_para == EVENT_503 {
                        let choice = pick_choice(dialog);
                        let label = dialog
                            .choices
                            .get(choice as usize)
                            .cloned()
                            .unwrap_or_else(|| "<dismiss>".into());
                        eprintln!(
                            "[live] frame {}: choice {choice} ({label:?})",
                            tally.frames_total
                        );
                        tally.choices_made.push((tally.frames_total, label));
                        let _ = cmd_tx
                            .send(AgentCommand::EndEventChoice {
                                event_id: dialog.event_id,
                                act_index: dialog.act_index,
                                event_num: dialog.event_num,
                                choice,
                            })
                            .await;
                    }
                }

                if tally.disconnected_reason.is_some() {
                    break Some("session disconnected".into());
                }

                if let Some(ended_at) = tally.event_ended_at {
                    if tally.item_536_slot.is_some() && tally.forced_move_target.is_some() {
                        break Some("event ended and both server rewards observed".into());
                    }
                    if now - ended_at > SERVER_RESPONSE_GRACE {
                        break Some("server-response grace expired after event end".into());
                    }
                } else if let Some(started_at) = tally.cutscene_started_at {
                    if now - started_at > PLAYBACK_DEADLINE {
                        break Some("playback deadline exceeded".into());
                    }
                } else if let Some(inzone_at) = tally.inzone_at {
                    if now - inzone_at > NO_EVENT_GRACE {
                        break Some("event 503 never started after zone-in".into());
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
        "event 503 auto-skipped instead of playing: {:?}",
        tally.auto_skipped_line
    );
    let started_at = tally.cutscene_started_at.unwrap_or_else(|| {
        panic!(
            "CutsceneStarted for event 503 never observed (stop: {stop_reason}, \
             frames so far: {:?})",
            &tally.frame_texts[..tally.frame_texts.len().min(8)]
        )
    });
    assert!(
        tally.event_ended_at.is_some(),
        "event 503 never ended (stop: {stop_reason}, frames: {})",
        tally.frames_total
    );
    let ended_at = tally.event_ended_at.unwrap();
    let playback_secs = ended_at.duration_since(started_at).as_secs_f32();
    assert!(
        playback_secs >= MIN_PLAYBACK_SECS,
        "playback took {playback_secs:.1}s < {MIN_PLAYBACK_SECS}s: the DAT-armed \
         holds did not run (skip-to-end failure mode)"
    );
    assert!(
        tally.frames_total >= MIN_DIALOG_FRAMES,
        "only {} input-gated frames observed, expected >= {MIN_DIALOG_FRAMES} on the \
         G1->adventuring->G4a path; frames: {:?}",
        tally.frames_total,
        &tally.frame_texts[..tally.frame_texts.len().min(12)]
    );
    assert!(
        tally.menu_frames >= 2,
        "expected both the G1 and nested G4a menus, saw {} menu frame(s); choices: {:?}",
        tally.menu_frames,
        tally.choices_made
    );
    let chosen: Vec<&str> = tally.choices_made.iter().map(|(_, l)| l.as_str()).collect();
    assert!(
        chosen.iter().any(|l| l.contains("adventuring")),
        "G1 menu did not take the 'adventuring' branch; choices made: {chosen:?}"
    );
    assert!(
        chosen
            .iter()
            .any(|l| l.to_lowercase().contains("helping people")),
        "nested G4a menu did not take 'Helping people'; choices made: {chosen:?}"
    );
    assert!(
        tally.coupon_line_seen,
        "no H2 coupon line mentioning Ailevia observed; frames: {:?}",
        &tally.frame_texts[..tally.frame_texts.len().min(12)]
    );
    assert!(
        tally.map_opened && tally.marker_label.as_deref() == Some("Ailevia") && tally.map_closed,
        "H1/H3 map sequence incomplete (open={}, marker={:?}, closed={})",
        tally.map_opened,
        tally.marker_label,
        tally.map_closed
    );
    assert!(
        tally.camera_lock_take && tally.camera_lock_release,
        "A4/H7 camera take/release missing (take={}, release={})",
        tally.camera_lock_take,
        tally.camera_lock_release
    );
    assert!(
        tally
            .scheduler_cues
            .iter()
            .any(|(dat, tag)| *dat == 30834 && tag == "s043"),
        "B1 camera routine s043 (DAT 30834) never fired; scheduler cues: {:?}",
        tally.scheduler_cues
    );
    assert!(
        tally.gesture_keys.iter().any(|k| k == "tlk0")
            && tally.gesture_keys.iter().any(|k| k == "thk1"),
        "D-beat gestures tlk0/thk1 (Tpc container A 32732) missing; keys: {:?}",
        tally.gesture_keys
    );
    assert!(
        tally.item_536_slot.is_some(),
        "onEventFinish never granted item {COUPON_ITEM_NO} (stop: {stop_reason})"
    );
    let target = tally
        .forced_move_target
        .unwrap_or_else(|| panic!("no ForcedMove observed after event end (stop: {stop_reason})"));
    assert!(
        (target[0] - GATE_WIRE_X).abs() <= GATE_TOLERANCE
            && (target[1] - GATE_WIRE_Y).abs() <= GATE_TOLERANCE
            && (target[2] - GATE_WIRE_Z).abs() <= GATE_TOLERANCE,
        "ForcedMove target {target:?} is not the gate setPos (-100, -40, +1 wire)"
    );

    eprintln!(
        "[live] PASS: event 503 played end to end in {playback_secs:.1}s — {} frames \
         ({} menus), {} scheduler cues, {} gestures, item 536 -> slot {:?}, gate move {:?}",
        tally.frames_total,
        tally.menu_frames,
        tally.scheduler_cues.len(),
        tally.gesture_keys.len(),
        tally.item_536_slot,
        target
    );
}

fn write_summary(out_dir: &Path, tally: &Tally, stop_reason: &str) {
    let summary_path = out_dir.join("event_503_summary.txt");
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
            "InZone at {}, CutsceneStarted at {}, EventEnded at {}",
            secs(tally.inzone_at),
            secs(tally.cutscene_started_at),
            secs(tally.event_ended_at)
        ),
    );
    if let Some(a) = &tally.auto_skipped_line {
        push_line(&mut s, &format!("AUTO-SKIP LINE: {a}"));
    }
    push_line(
        &mut s,
        &format!(
            "frames={} menus={} choices={:?}",
            tally.frames_total, tally.menu_frames, tally.choices_made
        ),
    );
    for (i, f) in tally.frame_texts.iter().enumerate() {
        push_line(&mut s, &format!("frame {i}: {f}"));
    }
    push_line(
        &mut s,
        &format!(
            "camera lock take={} release={}",
            tally.camera_lock_take, tally.camera_lock_release
        ),
    );
    for (dat, tag) in &tally.scheduler_cues {
        push_line(&mut s, &format!("scheduler cue: dat {dat} tag {tag}"));
    }
    push_line(
        &mut s,
        &format!(
            "gestures(32732): {:?}; actor moves: {}",
            tally.gesture_keys, tally.actor_moves
        ),
    );
    push_line(
        &mut s,
        &format!(
            "map open={} marker={:?} closed={}",
            tally.map_opened, tally.marker_label, tally.map_closed
        ),
    );
    push_line(
        &mut s,
        &format!(
            "item 536 slot: {:?}; forced move target: {:?}",
            tally.item_536_slot, tally.forced_move_target
        ),
    );
    if let Some(r) = &tally.disconnected_reason {
        push_line(&mut s, &format!("disconnected: {r}"));
    }
    fs::write(&summary_path, s).expect("writing event_503_summary.txt");
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
