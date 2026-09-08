//! Temporary probe: dump a model DAT's schedulers (stages), SE schedule, and
//! sp0?/sp1? clip frame extents. Usage: dat-burrow-dump <path-to.dat>

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::action::extract_se_schedule;
use ffxi_dat::datid::DatId;
use ffxi_dat::scheduler::{Scheduler, StageKind};
use ffxi_dat::skel_anim;
use ffxi_dat::{walk, ChunkKind};

fn main() -> ExitCode {
    let path = match env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: dat-burrow-dump <path-to.dat>");
            return ExitCode::from(2);
        }
    };
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("read {path}: {e}");
            return ExitCode::from(1);
        }
    };

    println!("== SE schedule (Scheduler->Generator/Sep chain) ==");
    for t in extract_se_schedule(&bytes) {
        let sec = t.frame as f32 / 60.0;
        println!(
            "  frame {:>4} (t={sec:5.2}s)  se_id={:<6} {}  scheduler={}",
            t.frame,
            t.se_id,
            if t.on_caster { "caster" } else { "target" },
            std::str::from_utf8(&t.scheduler).unwrap_or("????")
        );
    }

    println!();
    println!("== schedulers ==");
    for c in walk(&bytes) {
        let Ok(c) = c else { continue };
        if ChunkKind::from_u8(c.kind) != Some(ChunkKind::Scheduler) {
            continue;
        }
        let name = c.name_str();
        let Ok(s) = Scheduler::parse(c.name, c.data) else {
            println!("{name}: PARSE FAILED");
            continue;
        };
        println!("-- {name} ({} stages)", s.stages.len());
        for t in &s.stages {
            let st = &t.stage;
            let id = std::str::from_utf8(&st.id).unwrap_or("????");
            match st.kind {
                StageKind::Motion => println!(
                    "   f{:>4}  Motion      id={id} delay={} dur={} loops={} tin={} tout={}",
                    t.frame,
                    st.delay_frames,
                    st.duration_frames,
                    st.max_loops,
                    st.transition_in,
                    st.transition_out
                ),
                StageKind::SoundOnCaster
                | StageKind::SoundOnTarget
                | StageKind::SoundNonPositional => {
                    println!(
                        "   f{:>4}  {:?} id={id} delay={} dur={}",
                        t.frame, st.kind, st.delay_frames, st.duration_frames
                    )
                }
                StageKind::Particle => println!(
                    "   f{:>4}  Particle    id={id} delay={} dur={}",
                    t.frame, st.delay_frames, st.duration_frames
                ),
                StageKind::StopParticle => println!(
                    "   f{:>4}  StopParticle id={id} delay={} dur={}",
                    t.frame, st.delay_frames, st.duration_frames
                ),
                StageKind::SubRoutine
                | StageKind::BlockingSubRoutine
                | StageKind::SubRoutineOnTarget => {
                    println!(
                        "   f{:>4}  {:?} id={id} delay={} dur={}",
                        t.frame, st.kind, st.delay_frames, st.duration_frames
                    )
                }
                _ => println!(
                    "   f{:>4}  {:?}(0x{:<2X}) id={id} delay={} dur={}",
                    t.frame, st.kind, st.raw_type, st.delay_frames, st.duration_frames
                ),
            }
        }
    }

    println!();
    println!("== clips sp0?/sp1?/idl? ==");
    for c in walk(&bytes) {
        let Ok(c) = c else { continue };
        if ChunkKind::from_u8(c.kind) != Some(ChunkKind::AnimMo2) {
            continue;
        }
        let name = c.name_str();
        if !matches!(name.as_str(), "sp00" | "sp10" | "idl0") {
            continue;
        }
        let id = DatId::from_name(&c.name);
        let a = skel_anim::parse(id, c.data);
        println!(
            "-- {} joints={} frames={} kf_dur={:.4} length_in_frames={:.1}",
            name,
            a.num_joints,
            a.num_frames,
            a.key_frame_duration,
            a.length_in_frames()
        );
        // Compare frame 0 vs last frame per joint; report the movers.
        let mut movers: Vec<(u32, f32)> = Vec::new();
        for (&joint, frames) in &a.key_frame_sets {
            if frames.len() < 2 {
                continue;
            }
            let first = &frames[0];
            let last = frames.last().unwrap();
            let dy = (last.translation[1] - first.translation[1]).abs();
            let dxz = ((last.translation[0] - first.translation[0]).powi(2)
                + (last.translation[2] - first.translation[2]).powi(2))
            .sqrt();
            movers.push((joint, dy.max(dxz)));
        }
        movers.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (joint, delta) in movers.into_iter().take(8) {
            let f0 = a.get_joint_transform(joint, 0.0).unwrap();
            let fl = a.get_joint_transform(joint, a.length_in_frames()).unwrap();
            println!(
                "     joint {joint:>3}: delta={delta:9.2}  f0=({:7.2},{:7.2},{:7.2}) last=({:7.2},{:7.2},{:7.2})",
                f0.translation[0], f0.translation[1], f0.translation[2],
                fl.translation[0], fl.translation[1], fl.translation[2]
            );
        }
    }

    ExitCode::SUCCESS
}
