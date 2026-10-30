//! Per-clip motion amplitude of one model DAT: name, frames, joints, and the largest
//! consecutive-frame transform delta. Answers "does this clip move" offline.
//! `cargo run -p ffxi-dat --example zz-mo2-motion -- <file_id>`

use std::process::ExitCode;

use ffxi_dat::{resource_dir::ResourceDir, DatRoot};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(file_id) = args.next().and_then(|s| s.parse::<u32>().ok()) else {
        eprintln!("usage: zz-mo2-motion <file_id>");
        return ExitCode::from(2);
    };
    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("dat root: {e}");
            return ExitCode::from(1);
        }
    };
    let loc = match root.resolve(file_id) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("resolve {file_id}: {e}");
            return ExitCode::from(1);
        }
    };
    let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
        eprintln!("unreadable {}", loc.path_under(&root).display());
        return ExitCode::from(1);
    };
    println!("file_id={file_id} path={}", loc.path_under(&root).display());
    let dir = ResourceDir::from_bytes(bytes);
    for anim in dir.collect_animations() {
        let mut max_dt = 0.0f32;
        let mut max_dq = 0.0f32;
        let mut joints = 0usize;
        let mut set_len_mismatch = false;
        for frames in anim.key_frame_sets.values() {
            joints += 1;
            if frames.len() != anim.num_frames {
                set_len_mismatch = true;
            }
            for w in frames.windows(2) {
                let dt = (w[1].translation[0] - w[0].translation[0])
                    .abs()
                    .max((w[1].translation[1] - w[0].translation[1]).abs())
                    .max((w[1].translation[2] - w[0].translation[2]).abs());
                let dot = (w[1].rotation[0] * w[0].rotation[0]
                    + w[1].rotation[1] * w[0].rotation[1]
                    + w[1].rotation[2] * w[0].rotation[2]
                    + w[1].rotation[3] * w[0].rotation[3])
                    .abs()
                    .min(1.0);
                let dq = 1.0 - dot;
                if dt > max_dt {
                    max_dt = dt;
                }
                if dq > max_dq {
                    max_dq = dq;
                }
            }
        }
        println!(
            "clip {:>8} frames={:>3} joints={:>4} max_dtranslate={max_dt:.5} max_1minuscos={max_dq:.6}{}",
            anim.id.as_str(),
            anim.num_frames,
            joints,
            if set_len_mismatch { " [set-length mismatch]" } else { "" },
        );
    }
    ExitCode::SUCCESS
}
