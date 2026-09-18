//! Camera chunk census: chunk-kind counts per file and the AttachmentInfo
//! distribution of kind 0x06 chunks, on the retail 19-bit chunk walk and on the
//! xim-style 20-bit walk, plus an install-wide recount of attached routes.
//! The two walks are run side by side so a file one walk reads as camera-free
//! (an earlier pass found zero 0x06 chunks in ROM/0/23.DAT) can be checked
//! against the install-wide 0x06 count.
//!
//! Usage: cargo run -p ffxi-event --example zz-camera-census -- <install root>
//! where the install root is the folder containing ROM/.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ffxi_dat::{walk, CameraResource, DatError, DatRoot};

/// The seven files of B5: ROM/0/23.DAT (E18's 317-route parse) and the six
/// files Fix 4 named for its attached routes.
const CENSUS_FILES: [&str; 7] = [
    "ROM/0/23.DAT",
    "ROM/62/110.DAT",
    "ROM/62/112.DAT",
    "ROM/95/0.DAT",
    "ROM/174/22.DAT",
    "ROM/160/3.DAT",
    "ROM/139/62.DAT",
];

const KIND_CAMERA: u8 = 0x06;

#[derive(Default)]
struct WalkCensus {
    chunks: usize,
    kinds: BTreeMap<u8, usize>,
    cameras: usize,
    camera_parse_errors: usize,
    attach: BTreeMap<u32, usize>,
    first_error: Option<(usize, String)>,
}

impl WalkCensus {
    fn record(&mut self, kind: u8, body: &[u8]) {
        self.chunks += 1;
        *self.kinds.entry(kind).or_default() += 1;
        if kind == KIND_CAMERA {
            self.cameras += 1;
            if CameraResource::parse([0; 4], body).is_err() {
                self.camera_parse_errors += 1;
            }
            if let Some(info) = body.get(0..4) {
                let bytes: [u8; 4] = info.try_into().unwrap();
                *self.attach.entry(u32::from_le_bytes(bytes)).or_default() += 1;
            }
        }
    }

    fn fail(&mut self, offset: usize, err: String) {
        if self.first_error.is_none() {
            self.first_error = Some((offset, err));
        }
    }
}

/// One pass with the retail walk (ffxi_dat::chunk::walk: 7-bit kind, 19-bit
/// size in 16-byte units; a single error stops the walk).
fn census_retail(bytes: &[u8]) -> WalkCensus {
    let mut out = WalkCensus::default();
    for c in walk(bytes) {
        match c {
            Ok(chunk) => out.record(chunk.kind, chunk.data),
            Err(e) => {
                let offset = match &e {
                    DatError::TruncatedChunk { offset, .. } => *offset,
                    _ => 0,
                };
                out.fail(offset, format!("{e}"));
                break;
            }
        }
    }
    out
}

/// The same walk with xim's 20-bit size mask (ffxi_dat::chunk: "xim's 20-bit
/// walk is a known latent bug" — bit 26 is is_shadow, not size). Mirrors
/// ChunkWalker byte for byte except the mask, to reproduce the candidate
/// failure.
fn census_xim20(bytes: &[u8]) -> WalkCensus {
    let mut out = WalkCensus::default();
    let mut cursor = 0;
    while cursor + 16 <= bytes.len() {
        let value = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap());
        let kind = (value & 0x7F) as u8;
        let size_units = (value >> 7) & 0xFFFFF;
        let total = (size_units as usize).saturating_mul(16);
        if total < 16 {
            break;
        }
        let body_start = cursor + 16;
        let body_end = body_start + total - 16;
        if body_end > bytes.len() {
            out.fail(
                cursor,
                format!(
                    "truncated at chunk {cursor}: need {total} bytes, {} left",
                    bytes.len() - cursor
                ),
            );
            break;
        }
        out.record(kind, &bytes[body_start..body_end]);
        cursor = body_end;
    }
    out
}

fn fmt_kinds(kinds: &BTreeMap<u8, usize>) -> String {
    kinds
        .iter()
        .map(|(k, n)| format!("0x{k:02X} x{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn fmt_attach(attach: &BTreeMap<u32, usize>) -> String {
    attach
        .iter()
        .map(|(v, n)| format!("0x{v:03X} x{n}"))
        .collect::<Vec<_>>()
        .join(", ")
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

/// "ROM/62/110.DAT" -> (62, 110): the arithmetic dir*256+file pseudo-id the
/// first scan used before E16 corrected it to true VTABLE/FTABLE ids.
fn arithmetic_id(rel: &str) -> Option<u32> {
    let parts = rel.split('/').collect::<Vec<_>>();
    let [rom, dir, file] = parts.as_slice() else {
        return None;
    };
    let _ = rom;
    let file = file.strip_suffix(".DAT").unwrap_or(file);
    Some(dir.parse::<u32>().ok()? * 256 + file.parse::<u32>().ok()?)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(install) = args.get(1) else {
        eprintln!("usage: zz-camera-census <install root>");
        std::process::exit(2);
    };
    let root = Path::new(install);
    if !root.join("VTABLE.DAT").exists() {
        eprintln!("{install} has no VTABLE.DAT; not an install root");
        std::process::exit(2);
    }

    let mut report = String::new();
    let mut push = |line: &str| {
        report.push_str(line);
        report.push('\n');
    };

    push("== per-file census: retail 19-bit walk (ffxi_dat::walk) ==");
    for rel in CENSUS_FILES {
        let path = root.join(rel);
        let Ok(bytes) = std::fs::read(&path) else {
            push(&format!("{rel}: MISSING"));
            continue;
        };
        let retail = census_retail(&bytes);
        let xim = census_xim20(&bytes);
        push(&format!("{rel}: {} bytes", bytes.len()));
        push(&format!(
            "  retail: chunks {}  kinds: [{}]",
            retail.chunks,
            fmt_kinds(&retail.kinds)
        ));
        push(&format!(
            "  retail 0x06: {} (parse ok {}, parse err {})",
            retail.cameras,
            retail.cameras - retail.camera_parse_errors,
            retail.camera_parse_errors
        ));
        push(&format!(
            "  retail attach: [{}]",
            fmt_attach(&retail.attach)
        ));
        if let Some((off, err)) = &retail.first_error {
            push(&format!("  retail first error @ {off}: {err}"));
        }
        push(&format!(
            "  xim20: chunks {}  0x06 {}  kinds: [{}]",
            xim.chunks,
            xim.cameras,
            fmt_kinds(&xim.kinds)
        ));
        if let Some((off, err)) = &xim.first_error {
            push(&format!("  xim20 first error @ {off}: {err}"));
        }
    }

    push("");
    push("== DatRoot file id resolution ==");
    match DatRoot::open(root) {
        Ok(dat_root) => {
            let max_id = dat_root
                .app_summary()
                .iter()
                .map(|(_, v, _)| *v)
                .max()
                .unwrap_or(0);
            let mut id_by_path: BTreeMap<PathBuf, u32> = BTreeMap::new();
            for id in 0..max_id {
                if let Ok(loc) = dat_root.resolve(id) {
                    id_by_path.insert(loc.join_under(root), id);
                }
            }
            for rel in CENSUS_FILES {
                let path = root.join(rel);
                let true_id = id_by_path.get(&path).copied();
                let arith = arithmetic_id(rel);
                match (true_id, arith) {
                    (Some(t), Some(a)) if t == a => {
                        push(&format!("{rel}: id {t} (arithmetic id agrees)"));
                    }
                    (Some(t), Some(a)) => {
                        let arith_path = dat_root
                            .resolve(a)
                            .map(|loc| loc.join_under(root))
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        push(&format!(
                            "{rel}: true id {t}, arithmetic pseudo-id {a} resolves to {arith_path}"
                        ));
                    }
                    (Some(t), None) => {
                        push(&format!("{rel}: true id {t}, no arithmetic pseudo-id"));
                    }
                    (None, _) => {
                        push(&format!("{rel}: no file id resolves to this path"));
                    }
                }
            }
        }
        Err(e) => push(&format!("DatRoot open failed: {e}")),
    }

    push("");
    push("== install-wide census: retail 19-bit walk ==");
    let mut files = Vec::new();
    collect_dat_files(root, &mut files);
    let mut total_cameras = 0usize;
    let mut total_attached = 0usize;
    let mut attach_dist: BTreeMap<u32, usize> = BTreeMap::new();
    let mut mode_dist: BTreeMap<u32, usize> = BTreeMap::new();
    let mut per_file: BTreeMap<String, usize> = BTreeMap::new();
    let mut walk_errors = 0usize;
    let mut camera_files = 0usize;
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let census = census_retail(&bytes);
        if census.first_error.is_some() {
            walk_errors += 1;
        }
        if census.cameras > 0 {
            camera_files += 1;
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            per_file.insert(rel, census.cameras);
        }
        total_cameras += census.cameras;
        for (info, n) in &census.attach {
            *attach_dist.entry(*info).or_default() += n;
            if *info != 0 {
                total_attached += n;
            }
            // Attachment.cpp GetAttachMode: low nibble plus bit 16.
            let mode = (info & 0xF) + 16 * ((info >> 16) & 1);
            *mode_dist.entry(mode).or_default() += n;
        }
    }
    push(&format!(
        "files: {}  files with 0x06: {camera_files}  total 0x06: {total_cameras}  attached (info != 0): {total_attached}  walk errors: {walk_errors}",
        files.len()
    ));
    push(&format!(
        "attach distribution: [{}]",
        fmt_attach(&attach_dist)
    ));
    let mode_str = mode_dist
        .iter()
        .map(|(m, n)| format!("mode {m} x{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    push(&format!("mode distribution: [{mode_str}]"));
    let mut ranked: Vec<(&String, &usize)> = per_file.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    push("files with 0x06 (count desc):");
    for (rel, n) in ranked {
        push(&format!("  {rel}: {n}"));
    }

    // Land the report in the gitignored per-machine artifacts dir, for the record.
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let artifacts = workspace_root.join("artifacts").join("verify");
    std::fs::create_dir_all(&artifacts).ok();
    let out_path = artifacts.join("camera_census.txt");
    std::fs::write(&out_path, &report).ok();
    print!("{report}");
    eprintln!("written to {}", out_path.display());
}
