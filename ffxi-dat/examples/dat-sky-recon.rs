use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::zone_dat::ZONE_DAT_TABLE;
use ffxi_dat::{walk, ChunkKind, DatRoot};

#[derive(Default, Clone, Copy, Debug)]
struct KindStats {
    occurrences: u32,
    zones_present: u32,
    min_body: usize,
    max_body: usize,
    sum_body: u64,
}

impl KindStats {
    fn observe(&mut self, body_len: usize) {
        if self.occurrences == 0 {
            self.min_body = body_len;
            self.max_body = body_len;
        } else {
            self.min_body = self.min_body.min(body_len);
            self.max_body = self.max_body.max(body_len);
        }
        self.occurrences += 1;
        self.sum_body += body_len as u64;
    }
}

fn looks_like_dxt1(buf: &[u8]) -> bool {
    let n = buf.len().min(64);
    buf[..n].windows(4).any(|w| w == b"DXT1")
}

fn divisible_by_six(body_len: usize) -> bool {
    body_len != 0 && body_len.is_multiple_of(6)
}

fn hex_preview(buf: &[u8], n: usize) -> String {
    let take = buf.len().min(n);
    buf[..take]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn dump_one_zone(root: &DatRoot, zone_id: u16, file_id: u32) {
    let Ok(loc) = root.resolve(file_id) else {
        eprintln!("zone {zone_id} (file {file_id}): unresolved");
        return;
    };
    let bytes = match fs::read(loc.path_under(root.root())) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("zone {zone_id}: read error: {e}");
            return;
        }
    };
    println!("# zone {zone_id} → file {file_id}  ({} bytes)", bytes.len());
    for (i, c) in walk(&bytes).filter_map(Result::ok).enumerate() {
        let name = ChunkKind::from_u8(c.kind)
            .map(|k| format!("{k:?}"))
            .unwrap_or_else(|| "?".into());
        let flags = {
            let mut f = String::new();
            if divisible_by_six(c.data.len()) {
                f.push_str(" [body%6=0]");
            }
            if looks_like_dxt1(c.data) {
                f.push_str(" [DXT1]");
            }
            f
        };
        println!(
            "  chunk[{i:>3}] kind=0x{:02x} ({name:<10}) body={:>8}{}  {}",
            c.kind,
            c.data.len(),
            flags,
            hex_preview(c.data, 24)
        );
    }
}

fn run_survey(root: &DatRoot) -> ExitCode {
    let mut stats: BTreeMap<u8, KindStats> = BTreeMap::new();

    let mut first_unknown_preview: BTreeMap<u8, (u16, String)> = BTreeMap::new();
    let mut zone_count = 0;
    let mut unreadable = 0;

    for &(zone_id, file_id) in ZONE_DAT_TABLE {
        let loc = match root.resolve(file_id) {
            Ok(l) => l,
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        let bytes = match fs::read(loc.path_under(root.root())) {
            Ok(b) => b,
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        zone_count += 1;

        let mut seen_in_zone = std::collections::BTreeSet::<u8>::new();
        for c in walk(&bytes).filter_map(Result::ok) {
            let entry = stats.entry(c.kind).or_default();
            entry.observe(c.data.len());
            if seen_in_zone.insert(c.kind) {
                entry.zones_present += 1;
            }

            if ChunkKind::from_u8(c.kind).is_none() {
                first_unknown_preview
                    .entry(c.kind)
                    .or_insert_with(|| (zone_id, hex_preview(c.data, 32)));
            }
        }
    }

    println!("== survey complete ==");
    println!("zones scanned:    {zone_count}");
    println!("zones unreadable: {unreadable}");
    println!();
    println!(
        "{:<6} {:<10} {:>10} {:>10} {:>12} {:>12} {:>14}",
        "kind", "name", "occurs", "in_zones", "min_body", "max_body", "avg_body"
    );
    for (kind, s) in &stats {
        let name = ChunkKind::from_u8(*kind)
            .map(|k| format!("{k:?}"))
            .unwrap_or_else(|| "?".into());
        let avg = if s.occurrences == 0 {
            0
        } else {
            s.sum_body / s.occurrences as u64
        };
        println!(
            "0x{:02x}   {:<10} {:>10} {:>10} {:>12} {:>12} {:>14}",
            kind, name, s.occurrences, s.zones_present, s.min_body, s.max_body, avg
        );
    }

    if !first_unknown_preview.is_empty() {
        println!();
        println!("== unknown kinds (candidates for sky/weather) ==");
        for (kind, (zone_id, preview)) in &first_unknown_preview {
            let s = &stats[kind];
            println!(
                "0x{:02x}  occurs={:<5} in_zones={:<4} body=[{}..{}]  first@zone{}: {}",
                kind, s.occurrences, s.zones_present, s.min_body, s.max_body, zone_id, preview
            );
        }
    }

    ExitCode::SUCCESS
}

fn scan_range(root: &DatRoot, lo: u32, hi: u32) {
    let zone_set: std::collections::BTreeSet<u32> =
        ZONE_DAT_TABLE.iter().map(|(_, fid)| *fid).collect();
    let mut hits = 0;
    let mut scanned = 0;
    let mut readable = 0;
    println!("# scan_range [{lo}..={hi}] (excluding ZONE_DAT_TABLE file_ids)");
    for fid in lo..=hi {
        if zone_set.contains(&fid) {
            continue;
        }
        scanned += 1;
        let Ok(loc) = root.resolve(fid) else { continue };
        let bytes = match fs::read(loc.path_under(root.root())) {
            Ok(b) => b,
            Err(_) => continue,
        };
        readable += 1;
        for (i, c) in walk(&bytes).filter_map(Result::ok).enumerate() {
            let body = c.data;
            let dxt = looks_like_dxt1(body);
            let cube = divisible_by_six(body.len()) && body.len() >= 4096;
            if dxt || cube {
                hits += 1;
                let name = ChunkKind::from_u8(c.kind)
                    .map(|k| format!("{k:?}"))
                    .unwrap_or_else(|| "?".into());
                let mut flags = String::new();
                if dxt {
                    flags.push_str(" [DXT1]");
                }
                if cube {
                    flags.push_str(" [body%6=0]");
                }
                println!(
                    "  file {fid:>6} chunk[{i:>3}] kind=0x{:02x} ({name:<10}) body={:>8}{}  {}",
                    c.kind,
                    body.len(),
                    flags,
                    hex_preview(body, 24)
                );
            }
        }
    }
    println!("# scan_range done: scanned={scanned} readable={readable} hits={hits}");
}

fn main() -> ExitCode {
    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("DAT root unavailable: {e}");
            return ExitCode::from(2);
        }
    };

    let args: Vec<String> = env::args().collect();
    if args.len() >= 3 && args[1] == "--zone" {
        let zone_id: u16 = match args[2].parse() {
            Ok(z) => z,
            Err(_) => {
                eprintln!("invalid zone id: {}", args[2]);
                return ExitCode::from(2);
            }
        };
        let Some(file_id) = ffxi_dat::zone_dat::zone_id_to_mzb_file_id(zone_id) else {
            eprintln!("zone {zone_id} not in ZONE_DAT_TABLE");
            return ExitCode::from(2);
        };
        dump_one_zone(&root, zone_id, file_id);
        return ExitCode::SUCCESS;
    }
    if args.len() >= 3 && args[1] == "--file" {
        let fid: u32 = match args[2].parse() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("invalid file id: {}", args[2]);
                return ExitCode::from(2);
            }
        };
        dump_one_zone(&root, 0, fid);
        return ExitCode::SUCCESS;
    }
    if args.len() >= 4 && args[1] == "--scan" {
        let lo: u32 = args[2].parse().unwrap_or(0);
        let hi: u32 = args[3].parse().unwrap_or(lo);
        scan_range(&root, lo, hi);
        return ExitCode::SUCCESS;
    }

    run_survey(&root)
}
