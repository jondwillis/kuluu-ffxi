fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "usage: {} <skel_file_id> <mesh_file_id> <mesh_chunk_idx>",
            args.first()
                .map(String::as_str)
                .unwrap_or("ffxi-skin-probe")
        );
        std::process::exit(2);
    }
    let parse = |idx: usize, name: &str| -> u64 {
        args[idx].parse().unwrap_or_else(|e| {
            eprintln!("bad {name}: {e}");
            std::process::exit(2);
        })
    };
    let skel = parse(1, "skel_file_id") as u32;
    let mesh = parse(2, "mesh_file_id") as u32;
    let chunk = parse(3, "chunk_idx") as usize;

    ffxi_viewer_core::dat_vos2::probe_skinned_actor(skel, mesh, chunk);
}
