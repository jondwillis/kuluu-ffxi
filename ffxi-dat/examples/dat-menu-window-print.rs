//! Prints every menu window record whose root contains a given substring:
//! frame geometry, item blocks (id, rect, neighbour ids, both UI-string ids)
//! and the resolved caption from the two catalog DATs. Dev probe for layout
//! digs (bead kuluu-o9qt7.40 et al). Usage: dat-menu-window-print rmlo myroom

fn dmsg(root: &ffxi_dat::DatRoot, file_id: u32) -> Option<ffxi_dat::dmsg::DmsgTable> {
    let loc = root.resolve(file_id).ok()?;
    let bytes = std::fs::read(loc.path_under(root)).ok()?;
    ffxi_dat::dmsg::DmsgTable::parse(&bytes).ok()
}

fn main() {
    let root = ffxi_dat::DatRoot::from_env_or_default().unwrap();
    let windows = ffxi_dat::menu_windows::windows_from_root(&root).unwrap();
    let descs = dmsg(
        &root,
        ffxi_dat::menu_windows::CAPTION_CATALOG_TABLE_14_FILE_ID,
    );
    let short = dmsg(&root, ffxi_dat::menu_windows::CAPTION_SHORT_CATALOG_FILE_ID);
    let t12 = dmsg(
        &root,
        ffxi_dat::menu_windows::GETTEXT_TABLE_12_CATALOG_FILE_ID,
    );
    let want: Vec<String> = std::env::args().skip(1).collect();
    for (name, w) in &windows {
        if !want.iter().any(|s| name.contains(s.as_str())) {
            continue;
        }
        println!(
            "=== root={name:?} fourcc={} items={} size={}",
            w.fourcc, w.item_count, w.size
        );
        if let Some(f) = &w.frame {
            println!(
                "frame x={} y={} w={} h={} cursor=({},{}) anchor={}",
                f.x, f.y, f.width, f.height, f.cursor_a, f.cursor_b, f.anchor
            );
        }
        let caps: Vec<u32> = w.option_row_ids();
        if !caps.is_empty() {
            println!("widgets(caption ids): {caps:?}");
            for id in &caps {
                let read = |t: &Option<ffxi_dat::dmsg::DmsgTable>| -> String {
                    t.as_ref()
                        .and_then(|c| c.text(*id as usize).map(str::to_string))
                        .unwrap_or_else(|| format!("id{id}"))
                };
                println!(
                    "  widget {id}: short({}) table14({})",
                    read(&short),
                    read(&descs)
                );
            }
        }
        for (i, it) in w.items.iter().flatten().enumerate() {
            let cap = |id: Option<i32>, t: &Option<ffxi_dat::dmsg::DmsgTable>| -> String {
                match id {
                    None => "-".to_string(),
                    Some(-1) => "none".to_string(),
                    Some(v) => usize::try_from(v)
                        .ok()
                        .and_then(|i| t.as_ref().and_then(|c| c.text(i).map(str::to_string)))
                        .unwrap_or_else(|| format!("id{v}")),
                }
            };
            println!(
                "  [{i}] id={} x={} y={} w={} h={} up={:?} dn={:?} rt={:?} lf={:?} tA({}) tB({})",
                it.id,
                it.x,
                it.y,
                it.width,
                it.height,
                it.up,
                it.down,
                it.right,
                it.left,
                cap(it.text_a_id, &descs),
                cap(it.text_b_id, &short),
            );
        }
        if w.items.iter().flatten().count() == 0 && !caps.is_empty() {
            let rows = w.option_rows(|i| t12.as_ref().and_then(|c| c.text(i).map(str::to_string)));
            println!("table12 rows: {rows:?}");
        }
    }
}
