use bevy::prelude::*;
use bevy::ui::ComputedNode;

use crate::graphics_settings::{
    config_rows, dlss_config_rows, graphics_rows, GraphicsField, GraphicsSection, GraphicsSettings,
    MenuRow, CONFIG_SECTIONS, DLSS_CONFIG_SECTIONS, GRAPHICS_SECTIONS,
};
use crate::hud::gettext::GetText;
use crate::hud::style::{self, theme};
use crate::input_mode::{
    CursorMemory, InputMode, LastItemRow, MenuKind, MenuLevel, MenuStack, OpenCursor,
    TargetActionState, WindowCursor,
};

pub const ROOT_LOG_OUT: &str = "Log Out";
pub const ROOT_SHUT_DOWN: &str = "Shut Down";
// Caption strings verbatim from the client's UI caption DAT (ROM/165/76.DAT
// d_msg entries), which also carries `Commands` as the window title.
pub const ROOT_CURRENT_TIME: &str = "Current Time";
pub const ROOT_COMMUNICATION: &str = "Communication";
pub const COMM_EMOTE_LIST: &str = "Emote List";
/// Friend List row — the `com1` DAT carries windowps330 with caption token 61
/// = catalog string `Friend List`, listed before windowps928's `Emote List`
/// (token 180) (vault main_menu.md §37).
pub const COMM_FRIEND_LIST: &str = "Friend List";
/// Page-2 row 4. The caption is catalog id 107, which the `mogcont` window
/// record itself carries as its comwin title token (vault main_menu.md §32).
pub const ROOT_VIEW_HOUSE: &str = "View House";
/// Page-1 row 11 — the four-mode Region Info panel's selector label.
pub const ROOT_REGION_INFO: &str = "Region Info";
/// Page-1 row 3 — the menuwind item's short-caption id resolves to `Magic
/// List` (UI string catalog; pinned by ffxi-dat's
/// real_selector_row_strings_in_display_order), not kuluu's earlier `Magic`.
pub const ROOT_MAGIC_LIST: &str = "Magic List";
/// Page-2 row 5 — the socialme item's caption is `Set Bazaar` (same pin).
pub const ROOT_SET_BAZAAR: &str = "Set Bazaar";
/// Page 1's Map row, whose Mog House variant is `myroom`.
pub const ROOT_MAP: &str = "Map";
/// Page-2 row 6 — the caption id resolves to the literal `MacroPalette`; the
/// earlier kuluu string `Edit Macros` was invented and exists nowhere in the
/// catalog (same pin).
pub const ROOT_MACRO_PALETTE: &str = "MacroPalette";

/// Retail's Command menu (windows `menuwind`/`socialme`) shows twelve rows per
/// page across two pages; kuluu appends a third page of its own entries. "-"
/// and Right advance 1→2→3→1, Left steps back. A flip between the retail pages
/// keeps the cursor on the same screen row — retail carries it through an
/// explicit pair table (FFXiMain.dll retail-2026-09 RVA 0x36FF98), whose rows
/// line up with display order, so in this layout both retail pages share the
/// row index; entering or leaving page 3 keeps the row number clamped to its
/// length. ROOT_ENTRIES is page 1, then page 2 (retail display order, pinned by
/// `root_pages_match_retail_pair_table`), then page 3; the page a cursor is on
/// follows from its index (root_page_bounds), so no separate page state exists.
/// The Debug entry renders only when built with `--features debug-menu` —
/// opt-in and off by default, so retail parity holds as long as shipped builds
/// do not enable the flag; local test batches (build_cowland.bat) pass it on
/// RELEASE builds on purpose.
const ROOT_ENTRIES: &[&str] = &[
    // Page 1 (`menuwind`), in the DAT block's display order: Status,
    // Equipment, Magic List, Items, Synthesis, Abilities, Party, Trade,
    // Search, Linkshell, Region Info, Map. Captions are the items' short-
    // caption catalog strings (runtime override below; static copy pinned by
    // ffxi-dat real_selector_row_strings_in_display_order).
    "Status",
    "Equipment",
    ROOT_MAGIC_LIST,
    "Items",
    "Synthesis",
    "Abilities",
    "Party",
    "Trade",
    "Search",
    "Linkshell",
    "Region Info",
    ROOT_MAP,
    // Page 2 (`socialme`), display order: Missions, Quests, Key Items, View
    // House, Set Bazaar, MacroPalette, Config, Help Desk, Current Time,
    // Communication, Shut Down, Log Out.
    "Missions",
    "Quests",
    "Key Items",
    ROOT_VIEW_HOUSE,
    ROOT_SET_BAZAAR,
    ROOT_MACRO_PALETTE,
    "Config",
    "Help Desk",
    ROOT_CURRENT_TIME,
    ROOT_COMMUNICATION,
    ROOT_SHUT_DOWN,
    ROOT_LOG_OUT,
    // Page 3 (kuluu-only).
    "Graphics",
    "Interface",
    "Controls",
    #[cfg(feature = "debug-menu")]
    "Debug",
];

const ROOT_PAGE_1_LEN: usize = 12;
const ROOT_PAGE_2_LEN: usize = 12;

/// The Command menu cycles three pages: retail's two plus kuluu's.
pub const ROOT_PAGE_COUNT: usize = 3;

fn root_page_ranges() -> [(usize, usize); ROOT_PAGE_COUNT] {
    let p2 = ROOT_PAGE_1_LEN;
    let p3 = ROOT_PAGE_1_LEN + ROOT_PAGE_2_LEN;
    [(0, p2), (p2, p3), (p3, ROOT_ENTRIES.len())]
}

fn root_page_index(cursor: usize) -> usize {
    if cursor < ROOT_PAGE_1_LEN {
        0
    } else if cursor < ROOT_PAGE_1_LEN + ROOT_PAGE_2_LEN {
        1
    } else {
        2
    }
}

/// The half-open `[start, end)` range of `ROOT_ENTRIES` the page containing
/// `cursor` occupies. The Command menu shows one page at a time; "-" and Right
/// flip forward, Left steps back, Up/Down wrap within the current page.
pub fn root_page_bounds(cursor: usize) -> (usize, usize) {
    let cursor = cursor.min(ROOT_ENTRIES.len() - 1);
    root_page_ranges()[root_page_index(cursor)]
}

/// The cursor index one page forward ('-' / Right): page 1→2→3→1.
pub fn root_next_page_cursor(cursor: usize) -> usize {
    root_row_on_target_page(cursor, (root_page_index(cursor) + 1) % ROOT_PAGE_COUNT)
}

/// The cursor index one page back (Left), wrapping backwards through the pages.
pub fn root_prev_page_cursor(cursor: usize) -> usize {
    root_row_on_target_page(
        cursor,
        (root_page_index(cursor) + ROOT_PAGE_COUNT - 1) % ROOT_PAGE_COUNT,
    )
}

fn root_row_on_target_page(cursor: usize, target_page: usize) -> usize {
    let ranges = root_page_ranges();
    let row_in_page = cursor - ranges[root_page_index(cursor)].0;
    let (start, end) = ranges[target_page];
    start + row_in_page.min(end - start - 1)
}

/// The Root command list, for callers that enumerate the top-level entries
/// (e.g. the drill parity guard) without matching each label.
pub fn root_entries() -> &'static [&'static str] {
    ROOT_ENTRIES
}

// commenu's two list widgets in DAT order: windowps330 `Friend List` (61),
// then windowps928 `Emote List` (180) — vault main_menu.md §37.
const COMMUNICATION_ENTRIES: &[&str] = &[COMM_FRIEND_LIST, COMM_EMOTE_LIST];

const ITEMS_ENTRIES_STUB: &[&str] = &["(Items — Stage 3: pending inventory submenu)"];

pub const DROP_CONFIRM_YES: &str = "Yes";

pub const DROP_CONFIRM_NO: &str = "No";

/// The drop confirm's rows; the cursor opens on No (`DROP_CONFIRM_DEFAULT`),
/// like retail's Log Out prompt.
pub const DROP_CONFIRM_ENTRIES: &[&str] = &[DROP_CONFIRM_YES, DROP_CONFIRM_NO];

pub const DROP_CONFIRM_DEFAULT: usize = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicMenuRow {
    pub label: String,

    pub action: DynamicMenuAction,

    /// Retail's per-row colour law for item rows the legality check has an opinion on
    /// ([`equip_row_tint`] carries the law); `None` is ordinary text. Only builders that have
    /// tested equip legality set it — today that is [`equip_slot_rows`].
    pub tint: Option<MenuRowTint>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DynamicMenuAction {
    CastSpell {
        spell_id: u16,
    },

    JobAbility {
        ability_id: u16,
    },

    Weaponskill {
        skill_id: u16,
    },

    PetAbility {
        ability_id: u16,
    },

    /// Ranged attack (c2s action 0x10). Carries no id — only a target, picked
    /// through the same sub-target cursor as a weapon skill.
    RangedAttack,

    /// The two answers `prtyjoin` offers over a pending invite; sent as c2s
    /// GROUP_SOLICIT_RES (Res 1/0). Retail opens that window from the Party row's
    /// helper whenever its static state holds an unresolved solicit
    /// (FFXiMain.dll retail-2026-09 RVA 0x2016E0; vault main_menu.md section 24).
    AcceptPartyInvite,
    DeclinePartyInvite,

    /// One roster slot of the party window. Selecting a slot is inert until its
    /// member sub-action mapping is dug out of the prtyN select family
    /// (0x2059A0.., vault main_menu.md section 24 [I]).
    PartyMemberRow {
        char_id: u32,
    },

    /// One automaton slot row of the pupequip variant. `slot` is the LSB
    /// AutomatonSlot id (Head=0, Frame=1, Attachment1..12 = 2..13 — vendor/
    /// server/src/map/packets/c2s/0x102_extended_job.h). Selecting stays inert
    /// until the pupequip select family's equip flow is dug out and wired to
    /// c2s 0x102 pup_data_t (vault main_menu.md §39 [I]).
    AutomatonSlotRow {
        slot: u8,
    },

    /// One `configwi` hub row: select id i+1 name-opens page window
    /// RETAIL_CONFIG_PAGE_WINDOWS[i] (jump table .text 0x1968A4 of handler FFXiMain.dll
    /// retail-2026-09 RVA 0x196770; vault main_menu.md §42).
    OpenRetailConfigPage {
        page: u8,
    },
    /// One `conf5m` drill row: ids 1/2 name-open `conf5w2`/`conf5w1`, and further
    /// widgets answer the arm that opens `conf5win` (handler FFXiMain.dll retail-2026-09
    /// RVA 0x199240; vault main_menu.md §47). Index into RETAIL_CONFIG_SUB_WINDOWS.
    OpenRetailConfigSub {
        sub: u8,
    },
    /// A companion drill of a law'd Config page (`conftxtc`/`conf11m`/
    /// `conf11l`): kuluu records the mode dword retail writes into the companion
    /// object (handler FFXiMain.dll retail-2026-09 RVA 0x1984F0 arm body —
    /// `[textcol1]+0x14`, `[textcol2]+0x1C`, `[textcol3]+0x18`; RVA 0x19A0E0 —
    /// `[conf11l]+0x14`; RVA 0x19A160 — `conf11s`+0x34/0x38) and pushes the
    /// window retail leaves focused, i.e. the LAST entry of `opens` (manager
    /// open call order at .text 0x1985A8 then 0x1985BB; vault main_menu.md §59
    /// addendum 5).
    OpenRetailConfigCompanion {
        mode: i32,
        opens: &'static [u8],
    },
    /// A config row whose select arm is not lawed yet: kuluu keeps its record
    /// string id (§59) and answers the standard not-implemented line. The store
    /// ids live per-handler in the jump arms, NOT in this id (§48 correction via
    /// §58; bead .20.1 converts rows once their arm is read).
    ConfigOptionRow {
        option_id: u32,
    },
    /// A radio choice of a law'd config page: select writes
    /// (store_id, value) through the store wrapper FFXiMain.dll retail-2026-09
    /// RVA 0x193830 — rows of one group share a store id and each sets its own
    /// value (vault main_menu.md §58; bead kuluu-o9qt7.20.1). The live value in
    /// [`DynamicMenu::config_option_values`] marks the active row.
    ///
    /// The value is the full signed dword the wrapper accepts — `conf4`'s off
    /// arm pushes −1 (handler FFXiMain.dll retail-2026-09 RVA 0x19C557).
    ConfigOptionSet {
        store_id: u32,
        value: i32,
    },
    /// A binary row of the shared flag mask: select sets or clears `bit` in
    /// option [`CONFIG_OPTION_MISC_FLAGS`] through the read-modify-write helper
    /// FFXiMain.dll retail-2026-09 RVA 0x1938F0 (callee .text 0x193910 reads the
    /// option via wrapper 0x193850, ORs/ANDs one bit, writes back through 0x193830
    /// — there is no second store bank; vault main_menu.md §59 addendum 5). The
    /// row marks when the stored dword already holds that bit value.
    ConfigOptionBitSet {
        bit: u8,
        value: i32,
    },
    /// A filter-list page row (`fxfilter` today): select **flips** one bit of the option dword
    /// the window persists — class C's load method (.text FFXiMain.dll retail-2026-09 RVA
    /// 0x198370) reads [`CONFIG_OPTION_FILTER_CONSOLE_FLAGS`] into `[obj+0x30]` and its store
    /// method (.text 0x198390) writes it straight back; the row's descriptor byte picks the bit.
    /// Retail has no clear-pair here: the row toggles in place (vault main_menu.md §59
    /// addendum 14; bead kuluu-o9qt7.20.1.3).
    ConfigOptionToggleBit {
        store_id: u32,
        bit: u8,
    },
    /// A `cfilter` message-filter row: select dispatches through class B's shared select method
    /// (vtable slot +0x4c = .text FFXiMain.dll retail-2026-09 RVA 0x198080), which branches on the
    /// descriptor at [`CFILTER_ROWS`]`[`table_idx`]` — four transition classes over five persisted
    /// dwords (group level / console-bit toggle with master flip / pair-lane toggle / interlock
    /// machine; vault main_menu.md §59 addendum 15). The seam runs the exact transitions through
    /// [`cfilter_apply_select`]; silent, window stays open, nothing on the wire.
    ConfigCfilterSelect {
        table_idx: u8,
    },
    /// A slider row of a law'd config page (`conf2win` today): select pins the
    /// row into edit mode instead of writing — from then on Left/Right step the
    /// store value ±1 (immediate wrapper FFXiMain.dll retail-2026-09 RVA 0x193830
    /// writes, clamped so Right dies at 100 and Left at 0) while Up/Down are
    /// swallowed, and Enter/Esc both save the current value and leave edit mode
    /// (handler select arms + key tables .text 0x197874/0x19788C/0x1978A4 of
    /// handler RVA 0x197470; bead kuluu-o9qt7.20.1 part B).
    ConfigSliderEdit {
        store_id: u32,
    },

    /// A `WindowLocal` pair row's write: the choice lives only on the window
    /// object (retail `[obj+0x38]` + dirty byte `[obj+0x3c]`, arms .text FFXiMain.dll
    /// retail-2026-09 RVA 0x19aa85/0x19aa92) — never the option store, never the
    /// wire. `obj_off` is that object field offset and `window_key` comes from
    /// [`config_window_state_key`] (vault main_menu.md §59 addendum 9; bead
    /// kuluu-o9qt7.20.1.2).
    ConfigWindowChoiceSet {
        window_key: u8,
        obj_off: u16,
        value: i32,
    },
    /// The dialog row of `conf6win` (select id 10, arm .text FFXiMain.dll
    /// retail-2026-09 RVA 0x19B309): nothing is written at select — the handler only launches
    /// a yes/no prompt through the generic dialog service object ([0x10664520],
    /// FFXiMain.dll retail-2026-09 RVA 0x210E30) and returns. The callback .text 0x196F30
    /// acts on the answer: non-zero (Yes) tail-calls vtable slot +0x48 on both
    /// key-assignment panels, writing their factory-default templates through store wrapper
    /// RVA 0x193830 — see [`key_assign_reset_defaults`]; No writes nothing (vault main_menu.md §59 addenda 7–8; bead kuluu-o9qt7.20.1.1).
    OpenKeyAssignResetPrompt,

    /// A `roomlist` style row (bead kuluu-o9qt7.13.5). Retail's row callback
    /// (.text FFXiMain.dll retail-2026-09 0x1D6B20) raises the generic yes/no dialog —
    /// same service object [0x10664520] .text 0x210E30 as above, prompt GetText table 12
    /// id 451 "Make changes?" — and the YES callback is the MYROOM_IS sender wrapper
    /// .text 0x1D6AB0 → .text 0x1D6AD0 (Kind=Remodel, Param1=[obj+0x50], Param2=style).
    /// kuluu pushes [`MenuKind::RoomListConfirm`] carrying this row's `style`.
    MogHouseRemodelSelect {
        style: u16,
    },

    /// One filled macro slot row of the mcrselec select window (vault
    /// main_menu.md §41). Selecting stays inert until the macro-use seam is
    /// lawed end to end — the DAT grids prove capacity (2 blocks x 20 keytops),
    /// the store proves content, but which widget press runs a macro is an
    /// open-map item on bead .19.1.
    MacroSlotRow {
        block: u8,
        index: u8,
    },

    /// Answer an incoming trade request YES — c2s TRADE_RES Kind Start. Retail
    /// answers through the gift window's rows once s2c 0x021 lands (FFXiMain.dll
    /// retail-2026-09 RVA 0x1515E0; vault main_menu.md §25).
    TradeAcceptInvite,
    /// Answer NO — c2s TRADE_RES Kind Cancell. The server confirms nothing back
    /// to the decliner, so kuluu clears its pending window locally too (the same
    /// law accepted for party answers in §24).
    TradeDeclineInvite,
    /// The trade window's button row id 9 = Confirm: sends c2s TRADE_RES Kind
    /// Make while unconfirmed; retail sets its state to confirmed at send time
    /// (vault main_menu.md §25 select law).
    TradeConfirmRow,
    /// Button row id 10 = Exit: retail closes BOTH window names when nothing is
    /// pending (predicate 0x151340); kuluu sends TRADE_RES Kind Cancell and
    /// clears its own state.
    TradeCancelRow,
    /// One of our own offer slots — select ids 1..8 through the byte table at
    /// .rdata 0x371D60: an empty row opens the Add Item picker, a filled row
    /// releases via the qty-0 TRADE_LIST path (player_trade.cpp setSlot).
    TradeOwnSlot {
        trade_index: u8,
    },
    /// Button row id 11 = Add Gil. Retail posts an async keypad dialog
    /// (`call 0x140D90`, callback 0x1515C0) before staging slot 0 with the
    /// 0xFFFF sentinel; the keypad itself is [I-pending], so this row stays
    /// display-only for now (vault main_menu.md §25).
    TradeGilRow,
    /// Partner-side offer slot rows (`gift` window): display only — retail gives
    /// them no select arm at all.
    TradePartnerOfferRow {
        trade_index: u8,
    },
    /// Pick from the Add Item bag picker: c2s TRADE_LIST stages `bag_slot`'s
    /// contents into our offer at `trade_index` (player_trade.cpp setSlot).
    TradeStageFromBag {
        trade_index: u8,
        item_no: u16,
        qty: u32,
        bag_slot: u8,
    },

    /// Get off the mount (c2s action 0x12).
    /// GP_CLI_COMMAND_ACTION_ACTIONID,
    /// vendor/server/src/map/packets/c2s/0x01a_action.h.
    Dismount,

    /// Dig with the chocobo for a buried item (c2s action 0x11). Self-targeted
    /// like Dismount. GP_CLI_COMMAND_ACTION_ACTIONID,
    /// vendor/server/src/map/packets/c2s/0x01a_action.h.
    ChocoboDig,

    UseItem {
        container: u8,
        index: u8,
        item_no: u16,
    },

    /// c2s 0x029 ITEM_MOVE (full stack, server-picked destination slot).
    MoveItem {
        quantity: u32,
        from_container: u8,
        from_slot: u8,
        to_container: u8,
        item_no: u16,
    },

    /// Open the per-item context submenu for a slot.
    OpenItemAction {
        container: u8,
        index: u8,
        item_no: u16,
    },

    /// Item submenu "Drop": opens the Yes/No confirm for the whole stack
    /// (c2s 0x028 ITEM_DUMP on Yes;
    /// vendor/server/src/map/packets/c2s/0x028_item_dump.cpp).
    DropItem {
        container: u8,
        index: u8,
        item_no: u16,
        quantity: u32,
    },

    EquipItem {
        container: u8,
        container_index: u8,
        equip_slot: u8,
        item_no: u16,
    },

    /// Terminal Key Items row; selecting it echoes the name to chat, and
    /// cursoring over it marks the key item seen on menu close (c2s 0x064).
    KeyItem {
        id: u16,
    },

    /// Emote List row: dispatches the same AgentCommand::Emote as the slash
    /// command, at the current target.
    Emote {
        emote_id: u8,
    },

    /// The `searchma` run row — select id 1 hands to the query object's method
    /// 0x20EE70 (FFXiMain.dll retail-2026-09 law at handler RVA 0x20D340; vault
    /// main_menu.md section 26). Fires AgentCommand::PlayerSearch; merged
    /// CSearchListPacket frames land the results in SceneSnapshot.search.
    SearchRun,
    /// One hit row of the results list (retail's `scresult` model window). Selecting is inert
    /// until a search-to-target mapping exists - retail's own hit-row semantics stay [I] (vault
    /// main_menu.md section 26).
    SearchHitRow {
        index: usize,
    },

    /// One shell-slot row of the Link Shell window (retail `menu    link5`, opened by the page-1
    /// arm FFXiMain.dll retail-2026-09 RVA 0x1405EE through helper
    /// FFXiMain.dll retail-2026-09 RVA 0x1E4470; vault main_menu.md section 27).
    /// `linkshell_slot` is LSB's `LinkshellId` (1/2). Selecting stays inert while that window's own
    /// select table is [I-pending] (section 27.6): the row makes the s2c 0x0E0 state legible, it
    /// does not guess at equip/unequip UI.
    LinkShellSlot {
        linkshell_slot: u8,
    },

    /// One mode row of the Region Info window: items 1..4 install display modes
    /// 0..3 on that window itself (FFXiMain.dll retail-2026-09 RVA 0x140818;
    /// vault main_menu.md §29).
    RegionInfoMode {
        mode: u8,
    },

    /// One row of the `missionm` list opening its `miss00` detail (the handler's
    /// select tail name-opens miss00; FFXiMain.dll retail-2026-09 RVA 0x22DB40;
    /// vault main_menu.md §30).
    MissionOpenLog {
        port: u16,
    },

    /// A legible-only mission-log line (`miss00`'s paging semantics are
    /// [I-pending], vault main_menu.md section 30).
    MissionLogRow {
        port: u16,
    },

    /// One stall-picked row of the Bazaar window (the `bazaar` browser is
    /// data-driven chrome — FFXiMain.dll retail-2026-09 RVA 0x1F6C70; vault
    /// main_menu.md §33): select browses that seller's stall via c2s 0x105. The
    /// empty-list hint row carries `char_id: u32::MAX` and never resolves.
    BazaarStall {
        char_id: u32,
    },

    /// One row of the `quest00` list opening its `quest01` detail (the handler's
    /// select tail name-opens quest01 with the row value; FFXiMain.dll retail-2026-09
    /// RVA 0x2300EE; vault main_menu.md section 31).
    QuestOpenDetail {
        log: u8,
        id: u16,
    },

    /// A legible-only quest detail line (`quest01`'s paging semantics stay
    /// [I-pending], vault main_menu.md section 31).
    QuestDetailRow {
        log: u8,
        id: u16,
    },

    /// One craftable recipe row of the kuluu craft list (the `cmbmenu` row-2 branch opens this
    /// window pair; FFXiMain.dll retail-2026-09 RVA 0x1E29C1, vault main_menu.md §22/§28). The plan is resolved
    /// against the bag at render time: selecting sends exactly this c2s 0x096 ask, ingredients in
    /// the recipe row's own positional order.
    SynthesizeRecipe {
        crystal_id: u16,
        crystal_slot: u8,
        /// Entries in recipe column order; `None` fills the empty trailing columns
        /// (the ask's Items byte is the number of Somes).
        ingredients: [Option<ffxi_proto::synth::SynthIngredient>; 8],
    },

    /// One job row of the home-view pick list (`jobchang` → `jobcsel` family,
    /// vault main_menu.md §53): selecting sends c2s 0x100 MYROOM_JOB carrying
    /// this id on its own side and zero ("no change") on the other — exactly
    /// what LSB's validator walks before applying it (vendor/server/src/map/
    /// packets/c2s/0x100_myroom_job.cpp).
    HomeJobRow {
        job_id: u8,
        as_main: bool,
    },

    /// Edit one command line of a macro keytop (`None` = compose a new line);
    /// selects seed the level's `macro_line_edit` buffer — retail's mcrledit
    /// edit-and-blip-commit law (FFXiMain.dll retail-2026-09 RVA 0x1A6180;
    /// vault main_menu.md §34/§41, bead kuluu-o9qt7.19.3).
    MacroEditLine {
        line_index: Option<usize>,
    },

    /// Wipe the keytop wholesale (retail's blank-keytop state; vault
    /// main_menu.md §41 capacity law — a cleared slot leaves no file entry).
    MacroClearSlot,

    /// Run the keytop now: its command lines queue as pending chat-pipeline
    /// steps, one submitted per frame exactly as if typed (retail feeds a used
    /// macro's text into the chat pipeline — vault main_menu.md §41/§56; bead
    /// kuluu-o9qt7.19.4).
    MacroUse {
        block: u8,
        index: u8,
    },

    /// A read-only info line in a Status detail window (bead kuluu-0oy7b). The
    /// retail detail companions of statcom2 hold no commands at all — the
    /// selector's dispatch is OPEN-only and these windows never act on a press
    /// (FFXiMain.dll retail-2026-09 select handler RVA 0x148D00 over detail
    /// table RVA 0x3716B8) — so selecting such a row keeps its window open.
    DisplayInfo,

    /// One of kuluu's Controls-page preset commands: apply it to the live store
    /// and let the page re-list on top of it (bead kuluu-o9qt7.26.1). Kuluu-only;
    /// retail assigns keys per row instead (`ControlsPreset` has no DLL handler).
    ControlsPreset(crate::keybinds::Preset),

    /// …and the reset command, answering through the same seam as `/keybinds reset`.
    ControlsReset,

    /// The `abisortw` companion window's select item (mode 1 emote / 2 mount — its handler
    /// answers only this one, gated on the item arg, and sets the paired list window's swap
    /// state word: `[emote+0x6C]=1`, FFXiMain.dll retail-2026-09 `.text 0x223CF0` id-5 arm →
    /// `.text 0x223DC3`; bead kuluu-o9qt7.34). Client-local: never a wire action.
    AbisortwSelect {
        mode: u8,
    },

    /// A `ut_menu` row: DAT item 1 opens the Unity chat panel, 2 the ranking chooser, 3 the
    /// leader list (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
    UnityMenuItem {
        item: u8,
    },
    /// A Unity chat panel row choosing the c2s 0x118 mode it stands for.
    UnityChatMode {
        mode: u8,
    },
    /// A ranking chooser row opening the ranking list of `result_set`.
    UnityRankingSet {
        result_set: u8,
    },
    /// A ranking list row; its select does nothing.
    UnityRankingRow {
        concord: u8,
    },
    /// A Unity leader list row, `concord` 0 standing for the player's own.
    UnityLeaderRow {
        concord: u8,
    },
    /// An `fp_cat` category row.
    AlterEgoCategory {
        category: u16,
    },
    /// An `fp_cat2` upgrade row with the level cap its DAT record gives.
    AlterEgoUpgrade {
        upgrade: u16,
        cap: u8,
    },
    /// The `merityn` answer to an alter ego upgrade prompt.
    AlterEgoConfirmAnswer {
        yes: bool,
    },

    /// A merit or job point window row
    /// (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).
    Merit(crate::hud::merit_windows::MeritAction),

    /// An arrange editor window row
    /// (.agents/skills/retail-observe/references/2026-10-15-mog-house-arrange.md).
    Arrange(crate::myroom_arrange::ArrangeAction),
}

impl DynamicMenuAction {
    /// The item id a row acts on, for rows that carry one (item list, usable
    /// list, per-item context, equip). Lets the item window resolve icon +
    /// detail without matching each variant at every call site.
    pub fn item_no(&self) -> Option<u16> {
        match *self {
            DynamicMenuAction::UseItem { item_no, .. }
            | DynamicMenuAction::MoveItem { item_no, .. }
            | DynamicMenuAction::OpenItemAction { item_no, .. }
            | DynamicMenuAction::DropItem { item_no, .. }
            | DynamicMenuAction::EquipItem { item_no, .. } => Some(item_no),
            _ => None,
        }
    }

    /// The source `(container, index)` of an item-bearing action, so per-slot
    /// state (charges, recast) resolves the exact instance rather than the
    /// first item of the same id.
    pub fn item_slot(&self) -> Option<(u8, u8)> {
        match *self {
            DynamicMenuAction::UseItem {
                container, index, ..
            }
            | DynamicMenuAction::OpenItemAction {
                container, index, ..
            }
            | DynamicMenuAction::DropItem {
                container, index, ..
            } => Some((container, index)),
            DynamicMenuAction::EquipItem {
                container,
                container_index,
                ..
            } => Some((container, container_index)),
            _ => None,
        }
    }
}

/// One item row's label: the name, suffixed " xN" only for a real stack. Shared
/// by the inventory list and the usable-item list so partial stacks read the
/// same wherever an item appears.
pub fn item_qty_label(name: &str, quantity: u32) -> String {
    if quantity > 1 {
        format!("{name} x{quantity}")
    } else {
        name.to_string()
    }
}

/// Unseen ("new") key-item indicator suffix. Retail shows a yellow-bubble
/// glyph whose exact appearance is unverified (bead kuluu-h7x
/// retail_unknowns); a text marker stands in until a retail capture.
pub const KEY_ITEM_UNSEEN_SUFFIX: &str = " (new)";

pub fn key_item_row_label(id: u16, seen: bool) -> String {
    let name = ffxi_vocab::key_item_names::lookup(id)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Key Item #{id}"));
    if seen {
        name
    } else {
        format!("{name}{KEY_ITEM_UNSEEN_SUFFIX}")
    }
}

/// The emote window's two-step swap state — retail's `[emote+0x6C]` word: 0 Off, 1 await-first
/// pick, 2 await-second pick (the picked id rides where retail keeps its pending cursor
/// `[emote+0x70]`). Set by the `abisortw` select arm (.text FFXiMain.dll retail-2026-09
/// `0x223DC3`), cleared by the fill law (`0x225960` resets it on every refill) and by the
/// cancel arm (`0x225E34`; bead kuluu-o9qt7.34).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmoteSortState {
    #[default]
    Off,
    AwaitFirst,
    AwaitSecond(u8),
}

/// A UI sound the input layer asks for; the audio side plays it from the system SE table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiSound {
    Confirm,
    Buzzer,
    Cursor,
    /// Retail UI sound 0001, which the merit record names as the page toggle.
    PageToggle,
    /// The cancel sound of an Esc a window's handler takes without closing.
    Cancel,
}

#[derive(bevy::prelude::Resource, Debug, Clone, Default)]
pub struct DynamicMenu {
    /// UI sounds queued this frame, drained by the audio side.
    pub ui_sounds: Vec<UiSound>,
    /// Windows a window's own logic closed this update (a select's answer, an upkeep). Their sound
    /// is whatever that logic queued, so the menu-depth observer adds no cancel sound for them.
    pub closed_by_logic: bool,
    /// State the Unity and alter ego point windows keep while they are open.
    pub unity_fp: crate::hud::unity_fp::UnityFpWindows,
    /// State the merit and job point windows keep.
    pub merit: crate::hud::merit_windows::MeritWindows,
    /// The Mog House arrange editor and its windows.
    pub arrange: crate::myroom_arrange::ArrangeEditor,
    pub rows: Vec<DynamicMenuRow>,
    /// The top window [`Self::rows`] were built for.
    pub rows_kind: Option<MenuKind>,
    /// The rows of every dynamic window open under the top one, by kind, so a window that
    /// stays on screen under a newer one keeps listing its own rows.
    pub stacked_rows: Vec<(MenuKind, Vec<DynamicMenuRow>)>,
    /// The remodel confirm question, copied from the DAT table-12 catalog by
    /// [`refresh_dynamic_menu_rows`] (GetText id [`ROOMLIST_CONFIRM_PROMPT_ID`]; bead
    /// kuluu-o9qt7.13.5.2). `None` while that catalog has not loaded or holds no such string.
    pub roomlist_prompt: Option<String>,
    /// The myroom door row's caption for the current door state, folded in by
    /// [`refresh_dynamic_menu_rows`] from [`ConfigDatRows::home_door_captions`]. `None` until that
    /// catalog loads, in which case the static [`HOME_ROW_OPEN_MOG_HOUSE`] copy shows instead.
    pub home_door_caption: Option<String>,
    /// Copies of the `abisortw` mode captions / emote swap prompts held by
    /// [`ConfigDatRows`] (same law as [`Self::home_door_caption`]; bead kuluu-o9qt7.34).
    pub abisortw_captions: [Option<String>; 3],
    pub emote_sort_prompts: [Option<String>; 2],
    /// The emote window's two-step swap state — retail's `[emote+0x6C]` word (Off=0,
    /// "select first"=1, "select second"=2 with the picked id riding where retail keeps its
    /// pending cursor `[+0x70]`; FFXiMain.dll retail-2026-09 `.text 0x223DC3` set arm,
    /// `.text 0x225E34` cancel arm; bead kuluu-o9qt7.34).
    pub emote_sort_state: EmoteSortState,
    /// Page-1/2 Command-menu selector captions read live from the DAT window
    /// records and short-caption catalog (`menuwind` then `socialme`, display
    /// order). While non-empty, [`entry_label`] prefers it over the static
    /// ROOT_ENTRIES copy for indices within its range; the copy is pinned to
    /// the catalog by install-gated tests in both crates (bead kuluu-1z9qs).
    pub selector_captions: Vec<String>,
    /// Chat lines the client emits locally ride ids into GetText **table 9**
    /// (emitter .text FFXiMain.dll retail-2026-09 `.text 0x129DE0`, vault main_menu.md
    /// §59 addendum 35); this is that catalog, loaded once from DAT on the first
    /// [`Self::client_message`] and `None` while no install resolves it — text stays
    /// DAT-sourced, never transcribed (bead kuluu-o9qt7.41).
    pub client_messages: Option<ffxi_dat::dmsg::DmsgTable>,
    /// The client-side config option store, keyed by retail store id (§58;
    /// bead kuluu-o9qt7.20.1). Absent keys have no known current value — radio
    /// rows render unmarked rather than guess a default. Config writes stay on
    /// this machine: LSB's c2s 0x0DC accepts only the named SAVE_CONF bits
    /// (vendor/server/src/map/packets/c2s/0x0dc_config.cpp), not store options.
    pub config_option_values: std::collections::BTreeMap<u32, i32>,
    /// A slider row pinned in edit mode: (store id, live value). While set on
    /// the top window, Left/Right step the value ±1 with retail's clamps and
    /// write through immediately. Up/Down are NOT part of that control: they drop the pin
    /// and move rows like any other row (bead kuluu-8l5h4 — every direction reads its own
    /// neighbour byte in the input switch FFXiMain.dll retail-2026-09 RVA 0x118260); Enter/Esc
    /// save and unpin (conf2win key tables .text 0x197874/
    /// 0x19788C/0x1978A4 of handler FFXiMain.dll retail-2026-09 RVA 0x197470;
    /// bead kuluu-o9qt7.20.1 part B). Only config pages whose arms are lawed
    /// support pinning; any other top window drops the pin.
    pub option_edit: Option<(u32, u8)>,
    /// The mode dword retail writes into a companion window object when its row
    /// drills it open (`[textcol1]+0x14`, `[textcol2]+0x1C`, `[conf11l]+0x14` …;
    /// handlers FFXiMain.dll retail-2026-09 RVA 0x1984F0/0x19A0E0/0x19A160).
    /// Keyed by the companion's index into RETAIL_CONFIG_SUB_WINDOWS, so a
    /// companion's own rows can read which option opened it (vault main_menu.md
    /// §59 addendum 5; bead kuluu-o9qt7.20.1). Absent = never drilled.
    pub config_companion_modes: std::collections::BTreeMap<u8, i32>,
    /// The `WindowLocal` pair rows' state (`conf3win` rows 3/4): (window state key,
    /// retail object field offset) → choice, living only as long as this client
    /// session — mirroring the window objects the menu manager boots once ([obj+0x38]
    /// at −1 in the ctor .text FFXiMain.dll retail-2026-09 RVA 0x19a8b8; vault
    /// main_menu.md §59 addendum 9; bead kuluu-o9qt7.20.1.2).
    pub config_window_choices: std::collections::BTreeMap<(u8, u16), i32>,
    /// The announce throttle stamp of the `conf12wi` store-0xBF pair — retail keeps
    /// this clock global at [0x106627e4], written on every sent announce (.text
    /// FFXiMain.dll retail-2026-09 RVA 0x1A806E) and read back by the guard
    /// subtraction at RVA 0x1A7D05 (vault main_menu.md §59 addendum 16; bead
    /// kuluu-o9qt7.20.1.4). `None` = never announced, like retail's zeroed BSS.
    pub conf_bf_stamp: Option<std::time::Instant>,
    /// Where each remembered window's cursor last closed this session; every menu opens with it.
    pub window_cursors: CursorMemory,
}

impl DynamicMenu {
    /// The chat line for a client-local message id — retail's emitter `.text
    /// FFXiMain.dll retail-2026-09 `.text 0x129DE0` resolves every id through GetText **table
    /// 9**; this loads that catalog once (id [`ffxi_dat::menu_windows::GETTEXT_TABLE_9_CATALOG_FILE_ID`],
    /// vault main_menu.md §59 addendum 35) and reads the text from DAT. Without an install or a
    /// missing entry the line never drops silently: it names the id numerically instead.
    pub fn client_message(
        &mut self,
        dat: Option<&ffxi_dat::archive::DatRoot>,
        message_id: u16,
    ) -> String {
        if self.client_messages.is_none() {
            self.client_messages = (|| {
                let root = dat?;
                let loc = root
                    .resolve(ffxi_dat::menu_windows::GETTEXT_TABLE_9_CATALOG_FILE_ID)
                    .ok()?;
                let bytes = std::fs::read(loc.path_under(root)).ok()?;
                ffxi_dat::dmsg::DmsgTable::parse(&bytes).ok()
            })();
        }
        match self
            .client_messages
            .as_ref()
            .and_then(|t| t.text(usize::from(message_id)))
        {
            Some(text) => text.to_owned(),
            None => format!("[system message {message_id:#05X}]"),
        }
    }

    /// Read one config-option store key the way retail's slash query arms do. Plain getters
    /// (`.text FFXiMain.dll retail-2026-09 0x193890` for `/furcc`, `.text 0x193850` for
    /// `/layoutctrl`) only test the value; `/statusparty` instead bool-normalizes it
    /// (`neg/sbb/inc`) and **rewrites** it through setter `.text 0x193830` at merge .text
    /// **0x7EB1C**, so `write_back_normalized` stores 0/1 back for that family (bead
    /// kuluu-o9qt7.42; vault main_menu.md §59 addendum 35). Absent keys read as zero, like
    /// retail's zeroed store.
    pub fn config_query(&mut self, store_id: u32, write_back_normalized: bool) -> i32 {
        let raw = *self.config_option_values.get(&store_id).unwrap_or(&0);
        if write_back_normalized {
            let normalized = i32::from(raw != 0);
            self.config_option_values.insert(store_id, normalized);
            normalized
        } else {
            raw
        }
    }
}

pub const DYNAMIC_VISIBLE_ROWS: usize = 22;

const EQUIPMENT_ENTRIES: &[&str] = &[
    "Main", "Sub", "Ranged", "Ammo", "Head", "Body", "Hands", "Legs", "Feet", "Neck", "Waist",
    "L.Ear", "R.Ear", "L.Ring", "R.Ring", "Back",
];

/// Rows of [`STATUS_LABELS`] (display order) for Status ids 11, 7, 8 and 13.
const STATUS_ROW_UNITY: usize = 8;
const STATUS_ROW_MERIT_POINTS: usize = 10;
const STATUS_ROW_JOB_POINTS: usize = 11;
const STATUS_ROW_ALTER_EGO_POINTS: usize = 12;

const STATUS_LABELS: &[&str] = &[
    "Profile",
    "Job Levels",
    "Master Levels",
    "Combat Skill",
    "Magic Skill",
    "Craft Skill",
    "Currencies",
    "Currencies 2",
    "Unity",
    "Play Time",
    "Merit Points",
    "Job Points",
    "Alter Ego Points",
];

/// kuluu's Controls page is dynamic (bead kuluu-o9qt7.26.1): its body lists the
/// **live** key map, one row per bound action, and these four commands trail it.
/// The labels stay fixed so help-bar copy and tests can name them; retail has no
/// preset rows at all (its own 29-row key-assignment law lives in
/// [`crate::hud::controls_law`]), so the whole page is kuluu-only — page 3.
pub const CONTROLS_COMMAND_LABELS: [&str; 4] =
    ["Standard", "Compact 1", "Compact 2", "Reset to defaults"];

/// Build the Controls page from the live [`Bindings`](crate::keybinds::Bindings)
/// store: one `name: KEY` row per bound action in store order (retail's law of
/// drawing each row's key straight from its binding table — `.text 0x19CF60`
/// over FFXiMain.dll retail-2026-09 `.data 0x36D6B0`, vault main_menu.md §59
/// addendum 25 — applied to kuluu's store), then the preset/reset commands.
/// Because [`refresh_dynamic_menu_rows`] re-runs this every frame, a remap made
/// elsewhere (`/keybinds …`) shows on the open page without any restart.
pub fn controls_rows(bindings: &crate::keybinds::Bindings) -> Vec<DynamicMenuRow> {
    let mut rows: Vec<DynamicMenuRow> = bindings
        .iter()
        .map(|(action, _)| {
            let key = bindings.key_label(action).unwrap_or("...");
            DynamicMenuRow {
                tint: None,
                label: format!("{}: {}", action.display_name(), key),
                action: DynamicMenuAction::DisplayInfo,
            }
        })
        .collect();
    rows.extend(
        CONTROLS_COMMAND_LABELS
            .iter()
            .zip([
                DynamicMenuAction::ControlsPreset(crate::keybinds::Preset::Standard),
                DynamicMenuAction::ControlsPreset(crate::keybinds::Preset::Compact1),
                DynamicMenuAction::ControlsPreset(crate::keybinds::Preset::Compact2),
                DynamicMenuAction::ControlsReset,
            ])
            .map(|(label, action)| DynamicMenuRow {
                tint: None,
                label: (*label).to_string(),
                action,
            }),
    );
    rows
}

pub const DEBUG_PERF: &str = "Perf";
pub const DEBUG_TARGET_CYCLE: &str = "Target Cycle";
pub const DEBUG_MESH: &str = "Mesh Debug";
pub const DEBUG_NET_STATUS: &str = "Net Status";
pub const DEBUG_NOCLIP: &str = "NoClip";
/// Enhanced build only: forces the retail weapon draw/sheathe movement hold
/// back on (the gated build lifts it by default). Default off.
#[cfg(feature = "enhanced-engage-move-lock-off")]
pub const DEBUG_ENGAGE_ANIM_LOCK: &str = "Engage_anim_lock";
/// Debug auto-enter row (enternity-style): [on] = event-dialog message frames
/// advance themselves after their read time instead of parking on Enter;
/// choice frames, item lines, text-entry frames, server custom menus, and the
/// enternity blacklist (Paintbrush of Souls, Geomantic Reservoir) stay manual.
/// Off by default.
pub const DEBUG_AUTO_ENTER_CS: &str = "Auto-Enter CS";
/// Debug weather gate row: [on] = the active-weather modifier (ambient tint,
/// sun mul, lightning) and precipitation particles are applied; toggling it
/// off suppresses them for isolating scene-graphic errors. Default on.
pub const DEBUG_WEATHER: &str = "Weather";
/// Debug fog gate row: [on] = every fog layer (DAT distance fog, volumetric
/// ground haze) renders; toggling it off strips all of them. Default on.
pub const DEBUG_FOG: &str = "Fog";
/// Debug clock-gate row: [on] = the Vana clock is frozen at 18:00 (lamp
/// night-scene testing); off thaws it back to live time.
pub const DEBUG_FORCE_18: &str = "Force_18:00";
/// Debug Entity List overlay row: [on] = the scrollable live-entity dump
/// (id/name/kind/pos/status/hp/invis flags from the EntityTable) is shown.
/// Mouse wheel scrolls. Default off.
pub const DEBUG_ENTITY_LIST: &str = "Entity List";
pub const DEBUG_SOUND: &str = "Sound";
pub const DEBUG_VOLUME: &str = "Volume";
pub const DEBUG_PRINT_POS: &str = "Print POS";
pub const DEBUG_STAIR_DRAW: &str = "Draw Stair Climber";
pub const DEBUG_STAIR_STATUS: &str = "Show Stair Status";
pub const DEBUG_GRAPHICS_DEBUG: &str = "Graphics Debug";
pub const DEBUG_POSITION_LOG: &str = "Panel Pos Log";
pub const DEBUG_NAMEPLATES: &str = "Nameplate Debug";
pub const DEBUG_UI_SETTINGS: &str = "UI Settings";

/// Retail+ section (dev-only Debug menu): a separator row, the section label,
/// then the DLSS On/Off toggle plus — in enhanced builds only — Mob HP Under /
/// Job Display. All persist in GraphicsSettings, so each choice survives
/// restarts.
pub const DEBUG_RETAIL_SEPARATOR: &str = "────────────────────────────";
pub const DEBUG_RETAIL_LABEL: &str = "Retail+";
/// Makes DLSS selectable in the Graphics menu. Does NOT turn DLSS on — with
/// it off (the default) every DLSS row reads N/A even on capable machines.
pub const RETAIL_DLSS_MENU: &str = "DLSS On/Off";
/// Gates the mob/pet HP readout on nameplates (billboard bar + pct suffix;
/// default off). The row exists only with `enhanced-mob-hp-under`.
pub const RETAIL_MOB_HP_UNDER: &str = "Mob HP Under";
/// Gates the party-frame Job column (retail shows none; default off). The row
/// exists only with `enhanced-job-display`.
pub const RETAIL_JOB_DISPLAY: &str = "Job Display";
/// Makes the Status panel's rows actionable. Retail opens persona, which answers no menu command
/// events (FFXiMain.dll retail-2026-09 RVA 0x12AC60 is a ret-8 stub) — display-only. Default off.
pub const RETAIL_STATUS_ROWS: &str = "Status Rows";
/// Makes re-selecting the already-equipped item in an equip slot unequip it. Retail's equipment
/// handler equips on every select case - no toggle-off branch (FFXiMain.dll retail-2026-09 RVA
/// 0x1322B0, jump table 0x132608; vault main_menu.md §20). Default off = retail re-equip.
pub const RETAIL_RESELECT_UNEQUIP: &str = "Reselect Unequip";
/// Debug dev row: [on] = the player character takes knockback displacement
/// (the retail behavior, the default); off drops the travel only — flinch,
/// facing and the movement lock still play. The row exists only with
/// `enhanced-ignore-knockback-self`.
#[cfg(feature = "enhanced-ignore-knockback-self")]
pub const RETAIL_KNOCKBACK: &str = "Knockback";
/// Retail+ camera leash row: the chase camera's focus dead zone in yalms
/// (camera_collision.rs resolve_camera). Left/Right step it by
/// CAMERA_LEASH_STEP_YALMS; 0 turns the leash off. Persists in
/// GraphicsSettings.
pub const RETAIL_CAMERA_LEASH: &str = "Camera_leash";
/// Retail+ magic row: open the kuluu flat spell list instead of retail's
/// mgcmenu category picker. Kuluu's single-step list has no retail counterpart -
/// the real Magic flow is two windows (FFXiMain.dll retail-2026-09 RVA 0x13F750
/// opens the `magic` spell list through the category rows; vault main_menu.md §21)
/// - so the flat list survives as an off-by-default option.
pub const RETAIL_MAGIC_FLAT_LIST: &str = "Flat Magic List";

/// The client-side school numbers mgcmenu filters on: the internal ids its
/// handler pushes into the `magic` window (FFXiMain.dll retail-2026-09 RVA
/// 0x13F750 jump table 0x13F7CC maps item ids {4..B} to esi ∈ {1,2,5,4,3,6,7,8};
/// vault main_menu.md §21), which are the spell DAT's magicType numbers
/// (ffxi-dat spell_info::MagicType).
const MAGIC_CAT_WHITE: u8 = 1;
const MAGIC_CAT_BLACK: u8 = 2;
const MAGIC_CAT_SUMMONING: u8 = 3;
const MAGIC_CAT_NINJUTSU: u8 = 4;
const MAGIC_CAT_SONGS: u8 = 5;
const MAGIC_CAT_BLUE: u8 = 6;
const MAGIC_CAT_GEOMANCY: u8 = 7;
const MAGIC_CAT_TRUST: u8 = 8;

/// mgcmenu's rows as (client school id, caption) in retail display order - the
/// jump-table shuffle above says row 3 is Songs (5) and row 5 Summoning (3),
/// not numeric order (§21). Captions are d_msg catalog strings: ROM/165/77.DAT
/// indices 246-250 and 252, Blue Magic from ROM/165/76.DAT index 141 (the §10
/// caption catalogs). No short Trust caption exists in any ROM/165 d_msg table,
/// so "Trust Magic" follows the LSB wording [I].
pub const MAGIC_CATEGORY_ROWS: &[(u8, &str)] = &[
    (MAGIC_CAT_WHITE, "White Magic"),
    (MAGIC_CAT_BLACK, "Black Magic"),
    (MAGIC_CAT_SONGS, "Songs"),
    (MAGIC_CAT_NINJUTSU, "Ninjutsu"),
    (MAGIC_CAT_SUMMONING, "Summoning"),
    (MAGIC_CAT_BLUE, "Blue Magic"),
    (MAGIC_CAT_GEOMANCY, "Geomancy"),
    (MAGIC_CAT_TRUST, "Trust Magic"),
];

/// The same labels for the generic static-list path; pairing with
/// [`MAGIC_CATEGORY_ROWS`] is pinned by a test.
const MAGIC_CATEGORY_LABELS: &[&str] = &[
    "White Magic",
    "Black Magic",
    "Songs",
    "Ninjutsu",
    "Summoning",
    "Blue Magic",
    "Geomancy",
    "Trust Magic",
];

/// The client school of the mgcmenu row at display index `row`.
pub fn magic_category_at(row: usize) -> Option<u8> {
    MAGIC_CATEGORY_ROWS.get(row).map(|(cat, _)| *cat)
}

/// The client school a picked mgcmenu caption drills into.
pub fn magic_category_for_label(label: &str) -> Option<u8> {
    MAGIC_CATEGORY_ROWS
        .iter()
        .find(|(_, cap)| *cap == label)
        .map(|(cat, _)| *cat)
}

/// Retail+ abilities row: open the kuluu flat ability list instead of retail's
/// abimenu category picker. Kuluu's single-step list has no retail counterpart -
/// the real Abilities flow is two windows (FFXiMain.dll retail-2026-09 RVA
/// 0x13FA70 opens the `ability` list through its rows, row 6 opening `mount`;
/// vault main_menu.md §23) - so the flat list survives as an off-by-default
/// option.
pub const RETAIL_ABILITIES_FLAT_LIST: &str = "Flat Abilities List";

/// abimenu's five category rows in item-id order (vault main_menu.md §23:
/// items {1,2,3,5} open the `ability` list with filter triples and item 6 opens
/// `menu    mount`). The category semantics per row are [I-pending] on the DLL
/// side; this set is grounded by kuluu's existing target-action groups
/// (AbilityGroup::ALL in hud/action_model.rs, indices referenced positionally)
/// - noted on bead kuluu-o9qt7.7 per the ambiguity rule.
pub const ABILITIES_CATEGORY_LABELS: &[&str] = &[
    "Job Abilities",
    "Weapon Skills",
    "Ranged Attack",
    "Mount",
    "Pet Commands",
];

/// The picker row at display index `idx` as a positional index into
/// `AbilityGroup::ALL` (pairing pinned by a test).
pub fn abilities_category_index(label: &str) -> Option<usize> {
    ABILITIES_CATEGORY_LABELS
        .iter()
        .position(|cap| *cap == label)
}

const DEBUG_ENTRIES: &[&str] = &[
    DEBUG_PERF,
    DEBUG_TARGET_CYCLE,
    DEBUG_MESH,
    DEBUG_NET_STATUS,
    DEBUG_NOCLIP,
    #[cfg(feature = "enhanced-engage-move-lock-off")]
    DEBUG_ENGAGE_ANIM_LOCK,
    DEBUG_AUTO_ENTER_CS,
    DEBUG_WEATHER,
    DEBUG_FOG,
    DEBUG_FORCE_18,
    DEBUG_ENTITY_LIST,
    DEBUG_SOUND,
    DEBUG_VOLUME,
    DEBUG_PRINT_POS,
    DEBUG_STAIR_DRAW,
    DEBUG_STAIR_STATUS,
    DEBUG_GRAPHICS_DEBUG,
    DEBUG_POSITION_LOG,
    DEBUG_NAMEPLATES,
    DEBUG_UI_SETTINGS,
    DEBUG_RETAIL_SEPARATOR,
    DEBUG_RETAIL_LABEL,
    RETAIL_DLSS_MENU,
    #[cfg(feature = "enhanced-mob-hp-under")]
    RETAIL_MOB_HP_UNDER,
    #[cfg(feature = "enhanced-job-display")]
    RETAIL_JOB_DISPLAY,
    RETAIL_STATUS_ROWS,
    RETAIL_RESELECT_UNEQUIP,
    RETAIL_MAGIC_FLAT_LIST,
    RETAIL_ABILITIES_FLAT_LIST,
    #[cfg(feature = "enhanced-ignore-knockback-self")]
    RETAIL_KNOCKBACK,
    RETAIL_CAMERA_LEASH,
];

/// The settings pages whose rows are derived from `GraphicsSection` lists.
/// `None` for every other menu, which uses `static_entries`.
pub fn settings_rows(kind: MenuKind, dlss_supported: bool) -> Option<Vec<MenuRow>> {
    Some(match kind {
        MenuKind::Graphics => graphics_rows(dlss_supported),
        MenuKind::Config => config_rows(),
        MenuKind::GraphicsDlss => dlss_config_rows(),
        _ => return None,
    })
}

pub fn settings_row_at(kind: MenuKind, slot: usize, dlss_supported: bool) -> Option<MenuRow> {
    settings_rows(kind, dlss_supported)?.get(slot).copied()
}

/// The cyclable field at a cursor slot, or `None` on a header or action row.
pub fn settings_field_at(
    kind: MenuKind,
    slot: usize,
    dlss_supported: bool,
) -> Option<GraphicsField> {
    settings_row_at(kind, slot, dlss_supported)?.field()
}

/// Nudge a cursor off a header row in the direction it was already moving,
/// wrapping. Headers are chrome, so the cursor does not rest on one.
pub fn settle_cursor(kind: MenuKind, dlss_supported: bool, cursor: usize, down: bool) -> usize {
    let Some(rows) = settings_rows(kind, dlss_supported) else {
        return cursor;
    };
    if rows.is_empty() {
        return cursor;
    }
    let mut at = cursor.min(rows.len() - 1);
    for _ in 0..rows.len() {
        if rows[at].is_selectable() {
            return at;
        }
        at = if down {
            (at + 1) % rows.len()
        } else {
            at.checked_sub(1).unwrap_or(rows.len() - 1)
        };
    }
    cursor
}

/// Upper bound on a section page: one header per section plus its fields.
/// Callers add their own action rows.
const fn section_row_count(sections: &[GraphicsSection]) -> usize {
    let mut total = 0;
    let mut i = 0;
    while i < sections.len() {
        total += sections[i].fields.len() + 1;
        i += 1;
    }
    total
}

const MAX_ENTRY_COUNT: usize = {
    let r = ROOT_ENTRIES.len();
    let c = section_row_count(CONFIG_SECTIONS) + 1;
    let g = section_row_count(GRAPHICS_SECTIONS) + 2;
    let e = EQUIPMENT_ENTRIES.len();
    let s = STATUS_LABELS.len();

    let gd = section_row_count(DLSS_CONFIG_SECTIONS) + 1;

    let d = DYNAMIC_VISIBLE_ROWS;
    let rc = if r >= c { r } else { c };
    let rcg = if rc >= g { rc } else { g };
    let rcgd = if rcg >= gd { rcg } else { gd };
    let rcge = if rcgd >= e { rcgd } else { e };
    let rcges = if rcge >= s { rcge } else { s };
    if rcges >= d {
        rcges
    } else {
        d
    }
};

pub fn is_dynamic(kind: MenuKind) -> bool {
    matches!(
        kind,
        MenuKind::Magic
            | MenuKind::MagicGroup(_)
            | MenuKind::Abilities
            | MenuKind::AbilitiesGroup(_)
            | MenuKind::Party
            | MenuKind::Trade
            | MenuKind::TradePick { .. }
            | MenuKind::Items
            | MenuKind::KeyItems
            | MenuKind::UsableItems
            | MenuKind::ItemAction { .. }
            | MenuKind::EquipSlot(_)
            | MenuKind::EmoteList
            | MenuKind::Search
            | MenuKind::Linkshell
            | MenuKind::CraftList
            | MenuKind::RegionInfo { .. }
            | MenuKind::MissionList
            | MenuKind::MissionDetail { .. }
            | MenuKind::BazaarStalls
            | MenuKind::QuestList
            | MenuKind::QuestDetail { .. }
            | MenuKind::AutomatonEquip
            | MenuKind::MacroSelect
            | MenuKind::MacroEditor { .. }
            | MenuKind::JobPick { .. }
            | MenuKind::RoomList
            | MenuKind::RetailConfig
            | MenuKind::RetailConfigPage(_)
            | MenuKind::RetailConfigSub(_)
            | MenuKind::StatusDetail { .. }
            // kuluu's live key map (bead kuluu-o9qt7.26.1) — the store is the rows.
            | MenuKind::Controls
            | MenuKind::Arrange(_)
    ) || draws_itself(kind)
}

/// The Unity, alter ego point, merit and job point windows draw themselves
/// (`unity_fp_view`), each at its own record frame.
pub fn draws_itself(kind: MenuKind) -> bool {
    crate::hud::unity_fp::is_unity_fp_window(kind)
        || crate::hud::merit_windows::is_merit_window(kind)
}

pub fn entry_count(kind: MenuKind, dynamic: &DynamicMenu, dlss_supported: bool) -> usize {
    if is_dynamic(kind) {
        dynamic.rows.len().max(1)
    } else {
        entries_for(kind, dlss_supported).len()
    }
}

/// Rows to jump per Left/Right page in a vertical list menu — the visible-row
/// count of whichever panel renders `kind`, so one page-press scrolls a single
/// screenful (retail Left/Right paging). Items and the action-ring Usable list
/// ride the item_screen panel; everything else the generic panel.
pub fn list_page_rows(kind: MenuKind) -> usize {
    match kind {
        MenuKind::Items | MenuKind::UsableItems => crate::hud::item_screen::ITEM_LIST_ROWS,
        _ => crate::hud::unity_fp::unity_fp_page_rows(kind)
            .or_else(|| crate::hud::merit_windows::visible_list_rows(kind))
            .unwrap_or(DYNAMIC_VISIBLE_ROWS),
    }
}

/// New cursor after a Left/Right page press: jump `rows` toward `forward`
/// (Right/Down = true), clamped to the list — retail pages with no wrap, unlike
/// Up/Down which wrap one row at a time.
pub fn page_cursor(cursor: usize, entry_count: usize, rows: usize, forward: bool) -> usize {
    if forward {
        (cursor + rows).min(entry_count.saturating_sub(1))
    } else {
        cursor.saturating_sub(rows)
    }
}

/// The label at a cursor slot. Dynamic menus read the live row; every other
/// menu reads `static_entries`, which is empty for the section-derived pages
/// (Config/Graphics/GraphicsDlss) — callers that need those rows use
/// `entries_for`, which resolves them through `settings_rows`.
pub fn entry_label(kind: MenuKind, idx: usize, dynamic: &DynamicMenu) -> &str {
    if is_dynamic(kind) {
        if dynamic.rows.is_empty() {
            return empty_dynamic_hint(kind);
        }
        return dynamic
            .rows
            .get(idx)
            .map(|r| r.label.as_str())
            .unwrap_or("<unknown>");
    }
    if matches!(kind, MenuKind::Root) {
        if let Some(label) = dynamic.selector_captions.get(idx) {
            return label;
        }
    }
    // A shut house and an open one can never share visible text once the catalog has loaded.
    if matches!(kind, MenuKind::HomeView) && idx == home_door_row_index() {
        if let Some(label) = dynamic.home_door_caption.as_deref() {
            return label;
        }
    }
    static_entries(kind)
        .get(idx)
        .copied()
        .unwrap_or("<unknown>")
}

pub fn entry_action(
    kind: MenuKind,
    idx: usize,
    dynamic: &DynamicMenu,
) -> Option<DynamicMenuAction> {
    if !is_dynamic(kind) {
        return None;
    }
    dynamic.rows.get(idx).map(|r| r.action)
}

/// Remaining recast (seconds) for a menu action, or `None` if ready. `now` must
/// be [`kuluu_snapshot::recast_now_unix`] — the clock the 0x119 ABIL_RECAST
/// expiries are stamped with. The single
/// source of truth for both greying a menu row and blocking its confirm. Spells
/// have no snapshot recast field yet (LSB never sends magic recasts), so only
/// job/pet abilities resolve here.
pub fn action_recast_remaining(
    ability_recasts: &[(u16, u32)],
    action: &DynamicMenuAction,
    now: u32,
) -> Option<u32> {
    let ability_id = match action {
        DynamicMenuAction::JobAbility { ability_id }
        | DynamicMenuAction::PetAbility { ability_id } => *ability_id,
        _ => return None,
    };
    let recast_id = ffxi_vocab::recast::ability_recast_id(ability_id)?;
    let expiry = ability_recasts
        .iter()
        .find(|(rid, _)| *rid == recast_id)
        .map(|(_, e)| *e)?;
    let remaining = expiry.saturating_sub(now);
    (remaining > 0).then_some(remaining)
}

fn empty_dynamic_hint(kind: MenuKind) -> &'static str {
    match kind {
        MenuKind::Magic => "(no spells learned yet)",
        MenuKind::MagicGroup(_) => "(no spells learned in this school)",
        MenuKind::Abilities => "(no abilities available — wrong job?)",
        MenuKind::AbilitiesGroup(group) => crate::hud::action_model::AbilityGroup::ALL
            .get(group as usize)
            .map(|g| ability_group_empty_hint(*g))
            .unwrap_or("(empty)"),
        MenuKind::Party => "(not in a party)",
        // A trade window with no transaction behind it (the invite timed out on
        // the server and already sent RES{Cancell}) keeps the §25 hint state.
        MenuKind::Trade => "(not trading)",
        MenuKind::TradePick { .. } => "(nothing to offer)",
        MenuKind::Items => "(bag empty)",
        MenuKind::KeyItems => "(no key items)",
        MenuKind::UsableItems => "(no usable items)",
        MenuKind::ItemAction { .. } => "(item no longer in this bag)",
        MenuKind::EquipSlot(_) => "(no equippable items for this slot)",
        MenuKind::EmoteList => "(emote table unavailable)",
        // The Run row is always present; this hint covers the no-results state
        // (scresult empty before a query has run, vault main_menu.md section 26).
        MenuKind::Search => "(no search results yet)",
        _ => "(empty)",
    }
}

/// Every row label of a page, whichever model backs it.
pub fn entries_for(kind: MenuKind, dlss_supported: bool) -> Vec<&'static str> {
    match settings_rows(kind, dlss_supported) {
        Some(rows) => rows.iter().map(|r| r.label()).collect(),
        None => static_entries(kind).to_vec(),
    }
}

fn static_entries(kind: MenuKind) -> &'static [&'static str] {
    match kind {
        MenuKind::Root => ROOT_ENTRIES,
        MenuKind::Config => &[],
        // The live key map arrives via controls_rows in refresh_dynamic_menu_rows
        // (bead kuluu-o9qt7.26.1); nothing static stands here.
        MenuKind::Controls => &[],
        MenuKind::Debug => DEBUG_ENTRIES,
        MenuKind::Graphics => &[],

        // Both magic lists are snapshot-built (`spell_rows` / `magic_school_rows`):
        // an empty one renders its `empty_dynamic_hint`, never a stub.
        MenuKind::Magic => &[],
        MenuKind::MagicGroups => MAGIC_CATEGORY_LABELS,
        MenuKind::MagicGroup(_) => &[],
        // Both ability lists are snapshot-built (`ability_flat_rows` /
        // `ability_group_rows`, fed by the s2c COMMAND_DATA bitmaps), so an empty one
        // renders its `empty_dynamic_hint`, never a stub.
        MenuKind::Abilities => &[],
        MenuKind::AbilitiesGroups => ABILITIES_CATEGORY_LABELS,
        MenuKind::AbilitiesGroup(_) => &[],
        MenuKind::Party => &[],
        MenuKind::Trade => &[],
        MenuKind::TradePick { .. } => &[],
        MenuKind::Search => &[],
        MenuKind::Items => ITEMS_ENTRIES_STUB,
        MenuKind::KeyItems => &[],
        MenuKind::UsableItems => &[],

        MenuKind::Equipment => EQUIPMENT_ENTRIES,

        MenuKind::Status => STATUS_LABELS,

        // The detail windows' rows are the live snapshot readout built by
        // refresh_dynamic_menu_rows (vault main_menu.md §59); nothing static stands here.
        MenuKind::StatusDetail { .. } => &[],

        MenuKind::ItemAction { .. } => &[],
        MenuKind::ItemDropConfirm { .. } => DROP_CONFIRM_ENTRIES,
        MenuKind::PowerPrompt { .. } => DROP_CONFIRM_ENTRIES,
        // The key-assignment reset prompt answers through the same Yes/No law as
        // every other retail confirm (dialog service FFXiMain.dll retail-2026-09
        // RVA 0x210E30; vault main_menu.md §59 addenda 7–8).
        MenuKind::KeyAssignResetPrompt => DROP_CONFIRM_ENTRIES,
        // The pick list is always snapshot-built (equip_slot_rows); an empty bag
        // renders the `empty_dynamic_hint` for this kind, never a loading stub.
        MenuKind::EquipSlot(_) => &[],

        MenuKind::Communication => COMMUNICATION_ENTRIES,
        // Retail law: the friend roster is POL-era account data with no modern
        // wire — LSB carries SearchType Friend 0x0C enum-only (no handler case)
        // and ships no friends table, so on any LSB server the list is always
        // empty (vault main_menu.md §38.5; the live system of that family is the
        // black list, which kuluu may surface separately — not via this row).
        MenuKind::FriendList => &[],
        // The pupequip variant's fourteen rows come from s2c 0x044 (dynamic;
        // automaton_equip_rows, vault main_menu.md §39).
        MenuKind::AutomatonEquip => &[],
        // `mp_mncst` chrome carries only three unnamed buttons and no captions
        // (DAT mp_1, vault main_menu.md §39) — the grounded empty display.
        MenuKind::MpCustomize => &[],
        // The myroom home-view ops panel: fourteen captioned widgets read from
        // the DAT chunk (vault main_menu.md §40 as corrected by §53).
        MenuKind::HomeView => HOME_VIEW_ROWS,
        // jobchang's two widgets — catalog tokens 65/66 in DAT record 'jobc'
        // (vault main_menu.md §53); the pick lists are roster-built.
        MenuKind::JobChange => JOB_CHANGE_ROWS,
        // The roomlist's five rows are DAT-synthesised (record slots + table-12
        // captions through ConfigDatRows; refresh arm) — never transcribed here.
        MenuKind::RoomList => &[],
        // Retail raises the generic yes/no dialog on a style row (.text FFXiMain.dll
        // retail-2026-09 0x1D6B20 → dialog service .text 0x210E30, prompt id 451
        // "Make changes?"); kuluu serves it with the same two rows as every other
        // confirm panel (§59 addendum 20/21).
        MenuKind::RoomListConfirm { .. } => DROP_CONFIRM_ENTRIES,
        MenuKind::JobPick { .. } => &[],
        // Macro editor rows come from the store (one command line per row plus
        // the add/clear actions below).
        MenuKind::MacroEditor { .. } => &[],
        MenuKind::EmoteList => &[],
        // `abisortw` has no DAT window record at all [V-negative] and never renders
        // as a focused level in kuluu (its select rides the list's chain row), so it
        // owns no static entries (bead kuluu-o9qt7.34).
        MenuKind::AbilitySort { .. } => &[],
        MenuKind::GraphicsDlss => &[],
        // The Map screen renders its own bespoke panes; it has no generic list.
        MenuKind::Map => &[],
        // Slot rows come from the server state, never a static table (vault main_menu.md section 27).
        MenuKind::Linkshell => &[],
        // The cmbmenu launcher's two fixed rows: select ids 1-2 of handler FFXiMain.dll
        // retail-2026-09 RVA 0x1E29A0 (bag-pick, then craft list; vault main_menu.md §22/§28). No DAT
        // caption was recovered for either row (§22 caption law), so kuluu's labels are pinned by tests.
        MenuKind::SynthLauncher => SYNTH_LAUNCHER_ENTRIES,
        // The mcrmenu launcher's two fixed rows (handler FFXiMain.dll
        // retail-2026-09 RVA 0x1A64B0; vault main_menu.md §34).
        MenuKind::MacrosPalette => MACRO_LAUNCHER_ENTRIES,
        // mcrselec rows arrive from the store (dynamic; macro_select_rows and
        // vault main_menu.md §41). mcres20 shows run results, of which kuluu
        // has none until macros execute — the grounded empty display.
        MenuKind::MacroSelect => &[],
        MenuKind::MacroResult => &[],
        // DAT-driven rows: the ConfigDatRows arms of refresh_dynamic_menu_rows read
        // each window's captioned item blocks (row law vault main_menu.md §59;
        // drill law §42/§47; generations §49/§57); no transcribed row content
        // stands here anymore.
        MenuKind::RetailConfig | MenuKind::RetailConfigPage(_) | MenuKind::RetailConfigSub(_) => {
            &[]
        }
        // The faqsub Help Desk panel rows (vault main_menu.md §36: the
        // big-widget caption builder loads exactly these destination strings).
        MenuKind::HelpDeskPanel => HELPDESK_PANEL_ENTRIES,
        // cmbhlst-style rows come from the recipe table filtered against the bag (vault main_menu.md §28).
        MenuKind::CraftList => &[],
        MenuKind::RegionInfo { .. } => &[],
        MenuKind::MissionList => &[],
        MenuKind::MissionDetail { .. } => &[],
        // Stall rows come from the zone entity stream (vault main_menu.md §33).
        MenuKind::BazaarStalls => &[],
        MenuKind::QuestList => &[],
        MenuKind::QuestDetail { .. } => &[],
        // The Unity and alter ego point windows list DAT-labelled rows
        // (`unity_fp::unity_fp_rows`).
        MenuKind::UnityMenu
        | MenuKind::UnityChat
        | MenuKind::UnityRankingChooser
        | MenuKind::UnityRanking { .. }
        | MenuKind::UnityLeaders
        | MenuKind::AlterEgoCategories
        | MenuKind::AlterEgoUpgrades { .. }
        | MenuKind::AlterEgoConfirm { .. } => &[],
        // `merit_windows::merit_rows_for`.
        MenuKind::Merit(_) => &[],
        // `myroom_arrange::ArrangeEditor::rows`.
        MenuKind::Arrange(_) => &[],
    }
}

pub fn menu_title(kind: MenuKind) -> &'static str {
    match kind {
        MenuKind::Root => "Commands",
        MenuKind::Config => "Interface",
        MenuKind::Controls => "Controls",
        MenuKind::Debug => "Debug",
        MenuKind::Graphics => "Graphics",
        MenuKind::Equipment => "Equipment",
        MenuKind::Magic => "Magic",
        // The picker keeps the Magic row's own caption; the spell list it opens is
        // titled `Magic List` (ROM/165/76.DAT catalog index 76, §10 - which one the
        // root row uses stays [I], the list title is the grounded side).
        MenuKind::MagicGroups => "Magic",
        MenuKind::MagicGroup(_) => "Magic List",
        MenuKind::Abilities => "Abilities",
        // Like Magic's pair: the picker carries the row caption, the filtered
        // list window is titled by the row it answers (vault §23).
        MenuKind::AbilitiesGroups => "Abilities",
        MenuKind::AbilitiesGroup(_) => "Ability List",
        // The prtyN family's DAT records carry no inline captions (§24); the
        // Root-row caption is the grounded title choice. Neither does `trade`
        // or `gift` (vault main_menu.md §25), so kuluu titles its mirrored
        // pair-window the same way.
        MenuKind::Party => "Party",
        MenuKind::Trade => "Trade",
        MenuKind::Search => "Search",
        // Neither `lscswin` nor the `linkN` records carry inline captions (vault main_menu.md
        // section 27.5), so kuluu titles its slot window after the Root row, like Party/Trade/Search.
        MenuKind::Linkshell => "Link Shell",
        MenuKind::SynthLauncher => "Synthesis",
        // DAT `mcrm` carries comwin title token 80 = catalog id 80
        // `MacroPalette` (vault main_menu.md §34) — the grounded title.
        MenuKind::MacrosPalette => "MacroPalette",
        // Neither `mcrselec` nor `mcres20` carries a DAT title token (both
        // comwin records read -1; vault main_menu.md §41), so both inherit the
        // parent row's caption like the other token-less windows here.
        MenuKind::MacroSelect => "Macros",
        MenuKind::MacroResult => "Macros",
        // DAT `conf` carries comwin title token 54 = catalog id 54 `Config`
        // (vault main_menu.md §35) — the grounded title.
        MenuKind::RetailConfig => "Config",
        // No config page window carries a usable comwin title token (vault
        // main_menu.md §47), so each inherits the hub's grounded title.
        MenuKind::RetailConfigPage(_) => "Config",
        MenuKind::RetailConfigSub(_) => "Config",
        // The `faqs` DAT record carries no title (token -1); the grounded
        // caption is catalog string 60 `Help Desk` in ROM/165/76.DAT — same as
        // the Root row's own label (§36).
        MenuKind::HelpDeskPanel => "Help Desk",
        MenuKind::CraftList => "Craft List",
        MenuKind::RegionInfo { .. } => "Region Info",
        MenuKind::MissionList => "Missions",
        MenuKind::MissionDetail { .. } => "Mission Log",
        MenuKind::BazaarStalls => "Bazaar",
        MenuKind::QuestList => "Quests",
        MenuKind::QuestDetail { .. } => "Quest Log",
        // The bag picker hung off the trade window's Add-Item button row; its
        // own caption is [I] like the rest of the family (§25).
        MenuKind::TradePick { .. } => "Add Item",
        MenuKind::Items => "Items",
        MenuKind::KeyItems => "Key Items",
        MenuKind::UsableItems => "Items",
        MenuKind::ItemAction { .. } => "Item",
        MenuKind::ItemDropConfirm { .. } => "Drop",
        // Retail titles the prompt after the command row that opened it.
        MenuKind::PowerPrompt {
            shut_down: true, ..
        } => ROOT_SHUT_DOWN,
        MenuKind::PowerPrompt {
            shut_down: false, ..
        } => ROOT_LOG_OUT,
        // The reset prompt belongs to the Config family; retail's dialog carries its body text from DAT (caption catalog table 0xb, string id 0x8F), so kuluu keeps the family title for the frame.
        MenuKind::KeyAssignResetPrompt => "Config",
        MenuKind::Status => "Status",
        // The companion window's retail title IS its subject; the detail table
        // (FFXiMain.dll retail-2026-09 RVA 0x3716B8) opens each under the same
        // Status heading, so kuluu titles it by the row that opened it.
        MenuKind::StatusDetail { entry } => STATUS_LABELS
            .get(entry as usize)
            .copied()
            .unwrap_or("Status"),
        MenuKind::EquipSlot(_) => "Equip",
        MenuKind::Communication => "Communication",
        MenuKind::FriendList => "Friend List",
        // The pupequip DAT comwin record carries no title token (vault
        // main_menu.md §39), so the variant keeps the parent row's caption.
        MenuKind::AutomatonEquip => "Equipment",
        // DAT mp_1's comwin record carries title token 165, and the caption
        // catalog resolves it to `Customization` (vault main_menu.md §39).
        MenuKind::MpCustomize => "Customization",
        // The `myro` DAT record carries no title token (comwin `anc anc_s -1`,
        // vault main_menu.md §40), so — as with the pupequip variant — kuluu
        // inherits the parent row's caption.
        MenuKind::HomeView => "Map",
        // jobchang's frame carries no title token of its own in DAT record
        // 'jobc' (vault main_menu.md §53); catalog caption 67 grounds both it
        // and the pick lists drilled from its rows.
        MenuKind::JobChange => "Change Jobs",
        // Neither `roomlist` nor its confirm dialog carries a DAT title token (its
        // widget caption tokens are all −1 [V-neg]), so both inherit the myroom row's
        // label that opened them, like the other token-less windows here.
        MenuKind::RoomList | MenuKind::RoomListConfirm { .. } => HOME_ROW_REMODEL,
        MenuKind::JobPick { .. } => "Change Jobs",
        // mcrledit's DAT chrome has no title token — it inherits Macros (§41
        // title law).
        MenuKind::MacroEditor { .. } => "Macros",
        MenuKind::EmoteList => "Emote List",
        // No invented copy: `abisortw` has no DAT title (no window record at all
        // [V-negative]) and never shows as a focused level in kuluu; its only text is
        // the mode caption read from table 0x0E (bead kuluu-o9qt7.34).
        MenuKind::AbilitySort { .. } => "",
        MenuKind::GraphicsDlss => "DLSS Config",
        MenuKind::Map => "Map",
        // These windows draw their own DAT text; kuluu-only surfaces name them after the Status
        // row that opens them.
        MenuKind::UnityMenu
        | MenuKind::UnityChat
        | MenuKind::UnityRankingChooser
        | MenuKind::UnityRanking { .. }
        | MenuKind::UnityLeaders => STATUS_LABELS[STATUS_ROW_UNITY],
        MenuKind::AlterEgoCategories
        | MenuKind::AlterEgoUpgrades { .. }
        | MenuKind::AlterEgoConfirm { .. } => STATUS_LABELS[STATUS_ROW_ALTER_EGO_POINTS],
        MenuKind::Merit(window) if window.is_merit_chain() => {
            STATUS_LABELS[STATUS_ROW_MERIT_POINTS]
        }
        MenuKind::Merit(_) => STATUS_LABELS[STATUS_ROW_JOB_POINTS],
        // The arrange windows' captions are their DAT items; the bar keeps layout mode's title.
        MenuKind::Arrange(_) => "",
    }
}

/// MenuKind::Party rows, following the Party-row helper's state dispatch
/// (FFXiMain.dll retail-2026-09 RVA 0x2016E0; vault main_menu.md §24): while a
/// solicit is unresolved it opens `prtyjoin`, whose answer rows are YES/NO
/// (c2s GROUP_SOLICIT_RES Res=1/0, vendor/server/src/map/packets/c2s/
/// 0x074_group_solicit_res.h); otherwise the window shows the member roster,
/// and a solo player sees the display-only empty state (`prty1`).
pub fn party_window_rows(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<DynamicMenuRow> {
    if let Some(invite) = &snap.party_invite {
        return vec![
            DynamicMenuRow {
                tint: None,
                label: format!("Yes (join {})", invite.name),
                action: DynamicMenuAction::AcceptPartyInvite,
            },
            DynamicMenuRow {
                tint: None,
                label: "No".to_string(),
                action: DynamicMenuAction::DeclinePartyInvite,
            },
        ];
    }
    snap.party
        .iter()
        .map(|m| DynamicMenuRow {
            tint: None,
            label: m.name.clone().unwrap_or_else(|| format!("{:08X}", m.id)),
            action: DynamicMenuAction::PartyMemberRow { char_id: m.id },
        })
        .collect()
}

/// Label for one cell of the trade slot grid from its s2c view row. Neither
/// `trade` nor `gift` carries inline DAT captions (vault main_menu.md §25), so
/// these labels are kuluu's choice, pinned by tests; the 0xFFFF sentinel IS the
/// gil cell per player_trade.cpp.
fn trade_cell_label(view: Option<kuluu_snapshot::TradeOffer>, is_gil_slot: bool) -> String {
    match view {
        Some(offer) if is_gil_slot => format!("Gil x{}", offer.qty),
        Some(offer) => {
            let name = ffxi_vocab::item_names::lookup(offer.item_no)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{:#06X}", offer.item_no));
            if offer.qty > 1 {
                format!("{name} x{}", offer.qty)
            } else {
                name
            }
        }
        None if is_gil_slot => "Gil".to_string(),
        None => "(empty)".to_string(),
    }
}

/// Rows for the trade window pair — retail opens `trade` (own side, selectable)
/// and `gift` (partner view, display-only) together as one state machine
/// (FFXiMain.dll retail-2026-09 vtable 0x335060 handler RVA 0x1515E0; vault
/// main_menu.md §25). kuluu mirrors the pair as one menu: while s2c 0x021 is
/// unanswered it shows the answer rows, else gil (trade slot 0 per LSB's
/// `GilSlot = 0`), own item slots 1..8 (select byte table .rdata 0x371D60),
/// then the button law {9=Confirm, 10=Exit}, and finally the partner-side
/// views appended as display rows.
pub fn trade_window_rows(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<DynamicMenuRow> {
    let Some(trade) = snap.trade.as_deref() else {
        return Vec::new();
    };
    if trade.incoming_pending {
        let name = snap
            .entities
            .iter()
            .find(|e| e.id == trade.partner_id)
            .and_then(|e| e.name.clone())
            .unwrap_or_else(|| format!("{:08X}", trade.partner_id));
        return vec![
            DynamicMenuRow {
                tint: None,
                label: format!("Yes (accept {})", name),
                action: DynamicMenuAction::TradeAcceptInvite,
            },
            DynamicMenuRow {
                tint: None,
                label: "No".to_string(),
                action: DynamicMenuAction::TradeDeclineInvite,
            },
        ];
    }
    let mut rows = Vec::new();
    rows.push(DynamicMenuRow {
        tint: None,
        label: trade_cell_label(trade.my_slots.first().copied().flatten(), true),
        action: DynamicMenuAction::TradeGilRow,
    });
    for idx in 1..ffxi_proto::decode::TRADE_SLOT_COUNT {
        rows.push(DynamicMenuRow {
            tint: None,
            label: trade_cell_label(trade.my_slots.get(idx).copied().flatten(), false),
            action: DynamicMenuAction::TradeOwnSlot {
                trade_index: idx as u8,
            },
        });
    }
    rows.push(DynamicMenuRow {
        tint: None,
        label: if trade.my_confirmed {
            "Confirm (waiting for partner)".to_string()
        } else {
            "Confirm".to_string()
        },
        action: DynamicMenuAction::TradeConfirmRow,
    });
    rows.push(DynamicMenuRow {
        tint: None,
        label: "Exit".to_string(),
        action: DynamicMenuAction::TradeCancelRow,
    });
    for idx in 0..ffxi_proto::decode::TRADE_SLOT_COUNT {
        rows.push(DynamicMenuRow {
            tint: None,
            label: trade_cell_label(trade.partner_slots.get(idx).copied().flatten(), idx == 0),
            action: DynamicMenuAction::TradePartnerOfferRow {
                trade_index: idx as u8,
            },
        });
    }
    rows
}

/// Add-Item bag picker rows for one own-side slot (retail opens this from an
/// empty slot row; vault main_menu.md §25 [I-pending] keeps the qty keypad
/// undecoded, so kuluu stages ONE per pick — the minimum legal ItemNum under
/// player_trade.cpp setSlot). Locked and server-unselectable slots are skipped:
/// LSB's own validation refuses them (Exclusive flag / ItemLockFlg::NoSelect).
pub fn trade_pick_rows(
    snap: &kuluu_snapshot::SceneSnapshot,
    trade_index: u8,
) -> Vec<DynamicMenuRow> {
    snap.inventory_main()
        .iter()
        .filter(|slot| !slot.locked && !slot.unselectable)
        .filter_map(|slot| {
            let name = ffxi_vocab::item_names::lookup(slot.item_no)?;
            Some(DynamicMenuRow {
                tint: None,
                label: if slot.quantity > 1 {
                    format!("{name} x{}", slot.quantity)
                } else {
                    name.to_string()
                },
                action: DynamicMenuAction::TradeStageFromBag {
                    trade_index,
                    item_no: slot.item_no,
                    qty: 1,
                    bag_slot: slot.index,
                },
            })
        })
        .collect()
}

/// Rows of the Search surface — one menu for retail's `searchma` form and its
/// `scresult` model list (vault main_menu.md §26). The Run row leads (select id
/// 1 → query-object method 0x20EE70), then one row per hit in server order.
/// Label law follows the AddPlayer fields: name, job abbrev + levels (`???` for
/// the hidden shape behind flags1 & 0x4000 — those entries vanish from the
/// frame), and zone id. kuluu has no filter editor yet, so Run sends an empty
/// entry list, which LSB's _HandleSearchRequest defaults read as "everyone".
/// The form sub-windows (scname/searchjo/searchna/… select ids 2-7,17) are the
/// documented follow-ups for this bead family.
pub fn search_window_rows(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<DynamicMenuRow> {
    let mut rows = vec![DynamicMenuRow {
        tint: None,
        label: "Run Search".to_string(),
        action: DynamicMenuAction::SearchRun,
    }];
    let Some(results) = snap.search.as_deref() else {
        return rows;
    };
    for (index, hit) in results.players.iter().enumerate() {
        let shape = match (hit.main_job, hit.main_level) {
            (Some(job), Some(level)) => {
                let job = ffxi_vocab::job_names::abbrev(u16::from(job)).unwrap_or("??");
                let sub = match (hit.sub_job, hit.sub_level) {
                    (Some(sjob), Some(slvl)) if slvl > 0 => format!(
                        "/{}{slvl}",
                        ffxi_vocab::job_names::abbrev(u16::from(sjob)).unwrap_or("??")
                    ),
                    _ => String::new(),
                };
                format!("{job}{level}{sub}")
            }
            _ => "???".to_string(),
        };
        rows.push(DynamicMenuRow {
            tint: None,
            label: format!("{}  {shape}  Z{}", hit.name, hit.zone),
            action: DynamicMenuAction::SearchHitRow { index },
        });
    }
    rows
}

/// The submenu a Root category drills into, and thus the level the right pane
/// previews while that category is highlighted at Root. Single source for the
/// Root → submenu mapping: `text_input::resolve_menu_entry` drills through this
/// so the client dispatch and the preview can't disagree (guarded by a test in
/// the client). `None` = a Root entry with no browsable submenu (Macros isn't
/// built; Current Time/Key Items run their retail text commands, Log Out/Shut
/// Down act directly), so the right pane shows no preview.
/// `flat_magic` is the Retail+ "Flat Magic List" gate
/// ([`RETAIL_MAGIC_FLAT_LIST`] in GraphicsSettings): on, kuluu's one-step spell
/// list replaces retail's two-window flow (vault main_menu.md §21).
/// `flat_abilities` is the same pattern for Abilities
/// ([`RETAIL_ABILITIES_FLAT_LIST`]; vault main_menu.md §23).
/// Row law for the Link Shell window (vault main_menu.md section 27): retail's `menu    link5`
/// mirrors the two shell slots LSB tracks (`LinkshellNum` 1/2), and all the server ever says about a
/// slot is which inventory item is its pearl - s2c 0x0E0 `{ LinkshellNum, ItemIndex, Category }`,
/// zeros meaning "nothing equipped". The shell *name* lives in the pearl's exdata (which kuluu does
/// not ingest yet) and retail's own row captions are string-table ids (section 27.5), so rows stay
/// slot-labelled rather than invented.
pub fn linkshell_window_rows(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<DynamicMenuRow> {
    [1u8, 2]
        .into_iter()
        .map(|slot| {
            let state = snap.linkshells.slot(slot).copied().unwrap_or_default();
            let label = if state.is_occupied() {
                format!(
                    "Link Shell {}   pearl at container {}, index {}",
                    slot, state.category, state.item_index
                )
            } else {
                format!("Link Shell {}   (no pearl equipped)", slot)
            };
            DynamicMenuRow {
                tint: None,
                label,
                action: DynamicMenuAction::LinkShellSlot {
                    linkshell_slot: slot,
                },
            }
        })
        .collect()
}

/// The cmbmenu launcher's two rows. Neither window carries DAT captions (§22 caption law: the
/// `cmbmenu`/`cmbhlst` mini-chunks are chrome-only and no short craft strings exist in the ROM/165
/// message catalog), so these kuluu-picked labels stand pinned by tests; their order follows
/// select ids 1-2 of handler FFXiMain.dll retail-2026-09 RVA 0x1E29A0 (vault main_menu.md §22/§28).
pub const SYNTH_ROW_PICK_MATERIAL: &str = "Synthesize from a bag item";
pub const SYNTH_ROW_OPEN_LIST: &str = "Open the craft list";
const SYNTH_LAUNCHER_ENTRIES: &[&str] = &[SYNTH_ROW_PICK_MATERIAL, SYNTH_ROW_OPEN_LIST];

/// The mcrmenu launcher's two rows. DAT `mcrm` carries no captions — the
/// windowps pair is chrome-only (vault main_menu.md §34) — so these
/// kuluu-picked labels stand pinned by tests; their order follows select ids
/// {1,2} of handler FFXiMain.dll retail-2026-09 RVA 0x1A64B0: item 1 opens
/// `mcrselec`, item 2 opens `mcres20`.
pub const MACRO_ROW_SELECT_MACRO: &str = "Select Macro";
pub const MACRO_ROW_MACRO_RESULT: &str = "Macro Result";
const MACRO_LAUNCHER_ENTRIES: &[&str] = &[MACRO_ROW_SELECT_MACRO, MACRO_ROW_MACRO_RESULT];

/// The Config family's windows, in kuluu drill order — the shape the DATs cannot
/// state. Every row label inside these windows lives only in the DATs: rows are
/// the item blocks with a UI string (§59; `live_items`), their captions the
/// textA catalog strings (vault main_menu.md §43/§49/§57; generations resolved
/// through VTABLE/FTABLE by [`ffxi_dat::menu_windows::windows_from_root`], never
/// transcribed here).
///
/// Which page window each hub row opens is the handler's jump table: select id i+1 pushes
/// name literal `RETAIL_CONFIG_PAGE_WINDOWS[i]`, FFXiMain.dll retail-2026-09 RVA 0x196770,
/// jump table .text 0x1968A4 — the twelve literals of §42 in branch order. The hub opens
/// as `configwi` (the named-root generation, vault main_menu.md §42/§49); its current-gen
/// chunk carries **thirteen** widgets but the jump table has twelve name-open branches, so
/// row 13 (`Boots up the FINAL FANTASY XI GAMEPAD Config program.`) answers through the
/// handler's default tail — kuluu gives it no page drill (vault main_menu.md §57).
/// `conf5m`'s window drills further: select ids 1/2 name-open `conf5w2`/`conf5w1`, and its
/// third arm opens `conf5win` (handler FFXiMain.dll retail-2026-09 RVA 0x199240, vault
/// main_menu.md §47; the w↔p pairing stays [I] as recorded there). The two key-assignment
/// windows carry only key-slot chrome in both DAT generations (§47), so nothing drills
/// them yet and they list no captioned rows.
pub const RETAIL_CONFIG_HUB_WINDOW: &str = "configwi";
pub const RETAIL_CONFIG_PAGE_WINDOWS: [&str; 12] = [
    "conf2win", "cfilter", "conftxtc", "conf5m", "conf3win", "conf6win", "fxfilter", "conf7",
    "conf4", "conf11m", "conf12wi", "conf13wi",
];
/// The windows a Config-family page drills into, indexed by the arm tables
/// below. Indices 5..=10 are the cascade companions: `confyn` (padded literal
/// `menu    confyn  `, VA 0x1037A744), `textcol1`/`textcol2`
/// 0x1037A758/0x1037A76C, the RGB channel panel `textcol3` 0x1037A780 — the
/// window behind widget slot [0x10662738], whose menu record is registry
/// FFXiMain.dll retail-2026-09 RVA 0x3751F0 — and `conf11l`/`conf11s`
/// 0x1037A7D0/0x1037A7E4. Every one is opened through menu manager
/// FFXiMain.dll retail-2026-09 RVA 0x15E350 (vault main_menu.md §59 addenda
/// 4/5). They load here so each drill has DAT rows even before its own arm law
/// lands.
pub const RETAIL_CONFIG_SUB_WINDOWS: [&str; 11] = [
    "conf5win", "conf5w1", "conf5w2", "k1assign", "k2assign", "confyn", "textcol1", "textcol2",
    "textcol3", "conf11l", "conf11s",
];

/// Cascade-companion indices into [`RETAIL_CONFIG_SUB_WINDOWS`], named for the
/// handler arms that open them.
pub const CONFIG_SUB_CONFYN: u8 = 5;
pub const CONFIG_SUB_TEXTCOL2: u8 = 6;
pub const CONFIG_SUB_TEXTCOL1: u8 = 7;
pub const CONFIG_SUB_CONF11L: u8 = 9;
pub const CONFIG_SUB_CONF11S: u8 = 10;

/// The Config family's option rows, read from the DATs at runtime: each window kuluu
/// opens maps to its item blocks in file order (= the handler's select-id space;
/// `live_items` — NOT the partial widget list, see ffxi-dat's row-law pin),
/// labelled through their textA catalog strings (vault main_menu.md §43/§59).
/// Loaded once on first use; a machine without an install shows empty windows
/// rather than any transcribed copy, per the owner's DAT-first rule.
#[derive(Resource, Default)]
pub struct ConfigDatRows {
    pub loaded: bool,
    /// Rows per window root in **record file order** — the handler's select-id space
    /// (`configwi`'s inline class handler FFXiMain.dll retail-2026-09 RVA 0x196770 switches on
    /// `select-id − 1`, jump table .text 0x1968A4, so the arm tables index this order).
    pub by_root: std::collections::BTreeMap<String, Vec<(u32, String)>>,
    /// Each root's row block y in the same file order as [`Self::by_root`]. Retail draws a
    /// page wherever each item block says (`y`, then `x`) and that order is NOT select-id
    /// order — measured on `configwi`, whose thirteen rows display as ids
    /// 1,2,3,4,10,5,6,11,12,7,8,9,13 (vault main_menu.md §59 addendum 14). The arm stays
    /// attached to its own row, so only the finished row list is reordered.
    pub row_ys: std::collections::BTreeMap<String, Vec<i16>>,
    /// Each root's row DAT item ids in the same file order as [`Self::by_root`], for the windows
    /// whose rows are their record's items.
    pub item_ids: std::collections::BTreeMap<String, Vec<i16>>,
    /// Each root's record item count.
    pub item_counts: std::collections::BTreeMap<String, u8>,
    /// Every menu record's own frame (x, y, width in the 512×448 design space;
    /// anchor law vault main_menu.md §7/§16) — window placement IS DAT data
    /// (companion_windows law: each window sits at its record's rect).
    pub frames: std::collections::BTreeMap<String, (i16, i16, i16)>,
    /// Each record frame's height and anchor byte, keyed like [`Self::frames`].
    pub frame_extents: std::collections::BTreeMap<String, (i16, u8)>,
    /// The myroom door row's two visible captions: closed at
    /// [`ffxi_dat::menu_windows::MOGHOUSE_DOOR_CAPTION_CLOSED_ID`] and open at
    /// `.._OPEN_ID`, both from the caption catalog (index 0 = shut, 1 = open). Retail rewrites the
    /// row's text field on every `myroom` update — FFXiMain.dll retail-2026-09 `.text 0x145439`
    /// (shut) / `.text 0x145357` (open), through the caption setter `.text 0x11ADB0` — and picks by
    /// its door byte, so kuluu picks from [`kuluu_snapshot::MyRoom::door_open`] the same way.
    pub home_door_captions: [Option<String>; 2],
    /// The roomlist remodel confirm question, read from GetText **table 12 id
    /// [`ROOMLIST_CONFIRM_PROMPT_ID`]** at load — retail's row callback hands exactly that string
    /// to the dialog service (.text FFXiMain.dll retail-2026-09 0x1D6B85 fetches it just before
    /// calling helper 0x210E30; vault main_menu.md §59 addendum 21). `None` until the DAT loads.
    pub roomlist_prompt: Option<String>,
    /// The `abisortw` window's mode captions, read from GetText **table-0x0E** ids
    /// [`ABISORTW_CAPTION_MODE_EMOTE_ID`] / [`ABISORTW_CAPTION_MODE_MOUNT_ID`], indexed by the
    /// handler's mode word `[this+0x14]`: index 0 (ability) stays `None` because its drawer arm
    /// draws no caption there (FFXiMain.dll retail-2026-09 `.text 0x223E10`; bead kuluu-o9qt7.34).
    pub abisortw_captions: [Option<String>; 3],
    /// The emote window's two swap prompts, read from GetText **table-0x0E** ids
    /// [`EMOTE_SORT_PROMPT_FIRST_ID`] / [`EMOTE_SORT_PROMPT_SECOND_ID`]: retail swaps the whole
    /// handler family while its state word `[emote+0x6C]` is 1/2 and replaces the help line with
    /// these strings (.text FFXiMain.dll retail-2026-09 `.text 0x225FA2`; bead kuluu-o9qt7.34).
    pub emote_sort_prompts: [Option<String>; 2],
    /// The two Currencies windows' row labels, read from GetText **table 12** — the SAME catalog
    /// as [`Self::roomlist_prompt`] above. Index-aligned with
    /// [`crate::hud::currency_law::CurrencyMode::rows`] (99/97 descriptors; each slot's label id
    /// is the low half of the client descriptor dword, draw code `.text 0x229FA7`; vault §59
    /// addendum 27). Empty until load. Bead kuluu-o9qt7.35.
    pub currency_labels_1: Vec<Option<String>>,
    pub currency_labels_2: Vec<Option<String>>,
}

impl ConfigDatRows {
    pub fn window_frame(&self, name: &str) -> Option<(f32, f32, f32)> {
        self.frames
            .get(name)
            .map(|&(x, y, w)| (f32::from(x), f32::from(y), f32::from(w.max(1))))
    }

    pub fn record_frame(&self, name: &str) -> Option<RecordFrame> {
        let (x, y, width) = self.window_frame(name)?;
        let extent = self.frame_extents.get(name);
        Some(RecordFrame {
            x,
            y,
            width,
            height: extent.map(|&(height, _)| f32::from(height)),
            anchor: extent.map_or(FRAME_ANCHOR_NONE, |&(_, anchor)| anchor),
        })
    }
}

/// The menu design space every window record frame is measured in.
pub const MENU_DESIGN_WIDTH: f32 = 512.0;
pub const MENU_DESIGN_HEIGHT: f32 = 448.0;

/// Record frame anchor bits: right edge adds `screen width - 512` to x, bottom edge adds
/// `screen height - 448` to y (.agents/skills/retail-observe/references/2026-10-15-menu-window-stacking.md).
pub const FRAME_ANCHOR_NONE: u8 = 0;
pub const FRAME_ANCHOR_RIGHT_EDGE: u8 = 1;
pub const FRAME_ANCHOR_BOTTOM_EDGE: u8 = 2;

/// One DAT window record's frame in the menu design space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecordFrame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    /// `None` when only the position and width are known.
    pub height: Option<f32>,
    pub anchor: u8,
}

impl RecordFrame {
    /// A window sitting on an older window's whole rectangle hides it
    /// (.agents/skills/retail-observe/references/2026-10-15-menu-window-stacking.md). Frames
    /// pinned to different screen edges only meet at one screen size, and an unknown height
    /// covers nothing.
    pub fn covers(&self, older: &RecordFrame) -> bool {
        let (Some(height), Some(older_height)) = (self.height, older.height) else {
            return false;
        };
        self.anchor == older.anchor
            && self.x <= older.x
            && self.y <= older.y
            && self.x + self.width >= older.x + older.width
            && self.y + height >= older.y + older_height
    }
}

/// Puts a window node at its record frame. An anchored axis starts at 100% of the screen and
/// a margin of `x - 512` (or `y - 448`) brings it back, which is the record's
/// `x + screen width - 512` without knowing the screen size; kuluu sizes rows by content, so
/// only the width comes from the record.
pub fn place_at_record_frame(node: &mut Node, frame: &RecordFrame) {
    let right_edge = frame.anchor & FRAME_ANCHOR_RIGHT_EDGE != 0;
    let bottom_edge = frame.anchor & FRAME_ANCHOR_BOTTOM_EDGE != 0;
    node.position_type = PositionType::Absolute;
    (node.left, node.margin.left) = if right_edge {
        (Val::Percent(100.0), Val::Px(frame.x - MENU_DESIGN_WIDTH))
    } else {
        (Val::Px(frame.x), Val::ZERO)
    };
    (node.top, node.margin.top) = if bottom_edge {
        (Val::Percent(100.0), Val::Px(frame.y - MENU_DESIGN_HEIGHT))
    } else {
        (Val::Px(frame.y), Val::ZERO)
    };
    node.right = Val::Auto;
    node.bottom = Val::Auto;
    node.width = Val::Px(frame.width);
}

impl ConfigDatRows {
    pub fn ensure_loaded(&mut self, dat: Option<&ffxi_dat::archive::DatRoot>) {
        if self.loaded {
            return;
        }
        let Some(root) = dat else { return };
        let Ok(windows) = ffxi_dat::menu_windows::windows_from_root(root) else {
            return;
        };
        let catalog: Option<ffxi_dat::dmsg::DmsgTable> = (|| {
            let loc = root
                .resolve(ffxi_dat::menu_windows::CAPTION_CATALOG_TABLE_14_FILE_ID)
                .ok()?;
            let bytes = std::fs::read(loc.path_under(root)).ok()?;
            ffxi_dat::dmsg::DmsgTable::parse(&bytes).ok()
        })();
        // The `roomlist` window draws its five rows from GetText **table 12**, not the
        // caption catalogs — same loader law, different DAT record id
        // (`GETTEXT_TABLE_12_CATALOG_FILE_ID`; bead kuluu-o9qt7.13.5).
        let style_catalog: Option<ffxi_dat::dmsg::DmsgTable> = (|| {
            let loc = root
                .resolve(ffxi_dat::menu_windows::GETTEXT_TABLE_12_CATALOG_FILE_ID)
                .ok()?;
            let bytes = std::fs::read(loc.path_under(root)).ok()?;
            ffxi_dat::dmsg::DmsgTable::parse(&bytes).ok()
        })();
        // Rows are ITEM blocks in file order — select ids index them directly
        // (§7) — never the widget list: `conf5win` dispatches 16 items with
        // only 8 widgets (ffxi-dat real_conf_family_rows_are_items_not_widgets;
        // bead kuluu-o9qt7.20.1). Each row's textA names its caption in this
        // same catalog; chrome blocks carry no string and are dropped.
        let mut by_root = std::collections::BTreeMap::new();
        let mut row_ys = std::collections::BTreeMap::new();
        let mut item_ids = std::collections::BTreeMap::new();
        let mut item_counts = std::collections::BTreeMap::new();
        for name in config_window_names() {
            if let Some(window) = windows.get(name) {
                let mut rows: Vec<(u32, String)> = Vec::new();
                let mut ys: Vec<i16> = Vec::new();
                let mut ids: Vec<i16> = Vec::new();
                for item in window.live_items() {
                    let Some(id) = item.text_a_id.or(item.text_b_id) else {
                        continue;
                    };
                    ids.push(item.id);
                    let label = usize::try_from(id)
                        .ok()
                        .and_then(|i| catalog.as_ref().and_then(|c| c.text(i).map(str::to_string)));
                    rows.push((
                        id as u32,
                        label.unwrap_or_else(|| format!("item text {id}")),
                    ));
                    ys.push(item.y);
                }
                if rows.is_empty() {
                    // The filter-list windows hold their row table in code — the DAT record behind
                    // them has fourteen slots and not one string (§59 addendum 12) — while every
                    // label stays catalog-11 text read here, exactly as the window's drawer asks
                    // GetText for it (.text FFXiMain.dll retail-2026-09 RVA 0x1983B0; bead
                    // kuluu-o9qt7.20.1.3).
                    if let Some(spec) = filter_list_rows(&window.root) {
                        for row in spec {
                            let label = usize::try_from(row.caption_id).ok().and_then(|i| {
                                catalog.as_ref().and_then(|c| c.text(i).map(str::to_string))
                            });
                            rows.push((
                                row.caption_id,
                                label
                                    .unwrap_or_else(|| format!("filter option {}", row.caption_id)),
                            ));
                        }
                    } else if let Some(spec) = cfilter_descriptor_rows(&window.root) {
                        // …and `cfilter`’s fifty-five descriptor rows the same way, captions still
                        // read from catalog table 11 per row (.text FFXiMain.dll retail-2026-09
                        // RVA 0x152EF0 wrapper the drawer calls; bead kuluu-o9qt7.20.1.3).
                        for row in spec {
                            let label = usize::try_from(row.caption_id).ok().and_then(|i| {
                                catalog.as_ref().and_then(|c| c.text(i).map(str::to_string))
                            });
                            rows.push((
                                row.caption_id,
                                label
                                    .unwrap_or_else(|| format!("filter option {}", row.caption_id)),
                            ));
                        }
                    } else if window.root == ROOMLIST_DAT_WINDOW {
                        // `roomlist`’s five slots hold no string at all in the DAT record —
                        // retail fills them from GetText table 12 (.text FFXiMain.dll
                        // retail-2026-09 0x1D6900 fetches ids 450/446..449 into pool slots in
                        // this order; bead kuluu-o9qt7.13.5), so the labels load from that
                        // catalog here and nowhere else.
                        for &caption_id in ROOMLIST_ROW_CAPTION_IDS.iter() {
                            let label = usize::try_from(caption_id).ok().and_then(|i| {
                                style_catalog
                                    .as_ref()
                                    .and_then(|c| c.text(i).map(str::to_string))
                            });
                            rows.push((
                                caption_id,
                                label.unwrap_or_else(|| format!("room style {caption_id}")),
                            ));
                        }
                    }
                }
                by_root.insert(window.root.clone(), rows);
                row_ys.insert(window.root.clone(), ys);
                item_ids.insert(window.root.clone(), ids);
                item_counts.insert(window.root.clone(), window.item_count);
            }
        }
        self.by_root = by_root;
        self.row_ys = row_ys;
        self.item_ids = item_ids;
        self.item_counts = item_counts;
        let mut frames = std::collections::BTreeMap::new();
        let mut frame_extents = std::collections::BTreeMap::new();
        for (name, window) in &windows {
            if let Some(frame) = &window.frame {
                frames.insert(name.clone(), (frame.x, frame.y, frame.width));
                frame_extents.insert(name.clone(), (frame.height, frame.anchor));
            }
        }
        self.frames = frames;
        self.frame_extents = frame_extents;
        // …and the door row's two captions, which live in this same catalog (id pair above).
        let mut captions_pair: [Option<String>; 2] = [None, None];
        for (slot, id) in [
            (
                0usize,
                ffxi_dat::menu_windows::MOGHOUSE_DOOR_CAPTION_CLOSED_ID,
            ),
            (
                1usize,
                ffxi_dat::menu_windows::MOGHOUSE_DOOR_CAPTION_OPEN_ID,
            ),
        ] {
            captions_pair[slot] = catalog
                .as_ref()
                .and_then(|c| c.text(id).map(str::to_string));
        }
        self.home_door_captions = captions_pair;
        self.roomlist_prompt = usize::try_from(ROOMLIST_CONFIRM_PROMPT_ID)
            .ok()
            .and_then(|i| {
                style_catalog
                    .as_ref()
                    .and_then(|c| c.text(i).map(str::to_string))
            });
        // …and the `abisortw`/emote family's strings, all four from this same table-0x0E
        // catalog the caption pass reads (FFXiMain.dll retail-2026-09 `.text 0x223E10`).
        let mut abisortw_captions: [Option<String>; 3] = [None, None, None];
        for (mode, id) in [
            (1usize, ABISORTW_CAPTION_MODE_EMOTE_ID),
            (2usize, ABISORTW_CAPTION_MODE_MOUNT_ID),
        ] {
            abisortw_captions[mode] = catalog
                .as_ref()
                .and_then(|c| c.text(id).map(str::to_string));
        }
        self.abisortw_captions = abisortw_captions;
        let mut emote_sort_prompts: [Option<String>; 2] = [None, None];
        for (slot, id) in [
            (0usize, EMOTE_SORT_PROMPT_FIRST_ID),
            (1usize, EMOTE_SORT_PROMPT_SECOND_ID),
        ] {
            emote_sort_prompts[slot] = catalog
                .as_ref()
                .and_then(|c| c.text(id).map(str::to_string));
        }
        self.emote_sort_prompts = emote_sort_prompts;
        // The Currencies lists' labels come from this same table-12 catalog, one string per
        // descriptor slot in display order (bead kuluu-o9qt7.35; the ids themselves are client
        // `.rdata` law — `currency_law::MODE1_ROWS`/`MODE2_ROWS`).
        let load_currency_labels =
            |mode: crate::hud::currency_law::CurrencyMode| -> Vec<Option<String>> {
                mode.rows()
                    .iter()
                    .map(|&(_id, label_id)| {
                        style_catalog
                            .as_ref()
                            .and_then(|c| c.text(usize::from(label_id)).map(str::to_string))
                    })
                    .collect()
            };
        self.currency_labels_1 = load_currency_labels(crate::hud::currency_law::CurrencyMode::One);
        self.currency_labels_2 = load_currency_labels(crate::hud::currency_law::CurrencyMode::Two);
        self.loaded = true;
    }

    fn rows_for(&self, window: &str) -> &[(u32, String)] {
        self.by_root.get(window).map_or(&[], Vec::as_slice)
    }

    /// Which drawn row a window's last DAT item is; `None` until the records load.
    pub fn last_item_row(&self, window: &str) -> Option<LastItemRow> {
        if !self.loaded {
            return None;
        }
        let (Some(ids), Some(&count)) = (self.item_ids.get(window), self.item_counts.get(window))
        else {
            return Some(LastItemRow::Undrawn);
        };
        let Some(file_row) = ids.iter().position(|&id| id == i16::from(count)) else {
            return Some(LastItemRow::Undrawn);
        };
        let order = display_order(self.row_ys.get(window), self.rows_for(window).len());
        Some(LastItemRow::Row(
            order
                .iter()
                .position(|&row| row == file_row)
                .unwrap_or(file_row),
        ))
    }
}

/// A window's rows in draw order: by item-block `y`, file order breaking ties; file order when the
/// geometry list does not match the rows.
fn display_order(ys: Option<&Vec<i16>>, len: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    if let Some(ys) = ys.filter(|ys| ys.len() == len) {
        order.sort_by_key(|&index| (ys[index], index));
    }
    order
}

/// The retail selector windows' row captions, read from their DAT records at
/// runtime (bead kuluu-1z9qs): every live item block names its caption through
/// the short-caption catalog id (loader FFXiMain.dll retail-2026-09 RVA
/// 0x11B6B0; catalog registry vault main_menu.md §59).
#[derive(bevy::prelude::Resource, Debug, Default)]
pub struct CommandSelectorCaptions {
    loaded: bool,
    /// `menuwind` followed by `socialme`, in block display order. Populated
    /// only when BOTH selectors yield exactly twelve live captioned rows, so
    /// cursor and page arithmetic can never desync from a partial read.
    labels: Vec<String>,
}

impl CommandSelectorCaptions {
    pub fn ensure_loaded(&mut self, dat: Option<&ffxi_dat::archive::DatRoot>) {
        if self.loaded {
            return;
        }
        let Some(root) = dat else { return };
        let Ok(windows) = ffxi_dat::menu_windows::windows_from_root(root) else {
            return;
        };
        let catalog: Option<ffxi_dat::dmsg::DmsgTable> = (|| {
            let loc = root
                .resolve(ffxi_dat::menu_windows::CAPTION_SHORT_CATALOG_FILE_ID)
                .ok()?;
            let bytes = std::fs::read(loc.path_under(root)).ok()?;
            ffxi_dat::dmsg::DmsgTable::parse(&bytes).ok()
        })();
        let page_labels = |name: &str| -> Option<Vec<String>> {
            let live: Vec<_> = windows
                .get(name)?
                .items_in_display_order()
                .into_iter()
                .filter(|it| it.text_a_id.is_some() || it.text_b_id.is_some())
                .collect();
            if live.len() != 12 {
                return None;
            }
            live.into_iter()
                .map(|it| {
                    let id = usize::try_from(it.text_b_id?).ok()?;
                    catalog.as_ref()?.text(id).map(str::to_string)
                })
                .collect()
        };
        if let (Some(first), Some(second)) = (page_labels("menuwind"), page_labels("socialme")) {
            self.labels = first.into_iter().chain(second).collect();
        }
        // A failed read is not retried per frame; the pinned static copy serves.
        self.loaded = true;
    }
}

fn config_window_names() -> impl Iterator<Item = &'static str> {
    std::iter::once(ROOMLIST_DAT_WINDOW)
        .chain(std::iter::once(RETAIL_CONFIG_HUB_WINDOW))
        .chain(RETAIL_CONFIG_PAGE_WINDOWS)
        .chain(RETAIL_CONFIG_SUB_WINDOWS)
}

/// Every `conf5win` select arm writes an immediate through the store wrapper
/// FFXiMain.dll retail-2026-09 RVA 0x193830 — read from that handler's select
/// jump table .text 0x1994AC of handler RVA 0x199360 (vault main_menu.md §58;
/// engine bead kuluu-o9qt7.20.1). Row position = DAT widget file order =
/// select id − 1 (§43): four radio groups — stores 0xB4 and 0xB5 (values
/// 1/2/0), the eight-row list over store 0x12 (values 0..7) and the pair on
/// store 0xA4.
const CONF5WIN_RADIO_ARMS: [(u32, i32); 16] = [
    (0xB4, 1),
    (0xB4, 2),
    (0xB4, 0),
    (0xB5, 1),
    (0xB5, 2),
    (0xB5, 0),
    (0x12, 0),
    (0x12, 1),
    (0x12, 2),
    (0x12, 3),
    (0x12, 4),
    (0x12, 5),
    (0x12, 6),
    (0x12, 7),
    (0xA4, 0),
    (0xA4, 1),
];

/// A law'd select arm of a Config-family row: an immediate radio write through
/// the primary store wrapper FFXiMain.dll retail-2026-09 RVA 0x193830, the same
/// store reached one bit at a time through helper 0x1938F0, a companion drill, or
/// a slider row that pins into edit mode instead.
///
/// Values are signed dwords: `conf4`'s off arm stores −1 (helper FFXiMain.dll
/// retail-2026-09 RVA 0x19C510 zero branch, push at 0x19C557).
pub enum ConfigRowArm {
    Radio {
        store_id: u32,
        value: i32,
    },
    /// A binary pair whose arm calls the bit helper FFXiMain.dll retail-2026-09
    /// RVA 0x1938F0 rather than a plain store write: the immediate the handler
    /// pushes is a **bit index** into option [`CONFIG_OPTION_MISC_FLAGS`]'s flag
    /// dword (`conf12wi` rows 7/8 → bit 7 at .text 0x19BA8E, rows 13/14 → bit 6 at
    /// 0x19BADA; `conf3win` rows 12/13 → bit 2), and each row asks for one bit
    /// state. vault main_menu.md §59 addendum 5 corrects the earlier "bank-2 store
    /// id" reading from addendum 2.
    RadioBit {
        bit: u8,
        value: i32,
    },
    /// One row of a filter-list window: flips its own descriptor bit of the page's persisted
    /// option dword (`fxfilter`, see [`FILTER_LIST_FXFILTER_ARMS`]).
    FilterBit {
        store_id: u32,
        bit: u8,
    },
    Slider {
        store_id: u32,
    },
    /// A law'd name-open row: select drills a Config sub-window through menu
    /// manager FFXiMain.dll retail-2026-09 RVA 0x15E350 (the arms push the
    /// padded-name immediates, e.g. conf6win ids 8/9 open `menu    k1assign`/
    /// `k2assign`, vault main_menu.md §59 addendum 3).
    Sub {
        sub: u8,
    },
    /// A companion drill: the arm first writes a mode dword into the companion
    /// objects it is about to open, then name-opens every window of `opens` in
    /// order (`conftxtc` ids 1–4 = handler FFXiMain.dll retail-2026-09 RVA
    /// 0x1984F0 writing `[textcol1]+0x14`/`[textcol2]+0x1C`/`[textcol3]+0x18`
    /// then opening both; `conf11m` ids 1/2 = RVA 0x19A0E0 writing
    /// `[conf11l]+0x14`; `conf11l` ids 1–4 = RVA 0x19A160 driving `conf11s`
    /// +0x34/+0x38; vault main_menu.md §59 addendum 5). kuluu pushes the LAST
    /// opened window — the one retail leaves focused — and keeps the mode for
    /// it: retail's `textcol1` cancel arm closes both companions together
    /// (.text 0x19888E/0x19889D through manager close-by-name RVA 0x15E910), so
    /// one kuluu stack level pops the pair as a unit.
    Companion {
        mode: i32,
        opens: &'static [u8],
    },
    /// A confirm-dialog row: select launches a Yes/No prompt (retail through
    /// the generic dialog service object [0x10664520], FFXiMain.dll retail-2026-09 RVA 0x210E30) and answers later — on Yes the callback factory-resets both key-assignment option ranges (see [`key_assign_reset_defaults`]); No is inert (`conf6win` select id 10, arm .text FFXiMain.dll retail-2026-09 RVA 0x19B309; vault main_menu.md §59 addenda 7–8).
    ConfirmResetKeyAssignments,
    /// A pair row whose choice never leaves the window object: the arm writes
    /// `value` into the window's field at `obj_off` (and its dirty byte, which only
    /// drives retail's label refresh) — no option store, no wire. The pair marks
    /// from [`DynamicMenu::config_window_choices`]; a freshly booted window holds −1
    /// so neither row starts marked (`conf3win` ids 3/4 → `[obj+0x38]`, arms .text
    /// FFXiMain.dll retail-2026-09 RVA 0x19aa85/0x19aa92; boot −1 at .text
    /// 0x19a8b8; vault main_menu.md §59 addendum 9).
    WindowLocal {
        obj_off: u16,
        value: i32,
    },
    /// A row of the `cfilter` message-filter window, keyed by its raw index into class B's
    /// code-side descriptor table [`CFILTER_ROWS`] — the same index retail's shared select method
    /// (.text FFXiMain.dll retail-2026-09 RVA 0x198080) receives from the event dispatcher and
    /// indexes the table with. Marks ride the drawer state method .text 0x198240
    /// ([`cfilter_row_state`]); select runs [`DynamicMenuAction::ConfigCfilterSelect`] (vault
    /// main_menu.md §59 addendum 15; bead kuluu-o9qt7.20.1.3).
    CfilterSelect {
        table_idx: u8,
    },
}

/// `conf13wi`'s six select arms — jump table .text 0x19be98 (guard `dec;
/// cmp eax,5`) of handler FFXiMain.dll retail-2026-09 RVA 0x19BDD0: rows 1–3
/// are the triple on store 0xC9 (values 0/1/2), rows 4–6 the triple on store
/// 0xCE (values 1/2/0). All immediate wrapper writes; no sliders, no gates.
/// (vault main_menu.md §59 addendum 2; bead kuluu-o9qt7.20.1)
pub const CONF13WI_RADIO_ARMS: &[ConfigRowArm] = &[
    ConfigRowArm::Radio {
        store_id: 0xC9,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: 0xC9,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: 0xC9,
        value: 2,
    },
    ConfigRowArm::Radio {
        store_id: 0xCE,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: 0xCE,
        value: 2,
    },
    ConfigRowArm::Radio {
        store_id: 0xCE,
        value: 0,
    },
];

/// `conf12wi`'s fourteen rows. Its switch maps select id → shared arm through
/// a byte sub-table at .text 0x19BB20 (`xor edx,edx; mov dl,[ecx+...]`) over
/// the jump table .text 0x19bb04 of handler FFXiMain.dll retail-2026-09 RVA
/// 0x19B9C0; each shared arm computes its value from the row's own select id
/// (xor+cmp+sete/setne), so every pair is a binary option whose on/off order
/// varies per row — captured here as the resolved per-row values. Rows 7/8 and
/// 13/14 drive the bit helper 0x1938F0 on option 0x99 - **bit 7** (.text
/// 0x19BA8E) and **bit 6** (0x19BADA); addendum 5 corrected these from "stores".
/// Rows 3/4's arm additionally reads its old value (wrapper 0x193850 at .text
/// 0x19BA1D) and hands the transition to helper 0x1A8030 with one of three
/// modes — off is `helper(0)` (.text 0x19BA5D), writing 1 over stored-1 is
/// `helper(1)` (.text 0x19BA4F), and writing 1 over anything else is
/// `helper(4)` (.text 0x19BA43); the helper's own law lives in
/// [`conf_bf_packet_mode`]/[`conf_bf_announce_allowed`] (vault main_menu.md
/// §59 addendum 16, closing addendum 2's open note).
pub const CONF12WI_RADIO_ARMS: &[ConfigRowArm] = &[
    ConfigRowArm::Radio {
        store_id: 0xBE,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: 0xBE,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: 0xBF,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: 0xBF,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: 0xC0,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: 0xC0,
        value: 0,
    },
    ConfigRowArm::RadioBit { bit: 7, value: 0 },
    ConfigRowArm::RadioBit { bit: 7, value: 1 },
    ConfigRowArm::Radio {
        store_id: 0xC2,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: 0xC2,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: 0xC3,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: 0xC3,
        value: 0,
    },
    ConfigRowArm::RadioBit { bit: 6, value: 0 },
    ConfigRowArm::RadioBit { bit: 6, value: 1 },
];

/// The store dword driven by `conf12wi` rows 3/4. Their shared arm reads the
/// old value through FFXiMain.dll retail-2026-09 RVA 0x193850 (.text 0x19BA1D),
/// writes `(select id == 4)` as the new value through wrapper 0x193830 (.text
/// 0x19BA2A, polarity `cmp ax,4; sete dl` at .text 0x19BA0E/0x19BA18), and then
/// hands the transition to helper 0x1A8030 ([`conf_bf_packet_mode`] picks that
/// mode from the two values). The helper re-persists literal store 0xBF on its
/// success tail — `mode 0 → store←0` (wrapper call at .text FFXiMain.dll
/// retail-2026-09 RVA 0x1A8094) and `mode ∈ {1,3,4} → store←1` (.text 0x1A80B8)
/// — always the value the primary write already applied; neither the helper's
/// Monstrosity bail (.text 0x1A8036, predicate 0x97D50) nor its suppressed
/// branch rolls that write back (vault main_menu.md §59 addendum 16; bead
/// kuluu-o9qt7.20.1.4).
pub const CONFIG_OPTION_CONF_BF_PAIR: u32 = 0xBF;

/// The announce-mode law of `conf12wi` rows 3/4 (arm .text FFXiMain.dll
/// retail-2026-09 RVA 0x19BA32–0x19BA5D): writing 0 announces
/// [`CONF_BF_MODE_DISABLE`] whatever was stored; writing 1 over a store that
/// read back exactly 1 reasserts ([`CONF_BF_MODE_KEEP_ON`]); writing 1 over any
/// other stored value (including no known value) enables
/// ([`CONF_BF_MODE_ENABLE`]).
#[must_use]
pub const fn conf_bf_packet_mode(new_value: i32, old_stored: Option<i32>) -> u8 {
    if new_value == 0 {
        CONF_BF_MODE_DISABLE
    } else if matches!(old_stored, Some(1)) {
        CONF_BF_MODE_KEEP_ON
    } else {
        CONF_BF_MODE_ENABLE
    }
}

/// Mode bytes as the arm picks them at .text FFXiMain.dll retail-2026-09 RVA
/// 0x19BA32 (`cmp esi,1` on the written value → `helper(0)` at .text 0x19BA5D;
/// writing 1 branches on the old store through `cmp edi,1` — kept-on is
/// `helper(1)` at .text 0x19BA4F, enabling is `helper(4)` at .text 0x19BA43).
pub const CONF_BF_MODE_DISABLE: u8 = 0;
/// Reasserting an already-on pair announces mode 1.
pub const CONF_BF_MODE_KEEP_ON: u8 = 1;
/// Turning the pair on from a non-1 stored value announces mode 4.
pub const CONF_BF_MODE_ENABLE: u8 = 4;

/// Milliseconds the announce must stay silent after its previous send: guard
/// FFXiMain.dll retail-2026-09 RVA 0x1A7D05 subtracts the stamp global from the
/// client clock and passes only when `cmp eax, 0x2328; seta` — strictly greater.
pub const CONF_BF_ANNOUNCE_WINDOW_MS: u64 = 0x2328;

/// The suppress-or-send gate helper FFXiMain.dll retail-2026-09 RVA 0x1A804F
/// runs through guard 0x1A7D00 before touching the wire, given the milliseconds
/// since the last stamped announce ([`DynamicMenu::conf_bf_stamp`] — `None`
/// passes, mirroring retail's zero-initialised stamp global so a fresh client
/// always sends). On pass the helper stamps the clock (global store at .text
/// 0x1A806E); on suppression it skips the packet and pokes sub-window id 0x22
/// through the drawer singleton [0x10578158] (.text FFXiMain.dll retail-2026-09
/// RVA 0x1A8058–0x1A8068) instead — feedback chrome with no kuluu counterpart,
/// so the announce is simply dropped.
#[must_use]
pub fn conf_bf_announce_allowed(elapsed_ms: Option<u64>) -> bool {
    elapsed_ms.is_none_or(|ms| ms > CONF_BF_ANNOUNCE_WINDOW_MS)
}

/// `conf3win`'s fifteen arms - jump table .text 0x19ac30 (guard `dec; cmp
/// eax,0xe`) of handler FFXiMain.dll retail-2026-09 RVA 0x19AA10: rows 1/2/5 the
/// triple on store 0x13 (values ebx=0 from the prologue's `xor ebx,ebx`, 1, 2);
/// rows 3/4 are the window-local pair on `[obj+0x38]` (+ dirty byte `[obj+0x3c]`,
/// which only drives retail's label refresh — vault main_menu.md §59 addendum 9);
/// rows 6/7/8 store 0x3A {2, ebx=0, 1}; rows 9/10 store 0x3B {1, ebx=0}; row 11
/// pins a slider (widget [esi+0x48], edit byte [esi+0x20]) whose store lives in
/// the pin region from .text 0x19AB94 - open; rows 12/13 are the bit pair on
/// option 0x99 (**bit 2**, values 1/ebx=0); rows 14/15 store 0xA5 {ebx=0, 1} (vault
/// main_menu.md §59 addendum 3; bead kuluu-o9qt7.20.1).
const CONF3WIN_ROW_ARMS: [ConfigRowArm; 15] = [
    ConfigRowArm::Radio {
        store_id: 0x13,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: 0x13,
        value: 1,
    },
    // Ids 3/4 (index 2/3): the arms at .text FFXiMain.dll retail-2026-09 RVA
    // 0x19aa85/0x19aa92 write `[esi+0x38]`=choice and set the dirty byte — no store
    // wrapper call, no wire (vault main_menu.md §59 addendum 9).
    ConfigRowArm::WindowLocal {
        obj_off: 0x38,
        value: 0,
    },
    ConfigRowArm::WindowLocal {
        obj_off: 0x38,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: 0x13,
        value: 2,
    },
    ConfigRowArm::Radio {
        store_id: 0x3A,
        value: 2,
    },
    ConfigRowArm::Radio {
        store_id: 0x3A,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: 0x3A,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: 0x3B,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: 0x3B,
        value: 0,
    },
    // Row 11 is the slider entry arm (.text FFXiMain.dll retail-2026-09 RVA
    // 0x19AAFB): it sets the edit byte [esi+0x20]=1, stashes the widget value slot
    // [esi+0x48] into [esi+0x24] and clears the window's dirty bytes — nothing is
    // written until a step lands in store 0x3C.
    ConfigRowArm::Slider { store_id: 0x3C },
    ConfigRowArm::RadioBit { bit: 2, value: 1 },
    ConfigRowArm::RadioBit { bit: 2, value: 0 },
    ConfigRowArm::Radio {
        store_id: 0xA5,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: 0xA5,
        value: 1,
    },
];

/// `conf6win`'s fifteen arms - jump table .text 0x19b534 (guard `dec; cmp
/// eax,0xe`) of handler FFXiMain.dll retail-2026-09 RVA 0x19B200: rows 1/4/7 are
/// slider pins on stores 0x3E/0x40/0x43 (edit bytes [esi+0x20]/[esi+0x21]/
/// [esi+0x22]; increment guards `cmp eax,0x19` / `cmp eax,3` / `cmp eax,5`); rows
/// 2/3 the pair on store 0x3F {ebx=0, 1}; rows 5/6/11 the triple on store 0x42
/// {ebx=0, 1, 2}; row 8 name-opens `menu    k1assign` and row 9 `menu    k2assign`
/// through menu manager FFXiMain.dll retail-2026-09 RVA 0x15E350 (padded strings
/// at file-off 0x4C4BC/0x4C4E8); row 10 is a GetText+dialog arm; rows 12/13 store
/// 0x9D {ebx=0, 1}; rows 14/15 store 0x9E {ebx=0, 1} (vault main_menu.md §59
/// addendum 3; bead kuluu-o9qt7.20.1).
const CONF6WIN_ROW_ARMS: [ConfigRowArm; 15] = [
    ConfigRowArm::Slider { store_id: 0x3E },
    ConfigRowArm::Radio {
        store_id: 0x3F,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: 0x3F,
        value: 1,
    },
    ConfigRowArm::Slider { store_id: 0x40 },
    ConfigRowArm::Radio {
        store_id: 0x42,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: 0x42,
        value: 1,
    },
    ConfigRowArm::Slider { store_id: 0x43 },
    // Ids 8/9 are the name-opens through menu manager FFXiMain.dll
    // retail-2026-09 RVA 0x15E350 (padded immediates `menu    k1assign`/
    // `menu    k2assign`); id10 is the confirm-dialog arm (.text 0x19B309 —
    // GetText(table 0xb, string id 0x8F) into buffer [0x106626D8], then the
    // dialog service; vault main_menu.md §59 addenda 7–8).
    ConfigRowArm::Sub { sub: 3 },
    ConfigRowArm::Sub { sub: 4 },
    ConfigRowArm::ConfirmResetKeyAssignments,
    ConfigRowArm::Radio {
        store_id: 0x42,
        value: 2,
    },
    ConfigRowArm::Radio {
        store_id: 0x9D,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: 0x9D,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: 0x9E,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: 0x9E,
        value: 1,
    },
];

/// `conf4`'s nine select arms — jump table .text 0x19c378 (guard
/// `movsx eax,si; lea ecx,[eax−1]; cmp ecx,8`, ids 1–9) of handler FFXiMain.dll
/// retail-2026-09 RVA 0x19C300: rows 1/2 the pair on store 0x18 (values 0/1);
/// rows 3–8 share the tail at .text 0x19C345 computing V = 600·(id−2) and
/// calling helper FFXiMain.dll retail-2026-09 RVA 0x19C510, whose signed
/// magic-divide (mulhi by 0x88888889, + V, sar 5) writes store **0x41** =
/// 10·(id−2) — {10,20,…,60} — before applying it at 0x311391; row 9 calls the
/// same helper with 0 and its zero branch stores −1 (push at .text 0x19C557).
/// Each level arm also stashes the raw select id at [this+0x22] (vault
/// main_menu.md §59 addendum 3; bead kuluu-o9qt7.20.1).
pub const CONF4_ROW_ARMS: [ConfigRowArm; 9] = [
    ConfigRowArm::Radio {
        store_id: 0x18,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: 0x18,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: 0x41,
        value: 10,
    },
    ConfigRowArm::Radio {
        store_id: 0x41,
        value: 20,
    },
    ConfigRowArm::Radio {
        store_id: 0x41,
        value: 30,
    },
    ConfigRowArm::Radio {
        store_id: 0x41,
        value: 40,
    },
    ConfigRowArm::Radio {
        store_id: 0x41,
        value: 50,
    },
    ConfigRowArm::Radio {
        store_id: 0x41,
        value: 60,
    },
    ConfigRowArm::Radio {
        store_id: 0x41,
        value: -1,
    },
];

/// `conf7`'s fourteen select arms — handler FFXiMain.dll retail-2026-09 RVA
/// 0x19C130 carries no jump table: its guard (`cmp ax,1 / cmp ax,0xf jge`, ids
/// 1–14) indexes the stride-8 (store_id, value) pair table at .data VA
/// 0x1037A0B0 directly — seven radio pairs on stores 0x90, 0x8F, 0x91, 0x8C,
/// 0x8B, 0x8E and 0x8D (each {0,1}); after each write the handler re-reads the
/// same store through wrapper FFXiMain.dll retail-2026-09 RVA 0x193850 and
/// mirrors its on/off state into a per-pair window byte at
/// [this+0x18+(id−1)/2] (vault main_menu.md §59 addendum 3; bead
/// kuluu-o9qt7.20.1).
pub const CONF7_ROW_ARMS: [ConfigRowArm; 14] = [
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_POINTER_REFERENCE,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_POINTER_REFERENCE,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_EDGE_ROTATE,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_EDGE_ROTATE,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_CAMERA_BEHIND_LOCK,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_CAMERA_BEHIND_LOCK,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_3P_INVERT_Y,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_3P_INVERT_Y,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_3P_INVERT_X,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_3P_INVERT_X,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_FP_INVERT_Y,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_FP_INVERT_Y,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_FP_INVERT_X,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_FP_INVERT_X,
        value: 1,
    },
];

/// The law'd select-arm table of a Config page window, by position. Pages
/// without an entry keep the §46 option-record fallback.
/// The Text Color launcher's five select arms — handler FFXiMain.dll retail-2026-09
/// RVA 0x1984F0 answers event id 5 only (jump table .text 0x198604, guard
/// `dec; cmp eax,4`): rows 1–4 write their own index (id−1) into `[textcol1]+0x14`,
/// `[textcol2]+0x1C` and `[textcol3]+0x18` (.text 0x198512/0x19853A/0x198562/0x198584),
/// then name-open `menu    textcol2` (.text 0x1985A8) followed by `menu    textcol1`
/// (0x1985BB). Row 5 opens `menu    confyn  ` (0x1985D1) and zeroes `[confyn]+0x14`;
/// when the panel already holds a window it also runs cursor helper
/// FFXiMain.dll retail-2026-09 RVA 0x118E90(2) (.text 0x1985F5). kuluu encodes the row-5
/// helper as part of the same drill — it only moves that panel's cursor, never a value.
/// (vault main_menu.md §59 addendum 5; bead kuluu-o9qt7.20.1)
pub const CONFTEXT_ROW_ARMS: [ConfigRowArm; 5] = [
    ConfigRowArm::Companion {
        mode: 0,
        opens: CONFTXT_TEXT_OPENS,
    },
    ConfigRowArm::Companion {
        mode: 1,
        opens: CONFTXT_TEXT_OPENS,
    },
    ConfigRowArm::Companion {
        mode: 2,
        opens: CONFTXT_TEXT_OPENS,
    },
    ConfigRowArm::Companion {
        mode: 3,
        opens: CONFTXT_TEXT_OPENS,
    },
    ConfigRowArm::Companion {
        mode: 0,
        opens: CONFIG_CONFYN_OPENS,
    },
];

/// `conf11m`'s three arms — handler FFXiMain.dll retail-2026-09 RVA 0x19A0E0 answers
/// event id 5 only, a dec-chain over ids 1–3: rows 1/2 write `[conf11l]+0x14` =
/// 0/1 (.text 0x19A13B/0x19A12C) and name-open `menu    conf11l ` (0x19A150); row 3
/// opens `menu    confyn  ` (0x19A0FA) with `[confyn]+0x14` = 1 plus the same helper
/// FFXiMain.dll retail-2026-09 RVA 0x118E90(2) when its window exists.
pub const CONF11M_ROW_ARMS: [ConfigRowArm; 3] = [
    ConfigRowArm::Companion {
        mode: 0,
        opens: CONFIG_CONF11L_OPENS,
    },
    ConfigRowArm::Companion {
        mode: 1,
        opens: CONFIG_CONF11L_OPENS,
    },
    ConfigRowArm::Companion {
        mode: 1,
        opens: CONFIG_CONFYN_OPENS,
    },
];

/// `conf11l`'s four arms — handler FFXiMain.dll retail-2026-09 RVA 0x19A160 answers
/// event id 5, guard `dec; cmp eax,3`: every arm copies this window's own mode word
/// `[this+0x14]` into `[conf11s]+0x34`, writes its select id−1 to `[conf11s]+0x38`
/// and name-opens `menu    conf11s `, so the row's id is the mode kuluu carries.
pub const CONF11L_ROW_ARMS: [ConfigRowArm; 4] = [
    ConfigRowArm::Companion {
        mode: 0,
        opens: CONFIG_CONF11S_OPENS,
    },
    ConfigRowArm::Companion {
        mode: 1,
        opens: CONFIG_CONF11S_OPENS,
    },
    ConfigRowArm::Companion {
        mode: 2,
        opens: CONFIG_CONF11S_OPENS,
    },
    ConfigRowArm::Companion {
        mode: 3,
        opens: CONFIG_CONF11S_OPENS,
    },
];

/// Windows each companion drill name-opens, in the handler's manager-open order
/// (`conftxtc` .text 0x1985A8 then 0x1985BB; the rest are single-window arms).
pub const CONFTXT_TEXT_OPENS: &[u8] = &[CONFIG_SUB_TEXTCOL2, CONFIG_SUB_TEXTCOL1];
/// `confyn` is one window — the Yes/No panel answers event id 5 with select id 1
/// only and maps its own `[this+0x14]` {0→write 1, 1→write 2} through wrapper
/// FFXiMain.dll retail-2026-09 RVA 0x193870 before closing itself (handler
/// RVA 0x199170; vault main_menu.md §59 addendum 5).
pub const CONFIG_CONFYN_OPENS: &[u8] = &[CONFIG_SUB_CONFYN];
/// `conf11l`, drilled by `conf11m` rows 1/2.
pub const CONFIG_CONF11L_OPENS: &[u8] = &[CONFIG_SUB_CONF11L];
/// `conf11s`, drilled by every `conf11l` row.
pub const CONFIG_CONF11S_OPENS: &[u8] = &[CONFIG_SUB_CONF11S];

pub fn config_page_arms(window: &str) -> Option<&'static [ConfigRowArm]> {
    match window {
        "conftxtc" => Some(&CONFTEXT_ROW_ARMS),
        "conf11m" => Some(&CONF11M_ROW_ARMS),
        "conf11l" => Some(&CONF11L_ROW_ARMS),
        "conf2win" => Some(&CONF2WIN_ROW_ARMS),
        "conf3win" => Some(&CONF3WIN_ROW_ARMS),
        "conf6win" => Some(&CONF6WIN_ROW_ARMS),
        "conf4" => Some(&CONF4_ROW_ARMS),
        "conf5w1" => Some(&LOG_WINDOW_ONE_ROW_ARMS),
        "conf5w2" => Some(&LOG_WINDOW_TWO_ROW_ARMS),
        "conf7" => Some(&CONF7_ROW_ARMS),
        "conf13wi" => Some(CONF13WI_RADIO_ARMS),
        "conf12wi" => Some(CONF12WI_RADIO_ARMS),
        "fxfilter" => Some(&FILTER_LIST_FXFILTER_ARMS),
        "cfilter" => Some(&FILTER_LIST_CFILTER_ARMS),
        _ => None,
    }
}

/// `fxfilter`'s select arms — one per descriptor row of [`FXFILTER_ROWS`]: the bit flips in option
/// [`CONFIG_OPTION_FILTER_CONSOLE_FLAGS`] (mark law .text FFXiMain.dll retail-2026-09 RVA 0x193960
/// over `[obj+0x30]`, drawer .text 0x1983B0; vault main_menu.md §59 addendum 14).
pub const FILTER_LIST_FXFILTER_ARMS: [ConfigRowArm; 18] = [
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 0,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 1,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 2,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 3,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 4,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 5,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 6,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 7,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 8,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 9,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 10,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 11,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 12,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 13,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 14,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 15,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 16,
    },
    ConfigRowArm::FilterBit {
        store_id: CONFIG_OPTION_FILTER_CONSOLE_FLAGS,
        bit: 17,
    },
];

/// The Gameplay page's sound-effect volume slider store — row id 3, whose DAT help caption
/// reads "Set the volume of sound effects." (`conf2win` item block 3 in the `con8` record).
/// Row order equals arm order, so this is arm index 2 (jump table .text FFXiMain.dll
/// retail-2026-09 RVA 0x197848); pinned install-gated by
/// `conf2win_volume_slider_rows_pair_with_their_dat_help_captions`.
pub const CONFIG_STORE_SFX_VOLUME: u32 = 9;

/// The Gameplay page's background-music volume slider store — row id 4, DAT help caption
/// "Set the volume of the background music." (arm index 3).
pub const CONFIG_STORE_BGM_VOLUME: u32 = 0xA;

/// The Gameplay page's battle start-lock pair store (`conf2win` rows id 1/2, DAT help "Automatically
/// lock on to target at the start of battle." / "Do not automatically lock on to target."): arm
/// values are **0 = on** (id 1 arm pushes 0 at .text FFXiMain.dll retail-2026-09 RVA 0x1974D2) and
/// **1 = off** (.text 0x1974E4). A strict argument-position sweep of the store accessor family finds
/// no reader in the client — the rows are write-only latches, so consumers honour the captions: a
/// non-zero value disables engage-time target locking. The §58 option-table registration
/// (descriptor call FFXiMain.dll retail-2026-09 RVA 0x195460 reached from .text 0x193CD7) boots the
/// store at 0, i.e. locked on.
pub const CONFIG_STORE_AUTO_LOCK: u32 = 7;

/// The Gameplay page's auto-sort pair store (`conf2win` rows id 8/9, DAT help "Automatically sort
/// items as they enter your inventory." / "Disable the autosort function, and opt to manually sort
/// your inventory."): arm values are **1 = on** and **0 = off**.
pub const CONFIG_STORE_AUTO_SORT: u32 = 0x9A;

/// The Gameplay page's sort-key pair store (rows id 10/11, DAT help "Sort items by category when
/// organizing them." / "Sort items by name when organizing them and stacking them when
/// possible."): arm values are **0 = category** and **1 = name**.
pub const CONFIG_STORE_SORT_KEY: u32 = 0xCA;

/// The Mouse & Camera page's third-person axis-inversion stores. Each is a pair whose first row
/// reads "Reset … camera controls to the default setting." (value 0) and second row names the axis
/// it inverts (value 1): `conf7` rows id 9/10 are the X pair (**0x8B**) and id 7/8 the Y pair
/// (**0x8C**). Retail applies the flag by negating that aim delta outright: getter .text
/// FFXiMain.dll retail-2026-09 RVA 0x1EF6C/0x1EF8B, and the `fmul` with −1.0f (.rdata RVA 0x32A3F0)
/// at .text 0x1EF7C/0x1EF9B in the third-person camera update (func .text 0x1E48A).
pub const CONFIG_STORE_3P_INVERT_X: u32 = 0x8B;

/// [`CONFIG_STORE_3P_INVERT_X`]'s Y-axis twin (`conf7` rows id 7/8).
pub const CONFIG_STORE_3P_INVERT_Y: u32 = 0x8C;

/// The first-person pair of the same shape, read by the first-person camera update (func .text
/// FFXiMain.dll retail-2026-09 RVA 0x20F39) and negated through the same −1.0f at .text 0x20F8B: X
/// is store **0x8D** (`conf7` rows id 13/14).
pub const CONFIG_STORE_FP_INVERT_X: u32 = 0x8D;

/// [`CONFIG_STORE_FP_INVERT_X`]'s Y-axis twin (`conf7` rows id 11/12, negated at .text
/// FFXiMain.dll retail-2026-09 RVA 0x20FAA).
pub const CONFIG_STORE_FP_INVERT_Y: u32 = 0x8E;

/// The Mouse & Camera page's point-of-reference pair store (`conf7` rows id 1/2, DAT captions "Use
/// cursor as point of reference for movement." = 0 / "…your character…" = 1). Its consumer law (the
/// cursor-anchor capture vs the character-centre anchor, three handlers .text FFXiMain.dll
/// retail-2026-09 RVA 0x124930..0x124B23) has no kuluu equivalent yet, so nothing reads this store.
pub const CONFIG_STORE_POINTER_REFERENCE: u32 = 0x90;

/// The Mouse & Camera page's screen-edge rotation switch (`conf7` rows id 3/4): **0 = the camera
/// rotates when the pointer is parked at a screen edge** ("The camera will rotate when the mouse
/// pointer is moved to the edge of the screen."), any non-zero value = it does not. The gate reads
/// it at .text FFXiMain.dll retail-2026-09 RVA 0x124CC5 and jumps the whole edge-axis block when
/// non-zero; no boot default was found (the store registers table-driven), so an unwritten store is
/// treated as off here.
pub const CONFIG_STORE_EDGE_ROTATE: u32 = 0x8F;

/// The Mouse & Camera page's third-person freedom pair store (`conf7` rows id 5/6, DAT captions
/// "Allows unrestricted movement of the third-person camera." = 0 / "Locks the third-person camera
/// behind your character." = 1; consumer .text FFXiMain.dll retail-2026-09 RVA 0x1F984). Nothing in
/// kuluu enforces the behind-camera constraint yet, so nothing reads this store.
pub const CONFIG_STORE_CAMERA_BEHIND_LOCK: u32 = 0x91;

/// The store family behind an individual chat-log window — the pair of windows
/// `conf5m` ids 2/3 drill ("Adjust individual window settings.", DAT record
/// `conf5m`; vault main_menu.md §59 addendum 46). Both windows run one shared select
/// handler (.text FFXiMain.dll retail-2026-09 RVA 0x199850, vtables .rdata 0x336D70 and
/// .rdata 0x336DC0) over identical DAT row sets; each class maps its own numeric text-slot
/// ids onto its store set through the label table at .rva 0x379ED0, but window one's stores
/// are exactly the handler's literal label immediates while window two's class shifts every
/// write to the second set. `conf5win`'s shared-settings stores (0xB4/0xB5) stay separate.
pub const CONFIG_STORE_LOG_WINDOW_ONE_AUTO_SIZE: u32 = 0xE;
/// [`CONFIG_STORE_LOG_WINDOW_ONE_AUTO_SIZE`]'s max-lines stepper — entry arm seeds the edit
/// scratch from object field +0x3c (.text 0x1998C8), boot value 8.
pub const CONFIG_STORE_LOG_WINDOW_ONE_MAX_LINES: u32 = 0xF;
/// The min-lines stepper (scratch from +0x40, .text 0x1998F0, boot 0).
pub const CONFIG_STORE_LOG_WINDOW_ONE_MIN_LINES: u32 = 0x10;
/// The adjustment-interval stepper — scratch is a raw field at +0x24 shown as raw+3
/// (commit adds the offset at .text 0x1999AE), so the store holds the visible value.
pub const CONFIG_STORE_LOG_WINDOW_ONE_INTERVAL: u32 = 0x11;
/// The column-width stepper (scratch from +0x28, boot 100).
pub const CONFIG_STORE_LOG_WINDOW_ONE_WIDTH: u32 = 0x9C;

/// Window two's twin set — the shared handler's literal labels are translated through the
/// label table before reaching the store wrapper.
pub const CONFIG_STORE_LOG_WINDOW_TWO_AUTO_SIZE: u32 = 0xB6;
/// [`CONFIG_STORE_LOG_WINDOW_ONE_MAX_LINES`]'s window-two counterpart (store 0xB7).
pub const CONFIG_STORE_LOG_WINDOW_TWO_MAX_LINES: u32 = 0xB7;
/// Window two's min-lines stepper (store 0xB8).
pub const CONFIG_STORE_LOG_WINDOW_TWO_MIN_LINES: u32 = 0xB8;
/// Window two's interval stepper (store 0xC6).
pub const CONFIG_STORE_LOG_WINDOW_TWO_INTERVAL: u32 = 0xC6;
/// Window two's column-width stepper (store 0xC5, not 0x9C — the label table shifts it too).
pub const CONFIG_STORE_LOG_WINDOW_TWO_WIDTH: u32 = 0xC5;

/// The six rows of an individual log-window page in DAT file order (= select id − 1): ids
/// 1/2 write the auto-size radio straight through (arms push 0 at .text FFXiMain.dll
/// retail-2026-09 RVA 0x1998AA and 1 at 0x1998BB; **0 = "Automatically adjust log window's
/// size"**, 1 = fixed); ids 3–6 are stepper rows whose entry arms (.text 0x1998C8/.text
/// 0x1998F0/.text 0x199918/.text 0x199940) only set the edit flag and seed the scratch from
/// the field — nothing is written until a step (vault main_menu.md §59 addendum 46).
const LOG_WINDOW_ONE_ROW_ARMS: [ConfigRowArm; 6] = [
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_LOG_WINDOW_ONE_AUTO_SIZE,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_LOG_WINDOW_ONE_AUTO_SIZE,
        value: 1,
    },
    ConfigRowArm::Slider {
        store_id: CONFIG_STORE_LOG_WINDOW_ONE_MAX_LINES,
    },
    ConfigRowArm::Slider {
        store_id: CONFIG_STORE_LOG_WINDOW_ONE_MIN_LINES,
    },
    ConfigRowArm::Slider {
        store_id: CONFIG_STORE_LOG_WINDOW_ONE_WIDTH,
    },
    ConfigRowArm::Slider {
        store_id: CONFIG_STORE_LOG_WINDOW_ONE_INTERVAL,
    },
];

/// [`LOG_WINDOW_ONE_ROW_ARMS`]'s window-two twin (`conf5w2`, stores 0xB6–0xC6).
const LOG_WINDOW_TWO_ROW_ARMS: [ConfigRowArm; 6] = [
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_LOG_WINDOW_TWO_AUTO_SIZE,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_LOG_WINDOW_TWO_AUTO_SIZE,
        value: 1,
    },
    ConfigRowArm::Slider {
        store_id: CONFIG_STORE_LOG_WINDOW_TWO_MAX_LINES,
    },
    ConfigRowArm::Slider {
        store_id: CONFIG_STORE_LOG_WINDOW_TWO_MIN_LINES,
    },
    ConfigRowArm::Slider {
        store_id: CONFIG_STORE_LOG_WINDOW_TWO_WIDTH,
    },
    ConfigRowArm::Slider {
        store_id: CONFIG_STORE_LOG_WINDOW_TWO_INTERVAL,
    },
];

/// `conf2win`'s eleven select arms — jump table .text 0x197848 (guarded
/// `dec; cmp eax,0xa`) of handler FFXiMain.dll retail-2026-09 RVA 0x197470:
/// rows 1–2 the radio pair on store 7 (values 0/1); rows 3/4 slider-edit rows
/// over stores 9 and 0xA (edit bytes [esi+0x20]/[esi+0x21] at entry arms that
/// only stash the widget — no write until a step or save); rows 5–7 set the
/// window mode byte [esi+0x48] to 1/2/3, which pins edit on stores
/// 0x37/0x38/0x39; rows 8/9 and 10/11 are the radio pairs on stores 0x9A
/// (values 1/0) and 0xCA (values 0/1) (vault main_menu.md §59 addendum 2;
/// bead kuluu-o9qt7.20.1 part B).
pub const CONF2WIN_ROW_ARMS: [ConfigRowArm; 11] = [
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_AUTO_LOCK,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_AUTO_LOCK,
        value: 1,
    },
    ConfigRowArm::Slider {
        store_id: CONFIG_STORE_SFX_VOLUME,
    },
    ConfigRowArm::Slider {
        store_id: CONFIG_STORE_BGM_VOLUME,
    },
    ConfigRowArm::Slider { store_id: 0x37 },
    ConfigRowArm::Slider { store_id: 0x38 },
    ConfigRowArm::Slider { store_id: 0x39 },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_AUTO_SORT,
        value: 1,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_AUTO_SORT,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_SORT_KEY,
        value: 0,
    },
    ConfigRowArm::Radio {
        store_id: CONFIG_STORE_SORT_KEY,
        value: 1,
    },
];

/// Ceiling of every law'd config slider — the `cmp ecx,0x64` guard that skips
/// increment at this value (FFXiMain.dll retail-2026-09 RVA 0x19767F/0x197715
/// and their conf-family twins; vault main_menu.md §59 addendum 2).
pub const CONFIG_SLIDER_MAX: u8 = 100;

/// Boot defaults of the slider stores from the §58 client init region
/// (FFXiMain.dll retail-2026-09 RVA call table at 0x193D06/0x193D1B — stores
/// 9/0xA start at their max 100 — and 0x194276/0x19428A/0x19429E — stores
/// 0x37/0x38/0x39 start at half-scale 50; vault main_menu.md §58). A pinned
/// slider edits from the stored value if one exists, else this default.
pub fn config_slider_boot_default(store_id: u32) -> u8 {
    match store_id {
        9 | 0xA => 100,
        0x37..=0x39 => 50,
        // conf6win sliders boot from their §58 init values (FFXiMain.dll
        // retail-2026-09 RVA call sites 0x194301/0x1942ED: store 0x3E at its max
        // 25, store 0x40 at its max 3; store 0x43's init slot takes ebx=0).
        0x3E => 25,
        0x40 => 3,
        // `conf3win`'s slider store boots at its ceiling (option record built in
        // the §58 init region, .text call site FFXiMain.dll retail-2026-09 RVA
        // 0x1942D9: default and max both 0x32 = 50, floor 0x19 = 25).
        0x3C => 50,
        // The individual log-window steppers boot from the shared constructor's field
        // defaults (.text FFXiMain.dll retail-2026-09 RVA 0x199710: interval raw 5 at +0x24 —
        // shown as raw+3, store value 8; width 100 at +0x28; max lines 8 at +0x3c).
        CONFIG_STORE_LOG_WINDOW_ONE_INTERVAL | CONFIG_STORE_LOG_WINDOW_TWO_INTERVAL => 8,
        CONFIG_STORE_LOG_WINDOW_ONE_WIDTH | CONFIG_STORE_LOG_WINDOW_TWO_WIDTH => 100,
        CONFIG_STORE_LOG_WINDOW_ONE_MAX_LINES | CONFIG_STORE_LOG_WINDOW_TWO_MAX_LINES => 8,
        _ => 0,
    }
}

/// Per-store floor of a law'd slider. `conf3win`'s decrement arm only steps while
/// the value is strictly above **0x19** (`cmp eax, 0x19; jle` at FFXiMain.dll
/// retail-2026-09 RVA .text 0x19ABDB), so store 0x3C bottoms out at 25; other
/// sliders may reach 0 (vault main_menu.md §59 addendum 6).
pub fn config_slider_min(store_id: u32) -> u8 {
    match store_id {
        0x3C => 25,
        // The log-window max-lines decrement runs only while the value is above 4
        // (`cmp eax,4; jle` at FFXiMain.dll retail-2026-09 .text 0x199ADC), so its floor is
        // 4. The interval guards sit on the raw field (decrement only while raw > 0 —
        // `test eax,eax; jle` at .text 0x199D0), and the store holds raw+3, giving a store
        // floor of 3.
        CONFIG_STORE_LOG_WINDOW_ONE_MAX_LINES | CONFIG_STORE_LOG_WINDOW_TWO_MAX_LINES => 4,
        CONFIG_STORE_LOG_WINDOW_ONE_INTERVAL | CONFIG_STORE_LOG_WINDOW_TWO_INTERVAL => 3,
        _ => 0,
    }
}

/// Per-store ceiling of a law'd slider: the conf2win-family arms guard increment
/// with `cmp ecx,0x64`; the conf6win pin arms use `cmp eax,0x19` (store 0x3E),
/// `cmp eax,3` (0x40) and `cmp eax,5` (0x43) at FFXiMain.dll retail-2026-09 RVA
/// 0x19B407/0x19B491/0x19B4F8 - the step only runs strictly below the bound.
pub fn config_slider_max(store_id: u32) -> u8 {
    match store_id {
        0x3E => 25,
        0x40 => 3,
        0x43 => 5,
        // `conf3win`'s increment arm only steps strictly below 0x32
        // (`cmp eax, 0x32; jge` at FFXiMain.dll retail-2026-09 RVA .text 0x19ABF7).
        0x3C => 50,
        // Both log-window line-count steppers cap at 20 (`cmp eax,0x14; jge` at the family
        // arms .text FFXiMain.dll retail-2026-09 0x199B27/0x199BAC); the interval increment
        // caps raw below 27 (`cmp eax,0x1b; jge` at .text 0x199DA), so its store ceiling is
        // 30.
        CONFIG_STORE_LOG_WINDOW_ONE_MAX_LINES
        | CONFIG_STORE_LOG_WINDOW_TWO_MAX_LINES
        | CONFIG_STORE_LOG_WINDOW_ONE_MIN_LINES
        | CONFIG_STORE_LOG_WINDOW_TWO_MIN_LINES => 20,
        CONFIG_STORE_LOG_WINDOW_ONE_INTERVAL | CONFIG_STORE_LOG_WINDOW_TWO_INTERVAL => 30,
        _ => CONFIG_SLIDER_MAX,
    }
}

/// Per-store step of a law'd slider: every stepper family member moves by 1 except the
/// log-window column-width rows, whose arms add/subtract 10 and clamp **after** the
/// arithmetic (`add eax,0xa` with an above-0x64 fix-up at FFXiMain.dll retail-2026-09
/// .text 0x199A66/0x199A71 and `add eax,-0xa` clamped to 0 at 0x199A4B; vault main_menu.md
/// §59 addendum 46) — so a width pin sitting at 95 steps to its ceiling, not past it.
/// All four log-window steppers increment on Right and decrement on Left: in each row's
/// jump table (.rva FFXiMain.dll retail-2026-09 0x199C28/0x199C40/0x199C58/0x199C70) entry
/// 3 is the increment arm (interval .text 0x199D7, width .text 0x199A5E, max lines
/// .text 0x199B24, min lines .text 0x199BA9) and entry 4 the decrement, each committing
/// the store through the label accessor on every press.
pub fn config_slider_step(store_id: u32) -> i32 {
    match store_id {
        CONFIG_STORE_LOG_WINDOW_ONE_WIDTH | CONFIG_STORE_LOG_WINDOW_TWO_WIDTH => 10,
        _ => 1,
    }
}

/// The log-window line-count cross-clamp pair. Retail's max-lines decrement arm re-reads both
/// stores after its own write and lowers min-lines to the new maximum when it exceeds it
/// (getter pair .text FFXiMain.dll retail-2026-09 RVA 0x199AEF/0x199AF8, conditional setter at
/// 0x199B07..0x199B15); the min-lines increment arm raises max-lines to the new minimum
/// (0x199BBF..0x199BE8). Returns `(max store, min store)` for either member of a window's
/// pair so a stepper can mirror both clamps.
pub fn config_log_lines_pair(store_id: u32) -> Option<(u32, u32)> {
    match store_id {
        CONFIG_STORE_LOG_WINDOW_ONE_MAX_LINES | CONFIG_STORE_LOG_WINDOW_ONE_MIN_LINES => Some((
            CONFIG_STORE_LOG_WINDOW_ONE_MAX_LINES,
            CONFIG_STORE_LOG_WINDOW_ONE_MIN_LINES,
        )),
        CONFIG_STORE_LOG_WINDOW_TWO_MAX_LINES | CONFIG_STORE_LOG_WINDOW_TWO_MIN_LINES => Some((
            CONFIG_STORE_LOG_WINDOW_TWO_MAX_LINES,
            CONFIG_STORE_LOG_WINDOW_TWO_MIN_LINES,
        )),
        _ => None,
    }
}

/// True when the Config window's own arm table contains a slider row - only those
/// windows keep a pin alive: the volume/level pages and the two individual
/// log-window editors (`conf5w1`/`conf5w2`).
pub fn config_page_supports_slider_pin(window: &str) -> bool {
    config_page_arms(window).is_some_and(|arms| {
        arms.iter()
            .any(|arm| matches!(arm, ConfigRowArm::Slider { .. }))
    })
}

/// State key for a Config-family window: pages keep their index in
/// RETAIL_CONFIG_PAGE_WINDOWS and sub-windows start at 32 (index in
/// RETAIL_CONFIG_SUB_WINDOWS + 32) — a kuluu-internal coordinate, not an option id;
/// used with the retail object field offset by
/// [`DynamicMenu::config_window_choices`] (vault main_menu.md §59 addendum 9).
pub fn config_window_state_key(window: &str) -> Option<u8> {
    RETAIL_CONFIG_PAGE_WINDOWS
        .iter()
        .position(|w| *w == window)
        .map(|i| i as u8)
        .or_else(|| {
            RETAIL_CONFIG_SUB_WINDOWS
                .iter()
                .position(|w| *w == window)
                .map(|i| 32 + i as u8)
        })
}

/// The option the bit helper FFXiMain.dll retail-2026-09 RVA 0x1938F0 pins as its
/// read-modify-write target - a flag dword whose individual bits are the on/off
/// state of several Config pairs (`conf12wi` bits 7 and 6, `conf3win` bit 2;
/// vault main_menu.md §59 addendum 5).
pub const CONFIG_OPTION_MISC_FLAGS: u32 = 0x99;

/// The one option dword the filter-list window class persists: its load method (.text
/// FFXiMain.dll retail-2026-09 RVA 0x198370, `fxfilter`'s vtable slot +0x44) reads it through
/// wrapper 0x193850 into `[obj+0x30]`; its store method (.text 0x198390) writes the same id back
/// through 0x193830. Every row's state glyph is one bit of it (test helper .text 0x193960,
/// write helper .text 0x193980). Client-local: LSB carries only MessageFilter/MessageFilter2 on
/// c2s 0x0DB (`vendor/server/src/map/packets/c2s/0x0db_config_language.cpp`), never this store
/// (vault main_menu.md §59 addendum 14; bead kuluu-o9qt7.20.1.3).
pub const CONFIG_OPTION_FILTER_CONSOLE_FLAGS: u32 = 0x92;

/// One row of a filter-list window as its **code-side** descriptor table states it. The DAT
/// record behind these windows supplies geometry only — all fourteen item blocks there carry no
/// string at all (vault main_menu.md §59 addendum 12), so the row list is DLL data while every
/// label stays catalog data read from the DATs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterListRow {
    /// Bit index inside [`CONFIG_OPTION_FILTER_CONSOLE_FLAGS`] (`descriptor[0]`).
    pub bit: u8,
    /// Row caption: catalog table 0x0E string id (`descriptor+2`, resolved through
    /// [`CAPTION_CATALOG_TABLE_14_FILE_ID`](ffxi_dat::menu_windows::CAPTION_CATALOG_TABLE_14_FILE_ID)).
    pub caption_id: u32,
}

/// `fxfilter`'s complete row list = descriptor table .rdata **0x379B08** of FFXiMain.dll
/// retail-2026-09: exactly eighteen entries `{bit 0..=0x11, zero byte, catalog-11 id 153..170}`.
/// Its load method returns row count **0x12** (.text 0x198383) and the drawer (.text 0x1983B0)
/// labels every slot with `GetText(table 0xb, descriptor+2)` through wrapper RVA 0x152EF0 (vault
/// main_menu.md §59 addendum 14; bead kuluu-o9qt7.20.1.3).
pub const FXFILTER_ROWS: [FilterListRow; 18] = [
    FilterListRow {
        bit: 0,
        caption_id: 153,
    },
    FilterListRow {
        bit: 1,
        caption_id: 154,
    },
    FilterListRow {
        bit: 2,
        caption_id: 155,
    },
    FilterListRow {
        bit: 3,
        caption_id: 156,
    },
    FilterListRow {
        bit: 4,
        caption_id: 157,
    },
    FilterListRow {
        bit: 5,
        caption_id: 158,
    },
    FilterListRow {
        bit: 6,
        caption_id: 159,
    },
    FilterListRow {
        bit: 7,
        caption_id: 160,
    },
    FilterListRow {
        bit: 8,
        caption_id: 161,
    },
    FilterListRow {
        bit: 9,
        caption_id: 162,
    },
    FilterListRow {
        bit: 10,
        caption_id: 163,
    },
    FilterListRow {
        bit: 11,
        caption_id: 164,
    },
    FilterListRow {
        bit: 12,
        caption_id: 165,
    },
    FilterListRow {
        bit: 13,
        caption_id: 166,
    },
    FilterListRow {
        bit: 14,
        caption_id: 167,
    },
    FilterListRow {
        bit: 15,
        caption_id: 168,
    },
    FilterListRow {
        bit: 16,
        caption_id: 169,
    },
    FilterListRow {
        bit: 17,
        caption_id: 170,
    },
];

/// The code-side row tables of the single-bit filter-list windows.
pub fn filter_list_rows(window: &str) -> Option<&'static [FilterListRow]> {
    match window {
        "fxfilter" => Some(&FXFILTER_ROWS),
        _ => None,
    }
}

/// The five option dwords the `cfilter` window persists: its load method (.text FFXiMain.dll
/// retail-2026-09 RVA 0x197F50) pulls each through wrapper 0x193850 into `[obj+0x30]`..`[obj+0x40]`
/// and its store method (.text 0x197F90) writes all five straight back through 0x193830 — the row
/// count lives in the same load (.text FFXiMain.dll retail-2026-09 RVA 0x197F88 `mov eax, 0x37` =
/// **55** rows). Client-local like every other config store: LSB carries only
/// MessageFilter/MessageFilter2 on c2s 0x0DB
/// (`vendor/server/src/map/packets/c2s/0x0db_config_language.cpp`).
pub const CONFIG_OPTION_CFILTER_LANE_A: u32 = 0x32; // pair bits for descriptor kinds < 0x1e
pub const CONFIG_OPTION_CFILTER_LANE_B: u32 = 0x33; // pair bit (kind − 0x1e) for kinds ≥ 0x1e
/// The console dword `[obj+0x38]`: one bit per descriptor `param`, flipped by kind-0x33 rows and
/// driven by the interlock machine's C side.
pub const CONFIG_OPTION_CFILTER_CONSOLE: u32 = 0x34;
/// The group-level dword `[obj+0x3c]`: the three negative-param group rows store their level here.
pub const CONFIG_OPTION_CFILTER_GROUP_LEVEL: u32 = 0x35;
/// The master dword `[obj+0x40]` paired with console bit 2 — the param-2 row flips it (.text
/// FFXiMain.dll retail-2026-09 RVA 0x198124) when its toggle lands that bit clear, and a set
/// master switches that row's glyph to the {3,4} variants (.text 0x19827C).
pub const CONFIG_OPTION_CFILTER_MASTER: u32 = 0x47;

/// `/furcc`'s furnishing-camera-collision option — slash id **0x84**, handler .text
/// FFXiMain.dll retail-2026-09 `.text 0x7E9B0`; setter `.text 0x1938F0`, getter `.text
/// 0x193890`. The stored value is INVERTED from the words: `/furcc on` writes **0** and
/// `/furcc off` writes **1** (vault main_menu.md §59 addendum 35; bead kuluu-o9qt7.41).
pub const CONFIG_OPTION_FURNISHING_CAMERA_COLLISION: u32 = 6;
/// `/layoutctrl`'s arrangement-control-style option — slash id **0x85**, handler .text
/// FFXiMain.dll retail-2026-09 `.text 0x7EA40`; setter `.text 0x193830`, getter `.text
/// 0x193850`. Keywords are `type1`/`type2` (pair table .data 0x357AA0), values **0**/**1**
/// in that order — not on/off (§59 addendum 35; bead kuluu-o9qt7.41).
pub const CONFIG_OPTION_LAYOUT_CONTROL_TYPE: u32 = 0xC1;
/// [`CONFIG_OPTION_LAYOUT_CONTROL_TYPE`]'s values: `type1` reverses arrange floor steps while the
/// camera eye's z is at or past the focus's, `type2` never does
/// (.agents/skills/retail-observe/references/2026-10-15-mog-house-arrange.md B6).
pub const LAYOUT_CONTROL_TYPE1: i32 = 0;
pub const LAYOUT_CONTROL_TYPE2: i32 = 1;
/// `/statusparty`'s party-member-status-icons option — slash id **0x86**, handler .text
/// FFXiMain.dll retail-2026-09 `.text 0x7EAE0`; pair table .data 0x357AAC = {off,on} writes **0**/**1**
/// (NOT inverted: 1 = enabled), and its query path normalizes the stored dword to a bool and rewrites it
/// through setter `.text 0x193830` at merge **.text 0x7EB1C** (§59 addendum 35; bead kuluu-o9qt7.42).
pub const CONFIG_OPTION_STATUS_PARTY_ICONS: u32 = 0xC2;

/// One row of class B's code-side descriptor table — `.rdata` **FFXiMain.dll retail-2026-09 RVA
/// 0x379A28**, stride 4 bytes `{u8 kind, i8 param, u16 caption id}`. The DAT record behind the
/// window carries no strings at all (§59 addendum 12), so this table is the row list while every
/// label stays catalog-11 text read through `caption_id` at runtime.
#[derive(Debug, Clone, Copy)]
pub struct CfilterDescriptorRow {
    /// Row class: its own pair-lane bit index for most rows; [`CFILTER_KIND_CONSOLE_OR_GROUP`]
    /// marks the console/group family.
    pub kind: u8,
    /// Group rows carry −1/−2/−3 (level − table offset); console and interlock rows name their
    /// console-dword bit; plain toggle rows all carry [`CFILTER_PARAM_TOGGLE_MARKER`].
    pub param: i8,
    /// Row caption: catalog table 11 string id, read live like [`FilterListRow`]`'s`.
    pub caption_id: u32,
}

/// `cfilter`’s complete row list = descriptor table `.rdata FFXiMain.dll retail-2026-09 RVA
/// 0x379A28`, all **55** entries the load method counts (.text FFXiMain.dll retail-2026-09 RVA
/// 0x197F88). Entry idx 0 is the all-zero row retail draws first and still dispatches (interlock
/// class on lane bit 0 / console bit 0 — no special case in the shared select method .text
/// FFXiMain.dll retail-2026-09 RVA 0x198080). The three trailing negative-param rows are the group
/// levels; kind 0x33 with param ≥ 0 are console-bit rows.
pub const CFILTER_ROWS: [CfilterDescriptorRow; 55] = [
    CfilterDescriptorRow {
        kind: 0x00,
        param: 0,
        caption_id: 0,
    },
    CfilterDescriptorRow {
        kind: 0x33,
        param: 2,
        caption_id: 2,
    },
    CfilterDescriptorRow {
        kind: 0x33,
        param: 3,
        caption_id: 3,
    },
    CfilterDescriptorRow {
        kind: 0x33,
        param: 4,
        caption_id: 4,
    },
    CfilterDescriptorRow {
        kind: 0x33,
        param: 8,
        caption_id: 5,
    },
    CfilterDescriptorRow {
        kind: 0x31,
        param: 9,
        caption_id: 6,
    },
    CfilterDescriptorRow {
        kind: 0x32,
        param: 10,
        caption_id: 7,
    },
    CfilterDescriptorRow {
        kind: 0x33,
        param: 7,
        caption_id: 8,
    },
    CfilterDescriptorRow {
        kind: 0x03,
        param: 5,
        caption_id: 9,
    },
    CfilterDescriptorRow {
        kind: 0x01,
        param: 1,
        caption_id: 1,
    },
    CfilterDescriptorRow {
        kind: 0x2e,
        param: 11,
        caption_id: 195,
    },
    CfilterDescriptorRow {
        kind: 0x04,
        param: 11,
        caption_id: 11,
    },
    CfilterDescriptorRow {
        kind: 0x05,
        param: 11,
        caption_id: 12,
    },
    CfilterDescriptorRow {
        kind: 0x06,
        param: 11,
        caption_id: 13,
    },
    CfilterDescriptorRow {
        kind: 0x07,
        param: 11,
        caption_id: 14,
    },
    CfilterDescriptorRow {
        kind: 0x08,
        param: 11,
        caption_id: 15,
    },
    CfilterDescriptorRow {
        kind: 0x09,
        param: 11,
        caption_id: 16,
    },
    CfilterDescriptorRow {
        kind: 0x17,
        param: 11,
        caption_id: 30,
    },
    CfilterDescriptorRow {
        kind: 0x0d,
        param: 11,
        caption_id: 20,
    },
    CfilterDescriptorRow {
        kind: 0x0e,
        param: 11,
        caption_id: 21,
    },
    CfilterDescriptorRow {
        kind: 0x0f,
        param: 11,
        caption_id: 22,
    },
    CfilterDescriptorRow {
        kind: 0x10,
        param: 11,
        caption_id: 23,
    },
    CfilterDescriptorRow {
        kind: 0x11,
        param: 11,
        caption_id: 24,
    },
    CfilterDescriptorRow {
        kind: 0x18,
        param: 11,
        caption_id: 31,
    },
    CfilterDescriptorRow {
        kind: 0x12,
        param: 11,
        caption_id: 25,
    },
    CfilterDescriptorRow {
        kind: 0x13,
        param: 11,
        caption_id: 26,
    },
    CfilterDescriptorRow {
        kind: 0x14,
        param: 11,
        caption_id: 27,
    },
    CfilterDescriptorRow {
        kind: 0x15,
        param: 11,
        caption_id: 28,
    },
    CfilterDescriptorRow {
        kind: 0x16,
        param: 11,
        caption_id: 29,
    },
    CfilterDescriptorRow {
        kind: 0x27,
        param: 11,
        caption_id: 177,
    },
    CfilterDescriptorRow {
        kind: 0x26,
        param: 11,
        caption_id: 178,
    },
    CfilterDescriptorRow {
        kind: 0x22,
        param: 11,
        caption_id: 179,
    },
    CfilterDescriptorRow {
        kind: 0x23,
        param: 11,
        caption_id: 180,
    },
    CfilterDescriptorRow {
        kind: 0x24,
        param: 11,
        caption_id: 181,
    },
    CfilterDescriptorRow {
        kind: 0x25,
        param: 11,
        caption_id: 182,
    },
    CfilterDescriptorRow {
        kind: 0x21,
        param: 11,
        caption_id: 171,
    },
    CfilterDescriptorRow {
        kind: 0x20,
        param: 11,
        caption_id: 172,
    },
    CfilterDescriptorRow {
        kind: 0x1c,
        param: 11,
        caption_id: 173,
    },
    CfilterDescriptorRow {
        kind: 0x1d,
        param: 11,
        caption_id: 174,
    },
    CfilterDescriptorRow {
        kind: 0x1e,
        param: 11,
        caption_id: 175,
    },
    CfilterDescriptorRow {
        kind: 0x1f,
        param: 11,
        caption_id: 176,
    },
    CfilterDescriptorRow {
        kind: 0x19,
        param: 11,
        caption_id: 32,
    },
    CfilterDescriptorRow {
        kind: 0x0a,
        param: 11,
        caption_id: 17,
    },
    CfilterDescriptorRow {
        kind: 0x0b,
        param: 11,
        caption_id: 18,
    },
    CfilterDescriptorRow {
        kind: 0x0c,
        param: 11,
        caption_id: 19,
    },
    CfilterDescriptorRow {
        kind: 0x1a,
        param: 11,
        caption_id: 88,
    },
    CfilterDescriptorRow {
        kind: 0x1b,
        param: 11,
        caption_id: 144,
    },
    CfilterDescriptorRow {
        kind: 0x28,
        param: 11,
        caption_id: 191,
    },
    CfilterDescriptorRow {
        kind: 0x29,
        param: 11,
        caption_id: 192,
    },
    CfilterDescriptorRow {
        kind: 0x2a,
        param: 11,
        caption_id: 193,
    },
    CfilterDescriptorRow {
        kind: 0x2d,
        param: 11,
        caption_id: 194,
    },
    CfilterDescriptorRow {
        kind: 0x2f,
        param: 6,
        caption_id: 10,
    },
    CfilterDescriptorRow {
        kind: 0x33,
        param: -1,
        caption_id: 33,
    },
    CfilterDescriptorRow {
        kind: 0x33,
        param: -2,
        caption_id: 34,
    },
    CfilterDescriptorRow {
        kind: 0x33,
        param: -3,
        caption_id: 35,
    },
];

/// The `cfilter` DAT record's synthesis source (the record has fourteen string-less slots, §59
/// addendum 12); mirrors [`filter_list_rows`] for class B (bead kuluu-o9qt7.20.1.3).
pub fn cfilter_descriptor_rows(window: &str) -> Option<&'static [CfilterDescriptorRow]> {
    match window {
        "cfilter" => Some(&CFILTER_ROWS),
        _ => None,
    }
}

/// `cfilter`'s select arms — position-identical to [`CFILTER_ROWS`] (the raw table index retail's
/// event dispatcher computes as `scroll_base + row − 1`, .text FFXiMain.dll retail-2026-09 RVA
/// 0x197D00, and hands to the shared select method .text 0x198080) — a written-out identity so
/// position == table_idx cannot drift from [`CFILTER_ROWS`].
pub const FILTER_LIST_CFILTER_ARMS: [ConfigRowArm; 55] = [
    ConfigRowArm::CfilterSelect { table_idx: 0 },
    ConfigRowArm::CfilterSelect { table_idx: 1 },
    ConfigRowArm::CfilterSelect { table_idx: 2 },
    ConfigRowArm::CfilterSelect { table_idx: 3 },
    ConfigRowArm::CfilterSelect { table_idx: 4 },
    ConfigRowArm::CfilterSelect { table_idx: 5 },
    ConfigRowArm::CfilterSelect { table_idx: 6 },
    ConfigRowArm::CfilterSelect { table_idx: 7 },
    ConfigRowArm::CfilterSelect { table_idx: 8 },
    ConfigRowArm::CfilterSelect { table_idx: 9 },
    ConfigRowArm::CfilterSelect { table_idx: 10 },
    ConfigRowArm::CfilterSelect { table_idx: 11 },
    ConfigRowArm::CfilterSelect { table_idx: 12 },
    ConfigRowArm::CfilterSelect { table_idx: 13 },
    ConfigRowArm::CfilterSelect { table_idx: 14 },
    ConfigRowArm::CfilterSelect { table_idx: 15 },
    ConfigRowArm::CfilterSelect { table_idx: 16 },
    ConfigRowArm::CfilterSelect { table_idx: 17 },
    ConfigRowArm::CfilterSelect { table_idx: 18 },
    ConfigRowArm::CfilterSelect { table_idx: 19 },
    ConfigRowArm::CfilterSelect { table_idx: 20 },
    ConfigRowArm::CfilterSelect { table_idx: 21 },
    ConfigRowArm::CfilterSelect { table_idx: 22 },
    ConfigRowArm::CfilterSelect { table_idx: 23 },
    ConfigRowArm::CfilterSelect { table_idx: 24 },
    ConfigRowArm::CfilterSelect { table_idx: 25 },
    ConfigRowArm::CfilterSelect { table_idx: 26 },
    ConfigRowArm::CfilterSelect { table_idx: 27 },
    ConfigRowArm::CfilterSelect { table_idx: 28 },
    ConfigRowArm::CfilterSelect { table_idx: 29 },
    ConfigRowArm::CfilterSelect { table_idx: 30 },
    ConfigRowArm::CfilterSelect { table_idx: 31 },
    ConfigRowArm::CfilterSelect { table_idx: 32 },
    ConfigRowArm::CfilterSelect { table_idx: 33 },
    ConfigRowArm::CfilterSelect { table_idx: 34 },
    ConfigRowArm::CfilterSelect { table_idx: 35 },
    ConfigRowArm::CfilterSelect { table_idx: 36 },
    ConfigRowArm::CfilterSelect { table_idx: 37 },
    ConfigRowArm::CfilterSelect { table_idx: 38 },
    ConfigRowArm::CfilterSelect { table_idx: 39 },
    ConfigRowArm::CfilterSelect { table_idx: 40 },
    ConfigRowArm::CfilterSelect { table_idx: 41 },
    ConfigRowArm::CfilterSelect { table_idx: 42 },
    ConfigRowArm::CfilterSelect { table_idx: 43 },
    ConfigRowArm::CfilterSelect { table_idx: 44 },
    ConfigRowArm::CfilterSelect { table_idx: 45 },
    ConfigRowArm::CfilterSelect { table_idx: 46 },
    ConfigRowArm::CfilterSelect { table_idx: 47 },
    ConfigRowArm::CfilterSelect { table_idx: 48 },
    ConfigRowArm::CfilterSelect { table_idx: 49 },
    ConfigRowArm::CfilterSelect { table_idx: 50 },
    ConfigRowArm::CfilterSelect { table_idx: 51 },
    ConfigRowArm::CfilterSelect { table_idx: 52 },
    ConfigRowArm::CfilterSelect { table_idx: 53 },
    ConfigRowArm::CfilterSelect { table_idx: 54 },
];

/// The class-B kind that routes to the console/group row family — the shared select method's
/// `cmp al, 0x33` (.text FFXiMain.dll retail-2026-09 RVA 0x198090) and the state method's same
/// test (.text 0x198251).
const CFILTER_KIND_CONSOLE_OR_GROUP: u8 = 0x33;

/// The param every plain toggle row carries — select dispatches on `cmp dl, 0xb` (.text
/// FFXiMain.dll retail-2026-09 RVA 0x19814A; the state method branches on it at .text 0x1982B4).
const CFILTER_PARAM_TOGGLE_MARKER: i8 = 0xb;

/// Class B's pair-lane split, shared by the bit test (.text FFXiMain.dll retail-2026-09 RVA
/// 0x1939A0) and the lane write (.text 0x1939D0): below the helpers' `cmp ecx, 0x1e` index maps
/// to bit `index` of [`CONFIG_OPTION_CFILTER_LANE_A`] (`[obj+0x30]`), at/above it to bit
/// (index − 0x1e) of [`CONFIG_OPTION_CFILTER_LANE_B`] (`[obj+0x34]`).
const CFILTER_LANE_SPLIT: u8 = 0x1e; // .text FFXiMain.dll retail-2026-09 RVA 0x1939A4
fn cfilter_pair_lane(index: u8) -> (u32, u32) {
    if index < CFILTER_LANE_SPLIT {
        (CONFIG_OPTION_CFILTER_LANE_A, index as u32)
    } else {
        (
            CONFIG_OPTION_CFILTER_LANE_B,
            (index - CFILTER_LANE_SPLIT) as u32,
        )
    }
}

/// The drawer's row-state method (.text FFXiMain.dll retail-2026-09 RVA 0x198240): glyph codes —
/// **1** pair-lane bit set, **2** console/interlock C bit set, and for the param-2 row while the
/// master dword holds, the {3,4} variants keyed on console bit 2 (.text FFXiMain.dll retail-2026-09
/// RVA 0x198291 `neg al; sbb eax,eax; neg eax; add eax,3` — both non-zero, so the row marks even
/// with its console bit clear). kuluu marks whenever the code is non-zero. Plain rows never consult
/// the console dword: .text FFXiMain.dll retail-2026-09 RVA 0x1982B7 branches straight to the
/// pair-lane-only tail (.text 0x1982F7).
pub fn cfilter_row_state(
    table_idx: usize,
    option_values: &std::collections::BTreeMap<u32, i32>,
) -> u8 {
    let Some(row) = CFILTER_ROWS.get(table_idx) else {
        return 0;
    };
    let console_bit = |bit: i8| {
        // Bit test .text FFXiMain.dll retail-2026-09 RVA 0x193960 over the `[obj+0x38]` dword;
        // x86 masks dword shift counts, and every param in the table is well below that.
        let mask = option_values
            .get(&CONFIG_OPTION_CFILTER_CONSOLE)
            .copied()
            .unwrap_or(0) as u32;
        mask & (1u32 << ((bit as u32) % 32)) != 0
    };
    if row.kind == CFILTER_KIND_CONSOLE_OR_GROUP {
        if row.param < 0 {
            // Group rows (.text FFXiMain.dll retail-2026-09 RVA 0x19825E): marked while the stored
            // level has reached this row's own −param.
            let cur = option_values
                .get(&CONFIG_OPTION_CFILTER_GROUP_LEVEL)
                .copied()
                .unwrap_or(0);
            return u8::from(cur >= -(row.param as i32));
        }
        if row.param == 2 {
            // The master dword picks the glyph family (.text FFXiMain.dll retail-2026-09 RVA
            // 0x19827C); both variants mark.
            let master = option_values
                .get(&CONFIG_OPTION_CFILTER_MASTER)
                .copied()
                .unwrap_or(0);
            if master != 0 {
                return if console_bit(row.param) { 4 } else { 3 };
            }
        }
        if row.param == CFILTER_PARAM_TOGGLE_MARKER {
            // A kind-0x33 param-marker row never marks (.text FFXiMain.dll retail-2026-09 RVA
            // 0x1982A1, jump to the zero tail).
            return 0;
        }
        return u8::from(console_bit(row.param)) * 2;
    }
    let (lane_opt, lane_bit) = cfilter_pair_lane(row.kind);
    let pair_set =
        option_values.get(&lane_opt).copied().unwrap_or(0) as u32 & (1u32 << lane_bit) != 0;
    if row.param == CFILTER_PARAM_TOGGLE_MARKER {
        return u8::from(pair_set);
    }
    // Interlock rows: the L glyph wins while the pair bit holds (.text FFXiMain.dll retail-2026-09
    // RVA 0x1982B9–0x1982D1), else the shared console tail (.text 0x1982D4 movsx → 0x1982DC).
    if pair_set {
        return 1;
    }
    u8::from(console_bit(row.param)) * 2
}

/// Class B's shared select method — vtable slot +0x4c = .text FFXiMain.dll retail-2026-09 RVA
/// 0x198080, reached from the event dispatcher's row branch (.text 0x197D00) — as a transition on
/// the five persisted dwords. Four classes, in the method's own dispatch order:
/// - kind 0x33 with param < 0 (group rows): level `v = table_idx − 0x33`; store `v−1` when
///   `v <= cur` (.text FFXiMain.dll retail-2026-09 RVA 0x1980B2 `jle`) else `v`.
/// - kind 0x33 with param ≥ 0 (console rows): toggle bit `param` of the console dword; when
///   param == 2 and that toggle landed bit 2 **clear**, flip the master dword to
///   `(old == 0 ? 1 : 0)` (.text FFXiMain.dll retail-2026-09 RVA 0x19810E–0x19812E `sete`).
/// - kind ≠ 0x33 with param == 0xb (plain rows): toggle bit `kind` across the pair lanes
///   (.text FFXiMain.dll retail-2026-09 RVA 0x1981EB).
/// - kind ≠ 0x33 otherwise (interlock rows): a three-state cycle — with L set, clear L **and**
///   force C←1; else with C set, clear C; else set L (.text FFXiMain.dll retail-2026-09 RVA
///   0x198157–0x1981E8).
pub fn cfilter_apply_select(
    table_idx: usize,
    option_values: &mut std::collections::BTreeMap<u32, i32>,
) {
    let Some(row) = CFILTER_ROWS.get(table_idx) else {
        return;
    };
    // Sign-extended descriptor param as a dword bit index; x86 masks shift counts to 5 bits and
    // every table value sits far below that.
    let cbit = (row.param as u32) % 32;
    if row.kind == CFILTER_KIND_CONSOLE_OR_GROUP {
        if row.param < 0 {
            let v = table_idx as i32 - 0x33; // .text FFXiMain.dll retail-2026-09 RVA 0x1980AD `lea eax,[edi−0x33]`
            let cur = option_values
                .get(&CONFIG_OPTION_CFILTER_GROUP_LEVEL)
                .copied()
                .unwrap_or(0);
            let next = if v <= cur { v - 1 } else { v };
            option_values.insert(CONFIG_OPTION_CFILTER_GROUP_LEVEL, next);
            return;
        }
        if row.param == CFILTER_PARAM_TOGGLE_MARKER {
            // No-op arm (.text FFXiMain.dll retail-2026-09 RVA 0x1980C7 `cmp al,0xb; je` to ret).
            return;
        }
        let console = option_values
            .get(&CONFIG_OPTION_CFILTER_CONSOLE)
            .copied()
            .unwrap_or(0) as u32;
        // Toggle via the test/RMW pair (.text FFXiMain.dll retail-2026-09 RVA 0x1980D7/0x1980F9):
        // bit set → write value 0, clear → write value 1.
        let toggled = console ^ (1u32 << cbit);
        option_values.insert(CONFIG_OPTION_CFILTER_CONSOLE, toggled as i32);
        if row.param != 2 {
            return;
        }
        // The master flip rule (.text FFXiMain.dll retail-2026-09 RVA 0x19810E): re-test bit 2 of
        // the new console dword and touch `[obj+0x40]` only when it landed clear.
        if toggled & (1u32 << cbit) != 0 {
            return;
        }
        let master = option_values
            .get(&CONFIG_OPTION_CFILTER_MASTER)
            .copied()
            .unwrap_or(0);
        option_values.insert(CONFIG_OPTION_CFILTER_MASTER, i32::from(master == 0));
        return;
    }
    let (lane_opt, lane_bit) = cfilter_pair_lane(row.kind);
    let lane = option_values.get(&lane_opt).copied().unwrap_or(0) as u32;
    if row.param == CFILTER_PARAM_TOGGLE_MARKER {
        // Plain pair-lane toggle (.text FFXiMain.dll retail-2026-09 RVA 0x1981EB–0x198224).
        option_values.insert(lane_opt, (lane ^ (1u32 << lane_bit)) as i32);
        return;
    }
    if lane & (1u32 << lane_bit) != 0 {
        // L set: clear it (.text FFXiMain.dll retail-2026-09 RVA 0x198175, lane write value 0)…
        option_values.insert(lane_opt, (lane & !(1u32 << lane_bit)) as i32);
        // …and force the console bit on (.text FFXiMain.dll retail-2026-09 RVA 0x198188, RMW value 1).
        let console = option_values
            .get(&CONFIG_OPTION_CFILTER_CONSOLE)
            .copied()
            .unwrap_or(0) as u32;
        option_values.insert(
            CONFIG_OPTION_CFILTER_CONSOLE,
            (console | (1u32 << cbit)) as i32,
        );
    } else {
        let console = option_values
            .get(&CONFIG_OPTION_CFILTER_CONSOLE)
            .copied()
            .unwrap_or(0) as u32;
        if console & (1u32 << cbit) != 0 {
            // C set: clear it (.text FFXiMain.dll retail-2026-09 RVA 0x1981BF, RMW value 0).
            option_values.insert(
                CONFIG_OPTION_CFILTER_CONSOLE,
                (console & !(1u32 << cbit)) as i32,
            );
        } else {
            // Both clear: set L (.text FFXiMain.dll retail-2026-09 RVA 0x1981D0–0x1981DD, lane write value 1).
            option_values.insert(lane_opt, (lane | (1u32 << lane_bit)) as i32);
        }
    }
}

/// Base factory templates the two key-assignment panels write through store
/// wrapper FFXiMain.dll retail-2026-09 RVA 0x193830 when their vtable slot +0x48
/// runs (.text 0x19d1c0 for `k1assign`, .text 0x19ce70 for `k2assign` — the same method both constructors tail-invoke). Index is the panel's shadow byte (+0x29..=+0x48): k1 maps it to option ids **0x4A..=0x69** (offset + 0x21, loop .text 0x19d285) and k2 to **0x6A..=0x89** (offset + 0x41, loop .text 0x19cf2b).
/// Retail patches a few bytes when its keyboard-layout selector `[0x10456a28]+8` reports 2 or 3 (.text blocks 0x19d24e/0x19d272 and 0x19cf03/0x19cf19) — that selector is undug [I], so kuluu ships the base template until it lands with the live keymap (bead kuluu-o9qt7.26.1). These are DLL-code immediates, not DAT content, so encoding them here stands under the DAT-first rule.
const KEY_ASSIGN_K1_DEFAULTS: [i32; 32] = [
    0x11, 0x1f, 0x1e, 0x20, 0x17, 0x25, 0x24, 0x26, 0x00, 0x00, 0x00, 0x00, 0x34, 0x33, 0x00, 0x00,
    0x00, 0x00, 0x15, 0x31, 0x2f, 0x13, 0x23, 0x21, 0x0c, 0x2c, 0x1b, 0x1a, 0x12, 0x10, 0x00, 0x00,
];
const KEY_ASSIGN_K2_DEFAULTS: [i32; 32] = [
    0x1e, 0x17, 0x32, 0x24, 0x11, 0x30, 0x15, 0x31, 0x0c, 0x21, 0x1b, 0x1a, 0x2c, 0x34, 0x33, 0x00,
    0x00, 0x12, 0x2f, 0x23, 0x2e, 0x1f, 0x19, 0x14, 0x26, 0x13, 0x16, 0x2d, 0x10, 0x00, 0x00, 0x00,
];

/// The first option id of each panel's store range (k1 loop edi=0x4A..0x69;
/// k2 edi=0x6A..0x89).
pub const KEY_ASSIGN_K1_FIRST_OPTION: u32 = 0x4A;
pub const KEY_ASSIGN_K2_FIRST_OPTION: u32 = 0x6A;

/// All (option_id, default) pairs the reset writes — the Yes branch of `conf6win`'s dialog row. Client-local like every other config-store law: LSB's c2s 0x0DC accepts only the named SAVE_CONF bits (vendor/server/src/map/packets/c2s/0x0dc_config.cpp).
pub fn key_assign_reset_defaults() -> impl Iterator<Item = (u32, i32)> + Clone {
    KEY_ASSIGN_K1_DEFAULTS
        .iter()
        .enumerate()
        .map(|(i, &value)| (KEY_ASSIGN_K1_FIRST_OPTION + i as u32, value))
        .chain(
            KEY_ASSIGN_K2_DEFAULTS
                .iter()
                .enumerate()
                .map(|(i, &value)| (KEY_ASSIGN_K2_FIRST_OPTION + i as u32, value)),
        )
}

/// One Config-family window's rows: DAT captions in file order, each carrying
/// its handler-grounded select action — hub row i name-opens page i (twelve
/// jump-table branches; the thirteenth widget falls to the default tail),
/// `conf5m`'s widgets drill the log windows, and every other option row keeps
/// its store id for bead .20.1 while answering the standard not-implemented
/// line. A radio group member's label carries the marker when it holds the
/// store's current value. Empty when the DAT rows are not loaded.
/// The roomlist's five rows: labels straight from ConfigDatRows (DAT record slots +
/// table-12 catalog), each carrying its style word. Row 0 (`Rem's Tale Chapters 2
/// Stored`, style word 0) is display-only in kuluu because retail's sender bails before
/// the packet for it (.text FFXiMain.dll retail-2026-09 0x1D6AD0 `test ecx,ecx; je` at
/// .text 0x1D6AEC — select only refreshes/blips; vault main_menu.md §59 addendum 20/21).
pub fn roomlist_rows(table: &ConfigDatRows) -> Vec<DynamicMenuRow> {
    table
        .rows_for(ROOMLIST_DAT_WINDOW)
        .iter()
        .enumerate()
        .map(|(position, (_, label))| {
            let style = ROOMLIST_ROW_STYLES.get(position).copied().unwrap_or(0);
            DynamicMenuRow {
                tint: None,
                label: label.clone(),
                action: if style == 0 {
                    DynamicMenuAction::DisplayInfo
                } else {
                    DynamicMenuAction::MogHouseRemodelSelect { style }
                },
            }
        })
        .collect()
}

pub fn config_window_rows(
    window: &str,
    table: &ConfigDatRows,
    option_values: &std::collections::BTreeMap<u32, i32>,
    option_edit: Option<(u32, u8)>,
    // Per-window local choices for the `WindowLocal` pair rows, keyed by
    // config_window_state_key + the retail window object's field offset (`conf3win`'s
    // `[obj+0x38]`, arms .text FFXiMain.dll retail-2026-09 RVA 0x19aa85/0x19aa92).
    // Absent means never chosen — retail boots those fields to −1 (.text 0x19a8b8),
    // so neither row of a fresh pair carries the mark (vault main_menu.md §59
    // addendum 9; bead kuluu-o9qt7.20.1.2).
    window_choices: &std::collections::BTreeMap<(u8, u16), i32>,
) -> Vec<DynamicMenuRow> {
    let radio_arm = |position: usize| match window {
        "conf5win" => CONF5WIN_RADIO_ARMS.get(position).copied(),
        _ => None,
    };
    // Built in record file order (select-id space); reordered for display below.
    let rows = table
        .rows_for(window)
        .iter()
        .enumerate()
        .map(|(position, (option_id, label))| {
            let arm = radio_arm(position);
            let slider_store = match config_page_arms(window).and_then(|a| a.get(position)) {
                Some(ConfigRowArm::Slider { store_id }) => Some(*store_id),
                _ => None,
            };
            if let Some(store_id) = slider_store {
                let live = option_edit.filter(|(pin, _)| *pin == store_id).map_or_else(
                    || {
                        option_values
                            .get(&store_id)
                            .copied()
                            .unwrap_or_else(|| i32::from(config_slider_boot_default(store_id)))
                    },
                    |(_, val)| i32::from(val),
                );
                let pinned = option_edit.is_some_and(|(pin, _)| pin == store_id);
                return DynamicMenuRow {
                    tint: None,
                    label: if pinned {
                        format!("\u{bb} {label}: {live}")
                    } else {
                        format!("{label}: {live}")
                    },
                    action: DynamicMenuAction::ConfigSliderEdit { store_id },
                };
            }
            // Radio rows of any law'd page answer through the arm table.
            match config_page_arms(window).and_then(|a| a.get(position)) {
                Some(ConfigRowArm::Radio { store_id, value }) => {
                    return DynamicMenuRow {
                        tint: None,
                        label: if option_values.get(store_id) == Some(value) {
                            format!("\u{bb} {label}")
                        } else {
                            label.clone()
                        },
                        action: DynamicMenuAction::ConfigOptionSet {
                            store_id: *store_id,
                            value: *value,
                        },
                    };
                }
                Some(ConfigRowArm::RadioBit { bit, value }) => {
                    // The pair marks from one dword: the row is active when the
                    // stored flag mask already carries its own bit value.
                    let marked = option_values
                        .get(&CONFIG_OPTION_MISC_FLAGS)
                        .is_some_and(|mask| (((*mask as u32) >> *bit) & 1) as i32 == *value);
                    return DynamicMenuRow {
                        tint: None,
                        label: if marked {
                            format!("\u{bb} {label}")
                        } else {
                            label.clone()
                        },
                        action: DynamicMenuAction::ConfigOptionBitSet {
                            bit: *bit,
                            value: *value,
                        },
                    };
                }
                // The window-local pair rows mark from the per-window choice map
                // (arms .text FFXiMain.dll retail-2026-09 RVA 0x19aa85/0x19aa92 write
                // only `[obj+0x38]`); an unset window has no mark on either row.
                Some(ConfigRowArm::WindowLocal { obj_off, value }) => {
                    let key = config_window_state_key(window).unwrap_or_default();
                    return DynamicMenuRow {
                        tint: None,
                        label: if window_choices.get(&(key, *obj_off)) == Some(value) {
                            format!("\u{bb} {label}")
                        } else {
                            label.clone()
                        },
                        action: DynamicMenuAction::ConfigWindowChoiceSet {
                            window_key: key,
                            obj_off: *obj_off,
                            value: *value,
                        },
                    };
                }
                // A filter-list row's mark is one bit of the page's persisted dword (test helper
                // .text FFXiMain.dll retail-2026-09 RVA 0x193960 over `[obj+0x30]`); select flips
                // that bit in place — no pair row, and nothing on the wire.
                Some(ConfigRowArm::FilterBit { store_id, bit }) => {
                    let mask = option_values.get(store_id).copied().unwrap_or(0) as u32;
                    return DynamicMenuRow {
                        tint: None,
                        label: if mask & (1u32 << bit) != 0 {
                            format!("\u{bb} {label}")
                        } else {
                            label.clone()
                        },
                        action: DynamicMenuAction::ConfigOptionToggleBit {
                            store_id: *store_id,
                            bit: *bit,
                        },
                    };
                }
                // A `cfilter` row marks through the drawer's state method (.text FFXiMain.dll
                // retail-2026-09 RVA 0x198240 — glyph code non-zero means marked), and select rides
                // the shared select method's transition table (vault main_menu.md §59 addendum 15).
                Some(ConfigRowArm::CfilterSelect { table_idx }) => {
                    return DynamicMenuRow {
                        tint: None,
                        label: if cfilter_row_state(*table_idx as usize, option_values) != 0 {
                            format!("\u{bb} {label}")
                        } else {
                            label.clone()
                        },
                        action: DynamicMenuAction::ConfigCfilterSelect {
                            table_idx: *table_idx,
                        },
                    };
                }
                Some(ConfigRowArm::Sub { sub }) => {
                    return DynamicMenuRow {
                        tint: None,
                        label: label.clone(),
                        action: DynamicMenuAction::OpenRetailConfigSub { sub: *sub },
                    };
                }
                Some(ConfigRowArm::Companion { mode, opens }) => {
                    return DynamicMenuRow {
                        tint: None,
                        label: label.clone(),
                        action: DynamicMenuAction::OpenRetailConfigCompanion { mode: *mode, opens },
                    };
                }
                // The dialog row writes nothing at select — it only raises the
                // prompt; retail's callback is where the answer acts (arm .text
                // FFXiMain.dll retail-2026-09 RVA 0x19B309).
                Some(ConfigRowArm::ConfirmResetKeyAssignments) => {
                    return DynamicMenuRow {
                        tint: None,
                        label: label.clone(),
                        action: DynamicMenuAction::OpenKeyAssignResetPrompt,
                    };
                }
                _ => {}
            }
            DynamicMenuRow {
                tint: None,
                label: match arm {
                    Some((store_id, value)) if option_values.get(&store_id) == Some(&value) => {
                        format!("\u{bb} {label}")
                    }
                    _ => label.clone(),
                },
                action: match arm {
                    Some((store_id, value)) => {
                        DynamicMenuAction::ConfigOptionSet { store_id, value }
                    }
                    None if matches!(window, "conf5win") => {
                        // Past the law'd arm table the widget keeps its plain
                        // option-record identity rather than a guessed one.
                        DynamicMenuAction::ConfigOptionRow {
                            option_id: *option_id,
                        }
                    }
                    None if window == RETAIL_CONFIG_HUB_WINDOW
                        && position < RETAIL_CONFIG_PAGE_WINDOWS.len() =>
                    {
                        // Hub rows are jump-table branches 1..=12 in order (.text
                        // 0x1968A4 of handler FFXiMain.dll retail-2026-09 RVA 0x196770;
                        // vault main_menu.md §42).
                        DynamicMenuAction::OpenRetailConfigPage {
                            page: position as u8,
                        }
                    }
                    None if window == "conf5m" => match position {
                        // Handler FFXiMain.dll retail-2026-09 RVA 0x199240 switches the select id
                        // through a `dec eax / je` chain: id 1 arms .text 0x199282 pushing
                        // `menu    conf5win`, id 2 arms .text 0x19926C (`conf5w1`), and id 3 falls
                        // through to .text 0x199256 (`conf5w2`) — shared settings first, matching
                        // the DAT captions; an earlier §47 reading had the ends swapped.
                        0 => DynamicMenuAction::OpenRetailConfigSub { sub: 0 },
                        1 => DynamicMenuAction::OpenRetailConfigSub { sub: 1 },
                        _ => DynamicMenuAction::OpenRetailConfigSub { sub: 2 },
                    },
                    None => DynamicMenuAction::ConfigOptionRow {
                        option_id: *option_id,
                    },
                },
            }
        })
        .collect::<Vec<_>>();
    reorder_rows_by_record_y(window, table, rows)
}

/// Reorder a Config page's finished rows into retail **draw** order (item-block `y`, file
/// order as the tie-break) without disturbing their arms: each row already carries the action
/// its select-id earned, and only the order the player sees moves (`configwi` draws ids
/// 1,2,3,4,10,5,6,11,12,7,8,9,13 while its handler still switches on the raw id — vault
/// main_menu.md §59 addendum 14). A window whose geometry list disagrees with its row count is
/// left in file order rather than guessed.
fn reorder_rows_by_record_y(
    window: &str,
    table: &ConfigDatRows,
    rows: Vec<DynamicMenuRow>,
) -> Vec<DynamicMenuRow> {
    let order = display_order(table.row_ys.get(window), rows.len());
    if order
        .iter()
        .enumerate()
        .all(|(position, &index)| position == index)
    {
        return rows;
    }
    let mut slots: Vec<Option<DynamicMenuRow>> = rows.into_iter().map(Some).collect();
    let mut ordered = Vec::with_capacity(order.len());
    for index in order {
        if let Some(row) = slots[index].take() {
            ordered.push(row);
        }
    }
    ordered
}

/// The faqsub Help Desk panel's destinations. The DAT `faqs` chunk is
/// chrome-only with an empty title token (-1), so labels come from the
/// window's own caption builder strings (vault main_menu.md §36): table 9 =
/// ROM/165/72.DAT idx 0xC3 `Q&A`, 0xCF `Mentor Search`, 0xDD `Config` — the
/// three targets named by the transfer messages at 0xD7/0xD8/0xD9.
pub const FAQ_ROW_QA: &str = "Q&A";
pub const FAQ_ROW_MENTOR: &str = "Mentor Search";
pub const FAQ_ROW_CONFIG: &str = "Config";
const HELPDESK_PANEL_ENTRIES: &[&str] = &[FAQ_ROW_QA, FAQ_ROW_MENTOR, FAQ_ROW_CONFIG];

/// The home-view row that opens the mail chrome: select id 2 of handler
/// FFXiMain.dll retail-2026-09 RVA 0x145460 routes through holder method .text
/// 0x145BA0, which stores the sub-frame field and name-opens `menu    mogpost`
/// (literal pushed at .text 0x145BAB); the window carries no inline captions
/// and rides the c2s 0x04D PBX protocol (vault main_menu.md §53/§59).
pub const HOME_ROW_DELIVERY_BOX: &str = "Delivery Box";

/// The myroom door row — select id 6 of handler FFXiMain.dll retail-2026-09 RVA
/// 0x145460 (arm @0x14556C, the §40 label "furniture-edit mode" corrected in vault
/// main_menu.md §59 addendum 20). The arm branches on predicate .text 0xF96E0: byte
/// [game-state +0x40C40] nonzero (door open) → MYROOM_IS{Kind=Close}; zero →
/// MYROOM_IS{Kind=Open} plus widget rewrites 0x118B60(5,5)/(5,7) and byte[holder
/// +0x16]=1. Both catalog tokens flip with the door state (147 ↔ the closed/open
/// caption pair); LSB implements neither Kind, so kuluu keeps one reachable form.
pub const HOME_ROW_OPEN_MOG_HOUSE: &str = "Open Mog House";

/// Which of the two DAT captions the door row wears: index 1 while the client's door byte says the
/// house is open to visitors, index 0 otherwise (the pair `[shut, open]` comes straight from
/// [`ConfigDatRows::home_door_captions`], never transcribed). `None` before that catalog loads.
pub fn home_door_caption(captions: &[Option<String>; 2], door_open: bool) -> Option<&str> {
    captions[usize::from(door_open)].as_deref()
}

/// The door row's position in [`HOME_VIEW_ROWS`] — select id 6 of the handler, not a row number.
pub fn home_door_row_index() -> usize {
    HOME_VIEW_ROWS
        .iter()
        .position(|label| *label == HOME_ROW_OPEN_MOG_HOUSE)
        .unwrap_or(usize::MAX)
}

/// Which home-view rows an open door greys: Layout and Remodel (select ids 5 and 7 of the handler),
/// matched by caption so a re-ordered [`HOME_VIEW_ROWS`] cannot silently move the gate.
fn home_row_gated_by_door(index: usize) -> bool {
    HOME_VIEW_ROWS
        .get(index)
        .is_some_and(|label| *label == "Layout" || *label == HOME_ROW_REMODEL)
}

/// The myroom Remodel row — select id 7 of handler FFXiMain.dll retail-2026-09 RVA
/// 0x145460 (arm @.text 0x1454E7): instance [0x106631e0] refresh + tail-jmp to the
/// roomlist control's show slot, name-opening `menu    roomlist` (.text 0x1D6820; vault
/// main_menu.md §40/§59 addendum 20).
pub const HOME_ROW_REMODEL: &str = "Remodel";

/// The myroom home-view panel's captioned widgets, verbatim from the DAT
/// chunk `myro` (ROM/0/1.DAT file-off 0x282D80 size 0x4D0) windowps tokens,
/// each decoded against ROM/165/76.DAT. The full fourteen named widgets —
/// the §40 reading of six of them as "chrome" was wrong: their caption tokens
/// label real buttons (vault main_menu.md §53 re-read): 168 M.Ward., 3 Mog
/// Safe, 37 Storage, 138 Mog Locker, 151 Satchel, 147 Open Mog House, 166
/// Mog Case, 158 Mog Sack, 87 Delivery Box, 67 Change Jobs, 89 Gardening, 4
/// Layout, 181 Safe 2, 182 Remodel. Display order is fixed twice over by the
/// geometry y column (0x15..0xE5 stride 0x10) and the per-record prev/next
/// chain ring. The handler answers select ids 1..7
/// (FFXiMain.dll retail-2026-09 RVA 0x145460 jump table .rdata 0x1455BC):
/// id1 storage, id2 mogpost, id3 jobchang, id4 "/garden", id5 "/layout" (the
/// §40 literal pairing was swapped), id6 the door row `Open Mog House` (vault
/// main_menu.md §59 addendum 20; the §40 "furniture-edit mode" label was wrong),
/// id7 roomlist.
pub const HOME_VIEW_ROWS: &[&str] = &[
    "M.Ward.",
    "Mog Safe",
    "Storage",
    "Mog Locker",
    "Satchel",
    HOME_ROW_OPEN_MOG_HOUSE,
    "Mog Case",
    "Mog Sack",
    HOME_ROW_DELIVERY_BOX,
    "Change Jobs",
    "Gardening",
    "Layout",
    "Safe 2",
    HOME_ROW_REMODEL,
];

/// `jobchang`'s two rows, verbatim from the DAT record 'jobc' (ROM/0/1.DAT):
/// windowps273/274 with catalog tokens 65 "Main Job" and 66 "Support Job"
/// (vault main_menu.md §53).
pub const JOB_CHANGE_ROWS: &[&str] = &["Main Job", "Support Job"];

/// The roomlist window's DAT record root — frame (16,208,w366), five dynamic
/// `windowps945`-949 slots whose caption tokens are all −1 [V-negative]: the labels
/// arrive at runtime from GetText table 12 (bead kuluu-o9qt7.13.5).
pub const ROOMLIST_DAT_WINDOW: &str = "roomlist";

/// The five table-12 catalog ids as fetched by the pool builder (.text FFXiMain.dll
/// retail-2026-09 0x1D6900 writes them in this order at .text 0x1D695E..; labels are read
/// from the DAT catalog file `GETTEXT_TABLE_12_CATALOG_FILE_ID` at load, never copied).
/// (Fetch ids re-read at .text 0x1D6907..0x1D6950: `push 0x1c2` first — catalog id
/// 450 = "No changes" — then 0x1be..0x1c1.)
const ROOMLIST_ROW_CAPTION_IDS: [u32; 5] = [450, 446, 447, 448, 449];

/// GetText **table-0x0E** ids for the `abisortw`/emote family, fetched per mode/state by retail's
/// caption pass (`.text FFXiMain.dll retail-2026-09 0x223E10`) and help-line swap
/// (`.text 0x225FA2`; bead kuluu-o9qt7.34). The strings live in the DAT catalog — read, never
/// transcribed.
pub const ABISORTW_CAPTION_MODE_EMOTE_ID: usize = 927;
pub const ABISORTW_CAPTION_MODE_MOUNT_ID: usize = 997;
/// State-word prompts for the emote window's two-step swap (`[emote+0x6C]` 1/2, handler-family
/// bodies `.text FFXiMain.dll retail-2026-09 0x225EC0` / `0x225E00`).
pub const EMOTE_SORT_PROMPT_FIRST_ID: usize = 931;
pub const EMOTE_SORT_PROMPT_SECOND_ID: usize = 932;

/// The confirm question the remodel prompt shows, from the same GetText table 12 that supplies the
/// row captions. The row callback fetches it right before opening the dialog (.text FFXiMain.dll
/// retail-2026-09 0x1D6B85 → dialog service RVA 0x210E30; vault main_menu.md §59 addendum 21); the
/// catalog string itself is pinned against a real install by ffxi-dat's table-12 test.
pub const ROOMLIST_CONFIRM_PROMPT_ID: u32 = 451;

/// Row i of the pool sends `MYROOM_IS{Kind=Remodel, Param2 = ROW_STYLES[i]}` (sender
/// .text FFXiMain.dll retail-2026-09 0x1D6AD0 reads Param2 = u16@[record+2], written by
/// the fill loop from the style words {0, 615, 616, 617, 618} pushed at .text
/// 0x1D696B..0x1D6980 — GP_CLI_COMMAND_MYROOM_IS_PARAM2 in
/// vendor/server/src/map/packets/c2s/0x0cb_myroom_is.h). Row 0's style word is 0 and the
/// sender bails before the packet (`test ecx,ecx; je`, .text 0x1D6AEC): its select only
/// refreshes — no wire, ever.
pub const ROOMLIST_ROW_STYLES: [u16; 5] = [0, 615, 616, 617, 618];

/// The one roomlist row that needs the patio bell before it answers anything: record
/// position 4 (style word 618). The pool builder stores `HasKeyItem(3051)` into roomlist
/// object +0x51 (.text FFXiMain.dll retail-2026-09 0x1D6890); without it the row paints in
/// the dim colour word 0x80404040 (.text 0x1D6A1A) and its select is only the error bell
/// (.text 0x1D6B63) — neither the confirm dialog nor the MYROOM_IS send is reached.
pub const ROOMLIST_PATIO_ROW_POSITION: usize = 4;
pub const PATIO_BELL_KEY_ITEM: u16 = 3051;

/// The single place that decides which rows draw dim and refuse their select, so no window
/// can grey a row its handler still answers. Status gates per entry kind (the `statcom2`
/// open code disables ids 3/4/5/12/13 at .text FFXiMain.dll retail-2026-09 0x148B50, and
/// content lifts it); the roomlist gates its patio row on [`PATIO_BELL_KEY_ITEM`].
pub fn menu_row_disabled(
    kind: MenuKind,
    index: usize,
    snap: &kuluu_snapshot::SceneSnapshot,
) -> bool {
    match kind {
        MenuKind::Status => crate::hud::status_panel::STATUS_ENTRIES
            .get(index)
            .is_some_and(|entry| !crate::hud::status_panel::status_row_enabled(entry.kind, snap)),
        MenuKind::RoomList => {
            index == ROOMLIST_PATIO_ROW_POSITION && !snap.key_items.contains(&PATIO_BELL_KEY_ITEM)
        }
        // While the door stands open the myroom update slot parks Layout (select id 5) and Remodel
        // (id 7) in widget state 5 - dim, select answers only the refusal bell - where its shut-door
        // path writes the interactive state 0xB: handler FFXiMain.dll retail-2026-09 `.text
        // 0x145240` calls the widget setter with (id 5, state 5) at `.text 0x1452FE` and (id 7,
        // state 5) at `.text 0x14531A`, against (id 5, 0xB)/(id 7, 0xB) at `.text 0x1453C6` /
        // `.text 0x1453ED`. Only the door half of retail's gates is reachable from kuluu data: the
        // shut-door path also demands helpers `0x97B40` and (for Remodel) `0xF97A0`, neither of
        // which has a client-side source yet, so those rows stay live while the door is shut.
        MenuKind::HomeView => {
            snap.myroom.is_some_and(|room| room.door_open) && home_row_gated_by_door(index)
        }
        MenuKind::Arrange(window) => window.row_disabled(index),
        _ => false,
    }
}

/// The bag identities follow the LSB CONTAINER_ID enum (vendor/server/src/map/
/// item_container.h: MOGSAFE=1, STORAGE=2, MOGLOCKER=4, MOGSATCHEL=5, MOGSACK=
/// 6, MOGCASE=7, WARDROBE=8, MOGSAFE2=9 — vault main_menu.md §53); the 'M.Ward.'
/// caption is the wardrobe bag. Which of the two window classes `storage`/
/// `storage2` (name-opened by select id 1's holder method 0x145C70) binds to
/// which bag stays unmapped — those windows have no DAT record to read it from
/// [V-negative], so kuluu maps the "Storage" caption to its same-named LSB bag.
pub fn home_view_row_container(label: &str) -> Option<u8> {
    use ffxi_proto::map::container as c;
    match label {
        "Mog Safe" => Some(c::LOC_MOGSAFE),
        "Storage" => Some(c::LOC_STORAGE),
        "Mog Locker" => Some(c::LOC_MOGLOCKER),
        "Satchel" => Some(c::LOC_MOGSATCHEL),
        "Mog Case" => Some(c::LOC_MOGCASE),
        "Mog Sack" => Some(c::LOC_MOGSACK),
        "M.Ward." => Some(c::LOC_WARDROBE),
        "Safe 2" => Some(c::LOC_MOGSAFE2),
        _ => None,
    }
}

/// The two home-view rows that are text commands (select ids 4 and 5 of handler
/// FFXiMain.dll retail-2026-09 RVA 0x145460, arms @0x145541/@0x14554A): each
/// runs its literal through the runner (0x80DC0) and closes myroom by name via
/// 0x15E910. The §40 pairing of literals to ids was swapped — fresh read: id 4
/// pushes .data 0x3715F0 "/garden", id 5 pushes .data 0x3715E8 "/layout".
/// LSB ships neither command (C++ nor scripts/commands), so like /clock the
/// literal never reaches an LSB server as anything meaningful [V-negative].
pub fn home_view_text_command(label: &str) -> Option<&'static str> {
    match label {
        "Gardening" => Some("/garden"),
        "Layout" => Some("/layout"),
        _ => None,
    }
}

/// cmbhlst-shaped rows: every recipe the server could currently resolve into a synth — crystal
/// tier substitutable by element and all positional ingredients assignable to bag slots under
/// LSB's per-stack use law (vault main_menu.md §28; `resolveRecipe`,
/// `GP_CLI_COMMAND_COMBINE_ASK::process`). The row carries its full ask plan so the select is a
/// straight send. Rows keep recipe-id order for stable paging.
pub fn craft_list_rows(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<DynamicMenuRow> {
    let bag: Vec<(u8, u16, u32)> = snap
        .inventory_main()
        .iter()
        .filter(|slot| slot.item_no != 0 && slot.quantity > 0)
        .map(|slot| (slot.index, slot.item_no, slot.quantity))
        .collect();
    let mut rows = Vec::new();
    for recipe in ffxi_vocab::craft::SYNTH_RECIPES {
        // A key-item gate means the server cancels without it (`resolveRecipe`).
        if recipe.key_item != 0 && !snap.key_items.contains(&(recipe.key_item as u16)) {
            continue;
        }
        let Some((crystal_slot, held_crystal_id)) = bag.iter().find_map(|(slot, id, _)| {
            ffxi_vocab::craft::crystal_accepts(recipe.crystal, *id).then_some((*slot, *id))
        }) else {
            continue;
        };
        let needed: Vec<u16> = recipe
            .ingredients
            .iter()
            .copied()
            .filter(|&i| i != 0)
            .collect();
        let Some(entries) = ffxi_proto::synth::assign_ingredient_slots(&needed, &bag, crystal_slot)
        else {
            continue;
        };
        let mut plan: [Option<ffxi_proto::synth::SynthIngredient>; 8] = [None; 8];
        for (cell, entry) in plan.iter_mut().zip(entries) {
            *cell = Some(entry);
        }
        rows.push(DynamicMenuRow {
            tint: None,
            label: recipe.name.to_string(),
            action: DynamicMenuAction::SynthesizeRecipe {
                crystal_id: held_crystal_id,
                crystal_slot,
                ingredients: plan,
            },
        });
    }
    rows
}

pub fn root_child_kind(label: &str, flat_magic: bool, flat_abilities: bool) -> Option<MenuKind> {
    Some(match label {
        ROOT_MAGIC_LIST if flat_magic => MenuKind::Magic,
        ROOT_MAGIC_LIST => MenuKind::MagicGroups,
        "Abilities" if flat_abilities => MenuKind::Abilities,
        "Abilities" => MenuKind::AbilitiesGroups,
        // Retail's Party row opens the prtyN window chosen by party state
        // (FFXiMain.dll retail-2026-09 RVA 0x2016E0; vault main_menu.md §24) —
        // one kuluu window whose rows follow that same state.
        "Party" => MenuKind::Party,
        // Retail's Trade row hands off to helper 0x13FE70, which refuses (error
        // sound) without an eligible target — kuluu keeps the window drill and
        // enforces that gate at confirm time (vault main_menu.md §25).
        "Trade" => MenuKind::Trade,
        // Retail's Search row opens `searchma` unconditionally from its menuwind
        // arm 0x140626 — no gate, just the scresult null-check on entry select
        // (FFXiMain.dll retail-2026-09; vault main_menu.md §26).
        "Search" => MenuKind::Search,
        "Linkshell" => MenuKind::Linkshell,
        // Page-1 row 11 name-opens `menu    region` ungated (FFXiMain.dll
        // retail-2026-09 RVA 0x140691; vault main_menu.md §29).
        "Region Info" => MenuKind::RegionInfo { mode: 0 },

        // Page-2 row 1 name-opens `menu    missionm` (FFXiMain.dll retail-2026-09
        // RVA 0x14E86D; vault main_menu.md §30).
        "Missions" => MenuKind::MissionList,

        // Page-2 row 5 name-opens `menu    bazaar` ungated (FFXiMain.dll retail-2026-09
        // RVA 0x14E8A1; vault main_menu.md §33): kuluu drills the stall picker its
        // data-driven reading of that browser window builds from entity flags.
        ROOT_SET_BAZAAR => MenuKind::BazaarStalls,
        // Page-2 row 6 is the ungated name-open of `menu    mcrmenu`
        // (FFXiMain.dll retail-2026-09 RVA 0x14E8BD; vault main_menu.md
        // §34): the root row drills that launcher. The row's caption is the
        // literal catalog string `MacroPalette` (§59 short-caption catalog),
        // verified in ffxi-dat real_selector_row_strings_in_display_order.
        ROOT_MACRO_PALETTE => MenuKind::MacrosPalette,
        // Page-2 row 7 is the ungated name-open of `menu    configwi`
        // (FFXiMain.dll retail-2026-09 RVA 0x14E8D9; vault main_menu.md
        // §35): the retail Config row drills that twelve-page hub. kuluu's
        // own settings pages stay separate as Interface/Controls (page 3).
        "Config" => MenuKind::RetailConfig,
        // Page-2 row 8 goes through the Help Desk trampoline: socialme item
        // id 8 (FFXiMain.dll retail-2026-09 RVA 0x14E9C0) calls 0x2174C0, which
        // name-opens `menu    faqsub` once its boot singleton exists; kuluu
        // drills the panel directly (vault main_menu.md §36).
        "Help Desk" => MenuKind::HelpDeskPanel,
        // Page-1 row 5 is the ungated name-open of `menu    cmbmenu` (FFXiMain.dll retail-2026-09
        // RVA 0x1405D2; vault main_menu.md §28): the root row drills into that launcher.
        "Synthesis" => MenuKind::SynthLauncher,
        "Items" => MenuKind::Items,
        // "Key Items" and "Quests" are absent on purpose: retail page 2 fires the
        // "/keyitem" and "/quest" text commands instead of drilling (vault
        // main_menu.md §5 row id 3, §12 runner, and §31 for the quest arm),
        // handled as TextCommands in `resolve_menu_entry`.
        "Equipment" => MenuKind::Equipment,
        "Status" => MenuKind::Status,
        ROOT_COMMUNICATION => MenuKind::Communication,
        "Graphics" => MenuKind::Graphics,
        // The page-2 `Config` row is retail's configwi window (not built yet);
        // kuluu's own settings pages live on page 3 as Interface/Controls.
        "Interface" => MenuKind::Config,
        "Controls" => MenuKind::Controls,
        "Debug" => MenuKind::Debug,
        _ => return None,
    })
}

/// Kinds that render their own framed multi-panel screen (item_screen /
/// equipment_screen) instead of the generic two-pane list, so the generic pane
/// is suppressed and its pane-toggle does not apply while one is on top.
pub fn renders_bespoke_screen(kind: MenuKind) -> bool {
    matches!(
        kind,
        MenuKind::Equipment
            | MenuKind::EquipSlot(_)
            | MenuKind::Items
            | MenuKind::UsableItems
            | MenuKind::Map
    )
}

#[derive(Component)]
pub struct MainMenu;

#[derive(Component)]
pub struct MainMenuTitle;

#[derive(Component)]
pub struct MainMenuRow {
    pub slot: usize,
}

/// Frame of the mirrored selector shown beside a companion window (ground
/// truth C:\briefs\companion_windows.md §1, from FFXiMain.dll retail-2026-09
/// OpenMenu RVA 0x15E350 with activate=1/exclusive=0: opening a child leaves
/// the selector open; only the topmost interactive window holds keys).
#[derive(Component)]
pub struct MainMenuCompanionFrame;

#[derive(Component)]
pub struct CompanionTitle;

#[derive(Component)]
pub struct CompanionRow {
    pub slot: usize,
}

/// One framed window of the pool that keeps the open levels under the focused one on screen at
/// their own record frames. Its rows lack Button, like the mirror's.
#[derive(Component)]
pub struct MenuStackWindow {
    pub slot: usize,
}

#[derive(Component)]
pub struct MenuStackWindowTitle {
    pub window: usize,
}

#[derive(Component)]
pub struct MenuStackWindowRow {
    pub window: usize,
    pub slot: usize,
}

/// More record windows than any menu chain keeps open under the focused one; past it the
/// oldest stay open undrawn.
pub const MAX_STACKED_WINDOWS: usize = 8;

#[derive(Message, Debug, Clone, Copy)]
pub struct MenuRowActivated {
    pub slot: usize,
}

/// The single Command-menu column width; the Map screen's wide-scan column
/// reuses it so the pane widths can't drift.
pub const MENU_PANE_WIDTH: f32 = 220.0;

/// Screen-space right anchor of the selector column (retail record x=384 in a
/// 512-wide design, i.e. flush right with a small margin).
const MENU_RIGHT_EDGE: f32 = 8.0;
/// Top of the selector column: the selector records (menuwind, statcom2, configwi) sit at y=48.
const MENU_COLUMN_TOP: f32 = 48.0;
/// Any window whose record sits left of this design x draws as a side window.
const LEFT_HALF_MAX_X: f32 = 256.0;

#[cfg(test)]
fn companion_record_frame(kind: MenuKind, frames: &ConfigDatRows) -> Option<(f32, f32, f32)> {
    frames.window_frame(record_window_name(kind)?)
}

/// Second-window law (bead kuluu-o9qt7.30): every menu window that owns a DAT record draws
/// at strictly its own record frame. The Status flow first proved it, then the whole Config
/// family (retail-2026-09 `cfilter` included once its duplicate-chunk collision stopped
/// shadowing the parseable copy; vault main_menu.md §59 addendum 12), and after the
/// install-wide census (`real_menu_window_frames_census_surfaces_every_record_rect`, 426
/// records) every remaining kind whose retail window owns a left-half record joins: each
/// record name below is that window's own placement data, never an invented placeholder.
/// Kinds with no owned record (or one sitting on the selector side) fall through to
/// kuluu's default column.
pub fn record_window_name(kind: MenuKind) -> Option<&'static str> {
    let name = match kind {
        MenuKind::RetailConfig => "configwi",
        MenuKind::RetailConfigPage(page) => RETAIL_CONFIG_PAGE_WINDOWS
            .get(usize::from(page))
            .copied()
            .unwrap_or(""),
        MenuKind::RetailConfigSub(sub) => RETAIL_CONFIG_SUB_WINDOWS
            .get(usize::from(sub))
            .copied()
            .unwrap_or(""),
        // `/quest` chains through exactly these two windows (vault main_menu.md §31).
        MenuKind::QuestList => "quest00",
        MenuKind::QuestDetail { .. } => "quest01",
        // `miss00` is the only mission-detail record — it serves every port (vault main_menu.md §30).
        MenuKind::MissionDetail { .. } => "miss00",
        // Retail opens its trade pair left-half; kuluu merges both windows into one
        // surface anchored on its own side's record (vault main_menu.md §25).
        MenuKind::Trade => "trade",
        // The Linkshell row always name-opens `menu    link5` (vault main_menu.md §27.3).
        MenuKind::Linkshell => "link5",
        // kuluu's Search surface lists the model results (§26 select law) = retail's
        // boot-instantiated `scresult` window. Its name lifecycle is owned solely by its own
        // search-session methods (.text FFXiMain.dll retail-2026-09 RVA 0x20BFC4 opens,
        // .text 0x20C02D closes with `scoption`) — no command-menu chain opens it, and the
        // draw is always from its own record (vault main_menu.md §59 addendum 18 corrected).
        MenuKind::Search => "scresult",
        MenuKind::FriendList => "friend",
        MenuKind::HomeView => MYROOM_WINDOW,
        MenuKind::JobChange => JOBCHANG_WINDOW,
        // The roomlist draws at its own record frame (16,208,w366) — companion law
        // (vault main_menu.md §59 addendum 20; bead kuluu-o9qt7.13.5).
        MenuKind::RoomList => ROOMLIST_DAT_WINDOW,
        MenuKind::CraftList => "cmbhlst",
        // DAT-side anchor for kuluu's collapsed macro line editor (vault main_menu.md §41
        // [I-shape]): the one surface stands in for retail's mcrbedit/mcrNedit family and
        // block N draws at `mcr{N}edit`'s own frame.
        MenuKind::MacroEditor { block: 0, .. } => "mcr1edit",
        MenuKind::MacroEditor { block: 1, .. } => "mcr2edit",
        kind if crate::hud::unity_fp::is_unity_fp_window(kind) => {
            return crate::hud::unity_fp::unity_fp_window_name(kind);
        }
        MenuKind::Merit(window) => window.record_name(),
        MenuKind::Arrange(window) => window.record_name(),
        _ => return None,
    };
    if name.is_empty() {
        return None;
    }
    Some(name)
}

/// kuluu's one Root level stands for both main menu page windows.
pub const MAIN_MENU_WINDOW: &str = "menuwind";
pub const STATUS_SELECTOR_WINDOW: &str = "statcom2";
pub const MAGIC_LIST_WINDOW: &str = "magic";
pub const ABILITY_LIST_WINDOW: &str = "ability";
pub const MOUNT_LIST_WINDOW: &str = "mount";
pub const EMOTE_LIST_WINDOW: &str = "emote";
/// The Mog House menu.
pub const MYROOM_WINDOW: &str = "myroom";
/// Mog House Change Jobs.
pub const JOBCHANG_WINDOW: &str = "jobchang";

/// The registry window kuluu draws for a menu level, where kuluu's window is one retail window.
/// kuluu-only menus and levels that stand for several retail windows have none.
pub fn registry_window(kind: MenuKind) -> Option<&'static str> {
    if let Some(name) = record_window_name(kind) {
        return Some(name);
    }
    Some(match kind {
        MenuKind::Root => MAIN_MENU_WINDOW,
        MenuKind::Status => STATUS_SELECTOR_WINDOW,
        MenuKind::StatusDetail { entry } => {
            return STATUS_DETAIL_WINDOW_NAMES
                .get(usize::from(entry))
                .copied()
                .flatten();
        }
        MenuKind::KeyItems => "evitem",
        MenuKind::UsableItems => "iuse",
        MenuKind::Items => crate::myroom_arrange::INVENTOR_WINDOW,
        MenuKind::Equipment => "equip",
        MenuKind::AutomatonEquip => "pupequip",
        MenuKind::MpCustomize => "mp_mncst",
        MenuKind::MagicGroups => "mgcmenu",
        MenuKind::MagicGroup(_) => MAGIC_LIST_WINDOW,
        MenuKind::AbilitiesGroups => "abimenu",
        MenuKind::AbilitiesGroup(group)
            if crate::hud::action_model::AbilityGroup::ALL.get(usize::from(group))
                == Some(&crate::hud::action_model::AbilityGroup::Mount) =>
        {
            MOUNT_LIST_WINDOW
        }
        MenuKind::AbilitiesGroup(_) => ABILITY_LIST_WINDOW,
        MenuKind::AbilitySort { .. } => "abisortw",
        MenuKind::EmoteList => EMOTE_LIST_WINDOW,
        MenuKind::RegionInfo { .. } => "region",
        MenuKind::MissionList => "missionm",
        MenuKind::BazaarStalls => "bazaar",
        MenuKind::MacrosPalette => "mcrmenu",
        MenuKind::MacroSelect => "mcrselec",
        MenuKind::HelpDeskPanel => "faqsub",
        MenuKind::SynthLauncher => "cmbmenu",
        MenuKind::Communication => "commenu",
        MenuKind::PowerPrompt { .. }
        | MenuKind::KeyAssignResetPrompt
        | MenuKind::RoomListConfirm { .. } => crate::myroom_arrange::COMYN_WINDOW,
        _ => return None,
    })
}

/// The cursor-at-open rule of each registry window kuluu draws
/// (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md, "Cursor at
/// open"). Display-only windows never take the keys and have none.
pub fn cursor_rule(window: &str) -> Option<OpenCursor> {
    use crate::hud::unity_fp::{
        AREALIST_WINDOW, FP_CAT2_WINDOW, FP_CAT_WINDOW, MERITYN_WINDOW, QUEST00_WINDOW,
        QUEST01_WINDOW, UT_MENU_WINDOW,
    };
    use crate::myroom_arrange::{COMYN_WINDOW, INVENTOR_WINDOW};
    const FIRST_ITEM: &[&str] = &[
        "evitem",
        "iuse",
        INVENTOR_WINDOW,
        "trade",
        "scresult",
        "link5",
        "miss00",
        QUEST00_WINDOW,
        QUEST01_WINDOW,
        AREALIST_WINDOW,
        FP_CAT_WINDOW,
        FP_CAT2_WINDOW,
        MERITYN_WINDOW,
        COMYN_WINDOW,
        "confyn",
        "textcol1",
        "conf11l",
        "conf11s",
        "friend",
        ROOMLIST_DAT_WINDOW,
        "cmbhlst",
    ];
    const REMEMBERED: &[&str] = &[
        MAIN_MENU_WINDOW,
        STATUS_SELECTOR_WINDOW,
        "mgcmenu",
        MAGIC_LIST_WINDOW,
        "abimenu",
        ABILITY_LIST_WINDOW,
        MOUNT_LIST_WINDOW,
        "abisortw",
        EMOTE_LIST_WINDOW,
        "equip",
        "pupequip",
        "mp_mncst",
        "region",
        "missionm",
        "bazaar",
        "mcrmenu",
        "mcrselec",
        "mcr1edit",
        "mcr2edit",
        RETAIL_CONFIG_HUB_WINDOW,
        "conf5win",
        "conf5w1",
        "conf5w2",
        "k1assign",
        "k2assign",
        "textcol2",
        "textcol3",
        "faqsub",
        UT_MENU_WINDOW,
        MYROOM_WINDOW,
        JOBCHANG_WINDOW,
        "cmbmenu",
        "commenu",
    ];
    if FIRST_ITEM.contains(&window) {
        Some(OpenCursor::FirstItem)
    } else if REMEMBERED.contains(&window) || RETAIL_CONFIG_PAGE_WINDOWS.contains(&window) {
        Some(OpenCursor::Remembered)
    } else {
        None
    }
}

/// The window a level is and the rule it opens its cursor by. The merit and arrange windows keep
/// the cursor laws their own records give them.
pub fn window_cursor(kind: MenuKind) -> Option<WindowCursor> {
    if matches!(kind, MenuKind::Merit(_) | MenuKind::Arrange(_)) {
        return None;
    }
    let window = registry_window(kind)?;
    Some(WindowCursor {
        window,
        rule: cursor_rule(window)?,
    })
}

/// How many rows a window not built from live data lists; `None` for a built one.
pub fn fixed_item_count(kind: MenuKind) -> Option<usize> {
    (!is_dynamic(kind)).then(|| static_entries(kind).len())
}

/// The main menu page that is kuluu's own, not one of the two retail page windows.
const KULUU_ROOT_PAGE: usize = ROOT_PAGE_COUNT - 1;

/// What a remembered window leaves in the memory as it closes: `None` leaves the memory alone,
/// which is the main menu closing on kuluu's own page; a window closing on its unselectable item
/// remembers nothing.
pub fn remembered_close_cursor(level: &MenuLevel) -> Option<Option<usize>> {
    if level.kind == MenuKind::Root && root_page_index(level.cursor) == KULUU_ROOT_PAGE {
        return None;
    }
    Some((!level.cursor_locked()).then_some(level.cursor))
}

/// The registry windows that make their last DAT item unselectable when created
/// (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md, "Last item
/// unselectable at creation").
pub const LAST_ITEM_UNSELECTABLE_WINDOWS: &[&str] = &[
    "statcom", "statcom2", "ut_menu", "mgcmenu", "abimenu", "abiselec", "mp_abisc", "iuse",
    "prty1", "prty2", "prty3", "prty4", "prty5", "prty5us", "prty5fr", "prty5de", "prty7", "prty8",
    "link1", "link2", "link3", "link4", "link7", "link8", "link9", "link10", "link13", "searchma",
    "scoption", "acoption", "scsibori", "searchjo", "searchle", "searchna", "searchra", "comgenre",
    "sccompar", "sccomite", "sccomls", "commenu", "flistmai", "mes1rcv", "mes2frnd", "blkmain",
    "map0", "mapv2", "mapv3", "mapscan", "auc1", "auc2", "auc3", "auc4", "aucweapo", "aucammo",
    "aucarmor", "aucmagic", "aucmater", "aucfood", "aucmeals", "aucitem", "wepsort", "armsort",
    "mcrmenu", "mcrselec", "mcrselop", "mcres20", "mcresmn", "configwi", "conf5m", "conf11m",
    "conf11l", "sortyn", "bluesibo", "statpup",
];

/// Whether a level is one of those windows whose rows kuluu builds from the window's own record,
/// so the record can say which row the last item is; for the others the record does not
/// establish the DAT item count.
pub fn locks_last_item(kind: MenuKind) -> bool {
    matches!(
        kind,
        MenuKind::Status
            | MenuKind::UnityMenu
            | MenuKind::RetailConfig
            | MenuKind::RetailConfigPage(_)
            | MenuKind::RetailConfigSub(_)
    ) && registry_window(kind)
        .is_some_and(|window| LAST_ITEM_UNSELECTABLE_WINDOWS.contains(&window))
}

/// Which row a window's last DAT item is, read from its own record; `None` while the record has
/// not loaded. The Status selector's last item is a trailing dummy block, not a row. Windows whose
/// rows kuluu does not build from their records stay unresolved.
pub fn last_item_row(
    kind: MenuKind,
    config_rows: &ConfigDatRows,
    unity: &crate::hud::unity_fp::UnityFpDat,
) -> Option<LastItemRow> {
    match kind {
        MenuKind::Status => Some(LastItemRow::Undrawn),
        MenuKind::UnityMenu => unity.menu_last_item_row(),
        MenuKind::RetailConfig | MenuKind::RetailConfigPage(_) | MenuKind::RetailConfigSub(_) => {
            config_rows.last_item_row(registry_window(kind)?)
        }
        _ => None,
    }
}

/// The frame a kind draws at when its record sits left of the selector column. Selector
/// records (x=384, pinned to the right edge) draw in kuluu's selector column instead.
fn side_record_frame(kind: MenuKind, frames: &ConfigDatRows) -> Option<RecordFrame> {
    let frame = frames.record_frame(record_window_name(kind)?)?;
    (frame.x < LEFT_HALF_MAX_X).then_some(frame)
}

/// A window that docks keeps its record x wherever its record sits
/// (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md).
fn docked_record_frame(kind: MenuKind, frames: &ConfigDatRows) -> Option<RecordFrame> {
    let window = registry_window(kind)?;
    crate::hud::dock::registry_dock_slot(window)?;
    frames.record_frame(window)
}

/// Where the dock law put a level's window, once its registry window docks and has been placed.
pub fn dock_placement(
    kind: MenuKind,
    board: &crate::hud::dock::DockBoard,
) -> Option<crate::hud::dock::DockPlacement> {
    let window = registry_window(kind)?;
    crate::hud::dock::registry_dock_slot(window)?;
    board.placement(window)
}

/// statcom2 sits exactly over the main menu's page 1 and configwi over its page 2, so either one
/// hides the main menu under it.
fn covers_main_menu(kind: MenuKind) -> bool {
    matches!(kind, MenuKind::Status | MenuKind::RetailConfig)
}

/// Where one open window of the menu stack draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MenuWindowSeat {
    /// At its own left-side record frame.
    Record(RecordFrame),
    /// Kuluu's selector column on the right edge.
    Column,
    /// Left of the selector column, where kuluu keeps a focused selector's opener framed.
    BesideColumn,
}

/// Every window an open menu stack draws as (stack index, seat), oldest first with the focused
/// window last.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MenuStackLayout {
    pub windows: Vec<(usize, MenuWindowSeat)>,
}

impl MenuStackLayout {
    pub fn focused(&self) -> Option<(usize, MenuWindowSeat)> {
        self.windows.last().copied()
    }

    /// The window under the focused one drawn in or beside the selector column.
    pub fn column_window(&self) -> Option<(usize, MenuWindowSeat)> {
        let (_, below) = self.windows.split_last()?;
        below
            .iter()
            .copied()
            .find(|(_, seat)| !matches!(seat, MenuWindowSeat::Record(_)))
    }

    /// The windows under the focused one drawn at their own record frames, oldest first.
    pub fn record_windows(&self) -> Vec<(usize, RecordFrame)> {
        let below = self
            .windows
            .split_last()
            .map_or(&[][..], |(_, below)| below);
        below
            .iter()
            .filter_map(|&(index, seat)| match seat {
                MenuWindowSeat::Record(frame) => Some((index, frame)),
                _ => None,
            })
            .collect()
    }
}

/// Every open window stays on screen at its own record frame, oldest first and newest on top,
/// and a window sitting on an older one's whole rectangle hides it
/// (.agents/skills/retail-observe/references/2026-10-15-menu-window-stacking.md). A docked window
/// sits at its record frame wherever that lies; any other window
/// without a left-side record keeps kuluu's selector column: focused, it draws there with its
/// opener framed beside it; under a focused record window, the newest such window holds the
/// column. statcom2 and configwi cover the main menu, so nothing sits beside Status or the Config
/// hub.
pub fn menu_stack_layout(stack: &MenuStack, frames: &ConfigDatRows) -> MenuStackLayout {
    let levels = &stack.levels;
    // Status details draw in their own window beside persona (`update_status_detail_window`),
    // and the windows that draw themselves do so in `unity_fp_view`.
    let Some(focused) = levels
        .iter()
        .rposition(|level| {
            !matches!(level.kind, MenuKind::StatusDetail { .. }) && !draws_itself(level.kind)
        })
        .or_else(|| levels.len().checked_sub(1))
    else {
        return MenuStackLayout::default();
    };
    let side = |index: usize| {
        side_record_frame(levels[index].kind, frames)
            .or_else(|| docked_record_frame(levels[index].kind, frames))
    };
    let focused_frame = side(focused);
    let records: Vec<(usize, RecordFrame)> = (0..focused)
        .filter_map(|index| side(index).map(|frame| (index, frame)))
        .collect();
    let mut windows: Vec<(usize, MenuWindowSeat)> = records
        .iter()
        .enumerate()
        .filter(|&(at, (_, frame))| {
            !records[at + 1..]
                .iter()
                .map(|(_, newer)| newer)
                .chain(focused_frame.as_ref())
                .any(|newer| newer.covers(frame))
        })
        .map(|(_, &(index, frame))| (index, MenuWindowSeat::Record(frame)))
        .collect();
    let column = if covers_main_menu(levels[focused].kind) {
        None
    } else if focused_frame.is_some() {
        (0..focused)
            .rev()
            .find(|&index| side(index).is_none())
            .map(|index| (index, MenuWindowSeat::Column))
    } else {
        focused
            .checked_sub(1)
            .filter(|&index| side(index).is_none())
            .map(|index| (index, MenuWindowSeat::BesideColumn))
    };
    if let Some(window) = column.filter(|&(index, _)| !renders_bespoke_screen(levels[index].kind)) {
        windows.push(window);
        windows.sort_by_key(|&(index, _)| index);
    }
    windows.push((
        focused,
        focused_frame.map_or(MenuWindowSeat::Column, MenuWindowSeat::Record),
    ));
    MenuStackLayout { windows }
}

/// Wider pane for the Graphics menu: its rows are `label:<16>[value]` pairs
/// (longest: `> Texture Filtering:[Aniso 16x]`, 31 chars) that overflow the
/// 220px command column at 14px and used to wrap, doubling row heights and
/// pushing the pane past the window bottom.
pub const GRAPHICS_PANE_WIDTH: f32 = 320.0;

fn pane_width_for(kind: MenuKind) -> f32 {
    match kind {
        MenuKind::Config | MenuKind::Graphics | MenuKind::GraphicsDlss => GRAPHICS_PANE_WIDTH,
        _ => MENU_PANE_WIDTH,
    }
}

/// Single-line rows: a wrapped row silently occupies two lines, which breaks
/// the `DYNAMIC_VISIBLE_ROWS` viewport math (22 logical rows must be 22 drawn
/// lines). Truncate instead of wrapping if a row ever outgrows its pane.
fn text_nowrap() -> TextLayout {
    TextLayout {
        linebreak: LineBreak::NoWrap,
        ..default()
    }
}

/// Marker for the framed pane column so `update_main_menu` can retarget its
/// width per menu kind.
#[derive(Component)]
pub struct MainMenuPane;

pub fn spawn_main_menu(mut commands: Commands) {
    // Row order [companion, selector] mirrors retail placement: selectors sit
    // at design x=384 anchor-right and companion windows at x<384 (measured
    // DAT frames; vault main_menu.md §16 mini-chunks, §7 field law).
    commands
        .spawn((
            crate::components::InGameEntity,
            MainMenu,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                display: Display::None,
                ..default()
            },
            bevy::picking::Pickable::IGNORE,
        ))
        .with_children(|root| {
            spawn_companion_frame(root);
            for slot in 0..MAX_STACKED_WINDOWS {
                spawn_stacked_window(root, slot);
            }
            spawn_pane_column(root);
        });
}

fn spawn_stacked_window(parent: &mut ChildSpawnerCommands, slot: usize) {
    let (mut node, bg, border) = style::window_frame();
    node.position_type = PositionType::Absolute;
    node.display = Display::None;
    parent
        .spawn((MenuStackWindow { slot }, node, bg, border))
        .with_children(|w| {
            w.spawn((
                MenuStackWindowTitle { window: slot },
                Text::new(""),
                text_nowrap(),
                style::text_font(14.0),
                TextColor(theme::TITLE),
            ));
            for row in 0..DYNAMIC_VISIBLE_ROWS {
                w.spawn((
                    MenuStackWindowRow {
                        window: slot,
                        slot: row,
                    },
                    Node {
                        display: Display::None,
                        ..default()
                    },
                    Text::new(""),
                    text_nowrap(),
                    style::text_font(14.0),
                    TextColor(theme::TEXT),
                ));
            }
        });
}

/// The read-only mirror of the level under focus - a second framed window, so
/// a companion never appears to replace its selector (vault main_menu.md §59).
fn spawn_companion_frame(parent: &mut ChildSpawnerCommands) {
    let (mut node, bg, border) = style::window_frame();
    node.position_type = PositionType::Absolute;
    node.top = Val::Px(MENU_COLUMN_TOP);
    node.right = Val::Px(MENU_RIGHT_EDGE + MENU_PANE_WIDTH);
    node.width = Val::Px(MENU_PANE_WIDTH);
    node.display = Display::None;
    parent
        .spawn((MainMenuCompanionFrame, node, bg, border))
        .with_children(|c| {
            // Mirror nodes carry the same MainMenu title/row components as the
            // focused pane (one query fills both; the CompanionTitle marker
            // says which is which). They deliberately lack Button: only the
            // topmost interactive window takes mouse or keys (vault main_menu.md §59).
            c.spawn((
                MainMenuTitle,
                CompanionTitle,
                Text::new(""),
                text_nowrap(),
                style::text_font(14.0),
                TextColor(theme::TITLE),
            ));
            for slot in 0..MAX_ENTRY_COUNT {
                c.spawn((
                    MainMenuRow { slot },
                    CompanionRow { slot },
                    Text::new(""),
                    text_nowrap(),
                    style::text_font(14.0),
                    TextColor(theme::TEXT),
                    BackgroundColor(Color::NONE),
                ));
            }
        });
}

fn spawn_pane_column(parent: &mut ChildSpawnerCommands) {
    let (mut node, bg, border) = style::window_frame();
    node.position_type = PositionType::Absolute;
    node.top = Val::Px(MENU_COLUMN_TOP);
    node.right = Val::Px(MENU_RIGHT_EDGE);
    node.width = Val::Px(MENU_PANE_WIDTH);
    parent
        .spawn((MainMenuPane, node, bg, border))
        .with_children(|c| {
            c.spawn((
                MainMenuTitle,
                Text::new(""),
                text_nowrap(),
                style::text_font(14.0),
                TextColor(theme::TITLE),
            ));

            for slot in 0..MAX_ENTRY_COUNT {
                c.spawn((
                    MainMenuRow { slot },
                    Button,
                    Text::new(""),
                    text_nowrap(),
                    style::text_font(14.0),
                    TextColor(theme::TEXT),
                    BackgroundColor(Color::NONE),
                ));
            }
        });
}

/// Spell-school lookup for the Magic category filter. Retail buckets spells by
/// the spell DAT's magicType - the same numbers mgcmenu filters on (vault
/// main_menu.md §21); with no install loaded the scraped LSB `group` column
/// stands in, mapped onto those numbers.
#[derive(Resource, Default)]
pub struct MagicSchoolTable {
    loaded: bool,
    table: ffxi_dat::spell_info::SpellTable,
}

impl MagicSchoolTable {
    fn ensure_loaded(&mut self, root: Option<&ffxi_dat::DatRoot>) {
        if !self.loaded {
            if let Some(root) = root {
                self.table = ffxi_dat::spell_info::SpellTable::open_from_root(root);
                self.loaded = true;
            }
        }
    }

    fn school_of(&self, spell_id: u16) -> Option<u8> {
        if self.loaded {
            if let Some(school) = self
                .table
                .lookup(spell_id)
                .and_then(|s| s.magic_type.bucket_id())
            {
                return Some(school);
            }
        }
        ffxi_vocab::magic::group(u32::from(spell_id)).and_then(lsb_group_to_school)
    }
}

/// LSB xi.magic.spellGroup numbers onto the client school numbers of §21: the
/// two enums are bijective but shuffled (SONG 1→5, SUMMONING 5→3, WHITE 6→1,
/// BLUE 3→6; BLACK/NINJUTSU/GEOMANCY/TRUST keep their number).
fn lsb_group_to_school(group: u8) -> Option<u8> {
    use ffxi_vocab::magic as g;
    match group {
        g::GROUP_WHITE => Some(MAGIC_CAT_WHITE),
        g::GROUP_BLACK => Some(MAGIC_CAT_BLACK),
        g::GROUP_SUMMONING => Some(MAGIC_CAT_SUMMONING),
        g::GROUP_NINJUTSU => Some(MAGIC_CAT_NINJUTSU),
        g::GROUP_SONG => Some(MAGIC_CAT_SONGS),
        g::GROUP_BLUE => Some(MAGIC_CAT_BLUE),
        g::GROUP_GEOMANCY => Some(MAGIC_CAT_GEOMANCY),
        g::GROUP_TRUST => Some(MAGIC_CAT_TRUST),
        _ => None,
    }
}

/// The `magic` window's unfiltered list — kuluu's flat view of it, reached with
/// the Retail+ toggle (retail always drills a category first, vault
/// main_menu.md §21): every spell the snapshot reports as known.
pub fn spell_rows(spells_known: &[u16]) -> Vec<DynamicMenuRow> {
    spells_known
        .iter()
        .filter_map(|&id| {
            ffxi_vocab::spell_names::lookup(id).map(|name| DynamicMenuRow {
                tint: None,
                label: name.to_string(),
                action: DynamicMenuAction::CastSpell { spell_id: id },
            })
        })
        .collect()
}

/// The `magic` spell list for one school: known spells whose bucket matches.
pub fn magic_school_rows(
    spells_known: &[u16],
    school: u8,
    school_of: impl Fn(u16) -> Option<u8>,
) -> Vec<DynamicMenuRow> {
    spells_known
        .iter()
        .filter(|&&id| school_of(id) == Some(school))
        .filter_map(|&id| {
            ffxi_vocab::spell_names::lookup(id).map(|name| DynamicMenuRow {
                tint: None,
                label: name.to_string(),
                action: DynamicMenuAction::CastSpell { spell_id: id },
            })
        })
        .collect()
}

/// Region Info row captions. The `region` window is chrome-only — no string
/// hits for its buttons in DLL or DAT (vault main_menu.md §29) — so kuluu pins
/// these English mode names by test; naming informed by retail zoom semantics,
/// caption itself [I].
pub const REGION_MODE_LABELS_PINNED: [&str; 4] = ["World", "Nation", "Region", "Zone"];

/// Notice worn by the World row when the installed mode is 0 but the current
/// zone fails FFXiMain's predicate — kuluu's stand-in for that path's message
/// id 0x7B post (vault main_menu.md §29); retail caption text [I].
pub const REGION_UNAVAILABLE_NOTICE_PINNED: &str = "(unavailable)";

/// Zones accepted by the mode-install predicate: verbatim first-word copy of
/// the stride-8 table FFXiMain.dll retail-2026-09 RVA 0x1E6CA0 scans at RVA
/// 0x37F632 (vault main_menu.md §29), both trailing zero words kept as found.
pub const REGION_PRED_ZONES_PINNED: [u16; 146] = [
    0x00E6, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF, 0x00F0, 0x00F1,
    0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x0064, 0x0065, 0x008C, 0x008D, 0x008E, 0x008B, 0x00BE,
    0x00A7, 0x0067, 0x006C, 0x00C4, 0x0066, 0x00C1, 0x00F8, 0x0068, 0x0069, 0x00C3, 0x0095, 0x0096,
    0x0001, 0x0002, 0x006A, 0x006B, 0x008F, 0x0090, 0x00BF, 0x00AC, 0x00AD, 0x006D, 0x006E, 0x00C5,
    0x0093, 0x0094, 0x0073, 0x0074, 0x0091, 0x0092, 0x00A9, 0x00C0, 0x00C2, 0x00AA, 0x0076, 0x0075,
    0x00C6, 0x00F9, 0x00D5, 0x0003, 0x0004, 0x0077, 0x0078, 0x00C8, 0x0097, 0x0098, 0x0007, 0x0008,
    0x006F, 0x00A6, 0x00CC, 0x00CE, 0x00CB, 0x0009, 0x000A, 0x0070, 0x00A1, 0x00A2, 0x00A5, 0x0005,
    0x0006, 0x007E, 0x007F, 0x009D, 0x009E, 0x00B8, 0x00B3, 0x0079, 0x007A, 0x00FB, 0x0099, 0x009A,
    0x00CA, 0x0072, 0x007D, 0x00F7, 0x00D0, 0x00A8, 0x00D1, 0x0071, 0x00AE, 0x0080, 0x00D4, 0x00C9,
    0x007B, 0x00B0, 0x00FA, 0x00FC, 0x007C, 0x009F, 0x00A0, 0x00A3, 0x00CD, 0x00CF, 0x00D3, 0x0082,
    0x00B1, 0x00B2, 0x00B4, 0x00B5, 0x000B, 0x000C, 0x000D, 0x0018, 0x0019, 0x0020, 0x001B, 0x001C,
    0x001D, 0x001E, 0x001F, 0x00DF, 0x00E0, 0x00E1, 0x00DC, 0x00DD, 0x00E3, 0x00E4, 0x00E2, 0x001A,
    0x0000, 0x0000,
];

/// The mode-install predicate's first-word test (vault main_menu.md §29).
fn region_zone_known(zone: u16) -> bool {
    REGION_PRED_ZONES_PINNED.contains(&zone)
}

/// The four Region Info rows in handler item order (FFXiMain.dll retail-2026-09
/// RVA 0x140818; vault main_menu.md §29): the marker rides the installed mode,
/// and while mode 0 is installed over a zone outside FFXiMain's predicate table
/// the World row wears the unavailable notice. Four rows regardless of state —
/// the window answers only items 1..4.
pub fn region_info_rows(mode: u8, zone: Option<u16>) -> Vec<DynamicMenuRow> {
    REGION_MODE_LABELS_PINNED
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let mut label = (*label).to_string();
            if i == 0 && mode == 0 && !zone.is_some_and(region_zone_known) {
                label.push(' ');
                label.push_str(REGION_UNAVAILABLE_NOTICE_PINNED);
            }
            let marker = if u8::try_from(i).is_ok_and(|m| m == mode) {
                "\u{bb} "
            } else {
                ""
            };
            DynamicMenuRow {
                tint: None,
                label: format!("{marker}{label}"),
                action: DynamicMenuAction::RegionInfoMode { mode: i as u8 },
            }
        })
        .collect()
}

/// Nation names in LSB's MissionLog enum order (`Sandoria`/`Bastok`/`Windurst`,
/// vendor/server/src/map/enums/mission_log.h).
pub const MISSION_NATION_NAMES_PINNED: [&str; 3] = ["Sandoria", "Bastok", "Windurst"];

/// The complete-mission ports named by `MissionComplete` (vendor/server/src/map/
/// enums/mission_log.h) — the only OTHER ports the Missions surface lists; quest
/// ports belong to the "/quest" seam (vault main_menu.md §30).
pub const MISSION_COMPLETE_PORTS_PINNED: [(u16, &str); 4] = [
    (0x00D0, "Nations"),
    (0x00D8, "ToAU/WoTG"),
    (0x0030, "Campaign I"),
    (0x0038, "Campaign II"),
];

pub fn mission_nation_name(nation: u32) -> String {
    match MISSION_NATION_NAMES_PINNED.get(nation as usize) {
        Some(name) => (*name).to_string(),
        None => format!("Nation #{nation}"),
    }
}

/// The `missionm` rows (vault main_menu.md §30): the chrome carries no captions,
/// and kuluu lists exactly the logs its s2c 0x056 state holds — the nation log
/// line plus each complete-mission port with data. Select mirrors the handler's
/// tail name-open of miss00 by pushing that port's detail.
pub fn mission_list_rows(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<DynamicMenuRow> {
    let mut rows = Vec::new();
    if let Some(info) = &snap.missions.info {
        rows.push(DynamicMenuRow {
            tint: None,
            label: format!(
                "{}: mission #{}",
                mission_nation_name(info.nation),
                info.nation_mission
            ),
            action: DynamicMenuAction::MissionOpenLog {
                port: ffxi_proto::mission::PORT_MISSION_MAIN,
            },
        });
    }
    for entry in snap
        .missions
        .logs
        .iter()
        .filter(|e| e.data.iter().any(|&w| w != 0))
    {
        if let Some((port, name)) = MISSION_COMPLETE_PORTS_PINNED
            .iter()
            .find(|(p, _)| *p == entry.port)
        {
            rows.push(DynamicMenuRow {
                tint: None,
                label: format!("Completed missions — {name}"),
                action: DynamicMenuAction::MissionOpenLog { port: *port },
            });
        }
    }
    if rows.is_empty() {
        rows.push(DynamicMenuRow {
            tint: None,
            label: "(no mission logs reported)".to_string(),
            action: DynamicMenuAction::MissionLogRow { port: 0 },
        });
    }
    rows
}

/// The Equipment row's variant law (FFXiMain.dll retail-2026-09 RVA 0x1404D5;
/// vault main_menu.md §39): the Monstrosity predicate comes first (call 0x97D50
/// → helper 0x141BA0 opens `mp_mncst`), then — when the puppet flag byte has
/// bit 0 set and either job slot is PUP (`mov al, 0x12`, RVA 0x1404FB) — a
/// name-open of `pupequip`; otherwise the ordinary `equip` window.
///
/// kuluu's stand for both client flags is data presence: LSB only ever sends
/// s2c 0x044 MON while a monstrosity session is active and s2c 0x044 PUP only
/// for characters whose main or sub job is PUP (SendExtendedJobPackets,
/// vendor/server/src/map/utils/charutils.cpp), so `monstrosity.is_some()` /
/// `automaton.is_some()` mirror the gates exactly without a job byte of their
/// own on the snapshot.
pub fn equipment_drill_kind(snap: &kuluu_snapshot::SceneSnapshot) -> MenuKind {
    if snap.monstrosity.is_some() {
        return MenuKind::MpCustomize;
    }
    if snap.automaton.is_some() {
        return MenuKind::AutomatonEquip;
    }
    MenuKind::Equipment
}

/// The Map row's variant law (FFXiMain.dll retail-2026-09 RVA 0x140653; vault
/// main_menu.md §40): when the page-1 window object field +0x14 reads 1 and
/// predicate FFXiMain.dll retail-2026-09 RVA 0xF0C50 is true — bit 8 of the
/// game-state dword at [gameStateObj+0x3c35c] — the row name-opens `myroom`
/// instead of driving the mapframe family (call to 0x1F7810(0)).
///
/// kuluu's stand for both conditions is data presence: LSB marks the login
/// packet MYROOM whenever `PChar->inMogHouse()` holds and every zone change
/// re-authenticates through 0x00A (vendor/server/src/map/packets/s2c/
/// 0x00a_login.cpp), so `snap.myroom.is_some()` mirrors the gate exactly.
pub fn map_drill_kind(snap: &kuluu_snapshot::SceneSnapshot) -> MenuKind {
    if snap.myroom.is_some() {
        return MenuKind::HomeView;
    }
    MenuKind::Map
}

/// Name vocabulary for the packet's AutomatonHead byte (0 = nothing equipped);
/// verbatim from vendor/server/src/map/enums/automaton.h.
pub fn automaton_head_name(code: u8) -> Option<&'static str> {
    match code {
        1 => Some("Harlequin"),
        2 => Some("Valoredge"),
        3 => Some("Sharpshot"),
        4 => Some("Stormwaker"),
        5 => Some("Soulsoother"),
        6 => Some("Spiritreaver"),
        _ => None,
    }
}

/// Name vocabulary for the packet's AutomatonFrame byte; verbatim from the same
/// header (values 0x20..=0x23).
pub fn automaton_frame_name(code: u8) -> Option<&'static str> {
    match code {
        0x20 => Some("Harlequin"),
        0x21 => Some("Valoredge"),
        0x22 => Some("Sharpshot"),
        0x23 => Some("Stormwaker"),
        _ => None,
    }
}

/// The pupequip variant's fourteen rows: Head, Frame, then Attachment 1..12 in
/// AutomatonSlot order (Head=0, Frame=1, Attachment1..12 = 2..13 — vendor/
/// server/src/map/packets/c2s/0x102_extended_job.h; vault main_menu.md §39).
/// The DAT `pupe` record is chrome-only, so the slot words are kuluu pins held
/// by tests; equipped names come from the s2c 0x044 PUP frame (enum vocabulary
/// for head/frame, item names for attachments — empty slots read "(empty)" like
/// the trade grid's cells). Selecting is inert until §39's equip flow lands.
pub fn automaton_equip_rows(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<DynamicMenuRow> {
    let Some(automaton) = snap.automaton.as_deref() else {
        return Vec::new();
    };
    let named = |word: &str, name: Option<&'static str>, slot: u8| DynamicMenuRow {
        tint: None,
        label: format!("{word}: {}", name.unwrap_or("(empty)")),
        action: DynamicMenuAction::AutomatonSlotRow { slot },
    };
    let mut rows = vec![
        named("Head", automaton_head_name(automaton.head), 0),
        named("Frame", automaton_frame_name(automaton.frame), 1),
    ];
    for (i, &item) in automaton.attachments.iter().enumerate() {
        let name = if item == 0 {
            None
        } else {
            ffxi_vocab::item_names::lookup(u16::from(item))
        };
        rows.push(named(
            &format!("Attachment {}", i + 1),
            name,
            u8::try_from(i + 2).unwrap_or(13),
        ));
    }
    rows
}

/// The DAT-grounded macro capacity: two palette windows (`mcr1pall`/`mcr2pall`,
/// toggled by the helper family FFXiMain.dll retail-2026-09 RVA 0x1A44DE..) and
/// twenty keytop widgets per grid chunk — 'mcrs' (ROM/0/1.DAT file-off 0x2A3CC0,
/// root mcrselec) and 'mcr7' (@0x2A8560, root mcres20) each hold exactly twenty
/// `menu    keytops3-1` children (vault main_menu.md §41).
pub const MACRO_BLOCKS: usize = 2;
pub const MACRO_SLOTS_PER_BLOCK: usize = 20;

/// The mcrselec variant's rows: every filled slot of the client-local store in
/// (block, index) order. Retail renders empty keytops as blank cells; kuluu
/// lists only filled slots, so a fresh character sees the same nothing the
/// retail grid shows (vault main_menu.md §41). Selecting is inert [I] until
/// the macro-use seam lands.
pub fn macro_select_rows(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<DynamicMenuRow> {
    let mut slots = snap.macros.clone();
    slots.sort_by_key(|m| (m.block, m.index));
    slots
        .into_iter()
        .map(|m| DynamicMenuRow {
            tint: None,
            label: format!("B{}/{}: {}", m.block + 1, m.index + 1, m.text),
            action: DynamicMenuAction::MacroSlotRow {
                block: m.block,
                index: m.index,
            },
        })
        .collect()
}

/// Rows of kuluu's Bazaar stall picker: every PC in the zone whose
/// `Flags1.BazaarFlag` is set — LSB pushes it on the entity stream as
/// `hasBazaar()` (`char_update.cpp`; vault main_menu.md §33). Self never
/// browses its own stall from here. Labels are kuluu pins (the retail window
/// is chrome-only, §33); selecting sends c2s 0x105 for that seller.
pub fn bazaar_stall_rows(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<DynamicMenuRow> {
    let mut rows: Vec<DynamicMenuRow> = snap
        .entities
        .iter()
        .filter(|e| is_bazaar_stall_entity(e, snap.self_char_id))
        .map(|e| DynamicMenuRow {
            tint: None,
            label: format!("{}'s Bazaar", e.name.as_deref().unwrap_or_default()),
            action: DynamicMenuAction::BazaarStall { char_id: e.id },
        })
        .collect();
    if rows.is_empty() {
        rows.push(DynamicMenuRow {
            tint: None,
            label: "(no player bazaars open nearby)".to_string(),
            action: DynamicMenuAction::BazaarStall { char_id: u32::MAX },
        });
    }
    rows
}

/// The §33 stall predicate on one entity; the hint-row sentinel id never
/// matches, so those rows stay inert.
pub fn is_bazaar_stall_entity(e: &kuluu_snapshot::Entity, self_char_id: Option<u32>) -> bool {
    e.kind == kuluu_snapshot::EntityKind::Pc
        && e.char_flags.bazaar
        && Some(e.id) != self_char_id
        && (e.id as u64) < u32::MAX as u64
}

/// The `miss00` detail lines for one port (vault main_menu.md §30): the field
/// dump legible, every row inert while that window's paging semantics stay
/// [I-pending]. Labels are kuluu pins (§22/§30 chrome-only law).
pub fn mission_detail_rows(snap: &kuluu_snapshot::SceneSnapshot, port: u16) -> Vec<DynamicMenuRow> {
    let mut rows = Vec::new();
    if port == ffxi_proto::mission::PORT_MISSION_MAIN {
        if let Some(i) = &snap.missions.info {
            rows.push(format!(
                "{}: mission #{}",
                mission_nation_name(i.nation),
                i.nation_mission
            ));
            // Field-named lines: the LSB field names are [V] (the packet header); retail
            // expansion captions would be invented chrome ([I]) so kuluu keeps them out.
            for (name, value) in [
                ("RoZ", i.rotz),
                ("CoP", i.cop),
                ("ACP", u32::from(i.addon_acp)),
                ("AMK", u32::from(i.addon_amk)),
                ("ASA", u32::from(i.addon_asa)),
                ("SoA", i.soa),
                ("RoV", i.rov),
            ] {
                if value != 0 {
                    rows.push(format!("{name} progress #{value}"));
                }
            }
        }
    } else if let Some(entry) = snap.missions.logs.iter().find(|e| e.port == port) {
        for (i, w) in entry.data.iter().enumerate() {
            if *w != 0 {
                rows.push(format!("word[{i}] = {w:#010X}"));
            }
        }
    }
    let mut rows: Vec<DynamicMenuRow> = rows
        .into_iter()
        .map(|label| DynamicMenuRow {
            tint: None,
            label,
            action: DynamicMenuAction::MissionLogRow { port },
        })
        .collect();
    if rows.is_empty() {
        rows.push(DynamicMenuRow {
            tint: None,
            label: "(nothing recorded)".to_string(),
            action: DynamicMenuAction::MissionLogRow { port },
        });
    }
    rows
}

/// The `quest00` rows (vault main_menu.md §31): the chrome carries no captions,
/// and kuluu lists exactly the quest bits its s2c 0x056 quest-port state holds —
/// one row per `(log, id)` in log-then-id order, labelled from the scraped
/// `scripts/globals/quests.lua` lattice. A bit present in both a QuestOffer and
/// its QuestComplete port shows as complete (the window carries one row per
/// quest; which report wins is state order, filter semantics [I-pending]).
/// Select mirrors quest00's select tail name-open of quest01 by pushing detail.
pub fn quest_list_rows(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<DynamicMenuRow> {
    let mut statuses: std::collections::BTreeMap<(u8, u16), bool> = Default::default();
    for entry in &snap.missions.logs {
        if let Some((log, complete)) = ffxi_proto::mission::quest_port(entry.port) {
            for id in ffxi_proto::mission::quest_ids(&entry.data) {
                *statuses.entry((log, id)).or_default() |= complete;
            }
        }
    }
    let mut rows: Vec<DynamicMenuRow> = statuses
        .iter()
        .map(|(&(log, id), &complete)| DynamicMenuRow {
            tint: None,
            label: match quest_caption(log, id) {
                Some(name) if complete => format!("{name} (complete)"),
                Some(name) => name.to_string(),
                None if complete => format!("Quest #{id} (complete)"),
                None => format!("Quest #{id}"),
            },
            action: DynamicMenuAction::QuestOpenDetail { log, id },
        })
        .collect();
    if rows.is_empty() {
        rows.push(DynamicMenuRow {
            tint: None,
            label: "(no quests reported)".to_string(),
            action: DynamicMenuAction::QuestDetailRow { log: 0, id: 0 },
        });
    }
    rows
}

fn quest_caption(log: u8, id: u16) -> Option<&'static str> {
    ffxi_vocab::quest_lattice::quest_name(log, id)
}

/// The `quest01` detail lines for one quest bit (vault main_menu.md §31): the
/// field dump legible, every row inert while that window's paging law stays
/// [I-pending]. Labels are kuluu pins (§22/§31 chrome-only law).
pub fn quest_detail_rows(
    snap: &kuluu_snapshot::SceneSnapshot,
    log: u8,
    id: u16,
) -> Vec<DynamicMenuRow> {
    let mut bits = (false, false);
    for entry in &snap.missions.logs {
        if let Some((entry_log, complete)) = ffxi_proto::mission::quest_port(entry.port) {
            if entry_log == log && ffxi_proto::mission::quest_ids(&entry.data).contains(&id) {
                bits.0 |= !complete;
                bits.1 |= complete;
            }
        }
    }
    let area = ffxi_vocab::quest_lattice::QUEST_AREA_NAMES
        .get(log as usize)
        .copied()
        .unwrap_or("Unknown");
    let mut lines: Vec<String> = Vec::new();
    match quest_caption(log, id) {
        Some(name) => lines.push(name.to_string()),
        None => lines.push(format!("Quest #{id}")),
    }
    lines.push(format!("Area: {area}"));
    if bits.1 {
        lines.push("Complete.".to_string());
    } else if bits.0 {
        lines.push("In progress.".to_string());
    }
    let mut rows: Vec<DynamicMenuRow> = lines
        .into_iter()
        .map(|label| DynamicMenuRow {
            tint: None,
            label,
            action: DynamicMenuAction::QuestDetailRow { log, id },
        })
        .collect();
    if rows.is_empty() {
        rows.push(DynamicMenuRow {
            tint: None,
            label: "(nothing recorded)".to_string(),
            action: DynamicMenuAction::QuestDetailRow { log, id },
        });
    }
    rows
}

/// Rows for one side of a home-view job change: every JOBTYPE id whose bit is
/// set in the s2c 0x01B roster (bit 0 and never-unlocked jobs stay out — LSB's
/// MYROOM_JOB validator ranges MainJobIndex to 0x01..=MAX-1 and tests each
/// non-zero index against `jobs.unlocked`, vendor/server/src/map/packets/c2s/
/// 0x100_myroom_job.cpp; vault main_menu.md §53). A side picks nothing while
/// the roster is unknown (`snap.jobs` None) or the sub-job grant never arrived.
pub fn job_pick_rows(snap: &kuluu_snapshot::SceneSnapshot, main_pick: bool) -> Vec<DynamicMenuRow> {
    let Some(jobs) = snap.jobs.as_ref() else {
        return Vec::new();
    };
    if !main_pick && !jobs.sub_job_unlocked {
        return Vec::new();
    }
    let max_jobs = jobs.job_levels.len().min(32);
    (1u8..max_jobs as u8)
        .filter(|id| jobs.unlocked & (1 << *id) != 0)
        .map(|job_id| {
            let level = jobs.job_levels[usize::from(job_id)];
            DynamicMenuRow {
                tint: None,
                label: format!(
                    "{} LV{}",
                    ffxi_vocab::job_names::lookup(u16::from(job_id)).unwrap_or("???"),
                    level
                ),
                action: DynamicMenuAction::HomeJobRow {
                    job_id,
                    as_main: main_pick,
                },
            }
        })
        .collect()
}

/// The add/clear action rows of the macro line editor. kuluu-worded chrome —
/// mcrledit's DAT record names no captions (vault main_menu.md §34/§41).
pub const MACRO_ROW_USE: &str = "Use this macro";
pub const MACRO_ROW_ADD_LINE: &str = "+ Add command line";
pub const MACRO_ROW_CLEAR_SLOT: &str = "Clear this macro";

/// The slot's stored text as displayable command lines. A retail macro holds a
/// sequence of commands run in order (the chat-bar use path, vault main_menu.md
/// §41/§53); kuluu stores them newline-joined and lists one per row.
pub fn macro_slot_lines(text: &str) -> Vec<&str> {
    text.split('\n').filter(|l| !l.trim().is_empty()).collect()
}

/// Rows of the long-form/line editor for keytop (block, index): each command
/// line, then add/clear actions (bead kuluu-o9qt7.19.3; retail's mcrledit law
/// FFXiMain.dll retail-2026-09 RVA 0x1A6180).
pub fn macro_editor_rows(
    snap: &kuluu_snapshot::SceneSnapshot,
    block: u8,
    index: u8,
) -> Vec<DynamicMenuRow> {
    let mut rows = snap
        .macros
        .iter()
        .find(|s| s.block == block && s.index == index)
        .map(|s| {
            macro_slot_lines(&s.text)
                .into_iter()
                .enumerate()
                .map(|(i, line)| DynamicMenuRow {
                    tint: None,
                    label: line.to_string(),
                    action: DynamicMenuAction::MacroEditLine {
                        line_index: Some(i),
                    },
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // The use row first among the actions: running a keytop is what retail's
    // palette/grid interaction is for (vault main_menu.md §41/§56).
    let slot_present = snap
        .macros
        .iter()
        .any(|s| s.block == block && s.index == index);
    if slot_present {
        rows.push(DynamicMenuRow {
            tint: None,
            label: MACRO_ROW_USE.to_string(),
            action: DynamicMenuAction::MacroUse { block, index },
        });
    }
    rows.push(DynamicMenuRow {
        tint: None,
        label: MACRO_ROW_ADD_LINE.to_string(),
        action: DynamicMenuAction::MacroEditLine { line_index: None },
    });
    rows.push(DynamicMenuRow {
        tint: None,
        label: MACRO_ROW_CLEAR_SLOT.to_string(),
        action: DynamicMenuAction::MacroClearSlot,
    });
    rows
}

/// Applies one committed line edit to the whole set (the shape sent as
/// `AgentCommand::CommitMacros`): replace at `line_no`, append when `None`, an
/// empty result drops that line and a fully emptied slot disappears — retail's
/// blank keytops leave no store entry (vault main_menu.md §41).
pub fn merge_macro_line(
    slots: &[kuluu_snapshot::MacroSlot],
    block: u8,
    index: u8,
    line_no: Option<usize>,
    new_text: &str,
) -> Vec<kuluu_snapshot::MacroSlot> {
    let mut out = slots.to_vec();
    let slot_pos = out
        .iter()
        .position(|s| s.block == block && s.index == index);
    let mut lines: Vec<String> = match slot_pos {
        Some(p) => macro_slot_lines(&out[p].text)
            .into_iter()
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    };
    match line_no {
        Some(i) if i < lines.len() => lines[i] = new_text.to_string(),
        _ => lines.push(new_text.to_string()),
    }
    lines.retain(|l| !l.trim().is_empty());
    let joined = lines.join("\n");
    match (slot_pos, joined.trim().is_empty()) {
        (Some(p), true) => {
            out.remove(p);
        }
        (Some(p), false) => out[p].text = joined,
        (None, true) => {}
        (None, false) => out.push(kuluu_snapshot::MacroSlot {
            block,
            index,
            text: joined,
        }),
    }
    out
}

pub fn refresh_dynamic_menu_rows(
    mode: Res<InputMode>,
    scene: Res<crate::snapshot::SceneState>,
    sort: Res<crate::hud::item_detail::SortOptions>,
    active_bag: Res<crate::hud::item_screen::ItemScreenContainer>,
    dat_root: Option<Res<crate::dat_root::SharedDatRoot>>,
    mut school_table: ResMut<MagicSchoolTable>,
    mut config_rows: ResMut<ConfigDatRows>,
    mut selector_captions: ResMut<CommandSelectorCaptions>,
    mut equip_records: ResMut<EquipRecords>,
    bindings: Res<crate::keybinds::Bindings>,
    mut dynamic: ResMut<DynamicMenu>,
    mut unity_fp_dat: ResMut<crate::hud::unity_fp::UnityFpDat>,
    mut merit_dat: ResMut<crate::hud::merit_windows::MeritDat>,
) {
    // The remodel prompt's question is DAT text, so it rides in with every refresh (bead
    // kuluu-o9qt7.13.5.2) rather than being typed into the client.
    if dynamic.roomlist_prompt != config_rows.roomlist_prompt {
        let prompt = config_rows.roomlist_prompt.clone();
        dynamic.roomlist_prompt = prompt;
    }
    // The door row wears its caption from the catalog whichever window is open, and `myroom` is a
    // static-kind menu whose rows never pass through the per-arm loads below.
    let dat_for_captions = dat_root.as_ref().and_then(|r| r.get()).map(|v| &**v);
    config_rows.ensure_loaded(dat_for_captions);
    let wanted_door_caption = home_door_caption(
        &config_rows.home_door_captions,
        scene.snapshot.myroom.is_some_and(|room| room.door_open),
    )
    .map(str::to_string);
    if dynamic.home_door_caption != wanted_door_caption {
        dynamic.home_door_caption = wanted_door_caption;
    }
    // The `abisortw` caption and the two swap prompts ride in the same way (bead
    // kuluu-o9qt7.34): DAT text, copied while unchanged rows keep their borrow.
    if dynamic.abisortw_captions != config_rows.abisortw_captions {
        dynamic
            .abisortw_captions
            .clone_from(&config_rows.abisortw_captions);
    }
    if dynamic.emote_sort_prompts != config_rows.emote_sort_prompts {
        dynamic
            .emote_sort_prompts
            .clone_from(&config_rows.emote_sort_prompts);
    }
    let stack = match &*mode {
        InputMode::Menu(stack) => Some(stack),
        _ => None,
    };

    // The Unity and alter ego point windows read their own records and tables, loaded the
    // first time one opens; their per-window state follows the stack every frame one is up
    // and once more when the last one closes.
    let unity_fp_open = stack.is_some_and(|stack| {
        stack
            .levels
            .iter()
            .any(|level| crate::hud::unity_fp::is_unity_fp_window(level.kind))
    });
    if unity_fp_open {
        unity_fp_dat.ensure_loaded(dat_for_captions);
        let texts = &unity_fp_dat.texts;
        let message = texts
            .get_text(
                crate::hud::gettext::GETTEXT_TABLE_CLIENT_MESSAGES,
                crate::hud::unity_fp::NOT_ALLOWED_MESSAGE_ID,
            )
            .map(str::to_string);
        if dynamic.unity_fp.not_allowed_message != message {
            dynamic.unity_fp.not_allowed_message = message;
        }
        let leader_help = texts
            .get_text(
                crate::hud::gettext::GETTEXT_TABLE_CAPTIONS,
                crate::hud::unity_fp::LEADER_HELP_ID,
            )
            .map(str::to_string);
        if dynamic.unity_fp.leader_help != leader_help {
            dynamic.unity_fp.leader_help = leader_help;
        }
    }
    if unity_fp_open || dynamic.unity_fp != crate::hud::unity_fp::UnityFpWindows::default() {
        crate::hud::unity_fp::sync_unity_fp_windows(&mut dynamic, stack, &scene.snapshot);
    }

    let merit_open = stack.is_some_and(|stack| {
        stack
            .levels
            .iter()
            .any(|level| crate::hud::merit_windows::is_merit_window(level.kind))
    });
    if merit_open {
        merit_dat.ensure_loaded(dat_for_captions);
        refresh_merit_dat_text(&mut dynamic, &merit_dat.texts);
    }
    let merit_state_open = dynamic.merit.categories.is_some() || dynamic.merit.job_page.is_some();
    if merit_open || merit_state_open || !dynamic.merit.list_starts.is_empty() {
        crate::hud::merit_windows::sync_merit_windows(
            &mut dynamic,
            stack,
            &scene.snapshot,
            &merit_dat,
        );
    }

    // The equip gate needs item records whatever window is open, so the DATs resolve once here
    // rather than per arm.
    equip_records.ensure_loaded(dat_root.as_ref().and_then(|r| r.get()).map(|v| &**v));
    selector_captions.ensure_loaded(dat_root.as_ref().and_then(|r| r.get()).map(|v| &**v));
    if !selector_captions.labels.is_empty() && dynamic.selector_captions != selector_captions.labels
    {
        dynamic
            .selector_captions
            .clone_from(&selector_captions.labels);
    }

    let snap = &scene.snapshot;
    let stacked_rows: Vec<(MenuKind, Vec<DynamicMenuRow>)> = stack
        .and_then(|stack| stack.levels.split_last())
        .map(|(_, below)| {
            below
                .iter()
                .filter(|level| is_dynamic(level.kind))
                .map(|level| {
                    let rows = dynamic_rows_for(
                        level,
                        snap,
                        &sort,
                        &active_bag,
                        &dat_root,
                        &mut school_table,
                        &mut config_rows,
                        &equip_records,
                        &bindings,
                        &dynamic,
                        &unity_fp_dat,
                        &merit_dat,
                    );
                    (level.kind, rows)
                })
                .collect()
        })
        .unwrap_or_default();
    if dynamic.stacked_rows != stacked_rows {
        dynamic.stacked_rows = stacked_rows;
    }
    let top = stack.and_then(MenuStack::current);
    let rows_kind = top.map(|level| level.kind);
    if dynamic.rows_kind != rows_kind {
        dynamic.rows_kind = rows_kind;
    }
    let Some(level) = top.filter(|level| is_dynamic(level.kind)) else {
        if !dynamic.rows.is_empty() {
            dynamic.rows.clear();
        }
        return;
    };
    let rows = dynamic_rows_for(
        level,
        snap,
        &sort,
        &active_bag,
        &dat_root,
        &mut school_table,
        &mut config_rows,
        &equip_records,
        &bindings,
        &dynamic,
        &unity_fp_dat,
        &merit_dat,
    );
    if rows != dynamic.rows {
        dynamic.rows = rows;
    }
}

/// The menu stack an input mode keeps open, through a sub-target cursor waiting to return to it.
pub fn live_menu(mode: &InputMode) -> Option<&MenuStack> {
    match mode {
        InputMode::Menu(stack) => Some(stack),
        InputMode::SubTarget(state) => live_menu(&state.return_to),
        _ => None,
    }
}

/// The Command menu an input mode keeps open, through a sub-target cursor waiting to return to it.
pub fn live_command_menu(mode: &InputMode) -> Option<&TargetActionState> {
    match mode {
        InputMode::TargetAction(state) => Some(state),
        InputMode::SubTarget(state) => live_command_menu(&state.return_to),
        _ => None,
    }
}

/// What [`persist_window_cursors`] saw open last frame.
#[derive(Default)]
pub struct WindowsSeenOpen {
    menu: Option<MenuStack>,
    command_menu: Option<TargetActionState>,
}

/// Keeps the session's window cursor memory: an open menu's records join it every frame, and
/// when the menu or the Command menu goes away, every window still open closes into it
/// (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md).
pub fn persist_window_cursors(
    mode: Res<InputMode>,
    mut dynamic: ResMut<DynamicMenu>,
    mut seen: Local<WindowsSeenOpen>,
) {
    if !mode.is_changed() {
        return;
    }
    match live_menu(&mode) {
        Some(stack) => {
            if !dynamic.window_cursors.holds(&stack.cursor_memory) {
                dynamic.window_cursors.absorb(&stack.cursor_memory);
            }
            seen.menu = Some(stack.clone());
        }
        None => {
            if let Some(mut closed) = seen.menu.take() {
                closed.close_all();
                dynamic.window_cursors.absorb(&closed.cursor_memory);
            }
        }
    }
    match live_command_menu(&mode) {
        Some(state) => seen.command_menu = Some(state.clone()),
        None => {
            if let Some(closed) = seen.command_menu.take() {
                closed.record_pickers(&mut dynamic.window_cursors);
            }
        }
    }
}

/// The number of rows built for an open window: the top one's in `rows`, the others' in
/// `stacked_rows`. `None` until the rows refresh has built them for that window.
fn built_row_count(dynamic: &DynamicMenu, kind: MenuKind, top: bool) -> Option<usize> {
    if top {
        return (dynamic.rows_kind == Some(kind)).then_some(dynamic.rows.len());
    }
    dynamic
        .stacked_rows
        .iter()
        .find(|(open, _)| *open == kind)
        .map(|(_, rows)| rows.len())
}

/// Finishes what a window's creation could not know yet: a remembered cursor comes back inside
/// the rows the window got (part of its open-time placement), and the record tells which row is
/// the last item made unselectable. A window opened since the rows were last built waits for its
/// own rows.
pub fn settle_opened_windows(
    mut mode: ResMut<InputMode>,
    dynamic: Res<DynamicMenu>,
    config_rows: Res<ConfigDatRows>,
    unity: Res<crate::hud::unity_fp::UnityFpDat>,
) {
    let InputMode::Menu(stack) = &*mode else {
        return;
    };
    let resolved: Vec<Option<LastItemRow>> = stack
        .levels
        .iter()
        .map(|level| {
            level
                .last_item_lock
                .filter(|lock| lock.row == LastItemRow::Unresolved)
                .and_then(|_| last_item_row(level.kind, &config_rows, &unity))
        })
        .collect();
    let top = stack.levels.len().saturating_sub(1);
    let bounds: Vec<Option<usize>> = stack
        .levels
        .iter()
        .enumerate()
        .map(|(index, level)| {
            level
                .clamp_pending
                .then(|| built_row_count(&dynamic, level.kind, index == top))
                .flatten()
        })
        .collect();
    let pending = bounds.iter().any(Option::is_some) || resolved.iter().any(Option::is_some);
    if !pending {
        return;
    }
    let InputMode::Menu(stack) = &mut *mode else {
        return;
    };
    for (index, level) in stack.levels.iter_mut().enumerate() {
        if let Some(rows) = bounds[index] {
            level.cursor = level.cursor.min(rows.saturating_sub(1));
            level.clamp_pending = false;
            if let Some(lock) = level.last_item_lock.as_mut() {
                lock.open_cursor = level.cursor;
            }
        }
        if let (Some(lock), Some(row)) = (level.last_item_lock.as_mut(), resolved[index]) {
            lock.row = row;
        }
    }
}

/// Copies the DAT lines the merit windows' input path prints and the help bar shows.
fn refresh_merit_dat_text(dynamic: &mut DynamicMenu, texts: &crate::hud::gettext::GetTextTables) {
    use crate::hud::gettext::{GETTEXT_TABLE_CAPTIONS, GETTEXT_TABLE_CLIENT_MESSAGES};
    use crate::hud::merit_windows as mw;
    let line = |table: u8, id: u16| texts.get_text(table, id).map(str::to_string);
    let not_in_mog_house = line(
        GETTEXT_TABLE_CLIENT_MESSAGES,
        crate::hud::unity_fp::NOT_ALLOWED_MESSAGE_ID,
    );
    let mode_refused = line(GETTEXT_TABLE_CLIENT_MESSAGES, mw::MODE_REFUSED_MESSAGE_ID);
    let sort_help = [
        line(GETTEXT_TABLE_CAPTIONS, mw::SORT_HELP_RAISE_ID),
        line(GETTEXT_TABLE_CAPTIONS, mw::SORT_HELP_LOWER_ID),
    ];
    if dynamic.merit.not_in_mog_house_message != not_in_mog_house {
        dynamic.merit.not_in_mog_house_message = not_in_mog_house;
    }
    if dynamic.merit.mode_refused_message != mode_refused {
        dynamic.merit.mode_refused_message = mode_refused;
    }
    if dynamic.merit.sort_help != sort_help {
        dynamic.merit.sort_help = sort_help;
    }
}

fn dynamic_rows_for(
    level: &MenuLevel,
    snap: &kuluu_snapshot::SceneSnapshot,
    sort: &crate::hud::item_detail::SortOptions,
    active_bag: &crate::hud::item_screen::ItemScreenContainer,
    dat_root: &Option<Res<crate::dat_root::SharedDatRoot>>,
    school_table: &mut MagicSchoolTable,
    config_rows: &mut ConfigDatRows,
    equip_records: &EquipRecords,
    bindings: &crate::keybinds::Bindings,
    dynamic: &DynamicMenu,
    unity_fp_dat: &crate::hud::unity_fp::UnityFpDat,
    merit_dat: &crate::hud::merit_windows::MeritDat,
) -> Vec<DynamicMenuRow> {
    let kind = level.kind;
    let rows: Vec<DynamicMenuRow> = match kind {
        kind if crate::hud::unity_fp::is_unity_fp_window(kind) => {
            crate::hud::unity_fp::unity_fp_rows(level, unity_fp_dat, &dynamic.unity_fp)
        }
        MenuKind::Merit(_) => {
            crate::hud::merit_windows::merit_rows_for(level, merit_dat, &dynamic.merit, snap)
        }
        MenuKind::Arrange(window) => dynamic.arrange.rows(window),
        MenuKind::Magic => spell_rows(&snap.spells_known),
        MenuKind::MagicGroup(school) => {
            school_table.ensure_loaded(dat_root.as_ref().and_then(|r| r.get()).map(|v| &**v));
            magic_school_rows(&snap.spells_known, school, |id| school_table.school_of(id))
        }
        // Party window (FFXiMain.dll retail-2026-09 RVA 0x2016E0; vault
        // main_menu.md §24): invite pending → prtyjoin's YES/NO; else the party
        // roster as display rows; not in a party → empty-hint state.
        MenuKind::Party => party_window_rows(snap),
        // Trade window pair (FFXiMain.dll retail-2026-09 RVA 0x1515E0; vault
        // main_menu.md §25): answer rows → gil/own slots/buttons → partner
        // views, and the Add-Item bag picker under each own slot row.
        MenuKind::Trade => trade_window_rows(snap),
        MenuKind::Search => search_window_rows(snap),
        // Link Shell slots (helper FFXiMain.dll retail-2026-09 RVA 0x1E4470 always opens
        // `menu    link5`; vault main_menu.md section 27).
        MenuKind::Linkshell => linkshell_window_rows(snap),
        // The craft list (kuluu surface for the cmbHwin/cmbhlst pair behind cmbmenu row 2;
        // FFXiMain.dll retail-2026-09 RVA 0x1E29C1, vault main_menu.md §22/§28).
        MenuKind::CraftList => craft_list_rows(snap),
        // Region Info modes (vault main_menu.md §29): the rows also carry the
        // predicate gate notice for an unresolvable zone.
        MenuKind::RegionInfo { mode } => region_info_rows(mode, snap.zone_id),
        // The missionm list follows the folded s2c 0x056 battery (vault main_menu.md §30).
        MenuKind::MissionList => mission_list_rows(snap),
        MenuKind::MissionDetail { port } => mission_detail_rows(snap, port),
        // The bazaar browser lists the zone's open player stalls (§33 wire law).
        MenuKind::BazaarStalls => bazaar_stall_rows(snap),
        // The quest00 list follows the folded s2c 0x056 quest ports (vault
        // main_menu.md §31).
        MenuKind::QuestList => quest_list_rows(snap),
        MenuKind::QuestDetail { log, id } => quest_detail_rows(snap, log, id),
        // The pupequip variant's fourteen automaton slot rows (vault
        // main_menu.md §39).
        MenuKind::AutomatonEquip => automaton_equip_rows(snap),
        MenuKind::MacroSelect => macro_select_rows(snap),
        // Home-view job pick lists — both sides come from the same roster
        // (vault main_menu.md §53); the sub side renders empty until the
        // packet's sub-job grant arrived, like LSB refusing the row.
        MenuKind::JobPick { main_pick } => job_pick_rows(snap, main_pick),
        // roomlist's five DAT-synthesised rows (bead kuluu-o9qt7.13.5).
        MenuKind::RoomList => {
            config_rows.ensure_loaded(dat_root.as_ref().and_then(|r| r.get()).map(|v| &**v));
            roomlist_rows(config_rows)
        }
        // The long-form/line-editor family for one keytop (§34: mcrbedit/
        // mcr1edit/mcr2edit/mcrledit collapse to this drill [I-shape]).
        MenuKind::MacroEditor { block, index } => macro_editor_rows(snap, block, index),
        // The Config family: every window's rows are its DAT captioned item blocks
        // in file order (row law §59 via `live_items`; generation law §49/§57),
        // actions from the
        // handlers' jump tables (§42 hub map, §47 conf5m arms).
        MenuKind::RetailConfig | MenuKind::RetailConfigPage(_) | MenuKind::RetailConfigSub(_) => {
            config_rows.ensure_loaded(dat_root.as_ref().and_then(|r| r.get()).map(|v| &**v));
            let window = match kind {
                MenuKind::RetailConfig => RETAIL_CONFIG_HUB_WINDOW,
                MenuKind::RetailConfigPage(page) => RETAIL_CONFIG_PAGE_WINDOWS
                    .get(usize::from(page))
                    .copied()
                    .unwrap_or(""),
                MenuKind::RetailConfigSub(sub) => RETAIL_CONFIG_SUB_WINDOWS
                    .get(usize::from(sub))
                    .copied()
                    .unwrap_or(""),
                _ => "",
            };
            config_window_rows(
                window,
                config_rows,
                &dynamic.config_option_values,
                dynamic.option_edit,
                &dynamic.config_window_choices,
            )
        }
        MenuKind::TradePick { trade_index } => trade_pick_rows(snap, trade_index),
        // The Status detail windows (bead kuluu-0oy7b): every line is live
        // snapshot data or a grounded empty readout; selecting stays inert per
        // the retail dispatch law (FFXiMain.dll retail-2026-09 RVA 0x148D00). The two
        // Currencies lists additionally need the table-12 label catalog loaded (§59 add.
        // 27, bead kuluu-o9qt7.35) — same DAT record roomlist's confirm question reads.
        MenuKind::StatusDetail { entry } => {
            config_rows.ensure_loaded(dat_root.as_ref().and_then(|r| r.get()).map(|v| &**v));
            crate::hud::status_panel::status_detail_lines(
                usize::from(entry),
                snap,
                &config_rows.currency_labels_1,
                &config_rows.currency_labels_2,
            )
            .into_iter()
            .map(|label| DynamicMenuRow {
                tint: None,
                label,
                action: DynamicMenuAction::DisplayInfo,
            })
            .collect()
        }
        // abimenu category list (vault main_menu.md §23): the same group rows
        // kuluu's target-action groups already build.
        MenuKind::AbilitiesGroup(group) => crate::hud::action_model::AbilityGroup::ALL
            .get(group as usize)
            .map(|g| ability_group_rows(snap, *g))
            .unwrap_or_default(),
        MenuKind::Abilities => ability_flat_rows(snap),
        MenuKind::Items => inventory_rows(snap, active_bag.0, sort),
        MenuKind::KeyItems => key_item_rows(snap),
        // The retail opener `.text 0x225460` always opens the `abisortw` companion **first**,
        // so the captioned list lands on top and takes the keys (bead kuluu-o9qt7.34); kuluu
        // carries no live cross-pane focus, so the companion's single select item rides as the
        // chain row at the tail of the focused list — its label is the mode-1 DAT caption,
        // read never transcribed ([I] reading of retail's unfound focus-switch, recorded on
        // the bead).
        MenuKind::EmoteList => {
            let mut rows = emote_rows(snap);
            append_abisortw_chain_row(
                &mut rows,
                dynamic.abisortw_captions.get(1).and_then(|c| c.as_deref()),
            );
            rows
        }
        MenuKind::UsableItems => usable_item_rows(snap),
        MenuKind::ItemAction {
            container,
            index,
            item_no,
        } => item_action_rows(snap, container, index, item_no),
        MenuKind::EquipSlot(equip_slot) => equip_slot_rows(snap, equip_slot, Some(equip_records)),
        // The live key map (bead kuluu-o9qt7.26.1): rows are the store itself plus
        // the trailing preset commands, so any remap shows without a restart.
        MenuKind::Controls => controls_rows(bindings),
        _ => Vec::new(),
    };

    let mut rows = rows;
    // While a command line is being typed in the macro editor, echo its live
    // buffer as a trailing row (§34's mcrledit edit-then-commit law; kuluu
    // draws it inside the list). Edit-mode keys route to the buffer before the
    // cursor ever moves, so the echoed row never needs own semantics.
    if let MenuKind::MacroEditor { .. } = kind {
        if let Some(buf) = level.macro_line_edit.as_ref() {
            rows.push(DynamicMenuRow {
                tint: None,
                label: format!("{} > {}_", MACRO_ROW_ADD_LINE, buf.text),
                action: DynamicMenuAction::MacroEditLine {
                    line_index: buf.line_no,
                },
            });
        }
    }
    order_dynamic_rows(kind, sort, &mut rows);
    rows
}

/// One row per slot of `container`, labelled with the bare item name (the item
/// window draws the stack count as an icon badge). Auto-sort orders by category — item id, which
/// groups usables, then weapons, then armor by their DAT id ranges — or by name, exactly the pair
/// the Gameplay page offers; Manual keeps the raw slot order.
pub fn inventory_rows(
    snap: &kuluu_snapshot::SceneSnapshot,
    container: u8,
    sort: &crate::hud::item_detail::SortOptions,
) -> Vec<DynamicMenuRow> {
    let mut rows: Vec<DynamicMenuRow> = snap
        .container(container)
        .map(|c| c.items.as_slice())
        .unwrap_or(&[])
        .iter()
        .filter_map(|slot| {
            let name = ffxi_vocab::item_names::lookup(slot.item_no)?;
            Some(DynamicMenuRow {
                tint: None,
                label: name.to_string(),
                action: DynamicMenuAction::OpenItemAction {
                    container: slot.container,
                    index: slot.index,
                    item_no: slot.item_no,
                },
            })
        })
        .collect();
    order_dynamic_rows(MenuKind::Items, sort, &mut rows);
    rows
}

/// For callers whose list is id-ordered independently of the player's Config choice.
const ALWAYS_CATEGORY_SORT: crate::hud::item_detail::SortOptions =
    crate::hud::item_detail::SortOptions {
        auto: true,
        key: crate::hud::item_detail::SortKey::Category,
    };

/// Order the freshly built rows for one dynamic menu. Retail's Items window
/// sorts when "Auto" is on, by category (item id — grouping usable items, then weapons,
/// then armor by their DAT id ranges) or by name, and otherwise shows raw inventory-slot
/// order. The item context submenu keeps its built order; other dynamic menus
/// stay alphabetical.
fn order_dynamic_rows(
    kind: MenuKind,
    sort: &crate::hud::item_detail::SortOptions,
    rows: &mut [DynamicMenuRow],
) {
    use crate::hud::item_detail::SortKey;
    match kind {
        MenuKind::Items => {
            if sort.auto {
                match sort.key {
                    SortKey::Category => rows.sort_by_key(item_row_sort_key),
                    SortKey::Name => rows.sort_by(compare_item_rows_by_name),
                }
            }
        }
        // Retail's Command Menu Items list is always id-sorted.
        MenuKind::UsableItems => rows.sort_by_key(item_row_sort_key),
        // Key items keep id order: ascending global id groups the 512-id
        // tables together (whether retail sections match the tables is an
        // open question, bead kuluu-h7x retail_unknowns). Emotes keep the
        // scraped-table (id) order.
        MenuKind::ItemAction { .. }
        | MenuKind::KeyItems
        | MenuKind::EmoteList
        | MenuKind::Arrange(_) => {}
        _ => rows.sort_by(|a, b| a.label.cmp(&b.label)),
    }
}

/// The equip window's slot pick list (`FFXiMain.dll retail-2026-09 RVA 0x1322B0`
/// event-id 8 state==2; vault main_menu.md §20/§50): every inventory slot whose
/// item can legally land in `equip_slot`.
///
/// The predicate is retail's own equip legality check, FFXiMain.dll retail-2026-09 RVA 0x132050:
/// the client runs it while building this very list, and again on select — a refusal there costs
/// only the buzzer (caller `.text 0x132A1E` branches to its refusal sound right after). Every
/// requirement comes from the item's own DAT record, as [`record_allows_equip`] restates, so kuluu
/// reads it from there via [`EquipRecords`], resolved from the same install as every other DAT read.
///
/// With no readable record (no install) the list falls back to the scraped LSB columns, i.e. the
/// server's acceptance gate in vendor/server/src/map/utils/charutils.cpp `EquipArmor` (slot bit,
/// job bit, `level`) — it cannot express race or superior limits, so it is only ever a fallback.
pub fn equip_slot_rows(
    snap: &kuluu_snapshot::SceneSnapshot,
    equip_slot: u8,
    records: Option<&EquipRecords>,
) -> Vec<DynamicMenuRow> {
    let actor = equip_actor(snap);
    let rows: Vec<DynamicMenuRow> = snap
        .inventory_main()
        .iter()
        .filter_map(|slot| {
            let info = ffxi_vocab::equip_info::lookup(slot.item_no)?;
            if !ffxi_vocab::equip_info::fits_slot(&info, equip_slot) {
                return None;
            }
            match records.and_then(|r| r.record(slot.item_no)) {
                Some(record) => {
                    if !record_allows_equip(EquipRecordGates::from_record(&record), actor) {
                        return None;
                    }
                }
                None if equip_scrape_refuses(&info, actor) => return None,
                None => {}
            }
            let name = ffxi_vocab::item_names::lookup(slot.item_no)?;
            let label = if info.level > 0 {
                format!("{name} (Lv{})", info.level)
            } else {
                name.to_string()
            };
            // Every row that survives the gate is wearable right now, so it carries retail's
            // lime; the same-id equip-slot scan takes precedence, exactly as the colour
            // resolver orders them (FFXiMain.dll retail-2026-09 `.text 0x1DE157`).
            let tint = equip_row_tint(
                true,
                same_id_equipped_in_scanned_slots(&snap.equipped, slot.item_no),
            );
            Some(DynamicMenuRow {
                label,
                action: DynamicMenuAction::EquipItem {
                    container: slot.container,
                    container_index: slot.index,
                    equip_slot,
                    item_no: slot.item_no,
                },
                tint,
            })
        })
        .collect();
    rows
}

/// The equip gate's item records: retail keeps every requirement an equip depends on inside
/// the item's own DAT record: FFXiMain.dll retail-2026-09 RVA 0x15C4E0 attaches the record to the
/// item object at +0xB4 and the legality check (RVA 0x132050) reads it straight from there — races
/// mask `.text 0x1320D4`, jobs mask `.text 0x1320E5`, level `.text 0x1320F8`, superior
/// `.text 0x13210D`. kuluu therefore resolves the same records from the installed DATs rather than
/// carrying requirement columns in code (bead kuluu-o9qt7.3.4).
#[derive(Resource, Default)]
pub struct EquipRecords {
    table: Option<std::sync::Arc<ffxi_dat::item_dat::ItemTable>>,
}

impl EquipRecords {
    /// Opens the installed item DATs once; a missing install leaves it unloaded so the next call
    /// retries, matching every other DAT-backed menu resource here.
    pub fn ensure_loaded(&mut self, dat: Option<&ffxi_dat::archive::DatRoot>) {
        if self.table.is_some() {
            return;
        }
        let Some(root) = dat else { return };
        let table = ffxi_dat::item_dat::ItemTable::open(root.root());
        if table.is_empty() {
            return;
        }
        self.table = Some(std::sync::Arc::new(table));
    }

    pub fn record(&self, item_no: u16) -> Option<ffxi_dat::item_dat::ItemStatic> {
        self.table.as_ref()?.lookup(item_no)
    }
}

/// The actor side of retail's equip legality check. `race` comes from our own entity's look, which
/// is where FFXiMain.dll retail-2026-09 reads it too (`mov bl, byte ptr [entity + 0xEF]`,
/// `.text 0x132099` inside RVA 0x132050); `None` means the spawn carrying that byte has not
/// arrived yet.
#[derive(Debug, Clone, Copy)]
pub struct EquipActor {
    pub race: Option<u8>,
    pub main_job: u8,
    pub main_lv: u8,
    pub su_lv: u8,
    /// The player's per-slot equip-block dword (s2c 0x061 packet +0x60, the dword
    /// `.text FFXiMain.dll retail-2026-09 0xFD350` hands to the fifth legality test).
    /// 0 blocks nothing — what an unknown or absent player reads as.
    pub equip_slot_block: u32,
}

fn equip_actor(snap: &kuluu_snapshot::SceneSnapshot) -> EquipActor {
    let (main_job, main_lv) = snap
        .self_char_id
        .and_then(|id| snap.party.iter().find(|m| m.id == id))
        .map(|m| (m.main_job, m.main_job_lv))
        .unwrap_or((0, 0));
    let race = snap
        .self_char_id
        .and_then(|id| snap.entities.iter().find(|e| e.id == id))
        .and_then(|e| match e.look.as_ref()? {
            kuluu_snapshot::EntityLook::Equipped { race, .. } => Some(*race),
            _ => None,
        });
    EquipActor {
        race,
        main_job,
        main_lv,
        su_lv: snap.stats.map_or(0, |s| s.su_lv),
        equip_slot_block: snap.stats.map_or(0, |s| s.equip_slot_block),
    }
}

/// The record columns retail tests per equip. Fields are the record's own bytes at the
/// offsets the legality check indexes (record `level u16 +0xC`, `slots u16 +0xE`,
/// `races u16 +0x10`, `jobs u32 +0x14`, `su u8 +0x18`), indexed by FFXiMain.dll retail-2026-09
/// RVA 0x132050 across `.text 0x1320D4..0x132134`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EquipRecordGates {
    pub races_mask: u16,
    /// The record's slot mask (record +0xE). Only the fifth test reads it — against the
    /// player's equip-block dword, never against "the slot this pick list is for".
    pub slots_mask: u16,
    pub jobs_mask: u32,
    pub level: u8,
    pub superior_level: u8,
}

impl EquipRecordGates {
    pub fn from_record(record: &ffxi_dat::item_dat::ItemStatic) -> Self {
        Self {
            races_mask: record.races_mask,
            slots_mask: record.slot_mask,
            jobs_mask: record.jobs_mask,
            level: record.level,
            superior_level: record.superior_level,
        }
    }
}

/// Retail's equip legality law, FFXiMain.dll retail-2026-09 RVA 0x132050, in the DLL's own order
/// (races → jobs → item level → superior → slot block, `.text 0x1320D4..0x132134`), with helpers
/// `.text 0x1CC5D0` for the effective main-job level and `.text 0x1CC600` for the player's `su`.
/// A failing step returns immediately, so a record already refused for its race never reaches
/// the job test.
///
/// An unknown input skips only its own step — no entity look yet means no race test. Superior is
/// the exception: retail always compares, so an actor whose `su` is unknown (0) fails every item
/// carrying a superior requirement, exactly as the DLL's `cmp al, [ecx]; jb refuse` does.
///
/// The fifth test (`.text 0x132111`) intersects the record's slot mask with the player's
/// equip-block dword — game state `+0x40DF0`, read through `.text 0xFD350` and fed by s2c 0x061
/// packet +0x60 (stored at `.text 0xF0982`) — and refuses on any intersection. It answers a
/// different question from "does this item fit the slot window we are picking for", so kuluu's
/// pick lists keep their own slot filter too; no player object means `.text 0xFD350` returns 0,
/// which blocks nothing — that is also how an actor without the mask reads.
pub fn record_allows_equip(gates: EquipRecordGates, actor: EquipActor) -> bool {
    if let Some(race) = actor.race {
        let Some(bit) = (1u16).checked_shl(u32::from(race)) else {
            return false;
        };
        if gates.races_mask & bit == 0 {
            return false;
        }
    }
    if actor.main_job != 0 {
        let Some(bit) = (1u32).checked_shl(u32::from(actor.main_job)) else {
            return false;
        };
        if gates.jobs_mask & bit == 0 {
            return false;
        }
    }
    if actor.main_lv != 0 && gates.level > actor.main_lv {
        return false;
    }
    if gates.superior_level > actor.su_lv {
        return false;
    }
    // The fifth test: `.text FFXiMain.dll retail-2026-09 0x132111` refuses when the record's
    // slot word and the equip-block dword intersect (`test edx, eax; sete al`).
    (u32::from(gates.slots_mask) & actor.equip_slot_block) == 0
}

/// The fallback predicate while no item record is readable: the scraped LSB equipment columns
/// (the caller has already tested the slot bit), which carry job and level only and therefore
/// cannot express a race or superior limit — bead kuluu-o9qt7.3.4 keeps this path for machines
/// with no install, never as a substitute for the record.
fn equip_scrape_refuses(info: &ffxi_vocab::equip_info::EquipInfo, actor: EquipActor) -> bool {
    (actor.main_job != 0 && !ffxi_vocab::equip_info::fits_job(info, actor.main_job))
        || (actor.main_lv != 0 && info.level > actor.main_lv)
}

/// The two colours retail paints item rows with around the equip windows, beyond the shared
/// cursor/disabled pair. ARGB words from FFXiMain.dll retail-2026-09's per-row colour resolvers
/// (`.text 0x1DE143`, `.text 0x20F745`): the same-id-equip-slot scan result wins, else a row the
/// legality check accepts paints the wearable-now lime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuRowTint {
    /// ARGB 0x80909040 — wearable right now (`.text FFXiMain.dll retail-2026-09 0x1DE18D`).
    WearableNow,
    /// ARGB 0x80509050 — this id already stands in a scanned equip slot
    /// (`.text FFXiMain.dll retail-2026-09 0x1DE173`); beats the lime.
    SameKindEquipped,
    /// A furniture list row for a piece placed on this floor
    /// ([`ffxi_dat::furniture_arrange::LIST_PLACED_TINT`]).
    FurniturePlaced,
}

/// The slots `.text FFXiMain.dll retail-2026-09 0x131FA0` walks when looking for an already-
/// equipped copy of a row's id: its on-stack table is {0, 8, 9, A, B, C, D, E, F} — the ear/
/// ring/back group plus slot 0. The scan compares the equipped item *ids*, not slots.
pub const EQUIP_COLLISION_SCAN_SLOTS: [usize; 9] = [0, 8, 9, 0xA, 0xB, 0xC, 0xD, 0xE, 0xF];

/// [`EQUIP_COLLISION_SCAN_SLOTS`] applied to the snapshot's equipped table.
pub fn same_id_equipped_in_scanned_slots(equipped: &[Option<u16>; 16], item_no: u16) -> bool {
    EQUIP_COLLISION_SCAN_SLOTS
        .iter()
        .any(|&slot| equipped.get(slot).copied().flatten() == Some(item_no))
}

/// The resolver's choice of tints, in retail's own precedence order (`.text FFXiMain.dll
/// retail-2026-09 0x1DE157`: scan first, then legality; neither → no tint).
pub fn equip_row_tint(allows_now: bool, same_id_equipped: bool) -> Option<MenuRowTint> {
    if same_id_equipped {
        Some(MenuRowTint::SameKindEquipped)
    } else if allows_now {
        Some(MenuRowTint::WearableNow)
    } else {
        None
    }
}

/// One row per owned key item, ascending global id, name from the scraped LSB
/// table, unseen ids suffixed with [`KEY_ITEM_UNSEEN_SUFFIX`].
pub fn key_item_rows(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<DynamicMenuRow> {
    let mut ids: Vec<u16> = snap.key_items.clone();
    ids.sort_unstable();
    ids.into_iter()
        .map(|id| DynamicMenuRow {
            tint: None,
            label: key_item_row_label(id, snap.key_items_seen.binary_search(&id).is_ok()),
            action: DynamicMenuAction::KeyItem { id },
        })
        .collect()
}

/// The fill-order law of retail's `emote` list (bead kuluu-o9qt7.34): the stored
/// arrangement comes first in its stored order (ids the client no longer knows drop),
/// and every default id the store omits keeps its default place behind the arranged
/// ones — the same shape as retail's fill, which reads one whole user blob at open
/// (FFXiMain.dll retail-2026-09 `.text 0x2259C6`) rather than patching rows.
/// `default_ids` is the full id table (hidden ids included — they stay in the order).
pub fn merge_emote_order(default_ids: &[u8], stored: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(default_ids.len());
    for &id in stored {
        if default_ids.contains(&id) && !out.contains(&id) {
            out.push(id);
        }
    }
    for &id in default_ids {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// The swap helper's guard law (.text FFXiMain.dll retail-2026-09 `0x2258D0`): same slot,
/// either index out of range (a missing id here), or equal row ids — all no-ops; otherwise the
/// two positions exchange and true is returned (the caller then refills, bead kuluu-o9qt7.34).
pub fn swap_emote_order(order: &mut [u8], first: u8, second: u8) -> bool {
    if first == second {
        return false;
    }
    let Some(a) = order.iter().position(|&id| id == first) else {
        return false;
    };
    let Some(b) = order.iter().position(|&id| id == second) else {
        return false;
    };
    if a == b {
        return false;
    }
    order.swap(a, b);
    true
}

/// The committed shape of one swap: merge the player's stored order over the full id
/// table, then run the guard-law swap (FFXiMain.dll retail-2026-09 `.text 0x2258D0`).
/// `None` when the guards no-op it — same slot, or an id missing from the list — so the
/// caller keeps the pending state exactly as retail does (bead kuluu-o9qt7.34).
pub fn committed_emote_swap(
    snap: &kuluu_snapshot::SceneSnapshot,
    first: u8,
    second: u8,
) -> Option<Vec<u8>> {
    let defaults: Vec<u8> = ffxi_vocab::emote_names::EMOTES
        .iter()
        .map(|&(id, _)| id)
        .collect();
    let mut order = merge_emote_order(&defaults, &snap.emote_order);
    if swap_emote_order(&mut order, first, second) {
        Some(order)
    } else {
        None
    }
}

/// Append the `abisortw` chain row (the focused list's tail; [I] reading of retail's
/// two-window pair — see the refresh arm) when its mode-1 caption has loaded from DAT;
/// with no install the list simply carries no sort entry (bead kuluu-o9qt7.34).
pub fn append_abisortw_chain_row(rows: &mut Vec<DynamicMenuRow>, caption: Option<&str>) {
    if let Some(caption) = caption {
        rows.push(DynamicMenuRow {
            tint: None,
            label: caption.to_string(),
            action: DynamicMenuAction::AbisortwSelect { mode: 1 },
        });
    }
}

/// One row per canned emote from the scraped LSB table: label + "/command" column.
/// HELM-only ids are server-initiated and skipped; the Job row appears only when the
/// 0x11A bitfield unlocks the current main job's gesture (bit = job id - 1). Display
/// order follows [`merge_emote_order`] over the player's stored arrangement — retail's
/// fill reads its user blob whole at open (FFXiMain.dll retail-2026-09 `.text 0x2259C6`;
/// bead kuluu-o9qt7.34) — so a committed swap re-orders the rows on the next refresh.
pub fn emote_rows(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<DynamicMenuRow> {
    use ffxi_proto::map::emote;
    let main_job = snap
        .self_char_id
        .and_then(|id| snap.party.iter().find(|m| m.id == id))
        .map(|m| m.main_job)
        .unwrap_or(0);
    let defaults: Vec<u8> = ffxi_vocab::emote_names::EMOTES
        .iter()
        .map(|&(id, _)| id)
        .collect();
    merge_emote_order(&defaults, &snap.emote_order)
        .into_iter()
        .filter(|&id| !emote::HELM_ONLY.contains(&id))
        .filter(|&id| {
            id != emote::JOB
                || snap
                    .emote_jobs
                    .is_some_and(|bits| ffxi_proto::decode::EmoteList::job_bit_set(bits, main_job))
        })
        .map(|id| DynamicMenuRow {
            tint: None,
            label: format!(
                "{} (/{})",
                ffxi_vocab::emote_names::lookup(id).unwrap_or("?"),
                emote_command_word(id)
            ),
            action: DynamicMenuAction::Emote { emote_id: id },
        })
        .collect()
}

/// The slash-command word for an emote id — the scraped name lowercased,
/// except Job whose command is /jobemote.
pub fn emote_command_word(id: u8) -> String {
    if id == ffxi_proto::map::emote::JOB {
        return "jobemote".to_string();
    }
    ffxi_vocab::emote_names::lookup(id)
        .map(str::to_lowercase)
        .unwrap_or_else(|| format!("emote{id}"))
}

/// The by-name order the Gameplay page's second arm selects. Only the label is retail's law (the
/// DAT caption "Sort items by name …"); case-folding and the id tie-break keep kuluu's ordering
/// total and stable, since no comparator in the traced region shows how retail breaks ties.
fn compare_item_rows_by_name(a: &DynamicMenuRow, b: &DynamicMenuRow) -> std::cmp::Ordering {
    let name = |row: &DynamicMenuRow| row.label.to_lowercase();
    name(a).cmp(&name(b)).then_with(|| {
        item_row_sort_key(a)
            .cmp(&item_row_sort_key(b))
            // Distinct slots of the same stack share both label and id.
            .then_with(|| match (a.action, b.action) {
                (
                    DynamicMenuAction::OpenItemAction { index: left, .. },
                    DynamicMenuAction::OpenItemAction { index: right, .. },
                ) => left.cmp(&right),
                _ => std::cmp::Ordering::Equal,
            })
    })
}

fn item_row_sort_key(row: &DynamicMenuRow) -> u16 {
    match row.action {
        DynamicMenuAction::OpenItemAction { item_no, .. } => item_no,
        DynamicMenuAction::UseItem { item_no, .. } => item_no,
        _ => u16::MAX,
    }
}

/// LSB 0x037 item-use gate (vendor/server/src/map/packets/c2s — 0x037 item
/// use → CState checks + `item_usable`): an item can fire right now iff it
/// appears in `item_usable` and either
/// - it is a plain consumable (maxCharges == 0) sitting unlocked in
///   LOC_INVENTORY or LOC_TEMPITEMS, or
/// - it is charged equipment (maxCharges > 0) that is currently equipped
///   (equipping is what locks the slot; the equipped item ids are mirrored
///   in `SceneSnapshot::equipped`).
pub fn item_usable_now(
    snap: &kuluu_snapshot::SceneSnapshot,
    slot: &kuluu_snapshot::InventoryItem,
) -> bool {
    use ffxi_proto::map::container as c;

    let Some(info) = ffxi_vocab::item_usable::lookup(slot.item_no) else {
        return false;
    };
    let is_equipment = ffxi_vocab::equip_info::lookup(slot.item_no).is_some();
    if is_equipment {
        // Charged equipment: usable only while equipped. Equipment without
        // charges never fires from the menu even though it may sit in
        // item_usable (enchantment already consumed server-side).
        info.max_charges > 0
            && slot.locked
            && snap.equipped.contains(&Some(slot.item_no))
            && (slot.container == c::LOC_INVENTORY || c::is_wardrobe(slot.container))
    } else {
        // Consumables: LOC_INVENTORY / LOC_TEMPITEMS only, and never from a
        // locked (bazaar/linkshell-reserved) slot.
        (slot.container == c::LOC_INVENTORY || slot.container == c::LOC_TEMPITEMS) && !slot.locked
    }
}

/// Rows for the Command Menu "Items" submenu: every currently-usable item
/// across all containers, firing Use directly (kuluu-268h), in item-id order.
pub fn usable_item_rows(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<DynamicMenuRow> {
    let mut rows: Vec<DynamicMenuRow> = snap
        .containers
        .iter()
        .flat_map(|cont| cont.items.iter())
        .filter(|slot| item_usable_now(snap, slot))
        .filter_map(|slot| {
            let name = ffxi_vocab::item_names::lookup(slot.item_no)?;
            Some(DynamicMenuRow {
                tint: None,
                label: name.to_string(),
                action: DynamicMenuAction::UseItem {
                    container: slot.container,
                    index: slot.index,
                    item_no: slot.item_no,
                },
            })
        })
        .collect();
    // The quick-menu list is id-ordered whatever the player picked on the Gameplay page.
    order_dynamic_rows(MenuKind::UsableItems, &ALWAYS_CATEGORY_SORT, &mut rows);
    rows
}

/// Whether the quick-menu "Items" entry should be enabled at all.
pub fn any_usable_item(snap: &kuluu_snapshot::SceneSnapshot) -> bool {
    snap.containers
        .iter()
        .flat_map(|cont| cont.items.iter())
        .any(|slot| item_usable_now(snap, slot))
}

pub const ITEM_ACTION_USE: &str = "Use";

pub const ITEM_ACTION_DROP: &str = "Drop";

/// Context rows for one slot: retail's Use and Drop (Use is listed for every
/// item and silently refused when the LSB 0x037 gate would reject it, see
/// `item_usable_now`), then the moves LSB 0x029 allows
/// (vendor/server/src/map/packets/c2s/0x029_item_move.cpp): Gil and Temporary
/// items never move, wardrobes only take equipment, and "Take Out" leads when
/// browsing a storage bag. Drop follows 0x028's refusals
/// (vendor/server/src/map/packets/c2s/0x028_item_dump.cpp process): never Gil
/// or a busy slot (vendor/server/src/map/items/item.cpp CItem::isBusy:
/// equipped, bazaar-priced, placed furniture or mid-transaction; the 0x020
/// LockFlg byte this reads mirrors all but the transient transaction state).
pub fn item_action_rows(
    snap: &kuluu_snapshot::SceneSnapshot,
    container: u8,
    index: u8,
    item_no: u16,
) -> Vec<DynamicMenuRow> {
    use ffxi_proto::map::container as c;

    let Some(slot) = snap
        .container(container)
        .and_then(|v| v.items.iter().find(|s| s.index == index))
        .filter(|s| s.item_no == item_no)
    else {
        return Vec::new();
    };

    let mut rows = vec![DynamicMenuRow {
        tint: None,
        label: ITEM_ACTION_USE.to_string(),
        action: DynamicMenuAction::UseItem {
            container,
            index,
            item_no,
        },
    }];

    // Locked = equipped / linkshell / bazaar-reserved: the server rejects the
    // move (and the drop) silently, so don't offer them. The recycle bin is
    // the one container the 0x028 ITEM_DUMP handler does not accept as a
    // source (vendor/server/src/map/packets/c2s/0x028_item_dump.cpp).
    let droppable =
        item_no != ffxi_proto::map::GIL_ITEM_NO && !slot.locked && container != c::LOC_RECYCLEBIN;
    if droppable {
        rows.push(DynamicMenuRow {
            tint: None,
            label: ITEM_ACTION_DROP.to_string(),
            action: DynamicMenuAction::DropItem {
                container,
                index,
                item_no,
                quantity: slot.quantity,
            },
        });
    }
    let movable =
        item_no != ffxi_proto::map::GIL_ITEM_NO && !slot.locked && container != c::LOC_TEMPITEMS;
    if movable {
        let equipable = ffxi_vocab::equip_info::lookup(item_no).is_some();
        for dest in crate::hud::item_screen::accessible_containers(snap) {
            if dest == container || dest == c::LOC_TEMPITEMS || (c::is_wardrobe(dest) && !equipable)
            {
                continue;
            }
            // 0x029 item-move: recycle-bin items only come back out to the
            // inventory or the locker
            // (vendor/server/src/map/packets/c2s/0x029_item_move.cpp).
            if container == c::LOC_RECYCLEBIN
                && dest != c::LOC_INVENTORY
                && dest != c::LOC_MOGLOCKER
            {
                continue;
            }
            let Some(name) = c::name(dest) else { continue };
            let label = if dest == c::LOC_INVENTORY {
                "Take Out".to_string()
            } else {
                format!("Put in {name}")
            };
            rows.push(DynamicMenuRow {
                tint: None,
                label,
                action: DynamicMenuAction::MoveItem {
                    quantity: slot.quantity,
                    from_container: container,
                    from_slot: index,
                    to_container: dest,
                    item_no,
                },
            });
        }
    }
    rows
}

/// One row name from the weapon-skill id space. `COMMAND_DATA`'s three bitmaps are
/// three different id spaces: vendor/server/src/map/packets/s2c/0x0ac_command_data.cpp
/// copies WeaponSkills from `m_WeaponSkills`, whose accessors key on a WeaponSkillID
/// (`charutils::hasWeaponSkill`), while JobAbilities/PetAbilities come from
/// `m_Abilities`/`m_PetCommands`, whose accessors key on an AbilityID
/// (`charutils::hasPetAbility`). So weapon skills must be named from the scraped
/// `weapon_skills` table (`ffxi_vocab::weapon_skill_names`) and never from the abilities
/// table, where id 32 is a goblin's Warcry rather than Fast Blade.
fn weapon_skill_row_name(id: u16) -> Option<&'static str> {
    ffxi_vocab::weapon_skill_names::lookup(id)
}

/// The flat list behind kuluu's Retail+ "Flat Abilities List" option: the three
/// known-id arrays at once (job abilities, weapon skills, pet commands).
/// Retail has no such window — its abimenu always drills one category first (vault
/// main_menu.md §23) — so the suffixed labels exist here only to keep the three id
/// spaces apart in a single column.
pub fn ability_flat_rows(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<DynamicMenuRow> {
    let mut out: Vec<DynamicMenuRow> = Vec::with_capacity(
        snap.job_abilities_known.len()
            + snap.weaponskills_known.len()
            + snap.pet_abilities_known.len(),
    );
    out.extend(snap.job_abilities_known.iter().filter_map(|&id| {
        ffxi_vocab::ability_names::lookup(id).map(|name| DynamicMenuRow {
            tint: None,
            label: name.to_string(),
            action: DynamicMenuAction::JobAbility { ability_id: id },
        })
    }));
    out.extend(snap.weaponskills_known.iter().filter_map(|&id| {
        weapon_skill_row_name(id).map(|name| DynamicMenuRow {
            tint: None,
            label: format!("{name} (WS)"),
            action: DynamicMenuAction::Weaponskill { skill_id: id },
        })
    }));
    out.extend(snap.pet_abilities_known.iter().filter_map(|&id| {
        ffxi_vocab::ability_names::lookup(id).map(|name| DynamicMenuRow {
            tint: None,
            label: format!("{name} (Pet)"),
            action: DynamicMenuAction::PetAbility { ability_id: id },
        })
    }));
    out
}

/// One category list of the `ability` window (vault main_menu.md §23): the known ids
/// of one group, named from the server-side ability id space so a row's label and
/// its c2s 0x01A payload can never disagree.
pub fn ability_group_rows(
    snap: &kuluu_snapshot::SceneSnapshot,
    group: crate::hud::action_model::AbilityGroup,
) -> Vec<DynamicMenuRow> {
    use crate::hud::action_model::AbilityGroup as G;
    let mut rows: Vec<DynamicMenuRow> = match group {
        G::JobAbilities => snap
            .job_abilities_known
            .iter()
            .filter_map(|&id| {
                ffxi_vocab::ability_names::lookup(id).map(|name| DynamicMenuRow {
                    tint: None,
                    label: name.to_string(),
                    action: DynamicMenuAction::JobAbility { ability_id: id },
                })
            })
            .collect(),
        G::WeaponSkill => snap
            .weaponskills_known
            .iter()
            .filter_map(|&id| {
                weapon_skill_row_name(id).map(|name| DynamicMenuRow {
                    tint: None,
                    label: name.to_string(),
                    action: DynamicMenuAction::Weaponskill { skill_id: id },
                })
            })
            .collect(),
        G::PetCommand => snap
            .pet_abilities_known
            .iter()
            .filter_map(|&id| {
                ffxi_vocab::ability_names::lookup(id).map(|name| DynamicMenuRow {
                    tint: None,
                    label: name.to_string(),
                    action: DynamicMenuAction::PetAbility { ability_id: id },
                })
            })
            .collect(),
        G::RangedAttack => vec![DynamicMenuRow {
            tint: None,
            label: "Ranged Attack".to_string(),
            action: DynamicMenuAction::RangedAttack,
        }],
        // Retail's mounted commands are exactly these two — no other 0x01A
        // action id is gated on being mounted
        // (GP_CLI_COMMAND_ACTION_ACTIONID,
        // vendor/server/src/map/packets/c2s/0x01a_action.h). They only
        // exist while riding, which is also when this group has anything in it.
        G::Mount if snap.self_mount.is_some() => vec![
            DynamicMenuRow {
                tint: None,
                label: "Dismount".to_string(),
                action: DynamicMenuAction::Dismount,
            },
            DynamicMenuRow {
                tint: None,
                label: "Dig".to_string(),
                action: DynamicMenuAction::ChocoboDig,
            },
        ],
        G::Mount => Vec::new(),
    };
    rows.sort_by(|a, b| a.label.cmp(&b.label));
    rows
}

pub fn ability_group_empty_hint(group: crate::hud::action_model::AbilityGroup) -> &'static str {
    use crate::hud::action_model::AbilityGroup as G;
    match group {
        G::RangedAttack => "You cannot use that command here.",
        G::Mount => "You must be mounted to use that command.",
        _ => "No abilities available.",
    }
}

/// One framed menu window's content for a frame: the focused top level, and
/// the level mirrored into the companion frame under it when one is open.
#[derive(Clone, Copy)]
struct PaneView {
    kind: MenuKind,
    cursor: usize,
}

#[allow(clippy::type_complexity)]
pub fn update_main_menu(
    mode: Res<InputMode>,
    settings: Res<GraphicsSettings>,
    panels: Res<crate::hud::HudPanels>,
    net_status: Res<crate::hud::network_status::NetStatusVisible>,
    vana_clock: Option<Res<crate::vana_time::VanaClock>>,
    #[cfg(not(target_arch = "wasm32"))] audio_mute: Res<crate::audio::AudioMuteState>,

    scene: Res<crate::snapshot::SceneState>,
    dynamic: Res<DynamicMenu>,
    config_rows: Res<ConfigDatRows>,
    dock: Option<Res<crate::hud::dock::DockBoard>>,
    mut menu_q: Query<
        &mut Node,
        (
            With<MainMenu>,
            Without<MainMenuRow>,
            Without<MainMenuPane>,
            Without<MainMenuCompanionFrame>,
        ),
    >,
    mut pane_q: Query<
        (&mut Node, &mut ZIndex, &ComputedNode),
        (
            With<MainMenuPane>,
            Without<MainMenuRow>,
            Without<MainMenuCompanionFrame>,
            Without<CompanionRow>,
        ),
    >,
    // One fill for both windows: mirror nodes carry the same title/row
    // components, and the `Option<&Companion*>` markers pick which view they
    // show (vault main_menu.md §59).
    #[allow(clippy::type_complexity)] mut row_q: Query<
        (
            &MainMenuRow,
            Option<&CompanionRow>,
            &mut Node,
            &mut Text,
            &mut TextColor,
            &mut BackgroundColor,
        ),
        Without<MainMenuTitle>,
    >,
    mut title_q: Query<(&MainMenuTitle, Option<&CompanionTitle>, &mut Text)>,
    mut comp_frame_q: Query<
        (&mut Node, &mut ZIndex, &ComputedNode),
        (
            With<MainMenuCompanionFrame>,
            Without<MainMenuRow>,
            Without<MainMenuPane>,
            Without<MainMenu>,
        ),
    >,
) {
    let Ok(mut node) = menu_q.single_mut() else {
        return;
    };
    let dock = dock.as_deref();
    let dock_y = |want: &mut Node, kind: MenuKind, computed: &ComputedNode| {
        if let Some((board, placement)) =
            dock.and_then(|board| Some((board, dock_placement(kind, board)?)))
        {
            crate::hud::dock::place_docked(
                want,
                placement,
                crate::hud::dock::drawn_height(computed),
                board.surface_height,
            );
        }
    };

    let hide = |node: &mut Node| {
        if node.display != Display::None {
            node.display = Display::None;
        }
    };

    let stack = match &*mode {
        InputMode::Menu(stack) => stack,
        _ => {
            hide(&mut node);
            return;
        }
    };
    let Some(top_kind) = stack.current().map(|l| l.kind) else {
        hide(&mut node);
        return;
    };
    // The Equipment / Items screens render their own framed multi-panel layout;
    // suppress the generic two-pane while one is on top.
    if renders_bespoke_screen(top_kind) {
        hide(&mut node);
        return;
    }
    if node.display != Display::Flex {
        node.display = Display::Flex;
    }
    let want_width = pane_width_for(top_kind);
    let layout = menu_stack_layout(stack, &config_rows);
    let Some((view_index, view_seat)) = layout.focused() else {
        hide(&mut node);
        return;
    };
    for (mut pane_node, mut z_index, computed) in pane_q.iter_mut() {
        let mut want = pane_node.clone();
        match view_seat {
            MenuWindowSeat::Record(frame) => place_at_record_frame(&mut want, &frame),
            _ => place_in_column(&mut want, MENU_RIGHT_EDGE, want_width),
        }
        dock_y(&mut want, stack.levels[view_index].kind, computed);
        pane_node.set_if_neq(want);
        z_index.set_if_neq(stack_z_index(view_index));
    }

    let level_view = |index: usize| {
        stack.levels.get(index).map(|l| PaneView {
            kind: l.kind,
            cursor: l.cursor,
        })
    };
    let view = level_view(view_index);
    let column_window = layout.column_window();
    let parent_view = column_window.and_then(|(index, _)| level_view(index));
    let parent_rows = parent_view.map_or(&[][..], |p| stacked_rows_for(p.kind, &dynamic));

    for (_, comp, mut text) in title_q.iter_mut() {
        let want = if comp.is_some() {
            parent_view
                .map(|p| menu_title(p.kind).to_string())
                .unwrap_or_default()
        } else {
            view.as_ref().map(root_aware_title).unwrap_or_default()
        };
        if **text != want {
            **text = want;
        }
    }

    let Some(view) = view else {
        for (_, _, mut row_node, _, _, _) in row_q.iter_mut() {
            hide(&mut row_node);
        }
        return;
    };

    let (total, viewport_start) =
        resolve_viewport(view.kind, view.cursor, &dynamic, settings.dlss_supported);
    let window = visible_window(view.kind, total);
    // Mirror-side totals for the selector frame.
    let parent_total = parent_view
        .map(|p| rows_entry_count(p.kind, parent_rows, settings.dlss_supported))
        .unwrap_or(0);
    let parent_window = parent_view
        .map(|p| visible_window(p.kind, parent_total))
        .unwrap_or(0);
    let clock_debug_hold = vana_clock
        .as_ref()
        .is_some_and(|clock| clock.debug_hold_active());

    for (row, comp, mut row_node, mut text, mut color, mut bg) in row_q.iter_mut() {
        if comp.is_some() {
            // Mirror row: the selector's list, read-only (vault main_menu.md §59).
            let Some(parent) = parent_view else {
                hide(&mut row_node);
                continue;
            };
            if row.slot >= parent_window || row.slot >= parent_total {
                hide(&mut row_node);
                continue;
            }
            if row_node.display != Display::Flex {
                row_node.display = Display::Flex;
            }
            let label: &str = if is_dynamic(parent.kind) {
                rows_entry_label(parent.kind, row.slot, parent_rows, &dynamic)
            } else {
                entries_for(parent.kind, settings.dlss_supported)
                    .get(row.slot)
                    .copied()
                    .unwrap_or("<unknown>")
            };
            let want = if parent.cursor == row.slot {
                format!("> {label}")
            } else {
                format!("  {label}")
            };
            if **text != want {
                **text = want;
            }
            if color.0 != theme::TEXT {
                color.0 = theme::TEXT;
            }
            if bg.0 != Color::NONE {
                bg.0 = Color::NONE;
            }
            continue;
        }
        let list_idx = viewport_start + row.slot;
        if row.slot >= window || list_idx >= total {
            hide(&mut row_node);
            continue;
        }
        if row_node.display != Display::Flex {
            row_node.display = Display::Flex;
        }

        let label_owned: String = if is_dynamic(view.kind) {
            entry_label(view.kind, list_idx, &dynamic).to_string()
        } else {
            entries_for(view.kind, settings.dlss_supported)
                .get(list_idx)
                .copied()
                .unwrap_or("<unknown>")
                .to_string()
        };
        #[cfg(not(target_arch = "wasm32"))]
        let (sound_on, master_pct) = (!(audio_mute.bgm && audio_mute.sfx), audio_mute.master_pct());
        #[cfg(target_arch = "wasm32")]
        let (sound_on, master_pct) = (false, 0);
        let body = format_row_body(
            view.kind,
            list_idx,
            &label_owned,
            &settings,
            &panels,
            net_status.0,
            sound_on,
            clock_debug_hold,
            master_pct,
            &scene.snapshot,
        );

        let is_cursor = view.cursor == list_idx;
        // A `statcom2` row the client will not answer draws grey with the cursor
        // still on it: retail disables ids 3/4/5/12/13 when the list opens
        // (FFXiMain.dll retail-2026-09 `.text 0x148B50`), and its dim is exactly half
        // of ordinary grey — the same pair read for roomlist's gated row, where a
        // disabled line paints 0x80404040 instead of 0x80808080 (`.text 0x1D6A1A`).
        let gated = menu_row_disabled(view.kind, list_idx, &scene.snapshot);
        // Beyond cursor/grey, a builder may hand the row one of retail's two item tints —
        // wearable-now lime or same-kind-equipped green (`.text FFXiMain.dll retail-2026-09
        // 0x1DE143` paints these regardless of selection; only the dim outranks them).
        let tint = if is_dynamic(view.kind) {
            dynamic.rows.get(list_idx).and_then(|r| r.tint)
        } else {
            None
        };
        let base_color = menu_row_color(gated, tint, is_cursor);
        let (want, want_color, want_bg) = if is_cursor {
            (format!("> {body}"), base_color, Color::NONE)
        } else {
            (format!("  {body}"), base_color, Color::NONE)
        };
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

    let (Some(parent), Some((parent_index, parent_seat))) = (parent_view, column_window) else {
        for (mut frame, _, _) in comp_frame_q.iter_mut() {
            hide(&mut frame);
        }
        return;
    };
    let mirror_width = pane_width_for(parent.kind);
    let right = match parent_seat {
        MenuWindowSeat::BesideColumn => MENU_RIGHT_EDGE + mirror_width,
        _ => MENU_RIGHT_EDGE,
    };
    for (mut frame, mut z_index, computed) in comp_frame_q.iter_mut() {
        let mut want = frame.clone();
        place_in_column(&mut want, right, mirror_width);
        dock_y(&mut want, parent.kind, computed);
        want.display = Display::Flex;
        frame.set_if_neq(want);
        z_index.set_if_neq(stack_z_index(parent_index));
    }
}

/// Puts a window node in kuluu's selector column, `right` px in from the right screen edge.
fn place_in_column(node: &mut Node, right: f32, width: f32) {
    node.left = Val::Auto;
    node.top = Val::Px(MENU_COLUMN_TOP);
    node.right = Val::Px(right);
    node.bottom = Val::Auto;
    node.margin = UiRect::ZERO;
    node.width = Val::Px(width);
}

/// Open windows share one draw priority, so the newer one draws on top.
fn stack_z_index(stack_index: usize) -> ZIndex {
    ZIndex(i32::try_from(stack_index).unwrap_or(i32::MAX))
}

/// The rows a dynamic window open under the top one lists.
fn stacked_rows_for(kind: MenuKind, dynamic: &DynamicMenu) -> &[DynamicMenuRow] {
    dynamic
        .stacked_rows
        .iter()
        .find(|(open, _)| *open == kind)
        .map_or(&[], |(_, rows)| rows.as_slice())
}

/// [`entry_count`] over a given window's rows.
fn rows_entry_count(kind: MenuKind, rows: &[DynamicMenuRow], dlss_supported: bool) -> usize {
    if is_dynamic(kind) {
        rows.len().max(1)
    } else {
        entries_for(kind, dlss_supported).len()
    }
}

/// [`entry_label`] over a given window's rows.
fn rows_entry_label<'a>(
    kind: MenuKind,
    idx: usize,
    rows: &'a [DynamicMenuRow],
    dynamic: &'a DynamicMenu,
) -> &'a str {
    if !is_dynamic(kind) {
        return entry_label(kind, idx, dynamic);
    }
    match rows.get(idx) {
        Some(row) => &row.label,
        None if rows.is_empty() => empty_dynamic_hint(kind),
        None => "<unknown>",
    }
}

/// A row's text colour: grey when the client will not answer it, then the builder's tint, then
/// cursor gold on the focused window's cursor row.
fn menu_row_color(gated: bool, tint: Option<MenuRowTint>, lit_cursor: bool) -> Color {
    if gated {
        return theme::TEXT_DISABLED;
    }
    match tint {
        Some(MenuRowTint::SameKindEquipped) => theme::TEXT_EQUIP_SAME_KIND,
        Some(MenuRowTint::WearableNow) => theme::TEXT_EQUIP_OK,
        Some(MenuRowTint::FurniturePlaced) => {
            let [r, g, b, a] = ffxi_dat::furniture_arrange::LIST_PLACED_TINT;
            crate::hud::unity_fp::retail_text_color(crate::hud::unity_fp::retail_argb(a, r, g, b))
        }
        None if lit_cursor => theme::CURSOR,
        None => theme::TEXT,
    }
}

/// Draws the open windows under the focused one that sit at their own record frames, each with
/// its own rows and cursor, newer ones on top
/// (.agents/skills/retail-observe/references/2026-10-15-menu-window-stacking.md).
#[allow(clippy::type_complexity)]
pub fn update_menu_stack_windows(
    mode: Res<InputMode>,
    settings: Res<GraphicsSettings>,
    scene: Res<crate::snapshot::SceneState>,
    dynamic: Res<DynamicMenu>,
    config_rows: Res<ConfigDatRows>,
    dock: Option<Res<crate::hud::dock::DockBoard>>,
    mut window_q: Query<(&MenuStackWindow, &mut Node, &mut ZIndex, &ComputedNode)>,
    mut title_q: Query<(&MenuStackWindowTitle, &mut Text), Without<MenuStackWindowRow>>,
    mut row_q: Query<
        (&MenuStackWindowRow, &mut Node, &mut Text, &mut TextColor),
        Without<MenuStackWindow>,
    >,
) {
    struct Shown<'a> {
        stack_index: usize,
        frame: RecordFrame,
        level: &'a MenuLevel,
        rows: &'a [DynamicMenuRow],
        total: usize,
        start: usize,
        visible: usize,
    }
    let shown: Vec<Shown> = match &*mode {
        InputMode::Menu(stack) => {
            let records = menu_stack_layout(stack, &config_rows).record_windows();
            let newest = &records[records.len().saturating_sub(MAX_STACKED_WINDOWS)..];
            newest
                .iter()
                .filter_map(|&(stack_index, frame)| {
                    let level = stack.levels.get(stack_index)?;
                    let rows = stacked_rows_for(level.kind, &dynamic);
                    let count = rows_entry_count(level.kind, rows, settings.dlss_supported);
                    let (total, start) = viewport_over(level.kind, level.cursor, count);
                    Some(Shown {
                        stack_index,
                        frame,
                        level,
                        rows,
                        total,
                        start,
                        visible: visible_window(level.kind, total),
                    })
                })
                .collect()
        }
        _ => Vec::new(),
    };

    for (window, mut node, mut z_index, computed) in window_q.iter_mut() {
        let mut want = node.clone();
        match shown.get(window.slot) {
            Some(open) => {
                place_at_record_frame(&mut want, &open.frame);
                if let Some((board, placement)) = dock
                    .as_deref()
                    .and_then(|board| Some((board, dock_placement(open.level.kind, board)?)))
                {
                    crate::hud::dock::place_docked(
                        &mut want,
                        placement,
                        crate::hud::dock::drawn_height(computed),
                        board.surface_height,
                    );
                }
                want.display = Display::Flex;
                z_index.set_if_neq(stack_z_index(open.stack_index));
            }
            None => want.display = Display::None,
        }
        node.set_if_neq(want);
    }
    for (title, mut text) in title_q.iter_mut() {
        let want = shown
            .get(title.window)
            .map_or("", |open| menu_title(open.level.kind));
        if **text != *want {
            **text = want.to_string();
        }
    }
    for (row, mut node, mut text, mut color) in row_q.iter_mut() {
        let Some(open) = shown.get(row.window) else {
            continue;
        };
        let idx = open.start + row.slot;
        if row.slot >= open.visible || idx >= open.total {
            if node.display != Display::None {
                node.display = Display::None;
            }
            continue;
        }
        if node.display != Display::Flex {
            node.display = Display::Flex;
        }
        let kind = open.level.kind;
        let label = rows_entry_label(kind, idx, open.rows, &dynamic);
        let want = format!("{}{label}", style::cursor_prefix(open.level.cursor == idx));
        if **text != want {
            **text = want;
        }
        let gated = menu_row_disabled(kind, idx, &scene.snapshot);
        let tint = open.rows.get(idx).and_then(|r| r.tint);
        let want_color = menu_row_color(gated, tint, false);
        if color.0 != want_color {
            color.0 = want_color;
        }
    }
}

/// statcom2's detail windows, drawn beside the persona panel at their own record
/// frames — all `(x=130,y=48,w≈252)` in the 512×448 design space (vault
/// main_menu.md §59 addendum 10) — never inside the Commands column. Retail's
/// detail rows carry no cursor and take no keys: the list under them keeps both
/// (FFXiMain.dll retail-2026-09 focus law + detail table RVA 0x3716B8 behind handler
/// 0x148D00; the close-details helper at 0x148AC0 only ever closes these).
#[derive(Component)]
pub struct StatusDetailWindow;

#[derive(Component)]
pub struct StatusDetailTitle;

#[derive(Component)]
pub struct StatusDetailLine {
    pub slot: usize,
}

/// The DAT record each STATUS_ENTRIES row opens, by detail-table order — the
/// table FFXiMain.dll retail-2026-09 RVA 0x148D00 dispatches statcom2 selects
/// through (vault main_menu.md §59). Row ≠ item id: Master Levels is row 3 but
/// id 12, hence the explicit per-row list. Play Time runs `/playtime` and opens
/// no window (§2 id 9).
pub const STATUS_DETAIL_WINDOW_NAMES: [Option<&str>; 13] = [
    Some("profile"),
    Some(crate::hud::status_panel::JOBLEVEL_WINDOW),
    Some("masterle"),
    Some("btlskill"),
    Some("mgcskill"),
    Some("trdskill"),
    Some("evitem"),
    Some("evitem"),
    Some("ut_menu"),
    None,
    Some("merit1"),
    Some("merit2ca"),
    Some("fp_cat"),
];

pub fn spawn_status_detail_window(mut commands: Commands) {
    let (mut node, bg, border) = style::window_frame();
    node.position_type = PositionType::Absolute;
    node.left = Val::Px(130.0);
    node.top = Val::Px(48.0);
    node.width = Val::Px(252.0);
    node.display = Display::None;
    commands
        .spawn((
            crate::components::InGameEntity,
            StatusDetailWindow,
            node,
            bg,
            border,
        ))
        .with_children(|w| {
            w.spawn((
                StatusDetailTitle,
                Text::new(""),
                text_nowrap(),
                style::text_font(14.0),
                TextColor(theme::TITLE),
            ));
            for slot in 0..MAX_ENTRY_COUNT {
                let line_node = Node {
                    display: Display::None,
                    ..default()
                };
                w.spawn((
                    StatusDetailLine { slot },
                    line_node,
                    Text::new(""),
                    text_nowrap(),
                    style::text_font(13.0),
                    TextColor(theme::TEXT),
                    BackgroundColor(Color::NONE),
                ));
            }
        });
}

/// Where Status puts a detail window instead of its record frame.
fn status_code_move(window: &str) -> Option<(f32, f32)> {
    use crate::hud::status_panel::{JOBLEVEL_STATUS_LEFT, JOBLEVEL_STATUS_TOP, JOBLEVEL_WINDOW};
    (window == JOBLEVEL_WINDOW).then_some((JOBLEVEL_STATUS_LEFT, JOBLEVEL_STATUS_TOP))
}

/// Places and fills the left-side windows of the Status flow from DAT frames:
/// the persona panel takes `persona`'s record width (fallback keeps kuluu's own
/// width when no install is present), and the detail window sits at its record
/// rect beside it, or where Status code-moves it. Also hides everything once the flow closes.
#[allow(clippy::type_complexity)]
pub fn update_status_detail_window(
    mode: Res<InputMode>,
    dynamic: Res<DynamicMenu>,
    dat_root: Option<Res<crate::dat_root::SharedDatRoot>>,
    mut config_rows: ResMut<ConfigDatRows>,
    mut frame_q: Query<&mut Node, With<StatusDetailWindow>>,
    mut title_q: Query<&mut Text, (With<StatusDetailTitle>, Without<MainMenuTitle>)>,
    mut line_q: Query<
        (&StatusDetailLine, &mut Text, &mut Node),
        (
            Without<StatusDetailTitle>,
            Without<StatusDetailWindow>,
            Without<crate::hud::status_panel::StatusPanel>,
        ),
    >,
    mut persona_width_q: Query<
        &mut Node,
        (
            With<crate::hud::status_panel::StatusPanel>,
            (
                Without<StatusDetailWindow>,
                Without<StatusDetailTitle>,
                Without<StatusDetailLine>,
            ),
        ),
    >,
) {
    config_rows.ensure_loaded(dat_root.as_ref().and_then(|r| r.get()).map(|v| &**v));
    let top = match &*mode {
        InputMode::Menu(stack) => stack.current().map(|l| l.kind),
        _ => None,
    };
    // The persona panel answers with its record width the moment an install is
    // readable; retail's box is that narrow next to the statcom2 list.
    let persona_w = config_rows
        .window_frame("persona")
        .map(|(_, _, w)| w.max(90.0));
    if let Some(w) = persona_w {
        for mut node in persona_width_q.iter_mut() {
            if node.width != Val::Px(w) {
                node.width = Val::Px(w);
            }
        }
    }

    let idx = match top {
        Some(MenuKind::StatusDetail { entry }) => usize::from(entry),
        _ => {
            for mut node in frame_q.iter_mut() {
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
            for (_, _, mut line_node) in line_q.iter_mut() {
                if line_node.display != Display::None {
                    line_node.display = Display::None;
                }
            }
            return;
        }
    };
    let mut node = match frame_q.single_mut() {
        Ok(node) => node,
        Err(_) => return,
    };
    let name = STATUS_DETAIL_WINDOW_NAMES
        .get(idx)
        .copied()
        .flatten()
        .unwrap_or("profile");
    let frame = config_rows.window_frame(name);
    if let Some((x, y)) = status_code_move(name).or(frame.map(|(x, y, _)| (x, y))) {
        node.left = Val::Px(x);
        node.top = Val::Px(y);
    }
    if let Some((_, _, w)) = frame {
        node.width = Val::Px(w);
    }
    if node.display != Display::Flex {
        node.display = Display::Flex;
    }
    let want_title = menu_title(MenuKind::StatusDetail {
        entry: u8::try_from(idx).unwrap_or(u8::MAX),
    })
    .to_string();
    if let Ok(mut title_text) = title_q.single_mut() {
        if **title_text != want_title {
            **title_text = want_title;
        }
    }
    for (line, mut text, mut line_node) in line_q.iter_mut() {
        let Some(row) = dynamic.rows.get(line.slot) else {
            if line_node.display != Display::None {
                line_node.display = Display::None;
            }
            continue;
        };
        if **text != row.label {
            **text = row.label.clone();
        }
        if line_node.display != Display::Flex {
            line_node.display = Display::Flex;
        }
    }
}

/// The pane title, with a `‹1/2›` page marker appended for the Root Command
/// menu so the two-page flip (retail's "-") is discoverable.
fn root_aware_title(view: &PaneView) -> String {
    let title = menu_title(view.kind);
    if view.kind == MenuKind::Root {
        let page = root_page_index(view.cursor) + 1;
        format!("{title}  \u{2039}{page}/{ROOT_PAGE_COUNT}\u{203a}")
    } else {
        title.to_string()
    }
}

fn visible_window(kind: MenuKind, total: usize) -> usize {
    if kind == MenuKind::Root {
        total
    } else {
        total.min(DYNAMIC_VISIBLE_ROWS)
    }
}

fn resolve_viewport(
    kind: MenuKind,
    cursor: usize,
    dynamic: &DynamicMenu,
    dlss_supported: bool,
) -> (usize, usize) {
    viewport_over(kind, cursor, entry_count(kind, dynamic, dlss_supported))
}

fn viewport_over(kind: MenuKind, cursor: usize, total: usize) -> (usize, usize) {
    if kind == MenuKind::Root {
        // Show only the page the cursor is on: rows [start, end).
        let (start, end) = root_page_bounds(cursor);
        return (end, start);
    }
    if total <= DYNAMIC_VISIBLE_ROWS {
        return (total, 0);
    }
    // A list taller than the viewport (e.g. the 27-row Graphics menu) scrolls a
    // fixed window that keeps the cursor centered, so its bottom rows stay
    // reachable at the default window height instead of overflowing off-screen.
    let half = DYNAMIC_VISIBLE_ROWS / 2;
    let max_start = total.saturating_sub(DYNAMIC_VISIBLE_ROWS);
    let start = cursor.saturating_sub(half).min(max_start);
    (total, start)
}

pub fn menu_mouse_hover_system(
    mut mode: ResMut<InputMode>,
    settings: Res<GraphicsSettings>,
    dynamic: Res<DynamicMenu>,
    rows: Query<(&Interaction, &MainMenuRow), Changed<Interaction>>,
) {
    let InputMode::Menu(stack) = &mut *mode else {
        return;
    };
    let Some(level_idx) = stack.levels.len().checked_sub(1) else {
        return;
    };
    let kind = stack.levels[level_idx].kind;
    // These windows draw their own rows (`unity_fp_view`); the pane's rows belong to the window
    // beneath them.
    if draws_itself(kind) {
        return;
    }
    for (interaction, row) in &rows {
        if !matches!(interaction, Interaction::Hovered | Interaction::Pressed) {
            continue;
        }
        let cursor = stack.levels[level_idx].cursor;
        let (total, viewport_start) =
            resolve_viewport(kind, cursor, &dynamic, settings.dlss_supported);
        let window = visible_window(kind, total);
        let list_idx = viewport_start + row.slot;
        if list_idx >= total || row.slot >= window || stack.levels[level_idx].row_locked(list_idx) {
            continue;
        }
        if stack.levels[level_idx].cursor != list_idx {
            stack.levels[level_idx].cursor = list_idx;
        }
    }
}

pub fn menu_mouse_click_system(
    mode: Res<InputMode>,
    settings: Res<GraphicsSettings>,
    dynamic: Res<DynamicMenu>,
    rows: Query<(&Interaction, &MainMenuRow), Changed<Interaction>>,
    mut out: MessageWriter<MenuRowActivated>,
) {
    let InputMode::Menu(stack) = &*mode else {
        return;
    };
    let Some(level) = stack.current() else {
        return;
    };
    let kind = level.kind;
    if draws_itself(kind) {
        return;
    }
    for (interaction, row) in &rows {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let (total, viewport_start) =
            resolve_viewport(kind, level.cursor, &dynamic, settings.dlss_supported);
        let window = visible_window(kind, total);
        let list_idx = viewport_start + row.slot;
        if list_idx < total && row.slot < window && !level.row_locked(list_idx) {
            out.write(MenuRowActivated { slot: list_idx });
        }
    }
}

fn format_row_body(
    kind: MenuKind,
    slot: usize,
    label: &str,
    settings: &GraphicsSettings,
    panels: &crate::hud::HudPanels,
    net_status_on: bool,
    sound_on: bool,
    clock_debug_hold: bool,
    master_pct: i32,
    snapshot: &kuluu_snapshot::SceneSnapshot,
) -> String {
    match kind {
        MenuKind::Config | MenuKind::Graphics | MenuKind::GraphicsDlss => {
            match settings_field_at(kind, slot, settings.dlss_supported) {
                Some(field) => format!(
                    "{:<16}[{}]",
                    format!("{}:", field.label()),
                    settings.value_label(field)
                ),
                None => label.to_string(),
            }
        }
        MenuKind::Debug => {
            if label == DEBUG_VOLUME {
                format!("{label:<14}[{master_pct:>3}]")
            } else if label == DEBUG_PRINT_POS {
                format!("{label:<14}[enter]")
            } else if label == DEBUG_RETAIL_SEPARATOR || label == DEBUG_RETAIL_LABEL {
                label.to_string()
            } else if label == RETAIL_DLSS_MENU {
                format!(
                    "{label:<14}[{}]",
                    if !settings.dlss_supported {
                        "N/A"
                    } else if settings.dlss_menu_enabled {
                        "on"
                    } else {
                        "off"
                    }
                )
            } else {
                #[cfg(feature = "enhanced-mob-hp-under")]
                if label == RETAIL_MOB_HP_UNDER {
                    return format!(
                        "{label:<14}[{}]",
                        if settings.mob_hp_under { "on" } else { "off" }
                    );
                }
                #[cfg(feature = "enhanced-job-display")]
                if label == RETAIL_JOB_DISPLAY {
                    return format!(
                        "{label:<14}[{}]",
                        if settings.job_display { "on" } else { "off" }
                    );
                }
                #[cfg(feature = "enhanced-ignore-knockback-self")]
                if label == RETAIL_KNOCKBACK {
                    return format!(
                        "{label:<14}[{}]",
                        if settings.ignore_knockback_self {
                            "off"
                        } else {
                            "on"
                        }
                    );
                }
                if label == RETAIL_STATUS_ROWS {
                    return format!(
                        "{label:<14}[{}]",
                        if settings.interactive_status_rows {
                            "on"
                        } else {
                            "off"
                        }
                    );
                }
                if label == RETAIL_RESELECT_UNEQUIP {
                    return format!(
                        "{label:<14}[{}]",
                        if settings.reselect_unequip {
                            "on"
                        } else {
                            "off"
                        }
                    );
                }
                if label == RETAIL_MAGIC_FLAT_LIST {
                    return format!(
                        "{label:<14}[{}]",
                        if settings.magic_flat_list {
                            "on"
                        } else {
                            "off"
                        }
                    );
                }
                if label == RETAIL_ABILITIES_FLAT_LIST {
                    return format!(
                        "{label:<14}[{}]",
                        if settings.abilities_flat_list {
                            "on"
                        } else {
                            "off"
                        }
                    );
                }
                if label == RETAIL_CAMERA_LEASH {
                    return format!("{label:<14}[{:.1}]", settings.camera_leash_yalms);
                }
                let on =
                    debug_panel_state(label, panels, net_status_on, sound_on, clock_debug_hold);
                format!("{label:<14}[{}]", if on { "on" } else { "off" })
            }
        }
        MenuKind::Equipment => {
            let item_name = snapshot
                .equipped
                .get(slot)
                .copied()
                .flatten()
                .and_then(ffxi_vocab::item_names::lookup)
                .unwrap_or("—");
            format!("{label:<7}: {item_name}")
        }
        _ => label.to_string(),
    }
}

pub fn debug_panel_state(
    label: &str,
    panels: &crate::hud::HudPanels,
    net_status_on: bool,
    sound_on: bool,
    clock_debug_hold: bool,
) -> bool {
    match label {
        DEBUG_PERF => panels.perf,
        DEBUG_TARGET_CYCLE => panels.target_cycle,
        DEBUG_MESH => panels.mesh_debug,
        DEBUG_NOCLIP => panels.noclip,
        #[cfg(feature = "enhanced-engage-move-lock-off")]
        DEBUG_ENGAGE_ANIM_LOCK => panels.engage_anim_lock,
        DEBUG_AUTO_ENTER_CS => panels.auto_enter_cs,
        DEBUG_WEATHER => !panels.weather_off,
        DEBUG_FOG => !panels.fog_off,
        DEBUG_FORCE_18 => clock_debug_hold,
        DEBUG_ENTITY_LIST => panels.entity_list,
        DEBUG_NET_STATUS => net_status_on,
        DEBUG_SOUND => sound_on,
        DEBUG_STAIR_DRAW => panels.stair_draw,
        DEBUG_STAIR_STATUS => panels.stair_debug,
        DEBUG_GRAPHICS_DEBUG => panels.graphics_debug,
        DEBUG_POSITION_LOG => panels.position_log,
        DEBUG_NAMEPLATES => panels.nameplate_debug,
        DEBUG_UI_SETTINGS => panels.ui_settings,
        _ => false,
    }
}

/// The Mog-house capital zones whose View House arm is gated (vault main_menu.md
/// §32): the `socialme` arm runs its veto predicate FFXiMain.dll retail-2026-09
/// RVA 0xF9680 only through this zone table — Chateau d'Oraguille and Metalworks
/// are allow-always, every other capital needs the in-house byte, all non-
/// capitals are unconditionally allowed. Same set as LSB's
/// `scripts/globals/moghouse.lua` moghouseZones ∩ capitals.
pub const MOGHOUSE_CAPITAL_ZONE_MIN: u16 = 230;
pub const MOGHOUSE_CAPITAL_ZONE_MAX: u16 = 241;
pub const CHATEAU_DORAGUILLE_ZONE: u16 = 233;
pub const METALWORKS_ZONE: u16 = 237;

/// View House (page-2 row 4) refuses with the arm's error sound when standing
/// in a Mog-house capital without being inside your own Mog House — the
/// FFXiMain.dll retail-2026-09 RVA 0x14E8F5 veto law read off handler 0x14E7B0
/// (vault main_menu.md §32). `self_in_mog_house` is the snapshot reading of the
/// predicate's byte operand.
pub fn view_house_blocked(zone_id: Option<u16>, self_in_mog_house: bool) -> bool {
    zone_id.is_some_and(|zone| {
        (MOGHOUSE_CAPITAL_ZONE_MIN..=MOGHOUSE_CAPITAL_ZONE_MAX).contains(&zone)
            && zone != CHATEAU_DORAGUILLE_ZONE
            && zone != METALWORKS_ZONE
            && !self_in_mog_house
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_recast_remaining_gates_on_cooldown_abilities() {
        let provoke = DynamicMenuAction::JobAbility { ability_id: 35 };
        let rid = ffxi_vocab::recast::ability_recast_id(35).expect("provoke has a recast group");
        // On cooldown: expiry 30s in the future -> Some(remaining).
        assert_eq!(
            action_recast_remaining(&[(rid, 1030)], &provoke, 1000),
            Some(30)
        );
        // Ready: expiry in the past -> None.
        assert_eq!(action_recast_remaining(&[(rid, 990)], &provoke, 1000), None);
        // No recast entry -> None.
        assert_eq!(action_recast_remaining(&[], &provoke, 1000), None);
        // Non-ability action -> None.
        assert_eq!(
            action_recast_remaining(
                &[(rid, 1030)],
                &DynamicMenuAction::CastSpell { spell_id: 1 },
                1000
            ),
            None
        );
    }

    #[test]
    fn recast_expiry_stamped_with_recast_now_unix_reads_back_undrifted() {
        // Guard for kuluu-t815: expiries are absolute seconds on the
        // recast_now_unix() clock, so a consumer passing that same clock as
        // `now` must read back the stamped duration (to within one tick).
        // The gate (text_input) and display (target_action_menu) both do; a
        // consumer on any other clock (e.g. VanaClock) would drift by the
        // client/server skew this test cannot see — keep them on this helper.
        const REMAINING_SECS: u32 = 45;
        let provoke = DynamicMenuAction::JobAbility { ability_id: 35 };
        let rid = ffxi_vocab::recast::ability_recast_id(35).expect("provoke has a recast group");
        let expiry = kuluu_snapshot::recast_now_unix() + REMAINING_SECS;

        let remaining = action_recast_remaining(
            &[(rid, expiry)],
            &provoke,
            kuluu_snapshot::recast_now_unix(),
        )
        .expect("ability still on recast");
        assert!(
            (REMAINING_SECS - 1..=REMAINING_SECS).contains(&remaining),
            "remaining {remaining} drifted from stamped {REMAINING_SECS}"
        );
    }

    #[test]
    fn root_pages_split_three_ways_by_cursor() {
        let p2 = ROOT_PAGE_1_LEN;
        let p3 = ROOT_PAGE_1_LEN + ROOT_PAGE_2_LEN;
        assert_eq!(root_page_bounds(0), (0, ROOT_PAGE_1_LEN));
        assert_eq!(root_page_bounds(p2 - 1), (0, ROOT_PAGE_1_LEN));
        assert_eq!(root_page_bounds(p2), (p2, p3));
        assert_eq!(root_page_bounds(p3), (p3, ROOT_ENTRIES.len()));

        // "Map" lives on page 1, retail's "Config" row on page 2.
        let map = ROOT_ENTRIES.iter().position(|e| *e == "Map").unwrap();
        let config = ROOT_ENTRIES.iter().position(|e| *e == "Config").unwrap();
        assert!(map < p2 && (p2..p3).contains(&config));

        // '-' / Right: 1->2->3->1, Left steps back; page 3's first row wraps to
        // page 1 keeping its (row-0) cursor.
        assert_eq!(root_next_page_cursor(0), p2);
        assert_eq!(root_prev_page_cursor(p2), 0);
        assert_eq!(root_next_page_cursor(p3), 0);

        // Same-row carry between the retail pages, and a clamp to page 3's last
        // row when its length is shorter.
        for row in 0..ROOT_PAGE_1_LEN.min(ROOT_PAGE_2_LEN) {
            assert_eq!(root_next_page_cursor(row), p2 + row);
            assert_eq!(root_prev_page_cursor(p2 + row), row);
        }
        let p3_last = ROOT_ENTRIES.len() - 1;
        let row_in_p3 = p3_last - p3;
        assert_eq!(
            root_next_page_cursor(p3_last),
            row_in_p3.min(ROOT_PAGE_1_LEN - 1)
        );
        // Page 1 -> Left enters page 3 at the same row number, clamped: its
        // first row is page 3's first; page 1's deepest rows land on page 3's last.
        assert_eq!(root_prev_page_cursor(0), p3);
        assert_eq!(
            root_prev_page_cursor(ROOT_PAGE_1_LEN - 1),
            p3_last,
            "page-1 last row clamps onto page 3's last row"
        );
    }

    /// Retail carries the cursor across the menuwind<->socialme flip through an
    /// explicit pair table of per-page item ids (FFXiMain.dll retail-2026-09 RVA
    /// 0x36FF98, read by the page handlers at RVA 0x140423 / 0x14E7D3). The
    /// pages' display orders pair up row-for-row, which is what makes this
    /// layout's plain same-index carry correct; pin both readings.
    #[test]
    fn root_pages_match_retail_pair_table() {
        // Byte pairs (menuwind id, socialme id) at FFXiMain.dll retail-2026-09
        // RVA 0x36FF98, before the terminating 00 00.
        const PAIRS: [(u8, u8); 12] = [
            (1, 1),
            (2, 2),
            (3, 3),
            (4, 4),
            (5, 6),
            (6, 7),
            (7, 8),
            (8, 0xA),
            (9, 9),
            (0xA, 5),
            (0xB, 0xB),
            (0xC, 0xC),
        ];
        // Each screen row's handler item id per page: menuwind fires Synthesis as
        // item id 0xA (screen row 5) and linkshell as 9; socialme fires Current
        // Time as 10 (row 9) and Communication as 9. A pair (a,b) carries the
        // flip from a's row to b's row - always the same screen line.
        const P1_ROWS_BY_ID: [u8; 12] = [1, 2, 3, 4, 0xA, 5, 6, 7, 8, 9, 0xB, 0xC];
        const P2_ROWS_BY_ID: [u8; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 0xA, 9, 0xB, 0xC];
        assert!(
            P1_ROWS_BY_ID
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len()
                == 12
        );
        for (p1_id, p2_id) in PAIRS {
            let r1 = P1_ROWS_BY_ID.iter().position(|id| *id == p1_id).unwrap();
            let r2 = P2_ROWS_BY_ID.iter().position(|id| *id == p2_id).unwrap();
            assert_eq!(
                r1, r2,
                "pair ({p1_id:#x},{p2_id:#x}) must be the same screen row"
            );
        }

        // Kuluu's pages hold retail's rows in that display order (labels are the
        // client caption catalog strings).
        const RETAIL_P1: [&str; 12] = [
            "Status",
            "Equipment",
            ROOT_MAGIC_LIST,
            "Items",
            "Synthesis",
            "Abilities",
            "Party",
            "Trade",
            "Search",
            "Linkshell",
            "Region Info",
            "Map",
        ];
        const RETAIL_P2: [&str; 12] = [
            "Missions",
            "Quests",
            "Key Items",
            "View House",
            ROOT_SET_BAZAAR,
            ROOT_MACRO_PALETTE,
            "Config",
            "Help Desk",
            "Current Time",
            "Communication",
            "Shut Down",
            "Log Out",
        ];
        assert_eq!(&ROOT_ENTRIES[..12], &RETAIL_P1);
        assert_eq!(&ROOT_ENTRIES[12..24], &RETAIL_P2);
    }

    /// Live DAT captions supersede the static root copy while they last;
    /// kuluu's page-3 rows keep falling through to it (bead kuluu-1z9qs).
    #[test]
    fn selector_caption_override_replaces_root_pages_but_not_page_3() {
        let dynamic = DynamicMenu {
            selector_captions: (0..24).map(|i| format!("row {i}")).collect(),
            ..DynamicMenu::default()
        };
        assert_eq!(entry_label(MenuKind::Root, 0, &dynamic), "row 0");
        assert_eq!(entry_label(MenuKind::Root, 23, &dynamic), "row 23");
        let first_kuluu_row = static_entries(MenuKind::Root)[24];
        assert_eq!(entry_label(MenuKind::Root, 24, &dynamic), first_kuluu_row);
    }

    /// Gated on a retail install. The runtime caption read — the exact loader
    /// the refresh system feeds — yields precisely this static copy's rows, so
    /// the pinned strings cannot drift from the catalog unnoticed (bead
    /// kuluu-1z9qs).
    #[test]
    fn real_selector_captions_match_the_static_root_copy() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut captions = CommandSelectorCaptions::default();
        captions.ensure_loaded(Some(&root));
        assert_eq!(
            captions.labels.len(),
            24,
            "both selector pages must read through, or the override stays off"
        );
        assert_eq!(
            captions.labels,
            static_entries(MenuKind::Root)[..24].to_vec()
        );
    }

    /// Gated on a retail install. Each Gameplay volume row is named by its own DAT help text, so
    /// the store ids behind slider arms 2/3 cannot swap without this failing (the arm table pairs
    /// row order with select-id order).
    #[test]
    fn conf2win_volume_slider_rows_pair_with_their_dat_help_captions() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let windows = ffxi_dat::menu_windows::windows_from_root(&root).unwrap();
        let window = windows.get("conf2win").expect("conf2win record");
        let catalog: ffxi_dat::dmsg::DmsgTable = (|| {
            let loc = root
                .resolve(ffxi_dat::menu_windows::CAPTION_CATALOG_TABLE_14_FILE_ID)
                .ok()?;
            let bytes = std::fs::read(loc.path_under(&root)).ok()?;
            ffxi_dat::dmsg::DmsgTable::parse(&bytes).ok()
        })()
        .expect("table-14 caption catalog reads");
        let rows = window.live_items();
        assert_eq!(rows.len(), CONF2WIN_ROW_ARMS.len());
        for (index, want) in [(2usize, "sound effect"), (3, "background music")] {
            assert_eq!(
                rows[index].id,
                i16::try_from(index + 1).unwrap(),
                "row {index} keeps select-id order"
            );
            let help = rows[index]
                .text_a_id
                .and_then(|id| usize::try_from(id).ok())
                .and_then(|i| catalog.text(i))
                .unwrap_or_default()
                .to_lowercase();
            assert!(
                help.contains(want),
                "item id {} reads {help:?}, expected it to mention {want:?}",
                rows[index].id
            );
        }
    }

    fn root_menu_app(cursor: usize) -> App {
        use crate::hud::network_status::NetStatusVisible;
        use crate::snapshot::SceneState;

        let mut app = App::new();
        app.init_resource::<GraphicsSettings>()
            .init_resource::<crate::hud::HudPanels>()
            .init_resource::<NetStatusVisible>()
            .init_resource::<SceneState>()
            .init_resource::<DynamicMenu>()
            .init_resource::<ConfigDatRows>();
        #[cfg(not(target_arch = "wasm32"))]
        app.init_resource::<crate::audio::AudioMuteState>();

        let mut stack = MenuStack::root();
        stack.current_mut().unwrap().cursor = cursor;
        app.insert_resource(InputMode::Menu(stack));

        app.add_systems(Startup, spawn_main_menu);
        app.add_systems(Update, update_main_menu);
        app.update();
        app
    }

    fn title_text(app: &mut App) -> String {
        let mut q = app
            .world_mut()
            .query::<(&MainMenuTitle, Option<&CompanionTitle>, &Text)>();
        q.iter(app.world())
            .filter(|(_, comp, _)| comp.is_none())
            .map(|(_, _, text)| text.0.clone())
            .next()
            .expect("title exists")
    }

    fn visible_rows(app: &mut App) -> Vec<(usize, String)> {
        let mut q = app.world_mut().query::<(&MainMenuRow, &Text, &Node)>();
        let mut rows: Vec<(usize, String)> = q
            .iter(app.world())
            .filter(|(_, _, node)| node.display != Display::None)
            .map(|(row, text, _)| (row.slot, text.0.clone()))
            .collect();
        rows.sort_by_key(|(slot, _)| *slot);
        rows
    }

    #[test]
    fn graphics_menu_widens_the_pane_and_rows_stay_single_line() {
        // Regression: Graphics rows (`label:<16>[value]`) used to wrap inside
        // the 220px command column, doubling row heights and pushing the pane
        // past the window bottom.
        let mut app = root_menu_app(0);
        let pane_width = |app: &mut App| {
            let mut q = app.world_mut().query::<(&MainMenuPane, &Node)>();
            q.iter(app.world())
                .map(|(_, node)| node.width)
                .next()
                .expect("pane exists")
        };
        assert_eq!(pane_width(&mut app), Val::Px(MENU_PANE_WIDTH));

        // Push Graphics on top: the pane retargets to the wide width.
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Graphics);
        app.insert_resource(InputMode::Menu(stack));
        app.update();
        assert_eq!(pane_width(&mut app), Val::Px(GRAPHICS_PANE_WIDTH));

        // Back on the root, it narrows again.
        app.insert_resource(InputMode::Menu(MenuStack::root()));
        app.update();
        assert_eq!(pane_width(&mut app), Val::Px(MENU_PANE_WIDTH));

        // Rows and title must never wrap: a wrapped row breaks the
        // DYNAMIC_VISIBLE_ROWS viewport math.
        let mut q = app
            .world_mut()
            .query::<(&TextLayout, &MainMenuRow, Option<&CompanionRow>)>();
        let mut count = 0;
        for (layout, _row, comp) in q.iter(app.world()) {
            assert_eq!(layout.linebreak, LineBreak::NoWrap);
            if comp.is_none() {
                count += 1;
            }
        }
        assert_eq!(count, MAX_ENTRY_COUNT, "every focused row carries NoWrap");
        let mut q = app
            .world_mut()
            .query_filtered::<&TextLayout, With<MainMenuTitle>>();
        assert!(q
            .iter(app.world())
            .all(|l| l.linebreak == LineBreak::NoWrap));
    }

    fn companion_frame_visible(app: &mut App) -> (bool, String, Vec<String>) {
        let mut visible = false;
        {
            let mut q = app.world_mut().query::<(&MainMenuCompanionFrame, &Node)>();
            for (_, node) in q.iter(app.world()) {
                if node.display != Display::None {
                    visible = true;
                }
            }
        }
        let title = app
            .world_mut()
            .query::<(&CompanionTitle, &Text)>()
            .iter(app.world())
            .map(|(_, t)| t.0.clone())
            .next()
            .unwrap_or_default();
        let mut rows: Vec<(usize, String)> = app
            .world_mut()
            .query::<(&CompanionRow, &Text, &Node)>()
            .iter(app.world())
            .filter(|(_, _, node)| node.display != Display::None)
            .map(|(row, text, _)| (row.slot, text.0.clone()))
            .collect();
        rows.sort_by_key(|(slot, _)| *slot);
        (visible, title, rows.into_iter().map(|(_, t)| t).collect())
    }

    /// Depth-1: the Command menu owns the screen alone - no mirror frame.
    #[test]
    fn root_only_stack_shows_no_companion_frame() {
        let mut app = root_menu_app(0);
        let (visible, _, _) = companion_frame_visible(&mut app);
        assert!(!visible, "no selector to mirror under a depth-1 stack");
    }

    /// The cover law (vault main_menu.md §59 addendum 10): statcom2's record
    /// frame `(384,48,w112,h218)` sits exactly over menuwind's — retail draws
    /// the list ON the Command spot and nothing else beside it. kuluu shows the
    /// thirteen rows in place, with no mirrored Commands frame next to them.
    #[test]
    fn status_list_covers_the_command_spot_without_a_mirror() {
        let mut app = root_menu_app(0);
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Status);
        app.insert_resource(InputMode::Menu(stack));
        app.update();
        let (visible, _, _) = companion_frame_visible(&mut app);
        assert!(
            !visible,
            "statcom2 covers menuwind's rect; retail shows only the list"
        );
        let rows = visible_rows(&mut app);
        assert_eq!(
            rows.len(),
            crate::hud::status_panel::STATUS_ENTRIES.len(),
            "the live statcom2 list draws in place"
        );
    }

    /// The detail law (vault main_menu.md §59 addendum 10): a statcom2 detail
    /// opens as its OWN framed window beside the persona panel (record rect,
    /// fallback `(130,48,w252)` with no install readable) while the right column
    /// keeps drawing the list under it. Detail rows answer plain — retail's
    /// display-only windows carry no cursor and never take keys.
    #[test]
    fn status_detail_opens_beside_the_list_not_inside_it() {
        use crate::hud::network_status::NetStatusVisible;
        use crate::snapshot::SceneState;

        let mut app = App::new();
        app.init_resource::<GraphicsSettings>()
            .init_resource::<crate::hud::HudPanels>()
            .init_resource::<NetStatusVisible>()
            .init_resource::<SceneState>()
            .init_resource::<DynamicMenu>()
            .init_resource::<ConfigDatRows>();
        #[cfg(not(target_arch = "wasm32"))]
        app.init_resource::<crate::audio::AudioMuteState>();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Status);
        stack.push(MenuKind::StatusDetail { entry: 0 });
        app.insert_resource(InputMode::Menu(stack));
        app.add_systems(Startup, (spawn_main_menu, spawn_status_detail_window));
        app.add_systems(
            Update,
            (
                update_status_detail_window,
                update_main_menu.after(update_status_detail_window),
            ),
        );
        {
            let mut d = app.world_mut().resource_mut::<DynamicMenu>();
            d.rows = vec![
                DynamicMenuRow {
                    tint: None,
                    label: "Cowrevenge".into(),
                    action: DynamicMenuAction::DisplayInfo,
                },
                DynamicMenuRow {
                    tint: None,
                    label: "Blue Mage - Level 75".into(),
                    action: DynamicMenuAction::DisplayInfo,
                },
            ];
        }
        app.update();
        app.update();

        let (mut visible, mut left, mut width) = (false, Val::Auto, Val::Auto);
        {
            let mut q = app.world_mut().query::<(&StatusDetailWindow, &Node)>();
            for (_, node) in q.iter(app.world()) {
                if node.display != Display::None {
                    visible = true;
                    left = node.left;
                    width = node.width;
                }
            }
        }
        assert!(visible, "the detail draws as its own framed window");
        assert_eq!(left, Val::Px(130.0), "beside the persona column");
        assert_eq!(width, Val::Px(252.0), "profile's record width (fallback)");

        let title = app
            .world_mut()
            .query::<(&StatusDetailTitle, &Text)>()
            .iter(app.world())
            .map(|(_, text)| text.0.clone())
            .next()
            .expect("detail window has a title node");
        assert_eq!(title, menu_title(MenuKind::StatusDetail { entry: 0 }));

        let mut line_q = app.world_mut().query::<(&StatusDetailLine, &Text, &Node)>();
        let mut lines: Vec<(usize, String)> = line_q
            .iter(app.world())
            .filter(|(_, _, node)| node.display != Display::None)
            .map(|(line, text, _)| (line.slot, text.0.clone()))
            .collect();
        lines.sort_by_key(|(slot, _)| *slot);
        assert_eq!(
            lines.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(),
            vec!["Cowrevenge", "Blue Mage - Level 75"],
            "live readout, no cursor decoration"
        );

        // The right column keeps drawing the statcom2 list, not the detail.
        let rows = visible_rows(&mut app);
        assert_eq!(rows.len(), crate::hud::status_panel::STATUS_ENTRIES.len());
        assert!(
            !rows.iter().any(|(_, t)| t.contains("Cowrevenge")),
            "the list column never shows detail text: {rows:?}"
        );
        let (mirror, _, _) = companion_frame_visible(&mut app);
        assert!(!mirror, "and no Commands mirror beside it either");
    }

    /// The second-window law (vault main_menu.md §59 addendum 11): every
    /// left-half Config page record sits at x=16,w366, so its rows draw in a
    /// wide window THERE while the parent list keeps the selector column.
    /// Frames seeded by hand keep this deterministic on machines without an
    /// install; `config_family_frames_census` (ffxi-dat) pins the real values.
    #[test]
    fn config_pages_open_wide_on_the_left_with_the_parent_kept_right() {
        let mut app = root_menu_app(0);
        {
            let mut rows = app.world_mut().resource_mut::<ConfigDatRows>();
            rows.loaded = true;
            rows.frames.insert("configwi".into(), (384, 48, 112));
            rows.frames.insert("conf2win".into(), (16, 48, 366));
        }
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfig);
        app.update();
        {
            let mut q = app.world_mut().query::<(&MainMenuPane, &Node)>();
            let pane = q.iter(app.world()).next().expect("pane exists").1;
            assert!(
                matches!(pane.left, Val::Auto),
                "configwi stays in the column"
            );
            assert_eq!(pane.right, Val::Px(8.0));
        }

        stack.push(MenuKind::RetailConfigPage(0)); // conf2win
        app.insert_resource(InputMode::Menu(stack));
        app.update();
        {
            let mut q = app.world_mut().query::<(&MainMenuPane, &Node)>();
            let pane = q.iter(app.world()).next().expect("pane exists").1;
            assert_eq!(pane.left, Val::Px(16.0), "wide page window at record x");
            assert_eq!(pane.top, Val::Px(48.0));
            assert_eq!(pane.width, Val::Px(366.0), "record width");
            assert!(matches!(pane.right, Val::Auto));
        }
        let (visible, title, rows) = companion_frame_visible(&mut app);
        assert!(visible, "the parent list stays visible on the right");
        assert_eq!(title, menu_title(MenuKind::RetailConfig));
        assert!(!rows.is_empty());

        // Going back moves nothing: pop closes the page only (retail keeps the
        // hub), so the column owns its anchor again.
        let mut back = MenuStack::root();
        back.push(MenuKind::RetailConfig);
        app.insert_resource(InputMode::Menu(back));
        app.update();
        {
            let mut q = app.world_mut().query::<(&MainMenuPane, &Node)>();
            let pane = q.iter(app.world()).next().expect("pane exists").1;
            assert!(
                matches!(pane.left, Val::Auto),
                "back to the selector column"
            );
            assert_eq!(pane.right, Val::Px(8.0));
        }
    }

    /// The whole-record frame census, read live from the install (bead
    /// kuluu-o9qt7.30): every menu window record's own rect is placement data
    /// (companion_windows law §4), so the loader must surface them for ALL
    /// windows, not just the Config family. Prints every left-half window (x <
    /// 256) — the companion-placement candidate set — and pins that both known
    /// halves of it resolve: a Status-flow selector (`statcom2`, interactive,
    /// sits right) next to its display companions (`persona`, `level`) and the
    /// Config hub (`configwi`).
    #[test]
    fn real_menu_window_frames_census_surfaces_every_record_rect() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut table = ConfigDatRows::default();
        table.ensure_loaded(Some(&root));
        let left_half: Vec<(&String, &(i16, i16, i16))> = table
            .frames
            .iter()
            .filter(|(_, (x, _, _))| *x < 256)
            .collect();
        // SAFETY: a thin parse would make every contains_key below vacuous; 426 windows
        // on retail-2026-09, so demand well over a hundred.
        assert!(
            table.frames.len() > 100,
            "frame census too thin: {} windows",
            table.frames.len()
        );
        for name in [
            "configwi", "statcom2", "persona", "level", "conf2win", "cfilter", "fxfilter",
            "conf12wi",
            // Bead kuluu-o9qt7.30: every record the extended companion law places from.
            "quest00", "quest01", "miss00", "trade", "link5", "friend", "myroom", "jobchang",
            "cmbhlst", "mcr1edit", "scresult",
            // Bead kuluu-o9qt7.13.5: the roomlist places from its own record (16,208,w366).
            "roomlist",
        ] {
            assert!(
                table.frames.contains_key(name),
                "{name} lost its record frame in the census"
            );
        }
        eprintln!(
            "FRAME CENSUS ({} windows, {} left-half):",
            table.frames.len(),
            left_half.len()
        );
        for (name, (x, y, w)) in &left_half {
            eprintln!("  {name} x={x} y={y} w={w}");
        }
        for name in [
            "missionm",
            "mgcmenu",
            "abimenu",
            "searchma",
            "lscswin",
            "commenu",
            "cmbmenu",
            "inventor",
            "link1",
            "ut_menu",
            "conftxtc",
            "keylayout",
            "prtyjoin",
            // Absence probes for bead kuluu-o9qt7.30: `None` means the name has no parsed
            // record frame at all, so nothing companion-places from a DAT rect there.
            "mcrmenu",
            "bazaar",
            "region",
            "pupequip",
            "mp_mncst",
            "keyitem",
            "quset",
            "scresult",
            "emote",
            "gift",
        ] {
            eprintln!("DRILL {name} = {:?}", table.frames.get(name));
        }
    }

    /// Row law for bead kuluu-o9qt7.13.5 (vault main_menu.md §59 addendum 20/21):
    /// `roomlist` row i carries style word ROOMLIST_ROW_STYLES[i]; the zero-style row
    /// ("No changes") answers as display-only — the row callback .text
    /// FFXiMain.dll retail-2026-09 RVA 0x1D6B52 tests its style word zero and only
    /// refreshes/blips (.text 0x1D6B10), never arming a send — and every
    /// other row's action carries its Param2 style for the confirm→send chain.
    /// One law decides dim rows for both gated windows: Status gates per entry kind (the
    /// `statcom2` open code disables ids 3/4/5/12/13 at .text FFXiMain.dll retail-2026-09
    /// 0x148B50 and content lifts it), and the roomlist gates only its patio row, on key item
    /// 3051 (dim word .text 0x1D6A1A; bell-only select .text 0x1D6B63).
    #[test]
    fn disabled_rows_track_their_own_gate() {
        let plain = kuluu_snapshot::SceneSnapshot::default();
        assert!(
            menu_row_disabled(MenuKind::RoomList, ROOMLIST_PATIO_ROW_POSITION, &plain),
            "the patio row is dim without the bell"
        );
        for position in 0..ROOMLIST_ROW_STYLES.len() {
            if position == ROOMLIST_PATIO_ROW_POSITION {
                continue;
            }
            assert!(
                !menu_row_disabled(MenuKind::RoomList, position, &plain),
                "roomlist row {position} is live whatever the bell says"
            );
        }

        let mut with_bell = kuluu_snapshot::SceneSnapshot::default();
        with_bell.key_items.push(PATIO_BELL_KEY_ITEM);
        assert!(
            !menu_row_disabled(MenuKind::RoomList, ROOMLIST_PATIO_ROW_POSITION, &with_bell),
            "owning the bell lifts the row"
        );

        // Status keeps its own gate through the same door: Combat Skill (row 3) is dim until
        // s2c 0x062 brings a visible word.
        assert!(menu_row_disabled(MenuKind::Status, 3, &plain));
    }

    #[test]
    fn roomlist_rows_carry_their_style_words_and_actions() {
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            ROOMLIST_DAT_WINDOW.to_string(),
            (0..5u32).map(|i| (400 + i, format!("row{i}"))).collect(),
        );
        let rows = roomlist_rows(&table);
        assert_eq!(rows.len(), 5);
        assert!(
            matches!(rows[0].action, DynamicMenuAction::DisplayInfo),
            "the zero-style row never arms a send"
        );
        for (offset, &style) in ROOMLIST_ROW_STYLES.iter().enumerate().skip(1) {
            let DynamicMenuAction::MogHouseRemodelSelect { style: got } = rows[offset].action
            else {
                panic!("roomlist row {offset} must arm a remodel select");
            };
            assert_eq!(got, style, "row {offset} carries its Param2 style word");
        }
    }

    /// Gated on a retail install (bead kuluu-o9qt7.13.5): the roomlist's five row labels
    /// come from the table-12 catalog through ConfigDatRows — never transcribed — and its
    /// record frame stays at (16,208,w366) for companion placement.
    #[test]
    fn real_roomlist_rows_load_from_the_record_and_table12_catalog() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut table = ConfigDatRows::default();
        table.ensure_loaded(Some(&root));
        assert_eq!(
            table.frames.get(ROOMLIST_DAT_WINDOW),
            Some(&(16, 208, 366)),
            "the roomlist record frame moved"
        );
        let rows = table.rows_for(ROOMLIST_DAT_WINDOW);
        let labels: Vec<&str> = rows.iter().map(|(_, label)| label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "No changes",
                "San d'Orian style",
                "Bastokan style",
                "Windurstian style",
                "Mog Patio",
            ],
            "roomlist captions must stay the table-12 catalog strings"
        );
        assert_eq!(
            table.roomlist_prompt.as_deref(),
            Some("Make changes?"),
            "the remodel confirm question must be GetText table-12 id {ROOMLIST_CONFIRM_PROMPT_ID} text (bead kuluu-o9qt7.13.5.2)"
        );
    }

    /// The prompt id is what the row callback fetches just before opening the dialog (.text
    /// FFXiMain.dll retail-2026-09 0x1D6B85 → dialog service RVA 0x210E30), and until that catalog
    /// loads nothing renders in its place — there is no typed fallback (bead kuluu-o9qt7.13.5.2).
    #[test]
    fn remodel_prompt_id_matches_the_fetch_and_absent_dat_shows_nothing() {
        assert_eq!(ROOMLIST_CONFIRM_PROMPT_ID, 451);
        assert_eq!(DynamicMenu::default().roomlist_prompt, None);
        assert_eq!(ConfigDatRows::default().roomlist_prompt, None);
    }

    /// Placement law: every Config page draws at its own menu record's rect; when a
    /// record's frame is unavailable the page falls back to kuluu's normal column,
    /// never to an invented wide-window rect (bead kuluu-o9qt7.20.1.3; vault
    /// main_menu.md §59 addendum 12).
    #[test]
    fn config_pages_place_only_from_their_own_record_frame() {
        let mut frames = ConfigDatRows::default();
        // Hub jump-table branch order (FFXiMain.dll retail-2026-09 RVA 0x196770):
        // conf2win is page 0, cfilter page 1 and fxfilter page 6.
        for page in [0u8, 1, 6] {
            let name = RETAIL_CONFIG_PAGE_WINDOWS[usize::from(page)];
            frames.frames.insert(name.into(), (16, 48, 366));
            assert_eq!(
                companion_record_frame(MenuKind::RetailConfigPage(page), &frames),
                Some((16.0, 48.0, 366.0)),
                "{name} (page {page}) places from its own record"
            );
        }
        frames.frames.remove("cfilter");
        assert_eq!(
            companion_record_frame(MenuKind::RetailConfigPage(1), &frames),
            None,
            "no placeholder survives for a page whose frame is missing"
        );
    }

    /// The extended second-window law (bead kuluu-o9qt7.30): each companion kind draws
    /// strictly from its own record — the frames are the retail-2026-09 census values
    /// (`real_menu_window_frames_census_surfaces_every_record_rect`) — and kinds with no
    /// owned left-half record keep kuluu's default seat instead of being over-mapped.
    #[test]
    fn companion_windows_place_strictly_from_their_own_record() {
        let mut frames = ConfigDatRows::default();
        for (name, frame) in [
            ("quest00", (16, 48, 366)),
            ("quest01", (16, 48, 366)),
            ("miss00", (16, 48, 366)),
            ("trade", (16, 48, 182)),
            ("link5", (16, 106, 366)),
            ("scresult", (16, 48, 366)),
            ("friend", (16, 48, 366)),
            ("myroom", (16, 208, 112)),
            ("jobchang", (16, 256, 112)),
            ("cmbhlst", (16, 48, 182)),
            ("mcr1edit", (16, 48, 480)),
            ("mcr2edit", (16, 48, 480)),
        ] {
            frames.frames.insert(name.into(), frame);
        }
        for (kind, name) in [
            (MenuKind::QuestList, "quest00"),
            (MenuKind::QuestDetail { log: 1, id: 7 }, "quest01"),
            (MenuKind::MissionDetail { port: 0x00D0 }, "miss00"),
            (MenuKind::Trade, "trade"),
            (MenuKind::Linkshell, "link5"),
            (MenuKind::Search, "scresult"),
            (MenuKind::FriendList, "friend"),
            (MenuKind::HomeView, "myroom"),
            (MenuKind::JobChange, "jobchang"),
            (MenuKind::CraftList, "cmbhlst"),
            (MenuKind::MacroEditor { block: 0, index: 1 }, "mcr1edit"),
            (MenuKind::MacroEditor { block: 1, index: 1 }, "mcr2edit"),
        ] {
            assert_eq!(
                companion_record_frame(kind, &frames),
                frames.window_frame(name),
                "{name} must place the window at its own record frame"
            );
        }
        // Over-mapping guards, per bead kuluu-o9qt7.30.1 waivers: `menu emote` belongs to the
        // abisortw pairing opened only by helper .text FFXiMain.dll retail-2026-09 RVA 0x225460
        // (vault main_menu.md §59 addendum 18 corrected; family banked as bead kuluu-o9qt7.34),
        // so the Communication-side EmoteList keeps its seat; macro launcher/select windows sit
        // on the selector side (census); bazaar/region/pupequip/mp_mncst share the 384 seat.
        // `quset` is a dead DAT alias — the bytes "quset" appear nowhere in FFXiMain.dll
        // [V-neg].
        for kind in [
            MenuKind::MacrosPalette,
            MenuKind::MacroSelect,
            MenuKind::MacroResult,
            MenuKind::BazaarStalls,
            MenuKind::EmoteList,
            MenuKind::KeyItems,
        ] {
            assert_eq!(
                companion_record_frame(kind, &frames),
                None,
                "{kind:?} keeps kuluu's default seat"
            );
        }
    }

    /// configwi sits over the main menu's page 2 the way statcom2 sits over page 1: the Config
    /// hub draws in the Command spot with nothing mirrored beside it, and a window that does not
    /// cover the main menu still leaves it framed beside its own.
    #[test]
    fn the_config_hub_covers_the_command_spot_without_a_mirror() {
        let mut app = root_menu_app(0);
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfig);
        app.insert_resource(InputMode::Menu(stack));
        app.update();
        let (visible, _, _) = companion_frame_visible(&mut app);
        assert!(!visible, "configwi covers the main menu");
        assert_eq!(title_text(&mut app), menu_title(MenuKind::RetailConfig));

        let mut stack = MenuStack::root();
        stack.push(MenuKind::Graphics);
        app.insert_resource(InputMode::Menu(stack));
        app.update();
        let (visible, title, rows) = companion_frame_visible(&mut app);
        assert!(visible, "the Command menu stays framed beside it");
        assert_eq!(title, menu_title(MenuKind::Root));
        assert!(
            !rows.is_empty(),
            "the mirrored selector still lists its page"
        );
    }

    fn stack_app(stack: MenuStack, frames: &[(&str, (i16, i16, i16, i16), u8)]) -> App {
        let mut app = root_menu_app(0);
        app.add_systems(Update, update_menu_stack_windows);
        {
            let mut rows = app.world_mut().resource_mut::<ConfigDatRows>();
            rows.loaded = true;
            for &(name, (x, y, width, height), anchor) in frames {
                rows.frames.insert(name.into(), (x, y, width));
                rows.frame_extents.insert(name.into(), (height, anchor));
            }
        }
        app.insert_resource(InputMode::Menu(stack));
        app
    }

    fn display_row(label: &str) -> DynamicMenuRow {
        DynamicMenuRow {
            tint: None,
            label: label.into(),
            action: DynamicMenuAction::DisplayInfo,
        }
    }

    struct DrawnWindow {
        title: String,
        rows: Vec<String>,
        node: Node,
        z: i32,
    }

    /// The stacked record windows on screen, oldest first.
    fn drawn_stack_windows(app: &mut App) -> Vec<DrawnWindow> {
        let windows: Vec<(usize, Node, i32)> = app
            .world_mut()
            .query::<(&MenuStackWindow, &Node, &ZIndex)>()
            .iter(app.world())
            .filter(|(_, node, _)| node.display != Display::None)
            .map(|(window, node, z)| (window.slot, node.clone(), z.0))
            .collect();
        let titles: Vec<(usize, String)> = app
            .world_mut()
            .query::<(&MenuStackWindowTitle, &Text)>()
            .iter(app.world())
            .map(|(title, text)| (title.window, text.0.clone()))
            .collect();
        let mut rows: Vec<(usize, usize, String)> = app
            .world_mut()
            .query::<(&MenuStackWindowRow, &Text, &Node)>()
            .iter(app.world())
            .filter(|(_, _, node)| node.display != Display::None)
            .map(|(row, text, _)| (row.window, row.slot, text.0.clone()))
            .collect();
        rows.sort();
        let mut drawn: Vec<DrawnWindow> = windows
            .into_iter()
            .map(|(slot, node, z)| DrawnWindow {
                title: titles
                    .iter()
                    .find(|(window, _)| *window == slot)
                    .map(|(_, text)| text.clone())
                    .unwrap_or_default(),
                rows: rows
                    .iter()
                    .filter(|(window, _, _)| *window == slot)
                    .map(|(_, _, text)| text.clone())
                    .collect(),
                node,
                z,
            })
            .collect();
        drawn.sort_by_key(|window| window.z);
        drawn
    }

    fn drawn_pane(app: &mut App) -> (Node, i32) {
        app.world_mut()
            .query::<(&MainMenuPane, &Node, &ZIndex)>()
            .iter(app.world())
            .map(|(_, node, z)| (node.clone(), z.0))
            .next()
            .expect("pane exists")
    }

    fn drawn_mirror(app: &mut App) -> Option<(Node, i32)> {
        app.world_mut()
            .query::<(&MainMenuCompanionFrame, &Node, &ZIndex)>()
            .iter(app.world())
            .find(|(_, node, _)| node.display != Display::None)
            .map(|(_, node, z)| (node.clone(), z.0))
    }

    fn frame_rect(node: &Node) -> (Val, Val, Val) {
        (node.left, node.top, node.width)
    }

    /// Two record windows stay on screen together: the quest list keeps its own frame, rows and
    /// cursor under the detail it opened, the main menu keeps the selector column, and each
    /// newer window draws over the older ones.
    #[test]
    fn two_open_record_windows_draw_at_their_own_frames_newest_on_top() {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::QuestList);
        stack.current_mut().unwrap().cursor = 1;
        stack.push(MenuKind::QuestDetail { log: 1, id: 7 });
        let mut app = stack_app(
            stack,
            &[
                ("quest00", (16, 48, 366, 200), FRAME_ANCHOR_NONE),
                ("quest01", (40, 120, 300, 150), FRAME_ANCHOR_NONE),
            ],
        );
        app.world_mut().resource_mut::<DynamicMenu>().stacked_rows = vec![(
            MenuKind::QuestList,
            vec![display_row("Quest A"), display_row("Quest B")],
        )];
        app.update();

        let (pane, pane_z) = drawn_pane(&mut app);
        assert_eq!(
            frame_rect(&pane),
            (Val::Px(40.0), Val::Px(120.0), Val::Px(300.0)),
            "the detail sits at its own frame"
        );
        let stacked = drawn_stack_windows(&mut app);
        assert_eq!(stacked.len(), 1, "the list it opened from stays drawn");
        let list = &stacked[0];
        assert_eq!(
            frame_rect(&list.node),
            (Val::Px(16.0), Val::Px(48.0), Val::Px(366.0)),
            "the list keeps its own frame, not the right seat"
        );
        assert_eq!(list.title, menu_title(MenuKind::QuestList));
        assert_eq!(list.rows, vec!["  Quest A", "> Quest B"]);
        let (mirror, mirror_z) = drawn_mirror(&mut app).expect("the main menu stays open");
        assert_eq!(mirror.right, Val::Px(MENU_RIGHT_EDGE));
        let (_, title, _) = companion_frame_visible(&mut app);
        assert_eq!(title, menu_title(MenuKind::Root));
        assert!(
            mirror_z < list.z && list.z < pane_z,
            "oldest first, newest on top: {mirror_z} {} {pane_z}",
            list.z
        );
    }

    /// The Config hub, a page and the window the page opened all stay on screen: the hub keeps
    /// the selector column with its own rows, and the page and its window each sit at their own
    /// frame, oldest first.
    #[test]
    fn three_open_config_windows_stay_drawn_oldest_first() {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfig);
        stack.push(MenuKind::RetailConfigPage(3));
        stack.push(MenuKind::RetailConfigSub(0));
        let mut app = stack_app(
            stack,
            &[
                (
                    RETAIL_CONFIG_HUB_WINDOW,
                    (384, 48, 112, 218),
                    FRAME_ANCHOR_RIGHT_EDGE,
                ),
                (
                    RETAIL_CONFIG_PAGE_WINDOWS[3],
                    (16, 48, 366, 296),
                    FRAME_ANCHOR_NONE,
                ),
                (
                    RETAIL_CONFIG_SUB_WINDOWS[0],
                    (40, 200, 300, 120),
                    FRAME_ANCHOR_NONE,
                ),
            ],
        );
        {
            let mut dynamic = app.world_mut().resource_mut::<DynamicMenu>();
            dynamic.stacked_rows = vec![
                (
                    MenuKind::RetailConfig,
                    vec![display_row("Hub A"), display_row("Hub B")],
                ),
                (MenuKind::RetailConfigPage(3), vec![display_row("Page A")]),
            ];
            dynamic.rows = vec![display_row("Sub A")];
        }
        app.update();

        let (pane, pane_z) = drawn_pane(&mut app);
        assert_eq!(
            frame_rect(&pane),
            (Val::Px(40.0), Val::Px(200.0), Val::Px(300.0))
        );
        let stacked = drawn_stack_windows(&mut app);
        assert_eq!(stacked.len(), 1, "the page stays drawn under its window");
        assert_eq!(
            frame_rect(&stacked[0].node),
            (Val::Px(16.0), Val::Px(48.0), Val::Px(366.0))
        );
        assert_eq!(stacked[0].title, menu_title(MenuKind::RetailConfigPage(3)));
        assert_eq!(stacked[0].rows, vec!["> Page A"]);
        let (mirror, mirror_z) = drawn_mirror(&mut app).expect("the hub stays visible");
        assert_eq!(
            mirror.right,
            Val::Px(MENU_RIGHT_EDGE),
            "the hub keeps the selector column"
        );
        let (_, title, rows) = companion_frame_visible(&mut app);
        assert_eq!(title, menu_title(MenuKind::RetailConfig));
        assert_eq!(
            rows,
            vec!["> Hub A", "  Hub B"],
            "the hub lists its own rows, not the focused window's"
        );
        assert!(mirror_z < stacked[0].z && stacked[0].z < pane_z);
    }

    /// A newer window on an older window's whole record rectangle hides it; with no known
    /// height, nothing is hidden.
    #[test]
    fn a_window_on_an_older_windows_whole_rect_hides_it() {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::QuestList);
        stack.push(MenuKind::QuestDetail { log: 1, id: 7 });
        let mut app = stack_app(
            stack.clone(),
            &[
                ("quest00", (16, 48, 366, 200), FRAME_ANCHOR_NONE),
                ("quest01", (16, 48, 366, 210), FRAME_ANCHOR_NONE),
            ],
        );
        app.update();
        assert!(
            drawn_stack_windows(&mut app).is_empty(),
            "quest01 covers quest00 whole"
        );

        let mut unknown = ConfigDatRows::default();
        unknown.frames.insert("quest00".into(), (16, 48, 366));
        unknown.frames.insert("quest01".into(), (16, 48, 366));
        assert_eq!(
            menu_stack_layout(&stack, &unknown)
                .record_windows()
                .iter()
                .map(|(index, _)| *index)
                .collect::<Vec<_>>(),
            vec![1],
            "an unknown height covers nothing"
        );
    }

    /// A record window whose child keeps kuluu's column stays at its own frame instead of being
    /// mirrored beside the column.
    #[test]
    fn record_parent_of_a_column_window_stays_at_its_own_frame() {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Trade);
        stack.push(MenuKind::TradePick { trade_index: 0 });
        let mut app = stack_app(stack, &[("trade", (16, 48, 182, 200), FRAME_ANCHOR_NONE)]);
        app.update();

        let (pane, _) = drawn_pane(&mut app);
        assert_eq!(
            pane.right,
            Val::Px(MENU_RIGHT_EDGE),
            "the pick list keeps the column"
        );
        let stacked = drawn_stack_windows(&mut app);
        assert_eq!(stacked.len(), 1);
        assert_eq!(
            frame_rect(&stacked[0].node),
            (Val::Px(16.0), Val::Px(48.0), Val::Px(182.0))
        );
        assert_eq!(stacked[0].title, menu_title(MenuKind::Trade));
        assert!(
            drawn_mirror(&mut app).is_none(),
            "nothing is mirrored beside it"
        );
    }

    /// Windows with no side record keep the seats kuluu gave them: the focused one in the
    /// column with its opener beside it, and Status and the Config hub alone because statcom2 and
    /// configwi cover the main menu.
    #[test]
    fn windows_without_a_side_record_keep_their_column_seats() {
        let frames = ConfigDatRows::default();
        let mut graphics = MenuStack::root();
        graphics.push(MenuKind::Graphics);
        assert_eq!(
            menu_stack_layout(&graphics, &frames).windows,
            vec![
                (0, MenuWindowSeat::BesideColumn),
                (1, MenuWindowSeat::Column)
            ]
        );
        let mut magic = MenuStack::root();
        magic.push(MenuKind::MagicGroups);
        magic.push(MenuKind::MagicGroup(0));
        assert_eq!(
            menu_stack_layout(&magic, &frames).windows,
            vec![
                (1, MenuWindowSeat::BesideColumn),
                (2, MenuWindowSeat::Column)
            ],
            "only the opener sits beside the column"
        );
        let mut status = MenuStack::root();
        status.push(MenuKind::Status);
        status.push(MenuKind::StatusDetail { entry: 0 });
        assert_eq!(
            menu_stack_layout(&status, &frames).windows,
            vec![(1, MenuWindowSeat::Column)]
        );
        let mut config = MenuStack::root();
        config.push(MenuKind::RetailConfig);
        assert_eq!(
            menu_stack_layout(&config, &frames).windows,
            vec![(1, MenuWindowSeat::Column)]
        );
    }

    /// The anchor byte pins a frame to the screen edge it names: x and y gain
    /// `screen - 512` / `screen - 448`, spelled as 100% plus a margin of `x - 512` / `y - 448`.
    #[test]
    fn record_frames_pin_to_the_screen_edges_their_anchor_names() {
        let placed = |anchor: u8| {
            let mut node = Node::default();
            place_at_record_frame(
                &mut node,
                &RecordFrame {
                    x: 384.0,
                    y: 400.0,
                    width: 112.0,
                    height: Some(30.0),
                    anchor,
                },
            );
            node
        };
        let none = placed(FRAME_ANCHOR_NONE);
        assert_eq!(
            (none.left, none.margin.left, none.top, none.margin.top),
            (Val::Px(384.0), Val::ZERO, Val::Px(400.0), Val::ZERO)
        );
        let right = placed(FRAME_ANCHOR_RIGHT_EDGE);
        assert_eq!(
            (right.left, right.margin.left, right.top, right.margin.top),
            (
                Val::Percent(100.0),
                Val::Px(384.0 - MENU_DESIGN_WIDTH),
                Val::Px(400.0),
                Val::ZERO
            )
        );
        let bottom = placed(FRAME_ANCHOR_BOTTOM_EDGE);
        assert_eq!(
            (bottom.left, bottom.top, bottom.margin.top),
            (
                Val::Px(384.0),
                Val::Percent(100.0),
                Val::Px(400.0 - MENU_DESIGN_HEIGHT)
            )
        );
        let both = placed(FRAME_ANCHOR_RIGHT_EDGE | FRAME_ANCHOR_BOTTOM_EDGE);
        assert_eq!(
            (both.left, both.top),
            (Val::Percent(100.0), Val::Percent(100.0))
        );
        for node in [none, right, bottom, both] {
            assert_eq!(node.position_type, PositionType::Absolute);
            assert_eq!((node.right, node.bottom), (Val::Auto, Val::Auto));
            assert_eq!(node.width, Val::Px(112.0));
        }
    }

    /// Every dynamic window under the top one keeps its own rows; the top's stay in `rows`.
    #[test]
    fn rows_refresh_for_every_open_dynamic_window() {
        let mut app = App::new();
        app.init_resource::<crate::snapshot::SceneState>()
            .init_resource::<crate::hud::item_detail::SortOptions>()
            .init_resource::<crate::hud::item_screen::ItemScreenContainer>()
            .init_resource::<MagicSchoolTable>()
            .init_resource::<ConfigDatRows>()
            .init_resource::<CommandSelectorCaptions>()
            .init_resource::<EquipRecords>()
            .init_resource::<crate::keybinds::Bindings>()
            .init_resource::<crate::hud::unity_fp::UnityFpDat>()
            .init_resource::<crate::hud::merit_windows::MeritDat>()
            .init_resource::<DynamicMenu>();
        {
            let mut rows = app.world_mut().resource_mut::<ConfigDatRows>();
            rows.loaded = true;
            rows.by_root
                .insert(RETAIL_CONFIG_HUB_WINDOW.into(), vec![(1, "Hub row".into())]);
            rows.by_root.insert(
                RETAIL_CONFIG_PAGE_WINDOWS[0].into(),
                vec![(2, "Page row".into())],
            );
        }
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfig);
        stack.push(MenuKind::RetailConfigPage(0));
        app.insert_resource(InputMode::Menu(stack));
        app.add_systems(Update, refresh_dynamic_menu_rows);
        app.update();

        let dynamic = app.world().resource::<DynamicMenu>();
        let labels = |rows: &[DynamicMenuRow]| -> Vec<String> {
            rows.iter().map(|row| row.label.clone()).collect()
        };
        assert!(labels(&dynamic.rows).iter().any(|l| l.contains("Page row")));
        let hub = labels(stacked_rows_for(MenuKind::RetailConfig, dynamic));
        assert!(hub.iter().any(|l| l.contains("Hub row")), "{hub:?}");
        assert!(!hub.iter().any(|l| l.contains("Page row")), "{hub:?}");
        assert!(
            stacked_rows_for(MenuKind::RetailConfigPage(0), dynamic).is_empty(),
            "the top window's rows live in `rows`"
        );
    }

    /// User bug kuluu-tfmgd: Region Info rides the companion law - opening it
    /// keeps the Command menu framed beside it, and its pane answers with the
    /// live four-mode rows (mode marker on World; handler FFXiMain.dll
    /// retail-2026-09 RVA 0x140780 installs modes in place - vault
    /// main_menu.md §29), never an empty window.
    #[test]
    fn region_info_opens_beside_the_selector_frame() {
        let mut app = root_menu_app(0);
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RegionInfo { mode: 0 });
        app.insert_resource(InputMode::Menu(stack));
        {
            let mut dynamic = app.world_mut().resource_mut::<DynamicMenu>();
            dynamic.rows = region_info_rows(0, Some(0x00E6));
        }
        app.update();

        let (visible, title, _) = companion_frame_visible(&mut app);
        assert!(visible, "the Command menu stays framed beside Region Info");
        assert_eq!(title, "Commands");

        let mut q = app
            .world_mut()
            .query::<(&MainMenuRow, Option<&CompanionRow>, &Text, &Node)>();
        let pane_rows: Vec<String> = q
            .iter(app.world())
            .filter(|(_, comp, _, node)| comp.is_none() && node.display != Display::None)
            .map(|(_, _, text, _)| text.0.clone())
            .collect();
        assert_eq!(
            pane_rows.len(),
            4,
            "the four mode rows render: {pane_rows:?}"
        );
        assert!(
            pane_rows
                .iter()
                .any(|t| t.contains('\u{bb}') && t.contains("World")),
            "the installed mode wears the marker: {pane_rows:?}"
        );
    }

    #[test]
    fn single_pane_shows_only_the_cursors_root_page() {
        // Cursor on page 1 (Status at index 0): only page-1 entries render, and
        // the title carries the ‹1/3› marker.
        let mut app = root_menu_app(0);
        assert!(title_text(&mut app).contains("Commands"));
        assert!(title_text(&mut app).contains("1/3"));
        let rows = visible_rows(&mut app);
        assert_eq!(rows.len(), ROOT_PAGE_1_LEN, "only page 1 rows: {rows:?}");
        assert_eq!(rows[0].1, "> Status");
        assert!(rows.iter().any(|(_, t)| t.contains("Map")));
        assert!(
            !rows.iter().any(|(_, t)| t.contains("Config")),
            "page 2 entries stay hidden on page 1"
        );

        // Cursor on page 2 (Config): only page-2 entries render, marker flips.
        let config = ROOT_ENTRIES.iter().position(|e| *e == "Config").unwrap();
        let mut app = root_menu_app(config);
        assert!(title_text(&mut app).contains("2/3"));
        let rows = visible_rows(&mut app);
        assert_eq!(rows.len(), ROOT_PAGE_2_LEN);
        assert!(rows.iter().any(|(_, t)| t.contains("Config")));
        assert!(!rows.iter().any(|(_, t)| t.contains("Status")));

        // Cursor on page 3 (Graphics): kuluu's own rows only, marker ‹3/3›.
        let graphics = ROOT_ENTRIES.iter().position(|e| *e == "Graphics").unwrap();
        let mut app = root_menu_app(graphics);
        assert!(title_text(&mut app).contains("3/3"));
        let rows = visible_rows(&mut app);
        assert_eq!(
            rows.len(),
            ROOT_ENTRIES.len() - ROOT_PAGE_1_LEN - ROOT_PAGE_2_LEN
        );
        assert!(rows.iter().any(|(_, t)| t.contains("Graphics")));
        assert!(!rows.iter().any(|(_, t)| t.contains("Missions")));
    }

    #[test]
    fn list_page_rows_matches_owning_panel_viewport() {
        assert_eq!(
            list_page_rows(MenuKind::Items),
            crate::hud::item_screen::ITEM_LIST_ROWS
        );
        assert_eq!(
            list_page_rows(MenuKind::UsableItems),
            crate::hud::item_screen::ITEM_LIST_ROWS
        );
        assert_eq!(list_page_rows(MenuKind::Magic), DYNAMIC_VISIBLE_ROWS);
        assert_eq!(list_page_rows(MenuKind::KeyItems), DYNAMIC_VISIBLE_ROWS);
    }

    #[test]
    fn page_cursor_jumps_a_page_and_clamps_without_wrap() {
        // Forward jumps by `rows`, clamped to the last index (no wrap past end).
        assert_eq!(page_cursor(0, 30, 13, true), 13);
        assert_eq!(page_cursor(13, 30, 13, true), 26);
        assert_eq!(page_cursor(26, 30, 13, true), 29);
        assert_eq!(page_cursor(29, 30, 13, true), 29);
        // Backward jumps by `rows`, clamped to 0 (no wrap past start).
        assert_eq!(page_cursor(29, 30, 13, false), 16);
        assert_eq!(page_cursor(5, 30, 13, false), 0);
        assert_eq!(page_cursor(0, 30, 13, false), 0);
        // A page press moves more than one row — the whole point of the bug fix.
        assert!(page_cursor(0, 30, 13, true) > 1);
        // Empty list stays put.
        assert_eq!(page_cursor(0, 0, 13, true), 0);
    }

    #[test]
    fn item_qty_label_suffixes_only_real_stacks() {
        assert_eq!(item_qty_label("Bird Egg", 1), "Bird Egg");
        assert_eq!(item_qty_label("Bird Egg", 0), "Bird Egg");
        assert_eq!(item_qty_label("Bird Egg", 12), "Bird Egg x12");
    }

    #[test]
    fn action_item_no_covers_item_bearing_variants() {
        let use_it = DynamicMenuAction::UseItem {
            container: 0,
            index: 3,
            item_no: 4096,
        };
        let open = DynamicMenuAction::OpenItemAction {
            container: 0,
            index: 3,
            item_no: 4096,
        };
        assert_eq!(use_it.item_no(), Some(4096));
        assert_eq!(open.item_no(), Some(4096));
        assert_eq!(DynamicMenuAction::KeyItem { id: 5 }.item_no(), None);
        assert_eq!(DynamicMenuAction::CastSpell { spell_id: 1 }.item_no(), None);
    }

    fn mh_snapshot() -> kuluu_snapshot::SceneSnapshot {
        use ffxi_proto::map::container as c;
        let bag =
            |id: u8, capacity: u16, items: Vec<(u8, u16, u32)>| kuluu_snapshot::ContainerView {
                id,
                capacity,
                items: items
                    .into_iter()
                    .map(|(index, item_no, quantity)| kuluu_snapshot::InventoryItem {
                        container: id,
                        index,
                        item_no,
                        quantity,
                        locked: false,
                        unselectable: false,
                        charges_remaining: None,
                        next_use_vana_ts: None,
                        use_delay_end_vana_ts: None,
                        ready: None,
                        extdata: None,
                    })
                    .collect(),
            };
        kuluu_snapshot::SceneSnapshot {
            myroom: Some(kuluu_snapshot::MyRoom {
                model: 289,
                sub_map: 0,
                door_open: false,
            }),
            containers: vec![
                // Slot 0 carries Gil (retail keeps it in inventory slot 0).
                bag(
                    c::LOC_INVENTORY,
                    30,
                    vec![(0, ffxi_proto::map::GIL_ITEM_NO, 1000), (1, 4509, 12)],
                ),
                bag(c::LOC_MOGSAFE, 50, vec![(1, 13327, 1)]),
                bag(c::LOC_STORAGE, 8, vec![]),
                bag(c::LOC_TEMPITEMS, 20, vec![(0, 4212, 1)]),
                bag(c::LOC_WARDROBE, 8, vec![]),
            ],
            ..Default::default()
        }
    }

    /// Pins LSB 0x029 isValidMovement's and 0x028's CItem::isBusy rejections:
    /// locked slots (equipped / placed furniture / bazaar-priced, the states
    /// 0x020 lockFlagFor puts on the wire) get no move rows and no Drop. Use
    /// stays listed like retail; the confirm path refuses it.
    #[test]
    fn locked_slot_offers_no_moves_or_drop() {
        use ffxi_proto::map::container as c;
        let mut snap = mh_snapshot();
        snap.containers[0].items[1].locked = true;
        let rows = item_action_rows(&snap, c::LOC_INVENTORY, 1, 4509);
        assert!(
            !rows
                .iter()
                .any(|r| matches!(r.action, DynamicMenuAction::MoveItem { .. })),
            "{rows:?}"
        );
        assert!(
            !rows
                .iter()
                .any(|r| matches!(r.action, DynamicMenuAction::DropItem { .. })),
            "{rows:?}"
        );
        assert_eq!(labels(&rows), [ITEM_ACTION_USE]);
    }

    /// Pins the LSB validContainers Safe-2F gate (mhflag & 0x20,
    /// 0x029_item_move.cpp): capacity alone must not offer Mog Safe 2.
    #[test]
    fn safe2_requires_the_2f_flag() {
        use crate::hud::item_screen::container_accessible;
        use ffxi_proto::map::container as c;
        let mut snap = mh_snapshot();
        snap.containers.push(kuluu_snapshot::ContainerView {
            id: c::LOC_MOGSAFE2,
            capacity: 60,
            items: Vec::new(),
        });
        assert!(
            !container_accessible(&snap, c::LOC_MOGSAFE2),
            "flag unknown"
        );
        snap.mh_2f_unlocked = Some(false);
        assert!(!container_accessible(&snap, c::LOC_MOGSAFE2));
        snap.mh_2f_unlocked = Some(true);
        assert!(container_accessible(&snap, c::LOC_MOGSAFE2));
        assert!(
            container_accessible(&snap, c::LOC_MOGSAFE),
            "the 2F flag must not gate the 1F safe"
        );
    }

    #[test]
    fn storage_item_leads_with_take_out() {
        use ffxi_proto::map::container as c;
        // 13327 = an equipable ring in the retail id space; equip_info lookup
        // decides wardrobe eligibility, not this test.
        let rows = item_action_rows(&mh_snapshot(), c::LOC_MOGSAFE, 1, 13327);
        assert_eq!(
            &labels(&rows)[..3],
            [ITEM_ACTION_USE, ITEM_ACTION_DROP, "Take Out"],
            "retail order: Use, Drop, then the moves"
        );
        match rows[2].action {
            DynamicMenuAction::MoveItem {
                from_container,
                to_container,
                from_slot,
                quantity,
                ..
            } => {
                assert_eq!(from_container, c::LOC_MOGSAFE);
                assert_eq!(to_container, c::LOC_INVENTORY);
                assert_eq!(from_slot, 1);
                assert_eq!(quantity, 1);
            }
            _ => panic!("Take Out must be a MoveItem"),
        }
    }

    #[test]
    fn inventory_item_offers_use_and_put_in_bags() {
        use ffxi_proto::map::container as c;
        let rows = item_action_rows(&mh_snapshot(), c::LOC_INVENTORY, 1, 4509);
        assert_eq!(rows[0].label, ITEM_ACTION_USE);
        assert!(
            matches!(
                rows[1].action,
                DynamicMenuAction::DropItem {
                    container: c::LOC_INVENTORY,
                    index: 1,
                    item_no: 4509,
                    quantity: 12,
                }
            ),
            "Drop carries the whole stack: {rows:?}"
        );
        assert!(rows.iter().any(|r| r.label == "Put in Mog Safe"));
        assert!(rows.iter().any(|r| r.label == "Put in Storage"));
        assert!(
            !rows.iter().any(|r| r.label.contains("Temporary")),
            "temp items bag is never a move destination: {rows:?}"
        );
        // 4509 (Distilled Water) is not equipment, so no wardrobe row.
        assert!(!rows.iter().any(|r| r.label.contains("Wardrobe")));
    }

    /// Gil and Temporary items never move (LSB 0x029 isValidMovement /
    /// validContainers); Gil cannot be dropped either, Temporary items can
    /// (0x028_item_dump.cpp validContainers).
    #[test]
    fn gil_and_temp_items_cannot_move() {
        use ffxi_proto::map::container as c;
        let snap = mh_snapshot();
        let gil = item_action_rows(&snap, c::LOC_INVENTORY, 0, ffxi_proto::map::GIL_ITEM_NO);
        assert_eq!(labels(&gil), [ITEM_ACTION_USE], "{gil:?}");
        let temp = item_action_rows(&snap, c::LOC_TEMPITEMS, 0, 4212);
        assert_eq!(
            labels(&temp),
            [ITEM_ACTION_USE, ITEM_ACTION_DROP],
            "{temp:?}"
        );
    }

    #[test]
    fn stale_slot_yields_no_rows() {
        use ffxi_proto::map::container as c;
        let snap = mh_snapshot();
        assert!(item_action_rows(&snap, c::LOC_MOGSAFE, 5, 13327).is_empty());
        assert!(
            item_action_rows(&snap, c::LOC_MOGSAFE, 1, 999).is_empty(),
            "item id mismatch means the slot changed under the menu"
        );
    }

    fn war_party_member(id: u32) -> kuluu_snapshot::PartyMember {
        kuluu_snapshot::PartyMember {
            id,
            act_index: 0x100,
            name: Some("Kupo".into()),
            hp: 30,
            mp: 0,
            tp: 0,
            hp_pct: 100,
            mp_pct: 0,
            zone_no: 230,
            main_job: 1,
            main_job_lv: 1,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: true,
            is_alliance_leader: false,
            in_mog_house: false,
            party_no: 0,
        }
    }

    /// Pins the emote-list gating: HELM-only ids never appear (server-initiated,
    /// emote.h), and the Job row needs both a known main job and its 0x11A bit.
    #[test]
    fn emote_rows_skip_helm_and_gate_job_on_0x11a_bits() {
        use ffxi_proto::map::emote;
        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        let rows = emote_rows(&snap);
        assert!(rows.iter().any(|r| r.label == "Wave (/wave)"));
        assert!(rows.iter().any(|r| r.label == "Aim (/aim)"));
        assert!(
            !rows.iter().any(|r| r.label.contains("Logging")),
            "HELM-only emotes are server-initiated"
        );
        let is_job = |r: &DynamicMenuRow| matches!(r.action, DynamicMenuAction::Emote { emote_id } if emote_id == emote::JOB);
        assert!(
            !rows.iter().any(is_job),
            "Job row hidden until 0x11A unlocks it"
        );

        snap.self_char_id = Some(1);
        snap.party = vec![war_party_member(1)];
        snap.emote_jobs = Some(0);
        assert!(!emote_rows(&snap).iter().any(is_job), "WAR bit not set");
        snap.emote_jobs = Some(1 << 0);
        assert!(
            emote_rows(&snap).iter().any(is_job),
            "WAR main + WAR gesture bit shows the Job row"
        );

        snap.party[0].main_job = 33;
        snap.emote_jobs = Some(u32::MAX);
        assert!(
            !emote_rows(&snap).iter().any(is_job),
            "wire-supplied main job past the u32 bit width reads locked (no shift panic)"
        );
    }

    #[test]
    fn emote_command_words_lowercase_names_except_job() {
        use ffxi_proto::map::emote;
        assert_eq!(emote_command_word(8), "wave");
        assert_eq!(emote_command_word(65), "dance1");
        assert_eq!(emote_command_word(emote::JOB), "jobemote");
    }

    #[test]
    fn status_labels_match_entries() {
        use crate::hud::status_panel::STATUS_ENTRIES;
        assert_eq!(STATUS_LABELS.len(), STATUS_ENTRIES.len());
        for (i, entry) in STATUS_ENTRIES.iter().enumerate() {
            assert_eq!(STATUS_LABELS[i], entry.label, "Status row {i} label drift");
        }
    }

    #[test]
    fn static_menus_fit_pool_and_window_when_taller_than_viewport() {
        for kind in [
            MenuKind::Root,
            MenuKind::Config,
            MenuKind::Debug,
            MenuKind::Graphics,
            MenuKind::Equipment,
            MenuKind::Status,
            MenuKind::Communication,
        ] {
            let total = static_entries(kind).len();
            assert!(
                total <= MAX_ENTRY_COUNT,
                "{kind:?} has {total} rows, exceeds row pool {MAX_ENTRY_COUNT}"
            );
            // Root pages internally; every other static menu that outgrows the
            // viewport must scroll a DYNAMIC_VISIBLE_ROWS window rather than
            // render every row off the bottom of the screen (kuluu-b9kr).
            let expected = if kind == MenuKind::Root {
                total
            } else {
                total.min(DYNAMIC_VISIBLE_ROWS)
            };
            assert_eq!(
                visible_window(kind, total),
                expected,
                "{kind:?} visible window must cap at the viewport height"
            );
        }
    }

    #[test]
    fn graphics_menu_scrolls_bottom_rows_into_view() {
        let total = graphics_rows(true).len();
        assert!(
            total > DYNAMIC_VISIBLE_ROWS,
            "test presumes Graphics outgrows the viewport"
        );
        let dynamic = DynamicMenu::default();
        let last = total - 1;
        let (t, start) = resolve_viewport(MenuKind::Graphics, last, &dynamic, true);
        let window = visible_window(MenuKind::Graphics, t);
        assert!(
            last >= start && last < start + window,
            "bottom row {last} not in window [{start}, {})",
            start + window
        );
    }

    #[test]
    fn current_time_appears_exactly_once_in_root() {
        assert_eq!(
            ROOT_ENTRIES
                .iter()
                .filter(|l| **l == ROOT_CURRENT_TIME)
                .count(),
            1
        );
    }

    #[test]
    fn debug_rows_map_to_distinct_panel_state() {
        let panels = crate::hud::HudPanels {
            perf: true,
            target_cycle: false,
            mesh_debug: true,
            noclip: true,
            ..Default::default()
        };
        assert!(debug_panel_state(DEBUG_PERF, &panels, false, false, false));
        assert!(!debug_panel_state(
            DEBUG_TARGET_CYCLE,
            &panels,
            false,
            false,
            false
        ));
        assert!(debug_panel_state(DEBUG_MESH, &panels, false, false, false));
        assert!(debug_panel_state(
            DEBUG_NOCLIP,
            &panels,
            false,
            false,
            false
        ));
        assert!(debug_panel_state(
            DEBUG_NET_STATUS,
            &panels,
            true,
            false,
            false
        ));
        assert!(!debug_panel_state(
            DEBUG_NET_STATUS,
            &panels,
            false,
            false,
            false
        ));
        assert!(debug_panel_state(
            DEBUG_FORCE_18,
            &panels,
            false,
            false,
            true
        ));
        assert!(!debug_panel_state(
            DEBUG_FORCE_18,
            &panels,
            false,
            false,
            false
        ));

        for label in DEBUG_ENTRIES {
            assert_eq!(
                static_entries(MenuKind::Debug)
                    .iter()
                    .filter(|e| *e == label)
                    .count(),
                1,
                "Debug row {label:?} must appear exactly once"
            );
        }
    }

    /// Two rows whose label order (Apple, Zeta) disagrees with their item-id
    /// order (50, 10), so each ordering branch is distinguishable.
    fn conflicting_item_rows() -> Vec<DynamicMenuRow> {
        let item_row = |item_no: u16| DynamicMenuAction::OpenItemAction {
            container: 0,
            index: 0,
            item_no,
        };
        vec![
            DynamicMenuRow {
                tint: None,
                label: "Apple".to_string(),
                action: item_row(50),
            },
            DynamicMenuRow {
                tint: None,
                label: "Zeta".to_string(),
                action: item_row(10),
            },
        ]
    }

    fn labels(rows: &[DynamicMenuRow]) -> Vec<&str> {
        rows.iter().map(|r| r.label.as_str()).collect()
    }

    #[test]
    fn key_item_rows_sorted_by_id_with_unseen_suffix() {
        // Key items 1/8 = Zeruhn Report / Airship Pass
        // (vendor/server/scripts/enum/key_item.lua).
        let snap = kuluu_snapshot::SceneSnapshot {
            key_items: vec![8, 1],
            key_items_seen: vec![8],
            ..Default::default()
        };
        let rows = key_item_rows(&snap);
        assert_eq!(
            labels(&rows),
            [
                format!("Zeruhn Report{KEY_ITEM_UNSEEN_SUFFIX}").as_str(),
                "Airship Pass"
            ]
        );
        assert_eq!(rows[0].action, DynamicMenuAction::KeyItem { id: 1 });

        let mut rows = rows;
        order_dynamic_rows(MenuKind::KeyItems, &ALWAYS_CATEGORY_SORT, &mut rows);
        assert_eq!(
            rows[0].action,
            DynamicMenuAction::KeyItem { id: 1 },
            "key items keep id order, never alphabetical"
        );
    }

    #[test]
    fn item_rows_sort_by_id_when_auto() {
        let mut rows = conflicting_item_rows();
        order_dynamic_rows(MenuKind::Items, &ALWAYS_CATEGORY_SORT, &mut rows);
        assert_eq!(labels(&rows), ["Zeta", "Apple"]);
    }

    #[test]
    fn item_rows_sort_by_label_when_the_gameplay_page_picked_name() {
        use crate::hud::item_detail::{SortKey, SortOptions};
        let mut rows = conflicting_item_rows();
        order_dynamic_rows(
            MenuKind::Items,
            &SortOptions {
                auto: true,
                key: SortKey::Name,
            },
            &mut rows,
        );
        assert_eq!(labels(&rows), ["Apple", "Zeta"]);
    }

    #[test]
    fn item_rows_keep_slot_order_when_manual() {
        use crate::hud::item_detail::SortOptions;
        let mut rows = conflicting_item_rows();
        order_dynamic_rows(
            MenuKind::Items,
            &SortOptions {
                auto: false,
                ..SortOptions::default()
            },
            &mut rows,
        );
        assert_eq!(labels(&rows), ["Apple", "Zeta"]);
    }

    #[test]
    fn item_action_rows_keep_built_order() {
        let mut rows = conflicting_item_rows();
        order_dynamic_rows(
            MenuKind::ItemAction {
                container: 0,
                index: 0,
                item_no: 50,
            },
            &ALWAYS_CATEGORY_SORT,
            &mut rows,
        );
        assert_eq!(labels(&rows), ["Apple", "Zeta"]);
    }

    #[test]
    fn other_dynamic_menus_sort_alphabetically() {
        let mut rows = conflicting_item_rows();
        rows.swap(0, 1); // start Zeta-first so the sort has work to do
        order_dynamic_rows(MenuKind::Magic, &ALWAYS_CATEGORY_SORT, &mut rows);
        assert_eq!(labels(&rows), ["Apple", "Zeta"]);
    }

    /// `conf6win` id 10 raises a Yes/No prompt instead of writing at select (arm
    /// .text FFXiMain.dll retail-2026-09 RVA 0x19B309; vault main_menu.md §59
    /// addenda 7–8): the prompt shares the Drop confirm's law, and its Yes branch
    /// writes both panels' factory templates (slot +0x48 methods .text
    /// 0x19d1c0/0x19ce70) over option ids 0x4A..=0x69 and 0x6A..=0x89.
    #[test]
    fn conf6win_reset_prompt_takes_the_yes_no_law_and_pins_both_templates() {
        assert_eq!(
            static_entries(MenuKind::KeyAssignResetPrompt),
            DROP_CONFIRM_ENTRIES
        );
        assert_eq!(menu_title(MenuKind::KeyAssignResetPrompt), "Config");
        let pairs = key_assign_reset_defaults().collect::<Vec<_>>();
        assert_eq!(pairs.len(), 64, "32 options per panel (.text loop bounds)");
        // k1 shadow +0x29 = 0x11 written by .text 0x19d1ca through the loop at
        // .text 0x19d285; k2 shadow +0x29 = 0x1e from .text 0x19ce7a.
        assert_eq!(pairs[0], (KEY_ASSIGN_K1_FIRST_OPTION, 0x11));
        assert_eq!(pairs[31], (0x69, 0)); // k1 range closes at option 0x69
        assert_eq!(pairs[32], (KEY_ASSIGN_K2_FIRST_OPTION, 0x1e));
        // pairs[58] = k2 template slot 26 → shadow byte +0x43 = 0x16 (.text
        // 0x19cede), option id 0x6A+26 = 0x84.
        assert_eq!(pairs[58], (0x84, 0x16));
        assert_eq!(pairs[63], (0x89, 0)); // k2 range closes at option 0x89
    }

    /// Retail's Log Out / Shut Down rows answer through a yes/no prompt titled
    /// after the row, sharing the Drop confirm's Yes/No law (cursor default No).
    #[test]
    fn power_prompts_share_the_yes_no_law_and_take_the_row_name() {
        for shut_down in [false, true] {
            let kind = MenuKind::PowerPrompt { shut_down };
            assert_eq!(static_entries(kind), DROP_CONFIRM_ENTRIES);
        }
        assert_eq!(
            menu_title(MenuKind::PowerPrompt { shut_down: false }),
            ROOT_LOG_OUT
        );
        assert_eq!(
            menu_title(MenuKind::PowerPrompt { shut_down: true }),
            ROOT_SHUT_DOWN
        );
    }

    #[test]
    fn config_page_groups_fields_and_keeps_controls_reachable() {
        use crate::graphics_settings::MenuAction;
        let rows = config_rows();
        assert_eq!(
            rows.last(),
            Some(&MenuRow::Action(MenuAction::Controls)),
            "Controls must stay the last row of the settings page"
        );
        assert!(matches!(rows.first(), Some(MenuRow::Header(_))));
        // The kuluu settings page hangs off the page-3 "Interface" row; the
        // page-2 "Config" row is retail's configwi hub (vault main_menu.md §35),
        // built now as MenuKind::RetailConfig.
        assert_eq!(
            root_child_kind("Interface", false, false),
            Some(MenuKind::Config)
        );
        assert_eq!(
            root_child_kind("Config", false, false),
            Some(MenuKind::RetailConfig)
        );
        // The Controls page is dynamic now (bead kuluu-o9qt7.26.1): nothing
        // static stands, and its command rows trail the live key map.
        assert!(static_entries(MenuKind::Controls).is_empty());
        let std_rows = controls_rows(&crate::keybinds::Preset::Standard.bindings());
        for label in CONTROLS_COMMAND_LABELS {
            assert!(std_rows.iter().any(|r| r.label == label));
        }
        assert_eq!(pane_width_for(MenuKind::Config), GRAPHICS_PANE_WIDTH);
    }

    /// The bead law: bindings appear on the page **after a change, without any
    /// restart** — rows are rebuilt from the live store every refresh.
    #[test]
    fn controls_page_lists_the_live_key_map_and_follows_the_store() {
        use crate::keybinds::{Action, Bindings, KeyBind};
        let mut bindings = Bindings::empty();
        bindings.insert(
            Action::ConfirmAction,
            KeyBind::new(bevy::input::keyboard::KeyCode::Enter),
        );
        bindings.insert(
            Action::OpenChat,
            KeyBind::with(
                bevy::input::keyboard::KeyCode::Space,
                crate::keybinds::Modifiers::CTRL,
            ),
        );
        let rows = controls_rows(&bindings);
        assert_eq!(rows.len(), 2 + CONTROLS_COMMAND_LABELS.len());
        // Store order (Action's Ord) puts OpenChat before ConfirmAction.
        // A modified bind has no plain key name, and draws the placeholder.
        assert_eq!(rows[0].label, "Open chat: ...");
        assert_eq!(rows[1].label, "Confirm: Enter");
        // Remapping the same store re-lists it — no restart involved.
        bindings.insert(
            Action::ConfirmAction,
            KeyBind::new(bevy::input::keyboard::KeyCode::KeyJ),
        );
        let after = controls_rows(&bindings);
        assert_eq!(after[1].label, "Confirm: J");
    }

    #[test]
    fn controls_command_rows_carry_preset_and_reset_actions() {
        use crate::keybinds::{Bindings, Preset};
        let rows = controls_rows(&Bindings::empty());
        let actions: Vec<DynamicMenuAction> = rows.iter().map(|r| r.action).collect();
        assert_eq!(
            actions,
            vec![
                DynamicMenuAction::ControlsPreset(Preset::Standard),
                DynamicMenuAction::ControlsPreset(Preset::Compact1),
                DynamicMenuAction::ControlsPreset(Preset::Compact2),
                DynamicMenuAction::ControlsReset,
            ]
        );
    }

    // FFXiMain.dll retail-2026-09 RVA 0x13F750 answers mgcmenu selects and maps
    // the eight item ids {4..B} to internal school ids esi ∈ {1,2,5,4,3,6,7,8}
    // through jump table 0x13F7CC - display order is NOT numeric (vault main_menu.md §21).
    #[test]
    fn mgcmenu_picker_rows_follow_the_jump_table_order() {
        let cats: Vec<u8> = (0..MAGIC_CATEGORY_ROWS.len())
            .map(|i| magic_category_at(i).unwrap())
            .collect();
        assert_eq!(cats, [1, 2, 5, 4, 3, 6, 7, 8]);
        assert_eq!(static_entries(MenuKind::MagicGroups), MAGIC_CATEGORY_LABELS);
        let labels: Vec<&str> = MAGIC_CATEGORY_ROWS.iter().map(|(_, l)| *l).collect();
        assert_eq!(labels.to_vec(), MAGIC_CATEGORY_LABELS.to_vec());
        assert_eq!(magic_category_for_label("Songs"), Some(5));
        assert_eq!(magic_category_at(MAGIC_CATEGORY_ROWS.len()), None);
    }

    #[test]
    fn magic_windows_keep_retail_titles() {
        // The picker carries the Magic row's own caption; the spell list it opens is
        // titled from the ROM/165/76.DAT catalog index 76 (§10).
        assert_eq!(menu_title(MenuKind::MagicGroups), "Magic");
        assert_eq!(menu_title(MenuKind::MagicGroup(1)), "Magic List");
    }

    #[test]
    fn magic_group_lists_only_its_school_and_casts_like_magic() {
        // No install loaded, so this exercises the LSB `group`-column fallback.
        let table = MagicSchoolTable::default();
        assert_eq!(table.school_of(1), Some(1)); // cure -> white
        assert_eq!(table.school_of(144), Some(2)); // fire -> black
        assert_eq!(table.school_of(603), Some(6)); // wild oats -> blue
        let rows = magic_school_rows(&[1, 144, 603], 2, |id| table.school_of(id));
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].action,
            DynamicMenuAction::CastSpell { spell_id: 144 }
        );
        let unknown_school = magic_school_rows(&[], 8, |id| table.school_of(id));
        assert!(unknown_school.is_empty());
        assert!(is_dynamic(MenuKind::MagicGroup(2)));
    }

    /// One real spell per school bucket (ids are the first row of each
    /// `spell_list.group` value in vendor/server/sql/spell_list.sql). The client's
    /// school numbering is a shuffled bijection of `xi.magic.spellGroup`
    /// (vault main_menu.md §21), so pinning all eight keeps the picker → list link
    /// from silently drifting when either enum changes.
    #[test]
    fn every_school_bucket_maps_real_spells_to_the_client_number() {
        let table = MagicSchoolTable::default();
        const PINNED: &[(u16, u8, &str)] = &[
            (1, MAGIC_CAT_WHITE, "cure (lsb group 6)"),
            (144, MAGIC_CAT_BLACK, "fire (2)"),
            (368, MAGIC_CAT_SONGS, "foe requiem (1)"),
            (318, MAGIC_CAT_NINJUTSU, "monomi ichi (4)"),
            (288, MAGIC_CAT_SUMMONING, "fire spirit (5)"),
            (513, MAGIC_CAT_BLUE, "venom shell (3)"),
            (768, MAGIC_CAT_GEOMANCY, "indi-regen (7)"),
            (896, MAGIC_CAT_TRUST, "shantotto (8)"),
        ];
        for (id, school, what) in PINNED {
            assert_eq!(table.school_of(*id), Some(*school), "{what}");
        }
        let all: Vec<u16> = PINNED.iter().map(|(id, _, _)| *id).collect();
        for (school, label) in MAGIC_CATEGORY_ROWS {
            let rows = magic_school_rows(&all, *school, |id| table.school_of(id));
            assert_eq!(
                rows.len(),
                1,
                "the {label} list must hold exactly one of the pinned spells"
            );
        }
    }

    /// The flat spell list (kuluu's Retail+ view) names every known spell from the
    /// scraped table and casts it by id; unknown ids drop out. Neither magic kind
    /// carries a static placeholder, so an empty one shows its grounded hint.
    #[test]
    fn magic_lists_build_from_data_and_keep_no_placeholder() {
        let rows = spell_rows(&[1, 144]);
        assert_eq!(
            rows.iter()
                .map(|r| (r.label.as_str(), r.action))
                .collect::<Vec<_>>(),
            vec![
                ("Cure", DynamicMenuAction::CastSpell { spell_id: 1 }),
                ("Fire", DynamicMenuAction::CastSpell { spell_id: 144 }),
            ]
        );
        assert!(spell_rows(&[0]).is_empty(), "unknown spell ids drop out");

        let none = DynamicMenu::default();
        assert!(static_entries(MenuKind::Magic).is_empty());
        assert!(static_entries(MenuKind::MagicGroup(2)).is_empty());
        assert_eq!(
            entry_label(MenuKind::Magic, 0, &none),
            "(no spells learned yet)"
        );
        assert_eq!(
            entry_label(MenuKind::MagicGroup(2), 0, &none),
            "(no spells learned in this school)"
        );
    }

    #[test]
    fn root_magic_row_drills_into_the_picker_unless_flat_is_on() {
        assert_eq!(
            root_child_kind(ROOT_MAGIC_LIST, false, false),
            Some(MenuKind::MagicGroups)
        );
        // The Abilities row mirrors Magic: picker by default, flat list behind
        // the Retail+ toggle (vault main_menu.md §23).
        assert_eq!(
            root_child_kind("Abilities", false, false),
            Some(MenuKind::AbilitiesGroups)
        );
        assert_eq!(
            root_child_kind("Abilities", true, false),
            Some(MenuKind::AbilitiesGroups)
        );
        assert_eq!(
            root_child_kind("Abilities", false, true),
            Some(MenuKind::Abilities)
        );
        assert_eq!(
            root_child_kind(ROOT_MAGIC_LIST, true, false),
            Some(MenuKind::Magic)
        );
    }

    /// s2c COMMAND_DATA ships three bitmaps in three id spaces, so a
    /// weapon-skill row must be named from `weapon_skills` and a job/pet row from
    /// `abilities`. Ability id 32 is Warcry while weapon skill 32 is Fast Blade —
    /// naming one through the other table is the bug this pins (vault §52).
    #[test]
    fn ability_rows_name_each_id_space_from_its_own_table() {
        use crate::hud::action_model::AbilityGroup;
        let snap = kuluu_snapshot::SceneSnapshot {
            weaponskills_known: vec![1, 32],
            job_abilities_known: vec![16, 32],
            pet_abilities_known: vec![80],
            ..Default::default()
        };
        let ws: Vec<String> = ability_group_rows(&snap, AbilityGroup::WeaponSkill)
            .iter()
            .map(|r| r.label.clone())
            .collect();
        assert_eq!(ws, ["Combo", "Fast Blade"], "weapon skills sort by name");

        let job: Vec<String> = ability_group_rows(&snap, AbilityGroup::JobAbilities)
            .iter()
            .map(|r| r.label.clone())
            .collect();
        assert_eq!(job, ["Mighty Strikes", "Warcry"]);

        let pet: Vec<String> = ability_group_rows(&snap, AbilityGroup::PetCommand)
            .iter()
            .map(|r| r.label.clone())
            .collect();
        assert_eq!(pet, ["Spirit Link"]);
    }

    /// The flat list keeps the three spaces apart by suffix and drops nothing that
    /// a bitmap reports; neither ability kind carries a static placeholder, so an
    /// empty one falls back to its grounded hint.
    #[test]
    fn flat_ability_list_separates_the_spaces_and_keeps_no_placeholder() {
        let snap = kuluu_snapshot::SceneSnapshot {
            weaponskills_known: vec![32],
            job_abilities_known: vec![16],
            pet_abilities_known: vec![80],
            ..Default::default()
        };
        assert_eq!(
            ability_flat_rows(&snap)
                .iter()
                .map(|r| (r.label.as_str(), r.action))
                .collect::<Vec<_>>(),
            vec![
                (
                    "Mighty Strikes",
                    DynamicMenuAction::JobAbility { ability_id: 16 }
                ),
                (
                    "Fast Blade (WS)",
                    DynamicMenuAction::Weaponskill { skill_id: 32 }
                ),
                (
                    "Spirit Link (Pet)",
                    DynamicMenuAction::PetAbility { ability_id: 80 }
                ),
            ]
        );

        let none = DynamicMenu::default();
        assert!(static_entries(MenuKind::Abilities).is_empty());
        assert!(static_entries(MenuKind::AbilitiesGroup(1)).is_empty());
        assert_eq!(
            entry_label(MenuKind::Abilities, 0, &none),
            "(no abilities available — wrong job?)"
        );
    }

    /// The abimenu picker rows pair positionally with the target-action groups
    /// (AbilityGroup::ALL) that back their lists, and keep the retail two-window
    /// titles like Magic's pair (vault main_menu.md §23).
    #[test]
    fn abilities_picker_rows_pair_with_the_groups_and_keep_titles() {
        use crate::hud::action_model::AbilityGroup;
        assert_eq!(ABILITIES_CATEGORY_LABELS.len(), AbilityGroup::ALL.len());
        for (idx, _group) in AbilityGroup::ALL.iter().enumerate() {
            let row = ABILITIES_CATEGORY_LABELS[idx];
            assert!(!row.is_empty(), "picker row {idx} needs a label");
            assert_eq!(abilities_category_index(row), Some(idx));
        }
        assert_eq!(
            static_entries(MenuKind::AbilitiesGroups),
            ABILITIES_CATEGORY_LABELS
        );
        assert!(is_dynamic(MenuKind::AbilitiesGroup(2)));
        assert_eq!(menu_title(MenuKind::AbilitiesGroups), "Abilities");
        assert_eq!(menu_title(MenuKind::AbilitiesGroup(1)), "Ability List");
    }

    /// The Party window's rows follow the helper's state dispatch (FFXiMain.dll
    /// retail-2026-09 RVA 0x2016E0; vault main_menu.md §24): a pending solicit
    /// yields prtyjoin's YES/NO answer rows, else the roster renders as display
    /// rows, and solo leaves no rows so the "(not in a party)" hint stands.
    #[test]
    fn party_window_rows_follow_the_helper_law() {
        let solo = kuluu_snapshot::SceneSnapshot::default();
        assert!(
            party_window_rows(&solo).is_empty(),
            "solo shows the empty hint"
        );

        let rostered = kuluu_snapshot::SceneSnapshot {
            party: vec![war_party_member(7)],
            ..Default::default()
        };
        let rows = party_window_rows(&rostered);
        assert_eq!(labels(&rows), ["Kupo"]);
        assert_eq!(
            rows[0].action,
            DynamicMenuAction::PartyMemberRow { char_id: 7 }
        );

        let invited = kuluu_snapshot::SceneSnapshot {
            party_invite: Some(kuluu_snapshot::PartyInvite {
                char_id: 9,
                name: "Pruneo".into(),
                alliance: false,
            }),
            party: vec![war_party_member(7)],
            ..Default::default()
        };
        let rows = party_window_rows(&invited);
        assert_eq!(labels(&rows), ["Yes (join Pruneo)", "No"]);
        assert_eq!(rows[0].action, DynamicMenuAction::AcceptPartyInvite);
        assert_eq!(rows[1].action, DynamicMenuAction::DeclinePartyInvite);
    }

    #[test]
    fn party_root_row_drills_into_the_party_window() {
        assert_eq!(
            root_child_kind("Party", false, false),
            Some(MenuKind::Party)
        );
        assert_eq!(menu_title(MenuKind::Party), "Party");
        assert!(is_dynamic(MenuKind::Party));
    }

    #[test]
    fn settings_pages_are_disjoint_and_have_no_duplicate_rows() {
        let mut seen: Vec<GraphicsField> = Vec::new();
        for page in [
            crate::graphics_settings::graphics_fields(),
            crate::graphics_settings::config_fields(),
            crate::graphics_settings::dlss_config_fields(),
        ] {
            for f in page {
                assert!(!seen.contains(&f), "{f:?} appears on more than one page");
                seen.push(f);
            }
        }
        assert!(
            crate::graphics_settings::dlss_config_fields()
                .iter()
                .all(|f| f.is_dlss_config()),
            "every DLSS submenu row must be flagged is_dlss_config"
        );
        assert!(
            !crate::graphics_settings::graphics_fields()
                .iter()
                .any(|f| f.is_dlss_config()),
            "DLSS submenu rows must not also sit on the main Graphics page"
        );
    }

    #[test]
    fn field_rows_carry_their_field_label() {
        for kind in [MenuKind::Graphics, MenuKind::Config, MenuKind::GraphicsDlss] {
            for (slot, row) in settings_rows(kind, true).unwrap().iter().enumerate() {
                if let MenuRow::Field(field) = row {
                    assert_eq!(
                        settings_field_at(kind, slot, true),
                        Some(*field),
                        "{kind:?} slot {slot} resolves to the wrong field"
                    );
                    assert_eq!(row.label(), field.label());
                }
            }
        }
    }

    #[test]
    fn graphics_rows_drop_both_dlss_rows_when_unsupported() {
        use crate::graphics_settings::MenuAction;
        let bare = graphics_rows(false);
        assert!(!bare
            .iter()
            .any(|r| *r == MenuRow::Field(GraphicsField::Dlss)
                || *r == MenuRow::Action(MenuAction::DlssConfig)));
        assert_eq!(
            bare.last(),
            Some(&MenuRow::Action(MenuAction::ResetToMinimum))
        );

        let full = graphics_rows(true);
        let dlss = full
            .iter()
            .position(|r| *r == MenuRow::Field(GraphicsField::Dlss))
            .expect("DLSS row present when supported");
        assert_eq!(
            full[dlss + 1],
            MenuRow::Action(MenuAction::DlssConfig),
            "the config row must sit directly under the DLSS on/off row"
        );
        assert_eq!(bare.len(), full.len() - 2);
    }

    #[test]
    fn the_cursor_never_rests_on_a_header() {
        for kind in [MenuKind::Graphics, MenuKind::Config, MenuKind::GraphicsDlss] {
            for dlss in [true, false] {
                let rows = settings_rows(kind, dlss).unwrap();
                for (slot, _) in rows.iter().enumerate() {
                    for down in [true, false] {
                        let settled = settle_cursor(kind, dlss, slot, down);
                        assert!(
                            rows[settled].is_selectable(),
                            "{kind:?} slot {slot} settled onto header row {settled}"
                        );
                    }
                }
                assert!(rows[settle_cursor(kind, dlss, 0, true)].is_selectable());
            }
        }
    }

    #[test]
    fn enhanced_only_rows_sit_in_the_trailing_group() {
        use crate::graphics_settings::{ENHANCED_SECTION, GRAPHICS_SECTIONS};
        let last = GRAPHICS_SECTIONS.last().expect("sections");
        assert_eq!(
            last.header, ENHANCED_SECTION,
            "the no-Vanilla-equivalent group must be the last section on the page"
        );
        for section in GRAPHICS_SECTIONS
            .iter()
            .filter(|s| s.header != ENHANCED_SECTION)
        {
            assert!(
                !section.fields.contains(&GraphicsField::CameraSpring)
                    && !section.fields.contains(&GraphicsField::DepthOfField)
                    && !section.fields.contains(&GraphicsField::BloomIntensity),
                "{} must not hold a row the original client never had",
                section.header
            );
        }
    }

    /// One-slot snapshot builder for the LSB 0x037 usability gate tests
    /// (kuluu-268h). Item ids pinned against vendor/server/sql:
    /// 4112 = potion (consumable), 15840 = kupofried's ring (charged
    /// equipment), 13327 = silver earring (plain equipment, no activation).
    fn slot_snapshot(container: u8, item_no: u16, locked: bool) -> kuluu_snapshot::SceneSnapshot {
        kuluu_snapshot::SceneSnapshot {
            containers: vec![kuluu_snapshot::ContainerView {
                id: container,
                capacity: 30,
                items: vec![kuluu_snapshot::InventoryItem {
                    container,
                    index: 0,
                    item_no,
                    quantity: 1,
                    locked,
                    unselectable: false,
                    charges_remaining: None,
                    next_use_vana_ts: None,
                    use_delay_end_vana_ts: None,
                    ready: None,
                    extdata: None,
                }],
            }],
            ..Default::default()
        }
    }

    fn only_slot(snap: &kuluu_snapshot::SceneSnapshot) -> &kuluu_snapshot::InventoryItem {
        &snap.containers[0].items[0]
    }

    /// Consumables fire from LOC_INVENTORY and LOC_TEMPITEMS only
    /// (0x037 → CState container check).
    #[test]
    fn consumable_usable_from_inventory_and_tempitems_only() {
        use ffxi_proto::map::container as c;
        for (container, expect) in [
            (c::LOC_INVENTORY, true),
            (c::LOC_TEMPITEMS, true),
            (c::LOC_MOGSAFE, false),
            (c::LOC_STORAGE, false),
            (c::LOC_WARDROBE, false),
        ] {
            let snap = slot_snapshot(container, 4112, false);
            assert_eq!(
                item_usable_now(&snap, only_slot(&snap)),
                expect,
                "potion in container {container}"
            );
        }
    }

    /// Locked (bazaar/linkshell-reserved) consumables are rejected by the
    /// server, so the menu must not offer them.
    #[test]
    fn locked_consumable_is_not_usable() {
        use ffxi_proto::map::container as c;
        let snap = slot_snapshot(c::LOC_INVENTORY, 4112, true);
        assert!(!item_usable_now(&snap, only_slot(&snap)));
    }

    /// Charged equipment (kupofried's ring) is usable only while equipped:
    /// locked slot + mirrored in SceneSnapshot::equipped.
    #[test]
    fn charged_equipment_usable_only_while_equipped() {
        use ffxi_proto::map::container as c;
        let mut snap = slot_snapshot(c::LOC_INVENTORY, 15840, true);
        snap.equipped[13] = Some(15840);
        assert!(item_usable_now(&snap, only_slot(&snap)));

        // Sitting unequipped in the bag: not usable.
        let unequipped = slot_snapshot(c::LOC_INVENTORY, 15840, false);
        assert!(!item_usable_now(&unequipped, only_slot(&unequipped)));

        // Equipped but stored via a non-wardrobe container: not usable.
        let mut stored = slot_snapshot(c::LOC_MOGSAFE, 15840, true);
        stored.equipped[13] = Some(15840);
        assert!(!item_usable_now(&stored, only_slot(&stored)));
    }

    /// Plain equipment (no item_usable entry) never appears, even equipped.
    #[test]
    fn plain_equipment_is_never_usable() {
        use ffxi_proto::map::container as c;
        let mut snap = slot_snapshot(c::LOC_INVENTORY, 13327, true);
        snap.equipped[11] = Some(13327);
        assert!(!item_usable_now(&snap, only_slot(&snap)));
    }

    /// `vendor/server/sql/item_equipment.sql` rows used by the equip pick-list
    /// tests, with their (`level`, `jobs`, `slot`) columns: bronze cap (1, WAR,
    /// head), Pummelers Mask +2 (99, WAR-only, head), white belt (1, MNK-only,
    /// waist), bronze dagger (1, WAR, main+sub).
    const BRONZE_CAP: u16 = 12448;
    const PM_MASK_PLUS_2: u16 = 23040;
    const WHITE_BELT: u16 = 13184;
    const BRONZE_DAGGER: u16 = 16448;

    /// Main-bag snapshot holding those four pieces, self rostered as `main_job`
    /// at level `main_job_lv` (the pair the server's gate reads).
    fn equip_bag_snapshot(main_job: u8, main_job_lv: u8) -> kuluu_snapshot::SceneSnapshot {
        let gear = |index: u8, item_no: u16| kuluu_snapshot::InventoryItem {
            container: 0,
            index,
            item_no,
            quantity: 1,
            locked: false,
            unselectable: false,
            charges_remaining: None,
            next_use_vana_ts: None,
            use_delay_end_vana_ts: None,
            ready: None,
            extdata: None,
        };
        let mut member = war_party_member(7);
        member.main_job = main_job;
        member.main_job_lv = main_job_lv;
        kuluu_snapshot::SceneSnapshot {
            self_char_id: Some(7),
            party: vec![member],
            containers: vec![kuluu_snapshot::ContainerView {
                id: 0,
                capacity: 30,
                items: vec![
                    gear(4, BRONZE_CAP),
                    gear(5, PM_MASK_PLUS_2),
                    gear(6, WHITE_BELT),
                    gear(7, BRONZE_DAGGER),
                ],
            }],
            ..Default::default()
        }
    }

    /// What the window actually shows: `equip_slot_rows` then the refresh pass's
    /// ordering step for this kind.
    fn equip_pick_labels(snap: &kuluu_snapshot::SceneSnapshot, equip_slot: u8) -> Vec<String> {
        let mut rows = equip_slot_rows(snap, equip_slot, None);
        order_dynamic_rows(
            MenuKind::EquipSlot(equip_slot),
            &ALWAYS_CATEGORY_SORT,
            &mut rows,
        );
        rows.iter().map(|r| r.label.clone()).collect()
    }

    /// The pick list is the bag filtered by the server's own acceptance gate
    /// (`EquipArmor`: equip-slot bit, main-job bit, required level) — vault
    /// main_menu.md §50. A wrong-slot item never shows; an over-level or
    /// wrong-job item never shows.
    #[test]
    fn equip_pick_list_filters_by_slot_job_and_level() {
        let war40 = equip_bag_snapshot(1, 40);
        assert_eq!(
            equip_pick_labels(&war40, 4),
            ["Bronze Cap (Lv1)"],
            "head: the level-99 mask is gated and the belt/dagger are wrong-slot"
        );
        assert_eq!(equip_pick_labels(&war40, 0), ["Bronze Dagger (Lv1)"]);
        assert!(
            equip_pick_labels(&war40, 10).is_empty(),
            "a white belt is MNK-only: WAR must not be offered it"
        );

        let mnk99 = equip_bag_snapshot(2, 99);
        assert_eq!(
            equip_pick_labels(&mnk99, 10),
            ["White Belt (Lv1)"],
            "MNK gets the belt it can wear"
        );
        assert_eq!(
            equip_pick_labels(&mnk99, 4),
            ["Bronze Cap (Lv1)"],
            "the WAR-only mask stays out even at level 99"
        );
    }

    /// Rows carry the exact c2s 0x050 field triple (bag, bag index, equip slot)
    /// so a select equips that very slot, and the empty state renders the
    /// grounded hint rather than any loading placeholder.
    #[test]
    fn equip_pick_rows_carry_the_equip_set_fields_and_hint_when_empty() {
        let snap = equip_bag_snapshot(1, 40);
        let rows = equip_slot_rows(&snap, 4, None);
        assert_eq!(
            rows.iter().map(|r| r.action).collect::<Vec<_>>(),
            vec![DynamicMenuAction::EquipItem {
                container: 0,
                container_index: 4,
                equip_slot: 4,
                item_no: BRONZE_CAP,
            }],
            "one row, and it must name the bag slot the packet carries"
        );

        let empty = DynamicMenu::default();
        assert!(static_entries(MenuKind::EquipSlot(4)).is_empty());
        assert_eq!(
            entry_label(MenuKind::EquipSlot(4), 0, &empty),
            "(no equippable items for this slot)"
        );
    }

    /// Retail's legality check (FFXiMain.dll retail-2026-09 RVA 0x132050) runs four steps in a
    /// fixed order and returns as soon as one fails, so each step is pinned on its own: a
    /// reordering must fail loudly rather than quietly offer gear a player cannot use.
    #[test]
    fn record_equip_gate_follows_the_dll_step_order() {
        let base = || EquipRecordGates {
            races_mask: 0b0000_0010,
            slots_mask: 0x1234,
            jobs_mask: 1 << 1,
            level: 1,
            superior_level: 0,
        };
        let actor = || EquipActor {
            race: Some(1),
            main_job: 1,
            main_lv: 40,
            su_lv: 0,
            equip_slot_block: 0,
        };
        assert!(record_allows_equip(base(), actor()));

        // Step 1, races: the bit index is the entity's race byte, so a record whose mask omits
        // it dies before the job test even runs.
        let wrong_race = EquipActor {
            race: Some(2),
            ..actor()
        };
        assert!(!record_allows_equip(base(), wrong_race));
        // No entity look yet means no race step — skipped, never guessed as a refusal.
        assert!(record_allows_equip(
            base(),
            EquipActor {
                race: None,
                ..actor()
            }
        ));

        // Step 2, jobs (the record's mask is indexed by the job id itself — `.text 0x1320EE`).
        assert!(!record_allows_equip(
            base(),
            EquipActor {
                main_job: 7,
                ..actor()
            }
        ));

        // Step 3, item level against the effective main-job level.
        let lv50 = EquipRecordGates {
            level: 50,
            ..base()
        };
        assert!(!record_allows_equip(lv50, actor()));
        assert!(record_allows_equip(
            lv50,
            EquipActor {
                main_lv: 60,
                ..actor()
            }
        ));

        // Step 4, superior level — always compared, so su 0 refuses every Su item and the gate
        // lifts exactly at the record's own requirement.
        let su4 = EquipRecordGates {
            superior_level: 4,
            ..base()
        };
        assert!(!record_allows_equip(su4, actor()));
        assert!(record_allows_equip(
            su4,
            EquipActor {
                su_lv: 4,
                ..actor()
            }
        ));

        // An earlier failing step is the only reason reported: a record bad for race stays bad
        // however good its other columns are (the DLL's early-out).
        assert!(!record_allows_equip(
            EquipRecordGates {
                races_mask: 1 << 9,
                slots_mask: 0x1234,
                jobs_mask: u32::MAX,
                level: 0,
                superior_level: 0,
            },
            actor()
        ));
    }

    /// The scan helper (`.text FFXiMain.dll retail-2026-09 0x131FA0`) walks its own slot
    /// table — ears/ring/back group plus main — comparing equipped item ids; a copy in an
    /// unscanned slot stays invisible to the green.
    #[test]
    fn same_id_scan_walks_only_the_dll_slot_table() {
        let mut equipped = [None; 16];
        for &slot in EQUIP_COLLISION_SCAN_SLOTS.iter().take(4) {
            equipped[slot] = Some(0x2C3A);
        }
        assert!(same_id_equipped_in_scanned_slots(&equipped, 0x2C3A));
        // Slot 5 (legs) is not in the table; neither is slot 1's ring group here.
        let mut elsewhere = [None; 16];
        elsewhere[5] = Some(0x2C3A);
        elsewhere[7] = Some(0x2C3A);
        assert!(!same_id_equipped_in_scanned_slots(&elsewhere, 0x2C3A));
        // Ids must match exactly — the scanned slot holding a different item is no collision.
        let mut other_item = [None; 16];
        other_item[0] = Some(0x1111);
        assert!(!same_id_equipped_in_scanned_slots(&other_item, 0x2C3A));
    }

    /// Precedence (`.text FFXiMain.dll retail-2026-09 0x1DE157`): the green scan wins over
    /// the lime; a row that neither scans nor passes legality carries no tint at all.
    #[test]
    fn equip_row_tint_follows_the_resolver_precedence() {
        assert_eq!(
            equip_row_tint(false, true),
            Some(MenuRowTint::SameKindEquipped)
        );
        assert_eq!(equip_row_tint(true, false), Some(MenuRowTint::WearableNow));
        // Green outranks lime even when both apply.
        assert_eq!(
            equip_row_tint(true, true),
            Some(MenuRowTint::SameKindEquipped)
        );
        assert_eq!(equip_row_tint(false, false), None);
    }

    /// The fifth test (`.text FFXiMain.dll retail-2026-09 0x132111`): the record's slot word
    /// intersected with the player's equip-block dword refuses on any shared bit, and a zero
    /// block — what `.text 0xFD350` answers for an absent player — never refuses.
    #[test]
    fn fifth_test_intersects_record_slots_with_the_player_block() {
        let gates = || EquipRecordGates {
            races_mask: u16::MAX,
            slots_mask: 0x0000_1100,
            jobs_mask: u32::MAX,
            level: 0,
            superior_level: 0,
        };
        let actor = || EquipActor {
            race: Some(1),
            main_job: 1,
            main_lv: 75,
            su_lv: 0,
            equip_slot_block: 0,
        };
        // Disjoint bitmaps still pass; the base fixture's block of 0 passes everything.
        assert!(record_allows_equip(
            gates(),
            EquipActor {
                equip_slot_block: 0x00FF,
                ..actor()
            }
        ));
        // Any single shared bit refuses (bit 8 here is in both).
        assert!(!record_allows_equip(
            gates(),
            EquipActor {
                equip_slot_block: 0x0100,
                ..actor()
            }
        ));
        // The record's own slots do not have to be inside the block — only intersection matters.
        assert!(!record_allows_equip(
            gates(),
            EquipActor {
                equip_slot_block: u32::MAX,
                ..actor()
            }
        ));
    }

    /// On a machine with the retail DATs installed, the gate must come from the record — and the
    /// record's columns must line up with the scraped server columns where both express the same
    /// rule (bead kuluu-o9qt7.3.4). Measured on the 2026-09 install over a strided sample of the
    /// equipment range: slot masks agree exactly, job bits use the same job numbering as the
    /// server's own list except that a handful of items omit Rune Fencer (job 22) client-side,
    /// and item levels differ only for empty placeholder slots.
    #[test]
    fn installed_records_drive_the_equip_gate() {
        let Some(root_path) = crate::scheduler_runtime::install_root_from_env() else {
            eprintln!("no install: pin skipped");
            return;
        };
        let root = ffxi_dat::DatRoot::open(&root_path).expect("install opens");
        let mut recs = EquipRecords::default();
        recs.ensure_loaded(Some(&root));
        // The two newest job ids (GEO, RUN) are the only places where the server's item list and
        // retail's installed records disagree: LSB carries them on gear whose retail record omits
        // the bit. Every other job bit matched for every sampled record.
        const JOB_BITS_WHERE_LSB_LEADS_RETAIL: std::ops::RangeInclusive<u8> = 21..=22;
        let mut sampled = 0u32;
        let (mut compared, mut level_off, mut job_off) = (0usize, Vec::new(), Vec::new());
        let mut su_items = 0u32;
        let mut race_restricted = 0u32;
        for id in (0x1001u32..=0x6FFF).step_by(0x3B) {
            let id = id as u16;
            let Some(rec) = recs.record(id) else { continue };
            // Unused record slots carry no real name (a lone `.` or nothing at all) and hold
            // placeholder columns, so they take part in no comparison.
            if rec.slot_mask == 0
                || !rec
                    .name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric())
            {
                continue;
            }
            sampled += 1;
            if rec.superior_level > 0 {
                su_items += 1;
                // A superior item is refused below its own requirement and taken at it.
                let gates = EquipRecordGates::from_record(&rec);
                let short = 1;
                assert!(
                    !record_allows_equip(
                        gates,
                        EquipActor {
                            race: Some(0),
                            main_job: 0,
                            main_lv: u8::MAX,
                            su_lv: rec.superior_level.saturating_sub(short),
                            equip_slot_block: 0,
                        }
                    ),
                    "{id} {} (su {}) must be refused at su {}",
                    rec.name,
                    rec.superior_level,
                    rec.superior_level - short
                );
            }
            if rec.races_mask != 0 && rec.races_mask != u16::MAX {
                race_restricted += 1;
            }
            let Some(info) = ffxi_vocab::equip_info::lookup(id) else {
                continue;
            };
            compared += 1;
            assert_eq!(
                rec.slot_mask, info.slot_mask,
                "{id} {} slot mask: record vs server scrape",
                rec.name
            );
            if rec.level != info.level {
                level_off.push(format!(
                    "{id} {}: record {} / server {}",
                    rec.name, rec.level, info.level
                ));
            }
            for job in 1..=31u8 {
                let scraped = ffxi_vocab::equip_info::fits_job(&info, job);
                if scraped != gates_has_job(&rec.jobs_mask, job)
                    && !JOB_BITS_WHERE_LSB_LEADS_RETAIL.contains(&job)
                {
                    job_off.push(format!("{id} {} job {job}", rec.name));
                }
            }
        }
        assert!(
            job_off.is_empty(),
            "job-bit disagreements outside GEO/RUN ({compared} compared): {job_off:?}"
        );
        // A handful of records are for things no equip window lists (pet food and its family);
        // their retail level column simply differs from the server's list. They must stay a small
        // minority or the record columns cannot be trusted to drive the gate.
        const MAX_RECORD_LEVEL_DISAGREEMENT_DENOMINATOR: usize = 4;
        assert!(
            level_off.len() * MAX_RECORD_LEVEL_DISAGREEMENT_DENOMINATOR <= compared,
            "{}/{} sampled records disagree on required level: {level_off:?}",
            level_off.len(),
            compared
        );
        assert!(sampled > 200, "only {sampled} records sampled");
        assert!(
            su_items > 5,
            "no superior-requirement gear found among {sampled}"
        );
        assert!(
            race_restricted > 5,
            "no race-restricted gear among {sampled}"
        );
    }

    /// The record's job mask is indexed by the job id (`1 << main_job`, `.text FFXiMain.dll
    /// retail-2026-09 0x1320EE`), unlike the server scrape, which stores job j at bit j-1.
    fn gates_has_job(jobs_mask: &u32, job: u8) -> bool {
        (1u32)
            .checked_shl(u32::from(job))
            .is_some_and(|bit| jobs_mask & bit != 0)
    }

    /// Dev measurement behind `installed_records_drive_the_equip_gate`: prints, over a strided
    /// sample of the equipment range, how the record columns compare with the scraped server
    /// columns (run with
    /// `cargo test -p kuluu-render --lib measure_record_vs_scrape_columns -- --ignored --nocapture`).
    #[test]
    #[ignore = "measurement printout; needs the installed retail DATs"]
    fn measure_record_vs_scrape_columns() {
        let Some(root_path) = crate::scheduler_runtime::install_root_from_env() else {
            eprintln!("NO INSTALL");
            return;
        };
        let root = ffxi_dat::DatRoot::open(&root_path).expect("install opens");
        let mut recs = EquipRecords::default();
        recs.ensure_loaded(Some(&root));
        let (mut both, mut lv_eq, mut job_eq) = (0u32, 0u32, 0u32);
        let (mut job_mism, mut slot_mism) = (Vec::new(), Vec::new());
        let mut race_hist: std::collections::BTreeMap<u16, u32> = Default::default();
        for id in (0x2800u32..=0x6FFF).step_by(0x41) {
            let id = id as u16;
            let Some(rec) = recs.record(id) else { continue };
            if rec.slot_mask == 0 {
                continue;
            }
            *race_hist.entry(rec.races_mask).or_default() += 1;
            let Some(info) = ffxi_vocab::equip_info::lookup(id) else {
                continue;
            };
            both += 1;
            if rec.level == info.level {
                lv_eq += 1;
            }
            let mut same = true;
            for job in 1..=31u8 {
                let scrape = ffxi_vocab::equip_info::fits_job(&info, job);
                let record = rec.jobs_mask & (1u32 << u32::from(job)) != 0;
                if scrape != record {
                    same = false;
                }
            }
            let sql_bits: Vec<u8> = (1..=31u8)
                .filter(|j| ffxi_vocab::equip_info::fits_job(&info, *j))
                .collect();
            let rec_bits: Vec<u8> = (0..=31u8)
                .filter(|j| rec.jobs_mask & (1u32 << *j) != 0)
                .collect();
            if sql_bits.len() == 1 {
                eprintln!(
                    "single-sql id={id} {:?} sql_job={:?} rec_bits={:?} lv {} vs {}",
                    rec.name, sql_bits, rec_bits, rec.level, info.level
                );
            }
            if rec.level != info.level {
                eprintln!(
                    "LEVEL MISMATCH id={id} {:?} rec={} sql={} jobs_mask={:08X}",
                    rec.name, rec.level, info.level, rec.jobs_mask
                );
            }
            if rec.slot_mask != info.slot_mask {
                slot_mism.push((id, rec.slot_mask, info.slot_mask));
            }
            if same {
                job_eq += 1;
            } else {
                job_mism.push((
                    id,
                    format!(
                        "{:?} sql={:?} rec={:08X}/{:?}",
                        rec.name, sql_bits, rec.jobs_mask, rec_bits
                    ),
                ));
            }
        }
        eprintln!("both={both} level_equal={lv_eq} jobs_identical={job_eq}");
        eprintln!("race masks: {race_hist:?}");
        eprintln!(
            "slot mismatch count={} sample={:?}",
            slot_mism.len(),
            &slot_mism[..slot_mism.len().min(6)]
        );
        for m in job_mism.iter().take(16) {
            eprintln!("JOBS MISMATCH {}", m.1);
        }
    }

    /// The quick-menu Items rows fire Use directly and skip unusable slots;
    /// any_usable_item gates the entry itself.
    #[test]
    fn usable_item_rows_fire_use_directly() {
        use ffxi_proto::map::container as c;
        let snap = slot_snapshot(c::LOC_INVENTORY, 4112, false);
        let rows = usable_item_rows(&snap);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(matches!(
            rows[0].action,
            DynamicMenuAction::UseItem {
                container: c::LOC_INVENTORY,
                index: 0,
                item_no: 4112,
            }
        ));
        assert!(any_usable_item(&snap));

        let safe = slot_snapshot(c::LOC_MOGSAFE, 4112, false);
        assert!(usable_item_rows(&safe).is_empty());
        assert!(!any_usable_item(&safe));
    }

    fn trade_offer(item_no: u16, qty: u32) -> kuluu_snapshot::TradeOffer {
        kuluu_snapshot::TradeOffer { item_no, qty }
    }

    /// The trade window's row law (FFXiMain.dll retail-2026-09 vtable 0x335060
    /// handler RVA 0x1515E0; vault main_menu.md §25): an unanswered s2c 0x021
    /// yields the answer rows; a live trade shows gil (trade slot 0 per LSB's
    /// `GilSlot = 0`), own item slots {1..8} (select byte table .rdata 0x371D60),
    /// then the button law {9=Confirm, 10=Exit}; finally the partner-side views.
    #[test]
    fn trade_window_rows_follow_the_helper_law() {
        use kuluu_snapshot::{TradeOffer, TradeWindow};

        // No transaction behind the window: the empty-hint display state.
        let solo = kuluu_snapshot::SceneSnapshot::default();
        assert!(trade_window_rows(&solo).is_empty());

        // Unanswered invite (no entity fixture, so the hex fallback names the
        // partner): YES/NO rows answer c2s TRADE_RES Start/Cancell.
        let invited = kuluu_snapshot::SceneSnapshot {
            trade: Some(Box::new(TradeWindow {
                partner_id: 42,
                incoming_pending: true,
                active: false,
                my_slots: Vec::new(),
                partner_slots: Vec::new(),
                my_confirmed: false,
                partner_confirmed: false,
            })),
            ..Default::default()
        };
        let rows = trade_window_rows(&invited);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].label, "Yes (accept 0000002A)");
        assert_eq!(rows[1].label, "No");
        assert_eq!(rows[0].action, DynamicMenuAction::TradeAcceptInvite);
        assert_eq!(rows[1].action, DynamicMenuAction::TradeDeclineInvite);

        // Live trade: gil staged in slot 0 plus an item stack in slot 3, the
        // partner offering a single unit in their slot 7.
        let mut my_slots: Vec<Option<TradeOffer>> = vec![None; 9];
        my_slots[0] = Some(trade_offer(ffxi_proto::decode::TRADE_GIL_ITEM_NO, 150));
        my_slots[3] = Some(trade_offer(4112, 2));
        let mut partner_slots: Vec<Option<TradeOffer>> = vec![None; 9];
        partner_slots[7] = Some(trade_offer(4112, 1));
        let live = kuluu_snapshot::SceneSnapshot {
            trade: Some(Box::new(TradeWindow {
                partner_id: 42,
                incoming_pending: false,
                active: true,
                my_slots,
                partner_slots,
                my_confirmed: false,
                partner_confirmed: true,
            })),
            ..Default::default()
        };
        let rows = trade_window_rows(&live);
        let potion = ffxi_vocab::item_names::lookup(4112).expect("4112 has a retail name");
        assert_eq!(
            rows.len(),
            1 + 8 + 2 + 9,
            "gil cell + own slots 1..=8 + Confirm/Exit + partner views: {rows:?}"
        );
        assert_eq!(&rows[0].label, &format!("Gil x{GIL}", GIL = 150));
        assert!(matches!(rows[0].action, DynamicMenuAction::TradeGilRow));
        // Row index == own trade-slot index (slot 0 sits above slots 1..8 in the
        // same column), so slot 3 is row 3.
        assert_eq!(&rows[3].label, &format!("{potion} x2"));
        assert_eq!(
            rows[3].action,
            DynamicMenuAction::TradeOwnSlot { trade_index: 3 }
        );
        assert_eq!(rows[1].label, "(empty)");
        assert_eq!(&rows[9].label, "Confirm");
        assert!(matches!(rows[9].action, DynamicMenuAction::TradeConfirmRow));
        assert_eq!(&rows[10].label, "Exit");
        assert!(matches!(rows[10].action, DynamicMenuAction::TradeCancelRow));
        // Partner views append after the buttons, their slot 7 at row 18.
        assert_eq!(&rows[18].label, potion);
        assert_eq!(
            rows[18].action,
            DynamicMenuAction::TradePartnerOfferRow { trade_index: 7 }
        );
    }

    /// The Add-Item picker skips locked and server-unselectable slots (LSB's
    /// setSlot refuses them) and stages ONE unit per pick while the qty keypad
    /// stays [I-pending] (vault main_menu.md §25).
    #[test]
    fn trade_pick_rows_skip_locked_slots() {
        let snap = kuluu_snapshot::SceneSnapshot {
            containers: vec![kuluu_snapshot::ContainerView {
                id: 0,
                capacity: 30,
                items: vec![
                    kuluu_snapshot::InventoryItem {
                        container: 0,
                        index: 4,
                        item_no: 4112,
                        quantity: 3,
                        locked: false,
                        unselectable: false,
                        charges_remaining: None,
                        next_use_vana_ts: None,
                        use_delay_end_vana_ts: None,
                        ready: None,
                        extdata: None,
                    },
                    kuluu_snapshot::InventoryItem {
                        container: 0,
                        index: 5,
                        item_no: 4112,
                        quantity: 1,
                        locked: true,
                        unselectable: false,
                        charges_remaining: None,
                        next_use_vana_ts: None,
                        use_delay_end_vana_ts: None,
                        ready: None,
                        extdata: None,
                    },
                ],
            }],
            ..Default::default()
        };
        let rows = trade_pick_rows(&snap, 5);
        assert_eq!(
            rows.len(),
            1,
            "the locked slot must not be offered: {rows:?}"
        );
        let potion = ffxi_vocab::item_names::lookup(4112).expect("4112 has a retail name");
        assert_eq!(&rows[0].label, &format!("{potion} x3"));
        assert_eq!(
            rows[0].action,
            DynamicMenuAction::TradeStageFromBag {
                trade_index: 5,
                item_no: 4112,
                qty: 1,
                bag_slot: 4,
            }
        );
    }

    fn craft_bag(items: &[(u8, u16, u32)], key_items: &[u16]) -> kuluu_snapshot::SceneSnapshot {
        kuluu_snapshot::SceneSnapshot {
            containers: vec![kuluu_snapshot::ContainerView {
                id: 0,
                capacity: 30,
                items: items
                    .iter()
                    .map(|(index, item_no, quantity)| kuluu_snapshot::InventoryItem {
                        container: 0,
                        index: *index,
                        item_no: *item_no,
                        quantity: *quantity,
                        locked: false,
                        unselectable: false,
                        charges_remaining: None,
                        next_use_vana_ts: None,
                        use_delay_end_vana_ts: None,
                        ready: None,
                        extdata: None,
                    })
                    .collect(),
            }],
            key_items: key_items.to_vec(),
            ..Default::default()
        }
    }

    fn plan_of(row: &DynamicMenuRow) -> Vec<ffxi_proto::synth::SynthIngredient> {
        match row.action {
            DynamicMenuAction::SynthesizeRecipe { ingredients, .. } => {
                ingredients.into_iter().flatten().collect()
            }
            other => panic!("expected a synth plan, got {other:?}"),
        }
    }

    fn crystal_of(row: &DynamicMenuRow) -> (u16, u8) {
        match row.action {
            DynamicMenuAction::SynthesizeRecipe {
                crystal_id,
                crystal_slot,
                ..
            } => (crystal_id, crystal_slot),
            other => panic!("expected a synth plan, got {other:?}"),
        }
    }

    /// Craft-list law (vault main_menu.md §28): a recipe earns a row when its crystal passes the
    /// element-tier gate (`crystalProps` folds tiers; `resolveRecipe` keys on the folded name) and
    /// every positional ingredient assigns to bag slots under LSB's per-stack use law. Rows carry
    /// the wire plan verbatim, in recipe-id order; key-item gates mirror `resolveRecipe`.
    #[test]
    fn craft_list_rows_are_the_recipes_the_bag_can_currently_satisfy() {
        assert!(craft_list_rows(&kuluu_snapshot::SceneSnapshot::default()).is_empty());

        // synth_recipes rows 1-3: Banquet Table (earth crystal + 3553/3554/3555), Arrowwood
        // Lumber (wind crystal + one 688), and the key-item-1986 synth (688 x3 + 1657). The seeded
        // crystals are each ONE TIER UP (CYCLONE for wind, TERRA for earth) to also prove the §28
        // element-tier substitution law on every row.
        let bag = craft_bag(
            &[
                (2, 4240, 1),
                (3, 4241, 1),
                (5, 3553, 1),
                (6, 3554, 1),
                (7, 3555, 1),
                (8, 688, 3),
                (9, 1657, 1),
            ],
            &[1986],
        );
        let rows = craft_list_rows(&bag);

        let table = rows
            .iter()
            .find(|r| r.label == "Banquet Table")
            .expect("row 1");
        assert_eq!(crystal_of(table), (4241, 3));
        assert_eq!(
            plan_of(table).iter().map(|e| e.item_id).collect::<Vec<_>>(),
            vec![3553, 3554, 3555],
            "entries keep the recipe row's own column order"
        );

        // Stack law on the KI synth: three copies of item 688 share ONE slot-8 stack (LSB counts
        // uses per slot against the quantity - slotQty in GP_CLI_COMMAND_COMBINE_ASK::process).
        let triple = rows
            .iter()
            .find(|r| plan_of(r).iter().any(|e| e.item_id == 1657))
            .expect("the KI synth");
        assert_eq!(triple.label, "Arrowwood Lumber");
        assert_eq!(
            plan_of(triple)
                .iter()
                .map(|e| (e.item_id, e.inv_slot))
                .collect::<Vec<_>>(),
            vec![(688, 8), (688, 8), (688, 8), (1657, 9)]
        );

        // Id order: Banquet Table (id 1) precedes both Arrowwood rows (ids 2 and 3), and the KI gate
        // drops id 3 when the key item is unknown (`resolveRecipe`).
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        let first_table = labels.iter().position(|l| *l == "Banquet Table").unwrap();
        let last_row = labels.len() - 1;
        assert!(labels[..last_row].contains(&"Arrowwood Lumber"));
        assert_eq!(
            rows.iter()
                .filter(|r| r.label == "Arrowwood Lumber")
                .count(),
            2,
            "id order keeps both Arrowwood rows after Banquet Table (pos {first_table})"
        );

        let no_ki = craft_bag(
            &[
                (2, 4240, 1),
                (3, 4241, 1),
                (5, 3553, 1),
                (6, 3554, 1),
                (7, 3555, 1),
                (8, 688, 3),
            ],
            &[],
        );
        assert_eq!(
            craft_list_rows(&no_ki)
                .iter()
                .filter(|r| r.label == "Arrowwood Lumber")
                .count(),
            1,
            "without KI 1986 the server cancels that recipe"
        );

        // A bag without any matching crystal earns nothing even when materials sit there.
        let no_crystal = craft_bag(&[(5, 3553, 1), (6, 3554, 1), (7, 3555, 1)], &[]);
        assert!(craft_list_rows(&no_crystal)
            .iter()
            .all(|r| r.label != "Banquet Table"));
    }

    /// The `is_dynamic` registry must list every kind whose rows come from the refresh arms;
    /// Search and Linkshell were missing it until the craft-list round (§26/§27 surfaces).
    #[test]
    fn server_state_windows_are_registered_as_dynamic() {
        assert!(is_dynamic(MenuKind::Search));
        assert!(is_dynamic(MenuKind::Linkshell));
        assert!(is_dynamic(MenuKind::CraftList));
    }

    /// The cmbmenu launcher law (vault main_menu.md §22/§28): two fixed rows in select-id order;
    /// the window keeps the Synthesis title, and the Root row drills it directly. Captions stay
    /// [I] from DATs (§22 caption law), so kuluu's labels are pinned here instead.
    #[test]
    fn synth_launcher_rows_and_title_track_the_cmbmenu_law() {
        assert_eq!(
            entries_for(MenuKind::SynthLauncher, false),
            &[SYNTH_ROW_PICK_MATERIAL, SYNTH_ROW_OPEN_LIST]
        );
        assert_eq!(menu_title(MenuKind::SynthLauncher), "Synthesis");
        assert_eq!(menu_title(MenuKind::CraftList), "Craft List");
        assert_eq!(
            root_child_kind("Synthesis", false, false),
            Some(MenuKind::SynthLauncher)
        );
    }

    /// The mcrmenu launcher law (vault main_menu.md §34): two rows in
    /// select-id order, the DAT-derived MacroPalette title (comwin token 80 =
    /// ROM/165/76.DAT catalog index 80), and the Root `Edit Macros` row
    /// drilling it directly. Row captions stay [I] (§34 chrome-only law).
    #[test]
    fn macros_palette_launcher_rows_and_title_track_the_mcrmenu_law() {
        assert_eq!(
            entries_for(MenuKind::MacrosPalette, false),
            &[MACRO_ROW_SELECT_MACRO, MACRO_ROW_MACRO_RESULT]
        );
        assert_eq!(menu_title(MenuKind::MacrosPalette), "MacroPalette");
        assert_eq!(
            root_child_kind(ROOT_MACRO_PALETTE, false, false),
            Some(MenuKind::MacrosPalette)
        );
    }

    /// The Config-family window table (vault main_menu.md §42/§47): the hub opens
    /// `configwi`; select ids 1..=12 name-open the page windows in jump-table order;
    /// the conf5m drills and key-assign windows follow. Row *content* is DAT-only —
    /// measured window rows are pinned end-to-end in ffxi-dat
    /// tests/menu_windows_current_gen.rs, never transcribed here.
    #[test]
    fn config_window_tables_follow_the_handler_jump_law() {
        assert_eq!(RETAIL_CONFIG_HUB_WINDOW, "configwi");
        assert_eq!(
            RETAIL_CONFIG_PAGE_WINDOWS,
            [
                "conf2win", "cfilter", "conftxtc", "conf5m", "conf3win", "conf6win", "fxfilter",
                "conf7", "conf4", "conf11m", "conf12wi", "conf13wi",
            ]
        );
        assert_eq!(
            RETAIL_CONFIG_SUB_WINDOWS,
            [
                "conf5win", "conf5w1", "conf5w2", "k1assign", "k2assign", "confyn", "textcol1",
                "textcol2", "textcol3", "conf11l", "conf11s",
            ]
        );
        assert_eq!(menu_title(MenuKind::RetailConfig), "Config");
        for page in 0..12u8 {
            assert_eq!(menu_title(MenuKind::RetailConfigPage(page)), "Config");
        }
        assert_eq!(
            root_child_kind("Config", false, false),
            Some(MenuKind::RetailConfig)
        );
    }

    /// Select-action law (vault main_menu.md §42/§47/§57): hub row i drills page i and
    /// the thirteenth widget (no jump-table branch) does not; `conf5m`'s widgets drill
    /// conf5w2/conf5w1 then further widgets hit the conf5win arm; every other option
    /// row carries its store-keyed option id. Built from a synthetic table — real rows
    /// come from the DAT at runtime.
    /// Retail draws a Config page by item-block y, but the handler dispatches on the raw select
    /// id — `configwi`'s thirteen rows display as ids 1,2,3,4,10,5,6,11,12,7,8,9,13 while its
    /// jump table stays in id order (vault main_menu.md §59 addendum 14). The reorder therefore
    /// moves labels only: every row keeps the page drill its own select-id earned.
    /// `fxfilter` row law (vault main_menu.md §59 addendum 14; bead kuluu-o9qt7.20.1.3): all
    /// eighteen descriptor rows (.rdata FFXiMain.dll retail-2026-09 RVA 0x379B08) toggle their own
    /// bit of the single persisted dword [`CONFIG_OPTION_FILTER_CONSOLE_FLAGS`] — class C's store
    /// method .text 0x198390 writes `[obj+0x30]` back whole, and the drawer's mark rides the bit
    /// test .text 0x193960. A fresh store marks nothing; mask `(1<<3)|(1<<17)` marks exactly rows 3
    /// and 17.
    #[test]
    fn fxfilter_rows_come_from_the_descriptor_table_and_toggle_one_bit() {
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "fxfilter".to_string(),
            FXFILTER_ROWS
                .iter()
                .map(|row| (row.caption_id, format!("fx{}", row.bit)))
                .collect(),
        );

        let rows = config_window_rows(
            "fxfilter",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(rows.len(), 18);
        for (position, row) in rows.iter().enumerate() {
            assert!(
                !row.label.starts_with('\u{bb}'),
                "fresh store marked {position}"
            );
            match row.action {
                DynamicMenuAction::ConfigOptionToggleBit { store_id, bit } => {
                    assert_eq!(store_id, CONFIG_OPTION_FILTER_CONSOLE_FLAGS);
                    assert_eq!(
                        bit, position as u8,
                        "row {position} lost its descriptor bit"
                    );
                }
                other => panic!("fxfilter row {position}: unexpected {other:?}"),
            }
        }

        let mut options = std::collections::BTreeMap::new();
        options.insert(CONFIG_OPTION_FILTER_CONSOLE_FLAGS, (1 << 3) | (1 << 17));
        let marked: Vec<usize> = config_window_rows(
            "fxfilter",
            &table,
            &options,
            None,
            &std::collections::BTreeMap::new(),
        )
        .iter()
        .enumerate()
        .filter(|(_, row)| row.label.starts_with('\u{bb}'))
        .map(|(position, _)| position)
        .collect();
        assert_eq!(marked, vec![3, 17]);
    }

    /// Gated on a retail install: `fxfilter`'s eighteen captions really come from catalog table 11 —
    /// the DAT record behind the window carries fourteen string-less blocks (§59 addendum 12), so
    /// any other source would mean kuluu had transcribed the labels itself.
    #[test]
    fn real_fxfilter_page_labels_come_from_the_caption_catalog() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut table = ConfigDatRows::default();
        table.ensure_loaded(Some(&root));
        // The synthesis path filled the string-less record from the descriptor table.
        assert_eq!(table.rows_for("fxfilter").len(), 18);
        assert_eq!(table.rows_for("fxfilter")[0].0, 153);

        let rows = config_window_rows(
            "fxfilter",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(rows.len(), 18);
        assert_eq!(rows[0].label, "Return key assignments to default settings.");
        // Bit 4 ↔ catalog id 157 (vault §59 addendum 14 row table).
        assert_eq!(rows[4].label, "Play music in stereo.");
    }

    /// `cfilter`'s full row law (vault main_menu.md §59 addendum 15): the page is exactly the
    /// descriptor table (.rdata FFXiMain.dll retail-2026-09 RVA 0x379A28, count 55 from the load
    /// method .text 0x197F88), every row selects through the shared select-method arm keyed on its
    /// raw table index (.text 0x198080), and marks ride the drawer state method .text 0x198240:
    /// pair-lane bit → glyph 1, console/interlock C bit → 2, the param-2 row with master dword set
    /// → {3,4} (.text 0x198291) even while its console bit is clear. A fresh store marks nothing.
    #[test]
    fn cfilter_rows_are_the_descriptor_table_and_mark_through_the_state_law() {
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "cfilter".to_string(),
            CFILTER_ROWS
                .iter()
                .enumerate()
                .map(|(idx, row)| (row.caption_id, format!("cf{idx}")))
                .collect(),
        );

        let rows = config_window_rows(
            "cfilter",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(rows.len(), CFILTER_ROWS.len());
        for (position, row) in rows.iter().enumerate() {
            assert!(
                !row.label.starts_with('\u{bb}'),
                "fresh store marked row {position}"
            );
            match row.action {
                DynamicMenuAction::ConfigCfilterSelect { table_idx } => {
                    // The dispatcher's raw index: scroll_base 0 + row − 1 == position (.text
                    // FFXiMain.dll retail-2026-09 RVA 0x197D00).
                    assert_eq!(
                        table_idx as usize, position,
                        "row {position} lost its table idx"
                    );
                }
                other => panic!("cfilter row {position}: unexpected {other:?}"),
            }
        }

        // Glyph codes straight from the state method (.text FFXiMain.dll retail-2026-09 RVA
        // 0x198240): group rows idx 52–54 mark at level ≥ 1/2/3; console row idx 7 (kind 0x33,
        // param 7) marks with console bit 7 as glyph 2; the param-2 row idx 1 picks its {3,4}
        // family from the master dword (.text 0x19827C) keyed on console bit 2; interlock row
        // idx 5 (kind 0x31 → LANE_B bit 19, C = console bit 9) shows L first; and plain row
        // idx 11 (kind 0x04, param 0xb) consults ONLY its lane (.text 0x1982B7 → .text 0x1982F7).
        let mut options = std::collections::BTreeMap::new();
        options.insert(CONFIG_OPTION_CFILTER_GROUP_LEVEL, 2);
        assert_eq!(cfilter_row_state(52, &options), 1);
        assert_eq!(cfilter_row_state(53, &options), 1);
        assert_eq!(cfilter_row_state(54, &options), 0);

        let mut options = std::collections::BTreeMap::new();
        options.insert(CONFIG_OPTION_CFILTER_CONSOLE, 1 << 7);
        assert_eq!(cfilter_row_state(7, &options), 2);
        // …and a plain row never marks off that same dword (.text FFXiMain.dll retail-2026-09
        // RVA 0x1982B7 jumps over the console tail entirely).
        options.insert(CONFIG_OPTION_CFILTER_CONSOLE, 1 << 4);
        assert_eq!(cfilter_row_state(11, &options), 0);
        options.insert(CONFIG_OPTION_CFILTER_LANE_A, 1 << 4);
        assert_eq!(cfilter_row_state(11, &options), 1);

        let mut options = std::collections::BTreeMap::new();
        options.insert(CONFIG_OPTION_CFILTER_MASTER, 9);
        assert_eq!(cfilter_row_state(1, &options), 3);
        options.insert(CONFIG_OPTION_CFILTER_CONSOLE, 1 << 2);
        assert_eq!(cfilter_row_state(1, &options), 4);

        let mut options = std::collections::BTreeMap::new();
        options.insert(CONFIG_OPTION_CFILTER_CONSOLE, 1 << 9);
        assert_eq!(cfilter_row_state(5, &options), 2);
        options.insert(CONFIG_OPTION_CFILTER_LANE_B, 1 << (0x31 - 0x1e));
        // The L glyph wins while the pair bit holds (.text FFXiMain.dll retail-2026-09 RVA
        // 0x1982C8–0x1982D1).
        assert_eq!(cfilter_row_state(5, &options), 1);
    }

    /// The four transition classes of class B's shared select method (.text FFXiMain.dll
    /// retail-2026-09 RVA 0x198080) as run on the five persisted dwords:
    /// group store rule `if v <= cur {v−1} else {v}` with `v = idx − 0x33` (.text 0x1980AD/0x1980B2);
    /// console toggle plus the param-2 master flip that fires only when bit 2 lands clear
    /// (.text 0x19810E–0x19812E `sete`); plain pair-lane toggle (.text 0x1981EB); and the
    /// interlock three-state cycle off → L → (L cleared, C set) → off (.text 0x198157–0x1981E8).
    #[test]
    fn cfilter_select_runs_the_shared_method_transitions() {
        let mut o = std::collections::BTreeMap::new();
        cfilter_apply_select(52, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_GROUP_LEVEL), Some(&1));
        cfilter_apply_select(52, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_GROUP_LEVEL), Some(&0));
        cfilter_apply_select(53, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_GROUP_LEVEL), Some(&2));
        cfilter_apply_select(53, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_GROUP_LEVEL), Some(&1));
        // Row idx 54's level law needs the table index (54 − 0x33 = 3): from a level-2 store,
        // select stores 3, and again drops to 2 (.text FFXiMain.dll retail-2026-09 RVA 0x1980B2).
        cfilter_apply_select(54, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_GROUP_LEVEL), Some(&3));
        cfilter_apply_select(54, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_GROUP_LEVEL), Some(&2));

        let mut o = std::collections::BTreeMap::new();
        cfilter_apply_select(7, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_CONSOLE), Some(&(1 << 7)));
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_MASTER), None);
        cfilter_apply_select(7, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_CONSOLE), Some(&0));

        // The param-2 row idx 1: setting bit 2 leaves the master alone; landing it clear flips
        // `[obj+0x40]` via sete — old non-zero → 0, old zero → 1.
        let mut o = std::collections::BTreeMap::new();
        cfilter_apply_select(1, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_CONSOLE), Some(&(1 << 2)));
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_MASTER), None);
        cfilter_apply_select(1, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_CONSOLE), Some(&0));
        // .text FFXiMain.dll retail-2026-09 RVA 0x198124: sete stores (old == 0) into `[obj+0x40]`.
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_MASTER), Some(&1));
        let mut o = std::collections::BTreeMap::new();
        o.insert(CONFIG_OPTION_CFILTER_CONSOLE, 1 << 2);
        o.insert(CONFIG_OPTION_CFILTER_MASTER, 7);
        cfilter_apply_select(1, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_CONSOLE), Some(&0));
        // …and the same sete path maps a non-zero master to 0.
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_MASTER), Some(&0));
        // A select that lands bit 2 SET never touches the master (.text FFXiMain.dll retail-2026-09
        // RVA 0x19811E `jne` straight past the flip).
        let mut o = std::collections::BTreeMap::new();
        o.insert(CONFIG_OPTION_CFILTER_MASTER, 7);
        cfilter_apply_select(1, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_CONSOLE), Some(&(1 << 2)));
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_MASTER), Some(&7));

        // Plain row idx 11 (kind 0x04): toggles lane A bit 4 only.
        let mut o = std::collections::BTreeMap::new();
        cfilter_apply_select(11, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_LANE_A), Some(&(1 << 4)));
        // Kind 0x2e (idx 10) lives in lane B at bit 0x2e − 0x1e (.text FFXiMain.dll retail-2026-09
        // RVA 0x193A14 `cmp ecx, 0x1e`).
        cfilter_apply_select(10, &mut o);
        assert_eq!(
            o.get(&CONFIG_OPTION_CFILTER_LANE_B),
            Some(&(1 << (0x2e - 0x1e)))
        );
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_CONSOLE), None);

        // Interlock row idx 5 (kind 0x31, C = console bit 9): off → L; select again clears L and
        // forces C; a third select leaves only C cleared.
        let mut o = std::collections::BTreeMap::new();
        cfilter_apply_select(5, &mut o);
        assert_eq!(
            o.get(&CONFIG_OPTION_CFILTER_LANE_B),
            Some(&(1 << (0x31 - 0x1e)))
        );
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_CONSOLE), None);
        // L was set: the machine clears it and forces C (.text FFXiMain.dll retail-2026-09 RVA
        // 0x198175 lane-write value 0, .text 0x198188 console RMW value 1).
        cfilter_apply_select(5, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_LANE_B), Some(&0));
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_CONSOLE), Some(&(1 << 9)));
        // Only C held: select clears it and leaves the lanes alone (.text FFXiMain.dll
        // retail-2026-09 RVA 0x1981BF console RMW value 0).
        cfilter_apply_select(5, &mut o);
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_CONSOLE), Some(&0));
        assert_eq!(o.get(&CONFIG_OPTION_CFILTER_LANE_B), Some(&0));
    }

    /// Gated on a retail install: `cfilter`'s row list really is the 55-entry descriptor table —
    /// the DAT record carries fourteen string-less slots (§59 addendum 12) and every caption
    /// resolves through catalog table 11, so kuluu transcribes no label itself (bead
    /// kuluu-o9qt7.20.1.3).
    #[test]
    fn real_cfilter_page_rows_are_the_descriptor_table() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut table = ConfigDatRows::default();
        table.ensure_loaded(Some(&root));
        // The load method's own count: `mov eax, 0x37` (.text FFXiMain.dll retail-2026-09 RVA
        // 0x197F88).
        assert_eq!(table.rows_for("cfilter").len(), CFILTER_ROWS.len());
        for (idx, row) in table.rows_for("cfilter").iter().enumerate() {
            assert_eq!(
                row.0, CFILTER_ROWS[idx].caption_id,
                "cfilter row {idx} lost its catalog id"
            );
        }
        let rows = config_window_rows(
            "cfilter",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(rows.len(), CFILTER_ROWS.len());
    }

    #[test]
    fn config_page_rows_display_by_record_y_but_keep_their_select_id_arms() {
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "configwi".to_string(),
            (0..13u32)
                .map(|i| (226 + i, format!("hub{}", i + 1)))
                .collect(),
        );
        // The measured y column of configwi's item blocks, in file (= select-id) order.
        table.row_ys.insert(
            "configwi".to_string(),
            vec![6, 22, 38, 54, 86, 102, 150, 166, 182, 70, 118, 134, 198],
        );

        let hub = config_window_rows(
            RETAIL_CONFIG_HUB_WINDOW,
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(hub.len(), 13);
        let labels: Vec<&str> = hub.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "hub1", "hub2", "hub3", "hub4", "hub10", "hub5", "hub6", "hub11", "hub12", "hub7",
                "hub8", "hub9", "hub13",
            ],
        );
        // Display row 5 is item id 10 → jump-table branch 10 = conf11m, page index 9.
        assert!(matches!(
            hub[4].action,
            DynamicMenuAction::OpenRetailConfigPage { page: 9 }
        ));
        // …and every drawn row still carries the page of its OWN select id (the list is what
        // moves, never the arm): display slot → jump-table branch it came from.
        let pages_by_display_row: [Option<u8>; 13] = [
            Some(0),
            Some(1),
            Some(2),
            Some(3),
            Some(9),
            Some(4),
            Some(5),
            Some(10),
            Some(11),
            Some(6),
            Some(7),
            Some(8),
            None,
        ];
        for (position, row) in hub.iter().enumerate() {
            match (pages_by_display_row[position], &row.action) {
                (Some(page), DynamicMenuAction::OpenRetailConfigPage { page: got }) => assert_eq!(
                    *got, page,
                    "display row {position} ({}) lost its arm",
                    labels[position]
                ),
                (None, DynamicMenuAction::ConfigOptionRow { .. }) => {}
                (expected, action) => {
                    panic!(
                        "hub display row {position} ({}): expected {expected:?}, got {action:?}",
                        labels[position]
                    )
                }
            }
        }
    }

    /// Gated on a retail install: the hub's draw order really is the record y column, and the
    /// row that lands in display slot 5 opens `conf11m` because its own item id is 10.
    #[test]
    fn real_config_hub_rows_draw_in_record_y_order_with_their_own_page_drills() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut table = ConfigDatRows::default();
        table.ensure_loaded(Some(&root));
        assert_eq!(table.rows_for(RETAIL_CONFIG_HUB_WINDOW).len(), 13);
        assert_eq!(
            table.row_ys.get(RETAIL_CONFIG_HUB_WINDOW).map(Vec::len),
            Some(13)
        );

        let hub = config_window_rows(
            RETAIL_CONFIG_HUB_WINDOW,
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        assert!(
            hub[4].label.starts_with("Designate which messages appear"),
            "display row 5: {:?}",
            hub[4].label
        );
        assert!(matches!(
            hub[4].action,
            DynamicMenuAction::OpenRetailConfigPage { page: 9 }
        ));
        // conf11m is jump-table branch 10; the row that opens it sits in display slot 5.
        assert_eq!(RETAIL_CONFIG_PAGE_WINDOWS[9], "conf11m");
    }

    #[test]
    fn config_window_rows_assign_handler_ground_actions() {
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "configwi".to_string(),
            (0..13u32).map(|i| (200 + i, format!("hub{i}"))).collect(),
        );
        table.by_root.insert(
            "conf5m".to_string(),
            vec![
                (894, "Adjust shared window settings.".to_string()),
                (895, "Adjust individual window settings.".to_string()),
                (895, "Adjust individual window settings.".to_string()),
            ],
        );

        let hub = config_window_rows(
            RETAIL_CONFIG_HUB_WINDOW,
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(hub.len(), 13);
        for (position, row) in hub.iter().enumerate() {
            if position < RETAIL_CONFIG_PAGE_WINDOWS.len() {
                assert!(
                    matches!(row.action, DynamicMenuAction::OpenRetailConfigPage { page } if page as usize == position),
                    "hub row {position}: {:?}",
                    row.action
                );
            } else {
                // The GAMEPAD-Config widget: the jump table has no branch for
                // it (vault main_menu.md §42/§57).
                assert!(matches!(
                    row.action,
                    DynamicMenuAction::ConfigOptionRow { .. }
                ));
            }
        }

        let launcher = config_window_rows(
            "conf5m",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        let subs: Vec<u8> = launcher
            .iter()
            .map(|row| match row.action {
                DynamicMenuAction::OpenRetailConfigSub { sub } => sub,
                other => panic!("launcher row lost its drill: {other:?}"),
            })
            .collect();
        // The handler's `dec/je` chain arms id 1 → conf5win (sub 0, the shared-settings page),
        // id 2 → conf5w1 and id 3 → conf5w2 (.text FFXiMain.dll retail-2026-09 RVA 0x19924D..).
        assert_eq!(subs, vec![0u8, 1, 2]);

        let page = config_window_rows(
            "conf7",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        assert!(page.is_empty(), "unloaded window lists nothing");
    }

    /// The `conf5win` rows carry their jump-table (store id, value) pair and
    /// only the row matching the store's current value wears the marker
    /// (bead kuluu-o9qt7.20.1).
    #[test]
    fn conf5win_radio_table_marks_the_current_choice() {
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "conf5win".to_string(),
            (0..16u32).map(|i| (900 + i, format!("opt{i}"))).collect(),
        );
        let mut values = std::collections::BTreeMap::new();
        values.insert(0xB4u32, 2i32);
        let rows = config_window_rows(
            "conf5win",
            &table,
            &values,
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(rows.len(), CONF5WIN_RADIO_ARMS.len());
        for (position, row) in rows.iter().enumerate() {
            match row.action {
                DynamicMenuAction::ConfigOptionSet { store_id, value } => {
                    let (want_store, want_value) = CONF5WIN_RADIO_ARMS[position];
                    assert_eq!((store_id, value), (want_store, want_value));
                }
                other => panic!("radio arm lost to {other:?} at position {position}"),
            }
        }
        // Arms 0..3 are (0xB4,1) (0xB4,2) — the marker sits on row 1 alone.
        assert_eq!(rows[1].label, "\u{bb} opt1");
        assert!(!rows[0].label.starts_with('\u{bb}'));
    }

    /// Both individual log-window editors (`conf5w1`/`conf5w2`) carry the same six
    /// rows their DAT records share, armed by the shared select handler (.text
    /// FFXiMain.dll retail-2026-09 RVA 0x199850): the auto-size radio pair (values 0/1,
    /// pushes at .text 0x1998AA/0x1998BB) then four stepper rows — max lines, min
    /// lines, column width, interval — each labelled with its boot-or-stored value.
    /// Pins the per-store floor/ceiling/boot/step law and the cross-clamp pair map
    /// (vault main_menu.md §59 addendum 46).
    #[test]
    fn log_window_editor_rows_follow_their_shared_handler_arms() {
        for (window, auto_size, max_lines, min_lines, width, interval) in [
            (
                "conf5w1",
                CONFIG_STORE_LOG_WINDOW_ONE_AUTO_SIZE,
                CONFIG_STORE_LOG_WINDOW_ONE_MAX_LINES,
                CONFIG_STORE_LOG_WINDOW_ONE_MIN_LINES,
                CONFIG_STORE_LOG_WINDOW_ONE_WIDTH,
                CONFIG_STORE_LOG_WINDOW_ONE_INTERVAL,
            ),
            (
                "conf5w2",
                CONFIG_STORE_LOG_WINDOW_TWO_AUTO_SIZE,
                CONFIG_STORE_LOG_WINDOW_TWO_MAX_LINES,
                CONFIG_STORE_LOG_WINDOW_TWO_MIN_LINES,
                CONFIG_STORE_LOG_WINDOW_TWO_WIDTH,
                CONFIG_STORE_LOG_WINDOW_TWO_INTERVAL,
            ),
        ] {
            let mut table = ConfigDatRows {
                loaded: true,
                ..Default::default()
            };
            table.by_root.insert(
                window.to_string(),
                (0..6u32)
                    .map(|i| (700 + i, format!("{window}row{i}")))
                    .collect(),
            );
            let rows = config_window_rows(
                window,
                &table,
                &std::collections::BTreeMap::new(),
                None,
                &std::collections::BTreeMap::new(),
            );
            assert_eq!(rows.len(), 6, "{window} rows");
            assert!(
                matches!(rows[0].action, DynamicMenuAction::ConfigOptionSet { store_id: s, value: 0 } if s == auto_size),
                "{window} radio row 0: {:?}",
                rows[0].action
            );
            assert!(
                matches!(rows[1].action, DynamicMenuAction::ConfigOptionSet { store_id: s, value: 1 } if s == auto_size),
                "{window} radio row 1: {:?}",
                rows[1].action
            );
            for (position, store) in [(2, max_lines), (3, min_lines), (4, width), (5, interval)] {
                assert!(
                    matches!(rows[position].action, DynamicMenuAction::ConfigSliderEdit { store_id: s } if s == store),
                    "{window} stepper row {position}: {:?}",
                    rows[position].action
                );
            }
            // Boot values ride the labels: max 8 (field +0x3c), min 0 (+0x40), width
            // 100 (+0x28) and interval 8 (raw 5 at +0x24 shown as raw+3).
            assert_eq!(rows[2].label, format!("{window}row2: 8"));
            assert_eq!(rows[3].label, format!("{window}row3: 0"));
            assert_eq!(rows[4].label, format!("{window}row4: 100"));
            assert_eq!(rows[5].label, format!("{window}row5: 8"));

            let mut values = std::collections::BTreeMap::new();
            values.insert(auto_size, 1i32);
            let rows = config_window_rows(
                window,
                &table,
                &values,
                None,
                &std::collections::BTreeMap::new(),
            );
            assert_eq!(rows[1].label, format!("\u{bb} {window}row1"));

            // Per-store slider law.
            for store in [max_lines, min_lines] {
                assert_eq!(config_slider_max(store), 20);
            }
            assert_eq!(config_slider_min(max_lines), 4);
            assert_eq!(config_slider_boot_default(max_lines), 8);
            assert_eq!(config_slider_min(min_lines), 0);
            assert_eq!(config_slider_boot_default(min_lines), 0);
            assert_eq!(config_slider_step(width), 10);
            assert_eq!(config_slider_boot_default(width), 100);
            assert_eq!(config_slider_max(width), CONFIG_SLIDER_MAX);
            assert_eq!(config_slider_min(interval), 3);
            assert_eq!(config_slider_max(interval), 30);
            assert_eq!(config_slider_step(interval), 1);
            assert_eq!(
                config_log_lines_pair(max_lines),
                Some((max_lines, min_lines))
            );
            assert_eq!(
                config_log_lines_pair(min_lines),
                Some((max_lines, min_lines))
            );
            assert!(config_log_lines_pair(width).is_none());
            assert!(config_log_lines_pair(interval).is_none());
        }
    }

    /// Gated on a retail install: `conf5win`'s widget count is exactly the
    /// law'd radio-table length, so no row can silently miss its arm.
    #[test]
    fn real_conf5win_widgets_match_the_radio_table() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut table = ConfigDatRows::default();
        table.ensure_loaded(Some(&root));
        let rows = config_window_rows(
            "conf5win",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(rows.len(), CONF5WIN_RADIO_ARMS.len());
    }

    /// The `conf2win` row law (bead kuluu-o9qt7.20.1 part B): radio rows write
    /// through immediately, slider rows pin into edit; slider labels carry the
    /// store's value — boot default from §58 until one is written — and a
    /// pinned row wears the marker.
    #[test]
    fn conf2win_rows_carry_the_radio_and_slider_law() {
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "conf2win".to_string(),
            (0..11u32).map(|i| (700 + i, format!("row{i}"))).collect(),
        );
        let rows = config_window_rows(
            "conf2win",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(rows.len(), CONF2WIN_ROW_ARMS.len());
        // Rows 1-2: radio pair store 7 (0/1). Slider stores keep their §58 boot
        // defaults; the pin marks its row and shows the live value.
        assert!(matches!(
            rows[0].action,
            DynamicMenuAction::ConfigOptionSet {
                store_id: 7,
                value: 0
            }
        ));
        assert!(matches!(
            rows[1].action,
            DynamicMenuAction::ConfigOptionSet {
                store_id: 7,
                value: 1
            }
        ));
        assert_eq!(rows[2].label, "row2: 100", "store 9 boots at its max");
        assert_eq!(rows[3].label, "row3: 100", "store 0xA boots at its max");
        assert_eq!(rows[4].label, "row4: 50", "stores 0x37.. start half-scale");
        for (position, store_id) in [(2u8, 9u32), (3, 0xA), (4, 0x37), (5, 0x38), (6, 0x39)] {
            assert!(
                matches!(rows[usize::from(position)].action,
                    DynamicMenuAction::ConfigSliderEdit { store_id: s } if s == store_id),
                "slider row {position}: {:?}",
                rows[usize::from(position)].action
            );
        }
        assert!(matches!(
            rows[7].action,
            DynamicMenuAction::ConfigOptionSet {
                store_id: 0x9A,
                value: 1,
                ..
            }
        ));
        assert!(matches!(
            rows[10].action,
            DynamicMenuAction::ConfigOptionSet {
                store_id: 0xCA,
                value: 1
            }
        ));
        let mut values = std::collections::BTreeMap::new();
        values.insert(9u32, 42i32);
        let rows = config_window_rows(
            "conf2win",
            &table,
            &values,
            Some((0x37u32, 51u8)),
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(
            rows[2].label, "row2: 42",
            "a written value replaces the boot default"
        );
        assert_eq!(
            rows[4].label, "\u{bb} row4: 51",
            "the pinned slider is marked live"
        );
        // A radio group member matching the store's current value wears the marker.
        values.insert(0xCAu32, 1i32);
        let rows = config_window_rows(
            "conf2win",
            &table,
            &values,
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(rows[10].label, "\u{bb} row10");
        assert!(!rows[9].label.starts_with('\u{bb}'));
    }

    /// `conf13wi` and `conf12wi` row laws (bead kuluu-o9qt7.20.1): triples and
    /// pairs land on their resolved stores; bank-2 rows mark from the second
    /// map, primary rows from the first.
    #[test]
    fn conf13wi_and_conf12wi_rows_carry_their_resolved_arms() {
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "conf13wi".to_string(),
            (0..6u32).map(|i| (600 + i, format!("t{i}"))).collect(),
        );
        let rows = config_window_rows(
            "conf13wi",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(rows.len(), 6);
        let want = [
            (0usize, 0xC9u32, 0i32),
            (1, 0xC9, 1),
            (2, 0xC9, 2),
            (3, 0xCE, 1),
            (4, 0xCE, 2),
            (5, 0xCE, 0),
        ];
        for (position, store_id, value) in want {
            assert!(
                matches!(rows[position].action,
                    DynamicMenuAction::ConfigOptionSet { store_id: s, value: v }
                        if s == store_id && v == value),
                "conf13wi row {position}: {:?}",
                rows[position].action
            );
        }
        table.by_root.insert(
            "conf12wi".to_string(),
            (0..14u32).map(|i| (800 + i, format!("p{i}"))).collect(),
        );
        let mut values = std::collections::BTreeMap::new();
        values.insert(0xC0u32, 1i32); // the "on" member of the C0 pair is select id 5
                                      // The two bit pairs share one dword (option CONFIG_OPTION_MISC_FLAGS), so a
                                      // mask with bit 6 set marks row 14 and leaves row 13 unmarked (vault
                                      // main_menu.md §59 addendum 5).
        values.insert(CONFIG_OPTION_MISC_FLAGS, 1 << 6);
        let rows = config_window_rows(
            "conf12wi",
            &table,
            &values,
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(rows.len(), CONF12WI_RADIO_ARMS.len());
        assert_eq!(
            rows[4].label, "\u{bb} p4",
            "primary store mark: C0 on-member is index 4"
        );
        assert_eq!(
            rows[13].label, "\u{bb} p13",
            "the bit-6 on-member marks from the shared flag mask"
        );
        assert!(
            !rows[12].label.starts_with('\u{bb}'),
            "the matching off-member stays unmarked"
        );
        for position in [6usize, 7, 12, 13] {
            let bit = if position < 8 { 7u8 } else { 6 };
            assert!(
                matches!(rows[position].action,
                    DynamicMenuAction::ConfigOptionBitSet { bit: b, .. } if b == bit),
                "conf12wi row {position} lost its bit pair arm: {:?}",
                rows[position].action
            );
        }
    }

    /// Rows 3/4 of `conf12wi` are the store-0xBF pair with the polarity the shared
    /// arm computes at .text FFXiMain.dll retail-2026-09 RVA 0x19BA0E (`cmp ax,4;
    /// sete dl`): select id 3 writes 0, select id 4 writes 1 (bead
    /// kuluu-o9qt7.20.1.4; vault main_menu.md §59 addendum 16).
    #[test]
    fn conf12wi_rows_3_and_4_are_the_bf_store_pair() {
        assert!(matches!(
            CONF12WI_RADIO_ARMS[2],
            ConfigRowArm::Radio {
                store_id: CONFIG_OPTION_CONF_BF_PAIR,
                value: 0
            }
        ));
        assert!(matches!(
            CONF12WI_RADIO_ARMS[3],
            ConfigRowArm::Radio {
                store_id: CONFIG_OPTION_CONF_BF_PAIR,
                value: 1
            }
        ));
    }

    /// The announce-mode law helper FFXiMain.dll retail-2026-09 RVA 0x1A8030 is
    /// handed at .text 0x19BA32–0x19BA5D: the off row announces 0 whatever was
    /// stored, writing 1 over a stored 1 reasserts with 1, and enabling over any
    /// other value (absent or not-1) announces 4.
    #[test]
    fn conf_bf_packet_modes_follow_the_written_and_stored_values() {
        assert_eq!(conf_bf_packet_mode(0, None), CONF_BF_MODE_DISABLE);
        assert_eq!(conf_bf_packet_mode(0, Some(1)), CONF_BF_MODE_DISABLE);
        assert_eq!(
            conf_bf_packet_mode(1, Some(1)),
            CONF_BF_MODE_KEEP_ON,
            "stored 1 keeps reasserting — .text 0x19BA3C `cmp edi,1` takes the je"
        );
        assert_eq!(conf_bf_packet_mode(1, None), CONF_BF_MODE_ENABLE);
        assert_eq!(conf_bf_packet_mode(1, Some(0)), CONF_BF_MODE_ENABLE);
        assert_eq!(conf_bf_packet_mode(1, Some(2)), CONF_BF_MODE_ENABLE);
    }

    /// The guard FFXiMain.dll retail-2026-09 RVA 0x1A7D05 passes only strictly past
    /// its 0x2328 ms window (`cmp eax, 0x2328; seta`), and an unstamped client
    /// (retail's zeroed stamp global) always sends.
    #[test]
    fn conf_bf_announce_gate_is_strictly_past_the_window() {
        assert!(conf_bf_announce_allowed(None));
        assert!(!conf_bf_announce_allowed(Some(0)));
        assert!(!conf_bf_announce_allowed(Some(CONF_BF_ANNOUNCE_WINDOW_MS)));
        assert!(conf_bf_announce_allowed(Some(
            CONF_BF_ANNOUNCE_WINDOW_MS + 1
        )));
    }

    /// `conf3win`/`conf6win` arm tables render their law'd rows (bead
    /// kuluu-o9qt7.20.1): radios/pairs/triples carry resolved stores, slider rows
    /// pin with per-store boot values and ceilings, and undug positions keep the
    /// option-record fallback.
    #[test]
    fn conf3win_and_conf6win_rows_answer_their_arm_tables() {
        let mark = "\u{bb}";
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        for w in ["conf3win", "conf6win"] {
            table.by_root.insert(
                w.to_string(),
                (0..15u32).map(|i| (400 + i, format!("{w}{i}"))).collect(),
            );
        }
        let r3 = config_window_rows(
            "conf3win",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(r3.len(), 15);
        // Ids 3/4 are the window-local pair (arms .text FFXiMain.dll retail-2026-09
        // RVA 0x19aa85/0x19aa92): unmarked while no choice is stored — retail boots
        // `[obj+0x38]` to −1 (.text 0x19a8b8) so neither row starts marked (vault
        // main_menu.md §59 addendum 9).
        assert!(!r3[2].label.starts_with('\u{bb}'));
        assert!(!r3[3].label.starts_with('\u{bb}'));
        assert!(matches!(
            r3[2].action,
            DynamicMenuAction::ConfigWindowChoiceSet {
                window_key: 4,
                obj_off: 0x38,
                value: 0
            }
        ));
        let mut choices = std::collections::BTreeMap::new();
        choices.insert((4u8, 0x38u16), 1i32);
        let r3 = config_window_rows(
            "conf3win",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &choices,
        );
        assert_eq!(r3[2].label, "conf3win2");
        assert_eq!(r3[3].label, "\u{bb} conf3win3");
        for (position, store_id) in [(0usize, 0x13u32), (4, 0x13)] {
            let want = if position == 4 { 2 } else { 0 };
            assert!(
                matches!(r3[position].action, DynamicMenuAction::ConfigOptionSet { store_id: s, value: v } if s == store_id && v == want),
                "conf3win radio {position}: {:?}",
                r3[position].action
            );
        }
        for (position, value) in [(5usize, 2i32), (6, 0), (7, 1)] {
            assert!(
                matches!(r3[position].action, DynamicMenuAction::ConfigOptionSet { store_id: 0x3A, value: v } if v == value)
            );
        }
        for (position, value) in [(8usize, 1i32), (9, 0)] {
            assert!(
                matches!(r3[position].action, DynamicMenuAction::ConfigOptionSet { store_id: 0x3B, value: v } if v == value)
            );
        }
        assert!(matches!(
            r3[11].action,
            DynamicMenuAction::ConfigOptionBitSet { bit: 2, value: 1 }
        ));
        assert!(matches!(
            r3[12].action,
            DynamicMenuAction::ConfigOptionBitSet { bit: 2, value: 0 }
        ));
        assert!(matches!(
            r3[14].action,
            DynamicMenuAction::ConfigOptionSet {
                store_id: 0xA5,
                value: 1
            }
        ));
        // Ids 3/4 answer their arms (.text FFXiMain.dll retail-2026-09 RVA
        // 0x19aa85/0x19aa92) as the window-local pair, not a fallback row.
        for (position, value) in [(2usize, 0i32), (3, 1)] {
            assert!(
                matches!(
                    r3[position].action,
                    DynamicMenuAction::ConfigWindowChoiceSet { window_key: 4, obj_off: 0x38, value: v } if v == value
                ),
                "conf3win window-local row {position}"
            );
        }
        // Row 11 is the slider entry arm on store 0x3C (boot/ceiling 50).
        assert!(matches!(
            r3[10].action,
            DynamicMenuAction::ConfigSliderEdit { store_id: 0x3C }
        ));
        assert_eq!(r3[10].label, "conf3win10: 50");

        let r6 = config_window_rows(
            "conf6win",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(r6.len(), 15);
        for (position, store_id) in [(0usize, 0x3Eu32), (3, 0x40), (6, 0x43)] {
            assert!(
                matches!(r6[position].action, DynamicMenuAction::ConfigSliderEdit { store_id: s } if s == store_id),
                "conf6win slider row {position}: {:?}",
                r6[position].action
            );
        }
        assert_eq!(r6[0].label, "conf6win0: 25");
        assert_eq!(r6[3].label, "conf6win3: 3");
        for (position, value) in [(4usize, 0i32), (5, 1), (10, 2)] {
            assert!(
                matches!(r6[position].action, DynamicMenuAction::ConfigOptionSet { store_id: 0x42, value: v } if v == value)
            );
        }
        // Ids 8/9 (index 7/8) drill k1assign/k2assign; id10 arms the confirm
        // dialog (.text FFXiMain.dll retail-2026-09 RVA 0x19B309), which raises
        // the reset prompt instead of writing anything.
        assert!(matches!(
            r6[7].action,
            DynamicMenuAction::OpenRetailConfigSub { sub: 3 }
        ));
        assert!(matches!(
            r6[8].action,
            DynamicMenuAction::OpenRetailConfigSub { sub: 4 }
        ));
        assert_eq!(r6[9].label, "conf6win9"); // no store mark on a dialog row
        assert!(matches!(
            r6[9].action,
            DynamicMenuAction::OpenKeyAssignResetPrompt
        ));
        // Store 0x9D writes through the primary wrapper, so its active-row mark
        // comes straight from `option_values` (handler FFXiMain.dll retail-2026-09
        // RVA 0x19B358) rather than a bit of the flag mask.
        let mut primary = std::collections::BTreeMap::new();
        primary.insert(0x9Du32, 1i32);
        let r6 = config_window_rows(
            "conf6win",
            &table,
            &primary,
            None,
            &std::collections::BTreeMap::new(),
        );
        assert!(
            r6[12].label.starts_with(mark),
            "primary mark on store 0x9D row: {}",
            r6[12].label
        );
        // The window state keys are kuluu coordinates: pages by their index in
        // RETAIL_CONFIG_PAGE_WINDOWS, subs at 32+index — and the hub itself has no
        // key because it carries no WindowLocal rows.
        assert_eq!(config_window_state_key("conf3win"), Some(4));
        assert_eq!(config_window_state_key("k1assign"), Some(35));
        assert_eq!(config_window_state_key(RETAIL_CONFIG_HUB_WINDOW), None);
    }

    /// `conf4`/`conf7` rows answer their arm tables (bead kuluu-o9qt7.20.1):
    /// conf4 keeps the 0x18 pair plus store 0x41 levels {10..60} and a −1 off
    /// row; conf7's fourteen positions are the seven radio pairs read from the
    /// handler's .data pair table (vault main_menu.md §59 addendum 3).
    #[test]
    fn conf4_and_conf7_rows_answer_their_arm_tables() {
        let mark = "\u{bb}";
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        for (w, n) in [("conf4", 9usize), ("conf7", 14)] {
            table.by_root.insert(
                w.to_string(),
                (0..n as u32)
                    .map(|i| (500 + i, format!("{w}{i}")))
                    .collect(),
            );
        }
        let empty = std::collections::BTreeMap::new();
        let r4 = config_window_rows(
            "conf4",
            &table,
            &empty,
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(r4.len(), 9);
        for (position, value) in [(0usize, 0i32), (1, 1)] {
            assert!(
                matches!(r4[position].action, DynamicMenuAction::ConfigOptionSet { store_id: 0x18, value: v } if v == value),
                "conf4 row {position}: {:?}",
                r4[position].action
            );
        }
        for (position, value) in [
            (2usize, 10i32),
            (3, 20),
            (4, 30),
            (5, 40),
            (6, 50),
            (7, 60),
            (8, -1),
        ] {
            assert!(
                matches!(r4[position].action, DynamicMenuAction::ConfigOptionSet { store_id: 0x41, value: v } if v == value),
                "conf4 level row {position}: {:?}",
                r4[position].action
            );
        }
        let mut values = std::collections::BTreeMap::new();
        values.insert(0x41u32, -1i32);
        let r4 = config_window_rows(
            "conf4",
            &table,
            &values,
            None,
            &std::collections::BTreeMap::new(),
        );
        assert!(
            r4[8].label.starts_with(mark),
            "off row must mark at store 0x41 = −1: {}",
            r4[8].label
        );
        let marked_levels: Vec<usize> = r4
            .iter()
            .enumerate()
            .filter(|(index, row)| (2..8).contains(index) && row.label.starts_with(mark))
            .map(|(index, _)| index)
            .collect();
        assert!(
            marked_levels.is_empty(),
            "level rows must not mark at store 0x41 = −1: {marked_levels:?}"
        );
        let r7 = config_window_rows(
            "conf7",
            &table,
            &empty,
            None,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(r7.len(), 14);
        for (position, (store_id, value)) in [
            (0usize, (0x90u32, 0i32)),
            (1, (0x90, 1)),
            (2, (0x8F, 0)),
            (3, (0x8F, 1)),
            (4, (0x91, 0)),
            (5, (0x91, 1)),
            (6, (0x8C, 0)),
            (7, (0x8C, 1)),
            (8, (0x8B, 0)),
            (9, (0x8B, 1)),
            (10, (0x8E, 0)),
            (11, (0x8E, 1)),
            (12, (0x8D, 0)),
            (13, (0x8D, 1)),
        ] {
            assert!(
                matches!(r7[position].action, DynamicMenuAction::ConfigOptionSet { store_id: s, value: v } if s == store_id && v == value),
                "conf7 row {position}: {:?}",
                r7[position].action
            );
        }
    }

    /// Gated on a retail install: every law'd config page's DAT item count must
    /// equal the arm table length - each select id inside the handler guard maps
    /// to an arm (conf2win 11, conf4 9, conf7 14, conf13wi 6, conf12wi 14; bead
    /// kuluu-o9qt7.20.1).
    #[test]
    fn real_conf_page_row_counts_match_their_arm_tables() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut table = ConfigDatRows::default();
        table.ensure_loaded(Some(&root));
        for (window, want) in [
            ("conf2win", CONF2WIN_ROW_ARMS.len()),
            ("conf3win", CONF3WIN_ROW_ARMS.len()),
            ("conf6win", CONF6WIN_ROW_ARMS.len()),
            ("conf4", CONF4_ROW_ARMS.len()),
            ("conf7", CONF7_ROW_ARMS.len()),
            ("conf13wi", CONF13WI_RADIO_ARMS.len()),
            ("conf12wi", CONF12WI_RADIO_ARMS.len()),
        ] {
            let n = table.rows_for(window).len();
            assert_eq!(n, want, "{window} DAT rows vs arm table");
        }
    }

    /// Gated on a retail install: the cascade pages' captioned item counts pin
    /// their handlers' select-id spaces — conftxtc ids 1–5 (handler FFXiMain.dll
    /// retail-2026-09 RVA 0x1984F0, guard `dec; cmp eax,4`), conf11m ids 1–3
    /// (its handler's dec chain at that RVA 0x19A0E0) and conf11l ids 1–4
    /// (handler FFXiMain.dll retail-2026-09 RVA 0x19A160, guard `dec; cmp eax,3`);
    /// chrome item blocks never join the select space. DAT-first: counts are read
    /// from the install here, not transcribed (bead kuluu-o9qt7.20.1).
    #[test]
    fn real_cascade_pages_match_their_handler_id_spaces() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut table = ConfigDatRows::default();
        table.ensure_loaded(Some(&root));
        for (window, want) in [("conftxtc", 5usize), ("conf11m", 3), ("conf11l", 4)] {
            assert_eq!(
                table.rows_for(window).len(),
                want,
                "{window} DAT live items"
            );
            let arms = config_page_arms(window).unwrap_or_default();
            assert_eq!(
                arms.len(),
                want,
                "{window} handler select ids must cover every DAT row"
            );
        }
    }

    /// The companion-drill law (vault main_menu.md §59 addendum 5): the Text
    /// Color rows open `textcol2` then `textcol1` (focused) carrying their own
    /// index as the mode dword; row 5 opens the Yes/No panel. `conf11m` drills
    /// `conf11l`, whose rows in turn drill `conf11s`.
    #[test]
    fn companion_drills_open_the_windows_of_their_handler_arms() {
        let mut table = ConfigDatRows {
            loaded: true,
            ..ConfigDatRows::default()
        };
        for (window, n) in [("conftxtc", 5), ("conf11m", 3), ("conf11l", 4)] {
            table.by_root.insert(
                window.to_string(),
                (0..n)
                    .map(|i| (900 + i as u32, format!("{window}{i}")))
                    .collect(),
            );
        }
        let empty = std::collections::BTreeMap::new();
        let rows = config_window_rows(
            "conftxtc",
            &table,
            &empty,
            None,
            &std::collections::BTreeMap::new(),
        );
        for mode in 0..4i32 {
            assert!(
                matches!(
                    &rows[mode as usize].action,
                    DynamicMenuAction::OpenRetailConfigCompanion { mode: m, opens }
                        if *m == mode && **opens == [CONFIG_SUB_TEXTCOL2, CONFIG_SUB_TEXTCOL1]
                ),
                "conftxtc row {mode} companion drill"
            );
        }
        assert!(
            matches!(
                &rows[4].action,
                DynamicMenuAction::OpenRetailConfigCompanion { mode: 0, opens }
                    if **opens == [CONFIG_SUB_CONFYN]
            ),
            "conftxtc row 5 opens the Yes/No panel"
        );

        let rows = config_window_rows(
            "conf11m",
            &table,
            &empty,
            None,
            &std::collections::BTreeMap::new(),
        );
        for (position, mode) in [(0usize, 0i32), (1, 1)] {
            assert!(
                matches!(
                    &rows[position].action,
                    DynamicMenuAction::OpenRetailConfigCompanion { mode: m, opens }
                        if *m == mode && **opens == [CONFIG_SUB_CONF11L]
                ),
                "conf11m row {position} drills conf11l with mode {mode}"
            );
        }

        let rows = config_window_rows(
            "conf11l",
            &table,
            &empty,
            None,
            &std::collections::BTreeMap::new(),
        );
        assert!(rows.iter().enumerate().all(|(position, row)| matches!(
            &row.action,
            DynamicMenuAction::OpenRetailConfigCompanion { mode, opens }
                if *mode == position as i32 && **opens == [CONFIG_SUB_CONF11S]
        )));
    }

    /// The faqsub panel law (vault main_menu.md §36): three destinations
    /// labelled from the window's own caption-builder strings (table 9 ids
    /// 0xC3/0xCF/0xDD), title `Help Desk` (ROM/165/76.DAT idx 60), and the
    /// Root Help Desk row drills it.
    #[test]
    fn helpdesk_panel_rows_and_title_track_the_faqsub_law() {
        assert_eq!(
            entries_for(MenuKind::HelpDeskPanel, false),
            &["Q&A", "Mentor Search", "Config"]
        );
        assert_eq!(menu_title(MenuKind::HelpDeskPanel), "Help Desk");
        assert_eq!(
            root_child_kind("Help Desk", false, false),
            Some(MenuKind::HelpDeskPanel)
        );
    }

    /// The Equipment-row variant gate order, fresh from FFXiMain.dll
    /// retail-2026-09 RVA 0x1404D5 (vault main_menu.md §39): Monstrosity beats
    /// puppetry, puppetry beats the ordinary equip window; data presence on the
    /// folded snapshot stands in for both client flags.
    #[test]
    fn equipment_row_variant_gate_order_follows_the_dll_branch_law() {
        let plain = kuluu_snapshot::SceneSnapshot::default();
        assert_eq!(equipment_drill_kind(&plain), MenuKind::Equipment);

        let automaton = kuluu_snapshot::AutomatonInfo {
            is_sub_job: false,
            head: 0,
            frame: 0,
            attachments: [0; 12],
            unlocked_heads: 0x3F,
            unlocked_frames: 0x0F,
        };
        let pup = kuluu_snapshot::SceneSnapshot {
            automaton: Some(Box::new(automaton.clone())),
            ..Default::default()
        };
        assert_eq!(equipment_drill_kind(&pup), MenuKind::AutomatonEquip);

        // Both records present — the DLL tests the Monstrosity predicate first
        // (call 0x97D50 before the flag/job compares), so mp wins.
        let both = kuluu_snapshot::SceneSnapshot {
            automaton: Some(Box::new(automaton)),
            monstrosity: Some(kuluu_snapshot::MonstrosityInfo {
                species: 1,
                equipped_instincts: [0; 12],
            }),
            ..Default::default()
        };
        assert_eq!(equipment_drill_kind(&both), MenuKind::MpCustomize);
    }

    /// The Map-row variant gate, fresh from FFXiMain.dll retail-2026-09 RVA
    /// 0x140653 (vault main_menu.md §40): the myroom name-open only when the
    /// server holds the player in a Mog House; data presence stands in.
    #[test]
    fn map_row_variant_gate_follows_mog_house_presence() {
        let plain = kuluu_snapshot::SceneSnapshot::default();
        assert_eq!(map_drill_kind(&plain), MenuKind::Map);

        let home = kuluu_snapshot::SceneSnapshot {
            myroom: Some(kuluu_snapshot::MyRoom {
                model: 615,
                sub_map: 1,
                door_open: false,
            }),
            ..Default::default()
        };
        assert_eq!(map_drill_kind(&home), MenuKind::HomeView);
    }

    /// The myroom panel rows and inherited title (DAT 'myro' caption tokens
    /// against ROM/165/76.DAT; vault main_menu.md §40 as corrected by §53 —
    /// fourteen named widgets, display order pinned twice over by the geometry
    /// y column 0x15..0xE5 and the records' prev/next chain ring).
    /// The door row sits at `HOME_VIEW_ROWS` index 5 while the handler answers it as select id 6 —
    /// rows are never ids (FFXiMain.dll retail-2026-09 `.text 0x145460` switch). Its two DAT
    /// captions pair shut=index 0 / open=index 1, exactly as the handler fetches them at `.text
    /// 0x145439` (shut) and `.text 0x145357` (open).
    /// The `abisortw`/emote family's four strings load through the ConfigDatRows
    /// reader (bead kuluu-o9qt7.34): captions from the mode-dispatched pass (.text
    /// FFXiMain.dll retail-2026-09 0x223E10), prompts from the help-line swap
    /// (`.text 0x225FA2`). The expected values below are regression pins of the installed
    /// DAT — the code never carries them.
    #[test]
    fn abisortw_family_strings_load_from_the_caption_catalog() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut rows = ConfigDatRows::default();
        rows.ensure_loaded(Some(&root));
        assert!(rows.loaded, "install-gated test must reach the catalog");
        assert_eq!(
            rows.abisortw_captions[0], None,
            "mode ability draws no caption (.text 0x223E10 arm)"
        );
        assert_eq!(
            rows.abisortw_captions[1].as_deref(),
            Some("Manually rearrange emotes."),
            "table-0x0E id {ABISORTW_CAPTION_MODE_EMOTE_ID}"
        );
        assert_eq!(
            rows.abisortw_captions[2].as_deref(),
            Some("Rearrange your mounts."),
            "table-0x0E id {ABISORTW_CAPTION_MODE_MOUNT_ID}"
        );
        assert_eq!(
            rows.emote_sort_prompts[0].as_deref(),
            Some("Select emote to move."),
            "table-0x0E id {EMOTE_SORT_PROMPT_FIRST_ID}"
        );
        assert_eq!(
            rows.emote_sort_prompts[1].as_deref(),
            Some("Select emote to switch with the first."),
            "table-0x0E id {EMOTE_SORT_PROMPT_SECOND_ID}"
        );
    }

    /// Fill law (bead kuluu-o9qt7.34): stored order leads in its own order, unknown ids
    /// drop, and every default the store omits keeps its default place behind — one whole
    /// blob read at open (.text FFXiMain.dll retail-2026-09 0x2259C6), never row patches.
    #[test]
    fn merge_emote_order_puts_stored_first_and_keeps_the_rest() {
        let defaults = [1u8, 2, 3, 4];
        let never_arranged: &[u8] = &[];
        assert_eq!(
            merge_emote_order(&defaults, never_arranged),
            vec![1, 2, 3, 4]
        );
        assert_eq!(
            merge_emote_order(&defaults, &[4, 9, 2]),
            vec![4, 2, 1, 3],
            "stored 4 and 2 lead; unknown 9 drops; the rest keep default order behind"
        );
    }

    /// The swap helper's guards (.text FFXiMain.dll retail-2026-09 0x2258D0): same slot,
    /// missing id, or equal row ids no-op; a real pair exchanges (bead kuluu-o9qt7.34).
    #[test]
    fn swap_emote_order_guards_match_the_helper() {
        let mut order = vec![6u8, 4, 12, 3];
        assert!(swap_emote_order(&mut order, 6, 12));
        assert_eq!(order, vec![12, 4, 6, 3]);
        assert!(
            !swap_emote_order(&mut order, 4, 4),
            "same id is the equal-rows guard"
        );
        assert!(
            !swap_emote_order(&mut order, 4, 99),
            "a missing id is the out-of-range guard"
        );
        assert_eq!(order, vec![12, 4, 6, 3], "guard arms touch nothing");
    }

    /// The committed blob keeps hidden ids (they live in retail's user file too): a swap
    /// over an empty store returns the full default table with just those two exchanged.
    #[test]
    fn committed_emote_swap_returns_the_full_blob_with_two_rows_moved() {
        let snap = kuluu_snapshot::SceneSnapshot::default();
        let defaults: Vec<u8> = ffxi_vocab::emote_names::EMOTES
            .iter()
            .map(|&(id, _)| id)
            .collect();
        let swapped = committed_emote_swap(&snap, 2, 10).expect("both ids are real emotes");
        assert_eq!(swapped.len(), defaults.len(), "hidden ids stay in the blob");
        let a0 = defaults.iter().position(|&id| id == 2).unwrap();
        let b0 = defaults.iter().position(|&id| id == 10).unwrap();
        assert_eq!(swapped[a0], 10);
        assert_eq!(swapped[b0], 2);
        let mut untouched: Vec<usize> = (0..defaults.len())
            .filter(|&i| swapped[i] != defaults[i])
            .collect();
        untouched.sort_unstable();
        assert_eq!(untouched, vec![a0, b0], "only the two picked rows move");

        assert_eq!(committed_emote_swap(&snap, 2, 2), None, "equal-rows guard");
        assert_eq!(
            committed_emote_swap(&snap, 2, u8::MAX),
            None,
            "missing-id guard (and no real emote is id {})",
            u8::MAX
        );
    }

    /// The list re-orders with the stored blob without any restart (bead kuluu-o9qt7.34):
    /// a committed swap's fold lands on the next [`emote_rows`] read.
    #[test]
    fn emote_rows_follow_the_stored_display_order() {
        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        let base = emote_rows(&snap);
        assert!(base.len() >= 2, "the canned table has at least two rows");
        let moved_id = match base[1].action {
            DynamicMenuAction::Emote { emote_id } => emote_id,
            other => panic!("expected an emote row, got {other:?}"),
        };
        snap.emote_order.push(moved_id);
        let reordered = emote_rows(&snap);
        assert_eq!(reordered.len(), base.len());
        assert!(
            matches!(reordered[0].action, DynamicMenuAction::Emote { emote_id } if emote_id == moved_id),
            "the stored id leads the list after one swap commit"
        );
    }

    /// The chain row exists only while its DAT caption has loaded (bead kuluu-o9qt7.34):
    /// a machine without an install gets no sort entry at all, never invented copy.
    #[test]
    fn abisortw_chain_row_rides_only_with_its_caption() {
        let mut rows = vec![DynamicMenuRow {
            tint: None,
            label: "Wave (/wave)".to_string(),
            action: DynamicMenuAction::Emote { emote_id: 6 },
        }];
        append_abisortw_chain_row(&mut rows, None);
        assert_eq!(rows.len(), 1, "no caption — no chain row");
        append_abisortw_chain_row(&mut rows, Some("Manually rearrange emotes."));
        assert_eq!(rows.len(), 2);
        let tail = &rows[1];
        assert_eq!(tail.label, "Manually rearrange emotes.");
        assert_eq!(tail.action, DynamicMenuAction::AbisortwSelect { mode: 1 });
    }

    #[test]
    fn home_door_row_index_and_caption_pair_follow_the_handler() {
        assert_eq!(home_door_row_index(), 5);
        let captions: [Option<String>; 2] = [
            Some("Open your Mog House to party and alliance members.".to_string()),
            Some("Close your Mog House to visitors.".to_string()),
        ];
        assert_eq!(
            home_door_caption(&captions, false),
            Some("Open your Mog House to party and alliance members.")
        );
        assert_eq!(
            home_door_caption(&captions, true),
            Some("Close your Mog House to visitors.")
        );
    }

    /// The open-door gate: Layout (select id 5) and Remodel (id 7) go dim and refuse their select
    /// while the house is open to visitors; every other home-view row stays live, and no row is
    /// greyed while the door is shut or outside a Mog House altogether (handler FFXiMain.dll
    /// retail-2026-09 `.text 0x1452FE`/`.text 0x14531A`, against the interactive state its shut-door
    /// path writes at `.text 0x1453C6`/`.text 0x1453ED`).
    #[test]
    fn an_open_mog_house_door_greys_layout_and_remodel_only() {
        let shut = kuluu_snapshot::MyRoom {
            model: 257,
            sub_map: 0,
            door_open: false,
        };
        let open = kuluu_snapshot::MyRoom {
            door_open: true,
            ..shut
        };

        for (index, label) in HOME_VIEW_ROWS.iter().enumerate() {
            let gated = *label == "Layout" || *label == HOME_ROW_REMODEL;
            let mut snap = kuluu_snapshot::SceneSnapshot::default();
            assert!(
                !menu_row_disabled(MenuKind::HomeView, index, &snap),
                "no myroom at all keeps row {label} live"
            );
            snap.myroom = Some(shut);
            assert!(
                !menu_row_disabled(MenuKind::HomeView, index, &snap),
                "a shut house keeps row {label} live"
            );
            snap.myroom = Some(open);
            assert_eq!(
                menu_row_disabled(MenuKind::HomeView, index, &snap),
                gated,
                "row {label} gated={gated} while the door stands open"
            );
        }
    }

    #[test]
    fn home_view_rows_and_title_track_the_myro_dat_law() {
        assert_eq!(
            entries_for(MenuKind::HomeView, false),
            &[
                "M.Ward.",
                "Mog Safe",
                "Storage",
                "Mog Locker",
                "Satchel",
                "Open Mog House",
                "Mog Case",
                "Mog Sack",
                "Delivery Box",
                "Change Jobs",
                "Gardening",
                "Layout",
                "Safe 2",
                "Remodel",
            ]
        );
        assert_eq!(menu_title(MenuKind::HomeView), "Map");
    }

    /// Container rows name their LSB bags (vendor/server/src/map/item_container.h)
    /// and the Gardening/Layout rows their literals — id 4 = "/garden", id 5 =
    /// "/layout" per the corrected arms of handler FFXiMain.dll retail-2026-09
    /// RVA 0x145460 (vault main_menu.md §53).
    #[test]
    fn home_view_rows_map_to_lsb_bags_and_text_command_literals() {
        use ffxi_proto::map::container as c;
        assert_eq!(home_view_row_container("Mog Safe"), Some(c::LOC_MOGSAFE));
        assert_eq!(home_view_row_container("Storage"), Some(c::LOC_STORAGE));
        assert_eq!(
            home_view_row_container("Mog Locker"),
            Some(c::LOC_MOGLOCKER)
        );
        assert_eq!(home_view_row_container("Satchel"), Some(c::LOC_MOGSATCHEL));
        assert_eq!(home_view_row_container("Mog Case"), Some(c::LOC_MOGCASE));
        assert_eq!(home_view_row_container("Mog Sack"), Some(c::LOC_MOGSACK));
        assert_eq!(home_view_row_container("M.Ward."), Some(c::LOC_WARDROBE));
        assert_eq!(home_view_row_container("Safe 2"), Some(c::LOC_MOGSAFE2));
        assert_eq!(home_view_text_command("Gardening"), Some("/garden"));
        assert_eq!(home_view_text_command("Layout"), Some("/layout"));
        // The rows whose retail arms are unbuilt stay un-mapped here and reach
        // the dispatcher's NotImplemented path.
        for label in ["Open Mog House", "Delivery Box", "Remodel", "Change Jobs"] {
            assert!(home_view_row_container(label).is_none());
            assert!(home_view_text_command(label).is_none());
        }
    }

    /// `jobchang` shows its two DAT rows; the pick lists build from the folded
    /// s2c 0x01B roster — locked jobs are not offered, and a sub-side list
    /// waits for the packet's grant (vault main_menu.md §53).
    #[test]
    fn job_change_drill_rows_and_roster_built_picks() {
        assert_eq!(
            entries_for(MenuKind::JobChange, false),
            &["Main Job", "Support Job"]
        );

        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        assert!(job_pick_rows(&snap, true).is_empty(), "roster unknown");
        snap.jobs = Some(kuluu_snapshot::JobList {
            unlocked: (1 << 1) | (1 << 3),
            sub_job_unlocked: false,
            job_levels: vec![0, 99, 0, 24],
        });
        let mains = job_pick_rows(&snap, true);
        assert_eq!(mains.len(), 2, "only unlocked jobs are offered");
        let warrior = ffxi_vocab::job_names::lookup(1).unwrap_or("???");
        assert_eq!(format!("{warrior} LV99"), mains[0].label);
        assert!(matches!(
            mains[0].action,
            DynamicMenuAction::HomeJobRow {
                job_id: 1,
                as_main: true
            }
        ));
        assert!(
            job_pick_rows(&snap, false).is_empty(),
            "sub side waits for the grant"
        );
        snap.jobs.as_mut().unwrap().sub_job_unlocked = true;
        let subs = job_pick_rows(&snap, false);
        assert_eq!(subs.len(), 2);
        assert!(matches!(
            subs[1].action,
            DynamicMenuAction::HomeJobRow {
                job_id: 3,
                as_main: false
            }
        ));
    }

    /// The line editor lists the slot's command lines first (newline-joined in
    /// the store, one per row on screen) and always ends with add/clear rows
    /// (bead kuluu-o9qt7.19.3; mcrledit law vault main_menu.md §34/§41).
    #[test]
    fn macro_editor_rows_list_lines_then_actions() {
        let snap = kuluu_snapshot::SceneSnapshot {
            macros: vec![kuluu_snapshot::MacroSlot {
                block: 0,
                index: 3,
                text: "/ra\n/heal".to_string(),
            }],
            ..Default::default()
        };
        let rows = macro_editor_rows(&snap, 0, 3);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[2].label, MACRO_ROW_USE);
        assert_eq!(
            rows[2].action,
            DynamicMenuAction::MacroUse { block: 0, index: 3 }
        );
        assert_eq!(rows[0].label, "/ra");
        assert_eq!(
            rows[0].action,
            DynamicMenuAction::MacroEditLine {
                line_index: Some(0)
            }
        );
        assert_eq!(rows[1].label, "/heal");
        assert_eq!(
            rows[1].action,
            DynamicMenuAction::MacroEditLine {
                line_index: Some(1)
            }
        );
        // With the use row in, add/clear shifted one down.
        assert_eq!(
            rows[3].label, MACRO_ROW_ADD_LINE,
            "row after the actions is the add-action"
        );
        assert!(matches!(
            rows[3].action,
            DynamicMenuAction::MacroEditLine { line_index: None }
        ));
        assert_eq!(rows[4].label, MACRO_ROW_CLEAR_SLOT);
        assert_eq!(rows[4].action, DynamicMenuAction::MacroClearSlot);

        // A keytop nobody assigned yet still edits (the two action rows).
        let empty = kuluu_snapshot::SceneSnapshot::default();
        let rows = macro_editor_rows(&empty, 1, 7);
        assert_eq!(rows.len(), 2);
        assert!(matches!(
            rows[0].action,
            DynamicMenuAction::MacroEditLine { line_index: None }
        ));
    }

    /// Commit-time merge law (bead kuluu-o9qt7.19.3): replace in place, append
    /// for new lines, and emptying lines prunes them — a keytop with nothing
    /// left leaves no store entry at all.
    #[test]
    fn merge_macro_line_replaces_appends_and_prunes() {
        use kuluu_snapshot::MacroSlot;
        let base = vec![MacroSlot {
            block: 1,
            index: 2,
            text: "/a\n/b".to_string(),
        }];

        let replaced = merge_macro_line(&base, 1, 2, Some(0), "/poke");
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].text, "/poke\n/b");

        let appended = merge_macro_line(&base, 1, 2, None, "/c");
        assert_eq!(appended[0].text, "/a\n/b\n/c");

        // Blanking a line removes just that line…
        let blanked = merge_macro_line(&base, 1, 2, Some(1), "   ");
        assert_eq!(blanked[0].text, "/a");
        // …and blanking the last one removes the keytop.
        let wiped = merge_macro_line(&blanked, 1, 2, Some(0), "");
        assert!(wiped.is_empty(), "cleared slot leaves no store entry");

        // Appending onto a fresh keytop creates it.
        let created = merge_macro_line(&[], 0, 9, None, "/oom");
        assert_eq!(created.len(), 1);
        assert_eq!((created[0].block, created[0].index), (0, 9));
    }

    /// The pupequip rows (vault main_menu.md §39): fourteen in AutomatonSlot
    /// order — Head, Frame, Attachment 1..12 — with the LSB enum vocabulary for
    /// head/frame and "(empty)" cells; slot ids match c2s 0x102's AutomatonSlot.
    #[test]
    fn pupequip_rows_are_fourteen_slots_in_automatonslot_order() {
        let snap = kuluu_snapshot::SceneSnapshot {
            automaton: Some(Box::new(kuluu_snapshot::AutomatonInfo {
                is_sub_job: false,
                head: 2, // Valoredge (vendor/server/src/map/enums/automaton.h)
                frame: 0x21,
                attachments: [0; 12],
                unlocked_heads: 0x3F,
                unlocked_frames: 0x0F,
            })),
            ..Default::default()
        };
        let rows = automaton_equip_rows(&snap);
        assert_eq!(rows.len(), 14);
        assert_eq!(rows[0].label, "Head: Valoredge");
        assert_eq!(rows[1].label, "Frame: Valoredge");
        assert_eq!(rows[2].label, "Attachment 1: (empty)");
        assert_eq!(rows[13].label, "Attachment 12: (empty)");
        let slots: Vec<u8> = rows
            .iter()
            .map(|r| match r.action {
                DynamicMenuAction::AutomatonSlotRow { slot } => slot,
                ref a => panic!("unexpected action {a:?}"),
            })
            .collect();
        assert_eq!(slots, (0u8..14).collect::<Vec<u8>>());
        // The window with no automaton record (frame not arrived) shows nothing.
        assert!(automaton_equip_rows(&kuluu_snapshot::SceneSnapshot::default()).is_empty());
    }

    /// `mp_mncst` is the grounded-title empty display: catalog 76.DAT idx 165
    /// = Customization; its DAT record names no rows (vault main_menu.md §39).
    #[test]
    fn mp_customize_window_is_the_grounded_title_empty_display() {
        assert_eq!(menu_title(MenuKind::MpCustomize), "Customization");
        assert!(entries_for(MenuKind::MpCustomize, false).is_empty());
        assert_eq!(menu_title(MenuKind::AutomatonEquip), "Equipment");
    }

    /// The §41 macro law: DAT grids 'mcrs'/'mcr7' hold 20 keytops each under
    /// two palette windows, so capacity is 2 x 20; mcrselec lists filled store
    /// slots in (block, index) order with pinned labels, and both sub-windows
    /// inherit `Macros` because their comwin tokens read -1.
    #[test]
    fn macro_windows_track_the_dat_grid_capacity_law() {
        assert_eq!(MACRO_BLOCKS * MACRO_SLOTS_PER_BLOCK, 40);
        assert!(is_dynamic(MenuKind::MacroSelect));
        assert_eq!(menu_title(MenuKind::MacroSelect), "Macros");
        assert_eq!(menu_title(MenuKind::MacroResult), "Macros");
        // A fresh character: retail's keytop cells are blank, kuluu lists
        // nothing.
        let fresh = kuluu_snapshot::SceneSnapshot::default();
        assert!(macro_select_rows(&fresh).is_empty());

        let seeded = kuluu_snapshot::SceneSnapshot {
            macros: vec![
                kuluu_snapshot::MacroSlot {
                    block: 1,
                    index: 0,
                    text: "/lastshout".to_string(),
                },
                kuluu_snapshot::MacroSlot {
                    block: 0,
                    index: 4,
                    text: "/ra".to_string(),
                },
            ],
            ..Default::default()
        };
        let rows = macro_select_rows(&seeded);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "B1/5: /ra");
        assert_eq!(rows[1].label, "B2/1: /lastshout");
        assert_eq!(
            rows[0].action,
            DynamicMenuAction::MacroSlotRow { block: 0, index: 4 }
        );
    }

    /// The commenu law (vault main_menu.md §37): two list widgets in DAT order —
    /// Friend List (catalog 61) then Emote List (catalog 180) — under the grounded
    /// title Communication (comwin token 179). The friend window's own row set is
    /// retail-empty on LSB servers (§38.5): no friends table, SearchType Friend
    /// 0x0C has no handler case there, so kuluu shows an empty grounded window.
    #[test]
    fn communication_menu_rows_track_the_commen_law() {
        assert_eq!(
            entries_for(MenuKind::Communication, false),
            &[COMM_FRIEND_LIST, COMM_EMOTE_LIST]
        );
        assert_eq!(menu_title(MenuKind::Communication), "Communication");
        assert_eq!(menu_title(MenuKind::FriendList), "Friend List");
        assert_eq!(entries_for(MenuKind::FriendList, false).len(), 0);
    }

    /// The Link Shell law (vault main_menu.md §27): the window always shows both shell slots the
    /// server tracks, labelled by whether s2c 0x0E0 reports a pearl and where it lives.
    #[test]
    fn linkshell_window_rows_track_the_two_shell_slots_the_server_reports() {
        // Both slots always exist (vault main_menu.md section 27.7: LSB pushes LinkshellNum 1 and 2
        // at login); only the s2c 0x0E0 payload changes whether a slot holds a pearl, and where.
        let empty = kuluu_snapshot::SceneSnapshot::default();
        let rows = linkshell_window_rows(&empty);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "Link Shell 1   (no pearl equipped)");
        assert_eq!(rows[1].label, "Link Shell 2   (no pearl equipped)");
        assert_eq!(
            rows[1].action,
            DynamicMenuAction::LinkShellSlot { linkshell_slot: 2 }
        );

        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        snap.linkshells.two = kuluu_snapshot::LinkShellSlot {
            category: 0,
            item_index: 12,
        };
        let rows = linkshell_window_rows(&snap);
        assert_eq!(rows[0].label, "Link Shell 1   (no pearl equipped)");
        assert_eq!(
            rows[1].label,
            "Link Shell 2   pearl at container 0, index 12"
        );

        // Root row 10 and the right-pane preview share one mapping.
        assert_eq!(
            root_child_kind("Linkshell", false, false),
            Some(MenuKind::Linkshell)
        );
        assert_eq!(menu_title(MenuKind::Linkshell), "Link Shell");
    }

    /// The search surface law (vault main_menu.md §26): the Run row leads, then
    /// one hit row per server frame entry in order. Label = name, job abbrevs +
    /// levels from the AddPlayer entries — or `???` for a hidden player whose
    /// job/level/race/rank entries were dropped (flags1 & 0x4000), then zone.
    #[test]
    fn search_window_rows_lead_with_run_then_hits_in_server_order() {
        use kuluu_snapshot::{SearchPlayer, SearchResults};

        let empty = kuluu_snapshot::SceneSnapshot::default();
        assert_eq!(search_window_rows(&empty).len(), 1);

        let snap = kuluu_snapshot::SceneSnapshot {
            search: Some(Box::new(SearchResults {
                total: 2,
                players: vec![
                    SearchPlayer {
                        name: "bob".into(),
                        zone: 103,
                        nation: Some(1),
                        main_job: Some(3),
                        sub_job: Some(1),
                        main_level: Some(75),
                        sub_level: Some(37),
                        race: Some(2),
                        rank: Some(3),
                        flags1: 0x42,
                        id: 0x1_2345,
                        comment_type: None,
                        flags2: 0,
                        languages: 3,
                    },
                    SearchPlayer {
                        name: "ALongNameX".into(),
                        zone: 0,
                        nation: None,
                        main_job: None,
                        sub_job: None,
                        main_level: None,
                        sub_level: None,
                        race: None,
                        rank: None,
                        flags1: 0x4000,
                        id: 99,
                        comment_type: Some(7),
                        flags2: 0xDEAD,
                        languages: 1,
                    },
                ],
            })),
            ..Default::default()
        };
        let rows = search_window_rows(&snap);
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert_eq!(rows[0].label, "Run Search");
        // Job ids 3/1 are WHM/WAR in the LSB job table scraped by ffxi-vocab.
        assert_eq!(&rows[1].label, &{
            let whm = ffxi_vocab::job_names::abbrev(3).expect("WHM has a name");
            let war = ffxi_vocab::job_names::abbrev(1).expect("WAR has a name");
            format!("bob  {whm}75/{war}37  Z103")
        });
        assert_eq!(rows[1].action, DynamicMenuAction::SearchHitRow { index: 0 });
        // The hidden shape carries no job/level entries — the row says so.
        assert_eq!(&rows[2].label, "ALongNameX  ???  Z0");
        assert_eq!(rows[2].action, DynamicMenuAction::SearchHitRow { index: 1 });
    }

    /// Region Info law (vault main_menu.md §29): the window answers items 1..4 as display modes
    /// 0..3; kuluu renders exactly four rows in that order with the marker on the installed mode.
    #[test]
    fn region_info_rows_are_the_four_modes_with_the_active_marker() {
        let known = Some(REGION_PRED_ZONES_PINNED[0]);
        let rows = region_info_rows(2, known);
        assert_eq!(rows.len(), 4, "items 1..4 only — always four rows");
        assert_eq!(
            rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
            vec!["World", "Nation", "\u{bb} Region", "Zone"],
            "handler item order 1..4, marker on the installed mode"
        );
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(
                row.action,
                DynamicMenuAction::RegionInfoMode { mode: i as u8 }
            );
        }

        // The message-0x7B seam: with mode 0 installed over a zone outside FFXiMain's predicate
        // table (and with no zone resolved yet) the World row wears the notice; other rows never.
        let unknown = region_info_rows(0, Some(u16::MAX));
        assert_eq!(
            unknown[0].label,
            format!("\u{bb} World {}", REGION_UNAVAILABLE_NOTICE_PINNED),
            "the notice rides the active marker on the same row"
        );
        assert!(!unknown[1].label.contains("unavailable"));
        let no_zone = region_info_rows(0, None);
        assert!(no_zone[0].label.contains(REGION_UNAVAILABLE_NOTICE_PINNED));

        // Known zone or non-zero mode: the notice is gone.
        assert_eq!(region_info_rows(0, known)[0].label, "\u{bb} World");
        assert_eq!(region_info_rows(3, Some(u16::MAX))[0].label, "World");
    }

    #[test]
    fn region_info_window_is_registered_dynamic_and_titled() {
        assert!(is_dynamic(MenuKind::RegionInfo { mode: 0 }));
        assert_eq!(menu_title(MenuKind::RegionInfo { mode: 0 }), "Region Info");
        assert_eq!(
            root_child_kind("Region Info", false, false),
            Some(MenuKind::RegionInfo { mode: 0 })
        );
    }

    /// Mission law (vault main_menu.md §30): missionm lists exactly the logs the
    /// s2c 0x056 state holds and every row drills that port's miss00 detail.
    #[test]
    fn mission_list_rows_follow_the_folded_battery() {
        let empty = kuluu_snapshot::SceneSnapshot::default();
        let rows = mission_list_rows(&empty);
        assert_eq!(rows.len(), 1, "logged-out state keeps one hint row");
        assert!(rows[0].label.contains("no mission logs"));

        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        snap.missions.info = Some(kuluu_snapshot::MissionInfo {
            nation: 2,
            nation_mission: 3,
            rotz: 0,
            cop: 0,
            cop2: 0,
            addon_acp: 0,
            addon_amk: 0,
            addon_asa: 0,
            tales_beginning: 0,
            soa: 0,
            rov: 0,
        });
        snap.missions.logs.push(kuluu_snapshot::MissionLogEntry {
            port: 0x00D0,
            data: [1, 0, 0, 0, 0, 0, 0, 0],
        });
        let rows = mission_list_rows(&snap);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "Windurst: mission #3");
        assert_eq!(
            rows[0].action,
            DynamicMenuAction::MissionOpenLog {
                port: ffxi_proto::mission::PORT_MISSION_MAIN
            }
        );
        assert_eq!(rows[1].label, "Completed missions — Nations");

        // A quest port carries data but belongs to the /quest seam, not here.
        snap.missions.logs.push(kuluu_snapshot::MissionLogEntry {
            port: 0x0058,
            data: [7; 8],
        });
        assert_eq!(mission_list_rows(&snap).len(), 2);

        // Detail rows are legible-only field dumps per the [I-pending] paging law.
        let detail = mission_detail_rows(&snap, ffxi_proto::mission::PORT_MISSION_MAIN);
        assert_eq!(detail[0].label, "Windurst: mission #3");
        for row in &detail {
            assert!(matches!(
                row.action,
                DynamicMenuAction::MissionLogRow { .. }
            ));
        }
    }

    #[test]
    fn mission_windows_are_dynamic_titled_and_rooted() {
        assert!(is_dynamic(MenuKind::MissionList));
        assert!(is_dynamic(MenuKind::MissionDetail { port: 0xD0 }));
        assert_eq!(menu_title(MenuKind::MissionList), "Missions");
        assert_eq!(
            root_child_kind("Missions", false, false),
            Some(MenuKind::MissionList)
        );
    }

    #[test]
    fn quest_list_rows_expand_the_quest_ports() {
        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        let empty = quest_list_rows(&snap);
        assert_eq!(empty.len(), 1, "logged-out state keeps one hint row");
        assert!(empty[0].label.contains("no quests"));

        // San d'Oria offer bits 0/2 and the id-18 overlap between that same
        // offer port and Sandoria's complete port: three rows, log-then-id order.
        snap.missions.logs.push(kuluu_snapshot::MissionLogEntry {
            port: ffxi_proto::mission::QUEST_OFFER_PORTS[0],
            data: [1 | (1 << 2) | (1 << 18), 0, 0, 0, 0, 0, 0, 0],
        });
        snap.missions.logs.push(kuluu_snapshot::MissionLogEntry {
            port: ffxi_proto::mission::QUEST_COMPLETE_PORTS[0],
            data: [1 << 18, 0, 0, 0, 0, 0, 0, 0],
        });
        // A complete-mission port must not leak into this seam either direction.
        snap.missions.logs.push(kuluu_snapshot::MissionLogEntry {
            port: 0x00D0,
            data: [7; 8],
        });

        let rows = quest_list_rows(&snap);
        assert_eq!(rows.len(), 3, "one row per (log, id) bit held");
        // scripts/globals/quests.lua San d'Oria block.
        assert_eq!(rows[0].label, "A Sentrys Peril");
        assert_eq!(rows[1].label, "Rosel the Armorer");
        assert_eq!(rows[2].label, "Gates to Paradise (complete)");
        assert_eq!(
            rows[0].action,
            DynamicMenuAction::QuestOpenDetail { log: 0, id: 0 }
        );
        assert_eq!(
            rows[2].action,
            DynamicMenuAction::QuestOpenDetail { log: 0, id: 18 }
        );

        // The quest01 stand-in is a legible-only dump of the fields kuluu holds.
        let detail = quest_detail_rows(&snap, 0, 18);
        assert_eq!(detail[0].label, "Gates to Paradise");
        assert_eq!(detail[1].label, "Area: Sandoria");
        assert_eq!(detail[2].label, "Complete.");
        for row in &detail {
            assert!(matches!(
                row.action,
                DynamicMenuAction::QuestDetailRow { log: 0, id: 18 }
            ));
        }
    }

    #[test]
    fn quest_windows_are_dynamic_titled_and_never_root_drilled() {
        assert!(is_dynamic(MenuKind::QuestList));
        assert!(is_dynamic(MenuKind::QuestDetail { log: 0, id: 4 }));
        assert_eq!(menu_title(MenuKind::QuestList), "Quests");
        assert_eq!(
            menu_title(MenuKind::QuestDetail { log: 2, id: 9 }),
            "Quest Log"
        );
        // Retail opens the list through the "/quest" text command, never a row drill.
        assert_eq!(root_child_kind("Quests", false, false), None);
    }

    #[test]
    fn view_house_veto_follows_the_capital_zone_table() {
        // Gate law pinned from the FFXiMain.dll retail-2026-09 RVA 0xF9680 zone
        // map (vault main_menu.md §32): ten capitals need the in-house operand,
        // Chateau d'Oraguille and Metalworks never do, anything else is free.
        assert!(view_house_blocked(Some(230), false));
        assert!(view_house_blocked(Some(241), false));
        assert!(!view_house_blocked(Some(233), false));
        assert!(!view_house_blocked(Some(237), false));
        assert!(!view_house_blocked(Some(229), false));
        assert!(!view_house_blocked(Some(242), false));
        // In your own Mog House the predicate passes in a capital too.
        assert!(!view_house_blocked(Some(230), true));
        // Unknown zone (not yet reported) never vetoes.
        assert!(!view_house_blocked(None, false));
    }

    #[test]
    fn bazaar_stall_picker_is_dynamic_titled_and_root_drilled() {
        assert!(is_dynamic(MenuKind::BazaarStalls));
        assert_eq!(menu_title(MenuKind::BazaarStalls), "Bazaar");
        // The ungated page-2 arm (vault main_menu.md §33) drills it from Root.
        assert_eq!(
            root_child_kind(ROOT_SET_BAZAAR, false, false),
            Some(MenuKind::BazaarStalls)
        );
    }

    #[test]
    fn bazaar_stall_rows_follow_the_entity_bazaar_flag() {
        use kuluu_snapshot::{Entity, EntityKind};
        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        assert_eq!(bazaar_stall_rows(&snap).len(), 1);
        assert!(bazaar_stall_rows(&snap)[0]
            .label
            .contains("no player bazaars"));

        snap.entities.push(Entity {
            id: 7,
            act_index: 2,
            kind: EntityKind::Pc,
            name: Some("Kupipi".into()),
            pos: kuluu_snapshot::Vec3::default(),
            heading: 0,
            hp_pct: Some(100),
            bt_target_id: 0,
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            mount: None,
            status: 0,
            char_flags: Default::default(),
            monstrosity: false,
            name_vis: None,
        });
        assert!(
            bazaar_stall_rows(&snap)[0]
                .label
                .contains("no player bazaars"),
            "the flag is required"
        );
        snap.entities[0].char_flags.bazaar = true;
        let rows = bazaar_stall_rows(&snap);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "Kupipi's Bazaar");
        assert_eq!(
            rows[0].action,
            DynamicMenuAction::BazaarStall { char_id: 7 }
        );
        snap.self_char_id = Some(7);
        assert!(bazaar_stall_rows(&snap)[0]
            .label
            .contains("no player bazaars"));
    }

    /// `DynamicMenu::client_message` reads the DAT-sourced table-9 catalog once (retail's
    /// emitter `.text FFXiMain.dll retail-2026-09 `.text 0x129DE0`; vault main_menu.md §59
    /// addendum 35) and never loses a line: without an install — or for an id the catalog
    /// lacks — it names the id numerically instead (bead kuluu-o9qt7.41).
    #[test]
    fn client_message_falls_back_to_the_numeric_id_without_an_install() {
        let mut menu = DynamicMenu::default();
        assert_eq!(
            menu.client_message(None, 0x12D),
            "[system message 0x12D]",
            "no install → the numeric line, never a dropped toast"
        );
    }

    /// Install-gated half of the same law: on a retail install the exact catalog wording is
    /// returned (the strings live only in DAT — file id 55648, ROM/165/72.DAT per FTABLE).
    #[test]
    fn client_message_returns_the_table9_dat_wording_on_a_retail_install() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut menu = DynamicMenu::default();
        assert_eq!(
            menu.client_message(Some(&root), 0x12D),
            "Furnishing camera collision has been turned on."
        );
        // The second call is served from the cached catalog, and setter vs query wording
        // stays distinct (0x132 "has been set", 0x134 "is currently set").
        assert_eq!(
            menu.client_message(Some(&root), 0x132),
            "Layout control style has been set to type 2."
        );
        assert_eq!(
            menu.client_message(Some(&root), 0x134),
            "Layout control style is currently set to type 2."
        );
    }

    /// The store side of `/furcc`/`/layoutctrl`: the ConfigSet arm writes the raw inverted
    /// value and a follow-up query must read it back non-zero/zero — pinning the exact
    /// `BTreeMap::get` semantics the apply seam relies on (bead kuluu-o9qt7.41).
    #[test]
    fn config_option_store_roundtrip_reports_the_written_side() {
        let mut menu = DynamicMenu::default();
        let key = CONFIG_OPTION_FURNISHING_CAMERA_COLLISION;
        assert_eq!(*menu.config_option_values.get(&key).unwrap_or(&0), 0);
        menu.config_option_values.insert(key, 1);
        assert_ne!(*menu.config_option_values.get(&key).unwrap_or(&0), 0);
        menu.config_option_values.insert(key, 0);
        assert_eq!(*menu.config_option_values.get(&key).unwrap_or(&0), 0);
    }

    /// `DynamicMenu::config_query` splits retail's two query shapes: plain getters that only
    /// test the value (`/furcc`, `/layoutctrl`) leave any dword untouched, while `/statusparty`
    /// bool-normalizes (`neg/sbb/inc`) and **rewrites** through the setter at merge .text
    /// FFXiMain.dll retail-2026-09 0x7EB1C — so a stored 5 reads back as 1 afterwards, and an
    /// absent key answers zero either way (bead kuluu-o9qt7.42; vault §59 addendum 35).
    #[test]
    fn config_query_plain_reads_survive_and_statusparty_normalizes_the_store() {
        let mut menu = DynamicMenu::default();
        // Plain read: raw non-zero is reported as-is, store untouched.
        menu.config_option_values
            .insert(CONFIG_OPTION_LAYOUT_CONTROL_TYPE, 5);
        assert_eq!(
            menu.config_query(CONFIG_OPTION_LAYOUT_CONTROL_TYPE, false),
            5
        );
        assert_eq!(
            *menu
                .config_option_values
                .get(&CONFIG_OPTION_LAYOUT_CONTROL_TYPE)
                .unwrap_or(&0),
            5,
            "furcc/layoutctrl getters never write back"
        );
        // statusparty family: normalize + rewrite, and absent keys read as zero.
        assert_eq!(menu.config_query(CONFIG_OPTION_STATUS_PARTY_ICONS, true), 0);
        menu.config_option_values
            .insert(CONFIG_OPTION_STATUS_PARTY_ICONS, -3);
        assert_eq!(menu.config_query(CONFIG_OPTION_STATUS_PARTY_ICONS, true), 1);
        assert_eq!(
            *menu
                .config_option_values
                .get(&CONFIG_OPTION_STATUS_PARTY_ICONS)
                .unwrap_or(&0),
            1,
            "the normalized bool is stored back at the merge point"
        );
    }

    fn cursor_memory_app(mode: InputMode) -> App {
        let mut app = App::new();
        app.insert_resource(mode)
            .init_resource::<DynamicMenu>()
            .add_systems(Update, persist_window_cursors);
        app
    }

    /// Windows still open when the whole menu goes away close into the session memory, and the
    /// next menu opens with it
    /// (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md).
    #[test]
    fn the_memory_outlives_the_menu_for_the_session() {
        let mut stack = MenuStack::root();
        stack.levels[0].cursor = 3;
        stack.push(MenuKind::MagicGroups);
        stack.current_mut().unwrap().cursor = 4;
        let mut app = cursor_memory_app(InputMode::Menu(stack));
        app.update();
        *app.world_mut().resource_mut::<InputMode>() = InputMode::World;
        app.update();

        let memory = app.world().resource::<DynamicMenu>().window_cursors.clone();
        let magic = registry_window(MenuKind::MagicGroups).unwrap();
        assert_eq!(memory.recall(magic), Some(4));
        assert_eq!(memory.recall(MAIN_MENU_WINDOW), Some(3));
        let reopened = MenuStack::open_main_menu(&memory);
        assert_eq!(reopened.levels[0].cursor, 3);
    }

    /// A menu waiting behind a sub-target cursor is still open; the Command menu's pickers close
    /// into the memory with it.
    #[test]
    fn a_sub_target_cursor_keeps_its_menu_open_and_pickers_close_with_the_command_menu() {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::MagicGroups);
        stack.current_mut().unwrap().cursor = 2;
        let waiting = InputMode::SubTarget(crate::input_mode::SubTargetState::open(
            crate::input_mode::SubTargetAction::Ranged,
            0,
            InputMode::Menu(stack),
        ));
        let mut app = cursor_memory_app(waiting);
        app.update();
        let magic = registry_window(MenuKind::MagicGroups).unwrap();
        assert_eq!(
            app.world()
                .resource::<DynamicMenu>()
                .window_cursors
                .recall(magic),
            None,
            "the menu has not closed"
        );

        let mut command = TargetActionState::open(Default::default());
        command.chat_mode_idx = 1;
        *app.world_mut().resource_mut::<InputMode>() = InputMode::TargetAction(command);
        app.update();
        *app.world_mut().resource_mut::<InputMode>() = InputMode::World;
        app.update();
        let memory = &app.world().resource::<DynamicMenu>().window_cursors;
        assert_eq!(memory.recall(magic), Some(2));
        assert_eq!(memory.recall(crate::input_mode::CHATCTRL_WINDOW), Some(1));
    }

    /// A remembered cursor that opened past a built window's rows comes back to its last row;
    /// an empty window parks it on item 1.
    #[test]
    fn a_remembered_cursor_waits_for_its_rows_then_settles_inside_them() {
        let list = registry_window(MenuKind::MagicGroup(1)).unwrap();
        let mut memory = CursorMemory::default();
        memory.record(list, Some(9));
        let mut stack = MenuStack::open_main_menu(&memory);
        stack.push(MenuKind::MagicGroups);
        stack.push(MenuKind::MagicGroup(1));
        let rows: Vec<DynamicMenuRow> = (0..3)
            .map(|spell_id| DynamicMenuRow {
                label: String::new(),
                action: DynamicMenuAction::CastSpell { spell_id },
                tint: None,
            })
            .collect();
        let rows_kind = Some(MenuKind::MagicGroup(1));
        let mut app = App::new();
        app.insert_resource(InputMode::Menu(stack.clone()))
            .insert_resource(DynamicMenu {
                rows: rows.clone(),
                rows_kind,
                ..Default::default()
            })
            .init_resource::<ConfigDatRows>()
            .init_resource::<crate::hud::unity_fp::UnityFpDat>()
            .add_systems(Update, settle_opened_windows);
        app.update();
        let InputMode::Menu(settled) = app.world().resource::<InputMode>() else {
            panic!("menu");
        };
        let top = settled.current().unwrap();
        assert_eq!(top.cursor, rows.len() - 1);
        assert!(!top.clamp_pending);

        app.insert_resource(InputMode::Menu(stack))
            .insert_resource(DynamicMenu {
                rows_kind,
                ..Default::default()
            });
        app.update();
        let InputMode::Menu(settled) = app.world().resource::<InputMode>() else {
            panic!("menu");
        };
        assert_eq!(settled.current().unwrap().cursor, 0);
    }

    /// A window the input opens after this frame's rows were built keeps its remembered cursor
    /// until rows built for that window bound it, whatever order the systems run in.
    #[test]
    fn a_remembered_cursor_is_not_bounded_by_the_previous_windows_rows() {
        const REMEMBERED: usize = 5;
        fn open_emotes(mut mode: ResMut<InputMode>) {
            if let InputMode::Menu(stack) = &mut *mode {
                if stack
                    .current()
                    .is_some_and(|level| level.kind == MenuKind::Root)
                {
                    stack.push(MenuKind::EmoteList);
                }
            }
        }
        let mut memory = CursorMemory::default();
        memory.record(
            registry_window(MenuKind::EmoteList).unwrap(),
            Some(REMEMBERED),
        );
        let mut app = App::new();
        app.init_resource::<crate::snapshot::SceneState>()
            .init_resource::<crate::hud::item_detail::SortOptions>()
            .init_resource::<crate::hud::item_screen::ItemScreenContainer>()
            .init_resource::<MagicSchoolTable>()
            .init_resource::<ConfigDatRows>()
            .init_resource::<CommandSelectorCaptions>()
            .init_resource::<EquipRecords>()
            .init_resource::<crate::keybinds::Bindings>()
            .init_resource::<crate::hud::unity_fp::UnityFpDat>()
            .init_resource::<crate::hud::merit_windows::MeritDat>()
            .init_resource::<DynamicMenu>()
            .insert_resource(InputMode::Menu(MenuStack::open_main_menu(&memory)))
            .add_systems(
                Update,
                (
                    refresh_dynamic_menu_rows,
                    open_emotes,
                    settle_opened_windows,
                )
                    .chain(),
            );
        app.update();
        app.update();
        let InputMode::Menu(stack) = app.world().resource::<InputMode>() else {
            panic!("menu");
        };
        let top = stack.current().unwrap();
        assert_eq!(top.kind, MenuKind::EmoteList);
        assert!(app.world().resource::<DynamicMenu>().rows.len() > REMEMBERED);
        assert_eq!(top.cursor, REMEMBERED);
        assert!(!top.clamp_pending);
    }

    /// The rule belongs to the window name: windows kuluu shares between kinds share one rule and
    /// one memory, display-only and kuluu-only windows have none, and the merit and arrange windows
    /// keep their own records' laws.
    #[test]
    fn cursor_rules_follow_the_window_name() {
        for (kind, rule) in [
            (MenuKind::Root, OpenCursor::Remembered),
            (MenuKind::Status, OpenCursor::Remembered),
            (MenuKind::QuestList, OpenCursor::FirstItem),
            (MenuKind::UnityChat, OpenCursor::FirstItem),
            (MenuKind::UnityMenu, OpenCursor::Remembered),
            (MenuKind::RetailConfig, OpenCursor::Remembered),
            (
                MenuKind::RetailConfigSub(CONFIG_SUB_CONFYN),
                OpenCursor::FirstItem,
            ),
            (
                MenuKind::PowerPrompt { shut_down: false },
                OpenCursor::FirstItem,
            ),
            (MenuKind::RoomList, OpenCursor::FirstItem),
            (MenuKind::HomeView, OpenCursor::Remembered),
            (MenuKind::EmoteList, OpenCursor::Remembered),
        ] {
            assert_eq!(window_cursor(kind).map(|w| w.rule), Some(rule), "{kind:?}");
        }
        assert_eq!(
            registry_window(MenuKind::MagicGroup(1)),
            registry_window(MenuKind::MagicGroup(2)),
            "every school lists in the one `magic` window"
        );
        for kind in [
            MenuKind::Controls,
            MenuKind::Debug,
            MenuKind::Magic,
            MenuKind::StatusDetail { entry: 0 },
            MenuKind::Merit(crate::hud::merit_windows::MeritWindow::Menu),
        ] {
            assert_eq!(window_cursor(kind), None, "{kind:?}");
        }
    }

    fn three_rows() -> Vec<DynamicMenuRow> {
        ["one", "two", "three"].map(display_row).to_vec()
    }

    fn hub_records(item_count: u8) -> ConfigDatRows {
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        let hub = RETAIL_CONFIG_HUB_WINDOW.to_string();
        table
            .by_root
            .insert(hub.clone(), (0..3).map(|id| (id, String::new())).collect());
        table.item_ids.insert(hub.clone(), vec![1, 2, 3]);
        table.item_counts.insert(hub.clone(), item_count);
        table.row_ys.insert(hub, vec![10, 30, 20]);
        table
    }

    /// The unselectable item is the one whose DAT item id equals the record's item count, at
    /// whatever row it draws; a trailing block that is not a drawn row locks nothing
    /// (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md).
    #[test]
    fn the_record_names_the_unselectable_row() {
        const ITEMS: u8 = 3;
        assert_eq!(
            hub_records(ITEMS).last_item_row(RETAIL_CONFIG_HUB_WINDOW),
            Some(LastItemRow::Row(1)),
            "item 3 draws second, by its block y"
        );
        assert_eq!(
            hub_records(ITEMS + 1).last_item_row(RETAIL_CONFIG_HUB_WINDOW),
            Some(LastItemRow::Undrawn)
        );
        assert_eq!(
            ConfigDatRows::default().last_item_row(RETAIL_CONFIG_HUB_WINDOW),
            None,
            "unread until the records load"
        );
    }

    #[test]
    fn opened_windows_learn_their_unselectable_row_and_status_locks_none() {
        const ITEMS: u8 = 3;
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfig);
        let mut app = App::new();
        app.insert_resource(InputMode::Menu(stack))
            .insert_resource(DynamicMenu {
                rows: three_rows(),
                ..Default::default()
            })
            .insert_resource(hub_records(ITEMS))
            .init_resource::<crate::hud::unity_fp::UnityFpDat>()
            .add_systems(Update, settle_opened_windows);
        app.update();
        let InputMode::Menu(stack) = app.world().resource::<InputMode>() else {
            panic!("menu");
        };
        assert!(stack.current().unwrap().row_locked(1));

        let mut status = MenuStack::root();
        status.push(MenuKind::Status);
        app.insert_resource(InputMode::Menu(status));
        app.update();
        let InputMode::Menu(status) = app.world().resource::<InputMode>() else {
            panic!("menu");
        };
        let level = status.current().unwrap();
        assert!(
            level.last_item_lock.is_some(),
            "statcom2 is in the record's table"
        );
        assert!(
            (0..STATUS_LABELS.len()).all(|row| !level.row_locked(row)),
            "its last item is the trailing dummy block, no drawn row"
        );
    }

    fn locked_hub(row: usize) -> MenuStack {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfig);
        stack.current_mut().unwrap().last_item_lock = Some(crate::input_mode::LastItemLock {
            open_cursor: 0,
            row: LastItemRow::Row(row),
        });
        stack
    }

    /// The pointer neither moves the cursor onto the unselectable item nor selects it.
    #[test]
    fn the_pointer_ignores_the_unselectable_item() {
        const LOCKED: usize = 2;
        let mut app = App::new();
        app.insert_resource(InputMode::Menu(locked_hub(LOCKED)))
            .init_resource::<GraphicsSettings>()
            .insert_resource(DynamicMenu {
                rows: three_rows(),
                ..Default::default()
            })
            .add_message::<MenuRowActivated>()
            .add_systems(Update, (menu_mouse_hover_system, menu_mouse_click_system));
        let row = app
            .world_mut()
            .spawn((MainMenuRow { slot: LOCKED }, Interaction::Pressed))
            .id();
        app.update();
        let InputMode::Menu(stack) = app.world().resource::<InputMode>() else {
            panic!("menu");
        };
        assert_eq!(stack.current().unwrap().cursor, 0);
        assert!(app
            .world_mut()
            .resource_mut::<Messages<MenuRowActivated>>()
            .drain()
            .next()
            .is_none());

        app.world_mut()
            .entity_mut(row)
            .insert(MainMenuRow { slot: 1 });
        app.world_mut().entity_mut(row).insert(Interaction::Hovered);
        app.update();
        let InputMode::Menu(stack) = app.world().resource::<InputMode>() else {
            panic!("menu");
        };
        assert_eq!(
            stack.current().unwrap().cursor,
            1,
            "any other row still answers"
        );
    }

    /// The unselectable item draws in the normal colour, not greyed.
    #[test]
    fn the_unselectable_item_draws_in_the_normal_colour() {
        const LOCKED: usize = 2;
        let mut app = root_menu_app(0);
        app.insert_resource(InputMode::Menu(locked_hub(LOCKED)));
        app.world_mut().resource_mut::<DynamicMenu>().rows = three_rows();
        app.update();
        let color = app
            .world_mut()
            .query_filtered::<(&MainMenuRow, &TextColor), Without<CompanionRow>>()
            .iter(app.world())
            .find(|(row, _)| row.slot == LOCKED)
            .map(|(_, color)| color.0)
            .expect("the row is drawn");
        assert_eq!(color, theme::TEXT);
    }

    const DOCK_SURFACE_WIDTH: u32 = 1000;
    const DOCK_SURFACE_HEIGHT: u32 = 720;
    const LOG_TOP: f32 = 500.0;
    const LOG_HEIGHT: f32 = 160.0;
    const LOG_WIDTH: f32 = 480.0;
    const IN_ZONE: u16 = 1;

    /// `stack` drawn in a zone on a `DOCK_SURFACE_HEIGHT`-tall screen with the dock law running
    /// against a chat log whose top edge is at `LOG_TOP`.
    fn dock_app(stack: MenuStack, frames: &[(&str, (i16, i16, i16, i16), u8)]) -> (App, Entity) {
        let mut app = stack_app(stack, frames);
        app.init_resource::<crate::hud::dock::DockBoard>()
            .add_systems(
                Update,
                crate::hud::dock::dock_windows_system
                    .before(update_main_menu)
                    .before(update_menu_stack_windows),
            );
        app.world_mut()
            .resource_mut::<crate::snapshot::SceneState>()
            .snapshot
            .zone_id = Some(IN_ZONE);
        app.world_mut().spawn((
            Window {
                resolution: bevy::window::WindowResolution::new(
                    DOCK_SURFACE_WIDTH,
                    DOCK_SURFACE_HEIGHT,
                ),
                ..default()
            },
            bevy::window::PrimaryWindow,
        ));
        let log = app
            .world_mut()
            .spawn((crate::hud::chat_panel::ChatPanelGroup, Node::default()))
            .id();
        move_log(&mut app, log, LOG_TOP);
        (app, log)
    }

    fn move_log(app: &mut App, log: Entity, top: f32) {
        app.world_mut().entity_mut(log).insert((
            ComputedNode {
                size: Vec2::new(LOG_WIDTH, LOG_HEIGHT),
                inverse_scale_factor: 1.0,
                ..default()
            },
            bevy::ui::UiGlobalTransform::from_xy(0.0, top + LOG_HEIGHT / 2.0),
        ));
    }

    /// The node offset that puts a window's bottom edge 2 above a log whose top edge is at
    /// `log_top`.
    fn docked_bottom(log_top: f32) -> Val {
        Val::Px(DOCK_SURFACE_HEIGHT as f32 - (log_top - crate::hud::dock::DOCK_GAP))
    }

    /// A docked window keeps its record x, a right-half record included, and hangs 2 above the
    /// chat log's top edge. It follows that edge, and stays where it is while the log is hidden
    /// and after it is gone.
    #[test]
    fn a_docked_window_keeps_its_record_x_and_follows_the_chat_logs_top_edge() {
        const EMOTE_X: i16 = 400;
        const LOG_RAISE: f32 = 40.0;
        let mut stack = MenuStack::root();
        stack.push(MenuKind::EmoteList);
        let (mut app, log) = dock_app(
            stack,
            &[(
                EMOTE_LIST_WINDOW,
                (EMOTE_X, 120, 100, 200),
                FRAME_ANCHOR_NONE,
            )],
        );
        app.update();
        let (pane, _) = drawn_pane(&mut app);
        assert!(f32::from(EMOTE_X) >= LEFT_HALF_MAX_X);
        assert_eq!(
            (pane.left, pane.margin.left, pane.right),
            (Val::Px(f32::from(EMOTE_X)), Val::ZERO, Val::Auto)
        );
        assert_eq!((pane.top, pane.bottom), (Val::Auto, docked_bottom(LOG_TOP)));

        move_log(&mut app, log, LOG_TOP - LOG_RAISE);
        app.update();
        assert_eq!(
            drawn_pane(&mut app).0.bottom,
            docked_bottom(LOG_TOP - LOG_RAISE)
        );

        app.world_mut().get_mut::<Node>(log).unwrap().display = Display::None;
        app.update();
        assert_eq!(
            drawn_pane(&mut app).0.bottom,
            docked_bottom(LOG_TOP - LOG_RAISE),
            "a hidden log leaves it in place"
        );

        app.world_mut().entity_mut(log).despawn();
        app.update();
        assert_eq!(
            drawn_pane(&mut app).0.bottom,
            docked_bottom(LOG_TOP - LOG_RAISE),
            "a closed log leaves it in place"
        );
    }

    /// A docked window too tall for the room above the log sits at the dock floor.
    #[test]
    fn a_tall_docked_window_stops_at_the_dock_floor() {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::EmoteList);
        let (mut app, _) = dock_app(
            stack,
            &[(EMOTE_LIST_WINDOW, (16, 120, 100, 200), FRAME_ANCHOR_NONE)],
        );
        let pane = app
            .world_mut()
            .query_filtered::<Entity, With<MainMenuPane>>()
            .single(app.world())
            .expect("one pane");
        app.world_mut().entity_mut(pane).insert(ComputedNode {
            size: Vec2::new(MENU_PANE_WIDTH, LOG_TOP),
            inverse_scale_factor: 1.0,
            ..default()
        });
        app.update();
        let (pane, _) = drawn_pane(&mut app);
        assert_eq!(
            (pane.top, pane.bottom),
            (Val::Px(crate::hud::dock::DOCK_MIN_TOP), Val::Auto)
        );
    }

    /// Two open windows in the log slot share one bottom line above the log, each at its own
    /// record x: the Mog House menu under Change Jobs.
    #[test]
    fn windows_in_one_slot_share_the_line_above_the_log() {
        const MYROOM_X: i16 = 16;
        const JOBCHANG_X: i16 = 150;
        let mut stack = MenuStack::root();
        stack.push(MenuKind::HomeView);
        stack.push(MenuKind::JobChange);
        let (mut app, _) = dock_app(
            stack,
            &[
                (MYROOM_WINDOW, (MYROOM_X, 100, 120, 150), FRAME_ANCHOR_NONE),
                (
                    JOBCHANG_WINDOW,
                    (JOBCHANG_X, 60, 200, 100),
                    FRAME_ANCHOR_NONE,
                ),
            ],
        );
        app.update();
        let (pane, _) = drawn_pane(&mut app);
        assert_eq!(
            (pane.left, pane.bottom),
            (Val::Px(f32::from(JOBCHANG_X)), docked_bottom(LOG_TOP))
        );
        let stacked = drawn_stack_windows(&mut app);
        assert_eq!(stacked.len(), 1, "the Mog House menu stays drawn");
        assert_eq!(
            (stacked[0].node.left, stacked[0].node.bottom),
            (Val::Px(f32::from(MYROOM_X)), docked_bottom(LOG_TOP))
        );
    }

    /// A window the record does not dock keeps its record frame beside a chat log, and a docking
    /// window with no log to dock against opens at its record frame.
    #[test]
    fn undocked_windows_and_windows_without_an_anchor_keep_their_record_frames() {
        const QUEST_Y: i16 = 48;
        const EMOTE_Y: i16 = 120;
        let mut stack = MenuStack::root();
        stack.push(MenuKind::QuestList);
        let (mut app, log) = dock_app(
            stack,
            &[
                (
                    crate::hud::unity_fp::QUEST00_WINDOW,
                    (16, QUEST_Y, 366, 200),
                    FRAME_ANCHOR_NONE,
                ),
                (
                    EMOTE_LIST_WINDOW,
                    (16, EMOTE_Y, 100, 200),
                    FRAME_ANCHOR_NONE,
                ),
            ],
        );
        app.update();
        let (pane, _) = drawn_pane(&mut app);
        assert_eq!(
            (pane.top, pane.bottom),
            (Val::Px(f32::from(QUEST_Y)), Val::Auto)
        );

        app.world_mut().entity_mut(log).despawn();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::EmoteList);
        app.insert_resource(InputMode::Menu(stack));
        app.update();
        let (pane, _) = drawn_pane(&mut app);
        assert_eq!(
            (pane.top, pane.bottom),
            (Val::Px(f32::from(EMOTE_Y)), Val::Auto)
        );
    }

    /// Status code-moves `joblevel` to (130, 48) whatever its record says, keeping the record
    /// width; the other details sit at their record frames.
    #[test]
    fn joblevel_opened_from_status_sits_at_its_code_move() {
        use crate::hud::status_panel::{
            JOBLEVEL_STATUS_LEFT, JOBLEVEL_STATUS_TOP, JOBLEVEL_WINDOW,
        };
        const JOBLEVEL_FRAME: (i16, i16, i16) = (200, 100, 180);
        const PROFILE_FRAME: (i16, i16, i16) = (150, 60, 240);
        const PROFILE_ENTRY: u8 = 0;
        let entry_of = |window: &str| {
            STATUS_DETAIL_WINDOW_NAMES
                .iter()
                .position(|name| *name == Some(window))
                .and_then(|entry| u8::try_from(entry).ok())
                .expect("a Status detail")
        };
        let profile_window =
            STATUS_DETAIL_WINDOW_NAMES[usize::from(PROFILE_ENTRY)].expect("profile is a window");
        let mut app = App::new();
        app.init_resource::<ConfigDatRows>()
            .init_resource::<DynamicMenu>()
            .add_systems(Startup, spawn_status_detail_window)
            .add_systems(Update, update_status_detail_window);
        {
            let mut rows = app.world_mut().resource_mut::<ConfigDatRows>();
            rows.frames.insert(JOBLEVEL_WINDOW.into(), JOBLEVEL_FRAME);
            rows.frames.insert(profile_window.into(), PROFILE_FRAME);
        }
        let detail = |app: &mut App, entry: u8| -> (Val, Val, Val) {
            let mut stack = MenuStack::root();
            stack.push(MenuKind::Status);
            stack.push(MenuKind::StatusDetail { entry });
            app.insert_resource(InputMode::Menu(stack));
            app.update();
            let node = app
                .world_mut()
                .query_filtered::<&Node, With<StatusDetailWindow>>()
                .single(app.world())
                .expect("the detail window");
            (node.left, node.top, node.width)
        };
        assert_eq!(
            detail(&mut app, entry_of(JOBLEVEL_WINDOW)),
            (
                Val::Px(JOBLEVEL_STATUS_LEFT),
                Val::Px(JOBLEVEL_STATUS_TOP),
                Val::Px(f32::from(JOBLEVEL_FRAME.2))
            )
        );
        assert_eq!(
            detail(&mut app, PROFILE_ENTRY),
            (
                Val::Px(f32::from(PROFILE_FRAME.0)),
                Val::Px(f32::from(PROFILE_FRAME.1)),
                Val::Px(f32::from(PROFILE_FRAME.2))
            )
        );
    }
}
