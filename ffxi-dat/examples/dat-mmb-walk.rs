use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::mmb::{MmbHeader, MmbSubRecord};
use ffxi_dat::{mmb, walk, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: FFXI_DAT_PATH=... {} <file_id> <chunk_idx>", args[0]);
        return ExitCode::from(2);
    }

    let file_id: u32 = args[1].parse().unwrap();
    let chunk_idx: usize = args[2].parse().unwrap();

    let root = DatRoot::from_env().unwrap();
    let location = root.resolve(file_id).unwrap();
    let bytes = fs::read(location.path_under(root.root())).unwrap();
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    let chunk = &chunks[chunk_idx];

    let decrypted = mmb::decrypt(chunk.data).unwrap();
    let header = MmbHeader::parse(&decrypted).unwrap();

    println!("file_id        {file_id}");
    println!(
        "chunk          name={:?} kind={} body_len={}",
        chunk.name_str(),
        chunk.kind,
        chunk.data.len()
    );
    println!("MMB version    {}", header.version);
    println!("MMB key_index  {:#x}", header.key_index);
    println!("MMB asset_name {:?}", header.asset_name_str());
    println!();

    let records = MmbSubRecord::find_all(header.payload);
    println!("found {} sub-records:", records.len());
    for r in &records {
        println!(
            "  offset=0x{:04x}  variant={:?}  count={}  body_len={}",
            r.offset,
            r.variant_name_str(),
            r.count,
            r.body.len()
        );
    }

    ExitCode::SUCCESS
}
