use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::time::Instant;

use ffxi_dat::mmb::{self, MmbHeader};
use ffxi_dat::{walk, DatRoot};

fn main() {
    let args: Vec<String> = env::args().collect();
    let mode = args.get(1).cloned().unwrap_or_else(|| "fs".to_string());

    let root = DatRoot::from_env().expect("FFXI_DAT_PATH not set / invalid");
    let t0 = Instant::now();

    let (index, files_scanned, files_with_mmb, total_mmb) = match mode.as_str() {
        "fs" => scan_fs(&root, &t0),
        "ids" => {
            let start: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            let end: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(130_000);
            scan_ids(&root, start, end, &t0)
        }
        other => panic!("unknown mode {other:?}; use fs or ids"),
    };

    let out_path = match mode.as_str() {
        "fs" => args
            .get(2)
            .cloned()
            .unwrap_or_else(|| "/tmp/mmb_index.json".to_string()),
        _ => args
            .get(4)
            .cloned()
            .unwrap_or_else(|| "/tmp/mmb_index.json".to_string()),
    };

    let unique_names = index.len();
    let duplicate_names = index.values().filter(|v| v.len() > 1).count();
    let elapsed = t0.elapsed().as_secs_f32();

    let mut f = fs::File::create(&out_path).expect("create output file");
    writeln!(f, "{{").unwrap();
    writeln!(
        f,
        "  \"stats\": {{ \"files_scanned\": {}, \"files_with_mmb\": {}, \"total_mmb\": {}, \"unique_names\": {}, \"duplicate_names\": {}, \"elapsed_secs\": {:.2} }},",
        files_scanned, files_with_mmb, total_mmb, unique_names, duplicate_names, elapsed
    )
    .unwrap();
    writeln!(f, "  \"index\": {{").unwrap();
    let mut first = true;
    for (name, locs) in &index {
        if !first {
            writeln!(f, ",").unwrap();
        } else {
            first = false;
        }
        write!(f, "    {}: [", json_str(name)).unwrap();
        let mut lfirst = true;
        for (fid, ci) in locs {
            if !lfirst {
                write!(f, ", ").unwrap();
            }
            lfirst = false;
            write!(f, "[{},{}]", fid, ci).unwrap();
        }
        write!(f, "]").unwrap();
    }
    writeln!(f, "\n  }}\n}}").unwrap();

    println!("scan complete in {:.2}s ({} mode)", elapsed, mode);
    println!("  files_scanned    = {}", files_scanned);
    println!("  files_with_mmb   = {}", files_with_mmb);
    println!("  total_mmb_chunks = {}", total_mmb);
    println!("  unique_names     = {}", unique_names);
    println!("  duplicate_names  = {}", duplicate_names);
    println!("output: {}", out_path);
}

type IndexMap = BTreeMap<String, Vec<(u32, u32)>>;

fn process_file(bytes: &[u8], file_id: u32, index: &mut IndexMap) -> (bool, u32) {
    let mut has_mmb_candidate = false;
    let mut i = 4usize;
    while i < bytes.len() {
        if (bytes[i] & 0x7F) == 0x2E {
            has_mmb_candidate = true;
            break;
        }
        i += 16;
    }
    if !has_mmb_candidate {
        return (false, 0);
    }
    let mut had_mmb = false;
    let mut added = 0u32;
    let mut chunk_idx: u32 = 0;
    for c in walk(bytes).flatten() {
        if c.kind != 0x2E {
            chunk_idx = chunk_idx.wrapping_add(1);
            continue;
        }
        had_mmb = true;
        if let Ok(d) = mmb::decrypt(c.data) {
            if let Ok(h) = MmbHeader::parse(&d) {
                let name = h.asset_name_str().trim_end().to_string();
                if !name.is_empty() {
                    index.entry(name).or_default().push((file_id, chunk_idx));
                    added += 1;
                }
            }
        }
        chunk_idx = chunk_idx.wrapping_add(1);
    }
    (had_mmb, added)
}

fn scan_ids(root: &DatRoot, start: u32, end: u32, t0: &Instant) -> (IndexMap, u64, u64, u64) {
    let mut index: IndexMap = BTreeMap::new();
    let mut files_scanned: u64 = 0;
    let mut files_with_mmb: u64 = 0;
    let mut total_mmb: u64 = 0;
    for file_id in start..=end {
        if (file_id % 5000) == 0 {
            eprintln!(
                "  ...at file_id={file_id}  scanned={files_scanned}  mmb_files={files_with_mmb}  total_mmb={total_mmb}  uniq={}  elapsed={:.1}s",
                index.len(),
                t0.elapsed().as_secs_f32()
            );
        }
        let Ok(loc) = root.resolve(file_id) else {
            continue;
        };
        let Ok(bytes) = fs::read(loc.path_under(root.root())) else {
            continue;
        };
        files_scanned += 1;
        let (had_mmb, added) = process_file(&bytes, file_id, &mut index);
        if had_mmb {
            files_with_mmb += 1;
        }
        total_mmb += added as u64;
    }
    (index, files_scanned, files_with_mmb, total_mmb)
}

fn scan_fs(root: &DatRoot, t0: &Instant) -> (IndexMap, u64, u64, u64) {
    let root_path = root.root().to_path_buf();
    let app_summary = root.app_summary();
    let mut files_scanned: u64 = 0;
    let mut files_with_mmb: u64 = 0;
    let mut total_mmb: u64 = 0;
    let mut index: IndexMap = BTreeMap::new();

    let mut last_log = Instant::now();

    for (rom_dir, _vlen, _flen) in &app_summary {
        let rom_path = root_path.join(rom_dir);
        let rom_idx: u32 = if rom_dir == "ROM" {
            1
        } else {
            rom_dir.trim_start_matches("ROM").parse().unwrap_or(0)
        };
        let Ok(dirs) = fs::read_dir(&rom_path) else {
            continue;
        };
        for entry in dirs.flatten() {
            let dir_path = entry.path();
            if !dir_path.is_dir() {
                continue;
            }
            let Some(dir_num) = dir_path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(files) = fs::read_dir(&dir_path) else {
                continue;
            };
            for fe in files.flatten() {
                let fp = fe.path();
                if fp.extension().and_then(|e| e.to_str()) != Some("DAT") {
                    continue;
                }
                let Some(file_num) = fp
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .and_then(|s| s.parse::<u32>().ok())
                else {
                    continue;
                };

                let synth_id = (rom_idx << 24) | ((dir_num & 0xFFF) << 12) | (file_num & 0xFFF);
                let Ok(bytes) = fs::read(&fp) else { continue };
                files_scanned += 1;
                let (had_mmb, added) = process_file(&bytes, synth_id, &mut index);
                if had_mmb {
                    files_with_mmb += 1;
                }
                total_mmb += added as u64;

                if last_log.elapsed().as_secs_f32() >= 5.0 {
                    eprintln!(
                        "  ...scanned={files_scanned}  mmb_files={files_with_mmb}  total_mmb={total_mmb}  uniq={}  elapsed={:.1}s",
                        index.len(),
                        t0.elapsed().as_secs_f32()
                    );
                    last_log = Instant::now();
                }
            }
        }
    }

    (index, files_scanned, files_with_mmb, total_mmb)
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
