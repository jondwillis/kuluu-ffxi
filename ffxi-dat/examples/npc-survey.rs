//! NPC survey: empirical helper for the `(zone_id, modelid) →
//! (file_id, chunk_idx)` mapping that
//! `ffxi-viewer-core/src/look_resolver.rs:MODELID_TABLE` will need
//! row-by-row.
//!
//! Two passes:
//!
//! 1. **SQL parse.** Read `vendor/server/sql/npc_list.sql` (LSB), find
//!    the requested zone block, decode each NPC's `look BINARY(20)`
//!    and bucket them by `LookData` variant. Output:
//!      - Standard-look NPCs grouped by modelid (the targets for the
//!        resolver: each unique modelid is one table row to derive)
//!      - Equipped-look NPCs (race/sex distribution — informational)
//!      - Door / transport counts (informational)
//!
//! 2. **(optional) DAT probe.** Given `--probe LOW..HIGH`, walk that
//!    range of file_ids, find every MMB chunk, and for each unique
//!    standard-modelid from pass 1 print the file_ids whose MMB
//!    chunk count is `> modelid` (so `chunk_idx = modelid` would land
//!    in bounds). These are *candidates* — the operator still has
//!    to use `/load_mmb_on <entity_id> <file_id> <modelid>` in-game
//!    to confirm which one visually matches.
//!
//! Usage:
//!
//! ```text
//! cargo run -p ffxi-dat --example npc-survey -- [--zone 230] [--probe 0..30000]
//! FFXI_DAT_PATH=/path/to/install \
//!     cargo run -p ffxi-dat --example npc-survey -- --zone 230 --probe 200..400
//! ```
//!
//! No flags → zone 230 (Southern San d'Oria), SQL pass only.
//!
//! # Why a Rust binary and not a shell script
//!
//! The look-blob decoding mirrors `ffxi_proto::decode::LookData` —
//! same MODELTYPE tag rules, same 20-byte layout. Doing it here in
//! Rust keeps the format-of-record in one language; a shell-level
//! awk/grep would silently diverge if LSB ever adds a new MODELTYPE.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use ffxi_dat::mmb::{self, MmbHeader};
use ffxi_dat::{walk, ChunkKind, DatRoot};

/// Decoded `look_t` enum mirroring `ffxi_proto::decode::LookData`
/// without the dependency — the example shouldn't pull in protocol
/// crates. The variants and tag values match
/// `vendor/server/src/map/packets/entity_update.cpp:451-484` and
/// `entity_update.h:29-39` (MODELTYPE enum).
#[derive(Debug, Clone, Copy)]
enum Look {
    /// MODEL_STANDARD = 0, MODEL_UNK_5 = 5, MODEL_AUTOMATON = 6. The
    /// modelid is what NPC packet 0x00E carries for fixed scenery /
    /// monstrosity NPCs.
    Standard { modelid: u16 },
    Equipped {
        race: u8,
        face: u8,
        head: u16,
        body: u16,
    },
    /// MODEL_DOOR = 2.
    Door,
    /// MODEL_SHIP = 4, MODEL_ELEVATOR = 3.
    Transport,
    /// Anything else (MODEL_CHOCOBO = 7 is technically Equipped but we
    /// surface it separately because its model resolution rules differ
    /// from PC-style equipped looks).
    Chocobo,
}

/// Parse a 20-byte LSB `look` BLOB. Returns `None` only on length
/// mismatch — unrecognized type tags fall into the `Standard`-with-
/// modelid bucket, mirroring how the in-client decoder treats unknown
/// MODELTYPE values (it logs and continues with a zero modelid). For a
/// survey we'd rather see them than drop them silently.
fn decode_look(bytes: &[u8]) -> Option<Look> {
    if bytes.len() < 20 {
        return None;
    }
    let size = u16::from_le_bytes([bytes[0], bytes[1]]);
    match size {
        0 | 5 | 6 => Some(Look::Standard {
            modelid: u16::from_le_bytes([bytes[2], bytes[3]]),
        }),
        1 => Some(Look::Equipped {
            face: bytes[2],
            race: bytes[3],
            head: u16::from_le_bytes([bytes[4], bytes[5]]),
            body: u16::from_le_bytes([bytes[6], bytes[7]]),
        }),
        2 => Some(Look::Door),
        3 | 4 => Some(Look::Transport),
        7 => Some(Look::Chocobo),
        _ => Some(Look::Standard {
            modelid: u16::from_le_bytes([bytes[2], bytes[3]]),
        }),
    }
}

/// One NPC row, after we've stripped everything we don't care about
/// from the SQL INSERT statement.
struct NpcRow {
    npc_id: u32,
    name: String,
    look: Look,
}

/// Locate `vendor/server/sql/npc_list.sql` relative to the workspace
/// root. `CARGO_MANIFEST_DIR` is the crate root (`ffxi-dat/`); the
/// workspace lives one directory up. Falls back to `cwd/vendor/...`
/// for the rare case someone runs this outside cargo.
fn npc_list_sql_path() -> PathBuf {
    if let Ok(manifest) = env::var("CARGO_MANIFEST_DIR") {
        let p = PathBuf::from(manifest)
            .parent()
            .map(|w| w.join("vendor/server/sql/npc_list.sql"));
        if let Some(p) = p {
            if p.exists() {
                return p;
            }
        }
    }
    PathBuf::from("vendor/server/sql/npc_list.sql")
}

/// Parse one INSERT INTO `npc_list` line, lifting just the four
/// fields we need: `npcid`, `name`, `look`. Returns `None` for any
/// row whose look field can't be parsed — better to drop the row than
/// to mis-attribute its modelid to a different NPC.
fn parse_row(line: &str) -> Option<NpcRow> {
    // Find the parenthesised VALUES list. Cheap-and-cheerful: cut on
    // `VALUES (` and on `);`.
    let after_values = line.find("VALUES (")?;
    let rest = &line[after_values + "VALUES (".len()..];
    let close = rest.rfind(");")?;
    let body = &rest[..close];

    // npcid: first comma-separated value.
    let first_comma = body.find(',')?;
    let npc_id: u32 = body[..first_comma].trim().parse().ok()?;

    // name: between the next pair of single quotes. (polutils_name
    // follows but we use the binary `name` column — closer to what
    // the operator sees in-game via `/look`.)
    let after_id = &body[first_comma + 1..];
    let name_start = after_id.find('\'')? + 1;
    let after_name_open = &after_id[name_start..];
    let name_end = after_name_open.find('\'')?;
    let name = after_name_open[..name_end].to_string();

    // look: 0x followed by 40 hex chars somewhere later in the body.
    // Find by scanning forward — there's only one `0x` prefix per
    // row that's followed by exactly 40 hex digits (the look blob).
    let hex_start = body.find("0x")? + 2;
    let look_hex = &body[hex_start..hex_start + 40];
    if look_hex.len() < 40 || !look_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let look_bytes = hex_decode_20(look_hex)?;
    let look = decode_look(&look_bytes)?;
    Some(NpcRow {
        npc_id,
        name,
        look,
    })
}

/// Decode 40 hex chars to a 20-byte array. Returns None on any
/// invalid nibble. (We could pull in `hex` as a dev-dep but a tiny
/// loop avoids the dependency.)
fn hex_decode_20(s: &str) -> Option<[u8; 20]> {
    let mut out = [0u8; 20];
    let b = s.as_bytes();
    for i in 0..20 {
        let hi = (b[i * 2] as char).to_digit(16)?;
        let lo = (b[i * 2 + 1] as char).to_digit(16)?;
        out[i] = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

/// Collect every NPC row in `sql` whose preceding `-- (Zone N)`
/// marker matches `zone_id`. The SQL dump is sorted by `npcid`,
/// blocks are delimited by the comment header, and the next block's
/// header is the stop sentinel.
fn rows_for_zone(sql: &str, zone_id: u16) -> Vec<NpcRow> {
    let marker = format!("(Zone {zone_id})");
    let Some(start) = sql.find(&marker) else { return Vec::new() };
    let rest = &sql[start..];
    // End of block: next `-- ------` / `-- (Zone N)` header. We look
    // for the SECOND occurrence of "(Zone " after start — the first
    // is our own header.
    let after_self = &rest["(Zone ".len()..];
    let end = after_self.find("(Zone ").map(|i| i + "(Zone ".len()).unwrap_or(rest.len());
    let block = &rest[..end.min(rest.len())];

    block
        .lines()
        .filter(|l| l.contains("INSERT INTO `npc_list`"))
        .filter_map(parse_row)
        .collect()
}

/// Parse `--probe LOW..HIGH` into an inclusive lower / exclusive
/// upper bound. Returns `None` on missing flag.
fn parse_probe_arg(args: &[String]) -> Option<(u32, u32)> {
    let idx = args.iter().position(|a| a == "--probe")?;
    let spec = args.get(idx + 1)?;
    let mut parts = spec.splitn(2, "..");
    let lo: u32 = parts.next()?.parse().ok()?;
    let hi: u32 = parts.next()?.parse().ok()?;
    Some((lo, hi))
}

fn parse_zone_arg(args: &[String]) -> u16 {
    args.iter()
        .position(|a| a == "--zone")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(230)
}

/// `--min-chunks N` — only print probe matches whose MMB chunk count
/// is at least N. Default 50: filters out terrain-fragment DATs and
/// small assets, leaving the dense-chunk files where NPC props are
/// likely to live.
fn parse_min_chunks_arg(args: &[String]) -> usize {
    args.iter()
        .position(|a| a == "--min-chunks")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(50)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let zone_id = parse_zone_arg(&args);
    let probe = parse_probe_arg(&args);
    let min_chunks = parse_min_chunks_arg(&args);

    let sql_path = npc_list_sql_path();
    let sql = match fs::read_to_string(&sql_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("read {}: {e}", sql_path.display());
            return ExitCode::from(2);
        }
    };

    let rows = rows_for_zone(&sql, zone_id);
    if rows.is_empty() {
        eprintln!("no NPC rows found for zone {zone_id}");
        return ExitCode::from(1);
    }

    // Bucket by Look variant.
    let mut standard: BTreeMap<u16, Vec<&NpcRow>> = BTreeMap::new();
    let mut equipped: Vec<&NpcRow> = Vec::new();
    let mut doors = 0usize;
    let mut transports = 0usize;
    let mut chocobos = 0usize;
    for r in &rows {
        match r.look {
            Look::Standard { modelid } => {
                standard.entry(modelid).or_default().push(r);
            }
            Look::Equipped { .. } => equipped.push(r),
            Look::Door => doors += 1,
            Look::Transport => transports += 1,
            Look::Chocobo => chocobos += 1,
        }
    }

    println!("=== Zone {zone_id} NPC survey ===");
    println!("total rows: {}", rows.len());
    println!(
        "  standard: {}  equipped: {}  doors: {}  transports: {}  chocobos: {}",
        standard.values().map(|v| v.len()).sum::<usize>(),
        equipped.len(),
        doors,
        transports,
        chocobos,
    );

    println!();
    println!("--- Standard-look NPCs (the table rows we need) ---");
    println!("modelid │ count │ npc_id      │ names (first 3)");
    for (modelid, rows) in &standard {
        let first_npc = rows.first().map(|r| r.npc_id).unwrap_or(0);
        let preview: Vec<&str> = rows.iter().take(3).map(|r| r.name.as_str()).collect();
        println!(
            "  {:>5} │ {:>5} │ {:>11} │ {}",
            modelid,
            rows.len(),
            first_npc,
            preview.join(", "),
        );
    }

    if !equipped.is_empty() {
        println!();
        println!("--- Equipped-look NPCs (race/face/head/body sample) ---");
        println!("npc_id      │ name                  │ race face  head    body");
        for r in equipped.iter().take(8) {
            if let Look::Equipped { race, face, head, body } = r.look {
                println!(
                    "  {:>11} │ {:<22} │ {:>4} {:>4}  {:#06x}  {:#06x}",
                    r.npc_id, r.name, race, face, head, body,
                );
            }
        }
        if equipped.len() > 8 {
            println!("  ... and {} more", equipped.len() - 8);
        }
    }

    // Optional probe pass.
    let Some((lo, hi)) = probe else {
        println!();
        println!("(skip DAT probe — pass `--probe LOW..HIGH` to scan a file_id range)");
        println!();
        println!("To confirm a mapping in-game:");
        println!(
            "  1. log into zone {zone_id}; target an NPC of interest (e.g. Well, modelid {}).",
            standard.keys().next().copied().unwrap_or(0),
        );
        println!("  2. run `/look <name>` to read its modelid from the wire.");
        println!("  3. run `/load_mmb_on <entity_id> <file_id> <chunk_idx>` against candidates");
        println!("     until the mesh visually matches.");
        println!("  4. add the confirmed row to MODELID_TABLE in");
        println!("     `ffxi-viewer-core/src/look_resolver.rs`.");
        return ExitCode::SUCCESS;
    };

    // DAT probe — open root, walk requested file_ids, find MMBs.
    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("DatRoot::from_env_or_default: {e}");
            eprintln!("(set FFXI_DAT_PATH or place install at vendor/Game/SquareEnix/...)");
            return ExitCode::from(2);
        }
    };

    println!();
    println!(
        "--- DAT probe: file_ids {lo}..{hi}, min-chunks {min_chunks} ---"
    );
    println!("(streaming; progress printed every 5000 file_ids on stderr)");
    println!("file_id │ mmb_chunks │ asset(s) of first MMB sub-record");
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let mut files_with_mmb: BTreeMap<u32, (usize, String)> = BTreeMap::new();
    for file_id in lo..hi {
        if file_id % 5000 == 0 && file_id > lo {
            // Progress goes to stderr so stdout stays a clean table.
            eprintln!(
                "[progress] scanned {} / {} file_ids, matched {}",
                file_id - lo,
                hi - lo,
                files_with_mmb.len(),
            );
        }
        let loc = match root.resolve(file_id) {
            Ok(l) => l,
            Err(_) => continue,
        };
        let path = loc.path_under(root.root());
        // Cheap pre-filter: stat-only check would be cheaper still
        // but `fs::read` is unavoidable for the chunk walk. We do
        // skip files smaller than 4 KiB — too small to plausibly
        // hold `min_chunks` MMBs with their headers + payload.
        let Ok(meta) = fs::metadata(&path) else { continue };
        if meta.len() < 4096 {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        // Skip the decrypt/parse for asset-name on the bulk scan —
        // it's the heaviest per-chunk step. Just count MMB chunks
        // first; only decrypt the *first* MMB chunk for files that
        // pass the threshold.
        let mut mmb_count = 0usize;
        let mut first_mmb_data: Option<&[u8]> = None;
        for c in walk(&bytes).filter_map(Result::ok) {
            if ChunkKind::from_u8(c.kind) == Some(ChunkKind::Mmb) {
                if first_mmb_data.is_none() {
                    first_mmb_data = Some(c.data);
                }
                mmb_count += 1;
            }
        }
        if mmb_count < min_chunks {
            continue;
        }
        let asset = first_mmb_data
            .and_then(|d| mmb::decrypt(d).ok())
            .and_then(|d| MmbHeader::parse(&d).ok().map(|h| h.asset_name_str().to_string()))
            .unwrap_or_default();
        let asset_short = asset.trim_end_matches('\0').trim().to_string();
        // Stream the row immediately so the operator sees progress
        // and can Ctrl-C once they have enough data.
        let _ = writeln!(
            stdout,
            "  {file_id:>7} │ {mmb_count:>10} │ {asset_short}"
        );
        let _ = stdout.flush();
        files_with_mmb.insert(file_id, (mmb_count, asset_short));
    }
    if files_with_mmb.is_empty() {
        let _ = writeln!(stdout, "  (no MMB-bearing files in range)");
    }
    let _ = stdout.flush();

    // Candidate suggestions per modelid: any file with enough chunks
    // to make `chunk_idx = modelid` in-range.
    println!();
    println!("--- Candidate file_ids per Standard modelid (chunk_count > modelid) ---");
    for modelid in standard.keys() {
        let candidates: Vec<u32> = files_with_mmb
            .iter()
            .filter(|(_, (n, _))| *n as u32 > u32::from(*modelid))
            .map(|(fid, _)| *fid)
            .collect();
        if candidates.is_empty() {
            println!("  modelid {modelid:>5} → no files in probed range with > {modelid} chunks");
        } else {
            // Print at most 8 candidates per modelid to keep output legible.
            let preview: Vec<String> = candidates.iter().take(8).map(|f| f.to_string()).collect();
            let more = if candidates.len() > 8 {
                format!(" (+ {} more)", candidates.len() - 8)
            } else {
                String::new()
            };
            println!("  modelid {modelid:>5} → {}{}", preview.join(", "), more);
        }
    }

    println!();
    println!("Next step: pick one (modelid, file_id) pair, run");
    println!("  /load_mmb_on <entity_id> <file_id> <modelid>");
    println!("against an NPC of that modelid. If the mesh matches, add the row to");
    println!("ffxi-viewer-core/src/look_resolver.rs:MODELID_TABLE.");

    ExitCode::SUCCESS
}
