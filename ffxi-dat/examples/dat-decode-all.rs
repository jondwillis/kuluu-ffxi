//! End-to-end smoke test: probe a DAT, identify chunk types, try every
//! decoder we have on it. Reports which decoders succeed.

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::mmb::{MmbHeader, MmbSubRecord};
use ffxi_dat::{anim, bone, mmb, mzb, texture, walk, ChunkKind, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let file_id: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(115);
    let root = DatRoot::from_env().unwrap();
    let path = root.resolve(file_id).unwrap().path_under(root.root());
    let bytes = fs::read(&path).unwrap();

    println!("file_id        {file_id}");
    println!("path           {}", path.display());
    println!();

    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    println!(
        "{:>4}  {:>8}  {:6}  {:>4} {:8}  details",
        "idx", "offset", "name", "kind", "label"
    );

    for (idx, chunk) in chunks.iter().enumerate().take(40) {
        let label = ChunkKind::label(chunk.kind);
        print!(
            "{idx:>4}  {:>8}  {:6}  {:>4} {label:8}  ",
            chunk.offset,
            chunk.name_str(),
            chunk.kind
        );

        match ChunkKind::from_u8(chunk.kind) {
            Some(ChunkKind::Mmb) => {
                let decrypted = mmb::decrypt(chunk.data).unwrap();
                if let Ok(h) = MmbHeader::parse(&decrypted) {
                    let subs = MmbSubRecord::find_all(h.payload);
                    let vert_count: u32 = subs
                        .iter()
                        .filter_map(|s| s.parse_vertices().map(|v| v.len() as u32))
                        .sum();
                    println!(
                        "asset={:?} subrecords={} parsed_verts={}",
                        h.asset_name_str(),
                        subs.len(),
                        vert_count
                    );
                } else {
                    println!("MMB parse failed");
                }
            }
            Some(ChunkKind::Mzb) => match mzb::parse_all(chunk.data) {
                Ok((h, meshes)) => {
                    let verts: usize = meshes.iter().map(|m| m.vertices.len()).sum();
                    let tris: usize = meshes.iter().map(|m| m.triangles.len()).sum();
                    println!(
                        "MZB version=0x{:02X} meshes={} verts={} tris={}",
                        h.version,
                        meshes.len(),
                        verts,
                        tris
                    );
                }
                Err(e) => println!("MZB parse failed: {e}"),
            },
            Some(ChunkKind::Img) => match texture::find_texture_format(chunk.data) {
                Ok(Some((off, fmt))) => println!("texture magic={fmt:?} at off=0x{off:x}"),
                _ => println!("no texture magic found"),
            },
            Some(ChunkKind::Bone) => match bone::Skeleton::parse(chunk.data) {
                Ok(skel) => println!(
                    "bones: count={} pad=0x{:04x} (first bone parent={} flags={})",
                    skel.header.count,
                    skel.header.pad,
                    skel.bones.first().map(|b| b.parent).unwrap_or(0),
                    skel.bones.first().map(|b| b.flags).unwrap_or(0),
                ),
                Err(e) => println!("Sk2 parse failed: {e}"),
            },
            Some(ChunkKind::AnimMo2) => {
                let quats = anim::find_quaternion_records(chunk.data).unwrap();
                let keyframes: Vec<_> = (0..chunk.data.len().saturating_sub(14))
                    .filter_map(|i| anim::decode_keyframe(&chunk.data[i..i + 14]).map(|k| (i, k)))
                    .collect();
                println!(
                    "anim: unit-quat windows={} keyframes(stride={})={}",
                    quats.len(),
                    anim::KEYFRAME_STRIDE,
                    keyframes.len()
                );
            }
            _ => println!(""),
        }
    }
    if chunks.len() > 40 {
        println!("... +{} more chunks", chunks.len() - 40);
    }

    ExitCode::SUCCESS
}
