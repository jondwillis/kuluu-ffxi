use bevy::prelude::*;

use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::item_meta::{self, ItemDetail, ItemStatic};
use crate::hud::palette;
use crate::input_mode::{InputMode, MenuKind};
use crate::snapshot::SceneState;

mod flag {

    pub const RARE: u16 = 0x8000;

    pub const EX: u16 = 0x4000;
}

const SLOT_NAMES: &[&str] = &[
    "Main", "Sub", "Ranged", "Ammo", "Head", "Body", "Hands", "Legs", "Feet", "Neck", "Waist",
    "L.Ear", "R.Ear", "L.Ring", "R.Ring", "Back",
];

fn format_slots(slot_mask: u32) -> String {
    if slot_mask == 0 {
        return "—".to_string();
    }
    let parts: Vec<&str> = SLOT_NAMES
        .iter()
        .enumerate()
        .filter(|(bit, _)| slot_mask & (1 << bit) != 0)
        .map(|(_, name)| *name)
        .collect();
    if parts.is_empty() {
        "—".to_string()
    } else {
        parts.join("/")
    }
}

fn format_rare_ex(flags: u16) -> Option<String> {
    let rare = flags & flag::RARE != 0;
    let ex = flags & flag::EX != 0;
    match (rare, ex) {
        (true, true) => Some("Rare Ex".to_string()),
        (true, false) => Some("Rare".to_string()),
        (false, true) => Some("Ex".to_string()),
        (false, false) => None,
    }
}

fn format_uses(max_charges: Option<u8>, remaining: Option<u8>) -> Option<String> {
    let max = max_charges?;

    let used = remaining.unwrap_or(max);
    Some(format!("{used}/{max}"))
}

fn format_recast(recast_base: Option<u16>, live: Option<(u16, u16)>) -> Option<String> {
    let base = recast_base.or_else(|| live.map(|(_, total)| total))?;
    let current = live.map(|(remaining, _)| remaining).unwrap_or(0);
    Some(format!("{}/({})", mmss(current), mmss(base)))
}

fn mmss(secs: u16) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}:{s:02}")
}

fn lookup_static(dat_bytes: &[u8], item_no: u16) -> Option<ItemStatic> {
    let s = ffxi_dat::item_dat::lookup(dat_bytes, item_no)?;
    Some(ItemStatic {
        name: s.name,
        description: s.description,

        slot_mask: s.slot_mask as u32,
        jobs_mask: s.jobs_mask,
        races_mask: s.races_mask,
        level: s.level as u16,
        flags: s.flags,
        max_charges: (s.max_charges != 0).then_some(s.max_charges),
        recast_base: (s.recast_base != 0).then_some(s.recast_base),
    })
}

fn detail_rows(detail: &ItemDetail) -> Vec<String> {
    let mut rows = Vec::new();
    let Some(s) = &detail.static_ else {
        rows.push("(no item DAT — names only)".to_string());
        return rows;
    };

    if let Some(tag) = format_rare_ex(s.flags) {
        rows.push(tag);
    }
    rows.push(format!("Slot: {}", format_slots(s.slot_mask)));
    rows.push(format!("Races: {}", format_races(s.races_mask)));
    rows.push(format!("Jobs: {}", format_jobs(s.jobs_mask)));
    if s.level > 0 {
        rows.push(format!("Lv. {}", s.level));
    }
    if let Some(uses) = format_uses(s.max_charges, detail.charges_remaining) {
        rows.push(format!("Uses: {uses}"));
    }
    if let Some(recast) = format_recast(s.recast_base, detail.recast) {
        rows.push(format!("Recast: {recast}"));
    }
    if detail.equipped {
        rows.push("(equipped)".to_string());
    }
    if !s.description.is_empty() {
        rows.push(s.description.clone());
    }
    rows
}

fn format_jobs(jobs_mask: u32) -> String {
    if jobs_mask == 0 {
        return "All".to_string();
    }
    let parts: Vec<String> = (0..32u32)
        .filter(|bit| jobs_mask & (1 << bit) != 0)
        .map(|bit| job_abbrev(bit as u8))
        .collect();
    if parts.is_empty() {
        "All".to_string()
    } else {
        parts.join("/")
    }
}

fn job_abbrev(job_id: u8) -> String {
    let s = match job_id {
        1 => "WAR",
        2 => "MNK",
        3 => "WHM",
        4 => "BLM",
        5 => "RDM",
        6 => "THF",
        7 => "PLD",
        8 => "DRK",
        9 => "BST",
        10 => "BRD",
        11 => "RNG",
        12 => "SAM",
        13 => "NIN",
        14 => "DRG",
        15 => "SMN",
        16 => "BLU",
        other => return format!("J{other}"),
    };
    s.to_string()
}

fn format_races(races_mask: u16) -> String {
    if races_mask == 0 {
        return "All".to_string();
    }

    const RACES: &[(u16, &str)] = &[
        (0x0003, "Hume"),
        (0x000C, "Elvaan"),
        (0x0030, "Tarutaru"),
        (0x0040, "Mithra"),
        (0x0080, "Galka"),
    ];
    let parts: Vec<&str> = RACES
        .iter()
        .filter(|(mask, _)| races_mask & mask != 0)
        .map(|(_, name)| *name)
        .collect();
    if parts.is_empty() {
        "All".to_string()
    } else {
        parts.join("/")
    }
}

#[derive(Component)]
pub struct ItemDetailPanel;

#[derive(Component)]
pub struct ItemDetailName;

#[derive(Component)]
pub struct ItemDetailIcon;

#[derive(Component)]
pub struct ItemDetailBodyRow {
    pub slot: usize,
}

#[derive(Component)]
pub struct ItemDetailCounts;

#[derive(Component)]
pub struct SortOptionsPanel;

#[derive(Component)]
pub struct SortOptionRow {
    pub slot: usize,
}

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct SortOptions {
    pub auto: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOptionId {
    Auto,
    Manual,
    Recycle,
}

pub const SORT_OPTIONS: &[SortOptionId] = &[
    SortOptionId::Auto,
    SortOptionId::Manual,
    SortOptionId::Recycle,
];

const MAX_BODY_ROWS: usize = 10;

const ICON_SIZE_PX: f32 = 32.0;

const MAIN_BAG_CAPACITY: u32 = 80;

pub fn item_detail_open(mode: &InputMode) -> bool {
    match mode {
        InputMode::Menu(stack) => stack
            .current()
            .map(|l| matches!(l.kind, MenuKind::Items))
            .unwrap_or(false),
        _ => false,
    }
}

fn selected_item_no(mode: &InputMode, dynamic: &crate::hud::menu::DynamicMenu) -> Option<u16> {
    let stack = match mode {
        InputMode::Menu(stack) => stack,
        _ => return None,
    };
    let level = stack.current()?;
    if !matches!(level.kind, MenuKind::Items) {
        return None;
    }
    match dynamic.rows.get(level.cursor)?.action {
        crate::hud::menu::DynamicMenuAction::UseItem { item_no, .. } => Some(item_no),
        _ => None,
    }
}

pub fn spawn_item_detail(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = transparent_placeholder(&mut images);

    commands
        .spawn((
            crate::components::InGameEntity,
            ItemDetailPanel,
            Node {
                position_type: PositionType::Relative,
                bottom: Val::Px(8.0),
                left: Val::Px(8.0),
                width: Val::Px(240.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|p| {
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|h| {
                h.spawn((
                    ItemDetailIcon,
                    Node {
                        width: Val::Px(ICON_SIZE_PX),
                        height: Val::Px(ICON_SIZE_PX),
                        display: Display::None,
                        ..default()
                    },
                    ImageNode::new(placeholder.clone()),
                ));
                h.spawn((
                    ItemDetailName,
                    Text::new(""),
                    TextFont {
                        font_size: 14.0,
                        ..default()
                    },
                    TextColor(palette::ACCENT),
                ));
            });

            for slot in 0..MAX_BODY_ROWS {
                p.spawn((
                    ItemDetailBodyRow { slot },
                    Text::new(""),
                    TextFont {
                        font_size: 12.0,
                        ..default()
                    },
                    TextColor(palette::TEXT),
                    Node {
                        display: Display::None,
                        ..default()
                    },
                ));
            }

            p.spawn((
                ItemDetailCounts,
                Text::new(""),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(palette::MUTED),
            ));
        });

    commands
        .spawn((
            crate::components::InGameEntity,
            SortOptionsPanel,
            Node {
                position_type: PositionType::Relative,
                bottom: Val::Px(8.0),
                left: Val::Px(256.0),
                width: Val::Px(140.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::BORDER),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("Sort Options"),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(palette::ACCENT),
            ));
            for slot in 0..SORT_OPTIONS.len() {
                p.spawn((
                    SortOptionRow { slot },
                    Text::new(""),
                    TextFont {
                        font_size: 12.0,
                        ..default()
                    },
                    TextColor(palette::MUTED),
                ));
            }
        });
}

fn transparent_placeholder(images: &mut Assets<Image>) -> Handle<Image> {
    use bevy::asset::RenderAssetUsages;
    use bevy::image::ImageSampler;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    let mut image = Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0u8, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::nearest();
    images.add(image)
}

#[allow(clippy::too_many_arguments)]
pub fn update_item_detail(
    mode: Res<InputMode>,
    state: Res<SceneState>,
    dynamic: Res<crate::hud::menu::DynamicMenu>,
    dat_root: Res<ItemDatRoot>,
    mut icon_cache: ResMut<ItemIconCache>,
    mut images: ResMut<Assets<Image>>,
    mut panel_q: Query<&mut Node, With<ItemDetailPanel>>,
    mut name_q: Query<
        &mut Text,
        (
            With<ItemDetailName>,
            Without<ItemDetailBodyRow>,
            Without<ItemDetailCounts>,
        ),
    >,
    mut icon_q: Query<
        (&mut Node, &mut ImageNode),
        (With<ItemDetailIcon>, Without<ItemDetailPanel>),
    >,
    mut body_q: Query<
        (&ItemDetailBodyRow, &mut Node, &mut Text),
        (
            Without<ItemDetailPanel>,
            Without<ItemDetailIcon>,
            Without<ItemDetailName>,
            Without<ItemDetailCounts>,
        ),
    >,
    mut counts_q: Query<
        &mut Text,
        (
            With<ItemDetailCounts>,
            Without<ItemDetailName>,
            Without<ItemDetailBodyRow>,
        ),
    >,
) {
    let open = item_detail_open(&mode);
    if let Ok(mut node) = panel_q.single_mut() {
        let want = if open { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
    if !open {
        return;
    }

    let snapshot = &state.snapshot;
    let item_no = selected_item_no(&mode, &dynamic);

    let (name, detail): (String, Option<ItemDetail>) = match item_no {
        Some(item_no) => {
            let dat_static = icon_cache_static(&mut icon_cache, &dat_root, item_no);
            let detail = item_meta::compose_item_detail(item_no, snapshot, dat_static.clone());

            let name = dat_static
                .as_ref()
                .map(|s| s.name.clone())
                .filter(|n| !n.is_empty())
                .or_else(|| ffxi_proto::item_names::lookup(item_no).map(|s| s.to_string()))
                .unwrap_or_else(|| format!("Item #{item_no}"));
            (name, Some(detail))
        }
        None => ("Select an item.".to_string(), None),
    };

    if let Ok(mut text) = name_q.single_mut() {
        if **text != name {
            **text = name;
        }
    }

    if let Ok((mut icon_node, mut image_node)) = icon_q.single_mut() {
        let handle = item_no.and_then(|n| icon_cache.ensure(n, &dat_root, &mut images));
        match handle {
            Some(h) => {
                if image_node.image != h {
                    image_node.image = h;
                }
                if image_node.color != Color::WHITE {
                    image_node.color = Color::WHITE;
                }
                if icon_node.display != Display::Flex {
                    icon_node.display = Display::Flex;
                }
            }
            None => {
                if icon_node.display != Display::None {
                    icon_node.display = Display::None;
                }
            }
        }
    }

    let rows = match &detail {
        Some(d) => detail_rows(d),

        None => Vec::new(),
    };
    for (row, mut node, mut text) in body_q.iter_mut() {
        match rows.get(row.slot) {
            Some(line) => {
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
                if **text != *line {
                    **text = line.clone();
                }
            }
            None => {
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
        }
    }

    if let Ok(mut text) = counts_q.single_mut() {
        let want = format_counts(snapshot, detail.as_ref());
        if **text != want {
            **text = want;
        }
    }
}

fn icon_cache_static(
    cache: &mut ItemIconCache,
    dat_root: &ItemDatRoot,
    item_no: u16,
) -> Option<ItemStatic> {
    let bytes = cache.dat_bytes_for_static(dat_root)?;
    lookup_static(&bytes, item_no)
}

fn format_counts(
    snapshot: &ffxi_viewer_wire::SceneSnapshot,
    detail: Option<&ItemDetail>,
) -> String {
    let total = snapshot.inventory_main.len() as u32;
    let usable = snapshot
        .inventory_main
        .iter()
        .filter(|s| s.quantity > 0)
        .count() as u32;
    let held = detail.map(|d| d.quantity).unwrap_or(total);
    format!("Usable {usable}/{total} · Held {held}/{MAIN_BAG_CAPACITY}")
}

pub fn update_sort_options(
    mode: Res<InputMode>,
    sort: Res<SortOptions>,
    mut panel_q: Query<&mut Node, With<SortOptionsPanel>>,
    mut row_q: Query<(&SortOptionRow, &mut Text), Without<SortOptionsPanel>>,
) {
    let open = item_detail_open(&mode);
    if let Ok(mut node) = panel_q.single_mut() {
        let want = if open { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
    if !open {
        return;
    }

    for (row, mut text) in row_q.iter_mut() {
        let want = match SORT_OPTIONS.get(row.slot) {
            Some(SortOptionId::Auto) => {
                format!("Auto: {}", if sort.auto { "Yes" } else { "No" })
            }
            Some(SortOptionId::Manual) => "Manual (—)".to_string(),
            Some(SortOptionId::Recycle) => "Recycle (—)".to_string(),
            None => String::new(),
        };
        if **text != want {
            **text = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_render_slash_joined() {
        assert_eq!(format_slots(0), "—");
        assert_eq!(format_slots(1 << 4), "Head");
        assert_eq!(format_slots((1 << 11) | (1 << 12)), "L.Ear/R.Ear");
    }

    #[test]
    fn rare_ex_tag() {
        assert_eq!(format_rare_ex(0), None);
        assert_eq!(format_rare_ex(flag::RARE), Some("Rare".to_string()));
        assert_eq!(format_rare_ex(flag::EX), Some("Ex".to_string()));
        assert_eq!(
            format_rare_ex(flag::RARE | flag::EX),
            Some("Rare Ex".to_string())
        );
    }

    #[test]
    fn uses_shows_remaining_over_max() {
        assert_eq!(format_uses(None, None), None);
        assert_eq!(format_uses(Some(10), Some(9)), Some("9/10".to_string()));

        assert_eq!(format_uses(Some(10), None), Some("10/10".to_string()));
    }

    #[test]
    fn recast_formats_current_over_base() {
        assert_eq!(format_recast(None, None), None);

        assert_eq!(
            format_recast(Some(60), None),
            Some("0:00/(1:00)".to_string())
        );

        assert_eq!(
            format_recast(Some(60), Some((30, 60))),
            Some("0:30/(1:00)".to_string())
        );
    }

    #[test]
    fn jobs_all_when_unrestricted() {
        assert_eq!(format_jobs(0), "All");

        assert_eq!(format_jobs(1 << 1), "WAR");
        assert_eq!(format_jobs((1 << 1) | (1 << 3)), "WAR/WHM");
    }

    #[test]
    fn races_collapse_per_gender_bits() {
        assert_eq!(format_races(0), "All");

        assert_eq!(format_races(0x0003), "Hume");
        assert_eq!(format_races(0x0040), "Mithra");
    }

    #[test]
    fn select_an_item_when_off_items_view() {
        let mode = InputMode::World;
        assert!(!item_detail_open(&mode));
    }
}
