//! Validate the MO2 (`mot_`) animation parser against a real chunk.
//! Prints header values, per-bone keyframe counts, and a sampled
//! frame so we can sanity-check parse_mo2 produces unit quaternions
//! and sensible translation/scale.
//!
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-mo2-probe -- <file_id>

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::anim::parse_mo2;
use ffxi_dat::{walk, ChunkKind, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: FFXI_DAT_PATH=... {} <file_id>", args[0]);
        return ExitCode::from(2);
    }
    let file_id: u32 = args[1].parse().unwrap();
    let root = DatRoot::from_env().unwrap();
    let location = root.resolve(file_id).unwrap();
    let bytes = fs::read(location.path_under(root.root())).unwrap();

    let mut count = 0;
    for (idx, chunk) in walk(&bytes).enumerate() {
        let Ok(c) = chunk else { continue };
        if ChunkKind::from_u8(c.kind) != Some(ChunkKind::AnimMo2) {
            continue;
        }
        count += 1;
        let Ok(anim) = parse_mo2(c.data, &c.name) else {
            println!("[chunk {idx}] parse failed");
            continue;
        };
        let unit_q = anim
            .per_bone
            .values()
            .flat_map(|kfs| kfs.iter())
            .filter(|f| {
                let m = f.rotation[0].powi(2) + f.rotation[1].powi(2)
                    + f.rotation[2].powi(2) + f.rotation[3].powi(2);
                (m - 1.0).abs() < 0.05
            })
            .count();
        let total_q = anim
            .per_bone
            .values()
            .map(|kfs| kfs.len())
            .sum::<usize>();
        println!(
            "[chunk {idx}] name={:?} bones={} frames={} speed={:.4}  unit-quats={}/{}",
            anim.name,
            anim.per_bone.len(),
            anim.frames,
            anim.speed,
            unit_q,
            total_q,
        );
        if count <= 3 {
            // Sample frame 0 of the first bone.
            if let Some((&bone_id, frames)) = anim.per_bone.iter().next() {
                if let Some(f) = frames.first() {
                    println!(
                        "   bone {bone_id}.frame[0]: rot={:?} trans={:?} scale={:?}",
                        f.rotation, f.translation, f.scale
                    );
                }
            }
        }
    }
    println!("\ntotal AnimMo2 chunks: {count}");
    ExitCode::SUCCESS
}
