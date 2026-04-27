use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{walk, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some(file_id_arg) = args.get(1) else {
        eprintln!(
            "usage: FFXI_DAT_PATH=<path> {} <file_id>",
            args.first().map(String::as_str).unwrap_or("dat-chunks")
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

    let root = match DatRoot::from_env() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("could not open DAT root: {e}");
            return ExitCode::from(1);
        }
    };

    let location = match root.resolve(file_id) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("resolve({file_id}) failed: {e}");
            return ExitCode::from(1);
        }
    };

    let path = location.path_under(root.root());
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("read {} failed: {e}", path.display());
            return ExitCode::from(1);
        }
    };

    println!("file_id        {file_id}");
    println!("path           {}", path.display());
    println!("file size      {} bytes", bytes.len());
    println!();
    println!(
        "{:>5}  {:>8}  {:6}  {:>4}  {:>10}  preview",
        "idx", "offset", "name", "kind", "body_len"
    );

    let mut idx = 0;
    let mut errs = 0;
    let mut total_body = 0usize;
    for result in walk(&bytes) {
        match result {
            Ok(chunk) => {
                let preview: String = chunk
                    .data
                    .iter()
                    .take(12)
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                println!(
                    "{idx:>5}  {:>8}  {:6}  {:>4}  {:>10}  {preview}",
                    chunk.offset,
                    chunk.name_str(),
                    chunk.kind,
                    chunk.data.len()
                );
                total_body += chunk.data.len();
                idx += 1;
            }
            Err(e) => {
                eprintln!("\nchunk walk error: {e}");
                errs += 1;
                break;
            }
        }
    }

    println!();
    println!("decoded {idx} chunks, {total_body} body bytes total, {errs} errors");
    if errs == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
