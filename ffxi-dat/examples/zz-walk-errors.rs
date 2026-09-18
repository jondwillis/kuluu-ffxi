//! Walk-error census: every .DAT under the install root walked with the
//! retail 19-bit `ffxi_dat::walk`; the files that error are categorized by
//! where the error lands, what the error chunk's header says, and what the
//! clean prefix still carries — because an earlier census found 2,282 walk-error
//! files (4.3% of the install), and the open question was whether any of them hide
//! 0x2D scene-key carriers (scheduler chunks) or attached camera routes
//! (kind 0x06, AttachmentInfo nonzero).
//!
//! Census result (retail install; the exposure sections below are the close
//! criteria): 52,926 files, 2,282 walk errors (4.3%), all five known 0x2D
//! carriers and every zone model DAT walk clean, and no failing prefix carries
//! a resolvable 0x2D key or a kind-0x06 route with nonzero AttachmentInfo —
//! the two failing files with a scheduler chunk in the prefix carry garbage
//! names, not routine carriers. Nothing the client can reach is hidden in a
//! walk error, so the census closes without code.
//!
//! Usage: cargo run -p ffxi-dat --example zz-walk-errors -- <install root>
//! where the install root is the folder containing ROM/.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ffxi_dat::scheduler::NON_MODEL_SCENE_CARRIERS;
use ffxi_dat::{walk, DatError};

const KIND_CAMERA: u8 = 0x06;
const KIND_SCHEDULER: u8 = 0x07;

#[derive(Default)]
struct Prefix {
    #[allow(dead_code)]
    chunks: usize,
    #[allow(dead_code)]
    kinds: BTreeMap<u8, usize>,
    attached_routes: BTreeMap<u32, usize>,
    scheduler_names: Vec<String>,
}

struct FailRecord {
    rel: String,
    len: usize,
    offset: usize,
    needed: usize,
    available: usize,
    error_chunk_kind: Option<u8>,
    error_chunk_name: Option<String>,
    prefix: Prefix,
}

/// Walk to the first error; the chunks before it are the readable prefix.
fn walk_to_error(bytes: &[u8]) -> Option<(usize, usize, usize, Prefix)> {
    let mut prefix = Prefix::default();
    for c in walk(bytes) {
        match c {
            Ok(chunk) => {
                prefix.chunks += 1;
                *prefix.kinds.entry(chunk.kind).or_default() += 1;
                if chunk.kind == KIND_CAMERA {
                    if let Some(info) = chunk.data.get(0..4) {
                        let bytes: [u8; 4] = info.try_into().unwrap();
                        let attach = u32::from_le_bytes(bytes);
                        if attach != 0 {
                            *prefix.attached_routes.entry(attach).or_default() += 1;
                        }
                    }
                }
                if chunk.kind == KIND_SCHEDULER {
                    prefix.scheduler_names.push(chunk.name_str());
                }
            }
            Err(e) => {
                let (offset, needed, available) = match &e {
                    DatError::TruncatedChunk {
                        offset,
                        needed,
                        available,
                    } => (*offset, *needed, *available),
                    other => {
                        // The walk only yields TruncatedChunk; anything else is
                        // a scanner bug, not a file property.
                        panic!("unexpected walk error: {other}");
                    }
                };
                return Some((offset, needed, available, prefix));
            }
        }
    }
    None
}

/// The header at `offset`, if the file holds one: the retail kind + name of
/// the chunk whose body ran past the end.
fn header_at(bytes: &[u8], offset: usize) -> (Option<u8>, Option<String>) {
    let Some(header) = bytes.get(offset..offset + 16) else {
        return (None, None);
    };
    let value = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let kind = (value & 0x7F) as u8;
    let name = header
        .iter()
        .take(4)
        .map(|&b| if b == 0 { '.' } else { b as char })
        .collect();
    (Some(kind), Some(name))
}

fn position_class(offset: usize, len: usize) -> &'static str {
    if offset == 0 {
        return "first chunk";
    }
    let frac = offset as f64 / len as f64;
    if frac < 0.25 {
        "0..25%"
    } else if frac < 0.5 {
        "25..50%"
    } else if frac < 0.75 {
        "50..75%"
    } else {
        "75..100%"
    }
}

fn collect_dat_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_dat_files(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dat"))
        {
            out.push(path);
        }
    }
}

fn fmt_map(map: &BTreeMap<u32, usize>) -> String {
    map.iter()
        .map(|(v, n)| format!("0x{v:03X} x{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(install) = args.get(1) else {
        eprintln!("usage: zz-walk-errors <install root>");
        std::process::exit(2);
    };
    let root = Path::new(install);
    if !root.join("VTABLE.DAT").exists() {
        eprintln!("{install} has no VTABLE.DAT; not an install root");
        std::process::exit(2);
    }

    let mut files = Vec::new();
    collect_dat_files(root, &mut files);
    files.sort();

    let mut report = String::new();
    let mut push = |line: &str| {
        report.push_str(line);
        report.push('\n');
    };

    let mut fails: Vec<FailRecord> = Vec::new();
    let mut clean = 0usize;
    let mut unreadable = 0usize;
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            unreadable += 1;
            continue;
        };
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        match walk_to_error(&bytes) {
            None => clean += 1,
            Some((offset, needed, available, prefix)) => {
                let (error_chunk_kind, error_chunk_name) = header_at(&bytes, offset);
                fails.push(FailRecord {
                    rel,
                    len: bytes.len(),
                    offset,
                    needed,
                    available,
                    error_chunk_kind,
                    error_chunk_name,
                    prefix,
                });
            }
        }
    }
    fails.sort_by(|a, b| a.rel.cmp(&b.rel));

    push("== totals ==");
    push(&format!(
        "files: {}  walked clean: {}  walk error: {}  unreadable: {}",
        files.len(),
        clean,
        fails.len(),
        unreadable
    ));

    push("");
    push("== error position (offset of the truncated chunk / file size) ==");
    let mut by_position: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &fails {
        *by_position
            .entry(position_class(f.offset, f.len))
            .or_default() += 1;
    }
    for (k, n) in &by_position {
        push(&format!("{k}: {n}"));
    }

    push("");
    push("== error chunk kind (header at the error offset) ==");
    let mut by_kind: BTreeMap<Option<u8>, usize> = BTreeMap::new();
    for f in &fails {
        *by_kind.entry(f.error_chunk_kind).or_default() += 1;
    }
    for (k, n) in &by_kind {
        push(&format!(
            "{}: {n}",
            k.map(|k| format!("0x{k:02X}"))
                .unwrap_or_else(|| "<no header>".into())
        ));
    }

    push("");
    push("== truncation depth (needed - available, how far past EOF the body runs) ==");
    let mut by_depth: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &fails {
        let delta = f.needed - f.available;
        let cls = if delta <= 16 {
            "<=16"
        } else if delta <= 256 {
            "17..256"
        } else if delta <= 4096 {
            "257..4096"
        } else if delta <= 65536 {
            "4097..65536"
        } else {
            ">65536"
        };
        *by_depth.entry(cls).or_default() += 1;
    }
    for (k, n) in &by_depth {
        push(&format!("{k}: {n}"));
    }

    push("");
    push("== by top-level directory ==");
    let mut by_dir: BTreeMap<String, usize> = BTreeMap::new();
    for f in &fails {
        let dir = f.rel.split('/').next().unwrap_or("?");
        *by_dir.entry(dir.to_string()).or_default() += 1;
    }
    for (k, n) in &by_dir {
        push(&format!("{k}: {n}"));
    }

    push("");
    push("== 0x2D carrier exposure: known carriers among the failing files ==");
    let fail_paths: std::collections::HashSet<String> =
        fails.iter().map(|f| f.rel.clone()).collect();
    let dat_root = ffxi_dat::DatRoot::open(root).expect("install root opens");
    for id in NON_MODEL_SCENE_CARRIERS {
        match dat_root.resolve(id) {
            Ok(loc) => {
                let rel = loc
                    .join_under(root)
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let status = if fail_paths.contains(&rel) {
                    "WALK ERROR (carrier hidden)"
                } else {
                    "walks clean"
                };
                push(&format!("carrier {id}: {rel} — {status}"));
            }
            Err(e) => push(&format!("carrier {id}: resolve failed: {e}")),
        }
    }

    push("");
    push("== 0x2D carrier exposure: zone model DATs among the failing files ==");
    let mut zone_model_fails: Vec<(u16, String)> = Vec::new();
    for &(zone, file_id) in ffxi_dat::zone_dat::ZONE_DAT_TABLE {
        if let Ok(loc) = dat_root.resolve(file_id) {
            let rel = loc
                .join_under(root)
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if fail_paths.contains(&rel) {
                zone_model_fails.push((zone, rel));
            }
        }
    }
    if zone_model_fails.is_empty() {
        push("none: every zone model DAT walks clean");
    } else {
        for (zone, rel) in &zone_model_fails {
            push(&format!("zone {zone}: {rel}"));
        }
    }

    push("");
    push("== 0x2D carrier exposure: failing files with scheduler chunks in the clean prefix ==");
    let sched_fails: Vec<&FailRecord> = fails
        .iter()
        .filter(|f| !f.prefix.scheduler_names.is_empty())
        .collect();
    if sched_fails.is_empty() {
        push("none: no failing file carries a readable scheduler chunk");
    } else {
        push(&format!("{} files:", sched_fails.len()));
        for f in &sched_fails {
            let names: Vec<String> = f.prefix.scheduler_names.iter().take(8).cloned().collect();
            let more = f.prefix.scheduler_names.len().saturating_sub(names.len());
            push(&format!(
                "  {} (err @ {}/{}): {}{}",
                f.rel,
                f.offset,
                f.len,
                names.join(", "),
                if more > 0 {
                    format!(" (+{more} more)")
                } else {
                    String::new()
                }
            ));
        }
    }

    push("");
    push("== attached-route exposure: kind 0x06 with AttachmentInfo != 0 in failing prefixes ==");
    let mut attached_total = 0usize;
    let mut attached_values: BTreeMap<u32, usize> = BTreeMap::new();
    let mut attached_files: Vec<(&FailRecord, usize)> = Vec::new();
    for f in &fails {
        let n: usize = f.prefix.attached_routes.values().sum();
        if n > 0 {
            attached_total += n;
            attached_files.push((f, n));
            for (v, c) in &f.prefix.attached_routes {
                *attached_values.entry(*v).or_default() += c;
            }
        }
    }
    push(&format!(
        "routes: {attached_total} in {} files",
        attached_files.len()
    ));
    if !attached_values.is_empty() {
        push(&format!("values: [{}]", fmt_map(&attached_values)));
    }
    for (f, n) in &attached_files {
        let values: Vec<String> = f
            .prefix
            .attached_routes
            .iter()
            .map(|(v, c)| format!("0x{v:03X} x{c}"))
            .collect();
        push(&format!(
            "  {} ({} routes, err @ {}/{}): {}",
            f.rel,
            n,
            f.offset,
            f.len,
            values.join(", ")
        ));
    }

    push("");
    push("== first 40 failing files ==");
    for f in fails.iter().take(40) {
        push(&format!(
            "{} ({} bytes, err @ {}, {} left, kind {}, name {})",
            f.rel,
            f.len,
            f.offset,
            f.available,
            f.error_chunk_kind
                .map(|k| format!("0x{k:02X}"))
                .unwrap_or_default(),
            f.error_chunk_name.clone().unwrap_or_default()
        ));
    }

    print!("{report}");
}
