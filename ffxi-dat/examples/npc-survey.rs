use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use ffxi_dat::mmb::{self, MmbHeader};
use ffxi_dat::{walk, ChunkKind, DatRoot};

#[derive(Debug, Clone, Copy)]
enum Look {
    Standard {
        modelid: u16,
    },
    Equipped {
        race: u8,
        face: u8,
        head: u16,
        body: u16,
    },

    Door,

    Transport,

    Chocobo,
}

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

struct NpcRow {
    npc_id: u32,
    name: String,
    look: Look,
}

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

fn parse_row(line: &str) -> Option<NpcRow> {
    let after_values = line.find("VALUES (")?;
    let rest = &line[after_values + "VALUES (".len()..];
    let close = rest.rfind(");")?;
    let body = &rest[..close];

    let first_comma = body.find(',')?;
    let npc_id: u32 = body[..first_comma].trim().parse().ok()?;

    let after_id = &body[first_comma + 1..];
    let name_start = after_id.find('\'')? + 1;
    let after_name_open = &after_id[name_start..];
    let name_end = after_name_open.find('\'')?;
    let name = after_name_open[..name_end].to_string();

    let hex_start = body.find("0x")? + 2;
    let look_hex = &body[hex_start..hex_start + 40];
    if look_hex.len() < 40 || !look_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let look_bytes = hex_decode_20(look_hex)?;
    let look = decode_look(&look_bytes)?;
    Some(NpcRow { npc_id, name, look })
}

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

fn rows_for_zone(sql: &str, zone_id: u16) -> Vec<NpcRow> {
    let marker = format!("(Zone {zone_id})");
    let Some(start) = sql.find(&marker) else {
        return Vec::new();
    };
    let rest = &sql[start..];

    let after_self = &rest["(Zone ".len()..];
    let end = after_self
        .find("(Zone ")
        .map(|i| i + "(Zone ".len())
        .unwrap_or(rest.len());
    let block = &rest[..end.min(rest.len())];

    block
        .lines()
        .filter(|l| l.contains("INSERT INTO `npc_list`"))
        .filter_map(parse_row)
        .collect()
}

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
            if let Look::Equipped {
                race,
                face,
                head,
                body,
            } = r.look
            {
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

    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("DatRoot::from_env_or_default: {e}");
            eprintln!("(set FFXI_DAT_PATH or place install at vendor/game-files/SquareEnix/...)");
            return ExitCode::from(2);
        }
    };

    println!();
    println!("--- DAT probe: file_ids {lo}..{hi}, min-chunks {min_chunks} ---");
    println!("(streaming; progress printed every 5000 file_ids on stderr)");
    println!("file_id │ mmb_chunks │ asset(s) of first MMB sub-record");
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let mut files_with_mmb: BTreeMap<u32, (usize, String)> = BTreeMap::new();
    for file_id in lo..hi {
        if file_id % 5000 == 0 && file_id > lo {
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

        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if meta.len() < 4096 {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };

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
            .and_then(|d| {
                MmbHeader::parse(&d)
                    .ok()
                    .map(|h| h.asset_name_str().to_string())
            })
            .unwrap_or_default();
        let asset_short = asset.trim_end_matches('\0').trim().to_string();

        let _ = writeln!(stdout, "  {file_id:>7} │ {mmb_count:>10} │ {asset_short}");
        let _ = stdout.flush();
        files_with_mmb.insert(file_id, (mmb_count, asset_short));
    }
    if files_with_mmb.is_empty() {
        let _ = writeln!(stdout, "  (no MMB-bearing files in range)");
    }
    let _ = stdout.flush();

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
