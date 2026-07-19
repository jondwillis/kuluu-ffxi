use ffxi_dat::weather::collect_zone_weather_sets;
use ffxi_dat::DatRoot;

fn main() {
    let root = DatRoot::from_env_or_default().expect("dat root");
    for arg in std::env::args().skip(1) {
        let file_id: u32 = arg.parse().expect("file id");
        let loc = root.resolve(file_id).expect("resolve");
        let bytes = std::fs::read(loc.path_under(root.root())).expect("read");
        let sets = collect_zone_weather_sets(&bytes);
        let mut lights = 0usize;
        for c in ffxi_dat::chunk::walk(&bytes).filter_map(Result::ok) {
            if ffxi_dat::kind::ChunkKind::from_u8(c.kind)
                == Some(ffxi_dat::kind::ChunkKind::Generator)
                && matches!(
                    ffxi_dat::generator::Generator::parse_point_light(c.data),
                    Ok(Some(_))
                )
            {
                lights += 1;
            }
        }
        println!("== DAT {file_id} == ({lights} generator point lights)");
        for (ty, set) in &sets.by_type {
            let ty: String = ty.iter().map(|&b| b as char).collect();
            let ind: Vec<bool> = set.outdoor.iter().map(|r| r.indoors).collect();
            println!(
                "  type {ty}: outdoor {} recs indoors={ind:?}, indo {} recs",
                set.outdoor.len(),
                set.indoor.len()
            );
            if let Some(r) = set.outdoor.first() {
                println!(
                    "    sun_diff_land={:?} moon_diff_land={:?} amb_land={:?} mul={}",
                    r.sunlight_diffuse_landscape,
                    r.moonlight_diffuse_landscape,
                    r.ambient_landscape,
                    r.diffuse_mul_landscape
                );
            }
        }
        println!(
            "  flat: {} recs indoors={:?}",
            sets.flat.len(),
            sets.flat.iter().map(|r| r.indoors).collect::<Vec<_>>()
        );
        if let Some(r) = sets.flat.first() {
            println!(
                "    sun_diff_land={:?} moon_diff_land={:?} amb_land={:?} mul={}",
                r.sunlight_diffuse_landscape,
                r.moonlight_diffuse_landscape,
                r.ambient_landscape,
                r.diffuse_mul_landscape
            );
        }
    }
}
