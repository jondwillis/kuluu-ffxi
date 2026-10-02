//! Enumerates the "font    usgaiji " UI-element group (retail weather icons,
//! research/xim Compass.kt drawWeather) across the four static menu UI DATs.

use ffxi_dat::ui_element::{find_ui_element_group, ui_sprite, UI_SHEET_FILE_ID};

// kuluu-render::ui_element_atlas::UI_DAT_FILE_IDS
const UI_DAT_FILE_IDS: [u32; 4] = [13, UI_SHEET_FILE_ID, 39551, 39560];
const GROUPS: [&str; 2] = ["font    usgaiji ", "font    gaiji   "];

fn main() {
    let root = ffxi_dat::DatRoot::from_env_or_default().unwrap();
    for id in UI_DAT_FILE_IDS {
        let Some(bytes) = root
            .resolve(id)
            .ok()
            .and_then(|loc| std::fs::read(loc.path_under(&root)).ok())
        else {
            println!("id {id}: unreadable");
            continue;
        };
        for group_name in GROUPS {
            let Some(group) = find_ui_element_group(&bytes, group_name) else {
                continue;
            };
            println!(
                "id {id} group={:?} textures={:?} elements={}",
                group.name,
                group.texture_names,
                group.elements.len()
            );
            for i in 0..group.elements.len().min(16) {
                match ui_sprite(&bytes, group_name, i) {
                    Some(s) => println!("  [{i}] {}x{}", s.width, s.height),
                    None => println!("  [{i}] no sprite"),
                }
            }
        }
    }
}
