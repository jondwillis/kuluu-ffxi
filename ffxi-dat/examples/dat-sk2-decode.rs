//! Decode a Sk2 chunk using the lotus-ffxi layout:
//!
//!   struct Skeleton { u16 _pad; u16 bone_count; }
//!   struct Bone     { u16 parent_index; quat rot(x,y,z,w); vec3 trans; }  // packed(2), 30B
//!
//! Prints the first N bones with `parent`, quaternion magnitude (should
//! be ~1.0 for a real unit quat), and translation. If quat magnitudes
//! cluster near 1.0 the layout is correct; if they're wild, the layout
//! is off and we need to rethink.
//!
//!   cargo run -p ffxi-dat --example dat-sk2-decode -- <file_id> <chunk_idx> [n_bones]

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{walk, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: {} <file_id> <chunk_idx> [n_bones]", args[0]);
        return ExitCode::from(2);
    }
    let file_id: u32 = args[1].parse().unwrap();
    let want_idx: usize = args[2].parse().unwrap();
    let n_show: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);

    let root = DatRoot::from_env_or_default().unwrap();
    let loc = root.resolve(file_id).unwrap();
    let bytes = fs::read(loc.path_under(root.root())).unwrap();
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    let chunk = match chunks.get(want_idx) {
        Some(c) if c.kind == 0x29 => c,
        Some(c) => {
            eprintln!("chunk[{want_idx}] is kind 0x{:02x}, not Sk2 (0x29)", c.kind);
            return ExitCode::from(1);
        }
        None => {
            eprintln!("file has no chunk[{want_idx}]");
            return ExitCode::from(1);
        }
    };
    let body = chunk.data;
    if body.len() < 4 {
        eprintln!("body too short for Sk2 header");
        return ExitCode::from(1);
    }
    let pad = u16::from_le_bytes(body[0..2].try_into().unwrap());
    let count = u16::from_le_bytes(body[2..4].try_into().unwrap());
    println!(
        "file_id={file_id} chunk[{want_idx}] body_len={} _pad=0x{pad:04x} bone_count={count}",
        body.len()
    );

    const BONE_STRIDE: usize = 30; // u16 + 4×f32 + 3×f32, pack(2)
    let bones_end = 4 + count as usize * BONE_STRIDE;
    if bones_end > body.len() {
        eprintln!("bones overrun body: need {bones_end}, have {}", body.len());
    }
    println!("bones [4..{bones_end}]  trailing bytes after bones = {}", body.len().saturating_sub(bones_end));

    let to_show = (count as usize).min(n_show);
    println!("\nidx  parent     quat(x,y,z,w)                              |q|       trans(x,y,z)");
    let mut mag_sum = 0f32;
    let mut mag_n = 0u32;
    for i in 0..count as usize {
        let off = 4 + i * BONE_STRIDE;
        if off + BONE_STRIDE > body.len() {
            break;
        }
        // Pack(2): u16 at off+0, f32 at off+2 (unaligned). Use byte
        // copies via from_le_bytes which doesn't require alignment.
        let parent = u16::from_le_bytes([body[off], body[off + 1]]);
        let f = |a: usize| f32::from_le_bytes([body[a], body[a + 1], body[a + 2], body[a + 3]]);
        let qx = f(off + 2);
        let qy = f(off + 6);
        let qz = f(off + 10);
        let qw = f(off + 14);
        let tx = f(off + 18);
        let ty = f(off + 22);
        let tz = f(off + 26);
        let mag = (qx * qx + qy * qy + qz * qz + qw * qw).sqrt();
        if mag.is_finite() {
            mag_sum += mag;
            mag_n += 1;
        }
        if i < to_show {
            println!(
                "{i:>3}  {parent:>5}({})  ({qx:>7.4},{qy:>7.4},{qz:>7.4},{qw:>7.4})  |q|={mag:>6.3}  ({tx:>7.3},{ty:>7.3},{tz:>7.3})",
                if parent == 0xFFFF { "root" } else { "" }
            );
        }
    }
    if mag_n > 0 {
        println!(
            "\nmean |q| over {mag_n} bones = {:.3}  (1.0 = unit quaternion → layout matches)",
            mag_sum / mag_n as f32
        );
    }
    ExitCode::SUCCESS
}
