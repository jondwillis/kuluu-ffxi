use ffxi_dat::resource_dir::ResourceDir;
use ffxi_dat::skel_mesh::MeshType;
use ffxi_dat::{walk_tree, ChunkKind, ChunkNode, DatRoot};
use std::fs;

fn count_chunks<'a>(node: &ChunkNode<'a>, out: &mut std::collections::BTreeMap<u8, usize>) {
    *out.entry(node.chunk.kind).or_default() += 1;
    for c in &node.children {
        count_chunks(c, out);
    }
}

fn main() {
    let file_id: u32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1308);
    let root = DatRoot::from_env_or_default().expect("DatRoot");
    let loc = root.resolve(file_id).expect("resolve");
    let bytes = fs::read(loc.path_under(root.root())).expect("read");
    println!("file_id={file_id} bytes={}", bytes.len());

    let mut kinds = std::collections::BTreeMap::new();
    count_chunks(&walk_tree(&bytes), &mut kinds);
    println!("chunk kinds:");
    for (k, c) in &kinds {
        let name = ChunkKind::from_u8(*k)
            .map(|x| format!("{x:?}"))
            .unwrap_or_else(|| "?".into());
        println!("  0x{k:02x} {name} x{c}");
    }

    fn walk_d3m<'a>(node: &ChunkNode<'a>, out: &mut Vec<ffxi_dat::d3m::D3m>) {
        if ChunkKind::from_u8(node.chunk.kind) == Some(ChunkKind::D3m) {
            if let Ok(d) = ffxi_dat::d3m::D3m::parse(node.chunk.name, node.chunk.data) {
                out.push(d);
            }
        }
        for c in &node.children {
            walk_d3m(c, out);
        }
    }
    let mut d3ms = Vec::new();
    walk_d3m(&walk_tree(&bytes), &mut d3ms);
    println!("D3M chunks: {}", d3ms.len());
    for (i, d) in d3ms.iter().enumerate() {
        let (mut xmn, mut xmx, mut ymn, mut ymx, mut zmn, mut zmx) = (
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
        );
        for v in &d.vertices {
            xmn = xmn.min(v.pos[0]);
            xmx = xmx.max(v.pos[0]);
            ymn = ymn.min(v.pos[1]);
            ymx = ymx.max(v.pos[1]);
            zmn = zmn.min(v.pos[2]);
            zmx = zmx.max(v.pos[2]);
        }
        println!(
            "  d3m[{i}] name={:?} tris={} tex='{}' bbox x[{xmn:.3},{xmx:.3}] y[{ymn:.3},{ymx:.3}] z[{zmn:.3},{zmx:.3}]",
            String::from_utf8_lossy(&d.name), d.num_triangles, d.texture_name_str()
        );
        for v in d.vertices.iter().take(3) {
            println!("     v pos={:?} uv={:?} col={:?}", v.pos, v.uv, v.color);
        }
    }

    fn walk_img<'a>(node: &ChunkNode<'a>, out: &mut Vec<String>) {
        if ChunkKind::from_u8(node.chunk.kind) == Some(ChunkKind::Img) {
            let raw: String = node
                .chunk
                .data
                .get(1..0x11)
                .map(|b| b.iter().map(|&c| c as char).collect())
                .unwrap_or_default();
            out.push(format!("{:?}", raw));
        }
        for c in &node.children {
            walk_img(c, out);
        }
    }
    let mut imgs = Vec::new();
    walk_img(&walk_tree(&bytes), &mut imgs);
    println!("Img names: {:?}", imgs);

    let dir = ResourceDir::from_bytes(bytes.clone());
    let skels = dir.collect_skeletons();
    let meshes = dir.collect_skel_meshes();
    println!("skels={} skel_meshes={}", skels.len(), meshes.len());
    if let Some(sk) = skels.first() {
        println!("joints={}", sk.joints.len());
        use ffxi_actor::skeleton_instance::{pose_world, RootTransform};
        let pose = pose_world(
            sk,
            |_| None,
            RootTransform {
                facing_dir: 0.0,
                skew: 0.0,
                slope_oriented: false,
                scale: ffxi_actor::Vec3::splat(1.0),
            },
            &[],
        );
        for (i, m) in pose.iter().enumerate() {
            let t = m.w_axis;
            println!(
                "  joint[{i}] world pos=({:.3},{:.3},{:.3}) parent={:?}",
                t.x, t.y, t.z, sk.joints[i].parent
            );
        }
    }
    for (mi, m) in meshes.iter().enumerate() {
        println!(
            "mesh[{mi}] occlude_type={} buffers={}",
            m.occlude_type,
            m.meshes.len()
        );
        for (bi, b) in m.meshes.iter().enumerate() {
            let mt = match b.mesh_type {
                MeshType::Strip => "Strip",
                MeshType::Mesh => "Mesh",
            };
            let (mut xmn, mut xmx, mut ymn, mut ymx, mut zmn, mut zmx) = (
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
            );
            for v in &b.vertices {
                xmn = xmn.min(v.p0[0]);
                xmx = xmx.max(v.p0[0]);
                ymn = ymn.min(v.p0[1]);
                ymx = ymx.max(v.p0[1]);
                zmn = zmn.min(v.p0[2]);
                zmx = zmx.max(v.p0[2]);
            }
            println!(
                "  buf[{bi}] type={mt} tex='{}' dtf={} nverts={} p0 bbox x[{xmn:.3},{xmx:.3}] y[{ymn:.3},{ymx:.3}] z[{zmn:.3},{zmx:.3}]",
                b.texture_name.trim_end_matches(['\0', ' ']),
                b.render_properties.display_type_flag,
                b.vertices.len(),
            );
            for v in b.vertices.iter().take(3) {
                println!(
                    "     v p0={:?} u={:.3} v={:.3} w={:.3} j0={} j1={} col={:?}",
                    v.p0, v.u, v.v, v.joint0_weight, v.joint_index0, v.joint_index1, v.color
                );
            }
        }
    }
}
