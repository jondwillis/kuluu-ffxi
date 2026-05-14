//! Print vertex AABB for several MMBs in a zone DAT. If MMBs were
//! pre-baked in world space, AABBs spread across the zone footprint
//! (~hundreds of yalms apart). If local-pivot, all AABBs cluster
//! around origin.
//!
//! Usage:
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-mmb-aabb -- <file_id>

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::mmb::{self, MmbHeader, MmbSubRecord};
use ffxi_dat::{walk, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let file_id: u32 = args[1].parse().unwrap();
    let root = DatRoot::from_env().unwrap();
    let location = root.resolve(file_id).unwrap();
    let bytes = fs::read(location.path_under(root.root())).unwrap();
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();

    let mut printed = 0;
    let mut centers: Vec<(String, [f32; 3])> = Vec::new();
    for (idx, c) in chunks.iter().enumerate() {
        if c.kind != 0x2E {
            continue;
        }
        let dec = match mmb::decrypt(c.data) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let hdr = match MmbHeader::parse(&dec) {
            Ok(h) => h,
            Err(_) => continue,
        };
        let records = MmbSubRecord::find_all(hdr.payload);

        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut vcount = 0usize;
        for r in &records {
            if let Some(verts) = r.parse_vertices() {
                for v in &verts {
                    for k in 0..3 {
                        if v.pos[k] < min[k] {
                            min[k] = v.pos[k];
                        }
                        if v.pos[k] > max[k] {
                            max[k] = v.pos[k];
                        }
                    }
                    vcount += 1;
                }
            }
        }
        if vcount == 0 {
            continue;
        }
        let center = [
            (min[0] + max[0]) * 0.5,
            (min[1] + max[1]) * 0.5,
            (min[2] + max[2]) * 0.5,
        ];
        let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        centers.push((hdr.asset_name_str(), center));
        if printed < 12 {
            println!(
                "[{idx:>4}] {:<20}  center=({:>8.2},{:>8.2},{:>8.2})  size=({:>7.2},{:>7.2},{:>7.2})  v={vcount}",
                hdr.asset_name_str(),
                center[0], center[1], center[2],
                size[0], size[1], size[2]
            );
            printed += 1;
        }
    }

    println!();
    println!("--- aggregate ({} MMBs with verts) ---", centers.len());
    if !centers.is_empty() {
        let (mut cmin, mut cmax) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for (_, c) in &centers {
            for k in 0..3 {
                if c[k] < cmin[k] {
                    cmin[k] = c[k];
                }
                if c[k] > cmax[k] {
                    cmax[k] = c[k];
                }
            }
        }
        println!(
            "centers AABB: min=({:>8.2},{:>8.2},{:>8.2})  max=({:>8.2},{:>8.2},{:>8.2})  spread=({:>7.2},{:>7.2},{:>7.2})",
            cmin[0], cmin[1], cmin[2], cmax[0], cmax[1], cmax[2],
            cmax[0]-cmin[0], cmax[1]-cmin[1], cmax[2]-cmin[2]
        );
    }
    ExitCode::SUCCESS
}
