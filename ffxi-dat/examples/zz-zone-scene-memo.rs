//! Probe for the `zone_scene_file_id` memo: times the first
//! MAPSCHEDULOR lookup against the install (one full DAT read + parse per candidate
//! file) and the repeat (served from the process-local memo). Needs an install
//! via FFXI_DAT_PATH or the default location.
fn main() {
    let root = ffxi_dat::DatRoot::from_env_or_default()
        .expect("an install is required (FFXI_DAT_PATH or the default location)");
    let mut last = std::time::Instant::now();
    let r1 = ffxi_dat::scheduler::zone_scene_file_id(&root, 168, *b"215s");
    let d1 = last.elapsed();
    last = std::time::Instant::now();
    let r2 = ffxi_dat::scheduler::zone_scene_file_id(&root, 168, *b"215s");
    let d2 = last.elapsed();
    last = std::time::Instant::now();
    let r3 = ffxi_dat::scheduler::zone_scene_file_id(&root, 168, *b"zz99");
    let d3 = last.elapsed();
    last = std::time::Instant::now();
    let r4 = ffxi_dat::scheduler::zone_scene_file_id(&root, 168, *b"zz99");
    let d4 = last.elapsed();
    println!("first 215s (hit, 1 candidate file): {r1:?} in {d1:?}");
    println!("repeat 215s (memo): {r2:?} in {d2:?}");
    println!("first zz99 (miss, own + 5 carrier candidates): {r3:?} in {d3:?}");
    println!("repeat zz99 (memo): {r4:?} in {d4:?}");
}
