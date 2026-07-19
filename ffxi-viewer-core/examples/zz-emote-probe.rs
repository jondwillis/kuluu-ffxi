fn main() {
    let root = ffxi_dat::DatRoot::from_env_or_default().expect("dat root");
    let dll = ffxi_dat::main_dll::MainDll::load(root.root()).expect("main dll");
    let base = dll.base_emote_index(1).expect("humem base") as u32;
    for off in 0..16u32 {
        let file_id = base + off;
        let Ok(loc) = root.resolve(file_id) else {
            println!("off {off}: unresolvable");
            continue;
        };
        let path = loc.path_under(root.root());
        let Ok(bytes) = std::fs::read(&path) else {
            println!("off {off}: unreadable {path:?}");
            continue;
        };
        let (scheds, assets) = ffxi_viewer_core::scheduler_runtime::parse_action_bytes(&bytes);
        let mut ems: Vec<_> = scheds
            .iter()
            .filter(|s| s.name.starts_with(b"em"))
            .collect();
        ems.sort_by_key(|s| s.name);
        let clips: Vec<String> = assets
            .animations
            .iter()
            .map(|a| format!("{:?}", a.id))
            .collect();
        println!("off {off} file {file_id} ({path:?}): anims {:?}", clips);
        for s in ems {
            let stages: Vec<String> = s
                .stages
                .iter()
                .map(|t| {
                    format!(
                        "{:?}@{}+{} {}",
                        t.stage.kind,
                        t.stage.delay_frames,
                        t.stage.duration_frames,
                        String::from_utf8_lossy(&t.stage.id)
                    )
                })
                .collect();
            println!("  {} -> {stages:?}", String::from_utf8_lossy(&s.name));
        }
    }
}
