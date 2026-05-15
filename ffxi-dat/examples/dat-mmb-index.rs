//! Build a `(file_id, chunk_idx, asset_name)` index over a range of DATs.
//! Bootstrap aid for the modelid → MMB file_id mapping: pair this output
//! with `/look` chat readouts to hand-correlate look bytes to assets.
//!
//! Output: TSV on stdout, one row per MMB chunk found.
//!   file_id<TAB>chunk_idx<TAB>asset_name<TAB>num_sub_records<TAB>chunk_name
//!
//! Errors are written to stderr but do not abort the scan. A file_id
//! that isn't claimed by any APPID is skipped silently.
//!
//! Usage:
//!   cargo run -p ffxi-dat --example dat-mmb-index -- <start> [end]
//!   # default end = start + 1000 (one batch). Whole-corpus scan needs
//!   # ~125 000 file_ids — slow but tractable.

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
            Err(_) => continue, // unclaimed file_id — common, not an error
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
            // Strip control bytes from names for TSV safety.
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
