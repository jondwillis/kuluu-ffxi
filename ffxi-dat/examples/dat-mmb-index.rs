use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::mmb::{self, MmbHeader, MmbSubRecord};
use ffxi_dat::{walk, ChunkKind, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let (start, end) = match args.len() {
        2 => match args[1].parse::<u32>() {
            Ok(s) => (s, s + 1000),
            Err(_) => return usage(&args[0]),
        },
        3 => match (args[1].parse::<u32>(), args[2].parse::<u32>()) {
            (Ok(s), Ok(e)) => (s, e),
            _ => return usage(&args[0]),
        },
        _ => return usage(&args[0]),
    };

    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("DatRoot::from_env_or_default: {e}");
            return ExitCode::from(1);
        }
    };

    eprintln!("scanning file_ids {start}..{end} for MMB chunks");
    println!("file_id\tchunk_idx\tasset_name\tnum_sub_records\tchunk_name");

    let mut scanned = 0u32;
    let mut mmb_count = 0u32;
    for file_id in start..end {
        scanned += 1;
        let location = match root.resolve(file_id) {
            Ok(l) => l,
            Err(_) => continue,
        };
        let path = location.path_under(root.root());
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("file_id={file_id}: read {}: {e}", path.display());
                continue;
            }
        };
        for (chunk_idx, chunk_res) in walk(&bytes).enumerate() {
            let chunk = match chunk_res {
                Ok(c) => c,
                Err(_) => continue,
            };
            if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::Mmb) {
                continue;
            }
            let decrypted = match mmb::decrypt(chunk.data) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let header = match MmbHeader::parse(&decrypted) {
                Ok(h) => h,
                Err(_) => continue,
            };
            let subs = MmbSubRecord::find_all(header.payload);

            let asset = header.asset_name_str().replace(['\t', '\n', '\r'], " ");
            let chunk_name = chunk.name_str().replace(['\t', '\n', '\r'], " ");
            println!(
                "{file_id}\t{chunk_idx}\t{asset}\t{}\t{chunk_name}",
                subs.len()
            );
            mmb_count += 1;
        }
    }
    eprintln!("done: scanned {scanned} file_ids, found {mmb_count} MMB chunks");
    ExitCode::SUCCESS
}

fn usage(arg0: &str) -> ExitCode {
    eprintln!("usage: FFXI_DAT_PATH=... {arg0} <start_file_id> [end_file_id]");
    ExitCode::from(2)
}
