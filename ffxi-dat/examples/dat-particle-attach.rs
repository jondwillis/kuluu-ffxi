use std::process::ExitCode;

use ffxi_dat::{
    chunk::walk,
    kind::ChunkKind,
    particle_gen::{
        ParticleGeneratorDef, ATTACH_JOINT0_MASK, ATTACH_JOINT0_SHIFT, ATTACH_JOINT1_MASK,
        ATTACH_JOINT1_SHIFT,
    },
    DatRoot,
};

fn attach_name(flag: u16) -> &'static str {
    match flag & 0x0F {
        0x0 => "None",
        0x1 => "SourceActor",
        0x2 => "TargetActor",
        0x3 => "SourceToTargetBasis",
        0x4 => "TargetActorSourceFacing",
        0x5 => "SourceActorTargetFacing",
        0x6 => "TargetToSourceBasis",
        0x9 => "SourceActorWeapon",
        0xA => "ZoneActor0xA",
        0xB => "ZoneActor0xB",
        0xC => "ZoneActor0xC",
        0xE => "Sun",
        0xF => "Moon",
        _ => "?",
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: FFXI_DAT_PATH=<path> dat-particle-attach <file_id>...");
        return ExitCode::from(2);
    }
    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("could not open DAT root: {e}");
            return ExitCode::from(1);
        }
    };
    for arg in &args {
        let Ok(file_id) = arg.parse::<u32>() else {
            continue;
        };
        let Ok(loc) = root.resolve(file_id) else {
            eprintln!("{file_id}: unresolvable");
            continue;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            eprintln!("{file_id}: unreadable");
            continue;
        };
        println!("== file {file_id} ({} bytes)", bytes.len());
        for c in walk(&bytes).flatten() {
            if ChunkKind::from_u8(c.kind) != Some(ChunkKind::Generator) {
                continue;
            }
            let name = String::from_utf8_lossy(&c.name).trim_end().to_string();
            if c.data.len() < 4 {
                continue;
            }
            let attach_flags = u16::from_le_bytes([c.data[0], c.data[1]]);
            let extra = u16::from_le_bytes([c.data[2], c.data[3]]);
            let parsed = ParticleGeneratorDef::parse(c.data).ok().flatten();
            println!(
                "  {name:<6} attach=0x{:04X} -> {:<24} j0={} j1={} extra=0x{extra:04X}",
                attach_flags,
                attach_name(attach_flags),
                (attach_flags & ATTACH_JOINT0_MASK) >> ATTACH_JOINT0_SHIFT,
                (attach_flags & ATTACH_JOINT1_MASK) >> ATTACH_JOINT1_SHIFT,
            );
            if let Some(d) = parsed {
                println!(
                    "         mesh={} kind={:?} blend={:?} color={:?} life={} bb={} auto={}",
                    String::from_utf8_lossy(&d.mesh_id).trim_end(),
                    d.mesh_kind,
                    d.blend,
                    d.init_color,
                    d.max_life_frames,
                    d.camera_billboard,
                    d.auto_run,
                );
            }
        }
    }
    ExitCode::SUCCESS
}
