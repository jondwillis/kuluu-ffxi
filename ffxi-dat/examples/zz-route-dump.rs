use std::process::ExitCode;

use ffxi_dat::{walk, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: zz-route-dump <file_id> <name> [<name>...]");
        return ExitCode::from(2);
    }
    let Ok(file_id) = args[0].parse::<u32>() else {
        return ExitCode::from(2);
    };
    let want: Vec<[u8; 4]> = args[1..]
        .iter()
        .map(|s| {
            let mut k = *b"    ";
            k[..s.len().min(4)].copy_from_slice(&s.as_bytes()[..s.len().min(4)]);
            k
        })
        .collect();
    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("could not open DAT root: {e}");
            return ExitCode::from(1);
        }
    };
    let Ok(loc) = root.resolve(file_id) else {
        eprintln!("{file_id}: unresolvable");
        return ExitCode::from(2);
    };
    let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
        return ExitCode::from(1);
    };
    for c in walk(&bytes).filter_map(Result::ok) {
        if c.kind != 0x06 {
            continue;
        }
        let mut name = *b"    ";
        let src = c.name_str();
        let n = src.len().min(4);
        name[..n].copy_from_slice(&src.as_bytes()[..n]);
        if !want.is_empty() && !want.contains(&name) {
            continue;
        }
        match ffxi_dat::camera::CameraResource::parse(name, c.data) {
            Ok(cam) => println!("{cam:#?}"),
            Err(e) => println!("{src}: parse error {e}"),
        }
    }
    ExitCode::SUCCESS
}
