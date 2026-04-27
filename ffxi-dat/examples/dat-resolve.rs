use std::env;
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

use ffxi_dat::DatRoot;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some(file_id_arg) = args.get(1) else {
        eprintln!(
            "usage: FFXI_DAT_PATH=<path-to-FF-XI-dir> {} <file_id>",
            args.first().map(String::as_str).unwrap_or("dat-resolve")
        );
        return ExitCode::from(2);
    };

    let file_id: u32 = match file_id_arg.parse() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("invalid file_id {file_id_arg:?}: {e}");
            return ExitCode::from(2);
        }
    };

    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("could not open DAT root: {e}");
            return ExitCode::from(1);
        }
    };

    println!("install root:  {}", root.root().display());
    println!("loaded APPIDs:");
    for (rom_dir, v_len, f_len) in root.app_summary() {
        println!("  {rom_dir:6} VTABLE={v_len:6} FTABLE={f_len:6}");
    }

    let location = match root.resolve(file_id) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("resolve({file_id}) failed: {e}");
            return ExitCode::from(1);
        }
    };

    let path = location.path_under(root.root());
    println!();
    println!("file_id        {file_id}");
    println!("rom_dir        {}", location.rom_dir);
    println!(
        "sub_path       dir={}  file={}",
        location.sub_path.dir, location.sub_path.file
    );
    println!("resolved path  {}", path.display());

    match File::open(&path) {
        Ok(mut f) => {
            let mut head = [0u8; 64];
            match f.read(&mut head) {
                Ok(n) => {
                    println!("file size      (open ok)");
                    print!("first {n} bytes ");
                    for (i, b) in head[..n].iter().enumerate() {
                        if i > 0 && i % 16 == 0 {
                            print!("\n               ");
                        }
                        print!("{b:02x} ");
                    }
                    println!();
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("read failed: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Err(e) => {
            eprintln!("open {} failed: {e}", path.display());
            ExitCode::from(1)
        }
    }
}
