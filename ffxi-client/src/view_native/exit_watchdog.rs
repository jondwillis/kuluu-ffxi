use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

#[derive(Clone, Copy)]
pub enum Stage {
    AppExitObserved = 1,
    AppRunReturned = 2,
    RuntimeDropped = 3,
    MainReturning = 4,
}

fn stage_name(v: u8) -> &'static str {
    match v {
        0 => "idle (no AppExit yet)",
        1 => "AppExit observed — Bevy/winit/wgpu teardown beginning",
        2 => "app.run() returned — winit loop exited cleanly",
        3 => "tokio runtime dropped",
        4 => "main returning — process should terminate now",
        _ => "unknown",
    }
}

static LAST_STAGE: AtomicU8 = AtomicU8::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
static COMPLETE: AtomicBool = AtomicBool::new(false);

pub fn mark(stage: Stage) {
    let v = stage as u8;
    LAST_STAGE.store(v, Ordering::SeqCst);
    tracing::info!(stage = stage_name(v), "exit-watchdog: teardown checkpoint");
}

pub fn note_complete() {
    COMPLETE.store(true, Ordering::SeqCst);
}

pub fn arm() {
    if ARMED.swap(true, Ordering::SeqCst) {
        return;
    }
    mark(Stage::AppExitObserved);

    let mode = std::env::var("FFXI_EXIT_WATCHDOG").unwrap_or_default();
    if mode == "off" {
        tracing::info!("exit-watchdog: disabled via FFXI_EXIT_WATCHDOG=off");
        return;
    }
    let abort_on_timeout = mode != "exit";
    let secs = std::env::var("FFXI_EXIT_WATCHDOG_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(8);

    let _ = std::thread::Builder::new()
        .name("exit-watchdog".into())
        .spawn(move || {
            let limit = Duration::from_secs(secs);
            let step = Duration::from_millis(100);
            let mut waited = Duration::ZERO;
            while waited < limit {
                if COMPLETE.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(step);
                waited += step;
            }
            if COMPLETE.load(Ordering::SeqCst) {
                return;
            }

            let stage = LAST_STAGE.load(Ordering::SeqCst);
            tracing::error!(
                stuck_after_secs = secs,
                last_stage = stage_name(stage),
                "exit-watchdog: process did not finish exiting; the main thread is wedged"
            );
            if abort_on_timeout {
                // SIGABRT (rather than a quiet exit) so the OS captures an all-thread
                // backtrace pinpointing the wedged call — macOS writes it to
                // ~/Library/Logs/DiagnosticReports/. FFXI_EXIT_WATCHDOG=exit for a
                // quiet exit, =off to disable.
                tracing::error!(
                    "exit-watchdog: aborting to capture a crash report \
                     (~/Library/Logs/DiagnosticReports/)"
                );
                std::process::abort();
            } else {
                std::process::exit(0);
            }
        });
}
