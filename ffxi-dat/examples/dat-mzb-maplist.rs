//! Probe MZB header `maplist_offset` / `maplist_count` to test the
//! hypothesis that this region is the MMB-placement table (name →
//! transform) for the zone's visual meshes.
//!
//! Usage:
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-mzb-maplist -- <file_id>

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{mzb, walk, ChunkKind, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: FFXI_DAT_PATH=... {} <file_id>", args[0]);
        return ExitCode::from(2);
    }
    let file_id: u32 = args[1].parse().expect("bad file_id");

    let root = DatRoot::from_env().expect("DatRoot::from_env");
    let loc = root.resolve(file_id).expect("resolve");
    let bytes = fs::read(loc.path_under(root.root())).expect("read");
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    let (_, chunk) = chunks
        .iter()
        .enumerate()
        .find(|(_, c)| c.kind == ChunkKind::Mzb as u8)
        .expect("no MZB");

    // Decrypt, parse header.
    let body = mzb::decrypt(chunk.data).expect("decrypt");
    let header = mzb::MzbHeader::parse(&body).expect("header");

    println!("file_id={file_id} body_len={}", body.len());
    println!(
        "maplist_offset=0x{:X} ({})  maplist_count={} (0x{:X})",
        header.maplist_offset, header.maplist_offset, header.maplist_count, header.maplist_count
    );
    println!("quadtree_offset=0x{:X}  mesh_table_offset=0x{:X}", header.quadtree_offset, header.mesh_table_offset);
    println!();

    // Dump first 32 bytes of body so we can sanity-check field positions.
    println!("first 32 bytes of body:");
    hexdump(&body[..32.min(body.len())], 0);
    println!();

    let off = header.maplist_offset as usize;
    if off >= body.len() {
        eprintln!("maplist_offset out of range");
        return ExitCode::from(1);
    }

    // Dump 256 bytes starting at maplist_offset.
    let end = (off + 256).min(body.len());
    println!("body[0x{:X}..0x{:X}]:", off, end);
    hexdump(&body[off..end], off);
    println!();

    // SMZBBlock100 hypothesis: 100-byte records at 0x20, first 16 bytes
    // are name XORed with 0x55. Count = node_count.
    let stride = 100usize;
    let count = header.node_count as usize;
    println!(
        "SMZBBlock100 hypothesis: count={count}, stride={stride}, end=0x{:X}",
        0x20 + count * stride
    );
    println!("first 6 records (name XORed with 0x55, then float-3 trans/rot/scale):");
    for i in 0..6.min(count) {
        let off = 0x20 + i * stride;
        if off + stride > body.len() {
            break;
        }
        let rec = &body[off..off + stride];
        let name_xor: String = rec[..16]
            .iter()
            .map(|&b| (b ^ 0x55) as char)
            .take_while(|&c| c != '\0')
            .collect();
        let name_plain: String = rec[..16]
            .iter()
            .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
            .take_while(|&c| c != '\0')
            .collect();
        let name = format!("plain={:?} xor={:?}", name_plain, name_xor);
        let f = |o: usize| f32::from_le_bytes([rec[o], rec[o + 1], rec[o + 2], rec[o + 3]]);
        let trans = [f(16), f(20), f(24)];
        let rot = [f(28), f(32), f(36)];
        let scale = [f(40), f(44), f(48)];
        println!(
            "  [{i:>4}] name={:<16?} trans=({:>8.2},{:>8.2},{:>8.2}) rot=({:>6.2},{:>6.2},{:>6.2}) scale=({:>5.2},{:>5.2},{:>5.2})",
            name, trans[0], trans[1], trans[2], rot[0], rot[1], rot[2], scale[0], scale[1], scale[2]
        );
    }
    println!();

    // Original name-search:
    // ASCII tokens 4..=16 chars that match known Bastok asset names.
    let needles = [
        b"tshimonoshop_b".as_slice(),
        b"tshimonofount".as_slice(),
        b"tshimonowall".as_slice(),
        b"tshimonostep".as_slice(),
        b"tshimonomain".as_slice(),
        b"tshimonostar".as_slice(),
        b"tshimono".as_slice(),
        b"s_kabe".as_slice(),
        b"s_yuka".as_slice(),
    ];
    println!("name-substring hits in body:");
    for needle in needles {
        let mut hits = Vec::new();
        for i in 0..body.len().saturating_sub(needle.len()) {
            if &body[i..i + needle.len()] == needle {
                hits.push(i);
                if hits.len() >= 5 {
                    break;
                }
            }
        }
        println!(
            "  {:<20}: {} hit(s){}",
            std::str::from_utf8(needle).unwrap(),
            hits.len(),
            if hits.is_empty() {
                String::new()
            } else {
                format!(
                    "  first @ 0x{:X}{}",
                    hits[0],
                    if hits.len() > 1 {
                        format!(", 0x{:X}", hits[1])
                    } else {
                        String::new()
                    }
                )
            }
        );
    }
    println!();

    // Treat maplist_count as: (a) record count w/ stride 16/32/48/64/80,
    // (b) byte length. Report which fits in body.
    println!("size-fit check for maplist_count={}:", header.maplist_count);
    for &stride in &[12usize, 16, 24, 32, 48, 64, 76, 80, 96, 112, 128] {
        let total = header.maplist_count as usize * stride;
        let fits = off + total <= body.len();
        println!(
            "  stride={:>3}: total={:>10} bytes, end=0x{:X}, fits={}",
            stride,
            total,
            off + total,
            fits
        );
    }
    let count_as_bytes = header.maplist_count as usize;
    println!(
        "  count-as-bytes: end=0x{:X}, fits={}",
        off + count_as_bytes,
        off + count_as_bytes <= body.len()
    );

    ExitCode::SUCCESS
}

fn hexdump(data: &[u8], base: usize) {
    for (i, chunk) in data.chunks(16).enumerate() {
        let off = base + i * 16;
        let hex: Vec<String> = chunk.iter().map(|b| format!("{:02x}", b)).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
            .collect();
        println!("  {:08x}  {:<48}  {}", off, hex.join(" "), ascii);
    }
}
