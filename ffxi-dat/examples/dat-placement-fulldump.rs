//! For each placement record matching a given name substring, dump
//! the full 100-byte record's tail fields (fa..fd floats and fe..fl
//! longs). Looking for a chunk-index disambiguator in the trailing 48
//! bytes that distinguishes duplicate-name placements.
//!
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-placement-fulldump -- <file_id> <name_substr>

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{mzb, walk, ChunkKind, DatRoot};

fn read_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn read_i32(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn read_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "usage: FFXI_DAT_PATH=... {} <file_id> <name_substr>",
            args[0]
        );
        return ExitCode::from(2);
    }
    let file_id: u32 = args[1].parse().unwrap();
    let needle = &args[2];

    let root = DatRoot::from_env().unwrap();
    let location = root.resolve(file_id).unwrap();
    let bytes = fs::read(location.path_under(root.root())).unwrap();
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();

    let mzb_chunk = chunks
        .iter()
        .find(|c| c.kind == ChunkKind::Mzb as u8)
        .unwrap();
    let plain = mzb::decrypt(mzb_chunk.data).unwrap();
    let header = mzb::MzbHeader::parse(&plain).unwrap();

    let count = header.node_count as usize;
    println!("DAT {file_id}: scanning {count} placement records at 0x20 stride 100");
    println!("Filtering to records whose id contains '{needle}'");
    println!();

    let mut match_count = 0;
    for i in 0..count {
        let off = 0x20 + i * 100;
        if off + 100 > plain.len() {
            break;
        }
        let rec = &plain[off..off + 100];
        // id is bytes 0..16; trim nulls and whitespace.
        let end = rec[..16].iter().position(|&b| b == 0).unwrap_or(16);
        let id = std::str::from_utf8(&rec[..end]).unwrap_or("?").trim();
        if needle != "*" && !id.contains(needle) {
            continue;
        }
        match_count += 1;
        if match_count > 15 && needle == "*" {
            continue;
        }

        // Offsets within the record:
        //   0..16   id
        //   16..28  trans
        //   28..40  rot
        //   40..52  scale
        //   52..68  fa, fb, fc, fd (4 floats)
        //   68..100 fe..fl (8 i32)
        let tr = (read_f32(rec, 16), read_f32(rec, 20), read_f32(rec, 24));
        let fa = read_f32(rec, 52);
        let fb = read_f32(rec, 56);
        let fc = read_f32(rec, 60);
        let fd = read_f32(rec, 64);
        let longs: [i32; 8] = [
            read_i32(rec, 68),
            read_i32(rec, 72),
            read_i32(rec, 76),
            read_i32(rec, 80),
            read_i32(rec, 84),
            read_i32(rec, 88),
            read_i32(rec, 92),
            read_i32(rec, 96),
        ];
        let ulongs: [u32; 8] = [
            read_u32(rec, 68),
            read_u32(rec, 72),
            read_u32(rec, 76),
            read_u32(rec, 80),
            read_u32(rec, 84),
            read_u32(rec, 88),
            read_u32(rec, 92),
            read_u32(rec, 96),
        ];

        println!(
            "[{i:04}] id={id:16} trans=({:7.1},{:6.1},{:7.1})  fabcd=({fa:.3}, {fb:.3}, {fc:.3}, {fd:.3})",
            tr.0, tr.1, tr.2,
        );
        print!("       i32 fe..fl: ");
        for v in &longs {
            print!("{v:11} ");
        }
        println!();
        print!("       u32 fe..fl: ");
        for v in &ulongs {
            print!("{v:11} ");
        }
        println!();
        print!("       hex fe..fl: ");
        for v in &ulongs {
            print!("{v:08x}    ");
        }
        println!();
        println!();
    }
    println!("Total matches: {match_count}");
    ExitCode::SUCCESS
}
