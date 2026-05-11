//! Resolve a static-NPC name from an LSB/retail npc id against a real
//! FFXI install.
//!
//! Verification entry point for ffxi-dat's `npc_names` decoder. Mirrors
//! the lookup path that `ffxi-client` will eventually take when a
//! CHAR_NPC packet arrives without an inline name (most do — see
//! LSB `entity_update.cpp:293-295`).
//!
//! Usage:
//!   FFXI_DAT_PATH="/.../FINAL FANTASY XI" \
//!       cargo run -p ffxi-dat --example dat-npc-name -- 17719306

use std::env;
use std::process::ExitCode;

use ffxi_dat::{split_id, DatRoot, NpcNameTable};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some(arg) = args.get(1) else {
        eprintln!(
            "usage: FFXI_DAT_PATH=<path-to-FF-XI-dir> {} <npcid>",
            args.first().map(String::as_str).unwrap_or("dat-npc-name")
        );
        return ExitCode::from(2);
    };

    let npc_id: u32 = match arg.parse() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("invalid npc_id {arg:?}: {e}");
            return ExitCode::from(2);
        }
    };

    let Some((zone_id, slot)) = split_id(npc_id) else {
        eprintln!(
            "npc_id {npc_id} ({npc_id:#010x}) is not a valid entity id (missing 0x01 marker)"
        );
        return ExitCode::from(1);
    };

    println!("npc_id     {npc_id} ({npc_id:#010x})");
    println!("zone       {zone_id}");
    println!("slot       {slot}");

    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("could not open DAT root: {e}");
            return ExitCode::from(1);
        }
    };

    let table = match NpcNameTable::open(&root, zone_id) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("could not open NPC-name table for zone {zone_id}: {e}");
            return ExitCode::from(1);
        }
    };
    println!("source     {}", table.source().display());
    println!("records    {}", table.len());

    match table.lookup_by_id(npc_id) {
        Some(name) => {
            println!("name       {name:?}");
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("name       <unresolved>");
            ExitCode::from(1)
        }
    }
}
