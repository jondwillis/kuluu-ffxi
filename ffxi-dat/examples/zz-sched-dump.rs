//! Resolve one DAT file id exactly as the event VM does (`DatRoot::resolve`
//! plus the install's overlay search), list its chunks by kind, parse requested
//! scheduler tags, and dump parsed camera-route (kind 0x06) chunks. A scheduler
//! `CameraRoute` stage names one of these fourcc routes; comparing the authored
//! points across zones is how "the angles are wrong" gets settled from bytes.
//!
//! `cargo run -q --release -p ffxi-dat --example zz-sched-dump -- <dat_id> [--all-cameras] [tag ...]`

use std::process::ExitCode;

use ffxi_dat::{camera::CameraResource, chunk, kind::ChunkKind, scheduler::Scheduler, DatRoot};

fn fourcc_str(name: &[u8; 4]) -> String {
    name.iter()
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '.'
            }
        })
        .collect()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(id_arg) = args.first() else {
        eprintln!("usage: zz-sched-dump <dat_id> [--all-cameras] [tag ...]");
        return ExitCode::from(2);
    };
    let Ok(dat_id) = id_arg.parse::<u32>() else {
        return fail("dat_id must be numeric");
    };
    let all_cameras = args.iter().any(|a| a == "--all-cameras");
    let wanted: Vec<[u8; 4]> = args[1..]
        .iter()
        .filter(|t| !t.starts_with("--"))
        .filter_map(|t| t.as_bytes().try_into().ok())
        .collect();

    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => return fail(&format!("dat root: {e}")),
    };
    let loc = match root.resolve(dat_id) {
        Ok(l) => l,
        Err(e) => return fail(&format!("resolve({dat_id}): {e}")),
    };
    let path = loc.path_under(&root);
    println!("dat_id {dat_id} -> {}", path.display());
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => return fail(&format!("read: {e}")),
    };

    let mut cams = Vec::new();
    let mut scheds = Vec::new();
    for c in chunk::walk(&bytes) {
        match c {
            Ok(c) if c.kind == ChunkKind::Camera as u8 => cams.push(c),
            Ok(c) if c.kind == ChunkKind::Scheduler as u8 => scheds.push(c),
            Ok(_) => {}
            Err(e) => println!("  chunk walk error: {e}"),
        }
    }
    let cam_names: Vec<String> = cams.iter().map(|c| fourcc_str(&c.name)).collect();
    let sched_names: Vec<String> = scheds.iter().map(|c| fourcc_str(&c.name)).collect();
    println!("camera chunks ({}): {}", cams.len(), cam_names.join(" "));
    println!(
        "scheduler chunks ({}): {}",
        scheds.len(),
        sched_names.join(" ")
    );

    for c in &scheds {
        if wanted.is_empty() || !wanted.contains(&c.name) {
            continue;
        }
        print!("--- scheduler '{}' parse: ", fourcc_str(&c.name));
        let s = match Scheduler::parse(c.name, c.data) {
            Ok(s) => s,
            Err(e) => {
                println!("Err({e})");
                continue;
            }
        };
        println!("{} stages end_frame={}", s.stages.len(), s.end_frame());
        for t in &s.stages {
            let st = &t.stage;
            let mut extra = String::new();
            if let Some(m) = &st.model_transform {
                extra.push_str(&format!(" transform={m:?}"));
            }
            if let Some(f) = &st.follow_points {
                extra.push_str(&format!(" follow={f:?}"));
            }
            if let Some(r) = st.actor_rotation {
                extra.push_str(&format!(" rot={r:?}"));
            }
            println!(
                "  frame={:<5} {:?} raw={:#04x} words={} delay={} dur={} id='{}'{extra}",
                t.frame,
                st.kind,
                st.raw_type,
                st.stage_words,
                st.delay_frames,
                st.duration_frames,
                fourcc_str(&st.id),
            );
        }
    }

    let dump_cam = |c: &ffxi_dat::chunk::Chunk<'_>| {
        print!("--- camera '{}' parse: ", fourcc_str(&c.name));
        match CameraResource::parse(c.name, c.data) {
            Ok(r) => {
                println!(
                    "attach={} interp={} flags={:#06x?} smooth={:?} points={}",
                    r.attachment_info,
                    r.interp_factor,
                    r.flags,
                    r.smoothing,
                    r.points.len()
                );
                for p in &r.points {
                    println!(
                        "    t={:<8.4} pos=[{:9.3},{:8.3},{:9.3}] tgt=[{:9.3},{:8.3},{:9.3}] focal={:>8.3} roll={:.3}",
                        p.param[0],
                        p.position[0],
                        p.position[1],
                        p.position[2],
                        p.target[0],
                        p.target[1],
                        p.target[2],
                        p.focal_length,
                        p.roll
                    );
                }
            }
            Err(e) => println!("Err({e})"),
        }
    };
    if all_cameras {
        for c in &cams {
            dump_cam(c);
        }
    } else {
        for c in &cams {
            if wanted.contains(&c.name) {
                dump_cam(c);
            }
        }
    }
    ExitCode::SUCCESS
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("{msg}");
    ExitCode::from(1)
}
