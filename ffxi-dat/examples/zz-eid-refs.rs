//! Which way do the shipped skeleton EID reference points point? Dump refs 0..48 for a race's
//! race-config skeleton file (body center at 21, name tag at 2, feet at 8/9) so the sign of "up"
//! in the model frame is settled from real data, not from tests.
use ffxi_dat::{resource_dir::ResourceDir, DatRoot};
use std::process::ExitCode;

fn main() -> ExitCode {
    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let dll = match ffxi_dat::main_dll::MainDll::load(root.root()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    for race in [1u8, 2, 32, 33] {
        let Some(cfg) = dll.base_race_config_index(race) else {
            println!("race {race}: no race-config");
            continue;
        };
        let file_id = u32::from(cfg);
        let Ok(loc) = root.resolve(file_id) else {
            println!("race {race}: cfg {file_id} unresolved");
            continue;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            println!("race {race}: read fail");
            continue;
        };
        let dir = ResourceDir::from_bytes(bytes);
        let Some(skel) = dir.collect_skeletons().into_iter().next() else {
            println!("race {race} file {file_id}: no skeleton");
            continue;
        };
        println!(
            "race {race} file {file_id} joints={} refs={}",
            skel.joints.len(),
            skel.references.len()
        );
        for (i, r) in skel
            .references
            .iter()
            .enumerate()
            .take(ffxi_dat::camera::EID_NORMAL_MAX as usize)
        {
            if i == 0 || i <= 12 || (19..=24).contains(&i) || i == 32 || i == 42 {
                println!(
                    "  ref {i:2}: idx={:3} off=[{:8.4},{:8.4},{:8.4}] rot=[{:.2},{:.2},{:.2}]",
                    r.index,
                    r.position_offset[0],
                    r.position_offset[1],
                    r.position_offset[2],
                    r.rotation[0],
                    r.rotation[1],
                    r.rotation[2]
                );
            }
        }
    }
    ExitCode::SUCCESS
}
