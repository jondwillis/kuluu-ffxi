//! Throwaway Phase-0 spike for the ui_kit plan (kuluu-hjr6): enumerate the
//! "menu    frames  " UI-element group and report per-element component
//! geometry, so we can identify retail window-frame parts (corners/edges/
//! center) and record nine-slice insets as named consts.

use ffxi_dat::ui_element::{find_ui_element_group, ui_sprite};

const UI_DAT_PATHS: [&str; 4] = [
    "ROM/0/13.DAT",
    "ROM/119/51.DAT",
    "ROM/280/15.DAT",
    "ROM/324/95.DAT",
];
const GROUPS: [&str; 2] = ["menu    frames  ", "menu    framesus"];

fn main() {
    let root = ffxi_dat::DatRoot::from_env_or_default().unwrap();
    for rel in UI_DAT_PATHS {
        let Ok(bytes) = std::fs::read(root.root().join(rel)) else {
            println!("{rel}: unreadable");
            continue;
        };
        for group_name in GROUPS {
            let Some(group) = find_ui_element_group(&bytes, group_name) else {
                continue;
            };
            println!(
                "{rel} group={:?} textures={:?} elements={}",
                group.name,
                group.texture_names,
                group.elements.len()
            );
            for (i, el) in group.elements.iter().enumerate() {
                let comps: Vec<String> = el
                    .components
                    .iter()
                    .map(|c| {
                        format!(
                            "uv({},{} {}x{}) pos{:?} flip={} tex={:?}{}",
                            c.uv_offset_x,
                            c.uv_offset_y,
                            c.uv_width,
                            c.uv_height,
                            c.positions,
                            c.flip_mode,
                            c.texture_ref,
                            if c.draw_enabled { "" } else { " OFF" }
                        )
                    })
                    .collect();
                let size = ui_sprite(&bytes, group_name, i)
                    .map(|s| format!("{}x{}", s.width, s.height))
                    .unwrap_or_else(|| "-".into());
                println!("  [{i:3}] sprite={size} comps={}", comps.join(" | "));
            }
        }
    }
}
