//! Probe a single chunk in a DAT and try MMB decryption on its body.
//! Usage: cargo run -p ffxi-dat --example dat-mmb-probe -- <file_id> <chunk_idx>

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{mmb, walk, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "usage: FFXI_DAT_PATH=... {} <file_id> <chunk_idx>",
            args.first().map(String::as_str).unwrap_or("dat-mmb-probe")
        );
        return ExitCode::from(2);
    }

    let file_id: u32 = args[1].parse().unwrap();
    let chunk_idx: usize = args[2].parse().unwrap();

    let root = DatRoot::from_env().unwrap();
    let location = root.resolve(file_id).unwrap();
    let path = location.path_under(root.root());
    let bytes = fs::read(&path).unwrap();

    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    if chunk_idx >= chunks.len() {
        eprintln!("file has only {} chunks", chunks.len());
        return ExitCode::from(1);
    }

    let chunk = &chunks[chunk_idx];
    println!("file_id     {file_id}");
    println!("path        {}", path.display());
    println!(
        "chunk[{chunk_idx}]   name={:?} kind={} body_len={}",
        chunk.name_str(),
        chunk.kind,
        chunk.data.len()
    );
    println!();

    let body = chunk.data;
    print!("first 32 bytes encrypted:    ");
    for b in body.iter().take(32) {
        print!("{b:02x} ");
    }
    println!();

    let mut probe = body.to_vec();
    match mmb::decrypt_in_place(&mut probe) {
        Ok(()) => {
            print!("first 32 bytes decrypted:    ");
            for b in probe.iter().take(32) {
                print!("{b:02x} ");
            }
            println!();
            print!("first 32 chars decrypted:    ");
            for b in probe.iter().take(32) {
                if b.is_ascii_graphic() || *b == b' ' {
                    print!(" {} ", *b as char);
                } else {
                    print!(" . ");
                }
            }
            println!();

            // Dump full decrypted blob for inspection.
            let dump = format!("/tmp/mmb-{file_id}-{chunk_idx}.bin");
            std::fs::write(&dump, &probe).unwrap();
            println!("\n  dumped full decrypted body to {dump}");
        }
        Err(e) => eprintln!("decrypt failed: {e}"),
    }

    ExitCode::SUCCESS
}
