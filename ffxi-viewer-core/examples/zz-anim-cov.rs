use ffxi_dat::resource_dir::ResourceDir;
use ffxi_dat::DatRoot;
use std::collections::BTreeSet;
use std::env;
use std::fs;

fn read(file_id: u32) -> Option<Vec<u8>> {
    let root = DatRoot::from_env_or_default().ok()?;
    let loc = root.resolve(file_id).ok()?;
    fs::read(loc.path_under(root.root())).ok()
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let skel_id: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(7072);
    let motion_id: u32 = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(skel_id + 2600);
    let want: Vec<String> = args.iter().skip(3).cloned().collect();
    let want: Vec<&str> = if want.is_empty() {
        vec!["run0", "run1", "idl0", "wlk0", "wlk1"]
    } else {
        want.iter().map(|s| s.as_str()).collect()
    };

    if let Some(bytes) = read(skel_id) {
        if let Some(skel) = ResourceDir::from_bytes(bytes)
            .collect_skeletons()
            .into_iter()
            .next()
        {
            println!("skeleton {skel_id}: joints={}", skel.joints.len());
            for (name, idx) in [
                ("AboveHead", 2usize),
                ("RightFoot", 8),
                ("LeftFoot", 9),
                ("LeftHand", 126),
                ("RightHand", 127),
            ] {
                let joint = skel.references.get(idx).map(|r| r.index);
                println!("  {name:<10} ref[{idx:>3}] -> joint {joint:?}");
            }

            print!("  parents: ");
            for (i, j) in skel.joints.iter().enumerate() {
                print!("{i}:{:?} ", j.parent);
                if i % 12 == 11 {
                    print!("\n           ");
                }
            }
            println!();
        }
    }

    let mut all = Vec::new();
    for id in [skel_id, motion_id] {
        if let Some(bytes) = read(id) {
            all.extend(ResourceDir::from_bytes(bytes).collect_animations());
        }
    }
    for name in want {
        if let Some(a) = all.iter().find(|a| a.id.as_str() == name) {
            let set: BTreeSet<u32> = a.key_frame_sets.keys().copied().collect();
            let v: Vec<u32> = set.into_iter().collect();
            println!("clip {name}: {} joints = {:?}", v.len(), v);
        } else {
            println!("clip {name}: NOT FOUND");
        }
    }
}
