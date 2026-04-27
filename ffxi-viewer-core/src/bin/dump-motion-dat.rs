use std::fs;

use ffxi_dat::{walk, ChunkKind, DatRoot};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let Some(motion_id_str) = args.get(1) else {
        eprintln!(
            "usage: {} <motion_dat_id>",
            args.first()
                .map(String::as_str)
                .unwrap_or("dump-motion-dat")
        );
        eprintln!();
        eprintln!("Motion DAT ids (from lotus PCSkeletonIDs):");
        eprintln!("  9672 Hume M   12848 Hume F   16024 Elv M   19200 Elv F");
        eprintln!(" 22376 Taru     25776 Mithra   28952 Galka");
        std::process::exit(2);
    };
    let motion_id: u32 = motion_id_str.parse()?;

    let root = DatRoot::from_env_or_default()?;
    let loc = root.resolve(motion_id)?;
    let bytes = fs::read(loc.path_under(root.root()))?;

    println!(
        "motion DAT {motion_id}: {}",
        loc.path_under(root.root()).display()
    );
    let mut mo2_count = 0;
    for chunk in walk(&bytes).filter_map(Result::ok) {
        if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::AnimMo2) {
            continue;
        }
        mo2_count += 1;
        let name = String::from_utf8_lossy(&chunk.name);
        match ffxi_dat::anim::parse_mo2(chunk.data, &chunk.name) {
            Ok(anim) => println!(
                "  MO2 {:?} ({} frames, speed={:.2}, bones={})",
                name,
                anim.frames,
                anim.speed,
                anim.per_bone.len()
            ),
            Err(e) => println!("  MO2 {:?} (parse error: {e})", name),
        }
    }
    println!("total MO2 chunks: {mo2_count}");
    Ok(())
}
