//! Dedicated retail-faithful delivery box screen.
//!
//! Replaces the old dialog+grid delivery UI. One modal window, gated on
//! `SceneSnapshot::delivery_box`, with a 4x2 slot grid, a recipient field, the
//! rich inventory list (reused item detail + icons), the shared numeric picker
//! ([`super::digit_spinner`]) for quantity and gil, and a Current Gil line.
//! Rendering reads the snapshot + [`DeliveryScreenState`]; the native input
//! layer drives focus and emits the delivery `AgentCommand`s.

use bevy::prelude::*;
use kuluu_snapshot::{DeliveryBoxNo, DeliveryBoxState, RecipientStatus, SceneSnapshot};

use crate::hud::digit_spinner::{self, DigitSpinner, SpinnerSlot, SpinnerUnit};
use crate::hud::list_view::{self, ListViewport, LIST_ROWS, ROW_ICON_PX};

/// Retail lays the 8 outgoing/incoming slots out 4 across, 2 down.
pub const GRID_COLS: usize = 4;
pub const GRID_ROWS: usize = 2;
pub const GRID_SLOTS: usize = GRID_COLS * GRID_ROWS;

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
    /// A row in the inventory item list. Reachable only from Enter on an empty
    /// outbox slot (.agents/skills/retail-observe/references/2026-07-17-moghouse-menu.md
    /// "Send flow" step 3), never as a sibling of the grid.
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
    /// The box this state was reset for. The Receive/Send switch keeps the
    /// input mode, so the panels' focus regions would otherwise carry across.
    pub box_no: Option<DeliveryBoxNo>,
    pub focus: DeliveryFocus,
    pub selector: Option<SpinnerBinding>,
    /// `Some` while the recipient text field is being edited.
    pub recipient_buf: Option<String>,
    /// The outbox slot the item list is staging into, set by the Enter that
    /// opened the list. Staging targets this, not whichever slot happens to be
    /// free when the pick lands.
    pub pick_slot: Option<usize>,
    pub viewport: ListViewport,
    /// The dispatch control is armed and waiting for a second, deliberate
    /// press. Sending is irreversible: the parcel leaves the bag at once.
    pub confirm_send: bool,
    pub last_out_slot: usize,
    pub last_in_slot: usize,
    pub last_inv_row: usize,
}

impl DeliveryScreenState {
    /// Retail opens the Send panel on the Recipient field and the Receive panel
    /// on the grid (.agents/skills/retail-observe/references/2026-07-17-moghouse-menu.md
    /// "Send flow" step 1, "Delivery Box receive with pending mail").
    pub fn open(&mut self, box_no: DeliveryBoxNo) {
        *self = DeliveryScreenState {
            box_no: Some(box_no),
            focus: match box_no {
                DeliveryBoxNo::Outgoing => DeliveryFocus::Recipient,
                DeliveryBoxNo::Incoming => DeliveryFocus::Slot(0),
            },
            ..Default::default()
        };
    }

    /// Enter on an empty outbox slot hands the cursor to the item list.
    pub fn enter_item_list(&mut self, slot: usize, inv_len: usize) {
        self.pick_slot = Some(slot);
        self.last_out_slot = slot;
        let row = clamp_row(self.last_inv_row, inv_len);
        self.focus = DeliveryFocus::InvRow(row);
        self.viewport.follow(row, inv_len);
    }

    /// Esc out of the item list, back to the slot it was entered from.
    pub fn leave_item_list(&mut self) {
        if let DeliveryFocus::InvRow(i) = self.focus {
            self.last_inv_row = i;
        }
        self.focus = DeliveryFocus::Slot(clamp_slot(self.pick_slot.take().unwrap_or(0)));
    }

    /// Back to the grid cell the Take/Return buttons were acting on. Both
    /// actions consume the parcel, so staying on the button would leave the
    /// cursor on a control aimed at an empty slot.
    pub fn leave_parcel_actions(&mut self) {
        self.focus = DeliveryFocus::Slot(clamp_slot(self.last_in_slot));
    }

    pub fn close(&mut self) {
        *self = DeliveryScreenState::default();
    }

    /// Keep the cursor on something the panel actually draws after the list
    /// under it changes length.
    pub fn reclamp(&mut self, inv_len: usize) {
        if let DeliveryFocus::InvRow(i) = self.focus {
            if inv_len == 0 {
                self.leave_item_list();
                return;
            }
            let row = clamp_row(i, inv_len);
            self.focus = DeliveryFocus::InvRow(row);
            self.viewport.follow(row, inv_len);
        }
        self.last_inv_row = clamp_row(self.last_inv_row, inv_len);
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
    /// The item DAT's display name, so a list row and the card beside it show
    /// the item by the same name.
    pub name: String,
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

/// Move focus up. Grid top row escapes to Recipient (outgoing); the item list
/// steps a row; buttons walk back toward the grid.
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
        DeliveryFocus::InvRow(i) => step_list(state, i, ctx.inv_len, false),
        DeliveryFocus::Gil if ctx.outgoing() => {
            DeliveryFocus::Slot(GRID_COLS + state.last_out_slot % GRID_COLS)
        }
        DeliveryFocus::SendOk => DeliveryFocus::Gil,
        DeliveryFocus::Exit if ctx.outgoing() => DeliveryFocus::Gil,
        DeliveryFocus::TakeBtn | DeliveryFocus::RejectBtn => {
            DeliveryFocus::Slot(clamp_slot(state.last_in_slot))
        }
        other => other,
    };
}

/// Up/Down inside a list step one row and pull the page along.
fn step_list(
    state: &mut DeliveryScreenState,
    cursor: usize,
    total: usize,
    down: bool,
) -> DeliveryFocus {
    let row = list_view::step_cursor(cursor, total, down);
    state.viewport.follow(row, total);
    DeliveryFocus::InvRow(row)
}

/// Move focus down. Grid bottom row escapes to the Gil line (outgoing); buttons
/// step Gil → Send/Exit.
pub fn focus_down(state: &mut DeliveryScreenState, ctx: &DeliveryCtx) {
    state.remember(ctx.box_no);
    state.focus = match state.focus {
        DeliveryFocus::Recipient => DeliveryFocus::Slot(clamp_slot(state.last_out_slot)),
        DeliveryFocus::Slot(i) if row_of(i) + 1 < GRID_ROWS => DeliveryFocus::Slot(i + GRID_COLS),
        DeliveryFocus::Slot(_) if ctx.outgoing() => DeliveryFocus::Gil,
        DeliveryFocus::InvRow(i) => step_list(state, i, ctx.inv_len, true),
        DeliveryFocus::Gil => DeliveryFocus::SendOk,
        other => other,
    };
}

/// Move focus left: one grid cell, or a page back inside the item list.
/// Arrows stay within one surface: grid or list, not both.
pub fn focus_left(state: &mut DeliveryScreenState, ctx: &DeliveryCtx) {
    state.remember(ctx.box_no);
    state.focus = match state.focus {
        DeliveryFocus::Slot(i) if col_of(i) > 0 => DeliveryFocus::Slot(i - 1),
        DeliveryFocus::InvRow(i) => page_list(state, i, ctx.inv_len, false),
        DeliveryFocus::Exit => DeliveryFocus::SendOk,
        DeliveryFocus::RejectBtn => DeliveryFocus::TakeBtn,
        other => other,
    };
}

/// Move focus right: one grid cell, Send -> Exit, Take -> Return, or a page
/// forward inside the item list.
pub fn focus_right(state: &mut DeliveryScreenState, ctx: &DeliveryCtx) {
    state.remember(ctx.box_no);
    state.focus = match state.focus {
        DeliveryFocus::Slot(i) if col_of(i) + 1 < GRID_COLS => DeliveryFocus::Slot(i + 1),
        DeliveryFocus::InvRow(i) => page_list(state, i, ctx.inv_len, true),
        DeliveryFocus::SendOk => DeliveryFocus::Exit,
        DeliveryFocus::TakeBtn => DeliveryFocus::RejectBtn,
        other => other,
    };
}

/// Left/Right inside a list page it, the way every other retail item window
/// does ([`list_view::page_cursor`]); the cursor stays inside the list.
fn page_list(
    state: &mut DeliveryScreenState,
    cursor: usize,
    total: usize,
    forward: bool,
) -> DeliveryFocus {
    let row = list_view::page_cursor(cursor, total, forward);
    state.viewport.page(forward, total);
    state.viewport.follow(row, total);
    DeliveryFocus::InvRow(row)
}

/// The active digit's tint, which rides a cell's background rather than its
/// text (.agents/skills/retail-observe/references/auction-house.md "Price Set").
fn spinner_slot_bg(screen: &DeliveryScreenState, role: Role) -> Color {
    let (Role::Spinner(slot), Some(b)) = (role, screen.selector.as_ref()) else {
        return Color::NONE;
    };
    digit_spinner::slot_style(&b.spinner, slot, b.target.unit()).2
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

/// What a confirmed amount applies to. Both route to a delivery `Set`: gil is
/// inventory slot 0 (LSB stores gil as item 65535 at LOC_INVENTORY[0]), items
/// use their inventory index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpinnerTarget {
    ItemQty { inv_slot: u8, out_slot: u8 },
    Gil { out_slot: u8 },
}

impl SpinnerTarget {
    /// LOC_INVENTORY index for the `Set` (gil = slot 0).
    pub fn inventory_slot(&self) -> u8 {
        match self {
            SpinnerTarget::ItemQty { inv_slot, .. } => *inv_slot,
            SpinnerTarget::Gil { .. } => 0,
        }
    }

    pub fn out_slot(&self) -> u8 {
        match self {
            SpinnerTarget::ItemQty { out_slot, .. } | SpinnerTarget::Gil { out_slot } => *out_slot,
        }
    }

    pub fn unit(&self) -> SpinnerUnit {
        match self {
            SpinnerTarget::ItemQty { .. } => SpinnerUnit::Count,
            SpinnerTarget::Gil { .. } => SpinnerUnit::Gil,
        }
    }
}

/// An open picker plus what it will stage on confirm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpinnerBinding {
    pub spinner: DigitSpinner,
    pub target: SpinnerTarget,
}

/// Begin staging the inventory item at `row` into `target`, the outbox slot the
/// cursor entered the list from. `None` if the row is not deliverable.
pub fn begin_item_stage(row: &InvRow, target: Option<usize>) -> Option<SpinnerBinding> {
    if !row.deliverable {
        return None;
    }
    let out_slot = target? as u8;
    Some(SpinnerBinding {
        spinner: DigitSpinner::item(row.quantity.max(1)),
        target: SpinnerTarget::ItemQty {
            inv_slot: row.inv_slot,
            out_slot,
        },
    })
}

/// Begin entering a gil amount to send into the first free outbox slot.
pub fn begin_gil_stage(current_gil: u32, first_free: Option<usize>) -> Option<SpinnerBinding> {
    let out_slot = first_free? as u8;
    Some(SpinnerBinding {
        spinner: DigitSpinner::new(current_gil),
        target: SpinnerTarget::Gil { out_slot },
    })
}

/// Build the deliverable inventory rows from a snapshot's LOC_INVENTORY.
/// `deliverable` answers `item_flags::deliverable`; `dat` answers the item DAT
/// for the display name and the EX/RARE bits (both undeliverable). Gil (slot 0
/// / item 65535) and empty slots are skipped.
pub fn build_inventory<F, G>(snap: &SceneSnapshot, deliverable: F, dat: G) -> Vec<InvRow>
where
    F: Fn(u16) -> bool,
    G: Fn(u16) -> (String, bool),
{
    snap.inventory_main()
        .iter()
        .filter(|it| it.index != 0 && it.item_no != GIL_ITEM_NO && it.item_no != 0)
        .map(|it| {
            let (name, ex_rare) = dat(it.item_no);
            InvRow {
                inv_slot: it.index,
                item_no: it.item_no,
                quantity: it.quantity,
                deliverable: !it.locked && deliverable(it.item_no) && !ex_rare,
                name,
            }
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

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::item_ui::{self, transparent_placeholder};
use crate::hud::style::{cursor_prefix, text_font, theme, window_frame};
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

    /// Retail's Receive panel labels the send-back button "Return"
    /// (.agents/skills/retail-observe/references/2026-07-17-moghouse-menu.md); the wire command behind it is
    /// PBX Reject.
    fn caption(self) -> &'static str {
        match self {
            BtnId::Send => "Send",
            BtnId::Exit => "Exit",
            BtnId::Take => "Take",
            BtnId::Reject => "Return",
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
    SenderLine,
    Spinner(SpinnerSlot),
    DetailName,
    DetailRow(usize),
    InvHeader,
    InvRow(usize),
    InvQty(usize),
    Button(BtnId),
    Hint,
    ConfirmPrompt,
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
                                        crate::hud::item_grid::CellOverlay::StackCount,
                                        placeholder.clone(),
                                    );
                                }
                            });
                        }
                    });
                });

                spawn_row_hidden(col, Role::SenderLine, 13.0, theme::TEXT);

                // Current Gil line + spinner line.
                let (n, bg, bd) = window_frame();
                col.spawn((n, bg, bd)).with_children(|p| {
                    spawn_text(p, Role::GilLine, 13.0, theme::TEXT);
                });
                let (mut n, bg, bd) = window_frame();
                n.display = Display::None;
                col.spawn((DeliveryFrame(FrameId::SpinnerBox), n, bg, bd))
                    .with_children(|p| {
                        digit_spinner::spawn_row(p, digit_spinner::slots(), |s| {
                            DeliveryText(Role::Spinner(s))
                        });
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

                spawn_row_hidden(col, Role::ConfirmPrompt, 13.0, theme::CURSOR);
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
                        for i in 0..LIST_ROWS {
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
            display: Display::None,
            ..list_view::row_node()
        },
        BackgroundColor(theme::CELL_BG),
    ))
    .with_children(|row| {
        row.spawn((
            DeliveryIcon(IconId::Inv(i)),
            Node {
                width: Val::Px(ROW_ICON_PX),
                height: Val::Px(ROW_ICON_PX),
                flex_shrink: 0.0,
                display: Display::None,
                ..default()
            },
            ImageNode::new(placeholder),
        ));
        row.spawn(list_view::row_label_clip()).with_children(|col| {
            col.spawn((
                DeliveryText(Role::InvRow(i)),
                Text::new(""),
                text_font(12.0),
                TextColor(theme::TEXT),
                list_view::row_label_layout(),
            ));
        });
        row.spawn((
            DeliveryText(Role::InvQty(i)),
            Text::new(""),
            text_font(12.0),
            TextColor(theme::TEXT),
            Node {
                flex_shrink: 0.0,
                ..default()
            },
        ));
    });
}

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
    let dat = |item_no: u16| -> (String, bool) {
        let s = table
            .as_ref()
            .and_then(|t| crate::hud::item_detail::lookup_static(t, item_no));
        let ex_rare = s
            .as_ref()
            .map(|s| {
                s.flags & (crate::hud::item_detail::flag::RARE | crate::hud::item_detail::flag::EX)
                    != 0
            })
            .unwrap_or(false);
        (
            crate::hud::bazaar_view::item_name(item_no, s.map(|s| s.name)),
            ex_rare,
        )
    };
    let rows = build_inventory(snap, ffxi_vocab::item_flags::deliverable, dat);
    if inv.rows != rows {
        inv.rows = rows;
    }
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
        RecipientStatus::Pending => "(checking...)".to_string(),
        RecipientStatus::Ok { .. } => d.recipient.clone().unwrap_or_default(),
        RecipientStatus::NoSuchChar => "(no such character)".to_string(),
    }
}

/// The inventory list viewport only moves while the cursor is in the list, so
/// a background inventory change does not scroll it under the player.
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
        (
            &DeliveryText,
            &mut Text,
            &mut TextColor,
            &mut Node,
            Option<&mut BackgroundColor>,
        ),
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
        _ => focused_slot(&screen)
            .and_then(|i| d.slots.get(i))
            .and_then(|c| c.as_ref())
            .map(|it| it.item_no),
    };
    let (detail_name, detail_rows) =
        item_ui::focus_detail(focus_item, None, snap, &dat_root, &mut icon_cache);

    let total = inv.rows.len();
    let inv_cursor = match focus {
        DeliveryFocus::InvRow(i) => i,
        _ => screen.last_inv_row.min(total.saturating_sub(1)),
    };
    let inv_start = screen.viewport.start.min(ListViewport::max_start(total));

    // Text nodes.
    for (tag, mut text, mut color, mut node, background) in text_q.iter_mut() {
        if let Some(mut background) = background {
            let want = spinner_slot_bg(&screen, tag.0);
            if background.0 != want {
                background.0 = want;
            }
        }
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
        let tinted = matches!(tag.0, FrameId::Button(_) | FrameId::InvRow(_));
        let want_bg = if focused && tinted {
            theme::CURSOR_BG
        } else {
            theme::CELL_BG
        };
        if tinted && bg.0 != want_bg {
            bg.0 = want_bg;
        }
    }
}

/// Dispatch takes a second, deliberate press: the parcels leave the bag the
/// moment it lands and only the recipient can send them back.
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
                None => (String::new(), theme::MUTED, false),
            }
        }
        // Retail shows Current Gil on both panels, comma-grouped with a " G"
        // suffix (.agents/skills/retail-observe/references/2026-07-17-moghouse-menu.md).
        Role::GilLine => (
            format!("Current Gil  {} G", ffxi_vocab::gil::group_digits(gil)),
            if matches!(screen.focus, DeliveryFocus::Gil) {
                theme::CURSOR
            } else {
                theme::TEXT
            },
            true,
        ),
        // Retail labels the focused parcel's sender under the Receive grid
        // (.agents/skills/retail-observe/references/2026-07-17-moghouse-menu.md,
        // "Delivery Box receive with pending mail").
        Role::SenderLine => {
            let sender = (!outgoing)
                .then(|| focused_slot(screen))
                .flatten()
                .and_then(|i| d.slots.get(i))
                .and_then(|c| c.as_ref())
                .and_then(|it| it.counterpart.as_deref());
            match sender {
                Some(name) => (format!("Sender: {name}"), theme::TEXT, true),
                None => (String::new(), theme::TEXT, false),
            }
        }
        Role::Spinner(slot) => match &screen.selector {
            Some(b) => {
                let (s, c, _) = digit_spinner::slot_style(&b.spinner, slot, b.target.unit());
                (s, c, true)
            }
            None => (String::new(), theme::TEXT, false),
        },
        Role::DetailName => (detail_name.to_string(), theme::TITLE, true),
        Role::DetailRow(i) => match detail_rows.get(i) {
            Some(line) => (line.clone(), theme::TEXT, true),
            None => (String::new(), theme::TEXT, false),
        },
        Role::InvHeader => ("Items".to_string(), theme::TITLE, outgoing),
        Role::InvRow(i) => {
            let list_idx = inv_start + i;
            match rows.get(list_idx) {
                Some(r) => {
                    let cursor =
                        matches!(screen.focus, DeliveryFocus::InvRow(_)) && list_idx == inv_cursor;
                    let prefix = cursor_prefix(cursor);
                    (
                        format!("{prefix}{}", r.name),
                        inv_row_color(r, cursor),
                        outgoing,
                    )
                }
                None => (String::new(), theme::TEXT, false),
            }
        }
        Role::InvQty(i) => {
            let list_idx = inv_start + i;
            match rows.get(list_idx) {
                Some(r) if r.quantity > 1 => {
                    let cursor =
                        matches!(screen.focus, DeliveryFocus::InvRow(_)) && list_idx == inv_cursor;
                    (
                        format!(" x{}", r.quantity),
                        inv_row_color(r, cursor),
                        outgoing,
                    )
                }
                _ => (String::new(), theme::TEXT, false),
            }
        }
        Role::Button(id) => (id.caption().to_string(), theme::TEXT, true),
        Role::Hint => (
            if screen.confirm_send {
                "Enter send | Esc cancel".to_string()
            } else {
                "Enter select | Esc back".to_string()
            },
            theme::MUTED,
            true,
        ),
        Role::ConfirmPrompt => {
            let staged = d
                .slots
                .iter()
                .flatten()
                .filter(|it| it.stat != ffxi_proto::map::pbx::stat::SENT)
                .count();
            if screen.confirm_send && staged > 0 {
                let name = d.recipient.as_deref().unwrap_or("");
                let plural = if staged == 1 { "item" } else { "items" };
                (
                    format!("Send {staged} {plural} to {name}?"),
                    theme::CURSOR,
                    true,
                )
            } else {
                (String::new(), theme::CURSOR, false)
            }
        }
    }
}

fn inv_row_color(row: &InvRow, cursor: bool) -> Color {
    if !row.deliverable {
        theme::MUTED
    } else if cursor {
        theme::CURSOR
    } else {
        theme::TEXT
    }
}

/// The grid cell the panel is acting on: the focused cell, or the cell the
/// focused action button acts on.
fn focused_slot(screen: &DeliveryScreenState) -> Option<usize> {
    match screen.focus {
        DeliveryFocus::Slot(i) => Some(i),
        DeliveryFocus::TakeBtn | DeliveryFocus::RejectBtn => Some(screen.last_in_slot),
        _ => None,
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
        FrameId::DetailBox => (true, false),
        FrameId::InvRow(i) => (
            inv_start + i < total,
            focus == DeliveryFocus::InvRow(inv_start + i),
        ),
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
    use kuluu_snapshot::DeliverySlot;

    /// The picker the player actually sees: spawn the panel, open a stack
    /// quantity on it, run the real update system, and read the drawn row back
    /// off the entities. Covers the spawn/marker/update wiring that a direct
    /// `slot_style` call cannot.
    #[test]
    fn the_spawned_panel_draws_the_shared_quantity_row() {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        app.init_resource::<ItemDatRoot>();
        app.init_resource::<ItemIconCache>();
        app.init_resource::<DeliveryInventory>();

        let mut screen = DeliveryScreenState::default();
        screen.open(DeliveryBoxNo::Outgoing);
        screen.selector = Some(SpinnerBinding {
            spinner: DigitSpinner::item(12),
            target: SpinnerTarget::ItemQty {
                inv_slot: 1,
                out_slot: 0,
            },
        });
        app.insert_resource(screen);
        app.insert_resource(SceneState {
            snapshot: SceneSnapshot {
                delivery_box: Some(DeliveryBoxState {
                    box_no: DeliveryBoxNo::Outgoing,
                    slots: vec![None; GRID_SLOTS],
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        });

        app.add_systems(Startup, spawn_delivery_screen);
        app.add_systems(Update, update_delivery_screen);
        app.update();

        let drawn = drawn_spinner_row(&mut app);
        assert_eq!(drawn, "All \u{25c4} 1/12 \u{25ba}");
    }

    /// The same panel, same nodes, showing a gil amount: only the bound target
    /// changes, and the unit follows it.
    #[test]
    fn the_same_row_draws_a_gil_amount_with_its_unit() {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        app.init_resource::<ItemDatRoot>();
        app.init_resource::<ItemIconCache>();
        app.init_resource::<DeliveryInventory>();

        let mut screen = DeliveryScreenState::default();
        screen.open(DeliveryBoxNo::Outgoing);
        let mut spinner = DigitSpinner::new(17_488);
        for _ in 0..3 {
            spinner.left();
        }
        for _ in 0..9 {
            spinner.up();
        }
        screen.selector = Some(SpinnerBinding {
            spinner,
            target: SpinnerTarget::Gil { out_slot: 0 },
        });
        app.insert_resource(screen);
        app.insert_resource(SceneState {
            snapshot: SceneSnapshot {
                delivery_box: Some(DeliveryBoxState {
                    box_no: DeliveryBoxNo::Outgoing,
                    slots: vec![None; GRID_SLOTS],
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        });

        app.add_systems(Startup, spawn_delivery_screen);
        app.add_systems(Update, update_delivery_screen);
        app.update();

        assert_eq!(
            drawn_spinner_row(&mut app),
            "All \u{25c4} 9,000/17,488 G \u{25ba}"
        );
    }

    /// Concatenate the panel's spinner cells in draw order.
    fn drawn_spinner_row(app: &mut App) -> String {
        let mut cells: Vec<(usize, String)> = app
            .world_mut()
            .query::<(&DeliveryText, &Text)>()
            .iter(app.world())
            .filter_map(|(tag, text)| match tag.0 {
                Role::Spinner(slot) => Some((slot_order(slot), text.0.clone())),
                _ => None,
            })
            .collect();
        cells.sort_by_key(|(order, _)| *order);
        cells.into_iter().map(|(_, s)| s).collect()
    }

    fn slot_order(slot: SpinnerSlot) -> usize {
        digit_spinner::slots()
            .position(|s| s == slot)
            .expect("slot is drawn by this panel")
    }

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

    fn button_text(id: BtnId) -> (String, bool) {
        let state = DeliveryBoxState {
            box_no: DeliveryBoxNo::Incoming,
            slots: vec![None; ffxi_proto::map::pbx::SLOT_COUNT],
            ..Default::default()
        };
        let (text, _, visible) = text_value(
            Role::Button(id),
            &state,
            &DeliveryScreenState::default(),
            false,
            false,
            0,
            &[],
            0,
            0,
            "",
            &[],
        );
        (text, visible)
    }

    /// Same empty-box cause as the buttons: the Current Gil frame is spawned
    /// for both panels, so its text has to render on the Receive panel too.
    #[test]
    fn current_gil_line_renders_on_the_receive_panel() {
        let state = DeliveryBoxState {
            box_no: DeliveryBoxNo::Incoming,
            slots: vec![None; ffxi_proto::map::pbx::SLOT_COUNT],
            ..Default::default()
        };
        let (text, _, visible) = text_value(
            Role::GilLine,
            &state,
            &DeliveryScreenState::default(),
            false,
            false,
            80_146,
            &[],
            0,
            0,
            "",
            &[],
        );
        assert_eq!(text, "Current Gil  80,146 G");
        assert!(visible);
    }

    /// The action buttons rendered as empty bordered boxes: the frame showed
    /// but its caption was pinned to `Display::None`.
    #[test]
    fn action_buttons_show_their_retail_captions() {
        assert_eq!(button_text(BtnId::Take), ("Take".to_string(), true));
        assert_eq!(button_text(BtnId::Reject), ("Return".to_string(), true));
        assert_eq!(button_text(BtnId::Send), ("Send".to_string(), true));
        assert_eq!(button_text(BtnId::Exit), ("Exit".to_string(), true));
    }

    #[test]
    fn grid_nav_within_4x2() {
        let mut s = DeliveryScreenState {
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
            focus: DeliveryFocus::Slot(2),
            ..Default::default()
        };
        focus_up(&mut s, &ctx_out(0, false));
        assert_eq!(s.focus, DeliveryFocus::Recipient);
    }

    #[test]
    fn bottom_row_down_reaches_gil_then_send() {
        let mut s = DeliveryScreenState {
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

    /// Retail activates the item list from Enter on an empty slot, not as a
    /// sibling region of the grid (.agents/skills/retail-observe/references/
    /// 2026-07-17-moghouse-menu.md "Send flow" step 3).
    #[test]
    fn arrows_never_cross_between_the_grid_and_the_item_list() {
        let ctx = ctx_out(20, true);
        for start in [
            DeliveryFocus::Slot(3),
            DeliveryFocus::Slot(7),
            DeliveryFocus::Recipient,
            DeliveryFocus::Gil,
        ] {
            for step in [focus_left, focus_right, focus_up, focus_down] {
                let mut s = DeliveryScreenState {
                    focus: start,
                    ..Default::default()
                };
                step(&mut s, &ctx);
                assert!(
                    !matches!(s.focus, DeliveryFocus::InvRow(_)),
                    "{start:?} reached the item list with an arrow key"
                );
            }
        }

        let mut s = DeliveryScreenState {
            focus: DeliveryFocus::InvRow(4),
            pick_slot: Some(2),
            ..Default::default()
        };
        for step in [focus_left, focus_right, focus_up, focus_down] {
            step(&mut s, &ctx);
            assert!(
                matches!(s.focus, DeliveryFocus::InvRow(_)),
                "an arrow key left the item list"
            );
        }
    }

    /// Enter on an empty slot carries that slot into the list, and Esc takes
    /// the cursor back to it.
    #[test]
    fn the_item_list_remembers_the_slot_it_was_entered_from() {
        let mut s = DeliveryScreenState::default();
        s.open(DeliveryBoxNo::Outgoing);
        assert_eq!(
            s.focus,
            DeliveryFocus::Recipient,
            "retail opens on the name"
        );

        s.enter_item_list(5, 20);
        assert_eq!(s.focus, DeliveryFocus::InvRow(0));
        assert_eq!(s.pick_slot, Some(5));

        focus_down(&mut s, &ctx_out(20, true));
        s.leave_item_list();
        assert_eq!(s.focus, DeliveryFocus::Slot(5));
        assert_eq!(s.pick_slot, None);
        assert_eq!(s.last_inv_row, 1, "the row it was left on is remembered");
    }

    /// The Receive panel opens on the grid, where the parcel is.
    #[test]
    fn the_receive_panel_opens_on_the_grid() {
        let mut s = DeliveryScreenState::default();
        s.open(DeliveryBoxNo::Incoming);
        assert_eq!(s.focus, DeliveryFocus::Slot(0));
        assert_eq!(s.box_no, Some(DeliveryBoxNo::Incoming));
    }

    #[test]
    fn inventory_scroll_clamps_to_len() {
        let mut s = DeliveryScreenState {
            focus: DeliveryFocus::InvRow(0),
            ..Default::default()
        };
        let ctx = ctx_out(2, true);
        focus_down(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::InvRow(1));
        focus_down(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::InvRow(1), "clamps at last row");
    }

    /// Left/Right page the list the way every other retail item window does.
    #[test]
    fn left_right_page_the_item_list() {
        let total = LIST_ROWS * 3;
        let mut s = DeliveryScreenState {
            focus: DeliveryFocus::InvRow(0),
            ..Default::default()
        };
        let ctx = ctx_out(total, true);
        focus_right(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::InvRow(LIST_ROWS));
        assert_eq!(s.viewport.start, LIST_ROWS);
        focus_left(&mut s, &ctx);
        assert_eq!(s.focus, DeliveryFocus::InvRow(0));
        assert_eq!(s.viewport.start, 0);
    }

    /// Staging the last row leaves no cursor pointing past the shrunken list.
    #[test]
    fn a_shrinking_list_pulls_the_cursor_back() {
        let mut s = DeliveryScreenState {
            focus: DeliveryFocus::InvRow(9),
            pick_slot: Some(1),
            last_inv_row: 9,
            ..Default::default()
        };
        s.reclamp(5);
        assert_eq!(s.focus, DeliveryFocus::InvRow(4));
        s.reclamp(0);
        assert_eq!(
            s.focus,
            DeliveryFocus::Slot(1),
            "an empty list is not a list"
        );
    }

    /// The action buttons are an arrow-key dead end otherwise: Enter is the
    /// only way in and Esc the only way out.
    #[test]
    fn receive_buttons_walk_back_to_the_grid() {
        let mut s = DeliveryScreenState {
            focus: DeliveryFocus::TakeBtn,
            last_in_slot: 3,
            ..Default::default()
        };
        focus_up(&mut s, &ctx_in());
        assert_eq!(s.focus, DeliveryFocus::Slot(3));
    }

    #[test]
    fn incoming_has_no_recipient_or_gil() {
        let mut s = DeliveryScreenState {
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
    fn a_stack_opens_at_one_and_a_singleton_is_already_the_whole_stack() {
        let stack = InvRow {
            inv_slot: 3,
            item_no: 4096,
            quantity: 12,
            deliverable: true,
            name: String::new(),
        };
        let b = begin_item_stage(&stack, Some(0)).expect("binding");
        assert_eq!(b.spinner.value, 1, "stackable starts at 1");
        assert_eq!(b.spinner.cap, 12);

        let single = InvRow {
            inv_slot: 4,
            item_no: 5000,
            quantity: 1,
            deliverable: true,
            name: String::new(),
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
            name: String::new(),
        };
        assert!(begin_item_stage(&ex, Some(0)).is_none());
        let ok = InvRow {
            deliverable: true,
            ..ex
        };
        assert!(begin_item_stage(&ok, None).is_none(), "no target slot");
    }

    #[test]
    fn gil_stage_binds_slot_zero() {
        let b = begin_gil_stage(17_488, Some(1)).expect("binding");
        assert_eq!(b.spinner.cap, 17_488);
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
