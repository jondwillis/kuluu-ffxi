use std::process::ExitCode;

use ffxi_dat::{chunk::walk, generator::Generator, kind::ChunkKind, zone_dat, DatRoot};

fn main() -> ExitCode {
    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no DAT install: {e}");
            return ExitCode::from(1);
        }
    };

    let mut zones_with_lights = 0usize;
    let mut total_lights = 0usize;

    for zone_id in 0u16..1024 {
        let Some(file_id) = zone_dat::zone_id_to_mzb_file_id(zone_id) else {
            continue;
        };
        let Ok(loc) = root.resolve(file_id) else {
            continue;
        };
        let path = loc.path_under(root.root());
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };

        let mut lights = Vec::new();
        for c in walk(&bytes) {
            let Ok(c) = c else { continue };
            if ChunkKind::from_u8(c.kind) != Some(ChunkKind::Generator) {
                continue;
            }
            if let Ok(Some(pl)) = Generator::parse_point_light(c.data) {
                lights.push((c.name, pl));
            }
        }
        if lights.is_empty() {
            continue;
        }
        zones_with_lights += 1;
        total_lights += lights.len();
        let names: Vec<String> = lights
            .iter()
            .take(6)
            .map(|(n, _)| std::str::from_utf8(n).unwrap_or("????").to_string())
            .collect();
        let (_, sample) = &lights[0];
        println!(
            "zone {:>4}  file {:>5}  {:>2} lights  [{}{}]  e.g. range={:.1} color=[{:.2} {:.2} {:.2}] base=[{:.0} {:.0} {:.0}]  ({})",
            zone_id,
            file_id,
            lights.len(),
            names.join(" "),
            if lights.len() > 6 { " …" } else { "" },
            sample.range,
            sample.color[0], sample.color[1], sample.color[2],
            sample.base_position[0], sample.base_position[1], sample.base_position[2],
            path.display(),
        );
    }

    eprintln!("zones with point lights: {zones_with_lights}, total point lights: {total_lights}");
    ExitCode::SUCCESS
}
