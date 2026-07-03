//! Item-detail data: formatting of a focused item's DAT fields into display
//! rows, plus the sort-options state shared by the Items window. Rendering lives
//! in `hud::item_screen`; the composition helper is `hud::item_ui::focus_detail`.

use bevy::prelude::*;

use crate::hud::item_meta::{ItemDetail, ItemStatic};
use crate::input_mode::{InputMode, MenuKind};

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

pub(crate) fn lookup_static(
    table: &ffxi_dat::item_dat::ItemTable,
    item_no: u16,
) -> Option<ItemStatic> {
    let s = table.lookup(item_no)?;
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

pub(crate) fn detail_rows(detail: &ItemDetail) -> Vec<String> {
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
    // The *client item DAT* jobs field is 0-indexed (bit == job id), unlike LSB's
    // item_equipment.jobs which is 1-indexed (bit == job - 1). Verified: White
    // Belt (MNK-only, job 2) has DAT jobs 0x04 = bit 2. This consumes the DAT
    // mask, so do NOT apply the -1 used by equip_info::fits_job.
    // Bit 0 is JOB_NONE; real jobs are 1..=22 (canonical codes scraped from LSB).
    let parts: Vec<&str> = (1..32u32)
        .filter(|bit| jobs_mask & (1 << bit) != 0)
        .filter_map(|bit| ffxi_proto::job_names::abbrev(bit as u16))
        .collect();
    if parts.is_empty() {
        "All".to_string()
    } else {
        parts.join("/")
    }
}

fn format_races(races_mask: u16) -> String {
    // The item DAT races field is 1-indexed by race id (bit 0 = race None):
    // Hume M/F = bits 1/2, Elvaan M/F = 3/4, Taru M/F = 5/6, Mithra = 7, Galka = 8.
    // Verified vs retail DAT: Mithran Gaiters = 0x0080 (bit 7), Galkan Sandals =
    // 0x0100 (bit 8). "All races" is 0x01FE (bits 1..=8), not 0.
    const ALL_RACES: u16 = 0x01FE;
    if races_mask == 0 || races_mask & ALL_RACES == ALL_RACES {
        return "All".to_string();
    }

    const RACES: &[(u16, &str)] = &[
        (0x0006, "Hume"),
        (0x0018, "Elvaan"),
        (0x0060, "Tarutaru"),
        (0x0080, "Mithra"),
        (0x0100, "Galka"),
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

#[derive(Resource, Debug, Clone, Copy)]
pub struct SortOptions {
    pub auto: bool,
}

impl Default for SortOptions {
    fn default() -> Self {
        // Retail defaults the Items window to auto-sort on.
        Self { auto: true }
    }
}

/// Which pane of the Items window has keyboard focus. The item list owns focus
/// by default; pressing NavRight moves it into the sort-options box so Auto /
/// Manual become navigable, and NavLeft / NavCancel returns to the list.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct ItemMenuFocus {
    pub sort_focused: bool,
    pub sort_cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOptionId {
    Auto,
    Manual,
}

pub const SORT_OPTIONS: &[SortOptionId] = &[SortOptionId::Auto, SortOptionId::Manual];

/// Apply a sort choice. Auto keeps the list ordered by item id and consolidates
/// partial stacks (see the ITEM_STACK request); Manual shows raw inventory order
/// (see `menu::refresh_dynamic_menu_rows`).
pub fn apply_sort_option(sort: &mut SortOptions, id: SortOptionId) {
    sort.auto = matches!(id, SortOptionId::Auto);
}

pub fn item_detail_open(mode: &InputMode) -> bool {
    match mode {
        InputMode::Menu(stack) => stack
            .current()
            .map(|l| matches!(l.kind, MenuKind::Items))
            .unwrap_or(false),
        _ => false,
    }
}

pub(crate) fn selected_item_no(
    mode: &InputMode,
    dynamic: &crate::hud::menu::DynamicMenu,
) -> Option<u16> {
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

        // The client item DAT jobs field is 0-indexed (bit == job id).
        // bit 1 = WAR (job 1), bit 2 = MNK (job 2) — White Belt's DAT jobs is
        // 0x04 = bit 2 = MNK. bit 4 = BLM (job 4).
        assert_eq!(format_jobs(1 << 1), "WAR");
        assert_eq!(format_jobs(1 << 2), "MNK");
        assert_eq!(format_jobs((1 << 1) | (1 << 4)), "WAR/BLM");
    }

    #[test]
    fn races_collapse_per_gender_bits() {
        assert_eq!(format_races(0), "All");
        assert_eq!(format_races(0x01FE), "All"); // all 8 race bits set
                                                 // 1-indexed race ids: Hume M = bit 1, Mithra = bit 7, Galka = bit 8.
        assert_eq!(format_races(0x0002), "Hume");
        assert_eq!(format_races(0x0080), "Mithra"); // Mithran Gaiters, not Galka
        assert_eq!(format_races(0x0100), "Galka");
    }

    #[test]
    fn select_an_item_when_off_items_view() {
        let mode = InputMode::World;
        assert!(!item_detail_open(&mode));
    }

    #[test]
    fn sort_defaults_to_auto() {
        assert!(SortOptions::default().auto);
    }

    #[test]
    fn sort_options_are_auto_then_manual() {
        assert_eq!(SORT_OPTIONS, &[SortOptionId::Auto, SortOptionId::Manual]);
    }

    #[test]
    fn apply_sort_option_selects_mode() {
        let mut s = SortOptions { auto: true };
        apply_sort_option(&mut s, SortOptionId::Manual);
        assert!(!s.auto);
        apply_sort_option(&mut s, SortOptionId::Auto);
        assert!(s.auto);
    }
}
