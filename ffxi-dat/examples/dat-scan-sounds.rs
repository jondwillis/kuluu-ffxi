//! Scan a DAT file (or a whole ROM tree recursively) for Sep +
//! Scheduler chunks and print the (chunk_name, se_id) pairs and
//! stage timelines. Verifies the runtime path that the audio
//! plugin's follow-up SFX system will use to schedule SEs.
//!
//! ```text
//! cargo run --release -p ffxi-dat --example dat-scan-sounds -- <file_or_dir> [--summary] [--filter <chunk_name>]
//! ```
//!
//! - `--summary`: skip per-chunk listing, print only the totals
//!   (much faster on large trees because no stdout flush per chunk).
//! - `--filter <name>`: only print schedulers whose 4-char name
//!   matches (e.g. `s001`). Useful when hunting for one spell.
//!
//! Pass a single `.DAT` path or a directory; directories are walked
//! recursively. Parsing runs in parallel via rayon (one job per
//! file), which on a typical ROM tree (~100k DATs) saturates the
//! CPU and finishes in seconds instead of minutes.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use rayon::prelude::*;

use ffxi_dat::{
    chunk::walk,
    kind::ChunkKind,
    scheduler::{Scheduler, StageKind},
    sep::Sep,
};

#[derive(Default)]
struct FileReport {
    path: PathBuf,
    seps: Vec<Sep>,
    schedulers: Vec<Scheduler>,
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let summary_mode = args.iter().position(|a| a == "--summary").map(|i| {
        args.remove(i);
        true
    }).unwrap_or(false);
    let filter = args
        .iter()
        .position(|a| a == "--filter")
        .and_then(|i| {
            args.remove(i);
            (i < args.len()).then(|| args.remove(i))
        });

    if args.is_empty() {
        eprintln!("usage: dat-scan-sounds <file_or_dir> [--summary] [--filter <chunk_name>]");
        return ExitCode::from(1);
    }
    let target = PathBuf::from(&args[0]);

    // Collect every .DAT path. Recursive walk so passing a parent
    // dir (e.g. the install root) finds everything without the
    // caller writing a shell loop per ROM subdirectory.
    let mut files: Vec<PathBuf> = Vec::new();
    if target.is_file() {
        files.push(target);
    } else if target.is_dir() {
        collect_dats(&target, &mut files);
    } else {
        eprintln!("not a file or directory: {}", target.display());
        return ExitCode::from(2);
    }

    eprintln!("scanning {} files…", files.len());

    // Parse in parallel. Each `FileReport` is independent so rayon
    // can saturate cores. The collected vec is then sorted by path
    // for deterministic output regardless of work-stealing order.
    let mut reports: Vec<FileReport> = files
        .par_iter()
        .filter_map(|path| {
            let bytes = std::fs::read(path).ok()?;
            let mut report = FileReport {
                path: path.clone(),
                ..Default::default()
            };
            for c in walk(&bytes) {
                let Ok(c) = c else { continue };
                match ChunkKind::from_u8(c.kind) {
                    Some(ChunkKind::Sep) => {
                        if let Ok(s) = Sep::parse(c.name, c.data) {
                            report.seps.push(s);
                        }
                    }
                    Some(ChunkKind::Scheduler) => {
                        if let Ok(s) = Scheduler::parse(c.name, c.data) {
                            report.schedulers.push(s);
                        }
                    }
                    _ => {}
                }
            }
            (!report.seps.is_empty() || !report.schedulers.is_empty()).then_some(report)
        })
        .collect();
    reports.sort_by(|a, b| a.path.cmp(&b.path));

    let total_sep = AtomicUsize::new(0);
    let total_sched = AtomicUsize::new(0);
    let total_sound_events = AtomicUsize::new(0);
    let out = Mutex::new(std::io::stdout().lock());

    for report in &reports {
        let mut sound_events_in_file = 0usize;
        for sched in &report.schedulers {
            sound_events_in_file += sched.sound_events().count();
        }
        total_sep.fetch_add(report.seps.len(), Ordering::Relaxed);
        total_sched.fetch_add(report.schedulers.len(), Ordering::Relaxed);
        total_sound_events.fetch_add(sound_events_in_file, Ordering::Relaxed);

        if summary_mode {
            continue;
        }

        // Filter pass: if `--filter` was given, only print the file
        // if at least one scheduler matches the requested name.
        if let Some(want) = &filter {
            let any_match = report
                .schedulers
                .iter()
                .any(|s| std::str::from_utf8(&s.name).ok() == Some(want.as_str()));
            if !any_match {
                continue;
            }
        }

        let mut out = out.lock().unwrap();
        use std::io::Write;
        let _ = writeln!(out, "─── {} ───", report.path.display());
        for s in &report.seps {
            let (dir, file) = s.relative_path();
            let _ = writeln!(
                out,
                "  Sep {:?}: se_id={} → sound/win/se/{}/{}",
                std::str::from_utf8(&s.name).unwrap_or("????"),
                s.se_id,
                dir,
                file
            );
        }
        for sched in &report.schedulers {
            let filtered = filter
                .as_ref()
                .map(|want| std::str::from_utf8(&sched.name).ok() == Some(want.as_str()))
                .unwrap_or(false);
            if filter.is_some() && !filtered {
                continue;
            }
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
            let _ = writeln!(
                out,
                "  Scheduler {:?}: {} stages, {} sound events",
                std::str::from_utf8(&sched.name).unwrap_or("????"),
                sched.stages.len(),
                sound_count
            );
            for ev in sched.sound_events() {
                let _ = writeln!(
                    out,
                    "    frame {:>4}: {} ← {:?}",
                    ev.frame,
                    if ev.on_caster { "caster" } else { "target" },
                    std::str::from_utf8(&ev.id).unwrap_or("????")
                );
            }
            // When a filter is active, also dump every stage of the
            // matching scheduler — the user is probably hunting for
            // a non-sound stage (Generator, Motion) that references
            // a sibling chunk. Without this, `--filter main` was
            // showing only counts, which hid the real timeline.
            if filtered {
                for t in &sched.stages {
                    let _ = writeln!(
                        out,
                        "    frame {:>4} type=0x{:02x} dur={:>4} id={:?}",
                        t.frame,
                        t.stage.raw_type,
                        t.stage.duration_frames,
                        std::str::from_utf8(&t.stage.id).unwrap_or("????")
                    );
                }
            }
        }
    }

    eprintln!(
        "scanned {} files: {} Sep chunks, {} Schedulers, {} sound events",
        files.len(),
        total_sep.load(Ordering::Relaxed),
        total_sched.load(Ordering::Relaxed),
        total_sound_events.load(Ordering::Relaxed),
    );
    ExitCode::SUCCESS
}

fn collect_dats(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_dats(&p, out);
        } else if p
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("dat"))
            .unwrap_or(false)
        {
            out.push(p);
        }
    }
}
