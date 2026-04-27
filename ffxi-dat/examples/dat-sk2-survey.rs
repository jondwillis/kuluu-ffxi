use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{walk, ChunkKind, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: FFXI_DAT_PATH=... {} <file_id> [chunk_idx]", args[0]);
        return ExitCode::from(2);
    }
    let file_id: u32 = match args[1].parse() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("bad file_id `{}`: {e}", args[1]);
            return ExitCode::from(2);
        }
    };
    let only_chunk: Option<usize> = args.get(2).and_then(|s| s.parse().ok());

    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("DatRoot::from_env_or_default: {e}");
            return ExitCode::from(1);
        }
    };
    let loc = match root.resolve(file_id) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("resolve({file_id}): {e}");
            return ExitCode::from(1);
        }
    };
    let bytes = match fs::read(loc.path_under(root.root())) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("read: {e}");
            return ExitCode::from(1);
        }
    };
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    println!(
        "file_id={file_id} bytes={} chunks={}",
        bytes.len(),
        chunks.len()
    );

    let mut found = 0u32;
    for (i, chunk) in chunks.iter().enumerate() {
        if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::Bone) {
            continue;
        }
        if let Some(want) = only_chunk {
            if want != i {
                continue;
            }
        }
        found += 1;
        dump_sk2(i, chunk.data);
    }
    if found == 0 {
        let target = only_chunk
            .map(|c| format!("chunk[{c}]"))
            .unwrap_or_else(|| "any Sk2 (0x29) chunk".into());
        println!("no {target} in file_id={file_id}");
    }
    ExitCode::SUCCESS
}

fn dump_sk2(chunk_idx: usize, body: &[u8]) {
    println!(
        "\n=== chunk[{chunk_idx}] kind=0x29 (Bone/Sk2) body_len={} ===",
        body.len()
    );
    if body.len() < 16 {
        println!(
            "  body too short for Sk2 header (need >=16, got {})",
            body.len()
        );
        return;
    }
    let hdr = u32::from_le_bytes(body[0..4].try_into().unwrap());
    let count = (hdr & 0xFFFF) as u16;
    let flags = (hdr >> 16) as u16;
    let after_header = 16usize;
    let payload = body.len().saturating_sub(after_header);
    println!("  header u32 = 0x{hdr:08x}  bone count = {count}  flags = 0x{flags:04x}");
    println!(
        "  next 12 bytes (header pad) = {}",
        hex(&body[4..16.min(body.len())])
    );
    println!("  payload bytes after header = {payload}  → bytes-per-bone candidates:");
    for stride in [48usize, 56, 64, 80] {
        if count > 0 && payload.is_multiple_of(stride) && payload / stride == count as usize {
            println!("    EXACT MATCH: stride={stride}  (count*stride == payload)");
        } else if count > 0 {
            let extra = payload as i64 - count as i64 * stride as i64;
            println!(
                "    stride={stride}: count*stride={} (delta {extra:+})",
                count as usize * stride
            );
        }
    }

    let records_to_show = 10usize;
    for stride in [48usize, 64, 80] {
        if stride == 0 || stride % 4 != 0 {
            continue;
        }
        let floats_per_rec = stride / 4;
        println!(
            "\n  ── interpreting first {records_to_show} bones as {floats_per_rec} f32 each (stride {stride}):"
        );
        for r in 0..records_to_show {
            let off = after_header + r * stride;
            if off + stride > body.len() {
                println!("    bone[{r}]: truncated at offset {off}");
                break;
            }
            print!("    bone[{r:>2}] @ 0x{off:04x}:");

            let p_u16 = u16::from_le_bytes([body[off], body[off + 1]]);
            print!(" p_u16=0x{p_u16:04x}({p_u16})  floats:");
            for fi in 0..floats_per_rec {
                let f =
                    f32::from_le_bytes(body[off + fi * 4..off + fi * 4 + 4].try_into().unwrap());
                if f.is_finite() && f.abs() < 1e6 {
                    print!(" {f:>9.4}");
                } else {
                    print!(" {f:>9.2e}");
                }
                if fi == 3 || fi == 7 || fi == 11 {
                    print!(" |");
                }
            }
            println!();
        }
    }

    let head = (after_header + 256).min(body.len());
    println!(
        "\n  ── raw hex, first {} payload bytes:\n    {}",
        head - after_header,
        hex(&body[after_header..head])
    );

    println!("\n  ── stride divisibility (payload {payload} bytes):");
    for stride in [
        16usize, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80, 84, 88, 92, 96,
        112, 128,
    ] {
        let n = payload / stride;
        let rem = payload % stride;
        if rem == 0 {
            println!("    stride={stride:>3} → {n:>4} records  REMAINDER 0  ★");
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}
