use std::collections::{BTreeMap, VecDeque};

use bevy::prelude::Resource;

#[derive(Resource, Debug, Clone, Default)]
pub enum InputMode {
    #[default]
    World,

    Chat(ChatBuffer),

    Menu(MenuStack),

    QuickAction(QuickActionState),

    TargetAction(TargetActionState),

    Dialog(DialogCursor),

    PassiveCursor(PassiveCursorState),

    /// Retail sub-target confirm step: an action was chosen from a menu and
    /// the flashing sub-target cursor is asking "on whom?". Esc returns to
    /// `return_to`; confirm fires the action at `candidate`.
    SubTarget(SubTargetState),

    /// The dedicated delivery box screen is open and modal. Focus/selector
    /// state lives in `hud::delivery::DeliveryScreenState`; this variant just
    /// suppresses world movement/camera and routes keys to the delivery handler.
    DeliveryBox,

    /// The /check window owns input: arrows move its equipment grid, confirm on
    /// View Wares opens the target's bazaar, Esc closes. Cursor state lives in
    /// `hud::check_view::CheckTarget`.
    Check,

    /// A browsed bazaar's wares list. Cursor/quantity state lives in
    /// `hud::bazaar_view::BazaarScreenState`; Esc leaves the bazaar (c2s 0x104)
    /// and returns to the Check window.
    Bazaar,

    /// The Auction House counter UI is open and modal. Screen/cursor state
    /// lives in `hud::auction::AuctionScreenState`; the native input layer
    /// drives it and emits the Ah* `AgentCommand`s.
    Auction,

    /// An NPC shop window is open and modal. Focus/cursor state lives in
    /// `hud::shop::ShopScreenState`; the shop itself has no close packet, so
    /// cancelling out of this mode is what ends it.
    Shop,

    /// Mog House layout mode with no arrange window up: the arrange key actions go to the editor
    /// (`myroom_arrange`), never to menus, chat or movement
    /// (.agents/skills/retail-observe/references/2026-10-15-mog-house-arrange.md B2).
    Arrange,
}

/// The right mouse button's law (owner ruling, no capture: "RMB is supposed to be ESC/Cancel";
/// holding it - either button - IS the camera drag). Over a window surface a click press + release
/// that never engaged as a drag acts as the bound `NavCancel` key, so every surface keeps its own
/// Esc handler and none gains a mouse path. Two places already own RMB: World, where it feeds
/// drag-aim and item rotation (`mouse_camera_system` spends both only in that mode), which is why there nothing answers except an
/// open trade - the trade window does not change `InputMode` at all; and Chat/Arrange, where a stray cancel would eat typed text or
/// drop furniture mid-placement [I].
pub fn right_button_is_cancel(mode: &InputMode, trade_open: bool) -> bool {
    match mode {
        InputMode::World => trade_open,
        InputMode::Chat(_) | InputMode::Arrange => false,
        _ => true,
    }
}

/// The action pending behind a sub-target cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubTargetAction {
    Spell(u16),
    Ability(u16),
    WeaponSkill(u16),
    /// Ranged attack — targets an enemy, carries no id.
    Ranged,
    Item {
        container: u8,
        index: u8,
        item_no: u16,
    },
    /// "Switch Target": pick a different mob; confirm asks the server to move
    /// the battle target and it becomes the main target when the 0x058 lands.
    /// research/XiPackets/world/server/0x0058/README.md
    PickSub,
}

#[derive(Debug, Clone)]
pub struct SubTargetState {
    pub action: SubTargetAction,

    /// TARGETTYPE bitmask for the pending action (ffxi-proto valid_target).
    pub flags: u16,
    pub candidate_filter: Option<u16>,

    /// Entity currently under the sub-target cursor. None when no valid
    /// candidate exists in range (cursor parks on self only if SELF is valid).
    pub candidate: Option<u32>,

    /// Mode to restore on Esc (retail: back to the menu, cursor preserved).
    pub return_to: Box<InputMode>,

    /// "Switch Target" confirm sent a c2s ChangeTarget for this candidate: the
    /// picker holds until the server's 0x058 commits it into the main target
    /// (or the wait lapses), so the target frame only swaps on the server's
    /// word. Cycling to another candidate cancels the wait.
    /// research/XiPackets/world/server/0x0058/README.md
    pub pending_switch: Option<u32>,

    /// When `pending_switch` was armed; the wait lapses if no 0x058 lands.
    /// research/XiPackets/world/server/0x0058/README.md
    pub pending_since: Option<std::time::Instant>,
}

impl SubTargetState {
    pub fn open(action: SubTargetAction, flags: u16, return_to: InputMode) -> Self {
        Self {
            action,
            flags,
            candidate_filter: None,
            candidate: None,
            return_to: Box::new(return_to),
            pending_switch: None,
            pending_since: None,
        }
    }
}

pub const DIALOG_MAX_CHOICE: u32 = 7;

#[derive(Debug, Clone, Default)]
pub struct DialogCursor {
    pub cursor: u32,
    /// Line buffer for a free-text dialog frame (`DialogState::text_entry`,
    /// e.g. the delivery-box recipient prompt). `None` for choice/speech
    /// frames; initialized when the text-entry frame is first handled.
    pub entry: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ChatBuffer {
    pub text: String,

    /// How far back in [`ChatHistory`] Up/Down has paged. `None` is the fresh
    /// line being typed; `Some(0)` is the most recent submission.
    pub history_pos: Option<usize>,

    /// The line that was being typed when paging into history started, restored
    /// when Down walks back past the newest entry.
    pub draft: Option<String>,
}

impl ChatBuffer {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with_prefix(prefix: &str) -> Self {
        Self {
            text: prefix.to_string(),
            ..Self::default()
        }
    }

    /// Page one entry further back. No-op at the oldest entry or on an empty
    /// history.
    pub fn recall_older(&mut self, history: &ChatHistory) {
        let next = match self.history_pos {
            None => 0,
            Some(pos) => pos + 1,
        };
        let Some(entry) = history.get(next) else {
            return;
        };
        if self.history_pos.is_none() {
            self.draft = Some(std::mem::take(&mut self.text));
        }
        self.text = entry.to_string();
        self.history_pos = Some(next);
    }

    /// Page one entry forward, landing back on the stashed draft past the
    /// newest entry. No-op when not paging.
    pub fn recall_newer(&mut self, history: &ChatHistory) {
        let Some(pos) = self.history_pos else {
            return;
        };
        match pos
            .checked_sub(1)
            .and_then(|next| history.get(next).map(|entry| (next, entry.to_string())))
        {
            Some((next, entry)) => {
                self.text = entry;
                self.history_pos = Some(next);
            }
            None => {
                self.text = self.draft.take().unwrap_or_default();
                self.history_pos = None;
            }
        }
    }
}

/// Lines the player has submitted from the chat bar, newest first. Kept as a
/// resource so it outlives the per-open [`ChatBuffer`].
#[derive(Resource, Debug, Clone, Default)]
pub struct ChatHistory {
    entries: VecDeque<String>,
}

/// Retail's text input recalls a bounded number of past lines rather than the
/// whole session; 32 covers a fight's worth of commands without unbounded growth.
pub const CHAT_HISTORY_MAX: usize = 32;

impl ChatHistory {
    /// `index` 0 is the most recent submission.
    pub fn get(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Records a submitted line. Blank lines and an immediate repeat of the
    /// newest entry are dropped so paging stays useful when a command is spammed.
    pub fn push(&mut self, line: &str) {
        if line.trim().is_empty() || self.entries.front().is_some_and(|prev| prev == line) {
            return;
        }
        self.entries.push_front(line.to_string());
        self.entries.truncate(CHAT_HISTORY_MAX);
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MenuKind {
    Root,

    Config,

    Controls,

    Debug,

    Graphics,

    /// DLSS Config submenu pushed from the Graphics list's "DLSS Config" row:
    /// the quality tier plus the Neural Uplift block
    /// (graphics_settings::DLSS_CONFIG_SECTIONS).
    GraphicsDlss,

    /// Kuluu's flat spell list (all known spells, one page) - kept behind the
    /// Retail+ "Flat Magic List" toggle; retail has no such window.
    Magic,

    /// mgcmenu: retail's Magic category picker, eight fixed school rows in the
    /// jump-table display order (FFXiMain.dll retail-2026-09 RVA 0x13F750;
    /// vault main_menu.md §21). Picking a row pushes [`MenuKind::MagicGroup`].
    MagicGroups,

    /// The `magic` spell-list window for one school (payload = the client
    /// magicType number FFXiMain.dll's handler pushes after a category pick,
    /// vault main_menu.md §21); rows cast through the shared sub-target cursor.
    MagicGroup(u8),

    /// Kuluu's flat ability list (all known JA/WS/pet, one page) - kept behind
    /// the Retail+ "Flat Abilities List" toggle; retail has no such window.
    Abilities,

    /// abimenu: retail's Abilities category picker, five fixed rows in the
    /// handler's item order (FFXiMain.dll retail-2026-09 RVA 0x13FA70; vault
    /// main_menu.md §23). Picking a row pushes [`MenuKind::AbilitiesGroup`].
    AbilitiesGroups,

    /// The `ability` list window for one category (payload = positional index
    /// into `AbilityGroup::ALL`, the grounding kuluu already uses for its
    /// target-action groups; vault main_menu.md §23 notes per-row codes remain
    /// [I-pending]).
    AbilitiesGroup(u8),

    /// The Party window — one surface for the prtyN family: `prtyjoin`'s YES/NO
    /// rows while an invite is pending, else the member roster; retail's Party-row
    /// helper picks per party state (FFXiMain.dll retail-2026-09 RVA 0x2016E0;
    /// vault main_menu.md §24).
    Party,

    /// cmbmenu: the Synthesis row's launcher, two fixed rows - pick a bag material, or open the
    /// craft list (handler FFXiMain.dll retail-2026-09 RVA 0x1E29A0; vault main_menu.md §22/§28).
    SynthLauncher,

    /// The kuluu craft list behind launcher row 2 (the cmbHwin/cmbhlst pair, FFXiMain.dll
    /// retail-2026-09 RVA 0x1E29C1; vault main_menu.md §28): rows are recipes the bag can currently
    /// satisfy, selecting sends the resolved c2s 0x096 ask.
    CraftList,

    /// The trade window pair — retail opens `trade` (own side) and `gift`
    /// (partner view) together on either side of the transaction; kuluu mirrors
    /// both windows as one menu whose rows follow s2c 0x021/0x022/0x023/0x025
    /// (FFXiMain.dll retail-2026-09 vtable 0x335060 handler RVA 0x1515E0; vault
    /// main_menu.md §25).
    Trade,
    /// The Add-Item bag picker opened from one own-side slot row, staged into
    /// that trade slot by c2s TRADE_LIST (vault main_menu.md §25).
    TradePick {
        trade_index: u8,
    },

    /// The Search row family — one kuluu surface for retail's `searchma` form
    /// and the `scresult` model list; rows follow the select law (FFXiMain.dll
    /// retail-2026-09 vtable 0x33B900 handler RVA 0x20D340; vault main_menu.md
    /// §26).
    Search,

    /// The Link Shell row's window. Retail answers page-1 row 10 by handing the `link5` singleton
    /// to helper FFXiMain.dll retail-2026-09 RVA 0x1E4470 and always opening `menu    link5`; two
    /// rows, one per shell slot the server tracks (vault main_menu.md section 27).
    Linkshell,

    /// The `region` window behind page-1 row 11: its handler answers only select
    /// events, items 1..4 installing display modes 0..3 on the open window
    /// (FFXiMain.dll retail-2026-09 RVA 0x140780; vault main_menu.md §29). The
    /// payload is the installed mode — selects replace it in place, mirroring that
    /// install-in-place law rather than pushing another window.
    RegionInfo {
        mode: u8,
    },

    /// The `missionm` list behind page-2 row 1 (FFXiMain.dll retail-2026-09 RVA
    /// 0x22DB40; vault main_menu.md §30): rows follow the s2c 0x056 log state.
    MissionList,

    /// Kuluu's data-driven reading of the `bazaar` browser behind page-2 row 5
    /// (ungated name-open at FFXiMain.dll retail-2026-09 RVA 0x14E8A1, shared
    /// results-list class with `scresult`; vault main_menu.md §33): rows are the
    /// zone's PCs with LSB's entity-stream BazaarFlag; select sends c2s 0x105 for
    /// that seller and hands off to the bazaar browse screen.
    BazaarStalls,

    /// The bazaar comment editor page-2 row 5's `Edit Comment` raises: a small text
    /// field retail docks above the chat log (owner 2026-10-24). Bespoke surface, like
    /// `Map`; its draft lives on `DynamicMenu::bazaar_comment_draft` and what you
    /// commit goes out as c2s 0x0DE INSPECT_MESSAGE.
    BazaarComment,
    /// The `mcrmenu` macro-palette launcher: two rows = its handler's select
    /// items {1,2} (vault main_menu.md §34).
    MacrosPalette,
    /// The retail `configwi` page hub: twelve rows = handler item ids 1..12
    /// (vault main_menu.md §35).
    RetailConfig,
    /// One `configwi` page window, indexed by the hub row that opens it
    /// (`page` 0..11 = select-id 1..12 of handler FFXiMain.dll retail-2026-09
    /// RVA 0x196770; vault main_menu.md §47). Its rows are that window's own
    /// `windowpsNNN` option widgets, in DAT file order.
    RetailConfigPage(u8),
    /// A config sub-window reached from a page row rather than the hub: 0 =
    /// `conf5win`, 1 = `conf5w1`, 2 = `conf5w2`, 3 = `k1assign`, 4 = `k2assign`
    /// (name-opens in FFXiMain.dll retail-2026-09 RVA 0x199240 / 0x196E40 /
    /// 0x19B200; vault main_menu.md §47).
    RetailConfigSub(u8),
    /// The `faqsub` Help Desk panel opened by the page-2 row's trampoline
    /// (vault main_menu.md §36); three destinations per the transfer-message law.
    HelpDeskPanel,

    /// A Status detail window, opened beside the selector exactly as the retail
    /// statcom2 select handler dispatches it (FFXiMain.dll retail-2026-09 RVA
    /// 0x148D00 over the detail table RVA 0x3716B8; vault main_menu.md §59): re-opening
    /// the same row changes nothing, and a different row closes the previous
    /// detail first (helper RVA 0x148AC0). `entry` indexes `STATUS_ENTRIES` —
    /// whose order is statcom2's own display order (the record's block y-sorted
    /// item ids run 1,2,12,3,..., engine-side pin `STATCOM2_DISPLAY_ITEM_IDS`).
    /// The Play Time row never opens one of these: it runs `/playtime` instead
    /// (vault main_menu.md §59).
    StatusDetail {
        entry: u8,
    },
    /// `ut_menu`, opened by Status row id 11
    /// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md); its
    /// display-only `ut_point` readout stays up with it.
    UnityMenu,
    /// `quest00` as the Unity chat panel (`ut_menu` item 1).
    UnityChat,
    /// `quest00` as the Unity ranking chooser (`ut_menu` item 2).
    UnityRankingChooser,
    /// `quest01` listing one Unity ranking result set.
    UnityRanking {
        result_set: u8,
    },
    /// `arealist` as the Unity leader list (`ut_menu` item 3).
    UnityLeaders,
    /// `fp_cat`, opened by Status row id 13, with its `fp_info` and `bluehelp` companions.
    AlterEgoCategories,
    /// `fp_cat2`: the upgrades of one alter ego point category id.
    AlterEgoUpgrades {
        category: u16,
    },
    /// `merityn` confirming one alter ego upgrade id.
    AlterEgoConfirm {
        upgrade: u16,
    },
    /// A merit or job point window: the `merit1` chain from Status row id 7 and the `merit2ca`
    /// chain from row id 8
    /// (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).
    Merit(crate::hud::merit_windows::MeritWindow),
    /// A Mog House arrange editor window: `rmlo1`/`rmlo2`/`rmlo4`, the `bankmenu` shelf picker, the
    /// `inventor` furniture list or a `comyn` put-away question
    /// (.agents/skills/retail-observe/references/2026-10-15-mog-house-arrange.md B4).
    Arrange(crate::myroom_arrange::ArrangeWindow),
    /// The commenu Friend List widget's window; rows arrive with a local friend
    /// store in `.23.1` — LSB ships only black-list c2s 0x03c, no friends list
    /// (vault main_menu.md §37).
    FriendList,

    /// The `pupequip` variant of the Equipment row for a Puppetmaster: fourteen
    /// automaton slot rows fed by s2c 0x044 EXTENDED_JOB PUP
    /// (FFXiMain.dll retail-2026-09 RVA 0x1404D5/0x14051D; vault main_menu.md §39).
    AutomatonEquip,

    /// The `mp_mncst` variant of the Equipment row during a monstrosity session:
    /// the window catalog titles it Customization (76.DAT idx 165) and its DAT
    /// record carries only three unnamed chrome buttons, so kuluu renders the
    /// grounded title with no rows yet (vault main_menu.md §39).
    MpCustomize,

    /// The `myroom` variant of the Map row while the server holds the player in
    /// a Mog House: home-view ops panel whose DAT chunk carries captioned
    /// container widgets (vault main_menu.md §40).
    HomeView,

    /// The Mog House storage-box list page-2 row 4 (`View House`) name-opens while the player
    /// stands OUTSIDE a Mog House: retail's arm FFXiMain.dll retail-2026-09 RVA 0x14E8F5 opens
    /// `menu    mogcont`, whose handler (RVA 0x14EC40, vault main_menu.md §32) answers select items
    /// 1..0xF with one storage container each - Mog Safe, Mog Safe 2, Storage, Mog Locker, Mog
    /// Satchel, Mog Sack, Mog Case, then Mog Wardrobe and wardrobes 2-8. A bag the server's s2c
    /// 0x01C ITEM_MAX gives zero capacity draws dim and refuses its select (ur/65 law). Kuluu seats
    /// it in the command column, so opening it replaces Commands the way Missions does; inside a
    /// Mog House or rent-a-room the same row keeps serving `HomeView`'s ops panel instead.
    MogContainers,

    /// The `roomlist` window the myroom Remodel row opens (select id 7 → instance
    /// [0x106631e0] method .text FFXiMain.dll retail-2026-09 0x1D6890, tail jmp to the
    /// control's show slot +0x70 = .text 0x1D6820 name-opening `menu    roomlist`; vault
    /// main_menu.md §59 addendum 20/21). Five dynamic rows — captions are GetText
    /// table-12 strings the DAT slots do not carry, read at runtime (bead
    /// kuluu-o9qt7.13.5). Style rows confirm before they send MYROOM_IS Kind=Remodel.
    RoomList,

    /// The generic yes/no dialog a roomlist style row raises — retail runs its
    /// confirm helper (.text FFXiMain.dll retail-2026-09 0x210E30, prompt GetText
    /// table 12 id 451 "Make changes?") whose yes-callback IS the MYROOM_IS sender
    /// wrapper .text 0x1D6AB0. `style` = the Param2 byte pair carried to
    /// [`MenuKind::RoomList`] rows' send (615..618; vault §59 addendum 20/21).
    RoomListConfirm {
        style: u16,
    },

    /// The `jobchang` window drilled from myroom's Change Jobs row (name-open
    /// arm FFXiMain.dll retail-2026-09 RVA 0x145517; both paths through its
    /// predicate open the window — vault main_menu.md §53). The DAT record
    /// 'jobc' gives it exactly two widgets, captioned tokens 65 "Main Job" and
    /// 66 "Support Job"; each drills [`MenuKind::JobPick`].
    JobChange,

    /// One side of a home-view job change: every unlocked job from the folded
    /// s2c 0x01B JOB_INFO roster (`snap.jobs`, vault main_menu.md §53). Selecting
    /// sends c2s 0x100 MYROOM_JOB, whose LSB validator tests the pick against
    /// that same `jobs.unlocked` bitmask (vendor/server/src/map/packets/c2s/
    /// 0x100_myroom_job.cpp); the untouched side rides the packet's zero index.
    JobPick {
        main_pick: bool,
    },

    /// One keytop of the client-local macro store, edited through the long-
    /// form/mcrledit family (FFXiMain.dll retail-2026-09 RVA 0x1A5690 grid-chrome
    /// handler and 0x1A6180 line editor; vault main_menu.md §34/§41). The window
    /// lists the slot's command lines plus add/clear rows; typing while a line is
    /// being edited edits `MenuLevel::macro_line_edit` (kuluu collapses retail's
    /// mcrbedit/mcr1edit/mcr2edit/mcrledit family into this one drill [I-shape]).
    MacroEditor {
        block: u8,
        index: u8,
    },

    /// The `mcrselec` select window drilled from the launcher's Select Macro
    /// row: filled slots of the client-local store, 2 blocks x 20 keytops per
    /// the DAT grids 'mcrs'/'mcr7' (vault main_menu.md §41).
    MacroSelect,

    /// The `mcres20`/`mcresmn` result surface drilled from the launcher's
    /// Macro Result row — a ten-page candidate pager per handler FFXiMain.dll
    /// retail-2026-09 RVA 0x1A6EB0 (vault main_menu.md §41); grounded-empty
    /// until kuluu executes macros.
    MacroResult,

    /// The `mcresmn` options window a set's row raises out of the `mcres20` pager: eight blocks
    /// (`Edit this equipment set.` … `Set appearance of own equipment to that of this item set.`).
    /// `set` is the 0-based index into the two-hundred-set store.
    EquipSetOptions {
        set: u16,
    },

    /// One book's macro pallet - retail draws it as two stacked rows of ten keytop blocks
    /// (`mcr1pall`/`mcr2pall`, x=16 w=480 h=42 each), which kuluu collapses into one surface at
    /// that left-side record. `book` is the 0-based index into the forty-book store.
    MacroPallet {
        book: u8,
    },

    /// The `mcrselop` window a book's row raises beside the `mcrselec` pager: five
    /// blocks whose table-14 descriptions read `Edit macro book.` / `Edit name of
    /// macro book.` / `Copy macro book.` / `Paste macro book.` / `Delete macro
    /// book.`, under the caption band retail reads `Options`. `book` is the 0-based
    /// index into the forty-book store.
    MacroBookOptions {
        book: u8,
    },

    /// The `miss00` log window for one chain member — missionm's select tail
    /// name-opens it over itself (FFXiMain.dll retail-2026-09 RVA 0x22DB40; vault
    /// main_menu.md §30). `log` is the `MissionLog`/`xi.mission.log_id` chain index
    /// (`vendor/server/src/map/enums/mission_log.h`); `member` indexes that
    /// selector row's Left/Right cycle table (ur/64: "Left / Right by nation").
    MissionDetail {
        log: u8,
        member: u8,
    },

    /// One mission's text window — kuluu's answer to ur/80 ("clicking any mission
    /// leads to blank instead of mission description"): selecting a named row in
    /// the `miss00` log pushes this `(log, id)` payload and shows the install
    /// catalog's briefing for that mission. The retail sibling window is the
    /// unregistered 'miss' (`menu    missiton`, DAT geometry without a §1 registry
    /// record — vault main_menu.md §30), so kuluu reuses the `miss00` frame.
    MissionText {
        log: u8,
        id: u16,
    },

    /// The FIRST window the Quests row opens — owner capture ur/87 shows it before
    /// any quest list: two drill rows (Current, Completed) over a Records of
    /// Eminence block whose counters ride as grounded-empty readouts. Kuluu seats
    /// it on `quest00`'s frame like the mission family shares one record, and
    /// Commands stays open beside it (owner ruling ur/82-85).
    QuestMenu,

    /// The `quest00` list window under [`MenuKind::QuestMenu`]. The id-table handler
    /// resets the quest-window filter word before name-opening quest00 (FFXiMain.dll
    /// retail-2026-09 RVA 0x14E889 → runner, entry 0x4D → 0x1007D240; vault
    /// main_menu.md §31), so the hub hands that filter down as `completed`:
    /// false lists the incomplete half of the s2c 0x056 quest ports, true the
    /// complete half.
    /// Owner ruling (ur/91 round): one region shows at a time — the window opens
    /// on the first region with entries and steps regions with Left/Right over
    /// its caption arrows exactly like the mission logs do ('it should open with
    /// a < title >(Like Windurst) ... same as the misisons'); `region` carries
    /// the QuestLog index whose rows draw, named by
    /// `ffxi_vocab::quest_lattice::QUEST_AREA_NAMES`.
    QuestList {
        completed: bool,
        region: u8,
    },

    /// The `quest01` detail window — quest00's select tail name-opens it over
    /// itself with the row value (FFXiMain.dll retail-2026-09 RVA 0x2300EE; vault
    /// main_menu.md §31), so selects push this `(log, id)` payload.
    QuestDetail {
        log: u8,
        id: u16,
    },

    Items,

    /// The modern main-menu Key Item window opens on this page hub: retail lists
    /// its pages before any item, and the count comes from the install's own
    /// key-item table ([`hud::menu::KEY_ITEM_CATEGORY_LABELS`]). Selecting a row
    /// pushes [`MenuKind::KeyItemList`] for that page on the same `evitem` seat,
    /// so the one box relabels in place.
    KeyItems,

    /// One page list of the Key Items family (`category` indexes
    /// [`hud::menu::KEY_ITEM_CATEGORY_LABELS`]): the owned items the install's
    /// table files there, id order, page-sized and scrollable like every list
    /// since Currencies (ur/150).
    KeyItemList {
        category: u8,
    },

    /// ur/152: selecting a key item row asks first, exactly like the owner's
    /// capture ("missing examine item opened") - Yes/No rows on the same
    /// `evitem` seat as the rest of the family. Yes opens [`MenuKind::KeyItemText`].
    KeyItemExamine {
        id: u16,
    },

    /// ur/152 ("missing item description then opens"): the item's own `sc_item_`
    /// description lines read from the install DAT, retail's wrapping kept;
    /// an item whose block held no printable line shows the neutral empty note.
    KeyItemText {
        id: u16,
    },

    /// Retail Command Menu "Items": only items that can actually be used
    /// right now (LSB 0x037 semantics — see `hud::menu::item_usable_now`),
    /// each row firing Use directly instead of opening the full bag.
    UsableItems,

    /// Per-item context menu pushed from the Items window (retail's item
    /// submenu): Use / Take Out / Put in <bag> rows for the focused slot.
    ItemAction {
        container: u8,
        index: u8,
        item_no: u16,
    },

    /// Yes/No step under the Item submenu's Drop row, cursor defaulting to No.
    /// Confirming sends c2s 0x028 ITEM_DUMP for the whole stack.
    /// vendor/server/src/map/packets/c2s/0x028_item_dump.cpp
    ItemDropConfirm {
        container: u8,
        index: u8,
        item_no: u16,
        quantity: u32,
    },

    /// Retail's Log Out / Shut Down command rows answer through an in-game
    /// yes/no prompt before the action runs (FFXiMain.dll retail-2026-09
    /// RVA 0x160960 / 0x1609F0 route caption ids through text table 0xE into a
    /// shared system-menu singleton; its cancel companion sits at RVA 0x1609C0).
    PowerPrompt {
        shut_down: bool,
    },

    /// The Yes/No step under `conf6win`'s key-assignment reset row (select id 10): retail arms it through the generic dialog service object ([0x10664520], FFXiMain.dll retail-2026-09 RVA 0x210E30) with callback .text 0x196F30; the answer byte arrives at [esp+8] and a non-zero (Yes) tail-calls vtable slot +0x48 on both assignment panels — their factory reset, writing option ids 0x4A..=0x69 and 0x6A..=0x89 through wrapper RVA 0x193830 (vault main_menu.md §59 addenda 7–8; bead kuluu-o9qt7.20.1.1).
    KeyAssignResetPrompt,

    Equipment,

    Status,

    EquipSlot(u8),

    Communication,

    /// Browsable canned-emote list under Communication; rows come from the
    /// scraped LSB emote table, Job gated on the s2c 0x11A bits.
    EmoteList,

    /// The `abisortw` companion window retail opens **below** each rearrange-
    /// driven list — mode 0 ability (unused here), 1 emote, 2 mount — matching
    /// its handler's mode word `[this+0x14]` (FFXiMain.dll retail-2026-09
    /// `.text 0x223CF0`; registry record 256 flags 0x00080002). It carries the
    /// DAT caption (table 0x0E ids 927/997) and drives the list window's
    /// swap-state law (`[emote+0x6C]`, `.text 0x225EC0`/`.text 0x225E00`).
    AbilitySort {
        mode: u8,
    },

    /// Full-screen Map screen rendered by `hud::map_screen`: a full-screen map
    /// with a top-right command submenu (Markers / Wide Scan / Change Map). The
    /// stack only carries this level; the submode + cursor live in the bespoke
    /// `hud::map_screen::MapScreenState`, so the generic menu is suppressed while
    /// it is on top.
    Map,
}

#[derive(Debug, Clone)]
pub struct MenuLevel {
    pub kind: MenuKind,
    pub cursor: usize,
    /// While a macro command line is being typed (bead kuluu-o9qt7.19.2/§34's
    /// mcrledit law), the working buffer lives on this level — scoping cancel
    /// and commit to the window that opened it.
    pub macro_line_edit: Option<MacroLineBuffer>,
    /// The draft a `mcrselop` `Change Name` row is typing. It lives on the option window for the same
    /// reason as the line buffer above: Esc discards it there and Enter commits it there, through
    /// [`crate::hud::client_user_files::set_book_title`]. While it holds a draft that window's second
    /// row wears it (see `menu::macro_book_option_rows`).
    pub macro_book_name_edit: Option<String>,
    /// The remembered cursor this window opened on still waits for its rows, which bound it.
    pub clamp_pending: bool,
    /// Set when the window was created with its last DAT item unselectable.
    pub last_item_lock: Option<LastItemLock>,
}

impl MenuLevel {
    pub fn new(kind: MenuKind, cursor: usize) -> Self {
        Self {
            kind,
            cursor,
            macro_line_edit: None,
            macro_book_name_edit: None,
            clamp_pending: false,
            last_item_lock: None,
        }
    }

    /// Whether `row` is the window's unselectable last item.
    pub fn row_locked(&self, row: usize) -> bool {
        self.last_item_lock.and_then(LastItemLock::locked_row) == Some(row)
    }

    /// The cursor rests on the unselectable item, so the window ignores every key.
    pub fn cursor_locked(&self) -> bool {
        self.row_locked(self.cursor)
    }

    /// A direction key's move: onto the unselectable item, the cursor stays where it is.
    pub fn step_cursor_to(&mut self, row: usize) {
        if !self.row_locked(row) {
            self.cursor = row;
        }
    }
}

/// Which row a window's last DAT item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastItemRow {
    /// The window's record has not been read yet.
    Unresolved,
    Row(usize),
    /// The last DAT item is not one of the rows kuluu draws.
    Undrawn,
}

/// A window created with its last DAT item unselectable
/// (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md, "Last item
/// unselectable at creation"). Keys and the pointer cannot take it and it draws in the normal
/// colour; when the open-time cursor placement lands on it, it is an ordinary item instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastItemLock {
    /// The row the open-time cursor placement chose.
    pub open_cursor: usize,
    pub row: LastItemRow,
}

impl LastItemLock {
    /// The row that stays unselectable, once the record has told which it is.
    pub fn locked_row(self) -> Option<usize> {
        match self.row {
            LastItemRow::Row(row) if row != self.open_cursor => Some(row),
            _ => None,
        }
    }
}

/// Where a window puts its cursor each time it is opened, an open of a window that is already
/// open included (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCursor {
    /// DAT item id 1.
    FirstItem,
    /// The item that held the cursor when the window last closed; item 1 when nothing is
    /// remembered.
    Remembered,
}

/// The registry window a menu level draws and its cursor-at-open rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowCursor {
    pub window: &'static str,
    pub rule: OpenCursor,
}

/// Each remembered window's cursor as it last closed, for the session. A window that closed with
/// nothing to remember holds `None`, so one menu's records can be laid over another's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CursorMemory {
    closed: BTreeMap<&'static str, Option<usize>>,
}

impl CursorMemory {
    pub fn recall(&self, window: &str) -> Option<usize> {
        self.closed.get(window).copied().flatten()
    }

    pub fn record(&mut self, window: &'static str, cursor: Option<usize>) {
        self.closed.insert(window, cursor);
    }

    /// Takes every record `newer` holds.
    pub fn absorb(&mut self, newer: &CursorMemory) {
        for (&window, &cursor) in &newer.closed {
            self.closed.insert(window, cursor);
        }
    }

    /// Whether every record `newer` holds is already here.
    pub fn holds(&self, newer: &CursorMemory) -> bool {
        newer
            .closed
            .iter()
            .all(|(window, cursor)| self.closed.get(window) == Some(cursor))
    }
}

/// The answer the shared Yes/No confirm routine moves the cursor to once its window has opened:
/// item 2 (No) unless the prompt asks for Yes
/// (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmDefault {
    Yes,
    No,
}

/// Rows of a shared Yes/No confirm: item 1 answers Yes, item 2 No.
pub const CONFIRM_YES_ROW: usize = 0;
pub const CONFIRM_NO_ROW: usize = 1;

impl ConfirmDefault {
    pub const fn row(self) -> usize {
        match self {
            Self::Yes => CONFIRM_YES_ROW,
            Self::No => CONFIRM_NO_ROW,
        }
    }

    pub const fn is_yes(self) -> bool {
        matches!(self, Self::Yes)
    }
}

/// The Command menu's chat mode and ability category pickers.
pub const CHATCTRL_WINDOW: &str = "chatctrl";
pub const ABISELEC_WINDOW: &str = "abiselec";

/// One in-progress edit of a macro slot's command line (`line_no` `None` = an
/// appended new line). Enter commits the whole set through
/// `AgentCommand::CommitMacros`; Esc discards (retail's mcrledit commits with
/// its UI blip — FFXiMain.dll retail-2026-09 handler 0x1A6180 event arm calls
/// the 0x37E90 blip then the store commit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroLineBuffer {
    pub block: u8,
    pub index: u8,
    /// `None` while composing a brand-new line.
    pub line_no: Option<usize>,
    pub text: String,
}

/// Retail's Command Menu draws two columns: the left holds the parent level
/// (the Root command list at depth 1), the right the current/top level (a
/// preview of the highlighted category at depth 1). `active_pane` says which
/// column the cursor keys drive.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum Pane {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Default)]
pub struct MenuStack {
    pub levels: Vec<MenuLevel>,
    pub active_pane: Pane,
    /// The `-` keypress that opens the menu (Action::OpenMenu, handled in the
    /// input system that runs just before the text handler) also reaches the
    /// menu's `-` page-flip on the same frame. This absorbs exactly that opening
    /// press so the Commands menu lands on page 1, not page 2 (kuluu-bi1s.2).
    pub absorb_open_minus: bool,
    /// The session's window cursor memory as this menu knows it: what it was opened with plus
    /// every window that closed since.
    pub cursor_memory: CursorMemory,
}

impl MenuStack {
    pub fn root() -> Self {
        Self {
            levels: vec![MenuLevel::new(MenuKind::Root, 0)],
            active_pane: Pane::Right,
            absorb_open_minus: true,
            cursor_memory: CursorMemory::default(),
        }
    }

    /// The main menu, opened with the session's window memory.
    pub fn open_main_menu(memory: &CursorMemory) -> Self {
        let mut stack = Self::root();
        stack.cursor_memory = memory.clone();
        stack.levels.clear();
        stack.open_level(MenuKind::Root);
        stack
    }

    /// A menu built before the session's memory was at hand takes it, and its top window opens
    /// again on it.
    pub fn adopt_memory(&mut self, memory: &CursorMemory) {
        let own = std::mem::replace(&mut self.cursor_memory, memory.clone());
        self.cursor_memory.absorb(&own);
        if let Some(kind) = self.current().map(|level| level.kind) {
            self.push(kind);
        }
    }

    /// Where `kind`'s cursor goes as it opens, and whether its rows must still bound it.
    fn open_cursor(&self, kind: MenuKind) -> (usize, bool) {
        let Some(window) = crate::hud::menu::window_cursor(kind) else {
            return (0, false);
        };
        let remembered = match window.rule {
            OpenCursor::FirstItem => None,
            OpenCursor::Remembered => self.cursor_memory.recall(window.window),
        };
        let Some(cursor) = remembered else {
            return (0, false);
        };
        match crate::hud::menu::fixed_item_count(kind) {
            Some(count) => (cursor.min(count.saturating_sub(1)), false),
            None => (cursor, true),
        }
    }

    fn open_level(&mut self, kind: MenuKind) {
        let (cursor, clamp_pending) = self.open_cursor(kind);
        let mut level = MenuLevel::new(kind, cursor);
        level.clamp_pending = clamp_pending;
        level.last_item_lock = crate::hud::menu::locks_last_item(kind).then_some(LastItemLock {
            open_cursor: cursor,
            row: LastItemRow::Unresolved,
        });
        self.levels.push(level);
    }

    /// A window that closes leaves its cursor in the memory when its rule remembers one.
    fn record_close(&mut self, level: &MenuLevel) {
        let Some(window) = crate::hud::menu::window_cursor(level.kind) else {
            return;
        };
        if window.rule != OpenCursor::Remembered {
            return;
        }
        if let Some(cursor) = crate::hud::menu::remembered_close_cursor(level) {
            self.cursor_memory.record(window.window, cursor);
        }
    }

    fn close_top(&mut self) {
        if let Some(level) = self.levels.pop() {
            self.record_close(&level);
        }
    }

    /// Every open window closes, newest first, as the whole menu goes away.
    pub fn close_all(&mut self) {
        while !self.levels.is_empty() {
            self.close_top();
        }
    }

    /// Consume the one-shot open-`-` absorb flag; returns true the first time
    /// (the opening frame) and false thereafter.
    pub fn take_absorb_open_minus(&mut self) -> bool {
        std::mem::take(&mut self.absorb_open_minus)
    }

    pub fn current(&self) -> Option<&MenuLevel> {
        self.levels.last()
    }

    pub fn current_mut(&mut self) -> Option<&mut MenuLevel> {
        self.levels.last_mut()
    }

    /// Open `kind` the way retail opens a window: one name is at most one window.
    /// `OpenMenu` re-binds the existing instance when that name is already up
    /// (FFXiMain.dll retail-2026-09 RVA 0x15E350), so a second Commands press on
    /// Linkshell, Missions or Quests focuses the window that is already there instead
    /// of stacking another copy. Re-binding makes it the focused level again, which in
    /// this stack model means anything opened above it comes down. The cursor follows the
    /// window's open rule each time, a re-bound window included; kuluu-only menus keep theirs.
    pub fn push(&mut self, kind: MenuKind) {
        if let Some(existing) = self.levels.iter().position(|level| level.kind == kind) {
            while self.levels.len() > existing + 1 {
                self.close_top();
            }
            if crate::hud::menu::window_cursor(kind).is_some() {
                let (cursor, clamp_pending) = self.open_cursor(kind);
                let level = &mut self.levels[existing];
                level.cursor = cursor;
                level.clamp_pending = clamp_pending;
            }
            self.active_pane = Pane::Right;
            return;
        }
        self.open_level(kind);
        self.active_pane = Pane::Right;
    }

    /// The shared Yes/No confirm: its window opens by its own rule, then the cursor moves to the
    /// answer the prompt asks for.
    pub fn open_confirm(&mut self, kind: MenuKind, default: ConfirmDefault) {
        self.push(kind);
        if let Some(level) = self.current_mut() {
            level.cursor = default.row();
        }
    }

    pub fn pop(&mut self) -> bool {
        if self.levels.len() > 1 {
            self.close_top();
            self.active_pane = Pane::Right;
            true
        } else {
            false
        }
    }

    /// Level index a pane renders. Right = the top level; Left = its parent
    /// (or, at depth 1, the Root list itself). Both collapse to level 0 for a
    /// single-level stack, so Root navigation acts on the same cursor either
    /// way while the right column shows a non-interactive preview.
    pub fn pane_level_index(&self, pane: Pane) -> usize {
        match pane {
            Pane::Right => self.levels.len().saturating_sub(1),
            Pane::Left => self.levels.len().saturating_sub(2),
        }
    }

    pub fn active_level_index(&self) -> usize {
        self.pane_level_index(self.active_pane)
    }

    pub fn active_level(&self) -> Option<&MenuLevel> {
        self.levels.get(self.active_level_index())
    }

    pub fn active_level_mut(&mut self) -> Option<&mut MenuLevel> {
        let idx = self.active_level_index();
        self.levels.get_mut(idx)
    }
}

#[derive(Debug, Clone, Default)]
pub struct QuickActionState {
    pub cursor: usize,
    pub has_target: bool,
}

impl QuickActionState {
    pub fn for_target(has_target: bool) -> Self {
        Self {
            cursor: 0,
            has_target,
        }
    }
}

/// Which on-screen window the "active window" cursor (retail's Select-active-
/// window / F key) is focused on. The cycle order below matches how F steps
/// through the windows; World (unfocused) is the wrap-around resting state.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum PassiveCursorFocus {
    #[default]
    Chat,
    StatusIcons,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PassiveCursorState {
    pub focus: PassiveCursorFocus,

    /// Selection index into the status-icon ribbon while `focus == StatusIcons`.
    pub status_cursor: usize,

    /// Chat log expanded to the full-screen scrollback window (retail: confirm
    /// on the focused log window expands it, cancel contracts).
    pub chat_expanded: bool,
}

impl PassiveCursorState {
    pub fn fresh_chat() -> Self {
        Self {
            focus: PassiveCursorFocus::Chat,
            status_cursor: 0,
            chat_expanded: false,
        }
    }

    pub fn fresh_status() -> Self {
        Self {
            focus: PassiveCursorFocus::StatusIcons,
            status_cursor: 0,
            chat_expanded: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TargetActionState {
    pub cursor: usize,
    pub ctx: crate::hud::action_model::TargetActionContext,
    pub sub: Option<SubActionStack>,

    pub chat_mode_idx: usize,

    pub abilities_group_idx: usize,

    pub dismount_confirm: bool,
}

impl TargetActionState {
    // FFXiMain.dll retail-2026-09 VA 0x10210ECB selects row 2; VA 0x10211398 accepts only row 1.
    pub const DISMOUNT_NO_ROW: usize = 1;

    pub fn confirm_dismount(&mut self) {
        self.dismount_confirm = true;
        self.cursor = Self::DISMOUNT_NO_ROW;
    }

    pub fn open(ctx: crate::hud::action_model::TargetActionContext) -> Self {
        Self {
            cursor: 0,
            ctx,
            sub: None,
            chat_mode_idx: 0,
            abilities_group_idx: 0,
            dismount_confirm: false,
        }
    }

    /// The chat mode and ability category pickers open where they last closed, inside the
    /// choices they offer now.
    pub fn recall_pickers(
        &mut self,
        memory: &CursorMemory,
        chat_modes: usize,
        ability_groups: usize,
    ) {
        let recall = |window: &str, count: usize| {
            memory
                .recall(window)
                .map_or(0, |cursor| cursor.min(count.saturating_sub(1)))
        };
        self.chat_mode_idx = recall(CHATCTRL_WINDOW, chat_modes);
        self.abilities_group_idx = recall(ABISELEC_WINDOW, ability_groups);
    }

    /// The pickers close with the Command menu and leave their cursors behind.
    pub fn record_pickers(&self, memory: &mut CursorMemory) {
        memory.record(CHATCTRL_WINDOW, Some(self.chat_mode_idx));
        memory.record(ABISELEC_WINDOW, Some(self.abilities_group_idx));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAction {
    MagicCategory(crate::hud::overlay::SpellCategory),

    AbilitiesGroup(crate::hud::action_model::AbilityGroup),

    Items,

    ChatCompose,
}

#[derive(Debug, Clone, Default)]
pub struct SubActionStack {
    pub frames: Vec<SubAction>,
    pub cursor: usize,
}

impl SubActionStack {
    pub fn with(frame: SubAction) -> Self {
        Self {
            frames: vec![frame],
            cursor: 0,
        }
    }

    pub fn current(&self) -> Option<SubAction> {
        self.frames.last().copied()
    }

    pub fn push(&mut self, frame: SubAction) {
        self.frames.push(frame);
        self.cursor = 0;
    }

    pub fn pop(&mut self) -> bool {
        if self.frames.pop().is_some() {
            self.cursor = 0;
            !self.frames.is_empty()
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absorb_open_minus_fires_once_then_clears() {
        let mut stack = MenuStack::root();
        assert!(
            stack.take_absorb_open_minus(),
            "opening frame absorbs the '-'"
        );
        assert!(
            !stack.take_absorb_open_minus(),
            "subsequent '-' presses flip pages normally"
        );
    }

    #[test]
    fn pushed_stack_does_not_absorb() {
        let mut stack = MenuStack::default();
        assert!(!stack.take_absorb_open_minus());
    }

    /// One window name is one window: a second open of the same kind re-binds the
    /// level already on the stack rather than adding a copy, and it keeps its own
    /// cursor (FFXiMain.dll retail-2026-09 RVA 0x15E350 re-binds the instance).
    #[test]
    fn reopening_a_kind_rebinds_rather_than_stacking() {
        let mut stack = MenuStack::root();
        let depth_before = stack.levels.len();
        stack.push(MenuKind::Controls);
        stack.current_mut().unwrap().cursor = 4;

        stack.push(MenuKind::Controls);

        assert_eq!(stack.levels.len(), depth_before + 1, "no second copy");
        assert_eq!(stack.levels.last().unwrap().kind, MenuKind::Controls);
        assert_eq!(
            stack.current().unwrap().cursor,
            4,
            "the re-bound window keeps where its cursor was"
        );
    }

    /// Re-binding also brings that level back to the top: whatever had been opened
    /// above it is gone, which is what makes a repeated Commands press land on the
    /// single window instead of a pile of them.
    #[test]
    fn rebinding_brings_the_level_back_to_the_top() {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Controls);
        stack.push(MenuKind::Graphics);
        assert_eq!(stack.levels.last().unwrap().kind, MenuKind::Graphics);

        stack.push(MenuKind::Controls);

        assert_eq!(stack.current().unwrap().kind, MenuKind::Controls);
        assert!(
            !stack
                .levels
                .iter()
                .skip(1)
                .any(|level| level.kind == MenuKind::Graphics),
            "the window opened above it came down with the re-bind"
        );
    }

    fn cursor_of(stack: &MenuStack) -> usize {
        stack.current().expect("an open window").cursor
    }

    /// An item-1 window puts the cursor on item 1 at every open, an open of the window that is
    /// already open included
    /// (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md).
    #[test]
    fn item_one_windows_open_on_item_one_every_time() {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::QuestMenu);
        stack.current_mut().unwrap().cursor = 3;
        stack.pop();
        stack.push(MenuKind::QuestMenu);
        assert_eq!(cursor_of(&stack), 0, "reopened on item 1");
        stack.current_mut().unwrap().cursor = 2;
        stack.push(MenuKind::QuestMenu);
        assert_eq!(cursor_of(&stack), 0, "an open while open lands on item 1");
    }

    #[test]
    fn remembered_windows_reopen_where_they_last_closed() {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::MagicGroups);
        assert_eq!(cursor_of(&stack), 0, "nothing remembered yet: item 1");
        stack.current_mut().unwrap().cursor = 5;
        stack.pop();
        stack.push(MenuKind::MagicGroups);
        assert_eq!(cursor_of(&stack), 5);
        stack.current_mut().unwrap().cursor = 2;
        stack.push(MenuKind::MagicGroups);
        assert_eq!(
            cursor_of(&stack),
            5,
            "an open while open jumps to the remembered item, not the current one"
        );
    }

    /// Windows opened above a window that opens again close, and remember as they go.
    #[test]
    fn windows_closed_by_a_reopen_below_them_remember_their_cursor() {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Communication);
        stack.push(MenuKind::MagicGroups);
        stack.current_mut().unwrap().cursor = 6;
        stack.push(MenuKind::Communication);
        stack.push(MenuKind::MagicGroups);
        assert_eq!(cursor_of(&stack), 6);
    }

    #[test]
    fn a_remembered_cursor_is_clamped_to_the_item_count() {
        let window = crate::hud::menu::registry_window(MenuKind::MagicGroups).unwrap();
        let count = crate::hud::menu::fixed_item_count(MenuKind::MagicGroups).unwrap();
        let mut memory = CursorMemory::default();
        memory.record(window, Some(count + 5));
        let mut stack = MenuStack::open_main_menu(&memory);
        stack.push(MenuKind::MagicGroups);
        assert_eq!(cursor_of(&stack), count - 1);
        assert!(!stack.current().unwrap().clamp_pending);

        let list = crate::hud::menu::registry_window(MenuKind::MagicGroup(1)).unwrap();
        memory.record(list, Some(count + 5));
        let mut stack = MenuStack::open_main_menu(&memory);
        stack.push(MenuKind::MagicGroup(1));
        let level = stack.current().unwrap();
        assert_eq!(
            level.cursor,
            count + 5,
            "a built list bounds it once its rows exist"
        );
        assert!(level.clamp_pending);
    }

    /// kuluu's one Root level stands for both retail main menu pages: it reopens where it last
    /// closed on either, and closing on kuluu's own third page leaves that memory alone.
    #[test]
    fn the_main_menu_opens_where_it_last_closed_on_a_retail_page() {
        let page_two = crate::hud::menu::root_next_page_cursor(0);
        let page_three = crate::hud::menu::root_next_page_cursor(page_two);
        let mut stack = MenuStack::root();
        stack.levels[0].cursor = page_two + 1;
        stack.close_all();
        let mut reopened = MenuStack::open_main_menu(&stack.cursor_memory);
        assert_eq!(cursor_of(&reopened), page_two + 1);

        reopened.levels[0].cursor = page_three;
        reopened.close_all();
        let again = MenuStack::open_main_menu(&reopened.cursor_memory);
        assert_eq!(cursor_of(&again), page_two + 1);
    }

    /// A menu built before the memory was at hand reopens its top window on it, and keeps what it
    /// recorded itself.
    #[test]
    fn a_menu_built_without_the_memory_adopts_it() {
        let window = crate::hud::menu::registry_window(MenuKind::HomeView).unwrap();
        let mut memory = CursorMemory::default();
        memory.record(window, Some(4));
        let mut stack = MenuStack::root();
        stack.push(MenuKind::MagicGroups);
        stack.current_mut().unwrap().cursor = 3;
        stack.pop();
        stack.push(MenuKind::HomeView);
        assert_eq!(cursor_of(&stack), 0);
        stack.adopt_memory(&memory);
        assert_eq!(cursor_of(&stack), 4);
        let magic = crate::hud::menu::registry_window(MenuKind::MagicGroups).unwrap();
        assert_eq!(stack.cursor_memory.recall(magic), Some(3));
    }

    fn lock(open_cursor: usize, row: usize) -> Option<LastItemLock> {
        Some(LastItemLock {
            open_cursor,
            row: LastItemRow::Row(row),
        })
    }

    /// The last item a window made unselectable at creation refuses a direction key's move onto
    /// it; every other row still takes the cursor
    /// (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md).
    #[test]
    fn the_unselectable_last_item_refuses_the_cursor() {
        let mut level = MenuLevel::new(MenuKind::RetailConfig, 0);
        level.last_item_lock = lock(0, 3);
        level.step_cursor_to(3);
        assert_eq!(
            level.cursor, 0,
            "the move onto it leaves the cursor where it was"
        );
        level.step_cursor_to(2);
        assert_eq!(level.cursor, 2);
        assert!(level.row_locked(3) && !level.row_locked(2));
        assert!(!level.cursor_locked());
    }

    /// When the open-time cursor placement lands on the last item it is an ordinary item; a
    /// cursor put on it afterwards rests on an unselectable item.
    #[test]
    fn an_open_that_lands_on_the_last_item_makes_it_ordinary() {
        let mut level = MenuLevel::new(MenuKind::RetailConfig, 3);
        level.last_item_lock = lock(3, 3);
        assert!(!level.row_locked(3));
        assert!(!level.cursor_locked());
        level.step_cursor_to(1);
        level.step_cursor_to(3);
        assert_eq!(level.cursor, 3, "an ordinary item takes the cursor back");

        let mut placed = MenuLevel::new(MenuKind::RetailConfig, 0);
        placed.last_item_lock = lock(0, 3);
        placed.cursor = 3;
        assert!(placed.cursor_locked());
    }

    /// The lock comes with the window's creation, not with an open of the window already open,
    /// and only for windows the record lists.
    #[test]
    fn only_creation_locks_the_last_item() {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfig);
        assert_eq!(
            stack.current().unwrap().last_item_lock,
            Some(LastItemLock {
                open_cursor: 0,
                row: LastItemRow::Unresolved,
            })
        );
        stack.current_mut().unwrap().last_item_lock = None;
        stack.push(MenuKind::RetailConfig);
        assert_eq!(stack.current().unwrap().last_item_lock, None);
        stack.push(MenuKind::QuestList {
            completed: false,
            region: 0,
        });
        assert_eq!(stack.current().unwrap().last_item_lock, None);
    }

    #[test]
    fn a_window_closing_on_its_unselectable_item_remembers_nothing() {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfig);
        stack.current_mut().unwrap().cursor = 2;
        stack.pop();
        stack.push(MenuKind::RetailConfig);
        assert_eq!(cursor_of(&stack), 2);
        {
            let level = stack.current_mut().unwrap();
            level.last_item_lock = lock(2, 4);
            level.cursor = 4;
        }
        stack.pop();
        stack.push(MenuKind::RetailConfig);
        assert_eq!(cursor_of(&stack), 0, "nothing remembered: item 1");
    }

    /// The shared confirm opens its window by the window's rule, then moves to the answer the
    /// prompt asks for: No unless it asks for Yes.
    #[test]
    fn shared_confirms_open_on_the_answer_the_prompt_asks_for() {
        let mut stack = MenuStack::root();
        stack.open_confirm(
            MenuKind::PowerPrompt { shut_down: false },
            ConfirmDefault::No,
        );
        assert_eq!(cursor_of(&stack), CONFIRM_NO_ROW);
        stack.pop();
        stack.open_confirm(
            MenuKind::PowerPrompt { shut_down: false },
            ConfirmDefault::Yes,
        );
        assert_eq!(cursor_of(&stack), CONFIRM_YES_ROW);
    }

    #[test]
    fn command_menu_pickers_reopen_where_they_closed() {
        const CHAT_MODES: usize = 4;
        const GROUPS: usize = 5;
        let mut state = TargetActionState::open(Default::default());
        state.chat_mode_idx = 2;
        state.abilities_group_idx = 3;
        let mut memory = CursorMemory::default();
        state.record_pickers(&mut memory);
        let mut reopened = TargetActionState::open(Default::default());
        reopened.recall_pickers(&memory, CHAT_MODES, GROUPS);
        assert_eq!(reopened.chat_mode_idx, 2);
        assert_eq!(reopened.abilities_group_idx, 3);
        assert_eq!(
            reopened.cursor, 0,
            "the Command menu itself opens on item 1"
        );

        reopened.recall_pickers(&memory, 1, 1);
        assert_eq!(
            (reopened.chat_mode_idx, reopened.abilities_group_idx),
            (0, 0),
            "clamped to the choices on offer"
        );
    }

    fn history_of(lines: &[&str]) -> ChatHistory {
        let mut history = ChatHistory::default();
        for line in lines {
            history.push(line);
        }
        history
    }

    #[test]
    fn history_pages_newest_first_and_clamps_at_the_oldest() {
        let history = history_of(&["/heal", "hello", "/tell Zilart hi"]);
        let mut buffer = ChatBuffer::empty();

        buffer.recall_older(&history);
        assert_eq!(buffer.text, "/tell Zilart hi");
        buffer.recall_older(&history);
        assert_eq!(buffer.text, "hello");
        buffer.recall_older(&history);
        assert_eq!(buffer.text, "/heal");

        buffer.recall_older(&history);
        assert_eq!(buffer.text, "/heal", "oldest entry holds");
    }

    #[test]
    fn paging_forward_restores_the_stashed_draft() {
        let history = history_of(&["/heal"]);
        let mut buffer = ChatBuffer::empty();
        buffer.text = "half typed".into();

        buffer.recall_older(&history);
        assert_eq!(buffer.text, "/heal");

        buffer.recall_newer(&history);
        assert_eq!(buffer.text, "half typed");
        assert_eq!(buffer.history_pos, None);

        buffer.recall_newer(&history);
        assert_eq!(buffer.text, "half typed", "already at the fresh line");
    }

    #[test]
    fn empty_history_leaves_the_line_alone() {
        let history = ChatHistory::default();
        let mut buffer = ChatBuffer::empty();
        buffer.text = "typing".into();

        buffer.recall_older(&history);
        buffer.recall_newer(&history);

        assert_eq!(buffer.text, "typing");
        assert_eq!(buffer.history_pos, None);
        assert_eq!(buffer.draft, None);
    }

    #[test]
    fn blank_and_repeated_lines_are_not_recorded() {
        let history = history_of(&["/heal", "/heal", "   ", ""]);
        assert_eq!(history.len(), 1);
        assert_eq!(history.get(0), Some("/heal"));
    }

    #[test]
    fn history_is_capped_at_the_max() {
        let lines: Vec<String> = (0..CHAT_HISTORY_MAX + 8)
            .map(|i| format!("/l {i}"))
            .collect();
        let mut history = ChatHistory::default();
        for line in &lines {
            history.push(line);
        }

        assert_eq!(history.len(), CHAT_HISTORY_MAX);
        assert_eq!(history.get(0).map(str::to_string), lines.last().cloned());
        assert_eq!(history.get(CHAT_HISTORY_MAX), None);
    }

    #[test]
    fn right_button_cancels_every_window_surface() {
        for mode in [
            InputMode::Dialog(DialogCursor::default()),
            InputMode::DeliveryBox,
            InputMode::Check,
            InputMode::Bazaar,
            InputMode::Auction,
            InputMode::Shop,
        ] {
            assert!(right_button_is_cancel(&mode, false), "{mode:?}");
        }
    }

    #[test]
    fn right_button_yields_to_the_modes_that_already_own_it() {
        // At rest World spends the RMB drag on camera and rotation.
        assert!(!right_button_is_cancel(&InputMode::World, false));
        // The trade window rides World mode without changing it, so over a
        // trade there is no other owner and the button closes it like Esc.
        assert!(right_button_is_cancel(&InputMode::World, true));
        // Typed text and mid-placement furniture are not cancelled by a stray press.
        assert!(!right_button_is_cancel(
            &InputMode::Chat(ChatBuffer::empty()),
            false
        ));
        assert!(!right_button_is_cancel(&InputMode::Arrange, false));
    }
}
