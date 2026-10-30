use std::process::ExitCode;

use ffxi_dat::{resource_dir::ResourceDir, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: dat-routine-stages <file_id> <routine-name-prefix>");
        return ExitCode::from(2);
    }
    let Ok(file_id) = args[0].parse::<u32>() else {
        return ExitCode::from(2);
    };
    let prefix = args[1].as_bytes();
    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("could not open DAT root: {e}");
            return ExitCode::from(1);
        }
    };
    let Ok(loc) = root.resolve(file_id) else {
        eprintln!("{file_id}: unresolvable");
        return ExitCode::from(1);
    };
    let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
        return ExitCode::from(1);
    };
    let dir = ResourceDir::from_bytes(bytes);
    for sched in dir.collect_schedulers() {
        if !sched.name.starts_with(prefix) {
            continue;
        }
        println!("routine {}", String::from_utf8_lossy(&sched.name));
        for t in &sched.stages {
            if t.stage.kind == ffxi_dat::scheduler::StageKind::Unknown {
                println!(
                    "   frame {:>4} kind={:?} op=0x{:02X} words={} id_hex={:02X?} dur={}",
                    t.frame,
                    t.stage.kind,
                    t.stage.raw_type,
                    t.stage.stage_words,
                    t.stage.id,
                    t.stage.duration_frames,
                );
                continue;
            }
            println!(
                "   frame {:>4} kind={:?} op=0x{:02X} id={} dur={} loops={:?}",
                t.frame,
                t.stage.kind,
                t.stage.raw_type,
                String::from_utf8_lossy(&t.stage.id),
                t.stage.duration_frames,
                t.stage.max_loops,
            );
        }
    }
    ExitCode::SUCCESS
}
