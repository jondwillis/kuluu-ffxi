//! Scan a DAT file (or a whole ROM range) for Sep + Scheduler
//! chunks and print the (chunk_name, se_id) pairs and stage
//! timelines. Verifies the runtime path that the audio plugin's
//! follow-up SFX system will use to schedule SEs.
//!
//! ```text
//! cargo run -p ffxi-dat --example dat-scan-sounds -- <file_or_dir>
//! ```
//!
//! Pass a single `.DAT` path, or a ROM directory and it'll glob.

use std::path::PathBuf;
use std::process::ExitCode;

use ffxi_dat::{
    chunk::walk,
    kind::ChunkKind,
    scheduler::{Scheduler, StageKind},
    sep::Sep,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: dat-scan-sounds <file_or_dir>");
        return ExitCode::from(1);
    }
    let target = PathBuf::from(&args[0]);
    let files: Vec<PathBuf> = if target.is_file() {
        vec![target]
    } else if target.is_dir() {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&target) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_file()
                    && p.extension()
                        .and_then(|s| s.to_str())
                        .map(|s| s.eq_ignore_ascii_case("dat"))
                        .unwrap_or(false)
                {
                    out.push(p);
                }
            }
        }
        out
    } else {
        eprintln!("not a file or directory: {}", target.display());
        return ExitCode::from(2);
    };

    let mut total_sep = 0;
    let mut total_sched = 0;
    let mut total_sound_events = 0;
    for path in &files {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("read {}: {e}", path.display());
                continue;
            }
        };
        let mut seps: Vec<Sep> = Vec::new();
        let mut schedulers: Vec<Scheduler> = Vec::new();
        for c in walk(&bytes) {
            let Ok(c) = c else { continue };
            match ChunkKind::from_u8(c.kind) {
                Some(ChunkKind::Sep) => {
                    if let Ok(s) = Sep::parse(c.name, c.data) {
                        seps.push(s);
                    }
                }
                Some(ChunkKind::Scheduler) => {
                    if let Ok(s) = Scheduler::parse(c.name, c.data) {
                        schedulers.push(s);
                    }
                }
                _ => {}
            }
        }
        if seps.is_empty() && schedulers.is_empty() {
            continue;
        }
        println!("─── {} ───", path.display());
        for s in &seps {
            let (dir, file) = s.relative_path();
            println!(
                "  Sep {:?}: se_id={} → sound/win/se/{}/{}",
                std::str::from_utf8(&s.name).unwrap_or("????"),
                s.se_id,
                dir,
                file
            );
        }
        for sched in &schedulers {
            let stages = sched.stages.len();
            let sound_count = sched
                .stages
                .iter()
                .filter(|t| {
                    matches!(
                        t.stage.kind,
                        StageKind::SoundOnCaster | StageKind::SoundOnTarget
                    )
                })
                .count();
            println!(
                "  Scheduler {:?}: {} stages, {} sound events",
                std::str::from_utf8(&sched.name).unwrap_or("????"),
                stages,
                sound_count
            );
            for ev in sched.sound_events() {
                println!(
                    "    frame {:>4}: {} ← {:?}",
                    ev.frame,
                    if ev.on_caster { "caster" } else { "target" },
                    std::str::from_utf8(&ev.id).unwrap_or("????")
                );
                total_sound_events += 1;
            }
        }
        total_sep += seps.len();
        total_sched += schedulers.len();
    }
    eprintln!(
        "scanned {} files: {} Sep chunks, {} Schedulers, {} sound events",
        files.len(),
        total_sep,
        total_sched,
        total_sound_events
    );
    ExitCode::SUCCESS
}
