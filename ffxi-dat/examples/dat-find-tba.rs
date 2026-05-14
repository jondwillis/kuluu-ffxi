//! Scan all DAT files in the FFXI install for chunks named "t_ba" (or
//! variants), to locate sibling files that may carry MMB placement
//! data for zone 235 (Bastok Markets, MZB file_id 335).
//!
//! Usage:
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-find-tba --release

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;

use ffxi_dat::{walk, DatRoot};

fn main() {
    let root = DatRoot::from_env().expect("FFXI_DAT_PATH");
    let needles: &[&[u8]] = &[b"t_ba"];

    let mut hits: BTreeMap<String, Vec<(u8, String, usize)>> = BTreeMap::new();
    let mut file_count = 0usize;
    let mut chunk_count_total = 0usize;

    for entry in walkdir(root.root()) {
        if !entry
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.ends_with(".DAT"))
            .unwrap_or(false)
        {
            continue;
        }
        // Skip FTABLE/VTABLE
        let name = entry.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with("FTABLE") || name.starts_with("VTABLE") {
            continue;
        }
        file_count += 1;
        let bytes = match fs::read(&entry) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let chunks: Vec<_> = match walk(&bytes).collect::<Result<Vec<_>, _>>() {
            Ok(c) => c,
            Err(_) => continue,
        };
        chunk_count_total += chunks.len();
        for c in &chunks {
            let cname = c.name_str();
            let cbytes = cname.as_bytes();
            for needle in needles {
                if cbytes.starts_with(needle) {
                    let key = entry.to_string_lossy().to_string();
                    hits.entry(key).or_default().push((c.kind, cname.clone(), c.data.len()));
                    break;
                }
            }
        }
    }

    println!("scanned {file_count} DAT files, {chunk_count_total} chunks");
    println!("files with chunk name starting 't_ba': {}", hits.len());
    for (path, chunks) in &hits {
        println!();
        println!("{path}");
        for (k, n, sz) in chunks {
            println!("    kind=0x{k:02X} name={n:?} body_len={sz}");
        }
    }
}

fn walkdir(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = match fs::read_dir(&d) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}
