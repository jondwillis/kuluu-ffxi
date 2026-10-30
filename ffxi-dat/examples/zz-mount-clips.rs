//! Full clip census across the per-race action-animation block (base..base+N): every offset, every motion id.
//! Answers where `chi0`/`chi1`/`chi2` (chair sit/stand routines, xim Actor.kt updateSitChairDisplay) and any
//! mount get-on/get-off motions live among the offsets kuluu knows (`ACTION_ANIM_MOUNT_OFFSET`, fishing +0x01).
//! `cargo run -p ffxi-dat --example zz-mount-clips [races...]`

use std::process::ExitCode;

use ffxi_dat::{resource_dir::ResourceDir, DatRoot};

fn main() -> ExitCode {
    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("dat root: {e}");
            return ExitCode::from(1);
        }
    };
    let dll = match ffxi_dat::main_dll::MainDll::load(root.root()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("main dll: {e}");
            return ExitCode::from(1);
        }
    };
    let races: Vec<u8> = {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.is_empty() {
            vec![1, 2, 3, 4, 5, 6, 7]
        } else {
            args.iter().filter_map(|a| a.parse().ok()).collect()
        }
    };
    for race in races {
        let Some(base) = dll.base_action_animation_index(race) else {
            println!("race {race}: no action-animation base");
            continue;
        };
        println!("race {race}: action-anim base={base}");
        if let Some(cfg) = dll.base_race_config_index(race) {
            println!("race {race}: race-config base={cfg}");
            for offset in 0u16..=4 {
                let file_id = u32::from(cfg + offset);
                let Ok(loc) = root.resolve(file_id) else {
                    continue;
                };
                let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
                    continue;
                };
                let dir = ResourceDir::from_bytes(bytes);
                let mut clips: Vec<String> = dir
                    .collect_animations()
                    .into_iter()
                    .map(|a| format!("{}({})", a.id.as_str(), a.num_frames))
                    .collect();
                clips.sort();
                println!(
                    "  cfg+{offset} file_id={file_id} {}",
                    if clips.is_empty() {
                        "<no motions>".to_string()
                    } else {
                        clips.join(" ")
                    }
                );
            }
        }
        for offset in 0u16..=8 {
            let file_id = u32::from(base + offset);
            let Ok(loc) = root.resolve(file_id) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
                continue;
            };
            let dir = ResourceDir::from_bytes(bytes);
            let mut clips: Vec<String> = dir
                .collect_animations()
                .into_iter()
                .map(|a| format!("{}({})", a.id.as_str(), a.num_frames))
                .collect();
            clips.sort();
            println!(
                "  +{offset} file_id={file_id} {}",
                if clips.is_empty() {
                    "<no motions>".to_string()
                } else {
                    clips.join(" ")
                }
            );
        }
    }
    ExitCode::SUCCESS
}
