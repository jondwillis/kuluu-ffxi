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
        let sec = t.frame as f32 / 30.0;
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
