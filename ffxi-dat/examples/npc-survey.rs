use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use ffxi_dat::mmb::{self, MmbHeader};
use ffxi_dat::{walk, ChunkKind, DatRoot};
use lsb_scrape::{parse_yaml_enum_values, parse_yaml_npcs, Yaml};

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

struct NpcRow {
    npc_id: u32,
    name: String,
    look: Look,
}

const LSB_ZONE_ENUM_YAML: &str = "vendor/server/data/enums/zone.yaml";
const LSB_ZONES_DATA_DIR: &str = "vendor/server/data/zones";

fn workspace_root() -> PathBuf {
    env::var("CARGO_MANIFEST_DIR")
        .ok()
        .and_then(|m| PathBuf::from(m).parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `render.look` of a data/zones/<zone>/npcs.yaml entry, in the shape the
/// survey buckets by (vendor/server/src/map/data/datasets/zones/npcs/yaml.h
/// Look).
fn look_from_yaml(look: &Yaml) -> Option<Look> {
    let kind: String = look.field("type").ok()??;
    let u16_field = |key: &str| look.field::<u16>(key).ok().flatten();
    let u8_field = |key: &str| look.field::<u8>(key).ok().flatten();
    match kind.as_str() {
        "standard" | "automaton" => Some(Look::Standard {
            modelid: u16_field("model")?,
        }),
        "equipped" => Some(Look::Equipped {
            race: u8_field("race")?,
            face: u8_field("face")?,
            head: u16_field("head")?,
            body: u16_field("body")?,
        }),
        "door" => Some(Look::Door),
        "ship" | "elevator" => Some(Look::Transport),
        "chocobo" => Some(Look::Chocobo),
        _ => None,
    }
}

fn rows_for_zone(zone_id: u16) -> Result<Vec<NpcRow>, String> {
    let root = workspace_root();
    let enum_path = root.join(LSB_ZONE_ENUM_YAML);
    let enum_src =
        fs::read_to_string(&enum_path).map_err(|e| format!("read {}: {e}", enum_path.display()))?;
    let (zone_key, _) = parse_yaml_enum_values(&enum_src)
        .map_err(|e| format!("{}: {e:#}", enum_path.display()))?
        .into_iter()
        .find(|(_, id)| *id == u32::from(zone_id))
        .ok_or_else(|| format!("zone {zone_id} is not in {}", enum_path.display()))?;
    let path = root
        .join(LSB_ZONES_DATA_DIR)
        .join(&zone_key)
        .join("npcs.yaml");
    let src = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let npcs = parse_yaml_npcs(&src).map_err(|e| format!("{}: {e:#}", path.display()))?;
    Ok(npcs
        .into_iter()
        .filter_map(|(npc_id, npc)| {
            let name = npc
                .field::<String>("display_name")
                .ok()
                .flatten()
                .unwrap_or_default();
            let look = look_from_yaml(npc.get("render")?.get("look")?)?;
            Some(NpcRow { npc_id, name, look })
        })
        .collect())
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

    let rows = match rows_for_zone(zone_id) {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
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
        // The `//`-prefixed commands are the Dev set in kuluu/src/view_native/slash_commands.rs.
        println!("  2. run `//look <name>` to read its modelid from the wire.");
        println!("  3. run `//load_mmb_on <entity_id> <file_id> <chunk_idx>` against candidates");
        println!("     until the mesh visually matches.");
        println!("  4. add the confirmed row to MODELID_TABLE in");
        println!("     `kuluu-render/src/look_resolver.rs`.");
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
        let path = loc.path_under(&root);

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
    // Dev-set command, registered in kuluu/src/view_native/slash_commands.rs.
    println!("  //load_mmb_on <entity_id> <file_id> <modelid>");
    println!("against an NPC of that modelid. If the mesh matches, add the row to");
    println!("kuluu-render/src/look_resolver.rs:MODELID_TABLE.");

    ExitCode::SUCCESS
}
