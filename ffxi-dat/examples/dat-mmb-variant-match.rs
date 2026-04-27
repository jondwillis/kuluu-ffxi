use std::collections::HashSet;
use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::mmb::{MmbHeader, MmbSubRecord};
use ffxi_dat::{mmb, texture, walk, ChunkKind, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let file_id: u32 = args[1].parse().unwrap();
    let root = DatRoot::from_env().unwrap();
    let bytes = fs::read(root.resolve(file_id).unwrap().path_under(root.root())).unwrap();
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();

    let img_names: HashSet<String> = chunks
        .iter()
        .filter(|c| ChunkKind::from_u8(c.kind) == Some(ChunkKind::Img))
        .filter_map(|c| texture::extract_texture_name(c.data))
        .collect();
    println!("IMG names ({}):", img_names.len());
    let mut sorted: Vec<&String> = img_names.iter().collect();
    sorted.sort();
    for n in &sorted {
        println!("  {:?}", n);
    }
    println!();

    let mut model_subs = 0usize;
    let mut matched = 0usize;
    let mut unmatched_examples: Vec<String> = Vec::new();
    for c in &chunks {
        if c.kind != 0x2E {
            continue;
        }
        let Ok(d) = mmb::decrypt(c.data) else {
            continue;
        };
        let Ok(h) = MmbHeader::parse(&d) else {
            continue;
        };
        for sub in MmbSubRecord::find_all(h.payload) {
            if !sub.tag.starts_with(b"model") {
                continue;
            }
            model_subs += 1;
            let v = sub.variant_name_str().trim().to_string();
            if img_names.contains(&v) {
                matched += 1;
            } else if unmatched_examples.len() < 20 {
                unmatched_examples.push(v);
            }
        }
    }
    println!("model-tagged submeshes: {model_subs}");
    println!("matched to IMG by name: {matched}");
    println!(
        "match rate: {:.1}%",
        100.0 * matched as f64 / model_subs.max(1) as f64
    );
    println!();
    println!("sample unmatched variant_names:");
    for n in &unmatched_examples {
        println!("  {:?}", n);
    }
    ExitCode::SUCCESS
}
