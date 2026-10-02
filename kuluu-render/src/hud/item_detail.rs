//! Item-detail data: formatting of a focused item's DAT fields into display
//! rows, plus the sort-options state shared by the Items window. Rendering lives
//! in `hud::item_screen`; the composition helper is `hud::item_ui::focus_detail`.

use bevy::prelude::*;

use crate::hud::item_meta::{ItemDetail, ItemStatic};
use crate::input_mode::{InputMode, MenuKind};

pub(crate) mod flag {
    pub use ffxi_dat::item_dat::{ITEM_FLAG_EX as EX, ITEM_FLAG_RARE as RARE};
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

/// Retail item-tooltip charge/recast line: `<cur/max HH:MM:SS/[HH:MM:SS]>`,
/// shown only for charged (usable/enchanted) items. The first HH:MM:SS is the
/// live countdown to next use; the bracketed value is the static reuse delay.
/// Retail also carries the activation (cast) time as a second bracket value
/// (`[24:00:00, 0:30]`); that DAT field is not yet parsed (kuluu-ng3o), so the
/// bracket shows the reuse delay alone.
fn format_charge_line(detail: &ItemDetail) -> Option<String> {
    let charges = detail.charges_remaining?;
    let max = detail.static_.as_ref().and_then(|s| s.max_charges)?;
    let (remaining, base) = detail.recast?;
    Some(format!(
        "<{charges}/{max} {}/[{}]>",
        hhmmss(remaining),
        hhmmss(base)
    ))
}

fn hhmmss(secs: u32) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h}:{m:02}:{s:02}")
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
    // Retail frames an equipment description with the race line above and the
    // level+jobs line below, and shows a bare description for everything else
    // (.agents/skills/retail-observe/references/2026-09-11-items-window.md).
    let equipment = s.slot_mask != 0;
    if equipment {
        rows.push(format_races(s.races_mask));
    }
    rows.extend(s.description.lines().map(str::to_string));
    if equipment {
        rows.push(format!("Lv.{} {}", s.level, format_jobs(s.jobs_mask)));
    }
    if let Some(line) = format_charge_line(detail) {
        rows.push(line);
    }
    rows
}

/// A mask covering every job the scraped table knows reads "All Jobs" (retail
/// prints it for bait and level-1 gear whose DAT lists all 22).
fn format_jobs(jobs_mask: u32) -> String {
    let every_job = (1..32u32)
        .filter(|bit| ffxi_vocab::job_names::abbrev(*bit as u16).is_some())
        .all(|bit| jobs_mask & (1 << bit) != 0);
    if jobs_mask == 0 || every_job {
        return ALL_JOBS.to_string();
    }
    // The *client item DAT* jobs field is 0-indexed (bit == job id), unlike LSB's
    // item_equipment.jobs which is 1-indexed (bit == job - 1). Verified: White
    // Belt (MNK-only, job 2) has DAT jobs 0x04 = bit 2. This consumes the DAT
    // mask, so do NOT apply the -1 used by equip_info::fits_job.
    // Bit 0 is JOB_NONE; real jobs are 1..=22 (canonical codes scraped from LSB).
    let parts: Vec<&str> = (1..32u32)
        .filter(|bit| jobs_mask & (1 << bit) != 0)
        .filter_map(|bit| ffxi_vocab::job_names::abbrev(bit as u16))
        .collect();
    if parts.is_empty() {
        ALL_JOBS.to_string()
    } else {
        parts.join("/")
    }
}

const ALL_JOBS: &str = "All Jobs";

const ALL_RACES: &str = "All Races";

fn format_races(races_mask: u16) -> String {
    // The item DAT races field is 1-indexed by race id (bit 0 = race None):
    // Hume M/F = bits 1/2, Elvaan M/F = 3/4, Taru M/F = 5/6, Mithra = 7, Galka = 8.
    // Verified vs retail DAT: Mithran Gaiters = 0x0080 (bit 7), Galkan Sandals =
    // 0x0100 (bit 8). "All races" is 0x01FE (bits 1..=8), not 0.
    const ALL_RACE_BITS: u16 = 0x01FE;
    if races_mask == 0 || races_mask & ALL_RACE_BITS == ALL_RACE_BITS {
        return ALL_RACES.to_string();
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
        ALL_RACES.to_string()
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
/// by default; the "Select active window" key (`Action::SelectActiveWindow`)
/// steps focus through the bags and then into the sort-options box so Auto /
/// Manual become navigable, and NavLeft / NavCancel returns to the list.
///
/// The sort cursor only exists while the sort box has focus, and it is always
/// a valid `SortOptionId` — the invariant lives in this type rather than in a
/// (bool, usize) pair every caller must keep in sync.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemMenuFocus {
    #[default]
    List,
    Sort(SortOptionId),
}

impl ItemMenuFocus {
    pub fn sort_focused(&self) -> bool {
        matches!(self, Self::Sort(_))
    }

    /// The sort option under the cursor, if the sort box has focus.
    pub fn sort_selection(&self) -> Option<SortOptionId> {
        match *self {
            Self::Sort(id) => Some(id),
            Self::List => None,
        }
    }

    /// Move focus into the sort box with the cursor on `id` (keyboard entry
    /// lands on the currently active mode; mouse hover lands on the hovered
    /// row).
    pub fn enter_sort(&mut self, id: SortOptionId) {
        *self = Self::Sort(id);
    }

    /// Return focus to the item list.
    pub fn exit_sort(&mut self) {
        *self = Self::List;
    }
}

/// Rows of the Items window's Options box, in retail order (Auto, Manual,
/// Recycle Bin). Recycle Bin is not a sort mode: confirming it browses the
/// recycle-bin container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOptionId {
    Auto,
    Manual,
    RecycleBin,
}

pub const SORT_OPTIONS: &[SortOptionId] = &[
    SortOptionId::Auto,
    SortOptionId::Manual,
    SortOptionId::RecycleBin,
];

impl SortOptions {
    /// The `SortOptionId` currently in effect.
    pub fn active(&self) -> SortOptionId {
        if self.auto {
            SortOptionId::Auto
        } else {
            SortOptionId::Manual
        }
    }
}

/// Apply a sort choice. Auto keeps the list ordered by item id and consolidates
/// partial stacks (see the ITEM_STACK request); Manual shows raw inventory order
/// (see `menu::inventory_rows`). Returns whether a sort mode changed hands;
/// Recycle Bin leaves the mode alone.
pub fn apply_sort_option(sort: &mut SortOptions, id: SortOptionId) -> bool {
    match id {
        SortOptionId::Auto => {
            sort.auto = true;
            true
        }
        SortOptionId::Manual => {
            sort.auto = false;
            true
        }
        SortOptionId::RecycleBin => false,
    }
}

/// A keypress routed to the sort box while it has focus. `Other` is any key
/// not bound to sort-box navigation; it must still reach [`sort_pane_key`] so
/// the box swallows it instead of letting it leak into list navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortPaneKey {
    Up,
    Down,
    Confirm,
    Exit,
    Other,
}

/// Handle one keypress while the sort box has focus: Up/Down step the cursor
/// through [`SORT_OPTIONS`] with wrap-around, Confirm applies the option under
/// the cursor, and Exit returns focus to the item list. Returns the confirmed
/// option, if any — the caller sends the server ITEM_STACK request for a sort
/// mode or opens the recycle bin. No-op when the list has focus.
pub fn sort_pane_key(
    focus: &mut ItemMenuFocus,
    sort: &mut SortOptions,
    key: SortPaneKey,
) -> Option<SortOptionId> {
    let ItemMenuFocus::Sort(cursor) = *focus else {
        return None;
    };
    match key {
        SortPaneKey::Up => {
            focus.enter_sort(step_sort_option(cursor, -1));
            None
        }
        SortPaneKey::Down => {
            focus.enter_sort(step_sort_option(cursor, 1));
            None
        }
        SortPaneKey::Confirm => {
            apply_sort_option(sort, cursor);
            Some(cursor)
        }
        SortPaneKey::Exit => {
            focus.exit_sort();
            None
        }
        SortPaneKey::Other => None,
    }
}

/// Step `delta` rows through [`SORT_OPTIONS`], wrapping at both ends.
fn step_sort_option(id: SortOptionId, delta: isize) -> SortOptionId {
    let len = SORT_OPTIONS.len() as isize;
    let pos = SORT_OPTIONS.iter().position(|&o| o == id).unwrap_or(0) as isize;
    SORT_OPTIONS[(pos + delta).rem_euclid(len) as usize]
}

/// Emitted when the user activates a sort option, so the client sends the
/// server ITEM_STACK request. `container` is the LSB CONTAINER_ID to sort.
#[derive(Message, Debug, Clone, Copy)]
pub struct InventorySortRequested {
    pub container: u8,
}

/// What the inventory looked like the last time auto-sort asked the server to
/// consolidate it, so a sort the server declines is not re-sent every tick.
#[derive(Resource, Debug, Default)]
pub struct AutoSortedInventory {
    slots: Vec<(u8, u16, u32)>,
}

pub fn drain_auto_sorted_inventory(mut asked: ResMut<AutoSortedInventory>) {
    *asked = AutoSortedInventory::default();
}

/// Whether the server's sort pass would move anything: it tops off every pair of
/// same-item slots that are both under their stack size
/// (vendor/server/src/map/packets/c2s/0x03a_item_stack.cpp
/// GP_CLI_COMMAND_ITEM_STACK::process). Freeing a slot is not the test — 11 + 3
/// of a 12-stack merges to 12 + 2 and still occupies two slots. A slot carrying
/// any lock byte is left out: the server skips an item some transaction owns
/// (vendor/server/src/map/items/item.cpp CItem::isBusy), and treating every lock
/// as that one only costs an ask we did not need to make.
fn consolidates(items: &[kuluu_snapshot::InventoryItem]) -> bool {
    let mut partial: std::collections::HashSet<u16> = std::collections::HashSet::new();
    items
        .iter()
        .filter(|s| !s.locked && !s.unselectable)
        .filter(|s| s.quantity < u32::from(ffxi_vocab::item_flags::stack_size(s.item_no)))
        .any(|s| !partial.insert(s.item_no))
}

/// Auto-sort's standing half: retail's Options box calls it "Auto-sort /
/// Automatically rearrange items", and the rearranging is the server's
/// ITEM_STACK (c2s 0x03A, research/XiPackets/world/client/0x003A/README.md) —
/// so newly obtained items landing in a fresh slot beside a partial stack of
/// the same item have to be consolidated without the player re-confirming the
/// option. Only asks when a sort would actually free a slot, and only once per
/// inventory it has not already asked about, so a sort the server declines does
/// not repeat.
///
/// Unverified against retail: the observation record has the Options box and
/// its help strings but never confirmed a sort
/// (.agents/skills/retail-observe/references/2026-09-11-items-window.md).
/// The InZone/dialog gate: an ITEM_STACK mid-event is refused by the transport,
/// and a container read before the bootstrap finishes is not the real inventory.
pub fn auto_consolidate_inventory_system(
    sort: Res<SortOptions>,
    scene: Res<crate::snapshot::SceneState>,
    mut asked: ResMut<AutoSortedInventory>,
    mut sort_req: MessageWriter<InventorySortRequested>,
) {
    const CONTAINER: u8 = ffxi_proto::map::container::LOC_INVENTORY;
    if !scene.is_changed() && !sort.is_changed() {
        return;
    }
    let snap = &scene.snapshot;
    if !sort.auto || snap.stage != kuluu_snapshot::Stage::InZone || snap.dialog.is_some() {
        return;
    }
    let Some(items) = snap.container(CONTAINER).map(|c| c.items.as_slice()) else {
        return;
    };
    if !consolidates(items) {
        return;
    }
    let slots: Vec<(u8, u16, u32)> = items
        .iter()
        .map(|s| (s.index, s.item_no, s.quantity))
        .collect();
    if asked.slots == slots {
        return;
    }
    asked.slots = slots;
    sort_req.write(InventorySortRequested {
        container: CONTAINER,
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Any item the scraped table says stacks, and any it says does not — the
    /// predicate is about stack arithmetic, not about which item it is.
    fn stacking() -> (u16, u32) {
        let id = (1..=u16::MAX)
            .find(|&id| ffxi_vocab::item_flags::stack_size(id) > 1)
            .expect("some item stacks");
        (id, u32::from(ffxi_vocab::item_flags::stack_size(id)))
    }

    fn unstacking() -> u16 {
        (1..=u16::MAX)
            .find(|&id| ffxi_vocab::item_flags::stack_size(id) == 1)
            .expect("some item does not stack")
    }

    fn slot(index: u8, item_no: u16, quantity: u32) -> kuluu_snapshot::InventoryItem {
        kuluu_snapshot::InventoryItem {
            container: ffxi_proto::map::container::LOC_INVENTORY,
            index,
            item_no,
            quantity,
            locked: false,
            unselectable: false,
            charges_remaining: None,
            next_use_vana_ts: None,
            use_delay_end_vana_ts: None,
            ready: None,
        }
    }

    #[test]
    fn a_second_partial_stack_is_worth_consolidating() {
        let (id, stack) = stacking();
        let part = stack / 2;
        assert!(consolidates(&[
            slot(0, id, part),
            slot(1, id, stack - part)
        ]));
        assert!(
            !consolidates(&[slot(0, id, stack), slot(1, id, part)]),
            "a full stack beside a partial one cannot merge into fewer slots"
        );
        assert!(
            !consolidates(&[slot(0, id, part)]),
            "one stack is already consolidated"
        );
    }

    #[test]
    fn partial_stacks_are_topped_off_even_when_no_slot_is_freed() {
        let (id, stack) = stacking();
        assert!(
            consolidates(&[slot(0, id, stack - 1), slot(1, id, 1)]),
            "a near-full stack beside a remainder still merges, into a full stack and a smaller remainder"
        );
    }

    #[test]
    fn a_slot_an_action_owns_is_never_the_reason_to_ask() {
        let (id, stack) = stacking();
        let mut busy = slot(0, id, stack / 2);
        busy.unselectable = true;
        assert!(!consolidates(&[busy, slot(1, id, stack / 2)]));
    }

    #[test]
    fn a_locked_slot_is_never_the_reason_to_ask() {
        let (id, stack) = stacking();
        let mut locked = slot(0, id, stack / 2);
        locked.locked = true;
        assert!(!consolidates(&[locked, slot(1, id, stack / 2)]));
    }

    /// Two of the same non-stacking item occupy two slots however they sort.
    #[test]
    fn unstackable_duplicates_are_not_worth_consolidating() {
        let id = unstacking();
        assert!(!consolidates(&[slot(0, id, 1), slot(1, id, 1)]));
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
    fn hhmmss_handles_24h_reuse() {
        assert_eq!(hhmmss(0), "0:00:00");
        assert_eq!(hhmmss(30), "0:00:30");
        assert_eq!(hhmmss(3600), "1:00:00");
        assert_eq!(hhmmss(86_400), "24:00:00");
        assert_eq!(hhmmss(82_557), "22:55:57");
    }

    fn charged_detail(
        charges: Option<u8>,
        max: Option<u8>,
        recast: Option<(u32, u32)>,
    ) -> ItemDetail {
        ItemDetail {
            static_: Some(ItemStatic {
                max_charges: max,
                ..Default::default()
            }),
            charges_remaining: charges,
            recast,
            ..Default::default()
        }
    }

    #[test]
    fn charge_line_none_for_non_charged_item() {
        // No live charges (extdata header not 0x01) => no line.
        assert_eq!(
            format_charge_line(&charged_detail(None, Some(1), Some((0, 86_400)))),
            None
        );
    }

    #[test]
    fn charge_line_ready_item_zero_countdown() {
        assert_eq!(
            format_charge_line(&charged_detail(Some(1), Some(1), Some((0, 86_400)))),
            Some("<1/1 0:00:00/[24:00:00]>".to_string())
        );
    }

    #[test]
    fn charge_line_on_cooldown_shows_live_countdown() {
        assert_eq!(
            format_charge_line(&charged_detail(Some(1), Some(1), Some((82_557, 86_400)))),
            Some("<1/1 22:55:57/[24:00:00]>".to_string())
        );
    }

    #[test]
    fn charge_line_multi_charge_partial() {
        assert_eq!(
            format_charge_line(&charged_detail(Some(2), Some(3), Some((0, 1800)))),
            Some("<2/3 0:00:00/[0:30:00]>".to_string())
        );
    }

    #[test]
    fn jobs_all_when_unrestricted() {
        assert_eq!(format_jobs(0), "All Jobs");
        let every: u32 = (1..32u32)
            .filter(|bit| ffxi_vocab::job_names::abbrev(*bit as u16).is_some())
            .fold(0, |m, bit| m | (1 << bit));
        assert_eq!(format_jobs(every), "All Jobs");
        assert_eq!(
            format_jobs(every | 1),
            "All Jobs",
            "the JOB_NONE bit is ignored"
        );

        // The client item DAT jobs field is 0-indexed (bit == job id).
        // bit 1 = WAR (job 1), bit 2 = MNK (job 2) — White Belt's DAT jobs is
        // 0x04 = bit 2 = MNK. bit 4 = BLM (job 4).
        assert_eq!(format_jobs(1 << 1), "WAR");
        assert_eq!(format_jobs(1 << 2), "MNK");
        assert_eq!(format_jobs((1 << 1) | (1 << 4)), "WAR/BLM");
    }

    #[test]
    fn races_collapse_per_gender_bits() {
        assert_eq!(format_races(0), "All Races");
        assert_eq!(
            format_races(0x01FE),
            "All Races",
            "all 8 race bits set; 1-indexed race ids: Hume M = bit 1, Mithra = bit 7, Galka = bit 8"
        );
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
    fn options_are_auto_manual_recycle_bin() {
        assert_eq!(
            SORT_OPTIONS,
            &[
                SortOptionId::Auto,
                SortOptionId::Manual,
                SortOptionId::RecycleBin
            ]
        );
    }

    #[test]
    fn apply_sort_option_selects_mode() {
        let mut s = SortOptions { auto: true };
        assert!(apply_sort_option(&mut s, SortOptionId::Manual));
        assert!(!s.auto);
        assert!(apply_sort_option(&mut s, SortOptionId::Auto));
        assert!(s.auto);
        assert!(!apply_sort_option(&mut s, SortOptionId::RecycleBin));
        assert!(s.auto, "recycle bin leaves the sort mode alone");
    }

    #[test]
    fn focus_defaults_to_list() {
        let focus = ItemMenuFocus::default();
        assert!(!focus.sort_focused());
        assert_eq!(focus.sort_selection(), None);
    }

    #[test]
    fn focus_enter_and_exit_sort() {
        let mut focus = ItemMenuFocus::default();
        focus.enter_sort(SortOptionId::Manual);
        assert!(focus.sort_focused());
        assert_eq!(focus.sort_selection(), Some(SortOptionId::Manual));
        focus.exit_sort();
        assert!(!focus.sort_focused());
        assert_eq!(focus.sort_selection(), None);
    }

    #[test]
    fn sort_pane_up_down_wrap_through_options() {
        let mut focus = ItemMenuFocus::Sort(SortOptionId::Auto);
        let mut sort = SortOptions::default();
        // Up from the first row wraps to the last.
        assert_eq!(sort_pane_key(&mut focus, &mut sort, SortPaneKey::Up), None);
        assert_eq!(focus.sort_selection(), Some(SortOptionId::RecycleBin));
        // Down from the last row wraps back to the first.
        assert_eq!(
            sort_pane_key(&mut focus, &mut sort, SortPaneKey::Down),
            None
        );
        assert_eq!(focus.sort_selection(), Some(SortOptionId::Auto));
        assert_eq!(
            sort_pane_key(&mut focus, &mut sort, SortPaneKey::Down),
            None
        );
        assert_eq!(focus.sort_selection(), Some(SortOptionId::Manual));
        // Navigation alone never touches the applied mode.
        assert!(sort.auto);
    }

    #[test]
    fn sort_pane_confirm_applies_and_reports_choice() {
        let mut focus = ItemMenuFocus::Sort(SortOptionId::Manual);
        let mut sort = SortOptions::default();
        assert_eq!(
            sort_pane_key(&mut focus, &mut sort, SortPaneKey::Confirm),
            Some(SortOptionId::Manual)
        );
        assert!(!sort.auto);
        // Confirm keeps the sort box focused, matching retail.
        assert_eq!(focus.sort_selection(), Some(SortOptionId::Manual));
    }

    #[test]
    fn sort_pane_exit_returns_focus_to_list() {
        let mut focus = ItemMenuFocus::Sort(SortOptionId::Auto);
        let mut sort = SortOptions::default();
        assert_eq!(
            sort_pane_key(&mut focus, &mut sort, SortPaneKey::Exit),
            None
        );
        assert!(!focus.sort_focused());
    }

    #[test]
    fn sort_pane_swallows_unbound_keys() {
        let mut focus = ItemMenuFocus::Sort(SortOptionId::Auto);
        let mut sort = SortOptions::default();
        assert_eq!(
            sort_pane_key(&mut focus, &mut sort, SortPaneKey::Other),
            None
        );
        assert_eq!(focus, ItemMenuFocus::Sort(SortOptionId::Auto));
        assert!(sort.auto);
    }

    #[test]
    fn sort_pane_is_noop_while_list_focused() {
        let mut focus = ItemMenuFocus::List;
        let mut sort = SortOptions::default();
        for key in [
            SortPaneKey::Up,
            SortPaneKey::Down,
            SortPaneKey::Confirm,
            SortPaneKey::Exit,
            SortPaneKey::Other,
        ] {
            assert_eq!(sort_pane_key(&mut focus, &mut sort, key), None);
            assert_eq!(focus, ItemMenuFocus::List);
            assert!(sort.auto);
        }
    }

    #[test]
    fn sort_options_active_mirrors_auto_flag() {
        assert_eq!(SortOptions { auto: true }.active(), SortOptionId::Auto);
        assert_eq!(SortOptions { auto: false }.active(), SortOptionId::Manual);
    }
}
