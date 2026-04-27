use std::env;
use std::fs;
use std::process::ExitCode;
use std::time::Instant;

use ffxi_dat::{walk, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let start: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let end: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(130_000);
    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("DatRoot: {e}");
            return ExitCode::from(1);
        }
    };

    println!("file_id,chunk_idx,body_len,header_u32,bone_count_hi16");
    let t0 = Instant::now();
    let mut resolved = 0u64;
    let mut with_sk2 = 0u64;
    let mut total_sk2 = 0u64;
    for fid in start..=end {
        if fid % 5000 == 0 {
            eprintln!(
                "  ...fid={fid}  resolved={resolved}  with_sk2={with_sk2}  sk2_chunks={total_sk2}  elapsed={:.1}s",
                t0.elapsed().as_secs_f32()
            );
        }
        let Ok(loc) = root.resolve(fid) else { continue };
        let Ok(bytes) = fs::read(loc.path_under(root.root())) else {
            continue;
        };
        resolved += 1;

        let mut maybe = false;
        let mut i = 4usize;
        while i < bytes.len() {
            if (bytes[i] & 0x7F) == 0x29 {
                maybe = true;
                break;
            }
            i += 16;
        }
        if !maybe {
            continue;
        }

        let mut file_had_sk2 = false;
        for (ci, c) in walk(&bytes).filter_map(Result::ok).enumerate() {
            if c.kind != 0x29 {
                continue;
            }
            file_had_sk2 = true;
            total_sk2 += 1;
            let hdr = if c.data.len() >= 4 {
                u32::from_le_bytes(c.data[0..4].try_into().unwrap())
            } else {
                0
            };
            let count_hi = (hdr >> 16) as u16;
            println!("{fid},{ci},{},0x{hdr:08x},{count_hi}", c.data.len());
        }
        if file_had_sk2 {
            with_sk2 += 1;
        }
    }
    eprintln!(
        "\ndone: resolved={resolved}  files_with_sk2={with_sk2}  sk2_chunks={total_sk2}  elapsed={:.1}s",
        t0.elapsed().as_secs_f32()
    );
    ExitCode::SUCCESS
}
