//! Retail-faithful Auction House screens (beads kuluu-gxm1 / kuluu-oqdr).
//!
//! Oracle: .agents/skills/retail-observe/references/auction-house.md — every
//! user-visible string below is either observed there or marked provisional.
//! Rendering reads `SceneSnapshot::auction` + [`AuctionScreenState`]; the
//! native input layer (`view_native/text_input/auction.rs`) drives focus and
//! emits the `Ah*` `AgentCommand`s.

use bevy::picking::events::PointerPress;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::Button;
use kuluu_snapshot::{AhSaleView, SceneSnapshot};

use crate::hud::bazaar_view::group_digits;
use crate::hud::digit_spinner::{self, DigitSpinner, SpinnerSlot, SpinnerUnit};
use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::item_ui::{self, transparent_placeholder};
use crate::hud::style::{cursor_prefix, text_font, theme, window_frame};
use crate::snapshot::{EventLog, SceneState, ToastEvent};

// ---------------------------------------------------------------------------
// Category tree
// ---------------------------------------------------------------------------

/// One browsable leaf: the retail display label and LSB's wire category id
/// (vendor/server/docs/Auction Categories.txt).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AhLeaf {
    pub label: &'static str,
    pub id: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AhNode {
    Leaf(AhLeaf),
    Menu {
        label: &'static str,
        children: &'static [AhNode],
    },
}

impl AhNode {
    pub fn label(&self) -> &'static str {
        match self {
            AhNode::Leaf(l) => l.label,
            AhNode::Menu { label, .. } => label,
        }
    }
}

const fn leaf(label: &'static str, id: u8) -> AhNode {
    AhNode::Leaf(AhLeaf { label, id })
}

const AMMO_MISC: &[AhNode] = &[
    leaf("Ammunition", 15),
    leaf("Fishing Gear", 47),
    leaf("Pet Items", 48),
    leaf("Grips", 62),
];

const WEAPONS: &[AhNode] = &[
    leaf("Hand-to-Hand", 1),
    leaf("Daggers", 2),
    leaf("Swords", 3),
    leaf("Great Swords", 4),
    leaf("Axes", 5),
    leaf("Great Axes", 6),
    leaf("Scythes", 7),
    leaf("Polearms", 8),
    leaf("Katana", 9),
    leaf("Great Katana", 10),
    leaf("Clubs", 11),
    leaf("Staves", 12),
    leaf("Ranged", 13),
    leaf("Instruments", 14),
    AhNode::Menu {
        label: "Ammo&Misc.",
        children: AMMO_MISC,
    },
];

// Retail equip-slot order (Neck 3rd, Waist 6th), NOT LSB id order.
const ARMOR: &[AhNode] = &[
    leaf("Shields", 16),
    leaf("Head", 17),
    leaf("Neck", 22),
    leaf("Body", 18),
    leaf("Hands", 19),
    leaf("Waist", 23),
    leaf("Legs", 20),
    leaf("Feet", 21),
    leaf("Back", 26),
    leaf("Earrings", 24),
    leaf("Rings", 25),
];

const SCROLLS: &[AhNode] = &[
    leaf("White Magic", 28),
    leaf("Black Magic", 29),
    leaf("Songs", 32),
    leaf("Ninjutsu", 31),
    leaf("Summoning", 30),
    leaf("Dice", 60),
    leaf("Geomancy", 45),
];

const MATERIALS: &[AhNode] = &[
    leaf("Smithing", 38),
    leaf("Goldsmithing", 39),
    leaf("Clothcraft", 40),
    leaf("Leathercraft", 41),
    leaf("Bonecraft", 42),
    leaf("Woodworking", 43),
    leaf("Alchemy", 44),
    leaf("Alchemy 2", 63),
];

const MEALS: &[AhNode] = &[
    leaf("Meat & Eggs", 52),
    leaf("Seafood", 53),
    leaf("Vegetables", 54),
    leaf("Soups", 55),
    leaf("Breads & Rice", 56),
    leaf("Sweets", 57),
    leaf("Drinks", 58),
];

const FOOD: &[AhNode] = &[
    AhNode::Menu {
        label: "Meals",
        children: MEALS,
    },
    leaf("Ingredients", 59),
    leaf("Fish", 51),
];

const OTHERS: &[AhNode] = &[
    leaf("Misc.", 46),
    leaf("Misc. 2", 64),
    leaf("Misc. 3", 65),
    leaf("Beast-made", 50),
    leaf("Cards", 36),
    leaf("Ninja Tools", 49),
    leaf("Cursed Items", 37),
    leaf("Automaton", 61),
];

pub const AH_CATEGORY_ROOT: &[AhNode] = &[
    AhNode::Menu {
        label: "Weapons",
        children: WEAPONS,
    },
    AhNode::Menu {
        label: "Armor",
        children: ARMOR,
    },
    AhNode::Menu {
        label: "Scrolls",
        children: SCROLLS,
    },
    leaf("Medicines", 33),
    leaf("Furnishings", 34),
    AhNode::Menu {
        label: "Materials",
        children: MATERIALS,
    },
    AhNode::Menu {
        label: "Food",
        children: FOOD,
    },
    leaf("Crystals", 35),
    AhNode::Menu {
        label: "Others",
        children: OTHERS,
    },
];

/// The child list of the menu at `path` (indices of Menu nodes from the root);
/// `None` if the path crosses a leaf or runs off a list.
pub fn menu_children(path: &[usize]) -> Option<&'static [AhNode]> {
    let mut nodes = AH_CATEGORY_ROOT;
    for &idx in path {
        match nodes.get(idx)? {
            AhNode::Menu { children, .. } => nodes = children,
            AhNode::Leaf(_) => return None,
        }
    }
    Some(nodes)
}

/// The retail display label of a wire category id.
pub fn category_label(id: u8) -> Option<&'static str> {
    fn walk(nodes: &'static [AhNode], id: u8) -> Option<&'static str> {
        for node in nodes {
            match node {
                AhNode::Leaf(l) if l.id == id => return Some(l.label),
                AhNode::Leaf(_) => {}
                AhNode::Menu { children, .. } => {
                    if let Some(hit) = walk(children, id) {
                        return Some(hit);
                    }
                }
            }
        }
        None
    }
    walk(AH_CATEGORY_ROOT, id)
}

/// Every leaf category id in the tree, tree order.
pub fn leaf_ids() -> Vec<u8> {
    fn walk(nodes: &'static [AhNode], out: &mut Vec<u8>) {
        for node in nodes {
            match node {
                AhNode::Leaf(l) => out.push(l.id),
                AhNode::Menu { children, .. } => walk(children, out),
            }
        }
    }
    let mut out = Vec::new();
    walk(AH_CATEGORY_ROOT, &mut out);
    out
}

/// Whether the category gets the equipment sort menu (Damage/Delay/Level/Job/
/// Race) — the Weapons + Armor subtrees. Others get Alphabetical only.
pub fn is_equipment_category(id: u8) -> bool {
    (1..=26).contains(&id) || matches!(id, 47 | 48 | 62)
}

// ---------------------------------------------------------------------------
// Retail strings (observation record; provisional ones marked)
// ---------------------------------------------------------------------------

pub const AH_ROOT_TITLE: &str = "Auction";
pub const ROOT_BID: &str = "Bid";
pub const ROOT_SELL: &str = "Sell";
pub const ROOT_SALES_STATUS: &str = "Sales Status";
pub const HELP_BID: &str = "View all merchandise up for auction.";
pub const HELP_SELL: &str = "Place unwanted items on auction.";
pub const HELP_SALES_STATUS: &str = "Check your items currently placed on auction.";

pub const HELP_SELECT_ITEM: &str = "Select an item.";
pub const HELP_PRICE_SET: &str = "Specify the price.";
pub const HELP_HISTORY_TABLE: &str =
    "Sales data for the last ten transactions of selected merchandise.";

pub const ITEM_MENU_PRICE_HISTORY: &str = "Price History";
pub const ITEM_MENU_BID: &str = "Bid";
pub const ITEM_MENU_SORT: &str = "Sort";
pub const HELP_ITEM_PRICE_HISTORY: &str = "View recent sales data for this merchandise.";
pub const HELP_ITEM_BID: &str = "Place a bid on this merchandise.";
pub const HELP_ITEM_SORT: &str = "Rearrange the order of listed items.";

pub const SORT_RESET: &str = "Reset List";
pub const SORT_BY_DAMAGE: &str = "By Damage";
pub const SORT_BY_DELAY: &str = "By Delay";
pub const SORT_BY_LEVEL: &str = "By Level";
pub const SORT_JOB: &str = "Job";
pub const SORT_RACE: &str = "Race";
pub const SORT_ALPHABETICAL: &str = "Alphabetical";
pub const HELP_SORT_DAMAGE: &str = "Sort by highest damage rating.";
pub const HELP_SORT_DELAY: &str = "Sort by shortest delay.";
pub const HELP_SORT_LEVEL: &str = "Sort by highest required level.";
pub const HELP_SORT_JOB: &str = "Narrow down list to weapons usable by current job.";
// The record only captured "…by your race."; full phrasing provisional.
pub const HELP_SORT_RACE: &str = "Narrow down list to weapons usable by your race.";

pub const BUSY_DOWNLOADING: &str = "Downloading data ..";
pub const BUSY_PLACING_BID: &str = "Placing bid ...";
/// The animated-dot chrome around the busy text (static approximation of the
/// retail spinner frames).
pub const BUSY_PREFIX: &str = "\u{b7}\u{b7}\u{25cf}\u{25cf} ";
pub const BUSY_SUFFIX: &str = " \u{25cf}\u{25cf}\u{b7}\u{b7}";

pub const PRICE_HISTORY_TITLE: &str = "Price History";
pub const SALES_STATUS_EMPTY: &str = "You have no items up for auction.";

pub const CHAT_MERCH_PLACED: &str = "Merchandise placed on auction.";
pub const CHAT_POLICY_LINES: [&str; 3] = [
    "If merchandise remains unsold after 30 weeks (Vana'diel time), it will be returned to your current residence.",
    "If a successful bid is made, the proceeds from the sale will be delivered to your current residence.",
    "Signed items will lose their signature after being purchased.",
];
pub const CHAT_PARTIAL_STACK: &str =
    "You can only place a single item or a set of 12 such items on auction.";

pub fn chat_bid_failed(item: &str, price: u32) -> String {
    format!("You were unable to buy the {item} for {price} gil.")
}

pub fn chat_sales_count(n: usize) -> String {
    format!("You have {n} items listed for sale.")
}

pub fn fee_confirm_text(stack_quantity: Option<u32>, fee: u32) -> String {
    match stack_quantity {
        Some(qty) => {
            format!("The total transaction fee for a set of {qty} items is {fee} gil.")
        }
        None => format!("The total transaction fee for this item is {fee} gil."),
    }
}

pub fn place_confirm_text(item: &str, price: u32) -> String {
    format!(
        "Place {item} up on auction for {} gil?",
        group_digits(price)
    )
}

pub const CONFIRM_YES: &str = "Yes";
pub const CONFIRM_NO: &str = "No";

/// Top-level category help follows the observed "View weapons on auction."
/// pattern; only Weapons was captured, the rest are provisional.
pub fn category_menu_help(label: &str) -> String {
    format!("View {} on auction.", label.to_lowercase())
}

pub const HELP_H2H: &str = "Knuckles, claws, and other hand-to-hand weapons.";
/// Provisional generic for the sub-rows the recording did not narrate.
pub const HELP_SELECT_CATEGORY: &str = "Select a category.";

// ---------------------------------------------------------------------------
// Sort menu
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortChoice {
    /// Clear the params: LSB then orders by ascending itemid (retail's
    /// natural order).
    Reset,
    /// Server-side ORDER BY param (`ffxi_proto::search::SORT_*`).
    Param(u32),
    /// Client-side narrow (LSB comments: Job/Race filter client-side).
    JobFilter,
    RaceFilter,
}

pub fn sort_rows(category: u8) -> Vec<(&'static str, SortChoice, String)> {
    use ffxi_proto::search as s;
    if is_equipment_category(category) {
        vec![
            (SORT_RESET, SortChoice::Reset, HELP_ITEM_SORT.to_string()),
            (
                SORT_BY_DAMAGE,
                SortChoice::Param(s::SORT_DAMAGE_DESC),
                HELP_SORT_DAMAGE.to_string(),
            ),
            (
                SORT_BY_DELAY,
                SortChoice::Param(s::SORT_DELAY_DESC),
                HELP_SORT_DELAY.to_string(),
            ),
            (
                SORT_BY_LEVEL,
                SortChoice::Param(s::SORT_LEVEL_DESC),
                HELP_SORT_LEVEL.to_string(),
            ),
            (SORT_JOB, SortChoice::JobFilter, HELP_SORT_JOB.to_string()),
            (
                SORT_RACE,
                SortChoice::RaceFilter,
                HELP_SORT_RACE.to_string(),
            ),
        ]
    } else {
        vec![
            (SORT_RESET, SortChoice::Reset, HELP_ITEM_SORT.to_string()),
            (
                SORT_ALPHABETICAL,
                SortChoice::Param(s::SORT_NAME),
                HELP_ITEM_SORT.to_string(),
            ),
        ]
    }
}

// ---------------------------------------------------------------------------
// Dates
// ---------------------------------------------------------------------------

/// `YY/M/D` (e.g. `26/8/4`) from a unix `sell_date`, UTC. Days-to-civil per
/// Howard Hinnant's algorithm; no chrono dependency.
pub fn format_sell_date(unix: u32) -> String {
    let days = (unix / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 {
        y += 1;
    }
    format!("{:02}/{m}/{d}", y.rem_euclid(100))
}

/// Seller/buyer names ellipsize past this width (record: `Firedra…`).
pub const HISTORY_NAME_MAX: usize = 7;

pub fn ellipsize(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        name.to_string()
    } else {
        let cut: String = name.chars().take(max).collect();
        format!("{cut}\u{2026}")
    }
}

/// One rendered price-history row: `date  seller -> buyer  price G`.
pub fn history_row_text(sale: &AhSaleView) -> String {
    format!(
        "{}  {} \u{2192} {}",
        format_sell_date(sale.sell_date),
        ellipsize(&sale.seller, HISTORY_NAME_MAX),
        ellipsize(&sale.buyer, HISTORY_NAME_MAX),
    )
}

// ---------------------------------------------------------------------------
// Screen state
// ---------------------------------------------------------------------------

pub const CATALOG_ROWS: usize = 10;
pub const SALES_SLOTS: usize = kuluu_snapshot::AH_SALES_SLOT_COUNT;
/// Widest dock menu (the 15-entry Weapons list).
pub const MAX_DOCK_ROWS: usize = 16;
pub const HISTORY_ROWS: usize = 10;

/// GP_CLI_COMMAND_AUC::validate's Commission/BidPrice cap.
pub const PRICE_CAP: u32 = ffxi_proto::decode::AUCTION_PRICE_MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YesNo {
    Yes,
    No,
}

impl YesNo {
    pub fn toggled(self) -> Self {
        match self {
            YesNo::Yes => YesNo::No,
            YesNo::No => YesNo::Yes,
        }
    }
}

/// The inventory item being listed for sale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SellPick {
    pub inv_slot: u8,
    pub item_no: u16,
    pub quantity: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CatalogOverlay {
    ItemMenu { cursor: usize },
    Sort { cursor: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryReturn {
    Catalog,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AhScreen {
    Root {
        cursor: usize,
    },
    /// Browsing the category tree in the right dock. `path` = Menu indices
    /// from the tree root ([] = the 9-entry top level).
    Category {
        path: Vec<usize>,
        cursor: usize,
    },
    Catalog {
        cursor: usize,
        overlay: Option<CatalogOverlay>,
    },
    History {
        return_to: HistoryReturn,
    },
    BidPrice {
        item_no: u16,
        stack: bool,
        spinner: DigitSpinner,
    },
    SellList {
        cursor: usize,
    },
    /// Single-vs-stack chooser for a stackable sale (UI provisional — the
    /// recording never captured retail's chooser; observation record open
    /// question 3).
    SellStack {
        sell: SellPick,
        cursor: usize,
    },
    SellPrice {
        sell: SellPick,
        stack: bool,
        spinner: DigitSpinner,
    },
    FeeConfirm {
        sell: SellPick,
        stack: bool,
        price: u32,
        cursor: YesNo,
    },
    /// Default cursor Yes is provisional (observation record open question 1).
    PlaceConfirm {
        sell: SellPick,
        stack: bool,
        price: u32,
        cursor: YesNo,
    },
    SalesStatus {
        cursor: usize,
    },
    CancelConfirm {
        slot: usize,
        cursor: YesNo,
    },
}

/// A completed placement pending its LotIn ack — carries what the retail chat
/// echo needs (fee line + "Merchandise placed on auction.").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingListing {
    pub item_no: u16,
    pub fee: u32,
    pub price: u32,
    /// `Some(stack size)` when listing a stack.
    pub stack_quantity: Option<u32>,
}

#[derive(Resource, Debug, Clone, PartialEq)]
pub struct AuctionScreenState {
    /// The UI is open (the counter menu was triggered and not yet Esc'd out).
    pub active: bool,
    pub screen: AhScreen,
    /// Path + leaf index of the category currently browsed in the catalog, so
    /// Esc restores the category menu with its cursor remembered and the dock
    /// can render the sibling tab stack.
    pub browse_path: Vec<usize>,
    pub browse_leaf: usize,
    /// Current server-side sort params, re-sent by the Sort menu.
    pub sorts: Vec<u32>,
    /// Client-side Job narrow (LSB: Job/Race filter client-side).
    pub job_filter: bool,
    /// Catalog cursor stashed while History/BidPrice sit on top.
    pub catalog_cursor: usize,
    /// An `AhSell` quote request is in flight; the mode-sync system promotes
    /// Price Set to the fee confirm when the quote lands.
    pub awaiting_quote: bool,
    pub pending_listing: Option<PendingListing>,
}

impl Default for AuctionScreenState {
    fn default() -> Self {
        Self {
            active: false,
            screen: AhScreen::Root { cursor: 0 },
            browse_path: Vec::new(),
            browse_leaf: 0,
            sorts: Vec::new(),
            job_filter: false,
            catalog_cursor: 0,
            awaiting_quote: false,
            pending_listing: None,
        }
    }
}

impl AuctionScreenState {
    pub fn open(&mut self) {
        *self = AuctionScreenState {
            active: true,
            ..Default::default()
        };
    }

    pub fn close(&mut self) {
        *self = AuctionScreenState::default();
    }

    /// The leaf currently browsed in the catalog.
    pub fn browse_leaf_node(&self) -> Option<AhLeaf> {
        match menu_children(&self.browse_path)?.get(self.browse_leaf)? {
            AhNode::Leaf(l) => Some(*l),
            AhNode::Menu { .. } => None,
        }
    }
}

/// Up/Down wrap top↔bottom in every AH menu/list (observation record).
pub fn wrap_up(cursor: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else if cursor == 0 {
        len - 1
    } else {
        cursor - 1
    }
}

pub fn wrap_down(cursor: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (cursor + 1) % len
    }
}

/// First row of the viewport window that keeps `cursor` visible in a
/// `rows`-tall list (same centering as the delivery/inventory lists).
pub fn viewport_start(cursor: usize, total: usize, rows: usize) -> usize {
    if total <= rows {
        return 0;
    }
    let half = rows / 2;
    let max_start = total - rows;
    cursor.saturating_sub(half).min(max_start)
}

/// The bracketed stock count for a catalog row: singles plus stacks currently
/// listed (stack count absent for unstackables). `None` (no bracket) when
/// nothing is listed.
pub fn stock_count(listing: &kuluu_snapshot::AhListingView) -> Option<u32> {
    let total = listing.singles_for_sale + listing.stacks_for_sale.unwrap_or(0);
    (total > 0).then_some(total)
}

/// Which form (single/stack) a history or bid request should target when the
/// player has not chosen: singles unless only stacks are listed. The retail
/// single-vs-stack chooser is uncaptured (record open question 3).
pub fn default_stack_form(listing: &kuluu_snapshot::AhListingView) -> bool {
    listing.singles_for_sale == 0 && listing.stacks_for_sale.unwrap_or(0) > 0
}

// ---------------------------------------------------------------------------
// Sellable inventory
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SellRow {
    pub inv_slot: u8,
    pub item_no: u16,
    pub quantity: u32,
}

/// The AH sell picker list, rebuilt on snapshot change while the UI is open.
#[derive(Resource, Debug, Default, Clone)]
pub struct AuctionSellInventory {
    pub rows: Vec<SellRow>,
}

/// LOC_INVENTORY items the sell picker offers: not gil, not locked, and not
/// @FLAG_NOAUCTION (auctionutils SellingItems).
pub fn build_sell_rows(snap: &SceneSnapshot) -> Vec<SellRow> {
    snap.inventory_main()
        .iter()
        .filter(|it| {
            it.index != 0
                && it.item_no != 0
                && it.item_no != ffxi_proto::map::GIL_ITEM_NO
                && !it.locked
                && ffxi_vocab::item_flags::auctionable(it.item_no)
        })
        .map(|it| SellRow {
            inv_slot: it.index,
            item_no: it.item_no,
            quantity: it.quantity,
        })
        .collect()
}

pub fn rebuild_sell_inventory(
    state: Res<SceneState>,
    screen: Res<AuctionScreenState>,
    mut inv: ResMut<AuctionSellInventory>,
) {
    if !state.is_changed() && !screen.is_changed() {
        return;
    }
    if !screen.active {
        if !inv.rows.is_empty() {
            inv.rows.clear();
        }
        return;
    }
    let rows = build_sell_rows(&state.snapshot);
    if rows != inv.rows {
        inv.rows = rows;
    }
}

pub fn item_name(item_no: u16) -> String {
    ffxi_vocab::item_names::lookup(item_no)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Item #{item_no}"))
}

// ---------------------------------------------------------------------------
// Dock (right-side) menu model — shared by rendering, help bar, and input
// ---------------------------------------------------------------------------

/// What the right dock shows for the current screen: rows + the highlighted
/// index (`cursor: None` = display-only, e.g. the catalog tab stack).
pub struct DockView {
    pub title: String,
    pub rows: Vec<String>,
    pub cursor: Option<usize>,
    /// Highlighted-but-unfocused row (the active catalog tab in gold).
    pub active_tab: Option<usize>,
}

pub fn dock_view(state: &AuctionScreenState) -> Option<DockView> {
    match &state.screen {
        AhScreen::Root { cursor } => Some(DockView {
            title: AH_ROOT_TITLE.to_string(),
            rows: vec![
                ROOT_BID.to_string(),
                ROOT_SELL.to_string(),
                ROOT_SALES_STATUS.to_string(),
            ],
            cursor: Some(*cursor),
            active_tab: None,
        }),
        AhScreen::Category { path, cursor } => {
            let children = menu_children(path)?;
            let title = if path.is_empty() {
                AH_ROOT_TITLE.to_string()
            } else {
                // The submenu replaces the parent list in place; title the pane
                // with the menu's own label.
                let parent = menu_children(&path[..path.len() - 1])?;
                parent.get(*path.last()?)?.label().to_string()
            };
            Some(DockView {
                title,
                rows: children.iter().map(|n| n.label().to_string()).collect(),
                cursor: Some(*cursor),
                active_tab: None,
            })
        }
        AhScreen::Catalog {
            overlay: Some(CatalogOverlay::ItemMenu { cursor }),
            ..
        } => Some(DockView {
            title: String::new(),
            rows: vec![
                ITEM_MENU_PRICE_HISTORY.to_string(),
                ITEM_MENU_BID.to_string(),
                ITEM_MENU_SORT.to_string(),
            ],
            cursor: Some(*cursor),
            active_tab: None,
        }),
        AhScreen::Catalog {
            overlay: Some(CatalogOverlay::Sort { cursor }),
            ..
        } => {
            let category = state.browse_leaf_node()?.id;
            Some(DockView {
                title: ITEM_MENU_SORT.to_string(),
                rows: sort_rows(category)
                    .iter()
                    .map(|(label, _, _)| label.to_string())
                    .collect(),
                cursor: Some(*cursor),
                active_tab: None,
            })
        }
        AhScreen::Catalog { overlay: None, .. }
        | AhScreen::History { .. }
        | AhScreen::BidPrice { .. } => {
            // Sibling tab stack, active leaf in gold.
            let children = menu_children(&state.browse_path)?;
            Some(DockView {
                title: String::new(),
                rows: children.iter().map(|n| n.label().to_string()).collect(),
                cursor: None,
                active_tab: Some(state.browse_leaf),
            })
        }
        AhScreen::SellList { .. } | AhScreen::SellPrice { .. } => Some(DockView {
            title: AH_ROOT_TITLE.to_string(),
            rows: vec![
                ROOT_BID.to_string(),
                ROOT_SELL.to_string(),
                ROOT_SALES_STATUS.to_string(),
            ],
            cursor: None,
            active_tab: Some(1),
        }),
        AhScreen::SellStack { sell, cursor } => Some(DockView {
            title: item_name(sell.item_no),
            rows: vec![sell_single_label(), sell_stack_label(sell.quantity)],
            cursor: Some(*cursor),
            active_tab: None,
        }),
        AhScreen::SalesStatus { .. } | AhScreen::CancelConfirm { .. } => Some(DockView {
            title: AH_ROOT_TITLE.to_string(),
            rows: vec![
                ROOT_BID.to_string(),
                ROOT_SELL.to_string(),
                ROOT_SALES_STATUS.to_string(),
            ],
            cursor: None,
            active_tab: Some(2),
        }),
        AhScreen::FeeConfirm { .. } | AhScreen::PlaceConfirm { .. } => None,
    }
}

/// Single/stack chooser labels (provisional wording — record open question 3).
pub fn sell_single_label() -> String {
    "Single".to_string()
}

pub fn sell_stack_label(quantity: u32) -> String {
    format!("Stack ({quantity})")
}

// ---------------------------------------------------------------------------
// Help bar
// ---------------------------------------------------------------------------

/// `(mode label, help text)` for the top bar. During an async op the help slot
/// carries the retail dot-spinner line; during a modal confirm it blanks.
pub fn help_bar_content(
    state: &AuctionScreenState,
    snap: &SceneSnapshot,
    inv: &AuctionSellInventory,
) -> (String, String) {
    if let Some(busy) = snap.auction.busy {
        let text = match busy {
            kuluu_snapshot::AuctionBusy::Downloading => BUSY_DOWNLOADING,
            kuluu_snapshot::AuctionBusy::PlacingBid => BUSY_PLACING_BID,
        };
        return (
            mode_label(state).to_string(),
            format!("{BUSY_PREFIX}{text}{BUSY_SUFFIX}"),
        );
    }
    let hint = match &state.screen {
        AhScreen::Root { cursor } => match cursor {
            0 => HELP_BID.to_string(),
            1 => HELP_SELL.to_string(),
            _ => HELP_SALES_STATUS.to_string(),
        },
        AhScreen::Category { path, cursor } => {
            let node = menu_children(path).and_then(|c| c.get(*cursor));
            match node {
                Some(node) if path.is_empty() => category_menu_help(node.label()),
                Some(AhNode::Leaf(l)) if l.id == 1 => HELP_H2H.to_string(),
                Some(_) => HELP_SELECT_CATEGORY.to_string(),
                None => String::new(),
            }
        }
        AhScreen::Catalog {
            overlay: Some(CatalogOverlay::ItemMenu { cursor }),
            ..
        } => match cursor {
            0 => HELP_ITEM_PRICE_HISTORY.to_string(),
            1 => HELP_ITEM_BID.to_string(),
            _ => HELP_ITEM_SORT.to_string(),
        },
        AhScreen::Catalog {
            overlay: Some(CatalogOverlay::Sort { cursor }),
            ..
        } => state
            .browse_leaf_node()
            .map(|l| sort_rows(l.id))
            .and_then(|rows| rows.get(*cursor).map(|(_, _, help)| help.clone()))
            .unwrap_or_default(),
        AhScreen::Catalog { overlay: None, .. } => HELP_SELECT_ITEM.to_string(),
        AhScreen::History { .. } => HELP_HISTORY_TABLE.to_string(),
        AhScreen::BidPrice { .. } | AhScreen::SellPrice { .. } => HELP_PRICE_SET.to_string(),
        AhScreen::SellList { .. } | AhScreen::SellStack { .. } => {
            let _ = inv;
            HELP_SELECT_ITEM.to_string()
        }
        AhScreen::SalesStatus { .. } | AhScreen::CancelConfirm { .. } => String::new(),
        // The bar dims while a modal Yes/No has focus.
        AhScreen::FeeConfirm { .. } | AhScreen::PlaceConfirm { .. } => String::new(),
    };
    (mode_label(state).to_string(), hint)
}

pub fn mode_label(state: &AuctionScreenState) -> &'static str {
    match &state.screen {
        AhScreen::Root { .. } => "Auction",
        AhScreen::Category { path, .. } => {
            if path.is_empty() {
                "Auction"
            } else {
                menu_children(&path[..path.len() - 1])
                    .and_then(|parent| path.last().and_then(|&i| parent.get(i)))
                    .map(|n| n.label())
                    .unwrap_or("Auction")
            }
        }
        AhScreen::Catalog { overlay: None, .. } => "Bid",
        AhScreen::Catalog { .. } => "Auction",
        AhScreen::History { .. } => "Price History",
        AhScreen::BidPrice { .. } | AhScreen::SellPrice { .. } => "Price Set",
        AhScreen::SellList { .. } | AhScreen::SellStack { .. } => "Items",
        AhScreen::SalesStatus { .. } | AhScreen::CancelConfirm { .. } => "Sales Status",
        AhScreen::FeeConfirm { .. } | AhScreen::PlaceConfirm { .. } => "Auction",
    }
}

// ---------------------------------------------------------------------------
// Chat echoes (AH results arrive as edge-triggered ViewerEvents)
// ---------------------------------------------------------------------------

#[derive(Resource, Debug, Default)]
pub struct AuctionEventCursor(pub u64);

pub fn auction_chat_echo_system(
    events: Res<EventLog>,
    mut cursor: ResMut<AuctionEventCursor>,
    mut screen: ResMut<AuctionScreenState>,
    mut toasts: MessageWriter<ToastEvent>,
) {
    use kuluu_snapshot::{ChatChannel, ChatLine, ChatSpan, ChatSpanKind, ViewerEvent};
    let total = events.pushed_total;
    let len = events.recent.len() as u64;
    let first_global = total - len;
    let start = cursor.0.max(first_global);
    let white = |text: String| ChatLine {
        spans: Vec::new(),
        channel: ChatChannel::System,
        sender: String::new(),
        text,
        server_ts: 0,
        local_seq: 0,
    };
    for i in start..total {
        let ev = &events.recent[(i - first_global) as usize];
        match ev {
            ViewerEvent::AuctionBidResult {
                ok: false,
                item_no,
                price,
                ..
            } => {
                // Retail renders the item name green (spans), rest white.
                let name = item_name(*item_no);
                let text = chat_bid_failed(&name, *price);
                let (before, after) = match text.split_once(&name) {
                    Some((b, a)) => (b.to_string(), a.to_string()),
                    None => (text.clone(), String::new()),
                };
                toasts.write(ToastEvent {
                    line: ChatLine {
                        spans: vec![
                            ChatSpan {
                                text: before,
                                kind: ChatSpanKind::Text,
                            },
                            ChatSpan {
                                text: name,
                                kind: ChatSpanKind::Item,
                            },
                            ChatSpan {
                                text: after,
                                kind: ChatSpanKind::Text,
                            },
                        ],
                        channel: ChatChannel::System,
                        sender: String::new(),
                        text,
                        server_ts: 0,
                        local_seq: 0,
                    },
                });
            }
            // Successful-bid choreography is uncaptured (record open question
            // 2): the item delivery already lands via inventory packets; no
            // invented chat line.
            ViewerEvent::AuctionBidResult { ok: true, .. } => {}
            ViewerEvent::AuctionSellResult { ok } => {
                let pending = screen.pending_listing.take();
                if *ok {
                    if let Some(p) = pending {
                        toasts.write(ToastEvent {
                            line: white(fee_confirm_text(p.stack_quantity, p.fee)),
                        });
                    }
                    toasts.write(ToastEvent {
                        line: white(CHAT_MERCH_PLACED.to_string()),
                    });
                    for line in CHAT_POLICY_LINES {
                        toasts.write(ToastEvent {
                            line: white(line.to_string()),
                        });
                    }
                }
            }
            // 197 = auctionutils SellingItems reject; the partial-stack line is
            // the observed instance (other 197 causes render the same line —
            // provisional until more codes are captured).
            ViewerEvent::AuctionSellRefused { .. } => {
                toasts.write(ToastEvent {
                    line: white(CHAT_PARTIAL_STACK.to_string()),
                });
            }
            ViewerEvent::AuctionSearchFailed { message } => {
                toasts.write(ToastEvent::system(format!(
                    "Auction search failed: {message}"
                )));
            }
            _ => {}
        }
    }
    cursor.0 = total;
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    DockTitle,
    DockRow(usize),
    ListRowText(usize),
    ListRowCount(usize),
    ListEmpty,
    HistTitle,
    HistHeader,
    HistRow(usize),
    HistRowPrice(usize),
    DetailName,
    DetailRow(usize),
    GilLine,
    Spinner(SpinnerSlot),
    ConfirmText,
    ConfirmChoice(YesNo),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IconId {
    ListRow(usize),
    Detail,
    HistItem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameId {
    Dock,
    List,
    ListRow(usize),
    Hist,
    Detail,
    GilBox,
    SpinnerBox,
    Confirm,
}

#[derive(Component)]
pub struct AuctionScreenRoot;

#[derive(Component)]
pub(crate) struct AhText(Role);

#[derive(Component)]
pub(crate) struct AhIcon(IconId);

#[derive(Component)]
pub(crate) struct AhFrame(FrameId);

/// Mouse-interactive regions (hover moves the cursor, click activates).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AhRegion {
    Dock,
    List,
    Confirm,
}

#[derive(Component)]
pub struct AhHotRow {
    pub region: AhRegion,
    pub slot: usize,
}

#[derive(Message, Debug, Clone, Copy)]
pub struct AuctionRowActivated {
    pub region: AhRegion,
    pub slot: usize,
}

const DETAIL_ROWS: usize = 8;
const DETAIL_ICON_PX: f32 = 32.0;
const LIST_ICON_PX: f32 = 18.0;
const LEFT_COL_W: f32 = 340.0;
const DOCK_W: f32 = 200.0;
/// The cap line under the price, small enough to sit beside the Current Gil box.
const SPINNER_CAP_TEXT_PX: f32 = 12.0;

pub fn spawn_auction_screen(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = transparent_placeholder(&mut images);

    commands
        .spawn((
            crate::components::InGameEntity,
            AuctionScreenRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(crate::hud::menu_help_bar::help_line_bottom_left().y + 8.0),
                left: Val::Px(8.0),
                right: Val::Px(8.0),
                bottom: Val::Px(54.0),
                display: Display::None,
                ..default()
            },
            GlobalZIndex(crate::hud::style::WINDOW_Z),
        ))
        .with_children(|root| {
            // Left column: list / history / detail / price entry / confirm.
            root.spawn(Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Px(LEFT_COL_W),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|col| {
                // Item list (catalog / sell / sales status).
                let (n, bg, bd) = window_frame();
                col.spawn((AhFrame(FrameId::List), n, bg, bd))
                    .with_children(|p| {
                        spawn_text(p, Role::ListEmpty, 13.0, theme::TEXT);
                        for i in 0..CATALOG_ROWS {
                            spawn_list_row(p, i, placeholder.clone());
                        }
                    });

                // Price-history table.
                let (n, bg, bd) = window_frame();
                col.spawn((AhFrame(FrameId::Hist), n, bg, bd))
                    .with_children(|p| {
                        spawn_text(p, Role::HistTitle, 14.0, theme::TITLE);
                        p.spawn(Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(6.0),
                            ..default()
                        })
                        .with_children(|h| {
                            h.spawn((
                                AhIcon(IconId::HistItem),
                                Node {
                                    width: Val::Px(LIST_ICON_PX),
                                    height: Val::Px(LIST_ICON_PX),
                                    display: Display::None,
                                    ..default()
                                },
                                ImageNode::new(placeholder.clone()),
                            ));
                            spawn_text(h, Role::HistHeader, 13.0, theme::TITLE);
                        });
                        for i in 0..HISTORY_ROWS {
                            p.spawn(Node {
                                flex_direction: FlexDirection::Row,
                                justify_content: JustifyContent::SpaceBetween,
                                column_gap: Val::Px(8.0),
                                ..default()
                            })
                            .with_children(|row| {
                                spawn_text(row, Role::HistRow(i), 12.0, theme::TEXT);
                                spawn_text(row, Role::HistRowPrice(i), 12.0, theme::TEXT);
                            });
                        }
                    });

                // Item detail card.
                let (n, bg, bd) = window_frame();
                col.spawn((AhFrame(FrameId::Detail), n, bg, bd))
                    .with_children(|p| {
                        p.spawn(Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(6.0),
                            ..default()
                        })
                        .with_children(|h| {
                            h.spawn((
                                AhIcon(IconId::Detail),
                                Node {
                                    width: Val::Px(DETAIL_ICON_PX),
                                    height: Val::Px(DETAIL_ICON_PX),
                                    display: Display::None,
                                    ..default()
                                },
                                ImageNode::new(placeholder.clone()),
                            ));
                            spawn_text(h, Role::DetailName, 14.0, theme::TITLE);
                        });
                        for i in 0..DETAIL_ROWS {
                            spawn_text(p, Role::DetailRow(i), 12.0, theme::TEXT);
                        }
                    });

                // Price entry bar: Current Gil box + digit spinner box.
                col.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(6.0),
                    ..default()
                })
                .with_children(|bar| {
                    let (n, bg, bd) = window_frame();
                    bar.spawn((AhFrame(FrameId::GilBox), n, bg, bd))
                        .with_children(|p| {
                            spawn_text(p, Role::GilLine, 13.0, theme::TEXT);
                        });
                    let (n, bg, bd) = window_frame();
                    bar.spawn((AhFrame(FrameId::SpinnerBox), n, bg, bd))
                        .with_children(|p| {
                            digit_spinner::spawn_row(p, digit_spinner::slots_without_cap(), |s| {
                                AhText(Role::Spinner(s))
                            });
                            spawn_text(
                                p,
                                Role::Spinner(SpinnerSlot::Cap),
                                SPINNER_CAP_TEXT_PX,
                                theme::MUTED,
                            );
                        });
                });
            });

            // Right dock.
            let (mut n, bg, bd) = window_frame();
            n.position_type = PositionType::Absolute;
            n.top = Val::Px(0.0);
            n.right = Val::Px(0.0);
            n.width = Val::Px(DOCK_W);
            root.spawn((AhFrame(FrameId::Dock), n, bg, bd))
                .with_children(|p| {
                    spawn_text(p, Role::DockTitle, 14.0, theme::TITLE);
                    for i in 0..MAX_DOCK_ROWS {
                        p.spawn((
                            AhText(Role::DockRow(i)),
                            AhHotRow {
                                region: AhRegion::Dock,
                                slot: i,
                            },
                            Button,
                            Hovered::default(),
                            Text::new(""),
                            text_font(14.0),
                            TextColor(theme::TEXT),
                            BackgroundColor(Color::NONE),
                        ));
                    }
                });

            // Yes/No confirm (lower-left, retail's fee/placement dialogs).
            let (mut n, bg, bd) = window_frame();
            n.position_type = PositionType::Absolute;
            n.bottom = Val::Px(40.0);
            n.left = Val::Px(0.0);
            n.min_width = Val::Px(280.0);
            root.spawn((AhFrame(FrameId::Confirm), n, bg, bd))
                .with_children(|p| {
                    spawn_text(p, Role::ConfirmText, 14.0, theme::TEXT);
                    for (slot, choice) in [YesNo::Yes, YesNo::No].into_iter().enumerate() {
                        p.spawn((
                            AhText(Role::ConfirmChoice(choice)),
                            AhHotRow {
                                region: AhRegion::Confirm,
                                slot,
                            },
                            Button,
                            Hovered::default(),
                            Text::new(""),
                            text_font(14.0),
                            TextColor(theme::TEXT),
                            BackgroundColor(Color::NONE),
                        ));
                    }
                });
        });
}

fn spawn_text(p: &mut ChildSpawnerCommands, role: Role, size: f32, color: Color) {
    p.spawn((
        AhText(role),
        Text::new(""),
        text_font(size),
        TextColor(color),
    ));
}

fn spawn_list_row(p: &mut ChildSpawnerCommands, i: usize, placeholder: Handle<Image>) {
    p.spawn((
        AhFrame(FrameId::ListRow(i)),
        AhHotRow {
            region: AhRegion::List,
            slot: i,
        },
        Button,
        Hovered::default(),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(4.0),
            display: Display::None,
            ..default()
        },
        BackgroundColor(Color::NONE),
    ))
    .with_children(|row| {
        row.spawn((
            AhIcon(IconId::ListRow(i)),
            Node {
                width: Val::Px(LIST_ICON_PX),
                height: Val::Px(LIST_ICON_PX),
                display: Display::None,
                ..default()
            },
            ImageNode::new(placeholder),
        ));
        row.spawn((
            AhText(Role::ListRowText(i)),
            Text::new(""),
            text_font(13.0),
            TextColor(theme::TEXT),
            Node {
                flex_grow: 1.0,
                ..default()
            },
        ));
        row.spawn((
            AhText(Role::ListRowCount(i)),
            Text::new(""),
            text_font(13.0),
            TextColor(theme::TEXT),
        ));
    });
}

/// Everything `update_auction_screen` derives once per frame.
struct FrameModel {
    dock: Option<DockView>,
    list: ListModel,
    hist: Option<HistModel>,
    detail: Option<u16>,
    price: Option<PriceModel>,
    confirm: Option<ConfirmModel>,
}

#[derive(Default)]
struct ListModel {
    visible: bool,
    empty_text: String,
    rows: Vec<ListRowModel>,
}

struct ListRowModel {
    item_no: u16,
    text: String,
    count: String,
    is_cursor: bool,
    muted: bool,
}

struct HistModel {
    header: String,
    header_item: Option<u16>,
    rows: Vec<AhSaleView>,
}

struct PriceModel {
    gil: u32,
    spinner: DigitSpinner,
}

struct ConfirmModel {
    text: String,
    cursor: YesNo,
}

fn frame_model(
    state: &AuctionScreenState,
    snap: &SceneSnapshot,
    inv: &AuctionSellInventory,
) -> FrameModel {
    let busy_download = snap.auction.busy == Some(kuluu_snapshot::AuctionBusy::Downloading);
    let gil = crate::hud::delivery::current_gil(snap);
    let mut model = FrameModel {
        dock: dock_view(state),
        list: ListModel::default(),
        hist: None,
        detail: None,
        price: None,
        confirm: None,
    };

    match &state.screen {
        AhScreen::Root { .. } | AhScreen::Category { .. } => {}
        AhScreen::Catalog { cursor, .. } => {
            if busy_download {
                return model;
            }
            if snap.auction.browse.is_some() {
                let listings = filtered_listings(state, snap);
                let start = viewport_start(*cursor, listings.len(), CATALOG_ROWS);
                model.list.visible = true;
                model.list.rows = listings
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(CATALOG_ROWS)
                    .map(|(idx, l)| ListRowModel {
                        item_no: l.item_id,
                        text: item_name(l.item_id),
                        count: stock_count(l).map(|n| format!("[{n}]")).unwrap_or_default(),
                        is_cursor: idx == *cursor,
                        muted: false,
                    })
                    .collect();
                model.detail = listings.get(*cursor).map(|l| l.item_id);
            }
        }
        AhScreen::History { .. } => {
            if busy_download {
                return model;
            }
            if let Some(h) = snap.auction.history.as_ref() {
                model.hist = Some(HistModel {
                    header: history_header(h),
                    header_item: Some(h.item_id),
                    rows: h.sales.clone(),
                });
            }
        }
        AhScreen::BidPrice {
            item_no, spinner, ..
        } => {
            model.detail = Some(*item_no);
            model.price = Some(PriceModel {
                gil,
                spinner: spinner.clone(),
            });
        }
        AhScreen::SellList { .. } | AhScreen::SellStack { .. } => {
            // The chooser keeps the picked row selected; the list itself only
            // holds the cursor on the SellList screen.
            let (sel, focused_list) = match &state.screen {
                AhScreen::SellList { cursor } => (*cursor, true),
                AhScreen::SellStack { sell, .. } => (
                    inv.rows
                        .iter()
                        .position(|r| r.inv_slot == sell.inv_slot)
                        .unwrap_or(0),
                    false,
                ),
                _ => unreachable!("outer match arm"),
            };
            let sel = sel.min(inv.rows.len().saturating_sub(1));
            let start = viewport_start(sel, inv.rows.len(), CATALOG_ROWS);
            model.list.visible = true;
            model.list.empty_text = if inv.rows.is_empty() {
                "(nothing sellable)".to_string()
            } else {
                String::new()
            };
            model.list.rows = inv
                .rows
                .iter()
                .enumerate()
                .skip(start)
                .take(CATALOG_ROWS)
                .map(|(idx, r)| ListRowModel {
                    item_no: r.item_no,
                    text: crate::hud::menu::item_qty_label(&item_name(r.item_no), r.quantity),
                    count: String::new(),
                    is_cursor: focused_list && idx == sel,
                    muted: false,
                })
                .collect();
            model.detail = inv.rows.get(sel).map(|r| r.item_no);
        }
        AhScreen::SellPrice {
            sell,
            stack,
            spinner,
        } => {
            model.hist = snap.auction.history.as_ref().map(|h| HistModel {
                header: sell_history_header(h, *stack, sell.quantity),
                header_item: Some(sell.item_no),
                rows: h.sales.clone(),
            });
            model.price = Some(PriceModel {
                gil,
                spinner: spinner.clone(),
            });
        }
        AhScreen::FeeConfirm { sell, cursor, .. } => {
            if let Some(q) = snap.auction.fee_quote.as_ref() {
                let stack_qty = q.stack.then_some(sell.quantity).filter(|&n| n > 0);
                model.confirm = Some(ConfirmModel {
                    text: fee_confirm_text(stack_qty, q.fee),
                    cursor: *cursor,
                });
            }
        }
        AhScreen::PlaceConfirm {
            sell,
            price,
            cursor,
            ..
        } => {
            model.confirm = Some(ConfirmModel {
                text: place_confirm_text(&item_name(sell.item_no), *price),
                cursor: *cursor,
            });
        }
        AhScreen::SalesStatus { cursor } | AhScreen::CancelConfirm { slot: cursor, .. } => {
            let cursor = *cursor;
            let slots = &snap.auction.sales_status;
            let any = slots.iter().any(Option::is_some);
            model.list.visible = true;
            model.list.empty_text = if any {
                String::new()
            } else {
                SALES_STATUS_EMPTY.to_string()
            };
            if any {
                let focused = matches!(&state.screen, AhScreen::SalesStatus { .. });
                model.list.rows = slots
                    .iter()
                    .enumerate()
                    .map(|(i, s)| match s {
                        Some(s) => ListRowModel {
                            item_no: s.item_no,
                            text: crate::hud::menu::item_qty_label(
                                &item_name(s.item_no),
                                s.quantity as u32,
                            ),
                            count: format!("{} G", group_digits(s.price)),
                            is_cursor: focused && i == cursor,
                            muted: false,
                        },
                        None => ListRowModel {
                            item_no: 0,
                            text: "-".to_string(),
                            count: String::new(),
                            is_cursor: focused && i == cursor,
                            muted: true,
                        },
                    })
                    .collect();
            }
            if let AhScreen::CancelConfirm { slot, cursor } = &state.screen {
                if let Some(Some(s)) = slots.get(*slot) {
                    model.confirm = Some(ConfirmModel {
                        text: cancel_confirm_text(&item_name(s.item_no)),
                        cursor: *cursor,
                    });
                }
            }
        }
    }
    model
}

/// Provisional wording — the LotCancel confirm was not captured (record open
/// question 4).
pub fn cancel_confirm_text(item: &str) -> String {
    format!("Remove the {item} from auction?")
}

/// Bid-side history header: `<Category> :  <Item> [N]`.
fn history_header(h: &kuluu_snapshot::AhHistoryView) -> String {
    let cat = category_label(h.category as u8).unwrap_or("");
    let mut out = format!("{cat} :  {}", item_name(h.item_id));
    if h.open_listings > 0 {
        out.push_str(&format!("  [{}]", h.open_listings));
    }
    out
}

/// Sell-side Price Set header: `<Category> :  <Item>  [12]  [N]` — the stack
/// size only when listing a stack.
fn sell_history_header(h: &kuluu_snapshot::AhHistoryView, stack: bool, quantity: u32) -> String {
    let cat = category_label(h.category as u8).unwrap_or("");
    let mut out = format!("{cat} :  {}", item_name(h.item_id));
    if stack {
        out.push_str(&format!("  [{quantity}]"));
    }
    if h.open_listings > 0 {
        out.push_str(&format!("  [{}]", h.open_listings));
    }
    out
}

/// Catalog listings with the client-side Job narrow applied (LSB: the Job/Race
/// sort entries filter client-side).
pub fn filtered_listings(
    state: &AuctionScreenState,
    snap: &SceneSnapshot,
) -> Vec<kuluu_snapshot::AhListingView> {
    let Some(browse) = snap.auction.browse.as_ref() else {
        return Vec::new();
    };
    if !state.job_filter {
        return browse.listings.clone();
    }
    let main_job = snap
        .self_char_id
        .and_then(|id| snap.party.iter().find(|m| m.id == id))
        .map(|m| m.main_job)
        .unwrap_or(0);
    browse
        .listings
        .iter()
        .filter(|l| {
            main_job == 0
                || ffxi_vocab::equip_info::lookup(l.item_id)
                    .map(|info| ffxi_vocab::equip_info::fits_job(&info, main_job))
                    .unwrap_or(true)
        })
        .copied()
        .collect()
}

#[allow(clippy::type_complexity)]
pub(crate) fn update_auction_screen(
    state: Res<SceneState>,
    screen: Res<AuctionScreenState>,
    inv: Res<AuctionSellInventory>,
    dat_root: Res<ItemDatRoot>,
    mut icon_cache: ResMut<ItemIconCache>,
    mut images: ResMut<Assets<Image>>,
    mut root_q: Query<
        &mut Node,
        (
            With<AuctionScreenRoot>,
            Without<AhText>,
            Without<AhIcon>,
            Without<AhFrame>,
        ),
    >,
    mut text_q: Query<
        (&AhText, &mut Text, &mut TextColor, &mut BackgroundColor),
        (
            Without<AuctionScreenRoot>,
            Without<AhIcon>,
            Without<AhFrame>,
        ),
    >,
    mut icon_q: Query<
        (&AhIcon, &mut Node, &mut ImageNode),
        (
            Without<AuctionScreenRoot>,
            Without<AhText>,
            Without<AhFrame>,
        ),
    >,
    mut frame_q: Query<
        (&AhFrame, &mut Node, &mut BackgroundColor),
        (Without<AuctionScreenRoot>, Without<AhText>, Without<AhIcon>),
    >,
) {
    let Ok(mut root_node) = root_q.single_mut() else {
        return;
    };
    if !screen.active {
        if root_node.display != Display::None {
            root_node.display = Display::None;
        }
        return;
    }
    if root_node.display != Display::Flex {
        root_node.display = Display::Flex;
    }

    let snap = &state.snapshot;
    let model = frame_model(&screen, snap, &inv);

    let (detail_name, detail_rows) =
        item_ui::focus_detail(model.detail, None, snap, &dat_root, &mut icon_cache);
    let detail_visible = model.detail.is_some();

    // Frames.
    for (tag, mut node, mut bg) in frame_q.iter_mut() {
        let visible = match tag.0 {
            FrameId::Dock => model.dock.is_some(),
            FrameId::List => model.list.visible,
            FrameId::ListRow(i) => model.list.visible && i < model.list.rows.len(),
            FrameId::Hist => model.hist.is_some(),
            FrameId::Detail => detail_visible,
            FrameId::GilBox | FrameId::SpinnerBox => model.price.is_some(),
            FrameId::Confirm => model.confirm.is_some(),
        };
        let want = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
        if let FrameId::ListRow(i) = tag.0 {
            let is_cursor = model.list.rows.get(i).map(|r| r.is_cursor).unwrap_or(false);
            let want_bg = if is_cursor {
                theme::CURSOR_BG
            } else {
                Color::NONE
            };
            if bg.0 != want_bg {
                bg.0 = want_bg;
            }
        }
    }

    // Texts.
    for (tag, mut text, mut color, mut bg) in text_q.iter_mut() {
        let (want, want_color, want_bg) = text_value(tag.0, &model, &detail_name, &detail_rows);
        if **text != want {
            **text = want;
        }
        if color.0 != want_color {
            color.0 = want_color;
        }
        if bg.0 != want_bg {
            bg.0 = want_bg;
        }
    }

    // Icons.
    for (tag, mut node, mut image) in icon_q.iter_mut() {
        let item_no = match tag.0 {
            IconId::ListRow(i) => model
                .list
                .rows
                .get(i)
                .map(|r| r.item_no)
                .filter(|&n| n != 0),
            IconId::Detail => model.detail,
            IconId::HistItem => model.hist.as_ref().and_then(|h| h.header_item),
        };
        let handle = item_no.and_then(|no| icon_cache.ensure(no, &dat_root, &mut images));
        match handle {
            Some(h) => {
                image.image = h;
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
            }
            None => {
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
        }
    }
}

fn text_value(
    role: Role,
    model: &FrameModel,
    detail_name: &str,
    detail_rows: &[String],
) -> (String, Color, Color) {
    let plain = |s: String, c: Color| (s, c, Color::NONE);
    match role {
        Role::DockTitle => plain(
            model
                .dock
                .as_ref()
                .map(|d| d.title.clone())
                .unwrap_or_default(),
            theme::TITLE,
        ),
        Role::DockRow(i) => {
            let Some(dock) = model.dock.as_ref() else {
                return plain(String::new(), theme::TEXT);
            };
            let Some(label) = dock.rows.get(i) else {
                return plain(String::new(), theme::TEXT);
            };
            let is_cursor = dock.cursor == Some(i);
            let is_tab = dock.active_tab == Some(i);
            let color = if is_cursor || is_tab {
                theme::CURSOR
            } else {
                theme::TEXT
            };
            plain(format!("{}{label}", cursor_prefix(is_cursor)), color)
        }
        Role::ListRowText(i) => match model.list.rows.get(i) {
            Some(r) => {
                let color = if r.muted {
                    theme::MUTED
                } else if r.is_cursor {
                    theme::CURSOR
                } else {
                    theme::TEXT
                };
                plain(format!("{}{}", cursor_prefix(r.is_cursor), r.text), color)
            }
            None => plain(String::new(), theme::TEXT),
        },
        Role::ListRowCount(i) => match model.list.rows.get(i) {
            Some(r) => plain(r.count.clone(), theme::TEXT),
            None => plain(String::new(), theme::TEXT),
        },
        Role::ListEmpty => plain(model.list.empty_text.clone(), theme::TEXT),
        Role::HistTitle => plain(PRICE_HISTORY_TITLE.to_string(), theme::TITLE),
        Role::HistHeader => plain(
            model
                .hist
                .as_ref()
                .map(|h| h.header.clone())
                .unwrap_or_default(),
            theme::TITLE,
        ),
        Role::HistRow(i) => plain(
            model
                .hist
                .as_ref()
                .and_then(|h| h.rows.get(i))
                .map(history_row_text)
                .unwrap_or_default(),
            theme::TEXT,
        ),
        Role::HistRowPrice(i) => plain(
            model
                .hist
                .as_ref()
                .and_then(|h| h.rows.get(i))
                .map(|s| format!("{} G", group_digits(s.price)))
                .unwrap_or_default(),
            theme::TEXT,
        ),
        Role::DetailName => plain(detail_name.to_string(), theme::TITLE),
        Role::DetailRow(i) => plain(detail_rows.get(i).cloned().unwrap_or_default(), theme::TEXT),
        Role::GilLine => match model.price.as_ref() {
            Some(p) => plain(
                format!("Current Gil  {} G", group_digits(p.gil)),
                theme::TEXT,
            ),
            None => plain(String::new(), theme::TEXT),
        },
        Role::Spinner(slot) => match model.price.as_ref() {
            Some(p) => digit_spinner::slot_style(&p.spinner, slot, SpinnerUnit::Gil),
            None => (String::new(), theme::TEXT, Color::NONE),
        },
        Role::ConfirmText => plain(
            model
                .confirm
                .as_ref()
                .map(|c| c.text.clone())
                .unwrap_or_default(),
            theme::TEXT,
        ),
        Role::ConfirmChoice(choice) => match model.confirm.as_ref() {
            Some(c) => {
                let is_cursor = c.cursor == choice;
                let label = match choice {
                    YesNo::Yes => CONFIRM_YES,
                    YesNo::No => CONFIRM_NO,
                };
                let color = if is_cursor {
                    theme::CURSOR
                } else {
                    theme::TEXT
                };
                plain(format!("{}{label}", cursor_prefix(is_cursor)), color)
            }
            None => plain(String::new(), theme::TEXT),
        },
    }
}

// ---------------------------------------------------------------------------
// Mouse
// ---------------------------------------------------------------------------

/// Hover moves the cursor of whichever region the row belongs to.
pub fn auction_mouse_hover_system(
    mut screen: ResMut<AuctionScreenState>,
    inv: Res<AuctionSellInventory>,
    state: Res<SceneState>,
    rows: Query<(&Hovered, &AhHotRow), Changed<Hovered>>,
) {
    if !screen.active {
        return;
    }
    for (interaction, row) in &rows {
        if !interaction.0 {
            continue;
        }
        apply_hover(
            &mut screen,
            &state.snapshot,
            inv.rows.len(),
            row.region,
            row.slot,
        );
    }
}

/// Drop the cursor on `(region, slot)` — shared by mouse hover and the
/// click-to-activate path (a click selects the row it lands on, then confirms).
pub fn apply_hover(
    screen: &mut AuctionScreenState,
    snap: &SceneSnapshot,
    inv_len: usize,
    region: AhRegion,
    slot: usize,
) {
    let row = AhHotRow { region, slot };
    let row = &row;
    let catalog_len = filtered_listings(screen, snap).len();
    let sort_len = screen
        .browse_leaf_node()
        .map(|l| sort_rows(l.id).len())
        .unwrap_or(0);
    match (row.region, &mut screen.screen) {
        (AhRegion::Dock, AhScreen::Root { cursor }) if row.slot < 3 => *cursor = row.slot,
        (AhRegion::Dock, AhScreen::Category { path, cursor }) => {
            if menu_children(path).is_some_and(|c| row.slot < c.len()) {
                *cursor = row.slot;
            }
        }
        (
            AhRegion::Dock,
            AhScreen::Catalog {
                overlay: Some(CatalogOverlay::ItemMenu { cursor }),
                ..
            },
        ) if row.slot < 3 => *cursor = row.slot,
        (
            AhRegion::Dock,
            AhScreen::Catalog {
                overlay: Some(CatalogOverlay::Sort { cursor }),
                ..
            },
        ) if row.slot < sort_len => *cursor = row.slot,
        (AhRegion::Dock, AhScreen::SellStack { cursor, .. }) if row.slot < 2 => *cursor = row.slot,
        (
            AhRegion::List,
            AhScreen::Catalog {
                cursor,
                overlay: None,
            },
        ) => {
            let start = viewport_start(*cursor, catalog_len, CATALOG_ROWS);
            if start + row.slot < catalog_len {
                *cursor = start + row.slot;
            }
        }
        (AhRegion::List, AhScreen::SellList { cursor }) => {
            let start = viewport_start(*cursor, inv_len, CATALOG_ROWS);
            if start + row.slot < inv_len {
                *cursor = start + row.slot;
            }
        }
        (AhRegion::List, AhScreen::SalesStatus { cursor }) if row.slot < SALES_SLOTS => {
            *cursor = row.slot
        }
        (AhRegion::Confirm, AhScreen::FeeConfirm { cursor, .. })
        | (AhRegion::Confirm, AhScreen::PlaceConfirm { cursor, .. })
        | (AhRegion::Confirm, AhScreen::CancelConfirm { cursor, .. }) => {
            *cursor = if row.slot == 0 { YesNo::Yes } else { YesNo::No };
        }
        _ => {}
    }
}

/// Click activates a row (the client confirms it exactly like Enter).
pub fn auction_mouse_click_system(
    screen: Res<AuctionScreenState>,
    rows: Query<&AhHotRow>,
    mut out: MessageWriter<AuctionRowActivated>,
    mut presses: MessageReader<PointerPress>,
    parents: Query<&ChildOf>,
) {
    let pressed_rows = super::primary_pressed_rows(&mut presses, &rows, &parents);
    if !screen.active {
        return;
    }
    for row in pressed_rows {
        {
            out.write(AuctionRowActivated {
                region: row.region,
                slot: row.slot,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_snapshot::AhListingView;

    #[test]
    fn leaf_ids_match_the_lsb_category_doc() {
        // vendor/server/docs/Auction Categories.txt: every retail-
        // reachable category id, exactly once, nothing else.
        let mut ids = leaf_ids();
        ids.sort_unstable();
        let mut expect: Vec<u8> = (1..=65).collect();
        // 27 = UNUSED, and 13's siblings are all present; remove non-leaves.
        expect.retain(|id| *id != 27);
        assert_eq!(ids, expect);

        // Spot-check the ids called out in the doc.
        fn id_of(label: &str) -> u8 {
            fn walk(nodes: &'static [AhNode], label: &str) -> Option<u8> {
                for n in nodes {
                    match n {
                        AhNode::Leaf(l) if l.label == label => return Some(l.id),
                        AhNode::Leaf(_) => {}
                        AhNode::Menu { children, .. } => {
                            if let Some(hit) = walk(children, label) {
                                return Some(hit);
                            }
                        }
                    }
                }
                None
            }
            walk(AH_CATEGORY_ROOT, label).unwrap_or_else(|| panic!("leaf {label:?} missing"))
        }
        assert_eq!(id_of("Hand-to-Hand"), 1);
        assert_eq!(id_of("Ammunition"), 15);
        assert_eq!(id_of("Fishing Gear"), 47);
        assert_eq!(id_of("Pet Items"), 48);
        assert_eq!(id_of("Grips"), 62);
        assert_eq!(id_of("Meat & Eggs"), 52);
        assert_eq!(id_of("Drinks"), 58);
        assert_eq!(id_of("Fish"), 51);
        assert_eq!(id_of("Ingredients"), 59);
        assert_eq!(id_of("Geomancy"), 45);
        assert_eq!(id_of("Alchemy 2"), 63);
        assert_eq!(id_of("Misc. 3"), 65);
        assert_eq!(id_of("Medicines"), 33);
        assert_eq!(id_of("Furnishings"), 34);
        assert_eq!(id_of("Crystals"), 35);
    }

    #[test]
    fn top_level_and_armor_follow_retail_order() {
        let top: Vec<&str> = AH_CATEGORY_ROOT.iter().map(|n| n.label()).collect();
        assert_eq!(
            top,
            vec![
                "Weapons",
                "Armor",
                "Scrolls",
                "Medicines",
                "Furnishings",
                "Materials",
                "Food",
                "Crystals",
                "Others"
            ]
        );
        // Retail equip-slot order: Neck 3rd, Waist 6th (NOT LSB id order).
        let armor: Vec<&str> = ARMOR.iter().map(|n| n.label()).collect();
        assert_eq!(armor[2], "Neck");
        assert_eq!(armor[5], "Waist");
        assert_eq!(armor.len(), 11);
        // Weapons: 15 entries ending in the Ammo&Misc. submenu.
        assert_eq!(WEAPONS.len(), 15);
        assert!(matches!(WEAPONS[14], AhNode::Menu { .. }));
    }

    #[test]
    fn retail_strings_exact() {
        assert_eq!(HELP_BID, "View all merchandise up for auction.");
        assert_eq!(HELP_SELL, "Place unwanted items on auction.");
        assert_eq!(
            HELP_SALES_STATUS,
            "Check your items currently placed on auction."
        );
        assert_eq!(
            HELP_HISTORY_TABLE,
            "Sales data for the last ten transactions of selected merchandise."
        );
        assert_eq!(
            HELP_ITEM_PRICE_HISTORY,
            "View recent sales data for this merchandise."
        );
        assert_eq!(HELP_ITEM_BID, "Place a bid on this merchandise.");
        assert_eq!(HELP_ITEM_SORT, "Rearrange the order of listed items.");
        assert_eq!(SALES_STATUS_EMPTY, "You have no items up for auction.");
        assert_eq!(CHAT_MERCH_PLACED, "Merchandise placed on auction.");
        assert_eq!(
            CHAT_PARTIAL_STACK,
            "You can only place a single item or a set of 12 such items on auction."
        );
        assert_eq!(
            chat_bid_failed("cat baghnakhs", 490),
            "You were unable to buy the cat baghnakhs for 490 gil."
        );
        assert_eq!(
            fee_confirm_text(Some(12), 9),
            "The total transaction fee for a set of 12 items is 9 gil."
        );
        assert_eq!(
            fee_confirm_text(None, 17),
            "The total transaction fee for this item is 17 gil."
        );
        assert_eq!(
            place_confirm_text("bird eggs", 1180),
            "Place bird eggs up on auction for 1,180 gil?"
        );
        assert_eq!(category_menu_help("Weapons"), "View weapons on auction.");
        assert_eq!(HELP_H2H, "Knuckles, claws, and other hand-to-hand weapons.");
    }

    #[test]
    fn sell_dates_format_yy_m_d() {
        assert_eq!(format_sell_date(0), "70/1/1");
        // 2001-09-09 01:46:40 UTC.
        assert_eq!(format_sell_date(1_000_000_000), "01/9/9");
        // 2026-08-04 12:00:00 UTC.
        assert_eq!(format_sell_date(1_785_844_800), "26/8/4");
    }

    #[test]
    fn history_rows_ellipsize_names() {
        let sale = AhSaleView {
            price: 4_000,
            sell_date: 1_785_844_800,
            seller: "Firedragon".into(),
            buyer: "Bob".into(),
        };
        let row = history_row_text(&sale);
        assert!(row.starts_with("26/8/4"));
        assert!(row.contains("Firedra\u{2026}"));
        assert!(row.contains("Bob"));
    }

    #[test]
    fn wrap_and_viewport_math() {
        assert_eq!(wrap_up(0, 3), 2);
        assert_eq!(wrap_down(2, 3), 0);
        assert_eq!(wrap_down(0, 0), 0);
        assert_eq!(viewport_start(0, 30, 10), 0);
        assert_eq!(viewport_start(29, 30, 10), 20);
        assert_eq!(viewport_start(15, 30, 10), 10);
        assert_eq!(viewport_start(3, 8, 10), 0);
    }

    #[test]
    fn stock_counts_bracket_only_when_listed() {
        let none = AhListingView {
            item_id: 1,
            singles_for_sale: 0,
            stacks_for_sale: None,
        };
        assert_eq!(stock_count(&none), None);
        let some = AhListingView {
            item_id: 1,
            singles_for_sale: 12,
            stacks_for_sale: Some(2),
        };
        assert_eq!(stock_count(&some), Some(14));
        assert!(!default_stack_form(&some));
        let stacks_only = AhListingView {
            item_id: 1,
            singles_for_sale: 0,
            stacks_for_sale: Some(3),
        };
        assert!(default_stack_form(&stacks_only));
    }

    #[test]
    fn sort_menu_splits_equipment_from_goods() {
        let weapons = sort_rows(1);
        let labels: Vec<&str> = weapons.iter().map(|(l, _, _)| *l).collect();
        assert_eq!(
            labels,
            vec![
                SORT_RESET,
                SORT_BY_DAMAGE,
                SORT_BY_DELAY,
                SORT_BY_LEVEL,
                SORT_JOB,
                SORT_RACE
            ]
        );
        assert_eq!(
            weapons[1].1,
            SortChoice::Param(ffxi_proto::search::SORT_DAMAGE_DESC)
        );
        assert_eq!(
            weapons[2].1,
            SortChoice::Param(ffxi_proto::search::SORT_DELAY_DESC)
        );
        assert_eq!(
            weapons[3].1,
            SortChoice::Param(ffxi_proto::search::SORT_LEVEL_DESC)
        );

        let food = sort_rows(51);
        let labels: Vec<&str> = food.iter().map(|(l, _, _)| *l).collect();
        assert_eq!(labels, vec![SORT_RESET, SORT_ALPHABETICAL]);
        assert_eq!(food[1].1, SortChoice::Param(ffxi_proto::search::SORT_NAME));
        assert!(is_equipment_category(16));
        assert!(is_equipment_category(62));
        assert!(!is_equipment_category(33));
    }

    #[test]
    fn dock_view_tracks_screens() {
        let mut s = AuctionScreenState::default();
        s.open();
        let dock = dock_view(&s).expect("root dock");
        assert_eq!(dock.title, AH_ROOT_TITLE);
        assert_eq!(dock.rows, vec![ROOT_BID, ROOT_SELL, ROOT_SALES_STATUS]);
        assert_eq!(dock.cursor, Some(0));

        s.screen = AhScreen::Category {
            path: vec![0],
            cursor: 2,
        };
        let dock = dock_view(&s).expect("weapons dock");
        assert_eq!(dock.title, "Weapons");
        assert_eq!(dock.rows.len(), 15);
        assert_eq!(dock.rows[0], "Hand-to-Hand");

        // Catalog: sibling tab stack with the active leaf highlighted.
        s.browse_path = vec![0];
        s.browse_leaf = 0;
        s.screen = AhScreen::Catalog {
            cursor: 0,
            overlay: None,
        };
        let dock = dock_view(&s).expect("tab stack");
        assert_eq!(dock.cursor, None);
        assert_eq!(dock.active_tab, Some(0));
        assert_eq!(dock.rows.len(), 15);
    }

    #[test]
    fn help_bar_reports_busy_spinner() {
        let mut s = AuctionScreenState::default();
        s.open();
        let mut snap = SceneSnapshot::default();
        snap.auction.busy = Some(kuluu_snapshot::AuctionBusy::Downloading);
        let inv = AuctionSellInventory::default();
        let (title, hint) = help_bar_content(&s, &snap, &inv);
        assert_eq!(title, "Auction");
        assert!(hint.contains(BUSY_DOWNLOADING));
        snap.auction.busy = Some(kuluu_snapshot::AuctionBusy::PlacingBid);
        let (_, hint) = help_bar_content(&s, &snap, &inv);
        assert!(hint.contains(BUSY_PLACING_BID));
    }

    #[test]
    fn sellable_rows_skip_gil_locked_and_noauction() {
        use kuluu_snapshot::{ContainerView, InventoryItem};
        let inv = ffxi_proto::map::container::LOC_INVENTORY;
        let item = |index: u8, item_no: u16, locked: bool| InventoryItem {
            container: inv,
            index,
            item_no,
            quantity: 1,
            locked,
            unselectable: false,
            charges_remaining: None,
            next_use_vana_ts: None,
            use_delay_end_vana_ts: None,
            ready: None,

            extdata: None,
        };
        let snap = SceneSnapshot {
            containers: vec![ContainerView {
                id: inv,
                capacity: 30,
                items: vec![
                    item(0, ffxi_proto::map::GIL_ITEM_NO, false),
                    item(1, 4570, false),
                    item(2, 4571, true),
                    // Item 1 carries @FLAG_NOAUCTION in item_basic.sql.
                    item(3, 1, false),
                ],
            }],
            ..Default::default()
        };
        let rows = build_sell_rows(&snap);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].item_no, 4570);
    }
}
