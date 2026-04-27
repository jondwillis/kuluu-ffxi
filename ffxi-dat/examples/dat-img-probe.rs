use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::texture;
use ffxi_dat::{walk, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let file_id: u32 = args[1].parse().unwrap();
    let root = DatRoot::from_env().unwrap();
    let location = root.resolve(file_id).unwrap();
    let bytes = fs::read(location.path_under(root.root())).unwrap();
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();

    let mut ok = 0;
    let mut err = 0;
    let mut samples = 0;
    for (idx, c) in chunks.iter().enumerate() {
        if c.kind != 0x20 {
            continue;
        }
        match texture::decode_texture(c.data) {
            Ok(d) => {
                ok += 1;
                if samples < 12 {
                    let name_at_4: String = c.data[4..]
                        .iter()
                        .take(16)
                        .map(|&b| {
                            if (0x20..0x7f).contains(&b) {
                                b as char
                            } else {
                                '.'
                            }
                        })
                        .collect();
                    println!(
                        "[{idx:>4}] chunk_name={:?}  body_len={:>6}  decoded={:?} {}x{}  hdr@4={:?}",
                        c.name_str(),
                        c.data.len(),
                        d.format_tag,
                        d.width,
                        d.height,
                        name_at_4
                    );
                    samples += 1;
                }
            }
            Err(e) => {
                err += 1;
                if samples < 12 {
                    println!(
                        "[{idx:>4}] chunk_name={:?}  body_len={:>6}  DECODE ERR: {e}",
                        c.name_str(),
                        c.data.len(),
                    );
                    samples += 1;
                }
            }
        }
    }
    println!();
    println!("decode summary: ok={ok} err={err}");
    ExitCode::SUCCESS
}
