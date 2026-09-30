//! Lamp-glow diagnostics for a zone's effect DAT: dumps every generator def under `ligh/`
//! (mesh, init colour/scale, blend) plus the alpha/hue statistics of the lig* halo textures
//! and sheet frame extents — the data needed to judge how a lamp's wall glow should render.

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{
    chunk,
    kind::ChunkKind,
    particle_gen::{KeyFrameTrack, ParticleGeneratorDef},
    sprite_sheet::ParticleSpriteSheet,
    texture, DatRoot,
};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some(arg) = args.get(1) else {
        eprintln!("usage: dat-lamp-glow-probe <file_id> | sheets <file_id>");
        return ExitCode::FAILURE;
    };
    if arg == "gendef" && args.len() >= 4 {
        let root = DatRoot::from_env().unwrap();
        let file_id: u32 = args[2].parse().unwrap();
        let want = args[3].as_bytes();
        let location = root.resolve(file_id).unwrap();
        let bytes = fs::read(location.path_under(&root)).unwrap();
        fn walk(node: &chunk::ChunkNode, dir: [u8; 4], want: &[u8]) {
            for child in &node.children {
                if child.chunk.kind == 0x01 {
                    walk(child, child.chunk.name, want);
                    continue;
                }
                let name = child.chunk.name_str();
                if !name.as_bytes().starts_with(want) {
                    continue;
                }
                if ChunkKind::from_u8(child.chunk.kind) == Some(ChunkKind::Generator) {
                    if let Ok(Some(d)) = ParticleGeneratorDef::parse(child.chunk.data) {
                        println!(
                            "gen dir={:?} {:?} kind={:?} mesh={:?} init_color=({:.3},{:.3},{:.3},{:.3}) blend={:?} billboard={} cam_rel={} tod_a_driven={}",
                            dir, name, d.mesh_kind, d.mesh_id,
                            d.init_color[0], d.init_color[1], d.init_color[2], d.init_color[3],
                            d.blend, d.camera_billboard, d.camera_relative, d.tod_color_driven[3],
                        );
                    }
                }
            }
        }
        walk(&chunk::walk_tree(&bytes), [0; 4], want);
        return ExitCode::SUCCESS;
    }
    if arg == "idat" && args.len() >= 5 {
        let root = DatRoot::from_env().unwrap();
        let rom: String = args[2].clone();
        let dir: u16 = args[3].parse().unwrap();
        let file: u8 = args[4].parse().unwrap();
        match root.id_at(&rom, dir, file) {
            Some(id) => println!("idat {rom} {dir}/{file} -> id {id}"),
            None => println!("idat {rom} {dir}/{file} -> not claimed"),
        }
        return ExitCode::SUCCESS;
    }
    if arg == "info" {
        let root = DatRoot::from_env().unwrap();
        for (rom_dir, vlen, flen) in root.app_summary() {
            println!("app rom={rom_dir} vtable={vlen} ftable={flen}");
        }
        for id in [0u32, 335] {
            match root.resolve(id) {
                Ok(loc) => {
                    let p = &loc.sub_path;
                    println!(
                        "id {id} -> rom={:?} dir={} file={}",
                        loc.rom_dir, p.dir, p.file
                    );
                }
                Err(e) => println!("id {id} -> err {e}"),
            }
        }
        return ExitCode::SUCCESS;
    }
    if arg == "sheets" && args.len() >= 3 {
        let Ok(file_id) = args[2].parse::<u32>() else {
            eprintln!("bad file id {:?}", args[2]);
            return ExitCode::FAILURE;
        };
        list_sheets(file_id);
        return ExitCode::SUCCESS;
    }
    let Ok(file_id) = arg.parse::<u32>() else {
        eprintln!("bad file id {arg:?}");
        return ExitCode::FAILURE;
    };
    let root = DatRoot::from_env().unwrap();
    let location = root.resolve(file_id).unwrap();
    println!("{} -> {}", file_id, location.path_under(&root).display());
    let bytes = fs::read(location.path_under(&root)).unwrap();

    let mut out = String::new();
    let tree = chunk::walk_tree(&bytes);
    let mut keyframes: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    collect_keyframes(&tree, &mut keyframes);
    let mut tod_refs: Vec<[u8; 4]> = Vec::new();
    visit(&tree, [0; 4], &mut out, &mut tod_refs);
    dump_tod(&keyframes, &tod_refs, &mut out);
    print!("{out}");
    ExitCode::SUCCESS
}

fn fourcc(id: &[u8; 4]) -> String {
    String::from_utf8_lossy(id)
        .trim_end_matches('\0')
        .to_string()
}

/// Every SpriteSheet chunk in the file, name + bound texture id — the lookup keys the zone
/// particle loader uses to resolve a generator's mesh.
fn list_sheets(file_id: u32) {
    let root = DatRoot::from_env().unwrap();
    let location = root.resolve(file_id).unwrap();
    println!("{} -> {}", file_id, location.path_under(&root).display());
    let bytes = fs::read(location.path_under(&root)).unwrap();
    fn walk(node: &chunk::ChunkNode) {
        for child in &node.children {
            if child.chunk.kind == 0x01 {
                walk(child);
            } else if ChunkKind::from_u8(child.chunk.kind) == Some(ChunkKind::SpriteSheet) {
                if let Some(ss) = ParticleSpriteSheet::parse(child.chunk.data) {
                    println!(
                        "sheet {:?} ({}:{}) frames={}",
                        child.chunk.name,
                        ss.category,
                        ss.id,
                        ss.frames.len()
                    );
                }
            }
        }
    }
    walk(&chunk::walk_tree(&bytes));
}

fn collect_keyframes(node: &chunk::ChunkNode, out: &mut Vec<([u8; 4], Vec<u8>)>) {
    for child in &node.children {
        if child.chunk.kind == ChunkKind::KeyFrame as u8 {
            out.push((child.chunk.name, child.chunk.data.to_vec()));
        }
        if child.chunk.kind == 0x01 {
            collect_keyframes(child, out);
        }
    }
}

fn dump_tod(keyframes: &[([u8; 4], Vec<u8>)], refs: &[[u8; 4]], out: &mut String) {
    let hours = [0.0f32, 5.88, 12.0, 17.52];
    for id in refs.iter().collect::<std::collections::BTreeSet<_>>() {
        match keyframes.iter().find(|(n, _)| n == id) {
            Some((_, data)) => {
                let track = KeyFrameTrack::parse(data);
                let pts: Vec<(f32, f32)> = track
                    .points
                    .iter()
                    .map(|(t, v)| ((t * 2400.0).round() / 100.0, (v * 1000.0).round() / 1000.0))
                    .collect();
                out.push_str(&format!(
                    "track {} points(t_hours,value)={:?}\n",
                    fourcc(id),
                    pts
                ));
                for h in hours {
                    out.push_str(&format!(
                        "  at {:>5.2}h sample={:.3}\n",
                        h,
                        track.sample(h / 24.0)
                    ));
                }
            }
            None => out.push_str(&format!("track {} not in this DAT\n", fourcc(id))),
        }
    }
}

fn visit(node: &chunk::ChunkNode, dir: [u8; 4], out: &mut String, tod_refs: &mut Vec<[u8; 4]>) {
    for child in &node.children {
        if child.chunk.kind == 0x01 {
            visit(child, child.chunk.name, out, tod_refs);
            continue;
        }
        let name = child.chunk.name_str();
        match ChunkKind::from_u8(child.chunk.kind) {
            Some(ChunkKind::Generator) if dir == *b"ligh" || name.starts_with("lig") => {
                if let Ok(Some(d)) = ParticleGeneratorDef::parse(child.chunk.data) {
                    out.push_str(&format!(
                        "gen {:?}/{:?} kind={:?} mesh={:?} init_color=({:.3},{:.3},{:.3},{:.3}) \
                         init_scale=({:.2},{:.2},{:.2}) blend={:?} billboard={:?} life={}f \
                         base_pos=({:.3},{:.3},{:.3}) auto_run={} continuous={} camera_billboard={} follow_camera={} attached_base={}\n",
                        dir,
                        child.chunk.name,
                        d.mesh_kind,
                        d.mesh_id,
                        d.init_color[0],
                        d.init_color[1],
                        d.init_color[2],
                        d.init_color[3],
                        d.init_scale[0],
                        d.init_scale[1],
                        d.init_scale[2],
                        d.blend,
                        d.billboard,
                        d.max_life_frames.round() as u32,
                        d.base_position[0],
                        d.base_position[1],
                        d.base_position[2],
                        d.auto_run,
                        d.continuous,
                        d.camera_billboard,
                        d.follow_camera,
                        d.camera_attached_base,
                    ));
                    for (ch, id) in d.tod_color_tracks.iter().enumerate() {
                        if let Some(id) = id {
                            out.push_str(&format!(
                                "  gen {}/{} tod ch{} track={} driven={}\n",
                                fourcc(&dir),
                                fourcc(&child.chunk.name),
                                ch,
                                fourcc(id),
                                d.tod_color_driven[ch]
                            ));
                            tod_refs.push(*id);
                        }
                    }
                } else {
                    out.push_str(&format!(
                        "gen {:?}/{:?} (no particle def)\n",
                        dir, child.chunk.name
                    ));
                }
            }
            Some(ChunkKind::Img) if name.starts_with("lig") => {
                match texture::decode_texture(child.chunk.data) {
                    Ok(t) => image_stats(&name, &t, out),
                    Err(e) => out.push_str(&format!("img {name}: decode err {e}\n")),
                }
            }
            Some(ChunkKind::SpriteSheet) if name.starts_with("lig") => {
                if let Some(ss) = ParticleSpriteSheet::parse(child.chunk.data) {
                    for f in &ss.frames {
                        if f.positions.is_empty() {
                            continue;
                        }
                        let xs: Vec<f32> = f.positions.iter().map(|p| p[0]).collect();
                        let ys: Vec<f32> = f.positions.iter().map(|p| p[1]).collect();
                        out.push_str(&format!(
                            "sheet {:?} ({}:{}) frames={} quads={} pos=[{:.2},{:.2}]..[{:.2},{:.2}]\n",
                            name, ss.category, ss.id, ss.frames.len(), f.positions.len() / 4,
                            xs.iter().cloned().fold(f32::INFINITY, f32::min),
                            ys.iter().cloned().fold(f32::INFINITY, f32::min),
                            xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                            ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                        ));
                    }
                }
            }
            Some(ChunkKind::D3m) if name.starts_with("li") || name.starts_with("ghu") => {
                if let Ok(d) = ffxi_dat::d3m::D3m::parse(child.chunk.name, child.chunk.data) {
                    out.push_str(&format!("(dir={:?}) ", dir));
                    d3m_color_stats(&name, &d, out);
                }
            }
            _ => {}
        }
    }
}

// The fixture mesh is untextured: its whole look is vertex colour x TFACTOR (the D3m
// untextured table), so the authored vertex range IS the glass glow.
fn d3m_color_stats(name: &str, d: &ffxi_dat::d3m::D3m, out: &mut String) {
    let n = d.vertices.len() as f64;
    if n == 0.0 {
        return;
    }
    let mut sum = [0f64; 4];
    let (mut lo, mut hi) = ([f64::INFINITY; 4], [f64::NEG_INFINITY; 4]);
    for v in &d.vertices {
        for c in 0..4 {
            let x = v.color[c] as f64;
            sum[c] += x;
            lo[c] = lo[c].min(x);
            hi[c] = hi[c].max(x);
        }
    }
    out.push_str(&format!(
        "d3m {name} verts={} rgb_a=[lo {:.2}/{:.2}/{:.2}/{:.2}  mean {:.2}/{:.2}/{:.2}/{:.2}  hi {:.2}/{:.2}/{:.2}/{:.2}]\n",
        d.vertices.len(),
        lo[0], lo[1], lo[2], lo[3],
        sum[0] / n, sum[1] / n, sum[2] / n, sum[3] / n,
        hi[0], hi[1], hi[2], hi[3],
    ));
}

fn image_stats(name: &str, t: &texture::DecodedTexture, out: &mut String) {
    let mut alphas = Vec::with_capacity(t.rgba.len() / 4);
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for px in t.rgba.chunks_exact(4) {
        alphas.push(px[3]);
        if px[3] > 128 {
            r += u64::from(px[0]);
            g += u64::from(px[1]);
            b += u64::from(px[2]);
            n += 1;
        }
    }
    alphas.sort_unstable();
    let q = |p: usize| alphas.get(alphas.len() * p / 100).copied().unwrap_or(0);
    out.push_str(&format!(
        "img {name} {}x{} alpha[min/p25/med/p75/max]={}/{}/{}/{}/{} core_rgb=({:.3},{:.3},{:.3}) n={}\n",
        t.width, t.height, q(0), q(25), q(50), q(75), q(99),
        if n > 0 { r as f32 / n as f32 } else { 0.0 },
        if n > 0 { g as f32 / n as f32 } else { 0.0 },
        if n > 0 { b as f32 / n as f32 } else { 0.0 },
        n,
    ));
}
