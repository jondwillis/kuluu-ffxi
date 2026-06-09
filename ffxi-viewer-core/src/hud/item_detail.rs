//! Item detail panel — docked bottom-left, retail-styled.
//!
//! This is the read-out for the item the operator is hovering in the Items
//! leaf view (the `MenuKind::Items` submenu). It composes the *static* DAT
//! facts (name, rare/ex, slot, race/job, level, enchantment, max uses, base
//! recast, consumable status-grant) with the *dynamic* per-item state from
//! the snapshot (uses remaining, current recast, equipped, quantity) via
//! the single [`crate::hud::item_meta::compose_item_detail`] composer —
//! exactly mirroring how `status_ribbon` joins the icon sheet with live
//! `status_icons`.
//!
//! Static metadata comes from `ffxi_dat::item_dat::lookup` (the retail item
//! DAT parser, owned by the item-data feature agent), bridged into the
//! foundation [`crate::hud::item_meta::ItemStatic`] shape. The embedded
//! icon is decoded + cached by [`super::item_dat_root::ItemIconCache`].
//!
//! ## Docking + compass/clock
//!
//! The panel docks bottom-left. While it is visible it should hide the
//! compass + Vana'diel clock so they don't overlap; those visibility
//! flips are owned by the **wiring** stage (it has write access to
//! `CompassPanel` / `VanaClockPanel` and the panel-open signal). This
//! module only drives its own nodes and exposes [`item_detail_open`] so
//! wiring can read the open state without re-deriving it.
//!
//! ## Sort Options sub-panel
//!
//! A small companion panel exposes the retail "Sort" affordances over the
//! Items bag: **Auto** (yes/no — selecting toggles and emits the sort
//! command), plus **Manual** and **Recycle** rows that are stubbed for now
//! (retail's manual-arrange + auto-trash flows aren't wired yet).

use bevy::prelude::*;

use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::item_meta::{self, ItemDetail, ItemStatic};
use crate::hud::palette;
use crate::input_mode::{InputMode, MenuKind};
use crate::snapshot::SceneState;

// ---------------------------------------------------------------------------
// FFXI item flag bits + slot/race tables (static-tier formatting).
// ---------------------------------------------------------------------------

/// Item flag bits we surface in the detail panel. Values match the retail
/// item-DAT `flags` word (same family POLUtils documents). Only the bits
/// the panel renders are named; the rest are ignored.
mod flag {
    /// Cannot be sold / dropped — the "Rare" tag's sibling on retail.
    pub const RARE: u16 = 0x8000;
    /// Exclusive: cannot be traded / bazaared / mailed.
    pub const EX: u16 = 0x4000;
}

/// Retail `SLOTTYPE` bit → display name, indexed by bit position. Mirrors
/// the order `menu.rs::EQUIPMENT_ENTRIES` uses (LSB `slot.h`).
const SLOT_NAMES: &[&str] = &[
    "Main", "Sub", "Ranged", "Ammo", "Head", "Body", "Hands", "Legs", "Feet", "Neck", "Waist",
    "L.Ear", "R.Ear", "L.Ring", "R.Ring", "Back",
];

/// Render a slot bitmask as a human-readable, slash-joined list
/// ("Head" / "L.Ear/R.Ear"). Empty mask → "—" (not equippable).
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

/// "Rare/Ex" tag line for an item's flags. Returns `None` when neither
/// flag is set so the row can be omitted.
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

/// Format the enchantment "uses" line: `used/total` where `used` is what
/// remains. Retail shows charges as `remaining/max` (e.g. `9/10`). Returns
/// `None` for non-charge items.
fn format_uses(max_charges: Option<u8>, remaining: Option<u8>) -> Option<String> {
    let max = max_charges?;
    // When the server hasn't reported live charges, show the item at full
    // charges (its static maximum) rather than blanking the row.
    let used = remaining.unwrap_or(max);
    Some(format!("{used}/{max}"))
}

/// Format the recast line: `current/(base,activation)` — e.g.
/// `0:00/(1:00,15s)` means "ready now; base recast 1 min, 15 s
/// activation". The activation time isn't in the foundation `ItemStatic`
/// yet (no field), so it's rendered only when the DAT bridge supplies it
/// — for now the activation segment is omitted and the line reads
/// `current/(base)`.
fn format_recast(recast_base: Option<u16>, live: Option<(u16, u16)>) -> Option<String> {
    // No base recast → not a recast item; nothing to show.
    let base = recast_base.or_else(|| live.map(|(_, total)| total))?;
    let current = live.map(|(remaining, _)| remaining).unwrap_or(0);
    Some(format!("{}/({})", mmss(current), mmss(base)))
}

/// Format a seconds count as `M:SS` (retail recast style). Used for both
/// the live and base recast segments.
fn mmss(secs: u16) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}:{s:02}")
}

// ---------------------------------------------------------------------------
// Detail composition (static DAT lookup → foundation bridge → composer).
// ---------------------------------------------------------------------------

/// Resolve the static metadata for `item_no` from the item DAT bytes and
/// bridge it into the foundation [`ItemStatic`] shape. Returns `None` when
/// the DAT is unreachable or the item id has no entry (caller then renders
/// the label-only fallback).
///
/// Kept separate from the snapshot join so it stays a pure function of the
/// DAT bytes — trivially testable once the parser lands. The
/// `ffxi_dat::item_dat::ItemStatic` field set is identical to the
/// foundation type by design (see `item_meta` module docs), so the bridge
/// is a flat copy.
fn lookup_static(dat_bytes: &[u8], item_no: u16) -> Option<ItemStatic> {
    let s = ffxi_dat::item_dat::lookup(dat_bytes, item_no)?;
    Some(ItemStatic {
        name: s.name,
        description: s.description,
        // The DAT carries narrower widths than the foundation shape (a
        // u16 slot mask, a u8 level); widen them. `max_charges` /
        // `recast_base` use `0` as the "N/A" sentinel in the DAT, which
        // the foundation models as `Option::None`.
        slot_mask: s.slot_mask as u32,
        jobs_mask: s.jobs_mask,
        races_mask: s.races_mask,
        level: s.level as u16,
        flags: s.flags,
        max_charges: (s.max_charges != 0).then_some(s.max_charges),
        recast_base: (s.recast_base != 0).then_some(s.recast_base),
    })
}

/// Render-ready text rows for one composed [`ItemDetail`]. Pure: takes the
/// composed detail and produces the labeled lines the panel paints, so the
/// formatting is unit-testable without Bevy. Order matches retail's detail
/// box top-to-bottom.
fn detail_rows(detail: &ItemDetail) -> Vec<String> {
    let mut rows = Vec::new();
    let Some(s) = &detail.static_ else {
        // Label-only fallback (no DAT install): the name row is filled by
        // the caller from the LSB scrape; the body is just a hint.
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

/// Format a job bitmask as a slash-joined abbreviation list. Falls back to
/// "All" when no specific jobs are flagged (`jobs_mask == 0`). Uses the
/// LSB job ids → abbreviations via `ffxi_proto`'s job table when available;
/// otherwise prints the raw bit indices so the operator still sees gating.
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

/// LSB job id → 3-letter abbreviation. Classic jobs only (Horizon scope);
/// unknown ids print as `J<id>`.
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

/// Race bitmask → slash-joined list. 0 → "All".
fn format_races(races_mask: u16) -> String {
    if races_mask == 0 {
        return "All".to_string();
    }
    // FFXI race bits: 1=Hume♂ 2=Hume♀ 4=Elvaan♂ 8=Elvaan♀ 16=Tarutaru♂
    // 32=Tarutaru♀ 64=Mithra 128=Galka. Collapse the per-gender bits into
    // the conventional race names retail shows.
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

// ---------------------------------------------------------------------------
// Bevy components / resources.
// ---------------------------------------------------------------------------

/// Marker on the item-detail panel root (bottom-left dock).
#[derive(Component)]
pub struct ItemDetailPanel;

/// Marker on the panel's name/title row.
#[derive(Component)]
pub struct ItemDetailName;

/// Marker on the panel's icon image node.
#[derive(Component)]
pub struct ItemDetailIcon;

/// Marker on one body row (slot / jobs / recast / etc). `slot` is the
/// 0-based row index into the rendered [`detail_rows`] list.
#[derive(Component)]
pub struct ItemDetailBodyRow {
    pub slot: usize,
}

/// Marker on the count summary row ("Usable 3/8 · Held 80/80").
#[derive(Component)]
pub struct ItemDetailCounts;

/// Marker on the Sort Options sub-panel root.
#[derive(Component)]
pub struct SortOptionsPanel;

/// Marker on one Sort Options row. `slot` indexes [`SORT_OPTIONS`].
#[derive(Component)]
pub struct SortOptionRow {
    pub slot: usize,
}

/// Persistent Sort-Options state. The Auto toggle is sticky across panel
/// opens (matches retail, where the auto-sort preference persists). Not
/// session-scoped — it's a UI preference, so it is *not* drained on
/// logout.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct SortOptions {
    /// Auto-sort the bag whenever it changes. Selecting the Auto row
    /// toggles this and emits the sort command (wiring dispatches it).
    pub auto: bool,
}

/// Sort-Options row identities. `Manual` and `Recycle` are stubbed —
/// rendered but inert — until retail's manual-arrange / auto-trash flows
/// are wired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOptionId {
    Auto,
    Manual,
    Recycle,
}

/// The Sort-Options rows, in retail display order.
pub const SORT_OPTIONS: &[SortOptionId] =
    &[SortOptionId::Auto, SortOptionId::Manual, SortOptionId::Recycle];

/// Max body rows the panel pools. Covers the worst-case detail
/// (rare/ex + slot + races + jobs + level + uses + recast + equipped +
/// description) with headroom.
const MAX_BODY_ROWS: usize = 10;

/// Rendered icon edge length, px. Larger than the status ribbon's chips —
/// the detail panel gives the item icon prominence.
const ICON_SIZE_PX: f32 = 32.0;

/// Maximum stack/inventory capacity retail surfaces in the count summary.
/// The main bag is 80 slots on retail-era data; used for the "Held N/80"
/// readout when the snapshot doesn't carry an explicit capacity.
const MAIN_BAG_CAPACITY: u32 = 80;

// ---------------------------------------------------------------------------
// Open-state helper (read by wiring to flip compass/clock visibility).
// ---------------------------------------------------------------------------

/// Whether the item detail panel should currently be visible: true when the
/// operator is on the Items leaf view. Exposed so the wiring stage can hide
/// the compass + clock without re-deriving the condition.
pub fn item_detail_open(mode: &InputMode) -> bool {
    match mode {
        InputMode::Menu(stack) => stack
            .current()
            .map(|l| matches!(l.kind, MenuKind::Items))
            .unwrap_or(false),
        _ => false,
    }
}

/// Resolve the `item_no` the operator is currently hovering in the Items
/// menu, by reading the menu cursor against the dynamic-menu rows. Returns
/// `None` when not on the Items view or the cursor is off a real row.
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

// ---------------------------------------------------------------------------
// Spawners.
// ---------------------------------------------------------------------------

/// Spawn the item detail panel + its Sort-Options companion, both docked
/// bottom-left and hidden until the Items leaf view opens. Front-ends call
/// this via `add_hud_spawners` (Startup / OnEnter), like the other HUD
/// panels.
pub fn spawn_item_detail(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = transparent_placeholder(&mut images);

    commands
        .spawn((
            crate::components::InGameEntity,
            ItemDetailPanel,
            Node {
                position_type: PositionType::Absolute,
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
            // Header: icon + name on one row.
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
            // Body row pool.
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
            // Count summary footer.
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

    // Sort Options sub-panel, stacked just above the detail panel.
    commands
        .spawn((
            crate::components::InGameEntity,
            SortOptionsPanel,
            Node {
                position_type: PositionType::Absolute,
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

/// 1×1 transparent placeholder for the icon node before its real icon is
/// assigned (and as the hidden-image state when no icon is available).
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

// ---------------------------------------------------------------------------
// Update systems.
// ---------------------------------------------------------------------------

/// Per-frame: show/hide the detail panel with the Items leaf view, resolve
/// the hovered item, compose its detail, and paint the rows + icon.
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
    mut icon_q: Query<(&mut Node, &mut ImageNode), (With<ItemDetailIcon>, Without<ItemDetailPanel>)>,
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

    // Name row + composed detail.
    let (name, detail): (String, Option<ItemDetail>) = match item_no {
        Some(item_no) => {
            let dat_static = icon_cache_static(&mut icon_cache, &dat_root, item_no);
            let detail = item_meta::compose_item_detail(item_no, snapshot, dat_static.clone());
            // Prefer the DAT name; fall back to the LSB scrape for labels.
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

    // Icon.
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

    // Body rows.
    let rows = match &detail {
        Some(d) => detail_rows(d),
        // 'Select an item.' helper state: no body rows.
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

    // Count summary: usable/total + held/capacity.
    if let Ok(mut text) = counts_q.single_mut() {
        let want = format_counts(snapshot, detail.as_ref());
        if **text != want {
            **text = want;
        }
    }
}

/// Bridge `ffxi_dat::item_dat::lookup` through the cached DAT bytes so the
/// static lookup reuses the same file read the icon cache performs. Returns
/// `None` when the DAT is unreachable or the item has no entry.
fn icon_cache_static(
    cache: &mut ItemIconCache,
    dat_root: &ItemDatRoot,
    item_no: u16,
) -> Option<ItemStatic> {
    let bytes = cache.dat_bytes_for_static(dat_root)?;
    lookup_static(&bytes, item_no)
}

/// Compose the bottom count summary: "Usable U/T · Held H/C". `usable`
/// counts inventory rows whose item is currently usable (we approximate as
/// "has a positive quantity"); `total` is the distinct inventory rows;
/// `held` is the selected item's quantity (or total bag fill when nothing
/// is selected); capacity is the main-bag size.
fn format_counts(snapshot: &ffxi_viewer_wire::SceneSnapshot, detail: Option<&ItemDetail>) -> String {
    let total = snapshot.inventory_main.len() as u32;
    let usable = snapshot
        .inventory_main
        .iter()
        .filter(|s| s.quantity > 0)
        .count() as u32;
    let held = detail.map(|d| d.quantity).unwrap_or(total);
    format!("Usable {usable}/{total} · Held {held}/{MAIN_BAG_CAPACITY}")
}

/// Per-frame: show/hide the Sort-Options sub-panel with the Items leaf
/// view and paint each row's current state (Auto yes/no; Manual / Recycle
/// stubbed).
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
        // No live charge report → full charges.
        assert_eq!(format_uses(Some(10), None), Some("10/10".to_string()));
    }

    #[test]
    fn recast_formats_current_over_base() {
        assert_eq!(format_recast(None, None), None);
        // Ready, base 60s.
        assert_eq!(format_recast(Some(60), None), Some("0:00/(1:00)".to_string()));
        // On cooldown: 30s left of a 60s recast.
        assert_eq!(
            format_recast(Some(60), Some((30, 60))),
            Some("0:30/(1:00)".to_string())
        );
    }

    #[test]
    fn jobs_all_when_unrestricted() {
        assert_eq!(format_jobs(0), "All");
        // WAR (id 1) → bit 1.
        assert_eq!(format_jobs(1 << 1), "WAR");
        assert_eq!(format_jobs((1 << 1) | (1 << 3)), "WAR/WHM");
    }

    #[test]
    fn races_collapse_per_gender_bits() {
        assert_eq!(format_races(0), "All");
        // Hume male+female bits both collapse to "Hume".
        assert_eq!(format_races(0x0003), "Hume");
        assert_eq!(format_races(0x0040), "Mithra");
    }

    #[test]
    fn select_an_item_when_off_items_view() {
        let mode = InputMode::World;
        assert!(!item_detail_open(&mode));
    }
}
