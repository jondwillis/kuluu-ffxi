//! Reconnaissance for FFXI sky/weather DAT chunk types.
//!
//! Public RE projects (POLUtils, xi-tinkerer, AltanaViewer, atom0s repos)
//! don't document the binary format of FFXI's sky cubemaps or weather
//! particle assets. This tool walks every zone DAT and produces:
//!
//!   1. A per-zone histogram of chunk kinds (known vs. unknown).
//!   2. A cross-zone summary: for each kind byte, the number of zones
//!      it appears in and the body-size statistics.
//!   3. A focused list of UNKNOWN chunk kinds (anything not in
//!      `ChunkKind::from_u8`) with first-32-byte hex previews for the
//!      first occurrence per kind — those are the candidates for
//!      sky/weather visuals.
//!
//! Body-size signatures of interest (per Track A's empirical-RE hints):
//!   - DXT1 FOURCC `'DXT1'` (0x31_54_58_44 little-endian, or "DXT1"
//!     in ascii) anywhere near the chunk start.
//!   - Size divisible by 6 (cubemap = 6 faces of equal size).
//!
//! Usage:
//!   cargo run -p ffxi-dat --release --example dat-sky-recon
//!   cargo run -p ffxi-dat --release --example dat-sky-recon -- --zone 100
//!
//! With `--zone N`, dumps every chunk in that zone's DAT verbosely.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::zone_dat::ZONE_DAT_TABLE;
use ffxi_dat::{walk, ChunkKind, DatRoot};

#[derive(Default, Clone, Copy, Debug)]
struct KindStats {
    occurrences: u32,
    zones_present: u32, // counted once per zone via the per-zone set
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
    // ASCII "DXT1" anywhere in the first 64 bytes — DDS headers and
    // some FFXI texture wrappers put the FOURCC near the start, not at
    // a fixed offset.
    let n = buf.len().min(64);
    buf[..n].windows(4).any(|w| w == b"DXT1")
}

fn divisible_by_six(body_len: usize) -> bool {
    body_len != 0 && body_len % 6 == 0
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
    // First-occurrence hex preview, only for unknown kinds, to ground
    // pattern recognition.
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
        // Per-zone set to count zones_present correctly (multiple
        // occurrences in one zone shouldn't inflate the "appears in N
        // zones" stat).
        let mut seen_in_zone = std::collections::BTreeSet::<u8>::new();
        for c in walk(&bytes).filter_map(Result::ok) {
            let entry = stats.entry(c.kind).or_default();
            entry.observe(c.data.len());
            if seen_in_zone.insert(c.kind) {
                entry.zones_present += 1;
            }
            // Record first sighting (across all zones) of an unknown
            // kind, with its hex preview. Helps eyeball structure.
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

    run_survey(&root)
}
