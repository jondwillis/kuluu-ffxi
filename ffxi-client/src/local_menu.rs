//! Client-local menus (Mog House exit door "Where to?" and the Mog Menu) driven
//! through the same [`DialogState`] frames the event VM produces, so the HUD
//! dialog panel and headless agents consume them unchanged. Terminal picks map to
//! [`MogHouseExit`]/[`AgentCommand::ChangeJob`]-shaped results; the session owns
//! the packet sends.
//!
//! [`AgentCommand::ChangeJob`]: crate::state::AgentCommand::ChangeJob

use ffxi_proto::decode::ServerLoginMyroom;
use ffxi_proto::map::pbx;

use crate::delivery_box::item_name;
use crate::state::{
    DeliveryBoxNo, DeliveryBoxOp, DeliveryItem, DialogGrid, DialogGridCell, DialogState,
    JobInfoState, MogHouseExit, MyRoomInfo,
};

/// Synthetic client-local actor ids, kept in a reserved top-of-range block: LSB
/// unique_no encodings ((4<<28)|(zone<<12)|targid at the largest) never reach
/// 0xFFFF_FF00 (vendor/server/src/map/zone_entities.cpp id assignment).
pub const MH_DOOR_ENTITY_ID: u32 = 0xFFFF_FF01;
pub const MOG_MENU_ID: u32 = 0xFFFF_FF02;

/// Nameplate/dialog speaker for the synthesized exit door, matching XIM's
/// re-creation (research/xim .../assetviewer/AssetViewer.kt:669 — LSB spawns no
/// door NPC, so there is no server-side name to echo).
pub const MH_DOOR_NAME: &str = "Door: To Town";
pub const MOG_MENU_NPC_NAME: &str = "Moogle";

pub const WHERE_TO_PROMPT: &str = "Where to?";
pub const STAY_ROW: &str = "Stay in your room.";
pub const HOME_ROW: &str = "Area you entered from.";
pub const CHANGE_FLOORS_ROW: &str = "Change floors.";
pub const SELECT_AREA_ROW: &str = "Select an area to exit to.";
pub const MOG_GARDEN_ROW: &str = "Mog Garden.";

// Mog Menu labels, order, prompts, and disabled rules follow the retail client
// as observed on HorizonXI 2026-07-17 (artifacts/retail/moghouse-menu-notes.md).
pub const MOG_MENU_PROMPT: &str = "Mog Menu";
pub const STORAGE_ROW: &str = "Storage";
pub const DELIVERY_BOX_ROW: &str = "Delivery Box";
pub const CHANGE_JOBS_ROW: &str = "Change Jobs";
pub const GARDENING_ROW: &str = "Gardening";
pub const LAYOUT_ROW: &str = "Layout";
pub const OPEN_MOG_HOUSE_ROW: &str = "Open Mog House";
pub const REMODEL_ROW: &str = "Remodel";
pub const CANCEL_ROW: &str = "Cancel";

pub const STORAGE_PROMPT: &str = "Check all items in your Mog Safe and other storage systems.";
pub const MOG_SAFE_ROW: &str = "Mog Safe";
pub const MOG_SAFE2_ROW: &str = "Mog Safe 2";
pub const MOG_LOCKER_ROW: &str = "Mog Locker";
pub const MOG_SATCHEL_ROW: &str = "Mog Satchel";
pub const MOG_SACK_ROW: &str = "Mog Sack";
pub const MOG_CASE_ROW: &str = "Mog Case";
pub const MOG_WARDROBE_ROWS: [&str; 8] = [
    "Mog Wardrobe",
    "Mog Wardrobe 2",
    "Mog Wardrobe 3",
    "Mog Wardrobe 4",
    "Mog Wardrobe 5",
    "Mog Wardrobe 6",
    "Mog Wardrobe 7",
    "Mog Wardrobe 8",
];

pub const DELIVERY_PROMPT: &str = "Use the delivery system.";
pub const RECEIVE_ROW: &str = "Receive";
pub const SEND_ROW: &str = "Send";

// Retail Receive panel action buttons, observed order (moghouse-menu-notes.md
// "Take / Drop / Return"); Return is disabled for auction-house senders.
pub const TAKE_ROW: &str = "Take";
pub const DROP_ROW: &str = "Drop";
pub const RETURN_ROW: &str = "Return";
pub const SEND_ITEM_ROW: &str = "Send";
pub const TAKE_BACK_ROW: &str = "Take back";
pub const CANCEL_DELIVERY_ROW: &str = "Cancel delivery";

/// Retail panel headers ("Delivery Box" receive / "Deliveries" send),
/// artifacts/retail/moghouse-menu-notes.md.
pub const RECEIVE_PANEL_PROMPT: &str = "Select an item from the delivery box.";
/// Retail Send-panel header (artifacts/retail/moghouse-menu-notes.md:55).
pub const SEND_PANEL_PROMPT: &str =
    "Deliveries | After specifying recipient, place items in empty slots to send them to recipient's delivery box.";
/// Send-panel recipient field, mirroring retail's text box above the grid.
pub const RECIPIENT_ROW_PREFIX: &str = "Recipient: ";
/// Prompt for the recipient name-entry frame.
pub const RECIPIENT_PROMPT: &str = "Enter a recipient name.";
pub const RECIPIENT_UNSET: &str = "(not specified)";
/// An empty outbox grid cell (rows appear once the recipient is locked, like
/// retail's cursor order: recipient OK first, then the slot grid activates).
pub const EMPTY_SLOT_SUFFIX: &str = "(empty)";
/// Retail delivery panels lay the 8 slots out as a 2-row x 4-column grid.
pub const DELIVERY_GRID_COLS: u8 = 4;
/// Retail's item list header once an empty slot is entered
/// (artifacts/retail/moghouse-menu-notes.md:63 — `Items | Select an item.`).
pub const PICK_ITEM_PROMPT: &str = "Items | Select an item.";
pub const QUANTITY_PROMPT: &str = "Select a quantity.";
pub const BACK_ROW: &str = "Back";

/// LSB marks auction-house mail by a sender starting with "AH"; the retail
/// client disables Return for it (vendor/server/src/map/packets/s2c/
/// 0x04b_pbx_result.cpp:91).
const AH_SENDER_PREFIX: &str = "AH";

pub const MAIN_JOB_ROW: &str = "Main Job";
pub const SUPPORT_JOB_ROW: &str = "Support Job";

/// JOBTYPE 1=WAR..22=RUN are player-selectable; 23 (MON) is not.
/// vendor/server/src/map/entities/battleentity.h JOBTYPE.
const SELECTABLE_JOB_MAX: u8 = 22;

/// District rows per MyRoomExitBit; the slot is the MYROOMEXITMODE Option1-4 value
/// LSB maps to a destination zone (vendor/server/src/map/packets/c2s/
/// 0x05e_maprect.cpp:88-135).
fn district_rows(exit_bit: u8) -> &'static [(&'static str, u8)] {
    match exit_bit {
        1 => &[
            ("Southern San d'Oria", 1),
            ("Northern San d'Oria", 2),
            ("Port San d'Oria", 3),
        ],
        2 => &[
            ("Bastok Mines", 1),
            ("Bastok Markets", 2),
            ("Port Bastok", 3),
        ],
        3 => &[
            ("Windurst Waters", 1),
            ("Windurst Walls", 2),
            ("Port Windurst", 3),
            ("Windurst Woods", 4),
        ],
        4 => &[
            ("Ru'Lude Gardens", 1),
            ("Upper Jeuno", 2),
            ("Lower Jeuno", 3),
            ("Port Jeuno", 4),
        ],
        5 => &[("Al Zahbi", 1), ("Aht Urhgan Whitegate", 2)],
        9 => &[("Western Adoulin", 1), ("Eastern Adoulin", 2)],
        _ => &[],
    }
}

#[derive(Debug, Clone)]
enum Action {
    Close,
    Exit(MogHouseExit),
    OpenAreas {
        exit_bit: u8,
    },
    OpenJobType,
    OpenJobList {
        support: bool,
    },
    PickJob {
        support: bool,
        job: u8,
    },
    OpenStorageList,
    OpenStorage {
        container: u8,
    },
    OpenDeliveryBox,
    OpenMogRoot,
    /// Start the 0x04D open flow for `box_no` (Receive/Send rows).
    DeliveryOpen {
        box_no: DeliveryBoxNo,
    },
    /// Open the per-slot action submenu for an occupied box slot.
    DeliverySlot {
        box_no: DeliveryBoxNo,
        slot: u8,
    },
    /// Rebuild the box-content menu from the stored slot snapshot.
    DeliveryBack,
    /// The retail Accept → Get take chain for an inbox slot.
    DeliveryTake {
        box_no: DeliveryBoxNo,
        slot: u8,
    },
    /// A single pass-through 0x04D request (Reject/Clear/Send/Cancel/…).
    Delivery {
        op: DeliveryBoxOp,
    },
    /// The Send-panel recipient field: the session prompts for a name.
    DeliveryRecipient,
    /// An empty outbox grid cell: the session opens the item picker from its
    /// LOC_INVENTORY mirror.
    DeliveryPut {
        slot: u8,
    },
    /// An item-picker row: stage into `slot`, asking for a quantity first when
    /// the stack holds more than one.
    DeliveryPickItem {
        slot: u8,
        inventory_slot: u8,
        item_no: u16,
        quantity: u32,
    },
    /// A quantity row: emit the 0x04D Set for the stored recipient.
    DeliveryStage {
        slot: u8,
        inventory_slot: u8,
        quantity: u32,
    },
    Stub(&'static str),
}

struct Menu {
    npc_id: u32,
    npc_name: &'static str,
    prompt: String,
    rows: Vec<(String, Action)>,
}

/// Outcome of feeding a player response to the active local menu.
#[derive(Debug)]
pub enum Advance {
    Frame(DialogState),
    Exit(MogHouseExit),
    ChangeJob {
        main_job: Option<u8>,
        sub_job: Option<u8>,
    },
    /// A storage row: close the menu and browse `container` (an LSB
    /// CONTAINER_ID). The native viewer opens its Items window on it.
    OpenStorage {
        container: u8,
    },
    /// An unimplemented row: surface `notice` as a system chat line and keep the
    /// menu open with `frame`.
    Stub {
        notice: &'static str,
        frame: DialogState,
    },
    /// Receive/Send picked: the session opens the box (0x04D DeliOpen/PostOpen)
    /// and renders the content menu once the server-side flow settles.
    DeliveryOpen {
        box_no: DeliveryBoxNo,
    },
    /// A slot Take: the session drives the Accept → Get chain.
    DeliveryTake {
        box_no: DeliveryBoxNo,
        slot: u8,
    },
    /// A single 0x04D request the session sends as-is.
    Delivery {
        op: DeliveryBoxOp,
    },
    /// The recipient field was picked: the session raises a text-entry dialog
    /// frame; the answer comes back through [`LocalMenuSession::set_recipient`].
    DeliveryRecipient {
        frame: DialogState,
    },
    /// An empty outbox slot was picked: the session opens the item picker from
    /// its LOC_INVENTORY mirror via [`LocalMenuSession::open_delivery_pick`].
    DeliveryPut {
        slot: u8,
    },
    Close,
}

#[derive(Default)]
pub struct LocalMenuSession {
    menu: Option<Menu>,
    job_info: Option<JobInfoState>,
    container_caps: Option<Vec<u16>>,
    /// Snapshot behind the open box-content menu, so slot submenus can offer
    /// a Back row without re-querying the server.
    delivery: Option<(DeliveryBoxNo, [Option<DeliveryItem>; pbx::SLOT_COUNT])>,
    /// Send-panel recipient, kept across per-op menu rebuilds (retail keeps the
    /// locked name until the panel closes). Server-verified via 0x04D Query
    /// before empty-slot rows activate.
    recipient: Option<String>,
}

impl LocalMenuSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active(&self) -> bool {
        self.menu.is_some()
    }

    pub fn clear(&mut self) {
        self.menu = None;
        self.delivery = None;
        self.recipient = None;
    }

    /// The exit-door "Where to?" menu. "Change floors." shows when the player is
    /// on the 2F — LSB forces the full menu there (0x00a_login.cpp MH branch) —
    /// or when the 2F-unlock bit (char_sync.cpp:61) is known true; the server's
    /// rejection of a locked 2F request is log-only, so an ungated row would
    /// fail silently.
    pub fn open_mh_exit(
        &mut self,
        myroom: &MyRoomInfo,
        mh_2f_unlocked: Option<bool>,
    ) -> DialogState {
        let on_2f = myroom.sub_map == ServerLoginMyroom::SUB_MAP_2F;
        let mut rows = vec![
            (STAY_ROW.to_string(), Action::Close),
            (
                HOME_ROW.to_string(),
                Action::Exit(MogHouseExit::Home {
                    exit_bit: myroom.exit_bit,
                }),
            ),
        ];
        if on_2f || mh_2f_unlocked == Some(true) {
            let target = if on_2f {
                MogHouseExit::Mog1F
            } else {
                MogHouseExit::Mog2F
            };
            rows.push((CHANGE_FLOORS_ROW.to_string(), Action::Exit(target)));
        }
        if !district_rows(myroom.exit_bit).is_empty() {
            rows.push((
                SELECT_AREA_ROW.to_string(),
                Action::OpenAreas {
                    exit_bit: myroom.exit_bit,
                },
            ));
        }
        rows.push((
            MOG_GARDEN_ROW.to_string(),
            Action::Exit(MogHouseExit::MogGarden),
        ));
        self.set(Menu {
            npc_id: MH_DOOR_ENTITY_ID,
            npc_name: MH_DOOR_NAME,
            prompt: WHERE_TO_PROMPT.to_string(),
            rows,
        })
    }

    /// The Mog Menu (opened by s2c 0x02E or `AgentCommand::OpenMogMenu`).
    /// `container_caps` (0x01C ITEM_MAX, indexed by CONTAINER_ID) hides storage
    /// rows the server never granted — e.g. the Mog Locker on a pre-ToAU era
    /// server — since LSB rejects moves into a zero-size container.
    pub fn open_mog_menu(
        &mut self,
        job_info: Option<JobInfoState>,
        container_caps: Option<&[u16]>,
    ) -> DialogState {
        self.job_info = job_info;
        self.container_caps = container_caps.map(<[u16]>::to_vec);
        self.set(mog_menu())
    }

    /// The box-content menu, rendered once the 0x04D open flow settles: one row
    /// per occupied slot, Cancel closes the box (PostClose). Retail shows this
    /// as an 8-slot grid panel; the dialog list carries the same choices.
    pub fn open_delivery_box(
        &mut self,
        box_no: DeliveryBoxNo,
        slots: &[Option<DeliveryItem>; pbx::SLOT_COUNT],
    ) -> DialogState {
        self.delivery = Some((box_no, slots.clone()));
        let mut rows: Vec<(String, Action)> = Vec::new();
        if box_no == DeliveryBoxNo::Outgoing {
            rows.push((
                format!(
                    "{RECIPIENT_ROW_PREFIX}{}",
                    self.recipient.as_deref().unwrap_or(RECIPIENT_UNSET)
                ),
                Action::DeliveryRecipient,
            ));
        }
        // Build the retail 2x4 grid overlay alongside the flat rows: each
        // selectable cell records the choice index of the row it activates.
        let mut cells: Vec<DialogGridCell> = Vec::with_capacity(pbx::SLOT_COUNT);
        for (i, cell) in slots.iter().enumerate() {
            match cell {
                Some(item) => {
                    cells.push(DialogGridCell {
                        choice: Some(rows.len() as u32),
                        item_no: Some(item.item_no),
                        quantity: item.quantity,
                        sent: item.sent(),
                    });
                    rows.push((
                        delivery_slot_label(box_no, item),
                        Action::DeliverySlot {
                            box_no,
                            slot: i as u8,
                        },
                    ));
                }
                // Empty cells stay focusable so the cursor always lands on the
                // grid (even for an empty box) instead of skipping to Cancel.
                // Placing an item requires a locked recipient; otherwise
                // activation is a no-op notice.
                None => {
                    let action = match box_no {
                        DeliveryBoxNo::Outgoing if self.recipient.is_some() => {
                            Action::DeliveryPut { slot: i as u8 }
                        }
                        DeliveryBoxNo::Outgoing => {
                            Action::Stub("Specify a recipient before placing items.")
                        }
                        DeliveryBoxNo::Incoming => Action::Stub("This slot is empty."),
                    };
                    cells.push(DialogGridCell {
                        choice: Some(rows.len() as u32),
                        ..DialogGridCell::default()
                    });
                    rows.push((format!("Slot {} {EMPTY_SLOT_SUFFIX}", i + 1), action));
                }
            }
        }
        rows.push((
            CANCEL_ROW.to_string(),
            Action::Delivery {
                op: DeliveryBoxOp::PostClose { box_no },
            },
        ));
        let mut f = self.set(Menu {
            npc_id: MOG_MENU_ID,
            npc_name: MOG_MENU_NPC_NAME,
            prompt: match box_no {
                DeliveryBoxNo::Incoming => RECEIVE_PANEL_PROMPT.to_string(),
                DeliveryBoxNo::Outgoing => SEND_PANEL_PROMPT.to_string(),
            },
            rows,
        });
        f.grid = Some(DialogGrid {
            cols: DELIVERY_GRID_COLS,
            rows: (pbx::SLOT_COUNT as u8).div_ceil(DELIVERY_GRID_COLS),
            cells,
        });
        f
    }

    /// Re-open the Receive/Send submenu (after a PostClose ack).
    pub fn open_delivery_submenu(&mut self) -> DialogState {
        self.set(delivery_menu())
    }

    /// The Query-verified recipient, if one is locked.
    pub fn recipient(&self) -> Option<&str> {
        self.recipient.as_deref()
    }

    /// Lock a Query-verified recipient (`None` clears it) and rebuild the open
    /// Send panel so its empty-slot rows (de)activate accordingly.
    pub fn set_recipient(&mut self, name: Option<String>) -> Option<DialogState> {
        self.recipient = name;
        match self.delivery.clone() {
            Some((DeliveryBoxNo::Outgoing, slots)) => {
                Some(self.open_delivery_box(DeliveryBoxNo::Outgoing, &slots))
            }
            _ => None,
        }
    }

    /// The item picker for an empty outbox `slot`. `items` is the session's
    /// LOC_INVENTORY view, already filtered to sendable stacks (no EX, no
    /// equipped gear; FLAG_CAN_SEND_ACCT gating happens server-side too).
    pub fn open_delivery_pick(&mut self, slot: u8, items: &[PickableItem]) -> DialogState {
        let rows = items
            .iter()
            .map(|it| {
                (
                    format!("{} x{}", item_name(it.item_no), it.quantity),
                    Action::DeliveryPickItem {
                        slot,
                        inventory_slot: it.inventory_slot,
                        item_no: it.item_no,
                        quantity: it.quantity,
                    },
                )
            })
            .chain(std::iter::once((
                BACK_ROW.to_string(),
                Action::DeliveryBack,
            )))
            .collect();
        self.set(Menu {
            npc_id: MOG_MENU_ID,
            npc_name: MOG_MENU_NPC_NAME,
            prompt: PICK_ITEM_PROMPT.to_string(),
            rows,
        })
    }

    /// Emit the 0x04D Set for a staged pick; the recipient must still be
    /// locked (it is menu-gated, so a miss means state was torn down under us).
    fn stage(&mut self, slot: u8, inventory_slot: u8, quantity: u32) -> Advance {
        let Some(recipient) = self.recipient.clone() else {
            self.clear();
            return Advance::Close;
        };
        self.menu = None;
        Advance::Delivery {
            op: DeliveryBoxOp::Set {
                slot,
                inventory_slot,
                quantity,
                recipient,
            },
        }
    }

    pub fn advance(&mut self, choice: Option<u32>) -> Advance {
        let Some(menu) = self.menu.as_ref() else {
            return Advance::Close;
        };
        let action = match choice.and_then(|c| menu.rows.get(c as usize)) {
            Some((_, action)) => action.clone(),
            None => {
                self.clear();
                return Advance::Close;
            }
        };
        match action {
            Action::Close => {
                self.clear();
                Advance::Close
            }
            Action::Exit(kind) => {
                self.clear();
                Advance::Exit(kind)
            }
            Action::OpenAreas { exit_bit } => Advance::Frame(self.set(areas_menu(exit_bit))),
            Action::OpenJobType => match self.job_info {
                Some(info) => Advance::Frame(self.set(job_type_menu(&info))),
                None => Advance::Stub {
                    notice: "Job data has not arrived yet (no 0x01B JOB_INFO) — try again.",
                    frame: frame(self.menu.as_ref().expect("menu still active")),
                },
            },
            Action::OpenJobList { support } => match self.job_info {
                Some(info) => Advance::Frame(self.set(job_list_menu(&info, support))),
                None => {
                    self.clear();
                    Advance::Close
                }
            },
            Action::PickJob { support, job } => {
                self.clear();
                Advance::ChangeJob {
                    main_job: (!support).then_some(job),
                    sub_job: support.then_some(job),
                }
            }
            Action::OpenStorageList => {
                Advance::Frame(self.set(storage_menu(self.container_caps.as_deref())))
            }
            Action::OpenStorage { container } => {
                self.clear();
                Advance::OpenStorage { container }
            }
            // Retail lets you choose which box to open (Receive default); the
            // dedicated screen also toggles between them in-window.
            Action::OpenDeliveryBox => Advance::Frame(self.set(delivery_menu())),
            Action::OpenMogRoot => Advance::Frame(self.set(mog_menu())),
            Action::DeliveryOpen { box_no } => {
                self.clear();
                Advance::DeliveryOpen { box_no }
            }
            Action::DeliverySlot { box_no, slot } => {
                let item = self
                    .delivery
                    .as_ref()
                    .and_then(|(_, slots)| slots.get(slot as usize))
                    .and_then(Clone::clone);
                match item {
                    Some(item) => Advance::Frame(self.set(delivery_slot_menu(box_no, slot, &item))),
                    None => {
                        self.clear();
                        Advance::Close
                    }
                }
            }
            Action::DeliveryBack => match self.delivery.take() {
                Some((box_no, slots)) => Advance::Frame(self.open_delivery_box(box_no, &slots)),
                None => {
                    self.clear();
                    Advance::Close
                }
            },
            Action::DeliveryTake { box_no, slot } => {
                self.clear();
                Advance::DeliveryTake { box_no, slot }
            }
            Action::Delivery { op } => {
                // Retail keeps the locked recipient until the Send panel
                // closes; PostClose is the panel closing.
                let recipient = (!matches!(op, DeliveryBoxOp::PostClose { .. }))
                    .then(|| self.recipient.take())
                    .flatten();
                self.clear();
                self.recipient = recipient;
                Advance::Delivery { op }
            }
            Action::DeliveryRecipient => Advance::DeliveryRecipient {
                frame: recipient_entry_frame(),
            },
            Action::DeliveryPut { slot } => Advance::DeliveryPut { slot },
            Action::DeliveryPickItem {
                slot,
                inventory_slot,
                item_no,
                quantity,
            } => {
                if quantity > 1 {
                    Advance::Frame(self.set(quantity_menu(slot, inventory_slot, item_no, quantity)))
                } else {
                    self.stage(slot, inventory_slot, 1)
                }
            }
            Action::DeliveryStage {
                slot,
                inventory_slot,
                quantity,
            } => self.stage(slot, inventory_slot, quantity),
            Action::Stub(notice) => Advance::Stub {
                notice,
                frame: frame(self.menu.as_ref().expect("menu still active")),
            },
        }
    }

    fn set(&mut self, menu: Menu) -> DialogState {
        let f = frame(&menu);
        self.menu = Some(menu);
        f
    }
}

fn frame(menu: &Menu) -> DialogState {
    DialogState {
        event_id: menu.npc_id,
        npc_id: menu.npc_id,
        npc_name: Some(menu.npc_name.to_string()),
        act_index: 0,
        event_num: 0,
        event_para: 0,
        mode: 0,
        event_num2: 0,
        event_para2: 0,
        strings: Vec::new(),
        nums: Vec::new(),
        prompt: Some(menu.prompt.clone()),
        choices: menu.rows.iter().map(|(label, _)| label.clone()).collect(),
        text_entry: false,
        grid: None,
        custom_menu: false,
    }
}

fn areas_menu(exit_bit: u8) -> Menu {
    let rows = district_rows(exit_bit)
        .iter()
        .map(|&(label, slot)| {
            (
                label.to_string(),
                Action::Exit(MogHouseExit::from_bit_slot(exit_bit, slot)),
            )
        })
        .chain(std::iter::once((CANCEL_ROW.to_string(), Action::Close)))
        .collect();
    Menu {
        npc_id: MH_DOOR_ENTITY_ID,
        npc_name: MH_DOOR_NAME,
        prompt: SELECT_AREA_ROW.to_string(),
        rows,
    }
}

fn mog_menu() -> Menu {
    let rows = vec![
        (STORAGE_ROW.to_string(), Action::OpenStorageList),
        (DELIVERY_BOX_ROW.to_string(), Action::OpenDeliveryBox),
        (CHANGE_JOBS_ROW.to_string(), Action::OpenJobType),
        (
            GARDENING_ROW.to_string(),
            Action::Stub("Gardening is not yet implemented — tracked as kuluu-jdwl."),
        ),
        // Retail disables Layout/Open Mog House in a rent-a-room (non-home-nation
        // MH); once implemented, gate them on that rule rather than stubbing.
        (
            LAYOUT_ROW.to_string(),
            Action::Stub("Layout is not yet implemented — tracked as kuluu-6a0."),
        ),
        (
            OPEN_MOG_HOUSE_ROW.to_string(),
            Action::Stub("Open Mog House is not yet implemented — tracked as kuluu-6a0."),
        ),
        (
            REMODEL_ROW.to_string(),
            Action::Stub("Remodel is not yet implemented — tracked as kuluu-6a0."),
        ),
        (CANCEL_ROW.to_string(), Action::Close),
    ];
    Menu {
        npc_id: MOG_MENU_ID,
        npc_name: MOG_MENU_NPC_NAME,
        prompt: MOG_MENU_PROMPT.to_string(),
        rows,
    }
}

/// Retail Storage submenu order: Safe, Safe 2, Storage, Locker, Satchel, Sack,
/// Case, Wardrobe 1-8. The label→container pairs live here (the emitter);
/// consumers map labels back via [`storage_row_container`].
pub fn storage_rows() -> [(&'static str, u8); 15] {
    use ffxi_proto::map::container as c;
    [
        (MOG_SAFE_ROW, c::LOC_MOGSAFE),
        (MOG_SAFE2_ROW, c::LOC_MOGSAFE2),
        (STORAGE_ROW, c::LOC_STORAGE),
        (MOG_LOCKER_ROW, c::LOC_MOGLOCKER),
        (MOG_SATCHEL_ROW, c::LOC_MOGSATCHEL),
        (MOG_SACK_ROW, c::LOC_MOGSACK),
        (MOG_CASE_ROW, c::LOC_MOGCASE),
        (MOG_WARDROBE_ROWS[0], c::LOC_WARDROBE),
        (MOG_WARDROBE_ROWS[1], c::LOC_WARDROBE2),
        (MOG_WARDROBE_ROWS[2], c::LOC_WARDROBE3),
        (MOG_WARDROBE_ROWS[3], c::LOC_WARDROBE4),
        (MOG_WARDROBE_ROWS[4], c::LOC_WARDROBE5),
        (MOG_WARDROBE_ROWS[5], c::LOC_WARDROBE6),
        (MOG_WARDROBE_ROWS[6], c::LOC_WARDROBE7),
        (MOG_WARDROBE_ROWS[7], c::LOC_WARDROBE8),
    ]
}

pub fn storage_row_container(label: &str) -> Option<u8> {
    storage_rows()
        .iter()
        .find(|(row, _)| *row == label)
        .map(|&(_, container)| container)
}

/// `container_caps` (0x01C ITEM_MAX, indexed by CONTAINER_ID) hides containers
/// the server never granted — e.g. the Mog Locker or wardrobes on a pre-ToAU
/// era server — since LSB rejects moves into a zero-size container. Unknown
/// capacities (no 0x01C yet) show every row; the Items window then simply
/// renders an empty bag.
fn storage_menu(container_caps: Option<&[u16]>) -> Menu {
    let granted =
        |id: u8| container_caps.is_none_or(|caps| caps.get(id as usize).copied().unwrap_or(0) > 0);
    let rows = storage_rows()
        .iter()
        .filter(|&&(_, container)| granted(container))
        .map(|&(label, container)| (label.to_string(), Action::OpenStorage { container }))
        .chain(std::iter::once((
            CANCEL_ROW.to_string(),
            Action::OpenMogRoot,
        )))
        .collect();
    Menu {
        npc_id: MOG_MENU_ID,
        npc_name: MOG_MENU_NPC_NAME,
        prompt: STORAGE_PROMPT.to_string(),
        rows,
    }
}

fn delivery_menu() -> Menu {
    let rows = vec![
        (
            RECEIVE_ROW.to_string(),
            Action::DeliveryOpen {
                box_no: DeliveryBoxNo::Incoming,
            },
        ),
        (
            SEND_ROW.to_string(),
            Action::DeliveryOpen {
                box_no: DeliveryBoxNo::Outgoing,
            },
        ),
        (CANCEL_ROW.to_string(), Action::OpenMogRoot),
    ];
    Menu {
        npc_id: MOG_MENU_ID,
        npc_name: MOG_MENU_NPC_NAME,
        prompt: DELIVERY_PROMPT.to_string(),
        rows,
    }
}

/// Row label for an occupied slot: retail's grid cell (item, count,
/// counterpart) plus the observed send-state suffixes "(preparing)"/"(sent)"
/// (artifacts/retail/moghouse-menu-notes.md).
/// One sendable LOC_INVENTORY stack offered by the item picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickableItem {
    pub inventory_slot: u8,
    pub item_no: u16,
    pub quantity: u32,
}

/// The recipient text-entry frame. No choice rows: the session treats an
/// empty `choices` frame with this prompt as free-text input (retail pops a
/// name-entry box over the Send panel).
fn recipient_entry_frame() -> DialogState {
    DialogState {
        event_id: MOG_MENU_ID,
        npc_id: MOG_MENU_ID,
        npc_name: Some(MOG_MENU_NPC_NAME.to_string()),
        act_index: 0,
        event_num: 0,
        event_para: 0,
        mode: 0,
        event_num2: 0,
        event_para2: 0,
        strings: Vec::new(),
        nums: Vec::new(),
        prompt: Some(RECIPIENT_PROMPT.to_string()),
        choices: Vec::new(),
        text_entry: true,
        grid: None,
        custom_menu: false,
    }
}

/// Quantity rows 1..=stack for a picked stack. Retail uses a numeric spinner;
/// the dialog list carries the same range.
fn quantity_menu(slot: u8, inventory_slot: u8, item_no: u16, quantity: u32) -> Menu {
    let rows = (1..=quantity)
        .map(|q| {
            (
                format!("{} x{q}", item_name(item_no)),
                Action::DeliveryStage {
                    slot,
                    inventory_slot,
                    quantity: q,
                },
            )
        })
        .chain(std::iter::once((
            BACK_ROW.to_string(),
            Action::DeliveryBack,
        )))
        .collect();
    Menu {
        npc_id: MOG_MENU_ID,
        npc_name: MOG_MENU_NPC_NAME,
        prompt: QUANTITY_PROMPT.to_string(),
        rows,
    }
}

fn delivery_slot_label(box_no: DeliveryBoxNo, item: &DeliveryItem) -> String {
    let name = item_name(item.item_no);
    let who = item.counterpart.as_deref().unwrap_or("?");
    match box_no {
        DeliveryBoxNo::Incoming => format!("{name} x{} — from {who}", item.quantity),
        DeliveryBoxNo::Outgoing => {
            let suffix = if item.sent() {
                " (sent)"
            } else {
                " (preparing)"
            };
            format!("{name} x{} — to {who}{suffix}", item.quantity)
        }
    }
}

/// Per-slot actions: inbox Take / Drop / Return (retail button order; Return
/// omitted for auction-house mail), outbox Send / Take back for staged items
/// or Cancel delivery for dispatched ones.
fn delivery_slot_menu(box_no: DeliveryBoxNo, slot: u8, item: &DeliveryItem) -> Menu {
    let mut rows: Vec<(String, Action)> = Vec::new();
    match box_no {
        DeliveryBoxNo::Incoming => {
            rows.push((TAKE_ROW.to_string(), Action::DeliveryTake { box_no, slot }));
            rows.push((
                DROP_ROW.to_string(),
                Action::Delivery {
                    op: DeliveryBoxOp::Clear { box_no, slot },
                },
            ));
            let from_ah = item
                .counterpart
                .as_deref()
                .is_some_and(|s| s.starts_with(AH_SENDER_PREFIX));
            if !from_ah {
                rows.push((
                    RETURN_ROW.to_string(),
                    Action::Delivery {
                        op: DeliveryBoxOp::Reject { slot },
                    },
                ));
            }
        }
        DeliveryBoxNo::Outgoing => {
            if item.sent() {
                rows.push((
                    CANCEL_DELIVERY_ROW.to_string(),
                    Action::Delivery {
                        op: DeliveryBoxOp::Cancel { slot },
                    },
                ));
            } else {
                rows.push((
                    SEND_ITEM_ROW.to_string(),
                    Action::Delivery {
                        op: DeliveryBoxOp::Send { slot },
                    },
                ));
                rows.push((
                    TAKE_BACK_ROW.to_string(),
                    Action::Delivery {
                        op: DeliveryBoxOp::Get { box_no, slot },
                    },
                ));
            }
        }
    }
    rows.push((CANCEL_ROW.to_string(), Action::DeliveryBack));
    Menu {
        npc_id: MOG_MENU_ID,
        npc_name: MOG_MENU_NPC_NAME,
        prompt: delivery_slot_label(box_no, item),
        rows,
    }
}

fn job_type_menu(info: &JobInfoState) -> Menu {
    let mut rows = vec![(
        MAIN_JOB_ROW.to_string(),
        Action::OpenJobList { support: false },
    )];
    if info.sub_job_unlocked {
        rows.push((
            SUPPORT_JOB_ROW.to_string(),
            Action::OpenJobList { support: true },
        ));
    }
    rows.push((CANCEL_ROW.to_string(), Action::Close));
    Menu {
        npc_id: MOG_MENU_ID,
        npc_name: MOG_MENU_NPC_NAME,
        prompt: CHANGE_JOBS_ROW.to_string(),
        rows,
    }
}

fn job_list_menu(info: &JobInfoState, support: bool) -> Menu {
    let mut rows: Vec<(String, Action)> = Vec::new();
    for job in 1..=SELECTABLE_JOB_MAX {
        if info.unlocked & (1u32 << job) == 0 {
            continue;
        }
        let name = ffxi_proto::job_names::lookup(job as u16).unwrap_or("Unknown");
        let level = info
            .job_levels
            .get(job as usize)
            .copied()
            .unwrap_or_default();
        let marker = if job == info.mjob_no {
            " (main)"
        } else if job == info.sjob_no {
            " (support)"
        } else {
            ""
        };
        rows.push((
            format!("{name} Lv.{level}{marker}"),
            Action::PickJob { support, job },
        ));
    }
    rows.push((CANCEL_ROW.to_string(), Action::Close));
    Menu {
        npc_id: MOG_MENU_ID,
        npc_name: MOG_MENU_NPC_NAME,
        prompt: if support {
            "Select a support job.".to_string()
        } else {
            "Select a main job.".to_string()
        },
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(sub_map: u8, exit_bit: u8) -> MyRoomInfo {
        MyRoomInfo {
            model: 257,
            sub_map,
            exit_bit,
        }
    }

    fn pick(session: &mut LocalMenuSession, frame: &DialogState, label: &str) -> Advance {
        let idx = frame
            .choices
            .iter()
            .position(|c| c.starts_with(label))
            .unwrap_or_else(|| panic!("row `{label}` in {:?}", frame.choices));
        session.advance(Some(idx as u32))
    }

    fn job_info() -> JobInfoState {
        let mut job_levels = [0u8; ffxi_proto::decode::JobInfo::MAX_JOBTYPE];
        job_levels[1] = 75;
        job_levels[3] = 37;
        JobInfoState {
            mjob_no: 1,
            sjob_no: 3,
            unlocked: 0b1_1011,
            sub_job_unlocked: true,
            job_levels,
        }
    }

    #[test]
    fn synthetic_ids_stay_outside_lsb_unique_no_space() {
        // Largest LSB unique_no shape: (4<<28)|(zone<<12)|targid.
        let lsb_ceiling = (4u32 << 28) | (0xFFFu32 << 12) | 0xFFF;
        for id in [MH_DOOR_ENTITY_ID, MOG_MENU_ID] {
            assert!(id > lsb_ceiling, "0x{id:08X} collides with server id space");
        }
        assert_ne!(MH_DOOR_ENTITY_ID, MOG_MENU_ID);
    }

    /// Pins the LSB MYROOMEXITBIT/MYROOMEXITMODE contract
    /// (vendor/server/src/map/packets/c2s/0x05e_maprect.h:26-50).
    #[test]
    fn terminal_exit_rows_map_to_lsb_wire_pairs() {
        assert_eq!(MogHouseExit::Home { exit_bit: 1 }.wire_pair(), (1, 0));
        assert_eq!(MogHouseExit::Mog2F.wire_pair(), (0, 125));
        assert_eq!(MogHouseExit::Mog1F.wire_pair(), (0, 126));
        assert_eq!(MogHouseExit::MogGarden.wire_pair(), (0, 127));

        let mut s = LocalMenuSession::new();
        let f = s.open_mh_exit(&room(0, 1), Some(true));
        match pick(&mut s, &f, HOME_ROW) {
            Advance::Exit(kind) => assert_eq!(kind.wire_pair(), (1, 0)),
            _ => panic!("Home row must be a terminal exit"),
        }

        // Non-city MHs (exit_bit 0) echo bit 0 on the Home exit — retail derives
        // the bit from the zone (research/XiPackets/world/client/0x005E) and 0 is
        // MYROOMEXITBIT::Default in the LSB validator.
        let f = s.open_mh_exit(&room(0, 0), None);
        match pick(&mut s, &f, HOME_ROW) {
            Advance::Exit(kind) => assert_eq!(kind.wire_pair(), (0, 0)),
            _ => panic!("Home row must be a terminal exit"),
        }

        let f = s.open_mh_exit(&room(0, 1), Some(true));
        match pick(&mut s, &f, MOG_GARDEN_ROW) {
            Advance::Exit(kind) => assert_eq!(kind.wire_pair(), (0, 127)),
            _ => panic!("Mog Garden row must be a terminal exit"),
        }
    }

    #[test]
    fn change_floors_row_direction_and_gating() {
        let mut s = LocalMenuSession::new();

        let f = s.open_mh_exit(&room(ServerLoginMyroom::SUB_MAP_2F, 1), None);
        match pick(&mut s, &f, CHANGE_FLOORS_ROW) {
            Advance::Exit(kind) => assert_eq!(kind.wire_pair(), (0, 126), "2F goes down"),
            _ => panic!("2F must always show Change floors."),
        }

        let f = s.open_mh_exit(&room(0, 1), Some(true));
        match pick(&mut s, &f, CHANGE_FLOORS_ROW) {
            Advance::Exit(kind) => assert_eq!(kind.wire_pair(), (0, 125), "1F goes up"),
            _ => panic!("unlocked 1F must show Change floors."),
        }

        let f = s.open_mh_exit(&room(0, 1), None);
        assert!(
            !f.choices.iter().any(|c| c == CHANGE_FLOORS_ROW),
            "unknown 2F unlock hides the row (server rejection is silent)"
        );
        s.clear();
    }

    #[test]
    fn district_submenu_maps_slots_to_city_exits() {
        let mut s = LocalMenuSession::new();
        let f = s.open_mh_exit(&room(0, 3), None);
        let f = match pick(&mut s, &f, SELECT_AREA_ROW) {
            Advance::Frame(f) => f,
            _ => panic!("area row opens the district submenu"),
        };
        assert_eq!(
            f.choices,
            vec![
                "Windurst Waters",
                "Windurst Walls",
                "Port Windurst",
                "Windurst Woods",
                CANCEL_ROW,
            ]
        );
        match pick(&mut s, &f, "Windurst Walls") {
            Advance::Exit(kind) => assert_eq!(kind.wire_pair(), (3, 2)),
            _ => panic!("district row must be a terminal exit"),
        }
    }

    /// Pins every district row's label → (MyRoomExitBit, MyRoomExitMode) pair to
    /// LSB's destination-zone formulas — row order IS the destination, including
    /// the three irregular ones (Jeuno's Ru'Lude-first base, Whitegate's split
    /// Al Zahbi/Whitegate base, Adoulin's mode-2 Eastern special case)
    /// (vendor/server/src/map/packets/c2s/0x05e_maprect.cpp:88-135 + zone.h) —
    /// plus the `from_bit_slot` inverse and membership in the PacketValidator
    /// oneOf enums (0x05e_maprect.h:26-50), whose rejection is a silent drop.
    #[test]
    fn district_rows_match_lsb_destination_zone_formulas() {
        let expected: &[(u8, &[(&str, u8)])] = &[
            (
                1,
                &[
                    ("Southern San d'Oria", 1),
                    ("Northern San d'Oria", 2),
                    ("Port San d'Oria", 3),
                ],
            ),
            (
                2,
                &[
                    ("Bastok Mines", 1),
                    ("Bastok Markets", 2),
                    ("Port Bastok", 3),
                ],
            ),
            (
                3,
                &[
                    ("Windurst Waters", 1),
                    ("Windurst Walls", 2),
                    ("Port Windurst", 3),
                    ("Windurst Woods", 4),
                ],
            ),
            (
                4,
                &[
                    ("Ru'Lude Gardens", 1),
                    ("Upper Jeuno", 2),
                    ("Lower Jeuno", 3),
                    ("Port Jeuno", 4),
                ],
            ),
            (5, &[("Al Zahbi", 1), ("Aht Urhgan Whitegate", 2)]),
            (9, &[("Western Adoulin", 1), ("Eastern Adoulin", 2)]),
        ];
        const VALID_BITS: std::ops::RangeInclusive<u8> = 0..=9;
        const VALID_MODES: [u8; 8] = [0, 1, 2, 3, 4, 125, 126, 127];

        for &(bit, rows) in expected {
            let mut s = LocalMenuSession::new();
            let f = s.open_mh_exit(&room(0, bit), None);
            let f = match pick(&mut s, &f, SELECT_AREA_ROW) {
                Advance::Frame(f) => f,
                _ => panic!("area row opens the district submenu"),
            };
            let labels: Vec<&str> = rows.iter().map(|&(l, _)| l).collect();
            assert_eq!(
                f.choices[..rows.len()],
                labels[..],
                "row order is the destination for bit {bit}"
            );
            s.clear();

            for &(label, slot) in rows {
                let f = s.open_mh_exit(&room(0, bit), None);
                let f = match pick(&mut s, &f, SELECT_AREA_ROW) {
                    Advance::Frame(f) => f,
                    _ => panic!("area row opens the district submenu"),
                };
                let kind = match pick(&mut s, &f, label) {
                    Advance::Exit(kind) => kind,
                    _ => panic!("`{label}` must be a terminal exit"),
                };
                assert_eq!(kind.wire_pair(), (bit, slot), "{label}");
                assert_eq!(
                    MogHouseExit::from_bit_slot(bit, slot).wire_pair(),
                    (bit, slot),
                    "from_bit_slot must invert wire_pair for {label}"
                );
                let (b, m) = kind.wire_pair();
                assert!(VALID_BITS.contains(&b), "{label} bit {b} out of enum");
                assert!(VALID_MODES.contains(&m), "{label} mode {m} out of enum");
            }
        }
    }

    #[test]
    fn exit_bit_zero_hides_area_row() {
        let mut s = LocalMenuSession::new();
        let f = s.open_mh_exit(&room(0, 0), None);
        assert!(!f.choices.iter().any(|c| c == SELECT_AREA_ROW));
        assert!(f.choices.iter().any(|c| c == STAY_ROW));
        s.clear();
    }

    #[test]
    fn job_pick_produces_change_job() {
        let mut s = LocalMenuSession::new();
        let f = s.open_mog_menu(Some(job_info()), None);
        assert_eq!(f.npc_id, MOG_MENU_ID);
        let f = match pick(&mut s, &f, CHANGE_JOBS_ROW) {
            Advance::Frame(f) => f,
            _ => panic!("Change Jobs opens the type chooser"),
        };
        assert!(f.choices.iter().any(|c| c == SUPPORT_JOB_ROW));
        let f = match pick(&mut s, &f, MAIN_JOB_ROW) {
            Advance::Frame(f) => f,
            _ => panic!("Main Job opens the job list"),
        };
        assert!(
            f.choices[0].starts_with("Warrior Lv.75"),
            "raw level, no client-side halving: {:?}",
            f.choices
        );
        assert!(f.choices[0].ends_with("(main)"));
        match pick(&mut s, &f, "Warrior") {
            Advance::ChangeJob { main_job, sub_job } => {
                assert_eq!(main_job, Some(1));
                assert_eq!(sub_job, None);
            }
            _ => panic!("job pick must yield ChangeJob"),
        }
        assert!(!s.active());
    }

    #[test]
    fn support_row_gated_on_sub_job_unlocked() {
        let mut s = LocalMenuSession::new();
        let info = JobInfoState {
            sub_job_unlocked: false,
            ..job_info()
        };
        let f = s.open_mog_menu(Some(info), None);
        let f = match pick(&mut s, &f, CHANGE_JOBS_ROW) {
            Advance::Frame(f) => f,
            _ => panic!("Change Jobs opens the type chooser"),
        };
        assert!(!f.choices.iter().any(|c| c == SUPPORT_JOB_ROW));
        s.clear();
    }

    #[test]
    fn support_pick_fills_sub_job_only() {
        let mut s = LocalMenuSession::new();
        let f = s.open_mog_menu(Some(job_info()), None);
        let f = match pick(&mut s, &f, CHANGE_JOBS_ROW) {
            Advance::Frame(f) => f,
            _ => panic!(),
        };
        let f = match pick(&mut s, &f, SUPPORT_JOB_ROW) {
            Advance::Frame(f) => f,
            _ => panic!(),
        };
        match pick(&mut s, &f, "White Mage") {
            Advance::ChangeJob { main_job, sub_job } => {
                assert_eq!(main_job, None);
                assert_eq!(sub_job, Some(3));
            }
            _ => panic!("support pick must yield ChangeJob"),
        }
    }

    /// Retail top-level order as observed on HorizonXI 2026-07-17
    /// (artifacts/retail/moghouse-menu-notes.md).
    #[test]
    fn mog_menu_matches_retail_order() {
        let mut s = LocalMenuSession::new();
        let f = s.open_mog_menu(None, None);
        assert_eq!(
            f.choices,
            vec![
                STORAGE_ROW,
                DELIVERY_BOX_ROW,
                CHANGE_JOBS_ROW,
                GARDENING_ROW,
                LAYOUT_ROW,
                OPEN_MOG_HOUSE_ROW,
                REMODEL_ROW,
                CANCEL_ROW,
            ]
        );
        s.clear();
    }

    /// Storage is a submenu (retail): Safe, Safe 2, Storage, Locker, Satchel,
    /// Sack, Case, Wardrobe 1-8, each a terminal OpenStorage carrying its LSB
    /// CONTAINER_ID; Cancel returns to the root Mog Menu.
    #[test]
    fn storage_rows_open_their_containers_in_retail_order() {
        let mut s = LocalMenuSession::new();
        let f = s.open_mog_menu(None, None);
        let sub = match pick(&mut s, &f, STORAGE_ROW) {
            Advance::Frame(f) => f,
            _ => panic!("Storage opens the container submenu"),
        };
        assert_eq!(sub.prompt.as_deref(), Some(STORAGE_PROMPT));
        let expected: Vec<&str> = storage_rows()
            .iter()
            .map(|&(label, _)| label)
            .chain(std::iter::once(CANCEL_ROW))
            .collect();
        assert_eq!(sub.choices, expected);

        for (label, container) in storage_rows() {
            let mut s = LocalMenuSession::new();
            let f = s.open_mog_menu(None, None);
            let sub = match pick(&mut s, &f, STORAGE_ROW) {
                Advance::Frame(f) => f,
                _ => panic!(),
            };
            match pick(&mut s, &sub, label) {
                Advance::OpenStorage { container: c } => {
                    assert_eq!(c, container, "{label}");
                    assert_eq!(storage_row_container(label), Some(container));
                }
                _ => panic!("`{label}` must open storage"),
            }
            assert!(!s.active(), "storage rows close the menu");
        }
    }

    #[test]
    fn zero_capacity_hides_storage_rows() {
        use ffxi_proto::map::container::{LOC_MOGLOCKER, LOC_MOGSAFE, LOC_STORAGE};
        let mut caps = [0u16; 18];
        caps[LOC_MOGSAFE as usize] = 60;
        caps[LOC_STORAGE as usize] = 8;
        // Pre-ToAU era (e.g. Horizon): the locker capacity never arrives.
        assert_eq!(caps[LOC_MOGLOCKER as usize], 0);

        let mut s = LocalMenuSession::new();
        let f = s.open_mog_menu(None, Some(&caps));
        let sub = match pick(&mut s, &f, STORAGE_ROW) {
            Advance::Frame(f) => f,
            _ => panic!(),
        };
        assert!(sub.choices.iter().any(|c| c == MOG_SAFE_ROW));
        assert!(sub.choices.iter().any(|c| c == STORAGE_ROW));
        assert!(!sub.choices.iter().any(|c| c == MOG_LOCKER_ROW));
        assert!(!sub.choices.iter().any(|c| c == MOG_SAFE2_ROW));
        s.clear();
    }

    #[test]
    fn storage_cancel_returns_to_root() {
        let mut s = LocalMenuSession::new();
        let f = s.open_mog_menu(None, None);
        let sub = match pick(&mut s, &f, STORAGE_ROW) {
            Advance::Frame(f) => f,
            _ => panic!(),
        };
        match pick(&mut s, &sub, CANCEL_ROW) {
            Advance::Frame(root) => assert_eq!(root.choices, f.choices),
            _ => panic!("submenu Cancel must return to the root Mog Menu"),
        }
        assert!(s.active());
    }

    #[test]
    fn delivery_rows_open_their_boxes() {
        // The Delivery Box row opens a Receive/Send chooser (Receive default),
        // each row starting the open flow for its box.
        for (row, expected) in [
            (RECEIVE_ROW, DeliveryBoxNo::Incoming),
            (SEND_ROW, DeliveryBoxNo::Outgoing),
        ] {
            let mut s = LocalMenuSession::new();
            let f = s.open_mog_menu(None, None);
            let sub = match pick(&mut s, &f, DELIVERY_BOX_ROW) {
                Advance::Frame(f) => f,
                _ => panic!("Delivery Box opens the Receive/Send chooser"),
            };
            assert_eq!(sub.prompt.as_deref(), Some(DELIVERY_PROMPT));
            assert_eq!(sub.choices, vec![RECEIVE_ROW, SEND_ROW, CANCEL_ROW]);
            assert_eq!(sub.choices[0], RECEIVE_ROW, "Receive is the default row");
            match pick(&mut s, &sub, row) {
                Advance::DeliveryOpen { box_no } => assert_eq!(box_no, expected),
                _ => panic!("`{row}` must start the open flow"),
            }
            assert!(!s.active(), "protocol takes over until the flow settles");
        }
    }

    fn incoming_item(from: &str) -> DeliveryItem {
        DeliveryItem {
            item_no: 4869,
            quantity: 2,
            counterpart: Some(from.to_string()),
            stat: pbx::stat::INCOMING,
        }
    }

    #[test]
    fn delivery_box_menu_lists_slots_and_slot_actions() {
        let mut slots: [Option<DeliveryItem>; pbx::SLOT_COUNT] = Default::default();
        slots[1] = Some(incoming_item("Atti"));
        slots[4] = Some(incoming_item("AH-Jeuno"));

        let mut s = LocalMenuSession::new();
        let f = s.open_delivery_box(DeliveryBoxNo::Incoming, &slots);
        assert_eq!(f.prompt.as_deref(), Some(RECEIVE_PANEL_PROMPT));
        assert_eq!(
            f.choices.len(),
            pbx::SLOT_COUNT + 1,
            "all 8 slots focusable (even empty) + Cancel"
        );
        assert!(f.choices[0].ends_with(EMPTY_SLOT_SUFFIX));
        assert!(f.choices[1].contains("from Atti"));

        let sub = match pick(&mut s, &f, &f.choices[1].clone()) {
            Advance::Frame(f) => f,
            _ => panic!("occupied slot opens its action submenu"),
        };
        assert_eq!(
            sub.choices,
            vec![TAKE_ROW, DROP_ROW, RETURN_ROW, CANCEL_ROW],
            "retail Take / Drop / Return order"
        );
        match pick(&mut s, &sub, TAKE_ROW) {
            Advance::DeliveryTake { box_no, slot } => {
                assert_eq!(box_no, DeliveryBoxNo::Incoming);
                assert_eq!(slot, 1);
            }
            _ => panic!("Take must start the Accept→Get chain"),
        }

        // AH mail: Return is disabled (sender prefix rule).
        let f = s.open_delivery_box(DeliveryBoxNo::Incoming, &slots);
        let sub = match pick(&mut s, &f, &f.choices[4].clone()) {
            Advance::Frame(f) => f,
            _ => panic!(),
        };
        assert!(!sub.choices.iter().any(|c| c == RETURN_ROW));

        // Back returns to the box menu without touching the server.
        match pick(&mut s, &sub, CANCEL_ROW) {
            Advance::Frame(back) => assert_eq!(back.choices, f.choices),
            _ => panic!("slot Cancel must return to the box menu"),
        }
    }

    #[test]
    fn outbox_slot_actions_depend_on_sent_state() {
        let mut slots: [Option<DeliveryItem>; pbx::SLOT_COUNT] = Default::default();
        slots[0] = Some(DeliveryItem {
            stat: pbx::stat::STAGED,
            ..incoming_item("Atti")
        });
        slots[2] = Some(DeliveryItem {
            stat: pbx::stat::SENT,
            ..incoming_item("Atti")
        });

        let mut s = LocalMenuSession::new();
        let f = s.open_delivery_box(DeliveryBoxNo::Outgoing, &slots);
        // choices[0] is the recipient row; slot rows follow (empty slots
        // included, so slot N maps to choice N+1).
        assert!(f.choices[0].starts_with(RECIPIENT_ROW_PREFIX));
        assert!(f.choices[1].ends_with("(preparing)"));
        assert!(f.choices[2].ends_with(EMPTY_SLOT_SUFFIX));
        assert!(f.choices[3].ends_with("(sent)"));

        let sub = match pick(&mut s, &f, &f.choices[1].clone()) {
            Advance::Frame(f) => f,
            _ => panic!(),
        };
        assert_eq!(sub.choices, vec![SEND_ITEM_ROW, TAKE_BACK_ROW, CANCEL_ROW]);
        match pick(&mut s, &sub, SEND_ITEM_ROW) {
            Advance::Delivery {
                op: DeliveryBoxOp::Send { slot },
            } => assert_eq!(slot, 0),
            other => panic!("staged Send must dispatch, got {other:?}"),
        }

        let f = s.open_delivery_box(DeliveryBoxNo::Outgoing, &slots);
        let sub = match pick(&mut s, &f, &f.choices[3].clone()) {
            Advance::Frame(f) => f,
            _ => panic!(),
        };
        assert_eq!(sub.choices, vec![CANCEL_DELIVERY_ROW, CANCEL_ROW]);
    }

    #[test]
    fn delivery_box_cancel_closes_via_post_close() {
        let mut s = LocalMenuSession::new();
        let slots: [Option<DeliveryItem>; pbx::SLOT_COUNT] = Default::default();
        let f = s.open_delivery_box(DeliveryBoxNo::Incoming, &slots);
        match pick(&mut s, &f, CANCEL_ROW) {
            Advance::Delivery {
                op: DeliveryBoxOp::PostClose { box_no },
            } => assert_eq!(box_no, DeliveryBoxNo::Incoming),
            other => panic!("box Cancel must PostClose, got {other:?}"),
        }
        assert!(!s.active());
    }

    #[test]
    fn dismiss_and_out_of_range_close() {
        let mut s = LocalMenuSession::new();
        let f = s.open_mog_menu(None, None);
        assert!(matches!(s.advance(None), Advance::Close));
        assert!(!s.active());

        let f2 = s.open_mog_menu(None, None);
        assert_eq!(f.choices, f2.choices);
        assert!(matches!(s.advance(Some(99)), Advance::Close));
        assert!(!s.active());
    }
}
