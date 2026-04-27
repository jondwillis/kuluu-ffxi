use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{walk_tree, ChunkNode, DatRoot};

fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn rd_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn name4(n: &[u8; 4]) -> String {
    n.iter()
        .map(|&c| {
            if (0x20..0x7f).contains(&c) {
                c as char
            } else {
                '.'
            }
        })
        .collect()
}

// Extract the StandardParticleSetup (opcode 0x01) from a 0x05 generator body,
// reusing the validated layout in generator.rs.
struct Setup {
    attach: u8,
    config: u16,
    render: u16,
    linked_id: [u8; 4],
    linked_type: u8,
    base_pos: [f32; 3],
}

fn parse_setup(body: &[u8]) -> Option<Setup> {
    if body.len() < 0x80 {
        return None;
    }
    let attach = body[0] & 0x0F;
    let creation_offset = rd_u32(body, 0x74) as usize;
    if creation_offset < 16 || creation_offset - 16 >= body.len() {
        return None;
    }
    let mut c = creation_offset - 16;
    while c + 4 <= body.len() {
        let opcode = body[c];
        if opcode == 0x00 {
            break;
        }
        let size_words = (body[c + 1] & 0x1F) as usize;
        if size_words == 0 {
            break;
        }
        let block_len = size_words * 4;
        let payload = c + 4;
        if c + block_len > body.len() {
            break;
        }
        if opcode == 0x01 && payload + 30 <= body.len() {
            return Some(Setup {
                attach,
                config: rd_u16(body, payload),
                render: rd_u16(body, payload + 2),
                linked_id: [
                    body[payload + 8],
                    body[payload + 9],
                    body[payload + 10],
                    body[payload + 11],
                ],
                linked_type: body[payload + 29],
                base_pos: [
                    rd_f32(body, payload + 16),
                    rd_f32(body, payload + 20),
                    rd_f32(body, payload + 24),
                ],
            });
        }
        c += block_len;
    }
    None
}

fn linked_label(t: u8) -> &'static str {
    match t {
        0x01 => "Actor",
        0x0B => "StaticMesh(0x1F)",
        0x0E => "SpriteSheet(0x21)",
        0x1D => "WeightedMesh",
        0x39 => "LensFlare",
        0x3D => "Audio",
        0x47 => "PointLight",
        0x57 => "Null",
        _ => "?",
    }
}

fn visit(node: &ChunkNode, path: &[String]) {
    let here = name4(&node.chunk.name);
    let mut child_path = path.to_vec();
    if node.chunk.kind == 0x01 {
        child_path.push(here.clone());
    }
    if node.chunk.kind == 0x1F {
        let nm = name4(&node.chunk.name);
        match ffxi_dat::d3m::D3m::parse(node.chunk.name, node.chunk.data) {
            Ok(m) => println!(
                "  MESH[{}] {:<5} tris={:<4} tex=\"{}\"",
                path.join("/"),
                nm,
                m.num_triangles,
                m.texture_name_str()
            ),
            Err(e) => println!("  MESH[{}] {:<5} parse-err: {e}", path.join("/"), nm),
        }
    }
    if node.chunk.kind == 0x05 {
        if let Some(s) = parse_setup(node.chunk.data) {
            let follow_cam = s.config & 0x0004 != 0;
            println!(
                "{:<28} gen={:<5} attach=0x{:X} cfg=0x{:04x}{} render=0x{:04x} linked={:<17} id={:<5} base=[{:.0},{:.0},{:.0}] body={}",
                path.join("/"),
                name4(&node.chunk.name),
                s.attach,
                s.config,
                if follow_cam { "(camFollow)" } else { "" },
                s.render,
                linked_label(s.linked_type),
                name4(&s.linked_id),
                s.base_pos[0],
                s.base_pos[1],
                s.base_pos[2],
                node.chunk.data.len(),
            );
        }
    }
    for c in &node.children {
        visit(c, &child_path);
    }
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
    let fid: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(101);
    let loc = root.resolve(fid).unwrap();
    let bytes = fs::read(loc.path_under(root.root())).unwrap();
    println!(
        "# file {fid} ({} bytes) — all 0x05 generators by dir path",
        bytes.len()
    );
    let tree = walk_tree(&bytes);
    visit(&tree, &[]);
    ExitCode::SUCCESS
}
