use std::env;
use std::fs;

use ffxi_dat::weather::{collect_weather_records, sample_weather};
use ffxi_dat::{zone_dat, DatRoot};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<u16> = env::args()
        .skip(1)
        .map(|s| s.parse().expect("zone id must be u16"))
        .collect();
    if args.is_empty() {
        eprintln!("usage: dump-skybox <zone_id> [<zone_id> ...]");
        std::process::exit(1);
    }
    let root = DatRoot::from_env_or_default()?;
    for zone_id in args {
        dump_zone(&root, zone_id)?;
    }
    Ok(())
}

fn dump_zone(root: &DatRoot, zone_id: u16) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== zone 0x{:04X} ({}) ===", zone_id, zone_id);
    let Some(file_id) = zone_dat::zone_id_to_mzb_file_id(zone_id) else {
        println!("  no MZB file id mapping");
        return Ok(());
    };
    let location = root.resolve(file_id)?;
    let path = location.path_under(root.root());
    let bytes = fs::read(&path)?;
    let records = collect_weather_records(&bytes);
    if records.is_empty() {
        println!("  no Weather records");
        return Ok(());
    }
    println!("  {} keyframes:", records.len());
    for r in &records {
        println!(
            "    V{:02}:{:02} sky[0]={:?} sky[7]={:?} ambient={:?} fog={:?} brightness={:.3}",
            r.time_minutes / 60,
            r.time_minutes % 60,
            r.skybox_colors[0],
            r.skybox_colors[7],
            r.ambient_landscape,
            r.fog_landscape,
            r.brightness_landscape,
        );
    }

    let noon = 720u32;
    let Some(r0) = sample_weather(&records, noon) else {
        println!("  no record at V12:00");
        return Ok(());
    };
    let next_minute = (noon + 1) % 1440;
    let Some(r1) = sample_weather(&records, next_minute) else {
        println!("  no record at V12:01");
        return Ok(());
    };
    println!(
        "  @V12:00 floor record V{:02}:{:02}, ceil V{:02}:{:02}",
        r0.time_minutes / 60,
        r0.time_minutes % 60,
        r1.time_minutes / 60,
        r1.time_minutes % 60
    );
    for i in 0..8 {
        let c = r0.skybox_colors[i];
        let lin_r = srgb_to_linear(c[0]);
        let lin_g = srgb_to_linear(c[1]);
        let lin_b = srgb_to_linear(c[2]);
        println!(
            "    band {} alt={:.3} srgb=({:.3},{:.3},{:.3}) → linear=({:.3},{:.3},{:.3})",
            i, r0.skybox_altitudes[i], c[0], c[1], c[2], lin_r, lin_g, lin_b,
        );
    }
    let _ = &r1;
    Ok(())
}

fn srgb_to_linear(s: f32) -> f32 {
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}
