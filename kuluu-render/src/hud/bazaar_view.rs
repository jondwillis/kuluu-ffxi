//! The wares list of a browsed bazaar (View Wares in the /check window).
//!
//! Retail draws a fixed-height list of `<icon> <name> ....... <price> G` rows
//! over a Current Gil box and the focused item's description panel; the gil box
//! is replaced by the `All ◄ n/max ►` quantity picker while a purchase is being
//! sized, and the purchase itself is confirmed from a chat prompt; observed on
//! the horizonxi-2023 client.
//!
//! Rows come straight from the server's s2c 0x105 packets, one per priced
//! seller slot; the seller's own bazaar is the authority, so this window only
//! renders what arrived and sends purchases by slot index.

use bevy::prelude::*;
use kuluu_snapshot::{BazaarEntry, SceneSnapshot};

use crate::hud::delivery::current_gil;
use crate::hud::digit_spinner::{self, DigitSpinner, SpinnerSlot, SpinnerUnit};
use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::item_detail;
use crate::hud::item_ui::{framed_box, set_icon, text_font, theme, transparent_placeholder};
use crate::hud::list_view::{self, row_color, ListViewport, LIST_ROWS, ROW_ICON_PX};
use crate::snapshot::SceneState;

/// LSB's purchase validator caps a single buy at 99
/// (vendor/server/src/map/packets/c2s/0x106_bazaar_buy.cpp validate).
pub const MAX_BUY_QUANTITY: u32 = 99;

/// Cursor + pending-purchase state for the wares window. The row list itself
/// lives in the snapshot.
#[derive(Resource, Debug, Clone, Default)]
pub struct BazaarScreenState {
    pub cursor: usize,
    pub viewport: ListViewport,
    /// Active quantity picker for the focused row, once confirmed into. In our own
    /// price menu the same slot holds the price spinner being typed.
    pub quantity: Option<DigitSpinner>,
    /// Sized purchase awaiting the retail "Purchase N x for Y gil?" answer.
    pub pending: Option<PendingBuy>,
    /// Our own stall's `Set Prices` menu, open instead of a browsed bazaar (the
    /// same surface wears both, like retail's).
    pub prices: Option<BazaarPriceMode>,
}

/// One offered slot of our own price menu. `slot` is the LOC_INVENTORY index c2s
/// 0x10A reprices by; `price` is what we have asked for it this session (LSB's
/// ITEM_ATTR answer folds back onto the item as its lock flag, not as a number).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BazaarPriceRow {
    pub slot: u8,
    pub item_no: u16,
    pub price: u32,
}

/// The state of the `Set Prices` menu: the offered inventory slots and which row's
/// price is being typed (its spinner lives in [`BazaarScreenState::quantity`).
#[derive(Debug, Clone, Default)]
pub struct BazaarPriceMode {
    pub rows: Vec<BazaarPriceRow>,
    pub edit_row: Option<usize>,
}

/// The ceiling of one price cell, shared with the wire validator's range.
pub const PRICE_CAP: u32 = ffxi_proto::decode::BAZAAR_PRICE_MAX;

/// A purchase the player has sized but not yet confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingBuy {
    pub index: u8,
    pub item_no: u16,
    pub quantity: u32,
    pub total_gil: u32,
}

impl BazaarScreenState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Raise our own price menu over the inventory. Retail lists the sellable
    /// LOC_INVENTORY slots: not gil, and not locked, because LSB's 0x10A handler
    /// refuses currency and busy/reserved items outright
    /// (vendor/server/src/map/packets/c2s/0x10a_bazaar_itemset.cpp). Prices typed
    /// earlier in this session survive a rebuild of the list.
    pub fn open_prices(&mut self, snap: &SceneSnapshot) {
        let previous = self
            .prices
            .as_ref()
            .map(|mode| {
                mode.rows
                    .iter()
                    .map(|row| (row.slot, row.price))
                    .collect::<std::collections::HashMap<_, _>>()
            })
            .unwrap_or_default();
        let rows = snap
            .inventory_main()
            .iter()
            .filter(|it| {
                it.index != 0
                    && it.item_no != 0
                    && it.item_no != ffxi_proto::map::GIL_ITEM_NO
                    && !it.locked
            })
            .map(|it| BazaarPriceRow {
                slot: it.index,
                item_no: it.item_no,
                price: previous.get(&it.index).copied().unwrap_or(0),
            })
            .collect::<Vec<_>>();
        self.cursor = 0;
        self.viewport = ListViewport::default();
        self.quantity = None;
        self.pending = None;
        self.prices = Some(BazaarPriceMode {
            rows,
            edit_row: None,
        });
    }

    /// Keep the cursor inside a list the server may have shrunk under us.
    pub fn clamp(&mut self, len: usize) {
        self.cursor = self.cursor.min(len.saturating_sub(1));
        self.viewport.follow(self.cursor, len);
        if len == 0 {
            self.quantity = None;
            self.pending = None;
        }
    }

    pub fn move_cursor(&mut self, dy: i32, len: usize) {
        if len == 0 {
            return;
        }
        let n = len as i32;
        self.cursor = (self.cursor as i32 + dy).rem_euclid(n) as usize;
        self.viewport.follow(self.cursor, len);
    }

    /// Open the quantity picker for `entry`, or `None` for a single item (retail
    /// buys a lone item outright rather than asking for a count).
    pub fn begin_quantity(entry: &BazaarEntry) -> Option<DigitSpinner> {
        let max = entry.quantity.min(MAX_BUY_QUANTITY);
        (max > 1).then(|| DigitSpinner::item(max))
    }

    pub fn stage_purchase(&mut self, entry: &BazaarEntry, quantity: u32) -> PendingBuy {
        let buy = PendingBuy {
            index: entry.index,
            item_no: entry.item_no,
            quantity,
            total_gil: entry.total_price(quantity),
        };
        self.quantity = None;
        self.pending = Some(buy);
        buy
    }
}

/// The retail confirmation line, e.g.
/// `Purchase 5 pieces of hickory lumber for 124,995 gil?`.
pub fn purchase_prompt(item_name: &str, quantity: u32, total_gil: u32) -> String {
    format!(
        "Purchase {quantity} {item_name} for {} gil?",
        group_digits(total_gil)
    )
}

pub use ffxi_vocab::gil::group_digits;

#[derive(Component)]
pub struct BazaarPanel;

#[derive(Clone, Copy, PartialEq, Eq)]
enum BazaarRole {
    RowName(usize),
    RowPrice(usize),
    GilLabel,
    GilValue,
    /// The quantity picker that takes the Current Gil box's place while a purchase
    /// is being sized.
    Spinner(SpinnerSlot),
    /// Our own price entry, which instead takes the description panel's place
    /// (owner 2026-10-24: "the price replaces the item description while its open").
    PriceSpinner(SpinnerSlot),
    DetailName,
    DetailBody,
}

#[derive(Component, Clone, Copy)]
pub(crate) struct BazaarText(BazaarRole);

#[derive(Component, Clone, Copy)]
pub(crate) struct BazaarRowIcon(usize);

#[derive(Component)]
pub(crate) struct BazaarDetailIcon;

#[derive(Component)]
pub(crate) struct BazaarScrollTrack;

#[derive(Component)]
pub(crate) struct BazaarScrollThumb;

const PANEL_WIDTH_PX: f32 = 372.0;
const PRICE_COL_PX: f32 = 110.0;
/// Wide enough that the widest gil readout keeps to one line.
const GIL_BOX_PX: f32 = 150.0;
const LIST_GAP_PX: f32 = 4.0;
/// The bazaar law window draws a step under the standard menu scale (owner
/// 2026-10-24: "text a little big").
const ROW_TEXT_PX: f32 = 12.0;
const GIL_LABEL_PX: f32 = 11.0;
const GIL_VALUE_PX: f32 = 12.0;
const DETAIL_NAME_PX: f32 = 12.0;
const DETAIL_BODY_PX: f32 = 11.0;
const SPINNER_PX: f32 = 13.0;

pub(crate) fn spawn_bazaar_view(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = transparent_placeholder(&mut images);

    commands
        .spawn((
            crate::components::InGameEntity,
            BazaarPanel,
            // Retail runs both bazaar screens as a left-side law window, so the panel
            // stands where kuluu's Items (law) window does rather than mid-screen
            // (owner 2026-10-24).
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                left: Val::Px(8.0),
                row_gap: Val::Px(6.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexStart,
                display: Display::None,
                ..default()
            },
        ))
        .with_children(|root| {
            let (mut n, bg, bd) = framed_box();
            n.width = Val::Px(PANEL_WIDTH_PX);
            n.flex_direction = FlexDirection::Row;
            n.column_gap = Val::Px(LIST_GAP_PX);
            root.spawn((n, bg, bd)).with_children(|p| {
                p.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    flex_grow: 1.0,
                    min_width: Val::Px(0.0),
                    ..default()
                })
                .with_children(|list| {
                    for i in 0..LIST_ROWS {
                        list.spawn(list_view::row_node()).with_children(|row| {
                            row.spawn((
                                BazaarRowIcon(i),
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
                                    BazaarText(BazaarRole::RowName(i)),
                                    Text::new(""),
                                    text_font(ROW_TEXT_PX),
                                    TextColor(theme::TEXT),
                                    list_view::row_label_layout(),
                                ));
                            });
                            row.spawn((
                                BazaarText(BazaarRole::RowPrice(i)),
                                Text::new(""),
                                text_font(ROW_TEXT_PX),
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
                list_view::spawn_scrollbar(p, BazaarScrollTrack, BazaarScrollThumb);
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
                        BazaarText(BazaarRole::GilLabel),
                        Text::new(""),
                        text_font(GIL_LABEL_PX),
                        TextColor(theme::MUTED),
                    ));
                    g.spawn((
                        BazaarText(BazaarRole::GilValue),
                        Text::new(""),
                        text_font(GIL_VALUE_PX),
                        TextColor(theme::TEXT),
                    ));
                    digit_spinner::spawn_row_px(g, digit_spinner::slots(), SPINNER_PX, |s| {
                        BazaarText(BazaarRole::Spinner(s))
                    });
                });

                let (mut n, bg, bd) = framed_box();
                n.width = Val::Px(PANEL_WIDTH_PX - GIL_BOX_PX - 6.0);
                n.flex_direction = FlexDirection::Row;
                n.column_gap = Val::Px(6.0);
                under.spawn((n, bg, bd)).with_children(|d| {
                    d.spawn((
                        BazaarDetailIcon,
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
                            BazaarText(BazaarRole::DetailName),
                            Text::new(""),
                            text_font(DETAIL_NAME_PX),
                            TextColor(theme::TITLE),
                        ));
                        t.spawn((
                            BazaarText(BazaarRole::DetailBody),
                            Text::new(""),
                            text_font(DETAIL_BODY_PX),
                            TextColor(theme::TEXT),
                        ));
                    });
                    // The price entry rides in this box, over the description it
                    // replaces while a price is being typed. It wears no `/cap` readout (owner
                    // 2026-10-24); stepping and typing still stop at that ceiling.
                    digit_spinner::spawn_row_px(
                        d,
                        digit_spinner::slots_without_cap(),
                        SPINNER_PX,
                        |s| BazaarText(BazaarRole::PriceSpinner(s)),
                    );
                });
            });
        });
}

pub(crate) fn update_bazaar_view(
    state: Res<SceneState>,
    screen: Res<BazaarScreenState>,
    dat_root: Res<ItemDatRoot>,
    mut icon_cache: ResMut<ItemIconCache>,
    mut images: ResMut<Assets<Image>>,
    mut panel_q: Query<
        &mut Node,
        (
            With<BazaarPanel>,
            Without<BazaarText>,
            Without<BazaarRowIcon>,
            Without<BazaarDetailIcon>,
        ),
    >,
    mut text_q: Query<
        (
            &BazaarText,
            &mut Text,
            &mut TextColor,
            Option<&mut BackgroundColor>,
        ),
        Without<BazaarRowIcon>,
    >,
    mut icon_q: Query<(&BazaarRowIcon, &mut ImageNode), Without<BazaarDetailIcon>>,
    mut detail_icon_q: Query<(&mut Node, &mut ImageNode), With<BazaarDetailIcon>>,
) {
    let Ok(mut panel) = panel_q.single_mut() else {
        return;
    };
    let snap: &SceneSnapshot = &state.snapshot;
    let browsing = snap.bazaar.as_ref();
    if browsing.is_none() && screen.prices.is_none() {
        if panel.display != Display::None {
            panel.display = Display::None;
        }
        return;
    };
    if panel.display != Display::Flex {
        panel.display = Display::Flex;
    }

    let gil = current_gil(snap);
    // One surface wears both lists: a stall being browsed, or our own sellable
    // inventory while we price it. Both speak in (item, price cell, selectable).
    let row_at = |i: usize| -> Option<(u16, String, bool)> {
        match (&screen.prices, browsing) {
            (Some(mode), _) => mode.rows.get(i).map(|row| {
                (
                    row.item_no,
                    if row.price == 0 {
                        String::new()
                    } else {
                        format!("{} G", group_digits(row.price))
                    },
                    true,
                )
            }),
            (None, Some(view)) => view.items.get(i).map(|entry| {
                (
                    entry.item_no,
                    format!("{} G", group_digits(entry.price)),
                    entry.total_price(1) <= gil,
                )
            }),
            (None, None) => None,
        }
    };
    let row_count = shown_rows(&screen, snap);
    let start = screen
        .viewport
        .start
        .min(ListViewport::max_start(row_count));
    let focus_item = row_at(screen.cursor).map(|(item_no, _, _)| item_no);
    let table = icon_cache.table(&dat_root);
    let static_of = |item_no: u16| {
        table
            .as_ref()
            .and_then(|t| item_detail::lookup_static(t, item_no))
    };

    for (tag, mut text, mut color, background) in text_q.iter_mut() {
        let spinner_slot = match tag.0 {
            BazaarRole::Spinner(slot) | BazaarRole::PriceSpinner(slot) => Some(slot),
            _ => None,
        };
        if let (Some(mut background), Some(slot)) = (background, spinner_slot) {
            let unit = spinner_unit(&screen);
            let bg = match screen.quantity.as_ref() {
                Some(spin) => digit_spinner::slot_style(spin, slot, unit).2,
                None => Color::NONE,
            };
            if background.0 != bg {
                background.0 = bg;
            }
        }
        let (want, want_color) = match tag.0 {
            BazaarRole::RowName(i) => match row_at(start + i) {
                Some((item_no, _, enabled)) => (
                    item_name(item_no, static_of(item_no).map(|s| s.name)),
                    row_color(start + i == screen.cursor, enabled),
                ),
                None => (String::new(), theme::TEXT),
            },
            BazaarRole::RowPrice(i) => match row_at(start + i) {
                Some((_, price, enabled)) => {
                    (price, row_color(start + i == screen.cursor, enabled))
                }
                None => (String::new(), theme::TEXT),
            },
            // Retail swaps the Current Gil box for the quantity picker while a
            // purchase is being sized. Our own price menu keeps the gil readout
            // beside its spinner (owner's retail capture 2026-10-24).
            BazaarRole::GilLabel => match screen.prices.is_none() && screen.quantity.is_some() {
                true => (String::new(), theme::MUTED),
                false => ("Current Gil".to_string(), theme::MUTED),
            },
            // The gil-box picker belongs to browsing; the price picker lives in the
            // description box. Never both at once.
            BazaarRole::Spinner(slot) => {
                match (screen.prices.is_none(), screen.quantity.as_ref()) {
                    (true, Some(spin)) => {
                        let (s, c, _) = digit_spinner::slot_style(spin, slot, SpinnerUnit::Count);
                        (s, c)
                    }
                    _ => (String::new(), theme::TEXT),
                }
            }
            BazaarRole::PriceSpinner(slot) => {
                match (price_entry_open(&screen), screen.quantity.as_ref()) {
                    (true, Some(spin)) => {
                        let (s, c, _) = digit_spinner::slot_style(spin, slot, SpinnerUnit::Gil);
                        (s, c)
                    }
                    _ => (String::new(), theme::TEXT),
                }
            }
            BazaarRole::GilValue => match (screen.prices.as_ref(), screen.pending.as_ref()) {
                (Some(_), _) | (None, None) => (format!("{} G", group_digits(gil)), theme::TEXT),
                (None, Some(buy)) => (
                    format!("{} G?", group_digits(buy.total_gil)),
                    if buy.total_gil <= gil {
                        theme::CURSOR
                    } else {
                        theme::DANGER
                    },
                ),
            },
            // A price entry replaces the description while it is open; browsing rows
            // still read theirs.
            BazaarRole::DetailName if price_entry_open(&screen) => (String::new(), theme::MUTED),
            BazaarRole::DetailBody if price_entry_open(&screen) => (String::new(), theme::TEXT),
            BazaarRole::DetailName => match focus_item {
                Some(item_no) => (
                    item_name(item_no, static_of(item_no).map(|s| s.name)),
                    theme::TITLE,
                ),
                None if screen.prices.is_some() => (String::new(), theme::MUTED),
                None => ("Nothing for sale.".to_string(), theme::MUTED),
            },
            BazaarRole::DetailBody => match focus_item.and_then(static_of) {
                Some(s) => (s.description.clone(), theme::TEXT),
                None => (String::new(), theme::TEXT),
            },
        };
        if **text != want {
            **text = want;
        }
        if color.0 != want_color {
            color.0 = want_color;
        }
    }

    for (icon, mut image) in icon_q.iter_mut() {
        let handle = row_at(start + icon.0)
            .and_then(|(item_no, _, _)| icon_cache.ensure(item_no, &dat_root, &mut images));
        set_icon(&mut image, handle);
    }
    // The price entry owns the whole description box (owner 2026-10-24), so its plate goes too -
    // and with it the space the plate held.
    let pricing = price_entry_open(&screen);
    if let Ok((mut node, mut image)) = detail_icon_q.single_mut() {
        let want = if pricing {
            Display::None
        } else {
            Display::Flex
        };
        if node.display != want {
            node.display = want;
        }
        let handle = focus_item
            .filter(|_| !pricing)
            .and_then(|item_no| icon_cache.ensure(item_no, &dat_root, &mut images));
        set_icon(&mut image, handle);
    }
}

/// Whether a price entry currently owns the description box.
fn price_entry_open(screen: &BazaarScreenState) -> bool {
    screen
        .prices
        .as_ref()
        .and_then(|mode| mode.edit_row)
        .is_some()
}

/// Whether the shared surface is pricing our own stall (gil spinner) or browsing a
/// customer-side purchase (count spinner).
fn spinner_unit(screen: &BazaarScreenState) -> SpinnerUnit {
    if screen.prices.is_some() {
        SpinnerUnit::Gil
    } else {
        SpinnerUnit::Count
    }
}

/// How many rows the shared surface shows right now.
pub fn shown_rows(screen: &BazaarScreenState, snap: &SceneSnapshot) -> usize {
    match (&screen.prices, snap.bazaar.as_ref()) {
        (Some(mode), _) => mode.rows.len(),
        (None, Some(view)) => view.items.len(),
        (None, None) => 0,
    }
}

/// The wares list's scrollbar, in its own system so its `&mut Node` queries stay
/// disjoint from the panel's.
pub(crate) fn update_bazaar_scrollbar(
    state: Res<SceneState>,
    screen: Res<BazaarScreenState>,
    mut track_q: Query<&mut Node, (With<BazaarScrollTrack>, Without<BazaarScrollThumb>)>,
    mut thumb_q: Query<&mut Node, (With<BazaarScrollThumb>, Without<BazaarScrollTrack>)>,
) {
    if let (Ok(mut track), Ok(mut thumb)) = (track_q.single_mut(), thumb_q.single_mut()) {
        list_view::apply_scrollbar(
            &mut track,
            &mut thumb,
            screen.viewport.start,
            shown_rows(&screen, &state.snapshot),
            list_view::LIST_ROWS,
        );
    }
}

pub fn item_name(item_no: u16, dat_name: Option<String>) -> String {
    dat_name
        .filter(|n| !n.is_empty())
        .or_else(|| ffxi_vocab::item_names::lookup(item_no).map(str::to_string))
        .unwrap_or_else(|| format!("Item #{item_no}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(index: u8, item_no: u16, quantity: u32, price: u32, tax_rate: u16) -> BazaarEntry {
        BazaarEntry {
            index,
            item_no,
            quantity,
            price,
            tax_rate,
        }
    }

    #[test]
    fn total_price_applies_the_zone_tax() {
        // 5% tax = 500 hundredths of a percent (LSB tax/10000 basis points).
        let e = entry(1, 4096, 12, 1000, 500);
        assert_eq!(e.total_price(1), 1050);
        assert_eq!(e.total_price(3), 3150);
        // A tax-free zone charges the asking price exactly.
        assert_eq!(entry(1, 4096, 12, 1000, 0).total_price(4), 4000);
    }

    #[test]
    fn purchase_prompt_matches_the_retail_wording() {
        assert_eq!(
            purchase_prompt("pieces of hickory lumber", 5, 124_995),
            "Purchase 5 pieces of hickory lumber for 124,995 gil?"
        );
    }

    #[test]
    fn cursor_wraps_and_clamps_to_a_shrinking_list() {
        let mut s = BazaarScreenState::default();
        s.move_cursor(-1, 3);
        assert_eq!(s.cursor, 2, "up from the top wraps to the end");
        s.move_cursor(1, 3);
        assert_eq!(s.cursor, 0);
        s.cursor = 2;
        s.clamp(1);
        assert_eq!(s.cursor, 0, "a sold-out list pulls the cursor back");
    }

    #[test]
    fn an_empty_list_leaves_the_cursor_alone_and_drops_the_pending_buy() {
        let mut s = BazaarScreenState {
            cursor: 0,
            viewport: ListViewport::default(),
            quantity: Some(DigitSpinner::item(5)),
            pending: Some(PendingBuy {
                index: 1,
                item_no: 4096,
                quantity: 2,
                total_gil: 10,
            }),
            prices: None,
        };
        s.move_cursor(1, 0);
        assert_eq!(s.cursor, 0);
        s.clamp(0);
        assert!(s.quantity.is_none());
        assert!(s.pending.is_none());
    }

    #[test]
    fn quantity_picker_only_opens_for_a_real_stack() {
        assert!(BazaarScreenState::begin_quantity(&entry(1, 4096, 1, 100, 0)).is_none());
        let spin = BazaarScreenState::begin_quantity(&entry(1, 4096, 12, 100, 0)).expect("stack");
        assert_eq!(spin.cap, 12);
        assert_eq!(spin.confirm(), 1, "defaults to one like retail");
    }

    #[test]
    fn quantity_picker_respects_the_server_cap() {
        let spin = BazaarScreenState::begin_quantity(&entry(1, 4096, 250, 100, 0)).expect("stack");
        assert_eq!(spin.cap, MAX_BUY_QUANTITY);
    }

    #[test]
    fn staging_a_purchase_prices_it_and_closes_the_picker() {
        let mut s = BazaarScreenState {
            quantity: Some(DigitSpinner::item(5)),
            ..Default::default()
        };
        let buy = s.stage_purchase(&entry(3, 4096, 5, 1000, 500), 4);
        assert_eq!((buy.index, buy.quantity, buy.total_gil), (3, 4, 4200));
        assert!(s.quantity.is_none(), "picker closes behind the prompt");
        assert_eq!(s.pending, Some(buy));
    }

    #[test]
    fn the_page_trails_the_cursor_by_a_row_at_a_time() {
        const LEN: usize = 30;
        let mut s = BazaarScreenState::default();
        for _ in 0..LIST_ROWS - 1 {
            s.move_cursor(1, LEN);
        }
        assert_eq!(
            (s.cursor, s.viewport.start),
            (LIST_ROWS - 1, 0),
            "the last drawn row needs no scroll"
        );
        s.move_cursor(1, LEN);
        assert_eq!((s.cursor, s.viewport.start), (LIST_ROWS, 1));
        s.move_cursor(-1, LEN);
        assert_eq!(
            (s.cursor, s.viewport.start),
            (LIST_ROWS - 1, 1),
            "coming back inside the page leaves it where it was"
        );
    }

    #[test]
    fn wrapping_to_the_end_carries_the_page_with_it() {
        const LEN: usize = 30;
        let mut s = BazaarScreenState::default();
        s.move_cursor(-1, LEN);
        assert_eq!(
            (s.cursor, s.viewport.start),
            (LEN - 1, LEN - LIST_ROWS),
            "the wrapped-to row has to be drawn"
        );
    }

    #[test]
    fn a_sold_out_list_pulls_the_page_back_to_the_top() {
        let mut s = BazaarScreenState {
            cursor: 25,
            viewport: ListViewport { start: 16 },
            ..Default::default()
        };
        s.clamp(4);
        assert_eq!((s.cursor, s.viewport.start), (3, 0));
    }

    /// A ware name too long for its column has to clip on its row: the row plate
    /// is a fixed height and the label sits inside a clipping column, so the list
    /// cannot grow out from under the page that indexes it.
    #[test]
    fn a_long_ware_name_clips_instead_of_growing_its_row() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .add_systems(Startup, spawn_bazaar_view);
        app.update();

        let world = app.world_mut();
        let mut labels = world.query::<(Entity, &BazaarText, &TextLayout)>();
        let rows: Vec<(Entity, LineBreak, Justify)> = labels
            .iter(world)
            .filter(|(_, tag, _)| matches!(tag.0, BazaarRole::RowName(_)))
            .map(|(e, _, layout)| (e, layout.linebreak, layout.justify))
            .collect();
        assert_eq!(rows.len(), LIST_ROWS);

        for (label, linebreak, justify) in rows {
            let want = list_view::row_label_layout();
            assert_eq!(linebreak, want.linebreak);
            assert_eq!(justify, want.justify);
            let clip = world.get::<ChildOf>(label).expect("label has a column").0;
            let clip_node = world.get::<Node>(clip).expect("column is a node");
            assert_eq!(clip_node.overflow, list_view::row_label_clip().overflow);
            assert_eq!(clip_node.flex_basis, list_view::row_label_clip().flex_basis);
            let plate = world.get::<ChildOf>(clip).expect("column has a row").0;
            let plate_node = world.get::<Node>(plate).expect("row is a node");
            assert_eq!(plate_node.height, list_view::row_node().height);
            assert_eq!(plate_node.overflow, list_view::row_node().overflow);
        }
    }
}
