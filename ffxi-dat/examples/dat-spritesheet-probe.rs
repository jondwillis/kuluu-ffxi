use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::zone_dat::ZONE_DAT_TABLE;
use ffxi_dat::{walk, DatRoot};

fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn ascii(b: &[u8]) -> String {
    b.iter()
        .map(|&c| {
            if (0x20..0x7f).contains(&c) {
                c as char
            } else {
                '.'
            }
        })
        .collect()
}

// Parse a 0x21 sprite-sheet body (XIM SpriteSheetSection layout) and dump frames.
fn dump_sprite_sheet(b: &[u8]) {
    let unk_flag = rd_u16(b, 0);
    let num_mesh = rd_u16(b, 2);
    let lens = b[4];
    let norm = b[7];
    let tex_name = &b[8..24];
    println!(
        "  0x21: unkFlag={unk_flag} numMesh={num_mesh} lens={lens} normFlag={norm} texName=\"{}\" [{}]",
        ascii(tex_name),
        tex_name
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let unnormalized = unk_flag == 1 && norm == 0;
    let uv_scale = if unnormalized { 1.0 / 256.0 } else { 1.0 };

    let mut p = 24usize;
    for i in 0..num_mesh {
        if p + 4 > b.len() {
            println!("    [frame {i}] truncated");
            return;
        }
        let unk1 = rd_u16(b, p);
        let num_quads = b[p + 2];
        let unk2 = b[p + 3];
        p += 4;
        if lens == 1 {
            p += 16;
        }
        let num_verts = 6 * num_quads as usize;
        let (mut umin, mut vmin, mut umax, mut vmax) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let (mut xmin, mut xmax, mut ymin, mut ymax) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for _ in 0..num_verts {
            if p + 24 > b.len() {
                break;
            }
            let x = rd_f32(b, p);
            let y = rd_f32(b, p + 4);
            // z at p+8, rgba at p+12
            let u = rd_f32(b, p + 16) * uv_scale;
            let v = rd_f32(b, p + 20) * uv_scale;
            umin = umin.min(u);
            umax = umax.max(u);
            vmin = vmin.min(v);
            vmax = vmax.max(v);
            xmin = xmin.min(x);
            xmax = xmax.max(x);
            ymin = ymin.min(y);
            ymax = ymax.max(y);
            p += 24;
        }
        println!(
            "    frame {i:>2}: unk1={unk1} quads={num_quads} unk2={unk2}  uv=[{umin:.4},{vmin:.4}]..[{umax:.4},{vmax:.4}]  pos=[{xmin:.1},{ymin:.1}]..[{xmax:.1},{ymax:.1}]"
        );
    }
}

fn process(bytes: &[u8]) -> bool {
    let mut found = false;
    for c in walk(bytes).filter_map(Result::ok) {
        if c.kind != 0x21 {
            continue;
        }
        let b = c.data;
        if b.len() < 24 {
            continue;
        }
        let num_mesh = rd_u16(b, 2);
        let name = ascii(&b[8..24]);
        if num_mesh == 12 && b[4] == 0 && name.starts_with("moon") {
            dump_sprite_sheet(b);
            found = true;
        }
    }
    if found {
        match ffxi_dat::sprite_sheet::extract_moon_sprite_sheet(bytes) {
            Some(ms) => {
                let a: Vec<u8> = ms.texture.rgba.iter().skip(3).step_by(4).copied().collect();
                let amin = a.iter().copied().min().unwrap_or(0);
                let amax = a.iter().copied().max().unwrap_or(0);
                let azero = a.iter().filter(|&&x| x == 0).count();
                let afull = a.iter().filter(|&&x| x == 255).count();
                // alpha sampled inside frame 6 (full moon) center vs corner
                let w = ms.texture.width as usize;
                let f = ms.frames[6];
                let cx = ((f.u0 + f.u1) * 0.5 * w as f32) as usize;
                let cy = ((f.v0 + f.v1) * 0.5 * w as f32) as usize;
                let a_center = ms.texture.rgba[(cy * w + cx) * 4 + 3];
                println!(
                    "  --- extract_moon_sprite_sheet: {} frames, tex {}x{} frame0={:?} ---",
                    ms.frames.len(),
                    ms.texture.width,
                    ms.texture.height,
                    ms.frames[0],
                );
                println!(
                    "      alpha: min={amin} max={amax} zero%={:.1} full%={:.1} fullmoon_center_a={a_center}",
                    100.0 * azero as f32 / a.len() as f32,
                    100.0 * afull as f32 / a.len() as f32,
                );
            }
            None => println!("  --- extract_moon_sprite_sheet: NONE ---"),
        }
        println!("  --- graphics named *moon* in this file (scan_graphics) ---");
        for g in ffxi_dat::map_image::scan_graphics(bytes) {
            let name = format!("{}/{}", g.category, g.id);
            if name.to_lowercase().contains("moon") {
                println!(
                    "    graphic cat=\"{}\" id=\"{}\" {}x{} rgba={}B",
                    g.category,
                    g.id,
                    g.width,
                    g.height,
                    g.rgba.len()
                );
            }
        }
    }
    found
}

fn main() -> ExitCode {
    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("DAT root unavailable: {e}");
            return ExitCode::from(2);
        }
    };
    let args: Vec<String> = env::args().collect();
    if args.len() >= 3 && args[1] == "--zone" {
        let zone_id: u16 = args[2].parse().unwrap_or(0);
        let Some(fid) = ffxi_dat::zone_dat::zone_id_to_mzb_file_id(zone_id) else {
            eprintln!("zone {zone_id} not in table");
            return ExitCode::from(2);
        };
        let loc = root.resolve(fid).unwrap();
        let bytes = fs::read(loc.path_under(root.root())).unwrap();
        println!("# zone {zone_id} file {fid} ({} bytes)", bytes.len());
        process(&bytes);
        return ExitCode::SUCCESS;
    }

    // default: find first zone containing the moon sprite sheet
    for &(zone_id, file_id) in ZONE_DAT_TABLE {
        let Ok(loc) = root.resolve(file_id) else {
            continue;
        };
        let Ok(bytes) = fs::read(loc.path_under(root.root())) else {
            continue;
        };
        let mut has = false;
        for c in walk(&bytes).filter_map(Result::ok) {
            if c.kind == 0x21 && c.data.len() >= 24 && rd_u16(c.data, 2) == 12 && c.data[4] == 0 {
                let name = ascii(&c.data[8..24]);
                if name.starts_with("moon") {
                    has = true;
                    break;
                }
            }
        }
        if has {
            println!(
                "# first moon zone {zone_id} file {file_id} ({} bytes)",
                bytes.len()
            );
            process(&bytes);
            break;
        }
    }
    ExitCode::SUCCESS
}
