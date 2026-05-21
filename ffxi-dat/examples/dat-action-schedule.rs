//! Resolve a single action DAT to its full SE timeline.
//!
//! ```text
//! cargo run --release -p ffxi-dat --example dat-action-schedule -- <dat>
//! ```
//!
//! Prints every `(frame, se_id, caster|target, scheduler)` the
//! action will fire when played. For Fire (ROM/11/17.DAT) this
//! should show SE 3032 at one or more frames.

use std::path::PathBuf;
use std::process::ExitCode;

use ffxi_dat::action::extract_se_schedule;

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: dat-action-schedule <dat>");
        return ExitCode::from(1);
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("read {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };
    let schedule = extract_se_schedule(&bytes);
    if schedule.is_empty() {
        eprintln!("no SE schedule resolved — file has no Scheduler→Generator→Sep chain");
        return ExitCode::from(0);
    }
    println!("─── {} ───", path.display());
    for t in &schedule {
        let sec = t.frame as f32 / 30.0; // FFXI scheduler frames are 30 fps
        println!(
            "  frame {:>4} (t={:.2}s)  se_id={:<6} {}  scheduler={:?}",
            t.frame,
            sec,
            t.se_id,
            if t.on_caster { "caster" } else { "target" },
            std::str::from_utf8(&t.scheduler).unwrap_or("????"),
        );
    }
    eprintln!("{} SE event(s)", schedule.len());
    ExitCode::SUCCESS
}
