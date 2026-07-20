use std::env;
use std::process::ExitCode;

use ffxi_dat::chunk::walk;
use ffxi_dat::kind::ChunkKind;
use ffxi_dat::particle_gen::ParticleGeneratorDef;
use ffxi_dat::resource_dir::ResourceDir;
use ffxi_dat::DatRoot;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some(file_id_arg) = args.get(1) else {
        eprintln!("usage: dat-skel-renderprops <file_id>");
        return ExitCode::from(2);
    };
    let file_id: u32 = file_id_arg.parse().expect("file_id");

    let root = DatRoot::from_env_or_default().expect("DAT root");
    let loc = root.resolve(file_id).expect("resolve");
    let bytes = std::fs::read(loc.path_under(root.root())).expect("read");

    let dir = ResourceDir::from_bytes(bytes.clone());
    for sm in dir.collect_skel_meshes() {
        println!(
            "skel_mesh id={} occlude={} meshes={}",
            sm.id.as_str(),
            sm.occlude_type,
            sm.meshes.len()
        );
        for (i, m) in sm.meshes.iter().enumerate() {
            let rp = &m.render_properties;
            let alpha_min = m.vertices.iter().map(|v| v.color[3]).min().unwrap_or(0);
            let alpha_max = m.vertices.iter().map(|v| v.color[3]).max().unwrap_or(0);
            println!(
                "  mesh[{i}] type={:?} tex={:?} verts={} | t_factor={:?} spec={}({}) display={} amb={} flag0={:#04x} flag2={:#04x} flag3={:#04x} vtx_alpha=[{alpha_min},{alpha_max}]",
                m.mesh_type,
                m.texture_name.trim_end_matches(['\0', ' ']),
                m.vertices.len(),
                rp.t_factor,
                rp.specular_highlight_enabled,
                rp.specular_highlight_power,
                rp.display_type_flag,
                rp.ambient_multiplier,
                rp.flag0,
                rp.flag2,
                rp.flag3,
            );
        }
    }

    for c in walk(&bytes).flatten() {
        match ChunkKind::from_u8(c.kind) {
            Some(ChunkKind::Generator) => {
                if let Ok(Some(def)) = ParticleGeneratorDef::parse(c.data) {
                    println!(
                        "generator {:?} auto_run={} continuous={} mesh={:?} blend={:?} fpe={} ppe={} life={} color={:?} billboard={} scale={:?} rot={:?} base={:?}",
                        String::from_utf8_lossy(&c.name),
                        def.auto_run,
                        def.continuous,
                        String::from_utf8_lossy(&def.mesh_id),
                        def.blend,
                        def.frames_per_emission,
                        def.particles_per_emission,
                        def.max_life_frames,
                        def.init_color,
                        def.camera_billboard,
                        def.init_scale,
                        def.init_rotation,
                        def.base_position,
                    );
                }
            }
            Some(ChunkKind::D3m) => {
                if let Ok(d) = ffxi_dat::d3m::D3m::parse(c.name, c.data) {
                    println!(
                        "d3m {:?} tris={} tex={:?}",
                        String::from_utf8_lossy(&c.name),
                        d.num_triangles,
                        d.texture_name_str()
                    );
                }
            }
            _ => {}
        }
    }
    ExitCode::SUCCESS
}
