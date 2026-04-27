use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rayon::prelude::*;

use ffxi_dat::{
    chunk::walk,
    generator::{Generator, PointLightDef},
    kind::ChunkKind,
};

struct Hit {
    path: PathBuf,
    name: [u8; 4],
    light: PointLightDef,
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let summary_mode = args
        .iter()
        .position(|a| a == "--summary")
        .map(|i| {
            args.remove(i);
            true
        })
        .unwrap_or(false);

    if args.is_empty() {
        eprintln!("usage: dat-scan-pointlights <file_or_dir> [--summary]");
        return ExitCode::from(1);
    }
    let target = PathBuf::from(&args[0]);

    let mut files: Vec<PathBuf> = Vec::new();
    if target.is_file() {
        files.push(target);
    } else if target.is_dir() {
        collect_dats(&target, &mut files);
    } else {
        eprintln!("not a file or directory: {}", target.display());
        return ExitCode::from(2);
    }
    eprintln!("scanning {} files…", files.len());

    let per_file: Vec<(usize, Vec<Hit>)> = files
        .par_iter()
        .map(|path| {
            let Ok(bytes) = std::fs::read(path) else {
                return (0, Vec::new());
            };
            let mut gen_count = 0usize;
            let mut hits = Vec::new();
            for c in walk(&bytes) {
                let Ok(c) = c else { continue };
                if ChunkKind::from_u8(c.kind) != Some(ChunkKind::Generator) {
                    continue;
                }
                gen_count += 1;
                if let Ok(Some(light)) = Generator::parse_point_light(c.data) {
                    hits.push(Hit {
                        path: path.clone(),
                        name: c.name,
                        light,
                    });
                }
            }
            (gen_count, hits)
        })
        .collect();

    let mut all_hits: Vec<Hit> = per_file
        .iter()
        .flat_map(|(_, h)| h.iter().map(clone_hit))
        .collect();
    all_hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.name.cmp(&b.name)));
    let total_generators: usize = per_file.iter().map(|(g, _)| g).sum();

    if !summary_mode {
        for h in &all_hits {
            let c = h.light.color;
            println!(
                "{}  {:?}  range={:.2} atten={:.4} color=[{:.2} {:.2} {:.2} {:.2}] base=[{:.2} {:.2} {:.2}]",
                h.path.display(),
                std::str::from_utf8(&h.name).unwrap_or("????"),
                h.light.range,
                h.light.attenuation,
                c[0], c[1], c[2], c[3],
                h.light.base_position[0], h.light.base_position[1], h.light.base_position[2],
            );
        }
    }

    eprintln!(
        "scanned {} files: {} Generator chunks, {} point lights",
        files.len(),
        total_generators,
        all_hits.len(),
    );
    ExitCode::SUCCESS
}

fn clone_hit(h: &Hit) -> Hit {
    Hit {
        path: h.path.clone(),
        name: h.name,
        light: h.light,
    }
}

fn collect_dats(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_dats(&p, out);
        } else if p
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("dat"))
            .unwrap_or(false)
        {
            out.push(p);
        }
    }
}
