//! The Mog House arrange editor: entering and leaving layout mode, the browse and move states, the
//! `rmlo*` piece menus, the shelf picker and furniture list, put away, the camera, the moving overlay,
//! sounds and messages (.agents/skills/retail-observe/references/2026-10-15-mog-house-arrange.md
//! Part B). The editor lives in [`DynamicMenu::arrange`] so the input layer and the menu seams reach
//! it; [`systems`] keeps it in step with the drawn pieces and draws what it decides.

use std::collections::{BTreeMap, BTreeSet};

use bevy::input::keyboard::KeyCode;
use ffxi_dat::furniture::{
    self as law, FurnitureAttr, FurniturePose, FurnitureProperty, KIND_FLOWERPOT, KIND_MANNEQUIN,
};
use ffxi_dat::furniture_arrange::{
    self as arrange, ArrangePiece, CameraRig, CameraView, ColorBytes, MoveSession, OverlayQuad,
    Press, Pulse, Room, Turned, PULSE_ALPHA, PULSE_ALPHA_MANNEQUIN, TINT_DIMMED, TINT_NORMAL,
};
use ffxi_proto::map::container::{LOC_MOGSAFE, LOC_MOGSAFE2};
use kuluu_snapshot::MYROOM_MODE_LAYOUT;

use crate::hud::gettext::{
    GetText, GetTextTables, GETTEXT_TABLE_12, GETTEXT_TABLE_CAPTIONS,
    GETTEXT_TABLE_CLIENT_MESSAGES, GETTEXT_TABLE_SHORT_CAPTIONS,
};
use crate::hud::menu::{
    DynamicMenu, DynamicMenuAction, DynamicMenuRow, MenuRowTint, UiSound,
    CONFIG_OPTION_LAYOUT_CONTROL_TYPE, LAYOUT_CONTROL_TYPE1,
};
use crate::hud::unity_fp::{client_message_fallback, CONFIRM_ITEM_YES};
use crate::input_mode::{
    ConfirmDefault, CursorMemory, InputMode, MenuKind, MenuLevel, MenuStack, Pane,
};

#[cfg(not(target_arch = "wasm32"))]
pub mod systems;

pub const RMLO1_WINDOW: &str = "rmlo1";
pub const RMLO2_WINDOW: &str = "rmlo2";
pub const RMLO4_WINDOW: &str = "rmlo4";
pub const BANKMENU_WINDOW: &str = "bankmenu";
pub const INVENTOR_WINDOW: &str = "inventor";
pub const COMYN_WINDOW: &str = "comyn";
/// The window records the editor reads its item captions from.
pub const ARRANGE_WINDOWS: [&str; 5] = [
    RMLO1_WINDOW,
    RMLO2_WINDOW,
    RMLO4_WINDOW,
    BANKMENU_WINDOW,
    COMYN_WINDOW,
];

/// `rmlo1` item ids.
pub const RMLO1_PLACE: u8 = 1;
pub const RMLO1_PUT_AWAY_ALL: u8 = 2;
pub const RMLO1_FINISH: u8 = 3;
const RMLO1_ITEMS: u8 = RMLO1_FINISH;
/// `rmlo2` item ids.
pub const RMLO2_MOVE: u8 = 1;
pub const RMLO2_PUT_AWAY: u8 = 2;
pub const RMLO2_PLACE: u8 = 3;
pub const RMLO2_PUT_AWAY_ALL: u8 = 4;
const RMLO2_ITEMS: u8 = RMLO2_PUT_AWAY_ALL;
/// `rmlo4` item ids.
pub const RMLO4_DRESS: u8 = 1;
pub const RMLO4_MOVE: u8 = 2;
pub const RMLO4_PUT_AWAY: u8 = 3;
pub const RMLO4_PLACE: u8 = 4;
pub const RMLO4_PUT_AWAY_ALL: u8 = 5;
const RMLO4_ITEMS: u8 = RMLO4_PUT_AWAY_ALL;
/// `bankmenu` item ids: the first and the second shelf.
pub const BANKMENU_SHELF: u8 = 1;
pub const BANKMENU_SHELF2: u8 = 2;
const BANKMENU_ITEMS: u8 = BANKMENU_SHELF2;
const COMYN_ITEMS: u8 = crate::hud::unity_fp::CONFIRM_ITEM_NO;

/// The help bar while arranging: title GetText(13, 4), help GetText(14, 14).
pub const HELP_TITLE_ID: u16 = 4;
pub const HELP_TEXT_ID: u16 = 14;
/// The put-away questions: a planted pot GetText(12, 53), put away all GetText(14, 615).
pub const PLANTED_PROMPT_ID: u16 = 53;
pub const PUT_AWAY_ALL_PROMPT_ID: u16 = 615;
/// System messages: layout mode started, the flowerpot limit, the mannequin limit.
pub const SYSMES_LAYOUT_STARTED: usize = 54;
pub const SYSMES_FLOWERPOT_LIMIT: usize = 98;
pub const SYSMES_MANNEQUIN_LIMIT: usize = 99;

/// One arrange window: a DAT record the editor opens, with what its open-time state decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrangeWindow {
    /// `rmlo1`, opened when nothing is placed on this floor.
    Empty { put_away_all: bool },
    /// `rmlo2`, or `rmlo4` for a mannequin, for the selected piece. The flags are whether put away and
    /// put away all were enabled when it opened.
    Piece {
        mannequin: bool,
        put_away: bool,
        put_away_all: bool,
    },
    /// `bankmenu`: the shelf to place from; the second shelf greys while it is locked.
    Shelf { second_shelf: bool },
    /// `inventor` listing one shelf in furniture mode.
    Furniture { container: u8 },
    /// `comyn` asking one put-away question.
    Confirm(ArrangeConfirm),
}

/// The put-away questions `comyn` asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrangeConfirm {
    /// A planted pot is on the piece being put away; `keep_window` is `rmlo4`'s stay-open put away.
    Planted { keep_window: bool },
    /// Put everything away.
    All,
    /// A planted pot is among everything being put away.
    PlantedAll,
}

impl ArrangeWindow {
    pub fn record_name(self) -> &'static str {
        match self {
            Self::Empty { .. } => RMLO1_WINDOW,
            Self::Piece {
                mannequin: false, ..
            } => RMLO2_WINDOW,
            Self::Piece {
                mannequin: true, ..
            } => RMLO4_WINDOW,
            Self::Shelf { .. } => BANKMENU_WINDOW,
            Self::Furniture { .. } => INVENTOR_WINDOW,
            Self::Confirm(_) => COMYN_WINDOW,
        }
    }

    /// Item ids 1..=count of a fixed-item window; the furniture list lists records instead.
    fn item_count(self) -> u8 {
        match self {
            Self::Empty { .. } => RMLO1_ITEMS,
            Self::Piece {
                mannequin: false, ..
            } => RMLO2_ITEMS,
            Self::Piece {
                mannequin: true, ..
            } => RMLO4_ITEMS,
            Self::Shelf { .. } => BANKMENU_ITEMS,
            Self::Confirm(_) => COMYN_ITEMS,
            Self::Furniture { .. } => 0,
        }
    }

    /// Whether item `item` draws grey and its select does nothing.
    pub fn item_disabled(self, item: u8) -> bool {
        match self {
            Self::Empty { put_away_all } => item == RMLO1_PUT_AWAY_ALL && !put_away_all,
            Self::Piece {
                mannequin,
                put_away,
                put_away_all,
            } => {
                let (put_away_item, all_item) = if mannequin {
                    (RMLO4_PUT_AWAY, RMLO4_PUT_AWAY_ALL)
                } else {
                    (RMLO2_PUT_AWAY, RMLO2_PUT_AWAY_ALL)
                };
                (item == put_away_item && !put_away) || (item == all_item && !put_away_all)
            }
            Self::Shelf { second_shelf } => item == BANKMENU_SHELF2 && !second_shelf,
            Self::Furniture { .. } | Self::Confirm(_) => false,
        }
    }

    /// Rows are item ids in order, so a row index is its item id less one.
    pub fn row_disabled(self, row: usize) -> bool {
        u8::try_from(row + 1).is_ok_and(|item| self.item_disabled(item))
    }

    /// The `rmlo*` windows keep the cursor they closed on; the others open fresh.
    fn remembers_cursor(self) -> bool {
        matches!(self, Self::Empty { .. } | Self::Piece { .. })
    }
}

/// A row of an arrange window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrangeAction {
    /// DAT item `item` of an `rmlo*`, `bankmenu` or `comyn` window.
    Item { item: u8 },
    /// A furniture list record.
    Furniture {
        container: u8,
        slot: u8,
        item_no: u16,
    },
}

/// What the editor sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrangeRequest {
    /// c2s 0x0FA placing one piece.
    Layout {
        item_no: u16,
        slot: u8,
        container: u8,
        floor: u8,
        col: i8,
        level: i8,
        row: i8,
        rot: i8,
    },
    /// c2s 0x0FB putting one piece away.
    Bankin {
        item_no: u16,
        slot: u8,
        container: u8,
    },
    /// Back to normal play; the session sends the all-zero c2s 0x0FA.
    Leave,
}

/// The furniture DAT's own sound chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FurnitureSound {
    Step,
    Rotate,
    Placed,
}

/// The arrange key actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrangeKey {
    Arrow(Press),
    Confirm,
    Cancel,
    CycleView,
    Rotate,
    Ctrl,
    Alt,
}

/// The default keys: the arrows, Enter or keypad Enter, Esc, keypad 5 and keypad minus; Ctrl and Alt
/// are tracked because no action answers while either is held.
pub fn arrange_key_of(key: KeyCode) -> Option<ArrangeKey> {
    Some(match key {
        KeyCode::ArrowUp => ArrangeKey::Arrow(Press::Up),
        KeyCode::ArrowRight => ArrangeKey::Arrow(Press::Right),
        KeyCode::ArrowDown => ArrangeKey::Arrow(Press::Down),
        KeyCode::ArrowLeft => ArrangeKey::Arrow(Press::Left),
        KeyCode::Enter | KeyCode::NumpadEnter => ArrangeKey::Confirm,
        KeyCode::Escape => ArrangeKey::Cancel,
        KeyCode::Numpad5 => ArrangeKey::CycleView,
        KeyCode::NumpadSubtract => ArrangeKey::Rotate,
        KeyCode::ControlLeft | KeyCode::ControlRight => ArrangeKey::Ctrl,
        KeyCode::AltLeft | KeyCode::AltRight => ArrangeKey::Alt,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPhase {
    Press,
    Repeat,
    Release,
}

impl KeyPhase {
    pub fn of(pressed: bool, repeat: bool) -> Self {
        match (pressed, repeat) {
            (false, _) => Self::Release,
            (true, false) => Self::Press,
            (true, true) => Self::Repeat,
        }
    }
}

/// The house the editor arranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HouseKey {
    pub zone_generation: u64,
    pub floor: u8,
    pub map_number: u16,
}

/// What the editor keeps about a piece besides its pose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PieceInfo {
    pub container: u8,
    pub slot: u8,
    pub item_no: u16,
    pub kind: u8,
    /// Attr byte 0 is above zero (a planted pot).
    pub planted: bool,
    /// Storage slots it adds while placed.
    pub storage: u32,
    /// The pose its shelf record's Attr gives.
    pub recorded: Option<FurniturePose>,
}

/// A drawn piece as the scene holds it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawnPiece {
    pub seat: usize,
    pub info: PieceInfo,
    pub property: FurnitureProperty,
    pub pose: FurniturePose,
}

/// A shelf record the furniture list can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShelfEntry {
    pub container: u8,
    pub slot: u8,
    pub item_no: u16,
    pub kind: u8,
    pub storage: u32,
    pub attr: FurnitureAttr,
    pub property: Option<FurnitureProperty>,
}

/// Storage (container 2): free slots and list entries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageGauge {
    pub free: i64,
    pub entries: usize,
}

/// A piece the editor started moving before its record is installed; the scene makes its object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewPiece {
    pub container: u8,
    pub slot: u8,
    pub item_no: u16,
    pub kind: u8,
    pub attr: FurnitureAttr,
    pub pose: FurniturePose,
}

/// Everything one update of the editor reads from the scene.
#[derive(Debug, Clone, Default)]
pub struct ArrangeFrame {
    pub house: Option<HouseKey>,
    pub arrangeable: bool,
    /// The session's arrangement mode ([`kuluu_snapshot::SceneSnapshot::myroom_mode`]).
    pub myroom_mode: u8,
    /// An event holds the player.
    pub busy: bool,
    pub drawn: Vec<DrawnPiece>,
    pub shelf: Vec<ShelfEntry>,
    pub storage: StorageGauge,
    pub second_shelf_open: bool,
    /// Frame ticks elapsed since the last update.
    pub frame_ticks: f32,
}

/// DAT text the editor shows, copied from the install.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArrangeTexts {
    pub title: Option<String>,
    pub help: Option<String>,
    pub planted_prompt: Option<String>,
    pub put_away_all_prompt: Option<String>,
    /// The client message catalog (GetText table 7).
    pub client_messages: GetTextTables,
    /// System messages by id.
    pub system_messages: BTreeMap<usize, String>,
    /// Item captions by (window record, item id).
    pub item_labels: BTreeMap<(&'static str, u8), String>,
}

impl ArrangeTexts {
    pub fn client_message(&self, id: u16) -> String {
        self.client_messages
            .get_text(GETTEXT_TABLE_CLIENT_MESSAGES, id)
            .map(str::to_string)
            .unwrap_or_else(|| client_message_fallback(GETTEXT_TABLE_CLIENT_MESSAGES, id))
    }

    pub fn system_message(&self, id: usize) -> String {
        self.system_messages
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("[system message {id}]"))
    }

    fn item_label(&self, window: &'static str, item: u8) -> String {
        self.item_labels
            .get(&(window, item))
            .cloned()
            .unwrap_or_default()
    }
}

/// The GetText entries the editor reads, by table.
pub const ARRANGE_GETTEXT: [(u8, u16); 4] = [
    (GETTEXT_TABLE_SHORT_CAPTIONS, HELP_TITLE_ID),
    (GETTEXT_TABLE_CAPTIONS, HELP_TEXT_ID),
    (GETTEXT_TABLE_12, PLANTED_PROMPT_ID),
    (GETTEXT_TABLE_CAPTIONS, PUT_AWAY_ALL_PROMPT_ID),
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Moving {
    session: MoveSession,
    new_piece: bool,
    previous_selection: Option<usize>,
}

/// The arrange editor.
#[derive(Debug, Clone, Default)]
pub struct ArrangeEditor {
    active: bool,
    seen_mode: u8,
    house: Option<HouseKey>,
    room: Option<Room>,
    info: BTreeMap<usize, PieceInfo>,
    selection: Option<usize>,
    moving: Option<Moving>,
    base_tints: BTreeMap<usize, ColorBytes>,
    pulse: Pulse,
    tick_carry: f32,
    view: CameraView,
    view_zone: Option<u64>,
    rig: Option<CameraRig>,
    held: [bool; 4],
    ctrl_held: bool,
    alt_held: bool,
    /// The camera focal length layout mode replaced, restored on leaving.
    pub stored_focal: Option<f32>,
    shelf: Vec<ShelfEntry>,
    storage: StorageGauge,
    second_shelf_open: bool,
    remembered: BTreeMap<&'static str, usize>,
    /// Set on leaving: the scene puts every piece back to its normal tint once.
    pub tints_reset: bool,
    pub requests: Vec<ArrangeRequest>,
    pub furniture_sounds: Vec<FurnitureSound>,
    pub chat_lines: Vec<String>,
    pub spawns: Vec<NewPiece>,
    pub despawns: Vec<usize>,
    pub texts: ArrangeTexts,
}

/// The `myroom` window reopened on leaving, alone: the main menu stays closed.
pub fn myroom_menu() -> InputMode {
    let mut stack = MenuStack::default();
    stack.push(MenuKind::HomeView);
    InputMode::Menu(stack)
}

fn window_level(window: ArrangeWindow, cursor: usize) -> MenuLevel {
    MenuLevel::new(MenuKind::Arrange(window), cursor)
}

fn arrange_window(level: &MenuLevel) -> Option<ArrangeWindow> {
    match level.kind {
        MenuKind::Arrange(window) => Some(window),
        _ => None,
    }
}

fn press_index(press: Press) -> usize {
    Press::ALL
        .iter()
        .position(|p| *p == press)
        .unwrap_or_default()
}

impl ArrangeEditor {
    pub fn active(&self) -> bool {
        self.active
    }

    pub fn selection(&self) -> Option<usize> {
        self.selection
    }

    pub fn moving_seat(&self) -> Option<usize> {
        self.moving.as_ref().map(|moving| moving.session.seat)
    }

    pub fn view(&self) -> CameraView {
        self.view
    }

    pub fn room(&self) -> Option<&Room> {
        self.room.as_ref()
    }

    pub fn house(&self) -> Option<HouseKey> {
        self.house
    }

    /// Help bar title and help while arranging.
    pub fn help_title(&self) -> String {
        self.texts.title.clone().unwrap_or_default()
    }

    pub fn help_text(&self) -> String {
        self.texts.help.clone().unwrap_or_default()
    }

    /// The help line an arrange window shows: a prompt's question, else the arrange help.
    pub fn window_help(&self, window: ArrangeWindow) -> String {
        let prompt = match window {
            ArrangeWindow::Confirm(ArrangeConfirm::All) => &self.texts.put_away_all_prompt,
            ArrangeWindow::Confirm(_) => &self.texts.planted_prompt,
            _ => &self.texts.help,
        };
        prompt.clone().unwrap_or_default()
    }

    /// Outside a Mog House only the session's mode is followed, so the next entry is seen.
    pub fn observe_mode(&mut self, myroom_mode: u8) {
        self.seen_mode = myroom_mode;
    }

    /// One update: follow the session's mode and the house, then the drawn pieces and the pulse.
    /// Returns the input mode entering or leaving switches to.
    pub fn update(&mut self, frame: &ArrangeFrame) -> Option<InputMode> {
        let mut next = None;
        if self.active {
            if frame.house != self.house || !frame.arrangeable {
                self.leave();
                next = Some(InputMode::World);
            } else if frame.busy {
                self.requests.push(ArrangeRequest::Leave);
                self.leave();
                next = Some(InputMode::World);
            } else if self.seen_mode == MYROOM_MODE_LAYOUT
                && frame.myroom_mode != MYROOM_MODE_LAYOUT
            {
                self.leave();
                next = Some(myroom_menu());
            }
        } else if self.seen_mode != MYROOM_MODE_LAYOUT
            && frame.myroom_mode == MYROOM_MODE_LAYOUT
            && frame.arrangeable
        {
            if let Some(house) = frame.house {
                next = Some(self.enter(house, frame));
            }
        }
        self.seen_mode = frame.myroom_mode;
        if self.active {
            self.sync(frame);
            self.tick_carry += frame.frame_ticks.max(0.0);
            let ticks = self.tick_carry.floor();
            self.tick_carry -= ticks;
            self.pulse.advance(ticks as u32);
        }
        next
    }

    /// Entering layout mode: every piece normal, the first piece selected, the system message, and
    /// `rmlo1` when nothing is placed.
    fn enter(&mut self, house: HouseKey, frame: &ArrangeFrame) -> InputMode {
        self.active = true;
        self.house = Some(house);
        self.room = Room::new(house.floor, house.map_number);
        self.info.clear();
        self.base_tints.clear();
        self.moving = None;
        self.rig = None;
        self.release_keys();
        if self.view_zone != Some(house.zone_generation) {
            self.view = CameraView::default();
            self.view_zone = Some(house.zone_generation);
        }
        self.sync(frame);
        self.selection = self.room.as_ref().and_then(Room::first_seat);
        self.pulse = Pulse::default();
        self.chat_lines
            .push(self.texts.system_message(SYSMES_LAYOUT_STARTED));
        if self.room.as_ref().is_none_or(Room::is_empty) {
            self.open(ArrangeWindow::Empty {
                put_away_all: !arrange::put_away_all_greyed(self.storage.entries),
            })
        } else {
            InputMode::Arrange
        }
    }

    /// Leaving layout mode: a move in flight is cancelled, the selection clears and every tint goes
    /// back to normal. Returns the menu that reopens `myroom`.
    fn leave(&mut self) -> InputMode {
        if self.moving.is_some() {
            self.cancel_move();
        }
        self.active = false;
        self.selection = None;
        self.rig = None;
        self.base_tints.clear();
        self.tints_reset = true;
        myroom_menu()
    }

    fn sync(&mut self, frame: &ArrangeFrame) {
        self.shelf = frame.shelf.clone();
        self.storage = frame.storage;
        self.second_shelf_open = frame.second_shelf_open;
        let new_seat = self
            .moving
            .as_ref()
            .filter(|moving| moving.new_piece)
            .map(|moving| moving.session.seat);
        let drawn: BTreeSet<usize> = frame.drawn.iter().map(|piece| piece.seat).collect();
        let gone: Vec<usize> = self
            .room
            .iter()
            .flat_map(Room::pieces)
            .map(|piece| piece.seat)
            .filter(|seat| !drawn.contains(seat) && Some(*seat) != new_seat)
            .collect();
        for seat in gone {
            self.removed(seat);
        }
        let Some(room) = self.room.as_mut() else {
            return;
        };
        let mut arriving: Vec<&DrawnPiece> = frame
            .drawn
            .iter()
            .filter(|piece| room.piece(piece.seat).is_none())
            .collect();
        // Surfaces register before raised pieces, so what stands on a surface is found on it.
        arriving.sort_by_key(|piece| piece.pose.level > 0);
        for piece in arriving {
            room.insert(ArrangePiece {
                seat: piece.seat,
                property: piece.property,
                pose: piece.pose,
            });
        }
        for piece in &frame.drawn {
            self.info.insert(piece.seat, piece.info);
        }
    }

    /// A drawn piece disappeared: the piece nearest the selection's spot (the removed piece's when it
    /// was the selection, the room origin with none) becomes the selection, unless it already is.
    fn removed(&mut self, seat: usize) {
        let Some(room) = self.room.as_mut() else {
            return;
        };
        let reference = self
            .selection
            .and_then(|selected| room.point(selected))
            .map_or((0.0, 0.0), |[x, _, z]| (x, z));
        room.remove(seat);
        let nearest = room.nearest(reference);
        self.info.remove(&seat);
        self.base_tints.remove(&seat);
        if self.moving_seat() == Some(seat) {
            self.moving = None;
        }
        let next = arrange::selection_after_removal(nearest, self.selection);
        if next != self.selection {
            self.select(next);
        }
    }

    /// A selection change restarts the pulse after putting every tint back to normal.
    fn select(&mut self, seat: Option<usize>) {
        self.selection = seat;
        self.set_all_tints(TINT_NORMAL);
        self.pulse = Pulse::default();
    }

    fn set_all_tints(&mut self, tint: ColorBytes) {
        let seats: Vec<usize> = self
            .room
            .iter()
            .flat_map(Room::pieces)
            .map(|piece| piece.seat)
            .collect();
        self.base_tints = seats.into_iter().map(|seat| (seat, tint)).collect();
    }

    fn carried(&self, seat: usize) -> Vec<usize> {
        match &self.moving {
            Some(moving) if moving.session.seat == seat => moving.session.carried.clone(),
            _ => self
                .room
                .as_ref()
                .map(|room| room.occupancy().carried_by(seat).to_vec())
                .unwrap_or_default(),
        }
    }

    /// The tint a piece draws with: the highlighted piece and what stands on it pulse, the rest keep
    /// their base tint.
    pub fn tint_of(&self, seat: usize) -> ColorBytes {
        if !self.active {
            return TINT_NORMAL;
        }
        let highlighted = self.moving_seat().or(self.selection);
        if let Some(main) = highlighted {
            if seat == main {
                let mannequin = self
                    .info
                    .get(&seat)
                    .is_some_and(|info| info.kind == KIND_MANNEQUIN);
                return self.pulse.tint(if mannequin {
                    PULSE_ALPHA_MANNEQUIN
                } else {
                    PULSE_ALPHA
                });
            }
            if self.carried(main).contains(&seat) {
                return self.pulse.tint(PULSE_ALPHA);
            }
        }
        self.base_tints.get(&seat).copied().unwrap_or(TINT_NORMAL)
    }

    /// The pose the editor holds for a piece.
    pub fn pose_of(&self, seat: usize) -> Option<FurniturePose> {
        Some(self.room.as_ref()?.piece(seat)?.pose)
    }

    /// The overlay while a piece moves.
    pub fn overlay(&self) -> Vec<OverlayQuad> {
        match (&self.room, &self.moving) {
            (Some(room), Some(moving)) => room.overlay(&moving.session),
            _ => Vec::new(),
        }
    }

    /// Steps the arrange camera `updates` client updates toward the selection; `start` seeds it from
    /// the camera as it stands when layout mode begins. `None` while not arranging.
    pub fn camera_update(
        &mut self,
        updates: u32,
        start: impl FnOnce() -> CameraRig,
    ) -> Option<CameraRig> {
        if !self.active {
            self.rig = None;
            return None;
        }
        let floor = self.house?.floor;
        let selected = self
            .selection
            .and_then(|seat| self.room.as_ref()?.point(seat));
        let view = self.view;
        let rig = self.rig.get_or_insert_with(start);
        for _ in 0..updates {
            rig.update(selected, view, floor);
        }
        Some(*rig)
    }

    fn held_delta(&self) -> (i8, i8) {
        Press::ALL
            .iter()
            .zip(self.held)
            .filter(|(_, held)| *held)
            .fold((0, 0), |(x, z), (press, _)| {
                let (dx, dz) = press.delta();
                (x + dx, z + dz)
            })
    }

    /// Follows which arrows and modifiers are down. The input layer feeds every arrange key event
    /// here in every input mode, so a key released while a window or chat had the keys is not left
    /// held.
    pub fn track(&mut self, key: ArrangeKey, phase: KeyPhase) {
        let down = phase != KeyPhase::Release;
        match key {
            ArrangeKey::Ctrl => self.ctrl_held = down,
            ArrangeKey::Alt => self.alt_held = down,
            ArrangeKey::Arrow(press) => self.held[press_index(press)] = down,
            _ => {}
        }
    }

    /// Nothing is held: layout mode began, or the window lost the keyboard and its releases never
    /// arrive.
    pub fn release_keys(&mut self) {
        self.held = [false; 4];
        self.ctrl_held = false;
        self.alt_held = false;
    }

    /// One arrange key event. Returns the input mode to switch to.
    pub fn key(
        &mut self,
        key: ArrangeKey,
        phase: KeyPhase,
        layout_type1: bool,
        sounds: &mut Vec<UiSound>,
    ) -> Option<InputMode> {
        self.track(key, phase);
        if matches!(key, ArrangeKey::Ctrl | ArrangeKey::Alt)
            || phase == KeyPhase::Release
            || !self.active
            || self.ctrl_held
            || self.alt_held
        {
            return None;
        }
        let repeat = phase == KeyPhase::Repeat;
        if self.moving.is_some() {
            self.move_key(key, repeat, layout_type1, sounds)
        } else {
            self.browse_key(key, repeat, sounds)
        }
    }

    fn browse_key(
        &mut self,
        key: ArrangeKey,
        repeat: bool,
        sounds: &mut Vec<UiSound>,
    ) -> Option<InputMode> {
        match key {
            ArrangeKey::Arrow(_) => {
                sounds.push(UiSound::Cursor);
                let selected = self.selection?;
                let (mut dx, mut dz) = self.held_delta();
                if self.view.flips_presses() {
                    (dx, dz) = (-dx, -dz);
                }
                let next = self.room.as_ref()?.navigate(selected, (dx, dz))?;
                if next != selected {
                    self.select(Some(next));
                }
                None
            }
            ArrangeKey::Confirm if !repeat => {
                if let Some(selected) = self.selection {
                    sounds.push(UiSound::Confirm);
                    return Some(self.open_piece_menu(selected));
                }
                match self.room.as_ref().and_then(Room::first_seat) {
                    Some(first) => {
                        self.select(Some(first));
                        None
                    }
                    None => {
                        sounds.push(UiSound::Confirm);
                        Some(self.open(ArrangeWindow::Empty {
                            put_away_all: !arrange::put_away_all_greyed(self.storage.entries),
                        }))
                    }
                }
            }
            ArrangeKey::Cancel if !repeat => {
                sounds.push(UiSound::Cancel);
                self.requests.push(ArrangeRequest::Leave);
                Some(self.leave())
            }
            ArrangeKey::CycleView if !repeat && self.selection.is_some() => {
                self.view = self.view.next();
                None
            }
            _ => None,
        }
    }

    fn move_key(
        &mut self,
        key: ArrangeKey,
        repeat: bool,
        layout_type1: bool,
        sounds: &mut Vec<UiSound>,
    ) -> Option<InputMode> {
        let session = self.moving.as_ref()?.session.clone();
        match key {
            ArrangeKey::Arrow(press) => {
                let reversed =
                    layout_type1 && self.rig.is_some_and(|rig| rig.reverses_type1_presses());
                if self.room.as_mut()?.step(&session, press, reversed) {
                    self.furniture_sounds.push(FurnitureSound::Step);
                }
            }
            ArrangeKey::Rotate if !repeat => {
                if self.room.as_mut()?.turn(&session) == Some(Turned::Floor) {
                    self.furniture_sounds.push(FurnitureSound::Rotate);
                }
            }
            ArrangeKey::Confirm if !repeat => self.place(&session, sounds),
            ArrangeKey::Cancel if !repeat => {
                sounds.push(UiSound::Cancel);
                self.cancel_move();
            }
            _ => {}
        }
        None
    }

    /// Place: a valid spot sends c2s 0x0FA for the piece and everything on it and goes back to
    /// browsing with normal tints; anything else is the buzzer.
    fn place(&mut self, session: &MoveSession, sounds: &mut Vec<UiSound>) {
        let Some(room) = self.room.as_mut() else {
            return;
        };
        let Some(seats) = room.place(session) else {
            sounds.push(UiSound::Buzzer);
            return;
        };
        for seat in seats {
            let (Some(piece), Some(info)) = (room.piece(seat), self.info.get(&seat)) else {
                continue;
            };
            self.requests.push(ArrangeRequest::Layout {
                item_no: info.item_no,
                slot: info.slot,
                container: info.container,
                floor: room.floor,
                col: piece.pose.col,
                level: piece.pose.level,
                row: piece.pose.row,
                rot: piece.pose.rot,
            });
        }
        self.furniture_sounds.push(FurnitureSound::Placed);
        self.moving = None;
        self.set_all_tints(TINT_NORMAL);
    }

    /// Cancel: a placed piece and its load return to their records' poses; a new piece is discarded
    /// and the other pieces stay dimmed until the next selection change.
    fn cancel_move(&mut self) {
        let Some(moving) = self.moving.take() else {
            return;
        };
        let Some(room) = self.room.as_mut() else {
            return;
        };
        let seat = moving.session.seat;
        if moving.new_piece {
            room.remove(seat);
            self.info.remove(&seat);
            self.base_tints.remove(&seat);
            self.despawns.push(seat);
            self.selection = moving.previous_selection;
            return;
        }
        let info = &self.info;
        room.restore(&moving.session, |seat| {
            info.get(&seat).and_then(|info| info.recorded)
        });
        for seat in std::iter::once(seat).chain(moving.session.carried) {
            self.base_tints.insert(seat, TINT_NORMAL);
        }
    }

    /// Starting a move: the piece and its load leave the grid, every piece dims and it pulses.
    fn begin_move(&mut self, seat: usize, new_piece: bool) {
        let Some(room) = self.room.as_mut() else {
            return;
        };
        let session = room.begin_move(seat);
        self.moving = Some(Moving {
            session,
            new_piece,
            previous_selection: self.selection,
        });
        self.selection = Some(seat);
        self.set_all_tints(TINT_DIMMED);
        self.pulse = Pulse::default();
    }

    fn open(&mut self, window: ArrangeWindow) -> InputMode {
        let cursor = self.open_cursor(window);
        InputMode::Menu(MenuStack {
            levels: vec![window_level(window, cursor)],
            active_pane: Pane::Right,
            absorb_open_minus: false,
            cursor_memory: Default::default(),
        })
    }

    fn open_cursor(&self, window: ArrangeWindow) -> usize {
        if !window.remembers_cursor() {
            return 0;
        }
        self.remembered
            .get(window.record_name())
            .copied()
            .unwrap_or_default()
            .min(usize::from(window.item_count()).saturating_sub(1))
    }

    fn push(&mut self, stack: &mut MenuStack, window: ArrangeWindow) {
        if let ArrangeWindow::Confirm(_) = window {
            stack.open_confirm(MenuKind::Arrange(window), ConfirmDefault::No);
            return;
        }
        let cursor = self.open_cursor(window);
        stack.levels.push(window_level(window, cursor));
    }

    fn close_top(&mut self, stack: &mut MenuStack) {
        if let Some(level) = stack.levels.pop() {
            if let Some(window) = arrange_window(&level).filter(|w| w.remembers_cursor()) {
                self.remembered.insert(window.record_name(), level.cursor);
            }
        }
    }

    fn close_all(&mut self, stack: &mut MenuStack) {
        while !stack.levels.is_empty() {
            self.close_top(stack);
        }
    }

    fn piece_storage(&self, seat: usize) -> u64 {
        std::iter::once(seat)
            .chain(self.carried(seat))
            .filter_map(|seat| self.info.get(&seat))
            .map(|info| u64::from(info.storage))
            .sum()
    }

    fn open_piece_menu(&mut self, seat: usize) -> InputMode {
        let mannequin = self
            .info
            .get(&seat)
            .is_some_and(|info| info.kind == KIND_MANNEQUIN);
        self.open(ArrangeWindow::Piece {
            mannequin,
            put_away: !arrange::put_away_greyed(self.storage.free, self.piece_storage(seat)),
            put_away_all: !arrange::put_away_all_greyed(self.storage.entries),
        })
    }

    fn planted(&self, seat: usize) -> bool {
        std::iter::once(seat)
            .chain(self.carried(seat))
            .any(|seat| self.info.get(&seat).is_some_and(|info| info.planted))
    }

    fn put_away(&mut self, seat: usize) {
        let mut seats: Vec<usize> = std::iter::once(seat).chain(self.carried(seat)).collect();
        seats.sort_unstable();
        for seat in seats {
            self.request_bankin(seat);
        }
    }

    fn request_bankin(&mut self, seat: usize) {
        if let Some(info) = self.info.get(&seat) {
            self.requests.push(ArrangeRequest::Bankin {
                item_no: info.item_no,
                slot: info.slot,
                container: info.container,
            });
        }
    }

    fn put_away_all(&mut self) {
        let seats: Vec<usize> = self
            .room
            .iter()
            .flat_map(Room::pieces)
            .map(|piece| piece.seat)
            .collect();
        for seat in seats {
            self.request_bankin(seat);
        }
        if let Some(room) = self.room.as_mut() {
            room.clear_occupancy();
        }
    }

    fn any_planted(&self) -> bool {
        self.info.values().any(|info| info.planted)
    }

    fn put_away_selected(
        &mut self,
        stack: &mut MenuStack,
        sounds: &mut Vec<UiSound>,
        keep_window: bool,
    ) -> Option<InputMode> {
        let selected = self.selection?;
        if self.planted(selected) {
            sounds.push(UiSound::Confirm);
            self.push(
                stack,
                ArrangeWindow::Confirm(ArrangeConfirm::Planted { keep_window }),
            );
            return None;
        }
        self.put_away(selected);
        if keep_window {
            return None;
        }
        self.close_all(stack);
        Some(InputMode::Arrange)
    }

    fn start_moving_selected(&mut self, stack: &mut MenuStack) -> Option<InputMode> {
        self.close_all(stack);
        if let Some(selected) = self.selection {
            self.begin_move(selected, false);
        }
        Some(InputMode::Arrange)
    }

    fn open_shelf_picker(&mut self, stack: &mut MenuStack, sounds: &mut Vec<UiSound>) {
        sounds.push(UiSound::Confirm);
        let second_shelf = self.second_shelf_open;
        self.push(stack, ArrangeWindow::Shelf { second_shelf });
    }

    fn ask_put_away_all(&mut self, stack: &mut MenuStack, sounds: &mut Vec<UiSound>) {
        sounds.push(UiSound::Confirm);
        self.push(stack, ArrangeWindow::Confirm(ArrangeConfirm::All));
    }

    fn close_to_browse(&mut self, stack: &mut MenuStack) -> Option<InputMode> {
        self.close_all(stack);
        Some(InputMode::Arrange)
    }

    /// A select on an arrange window. Returns the input mode to switch to; `None` keeps the menu.
    pub fn select_row(
        &mut self,
        window: ArrangeWindow,
        action: Option<DynamicMenuAction>,
        stack: &mut MenuStack,
        sounds: &mut Vec<UiSound>,
    ) -> Option<InputMode> {
        let action = match action {
            Some(DynamicMenuAction::Arrange(action)) => action,
            _ => return None,
        };
        if let ArrangeAction::Furniture {
            container,
            slot,
            item_no,
        } = action
        {
            return self.pick(stack, container, slot, item_no, sounds);
        }
        let ArrangeAction::Item { item } = action else {
            return None;
        };
        if window.item_disabled(item) {
            return None;
        }
        match window {
            ArrangeWindow::Empty { .. } => match item {
                RMLO1_PLACE => {
                    self.open_shelf_picker(stack, sounds);
                    None
                }
                RMLO1_PUT_AWAY_ALL => {
                    self.ask_put_away_all(stack, sounds);
                    None
                }
                RMLO1_FINISH => {
                    self.close_all(stack);
                    self.requests.push(ArrangeRequest::Leave);
                    Some(self.leave())
                }
                _ => self.close_to_browse(stack),
            },
            ArrangeWindow::Piece {
                mannequin: false, ..
            } => match item {
                RMLO2_MOVE => self.start_moving_selected(stack),
                RMLO2_PUT_AWAY => self.put_away_selected(stack, sounds, false),
                RMLO2_PLACE => {
                    self.open_shelf_picker(stack, sounds);
                    None
                }
                RMLO2_PUT_AWAY_ALL => {
                    self.ask_put_away_all(stack, sounds);
                    None
                }
                _ => self.close_to_browse(stack),
            },
            ArrangeWindow::Piece {
                mannequin: true, ..
            } => match item {
                RMLO4_MOVE => self.start_moving_selected(stack),
                RMLO4_PUT_AWAY => self.put_away_selected(stack, sounds, true),
                RMLO4_PLACE => {
                    self.close_all(stack);
                    self.open_shelf_picker(stack, sounds);
                    None
                }
                RMLO4_PUT_AWAY_ALL => {
                    self.ask_put_away_all(stack, sounds);
                    None
                }
                _ => self.close_to_browse(stack),
            },
            ArrangeWindow::Shelf { .. } => {
                let container = match item {
                    BANKMENU_SHELF => LOC_MOGSAFE,
                    BANKMENU_SHELF2 => LOC_MOGSAFE2,
                    _ => {
                        self.close_top(stack);
                        return stack.levels.is_empty().then_some(InputMode::Arrange);
                    }
                };
                sounds.push(UiSound::Confirm);
                self.close_all(stack);
                self.push(stack, ArrangeWindow::Furniture { container });
                None
            }
            ArrangeWindow::Furniture { .. } => None,
            ArrangeWindow::Confirm(confirm) => {
                if item != CONFIRM_ITEM_YES {
                    self.close_top(stack);
                    return stack.levels.is_empty().then_some(InputMode::Arrange);
                }
                self.confirm_yes(confirm, stack, sounds)
            }
        }
    }

    fn confirm_yes(
        &mut self,
        confirm: ArrangeConfirm,
        stack: &mut MenuStack,
        sounds: &mut Vec<UiSound>,
    ) -> Option<InputMode> {
        match confirm {
            ArrangeConfirm::Planted { keep_window } => {
                self.close_top(stack);
                if let Some(selected) = self.selection {
                    self.put_away(selected);
                }
                if keep_window && !stack.levels.is_empty() {
                    return None;
                }
                self.close_to_browse(stack)
            }
            ArrangeConfirm::All if self.any_planted() => {
                self.close_top(stack);
                sounds.push(UiSound::Confirm);
                self.push(stack, ArrangeWindow::Confirm(ArrangeConfirm::PlantedAll));
                None
            }
            ArrangeConfirm::All | ArrangeConfirm::PlantedAll => {
                self.put_away_all();
                self.close_to_browse(stack)
            }
        }
    }

    /// A furniture list pick: refused with its system message over a limit; otherwise the piece
    /// starts moving (a new piece from its start pose) with the confirm sound.
    fn pick(
        &mut self,
        stack: &mut MenuStack,
        container: u8,
        slot: u8,
        item_no: u16,
        sounds: &mut Vec<UiSound>,
    ) -> Option<InputMode> {
        let entry = *self
            .shelf
            .iter()
            .find(|e| e.container == container && e.slot == slot && e.item_no == item_no)?;
        let count = |kind| self.info.values().filter(|info| info.kind == kind).count();
        if let Some(refusal) =
            arrange::pick_refusal(entry.kind, count(KIND_FLOWERPOT), count(KIND_MANNEQUIN))
        {
            let id = match refusal {
                arrange::PickRefusal::Flowerpots => SYSMES_FLOWERPOT_LIMIT,
                arrange::PickRefusal::Mannequins => SYSMES_MANNEQUIN_LIMIT,
            };
            self.chat_lines.push(self.texts.system_message(id));
            return None;
        }
        let seat = law::seat_index(container, slot);
        let room = self.room.as_mut()?;
        if room.piece(seat).is_none() {
            let property = entry.property?;
            let pose = room.new_piece_pose(&property, self.selection);
            room.insert_unregistered(ArrangePiece {
                seat,
                property,
                pose,
            });
            self.info.insert(
                seat,
                PieceInfo {
                    container,
                    slot,
                    item_no,
                    kind: entry.kind,
                    planted: entry.attr.flowerpot_step > 0,
                    storage: entry.storage,
                    recorded: Some(entry.attr.pose),
                },
            );
            self.spawns.push(NewPiece {
                container,
                slot,
                item_no,
                kind: entry.kind,
                attr: entry.attr,
                pose,
            });
            sounds.push(UiSound::Confirm);
            self.close_all(stack);
            self.begin_move(seat, true);
            return Some(InputMode::Arrange);
        }
        sounds.push(UiSound::Confirm);
        self.close_all(stack);
        self.begin_move(seat, false);
        Some(InputMode::Arrange)
    }

    /// Esc on an arrange window: the furniture list closes back to browsing; any other window closes
    /// alone. `Some(mode)` switches the input mode, `None` keeps the menu.
    pub fn cancel_window(
        &mut self,
        stack: &mut MenuStack,
        sounds: &mut Vec<UiSound>,
    ) -> Option<InputMode> {
        sounds.push(UiSound::Cancel);
        let top = stack.current().and_then(arrange_window);
        if matches!(top, Some(ArrangeWindow::Furniture { .. })) {
            return self.close_to_browse(stack);
        }
        self.close_top(stack);
        stack.levels.is_empty().then_some(InputMode::Arrange)
    }

    /// The rows of an arrange window.
    pub fn rows(&self, window: ArrangeWindow) -> Vec<DynamicMenuRow> {
        if let ArrangeWindow::Furniture { container } = window {
            return self.furniture_rows(container);
        }
        (1..=window.item_count())
            .map(|item| DynamicMenuRow {
                label: self.texts.item_label(window.record_name(), item),
                action: DynamicMenuAction::Arrange(ArrangeAction::Item { item }),
                tint: None,
            })
            .collect()
    }

    fn furniture_rows(&self, container: u8) -> Vec<DynamicMenuRow> {
        let Some(floor) = self.house.map(|house| house.floor) else {
            return Vec::new();
        };
        let mut entries: Vec<&ShelfEntry> = self
            .shelf
            .iter()
            .filter(|entry| entry.container == container)
            .collect();
        entries.sort_by_key(|entry| entry.slot);
        entries
            .into_iter()
            .filter_map(|entry| {
                let listed = arrange::furniture_list_row(
                    entry.kind,
                    entry.attr.installed,
                    entry.attr.layer,
                    floor,
                )?;
                Some(DynamicMenuRow {
                    label: ffxi_vocab::item_names::lookup(entry.item_no)
                        .unwrap_or_default()
                        .to_string(),
                    action: DynamicMenuAction::Arrange(ArrangeAction::Furniture {
                        container: entry.container,
                        slot: entry.slot,
                        item_no: entry.item_no,
                    }),
                    tint: (listed == arrange::Listed::PlacedHere)
                        .then_some(MenuRowTint::FurniturePlaced),
                })
            })
            .collect()
    }
}

/// `/layoutctrl type1` ([`LAYOUT_CONTROL_TYPE1`] stored, or nothing stored) reverses floor steps while
/// the camera eye's z is at or past the focus's.
fn layout_type1(dynamic: &DynamicMenu) -> bool {
    dynamic
        .config_option_values
        .get(&CONFIG_OPTION_LAYOUT_CONTROL_TYPE)
        .copied()
        .unwrap_or(LAYOUT_CONTROL_TYPE1)
        == LAYOUT_CONTROL_TYPE1
}

/// One arrange key event from the input layer.
pub fn arrange_key(
    dynamic: &mut DynamicMenu,
    key: ArrangeKey,
    phase: KeyPhase,
) -> Option<InputMode> {
    let type1 = layout_type1(dynamic);
    let DynamicMenu {
        arrange,
        ui_sounds,
        window_cursors,
        ..
    } = dynamic;
    with_session_memory(arrange.key(key, phase, type1, ui_sounds), window_cursors)
}

/// The `myroom` menu leaving layout mode reopens takes the session's window memory.
pub fn with_session_memory(next: Option<InputMode>, memory: &CursorMemory) -> Option<InputMode> {
    next.map(|mut mode| {
        if let InputMode::Menu(stack) = &mut mode {
            if !is_arrange_stack(stack) {
                stack.adopt_memory(memory);
            }
        }
        mode
    })
}

/// The confirm seam for an arrange window.
pub fn select_arrange_row(stack: &mut MenuStack, dynamic: &mut DynamicMenu) -> Option<InputMode> {
    let level = stack.current()?;
    let window = arrange_window(level)?;
    let action = crate::hud::menu::entry_action(level.kind, level.cursor, dynamic);
    let DynamicMenu {
        arrange,
        ui_sounds,
        window_cursors,
        ..
    } = dynamic;
    with_session_memory(
        arrange.select_row(window, action, stack, ui_sounds),
        window_cursors,
    )
}

/// The Esc seam: `None` when the top window is not an arrange window, else whether it switches the
/// input mode.
pub fn cancel_arrange_window(
    stack: &mut MenuStack,
    dynamic: &mut DynamicMenu,
) -> Option<Option<InputMode>> {
    stack.current().and_then(arrange_window)?;
    let DynamicMenu {
        arrange,
        ui_sounds,
        window_cursors,
        ..
    } = dynamic;
    Some(with_session_memory(
        arrange.cancel_window(stack, ui_sounds),
        window_cursors,
    ))
}

/// Whether a menu stack is the arrange editor's.
pub fn is_arrange_stack(stack: &MenuStack) -> bool {
    stack
        .levels
        .first()
        .is_some_and(|level| matches!(level.kind, MenuKind::Arrange(_)))
}

/// The input mode is the arrange editor's: browsing, moving or one of its windows.
pub fn arranging(mode: &InputMode) -> bool {
    match mode {
        InputMode::Arrange => true,
        InputMode::Menu(stack) => is_arrange_stack(stack),
        _ => false,
    }
}

/// While layout mode holds the camera the chase camera writes nothing.
#[derive(bevy::prelude::Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ArrangeCameraHold(pub bool);

#[cfg(test)]
mod tests;
