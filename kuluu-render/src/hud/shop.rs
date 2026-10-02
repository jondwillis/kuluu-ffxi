//! The NPC shop window.
//!
//! An NPC shop is not an event: LSB's vendors run `showText` then
//! `sendMenu(xi.menuType.SHOP)` (vendor/server/scripts/globals/shop.lua
//! `xi.shop.general`), and the server refuses SHOP_BUY outright while the
//! character is InEvent (vendor/server/src/map/packets/c2s/0x083_shop_buy.cpp
//! validate). There is likewise no close-shop packet — c2s 0x082 SHOP_REQ is
//! deprecated and GM-only (research/XiPackets client 0x0082) — so the window
//! opens on s2c 0x03E and closes entirely on the client's say-so.
//!
//! Retail splits the window across four menu primitives — `shop`, `shopmain`,
//! `shopbuy`, `shopsell` (research/XIClient
//! src/XIClient/source/UI/Windows/PrimMng.cpp) — which is the Buy/Sell picker
//! parented over a ware list and a per-side confirm box modelled here.

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use kuluu_snapshot::{SceneSnapshot, ShopItem, ShopSale};

use crate::hud::bazaar_view::{group_digits, item_name};
use crate::hud::delivery::current_gil;
use crate::hud::digit_spinner::{self, DigitSpinner, SpinnerSlot, SpinnerUnit};
use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::item_ui::{self, framed_box, set_icon, text_font, theme, transparent_placeholder};
use crate::hud::list_view::{self, row_color, ListViewport, LIST_ROWS, ROW_ICON_PX};
use crate::snapshot::SceneState;

/// `ShopNo` in c2s 0x083 SHOP_BUY. The retail client never sets it and the
/// server never reads it (research/XiPackets client 0x0083 ShopNo).
pub const SHOP_NO: u16 = 0;

/// How far the player may drift from the vendor before the window closes. LSB
/// accepts a Trigger only within 6 yalms of the NPC
/// (vendor/server/src/map/packets/c2s/0x01a_action.cpp Trigger), so past that
/// the conversation that opened the shop could not have started either.
pub const VENDOR_RANGE_YALMS: f32 = 6.0;

/// Which side of the shop the ware list is showing (retail's `shopbuy` /
/// `shopsell`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShopMode {
    #[default]
    Buy,
    Sell,
}

impl ShopMode {
    pub fn label(self) -> &'static str {
        match self {
            ShopMode::Buy => "Buy",
            ShopMode::Sell => "Sell",
        }
    }

    pub const ROWS: [ShopMode; 2] = [ShopMode::Buy, ShopMode::Sell];
}

/// What the cursor is driving. Cancel unwinds exactly one level per press, and
/// cancelling out of [`ShopFocus::Menu`] is what closes the shop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShopFocus {
    /// The Buy/Sell picker (`shopmain`).
    #[default]
    Menu,
    /// The ware list (buy) or the sellable-inventory list (sell).
    List,
    /// Sizing a stack before committing to it.
    Quantity,
    /// The priced yes/no step.
    Confirm,
}

/// One row of whichever list the current [`ShopMode`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShopRow {
    /// Shop table index (buy) or LOC_INVENTORY slot (sell).
    pub index: u8,
    pub item_no: u16,
    /// Unit price for a buy row; 0 on a sell row, whose price the server only
    /// reveals in the 0x03D appraisal
    /// (vendor/server/src/map/packets/s2c/0x03d_shop_sell.cpp).
    pub price: u32,
    /// Held quantity for a sell row; 0 on a buy row, where stock is unlimited.
    pub quantity: u32,
}

/// A price the server has quoted for one of the player's stacks (s2c 0x03D,
/// vendor/server/src/map/packets/s2c/0x03d_shop_sell.cpp).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Appraisal {
    pub item_index: u8,
    pub item_no: u16,
    pub unit_price: u32,
}

/// Which of the window's three cursors a pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShopRegion {
    /// The Buy/Sell picker.
    Menu,
    /// A drawn row of the ware list, numbered within the page.
    List,
    /// The confirm box: slot 0 is Yes, slot 1 is No.
    Confirm,
}

/// A row the pointer can reach.
#[derive(Component, Clone, Copy)]
pub struct ShopHotRow {
    pub region: ShopRegion,
    pub slot: usize,
}

/// A row the player clicked or tapped. The client answers it exactly as if
/// they had moved the cursor there and pressed Enter.
#[derive(Message, Debug, Clone, Copy)]
pub struct ShopRowActivated {
    pub region: ShopRegion,
    pub slot: usize,
}

/// Cursor + in-flight transaction state. The stock itself lives in the
/// snapshot; this resource is the part the player is moving around.
#[derive(Resource, Debug, Clone)]
pub struct ShopScreenState {
    pub mode: ShopMode,
    pub focus: ShopFocus,
    /// Cursor into the Buy/Sell picker.
    pub menu_cursor: usize,
    /// Cursor into the list the active mode shows.
    pub cursor: usize,
    /// First row the list is drawing.
    pub viewport: ListViewport,
    /// Every price the server has quoted since the list opened. Retail leaves
    /// an appraisal on its row for as long as the list is up, so walking the
    /// stock prices it as you go.
    pub appraised: Vec<Appraisal>,
    pub quantity: Option<DigitSpinner>,
    /// A buy the player has sized, awaiting the yes/no. Sell confirmations are
    /// driven by `snapshot.shop.pending_sale` instead, because their price
    /// comes from the server.
    pub pending_buy: Option<PendingBuy>,
    pub pending_sell: Option<(u8, u16, u32)>,

    /// The client has closed the window but the snapshot still carries the
    /// stock: `CloseShop` has to reach the session and the cleared state has to
    /// come back, which is several frames. Without this latch the sync system
    /// sees a live shop with the cursor back in the world and immediately
    /// reopens it, so the window and its help bar strobe until the round trip
    /// lands. Cleared when the snapshot's shop finally goes away.
    pub dismissed: bool,

    /// Which row the confirm box's Yes/No cursor sits on. Retail's comparable
    /// transaction confirm - the AH fee dialog - opens on Yes
    /// (.agents/skills/retail-observe/references/auction-house.md, sell flow).
    pub confirm_yes: bool,
}

impl Default for ShopScreenState {
    fn default() -> Self {
        Self {
            mode: ShopMode::default(),
            focus: ShopFocus::default(),
            menu_cursor: 0,
            cursor: 0,
            viewport: ListViewport::default(),
            appraised: Vec::new(),
            quantity: None,
            pending_buy: None,
            pending_sell: None,
            dismissed: false,
            confirm_yes: CONFIRM_DEFAULT_YES,
        }
    }
}

/// The confirm box opens on Yes. Retail's nearest observed equivalent, the
/// Auction House fee dialog, does the same; the No default this repo uses
/// elsewhere is for destructive prompts (Drop, Log Out), not for a transaction
/// the player walked three menus to reach.
pub const CONFIRM_DEFAULT_YES: bool = true;

/// A purchase the player has sized but not yet confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingBuy {
    pub shop_index: u8,
    pub item_no: u16,
    pub quantity: u32,
    pub total_gil: u32,
}

impl ShopScreenState {
    /// Fresh state for a newly opened shop: the Buy/Sell picker has the cursor,
    /// with the wares already listed behind it.
    pub fn opened() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Give up the window without forgetting that we did. Everything else
    /// resets; [`Self::dismissed`] stays set until the snapshot catches up.
    pub fn dismiss(&mut self) {
        self.reset();
        self.dismissed = true;
    }

    /// Up/Down: one row, clamped at both ends (retail's item lists do not
    /// wrap). Leaving the drawn page scrolls it by a single row, leaving the
    /// cursor on the edge row.
    pub fn move_cursor(&mut self, dy: i32, len: usize) {
        if len == 0 {
            return;
        }
        self.set_cursor(
            (self.cursor as i32 + dy).clamp(0, len as i32 - 1) as usize,
            len,
        );
    }

    /// Put the cursor on a row the player named outright (a click, a wheel
    /// notch), dragging the page after it.
    pub fn set_cursor(&mut self, cursor: usize, len: usize) {
        if len == 0 {
            return;
        }
        self.cursor = cursor.min(len - 1);
        self.viewport.follow(self.cursor, len);
    }

    /// Left/Right: cursor and page both step a whole page, each clamped on its
    /// own, so the cursor keeps its offset within the page except where the
    /// clamp bites. Observed sequence (cursor/page): 10/1 -> 20/11 -> 30/21.
    pub fn page(&mut self, dx: i32, len: usize) {
        if len == 0 {
            return;
        }
        let forward = dx > 0;
        self.cursor = list_view::page_cursor(self.cursor, len, forward);
        self.viewport.page(forward, len);
        self.viewport.follow(self.cursor, len);
    }

    pub fn record_appraisal(&mut self, sale: &ShopSale) {
        let quote = Appraisal {
            item_index: sale.item_index,
            item_no: sale.item_no,
            unit_price: sale.unit_price,
        };
        match self
            .appraised
            .iter_mut()
            .find(|a| a.item_index == quote.item_index && a.item_no == quote.item_no)
        {
            Some(existing) => *existing = quote,
            None => self.appraised.push(quote),
        }
    }

    /// What the server said this row is worth per unit, if it has said.
    pub fn appraised_price(&self, row: &ShopRow) -> Option<u32> {
        self.appraised
            .iter()
            .find(|a| a.item_index == row.index && a.item_no == row.item_no)
            .map(|a| a.unit_price)
    }

    pub fn move_menu_cursor(&mut self, dy: i32) {
        let n = ShopMode::ROWS.len() as i32;
        self.menu_cursor = (self.menu_cursor as i32 + dy).rem_euclid(n) as usize;
    }

    pub fn menu_mode(&self) -> ShopMode {
        ShopMode::ROWS[self.menu_cursor.min(ShopMode::ROWS.len() - 1)]
    }

    /// Enter the list for the highlighted picker row. Quotes belong to the list
    /// that collected them, so opening one starts with none.
    pub fn enter_list(&mut self) {
        self.mode = self.menu_mode();
        self.focus = ShopFocus::List;
        self.cursor = 0;
        self.viewport = ListViewport::default();
        self.appraised.clear();
        self.quantity = None;
        self.pending_buy = None;
    }

    /// Open the priced yes/no step with the cursor on its default row.
    pub fn enter_confirm(&mut self) {
        self.focus = ShopFocus::Confirm;
        self.confirm_yes = CONFIRM_DEFAULT_YES;
    }

    /// Keep the cursor inside a list the server or the player's bag shrank.
    pub fn clamp(&mut self, len: usize) {
        self.cursor = self.cursor.min(len.saturating_sub(1));
        self.viewport.follow(self.cursor, len);
        if len == 0 && matches!(self.focus, ShopFocus::List | ShopFocus::Quantity) {
            self.focus = ShopFocus::Menu;
            self.quantity = None;
            self.pending_buy = None;
        }
    }

    pub fn stage_buy(&mut self, row: &ShopRow, quantity: u32) -> PendingBuy {
        let buy = PendingBuy {
            shop_index: row.index,
            item_no: row.item_no,
            quantity,
            total_gil: row.price.saturating_mul(quantity),
        };
        self.quantity = None;
        self.pending_buy = Some(buy);
        self.enter_confirm();
        buy
    }
}

/// The rows the active mode lists. Buy rows come straight from the shop's
/// stock; sell rows are LOC_INVENTORY minus gil, locked slots, and anything
/// the server would refuse to appraise (`@FLAG_NOSALE`).
pub fn rows_for(mode: ShopMode, snap: &SceneSnapshot) -> Vec<ShopRow> {
    match mode {
        ShopMode::Buy => snap
            .shop
            .as_ref()
            .map(|shop| shop.items.iter().map(buy_row).collect())
            .unwrap_or_default(),
        ShopMode::Sell => snap
            .inventory_main()
            .iter()
            .filter(|it| {
                it.index != 0
                    && it.item_no != 0
                    && it.item_no != ffxi_proto::map::GIL_ITEM_NO
                    && !it.locked
                    && ffxi_vocab::item_flags::sellable(it.item_no)
            })
            .map(|it| ShopRow {
                index: it.index,
                item_no: it.item_no,
                price: 0,
                quantity: it.quantity,
            })
            .collect(),
    }
}

fn buy_row(item: &ShopItem) -> ShopRow {
    ShopRow {
        index: item.shop_index,
        item_no: item.item_no,
        price: item.price,
        quantity: 0,
    }
}

/// Retail's confirmation wording, e.g.
/// `Purchase 12 crystals for 1,200 gil?`.
pub fn purchase_prompt(item_name: &str, quantity: u32, total_gil: u32) -> String {
    format!(
        "Purchase {quantity} {item_name} for {} gil?",
        group_digits(total_gil)
    )
}

pub fn sale_prompt(item_name: &str, quantity: u32, total_gil: u32) -> String {
    format!(
        "Sell {quantity} {item_name} for {} gil?",
        group_digits(total_gil)
    )
}

/// Help-bar title and hint for the current focus.
pub fn help_bar_content(state: &ShopScreenState, _snap: &SceneSnapshot) -> (String, String) {
    let title = match state.focus {
        ShopFocus::Menu => SHOP_TITLE,
        _ => state.mode.label(),
    };
    let hint = match state.focus {
        ShopFocus::Menu => match state.menu_mode() {
            ShopMode::Buy => HELP_BUY,
            ShopMode::Sell => HELP_SELL,
        },
        ShopFocus::List => match state.mode {
            ShopMode::Buy => HELP_SELECT_WARE,
            ShopMode::Sell => HELP_SELECT_OWN_ITEM,
        },
        ShopFocus::Quantity => HELP_QUANTITY,
        // Retail dims or blanks the help bar while a modal Yes/No dialog has
        // focus (.agents/skills/retail-observe/references/auction-house.md);
        // the question itself is on the confirm box, not up here.
        ShopFocus::Confirm => "",
    };
    (title.to_string(), hint.to_string())
}

/// The priced yes/no line for whichever side is confirming, or `None` while a
/// sell appraisal is still in flight.
pub fn confirm_line(state: &ShopScreenState, snap: &SceneSnapshot) -> Option<String> {
    match state.mode {
        ShopMode::Buy => {
            let buy = state.pending_buy?;
            Some(purchase_prompt(
                &item_name(buy.item_no, None),
                buy.quantity,
                buy.total_gil,
            ))
        }
        ShopMode::Sell => {
            let sale = confirmed_sale(state, snap)?;
            Some(sale_prompt(
                &item_name(sale.item_no, None),
                sale.count,
                sale.total_gil(),
            ))
        }
    }
}

pub fn confirmed_sale<'a>(
    state: &ShopScreenState,
    snap: &'a SceneSnapshot,
) -> Option<&'a kuluu_snapshot::ShopSale> {
    let sale = snap.shop.as_ref()?.pending_sale.as_ref()?;
    (state.pending_sell == Some((sale.item_index, sale.item_no, sale.count))).then_some(sale)
}

const SHOP_TITLE: &str = "Shop";
const HELP_BUY: &str = "Purchase merchandise.";
const HELP_SELL: &str = "Sell your items.";
const HELP_SELECT_WARE: &str = "Select an item to purchase.";
const HELP_SELECT_OWN_ITEM: &str = "Select an item to sell.";
const HELP_QUANTITY: &str = "Select a quantity.";
const APPRAISING: &str = "Appraising...";

#[derive(Component)]
pub struct ShopPanel;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShopTextRole {
    RowName(usize),
    RowPrice(usize),
    /// Marks exactly one row: the one the cursor is on while the picker has
    /// focus, and the side being browsed once the list takes over.
    MenuRow(usize),
    MenuTitle,
    /// Retail swaps the Current Gil box for the quantity picker while a stack
    /// is being sized; the priced step labels what the figure under it is, so
    /// it is not misread as the player's purse. The picker under the label
    /// carries the cap, so the label does not repeat it.
    GilLabel,
    /// A sell total exists only once the server answers; until then the label
    /// above already promises a figure for this sale, so the purse under it
    /// would read as that figure.
    GilValue,
    ConfirmChoice(bool),
    QuantityColumn(SpinnerSlot),
    /// The per-unit price is not repeated here: it is on the row, where retail
    /// leaves it once the appraisal lands.
    DetailName,
    DetailBody,
}

#[derive(Component, Clone, Copy)]
pub(crate) struct ShopText(ShopTextRole);

#[derive(Component, Clone, Copy)]
pub(crate) struct ShopRowIcon(usize);

#[derive(Component)]
pub(crate) struct ShopDetailIcon;

/// The framed box the ware list lives in; the wheel scrolls the list under the
/// pointer, so it has to know where that box is.
#[derive(Component)]
pub(crate) struct ShopListBox;

#[derive(Component)]
pub(crate) struct ShopScrollTrack;

#[derive(Component)]
pub(crate) struct ShopScrollThumb;

const PANEL_WIDTH_PX: f32 = 360.0;
const PRICE_COL_PX: f32 = 110.0;
const GIL_BOX_PX: f32 = 116.0;
const DOCK_WIDTH_PX: f32 = 96.0;
/// Detail body lines, matching the rows `item_detail::detail_rows` composes.
const DETAIL_LINES: usize = 4;

pub(crate) fn spawn_shop_panel(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = transparent_placeholder(&mut images);

    commands
        .spawn((
            crate::components::InGameEntity,
            ShopPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(20.0),
                left: Val::Percent(28.0),
                row_gap: Val::Px(6.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexStart,
                display: Display::None,
                ..default()
            },
            GlobalZIndex(item_ui::WINDOW_Z),
        ))
        .with_children(|root| {
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::FlexStart,
                column_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|top| {
                let (mut n, bg, bd) = framed_box();
                n.width = Val::Px(PANEL_WIDTH_PX);
                n.flex_direction = FlexDirection::Row;
                n.column_gap = Val::Px(4.0);
                top.spawn((n, bg, bd, ShopListBox, RelativeCursorPosition::default()))
                    .with_children(|p| {
                        p.spawn(Node {
                            flex_direction: FlexDirection::Column,
                            flex_grow: 1.0,
                            min_width: Val::Px(0.0),
                            ..default()
                        })
                        .with_children(|list| {
                            for i in 0..LIST_ROWS {
                                list.spawn((
                                    ShopHotRow {
                                        region: ShopRegion::List,
                                        slot: i,
                                    },
                                    Button,
                                    list_view::row_node(),
                                ))
                                .with_children(|row| {
                                    row.spawn((
                                        ShopRowIcon(i),
                                        Node {
                                            width: Val::Px(ROW_ICON_PX),
                                            height: Val::Px(ROW_ICON_PX),
                                            flex_shrink: 0.0,
                                            ..default()
                                        },
                                        ImageNode::new(placeholder.clone()),
                                        BackgroundColor(theme::CELL_BG),
                                    ));
                                    row.spawn(list_view::row_label_clip()).with_children(|col| {
                                        col.spawn((
                                            ShopText(ShopTextRole::RowName(i)),
                                            Text::new(""),
                                            text_font(13.0),
                                            TextColor(theme::TEXT),
                                            list_view::row_label_layout(),
                                        ));
                                    });
                                    row.spawn((
                                        ShopText(ShopTextRole::RowPrice(i)),
                                        Text::new(""),
                                        text_font(13.0),
                                        TextColor(theme::TEXT),
                                        TextLayout {
                                            justify: Justify::Right,
                                            linebreak: LineBreak::NoWrap,
                                        },
                                        Node {
                                            width: Val::Px(PRICE_COL_PX),
                                            flex_shrink: 0.0,
                                            ..default()
                                        },
                                    ));
                                });
                            }
                        });
                        list_view::spawn_scrollbar(p, ShopScrollTrack, ShopScrollThumb);
                    });

                let (mut n, bg, bd) = framed_box();
                n.width = Val::Px(DOCK_WIDTH_PX);
                top.spawn((n, bg, bd)).with_children(|dock| {
                    dock.spawn((
                        ShopText(ShopTextRole::MenuTitle),
                        Text::new(SHOP_TITLE),
                        text_font(13.0),
                        TextColor(theme::TITLE),
                    ));
                    for i in 0..ShopMode::ROWS.len() {
                        dock.spawn((
                            ShopText(ShopTextRole::MenuRow(i)),
                            Button,
                            ShopHotRow {
                                region: ShopRegion::Menu,
                                slot: i,
                            },
                            Text::new(""),
                            text_font(13.0),
                            TextColor(theme::TEXT),
                        ));
                    }
                });
            });

            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::FlexStart,
                column_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|under| {
                let (mut n, bg, bd) = framed_box();
                n.width = Val::Px(GIL_BOX_PX);
                under.spawn((n, bg, bd)).with_children(|g| {
                    g.spawn((
                        ShopText(ShopTextRole::GilLabel),
                        Text::new(""),
                        text_font(12.0),
                        TextColor(theme::MUTED),
                    ));
                    g.spawn((
                        ShopText(ShopTextRole::GilValue),
                        Text::new(""),
                        text_font(13.0),
                        TextColor(theme::TEXT),
                    ));
                    digit_spinner::spawn_row(g, digit_spinner::slots(), |s| {
                        ShopText(ShopTextRole::QuantityColumn(s))
                    });
                    for (slot, yes) in [true, false].into_iter().enumerate() {
                        g.spawn((
                            ShopText(ShopTextRole::ConfirmChoice(yes)),
                            Button,
                            ShopHotRow {
                                region: ShopRegion::Confirm,
                                slot,
                            },
                            Text::new(""),
                            text_font(13.0),
                            TextColor(theme::TEXT),
                        ));
                    }
                });

                let (mut n, bg, bd) = framed_box();
                n.width = Val::Px(PANEL_WIDTH_PX - GIL_BOX_PX - 6.0);
                n.flex_direction = FlexDirection::Row;
                n.column_gap = Val::Px(6.0);
                under.spawn((n, bg, bd)).with_children(|d| {
                    d.spawn((
                        ShopDetailIcon,
                        Node {
                            width: Val::Px(32.0),
                            height: Val::Px(32.0),
                            ..default()
                        },
                        ImageNode::new(placeholder.clone()),
                    ));
                    d.spawn(Node {
                        flex_direction: FlexDirection::Column,
                        ..default()
                    })
                    .with_children(|t| {
                        t.spawn((
                            ShopText(ShopTextRole::DetailName),
                            Text::new(""),
                            text_font(13.0),
                            TextColor(theme::TITLE),
                        ));
                        t.spawn((
                            ShopText(ShopTextRole::DetailBody),
                            Text::new(""),
                            text_font(12.0),
                            TextColor(theme::TEXT),
                        ));
                    });
                });
            });
        });
}

#[allow(clippy::type_complexity)]
pub(crate) fn update_shop_panel_system(
    state: Res<SceneState>,
    screen: Res<ShopScreenState>,
    dat_root: Res<ItemDatRoot>,
    mut icon_cache: ResMut<ItemIconCache>,
    mut images: ResMut<Assets<Image>>,
    mut panel_q: Query<
        &mut Node,
        (
            With<ShopPanel>,
            Without<ShopText>,
            Without<ShopRowIcon>,
            Without<ShopDetailIcon>,
        ),
    >,
    mut text_q: Query<
        (
            &ShopText,
            &mut Text,
            &mut TextColor,
            Option<&mut BackgroundColor>,
        ),
        Without<ShopRowIcon>,
    >,
    mut icon_q: Query<(&ShopRowIcon, &mut ImageNode), Without<ShopDetailIcon>>,
    mut detail_icon_q: Query<&mut ImageNode, With<ShopDetailIcon>>,
) {
    let Ok(mut panel) = panel_q.single_mut() else {
        return;
    };
    let snap: &SceneSnapshot = &state.snapshot;
    if snap.shop.is_none() || screen.dismissed {
        if panel.display != Display::None {
            panel.display = Display::None;
        }
        return;
    }
    if panel.display != Display::Flex {
        panel.display = Display::Flex;
    }

    let rows = rows_for(screen.mode, snap);
    let gil = current_gil(snap);
    let start = screen
        .viewport
        .start
        .min(ListViewport::max_start(rows.len()));
    let focused = rows.get(screen.cursor).copied();
    let list_active = !matches!(screen.focus, ShopFocus::Menu);

    let (detail_name, detail_rows) = item_ui::focus_detail(
        focused.map(|r| r.item_no),
        None,
        snap,
        &dat_root,
        &mut icon_cache,
    );

    for (tag, mut text, mut color, background) in text_q.iter_mut() {
        let (want, want_color) = match tag.0 {
            ShopTextRole::RowName(i) => match rows.get(start + i) {
                Some(row) => (
                    row_label(row, screen.mode),
                    row_color(
                        list_active && start + i == screen.cursor,
                        affordable(row, gil),
                    ),
                ),
                None => (String::new(), theme::TEXT),
            },
            ShopTextRole::RowPrice(i) => match rows.get(start + i) {
                Some(row) => (
                    row_price(row, &screen),
                    row_color(
                        list_active && start + i == screen.cursor,
                        affordable(row, gil),
                    ),
                ),
                None => (String::new(), theme::TEXT),
            },
            ShopTextRole::MenuTitle => (SHOP_TITLE.to_string(), theme::TITLE),
            ShopTextRole::MenuRow(i) => {
                let mode = ShopMode::ROWS[i];
                let marked = if list_active {
                    screen.mode == mode
                } else {
                    screen.menu_cursor == i
                };
                (
                    format!("{}{}", item_ui::cursor_prefix(marked), mode.label()),
                    match (marked, list_active) {
                        (true, false) => theme::CURSOR,
                        (true, true) => theme::TITLE,
                        (false, _) => theme::TEXT,
                    },
                )
            }
            ShopTextRole::GilLabel => match (screen.quantity.as_ref(), screen.focus) {
                (Some(_), _) => ("Quantity".to_string(), theme::TITLE),
                (None, ShopFocus::Confirm) => (confirm_total_label(screen.mode), theme::MUTED),
                (None, _) => ("Current Gil".to_string(), theme::MUTED),
            },
            ShopTextRole::GilValue => match running_total(&screen, snap, focused.as_ref()) {
                Some(total) => (
                    format!("{} G?", group_digits(total)),
                    if screen.mode == ShopMode::Sell || total <= gil {
                        theme::CURSOR
                    } else {
                        theme::DANGER
                    },
                ),
                None if !matches!(screen.focus, ShopFocus::List | ShopFocus::Menu) => {
                    (APPRAISING.to_string(), theme::MUTED)
                }
                None => (format!("{} G", group_digits(gil)), theme::TEXT),
            },
            // The yes/no the priced question is asking for. Retail puts this
            // box lower-left, under the figure
            // (.agents/skills/retail-observe/references/auction-house.md).
            ShopTextRole::ConfirmChoice(yes) => match screen.focus {
                ShopFocus::Confirm => (
                    format!(
                        "{}{}",
                        item_ui::cursor_prefix(screen.confirm_yes == yes),
                        if yes { "Yes" } else { "No" }
                    ),
                    row_color(screen.confirm_yes == yes, true),
                ),
                _ => (String::new(), theme::TEXT),
            },
            ShopTextRole::QuantityColumn(slot) => {
                let (label, tint, bg) = screen
                    .quantity
                    .as_ref()
                    .map(|spinner| digit_spinner::slot_style(spinner, slot, SpinnerUnit::Count))
                    .unwrap_or((String::new(), theme::TEXT, Color::NONE));
                if let Some(mut background) = background {
                    background.0 = bg;
                }
                (label, tint)
            }
            ShopTextRole::DetailName => match (screen.focus, focused) {
                (ShopFocus::Confirm, _) => match confirm_line(&screen, snap) {
                    Some(line) => (line, theme::CURSOR),
                    None => (APPRAISING.to_string(), theme::MUTED),
                },
                (_, Some(_)) => (detail_name.clone(), theme::TITLE),
                (_, None) => (empty_list_prompt(screen.mode).to_string(), theme::MUTED),
            },
            ShopTextRole::DetailBody => {
                let body = detail_rows
                    .iter()
                    .take(DETAIL_LINES)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                (body, theme::TEXT)
            }
        };
        if **text != want {
            **text = want;
        }
        if color.0 != want_color {
            color.0 = want_color;
        }
    }

    for (icon, mut image) in icon_q.iter_mut() {
        let handle = rows
            .get(start + icon.0)
            .and_then(|r| icon_cache.ensure(r.item_no, &dat_root, &mut images));
        set_icon(&mut image, handle);
    }
    if let Ok(mut image) = detail_icon_q.single_mut() {
        let handle = focused.and_then(|r| icon_cache.ensure(r.item_no, &dat_root, &mut images));
        set_icon(&mut image, handle);
    }
}

/// Whether the window is on screen and answering.
fn shop_open(state: &SceneState, screen: &ShopScreenState) -> bool {
    state.snapshot.shop.is_some() && !screen.dismissed
}

/// The ware list's scrollbar, in its own system so its `&mut Node` queries stay
/// disjoint from the panel's.
pub(crate) fn update_shop_scrollbar(
    state: Res<SceneState>,
    screen: Res<ShopScreenState>,
    mut track_q: Query<&mut Node, (With<ShopScrollTrack>, Without<ShopScrollThumb>)>,
    mut thumb_q: Query<&mut Node, (With<ShopScrollThumb>, Without<ShopScrollTrack>)>,
) {
    if !shop_open(&state, &screen) {
        return;
    }
    let total = rows_for(screen.mode, &state.snapshot).len();
    if let (Ok(mut track), Ok(mut thumb)) = (track_q.single_mut(), thumb_q.single_mut()) {
        list_view::apply_scrollbar(&mut track, &mut thumb, screen.viewport.start, total);
    }
}

/// Keep every quote the server sends while the list is up, so each row can go on
/// showing its price the way retail's does.
pub(crate) fn record_shop_appraisals(state: Res<SceneState>, mut screen: ResMut<ShopScreenState>) {
    let Some(sale) = state
        .snapshot
        .shop
        .as_ref()
        .and_then(|shop| shop.pending_sale.as_ref())
    else {
        return;
    };
    let known = screen.appraised.iter().any(|a| {
        (a.item_index, a.item_no, a.unit_price) == (sale.item_index, sale.item_no, sale.unit_price)
    });
    if !known {
        screen.record_appraisal(sale);
    }
}

/// Hovering moves the cursor onto the row under the pointer, so a click lands
/// where it looks like it will. Only the region that already owns focus answers:
/// the pointer must not reach past a step that is waiting for an answer.
pub(crate) fn shop_mouse_hover_system(
    state: Res<SceneState>,
    mut screen: ResMut<ShopScreenState>,
    hot: Query<(&Interaction, &ShopHotRow), Changed<Interaction>>,
) {
    if !shop_open(&state, &screen) {
        return;
    }
    let len = rows_for(screen.mode, &state.snapshot).len();
    let start = screen.viewport.start;
    for (interaction, row) in &hot {
        if !matches!(interaction, Interaction::Hovered | Interaction::Pressed) {
            continue;
        }
        match (row.region, screen.focus) {
            (ShopRegion::Menu, ShopFocus::Menu) => {
                screen.menu_cursor = row.slot.min(ShopMode::ROWS.len() - 1);
            }
            (ShopRegion::List, ShopFocus::List) => {
                let index = start + row.slot;
                if index < len {
                    screen.set_cursor(index, len);
                }
            }
            (ShopRegion::Confirm, ShopFocus::Confirm) => {
                screen.confirm_yes = row.slot == CONFIRM_YES_SLOT;
            }
            _ => {}
        }
    }
}

/// Slot of the Yes row in the confirm box; the client spawns Yes first.
pub const CONFIRM_YES_SLOT: usize = 0;

pub(crate) fn shop_mouse_click_system(
    state: Res<SceneState>,
    screen: Res<ShopScreenState>,
    hot: Query<(&Interaction, &ShopHotRow), Changed<Interaction>>,
    mut out: MessageWriter<ShopRowActivated>,
) {
    if !shop_open(&state, &screen) {
        return;
    }
    for (interaction, row) in &hot {
        if *interaction == Interaction::Pressed {
            out.write(ShopRowActivated {
                region: row.region,
                slot: row.slot,
            });
        }
    }
}

/// The wheel walks the ware list under the pointer a row at a time. It moves the
/// cursor rather than the page on its own: the page follows the cursor every
/// frame, so a page scrolled away from it would snap straight back. Wheel away
/// from the player walks toward the top of the list, where the row index is
/// lowest.
pub(crate) fn shop_wheel_scroll_system(
    mut wheel: MessageReader<MouseWheel>,
    state: Res<SceneState>,
    mut screen: ResMut<ShopScreenState>,
    list_q: Query<&RelativeCursorPosition, With<ShopListBox>>,
    mut accum: Local<f32>,
) {
    let delta: f32 = wheel.read().map(|ev| ev.y).sum();
    if delta == 0.0 || !shop_open(&state, &screen) {
        return;
    }
    if !matches!(screen.focus, ShopFocus::List) {
        return;
    }
    if !list_q.iter().any(|rel| rel.cursor_over()) {
        return;
    }
    let len = rows_for(screen.mode, &state.snapshot).len();
    let (cursor, frac) = list_view::apply_wheel_delta(screen.cursor, *accum, -delta, len);
    *accum = frac;
    screen.set_cursor(cursor, len);
}

/// The per-unit price the server quoted for the focused sell row, once its
/// appraisal has come back. `None` on the buy side (the list already prices
/// every row) and while a sell quote is still in flight.
pub fn sell_unit_price(screen: &ShopScreenState, row: Option<&ShopRow>) -> Option<u32> {
    if screen.mode != ShopMode::Sell {
        return None;
    }
    screen.appraised_price(row?)
}

/// The gil figure the box is asking about: the priced confirm step, or the
/// amount a stack being sized is worth so far.
fn running_total(
    screen: &ShopScreenState,
    snap: &SceneSnapshot,
    row: Option<&ShopRow>,
) -> Option<u32> {
    match screen.focus {
        ShopFocus::Confirm => match screen.mode {
            ShopMode::Buy => screen.pending_buy.map(|b| b.total_gil),
            ShopMode::Sell => confirmed_sale(screen, snap).map(|sale| sale.total_gil()),
        },
        ShopFocus::Quantity => {
            let picked = screen.quantity.as_ref()?.value;
            let unit = match screen.mode {
                ShopMode::Buy => row?.price,
                ShopMode::Sell => sell_unit_price(screen, row)?,
            };
            Some(unit.saturating_mul(picked))
        }
        _ => None,
    }
}

/// The confirm box's two rows, cursor on the chosen one.
pub fn confirm_choices(yes: bool) -> String {
    format!(
        "{}Yes\n{}No",
        item_ui::cursor_prefix(yes),
        item_ui::cursor_prefix(!yes)
    )
}

/// What the gil box is showing while a transaction is priced.
fn confirm_total_label(mode: ShopMode) -> String {
    match mode {
        ShopMode::Buy => "Total Cost".to_string(),
        ShopMode::Sell => "You Receive".to_string(),
    }
}

fn empty_list_prompt(mode: ShopMode) -> &'static str {
    match mode {
        ShopMode::Buy => "Nothing for sale.",
        ShopMode::Sell => "Nothing you can sell.",
    }
}

/// The sell side shows how many the player is holding, as the bag list does.
fn row_label(row: &ShopRow, mode: ShopMode) -> String {
    let name = item_name(row.item_no, None);
    match mode {
        ShopMode::Buy => name,
        ShopMode::Sell if row.quantity > 1 => format!("{name} ({})", row.quantity),
        ShopMode::Sell => name,
    }
}

/// A sell row starts blank — the server only reveals a price in the 0x03D
/// appraisal a confirm asks for (vendor/server/src/map/packets/s2c/0x03d_shop_sell.cpp) —
/// and keeps the quote once it lands.
fn row_price(row: &ShopRow, screen: &ShopScreenState) -> String {
    match screen.mode {
        ShopMode::Buy => format!("{} G", group_digits(row.price)),
        ShopMode::Sell => screen
            .appraised_price(row)
            .map(|unit| format!("{} G", group_digits(unit)))
            .unwrap_or_default(),
    }
}

fn affordable(row: &ShopRow, gil: u32) -> bool {
    row.price <= gil
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_snapshot::{ContainerView, InventoryItem, ShopSale, ShopState};

    fn shop_with(items: &[(u8, u16, u32)]) -> ShopState {
        ShopState {
            items: items
                .iter()
                .map(|&(shop_index, item_no, price)| ShopItem {
                    price,
                    item_no,
                    shop_index,
                    skill: 0,
                    guild_info: 0,
                })
                .collect(),
            opened: true,
            ..Default::default()
        }
    }

    fn inv_item(index: u8, item_no: u16, quantity: u32, locked: bool) -> InventoryItem {
        InventoryItem {
            container: ffxi_proto::map::container::LOC_INVENTORY,
            index,
            item_no,
            quantity,
            locked,
            unselectable: false,
            charges_remaining: None,
            next_use_vana_ts: None,
            use_delay_end_vana_ts: None,
            ready: None,
        }
    }

    #[test]
    fn confirmation_renders_only_the_selected_answer_yellow() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<ItemDatRoot>()
            .init_resource::<ItemIconCache>()
            .insert_resource(SceneState {
                snapshot: SceneSnapshot {
                    shop: Some(shop_with(&[])),
                    ..Default::default()
                },
                ..Default::default()
            })
            .insert_resource(ShopScreenState {
                focus: ShopFocus::Confirm,
                ..Default::default()
            })
            .add_systems(Startup, spawn_shop_panel)
            .add_systems(Update, update_shop_panel_system);
        for selected in [true, false] {
            app.world_mut()
                .resource_mut::<ShopScreenState>()
                .confirm_yes = selected;
            app.update();
            let mut query = app.world_mut().query::<(&ShopText, &TextColor)>();
            let answers: Vec<_> = query
                .iter(app.world())
                .filter_map(|(tag, tint)| {
                    if let ShopTextRole::ConfirmChoice(yes) = tag.0 {
                        Some((yes, tint.0))
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(answers.len(), 2);
            for (yes, color) in answers {
                assert_eq!(
                    color,
                    if yes == selected {
                        theme::CURSOR
                    } else {
                        theme::TEXT
                    }
                );
            }
        }
    }

    #[test]
    fn confirmation_rejects_a_quote_for_another_stack_or_quantity() {
        let state = ShopScreenState {
            mode: ShopMode::Sell,
            pending_sell: Some((2, 4096, 10)),
            ..Default::default()
        };
        let mut snap = SceneSnapshot {
            shop: Some(ShopState {
                pending_sale: Some(ShopSale {
                    item_index: 1,
                    item_no: 4096,
                    count: 12,
                    unit_price: 20,
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(confirmed_sale(&state, &snap).is_none());
        let sale = snap.shop.as_mut().unwrap().pending_sale.as_mut().unwrap();
        sale.item_index = 2;
        sale.count = 1;
        assert!(confirmed_sale(&state, &snap).is_none());
        snap.shop
            .as_mut()
            .unwrap()
            .pending_sale
            .as_mut()
            .unwrap()
            .count = 10;
        assert!(confirmed_sale(&state, &snap).is_some());
    }

    #[test]
    fn buy_rows_come_from_the_shop_stock() {
        let snap = SceneSnapshot {
            shop: Some(shop_with(&[(0, 4096, 100), (1, 4097, 250)])),
            ..Default::default()
        };
        let rows = rows_for(ShopMode::Buy, &snap);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[1].index, rows[1].item_no, rows[1].price),
            (1, 4097, 250)
        );
        assert_eq!(rows[0].quantity, 0, "shop stock is unlimited");
    }

    #[test]
    fn sell_rows_drop_gil_locked_and_nosale_items() {
        // item 7 (gold_bed) carries @FLAG_NOSALE in item_basic.sql; item 2
        // (simple_bed) does not.
        let snap = SceneSnapshot {
            shop: Some(shop_with(&[])),
            containers: vec![ContainerView {
                id: ffxi_proto::map::container::LOC_INVENTORY,
                capacity: 30,
                items: vec![
                    inv_item(0, ffxi_proto::map::GIL_ITEM_NO, 5000, false),
                    inv_item(1, 2, 1, false),
                    inv_item(2, 7, 1, false),
                    inv_item(3, 2, 3, true),
                ],
            }],
            ..Default::default()
        };
        let rows = rows_for(ShopMode::Sell, &snap);
        assert_eq!(
            rows.len(),
            1,
            "gil, NoSale and locked slots are not offered"
        );
        assert_eq!((rows[0].index, rows[0].item_no), (1, 2));
    }

    #[test]
    fn cancel_unwinds_one_level_at_a_time() {
        let mut s = ShopScreenState::opened();
        assert_eq!(s.focus, ShopFocus::Menu, "the Buy/Sell picker opens first");
        s.enter_list();
        assert_eq!(s.focus, ShopFocus::List);
        assert_eq!(s.mode, ShopMode::Buy);
    }

    #[test]
    fn picker_cursor_selects_the_sell_side() {
        let mut s = ShopScreenState::opened();
        s.move_menu_cursor(1);
        assert_eq!(s.menu_mode(), ShopMode::Sell);
        s.move_menu_cursor(1);
        assert_eq!(s.menu_mode(), ShopMode::Buy, "the picker wraps");
        s.move_menu_cursor(-1);
        assert_eq!(s.menu_mode(), ShopMode::Sell);
        s.enter_list();
        assert_eq!(s.mode, ShopMode::Sell);
    }

    #[test]
    fn staging_a_buy_prices_it_and_moves_to_the_confirm_step() {
        let mut s = ShopScreenState {
            focus: ShopFocus::Quantity,
            quantity: Some(DigitSpinner::item(12)),
            ..Default::default()
        };
        let row = ShopRow {
            index: 3,
            item_no: 4096,
            price: 250,
            quantity: 0,
        };
        let buy = s.stage_buy(&row, 4);
        assert_eq!((buy.shop_index, buy.quantity, buy.total_gil), (3, 4, 1000));
        assert!(s.quantity.is_none(), "the picker closes behind the prompt");
        assert_eq!(s.focus, ShopFocus::Confirm);
    }

    /// The observed retail sequence, 0-based (cursor/page start):
    /// 10/1 -> 20/11 -> 30/21 -> 40/31 -> 50/41 -> 58/49 (clamped) -> Left -> 48/39
    /// (.agents/skills/retail-observe/references/2026-09-11-items-window.md).
    #[test]
    fn left_right_page_the_list_the_way_retail_does() {
        const LEN: usize = 59;
        let mut s = ShopScreenState {
            focus: ShopFocus::List,
            cursor: 10,
            viewport: ListViewport { start: 1 },
            ..Default::default()
        };
        for expected in [(20, 11), (30, 21), (40, 31), (50, 41), (58, 49)] {
            s.page(1, LEN);
            assert_eq!((s.cursor, s.viewport.start), expected);
        }
        s.page(1, LEN);
        assert_eq!(
            (s.cursor, s.viewport.start),
            (58, 49),
            "both ends clamp instead of wrapping"
        );
        s.page(-1, LEN);
        assert_eq!((s.cursor, s.viewport.start), (48, 39));
    }

    #[test]
    fn up_down_clamp_and_scroll_the_page_one_row() {
        const LEN: usize = 30;
        let mut s = ShopScreenState {
            focus: ShopFocus::List,
            ..Default::default()
        };
        s.move_cursor(-1, LEN);
        assert_eq!((s.cursor, s.viewport.start), (0, 0), "the top row holds");

        for _ in 0..LIST_ROWS {
            s.move_cursor(1, LEN);
        }
        assert_eq!(
            (s.cursor, s.viewport.start),
            (LIST_ROWS, 1),
            "leaving the page scrolls it by one row, cursor on the edge"
        );

        for _ in 0..LEN {
            s.move_cursor(1, LEN);
        }
        assert_eq!((s.cursor, s.viewport.start), (LEN - 1, LEN - LIST_ROWS));
    }

    #[test]
    fn a_short_list_never_scrolls() {
        let mut s = ShopScreenState {
            focus: ShopFocus::List,
            ..Default::default()
        };
        s.page(1, 4);
        assert_eq!((s.cursor, s.viewport.start), (3, 0));
    }

    #[test]
    fn a_sell_quote_prices_the_focused_row_only() {
        let row = ShopRow {
            index: 8,
            item_no: 16992,
            price: 0,
            quantity: 12,
        };
        let snap = SceneSnapshot {
            shop: Some(ShopState {
                pending_sale: Some(ShopSale {
                    item_index: 8,
                    item_no: 16992,
                    unit_price: 10,
                    count: 1,
                }),
                ..shop_with(&[])
            }),
            ..Default::default()
        };
        let mut sell = ShopScreenState {
            mode: ShopMode::Sell,
            focus: ShopFocus::Quantity,
            quantity: Some(DigitSpinner::item(12)),
            ..Default::default()
        };
        assert_eq!(
            sell_unit_price(&sell, Some(&row)),
            None,
            "nothing is priced before the server answers"
        );
        sell.record_appraisal(quote(&snap));
        assert_eq!(sell_unit_price(&sell, Some(&row)), Some(10));

        let other = ShopRow { index: 9, ..row };
        assert_eq!(
            sell_unit_price(&sell, Some(&other)),
            None,
            "a quote for a different slot does not price this row"
        );

        let buy = ShopScreenState {
            mode: ShopMode::Buy,
            ..sell.clone()
        };
        assert_eq!(
            sell_unit_price(&buy, Some(&row)),
            None,
            "the buy side prices off the listed price, not the quote"
        );
    }

    #[test]
    fn sizing_a_stack_shows_what_it_is_worth_so_far() {
        let row = ShopRow {
            index: 8,
            item_no: 16992,
            price: 0,
            quantity: 12,
        };
        let snap = SceneSnapshot {
            shop: Some(ShopState {
                pending_sale: Some(ShopSale {
                    item_index: 8,
                    item_no: 16992,
                    unit_price: 10,
                    count: 1,
                }),
                ..shop_with(&[])
            }),
            ..Default::default()
        };
        let mut spin = DigitSpinner::item(12);
        spin.set_all();
        let mut s = ShopScreenState {
            mode: ShopMode::Sell,
            focus: ShopFocus::Quantity,
            quantity: Some(spin),
            ..Default::default()
        };
        s.record_appraisal(quote(&snap));
        assert_eq!(running_total(&s, &snap, Some(&row)), Some(120));
    }

    fn quote(snap: &SceneSnapshot) -> &ShopSale {
        snap.shop
            .as_ref()
            .and_then(|shop| shop.pending_sale.as_ref())
            .expect("the fixture carries a quote")
    }

    /// Retail affixes a quote to the right of its row and leaves it there for as
    /// long as the list is up, so the player can price a bagful by walking it.
    #[test]
    fn an_appraisal_stays_on_its_row_until_the_list_closes() {
        let crystals = ShopRow {
            index: 8,
            item_no: 4096,
            price: 0,
            quantity: 12,
        };
        let ore = ShopRow {
            index: 9,
            item_no: 640,
            price: 0,
            quantity: 3,
        };
        let mut s = ShopScreenState::opened();
        s.mode = ShopMode::Sell;
        assert_eq!(row_price(&crystals, &s), "");

        s.record_appraisal(&ShopSale {
            item_index: crystals.index,
            item_no: crystals.item_no,
            unit_price: 1_250,
            count: 1,
        });
        assert_eq!(row_price(&crystals, &s), "1,250 G");
        assert_eq!(row_price(&ore, &s), "", "only the appraised row is priced");

        s.record_appraisal(&ShopSale {
            item_index: ore.index,
            item_no: ore.item_no,
            unit_price: 40,
            count: 1,
        });
        assert_eq!(
            row_price(&crystals, &s),
            "1,250 G",
            "a second quote does not displace the first"
        );
        assert_eq!(row_price(&ore, &s), "40 G");

        s.record_appraisal(&ShopSale {
            item_index: crystals.index,
            item_no: crystals.item_no,
            unit_price: 1_300,
            count: 12,
        });
        assert_eq!(s.appraised.len(), 2);
        assert_eq!(
            row_price(&crystals, &s),
            "1,300 G",
            "re-pricing the same stack replaces its quote"
        );

        s.move_menu_cursor(1);
        s.enter_list();
        assert_eq!(s.mode, ShopMode::Sell);
        assert_eq!(
            row_price(&crystals, &s),
            "",
            "quotes belong to the list that collected them"
        );
    }

    #[test]
    fn an_emptied_list_drops_the_cursor_back_to_the_picker() {
        let mut s = ShopScreenState {
            focus: ShopFocus::List,
            cursor: 3,
            ..Default::default()
        };
        s.clamp(0);
        assert_eq!(s.focus, ShopFocus::Menu);
        assert_eq!(s.cursor, 0);
    }

    #[test]
    fn confirm_lines_read_from_the_side_that_owns_the_price() {
        let buy_state = ShopScreenState {
            mode: ShopMode::Buy,
            focus: ShopFocus::Confirm,
            pending_buy: Some(PendingBuy {
                shop_index: 0,
                item_no: 4096,
                quantity: 3,
                total_gil: 1_234,
            }),
            ..Default::default()
        };
        let snap = SceneSnapshot::default();
        let line = confirm_line(&buy_state, &snap).expect("buy prompt");
        assert!(line.starts_with("Purchase 3 "), "{line}");
        assert!(line.ends_with("for 1,234 gil?"), "{line}");

        let sell_snap = SceneSnapshot {
            shop: Some(ShopState {
                pending_sale: Some(ShopSale {
                    item_index: 5,
                    item_no: 4096,
                    unit_price: 20,
                    count: 6,
                }),
                ..shop_with(&[])
            }),
            ..Default::default()
        };
        let sell_state = ShopScreenState {
            mode: ShopMode::Sell,
            focus: ShopFocus::Confirm,
            pending_sell: Some((5, 4096, 6)),
            ..Default::default()
        };
        let line = confirm_line(&sell_state, &sell_snap).expect("sell prompt");
        assert!(line.starts_with("Sell 6 "), "{line}");
        assert!(line.ends_with("for 120 gil?"), "{line}");
    }

    /// The question needs a visible answer. Retail's comparable transaction
    /// confirm (the AH fee dialog) opens on Yes
    /// (.agents/skills/retail-observe/references/auction-house.md).
    #[test]
    fn the_confirm_box_offers_yes_and_no_with_the_cursor_on_yes() {
        let mut s = ShopScreenState::opened();
        let row = ShopRow {
            index: 0,
            item_no: 4612,
            price: 23_400,
            quantity: 0,
        };
        s.stage_buy(&row, 1);
        assert_eq!(s.focus, ShopFocus::Confirm);
        assert!(s.confirm_yes);
        assert_eq!(confirm_choices(s.confirm_yes), "> Yes\n  No");

        s.confirm_yes = false;
        assert_eq!(confirm_choices(s.confirm_yes), "  Yes\n> No");
    }

    #[test]
    fn re_entering_the_confirm_step_re_arms_the_default() {
        let mut s = ShopScreenState::opened();
        s.confirm_yes = false;
        s.enter_confirm();
        assert!(
            s.confirm_yes,
            "a declined prompt does not poison the next one"
        );
    }

    /// Retail dims or blanks the help bar while a modal Yes/No owns focus; the
    /// question belongs on the confirm box, not in two places at once.
    #[test]
    fn the_help_bar_hint_clears_behind_the_confirm_box() {
        let snap = SceneSnapshot::default();
        let mut s = ShopScreenState::opened();
        s.enter_list();
        assert_eq!(help_bar_content(&s, &snap).1, HELP_SELECT_WARE);
        s.enter_confirm();
        let (title, hint) = help_bar_content(&s, &snap);
        assert_eq!(title, "Buy", "the bar still says where you are");
        assert!(hint.is_empty());
    }

    /// The gil box's label and its figure have to agree: while a sell quote is
    /// outstanding the label promises this sale's total, so the purse must not
    /// sit under it.
    #[test]
    fn an_unanswered_sale_shows_no_figure_rather_than_the_purse() {
        let snap = SceneSnapshot {
            shop: Some(shop_with(&[])),
            containers: vec![ContainerView {
                id: ffxi_proto::map::container::LOC_INVENTORY,
                capacity: 30,
                items: vec![inv_item(0, ffxi_proto::map::GIL_ITEM_NO, 921, false)],
            }],
            ..Default::default()
        };
        let mut s = ShopScreenState::opened();
        s.mode = ShopMode::Sell;

        s.focus = ShopFocus::List;
        assert_eq!(
            running_total(&s, &snap, None),
            None,
            "the list shows the purse"
        );

        s.enter_confirm();
        assert_eq!(
            running_total(&s, &snap, None),
            None,
            "no quote yet, so no total to show"
        );
    }

    #[test]
    fn a_sell_confirm_with_no_appraisal_yet_says_so() {
        let state = ShopScreenState {
            mode: ShopMode::Sell,
            focus: ShopFocus::Confirm,
            ..Default::default()
        };
        let snap = SceneSnapshot {
            shop: Some(shop_with(&[])),
            ..Default::default()
        };
        assert!(confirm_line(&state, &snap).is_none());
    }

    #[test]
    fn help_bar_tracks_the_focus() {
        let snap = SceneSnapshot::default();
        let mut s = ShopScreenState::opened();
        assert_eq!(
            help_bar_content(&s, &snap),
            (SHOP_TITLE.into(), HELP_BUY.into())
        );
        s.move_menu_cursor(1);
        assert_eq!(help_bar_content(&s, &snap).1, HELP_SELL);
        s.enter_list();
        assert_eq!(
            help_bar_content(&s, &snap),
            ("Sell".into(), HELP_SELECT_OWN_ITEM.into())
        );
    }
}
