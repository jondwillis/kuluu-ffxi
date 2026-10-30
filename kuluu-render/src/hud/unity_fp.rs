//! The Unity windows (`ut_menu` chain, Status row id 11) and the alter ego point windows
//! (`fp_cat` chain, Status row id 13): request rules, row builders, refusals and the text each
//! window draws, all from wire state and DAT text
//! (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).

use std::collections::BTreeMap;

use ffxi_dat::menu_windows::{ItemBlock, MenuWindow};
use kuluu_snapshot::{SceneSnapshot, UnityRankingSet, UNITY_CONCORDS, UNITY_RESULT_SETS};

use crate::hud::gettext::{
    format_template, ConfirmPrompt, GetText, GetTextTables, PromptArg, GETTEXT_TABLE_10,
    GETTEXT_TABLE_12, GETTEXT_TABLE_9, GETTEXT_TABLE_CAPTIONS, GETTEXT_TABLE_CLIENT_MESSAGES,
    GETTEXT_TABLE_SHORT_CAPTIONS, GETTEXT_TABLE_SUB_TEMPLATES,
};
use crate::hud::menu::{DynamicMenu, DynamicMenuAction, DynamicMenuRow, UiSound};
use crate::input_mode::{ConfirmDefault, MenuKind, MenuLevel, MenuStack};

pub const UT_MENU_WINDOW: &str = "ut_menu";
pub const UT_POINT_WINDOW: &str = "ut_point";
pub const QUEST00_WINDOW: &str = "quest00";
pub const QUEST01_WINDOW: &str = "quest01";
pub const AREALIST_WINDOW: &str = "arealist";
pub const TITLEWIN_WINDOW: &str = "titlewin";
pub const FP_CAT_WINDOW: &str = "fp_cat";
pub const FP_CAT2_WINDOW: &str = "fp_cat2";
pub const FP_INFO_WINDOW: &str = "fp_info";
pub const BLUEHELP_WINDOW: &str = "bluehelp";
pub const MERITYN_WINDOW: &str = "merityn";

/// Every window record these windows draw from.
pub const UNITY_FP_WINDOWS: [&str; 11] = [
    UT_MENU_WINDOW,
    UT_POINT_WINDOW,
    QUEST00_WINDOW,
    QUEST01_WINDOW,
    AREALIST_WINDOW,
    TITLEWIN_WINDOW,
    FP_CAT_WINDOW,
    FP_CAT2_WINDOW,
    FP_INFO_WINDOW,
    BLUEHELP_WINDOW,
    MERITYN_WINDOW,
];

/// Client option store key holding the Unity chat mode.
pub const UNITY_CHAT_OPTION: u32 = 178;

/// `ut_menu` DAT items 1..=3: the chat panel, the ranking chooser, the leader list.
pub const UT_MENU_ITEM_CHAT: u8 = 1;
pub const UT_MENU_ITEM_RANKING: u8 = 2;
pub const UT_MENU_ITEM_LEADERS: u8 = 3;
/// `ut_menu`'s rows, in order.
pub const UT_MENU_ROWS: [u8; 3] = [
    UT_MENU_ITEM_CHAT,
    UT_MENU_ITEM_RANKING,
    UT_MENU_ITEM_LEADERS,
];

/// `merityn` DAT item 1 answers Yes, item 2 No.
pub const CONFIRM_ITEM_YES: u8 = 1;
pub const CONFIRM_ITEM_NO: u8 = 2;

/// Main job 23 (MON) counts as Monstrosity for the alter ego point refusal.
pub const MONSTROSITY_MAIN_JOB: u8 = 23;

pub const UT_POINT_MAX: u32 = 99_999;
pub const UNITY_VALUE_CAP: u32 = 999_999;
pub const ALTER_EGO_POINT_MAX: u16 = 1000;

/// GetText table 7 entry printed when the alter ego point upgrades are not allowed here.
pub const NOT_ALLOWED_MESSAGE_ID: u16 = 178;

/// GetText table 0x0E entry every Unity leader list row shows as its help line.
pub const LEADER_HELP_ID: u16 = 892;

/// GetText table 12 entries.
pub mod text12 {
    pub const CHAT_TITLE: u16 = 395;
    pub const RANKING_TITLE: u16 = 396;
    /// Chat panel labels at y 43, 68, 93, 118, 143.
    pub const CHAT_LABELS: [u16; 5] = [397, 398, 399, 452, 453];
    /// `402 + u` names Concord u; 402 itself stands for no Concord.
    pub const CONCORD_NAME_BASE: u16 = 402;
    /// `400 + r`: the chooser's row r and the ranking list's header for result set r.
    pub const RANKING_SET_BASE: u16 = 400;
    /// Rank 0; `434 + rank` for ranks 1..=3.
    pub const RANK_TEXT_BASE: u16 = 434;
    /// printf template for ranks 4 and up.
    pub const RANK_TEMPLATE: u16 = 438;
    pub const CHAT_FOOTER: u16 = 439;
    /// `440 + r`: the chat panel's row r.
    pub const CHAT_ROW_BASE: u16 = 440;
    /// The leader list's row 0, the player's own Concord.
    pub const OWN_CONCORD_ROW: u16 = 442;
    /// The `fp_cat2` cost cell caption.
    pub const COST_CAPTION: u16 = 90;
}

/// GetText table 9 entries the ranking views show until their data is ready.
pub const NOT_READY_LINES: [u16; 2] = [97, 98];

/// Ranks 1..=3 have their own GetText entries.
pub const RANK_WITH_OWN_TEXT_MAX: u32 = 3;

/// Retail text colors are ARGB words whose 0x80 channels mean full strength.
pub const fn retail_argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    u32::from_be_bytes([a, r, g, b])
}

pub const ARGB_NEUTRAL: u32 = retail_argb(0x80, 0x80, 0x80, 0x80);
pub const ARGB_DIM: u32 = retail_argb(0x80, 0x40, 0x40, 0x40);
pub const ARGB_CAPPED: u32 = retail_argb(0x80, 0x20, 0x60, 0xA0);
pub const ARGB_FP_CURSOR: u32 = retail_argb(0x80, 0x80, 0x30, 0x40);

const RETAIL_FULL_CHANNEL: f32 = 128.0;

/// A retail ARGB word as a HUD color, scaled so neutral white is the theme's body text.
pub fn retail_text_color(argb: u32) -> bevy::prelude::Color {
    let [_, r, g, b] = argb.to_be_bytes();
    let text = crate::hud::style::theme::TEXT.to_srgba();
    let channel = |value: u8, base: f32| (f32::from(value) / RETAIL_FULL_CHANNEL * base).min(1.0);
    bevy::prelude::Color::srgb(
        channel(r, text.red),
        channel(g, text.green),
        channel(b, text.blue),
    )
}

/// Where one text starts: its left edge, or its right edge when right-aligned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Right,
}

/// One string a window draws, in window-relative design units.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelText {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub align: TextAlign,
    pub color: u32,
    /// Wrap at this width instead of running on one line.
    pub wrap_width: Option<f32>,
    /// Wrapped lines this far apart instead of the font's own line height.
    pub line_pitch: Option<f32>,
    /// A second run drawn straight after `text`, in its own color.
    pub suffix: Option<(String, u32)>,
}

impl PanelText {
    pub fn left(text: impl Into<String>, x: f32, y: f32, color: u32) -> Self {
        Self {
            text: text.into(),
            x,
            y,
            align: TextAlign::Left,
            color,
            wrap_width: None,
            line_pitch: None,
            suffix: None,
        }
    }

    pub fn right(text: impl Into<String>, x: f32, y: f32, color: u32) -> Self {
        Self {
            align: TextAlign::Right,
            ..Self::left(text, x, y, color)
        }
    }

    pub fn with_suffix(self, suffix: impl Into<String>, color: u32) -> Self {
        Self {
            suffix: Some((suffix.into(), color)),
            ..self
        }
    }

    /// The whole line as drawn.
    pub fn full_text(&self) -> String {
        match &self.suffix {
            Some((suffix, _)) => format!("{}{suffix}", self.text),
            None => self.text.clone(),
        }
    }
}

/// Layout of the windows, in window-relative design units.
pub mod layout {
    /// `ut_point`: right edge and top of its readout.
    pub const UT_POINT_RIGHT: f32 = 100.0;
    pub const UT_POINT_TOP: f32 = 20.0;
    /// `fp_info`: its readout ends this far left of the window's right edge, this far down.
    pub const FP_INFO_RIGHT_INSET: f32 = 8.0;
    pub const FP_INFO_TOP: f32 = 20.0;

    pub const CHAT_TITLE: (f32, f32) = (80.0, 15.0);
    pub const CHAT_FOOTER: (f32, f32) = (20.0, 168.0);
    pub const CHAT_LABEL_X: f32 = 20.0;
    pub const CHAT_LINE_YS: [f32; 5] = [43.0, 68.0, 93.0, 118.0, 143.0];
    pub const CHAT_FACTION_X: f32 = 180.0;
    pub const CHAT_RANK_X: f32 = 220.0;
    pub const CHAT_VALUE_RIGHT: f32 = 350.0;
    /// Bottom edges of the chat panel's rows (items 1 and 2) above the window's bottom edge.
    pub const CHAT_ROW_BOTTOM_INSETS: [f32; 2] = [35.0, 12.0];
    pub const CHAT_ROW_TEXT_X: f32 = 24.0;
    /// The chosen row's check sprite and the other row's bullet, at these item offsets.
    pub const CHECK_SPRITE_AT: (f32, f32) = (0.0, 1.0);
    pub const BULLET_SPRITE_AT: (f32, f32) = (4.0, 5.0);

    pub const CHOOSER_HEADER: (f32, f32) = (80.0, 15.0);
    /// The chooser's rows (items 1 and 2) span these design-space tops, each this tall.
    pub const CHOOSER_ROW_TOPS: [f32; 2] = [100.0, 130.0];
    pub const CHOOSER_ROW_HEIGHT: f32 = 18.0;
    pub const CHOOSER_ROW_TEXT_X: f32 = 16.0;

    pub const RANKING_HEADER: (f32, f32) = (60.0, 10.0);
    pub const RANKING_NOT_READY: [(f32, f32); 2] = [(20.0, 45.0), (20.0, 73.0)];
    pub const CHAT_NOT_READY: [(f32, f32); 2] = [(20.0, 43.0), (20.0, 68.0)];
    pub const RANKING_ROWS_PER_PAGE: usize = 8;
    pub const RANKING_RANK_X: f32 = 16.0;
    pub const RANKING_NAME_X: f32 = 58.0;
    pub const RANKING_VALUE_RIGHT: f32 = 250.0;

    pub const LEADER_ROWS: usize = 12;
    pub const LEADER_VISIBLE_ROWS: usize = 5;
    pub const LEADER_ROW_PITCH: f32 = 16.0;
    /// Name, tag, and the two columns the record leaves undescribed.
    pub const LEADER_COLUMNS_X: [f32; 4] = [20.0, 245.0, 320.0, 380.0];

    /// The `fp_cat` / `fp_cat2` list: name, level (right edge), `/cap`, cost caption, cost
    /// (right edge).
    pub const FP_NAME_X: f32 = 6.0;
    pub const FP_LEVEL_RIGHT: f32 = 252.0;
    pub const FP_CAP_X: f32 = 253.0;
    pub const FP_COST_CAPTION_X: f32 = 290.0;
    pub const FP_COST_RIGHT: f32 = 357.0;
    pub const FP_ROW_PITCH: f32 = 17.0;
    pub const FP_CAT_FIRST_ROW_TOP: f32 = 4.0;
    pub const FP_CAT2_FIRST_ROW_TOP: f32 = 17.0;
    pub const FP_TEXT_DROP: f32 = 2.0;
    pub const FP_VISIBLE_ROWS: usize = 10;

    /// Kuluu tuning, not record data: inset of the text kuluu draws where the record names no
    /// position (title bar, description box), and the line pitch below it.
    pub const KULUU_TEXT_INSET: f32 = 8.0;
    pub const KULUU_LINE_PITCH: f32 = 16.0;
    /// Kuluu's pointer stands this far left of a row that has no record cursor color.
    pub const KULUU_POINTER_INDENT: f32 = 14.0;

    /// `merityn`'s prompt text starts this far right of and below the window's left and top, wraps
    /// to the window width less [`MERITYN_WRAP_MARGIN`], and keeps its lines
    /// [`MERITYN_LINE_PITCH`] apart
    /// (.agents/skills/retail-observe/references/2026-10-15-gettext-tables.md).
    pub const MERITYN_TEXT_INSET: f32 = 4.0;
    pub const MERITYN_WRAP_MARGIN: f32 = 8.0;
    pub const MERITYN_LINE_PITCH: f32 = 16.0;
}

/// Kuluu's stand-ins for retail sprites whose atlases the record leaves unread: the quest
/// window's check mark and the ordinary row bullet.
pub const CHECK_SPRITE_STAND_IN: &str = "*";
pub const BULLET_SPRITE_STAND_IN: &str = "-";

/// Kuluu's pointer for rows the record gives no cursor color.
pub const POINTER_STAND_IN: &str = ">";

/// `"%d/%d"`, `"/%d"`, `"%dpt"` and `"%d P"` are client format strings.
pub fn ut_point_text(snap: &SceneSnapshot) -> String {
    if !snap.status_data_ready {
        return format!("/{UT_POINT_MAX}");
    }
    let points = snap.stats.map_or(0, |s| s.unity_points);
    format!("{points}/{UT_POINT_MAX}")
}

pub fn points_text(value: u32) -> String {
    format!("{}pt", value.min(UNITY_VALUE_CAP))
}

pub fn cost_text(cost: u32) -> String {
    format!("{cost} P")
}

/// `fp_info`'s readout and its color: blue once the balance reaches the cap.
pub fn fp_info_text(snap: &SceneSnapshot) -> (String, u32) {
    let points = snap.alter_ego_points.map_or(0, |p| p.points);
    let color = if points >= ALTER_EGO_POINT_MAX {
        ARGB_CAPPED
    } else {
        ARGB_NEUTRAL
    };
    (format!("{points}/{ALTER_EGO_POINT_MAX}"), color)
}

/// Concord u's value in one result set: points / members, 0 with no members. Index u - 1.
pub fn concord_values(set: &UnityRankingSet) -> [u32; UNITY_CONCORDS] {
    let mut values = [0u32; UNITY_CONCORDS];
    for (concord, value) in values.iter_mut().enumerate() {
        let members = set.members[concord];
        *value = set.points[concord].checked_div(members).unwrap_or(0);
    }
    values
}

/// Competition ranks of `values` (ties share a rank and the next rank skips), all 0 when every
/// value is 0. Index u - 1.
pub fn competition_ranks(values: &[u32; UNITY_CONCORDS]) -> [u32; UNITY_CONCORDS] {
    let mut ranks = [0u32; UNITY_CONCORDS];
    if values.iter().all(|&v| v == 0) {
        return ranks;
    }
    for (concord, rank) in ranks.iter_mut().enumerate() {
        let better = values.iter().filter(|&&v| v > values[concord]).count();
        *rank = u32::try_from(better).unwrap_or(u32::MAX) + 1;
    }
    ranks
}

/// One Concord's place in a ranking result set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConcordStanding {
    /// 1..=11.
    pub concord: u8,
    pub rank: u32,
    pub value: u32,
}

/// The ranking list: Concords 1..=11 by value, highest first, ties keeping the lower Concord
/// first.
pub fn ranking_list(set: &UnityRankingSet) -> Vec<ConcordStanding> {
    let values = concord_values(set);
    let ranks = competition_ranks(&values);
    let mut list: Vec<ConcordStanding> = (0..UNITY_CONCORDS)
        .map(|i| ConcordStanding {
            concord: concord_number(i),
            rank: ranks[i],
            value: values[i],
        })
        .collect();
    list.sort_by(|a, b| b.value.cmp(&a.value).then(a.concord.cmp(&b.concord)));
    list
}

/// Concord number (1-based) of a 0-based table index.
fn concord_number(index: usize) -> u8 {
    u8::try_from(index + 1).unwrap_or(u8::MAX)
}

/// One Concord's standing in a set, `None` for no Concord or one outside 1..=11.
pub fn concord_standing(set: &UnityRankingSet, concord: u8) -> Option<ConcordStanding> {
    let index = usize::from(concord).checked_sub(1)?;
    let values = concord_values(set);
    let ranks = competition_ranks(&values);
    Some(ConcordStanding {
        concord,
        rank: *ranks.get(index)?,
        value: values[index],
    })
}

/// The ranking views wait for the Status data and both result sets.
pub fn unity_ranking_ready(snap: &SceneSnapshot) -> bool {
    snap.status_data_ready && snap.unity.rankings.iter().all(|set| set.complete)
}

/// The rank's text: its own entry for ranks 0..=3, the template entry filled with the rank
/// beyond.
pub fn rank_text(texts: &impl GetText, rank: u32) -> Option<String> {
    if rank <= RANK_WITH_OWN_TEXT_MAX {
        let id = text12::RANK_TEXT_BASE + u16::try_from(rank).ok()?;
        return texts.get_text(GETTEXT_TABLE_12, id).map(str::to_string);
    }
    let template = texts.get_text(GETTEXT_TABLE_12, text12::RANK_TEMPLATE)?;
    Some(format_template(template, &[&rank.to_string()]))
}

fn concord_name_id(concord: u8) -> u16 {
    text12::CONCORD_NAME_BASE + u16::from(concord)
}

/// What a window asks the session to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnityFpRequest {
    /// c2s 0x116 for one result set.
    UnityRanking(u8),
    /// c2s 0x117.
    UnityQuest,
    /// c2s 0x118 with a Unity chat mode.
    UnityChat(u8),
    /// c2s 0x0C2.
    AlterEgoPoints,
    /// c2s 0x0C1 for one upgrade id.
    UpgradeAlterEgo(u16),
}

/// `ut_menu` open: one c2s 0x116 per result set, set 0 first, whose table completed since its
/// last request.
pub fn unity_ranking_requests(snap: &SceneSnapshot) -> Vec<UnityFpRequest> {
    (0..UNITY_RESULT_SETS)
        .filter(|&set| snap.unity.rankings[set].complete)
        .filter_map(|set| u8::try_from(set).ok())
        .map(UnityFpRequest::UnityRanking)
        .collect()
}

/// `quest01` open: c2s 0x117 once an s2c 0x110 arrived since the last one.
pub fn unity_quest_request_due(snap: &SceneSnapshot) -> bool {
    snap.unity.quest_armed
}

/// `fp_cat` / `fp_cat2` open: c2s 0x0C2 while no s2c 0x08E has arrived this session.
pub fn alter_ego_request_due(snap: &SceneSnapshot) -> bool {
    snap.alter_ego_points.is_none()
}

/// Marks a sent request in the local snapshot until the session's fold catches up, so a quick
/// reopen cannot send it twice.
fn note_sent(snap: &mut SceneSnapshot, request: UnityFpRequest) {
    match request {
        UnityFpRequest::UnityRanking(set) => {
            if let Some(set) = snap.unity.rankings.get_mut(usize::from(set)) {
                set.complete = false;
            }
        }
        UnityFpRequest::UnityQuest => snap.unity.quest_armed = false,
        _ => {}
    }
}

/// The latest s2c 0x00A `LoginState` puts the character in a Mog House.
pub fn in_mog_house(snap: &SceneSnapshot) -> bool {
    ffxi_proto::decode::ServerLoginMyroom::login_in_mog_house(snap.login_state)
}

/// s2c 0x00A `IsMonstrosity` was set, or s2c 0x061 names main job 23.
pub fn in_monstrosity(snap: &SceneSnapshot) -> bool {
    snap.login_is_monstrosity
        || snap
            .stats
            .is_some_and(|s| s.mjob_no == MONSTROSITY_MAIN_JOB)
}

/// Upgrades are allowed in a Mog House outside Monstrosity.
pub fn alter_ego_allowed(snap: &SceneSnapshot) -> bool {
    in_mog_house(snap) && !in_monstrosity(snap)
}

/// One `fp_cat2` row's state, which picks its colors and its select outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeRowState {
    /// Not allowed here, or no s2c 0x08E yet.
    Unavailable,
    AtCap,
    Affordable,
    TooCostly,
}

pub fn upgrade_row_state(snap: &SceneSnapshot, upgrade: u16, cap: u8) -> UpgradeRowState {
    let Some(points) = snap.alter_ego_points.filter(|_| alter_ego_allowed(snap)) else {
        return UpgradeRowState::Unavailable;
    };
    let slot = usize::from(upgrade);
    let level = points.upgrades.get(slot).copied().unwrap_or(0);
    if level >= cap {
        return UpgradeRowState::AtCap;
    }
    let cost = points.costs.get(slot).copied().unwrap_or(0);
    if points.points >= cost {
        UpgradeRowState::Affordable
    } else {
        UpgradeRowState::TooCostly
    }
}

/// Name, level/cap and cost-cell colors of one `fp_cat2` row; `None` hides the cost cell.
pub fn upgrade_row_colors(state: UpgradeRowState) -> (u32, u32, Option<u32>) {
    match state {
        UpgradeRowState::Unavailable => (ARGB_DIM, ARGB_DIM, None),
        UpgradeRowState::AtCap => (ARGB_NEUTRAL, ARGB_CAPPED, None),
        UpgradeRowState::Affordable => (ARGB_NEUTRAL, ARGB_NEUTRAL, Some(ARGB_NEUTRAL)),
        UpgradeRowState::TooCostly => (ARGB_NEUTRAL, ARGB_NEUTRAL, Some(ARGB_DIM)),
    }
}

/// What selecting an `fp_cat2` row does, first match winning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeSelect {
    /// No valid row: `fp_cat2` closes.
    Close,
    /// Not allowed: the GetText table 7 refusal message, then the buzzer.
    RefuseWithMessage,
    /// At cap, no data yet, or too costly: the buzzer.
    Buzzer,
    /// The confirm sound, then the yes/no prompt.
    Confirm,
}

pub fn upgrade_select(snap: &SceneSnapshot, row: Option<(u16, u8)>) -> UpgradeSelect {
    let Some((upgrade, cap)) = row else {
        return UpgradeSelect::Close;
    };
    if !alter_ego_allowed(snap) {
        return UpgradeSelect::RefuseWithMessage;
    }
    match upgrade_row_state(snap, upgrade, cap) {
        UpgradeRowState::Affordable => UpgradeSelect::Confirm,
        _ => UpgradeSelect::Buzzer,
    }
}

/// The alter ego point tables, read from the install.
#[derive(Debug, Clone, Default)]
pub struct AlterEgoDat {
    pub categories: Vec<ffxi_dat::alter_ego::AlterEgoCategory>,
    pub upgrade_records: Vec<u8>,
    /// Names and descriptions by string entry.
    pub strings: Vec<String>,
}

/// One install file by file id, `None` when it does not resolve or read.
pub fn read_dat_file(dat: &ffxi_dat::archive::DatRoot, file_id: u32) -> Option<Vec<u8>> {
    let loc = dat.resolve(file_id).ok()?;
    std::fs::read(loc.path_under(dat)).ok()
}

/// Every string of the d_msg catalog `file_id`, empty when it does not read.
pub fn read_dmsg_strings(dat: &ffxi_dat::archive::DatRoot, file_id: u32) -> Vec<String> {
    read_dat_file(dat, file_id)
        .and_then(|b| ffxi_dat::dmsg::DmsgTable::parse(&b).ok())
        .map(|catalog| crate::hud::gettext::dmsg_strings(&catalog))
        .unwrap_or_default()
}

/// The install's window records named in `names`.
pub fn load_record_windows(
    dat: &ffxi_dat::archive::DatRoot,
    names: &[&str],
) -> BTreeMap<String, MenuWindow> {
    ffxi_dat::menu_windows::windows_from_root(dat)
        .map(|mut all| {
            all.retain(|name, _| names.contains(&name.as_str()));
            all
        })
        .unwrap_or_default()
}

impl AlterEgoDat {
    pub fn load(dat: &ffxi_dat::archive::DatRoot) -> Self {
        Self {
            categories: read_dat_file(dat, ffxi_dat::alter_ego::CATEGORY_TABLE_FILE_ID)
                .map(|b| ffxi_dat::alter_ego::parse_categories(&b))
                .unwrap_or_default(),
            upgrade_records: read_dat_file(dat, ffxi_dat::alter_ego::UPGRADE_TABLE_FILE_ID)
                .unwrap_or_default(),
            strings: read_dmsg_strings(dat, ffxi_dat::alter_ego::STRINGS_FILE_ID_ENGLISH),
        }
    }

    pub fn name(&self, id: u16) -> Option<&str> {
        self.strings
            .get(ffxi_dat::alter_ego::name_entry(id))
            .map(String::as_str)
    }

    pub fn description(&self, id: u16) -> Option<&str> {
        self.strings
            .get(ffxi_dat::alter_ego::description_entry(id))
            .map(String::as_str)
    }

    pub fn cap(&self, upgrade: u16) -> u8 {
        ffxi_dat::alter_ego::upgrade_cap(&self.upgrade_records, upgrade).unwrap_or(0)
    }

    /// `fp_cat`'s rows: records in order whose category id and first upgrade id are both set.
    pub fn listed_categories(&self) -> Vec<u16> {
        self.categories
            .iter()
            .filter(|c| c.id != 0 && c.upgrades[0] != 0)
            .map(|c| c.id)
            .collect()
    }

    /// `fp_cat2`'s upgrade ids for a category, skipping empty slots. The category id doubles as
    /// the record index.
    pub fn category_upgrades(&self, category: u16) -> Vec<u16> {
        self.categories
            .get(usize::from(category))
            .map(|c| c.upgrades.iter().copied().filter(|&u| u != 0).collect())
            .unwrap_or_default()
    }
}

/// Everything these windows read from the install: window records, GetText tables and the
/// alter ego point tables. Loaded once; an install that lacks a piece leaves it empty.
#[derive(bevy::prelude::Resource, Debug, Default)]
pub struct UnityFpDat {
    loaded: bool,
    pub windows: BTreeMap<String, MenuWindow>,
    pub texts: GetTextTables,
    pub alter_ego: AlterEgoDat,
}

impl UnityFpDat {
    pub fn ensure_loaded(&mut self, dat: Option<&ffxi_dat::archive::DatRoot>) {
        if self.loaded {
            return;
        }
        let Some(dat) = dat else { return };
        self.windows = load_record_windows(dat, &UNITY_FP_WINDOWS);
        self.texts = GetTextTables::load(
            dat,
            &[
                GETTEXT_TABLE_SUB_TEMPLATES,
                GETTEXT_TABLE_CLIENT_MESSAGES,
                GETTEXT_TABLE_9,
                GETTEXT_TABLE_10,
                GETTEXT_TABLE_12,
                GETTEXT_TABLE_SHORT_CAPTIONS,
                GETTEXT_TABLE_CAPTIONS,
            ],
        );
        self.alter_ego = AlterEgoDat::load(dat);
        self.loaded = true;
    }

    pub fn window(&self, name: &str) -> Option<&MenuWindow> {
        self.windows.get(name)
    }

    /// The item with DAT item id `id` in a window record.
    pub fn item(&self, window: &str, id: u8) -> Option<&ItemBlock> {
        item_by_id(self.window(window)?, id)
    }

    /// Which `ut_menu` row is the record's last item; `None` until the records load.
    pub fn menu_last_item_row(&self) -> Option<crate::input_mode::LastItemRow> {
        use crate::input_mode::LastItemRow;
        if !self.loaded {
            return None;
        }
        let Some(window) = self.window(UT_MENU_WINDOW) else {
            return Some(LastItemRow::Undrawn);
        };
        Some(
            UT_MENU_ROWS
                .iter()
                .position(|&item| item == window.item_count)
                .map_or(LastItemRow::Undrawn, LastItemRow::Row),
        )
    }

    /// An item's own label: its short caption, else its caption-catalog text.
    pub fn item_label(&self, item: &ItemBlock) -> Option<&str> {
        item_label(&self.texts, item)
    }
}

pub fn item_by_id(window: &MenuWindow, id: u8) -> Option<&ItemBlock> {
    window
        .items
        .iter()
        .flatten()
        .find(|item| item.id == i16::from(id))
}

pub fn item_label<'a>(texts: &'a impl GetText, item: &ItemBlock) -> Option<&'a str> {
    let id = |raw: Option<i32>| raw.and_then(|v| u16::try_from(v).ok());
    id(item.text_b_id)
        .and_then(|i| texts.get_text(GETTEXT_TABLE_SHORT_CAPTIONS, i))
        .or_else(|| id(item.text_a_id).and_then(|i| texts.get_text(GETTEXT_TABLE_CAPTIONS, i)))
}

/// The line printed when DAT text for a client message is missing: the id, never invented prose.
pub fn client_message_fallback(table: u8, id: u16) -> String {
    format!("[client message {table}:{id}]")
}

/// Whole retail frames out of wall time, the fraction carried to the next call.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RetailFrameCarry {
    carry: f32,
}

impl RetailFrameCarry {
    pub fn frames(&mut self, seconds: f32) -> u32 {
        let frames = self.carry + seconds * crate::scheduler_runtime::RETAIL_FPS;
        let whole = frames.floor();
        self.carry = frames - whole;
        // A frame-time spike saturates instead of wrapping.
        whole as u32
    }
}

/// The Unity chat panel's mode and its send throttle: a change goes out the next frame the
/// countdown is at zero, which then restarts at [`UNITY_CHAT_SEND_FRAMES`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnityChatToggle {
    pub mode: u8,
    last_sent: u8,
    countdown: u32,
    frame_carry: RetailFrameCarry,
}

/// Retail frames between two c2s 0x118 sends from the chat panel.
pub const UNITY_CHAT_SEND_FRAMES: u32 = 30;

impl UnityChatToggle {
    /// Opens on the stored option; the opening value counts as already sent.
    pub fn open(stored: i32) -> Self {
        let mode = u8::from(stored != 0);
        Self {
            mode,
            last_sent: mode,
            countdown: 0,
            frame_carry: RetailFrameCarry::default(),
        }
    }

    /// Advances `frames` retail frames; returns the mode to send, if one is due.
    pub fn tick(&mut self, frames: u32) -> Option<u8> {
        if frames == 0 {
            return None;
        }
        self.countdown = self.countdown.saturating_sub(frames);
        if self.countdown != 0 || self.mode == self.last_sent {
            return None;
        }
        self.last_sent = self.mode;
        self.countdown = UNITY_CHAT_SEND_FRAMES;
        Some(self.mode)
    }

    /// [`Self::tick`] over `seconds` of wall time at the retail frame rate.
    pub fn advance(&mut self, seconds: f32) -> Option<u8> {
        let frames = self.frame_carry.frames(seconds);
        self.tick(frames)
    }
}

/// State the Unity and alter ego point windows keep while open, held on [`DynamicMenu`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UnityFpWindows {
    /// The Unity chat panel's mode, while it is open.
    pub chat: Option<UnityChatToggle>,
    /// The chat panel's rank and value of the player's Concord in result sets 0 and 1, frozen on
    /// the first ready frame after the panel opens.
    pub chat_standings: Option<[Option<ConcordStanding>; UNITY_RESULT_SETS]>,
    /// The ranking list's rows and the result set they were built for; built once while ready.
    pub ranking: Option<(u8, Vec<ConcordStanding>)>,
    /// The GetText table 7 refusal line, copied from DAT for the input path.
    pub not_allowed_message: Option<String>,
    /// The Unity leader list's help line, copied from DAT for the help bar.
    pub leader_help: Option<String>,
    /// First visible row of each scrolling list, by window.
    pub list_starts: Vec<(MenuKind, usize)>,
}

impl UnityFpWindows {
    pub fn list_start(&self, kind: MenuKind) -> usize {
        self.list_starts
            .iter()
            .find(|(k, _)| *k == kind)
            .map_or(0, |(_, start)| *start)
    }

    /// Scrolls a list the least that keeps the cursor among its `visible` rows.
    pub fn scroll_to(&mut self, kind: MenuKind, cursor: usize, visible: usize) -> usize {
        let start = scrolled_start(self.list_start(kind), cursor, visible);
        match self.list_starts.iter_mut().find(|(k, _)| *k == kind) {
            Some(entry) => entry.1 = start,
            None => self.list_starts.push((kind, start)),
        }
        start
    }
}

/// The first visible row after scrolling the least that keeps `cursor` visible.
pub fn scrolled_start(start: usize, cursor: usize, visible: usize) -> usize {
    let visible = visible.max(1);
    if cursor < start {
        cursor
    } else if cursor >= start + visible {
        cursor + 1 - visible
    } else {
        start
    }
}

fn is_open(stack: Option<&MenuStack>, wanted: impl Fn(MenuKind) -> bool) -> bool {
    stack.is_some_and(|s| s.levels.iter().any(|level| wanted(level.kind)))
}

/// Opens, freezes and closes per-window state to match the open windows. A closed chat panel
/// stores its mode into the client option store even when the last change was not sent yet.
pub fn sync_unity_fp_windows(
    dynamic: &mut DynamicMenu,
    stack: Option<&MenuStack>,
    snap: &SceneSnapshot,
) {
    let chat_open = is_open(stack, |k| k == MenuKind::UnityChat);
    match (chat_open, dynamic.unity_fp.chat) {
        (true, None) => {
            let stored = dynamic.config_query(UNITY_CHAT_OPTION, false);
            dynamic.unity_fp.chat = Some(UnityChatToggle::open(stored));
        }
        (false, Some(chat)) => {
            dynamic
                .config_option_values
                .insert(UNITY_CHAT_OPTION, i32::from(chat.mode));
            dynamic.unity_fp.chat = None;
        }
        _ => {}
    }
    if !chat_open {
        dynamic.unity_fp.chat_standings = None;
    } else if dynamic.unity_fp.chat_standings.is_none() && unity_ranking_ready(snap) {
        let faction = snap.stats.map_or(0, |s| s.unity_faction);
        dynamic.unity_fp.chat_standings = Some(
            snap.unity
                .rankings
                .map(|set| concord_standing(&set, faction)),
        );
    }

    let ranking_set = stack.and_then(|s| {
        s.levels.iter().rev().find_map(|level| match level.kind {
            MenuKind::UnityRanking { result_set } => Some(result_set),
            _ => None,
        })
    });
    match ranking_set {
        None => dynamic.unity_fp.ranking = None,
        Some(set) => {
            let stale = dynamic
                .unity_fp
                .ranking
                .as_ref()
                .is_none_or(|(built, rows)| *built != set || rows.is_empty());
            if stale {
                dynamic.unity_fp.ranking = unity_ranking_ready(snap)
                    .then(|| {
                        snap.unity
                            .rankings
                            .get(usize::from(set))
                            .map(|data| (set, ranking_list(data)))
                    })
                    .flatten();
            }
        }
    }

    let open_kinds: Vec<MenuKind> = stack
        .map(|s| s.levels.iter().map(|l| l.kind).collect())
        .unwrap_or_default();
    dynamic
        .unity_fp
        .list_starts
        .retain(|(kind, _)| open_kinds.contains(kind));
    for level in stack.map(|s| s.levels.as_slice()).unwrap_or_default() {
        if let Some(visible) = visible_list_rows(level.kind) {
            dynamic
                .unity_fp
                .scroll_to(level.kind, level.cursor, visible);
        }
    }
}

/// Rows a scrolling list shows at once.
pub fn visible_list_rows(kind: MenuKind) -> Option<usize> {
    match kind {
        MenuKind::AlterEgoCategories | MenuKind::AlterEgoUpgrades { .. } => {
            Some(layout::FP_VISIBLE_ROWS)
        }
        MenuKind::UnityLeaders => Some(layout::LEADER_VISIBLE_ROWS),
        _ => None,
    }
}

/// What one window action asks of the input layer beyond the stack and menu state it already
/// changed: packets to send and client message lines to print.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnityFpOutcome {
    pub requests: Vec<UnityFpRequest>,
    pub chat_lines: Vec<String>,
}

impl UnityFpOutcome {
    fn send(&mut self, snap: &mut SceneSnapshot, request: UnityFpRequest) {
        note_sent(snap, request);
        self.requests.push(request);
    }
}

/// Status row id 11: `ut_menu` opens over `statcom2` (whose display-only detail closes first),
/// after asking for every completed ranking set.
pub fn open_unity_menu(stack: &mut MenuStack, snap: &mut SceneSnapshot) -> UnityFpOutcome {
    close_status_detail(stack);
    let mut outcome = UnityFpOutcome::default();
    for request in unity_ranking_requests(snap) {
        outcome.send(snap, request);
    }
    stack.push(MenuKind::UnityMenu);
    outcome
}

/// Status row id 13: `fp_cat` opens over `statcom2`, asking for the alter ego point data first
/// when none has arrived.
pub fn open_alter_ego_categories(
    stack: &mut MenuStack,
    snap: &mut SceneSnapshot,
) -> UnityFpOutcome {
    close_status_detail(stack);
    let mut outcome = UnityFpOutcome::default();
    if alter_ego_request_due(snap) {
        outcome.send(snap, UnityFpRequest::AlterEgoPoints);
    }
    stack.push(MenuKind::AlterEgoCategories);
    outcome
}

/// `statcom2` closes its display-only detail before it opens another window.
pub fn close_status_detail(stack: &mut MenuStack) {
    if stack
        .current()
        .is_some_and(|l| matches!(l.kind, MenuKind::StatusDetail { .. }))
    {
        stack.pop();
    }
}

/// Answers a select on one of these windows. `action` is the cursor row's action, `None` when
/// the window lists no row there. The confirm sound of a select that opens a window comes from
/// the menu-depth observer (`audio::observe_ui_mode_transitions` plays it for every window that
/// opens), so only selects that open nothing queue their sound here; a select that closes windows
/// marks the close as its own (`DynamicMenu::closed_by_logic`) so the observer adds no cancel sound.
pub fn select_unity_fp_row(
    kind: MenuKind,
    action: Option<DynamicMenuAction>,
    stack: &mut MenuStack,
    dynamic: &mut DynamicMenu,
    snap: &mut SceneSnapshot,
) -> UnityFpOutcome {
    use DynamicMenuAction as A;
    let mut outcome = UnityFpOutcome::default();
    match (kind, action) {
        (MenuKind::UnityMenu, Some(A::UnityMenuItem { item })) => {
            let open = match item {
                UT_MENU_ITEM_CHAT => Some(MenuKind::UnityChat),
                UT_MENU_ITEM_RANKING => Some(MenuKind::UnityRankingChooser),
                UT_MENU_ITEM_LEADERS => Some(MenuKind::UnityLeaders),
                _ => None,
            };
            if let Some(open) = open {
                stack.push(open);
            }
        }
        (MenuKind::UnityChat, Some(A::UnityChatMode { mode })) => {
            let stored = dynamic.config_query(UNITY_CHAT_OPTION, false);
            dynamic
                .unity_fp
                .chat
                .get_or_insert_with(|| UnityChatToggle::open(stored))
                .mode = mode;
        }
        (MenuKind::UnityRankingChooser, Some(A::UnityRankingSet { result_set })) => {
            if unity_quest_request_due(snap) {
                outcome.send(snap, UnityFpRequest::UnityQuest);
            }
            stack.push(MenuKind::UnityRanking { result_set });
        }
        (MenuKind::UnityLeaders, Some(A::UnityLeaderRow { .. })) => {
            dynamic.ui_sounds.push(UiSound::Confirm);
            dynamic.closed_by_logic = true;
            stack.pop();
        }
        (MenuKind::AlterEgoCategories, Some(A::AlterEgoCategory { category })) => {
            if alter_ego_request_due(snap) {
                outcome.send(snap, UnityFpRequest::AlterEgoPoints);
            }
            stack.push(MenuKind::AlterEgoUpgrades { category });
        }
        (MenuKind::AlterEgoUpgrades { .. }, action) => {
            let row = match action {
                Some(A::AlterEgoUpgrade { upgrade, cap }) => Some((upgrade, cap)),
                _ => None,
            };
            match upgrade_select(snap, row) {
                UpgradeSelect::Close => {
                    stack.pop();
                }
                UpgradeSelect::RefuseWithMessage => {
                    outcome.chat_lines.push(
                        dynamic
                            .unity_fp
                            .not_allowed_message
                            .clone()
                            .unwrap_or_else(|| {
                                client_message_fallback(
                                    GETTEXT_TABLE_CLIENT_MESSAGES,
                                    NOT_ALLOWED_MESSAGE_ID,
                                )
                            }),
                    );
                    dynamic.ui_sounds.push(UiSound::Buzzer);
                }
                UpgradeSelect::Buzzer => dynamic.ui_sounds.push(UiSound::Buzzer),
                UpgradeSelect::Confirm => {
                    let pending = stack
                        .levels
                        .iter()
                        .any(|l| matches!(l.kind, MenuKind::AlterEgoConfirm { .. }));
                    if let (false, Some((upgrade, _))) = (pending, row) {
                        stack.open_confirm(
                            MenuKind::AlterEgoConfirm { upgrade },
                            ConfirmDefault::No,
                        );
                    }
                }
            }
        }
        (MenuKind::AlterEgoConfirm { upgrade }, Some(A::AlterEgoConfirmAnswer { yes })) => {
            if yes {
                outcome
                    .requests
                    .push(UnityFpRequest::UpgradeAlterEgo(upgrade));
            }
            dynamic.closed_by_logic = true;
            stack.pop();
        }
        _ => {}
    }
    outcome
}

/// Left/Right on the ranking list switch to the other result set once both are complete.
/// Returns whether the key was taken.
pub fn switch_ranking_set(
    stack: &mut MenuStack,
    dynamic: &mut DynamicMenu,
    snap: &SceneSnapshot,
) -> bool {
    let Some(level) = stack.current_mut() else {
        return false;
    };
    let MenuKind::UnityRanking { result_set } = level.kind else {
        return false;
    };
    if !snap.unity.rankings.iter().all(|set| set.complete) {
        return false;
    }
    let other = u8::from(result_set == 0);
    level.kind = MenuKind::UnityRanking { result_set: other };
    level.cursor = 0;
    dynamic.unity_fp.ranking = snap
        .unity
        .rankings
        .get(usize::from(other))
        .map(|set| (other, ranking_list(set)));
    dynamic.ui_sounds.push(UiSound::Cursor);
    true
}

fn row(label: Option<&str>, action: DynamicMenuAction) -> DynamicMenuRow {
    DynamicMenuRow {
        label: label.unwrap_or_default().to_string(),
        action,
        tint: None,
    }
}

/// The rows a window's handler answers, labelled from DAT.
pub fn unity_fp_rows(
    level: &MenuLevel,
    dat: &UnityFpDat,
    windows: &UnityFpWindows,
) -> Vec<DynamicMenuRow> {
    use DynamicMenuAction as A;
    let texts = &dat.texts;
    let t12 = |id: u16| texts.get_text(GETTEXT_TABLE_12, id);
    match level.kind {
        MenuKind::UnityMenu => UT_MENU_ROWS
            .into_iter()
            .map(|item| {
                let label = dat
                    .item(UT_MENU_WINDOW, item)
                    .and_then(|block| dat.item_label(block));
                row(label, A::UnityMenuItem { item })
            })
            .collect(),
        MenuKind::UnityChat => [
            ffxi_proto::map::unity_toggle_mode::ACTIVE,
            ffxi_proto::map::unity_toggle_mode::INACTIVE,
        ]
        .into_iter()
        .enumerate()
        .map(|(r, mode)| row(t12(chat_row_text_id(r)), A::UnityChatMode { mode }))
        .collect(),
        MenuKind::UnityRankingChooser => (0..UNITY_RESULT_SETS)
            .filter_map(|set| u8::try_from(set).ok())
            .map(|result_set| {
                row(
                    t12(text12::RANKING_SET_BASE + u16::from(result_set)),
                    A::UnityRankingSet { result_set },
                )
            })
            .collect(),
        MenuKind::UnityRanking { result_set } => windows
            .ranking
            .as_ref()
            .filter(|(set, _)| *set == result_set)
            .map(|(_, list)| {
                list.iter()
                    .map(|standing| {
                        row(
                            t12(concord_name_id(standing.concord)),
                            A::UnityRankingRow {
                                concord: standing.concord,
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        MenuKind::UnityLeaders => (0..layout::LEADER_ROWS)
            .map(|r| {
                let concord = u8::try_from(r).unwrap_or(0);
                let label = if concord == 0 {
                    t12(text12::OWN_CONCORD_ROW)
                } else {
                    t12(concord_name_id(concord))
                };
                row(label, A::UnityLeaderRow { concord })
            })
            .collect(),
        MenuKind::AlterEgoCategories => dat
            .alter_ego
            .listed_categories()
            .into_iter()
            .map(|category| {
                row(
                    dat.alter_ego.name(category),
                    A::AlterEgoCategory { category },
                )
            })
            .collect(),
        MenuKind::AlterEgoUpgrades { category } => dat
            .alter_ego
            .category_upgrades(category)
            .into_iter()
            .map(|upgrade| {
                row(
                    dat.alter_ego.name(upgrade),
                    A::AlterEgoUpgrade {
                        upgrade,
                        cap: dat.alter_ego.cap(upgrade),
                    },
                )
            })
            .collect(),
        MenuKind::AlterEgoConfirm { .. } => [(CONFIRM_ITEM_YES, true), (CONFIRM_ITEM_NO, false)]
            .into_iter()
            .map(|(item, yes)| {
                let label = dat
                    .item(MERITYN_WINDOW, item)
                    .and_then(|block| dat.item_label(block));
                row(label, A::AlterEgoConfirmAnswer { yes })
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn chat_row_text_id(row: usize) -> u16 {
    text12::CHAT_ROW_BASE + u16::try_from(row).unwrap_or(0)
}

/// The windows this module draws itself.
pub fn is_unity_fp_window(kind: MenuKind) -> bool {
    matches!(
        kind,
        MenuKind::UnityMenu
            | MenuKind::UnityChat
            | MenuKind::UnityRankingChooser
            | MenuKind::UnityRanking { .. }
            | MenuKind::UnityLeaders
            | MenuKind::AlterEgoCategories
            | MenuKind::AlterEgoUpgrades { .. }
            | MenuKind::AlterEgoConfirm { .. }
    )
}

/// The DAT window record each kind opens.
pub fn unity_fp_window_name(kind: MenuKind) -> Option<&'static str> {
    Some(match kind {
        MenuKind::UnityMenu => UT_MENU_WINDOW,
        MenuKind::UnityChat | MenuKind::UnityRankingChooser => QUEST00_WINDOW,
        MenuKind::UnityRanking { .. } => QUEST01_WINDOW,
        MenuKind::UnityLeaders => AREALIST_WINDOW,
        MenuKind::AlterEgoCategories => FP_CAT_WINDOW,
        MenuKind::AlterEgoUpgrades { .. } => FP_CAT2_WINDOW,
        MenuKind::AlterEgoConfirm { .. } => MERITYN_WINDOW,
        _ => return None,
    })
}

/// Rows the list pages by with Left/Right.
pub fn unity_fp_page_rows(kind: MenuKind) -> Option<usize> {
    match kind {
        MenuKind::AlterEgoCategories | MenuKind::AlterEgoUpgrades { .. } => {
            Some(layout::FP_VISIBLE_ROWS)
        }
        MenuKind::UnityRanking { .. } => Some(layout::RANKING_ROWS_PER_PAGE),
        MenuKind::UnityLeaders => Some(layout::LEADER_VISIBLE_ROWS),
        _ => None,
    }
}

/// One window to draw: its record, its place in the draw order, its text, its dock slot, and how
/// far its opener's logic moves it below its record frame.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelWindow {
    pub name: &'static str,
    pub stack_index: usize,
    pub texts: Vec<PanelText>,
    pub dock: Option<crate::hud::dock::DockSlot>,
    pub shift_y: f32,
}

impl PanelWindow {
    /// A window at its own record frame.
    pub fn at_record(name: &'static str, stack_index: usize, texts: Vec<PanelText>) -> Self {
        Self {
            name,
            stack_index,
            texts,
            dock: crate::hud::dock::registry_dock_slot(name),
            shift_y: 0.0,
        }
    }
}

/// Inputs every window builder reads.
pub struct PanelContext<'a> {
    pub snap: &'a SceneSnapshot,
    pub dat: &'a UnityFpDat,
    pub windows: &'a UnityFpWindows,
}

impl PanelContext<'_> {
    fn t(&self, table: u8, id: u16) -> Option<&str> {
        self.dat.texts.get_text(table, id)
    }

    fn text_at(&self, table: u8, id: u16, (x, y): (f32, f32), out: &mut Vec<PanelText>) {
        if let Some(text) = self.t(table, id) {
            out.push(PanelText::left(text, x, y, ARGB_NEUTRAL));
        }
    }

    fn frame(&self, window: &str) -> Option<ffxi_dat::menu_windows::FrameBlock> {
        self.dat.window(window)?.frame
    }

    fn item(&self, window: &str, id: u8) -> Option<&ItemBlock> {
        self.dat.item(window, id)
    }
}

/// Every window the open stack draws here, oldest first, each display-only companion right
/// after the window that opened it.
pub fn panel_windows(
    stack: &MenuStack,
    rows_of: impl Fn(usize) -> Vec<DynamicMenuRow>,
    cx: &PanelContext,
) -> Vec<PanelWindow> {
    let top = stack.levels.len().saturating_sub(1);
    let mut out = Vec::new();
    for (index, level) in stack.levels.iter().enumerate() {
        let Some(name) = unity_fp_window_name(level.kind) else {
            continue;
        };
        let focused = index == top;
        let rows = rows_of(index);
        out.push(PanelWindow::at_record(
            name,
            index,
            window_texts(level, focused, &rows, cx),
        ));
        match level.kind {
            MenuKind::UnityMenu => out.push(PanelWindow {
                dock: crate::hud::dock::DockSlot::from_registry_flags(UT_POINT_REGISTRY_FLAGS),
                ..PanelWindow::at_record(UT_POINT_WINDOW, index, ut_point_texts(cx))
            }),
            MenuKind::AlterEgoCategories => {
                out.push(PanelWindow::at_record(
                    FP_INFO_WINDOW,
                    index,
                    fp_info_texts(cx),
                ));
                out.push(PanelWindow::at_record(
                    BLUEHELP_WINDOW,
                    index,
                    bluehelp_texts(stack, &rows_of, cx),
                ));
            }
            _ => {}
        }
    }
    if let Some((index, title)) = title_bar(stack) {
        out.push(title_window(index, cx.t(GETTEXT_TABLE_12, title)));
    }
    out
}

/// The title bar `titlewin` showing `title` for the window at `stack_index`.
pub fn title_window(stack_index: usize, title: Option<&str>) -> PanelWindow {
    let texts = title
        .map(|title| {
            vec![PanelText::left(
                title,
                layout::KULUU_TEXT_INSET,
                layout::KULUU_TEXT_INSET,
                ARGB_NEUTRAL,
            )]
        })
        .unwrap_or_default();
    PanelWindow::at_record(TITLEWIN_WINDOW, stack_index, texts)
}

/// `ut_point`'s registry flags: display-only, docked in the party window's slot
/// (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md).
pub const UT_POINT_REGISTRY_FLAGS: u32 =
    crate::hud::dock::DOCK_FLAG_PARTY | crate::hud::dock::REGISTRY_FLAG_DISPLAY_ONLY;

/// The title bar text of the newest open Unity `quest00` / `quest01` window.
pub fn title_bar(stack: &MenuStack) -> Option<(usize, u16)> {
    stack
        .levels
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, level)| match level.kind {
            MenuKind::UnityChat => Some((index, text12::CHAT_TITLE)),
            MenuKind::UnityRankingChooser | MenuKind::UnityRanking { .. } => {
                Some((index, text12::RANKING_TITLE))
            }
            _ => None,
        })
}

/// A window record's own static labels, each at its item's position.
pub fn record_label_texts(record: Option<&MenuWindow>, texts: &impl GetText) -> Vec<PanelText> {
    record
        .into_iter()
        .flat_map(|record| record.items.iter().flatten())
        .filter_map(|item| {
            item_label(texts, item).map(|label| {
                PanelText::left(label, f32::from(item.x), f32::from(item.y), ARGB_NEUTRAL)
            })
        })
        .collect()
}

fn record_labels(window: &str, cx: &PanelContext, out: &mut Vec<PanelText>) {
    out.extend(record_label_texts(cx.dat.window(window), &cx.dat.texts));
}

pub fn ut_point_texts(cx: &PanelContext) -> Vec<PanelText> {
    let mut out = Vec::new();
    record_labels(UT_POINT_WINDOW, cx, &mut out);
    out.push(PanelText::right(
        ut_point_text(cx.snap),
        layout::UT_POINT_RIGHT,
        layout::UT_POINT_TOP,
        ARGB_NEUTRAL,
    ));
    out
}

pub fn fp_info_texts(cx: &PanelContext) -> Vec<PanelText> {
    let mut out = Vec::new();
    record_labels(FP_INFO_WINDOW, cx, &mut out);
    if let Some(frame) = cx.frame(FP_INFO_WINDOW) {
        let (text, color) = fp_info_text(cx.snap);
        out.push(PanelText::right(
            text,
            f32::from(frame.width) - layout::FP_INFO_RIGHT_INSET,
            layout::FP_INFO_TOP,
            color,
        ));
    }
    out
}

/// The description box follows the newer of `fp_cat` / `fp_cat2`: the highlighted id's name as
/// a header line over its description, blank with no valid row.
pub fn bluehelp_texts(
    stack: &MenuStack,
    rows_of: &impl Fn(usize) -> Vec<DynamicMenuRow>,
    cx: &PanelContext,
) -> Vec<PanelText> {
    let highlighted = stack
        .levels
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, level)| match level.kind {
            MenuKind::AlterEgoCategories | MenuKind::AlterEgoUpgrades { .. } => {
                Some(rows_of(index).get(level.cursor).map(|r| r.action))
            }
            _ => None,
        })
        .flatten()
        .and_then(|action| match action {
            DynamicMenuAction::AlterEgoCategory { category } => Some(category),
            DynamicMenuAction::AlterEgoUpgrade { upgrade, .. } => Some(upgrade),
            _ => None,
        });
    let Some(id) = highlighted else {
        return Vec::new();
    };
    description_box_texts(
        cx.frame(BLUEHELP_WINDOW),
        cx.dat.alter_ego.name(id),
        cx.dat.alter_ego.description(id),
    )
}

/// The description box `bluehelp`: a title line over a description wrapped to the box's record
/// width.
pub fn description_box_texts(
    frame: Option<ffxi_dat::menu_windows::FrameBlock>,
    title: Option<&str>,
    description: Option<&str>,
) -> Vec<PanelText> {
    let width = frame.map(|f| f32::from(f.width) - 2.0 * layout::KULUU_TEXT_INSET);
    let mut out = Vec::new();
    if let Some(title) = title {
        out.push(PanelText::left(
            title,
            layout::KULUU_TEXT_INSET,
            layout::KULUU_TEXT_INSET,
            ARGB_NEUTRAL,
        ));
    }
    if let Some(description) = description {
        out.push(PanelText {
            wrap_width: width,
            ..PanelText::left(
                description,
                layout::KULUU_TEXT_INSET,
                layout::KULUU_TEXT_INSET + layout::KULUU_LINE_PITCH,
                ARGB_NEUTRAL,
            )
        });
    }
    out
}

fn window_texts(
    level: &MenuLevel,
    focused: bool,
    rows: &[DynamicMenuRow],
    cx: &PanelContext,
) -> Vec<PanelText> {
    match level.kind {
        MenuKind::UnityMenu => item_rows(UT_MENU_WINDOW, level, focused, rows, cx),
        MenuKind::AlterEgoConfirm { upgrade } => {
            let mut out = item_rows(MERITYN_WINDOW, level, focused, rows, cx);
            out.push(merityn_prompt_text(
                cx.frame(MERITYN_WINDOW),
                upgrade_prompt(cx, upgrade).text(&cx.dat.texts),
            ));
            out
        }
        MenuKind::UnityChat => unity_chat_texts(level, focused, cx),
        MenuKind::UnityRankingChooser => ranking_chooser_texts(level, focused, cx),
        MenuKind::UnityRanking { result_set } => {
            ranking_texts(level, focused, result_set, rows, cx)
        }
        MenuKind::UnityLeaders => leader_texts(level, focused, rows, cx),
        MenuKind::AlterEgoCategories => fp_list_texts(level, focused, rows, cx),
        MenuKind::AlterEgoUpgrades { .. } => fp_list_texts(level, focused, rows, cx),
        _ => Vec::new(),
    }
}

/// Kuluu's pointer for a row whose text starts at (`x`, `y`).
pub fn pointer_at(x: f32, y: f32) -> PanelText {
    PanelText::left(
        POINTER_STAND_IN,
        x - layout::KULUU_POINTER_INDENT,
        y,
        ARGB_NEUTRAL,
    )
}

fn pointer(
    focused: bool,
    level: &MenuLevel,
    index: usize,
    x: f32,
    y: f32,
    out: &mut Vec<PanelText>,
) {
    if focused && level.cursor == index {
        out.push(pointer_at(x, y));
    }
}

/// Selector rows drawn at their own DAT items (row r at item id r + 1), each in its own color,
/// with the pointer on `cursor`.
pub fn selector_item_texts(
    record: Option<&MenuWindow>,
    rows: &[(String, u32)],
    cursor: Option<usize>,
) -> Vec<PanelText> {
    let Some(record) = record else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (index, (label, color)) in rows.iter().enumerate() {
        let Some(item) = u8::try_from(index + 1)
            .ok()
            .and_then(|id| item_by_id(record, id))
        else {
            continue;
        };
        let (x, y) = (f32::from(item.x), f32::from(item.y));
        if cursor == Some(index) {
            out.push(pointer_at(x, y));
        }
        out.push(PanelText::left(label.clone(), x, y, *color));
    }
    out
}

/// Rows drawn at their own DAT items (items 1, 2, ... in row order).
fn item_rows(
    window: &str,
    level: &MenuLevel,
    focused: bool,
    rows: &[DynamicMenuRow],
    cx: &PanelContext,
) -> Vec<PanelText> {
    let labels: Vec<(String, u32)> = rows
        .iter()
        .map(|row| (row.label.clone(), ARGB_NEUTRAL))
        .collect();
    selector_item_texts(
        cx.dat.window(window),
        &labels,
        focused.then_some(level.cursor),
    )
}

/// GetText table 10 entry of the alter ego upgrade prompt.
pub const ALTER_EGO_PROMPT_ENTRY: u16 = 123;

/// The `merityn` prompt for an alter ego upgrade: the upgrade name, then its s2c 0x08E cost.
pub fn upgrade_prompt(cx: &PanelContext, upgrade: u16) -> ConfirmPrompt {
    let name = cx.dat.alter_ego.name(upgrade).unwrap_or_default();
    let cost = cx
        .snap
        .alter_ego_points
        .and_then(|p| p.costs.get(usize::from(upgrade)).copied())
        .unwrap_or(0);
    ConfirmPrompt {
        entry: ALTER_EGO_PROMPT_ENTRY,
        args: vec![PromptArg::Name(name.to_string()), PromptArg::Points(cost)],
    }
}

/// `merityn`'s prompt text in the record's color at the record's place, wrapped to the window
/// `frame`'s width.
pub fn merityn_prompt_text(
    frame: Option<ffxi_dat::menu_windows::FrameBlock>,
    text: String,
) -> PanelText {
    PanelText {
        wrap_width: frame.map(|f| f32::from(f.width) - layout::MERITYN_WRAP_MARGIN),
        line_pitch: Some(layout::MERITYN_LINE_PITCH),
        ..PanelText::left(
            text,
            layout::MERITYN_TEXT_INSET,
            layout::MERITYN_TEXT_INSET,
            ARGB_NEUTRAL,
        )
    }
}

fn sprite_marker(chosen: bool, item: &ItemBlock, out: &mut Vec<PanelText>, top: f32) {
    let ((dx, dy), marker) = if chosen {
        (layout::CHECK_SPRITE_AT, CHECK_SPRITE_STAND_IN)
    } else {
        (layout::BULLET_SPRITE_AT, BULLET_SPRITE_STAND_IN)
    };
    out.push(PanelText::left(
        marker,
        f32::from(item.x) + dx,
        top + dy,
        ARGB_NEUTRAL,
    ));
}

/// The Unity chat panel (`quest00` from `ut_menu` item 1).
pub fn unity_chat_texts(level: &MenuLevel, focused: bool, cx: &PanelContext) -> Vec<PanelText> {
    let mut out = Vec::new();
    cx.text_at(
        GETTEXT_TABLE_12,
        text12::CHAT_TITLE,
        layout::CHAT_TITLE,
        &mut out,
    );
    cx.text_at(
        GETTEXT_TABLE_12,
        text12::CHAT_FOOTER,
        layout::CHAT_FOOTER,
        &mut out,
    );
    if !unity_ranking_ready(cx.snap) {
        for (id, at) in NOT_READY_LINES.into_iter().zip(layout::CHAT_NOT_READY) {
            cx.text_at(GETTEXT_TABLE_9, id, at, &mut out);
        }
    } else {
        for (id, y) in text12::CHAT_LABELS.into_iter().zip(layout::CHAT_LINE_YS) {
            cx.text_at(GETTEXT_TABLE_12, id, (layout::CHAT_LABEL_X, y), &mut out);
        }
        let faction = cx.snap.stats.map_or(0, |s| s.unity_faction);
        cx.text_at(
            GETTEXT_TABLE_12,
            concord_name_id(faction),
            (layout::CHAT_FACTION_X, layout::CHAT_LINE_YS[0]),
            &mut out,
        );
        let set_ys = [layout::CHAT_LINE_YS[1], layout::CHAT_LINE_YS[2]];
        if faction == 0 {
            for y in set_ys {
                cx.text_at(
                    GETTEXT_TABLE_12,
                    text12::RANK_TEXT_BASE,
                    (layout::CHAT_RANK_X, y),
                    &mut out,
                );
            }
        } else {
            let standings = cx.windows.chat_standings.unwrap_or_default();
            for (standing, y) in standings.into_iter().zip(set_ys) {
                let standing = standing.unwrap_or(ConcordStanding {
                    concord: faction,
                    rank: 0,
                    value: 0,
                });
                if let Some(text) = rank_text(&cx.dat.texts, standing.rank) {
                    out.push(PanelText::left(text, layout::CHAT_RANK_X, y, ARGB_NEUTRAL));
                }
                if standing.rank != 0 {
                    out.push(PanelText::right(
                        points_text(standing.value),
                        layout::CHAT_VALUE_RIGHT,
                        y,
                        ARGB_NEUTRAL,
                    ));
                }
            }
            let weekly = cx
                .snap
                .stats
                .map_or([0, 0], |s| [s.unity_points2, s.unity_points1]);
            for (points, y) in weekly
                .into_iter()
                .zip([layout::CHAT_LINE_YS[3], layout::CHAT_LINE_YS[4]])
            {
                out.push(PanelText::right(
                    points_text(u32::from(points)),
                    layout::CHAT_VALUE_RIGHT,
                    y,
                    ARGB_NEUTRAL,
                ));
            }
        }
    }

    let mode = cx
        .windows
        .chat
        .map_or(ffxi_proto::map::unity_toggle_mode::INACTIVE, |c| c.mode);
    let height = cx.frame(QUEST00_WINDOW).map(|f| f32::from(f.height));
    for (r, inset) in layout::CHAT_ROW_BOTTOM_INSETS.into_iter().enumerate() {
        let (Some(height), Some(item)) = (
            height,
            u8::try_from(r + 1)
                .ok()
                .and_then(|id| cx.item(QUEST00_WINDOW, id)),
        ) else {
            continue;
        };
        let top = height - inset - f32::from(item.height);
        let chosen = (mode == ffxi_proto::map::unity_toggle_mode::ACTIVE) == (r == 0);
        sprite_marker(chosen, item, &mut out, top);
        let x = f32::from(item.x) + layout::CHAT_ROW_TEXT_X;
        pointer(focused, level, r, x, top, &mut out);
        cx.text_at(GETTEXT_TABLE_12, chat_row_text_id(r), (x, top), &mut out);
    }
    out
}

/// The ranking chooser (`quest00` from `ut_menu` item 2).
pub fn ranking_chooser_texts(
    level: &MenuLevel,
    focused: bool,
    cx: &PanelContext,
) -> Vec<PanelText> {
    let mut out = Vec::new();
    cx.text_at(
        GETTEXT_TABLE_12,
        text12::RANKING_TITLE,
        layout::CHOOSER_HEADER,
        &mut out,
    );
    for (r, top) in layout::CHOOSER_ROW_TOPS.into_iter().enumerate() {
        let Some(item) = u8::try_from(r + 1)
            .ok()
            .and_then(|id| cx.item(QUEST00_WINDOW, id))
        else {
            continue;
        };
        sprite_marker(false, item, &mut out, top);
        let x = f32::from(item.x) + layout::CHOOSER_ROW_TEXT_X;
        pointer(focused, level, r, x, top, &mut out);
        cx.text_at(
            GETTEXT_TABLE_12,
            text12::RANKING_SET_BASE + u16::try_from(r).unwrap_or(0),
            (x, top),
            &mut out,
        );
    }
    out
}

/// The ranking list (`quest01`) for one result set, a page of eight rows at the record's items.
pub fn ranking_texts(
    level: &MenuLevel,
    focused: bool,
    result_set: u8,
    rows: &[DynamicMenuRow],
    cx: &PanelContext,
) -> Vec<PanelText> {
    let mut out = Vec::new();
    cx.text_at(
        GETTEXT_TABLE_12,
        text12::RANKING_SET_BASE + u16::from(result_set),
        layout::RANKING_HEADER,
        &mut out,
    );
    let list = cx
        .windows
        .ranking
        .as_ref()
        .filter(|(set, _)| *set == result_set)
        .map(|(_, list)| list.as_slice())
        .unwrap_or_default();
    if list.is_empty() || !unity_ranking_ready(cx.snap) {
        for (id, at) in NOT_READY_LINES.into_iter().zip(layout::RANKING_NOT_READY) {
            cx.text_at(GETTEXT_TABLE_9, id, at, &mut out);
        }
        return out;
    }
    let page = level.cursor / layout::RANKING_ROWS_PER_PAGE;
    let first = page * layout::RANKING_ROWS_PER_PAGE;
    for (slot, standing) in list
        .iter()
        .enumerate()
        .skip(first)
        .take(layout::RANKING_ROWS_PER_PAGE)
    {
        let Some(item) = u8::try_from(slot - first + 1)
            .ok()
            .and_then(|id| cx.item(QUEST01_WINDOW, id))
        else {
            continue;
        };
        let (x, y) = (f32::from(item.x), f32::from(item.y));
        sprite_marker(false, item, &mut out, y);
        pointer(focused, level, slot, x, y, &mut out);
        if let Some(text) = rank_text(&cx.dat.texts, standing.rank) {
            out.push(PanelText::left(
                text,
                x + layout::RANKING_RANK_X,
                y,
                ARGB_NEUTRAL,
            ));
        }
        let name = rows.get(slot).map(|r| r.label.clone()).unwrap_or_default();
        out.push(PanelText::left(
            name,
            x + layout::RANKING_NAME_X,
            y,
            ARGB_NEUTRAL,
        ));
        out.push(PanelText::right(
            points_text(standing.value),
            x + layout::RANKING_VALUE_RIGHT,
            y,
            ARGB_NEUTRAL,
        ));
    }
    out
}

/// The Unity leader list (`arealist` from `ut_menu` item 3): twelve rows, five visible, the
/// first at the record's item 1. The per-row Concord tags of column 2 are client constants the
/// record keeps out of its spec, so that column stays empty.
pub fn leader_texts(
    level: &MenuLevel,
    focused: bool,
    rows: &[DynamicMenuRow],
    cx: &PanelContext,
) -> Vec<PanelText> {
    let mut out = Vec::new();
    let Some(first) = cx.item(AREALIST_WINDOW, 1) else {
        return out;
    };
    let start = cx.windows.list_start(level.kind);
    let name_x = layout::LEADER_COLUMNS_X[0];
    for (slot, row) in rows
        .iter()
        .enumerate()
        .skip(start)
        .take(layout::LEADER_VISIBLE_ROWS)
    {
        let y = f32::from(first.y) + (slot - start) as f32 * layout::LEADER_ROW_PITCH;
        pointer(focused, level, slot, name_x, y, &mut out);
        out.push(PanelText::left(row.label.clone(), name_x, y, ARGB_NEUTRAL));
    }
    out
}

/// `fp_cat` and `fp_cat2`: ten rows seventeen apart, the cursor row's name in the cursor color,
/// and for upgrades the level, cap and cost cells colored by the row's state.
pub fn fp_list_texts(
    level: &MenuLevel,
    focused: bool,
    rows: &[DynamicMenuRow],
    cx: &PanelContext,
) -> Vec<PanelText> {
    let mut out = Vec::new();
    let first_top = match level.kind {
        MenuKind::AlterEgoUpgrades { .. } => layout::FP_CAT2_FIRST_ROW_TOP,
        _ => layout::FP_CAT_FIRST_ROW_TOP,
    };
    let start = cx.windows.list_start(level.kind);
    for (slot, row) in rows
        .iter()
        .enumerate()
        .skip(start)
        .take(layout::FP_VISIBLE_ROWS)
    {
        let y = first_top + (slot - start) as f32 * layout::FP_ROW_PITCH + layout::FP_TEXT_DROP;
        let lit = focused && level.cursor == slot;
        let (name_color, level_color, cost_color) = match row.action {
            DynamicMenuAction::AlterEgoUpgrade { upgrade, cap } => {
                upgrade_row_colors(upgrade_row_state(cx.snap, upgrade, cap))
            }
            _ => (ARGB_NEUTRAL, ARGB_NEUTRAL, None),
        };
        let name_color = if lit { ARGB_FP_CURSOR } else { name_color };
        out.push(PanelText::left(
            row.label.clone(),
            layout::FP_NAME_X,
            y,
            name_color,
        ));
        let DynamicMenuAction::AlterEgoUpgrade { upgrade, cap } = row.action else {
            continue;
        };
        let current = cx
            .snap
            .alter_ego_points
            .and_then(|p| p.upgrades.get(usize::from(upgrade)).copied())
            .unwrap_or(0);
        out.push(PanelText::right(
            current.to_string(),
            layout::FP_LEVEL_RIGHT,
            y,
            level_color,
        ));
        out.push(PanelText::left(
            format!("/{cap}"),
            layout::FP_CAP_X,
            y,
            level_color,
        ));
        if let Some(cost_color) = cost_color {
            if let Some(caption) = cx.t(GETTEXT_TABLE_12, text12::COST_CAPTION) {
                out.push(PanelText::left(
                    caption,
                    layout::FP_COST_CAPTION_X,
                    y,
                    cost_color,
                ));
            }
            let cost = cx
                .snap
                .alter_ego_points
                .and_then(|p| p.costs.get(usize::from(upgrade)).copied())
                .unwrap_or(0);
            out.push(PanelText::right(
                cost_text(u32::from(cost)),
                layout::FP_COST_RIGHT,
                y,
                cost_color,
            ));
        }
    }
    out
}

/// Window record builders the window tests share.
#[cfg(test)]
pub(crate) mod test_support {
    use ffxi_dat::menu_windows::{FrameBlock, ItemBlock, MenuWindow};

    pub fn item(id: i16, x: i16, y: i16, height: i16, text_b: Option<i32>) -> ItemBlock {
        ItemBlock {
            id,
            x,
            y,
            width: 0,
            height,
            up: None,
            down: None,
            right: None,
            left: None,
            text_a_id: None,
            text_b_id: text_b,
        }
    }

    pub fn window(root: &str, width: i16, height: i16, items: Vec<ItemBlock>) -> MenuWindow {
        MenuWindow {
            fourcc: String::new(),
            root: root.to_string(),
            offset: 0,
            size: 0,
            widgets: Vec::new(),
            item_count: u8::try_from(items.len()).unwrap(),
            frame: Some(FrameBlock {
                x: 0,
                y: 0,
                cursor_a: 0,
                cursor_b: 0,
                width,
                height,
                anchor: 0,
            }),
            items: items.into_iter().map(Some).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ut_menu` makes its last DAT item unselectable at creation; its record says which of the
    /// three rows that is, if any
    /// (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md).
    #[test]
    fn ut_menu_names_its_unselectable_row_from_its_record() {
        use crate::input_mode::LastItemRow;
        const ROW_PITCH: i16 = 16;
        let mut dat = UnityFpDat::default();
        assert_eq!(
            dat.menu_last_item_row(),
            None,
            "unread until the records load"
        );
        dat.loaded = true;
        let record = |count: i16| {
            let items = (1..=count)
                .map(|id| test_support::item(id, 0, id * ROW_PITCH, ROW_PITCH, None))
                .collect();
            test_support::window(UT_MENU_WINDOW, 0, 0, items)
        };
        let live_rows = i16::try_from(UT_MENU_ROWS.len()).unwrap();
        dat.windows.insert(UT_MENU_WINDOW.into(), record(live_rows));
        assert_eq!(
            dat.menu_last_item_row(),
            Some(LastItemRow::Row(UT_MENU_ROWS.len() - 1)),
            "three items: the leader list row is the last"
        );
        dat.windows
            .insert(UT_MENU_WINDOW.into(), record(live_rows + 1));
        assert_eq!(
            dat.menu_last_item_row(),
            Some(LastItemRow::Undrawn),
            "a trailing fourth item locks no row"
        );
    }
    use crate::hud::gettext::test_support::{prompt_templates, prompt_text};
    use crate::input_mode::CONFIRM_NO_ROW;
    use kuluu_snapshot::{AlterEgoPoints, CharStats, ALTER_EGO_UPGRADE_SLOTS};

    const HP: u16 = 8;
    const MP: u16 = 9;
    const STR: u16 = 10;
    const CATEGORY_STATS: u16 = 1;

    use super::test_support::{item, window};

    /// Every GetText entry the windows read reads back as `"<table>/<id>"`.
    fn texts() -> GetTextTables {
        let mut texts = GetTextTables::default();
        texts.insert_xistring(GETTEXT_TABLE_10, prompt_templates());
        let ids12 = [
            text12::CHAT_TITLE,
            text12::RANKING_TITLE,
            text12::CHAT_FOOTER,
            text12::OWN_CONCORD_ROW,
            text12::COST_CAPTION,
        ]
        .into_iter()
        .chain(text12::CHAT_LABELS)
        .chain(text12::CONCORD_NAME_BASE..=text12::CONCORD_NAME_BASE + 11)
        .chain(text12::RANKING_SET_BASE..text12::RANKING_SET_BASE + 2)
        .chain(text12::RANK_TEXT_BASE..text12::RANK_TEMPLATE)
        .chain(text12::CHAT_ROW_BASE..text12::CHAT_ROW_BASE + 2);
        for id in ids12 {
            texts.insert(GETTEXT_TABLE_12, id, format!("12/{id}"));
        }
        texts.insert(GETTEXT_TABLE_12, text12::RANK_TEMPLATE, "No.%d");
        for id in NOT_READY_LINES {
            texts.insert(GETTEXT_TABLE_9, id, format!("9/{id}"));
        }
        texts.insert(
            GETTEXT_TABLE_CLIENT_MESSAGES,
            NOT_ALLOWED_MESSAGE_ID,
            "not here",
        );
        for id in 1..=3 {
            texts.insert(
                GETTEXT_TABLE_SHORT_CAPTIONS,
                600 + id,
                format!("ut item {id}"),
            );
        }
        texts
    }

    fn category_record(id: u16, upgrades: &[u16]) -> ffxi_dat::alter_ego::AlterEgoCategory {
        let mut slots = [0u16; ffxi_dat::alter_ego::CATEGORY_UPGRADE_SLOTS];
        slots[..upgrades.len()].copy_from_slice(upgrades);
        ffxi_dat::alter_ego::AlterEgoCategory {
            id,
            upgrades: slots,
        }
    }

    fn dat() -> UnityFpDat {
        let mut windows = BTreeMap::new();
        windows.insert(
            UT_MENU_WINDOW.to_string(),
            window(
                UT_MENU_WINDOW,
                120,
                80,
                (1..=4)
                    .map(|id| item(id, 10, 10 * id, 12, (id < 4).then_some(600 + i32::from(id))))
                    .collect(),
            ),
        );
        windows.insert(
            UT_POINT_WINDOW.to_string(),
            window(UT_POINT_WINDOW, 110, 30, vec![]),
        );
        windows.insert(
            QUEST00_WINDOW.to_string(),
            window(
                QUEST00_WINDOW,
                360,
                190,
                (1..=4).map(|id| item(id, 30, 0, 14, None)).collect(),
            ),
        );
        windows.insert(
            QUEST01_WINDOW.to_string(),
            window(
                QUEST01_WINDOW,
                300,
                200,
                (1..=10)
                    .map(|id| item(id, 12, 20 + 16 * id, 14, None))
                    .collect(),
            ),
        );
        windows.insert(
            AREALIST_WINDOW.to_string(),
            window(AREALIST_WINDOW, 420, 120, vec![item(1, 0, 24, 14, None)]),
        );
        windows.insert(
            FP_INFO_WINDOW.to_string(),
            window(FP_INFO_WINDOW, 90, 40, vec![]),
        );
        windows.insert(
            MERITYN_WINDOW.to_string(),
            window(
                MERITYN_WINDOW,
                200,
                60,
                vec![
                    item(1, 20, 30, 14, Some(601)),
                    item(2, 20, 44, 14, Some(602)),
                ],
            ),
        );
        let mut strings = vec![String::new(); 40];
        for id in [CATEGORY_STATS, HP, MP, STR] {
            strings[ffxi_dat::alter_ego::name_entry(id)] = format!("name {id}");
            strings[ffxi_dat::alter_ego::description_entry(id)] = format!("about {id}");
        }
        let mut upgrade_records = vec![0u8; ffxi_dat::alter_ego::UPGRADE_RECORD_LEN * 20];
        for (upgrade, cap) in [(HP, 50u8), (MP, 50), (STR, 30)] {
            upgrade_records[usize::from(upgrade) * ffxi_dat::alter_ego::UPGRADE_RECORD_LEN
                + ffxi_dat::alter_ego::UPGRADE_CAP_BYTE] = cap;
        }
        UnityFpDat {
            loaded: true,
            windows,
            texts: texts(),
            alter_ego: AlterEgoDat {
                categories: vec![
                    category_record(0, &[HP]),
                    category_record(CATEGORY_STATS, &[HP, 0, MP, STR]),
                    category_record(2, &[]),
                ],
                upgrade_records,
                strings,
            },
        }
    }

    fn ranking_set(
        members: [u32; UNITY_CONCORDS],
        points: [u32; UNITY_CONCORDS],
    ) -> UnityRankingSet {
        UnityRankingSet {
            members,
            points,
            complete: true,
        }
    }

    fn ready_snapshot() -> SceneSnapshot {
        let mut snap = SceneSnapshot {
            status_data_ready: true,
            ..Default::default()
        };
        let mut points = [0u32; UNITY_CONCORDS];
        points[2] = 9_000;
        points[4] = 9_000;
        points[6] = 4_000;
        let set = ranking_set([10; UNITY_CONCORDS], points);
        snap.unity.rankings = [set, set];
        snap
    }

    fn fp_snapshot(points: u16, levels: &[(u16, u8, u16)]) -> SceneSnapshot {
        let mut block = AlterEgoPoints {
            points,
            upgrades: [0; ALTER_EGO_UPGRADE_SLOTS],
            costs: [0; ALTER_EGO_UPGRADE_SLOTS],
        };
        for &(upgrade, level, cost) in levels {
            block.upgrades[usize::from(upgrade)] = level;
            block.costs[usize::from(upgrade)] = cost;
        }
        SceneSnapshot {
            alter_ego_points: Some(block),
            login_state: ffxi_proto::decode::ServerLoginMyroom::LOGIN_STATE_MYROOM,
            ..Default::default()
        }
    }

    #[test]
    fn record_argb_words_pin() {
        const RECORD_PINNED_NEUTRAL: u32 = 0x8080_8080;
        const RECORD_PINNED_DIM: u32 = 0x8040_4040;
        const RECORD_PINNED_CAPPED: u32 = 0x8020_60A0;
        const RECORD_PINNED_FP_CURSOR: u32 = 0x8080_3040;
        assert_eq!(ARGB_NEUTRAL, RECORD_PINNED_NEUTRAL);
        assert_eq!(ARGB_DIM, RECORD_PINNED_DIM);
        assert_eq!(ARGB_CAPPED, RECORD_PINNED_CAPPED);
        assert_eq!(ARGB_FP_CURSOR, RECORD_PINNED_FP_CURSOR);
        assert_eq!(
            retail_text_color(ARGB_NEUTRAL),
            crate::hud::style::theme::TEXT,
            "neutral white is the theme's body text"
        );
    }

    #[test]
    fn ut_point_waits_for_the_status_data() {
        let mut snap = SceneSnapshot {
            stats: Some(CharStats {
                unity_points: 4321,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(ut_point_text(&snap), format!("/{UT_POINT_MAX}"));
        snap.status_data_ready = true;
        assert_eq!(ut_point_text(&snap), format!("4321/{UT_POINT_MAX}"));
    }

    #[test]
    fn fp_info_reads_zero_until_data_and_turns_blue_at_the_cap() {
        assert_eq!(
            fp_info_text(&SceneSnapshot::default()),
            (format!("0/{ALTER_EGO_POINT_MAX}"), ARGB_NEUTRAL)
        );
        let below = fp_snapshot(ALTER_EGO_POINT_MAX - 1, &[]);
        assert_eq!(fp_info_text(&below).1, ARGB_NEUTRAL);
        let at = fp_snapshot(ALTER_EGO_POINT_MAX, &[]);
        assert_eq!(
            fp_info_text(&at),
            (
                format!("{ALTER_EGO_POINT_MAX}/{ALTER_EGO_POINT_MAX}"),
                ARGB_CAPPED
            )
        );
    }

    /// value = points / members (0 without members); ties share a rank and the next skips; an
    /// all-zero set ranks nobody.
    #[test]
    fn concord_values_and_competition_ranks() {
        let mut members = [0u32; UNITY_CONCORDS];
        let mut points = [0u32; UNITY_CONCORDS];
        members[0] = 3;
        points[0] = 10;
        members[1] = 2;
        points[1] = 7;
        members[2] = 1;
        points[2] = 3;
        points[3] = 500;
        let set = ranking_set(members, points);
        let values = concord_values(&set);
        assert_eq!(
            &values[..4],
            &[3, 3, 3, 0],
            "trunc(10/3), trunc(7/2), 3/1, no members"
        );
        let ranks = competition_ranks(&values);
        assert_eq!(&ranks[..4], &[1, 1, 1, 4], "three-way tie at 1, then 4");
        assert_eq!(
            competition_ranks(&[0; UNITY_CONCORDS]),
            [0; UNITY_CONCORDS],
            "nobody ranks while every value is zero"
        );
    }

    #[test]
    fn ranking_list_sorts_by_value_then_lower_concord() {
        let snap = ready_snapshot();
        let list = ranking_list(&snap.unity.rankings[0]);
        assert_eq!(list.len(), UNITY_CONCORDS);
        assert_eq!(
            list.iter()
                .take(3)
                .map(|s| (s.concord, s.rank, s.value))
                .collect::<Vec<_>>(),
            vec![(3, 1, 900), (5, 1, 900), (7, 3, 400)]
        );
        assert_eq!(
            list[3].concord, 1,
            "the zero values follow in Concord order"
        );
    }

    #[test]
    fn rank_text_reads_its_own_entries_then_fills_the_template() {
        let texts = texts();
        assert_eq!(rank_text(&texts, 0).as_deref(), Some("12/434"));
        assert_eq!(rank_text(&texts, 3).as_deref(), Some("12/437"));
        assert_eq!(rank_text(&texts, 4).as_deref(), Some("No.4"));
        assert_eq!(
            points_text(UNITY_VALUE_CAP + 5),
            format!("{UNITY_VALUE_CAP}pt")
        );
    }

    /// `ut_menu` open asks for set 0 then set 1, each only once its table completed since its last
    /// request, and the local mark clears at once so a quick reopen sends nothing.
    #[test]
    fn unity_menu_open_requests_only_completed_sets() {
        let mut snap = ready_snapshot();
        snap.unity.rankings[0].complete = false;
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Status);
        stack.push(MenuKind::StatusDetail { entry: 0 });
        let outcome = open_unity_menu(&mut stack, &mut snap);
        assert_eq!(outcome.requests, vec![UnityFpRequest::UnityRanking(1)]);
        assert_eq!(
            stack.levels.iter().map(|l| l.kind).collect::<Vec<_>>(),
            vec![MenuKind::Root, MenuKind::Status, MenuKind::UnityMenu],
            "the display-only detail closes before ut_menu opens"
        );
        assert!(!snap.unity.rankings[1].complete);

        stack.pop();
        assert!(open_unity_menu(&mut stack, &mut snap).requests.is_empty());

        snap.unity.rankings = [ranking_set([0; 11], [0; 11]); 2];
        stack.pop();
        assert_eq!(
            open_unity_menu(&mut stack, &mut snap).requests,
            vec![
                UnityFpRequest::UnityRanking(0),
                UnityFpRequest::UnityRanking(1)
            ]
        );
    }

    #[test]
    fn quest01_open_requests_once_after_an_s2c_0x110() {
        let mut dynamic = DynamicMenu::default();
        let mut snap = ready_snapshot();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::UnityRankingChooser);
        let outcome = select_unity_fp_row(
            MenuKind::UnityRankingChooser,
            Some(DynamicMenuAction::UnityRankingSet { result_set: 1 }),
            &mut stack,
            &mut dynamic,
            &mut snap,
        );
        assert!(outcome.requests.is_empty(), "no 0x110 since the last 0x117");
        assert_eq!(
            stack.current().unwrap().kind,
            MenuKind::UnityRanking { result_set: 1 }
        );

        snap.unity.quest_armed = true;
        stack.pop();
        let outcome = select_unity_fp_row(
            MenuKind::UnityRankingChooser,
            Some(DynamicMenuAction::UnityRankingSet { result_set: 0 }),
            &mut stack,
            &mut dynamic,
            &mut snap,
        );
        assert_eq!(outcome.requests, vec![UnityFpRequest::UnityQuest]);
        assert!(!snap.unity.quest_armed);
    }

    #[test]
    fn fp_windows_request_points_only_before_any_s2c_0x08e() {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Status);
        let mut snap = SceneSnapshot::default();
        let outcome = open_alter_ego_categories(&mut stack, &mut snap);
        assert_eq!(outcome.requests, vec![UnityFpRequest::AlterEgoPoints]);
        assert_eq!(stack.current().unwrap().kind, MenuKind::AlterEgoCategories);

        let mut dynamic = DynamicMenu::default();
        let outcome = select_unity_fp_row(
            MenuKind::AlterEgoCategories,
            Some(DynamicMenuAction::AlterEgoCategory {
                category: CATEGORY_STATS,
            }),
            &mut stack,
            &mut dynamic,
            &mut snap,
        );
        assert_eq!(
            outcome.requests,
            vec![UnityFpRequest::AlterEgoPoints],
            "fp_cat2 applies the same rule"
        );
        assert_eq!(
            stack.current().unwrap().kind,
            MenuKind::AlterEgoUpgrades {
                category: CATEGORY_STATS
            }
        );

        let mut with_data = fp_snapshot(0, &[]);
        let mut stack = MenuStack::root();
        assert!(open_alter_ego_categories(&mut stack, &mut with_data)
            .requests
            .is_empty());
    }

    #[test]
    fn upgrades_are_allowed_in_a_mog_house_outside_monstrosity() {
        use ffxi_proto::decode::ServerLoginMyroom as L;
        let mut snap = fp_snapshot(0, &[]);
        for state in [
            L::LOGIN_STATE_MYROOM,
            L::LOGIN_STATE_JOBEXIT,
            L::LOGIN_STATE_POLEXIT_MYROOM,
        ] {
            snap.login_state = state;
            assert!(alter_ego_allowed(&snap), "LoginState {state}");
        }
        snap.login_state = L::LOGIN_STATE_GAME;
        assert!(!alter_ego_allowed(&snap));
        snap.login_state = L::LOGIN_STATE_MYROOM;
        snap.login_is_monstrosity = true;
        assert!(!alter_ego_allowed(&snap));
        snap.login_is_monstrosity = false;
        snap.stats = Some(CharStats {
            mjob_no: MONSTROSITY_MAIN_JOB,
            ..Default::default()
        });
        assert!(!alter_ego_allowed(&snap));
    }

    #[test]
    fn upgrade_rows_color_by_state() {
        let snap = fp_snapshot(5, &[(HP, 50, 1), (MP, 10, 2), (STR, 3, 9)]);
        assert_eq!(upgrade_row_state(&snap, HP, 50), UpgradeRowState::AtCap);
        assert_eq!(
            upgrade_row_state(&snap, MP, 50),
            UpgradeRowState::Affordable
        );
        assert_eq!(
            upgrade_row_state(&snap, STR, 30),
            UpgradeRowState::TooCostly
        );
        assert_eq!(
            upgrade_row_state(&SceneSnapshot::default(), HP, 50),
            UpgradeRowState::Unavailable
        );
        assert_eq!(
            upgrade_row_colors(UpgradeRowState::Unavailable),
            (ARGB_DIM, ARGB_DIM, None)
        );
        assert_eq!(
            upgrade_row_colors(UpgradeRowState::AtCap),
            (ARGB_NEUTRAL, ARGB_CAPPED, None)
        );
        assert_eq!(
            upgrade_row_colors(UpgradeRowState::Affordable),
            (ARGB_NEUTRAL, ARGB_NEUTRAL, Some(ARGB_NEUTRAL))
        );
        assert_eq!(
            upgrade_row_colors(UpgradeRowState::TooCostly),
            (ARGB_NEUTRAL, ARGB_NEUTRAL, Some(ARGB_DIM))
        );
    }

    fn select_upgrade(
        snap: &mut SceneSnapshot,
        dynamic: &mut DynamicMenu,
        stack: &mut MenuStack,
        row: Option<(u16, u8)>,
    ) -> UnityFpOutcome {
        select_unity_fp_row(
            MenuKind::AlterEgoUpgrades {
                category: CATEGORY_STATS,
            },
            row.map(|(upgrade, cap)| DynamicMenuAction::AlterEgoUpgrade { upgrade, cap }),
            stack,
            dynamic,
            snap,
        )
    }

    fn upgrades_stack() -> MenuStack {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::AlterEgoCategories);
        stack.push(MenuKind::AlterEgoUpgrades {
            category: CATEGORY_STATS,
        });
        stack
    }

    /// First match wins: no row closes `fp_cat2`; not allowed prints the table 7 line then
    /// buzzes; capped, dataless or unaffordable only buzz; otherwise the prompt opens on No.
    #[test]
    fn upgrade_select_refusal_order() {
        let mut dynamic = DynamicMenu::default();
        dynamic.unity_fp.not_allowed_message = Some("not here".into());

        let mut stack = upgrades_stack();
        let mut snap = fp_snapshot(5, &[(MP, 10, 2)]);
        assert_eq!(
            select_upgrade(&mut snap, &mut dynamic, &mut stack, None),
            UnityFpOutcome::default()
        );
        assert_eq!(stack.current().unwrap().kind, MenuKind::AlterEgoCategories);

        let mut stack = upgrades_stack();
        let mut outside = fp_snapshot(5, &[(MP, 10, 2)]);
        outside.login_state = ffxi_proto::decode::ServerLoginMyroom::LOGIN_STATE_GAME;
        let outcome = select_upgrade(&mut outside, &mut dynamic, &mut stack, Some((MP, 50)));
        assert_eq!(outcome.chat_lines, vec!["not here".to_string()]);
        assert_eq!(
            dynamic.ui_sounds.drain(..).collect::<Vec<_>>(),
            vec![UiSound::Buzzer]
        );

        for (mut snap, row) in [
            (fp_snapshot(5, &[(HP, 50, 1)]), (HP, 50)),
            (SceneSnapshot::default(), (MP, 50)),
            (fp_snapshot(1, &[(STR, 3, 9)]), (STR, 30)),
        ] {
            snap.login_state = ffxi_proto::decode::ServerLoginMyroom::LOGIN_STATE_MYROOM;
            let outcome = select_upgrade(&mut snap, &mut dynamic, &mut stack, Some(row));
            assert!(outcome.chat_lines.is_empty());
            assert_eq!(
                dynamic.ui_sounds.drain(..).collect::<Vec<_>>(),
                vec![UiSound::Buzzer]
            );
            assert_eq!(stack.levels.len(), 3, "nothing opens");
        }

        let outcome = select_upgrade(&mut snap, &mut dynamic, &mut stack, Some((MP, 50)));
        assert_eq!(outcome, UnityFpOutcome::default());
        let top = stack.current().unwrap();
        assert_eq!(top.kind, MenuKind::AlterEgoConfirm { upgrade: MP });
        assert_eq!(top.cursor, CONFIRM_NO_ROW, "the prompt opens on No");

        let depth = stack.levels.len();
        stack.levels.swap(depth - 1, depth - 2);
        select_upgrade(&mut snap, &mut dynamic, &mut stack, Some((MP, 50)));
        assert_eq!(stack.levels.len(), depth, "a pending prompt blocks another");
    }

    #[test]
    fn missing_refusal_text_prints_its_id_never_prose() {
        let mut dynamic = DynamicMenu::default();
        let mut stack = upgrades_stack();
        let mut outside = fp_snapshot(5, &[(MP, 10, 2)]);
        outside.login_state = 0;
        let outcome = select_upgrade(&mut outside, &mut dynamic, &mut stack, Some((MP, 50)));
        assert_eq!(
            outcome.chat_lines,
            vec![client_message_fallback(
                GETTEXT_TABLE_CLIENT_MESSAGES,
                NOT_ALLOWED_MESSAGE_ID
            )]
        );
    }

    #[test]
    fn confirm_yes_sends_c2s_0x0c1_and_no_sends_nothing() {
        let mut dynamic = DynamicMenu::default();
        let mut snap = fp_snapshot(5, &[(MP, 10, 2)]);
        for (yes, want) in [
            (true, vec![UnityFpRequest::UpgradeAlterEgo(MP)]),
            (false, Vec::new()),
        ] {
            let mut stack = upgrades_stack();
            stack.push(MenuKind::AlterEgoConfirm { upgrade: MP });
            let outcome = select_unity_fp_row(
                MenuKind::AlterEgoConfirm { upgrade: MP },
                Some(DynamicMenuAction::AlterEgoConfirmAnswer { yes }),
                &mut stack,
                &mut dynamic,
                &mut snap,
            );
            assert_eq!(outcome.requests, want);
            assert_eq!(
                stack.current().unwrap().kind,
                MenuKind::AlterEgoUpgrades {
                    category: CATEGORY_STATS
                },
                "either answer closes the prompt"
            );
        }
    }

    /// The first change goes out on the next frame; a change inside the 30-frame countdown waits
    /// for it; the opening value counts as sent.
    #[test]
    fn chat_toggle_throttles_sends_to_one_per_thirty_frames() {
        let mut toggle = UnityChatToggle::open(0);
        assert_eq!(toggle.tick(1), None, "unchanged");
        toggle.mode = ffxi_proto::map::unity_toggle_mode::ACTIVE;
        assert_eq!(
            toggle.tick(1),
            Some(ffxi_proto::map::unity_toggle_mode::ACTIVE)
        );
        toggle.mode = ffxi_proto::map::unity_toggle_mode::INACTIVE;
        assert_eq!(
            toggle.tick(UNITY_CHAT_SEND_FRAMES - 1),
            None,
            "countdown running"
        );
        assert_eq!(
            toggle.tick(1),
            Some(ffxi_proto::map::unity_toggle_mode::INACTIVE)
        );
        let mut by_time = UnityChatToggle::open(1);
        by_time.mode = ffxi_proto::map::unity_toggle_mode::INACTIVE;
        assert_eq!(by_time.advance(0.0), None, "no whole frame yet");
        let one_frame = 1.0 / crate::scheduler_runtime::RETAIL_FPS;
        assert_eq!(
            by_time.advance(one_frame),
            Some(ffxi_proto::map::unity_toggle_mode::INACTIVE)
        );
    }

    /// Opening the chat panel reads option 178; closing it stores the mode even when the last
    /// change never went out.
    #[test]
    fn chat_panel_opens_on_the_option_and_stores_it_on_close() {
        let mut dynamic = DynamicMenu::default();
        dynamic.config_option_values.insert(UNITY_CHAT_OPTION, 1);
        let snap = SceneSnapshot::default();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::UnityMenu);
        stack.push(MenuKind::UnityChat);
        sync_unity_fp_windows(&mut dynamic, Some(&stack), &snap);
        assert_eq!(
            dynamic.unity_fp.chat.map(|c| c.mode),
            Some(ffxi_proto::map::unity_toggle_mode::ACTIVE)
        );

        let mut scratch = snap.clone();
        select_unity_fp_row(
            MenuKind::UnityChat,
            Some(DynamicMenuAction::UnityChatMode {
                mode: ffxi_proto::map::unity_toggle_mode::INACTIVE,
            }),
            &mut stack,
            &mut dynamic,
            &mut scratch,
        );
        assert_eq!(
            stack.current().unwrap().kind,
            MenuKind::UnityChat,
            "stays open"
        );
        stack.pop();
        sync_unity_fp_windows(&mut dynamic, Some(&stack), &snap);
        assert_eq!(dynamic.unity_fp.chat, None);
        assert_eq!(
            dynamic.config_option_values.get(&UNITY_CHAT_OPTION),
            Some(&i32::from(ffxi_proto::map::unity_toggle_mode::INACTIVE))
        );
    }

    /// Rank and value freeze on the first ready frame after the panel opens; the ranking list
    /// builds once while ready and is not rebuilt while it holds rows.
    #[test]
    fn standings_freeze_and_the_ranking_list_builds_once() {
        let mut dynamic = DynamicMenu::default();
        let mut snap = ready_snapshot();
        snap.unity.rankings[0].complete = false;
        snap.stats = Some(CharStats {
            unity_faction: 5,
            ..Default::default()
        });
        let mut stack = MenuStack::root();
        stack.push(MenuKind::UnityChat);
        sync_unity_fp_windows(&mut dynamic, Some(&stack), &snap);
        assert_eq!(dynamic.unity_fp.chat_standings, None, "not ready");

        snap.unity.rankings[0].complete = true;
        sync_unity_fp_windows(&mut dynamic, Some(&stack), &snap);
        let frozen = dynamic.unity_fp.chat_standings.expect("first ready frame");
        assert_eq!(
            frozen[0],
            Some(ConcordStanding {
                concord: 5,
                rank: 1,
                value: 900
            })
        );
        snap.unity.rankings[0].points[4] = 0;
        sync_unity_fp_windows(&mut dynamic, Some(&stack), &snap);
        assert_eq!(
            dynamic.unity_fp.chat_standings,
            Some(frozen),
            "frozen until reopen"
        );

        stack.push(MenuKind::UnityRanking { result_set: 1 });
        sync_unity_fp_windows(&mut dynamic, Some(&stack), &snap);
        let built = dynamic.unity_fp.ranking.clone().expect("built while ready");
        assert_eq!(built.0, 1);
        snap.unity.rankings[1].points = [0; UNITY_CONCORDS];
        sync_unity_fp_windows(&mut dynamic, Some(&stack), &snap);
        assert_eq!(
            dynamic.unity_fp.ranking,
            Some(built),
            "not rebuilt while non-empty"
        );

        stack.pop();
        stack.pop();
        sync_unity_fp_windows(&mut dynamic, Some(&stack), &snap);
        assert_eq!(dynamic.unity_fp.ranking, None);
        assert_eq!(dynamic.unity_fp.chat_standings, None);
    }

    #[test]
    fn ranking_left_right_switch_sets_only_when_both_are_complete() {
        let mut dynamic = DynamicMenu::default();
        let mut snap = ready_snapshot();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::UnityRanking { result_set: 0 });
        assert!(switch_ranking_set(&mut stack, &mut dynamic, &snap));
        assert_eq!(
            stack.current().unwrap().kind,
            MenuKind::UnityRanking { result_set: 1 }
        );
        assert_eq!(dynamic.unity_fp.ranking.as_ref().map(|r| r.0), Some(1));
        assert_eq!(dynamic.ui_sounds, vec![UiSound::Cursor]);

        snap.unity.rankings[0].complete = false;
        assert!(!switch_ranking_set(&mut stack, &mut dynamic, &snap));
    }

    #[test]
    fn leader_row_select_closes_the_list_with_the_confirm_sound() {
        let mut dynamic = DynamicMenu::default();
        let mut snap = SceneSnapshot::default();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::UnityMenu);
        stack.push(MenuKind::UnityLeaders);
        select_unity_fp_row(
            MenuKind::UnityLeaders,
            Some(DynamicMenuAction::UnityLeaderRow { concord: 0 }),
            &mut stack,
            &mut dynamic,
            &mut snap,
        );
        assert_eq!(stack.current().unwrap().kind, MenuKind::UnityMenu);
        assert_eq!(dynamic.ui_sounds, vec![UiSound::Confirm]);
    }

    #[test]
    fn ut_menu_items_open_their_windows_without_a_request() {
        let mut dynamic = DynamicMenu::default();
        let mut snap = ready_snapshot();
        for (item, want) in [
            (UT_MENU_ITEM_CHAT, MenuKind::UnityChat),
            (UT_MENU_ITEM_RANKING, MenuKind::UnityRankingChooser),
            (UT_MENU_ITEM_LEADERS, MenuKind::UnityLeaders),
        ] {
            let mut stack = MenuStack::root();
            stack.push(MenuKind::UnityMenu);
            let outcome = select_unity_fp_row(
                MenuKind::UnityMenu,
                Some(DynamicMenuAction::UnityMenuItem { item }),
                &mut stack,
                &mut dynamic,
                &mut snap,
            );
            assert_eq!(outcome, UnityFpOutcome::default());
            assert_eq!(stack.current().unwrap().kind, want);
        }
        assert!(dynamic.ui_sounds.is_empty());
    }

    fn level(kind: MenuKind) -> MenuLevel {
        MenuLevel::new(kind, 0)
    }

    fn labels(rows: &[DynamicMenuRow]) -> Vec<&str> {
        rows.iter().map(|r| r.label.as_str()).collect()
    }

    #[test]
    fn rows_are_the_handlers_items_labelled_from_dat() {
        let dat = dat();
        let windows = UnityFpWindows::default();
        let rows = |kind| unity_fp_rows(&level(kind), &dat, &windows);
        assert_eq!(
            labels(&rows(MenuKind::UnityMenu)),
            vec!["ut item 1", "ut item 2", "ut item 3"],
            "three live items; the trailing dummy item gets no row"
        );
        assert_eq!(labels(&rows(MenuKind::UnityChat)), vec!["12/440", "12/441"]);
        assert_eq!(
            rows(MenuKind::UnityChat)[0].action,
            DynamicMenuAction::UnityChatMode {
                mode: ffxi_proto::map::unity_toggle_mode::ACTIVE
            }
        );
        assert_eq!(
            labels(&rows(MenuKind::UnityRankingChooser)),
            vec!["12/400", "12/401"]
        );
        let leaders = rows(MenuKind::UnityLeaders);
        assert_eq!(leaders.len(), layout::LEADER_ROWS);
        assert_eq!(leaders[0].label, "12/442");
        assert_eq!(leaders[1].label, "12/403");
        assert_eq!(leaders[11].label, "12/413");
        assert_eq!(
            labels(&rows(MenuKind::AlterEgoConfirm { upgrade: HP })),
            vec!["ut item 1", "ut item 2"]
        );
    }

    #[test]
    fn fp_rows_follow_the_category_table() {
        let dat = dat();
        let windows = UnityFpWindows::default();
        let categories = unity_fp_rows(&level(MenuKind::AlterEgoCategories), &dat, &windows);
        assert_eq!(
            categories.iter().map(|r| r.action).collect::<Vec<_>>(),
            vec![DynamicMenuAction::AlterEgoCategory {
                category: CATEGORY_STATS
            }],
            "id 0 and a record with no first upgrade are not listed"
        );
        assert_eq!(categories[0].label, format!("name {CATEGORY_STATS}"));
        let upgrades = unity_fp_rows(
            &level(MenuKind::AlterEgoUpgrades {
                category: CATEGORY_STATS,
            }),
            &dat,
            &windows,
        );
        assert_eq!(
            upgrades.iter().map(|r| r.action).collect::<Vec<_>>(),
            vec![
                DynamicMenuAction::AlterEgoUpgrade {
                    upgrade: HP,
                    cap: 50
                },
                DynamicMenuAction::AlterEgoUpgrade {
                    upgrade: MP,
                    cap: 50
                },
                DynamicMenuAction::AlterEgoUpgrade {
                    upgrade: STR,
                    cap: 30
                },
            ],
            "empty slots skip"
        );
    }

    fn texts_of(windows: &[PanelWindow], name: &str) -> Vec<PanelText> {
        windows
            .iter()
            .find(|w| w.name == name)
            .map(|w| w.texts.clone())
            .unwrap_or_default()
    }

    fn find<'a>(texts: &'a [PanelText], text: &str) -> &'a PanelText {
        texts
            .iter()
            .find(|t| t.text == text)
            .unwrap_or_else(|| panic!("no {text:?} in {texts:?}"))
    }

    #[test]
    fn ut_point_docks_to_the_party_slot_and_draws_its_readout() {
        let dat = dat();
        let snap = ready_snapshot();
        let windows = UnityFpWindows::default();
        let cx = PanelContext {
            snap: &snap,
            dat: &dat,
            windows: &windows,
        };
        let mut stack = MenuStack::root();
        stack.push(MenuKind::UnityMenu);
        let drawn = panel_windows(&stack, |_| Vec::new(), &cx);
        let names: Vec<&str> = drawn.iter().map(|w| w.name).collect();
        assert_eq!(names, vec![UT_MENU_WINDOW, UT_POINT_WINDOW]);
        assert_eq!(drawn[1].dock, Some(crate::hud::dock::DockSlot::Party));
        let readout = find(&drawn[1].texts, &format!("0/{UT_POINT_MAX}"));
        assert_eq!(
            (readout.x, readout.y, readout.align),
            (
                layout::UT_POINT_RIGHT,
                layout::UT_POINT_TOP,
                TextAlign::Right
            )
        );
    }

    #[test]
    fn chat_panel_draws_not_ready_lines_then_the_standings() {
        let dat = dat();
        let mut snap = ready_snapshot();
        snap.unity.rankings[1].complete = false;
        let mut windows = UnityFpWindows::default();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::UnityChat);
        let draw = |snap: &SceneSnapshot, windows: &UnityFpWindows, stack: &MenuStack| {
            let cx = PanelContext {
                snap,
                dat: &dat,
                windows,
            };
            texts_of(&panel_windows(stack, |_| Vec::new(), &cx), QUEST00_WINDOW)
        };
        let waiting = draw(&snap, &windows, &stack);
        let first = find(&waiting, "9/97");
        assert_eq!((first.x, first.y), layout::CHAT_NOT_READY[0]);
        assert_eq!(
            (find(&waiting, "12/395").x, find(&waiting, "12/395").y),
            layout::CHAT_TITLE
        );

        snap.unity.rankings[1].complete = true;
        let no_concord = draw(&snap, &windows, &stack);
        assert!(no_concord.iter().all(|t| !t.text.starts_with("9/")));
        assert_eq!(find(&no_concord, "12/402").x, layout::CHAT_FACTION_X);
        assert_eq!(
            no_concord.iter().filter(|t| t.text == "12/434").count(),
            2,
            "no Concord: rank 0 text in both set rows and nothing else"
        );
        assert!(no_concord.iter().all(|t| !t.text.ends_with("pt")));

        snap.stats = Some(CharStats {
            unity_faction: 5,
            unity_points1: 12,
            unity_points2: 34,
            ..Default::default()
        });
        let mut dynamic = DynamicMenu::default();
        sync_unity_fp_windows(&mut dynamic, Some(&stack), &snap);
        windows = dynamic.unity_fp.clone();
        let standing = draw(&snap, &windows, &stack);
        let value = find(&standing, "900pt");
        assert_eq!(
            (value.x, value.align),
            (layout::CHAT_VALUE_RIGHT, TextAlign::Right)
        );
        assert_eq!(
            find(&standing, "34pt").y,
            layout::CHAT_LINE_YS[3],
            "previous week first"
        );
        assert_eq!(find(&standing, "12pt").y, layout::CHAT_LINE_YS[4]);

        let row = find(&standing, "12/440");
        let quest00_height = 190.0;
        let item_height = 14.0;
        assert_eq!(
            row.y,
            quest00_height - layout::CHAT_ROW_BOTTOM_INSETS[0] - item_height,
            "item 1's bottom edge sits 35 above the window bottom"
        );
        assert_eq!(row.x, 30.0 + layout::CHAT_ROW_TEXT_X);
    }

    #[test]
    fn fp_list_colors_the_cursor_name_and_the_cells() {
        let dat = dat();
        let snap = fp_snapshot(5, &[(HP, 50, 1), (MP, 10, 2), (STR, 3, 9)]);
        let windows = UnityFpWindows::default();
        let cx = PanelContext {
            snap: &snap,
            dat: &dat,
            windows: &windows,
        };
        let kind = MenuKind::AlterEgoUpgrades {
            category: CATEGORY_STATS,
        };
        let rows = unity_fp_rows(&level(kind), &dat, &windows);
        let mut stack = MenuStack::root();
        stack.push(MenuKind::AlterEgoCategories);
        stack.push(kind);
        let drawn = panel_windows(
            &stack,
            |i| if i == 2 { rows.clone() } else { Vec::new() },
            &cx,
        );
        let texts = texts_of(&drawn, FP_CAT2_WINDOW);
        let hp = find(&texts, &format!("name {HP}"));
        assert_eq!(hp.color, ARGB_FP_CURSOR, "the cursor row's name");
        assert_eq!(
            (hp.x, hp.y),
            (
                layout::FP_NAME_X,
                layout::FP_CAT2_FIRST_ROW_TOP + layout::FP_TEXT_DROP
            )
        );
        assert_eq!(find(&texts, "/50").color, ARGB_CAPPED, "HP is at its cap");
        assert!(
            texts.iter().all(|t| t.y != hp.y || t.text != cost_text(1)),
            "a capped row hides its cost cell"
        );
        let str_cost = find(&texts, &cost_text(9));
        assert_eq!(
            (str_cost.x, str_cost.align, str_cost.color),
            (layout::FP_COST_RIGHT, TextAlign::Right, ARGB_DIM)
        );
        let names: Vec<&str> = drawn.iter().map(|w| w.name).collect();
        assert_eq!(
            names,
            vec![
                FP_CAT_WINDOW,
                FP_INFO_WINDOW,
                BLUEHELP_WINDOW,
                FP_CAT2_WINDOW
            ]
        );
        let help = texts_of(&drawn, BLUEHELP_WINDOW);
        assert_eq!(
            help.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            vec![format!("name {HP}"), format!("about {HP}")],
            "the description box follows fp_cat2's highlighted upgrade"
        );
    }

    /// The alter ego upgrade prompt formats entry 123 with the upgrade's name and its s2c 0x08E
    /// cost, drawn at `merityn`'s record place.
    #[test]
    fn upgrade_prompt_formats_its_entry_at_the_record_place() {
        const COST: u16 = 2;
        let dat = dat();
        let snap = fp_snapshot(5, &[(MP, 10, COST)]);
        let windows = UnityFpWindows::default();
        let cx = PanelContext {
            snap: &snap,
            dat: &dat,
            windows: &windows,
        };
        let name = format!("name {MP}");
        assert_eq!(
            upgrade_prompt(&cx, MP),
            ConfirmPrompt {
                entry: ALTER_EGO_PROMPT_ENTRY,
                args: vec![PromptArg::Name(name.clone()), PromptArg::Points(COST)],
            }
        );
        let mut stack = upgrades_stack();
        stack.push(MenuKind::AlterEgoConfirm { upgrade: MP });
        let drawn = panel_windows(&stack, |_| Vec::new(), &cx);
        let texts = texts_of(&drawn, MERITYN_WINDOW);
        let prompt = find(
            &texts,
            &prompt_text(ALTER_EGO_PROMPT_ENTRY, &name, Some(COST)),
        );
        assert_eq!(
            (prompt.x, prompt.y),
            (layout::MERITYN_TEXT_INSET, layout::MERITYN_TEXT_INSET)
        );
        let width = f32::from(cx.frame(MERITYN_WINDOW).expect("merityn record").width);
        assert_eq!(prompt.wrap_width, Some(width - layout::MERITYN_WRAP_MARGIN));
        assert_eq!(prompt.line_pitch, Some(layout::MERITYN_LINE_PITCH));
        assert_eq!(prompt.color, ARGB_NEUTRAL);
    }

    #[test]
    fn lists_scroll_the_least_that_keeps_the_cursor_visible() {
        assert_eq!(scrolled_start(0, 9, 10), 0);
        assert_eq!(scrolled_start(0, 10, 10), 1);
        assert_eq!(scrolled_start(4, 2, 10), 2);
        assert_eq!(scrolled_start(3, 7, 5), 3);
    }

    #[test]
    fn every_kind_names_its_record() {
        use crate::hud::menu::record_window_name;
        for (kind, name) in [
            (MenuKind::UnityMenu, UT_MENU_WINDOW),
            (MenuKind::UnityChat, QUEST00_WINDOW),
            (MenuKind::UnityRankingChooser, QUEST00_WINDOW),
            (MenuKind::UnityRanking { result_set: 0 }, QUEST01_WINDOW),
            (MenuKind::UnityLeaders, AREALIST_WINDOW),
            (MenuKind::AlterEgoCategories, FP_CAT_WINDOW),
            (MenuKind::AlterEgoUpgrades { category: 1 }, FP_CAT2_WINDOW),
            (MenuKind::AlterEgoConfirm { upgrade: HP }, MERITYN_WINDOW),
        ] {
            assert_eq!(record_window_name(kind), Some(name));
            assert!(crate::hud::menu::is_dynamic(kind));
        }
    }
}
