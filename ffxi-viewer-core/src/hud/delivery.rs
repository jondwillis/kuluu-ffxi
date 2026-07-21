//! Dedicated retail-faithful delivery box screen.
//!
//! Replaces the old dialog+grid delivery UI. One modal window, gated on
//! `SceneSnapshot::delivery_box`, with a 4x2 slot grid, a recipient field, the
//! rich inventory list (reused item detail + icons), a numeric quantity/gil
//! spinner ([`super::spinner`]), and a Current Gil line. Rendering reads the
//! snapshot + [`DeliveryScreenState`]; the native input layer drives focus and
//! emits the delivery `AgentCommand`s.

use bevy::prelude::*;
use ffxi_viewer_wire::{DeliveryBoxNo, DeliveryBoxState, RecipientStatus, SceneSnapshot};

use crate::hud::spinner::{Spinner, SpinnerBinding, SpinnerTarget};

/// Retail lays the 8 outgoing/incoming slots out 4 across, 2 down.
pub const GRID_COLS: usize = 4;
pub const GRID_ROWS: usize = 2;
pub const GRID_SLOTS: usize = GRID_COLS * GRID_ROWS;

/// Visible inventory rows in the send-box item list (matches `item_screen`).
pub const INV_LIST_ROWS: usize = 10;

/// LSB stores gil as item 65535 at LOC_INVENTORY slot 0.
pub const GIL_ITEM_NO: u16 = ffxi_proto::map::GIL_ITEM_NO;

/// Which region of the delivery screen currently has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryFocus {
    /// A grid cell (outgoing staging slot or incoming parcel).
    Slot(usize),
    /// Recipient text field (outgoing only).
    Recipient,
    /// Current Gil line — focusing it opens the gil spinner (outgoing only).
    Gil,
    /// A row in the inventory item list (outgoing only).
    InvRow(usize),
    /// The Send button: dispatch every staged slot (outgoing).
    SendOk,
    /// Close the box (PostClose).
    Exit,
    /// Take the focused incoming parcel (Accept→Get).
    TakeBtn,
    /// Return the focused incoming parcel to its sender.
    RejectBtn,
}

impl Default for DeliveryFocus {
    fn default() -> Self {
        DeliveryFocus::Slot(0)
    }
}

/// Transient UI state for the delivery screen (focus, active spinner, in-flight
/// recipient text, and per-region cursor memory so returning to a region lands
/// where you left it — "top on enter, historical on back").
#[derive(Resource, Debug, Clone, Default)]
pub struct DeliveryScreenState {
    /// Mirrors `snapshot.delivery_box.is_some()`; the sync system resets the
    /// rest of this struct on the open/close edge.
    pub active: bool,
    pub focus: DeliveryFocus,
    pub selector: Option<SpinnerBinding>,
    /// `Some` while the recipient text field is being edited.
    pub recipient_buf: Option<String>,
    pub last_out_slot: usize,
    pub last_in_slot: usize,
    pub last_inv_row: usize,
}

impl DeliveryScreenState {
    /// Reset to the default focus for a freshly opened box.
    pub fn open(&mut self) {
        *self = DeliveryScreenState {
            active: true,
            focus: DeliveryFocus::Slot(0),
            ..Default::default()
        };
    }

    pub fn close(&mut self) {
        *self = DeliveryScreenState::default();
    }

    /// Remember the cursor position of grid/list regions before leaving them.
    fn remember(&mut self, box_no: DeliveryBoxNo) {
        match self.focus {
            DeliveryFocus::Slot(i) => match box_no {
                DeliveryBoxNo::Outgoing => self.last_out_slot = i,
                DeliveryBoxNo::Incoming => self.last_in_slot = i,
            },
            DeliveryFocus::InvRow(i) => self.last_inv_row = i,
            _ => {}
        }
    }
}

/// A deliverable inventory row surfaced in the send list. `deliverable` is
/// false for EX/RARE/NoDelivery stacks (rendered greyed and inert), mirroring
/// retail. Gil is excluded (it's entered via the Gil line, not the list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvRow {
    pub inv_slot: u8,
    pub item_no: u16,
    pub quantity: u32,
    pub deliverable: bool,
}

/// The send-box inventory list, rebuilt only when the snapshot changes (not
/// per frame — a per-frame filter/sort over the bag would hitch).
#[derive(Resource, Debug, Default, Clone)]
pub struct DeliveryInventory {
    pub rows: Vec<InvRow>,
}

/// Context the pure focus functions need: what the snapshot currently shows.
#[derive(Debug, Clone, Copy)]
pub struct DeliveryCtx {
    pub box_no: DeliveryBoxNo,
    pub inv_len: usize,
    pub recipient_ok: bool,
}

impl DeliveryCtx {
    pub fn from(snap: &SceneSnapshot, inv_len: usize) -> Option<Self> {
        let d = snap.delivery_box.as_ref()?;
        Some(Self {
            box_no: d.box_no,
            inv_len,
            recipient_ok: matches!(d.recipient_status, RecipientStatus::Ok { .. }),
        })
    }

    fn outgoing(&self) -> bool {
        self.box_no == DeliveryBoxNo::Outgoing
    }
}

fn row_of(slot: usize) -> usize {
    slot / GRID_COLS
}

fn col_of(slot: usize) -> usize {
    slot % GRID_COLS
}

/// Move focus up. Grid top row escapes to Recipient (outgoing); the inventory
/// list scrolls; buttons walk toward the grid.
pub fn focus_up(state: &mut DeliveryScreenState, ctx: &DeliveryCtx) {
    state.remember(ctx.box_no);
    state.focus = match state.focus {
        DeliveryFocus::Slot(i) if row_of(i) == 0 => {
            if ctx.outgoing() {
                DeliveryFocus::Recipient
            } else {
                DeliveryFocus::Slot(i)
            }
        }
        DeliveryFocus::Slot(i) => DeliveryFocus::Slot(i - GRID_COLS),
        DeliveryFocus::InvRow(i) => DeliveryFocus::InvRow(i.saturating_sub(1)),
        DeliveryFocus::Gil => DeliveryFocus::Slot(GRID_COLS + state.last_out_slot % GRID_COLS),
        DeliveryFocus::SendOk => DeliveryFocus::Gil,
        DeliveryFocus::Exit if ctx.outgoing() => DeliveryFocus::Gil,
        other => other,
    };
}

/// Move focus down. Grid bottom row escapes to the Gil line (outgoing); buttons
/// step Gil → Send/Exit.
pub fn focus_down(state: &mut DeliveryScreenState, ctx: &DeliveryCtx) {
    state.remember(ctx.box_no);
    state.focus = match state.focus {
        DeliveryFocus::Recipient => DeliveryFocus::Slot(0),
        DeliveryFocus::Slot(i) if row_of(i) + 1 < GRID_ROWS => DeliveryFocus::Slot(i + GRID_COLS),
        DeliveryFocus::Slot(_) if ctx.outgoing() => DeliveryFocus::Gil,
        DeliveryFocus::Slot(i) => DeliveryFocus::Slot(i),
        DeliveryFocus::InvRow(i) => {
            let last = ctx.inv_len.saturating_sub(1);
            DeliveryFocus::InvRow((i + 1).min(last))
        }
        DeliveryFocus::Gil => DeliveryFocus::SendOk,
        other => other,
    };
}

/// Move focus left. From the grid's left edge / the button row, hop to the
/// inventory-list column's mirror; within a row, step one cell.
pub fn focus_left(state: &mut DeliveryScreenState, ctx: &DeliveryCtx) {
    state.remember(ctx.box_no);
    state.focus = match state.focus {
        DeliveryFocus::Slot(i) if col_of(i) > 0 => DeliveryFocus::Slot(i - 1),
        DeliveryFocus::InvRow(_) => DeliveryFocus::Slot(clamp_slot(state.last_out_slot)),
        DeliveryFocus::Exit => DeliveryFocus::SendOk,
        DeliveryFocus::RejectBtn => DeliveryFocus::TakeBtn,
        other => other,
    };
}

/// Move focus right. Grid right edge / recipient / gil hop to the inventory
/// list (outgoing); within a row, step one cell; Send → Exit.
pub fn focus_right(state: &mut DeliveryScreenState, ctx: &DeliveryCtx) {
    state.remember(ctx.box_no);
    state.focus = match state.focus {
        DeliveryFocus::Slot(i) if col_of(i) + 1 < GRID_COLS => DeliveryFocus::Slot(i + 1),
        DeliveryFocus::Slot(_) if ctx.outgoing() && ctx.inv_len > 0 => {
            DeliveryFocus::InvRow(clamp_row(state.last_inv_row, ctx.inv_len))
        }
        DeliveryFocus::Recipient if ctx.inv_len > 0 => {
            DeliveryFocus::InvRow(clamp_row(state.last_inv_row, ctx.inv_len))
        }
        DeliveryFocus::Gil if ctx.inv_len > 0 => {
            DeliveryFocus::InvRow(clamp_row(state.last_inv_row, ctx.inv_len))
        }
        DeliveryFocus::SendOk => DeliveryFocus::Exit,
        DeliveryFocus::TakeBtn => DeliveryFocus::RejectBtn,
        other => other,
    };
}

fn clamp_slot(slot: usize) -> usize {
    slot.min(GRID_SLOTS - 1)
}

fn clamp_row(row: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        row.min(len - 1)
    }
}

/// Begin staging the inventory item at `row` into the first free outbox slot:
/// stackables open a quantity spinner, singletons return a ready binding at
/// quantity 1. `None` if no free slot or the row is not deliverable.
pub fn begin_item_stage(row: &InvRow, first_free: Option<usize>) -> Option<SpinnerBinding> {
    if !row.deliverable {
        return None;
    }
    let out_slot = first_free? as u8;
    let target = SpinnerTarget::ItemQty {
        inv_slot: row.inv_slot,
        item_no: row.item_no,
        out_slot,
    };
    let mut spinner = Spinner::item(row.quantity.max(1));
    if row.quantity <= 1 {
        // Singleton: pre-confirm quantity 1 (no spinner needed).
        spinner.set_all();
    }
    Some(SpinnerBinding { spinner, target })
}

/// Begin entering a gil amount to send into the first free outbox slot.
pub fn begin_gil_stage(current_gil: u32, first_free: Option<usize>) -> Option<SpinnerBinding> {
    let out_slot = first_free? as u8;
    Some(SpinnerBinding {
        spinner: Spinner::gil(current_gil),
        target: SpinnerTarget::Gil { out_slot },
    })
}

/// Build the deliverable inventory rows from a snapshot's LOC_INVENTORY. `deliv`
/// answers `item_flags::deliverable`; `is_ex_rare` flags DAT EX/RARE (also
/// undeliverable). Gil (slot 0 / item 65535) and empty/locked slots are skipped.
pub fn build_inventory<F, G>(snap: &SceneSnapshot, deliverable: F, ex_rare: G) -> Vec<InvRow>
where
    F: Fn(u16) -> bool,
    G: Fn(u16) -> bool,
{
    snap.inventory_main()
        .iter()
        .filter(|it| it.index != 0 && it.item_no != GIL_ITEM_NO && it.item_no != 0)
        .map(|it| InvRow {
            inv_slot: it.index,
            item_no: it.item_no,
            quantity: it.quantity,
            deliverable: !it.locked && deliverable(it.item_no) && !ex_rare(it.item_no),
        })
        .collect()
}

/// Current gil = quantity of the gil item at LOC_INVENTORY slot 0.
pub fn current_gil(snap: &SceneSnapshot) -> u32 {
    snap.inventory_main()
        .iter()
        .find(|it| it.index == 0 || it.item_no == GIL_ITEM_NO)
        .map(|it| it.quantity)
        .unwrap_or(0)
}

/// First empty outgoing slot, if any.
pub fn first_free_slot(d: &DeliveryBoxState) -> Option<usize> {
    d.slots.iter().position(|s| s.is_none())
}

/// First filled cursor position `viewport_start` for a `rows`-tall list pool.
fn viewport_start(cursor: usize, total: usize, rows: usize) -> usize {
    if total <= rows {
        return 0;
    }
    let half = rows / 2;
    let max_start = total - rows;
    cursor.saturating_sub(half).min(max_start)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::item_ui::{self, transparent_placeholder};
use crate::hud::style::{text_font, theme, window_frame};
use crate::snapshot::SceneState;

/// A styled `Button`-like caption identifying an action region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtnId {
    Send,
    Exit,
    Take,
    Reject,
}

impl BtnId {
    fn focus(self) -> DeliveryFocus {
        match self {
            BtnId::Send => DeliveryFocus::SendOk,
            BtnId::Exit => DeliveryFocus::Exit,
            BtnId::Take => DeliveryFocus::TakeBtn,
            BtnId::Reject => DeliveryFocus::RejectBtn,
        }
    }

    fn caption(self) -> &'static str {
        match self {
            BtnId::Send => "Send",
            BtnId::Exit => "Exit",
            BtnId::Take => "Take",
            BtnId::Reject => "Reject",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    GridHeader,
    RecipientLabel,
    RecipientValue,
    CellQty(usize),
    GilLine,
    SpinnerLine,
    DetailName,
    DetailRow(usize),
    InvHeader,
    InvRow(usize),
    Button(BtnId),
    Hint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IconId {
    Cell(usize),
    Inv(usize),
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameId {
    Cell(usize),
    RecipientBox,
    SpinnerBox,
    InvBox,
    DetailBox,
    InvRow(usize),
    Button(BtnId),
}

#[derive(Component)]
pub(crate) struct DeliveryScreenRoot;

#[derive(Component)]
pub(crate) struct DeliveryText(Role);

#[derive(Component)]
pub(crate) struct DeliveryIcon(IconId);

#[derive(Component)]
pub(crate) struct DeliveryFrame(FrameId);

const DETAIL_ROWS: usize = 8;
const DETAIL_ICON_PX: f32 = 32.0;
const INV_ICON_PX: f32 = 18.0;

pub(crate) fn spawn_delivery_screen(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = transparent_placeholder(&mut images);

    commands
        .spawn((
            crate::components::InGameEntity,
            DeliveryScreenRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                left: Val::Px(8.0),
                column_gap: Val::Px(6.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::FlexStart,
                display: Display::None,
                ..default()
            },
        ))
        .with_children(|root| {
            // Left column: recipient, grid, gil/spinner, buttons.
            root.spawn(Node {
                width: Val::Px(230.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|col| {
                // Recipient field (outgoing only).
                let (n, bg, bd) = window_frame();
                col.spawn((DeliveryFrame(FrameId::RecipientBox), n, bg, bd))
                    .with_children(|p| {
                        spawn_text(p, Role::RecipientLabel, 13.0, theme::TITLE);
                        spawn_text(p, Role::RecipientValue, 14.0, theme::TEXT);
                    });

                // Grid box: header + 4x2 cells.
                let (n, bg, bd) = window_frame();
                col.spawn((n, bg, bd)).with_children(|p| {
                    spawn_text(p, Role::GridHeader, 14.0, theme::TITLE);
                    p.spawn(Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(4.0),
                        margin: UiRect::top(Val::Px(4.0)),
                        ..default()
                    })
                    .with_children(|grid| {
                        for row in 0..GRID_ROWS {
                            grid.spawn(Node {
                                flex_direction: FlexDirection::Row,
                                column_gap: Val::Px(4.0),
                                ..default()
                            })
                            .with_children(|line| {
                                for col_i in 0..GRID_COLS {
                                    let slot = row * GRID_COLS + col_i;
                                    crate::hud::item_grid::spawn_item_cell(
                                        line,
                                        DeliveryFrame(FrameId::Cell(slot)),
                                        DeliveryIcon(IconId::Cell(slot)),
                                        DeliveryText(Role::CellQty(slot)),
                                        "",
                                        placeholder.clone(),
                                    );
                                }
                            });
                        }
                    });
                });

                // Current Gil line + spinner line.
                let (n, bg, bd) = window_frame();
                col.spawn((n, bg, bd)).with_children(|p| {
                    spawn_text(p, Role::GilLine, 13.0, theme::TEXT);
                });
                let (mut n, bg, bd) = window_frame();
                n.display = Display::None;
                col.spawn((DeliveryFrame(FrameId::SpinnerBox), n, bg, bd))
                    .with_children(|p| {
                        spawn_text(p, Role::SpinnerLine, 15.0, theme::CURSOR);
                    });

                // Button row.
                col.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(6.0),
                    ..default()
                })
                .with_children(|row| {
                    for id in [BtnId::Send, BtnId::Exit, BtnId::Take, BtnId::Reject] {
                        spawn_button(row, id);
                    }
                });

                spawn_text(col, Role::Hint, 11.0, theme::MUTED);
            });

            // Right column: inventory list + detail (outgoing only).
            root.spawn(Node {
                width: Val::Px(230.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|col| {
                let (n, bg, bd) = window_frame();
                col.spawn((DeliveryFrame(FrameId::InvBox), n, bg, bd))
                    .with_children(|p| {
                        spawn_text(p, Role::InvHeader, 13.0, theme::TITLE);
                        for i in 0..INV_LIST_ROWS {
                            spawn_inv_row(p, i, placeholder.clone());
                        }
                    });

                let (n, bg, bd) = window_frame();
                col.spawn((DeliveryFrame(FrameId::DetailBox), n, bg, bd))
                    .with_children(|p| {
                        p.spawn(Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(6.0),
                            ..default()
                        })
                        .with_children(|h| {
                            h.spawn((
                                DeliveryIcon(IconId::Detail),
                                Node {
                                    width: Val::Px(DETAIL_ICON_PX),
                                    height: Val::Px(DETAIL_ICON_PX),
                                    display: Display::None,
                                    ..default()
                                },
                                ImageNode::new(placeholder.clone()),
                            ));
                            h.spawn((
                                DeliveryText(Role::DetailName),
                                Text::new(""),
                                text_font(14.0),
                                TextColor(theme::TITLE),
                            ));
                        });
                        for i in 0..DETAIL_ROWS {
                            spawn_row_hidden(p, Role::DetailRow(i), 12.0, theme::TEXT);
                        }
                    });
            });
        });
}

fn spawn_text(p: &mut ChildSpawnerCommands, role: Role, size: f32, color: Color) {
    p.spawn((
        DeliveryText(role),
        Text::new(""),
        text_font(size),
        TextColor(color),
    ));
}

fn spawn_row_hidden(p: &mut ChildSpawnerCommands, role: Role, size: f32, color: Color) {
    p.spawn((
        DeliveryText(role),
        Text::new(""),
        text_font(size),
        TextColor(color),
        Node {
            display: Display::None,
            ..default()
        },
    ));
}

fn spawn_button(p: &mut ChildSpawnerCommands, id: BtnId) {
    p.spawn((
        DeliveryFrame(FrameId::Button(id)),
        Node {
            padding: UiRect::axes(Val::Px(8.0), Val::Px(3.0)),
            border: UiRect::all(Val::Px(1.0)),
            display: Display::None,
            ..default()
        },
        BackgroundColor(theme::CELL_BG),
        BorderColor::all(theme::CELL_EDGE),
    ))
    .with_children(|b| {
        b.spawn((
            DeliveryText(Role::Button(id)),
            Text::new(id.caption()),
            text_font(13.0),
            TextColor(theme::TEXT),
        ));
    });
}

fn spawn_inv_row(p: &mut ChildSpawnerCommands, i: usize, placeholder: Handle<Image>) {
    p.spawn((
        DeliveryFrame(FrameId::InvRow(i)),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(4.0),
            display: Display::None,
            ..default()
        },
    ))
    .with_children(|row| {
        row.spawn((
            DeliveryIcon(IconId::Inv(i)),
            Node {
                width: Val::Px(INV_ICON_PX),
                height: Val::Px(INV_ICON_PX),
                display: Display::None,
                ..default()
            },
            ImageNode::new(placeholder),
        ));
        row.spawn((
            DeliveryText(Role::InvRow(i)),
            Text::new(""),
            text_font(12.0),
            TextColor(theme::TEXT),
        ));
    });
}

const FLAG_RARE: u16 = 0x8000;
const FLAG_EX: u16 = 0x4000;

/// Rebuild the deliverable inventory list only when the snapshot changes.
pub(crate) fn rebuild_delivery_inventory(
    state: Res<SceneState>,
    dat_root: Res<ItemDatRoot>,
    mut icon_cache: ResMut<ItemIconCache>,
    mut inv: ResMut<DeliveryInventory>,
) {
    if !state.is_changed() {
        return;
    }
    let snap = &state.snapshot;
    if snap.delivery_box.is_none() {
        if !inv.rows.is_empty() {
            inv.rows.clear();
        }
        return;
    }
    let table = icon_cache.table(&dat_root);
    let ex_rare = |item_no: u16| -> bool {
        table
            .as_ref()
            .and_then(|t| crate::hud::item_detail::lookup_static(t, item_no))
            .map(|s| s.flags & (FLAG_RARE | FLAG_EX) != 0)
            .unwrap_or(false)
    };
    inv.rows = build_inventory(snap, |id| ffxi_proto::item_flags::deliverable(id), ex_rare);
}

fn recipient_value_text(d: &DeliveryBoxState, editing: Option<&String>) -> String {
    if let Some(buf) = editing {
        return format!("{buf}_");
    }
    match &d.recipient_status {
        RecipientStatus::Unset => d
            .recipient
            .clone()
            .unwrap_or_else(|| "(not specified)".to_string()),
        RecipientStatus::Pending => "(checking…)".to_string(),
        RecipientStatus::Ok { .. } => d.recipient.clone().unwrap_or_default(),
        RecipientStatus::NoSuchChar => "(no such character)".to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_delivery_screen(
    state: Res<SceneState>,
    screen: Res<DeliveryScreenState>,
    inv: Res<DeliveryInventory>,
    dat_root: Res<ItemDatRoot>,
    mut icon_cache: ResMut<ItemIconCache>,
    mut images: ResMut<Assets<Image>>,
    mut root_q: Query<
        &mut Node,
        (
            With<DeliveryScreenRoot>,
            Without<DeliveryText>,
            Without<DeliveryIcon>,
            Without<DeliveryFrame>,
        ),
    >,
    mut text_q: Query<
        (&DeliveryText, &mut Text, &mut TextColor, &mut Node),
        (
            Without<DeliveryScreenRoot>,
            Without<DeliveryIcon>,
            Without<DeliveryFrame>,
        ),
    >,
    mut icon_q: Query<
        (&DeliveryIcon, &mut Node, &mut ImageNode),
        (
            Without<DeliveryScreenRoot>,
            Without<DeliveryText>,
            Without<DeliveryFrame>,
        ),
    >,
    mut frame_q: Query<
        (
            &DeliveryFrame,
            &mut Node,
            &mut BorderColor,
            &mut BackgroundColor,
        ),
        (
            Without<DeliveryScreenRoot>,
            Without<DeliveryText>,
            Without<DeliveryIcon>,
        ),
    >,
) {
    let snap = &state.snapshot;
    let Some(d) = snap.delivery_box.as_ref() else {
        if let Ok(mut node) = root_q.single_mut() {
            if node.display != Display::None {
                node.display = Display::None;
            }
        }
        return;
    };
    if let Ok(mut node) = root_q.single_mut() {
        if node.display != Display::Flex {
            node.display = Display::Flex;
        }
    }

    let outgoing = d.box_no == DeliveryBoxNo::Outgoing;
    let focus = screen.focus;
    let any_sent = d
        .slots
        .iter()
        .flatten()
        .any(|s| s.stat == ffxi_proto::map::pbx::stat::SENT);
    let gil = current_gil(snap);

    // Detail: the item under focus (inventory row or grid slot).
    let focus_item = match focus {
        DeliveryFocus::InvRow(i) => inv.rows.get(i).map(|r| r.item_no),
        DeliveryFocus::Slot(i) => d.slots.get(i).and_then(|c| c.as_ref()).map(|it| it.item_no),
        _ => None,
    };
    let (detail_name, detail_rows) =
        item_ui::focus_detail(focus_item, snap, &dat_root, &mut icon_cache);

    // Inventory list viewport.
    let total = inv.rows.len();
    let inv_cursor = match focus {
        DeliveryFocus::InvRow(i) => i,
        _ => screen.last_inv_row.min(total.saturating_sub(1)),
    };
    let inv_start = viewport_start(inv_cursor, total, INV_LIST_ROWS);

    // Text nodes.
    for (tag, mut text, mut color, mut node) in text_q.iter_mut() {
        let (s, c, visible) = text_value(
            tag.0,
            d,
            &screen,
            outgoing,
            any_sent,
            gil,
            &inv.rows,
            inv_start,
            inv_cursor,
            &detail_name,
            &detail_rows,
        );
        set_text(&mut text, &s);
        if color.0 != c {
            color.0 = c;
        }
        let want = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }

    // Icons.
    for (tag, mut node, mut image) in icon_q.iter_mut() {
        let item_no = match tag.0 {
            IconId::Cell(i) => d.slots.get(i).and_then(|c| c.as_ref()).map(|it| it.item_no),
            IconId::Inv(i) => inv.rows.get(inv_start + i).map(|r| r.item_no),
            IconId::Detail => focus_item,
        };
        let handle = item_no.and_then(|no| icon_cache.ensure(no, &dat_root, &mut images));
        match handle {
            Some(h) => {
                image.image = h;
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
                // Sent outgoing cells dim.
                if let IconId::Cell(i) = tag.0 {
                    let sent = d
                        .slots
                        .get(i)
                        .and_then(|c| c.as_ref())
                        .map(|it| it.stat == ffxi_proto::map::pbx::stat::SENT)
                        .unwrap_or(false);
                    image.color = if sent {
                        Color::srgba(1.0, 1.0, 1.0, 0.35)
                    } else {
                        Color::WHITE
                    };
                }
            }
            None => {
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
        }
    }

    // Frames: visibility + focus highlight.
    for (tag, mut node, mut border, mut bg) in frame_q.iter_mut() {
        let (visible, focused) = frame_state(
            tag.0,
            outgoing,
            focus,
            screen.selector.is_some(),
            inv_start,
            total,
        );
        let want = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
        let edge = if focused {
            theme::CURSOR
        } else {
            theme::CELL_EDGE
        };
        let want_border = BorderColor::all(edge);
        if *border != want_border {
            *border = want_border;
        }
        let want_bg = if focused && matches!(tag.0, FrameId::Button(_)) {
            theme::CURSOR_BG
        } else {
            theme::CELL_BG
        };
        if matches!(tag.0, FrameId::Button(_)) && bg.0 != want_bg {
            bg.0 = want_bg;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn text_value(
    role: Role,
    d: &DeliveryBoxState,
    screen: &DeliveryScreenState,
    outgoing: bool,
    any_sent: bool,
    gil: u32,
    rows: &[InvRow],
    inv_start: usize,
    inv_cursor: usize,
    detail_name: &str,
    detail_rows: &[String],
) -> (String, Color, bool) {
    match role {
        Role::GridHeader => (
            if outgoing {
                "Deliveries"
            } else {
                "Delivery Box"
            }
            .to_string(),
            theme::TITLE,
            true,
        ),
        Role::RecipientLabel => (
            if any_sent {
                "Recipient (sent)".to_string()
            } else {
                "Recipient".to_string()
            },
            theme::TITLE,
            outgoing,
        ),
        Role::RecipientValue => {
            let editing = screen.recipient_buf.as_ref();
            let color = if matches!(d.recipient_status, RecipientStatus::NoSuchChar) {
                Color::srgb(1.0, 0.5, 0.5)
            } else {
                theme::TEXT
            };
            (recipient_value_text(d, editing), color, outgoing)
        }
        Role::CellQty(i) => {
            let qty = d
                .slots
                .get(i)
                .and_then(|c| c.as_ref())
                .filter(|it| it.quantity > 1)
                .map(|it| it.quantity);
            match qty {
                Some(q) => (q.to_string(), theme::TEXT, true),
                None => (String::new(), theme::MUTED, true),
            }
        }
        Role::GilLine => (
            format!("Current Gil  {gil} G"),
            if matches!(screen.focus, DeliveryFocus::Gil) {
                theme::CURSOR
            } else {
                theme::TEXT
            },
            outgoing,
        ),
        Role::SpinnerLine => match &screen.selector {
            Some(b) => (b.spinner.label(), theme::CURSOR, true),
            None => (String::new(), theme::CURSOR, false),
        },
        Role::DetailName => (detail_name.to_string(), theme::TITLE, outgoing),
        Role::DetailRow(i) => match detail_rows.get(i) {
            Some(line) => (line.clone(), theme::TEXT, outgoing),
            None => (String::new(), theme::TEXT, false),
        },
        Role::InvHeader => ("Items".to_string(), theme::TITLE, outgoing),
        Role::InvRow(i) => {
            let list_idx = inv_start + i;
            match rows.get(list_idx) {
                Some(r) => {
                    let name = ffxi_proto::item_names::lookup(r.item_no)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("Item #{}", r.item_no));
                    let qty = if r.quantity > 1 {
                        format!(" x{}", r.quantity)
                    } else {
                        String::new()
                    };
                    let cursor =
                        matches!(screen.focus, DeliveryFocus::InvRow(_)) && list_idx == inv_cursor;
                    let prefix = if cursor { "> " } else { "  " };
                    let color = if !r.deliverable {
                        theme::MUTED
                    } else if cursor {
                        theme::CURSOR
                    } else {
                        theme::TEXT
                    };
                    (format!("{prefix}{name}{qty}"), color, outgoing)
                }
                None => (String::new(), theme::TEXT, false),
            }
        }
        Role::Button(id) => (id.caption().to_string(), theme::TEXT, false),
        Role::Hint => ("Enter select · Esc close".to_string(), theme::MUTED, true),
    }
}

fn frame_state(
    id: FrameId,
    outgoing: bool,
    focus: DeliveryFocus,
    selector_active: bool,
    inv_start: usize,
    total: usize,
) -> (bool, bool) {
    match id {
        FrameId::Cell(i) => (true, focus == DeliveryFocus::Slot(i)),
        FrameId::RecipientBox => (outgoing, focus == DeliveryFocus::Recipient),
        FrameId::SpinnerBox => (selector_active, false),
        FrameId::InvBox => (outgoing, false),
        FrameId::DetailBox => (outgoing, false),
        FrameId::InvRow(i) => (inv_start + i < total, false),
        FrameId::Button(bid) => {
            let visible = match bid {
                BtnId::Send | BtnId::Exit => outgoing,
                BtnId::Take | BtnId::Reject => !outgoing,
            };
            (visible, focus == bid.focus())
        }
    }
}

fn set_text(text: &mut Text, s: &str) {
    if text.0 != s {
        text.0 = s.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_wire::DeliverySlot;

    fn ctx_out(inv_len: usize, recipient_ok: bool) -> DeliveryCtx {
        DeliveryCtx {
            box_no: DeliveryBoxNo::Outgoing,
            inv_len,
            recipient_ok,
        }
    }

    fn ctx_in() -> DeliveryCtx {
        DeliveryCtx {
            box_no: DeliveryBoxNo::Incoming,
            inv_len: 0,
            recipient_ok: false,
        }
    }

    #[test]
    fn grid_nav_within_4x2() {
        let mut s = DeliveryScreenState {
            active: true,
            focus: DeliveryFocus::Slot(0),
            ..Default::default()
        };
        let ctx = ctx_out(5, true);
        focus_right(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::Slot(1));
        focus_down(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::Slot(5));
        focus_left(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::Slot(4));
        focus_up(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::Slot(0));
    }

    #[test]
    fn top_row_up_reaches_recipient_outgoing() {
        let mut s = DeliveryScreenState {
            active: true,
            focus: DeliveryFocus::Slot(2),
            ..Default::default()
        };
        focus_up(&mut s, &ctx_out(0, false));
        assert_eq!(s.focus, DeliveryFocus::Recipient);
    }

    #[test]
    fn bottom_row_down_reaches_gil_then_send() {
        let mut s = DeliveryScreenState {
            active: true,
            focus: DeliveryFocus::Slot(6),
            ..Default::default()
        };
        let ctx = ctx_out(0, true);
        focus_down(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::Gil);
        focus_down(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::SendOk);
        focus_right(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::Exit);
    }

    #[test]
    fn right_from_grid_edge_enters_inventory_and_left_returns() {
        let mut s = DeliveryScreenState {
            active: true,
            focus: DeliveryFocus::Slot(3), // right edge, row 0
            last_out_slot: 3,
            ..Default::default()
        };
        let ctx = ctx_out(4, true);
        focus_right(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::InvRow(0));
        focus_down(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::InvRow(1));
        // Left returns to the remembered grid slot (historical-on-back).
        focus_left(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::Slot(3));
    }

    #[test]
    fn inventory_scroll_clamps_to_len() {
        let mut s = DeliveryScreenState {
            active: true,
            focus: DeliveryFocus::InvRow(0),
            ..Default::default()
        };
        let ctx = ctx_out(2, true);
        focus_down(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::InvRow(1));
        focus_down(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::InvRow(1), "clamps at last row");
    }

    #[test]
    fn incoming_has_no_recipient_or_gil() {
        let mut s = DeliveryScreenState {
            active: true,
            focus: DeliveryFocus::Slot(1),
            ..Default::default()
        };
        focus_up(&mut s, &ctx_in());
        assert_eq!(
            s.focus,
            DeliveryFocus::Slot(1),
            "top row stays (no recipient)"
        );
        s.focus = DeliveryFocus::Slot(5);
        focus_down(&mut s, &ctx_in());
        assert_eq!(s.focus, DeliveryFocus::Slot(5), "bottom row stays (no gil)");
    }

    #[test]
    fn stage_stackable_opens_spinner_singleton_preconfirmed() {
        let stack = InvRow {
            inv_slot: 3,
            item_no: 4096,
            quantity: 12,
            deliverable: true,
        };
        let b = begin_item_stage(&stack, Some(0)).expect("binding");
        assert_eq!(b.spinner.value, 1, "stackable starts at 1");
        assert_eq!(b.spinner.max, 12);

        let single = InvRow {
            inv_slot: 4,
            item_no: 5000,
            quantity: 1,
            deliverable: true,
        };
        let b = begin_item_stage(&single, Some(2)).expect("binding");
        assert_eq!(b.spinner.confirm(), 1);
        assert!(b.spinner.is_all());
    }

    #[test]
    fn non_deliverable_and_full_box_reject_stage() {
        let ex = InvRow {
            inv_slot: 1,
            item_no: 1,
            quantity: 1,
            deliverable: false,
        };
        assert!(begin_item_stage(&ex, Some(0)).is_none());
        let ok = InvRow {
            deliverable: true,
            ..ex
        };
        assert!(begin_item_stage(&ok, None).is_none(), "no free slot");
    }

    #[test]
    fn gil_stage_binds_slot_zero() {
        let b = begin_gil_stage(17_488, Some(1)).expect("binding");
        assert_eq!(b.spinner.max, 17_488);
        assert_eq!(b.target.inventory_slot(), 0);
        assert_eq!(b.target.out_slot(), 1);
    }

    #[test]
    fn first_free_slot_finds_gap() {
        let mut d = DeliveryBoxState {
            box_no: DeliveryBoxNo::Outgoing,
            slots: vec![None; GRID_SLOTS],
            ..Default::default()
        };
        d.slots[0] = Some(DeliverySlot {
            item_no: 9,
            quantity: 1,
            ..Default::default()
        });
        assert_eq!(first_free_slot(&d), Some(1));
    }
}
