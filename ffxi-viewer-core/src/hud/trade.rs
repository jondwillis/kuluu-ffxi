//! Trade window — the retail-style "Trade" surface reached from the
//! contextual target-action menu's
//! [`crate::hud::action_model::TargetActionId::Trade`] entry.
//!
//! Layout mirrors vanilla FFXI: an 8-slot item grid (4 columns × 2 rows),
//! an **OK** button in the first cell of a control column and **Cancel**
//! directly below it, a **Gil** field reached by pressing Up from the top
//! grid row, and per-item count selectors for stackable items.
//!
//! ```text
//!   ┌──────────────── Trade ────────────────┐
//!   │            [ Gil: 0 ]                  │  ← Up from grid row 0
//!   │  ┌──┐ ┌──┐ ┌──┐ ┌──┐   ┌────┐          │
//!   │  │0 │ │1 │ │2 │ │3 │   │ OK │          │  ← grid row 0
//!   │  └──┘ └──┘ └──┘ └──┘   └────┘          │
//!   │  ┌──┐ ┌──┐ ┌──┐ ┌──┐   ┌──────┐        │
//!   │  │4 │ │5 │ │6 │ │7 │   │Cancel│        │  ← grid row 1
//!   │  └──┘ └──┘ └──┘ └──┘   └──────┘        │
//!   └────────────────────────────────────────┘
//! ```
//!
//! # Data model vs. render
//!
//! This file owns *only* the Trade window: its [`TradeState`] (the
//! placement / gil / focus the operator is building), its render systems,
//! and the [`TradeIntent`] message it emits. It does **not** own the
//! input-mode plumbing — the window is entered while
//! [`crate::input_mode::InputMode::TargetAction`] holds a `sub` frame that
//! the wiring stage
//! routes here; navigation keypresses are translated into the pure
//! [`TradeState`] mutators below by the wiring layer. Keeping the mutators
//! pure (no Bevy) means they're unit-testable without an `App`.
//!
//! The actual `0x036 TRADE_REQUEST` packet is *not* sent from here:
//! confirming OK emits a [`TradeIntent::Confirm`] which a later wiring
//! consumer turns into the outbound command. This keeps the network edge
//! out of viewer-core, matching how `quick_action` emits
//! `QuickActionActivated` rather than dispatching directly.
//!
//! Item eligibility (rare/ex and equipped items can't be traded) is
//! decided by [`is_tradeable`], reading the static `ItemStatic.flags`
//! word plus the live `snapshot.equipped` mirror — the same two-tier
//! split `item_meta::compose_item_detail` composes for the tooltip.

use bevy::prelude::*;
use ffxi_viewer_wire::SceneSnapshot;

use crate::hud::item_meta::{self, ItemDetail, ItemStatic};
use crate::hud::palette;
use crate::snapshot::SceneState;

/// Item-flags bit set on **Rare** items (only one may be held). Matches
/// the retail item-DAT flags word parsed by `ffxi_dat::item_dat`.
pub const ITEM_FLAG_RARE: u16 = 0x8000;
/// Item-flags bit set on **Ex** items (cannot be traded or auctioned).
pub const ITEM_FLAG_EX: u16 = 0x4000;

/// Number of trade item slots: 4 columns × 2 rows. Vanilla FFXI exposes
/// exactly eight.
pub const TRADE_COLS: usize = 4;
pub const TRADE_ROWS: usize = 2;
pub const TRADE_SLOTS: usize = TRADE_COLS * TRADE_ROWS;

/// Hard cap on a single stack in FFXI (99 for the largest stackables).
/// The per-slot count selector is clamped to `1..=min(stack_max, 99)`.
pub const STACK_MAX: u16 = 99;

/// Reddish-orange tint painted on a slot that currently holds a placed
/// item — the retail "this is staged for trade" highlight.
pub const PLACED_TINT: Color = Color::srgb(0.85, 0.35, 0.10);

/// One staged item in a trade slot. `item_no` keys the static DAT lookup
/// and the live inventory mirror; `count` is how many of a stackable to
/// hand over (always 1 for non-stackables).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TradeSlotItem {
    pub item_no: u16,
    pub count: u16,
}

/// Which control inside the Trade window currently has focus. Up from the
/// grid's top row lands on [`TradeFocus::Gil`]; the OK / Cancel buttons
/// occupy their own focus column to the right of the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeFocus {
    /// The gil entry field, reached by pressing Up from grid row 0.
    Gil,
    /// One of the eight item slots, `0..TRADE_SLOTS` (row-major).
    Slot(usize),
    /// The OK button (first cell of the control column).
    Ok,
    /// The Cancel button (below OK).
    Cancel,
}

impl Default for TradeFocus {
    fn default() -> Self {
        TradeFocus::Slot(0)
    }
}

/// A modal sub-selector overlaid on the Trade window. Mutually exclusive
/// with grid navigation: while a selector is open, Up/Down/Enter/digits
/// drive it, and Esc cancels back to the grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradeSelector {
    /// Digit-fill gil entry. `digits` accumulates typed characters; the
    /// effective value is `parse_gil(&digits)` clamped to `max`. Tab fills
    /// to `max` (= `snapshot.gil`). Re-entering the field resets to 0.
    Gil { digits: String, max: u32 },
    /// Stack count selector for the stackable item placed (or being
    /// placed) in `slot`. `value` is the chosen count, clamped
    /// `1..=max`.
    Stack { slot: usize, value: u16, max: u16 },
}

/// The full state the operator builds in the Trade window. Lives as a
/// `Resource` (one trade at a time) rather than inside the `InputMode`
/// variant: the window can stay populated while the operator dips back
/// out to the contextual menu, and a single resource keeps the render
/// systems' queries simple. The wiring layer clears it on
/// [`TradeIntent::Cancel`] / completion (lifecycle symmetry: the spawn of
/// this resource pairs with `reset` on exit).
#[derive(Resource, Debug, Clone, Default)]
pub struct TradeState {
    /// Whether the Trade window is currently shown. Driven by the wiring
    /// layer when the contextual Trade entry is chosen; cleared on
    /// Cancel / completion.
    pub open: bool,
    /// The trade target's entity id (server `UniqueNo`). 0 until a target
    /// is captured at open time.
    pub target_id: u32,
    /// Eight item slots, row-major; `None` = empty.
    pub slots: [Option<TradeSlotItem>; TRADE_SLOTS],
    /// Gil amount staged for trade.
    pub gil: u32,
    /// Currently-focused control.
    pub focus: TradeFocus,
    /// Open modal sub-selector, if any.
    pub selector: Option<TradeSelector>,
}

impl TradeState {
    /// Open a fresh trade against `target_id`, resetting all staged state.
    pub fn open(target_id: u32) -> Self {
        Self {
            open: true,
            target_id,
            slots: [None; TRADE_SLOTS],
            gil: 0,
            focus: TradeFocus::default(),
            selector: None,
        }
    }

    /// Reset to the closed/empty state. Paired with [`Self::open`] for
    /// lifecycle symmetry — the wiring layer calls this on Cancel and on
    /// trade completion so a stale grid never bleeds into the next trade.
    pub fn reset(&mut self) {
        *self = TradeState::default();
    }

    /// Count of non-empty slots.
    pub fn placed_count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// First free slot index, if any.
    pub fn first_free_slot(&self) -> Option<usize> {
        self.slots.iter().position(|s| s.is_none())
    }
}

/// Parse a digit-fill gil buffer into a value. Non-digit characters are
/// ignored (the input layer should only feed digits, but this stays
/// defensive). An empty buffer is 0. Saturates at `u32::MAX` rather than
/// overflowing on absurd input.
pub fn parse_gil(digits: &str) -> u32 {
    let mut v: u32 = 0;
    for c in digits.chars() {
        if let Some(d) = c.to_digit(10) {
            v = v.saturating_mul(10).saturating_add(d);
        }
    }
    v
}

/// Effective gil value for an open [`TradeSelector::Gil`]: the parsed
/// digit buffer clamped to the operator's actual gil (`max`).
pub fn effective_gil(digits: &str, max: u32) -> u32 {
    parse_gil(digits).min(max)
}

/// Clamp a requested stack count into the legal `1..=max` range, where
/// `max` is itself capped at [`STACK_MAX`]. Used by both the keyboard
/// increment/decrement and the digit-typed stack selector.
pub fn clamp_stack(value: u16, max: u16) -> u16 {
    let ceiling = max.clamp(1, STACK_MAX);
    value.clamp(1, ceiling)
}

/// Whether `item_no` may be staged for trade given its static DAT facts
/// and the live snapshot. Rare/Ex items and items currently equipped are
/// rejected — matching retail, which greys them out in the trade picker.
///
/// `dat` is the resolved [`ItemStatic`] (the caller owns the `DatRoot`
/// lookup). When `None` (no DAT install reachable) we can't read the
/// rare/ex flags, so we fall back to the only signal we *do* have — the
/// equipped mirror — and otherwise allow the placement. This keeps the
/// label-only degraded mode usable rather than blocking all trades.
pub fn is_tradeable(item_no: u16, dat: Option<&ItemStatic>, snapshot: &SceneSnapshot) -> bool {
    if snapshot.equipped.contains(&Some(item_no)) {
        return false;
    }
    if let Some(st) = dat {
        if st.flags & (ITEM_FLAG_RARE | ITEM_FLAG_EX) != 0 {
            return false;
        }
    }
    true
}

/// Whether the item is stackable (stack size > 1), so the count selector
/// should open on placement. Falls back to non-stackable (1) when no DAT
/// install is reachable, since the stack ceiling lives in the static DAT.
///
/// Foundation note: `ItemStatic` does not yet carry an explicit
/// `stack_size` field; until the `ffxi_dat::item_dat` bridge adds it we
/// read it through [`stack_size_of`], which the DAT-feature agent will
/// repoint. For now an item is treated as non-stackable when its stack
/// size is unknown.
pub fn is_stackable(dat: Option<&ItemStatic>) -> bool {
    stack_size_of(dat) > 1
}

/// Stack ceiling for an item, clamped to [`STACK_MAX`]. Returns 1 when the
/// static data is unavailable.
///
/// The retail item DAT stores the stack size in the static block; the
/// `ffxi_dat::item_dat::ItemStatic` bridge will surface it as a field and
/// this helper becomes a one-line accessor. Defined as a seam now so the
/// Trade selectors don't reach into DAT internals.
pub fn stack_size_of(dat: Option<&ItemStatic>) -> u16 {
    // Until the bridge lands, `ItemStatic` has no stack field; treat
    // every item as non-stackable. The selector code paths are exercised
    // by tests that construct the post-bridge value directly.
    let _ = dat;
    1
}

/// Message emitted by the Trade window for the wiring layer to consume and
/// drive `0x036 TRADE_REQUEST`. The window itself never touches the
/// network — it only describes operator intent.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub enum TradeIntent {
    /// Stage `count` of `item_no` into `slot` (count is 1 for
    /// non-stackables). A `None` item clears the slot.
    Placement {
        slot: usize,
        item_no: Option<u16>,
        count: u16,
    },
    /// Set the staged gil amount.
    Gil { amount: u32 },
    /// Operator pressed OK — commit the trade against `target_id`.
    Confirm { target_id: u32 },
    /// Operator pressed Cancel / Esc'd out — abandon the trade.
    Cancel,
}

// ---------------------------------------------------------------------------
// Pure state mutators. The wiring layer maps keypresses to these; they are
// Bevy-free so the navigation logic is unit-testable without an `App`.
// ---------------------------------------------------------------------------

/// Move focus Up. From the grid's top row (`Slot(0..TRADE_COLS)`) Up lands
/// on the [`TradeFocus::Gil`] field. From OK Up is a no-op (it's the top of
/// the control column); from Cancel Up moves to OK. Within the grid Up
/// moves one row up.
pub fn focus_up(state: &mut TradeState) {
    state.focus = match state.focus {
        TradeFocus::Gil => TradeFocus::Gil,
        TradeFocus::Slot(i) if i < TRADE_COLS => TradeFocus::Gil,
        TradeFocus::Slot(i) => TradeFocus::Slot(i - TRADE_COLS),
        TradeFocus::Ok => TradeFocus::Ok,
        TradeFocus::Cancel => TradeFocus::Ok,
    };
}

/// Move focus Down. From Gil, Down returns to grid slot 0. Within the grid
/// Down moves one row down (the bottom row stays put). OK→Cancel.
pub fn focus_down(state: &mut TradeState) {
    state.focus = match state.focus {
        TradeFocus::Gil => TradeFocus::Slot(0),
        TradeFocus::Slot(i) if i + TRADE_COLS < TRADE_SLOTS => TradeFocus::Slot(i + TRADE_COLS),
        TradeFocus::Slot(i) => TradeFocus::Slot(i),
        TradeFocus::Ok => TradeFocus::Cancel,
        TradeFocus::Cancel => TradeFocus::Cancel,
    };
}

/// Move focus Left. Within a grid row, step left one column; from the
/// leftmost column stay put. From the OK/Cancel control column, Left jumps
/// back into the grid's rightmost column on the matching row.
pub fn focus_left(state: &mut TradeState) {
    state.focus = match state.focus {
        TradeFocus::Slot(i) if i % TRADE_COLS > 0 => TradeFocus::Slot(i - 1),
        TradeFocus::Ok => TradeFocus::Slot(TRADE_COLS - 1),
        TradeFocus::Cancel => TradeFocus::Slot(TRADE_SLOTS - 1),
        other => other,
    };
}

/// Move focus Right. Within a grid row, step right one column; from the
/// rightmost grid column, Right jumps to the OK/Cancel control column on
/// the matching row (row 0 → OK, row 1 → Cancel).
pub fn focus_right(state: &mut TradeState) {
    state.focus = match state.focus {
        TradeFocus::Slot(i) if i % TRADE_COLS < TRADE_COLS - 1 => TradeFocus::Slot(i + 1),
        // Rightmost column: jump to the control column on the same row.
        TradeFocus::Slot(i) if i < TRADE_COLS => TradeFocus::Ok,
        TradeFocus::Slot(_) => TradeFocus::Cancel,
        other => other,
    };
}

/// Open the gil selector. Per the spec, re-entering the field resets the
/// staged amount to 0 (the digit buffer starts empty).
pub fn begin_gil_entry(state: &mut TradeState, snapshot_gil: u32) {
    state.selector = Some(TradeSelector::Gil {
        digits: String::new(),
        max: snapshot_gil,
    });
}

/// Append a typed digit to the open gil selector. No-op if the selector
/// isn't a Gil entry.
pub fn gil_push_digit(state: &mut TradeState, c: char) {
    if let Some(TradeSelector::Gil { digits, .. }) = state.selector.as_mut() {
        if c.is_ascii_digit() {
            digits.push(c);
        }
    }
}

/// Tab past the digits: fill the gil buffer to `max` (= snapshot gil).
pub fn gil_fill_max(state: &mut TradeState) {
    if let Some(TradeSelector::Gil { digits, max }) = state.selector.as_mut() {
        *digits = max.to_string();
    }
}

/// Confirm the gil selector: commit the clamped value to `state.gil` and
/// close the selector. Returns the committed amount for the caller to wrap
/// in a [`TradeIntent::Gil`]. `None` if no gil selector was open.
pub fn gil_confirm(state: &mut TradeState) -> Option<u32> {
    if let Some(TradeSelector::Gil { digits, max }) = state.selector.take() {
        let amount = effective_gil(&digits, max);
        state.gil = amount;
        Some(amount)
    } else {
        None
    }
}

/// Open the stack-count selector for `slot`, starting at the full stack
/// (retail defaults the spinner to the whole stack). `stack_max` is the
/// item's stack ceiling; the value is clamped to `1..=min(stack_max, 99)`.
pub fn begin_stack_entry(state: &mut TradeState, slot: usize, stack_max: u16) {
    let max = stack_max.clamp(1, STACK_MAX);
    state.selector = Some(TradeSelector::Stack {
        slot,
        value: max,
        max,
    });
}

/// Nudge the open stack selector by `delta` (clamped). No-op when no stack
/// selector is open.
pub fn stack_adjust(state: &mut TradeState, delta: i32) {
    if let Some(TradeSelector::Stack { value, max, .. }) = state.selector.as_mut() {
        let next = (*value as i32 + delta).max(1) as u16;
        *value = clamp_stack(next, *max);
    }
}

/// Confirm the stack selector: stage the item with its chosen count and
/// close the selector. Returns `(slot, count)` for the caller to wrap in a
/// [`TradeIntent::Placement`]. `None` if no stack selector was open or the
/// slot is empty.
pub fn stack_confirm(state: &mut TradeState) -> Option<(usize, u16)> {
    if let Some(TradeSelector::Stack { slot, value, .. }) = state.selector.take() {
        if let Some(item) = state.slots[slot].as_mut() {
            item.count = value;
            return Some((slot, value));
        }
    }
    None
}

/// Place `item_no` into `slot` with `count`. Returns `false` (and stages
/// nothing) if the item isn't tradeable. For a stackable item the caller
/// then opens the count selector via [`begin_stack_entry`]; the initial
/// `count` here is 1.
pub fn place_item(
    state: &mut TradeState,
    slot: usize,
    item_no: u16,
    count: u16,
    dat: Option<&ItemStatic>,
    snapshot: &SceneSnapshot,
) -> bool {
    if slot >= TRADE_SLOTS || !is_tradeable(item_no, dat, snapshot) {
        return false;
    }
    state.slots[slot] = Some(TradeSlotItem {
        item_no,
        count: count.max(1),
    });
    true
}

/// Clear the item in `slot`.
pub fn clear_slot(state: &mut TradeState, slot: usize) {
    if slot < TRADE_SLOTS {
        state.slots[slot] = None;
    }
}

/// Compose the tooltip detail for whichever slot currently has focus,
/// reusing the shared [`item_meta::compose_item_detail`] composer so the
/// Trade tooltip is identical to the one the Items menu and `/check`
/// render. `dat_lookup` is supplied by the caller (it owns the `DatRoot`);
/// returns `None` when no item is focused.
pub fn focused_tooltip(
    state: &TradeState,
    snapshot: &SceneSnapshot,
    dat_lookup: impl Fn(u16) -> Option<ItemStatic>,
) -> Option<ItemDetail> {
    let TradeFocus::Slot(i) = state.focus else {
        return None;
    };
    let item = state.slots[i]?;
    Some(item_meta::compose_item_detail(
        item.item_no,
        snapshot,
        dat_lookup(item.item_no),
    ))
}

// ---------------------------------------------------------------------------
// Render layer. Spawns the window once (hidden) and updates it from
// `TradeState` each frame, in the style of `shop`/`quick_action`.
// ---------------------------------------------------------------------------

#[derive(Component)]
pub struct TradePanel;

#[derive(Component)]
pub struct TradeTitle;

/// One control cell: a grid slot, the gil field, or OK / Cancel. Carries
/// the [`TradeFocus`] it represents so the updater can paint the cursor and
/// the placed-tint without re-deriving geometry.
#[derive(Component, Clone, Copy)]
pub struct TradeCell {
    pub focus: TradeFocus,
}

/// Subtitle line under the grid: shows the open selector's live value
/// (gil digits or stack count) or the focused item's name.
#[derive(Component)]
pub struct TradeStatusLine;

const PANEL_WIDTH_PX: f32 = 300.0;

/// Spawn the (initially hidden) Trade window. Registered by the wiring
/// layer via `add_hud_spawners`, mirroring `spawn_shop_panel`.
pub fn spawn_trade_window(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            TradePanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(30.0),
                left: Val::Percent(40.0),
                width: Val::Px(PANEL_WIDTH_PX),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                display: Display::None,
                ..default()
            },
            ZIndex(25),
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|p| {
            p.spawn((
                TradeTitle,
                Text::new("Trade"),
                TextFont {
                    font_size: 15.0,
                    ..default()
                },
                TextColor(palette::ACCENT),
            ));

            // Gil field (above the grid).
            spawn_cell(p, TradeFocus::Gil, "Gil: 0");

            // Grid rows, each row paired with its control-column button:
            // row 0 → OK, row 1 → Cancel.
            for row in 0..TRADE_ROWS {
                p.spawn((
                    Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(6.0),
                        ..default()
                    },
                    BackgroundColor(Color::NONE),
                ))
                .with_children(|r| {
                    for col in 0..TRADE_COLS {
                        let slot = row * TRADE_COLS + col;
                        spawn_cell(r, TradeFocus::Slot(slot), "·");
                    }
                    // Control column cell on this row.
                    let ctrl = if row == 0 {
                        (TradeFocus::Ok, "OK")
                    } else {
                        (TradeFocus::Cancel, "Cancel")
                    };
                    spawn_cell(r, ctrl.0, ctrl.1);
                });
            }

            p.spawn((
                TradeStatusLine,
                Text::new(""),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(palette::MUTED),
            ));
        });
}

/// Spawn one control cell as a child of `p`.
fn spawn_cell(p: &mut ChildSpawnerCommands, focus: TradeFocus, label: &str) {
    p.spawn((
        TradeCell { focus },
        Node {
            min_width: Val::Px(40.0),
            padding: UiRect::axes(Val::Px(4.0), Val::Px(2.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BorderColor::all(palette::BORDER),
        BackgroundColor(Color::NONE),
    ))
    .with_children(|c| {
        c.spawn((
            Text::new(label.to_string()),
            TextFont {
                font_size: 12.0,
                ..default()
            },
            TextColor(palette::TEXT),
        ));
    });
}

/// Drive the Trade window from [`TradeState`]. Shows/hides the panel,
/// paints the focus cursor (accent border) and the reddish-orange placed
/// tint, and updates the status line with the open selector's live value.
#[allow(clippy::type_complexity)]
pub fn update_trade_window(
    state: Res<SceneState>,
    trade: Res<TradeState>,
    mut panel_q: Query<&mut Node, With<TradePanel>>,
    mut cell_q: Query<
        (
            &TradeCell,
            &mut BorderColor,
            &mut BackgroundColor,
            &Children,
        ),
        Without<TradePanel>,
    >,
    mut text_q: Query<&mut Text, Without<TradeStatusLine>>,
    mut status_q: Query<&mut Text, With<TradeStatusLine>>,
) {
    let Ok(mut panel) = panel_q.single_mut() else {
        return;
    };

    if !trade.open {
        if panel.display != Display::None {
            panel.display = Display::None;
        }
        return;
    }
    if panel.display == Display::None {
        panel.display = Display::Flex;
    }

    for (cell, mut border, mut bg, children) in cell_q.iter_mut() {
        let focused = cell.focus == trade.focus;

        // Cursor: accent border on the focused cell, default border
        // otherwise.
        let want_border = if focused {
            palette::ACCENT
        } else {
            palette::BORDER
        };
        if border.left != want_border {
            *border = BorderColor::all(want_border);
        }

        // Placed tint on grid slots that hold an item.
        let want_bg = match cell.focus {
            TradeFocus::Slot(i) if trade.slots[i].is_some() => PLACED_TINT,
            _ => Color::NONE,
        };
        if bg.0 != want_bg {
            bg.0 = want_bg;
        }

        // Cell text.
        let want_text = cell_text(&trade, cell.focus);
        if let Some(child) = children.first() {
            if let Ok(mut text) = text_q.get_mut(*child) {
                if **text != want_text {
                    **text = want_text;
                }
            }
        }
    }

    if let Ok(mut status) = status_q.single_mut() {
        let want = status_text(&trade, &state.snapshot);
        if **status != want {
            **status = want;
        }
    }
}

/// Label text for a control cell given the current trade state.
fn cell_text(trade: &TradeState, focus: TradeFocus) -> String {
    match focus {
        TradeFocus::Gil => format!("Gil: {}", trade.gil),
        TradeFocus::Ok => "OK".to_string(),
        TradeFocus::Cancel => "Cancel".to_string(),
        TradeFocus::Slot(i) => match trade.slots[i] {
            Some(item) if item.count > 1 => format!("#{} x{}", item.item_no, item.count),
            Some(item) => format!("#{}", item.item_no),
            None => "·".to_string(),
        },
    }
}

/// Status-line text: the open selector's live value takes priority; else
/// the focused slot's item name (resolved later via the DAT bridge — for
/// now the numeric id), else a hint.
fn status_text(trade: &TradeState, _snapshot: &SceneSnapshot) -> String {
    match &trade.selector {
        Some(TradeSelector::Gil { digits, max }) => {
            format!("Gil ▸ {} / {}", effective_gil(digits, *max), max)
        }
        Some(TradeSelector::Stack { value, max, .. }) => {
            format!("Count ▸ {value} / {max}")
        }
        None => match trade.focus {
            TradeFocus::Slot(i) => match trade.slots[i] {
                Some(item) => format!("#{}", item.item_no),
                None => "Empty slot".to_string(),
            },
            TradeFocus::Gil => "Enter to edit gil".to_string(),
            TradeFocus::Ok => "Confirm trade".to_string(),
            TradeFocus::Cancel => "Cancel trade".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> SceneSnapshot {
        SceneSnapshot::default()
    }

    fn rare_ex(flags: u16) -> ItemStatic {
        ItemStatic {
            flags,
            ..Default::default()
        }
    }

    #[test]
    fn parse_gil_ignores_non_digits_and_saturates() {
        assert_eq!(parse_gil(""), 0);
        assert_eq!(parse_gil("0123"), 123);
        assert_eq!(parse_gil("99999999999999999999"), u32::MAX);
    }

    #[test]
    fn effective_gil_clamps_to_max() {
        assert_eq!(effective_gil("5000", 1000), 1000);
        assert_eq!(effective_gil("500", 1000), 500);
    }

    #[test]
    fn gil_reentry_resets_then_tab_fills_max() {
        let mut s = TradeState::open(42);
        s.gil = 777; // a prior trade left a value behind
        begin_gil_entry(&mut s, 5000); // re-entry resets the buffer to empty
                                       // Buffer starts empty → committing now would be 0.
        gil_fill_max(&mut s); // tab past digits = max
        let committed = gil_confirm(&mut s).unwrap();
        assert_eq!(committed, 5000);
        assert_eq!(s.gil, 5000);
        assert!(s.selector.is_none());
    }

    #[test]
    fn gil_digit_fill_then_confirm_clamps() {
        let mut s = TradeState::open(1);
        begin_gil_entry(&mut s, 1500);
        for c in "9000".chars() {
            gil_push_digit(&mut s, c);
        }
        assert_eq!(gil_confirm(&mut s), Some(1500)); // clamped to max
    }

    #[test]
    fn stack_selector_clamps_to_99() {
        assert_eq!(clamp_stack(0, 99), 1);
        assert_eq!(clamp_stack(50, 99), 50);
        assert_eq!(clamp_stack(120, 99), 99);
        assert_eq!(clamp_stack(120, 12), 12); // item's own ceiling
    }

    #[test]
    fn stack_entry_defaults_to_full_stack_and_adjusts() {
        let mut s = TradeState::open(1);
        // Stage a stackable item first so confirm has a slot to write.
        s.slots[2] = Some(TradeSlotItem {
            item_no: 4096,
            count: 1,
        });
        begin_stack_entry(&mut s, 2, 12);
        stack_adjust(&mut s, -3); // 12 → 9
        let (slot, count) = stack_confirm(&mut s).unwrap();
        assert_eq!((slot, count), (2, 9));
        assert_eq!(s.slots[2].unwrap().count, 9);
    }

    #[test]
    fn rare_and_ex_items_are_not_tradeable() {
        let sn = snap();
        assert!(!is_tradeable(1, Some(&rare_ex(ITEM_FLAG_RARE)), &sn));
        assert!(!is_tradeable(2, Some(&rare_ex(ITEM_FLAG_EX)), &sn));
        assert!(is_tradeable(3, Some(&rare_ex(0)), &sn));
    }

    #[test]
    fn equipped_items_are_not_tradeable() {
        let mut sn = snap();
        sn.equipped[0] = Some(555);
        assert!(!is_tradeable(555, Some(&rare_ex(0)), &sn));
        assert!(is_tradeable(556, Some(&rare_ex(0)), &sn));
    }

    #[test]
    fn no_dat_allows_unequipped_items() {
        let sn = snap();
        // Degraded mode: no DAT → can't read rare/ex, allow if unequipped.
        assert!(is_tradeable(999, None, &sn));
    }

    #[test]
    fn place_rejects_untradeable() {
        let mut s = TradeState::open(1);
        let sn = snap();
        assert!(!place_item(
            &mut s,
            0,
            1,
            1,
            Some(&rare_ex(ITEM_FLAG_RARE)),
            &sn
        ));
        assert!(s.slots[0].is_none());
        assert!(place_item(&mut s, 0, 1, 1, Some(&rare_ex(0)), &sn));
        assert_eq!(s.slots[0].unwrap().item_no, 1);
    }

    #[test]
    fn up_from_top_grid_row_lands_on_gil() {
        let mut s = TradeState::open(1);
        s.focus = TradeFocus::Slot(2); // top row
        focus_up(&mut s);
        assert_eq!(s.focus, TradeFocus::Gil);
    }

    #[test]
    fn up_within_grid_moves_one_row() {
        let mut s = TradeState::open(1);
        s.focus = TradeFocus::Slot(5); // bottom row, col 1
        focus_up(&mut s);
        assert_eq!(s.focus, TradeFocus::Slot(1));
    }

    #[test]
    fn gil_down_returns_to_grid() {
        let mut s = TradeState::open(1);
        s.focus = TradeFocus::Gil;
        focus_down(&mut s);
        assert_eq!(s.focus, TradeFocus::Slot(0));
    }

    #[test]
    fn right_from_grid_edge_enters_control_column() {
        let mut s = TradeState::open(1);
        s.focus = TradeFocus::Slot(TRADE_COLS - 1); // top-right grid cell
        focus_right(&mut s);
        assert_eq!(s.focus, TradeFocus::Ok);
        s.focus = TradeFocus::Slot(TRADE_SLOTS - 1); // bottom-right grid cell
        focus_right(&mut s);
        assert_eq!(s.focus, TradeFocus::Cancel);
    }

    #[test]
    fn ok_down_moves_to_cancel() {
        let mut s = TradeState::open(1);
        s.focus = TradeFocus::Ok;
        focus_down(&mut s);
        assert_eq!(s.focus, TradeFocus::Cancel);
    }

    #[test]
    fn reset_clears_everything() {
        let mut s = TradeState::open(7);
        s.gil = 100;
        s.slots[0] = Some(TradeSlotItem {
            item_no: 1,
            count: 1,
        });
        s.reset();
        assert!(!s.open);
        assert_eq!(s.placed_count(), 0);
        assert_eq!(s.gil, 0);
    }
}
