//! The Merit Points windows (`merit1` chain, Status row id 7) and the Job Points windows
//! (`merit2ca` chain, Status row id 8): rows from the merit and job point DAT tables, their gates,
//! colors and refusals, the raise / lower and spend flows, and the text each window draws
//! (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use ffxi_dat::menu_windows::{FrameBlock, MenuWindow};
use ffxi_dat::merits::{JobPointCategory, MeritCategory, MeritDefinition};
use kuluu_snapshot::{JobPointType, JobPointsRecord, MeritEntry, SceneSnapshot};

use crate::hud::gettext::{
    ConfirmPrompt, GetText, GetTextTables, PromptArg, GETTEXT_TABLE_10, GETTEXT_TABLE_12,
    GETTEXT_TABLE_CAPTIONS, GETTEXT_TABLE_CLIENT_MESSAGES, GETTEXT_TABLE_SHORT_CAPTIONS,
    GETTEXT_TABLE_SUB_TEMPLATES,
};
use crate::hud::menu::{DynamicMenu, DynamicMenuAction, DynamicMenuRow, UiSound};
use crate::hud::status_panel::MASTER_LEVEL_JOB_CAP;
use crate::hud::unity_fp::{
    self as kit, cost_text, retail_argb, PanelText, PanelWindow, UpgradeRowState, ARGB_CAPPED,
    ARGB_DIM, ARGB_FP_CURSOR, ARGB_NEUTRAL, BLUEHELP_WINDOW, CONFIRM_ITEM_YES, MERITYN_WINDOW,
    TITLEWIN_WINDOW,
};
use crate::input_mode::{ConfirmDefault, MenuKind, MenuLevel, MenuStack};

pub const MERIT1_WINDOW: &str = "merit1";
pub const MERIT2_WINDOW: &str = "merit2";
pub const MERIT3_WINDOW: &str = "merit3";
pub const MERITCAT_WINDOW: &str = "meritcat";
pub const MERITINF_WINDOW: &str = "meritinf";
pub const LEVMERIT_WINDOW: &str = "levmerit";
pub const SORTYN_WINDOW: &str = "sortyn";
pub const MERIT2CA_WINDOW: &str = "merit2ca";
pub const JBPCAT_WINDOW: &str = "jbpcat";
pub const LEVMERI2_WINDOW: &str = "levmeri2";

/// Every window record these windows draw from.
pub const MERIT_WINDOWS: [&str; 13] = [
    MERIT1_WINDOW,
    MERIT2_WINDOW,
    MERIT3_WINDOW,
    MERITCAT_WINDOW,
    MERITINF_WINDOW,
    LEVMERIT_WINDOW,
    SORTYN_WINDOW,
    MERIT2CA_WINDOW,
    JBPCAT_WINDOW,
    LEVMERI2_WINDOW,
    MERITYN_WINDOW,
    BLUEHELP_WINDOW,
    TITLEWIN_WINDOW,
];

/// `merit1` DAT items: 1 opens `meritcat`, 2 opens `merit2`.
pub const MERIT1_ITEM_CATEGORIES: u8 = 1;
pub const MERIT1_ITEM_MODE: u8 = 2;
/// `merit2` DAT items: 1 asks for EXP mode, 2 for Limit Points mode.
pub const MERIT2_ITEM_EXP: u8 = 1;
pub const MERIT2_ITEM_LIMIT_POINTS: u8 = 2;
/// `merit3` DAT items: 1 Raise, 2 Lower.
pub const MERIT3_ITEM_RAISE: u8 = 1;
pub const MERIT3_ITEM_LOWER: u8 = 2;

/// Status effects (level_restriction, besieged, level_sync) under which `merit1` item 2 refuses.
pub const MODE_REFUSING_EFFECTS: [u16; 3] = [143, 257, 269];

/// GetText table 7 line printed when `merit1` item 2 refuses.
pub const MODE_REFUSED_MESSAGE_ID: u16 = 187;
/// GetText table 13 (0x0D) entry titling the raise and lower prompts.
pub const PROMPT_TITLE_ID: u16 = 130;
/// GetText table 14 (0x0E) `sortyn` help lines for raising and lowering.
pub const SORT_HELP_RAISE_ID: u16 = 612;
pub const SORT_HELP_LOWER_ID: u16 = 613;
/// GetText table 10 prompt entries: raise (name, cost), lower (name), job point spend (name,
/// cost).
pub const RAISE_PROMPT_ENTRY: u16 = 80;
pub const LOWER_PROMPT_ENTRY: u16 = 81;
pub const SPEND_PROMPT_ENTRY: u16 = 116;

/// Merit categories 1..=5 and the weapon skill category hold one section, the category itself.
pub const SINGLE_SECTION_CATEGORIES: RangeInclusive<u16> = 1..=5;
pub const WEAPON_SKILL_SECTION: u16 = 26;
/// Every other (job) category `c` holds sections `c`, `c + 26` and `c + 58`, in that order.
pub const JOB_SECTION_OFFSETS: [u16; 3] = [0, 26, 58];
/// Section `s` lists merit indices `32 s ..= 32 s + 31`.
pub const MERITS_PER_SECTION: u16 = 32;
/// `meritcat`'s top level lists category record numbers 1..=31.
pub const TOP_LEVEL_RECORDS: RangeInclusive<usize> = 1..=31;
/// The weapon skill section needs this main job level and key item 2059 (HEART_OF_THE_BUSHIN).
pub const WEAPON_SKILL_LEVEL: u8 = 96;
pub const HEART_OF_THE_BUSHIN_KEY_ITEM: u16 = 2059;
/// Key items 3224 (PRIMER_ON_MARTIAL_TECHNIQUES) and 3225 (TREATISE_ON_MARTIAL_TECHNIQUES) each
/// raise the weapon skill section cap, which stops at [`WEAPON_SKILL_CAP_MAX`].
pub const MARTIAL_TECHNIQUE_KEY_ITEMS: [u16; 2] = [3224, 3225];
pub const MARTIAL_TECHNIQUE_CAP_BONUS: u8 = 5;
pub const WEAPON_SKILL_CAP_MAX: u8 = 100;

/// Merit name colors by section order inside a category.
pub const SECTION_COLORS: [u32; 3] = [
    ARGB_NEUTRAL,
    retail_argb(0x80, 0x90, 0x90, 0x40),
    retail_argb(0x80, 0x50, 0x90, 0x50),
];

/// Job point windows open for jobs 1..=22.
pub const JOB_POINT_JOBS: RangeInclusive<u8> = 1..=22;
pub const LIMIT_POINT_MAX: u32 = 10_000;
pub const CAPACITY_POINT_MAX: u32 = 30_000;
pub const JOB_POINT_MAX: u16 = 500;
/// Refresh passes a confirmed spend locks the job point prompt for.
pub const SPEND_LOCKOUT_PASSES: u32 = 30;
/// Under Monstrosity `levmerit` opens this far below its record frame.
pub const LEVMERIT_MONSTROSITY_DROP: f32 = 20.0;

/// Client format strings for the readouts without a Merits record.
pub const NO_RECORD_LIMIT_POINTS: &str = "----";
pub const NO_RECORD_MERIT_POINTS: &str = "-";

/// Layout of the windows, in window-relative design units.
pub mod layout {
    /// Readout numbers end this far left of their window's right edge.
    pub const READOUT_RIGHT_INSET: f32 = 8.0;
    /// `levmerit` and `levmeri2` readout lines.
    pub const READOUT_FIRST_TOP: f32 = 28.0;
    pub const READOUT_SECOND_TOP: f32 = 68.0;
    /// `meritinf` lines, and how far left of the right edge its second section total ends.
    pub const MERITINF_POINTS_TOP: f32 = 14.0;
    pub const MERITINF_TOTAL_TOP: f32 = 40.0;
    pub const MERITINF_SECOND_RIGHT_INSET: f32 = 64.0;

    /// `meritcat` and `merit2ca`: visible row k sits at `4 + 16 k`.
    pub const LIST_FIRST_ROW_TOP: f32 = 4.0;
    pub const LIST_ROW_PITCH: f32 = 16.0;
    pub const LIST_VISIBLE_ROWS: usize = 11;
    /// `jbpcat`: visible row k sits at `17 + 17 k`.
    pub const PAGE_FIRST_ROW_TOP: f32 = 17.0;
    pub const PAGE_ROW_PITCH: f32 = 17.0;
    pub const PAGE_VISIBLE_ROWS: usize = 10;
    /// Names sit this far below their row top, numbers this far.
    pub const NAME_DROP: f32 = 2.0;
    pub const NUMBER_DROP: f32 = 4.0;

    pub const NAME_X: f32 = 6.0;
    /// `meritcat` cells.
    pub const COUNT_RIGHT: f32 = 238.0;
    pub const COST_CAPTION_X: f32 = 260.0;
    pub const COST_RIGHT: f32 = 318.0;
    pub const HEADER_CAP_RIGHT: f32 = 340.0;
    /// `jbpcat` Job Points cells.
    pub const LEVEL_RIGHT: f32 = 252.0;
    pub const MAX_X: f32 = 253.0;
    pub const PAGE_COST_CAPTION_X: f32 = 290.0;
    pub const PAGE_COST_RIGHT: f32 = 357.0;
    /// `jbpcat` Gifts cells and the job's spent total.
    pub const GIFT_THRESHOLD_RIGHT: f32 = 342.0;
    pub const GIFTS_SPENT_AT: (f32, f32) = (258.0, 4.0);
}

/// One open merit or job point window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeritWindow {
    /// `merit1`.
    Menu,
    /// `merit2`, the EXP / Limit Points mode selector.
    Mode,
    /// `meritcat`: the category list, or the sections of the entered category.
    Categories { entered: Option<u16> },
    /// `merit3` for one merit index.
    RaiseLower { index: u16 },
    /// `sortyn` asking whether to raise (`raise`) or lower merit `index`.
    SortConfirm { index: u16, raise: bool },
    /// `merityn` confirming that raise or lower.
    Confirm { index: u16, raise: bool },
    /// `merit2ca`, the job list.
    Jobs,
    /// `jbpcat` for one job: its Job Points page, or its Gifts page.
    JobCategories { job: u8, gifts: bool },
    /// `merityn` confirming a job point spend.
    JobPointConfirm { job: u8, type_id: u16 },
}

impl MeritWindow {
    pub fn record_name(self) -> &'static str {
        match self {
            Self::Menu => MERIT1_WINDOW,
            Self::Mode => MERIT2_WINDOW,
            Self::Categories { .. } => MERITCAT_WINDOW,
            Self::RaiseLower { .. } => MERIT3_WINDOW,
            Self::SortConfirm { .. } => SORTYN_WINDOW,
            Self::Confirm { .. } | Self::JobPointConfirm { .. } => MERITYN_WINDOW,
            Self::Jobs => MERIT2CA_WINDOW,
            Self::JobCategories { .. } => JBPCAT_WINDOW,
        }
    }

    /// The Merit Points chain, as opposed to the Job Points chain.
    pub fn is_merit_chain(self) -> bool {
        !matches!(
            self,
            Self::Jobs | Self::JobCategories { .. } | Self::JobPointConfirm { .. }
        )
    }
}

pub fn merit_window(kind: MenuKind) -> Option<MeritWindow> {
    match kind {
        MenuKind::Merit(window) => Some(window),
        _ => None,
    }
}

pub fn is_merit_window(kind: MenuKind) -> bool {
    merit_window(kind).is_some()
}

pub fn merit_window_name(kind: MenuKind) -> Option<&'static str> {
    merit_window(kind).map(MeritWindow::record_name)
}

/// Rows a scrolling list shows at once.
pub fn visible_list_rows(kind: MenuKind) -> Option<usize> {
    match merit_window(kind)? {
        MeritWindow::Categories { .. } | MeritWindow::Jobs => Some(layout::LIST_VISIBLE_ROWS),
        MeritWindow::JobCategories { .. } => Some(layout::PAGE_VISIBLE_ROWS),
        _ => None,
    }
}

/// What a merit or job point window row stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeritAction {
    /// A selector's DAT item and whether its window enables it.
    Item { item: u8, enabled: bool },
    /// A `meritcat` category row: its record's byte-0 id.
    Category { id: u16 },
    /// A section header row.
    SectionHeader,
    /// A merit row.
    Merit { index: u16 },
    /// A `merit2ca` job row.
    Job { job: u8 },
    /// A `jbpcat` Job Points row.
    JobPointType { type_id: u16 },
    /// A `jbpcat` Gifts row.
    Gift,
}

/// The merit and job point tables, read from the install.
#[derive(Debug, Clone, Default)]
pub struct MeritTables {
    pub categories: Vec<MeritCategory>,
    pub definitions: Vec<u8>,
    pub merit_strings: Vec<String>,
    pub job_categories: Vec<JobPointCategory>,
    pub type_definitions: Vec<u8>,
    pub job_strings: Vec<String>,
    pub gift_strings: Vec<String>,
    /// Gift thresholds by job, for every job a job category record names.
    pub gift_thresholds: BTreeMap<u8, Vec<u16>>,
}

fn paired(strings: &[String], id: u16, description: bool) -> Option<&str> {
    let entry = if description {
        ffxi_dat::dmsg::paired_description_entry(id)
    } else {
        ffxi_dat::dmsg::paired_name_entry(id)
    };
    strings.get(entry).map(String::as_str)
}

impl MeritTables {
    pub fn load(dat: &ffxi_dat::archive::DatRoot) -> Self {
        use ffxi_dat::merits as m;
        let job_categories = kit::read_dat_file(dat, m::JOB_CATEGORY_FILE_ID)
            .map(|b| m::parse_job_categories(&b))
            .unwrap_or_default();
        let gift_thresholds = job_categories
            .iter()
            .filter(|record| record.job != 0)
            .filter_map(|record| {
                let bytes = kit::read_dat_file(dat, m::gift_threshold_file_id(record.job))?;
                Some((record.job, m::parse_gift_thresholds(&bytes)))
            })
            .collect();
        Self {
            categories: kit::read_dat_file(dat, m::MERIT_CATEGORY_FILE_ID)
                .map(|b| m::parse_categories(&b))
                .unwrap_or_default(),
            definitions: kit::read_dat_file(dat, m::MERIT_DEFINITION_FILE_ID).unwrap_or_default(),
            merit_strings: kit::read_dmsg_strings(dat, m::MERIT_STRINGS_FILE_ID_ENGLISH),
            job_categories,
            type_definitions: kit::read_dat_file(dat, m::JOB_POINT_TYPE_FILE_ID)
                .unwrap_or_default(),
            job_strings: kit::read_dmsg_strings(dat, m::JOB_POINT_STRINGS_FILE_ID_ENGLISH),
            gift_strings: kit::read_dmsg_strings(dat, m::GIFT_STRINGS_FILE_ID_ENGLISH),
            gift_thresholds,
        }
    }

    /// The name of a merit index or a category record's byte-0 id.
    pub fn merit_name(&self, id: u16) -> Option<&str> {
        paired(&self.merit_strings, id, false)
    }

    pub fn merit_description(&self, id: u16) -> Option<&str> {
        paired(&self.merit_strings, id, true)
    }

    /// The name of a job or of a job point type id.
    pub fn job_name(&self, id: u16) -> Option<&str> {
        paired(&self.job_strings, id, false)
    }

    pub fn job_description(&self, id: u16) -> Option<&str> {
        paired(&self.job_strings, id, true)
    }

    pub fn gift_name(&self, job: u8, gift: usize) -> Option<&str> {
        let entry = ffxi_dat::merits::gift_name_entry(job, gift)?;
        self.gift_strings.get(entry).map(String::as_str)
    }

    pub fn definition(&self, index: u16) -> Option<MeritDefinition> {
        ffxi_dat::merits::merit_definition(&self.definitions, index)
    }

    pub fn type_max(&self, type_id: u16) -> Option<u8> {
        ffxi_dat::merits::job_point_type_max(&self.type_definitions, type_id)
    }

    /// The first present category record whose byte 3 names `section`.
    pub fn section_record(&self, section: u16) -> Option<&MeritCategory> {
        self.categories
            .iter()
            .find(|record| record.is_present() && i32::from(record.section) == i32::from(section))
    }
}

/// Everything these windows read from the install: window records, GetText tables and the merit
/// and job point tables. Loaded once; an install that lacks a piece leaves it empty.
#[derive(bevy::prelude::Resource, Debug, Default)]
pub struct MeritDat {
    loaded: bool,
    pub windows: BTreeMap<String, MenuWindow>,
    pub texts: GetTextTables,
    pub tables: MeritTables,
}

impl MeritDat {
    pub fn ensure_loaded(&mut self, dat: Option<&ffxi_dat::archive::DatRoot>) {
        if self.loaded {
            return;
        }
        let Some(dat) = dat else { return };
        self.windows = kit::load_record_windows(dat, &MERIT_WINDOWS);
        self.texts = GetTextTables::load(
            dat,
            &[
                GETTEXT_TABLE_SUB_TEMPLATES,
                GETTEXT_TABLE_CLIENT_MESSAGES,
                GETTEXT_TABLE_10,
                GETTEXT_TABLE_12,
                GETTEXT_TABLE_SHORT_CAPTIONS,
                GETTEXT_TABLE_CAPTIONS,
            ],
        );
        self.tables = MeritTables::load(dat);
        self.loaded = true;
    }

    pub fn window(&self, name: &str) -> Option<&MenuWindow> {
        self.windows.get(name)
    }
}

/// One `meritcat` row as built.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeritRow {
    /// A top-level category row: its record's byte-0 id.
    Category {
        id: u16,
    },
    /// A section header: the section's order inside its category and its record's cap.
    Header {
        order: usize,
        cap: u8,
    },
    Merit(MeritCell),
}

/// One merit row's cells and colors as built.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeritCell {
    pub index: u16,
    pub section: u16,
    pub name_color: u32,
    pub count: u8,
    pub count_color: u32,
    /// `0` leaves the cost cell blank.
    pub next: u8,
    pub cost_color: u32,
    /// The record's E: Mog House at `meritcat` open, the main job allowed, and the weapon skill
    /// section's level and key item.
    pub eligible: bool,
}

impl MeritCell {
    pub fn can_raise(&self) -> bool {
        self.next != 0 && self.cost_color == ARGB_NEUTRAL
    }

    pub fn can_lower(&self) -> bool {
        self.count != 0 && self.eligible
    }
}

/// `meritcat`'s top level: category records 1..=31 in record order, skipping absent records and
/// records whose byte-0 id is 0.
pub fn category_rows(tables: &MeritTables) -> Vec<MeritRow> {
    tables
        .categories
        .iter()
        .enumerate()
        .filter(|(number, record)| {
            TOP_LEVEL_RECORDS.contains(number) && record.is_present() && record.id != 0
        })
        .map(|(_, record)| MeritRow::Category { id: record.id })
        .collect()
}

/// The section ids of category `category`, in order.
pub fn sections_of(category: u16) -> Vec<u16> {
    if SINGLE_SECTION_CATEGORIES.contains(&category) || category == WEAPON_SKILL_SECTION {
        vec![category]
    } else {
        JOB_SECTION_OFFSETS
            .iter()
            .filter_map(|offset| category.checked_add(*offset))
            .collect()
    }
}

/// Merit indices section `section` lists, empty past the u16 index space.
pub fn section_indices(section: u16) -> std::ops::Range<u16> {
    section
        .checked_mul(MERITS_PER_SECTION)
        .and_then(|first| Some(first..first.checked_add(MERITS_PER_SECTION)?))
        .unwrap_or(0..0)
}

pub fn is_job_category(category: u16) -> bool {
    sections_of(category).len() > 1
}

/// The section a merit index belongs to.
pub fn section_of_index(index: u16) -> u16 {
    index / MERITS_PER_SECTION
}

/// The upgrades stored for section `section`.
pub fn section_total(entries: &BTreeMap<u16, MeritEntry>, section: u16) -> u32 {
    entries
        .iter()
        .filter(|(index, _)| section_of_index(**index) == section)
        .map(|(_, entry)| u32::from(entry.count))
        .sum()
}

fn with_martial_technique_bonus(cap: u8, key_items: &[u16]) -> u8 {
    let held = MARTIAL_TECHNIQUE_KEY_ITEMS
        .iter()
        .filter(|key_item| key_items.contains(key_item))
        .count();
    let bonus = MARTIAL_TECHNIQUE_CAP_BONUS.saturating_mul(u8::try_from(held).unwrap_or(u8::MAX));
    cap.saturating_add(bonus).min(WEAPON_SKILL_CAP_MAX)
}

/// Section `section`'s cap: its record's byte 4, with the martial technique bonus on the weapon
/// skill section.
pub fn section_cap(tables: &MeritTables, section: u16, key_items: &[u16]) -> u8 {
    let cap = tables
        .section_record(section)
        .map_or(0, |record| record.cap);
    if section == WEAPON_SKILL_SECTION {
        with_martial_technique_bonus(cap, key_items)
    } else {
        cap
    }
}

/// `meritinf`'s cap: category record number `top_row + 1`'s byte 4, with the martial technique
/// bonus when that record's byte 3 is the weapon skill section.
pub fn info_cap(tables: &MeritTables, top_row: usize, key_items: &[u16]) -> u8 {
    let Some(record) = tables.categories.get(top_row + 1) else {
        return 0;
    };
    if i32::from(record.section) == i32::from(WEAPON_SKILL_SECTION) {
        with_martial_technique_bonus(record.cap, key_items)
    } else {
        record.cap
    }
}

/// The rows of category `category`: per section, a header row when a category record names the
/// section (job categories only), then the merits with a definition record.
pub fn merit_rows(
    category: u16,
    tables: &MeritTables,
    snap: &SceneSnapshot,
    mog_house_at_open: bool,
) -> Vec<MeritRow> {
    let main_job = snap.stats.map_or(0, |s| s.mjob_no);
    let main_level = snap.stats.map_or(0, |s| s.mjob_lv);
    let weapon_skills_open =
        main_level >= WEAPON_SKILL_LEVEL && snap.key_items.contains(&HEART_OF_THE_BUSHIN_KEY_ITEM);
    let merit_points = snap.merits.map(|m| m.merit_points);
    let sections = sections_of(category);
    let headed = sections.len() > 1;
    let mut rows = Vec::new();
    for (order, &section) in sections.iter().enumerate() {
        if let Some(record) = tables.section_record(section).filter(|_| headed) {
            rows.push(MeritRow::Header {
                order,
                cap: record.cap,
            });
        }
        let cap = u32::from(section_cap(tables, section, &snap.key_items));
        let total = section_total(&snap.merit_entries, section);
        for index in section_indices(section) {
            let Some(definition) = tables.definition(index) else {
                continue;
            };
            let eligible = mog_house_at_open
                && definition.allows_job(main_job)
                && (section != WEAPON_SKILL_SECTION || weapon_skills_open);
            let name_color = if eligible {
                SECTION_COLORS[order]
            } else {
                ARGB_DIM
            };
            let stored = snap.merit_entries.get(&index).copied().unwrap_or_default();
            let count_color = if stored.count == definition.max {
                ARGB_CAPPED
            } else {
                name_color
            };
            let affordable = merit_points.is_some_and(|points| stored.next <= points);
            let cost_color = if eligible && affordable && total < cap {
                ARGB_NEUTRAL
            } else {
                ARGB_DIM
            };
            rows.push(MeritRow::Merit(MeritCell {
                index,
                section,
                name_color,
                count: stored.count,
                count_color,
                next: stored.next,
                cost_color,
                eligible,
            }));
        }
    }
    rows
}

/// Job `job`'s level from s2c 0x01B, 0 when unknown.
pub fn job_level(snap: &SceneSnapshot, job: u8) -> u8 {
    snap.jobs
        .as_ref()
        .and_then(|jobs| jobs.job_levels.get(usize::from(job)).copied())
        .unwrap_or(0)
}

/// `merit2ca`'s rows: every job category record with a job, in record order, and its name color.
pub fn job_rows(tables: &MeritTables, snap: &SceneSnapshot) -> Vec<(u8, u32)> {
    tables
        .job_categories
        .iter()
        .filter(|record| record.job != 0)
        .map(|record| {
            let color = if job_level(snap, record.job) >= MASTER_LEVEL_JOB_CAP {
                ARGB_NEUTRAL
            } else {
                ARGB_DIM
            };
            (record.job, color)
        })
        .collect()
}

/// One `jbpcat` row as built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobPageRow {
    Type(JobPointCell),
    Gift(GiftCell),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobPointCell {
    pub type_id: u16,
    pub level: u8,
    pub max: u8,
    pub next: u16,
    pub state: UpgradeRowState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GiftCell {
    pub gift: usize,
    pub threshold: u16,
    pub color: u32,
}

/// A job point type's state: unavailable outside S or without stored data, capped, or affordable
/// by the job's unspent job points.
pub fn job_point_state(
    eligible: bool,
    stored: JobPointType,
    max: u8,
    current_jp: Option<u16>,
) -> UpgradeRowState {
    let has_data = stored.level != 0 || stored.next != 0;
    if !eligible || !has_data {
        UpgradeRowState::Unavailable
    } else if stored.level >= max {
        UpgradeRowState::AtCap
    } else if current_jp.is_some_and(|jp| jp >= stored.next) {
        UpgradeRowState::Affordable
    } else {
        UpgradeRowState::TooCostly
    }
}

fn job_totals(record: Option<&JobPointsRecord>, job: u8) -> Option<kuluu_snapshot::JobPointsJob> {
    record?.jobs.get(usize::from(job)).copied()
}

/// The Job Points page: job `job`'s record lists its type ids in slot order; empty slots and
/// types without a definition are skipped.
pub fn job_point_rows(
    job: u8,
    tables: &MeritTables,
    snap: &SceneSnapshot,
    eligible: bool,
) -> Vec<JobPageRow> {
    let Some(record) = tables.job_categories.get(usize::from(job)) else {
        return Vec::new();
    };
    let current_jp = job_totals(snap.job_points.as_ref(), job).map(|totals| totals.current_jp);
    record
        .types
        .iter()
        .copied()
        .filter(|&type_id| type_id != 0)
        .filter_map(|type_id| {
            let max = tables.type_max(type_id)?;
            let stored = snap
                .job_point_types
                .get(&type_id)
                .copied()
                .unwrap_or_default();
            Some(JobPageRow::Type(JobPointCell {
                type_id,
                level: stored.level,
                max,
                next: stored.next,
                state: job_point_state(eligible, stored, max, current_jp),
            }))
        })
        .collect()
}

/// The Gifts page: every gift of job `job` with a threshold, lit once the job has spent that many
/// job points.
pub fn gift_rows(job: u8, tables: &MeritTables, snap: &SceneSnapshot) -> Vec<JobPageRow> {
    let spent = job_totals(snap.job_points.as_ref(), job).map_or(0, |totals| totals.total_jp_spent);
    tables
        .gift_thresholds
        .get(&job)
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, &threshold)| threshold != 0)
        .map(|(gift, &threshold)| {
            let color = if spent >= threshold {
                ARGB_NEUTRAL
            } else {
                ARGB_DIM
            };
            JobPageRow::Gift(GiftCell {
                gift,
                threshold,
                color,
            })
        })
        .collect()
}

/// `meritcat` while it is open.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CategoriesState {
    pub mog_house_at_open: bool,
    /// The category row highlighted when the open category was entered.
    pub top_cursor: usize,
    /// What the rows were built for (`Some(None)` the top level) and the stored merits they
    /// were built from.
    pub built_for: Option<Option<u16>>,
    pub built_from: BTreeMap<u16, MeritEntry>,
    pub rows: Vec<MeritRow>,
}

impl CategoriesState {
    pub fn opened(mog_house: bool) -> Self {
        Self {
            mog_house_at_open: mog_house,
            ..Self::default()
        }
    }

    pub fn cell(&self, index: u16) -> Option<MeritCell> {
        self.rows.iter().find_map(|row| match row {
            MeritRow::Merit(cell) if cell.index == index => Some(*cell),
            _ => None,
        })
    }
}

/// What a page was built from: the job's stored type states and the JobPoints record.
pub type JobPageInputs = (Vec<(u16, JobPointType)>, Option<JobPointsRecord>);

/// `jbpcat` while it is open.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JobPageState {
    pub job: u8,
    /// S: in a Mog House and not in Monstrosity when `jbpcat` opened.
    pub eligible_at_open: bool,
    /// Each page's cursor row while `jbpcat` stays open: Job Points, then Gifts.
    pub page_cursors: [usize; 2],
    /// The page the rows were built for (`Some(true)` Gifts) and what they were built from.
    pub built_for: Option<bool>,
    pub built_from: Option<JobPageInputs>,
    pub rows: Vec<JobPageRow>,
}

impl JobPageState {
    pub fn opened(job: u8, eligible: bool) -> Self {
        Self {
            job,
            eligible_at_open: eligible,
            ..Self::default()
        }
    }

    pub fn cell(&self, type_id: u16) -> Option<JobPointCell> {
        self.rows.iter().find_map(|row| match row {
            JobPageRow::Type(cell) if cell.type_id == type_id => Some(*cell),
            _ => None,
        })
    }
}

fn job_page_inputs(job: u8, snap: &SceneSnapshot) -> JobPageInputs {
    let first = u16::from(job) * ffxi_proto::decode::JobPointWireEntry::TYPES_PER_JOB;
    let types = snap
        .job_point_types
        .range(first..first + ffxi_proto::decode::JobPointWireEntry::TYPES_PER_JOB)
        .map(|(type_id, state)| (*type_id, *state))
        .collect();
    (types, snap.job_points)
}

/// The spend lockout: after a confirmed spend the prompt stays shut for
/// [`SPEND_LOCKOUT_PASSES`] refresh passes of a focused `merit2ca`, or of `jbpcat` on its Job
/// Points page.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SpendLockout {
    remaining: u32,
    frame_carry: kit::RetailFrameCarry,
}

impl SpendLockout {
    pub fn start(&mut self) {
        *self = Self {
            remaining: SPEND_LOCKOUT_PASSES,
            frame_carry: kit::RetailFrameCarry::default(),
        };
    }

    pub fn is_running(&self) -> bool {
        self.remaining > 0
    }

    pub fn tick(&mut self, passes: u32) {
        self.remaining = self.remaining.saturating_sub(passes);
    }

    /// [`Self::tick`] over `seconds` of wall time at the retail frame rate.
    pub fn advance(&mut self, seconds: f32) {
        let frames = self.frame_carry.frames(seconds);
        self.tick(frames);
    }
}

/// State the merit and job point windows keep, held on [`DynamicMenu`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeritWindows {
    /// Cursors kept between openings: `merit1`'s, `merit2`'s and `meritcat`'s category row.
    pub kept_menu_cursor: usize,
    pub kept_mode_cursor: usize,
    pub kept_category_row: usize,
    /// Monstrosity held when Status item 7 opened `merit1`: `merit1` item 2 is disabled and
    /// `levmerit` sits lower.
    pub monstrosity_at_open: bool,
    pub categories: Option<CategoriesState>,
    /// The `merit3` item last chosen while it is open: `Some(true)` Raise.
    pub last_chosen: Option<bool>,
    pub job_page: Option<JobPageState>,
    pub spend_lockout: SpendLockout,
    /// DAT text the input path prints and the help bar shows, copied at refresh: table 7 lines 178
    /// and 187, and the `sortyn` help lines for raise and lower.
    pub not_in_mog_house_message: Option<String>,
    pub mode_refused_message: Option<String>,
    pub sort_help: [Option<String>; 2],
    /// First visible row of each scrolling list, by window.
    pub list_starts: Vec<(MenuKind, usize)>,
}

impl MeritWindows {
    pub fn list_start(&self, kind: MenuKind) -> usize {
        self.list_starts
            .iter()
            .find(|(k, _)| *k == kind)
            .map_or(0, |(_, start)| *start)
    }

    fn scroll_to(&mut self, kind: MenuKind, cursor: usize, visible: usize) {
        let start = kit::scrolled_start(self.list_start(kind), cursor, visible);
        match self.list_starts.iter_mut().find(|(k, _)| *k == kind) {
            Some(entry) => entry.1 = start,
            None => self.list_starts.push((kind, start)),
        }
    }

    /// Raise and Lower possible for merit `index`, from its built row.
    pub fn raise_lower(&self, index: u16) -> (bool, bool) {
        self.categories
            .as_ref()
            .and_then(|state| state.cell(index))
            .map_or((false, false), |cell| (cell.can_raise(), cell.can_lower()))
    }
}

/// Builds and keeps each open window's rows and remembered cursors, following the open stack.
pub fn sync_merit_windows(
    dynamic: &mut DynamicMenu,
    stack: Option<&MenuStack>,
    snap: &SceneSnapshot,
    dat: &MeritDat,
) {
    let levels = stack.map(|s| s.levels.as_slice()).unwrap_or_default();
    let windows = &mut dynamic.merit;
    let mut categories_open = None;
    let mut job_page_open = None;
    let mut raise_lower_open = false;
    for level in levels {
        match merit_window(level.kind) {
            Some(MeritWindow::Menu) => windows.kept_menu_cursor = level.cursor,
            Some(MeritWindow::Mode) => windows.kept_mode_cursor = level.cursor,
            Some(MeritWindow::Categories { entered }) => {
                categories_open = Some(entered);
                windows.kept_category_row = match (entered, &windows.categories) {
                    (Some(_), Some(state)) => state.top_cursor,
                    _ => level.cursor,
                };
            }
            Some(MeritWindow::RaiseLower { .. }) => raise_lower_open = true,
            Some(MeritWindow::JobCategories { job, gifts }) => job_page_open = Some((job, gifts)),
            _ => {}
        }
    }

    match categories_open {
        None => windows.categories = None,
        Some(entered) => {
            let state = windows
                .categories
                .get_or_insert_with(|| CategoriesState::opened(kit::in_mog_house(snap)));
            let stale = state.built_for != Some(entered) || state.built_from != snap.merit_entries;
            if stale {
                state.rows = match entered {
                    None => category_rows(&dat.tables),
                    Some(category) => {
                        merit_rows(category, &dat.tables, snap, state.mog_house_at_open)
                    }
                };
                state.built_for = Some(entered);
                state.built_from = snap.merit_entries.clone();
            }
        }
    }
    if !raise_lower_open {
        windows.last_chosen = None;
    }

    match job_page_open {
        None => windows.job_page = None,
        Some((job, gifts)) => {
            let eligible = kit::in_mog_house(snap) && !kit::in_monstrosity(snap);
            let state = windows
                .job_page
                .get_or_insert_with(|| JobPageState::opened(job, eligible));
            let inputs = job_page_inputs(job, snap);
            let stale = state.built_for != Some(gifts)
                || (!gifts && state.built_from.as_ref() != Some(&inputs));
            if stale {
                state.rows = if gifts {
                    gift_rows(job, &dat.tables, snap)
                } else {
                    job_point_rows(job, &dat.tables, snap, state.eligible_at_open)
                };
                state.built_for = Some(gifts);
                state.built_from = Some(inputs);
            }
        }
    }

    let open: Vec<MenuKind> = levels.iter().map(|level| level.kind).collect();
    windows.list_starts.retain(|(kind, _)| open.contains(kind));
    for level in levels {
        if let Some(visible) = visible_list_rows(level.kind) {
            windows.scroll_to(level.kind, level.cursor, visible);
        }
    }
}

/// Where `merit3` closes, together with the `sortyn` / `merityn` above it: when neither Raise nor
/// Lower remains possible, or the item last chosen has become impossible. The stack index to
/// truncate at.
pub fn raise_lower_close_at(stack: &MenuStack, windows: &MeritWindows) -> Option<usize> {
    let (at, index) =
        stack
            .levels
            .iter()
            .enumerate()
            .find_map(|(at, level)| match merit_window(level.kind) {
                Some(MeritWindow::RaiseLower { index }) => Some((at, index)),
                _ => None,
            })?;
    let (raise, lower) = windows.raise_lower(index);
    let chosen_gone = match windows.last_chosen {
        Some(true) => !raise,
        Some(false) => !lower,
        None => false,
    };
    (!(raise || lower) || chosen_gone).then_some(at)
}

/// The focused window counts down the spend lockout.
pub fn lockout_counts(stack: &MenuStack) -> bool {
    matches!(
        stack.current().and_then(|level| merit_window(level.kind)),
        Some(MeritWindow::Jobs | MeritWindow::JobCategories { gifts: false, .. })
    )
}

/// Each frame: closes `merit3` once its actions are gone and counts the spend lockout down.
pub fn merit_window_upkeep(
    time: bevy::prelude::Res<bevy::prelude::Time>,
    mut mode: bevy::prelude::ResMut<crate::input_mode::InputMode>,
    dynamic: Option<bevy::prelude::ResMut<DynamicMenu>>,
) {
    use crate::input_mode::InputMode;
    let Some(mut dynamic) = dynamic else { return };
    let (close_at, counts) = match &*mode {
        InputMode::Menu(stack) => (
            raise_lower_close_at(stack, &dynamic.merit),
            lockout_counts(stack),
        ),
        _ => return,
    };
    if let Some(at) = close_at {
        if let InputMode::Menu(stack) = &mut *mode {
            stack.levels.truncate(at);
            dynamic.closed_by_logic = true;
        }
    }
    if counts && dynamic.merit.spend_lockout.is_running() {
        dynamic.merit.spend_lockout.advance(time.delta_secs());
    }
}

/// What the session sends for a window action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeritRequest {
    /// c2s 0x0BE ChangeMode.
    SetMode { limit_points: bool },
    /// c2s 0x0BE EditMode.
    Edit { index: u16, raise: bool },
    /// c2s 0x0BF.
    SpendJobPoint { type_id: u16 },
    /// c2s 0x0C0.
    RequestJobPoints,
}

/// What one window action asks of the input layer beyond the stack and menu state it changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeritOutcome {
    pub requests: Vec<MeritRequest>,
    pub chat_lines: Vec<String>,
}

fn push_at(stack: &mut MenuStack, window: MeritWindow, cursor: usize) {
    stack.push(MenuKind::Merit(window));
    if let Some(level) = stack.current_mut() {
        level.cursor = cursor;
    }
}

/// Status row id 7: `merit1` opens over `statcom2`, on its kept cursor, with `levmerit`.
pub fn open_merit_menu(
    stack: &mut MenuStack,
    dynamic: &mut DynamicMenu,
    snap: &SceneSnapshot,
) -> MeritOutcome {
    kit::close_status_detail(stack);
    dynamic.merit.monstrosity_at_open = kit::in_monstrosity(snap);
    let cursor = dynamic
        .merit
        .kept_menu_cursor
        .min(usize::from(MERIT1_ITEM_MODE) - 1);
    push_at(stack, MeritWindow::Menu, cursor);
    MeritOutcome::default()
}

/// Status row id 8: `merit2ca` opens with `bluehelp` and `levmeri2`, asking for the job point data
/// while no s2c 0x08D has been processed.
pub fn open_job_point_jobs(stack: &mut MenuStack, snap: &SceneSnapshot) -> MeritOutcome {
    kit::close_status_detail(stack);
    let mut outcome = MeritOutcome::default();
    if !snap.job_points_received {
        outcome.requests.push(MeritRequest::RequestJobPoints);
    }
    push_at(stack, MeritWindow::Jobs, 0);
    outcome
}

/// A status effect that refuses the mode selector is active.
pub fn mode_refused(snap: &SceneSnapshot) -> bool {
    MODE_REFUSING_EFFECTS
        .iter()
        .any(|effect| snap.status_icons.contains(effect))
}

/// `merit2`'s items enabled: the current mode's item is disabled while a Merits record exists.
pub fn mode_items_enabled(snap: &SceneSnapshot) -> (bool, bool) {
    match snap.merits {
        Some(record) => (record.merit_mode_enabled, !record.merit_mode_enabled),
        None => (true, true),
    }
}

fn not_in_mog_house_line(windows: &MeritWindows) -> String {
    windows.not_in_mog_house_message.clone().unwrap_or_else(|| {
        kit::client_message_fallback(GETTEXT_TABLE_CLIENT_MESSAGES, kit::NOT_ALLOWED_MESSAGE_ID)
    })
}

/// Answers a select on one of these windows. `action` is the cursor row's action, `None` when the
/// window lists no row there. A select that opens a window gets its confirm sound from the
/// menu-depth observer, so only selects that open nothing queue their sound here; a Yes that closes
/// windows marks the close as its own (`DynamicMenu::closed_by_logic`) so the observer adds no
/// cancel sound.
pub fn select_merit_row(
    window: MeritWindow,
    action: Option<DynamicMenuAction>,
    stack: &mut MenuStack,
    dynamic: &mut DynamicMenu,
    snap: &SceneSnapshot,
) -> MeritOutcome {
    use MeritAction as A;
    let action = match action {
        Some(DynamicMenuAction::Merit(action)) => Some(action),
        _ => None,
    };
    let mut outcome = MeritOutcome::default();
    let cursor = stack.current().map_or(0, |level| level.cursor);
    match (window, action) {
        (_, Some(A::Item { enabled: false, .. })) => {}
        (MeritWindow::Menu, Some(A::Item { item, .. })) if item == MERIT1_ITEM_CATEGORIES => {
            dynamic.merit.categories = Some(CategoriesState::opened(kit::in_mog_house(snap)));
            let row = dynamic.merit.kept_category_row;
            push_at(stack, MeritWindow::Categories { entered: None }, row);
        }
        (MeritWindow::Menu, Some(A::Item { item, .. })) if item == MERIT1_ITEM_MODE => {
            if mode_refused(snap) {
                outcome
                    .chat_lines
                    .push(
                        dynamic
                            .merit
                            .mode_refused_message
                            .clone()
                            .unwrap_or_else(|| {
                                kit::client_message_fallback(
                                    GETTEXT_TABLE_CLIENT_MESSAGES,
                                    MODE_REFUSED_MESSAGE_ID,
                                )
                            }),
                    );
            } else {
                let cursor = dynamic
                    .merit
                    .kept_mode_cursor
                    .min(usize::from(MERIT2_ITEM_LIMIT_POINTS) - 1);
                push_at(stack, MeritWindow::Mode, cursor);
            }
        }
        (MeritWindow::Mode, Some(A::Item { item, .. })) => {
            outcome.requests.push(MeritRequest::SetMode {
                limit_points: item == MERIT2_ITEM_LIMIT_POINTS,
            });
        }
        (MeritWindow::Categories { entered: None }, Some(A::Category { id })) => {
            if let Some(state) = dynamic.merit.categories.as_mut() {
                state.top_cursor = cursor;
            }
            if let Some(level) = stack.current_mut() {
                level.kind = MenuKind::Merit(MeritWindow::Categories { entered: Some(id) });
                level.cursor = 0;
            }
            dynamic.ui_sounds.push(UiSound::Confirm);
        }
        (MeritWindow::Categories { entered: Some(_) }, Some(A::Merit { index })) => {
            let state = dynamic.merit.categories.as_ref();
            let mog_house = state.is_some_and(|state| state.mog_house_at_open);
            let (raise, lower) = dynamic.merit.raise_lower(index);
            if !mog_house {
                outcome
                    .chat_lines
                    .push(not_in_mog_house_line(&dynamic.merit));
                dynamic.ui_sounds.push(UiSound::Buzzer);
            } else if raise || lower {
                dynamic.merit.last_chosen = None;
                push_at(stack, MeritWindow::RaiseLower { index }, 0);
            } else {
                dynamic.ui_sounds.push(UiSound::Buzzer);
            }
        }
        (MeritWindow::RaiseLower { index }, Some(A::Item { item, .. })) => {
            let raise = item == MERIT3_ITEM_RAISE;
            dynamic.merit.last_chosen = Some(raise);
            stack.open_confirm(
                MenuKind::Merit(MeritWindow::SortConfirm { index, raise }),
                ConfirmDefault::No,
            );
        }
        (MeritWindow::SortConfirm { index, raise }, Some(A::Item { item, .. })) => {
            stack.pop();
            if item == CONFIRM_ITEM_YES {
                dynamic.closed_by_logic = true;
                if snap.merit_entries.contains_key(&index) {
                    stack.open_confirm(
                        MenuKind::Merit(MeritWindow::Confirm { index, raise }),
                        ConfirmDefault::No,
                    );
                }
            }
        }
        (MeritWindow::Confirm { index, raise }, Some(A::Item { item, .. })) => {
            stack.pop();
            if item == CONFIRM_ITEM_YES {
                dynamic.closed_by_logic = true;
                outcome.requests.push(MeritRequest::Edit { index, raise });
                if stack.current().is_some_and(|level| {
                    matches!(
                        merit_window(level.kind),
                        Some(MeritWindow::RaiseLower { .. })
                    )
                }) {
                    stack.pop();
                }
            }
        }
        (MeritWindow::Jobs, Some(A::Job { job })) => {
            if JOB_POINT_JOBS.contains(&job) && job_level(snap, job) >= MASTER_LEVEL_JOB_CAP {
                let eligible = kit::in_mog_house(snap) && !kit::in_monstrosity(snap);
                dynamic.merit.job_page = Some(JobPageState::opened(job, eligible));
                push_at(stack, MeritWindow::JobCategories { job, gifts: false }, 0);
            } else {
                dynamic.ui_sounds.push(UiSound::Buzzer);
            }
        }
        (MeritWindow::JobCategories { job, gifts: false }, Some(A::JobPointType { type_id })) => {
            let page = dynamic.merit.job_page.as_ref();
            let eligible = page.is_some_and(|page| page.eligible_at_open);
            let raisable = page
                .and_then(|page| page.cell(type_id))
                .is_some_and(|cell| cell.state == UpgradeRowState::Affordable);
            if !eligible {
                outcome
                    .chat_lines
                    .push(not_in_mog_house_line(&dynamic.merit));
                dynamic.ui_sounds.push(UiSound::Buzzer);
            } else if !raisable {
                dynamic.ui_sounds.push(UiSound::Buzzer);
            } else if dynamic.merit.spend_lockout.is_running() {
                dynamic.ui_sounds.push(UiSound::Confirm);
            } else {
                stack.open_confirm(
                    MenuKind::Merit(MeritWindow::JobPointConfirm { job, type_id }),
                    ConfirmDefault::No,
                );
            }
        }
        (MeritWindow::JobCategories { .. }, _) => dynamic.ui_sounds.push(UiSound::Buzzer),
        (MeritWindow::JobPointConfirm { type_id, .. }, Some(A::Item { item, .. })) => {
            stack.pop();
            if item == CONFIRM_ITEM_YES {
                dynamic.closed_by_logic = true;
                outcome
                    .requests
                    .push(MeritRequest::SpendJobPoint { type_id });
                dynamic.merit.spend_lockout.start();
            }
        }
        _ => {}
    }
    outcome
}

/// Esc on a merit or job point window whose registry entry hands Esc to the window: `meritcat`
/// inside a category returns to the category list on that category, and `merityn` only buzzes.
/// Returns whether the window took the key; otherwise the window closes as usual (`sortyn`'s
/// handler answers No, which closes it the same way).
pub fn cancel_merit_window(stack: &mut MenuStack, dynamic: &mut DynamicMenu) -> bool {
    let Some(level) = stack.current_mut() else {
        return false;
    };
    match merit_window(level.kind) {
        Some(MeritWindow::Categories { entered: Some(_) }) => {
            level.kind = MenuKind::Merit(MeritWindow::Categories { entered: None });
            level.cursor = dynamic
                .merit
                .categories
                .as_ref()
                .map_or(0, |state| state.top_cursor);
            dynamic.ui_sounds.push(UiSound::Cancel);
            true
        }
        Some(MeritWindow::Confirm { .. } | MeritWindow::JobPointConfirm { .. }) => {
            dynamic.ui_sounds.push(UiSound::Buzzer);
            true
        }
        _ => false,
    }
}

/// Left or Right on `jbpcat` toggles its page, each page keeping its own cursor row. Returns
/// whether the key was taken.
pub fn toggle_job_page(stack: &mut MenuStack, dynamic: &mut DynamicMenu) -> bool {
    let Some(level) = stack.current_mut() else {
        return false;
    };
    let Some(MeritWindow::JobCategories { job, gifts }) = merit_window(level.kind) else {
        return false;
    };
    if let Some(page) = dynamic.merit.job_page.as_mut() {
        page.page_cursors[usize::from(gifts)] = level.cursor;
        level.cursor = page.page_cursors[usize::from(!gifts)];
    } else {
        level.cursor = 0;
    }
    level.kind = MenuKind::Merit(MeritWindow::JobCategories { job, gifts: !gifts });
    dynamic.ui_sounds.push(UiSound::PageToggle);
    true
}

fn item_row(dat: &MeritDat, window: &str, item: u8, enabled: bool) -> DynamicMenuRow {
    let label = dat
        .window(window)
        .and_then(|record| kit::item_by_id(record, item))
        .and_then(|block| kit::item_label(&dat.texts, block));
    DynamicMenuRow {
        label: label.unwrap_or_default().to_string(),
        action: DynamicMenuAction::Merit(MeritAction::Item { item, enabled }),
        tint: None,
    }
}

fn row(label: Option<&str>, action: MeritAction) -> DynamicMenuRow {
    DynamicMenuRow {
        label: label.unwrap_or_default().to_string(),
        action: DynamicMenuAction::Merit(action),
        tint: None,
    }
}

/// The rows a window's handler answers, labelled from DAT.
pub fn merit_rows_for(
    level: &MenuLevel,
    dat: &MeritDat,
    windows: &MeritWindows,
    snap: &SceneSnapshot,
) -> Vec<DynamicMenuRow> {
    let tables = &dat.tables;
    let yes_no = |name: &str| {
        vec![
            item_row(dat, name, CONFIRM_ITEM_YES, true),
            item_row(dat, name, kit::CONFIRM_ITEM_NO, true),
        ]
    };
    let Some(window) = merit_window(level.kind) else {
        return Vec::new();
    };
    match window {
        MeritWindow::Menu => vec![
            item_row(dat, MERIT1_WINDOW, MERIT1_ITEM_CATEGORIES, true),
            item_row(
                dat,
                MERIT1_WINDOW,
                MERIT1_ITEM_MODE,
                !windows.monstrosity_at_open,
            ),
        ],
        MeritWindow::Mode => {
            let (exp, limit_points) = mode_items_enabled(snap);
            vec![
                item_row(dat, MERIT2_WINDOW, MERIT2_ITEM_EXP, exp),
                item_row(dat, MERIT2_WINDOW, MERIT2_ITEM_LIMIT_POINTS, limit_points),
            ]
        }
        MeritWindow::Categories { entered } => windows
            .categories
            .as_ref()
            .filter(|state| state.built_for == Some(entered))
            .map(|state| {
                state
                    .rows
                    .iter()
                    .map(|built| match *built {
                        MeritRow::Category { id } => {
                            row(tables.merit_name(id), MeritAction::Category { id })
                        }
                        MeritRow::Header { .. } => row(None, MeritAction::SectionHeader),
                        MeritRow::Merit(cell) => row(
                            tables.merit_name(cell.index),
                            MeritAction::Merit { index: cell.index },
                        ),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        MeritWindow::RaiseLower { index } => {
            let (raise, lower) = windows.raise_lower(index);
            vec![
                item_row(dat, MERIT3_WINDOW, MERIT3_ITEM_RAISE, raise),
                item_row(dat, MERIT3_WINDOW, MERIT3_ITEM_LOWER, lower),
            ]
        }
        MeritWindow::SortConfirm { .. } => yes_no(SORTYN_WINDOW),
        MeritWindow::Confirm { .. } | MeritWindow::JobPointConfirm { .. } => yes_no(MERITYN_WINDOW),
        MeritWindow::Jobs => job_rows(tables, snap)
            .into_iter()
            .map(|(job, _)| row(tables.job_name(u16::from(job)), MeritAction::Job { job }))
            .collect(),
        MeritWindow::JobCategories { job, gifts } => windows
            .job_page
            .as_ref()
            .filter(|page| page.job == job && page.built_for == Some(gifts))
            .map(|page| {
                page.rows
                    .iter()
                    .map(|built| match *built {
                        JobPageRow::Type(cell) => row(
                            tables.job_name(cell.type_id),
                            MeritAction::JobPointType {
                                type_id: cell.type_id,
                            },
                        ),
                        JobPageRow::Gift(cell) => {
                            row(tables.gift_name(job, cell.gift), MeritAction::Gift)
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Inputs every window builder reads.
pub struct MeritContext<'a> {
    pub snap: &'a SceneSnapshot,
    pub dat: &'a MeritDat,
    pub windows: &'a MeritWindows,
}

impl MeritContext<'_> {
    fn frame(&self, window: &str) -> Option<FrameBlock> {
        self.dat.window(window)?.frame
    }

    fn labels(&self, window: &str) -> Vec<PanelText> {
        kit::record_label_texts(self.dat.window(window), &self.dat.texts)
    }

    /// The x of a readout's right edge, `inset` left of the window's right edge.
    fn right_edge(&self, window: &str, inset: f32) -> Option<f32> {
        self.frame(window)
            .map(|frame| f32::from(frame.width) - inset)
    }
}

/// Every window the open stack draws here, oldest first, each display-only companion right after
/// the window that opened it.
pub fn panel_windows(
    stack: &MenuStack,
    rows_of: impl Fn(usize) -> Vec<DynamicMenuRow>,
    cx: &MeritContext,
) -> Vec<PanelWindow> {
    let top = stack.levels.len().saturating_sub(1);
    let mut out = Vec::new();
    let mut titled = None;
    for (index, level) in stack.levels.iter().enumerate() {
        let Some(window) = merit_window(level.kind) else {
            continue;
        };
        let focused = index == top;
        let rows = rows_of(index);
        let cursor = focused.then_some(level.cursor);
        let name = window.record_name();
        let selector = |name: &str| selector_texts(cx, name, &rows, cursor);
        match window {
            MeritWindow::Menu => {
                out.push(PanelWindow::at_record(name, index, selector(name)));
                out.push(PanelWindow {
                    shift_y: if cx.windows.monstrosity_at_open {
                        LEVMERIT_MONSTROSITY_DROP
                    } else {
                        0.0
                    },
                    ..PanelWindow::at_record(LEVMERIT_WINDOW, index, levmerit_texts(cx))
                });
            }
            MeritWindow::Mode | MeritWindow::RaiseLower { .. } => {
                out.push(PanelWindow::at_record(name, index, selector(name)));
            }
            MeritWindow::SortConfirm { .. } => {
                out.push(PanelWindow::at_record(name, index, selector(name)));
                titled = Some(index);
            }
            MeritWindow::Confirm {
                index: merit,
                raise,
            } => {
                let mut texts = selector(name);
                texts.push(prompt_line(cx, &merit_prompt(cx, merit, raise)));
                out.push(PanelWindow::at_record(name, index, texts));
                titled = Some(index);
            }
            MeritWindow::JobPointConfirm { job, type_id } => {
                let mut texts = selector(name);
                texts.push(prompt_line(cx, &spend_prompt(cx, job, type_id)));
                out.push(PanelWindow::at_record(name, index, texts));
            }
            MeritWindow::Categories { entered } => {
                let built = cx
                    .windows
                    .categories
                    .as_ref()
                    .filter(|state| state.built_for == Some(entered))
                    .map(|state| state.rows.as_slice())
                    .unwrap_or_default();
                out.push(PanelWindow::at_record(
                    name,
                    index,
                    meritcat_texts(level, focused, built, cx),
                ));
                out.push(PanelWindow::at_record(
                    MERITINF_WINDOW,
                    index,
                    meritinf_texts(cx, entered, level.cursor, built),
                ));
                out.push(PanelWindow::at_record(
                    BLUEHELP_WINDOW,
                    index,
                    merit_help_texts(cx, entered, level.cursor, built),
                ));
            }
            MeritWindow::Jobs => {
                let jobs = job_rows(&cx.dat.tables, cx.snap);
                let highlighted = jobs.get(level.cursor).map(|(job, _)| *job);
                out.push(PanelWindow::at_record(
                    name,
                    index,
                    merit2ca_texts(level, focused, &jobs, cx),
                ));
                out.push(PanelWindow::at_record(
                    BLUEHELP_WINDOW,
                    index,
                    kit::description_box_texts(
                        cx.frame(BLUEHELP_WINDOW),
                        highlighted.and_then(|job| cx.dat.tables.job_name(u16::from(job))),
                        highlighted.and_then(|job| cx.dat.tables.job_description(u16::from(job))),
                    ),
                ));
                out.push(PanelWindow::at_record(
                    LEVMERI2_WINDOW,
                    index,
                    levmeri2_texts(cx, highlighted),
                ));
            }
            MeritWindow::JobCategories { job, gifts } => {
                out.push(PanelWindow::at_record(
                    name,
                    index,
                    jbpcat_texts(level, focused, job, gifts, cx),
                ));
            }
        }
    }
    if let Some(index) = titled.filter(|&index| index == top) {
        out.push(kit::title_window(
            index,
            cx.dat
                .texts
                .get_text(GETTEXT_TABLE_SHORT_CAPTIONS, PROMPT_TITLE_ID),
        ));
    }
    out
}

/// A selector's rows at their DAT items, a disabled item in the record's unavailable grey.
fn selector_texts(
    cx: &MeritContext,
    window: &str,
    rows: &[DynamicMenuRow],
    cursor: Option<usize>,
) -> Vec<PanelText> {
    let labels: Vec<(String, u32)> = rows
        .iter()
        .map(|row| {
            let enabled = !matches!(
                row.action,
                DynamicMenuAction::Merit(MeritAction::Item { enabled: false, .. })
            );
            (
                row.label.clone(),
                if enabled { ARGB_NEUTRAL } else { ARGB_DIM },
            )
        })
        .collect();
    kit::selector_item_texts(cx.dat.window(window), &labels, cursor)
}

fn prompt_line(cx: &MeritContext, prompt: &ConfirmPrompt) -> PanelText {
    kit::merityn_prompt_text(cx.frame(MERITYN_WINDOW), prompt.text(&cx.dat.texts))
}

/// The raise prompt takes the merit name and its next cost, the lower prompt the name alone.
pub fn merit_prompt(cx: &MeritContext, index: u16, raise: bool) -> ConfirmPrompt {
    let name = PromptArg::Name(
        cx.dat
            .tables
            .merit_name(index)
            .unwrap_or_default()
            .to_string(),
    );
    if raise {
        let next = cx
            .snap
            .merit_entries
            .get(&index)
            .map_or(0, |entry| entry.next);
        ConfirmPrompt {
            entry: RAISE_PROMPT_ENTRY,
            args: vec![name, PromptArg::Points(u16::from(next))],
        }
    } else {
        ConfirmPrompt {
            entry: LOWER_PROMPT_ENTRY,
            args: vec![name],
        }
    }
}

/// The spend prompt takes the type name and its next cost.
pub fn spend_prompt(cx: &MeritContext, job: u8, type_id: u16) -> ConfirmPrompt {
    let next = cx
        .windows
        .job_page
        .as_ref()
        .filter(|page| page.job == job)
        .and_then(|page| page.cell(type_id))
        .map_or(0, |cell| cell.next);
    ConfirmPrompt {
        entry: SPEND_PROMPT_ENTRY,
        args: vec![
            PromptArg::Name(
                cx.dat
                    .tables
                    .job_name(type_id)
                    .unwrap_or_default()
                    .to_string(),
            ),
            PromptArg::Points(next),
        ],
    }
}

fn list_row_top(slot: usize, start: usize) -> f32 {
    layout::LIST_FIRST_ROW_TOP + (slot - start) as f32 * layout::LIST_ROW_PITCH
}

fn cost_cell(
    caption: Option<&str>,
    next: u32,
    caption_x: f32,
    right: f32,
    y: f32,
    color: u32,
) -> Vec<PanelText> {
    let mut out = Vec::new();
    if let Some(caption) = caption {
        out.push(PanelText::left(caption, caption_x, y, color));
    }
    out.push(PanelText::right(cost_text(next), right, y, color));
    out
}

/// `meritcat`: eleven rows sixteen apart, the cursor row's name in the cursor color.
pub fn meritcat_texts(
    level: &MenuLevel,
    focused: bool,
    rows: &[MeritRow],
    cx: &MeritContext,
) -> Vec<PanelText> {
    let tables = &cx.dat.tables;
    let caption = cx
        .dat
        .texts
        .get_text(GETTEXT_TABLE_12, kit::text12::COST_CAPTION);
    let start = cx.windows.list_start(level.kind);
    let mut out = Vec::new();
    for (slot, built) in rows
        .iter()
        .enumerate()
        .skip(start)
        .take(layout::LIST_VISIBLE_ROWS)
    {
        let top = list_row_top(slot, start);
        let (name_y, number_y) = (top + layout::NAME_DROP, top + layout::NUMBER_DROP);
        let lit = focused && level.cursor == slot;
        match *built {
            MeritRow::Category { id } => {
                let color = if lit { ARGB_FP_CURSOR } else { ARGB_NEUTRAL };
                let name = tables.merit_name(id).unwrap_or_default();
                out.push(PanelText::left(name, layout::NAME_X, name_y, color));
            }
            MeritRow::Header { cap, .. } => {
                out.push(PanelText::right(
                    cap.to_string(),
                    layout::HEADER_CAP_RIGHT,
                    number_y,
                    ARGB_NEUTRAL,
                ));
            }
            MeritRow::Merit(cell) => {
                let color = if lit { ARGB_FP_CURSOR } else { cell.name_color };
                let name = tables.merit_name(cell.index).unwrap_or_default();
                out.push(PanelText::left(name, layout::NAME_X, name_y, color));
                out.push(PanelText::right(
                    cell.count.to_string(),
                    layout::COUNT_RIGHT,
                    number_y,
                    cell.count_color,
                ));
                if cell.next != 0 {
                    out.extend(cost_cell(
                        caption,
                        u32::from(cell.next),
                        layout::COST_CAPTION_X,
                        layout::COST_RIGHT,
                        number_y,
                        cell.cost_color,
                    ));
                }
            }
        }
    }
    out
}

/// `"<total>/<cap>"`, the total in the capped color once it reaches the cap.
pub fn total_text(total: u32, cap: u8, right: f32, y: f32, color: u32) -> PanelText {
    let total_color = if total == u32::from(cap) {
        ARGB_CAPPED
    } else {
        color
    };
    PanelText::right(total.to_string(), right, y, total_color).with_suffix(format!("/{cap}"), color)
}

/// `meritinf`: the unspent merit points, then category `c`'s upgrades against the cap, and for job
/// categories the second section's beside it. At the top level `c` is the highlighted row's id;
/// inside a category it is the entered one.
pub fn meritinf_texts(
    cx: &MeritContext,
    entered: Option<u16>,
    cursor: usize,
    rows: &[MeritRow],
) -> Vec<PanelText> {
    let mut out = cx.labels(MERITINF_WINDOW);
    let Some(right) = cx.right_edge(MERITINF_WINDOW, layout::READOUT_RIGHT_INSET) else {
        return out;
    };
    let points = match cx.snap.merits {
        Some(record) => cost_text(u32::from(record.merit_points)),
        None => format!("{NO_RECORD_MERIT_POINTS} P"),
    };
    out.push(PanelText::right(
        points,
        right,
        layout::MERITINF_POINTS_TOP,
        ARGB_NEUTRAL,
    ));
    let top_row = match (entered, &cx.windows.categories) {
        (Some(_), Some(state)) => state.top_cursor,
        _ => cursor,
    };
    let category = entered.or_else(|| match rows.get(cursor) {
        Some(MeritRow::Category { id }) => Some(*id),
        _ => None,
    });
    let Some(category) = category else {
        return out;
    };
    let cap = info_cap(&cx.dat.tables, top_row, &cx.snap.key_items);
    let entries = &cx.snap.merit_entries;
    out.push(total_text(
        section_total(entries, category),
        cap,
        right,
        layout::MERITINF_TOTAL_TOP,
        ARGB_NEUTRAL,
    ));
    let second_section = is_job_category(category)
        .then(|| sections_of(category).get(1).copied())
        .flatten();
    if let (Some(second_section), Some(second_right)) = (
        second_section,
        cx.right_edge(MERITINF_WINDOW, layout::MERITINF_SECOND_RIGHT_INSET),
    ) {
        out.push(total_text(
            section_total(entries, second_section),
            cap,
            second_right,
            layout::MERITINF_TOTAL_TOP,
            SECTION_COLORS[1],
        ));
    }
    out
}

/// `bluehelp` beside `meritcat`: the highlighted row's name over its description. At the top level
/// the strings key off category record number (row position + 1); a header row shows nothing.
pub fn merit_help_texts(
    cx: &MeritContext,
    entered: Option<u16>,
    cursor: usize,
    rows: &[MeritRow],
) -> Vec<PanelText> {
    let tables = &cx.dat.tables;
    let id = match entered {
        None => tables.categories.get(cursor + 1).map(|record| record.id),
        Some(_) => match rows.get(cursor) {
            Some(MeritRow::Merit(cell)) => Some(cell.index),
            _ => None,
        },
    };
    let Some(id) = id else {
        return Vec::new();
    };
    kit::description_box_texts(
        cx.frame(BLUEHELP_WINDOW),
        tables.merit_name(id),
        tables.merit_description(id),
    )
}

/// `levmerit`: limit points against 10000, then merit points against the maximum.
pub fn levmerit_texts(cx: &MeritContext) -> Vec<PanelText> {
    let mut out = cx.labels(LEVMERIT_WINDOW);
    let Some(right) = cx.right_edge(LEVMERIT_WINDOW, layout::READOUT_RIGHT_INSET) else {
        return out;
    };
    let Some(record) = cx.snap.merits else {
        out.push(PanelText::right(
            format!("{NO_RECORD_LIMIT_POINTS}/{LIMIT_POINT_MAX}"),
            right,
            layout::READOUT_FIRST_TOP,
            ARGB_NEUTRAL,
        ));
        return out;
    };
    out.push(PanelText::right(
        format!("{}/{LIMIT_POINT_MAX}", record.limit_points),
        right,
        layout::READOUT_FIRST_TOP,
        ARGB_NEUTRAL,
    ));
    let color = if record.merit_points >= record.max_merit_points {
        ARGB_CAPPED
    } else {
        ARGB_NEUTRAL
    };
    out.push(PanelText::right(
        format!(
            "{}/{}",
            record.merit_points,
            cost_text(u32::from(record.max_merit_points))
        ),
        right,
        layout::READOUT_SECOND_TOP,
        color,
    ));
    out
}

/// `levmeri2` for the highlighted job: capacity points against 30000, then unspent job points
/// against 500.
pub fn levmeri2_texts(cx: &MeritContext, job: Option<u8>) -> Vec<PanelText> {
    let mut out = cx.labels(LEVMERI2_WINDOW);
    let (Some(right), Some(totals)) = (
        cx.right_edge(LEVMERI2_WINDOW, layout::READOUT_RIGHT_INSET),
        job.and_then(|job| job_totals(cx.snap.job_points.as_ref(), job)),
    ) else {
        return out;
    };
    out.push(PanelText::right(
        format!("{}/{CAPACITY_POINT_MAX}", totals.capacity_points),
        right,
        layout::READOUT_FIRST_TOP,
        ARGB_NEUTRAL,
    ));
    let color = if totals.current_jp >= JOB_POINT_MAX {
        ARGB_CAPPED
    } else {
        ARGB_NEUTRAL
    };
    out.push(PanelText::right(
        format!(
            "{}/{}",
            totals.current_jp,
            cost_text(u32::from(JOB_POINT_MAX))
        ),
        right,
        layout::READOUT_SECOND_TOP,
        color,
    ));
    out
}

/// `merit2ca`: the job names, eleven rows sixteen apart.
pub fn merit2ca_texts(
    level: &MenuLevel,
    focused: bool,
    jobs: &[(u8, u32)],
    cx: &MeritContext,
) -> Vec<PanelText> {
    let start = cx.windows.list_start(level.kind);
    jobs.iter()
        .enumerate()
        .skip(start)
        .take(layout::LIST_VISIBLE_ROWS)
        .map(|(slot, &(job, color))| {
            let color = if focused && level.cursor == slot {
                ARGB_FP_CURSOR
            } else {
                color
            };
            PanelText::left(
                cx.dat.tables.job_name(u16::from(job)).unwrap_or_default(),
                layout::NAME_X,
                list_row_top(slot, start) + layout::NAME_DROP,
                color,
            )
        })
        .collect()
}

/// `jbpcat`: ten rows seventeen apart of either page. The page graphic from the `windowps` group
/// is not drawn: which DAT holds that group is not established.
pub fn jbpcat_texts(
    level: &MenuLevel,
    focused: bool,
    job: u8,
    gifts: bool,
    cx: &MeritContext,
) -> Vec<PanelText> {
    let tables = &cx.dat.tables;
    let rows = cx
        .windows
        .job_page
        .as_ref()
        .filter(|page| page.job == job && page.built_for == Some(gifts))
        .map(|page| page.rows.as_slice())
        .unwrap_or_default();
    let mut out = Vec::new();
    if gifts {
        if let Some(totals) = job_totals(cx.snap.job_points.as_ref(), job) {
            let (x, y) = layout::GIFTS_SPENT_AT;
            out.push(PanelText::left(
                totals.total_jp_spent.to_string(),
                x,
                y,
                ARGB_NEUTRAL,
            ));
        }
    }
    let caption = cx
        .dat
        .texts
        .get_text(GETTEXT_TABLE_12, kit::text12::COST_CAPTION);
    let start = cx.windows.list_start(level.kind);
    for (slot, built) in rows
        .iter()
        .enumerate()
        .skip(start)
        .take(layout::PAGE_VISIBLE_ROWS)
    {
        let top = layout::PAGE_FIRST_ROW_TOP + (slot - start) as f32 * layout::PAGE_ROW_PITCH;
        let (name_y, number_y) = (top + layout::NAME_DROP, top + layout::NUMBER_DROP);
        let lit = focused && level.cursor == slot;
        match *built {
            JobPageRow::Type(cell) => {
                let (name_color, level_color, cost_color) = kit::upgrade_row_colors(cell.state);
                out.push(PanelText::left(
                    tables.job_name(cell.type_id).unwrap_or_default(),
                    layout::NAME_X,
                    name_y,
                    if lit { ARGB_FP_CURSOR } else { name_color },
                ));
                out.push(PanelText::right(
                    cell.level.to_string(),
                    layout::LEVEL_RIGHT,
                    number_y,
                    level_color,
                ));
                out.push(PanelText::left(
                    format!("/{}", cell.max),
                    layout::MAX_X,
                    number_y,
                    level_color,
                ));
                if let Some(cost_color) = cost_color {
                    out.extend(cost_cell(
                        caption,
                        u32::from(cell.next),
                        layout::PAGE_COST_CAPTION_X,
                        layout::PAGE_COST_RIGHT,
                        number_y,
                        cost_color,
                    ));
                }
            }
            JobPageRow::Gift(cell) => {
                out.push(PanelText::left(
                    tables.gift_name(job, cell.gift).unwrap_or_default(),
                    layout::NAME_X,
                    name_y,
                    if lit { ARGB_FP_CURSOR } else { cell.color },
                ));
                out.push(PanelText::right(
                    cell.threshold.to_string(),
                    layout::GIFT_THRESHOLD_RIGHT,
                    number_y,
                    cell.color,
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::gettext::test_support::{prompt_templates, prompt_text};
    use crate::hud::unity_fp::test_support::{item, window};
    use crate::hud::unity_fp::NOT_ALLOWED_MESSAGE_ID;
    use crate::input_mode::CONFIRM_NO_ROW;
    use kuluu_snapshot::{CharStats, JobList, JobPointsJob, MisdataMerits};

    const WAR: u8 = 1;
    const WHM: u8 = 3;
    const RDM: u8 = 5;
    const HP_MP: u16 = 1;
    const WAR_CATEGORY: u16 = 6;
    const WAR_SECOND_SECTION: u16 = WAR_CATEGORY + JOB_SECTION_OFFSETS[1];
    /// Merit indices: two in HP/MP, two in the WAR category's first section (WAR-only and
    /// WHM-only), one in its second section, one weapon skill merit.
    const MAX_HP: u16 = HP_MP * MERITS_PER_SECTION;
    const MAX_MP: u16 = MAX_HP + 1;
    const WAR_FIRST: u16 = WAR_CATEGORY * MERITS_PER_SECTION;
    const WHM_ONLY: u16 = WAR_FIRST + 1;
    const WAR_SECOND: u16 = WAR_SECOND_SECTION * MERITS_PER_SECTION;
    const WEAPON_SKILL: u16 = WEAPON_SKILL_SECTION * MERITS_PER_SECTION;
    const MERIT_MAX: u8 = 8;
    const SECTION_CAP: u8 = 10;
    const WEAPON_SKILL_CAP: u8 = 95;
    /// Job point types of WAR: two defined, one without a definition.
    const WAR_TYPE_A: u16 = 32;
    const WAR_TYPE_B: u16 = 33;
    const WAR_TYPE_UNDEFINED: u16 = 34;
    const TYPE_MAX: u8 = 20;
    const MERIT1_LABEL_ID: i32 = 700;

    fn category(id: u16, section: u16, cap: u8) -> MeritCategory {
        MeritCategory {
            id,
            section: i8::try_from(section).unwrap(),
            cap,
        }
    }

    fn definitions(entries: &[(u16, u8, u32)]) -> Vec<u8> {
        let len = entries.iter().map(|e| usize::from(e.0) + 1).max().unwrap();
        let mut bytes = vec![0u8; len * ffxi_dat::merits::MERIT_DEFINITION_LEN];
        for &(index, max, mask) in entries {
            let o = usize::from(index) * ffxi_dat::merits::MERIT_DEFINITION_LEN;
            bytes[o..o + 2].copy_from_slice(&index.to_le_bytes());
            bytes[o + 4] = max;
            bytes[o + 8..o + 12].copy_from_slice(&mask.to_le_bytes());
        }
        bytes
    }

    fn strings(prefix: char, ids: &[u16]) -> Vec<String> {
        let len = ids.iter().map(|&id| 2 * usize::from(id) + 2).max().unwrap();
        let mut out = vec![String::new(); len];
        for &id in ids {
            out[2 * usize::from(id)] = format!("{prefix}{id}");
            out[2 * usize::from(id) + 1] = format!("about {prefix}{id}");
        }
        out
    }

    fn tables() -> MeritTables {
        let mut categories = vec![category(0, 0, 0); 40];
        categories[1] = category(HP_MP, HP_MP, 15);
        categories[2] = category(2, 2, 0);
        categories[3] = category(0, 3, 9);
        categories[4] = category(WAR_CATEGORY, WAR_CATEGORY, SECTION_CAP);
        categories[5] = category(WEAPON_SKILL_SECTION, WEAPON_SKILL_SECTION, WEAPON_SKILL_CAP);
        categories[32] = category(WAR_SECOND_SECTION, WAR_SECOND_SECTION, SECTION_CAP);
        let all_jobs = u32::MAX;
        let mut job_categories = vec![
            JobPointCategory {
                job: 0,
                types: [0; ffxi_dat::merits::JOB_CATEGORY_TYPE_SLOTS],
            };
            6
        ];
        job_categories[usize::from(WAR)].job = WAR;
        job_categories[usize::from(WAR)].types[..4].copy_from_slice(&[
            WAR_TYPE_A,
            0,
            WAR_TYPE_UNDEFINED,
            WAR_TYPE_B,
        ]);
        job_categories[usize::from(RDM)].job = RDM;
        let mut type_definitions =
            vec![0u8; ffxi_dat::merits::JOB_POINT_TYPE_RECORD_LEN * usize::from(WAR_TYPE_B + 1)];
        for type_id in [WAR_TYPE_A, WAR_TYPE_B] {
            let o = usize::from(type_id) * ffxi_dat::merits::JOB_POINT_TYPE_RECORD_LEN;
            type_definitions[o..o + 2].copy_from_slice(&type_id.to_le_bytes());
            type_definitions[o + 3] = TYPE_MAX;
        }
        let mut gift_strings = vec![String::new(); 8];
        gift_strings[2] = "gift 1".into();
        gift_strings[6] = "gift 3".into();
        MeritTables {
            categories,
            definitions: definitions(&[
                (MAX_HP, MERIT_MAX, all_jobs),
                (MAX_MP, MERIT_MAX, all_jobs),
                (WAR_FIRST, MERIT_MAX, 1 << WAR),
                (WHM_ONLY, MERIT_MAX, 1 << WHM),
                (WAR_SECOND, MERIT_MAX, 1 << WAR),
                (WEAPON_SKILL, MERIT_MAX, all_jobs),
            ]),
            merit_strings: strings(
                'm',
                &[
                    HP_MP,
                    WAR_CATEGORY,
                    MAX_HP,
                    MAX_MP,
                    WAR_FIRST,
                    WHM_ONLY,
                    WAR_SECOND,
                    WEAPON_SKILL,
                ],
            ),
            job_categories,
            type_definitions,
            job_strings: strings(
                'j',
                &[u16::from(WAR), u16::from(RDM), WAR_TYPE_A, WAR_TYPE_B],
            ),
            gift_strings,
            gift_thresholds: [(WAR, vec![0, 100, 0, 500])].into_iter().collect(),
        }
    }

    fn texts() -> GetTextTables {
        let mut texts = GetTextTables::default();
        texts.insert_xistring(GETTEXT_TABLE_10, prompt_templates());
        texts.insert(GETTEXT_TABLE_SHORT_CAPTIONS, PROMPT_TITLE_ID, "title");
        texts.insert(GETTEXT_TABLE_12, kit::text12::COST_CAPTION, "cost");
        for id in 1..=2 {
            texts.insert(
                GETTEXT_TABLE_SHORT_CAPTIONS,
                u16::try_from(MERIT1_LABEL_ID).unwrap() + id,
                format!("item {id}"),
            );
        }
        texts
    }

    fn dat() -> MeritDat {
        let two_items = || {
            (1..=2)
                .map(|id| item(id, 10, 10 * id, 12, Some(MERIT1_LABEL_ID + i32::from(id))))
                .collect::<Vec<_>>()
        };
        let mut windows = BTreeMap::new();
        for name in [
            MERIT1_WINDOW,
            MERIT2_WINDOW,
            MERIT3_WINDOW,
            SORTYN_WINDOW,
            MERITYN_WINDOW,
        ] {
            windows.insert(name.to_string(), window(name, 120, 60, two_items()));
        }
        for (name, width) in [
            (LEVMERIT_WINDOW, 150),
            (MERITINF_WINDOW, 200),
            (LEVMERI2_WINDOW, 150),
            (BLUEHELP_WINDOW, 300),
        ] {
            windows.insert(name.to_string(), window(name, width, 90, vec![]));
        }
        MeritDat {
            loaded: true,
            windows,
            texts: texts(),
            tables: tables(),
        }
    }

    /// A WAR at 99 in the Mog House with 10 merit points and a few stored merits.
    fn snapshot() -> SceneSnapshot {
        let mut snap = SceneSnapshot {
            stats: Some(CharStats {
                mjob_no: WAR,
                mjob_lv: MASTER_LEVEL_JOB_CAP,
                ..Default::default()
            }),
            login_state: ffxi_proto::decode::ServerLoginMyroom::LOGIN_STATE_MYROOM,
            merits: Some(MisdataMerits {
                limit_points: 1234,
                merit_points: 10,
                blu_bonus: 0,
                can_use_merit_mode: true,
                xp_capped_or_merit_mode: false,
                merit_mode_enabled: false,
                max_merit_points: 30,
            }),
            ..Default::default()
        };
        snap.merit_entries
            .insert(MAX_HP, MeritEntry { next: 3, count: 1 });
        snap.merit_entries.insert(
            MAX_MP,
            MeritEntry {
                next: 0,
                count: MERIT_MAX,
            },
        );
        snap.merit_entries
            .insert(WAR_FIRST, MeritEntry { next: 2, count: 0 });
        snap.merit_entries
            .insert(WHM_ONLY, MeritEntry { next: 2, count: 1 });
        snap.merit_entries
            .insert(WAR_SECOND, MeritEntry { next: 11, count: 0 });
        snap
    }

    fn cells(rows: &[MeritRow]) -> Vec<MeritCell> {
        rows.iter()
            .filter_map(|row| match row {
                MeritRow::Merit(cell) => Some(*cell),
                _ => None,
            })
            .collect()
    }

    fn cell(rows: &[MeritRow], index: u16) -> MeritCell {
        cells(rows)
            .into_iter()
            .find(|cell| cell.index == index)
            .unwrap_or_else(|| panic!("no merit {index}"))
    }

    #[test]
    fn record_argb_words_pin() {
        const RECORD_PINNED_SECOND_SECTION: u32 = 0x8090_9040;
        const RECORD_PINNED_THIRD_SECTION: u32 = 0x8050_9050;
        assert_eq!(SECTION_COLORS[0], ARGB_NEUTRAL);
        assert_eq!(SECTION_COLORS[1], RECORD_PINNED_SECOND_SECTION);
        assert_eq!(SECTION_COLORS[2], RECORD_PINNED_THIRD_SECTION);
    }

    /// Top level: records 1..=31 in record order, skipping absent records (cap 0), byte-0 id 0
    /// and record 32 onward.
    #[test]
    fn category_rows_skip_absent_records_zero_ids_and_records_past_31() {
        assert_eq!(
            category_rows(&tables()),
            vec![
                MeritRow::Category { id: HP_MP },
                MeritRow::Category { id: WAR_CATEGORY },
                MeritRow::Category {
                    id: WEAPON_SKILL_SECTION
                },
            ]
        );
    }

    #[test]
    fn categories_1_to_5_and_26_hold_one_section_job_categories_three() {
        assert_eq!(sections_of(HP_MP), vec![HP_MP]);
        assert_eq!(
            sections_of(WEAPON_SKILL_SECTION),
            vec![WEAPON_SKILL_SECTION]
        );
        assert_eq!(
            sections_of(WAR_CATEGORY),
            vec![
                WAR_CATEGORY,
                WAR_SECOND_SECTION,
                WAR_CATEGORY + JOB_SECTION_OFFSETS[2]
            ]
        );
        assert_eq!(section_of_index(WAR_SECOND), WAR_SECOND_SECTION);
        assert_eq!(section_indices(HP_MP), MAX_HP..MAX_HP + MERITS_PER_SECTION);
        assert!(
            section_indices(u16::MAX).is_empty(),
            "a DAT id past the index space lists nothing"
        );
    }

    /// Job categories head each section that has a category record, then list its defined
    /// merits; one-section categories have no header.
    #[test]
    fn merit_rows_list_defined_merits_under_section_headers() {
        let snap = snapshot();
        let rows = merit_rows(WAR_CATEGORY, &tables(), &snap, true);
        assert_eq!(rows.len(), 5, "{rows:?}");
        assert_eq!(
            rows[0],
            MeritRow::Header {
                order: 0,
                cap: SECTION_CAP
            }
        );
        assert_eq!(
            cells(&rows).iter().map(|c| c.index).collect::<Vec<_>>(),
            vec![WAR_FIRST, WHM_ONLY, WAR_SECOND]
        );
        assert_eq!(
            rows[3],
            MeritRow::Header {
                order: 1,
                cap: SECTION_CAP
            }
        );
        let hp = merit_rows(HP_MP, &tables(), &snap, true);
        assert!(hp.iter().all(|row| matches!(row, MeritRow::Merit(_))));
        assert_eq!(hp.len(), 2);
    }

    /// E needs the Mog House at open and the job mask; the name takes the section color, the
    /// count turns blue at the max, the cost lights only when affordable under the section cap.
    #[test]
    fn merit_colors_follow_eligibility_sections_and_caps() {
        let mut snap = snapshot();
        let rows = merit_rows(WAR_CATEGORY, &tables(), &snap, true);
        let war = cell(&rows, WAR_FIRST);
        assert!(war.eligible);
        assert_eq!(war.name_color, SECTION_COLORS[0]);
        assert_eq!(war.cost_color, ARGB_NEUTRAL);
        assert!(war.can_raise() && !war.can_lower());
        let whm = cell(&rows, WHM_ONLY);
        assert!(!whm.eligible, "not the main job's merit");
        assert_eq!((whm.name_color, whm.cost_color), (ARGB_DIM, ARGB_DIM));
        assert!(!whm.can_raise() && !whm.can_lower(), "count 1 but not E");
        let second = cell(&rows, WAR_SECOND);
        assert_eq!(second.name_color, SECTION_COLORS[1]);
        assert_eq!(second.cost_color, ARGB_DIM, "next 11 > 10 merit points");

        let hp_rows = merit_rows(HP_MP, &tables(), &snap, true);
        assert_eq!(cell(&hp_rows, MAX_MP).count_color, ARGB_CAPPED);
        assert_eq!(cell(&hp_rows, MAX_HP).count_color, SECTION_COLORS[0]);

        let outside = merit_rows(WAR_CATEGORY, &tables(), &snap, false);
        assert!(cells(&outside)
            .iter()
            .all(|c| !c.eligible && c.name_color == ARGB_DIM));

        snap.merit_entries
            .insert(MAX_HP, MeritEntry { next: 3, count: 6 });
        let below_cap = merit_rows(HP_MP, &tables(), &snap, true);
        assert_eq!(
            cell(&below_cap, MAX_HP).cost_color,
            ARGB_NEUTRAL,
            "6 + 8 stays under HP/MP's cap of 15"
        );
        snap.merit_entries
            .insert(MAX_HP, MeritEntry { next: 3, count: 7 });
        let at_cap = merit_rows(HP_MP, &tables(), &snap, true);
        assert_eq!(
            cell(&at_cap, MAX_HP).cost_color,
            ARGB_DIM,
            "7 + 8 reaches the cap"
        );
        snap.merits = None;
        let no_record = merit_rows(WAR_CATEGORY, &tables(), &snap, true);
        assert_eq!(cell(&no_record, WAR_FIRST).cost_color, ARGB_DIM);
    }

    /// The weapon skill section needs main job level 96 and key item 2059; its cap takes 5 per
    /// martial technique key item, stopping at 100.
    #[test]
    fn weapon_skill_section_gates_and_cap_bonus() {
        let mut snap = snapshot();
        snap.merit_entries
            .insert(WEAPON_SKILL, MeritEntry { next: 1, count: 0 });
        let tables = tables();
        assert!(
            !cell(
                &merit_rows(WEAPON_SKILL_SECTION, &tables, &snap, true),
                WEAPON_SKILL
            )
            .eligible
        );
        snap.key_items.push(HEART_OF_THE_BUSHIN_KEY_ITEM);
        assert!(
            cell(
                &merit_rows(WEAPON_SKILL_SECTION, &tables, &snap, true),
                WEAPON_SKILL
            )
            .eligible
        );
        snap.stats = Some(CharStats {
            mjob_no: WAR,
            mjob_lv: WEAPON_SKILL_LEVEL - 1,
            ..Default::default()
        });
        assert!(
            !cell(
                &merit_rows(WEAPON_SKILL_SECTION, &tables, &snap, true),
                WEAPON_SKILL
            )
            .eligible
        );

        assert_eq!(
            section_cap(&tables, WEAPON_SKILL_SECTION, &[]),
            WEAPON_SKILL_CAP
        );
        assert_eq!(
            section_cap(
                &tables,
                WEAPON_SKILL_SECTION,
                &MARTIAL_TECHNIQUE_KEY_ITEMS[..1]
            ),
            WEAPON_SKILL_CAP + MARTIAL_TECHNIQUE_CAP_BONUS
        );
        assert_eq!(
            section_cap(&tables, WEAPON_SKILL_SECTION, &MARTIAL_TECHNIQUE_KEY_ITEMS),
            WEAPON_SKILL_CAP_MAX
        );
        assert_eq!(
            section_cap(&tables, HP_MP, &MARTIAL_TECHNIQUE_KEY_ITEMS),
            15,
            "the bonus is the weapon skill section's alone"
        );
    }

    /// `meritinf`'s cap comes from category record number (top-level row + 1), as the record
    /// states, whatever row that record backs.
    #[test]
    fn info_cap_reads_the_record_after_the_highlighted_row() {
        let tables = tables();
        assert_eq!(info_cap(&tables, 0, &[]), 15, "row 0 reads record 1");
        assert_eq!(
            info_cap(&tables, 3, &[]),
            SECTION_CAP,
            "row 3 reads record 4"
        );
        assert_eq!(
            info_cap(&tables, 4, &MARTIAL_TECHNIQUE_KEY_ITEMS),
            WEAPON_SKILL_CAP_MAX,
            "record 5 names section 26"
        );
        assert_eq!(info_cap(&tables, 99, &[]), 0);
    }

    fn menu_stack() -> MenuStack {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Status);
        stack
    }

    fn kinds(stack: &MenuStack) -> Vec<MenuKind> {
        stack.levels.iter().map(|level| level.kind).collect()
    }

    fn top(stack: &MenuStack) -> (MenuKind, usize) {
        let level = stack.current().unwrap();
        (level.kind, level.cursor)
    }

    fn select(
        stack: &mut MenuStack,
        dynamic: &mut DynamicMenu,
        snap: &SceneSnapshot,
        dat: &MeritDat,
    ) -> MeritOutcome {
        sync_merit_windows(dynamic, Some(stack), snap, dat);
        let level = stack.current().unwrap().clone();
        let rows = merit_rows_for(&level, dat, &dynamic.merit, snap);
        let action = rows.get(level.cursor).map(|row| row.action);
        let window = merit_window(level.kind).expect("a merit window");
        select_merit_row(window, action, stack, dynamic, snap)
    }

    fn sounds(dynamic: &mut DynamicMenu) -> Vec<UiSound> {
        std::mem::take(&mut dynamic.ui_sounds)
    }

    #[test]
    fn merit_menu_opens_with_levmerit_and_disables_item_2_under_monstrosity() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        let mut snap = snapshot();
        let mut stack = menu_stack();
        stack.push(MenuKind::StatusDetail { entry: 0 });
        assert_eq!(
            open_merit_menu(&mut stack, &mut dynamic, &snap),
            MeritOutcome::default()
        );
        assert_eq!(
            kinds(&stack),
            vec![
                MenuKind::Root,
                MenuKind::Status,
                MenuKind::Merit(MeritWindow::Menu)
            ],
            "the display-only detail closes first"
        );
        let level = stack.current().unwrap().clone();
        let rows = merit_rows_for(&level, &dat, &dynamic.merit, &snap);
        assert_eq!(
            rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
            vec!["item 1", "item 2"],
            "labels from the record"
        );
        assert_eq!(
            rows[1].action,
            DynamicMenuAction::Merit(MeritAction::Item {
                item: MERIT1_ITEM_MODE,
                enabled: true
            })
        );
        let cx = MeritContext {
            snap: &snap,
            dat: &dat,
            windows: &dynamic.merit,
        };
        let drawn = panel_windows(&stack, |_| rows.clone(), &cx);
        assert_eq!(
            drawn
                .iter()
                .map(|w| (w.name, w.shift_y))
                .collect::<Vec<_>>(),
            vec![(MERIT1_WINDOW, 0.0), (LEVMERIT_WINDOW, 0.0)]
        );

        snap.login_is_monstrosity = true;
        stack.pop();
        open_merit_menu(&mut stack, &mut dynamic, &snap);
        let rows = merit_rows_for(stack.current().unwrap(), &dat, &dynamic.merit, &snap);
        assert_eq!(
            rows[1].action,
            DynamicMenuAction::Merit(MeritAction::Item {
                item: MERIT1_ITEM_MODE,
                enabled: false
            })
        );
        let cx = MeritContext {
            snap: &snap,
            dat: &dat,
            windows: &dynamic.merit,
        };
        let drawn = panel_windows(&stack, |_| rows.clone(), &cx);
        assert_eq!(drawn[1].shift_y, LEVMERIT_MONSTROSITY_DROP);
        let item2 = drawn[0].texts.iter().find(|t| t.text == "item 2").unwrap();
        assert_eq!(item2.color, ARGB_DIM, "a disabled item draws grey");
        stack.current_mut().unwrap().cursor = 1;
        assert_eq!(
            select(&mut stack, &mut dynamic, &snap, &dat),
            MeritOutcome::default()
        );
        assert_eq!(top(&stack).0, MenuKind::Merit(MeritWindow::Menu), "inert");
    }

    /// Level restriction, besieged or level sync: message 187, no buzzer, nothing opens.
    #[test]
    fn mode_item_refuses_under_a_restricting_effect_with_message_187() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        dynamic.merit.mode_refused_message = Some("refused".into());
        for effect in MODE_REFUSING_EFFECTS {
            let mut snap = snapshot();
            snap.status_icons.push(effect);
            let mut stack = menu_stack();
            open_merit_menu(&mut stack, &mut dynamic, &snap);
            stack.current_mut().unwrap().cursor = 1;
            let outcome = select(&mut stack, &mut dynamic, &snap, &dat);
            assert_eq!(outcome.chat_lines, vec!["refused".to_string()]);
            assert!(sounds(&mut dynamic).is_empty(), "no buzzer");
            assert_eq!(top(&stack).0, MenuKind::Merit(MeritWindow::Menu));
        }
    }

    /// `merit2` disables the current mode's item and sends ChangeMode for the other, staying open.
    #[test]
    fn mode_items_disable_the_current_mode_and_send_change_mode() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        let mut snap = snapshot();
        assert_eq!(mode_items_enabled(&snap), (false, true), "EXP mode now");
        let mut stack = menu_stack();
        open_merit_menu(&mut stack, &mut dynamic, &snap);
        stack.current_mut().unwrap().cursor = 1;
        select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(top(&stack).0, MenuKind::Merit(MeritWindow::Mode));
        assert_eq!(
            select(&mut stack, &mut dynamic, &snap, &dat),
            MeritOutcome::default(),
            "item 1 (EXP) is disabled in EXP mode"
        );
        stack.current_mut().unwrap().cursor = 1;
        let outcome = select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(
            outcome.requests,
            vec![MeritRequest::SetMode { limit_points: true }]
        );
        assert_eq!(
            top(&stack).0,
            MenuKind::Merit(MeritWindow::Mode),
            "stays open"
        );
        assert!(sounds(&mut dynamic).is_empty(), "no handler sound");

        snap.merits.as_mut().unwrap().merit_mode_enabled = true;
        assert_eq!(mode_items_enabled(&snap), (true, false));
        snap.merits = None;
        assert_eq!(mode_items_enabled(&snap), (true, true));
    }

    fn open_categories(
        dynamic: &mut DynamicMenu,
        snap: &SceneSnapshot,
        dat: &MeritDat,
    ) -> MenuStack {
        let mut stack = menu_stack();
        open_merit_menu(&mut stack, dynamic, snap);
        stack.current_mut().unwrap().cursor = 0;
        select(&mut stack, dynamic, snap, dat);
        stack
    }

    /// Selecting a category enters it on its first row with 0002; Esc returns to the list with the
    /// cursor back on that category, and a later reopen lands on the row highlighted at close.
    #[test]
    fn category_select_enters_and_esc_returns_to_it() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        let snap = snapshot();
        let mut stack = open_categories(&mut dynamic, &snap, &dat);
        assert_eq!(
            top(&stack),
            (
                MenuKind::Merit(MeritWindow::Categories { entered: None }),
                0
            )
        );
        stack.current_mut().unwrap().cursor = 1;
        select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(
            top(&stack),
            (
                MenuKind::Merit(MeritWindow::Categories {
                    entered: Some(WAR_CATEGORY)
                }),
                0
            )
        );
        assert_eq!(sounds(&mut dynamic), vec![UiSound::Confirm]);
        sync_merit_windows(&mut dynamic, Some(&stack), &snap, &dat);
        let rows = merit_rows_for(stack.current().unwrap(), &dat, &dynamic.merit, &snap);
        assert_eq!(
            rows[0].action,
            DynamicMenuAction::Merit(MeritAction::SectionHeader)
        );
        assert_eq!(rows[1].label, format!("m{WAR_FIRST}"));
        assert!(
            select(&mut stack, &mut dynamic, &snap, &dat) == MeritOutcome::default()
                && sounds(&mut dynamic).is_empty(),
            "a header row does nothing"
        );

        assert!(cancel_merit_window(&mut stack, &mut dynamic));
        assert_eq!(
            top(&stack),
            (
                MenuKind::Merit(MeritWindow::Categories { entered: None }),
                1
            )
        );
        assert_eq!(sounds(&mut dynamic), vec![UiSound::Cancel]);
        assert!(
            !cancel_merit_window(&mut stack, &mut dynamic),
            "the top level closes"
        );
        sync_merit_windows(&mut dynamic, Some(&stack), &snap, &dat);
        stack.pop();
        sync_merit_windows(&mut dynamic, Some(&stack), &snap, &dat);
        assert_eq!(dynamic.merit.categories, None);
        stack.current_mut().unwrap().cursor = 0;
        select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(top(&stack).1, 1, "reopens on the row highlighted at close");
    }

    fn enter_war(dynamic: &mut DynamicMenu, snap: &SceneSnapshot, dat: &MeritDat) -> MenuStack {
        let mut stack = open_categories(dynamic, snap, dat);
        stack.current_mut().unwrap().cursor = 1;
        select(&mut stack, dynamic, snap, dat);
        sync_merit_windows(dynamic, Some(&stack), snap, dat);
        sounds(dynamic);
        stack
    }

    fn row_of(
        stack: &MenuStack,
        dynamic: &DynamicMenu,
        snap: &SceneSnapshot,
        dat: &MeritDat,
        index: u16,
    ) -> usize {
        merit_rows_for(stack.current().unwrap(), dat, &dynamic.merit, snap)
            .iter()
            .position(|row| row.action == DynamicMenuAction::Merit(MeritAction::Merit { index }))
            .unwrap()
    }

    /// Outside the Mog House at open: message 178 then the buzzer; neither action possible: the
    /// buzzer; otherwise `merit3` opens.
    #[test]
    fn merit_select_refusals_then_merit3() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        dynamic.merit.not_in_mog_house_message = Some("not here".into());
        let mut outside = snapshot();
        outside.login_state = ffxi_proto::decode::ServerLoginMyroom::LOGIN_STATE_GAME;
        let mut stack = enter_war(&mut dynamic, &outside, &dat);
        stack.current_mut().unwrap().cursor = row_of(&stack, &dynamic, &outside, &dat, WAR_FIRST);
        let outcome = select(&mut stack, &mut dynamic, &outside, &dat);
        assert_eq!(outcome.chat_lines, vec!["not here".to_string()]);
        assert_eq!(sounds(&mut dynamic), vec![UiSound::Buzzer]);

        let snap = snapshot();
        let mut stack = enter_war(&mut dynamic, &snap, &dat);
        stack.current_mut().unwrap().cursor = row_of(&stack, &dynamic, &snap, &dat, WHM_ONLY);
        assert_eq!(
            select(&mut stack, &mut dynamic, &snap, &dat),
            MeritOutcome::default()
        );
        assert_eq!(sounds(&mut dynamic), vec![UiSound::Buzzer]);

        stack.current_mut().unwrap().cursor = row_of(&stack, &dynamic, &snap, &dat, WAR_FIRST);
        select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(
            top(&stack),
            (
                MenuKind::Merit(MeritWindow::RaiseLower { index: WAR_FIRST }),
                0
            ),
            "merit3 opens on item 1"
        );
        let rows = merit_rows_for(stack.current().unwrap(), &dat, &dynamic.merit, &snap);
        assert_eq!(
            rows.iter().map(|r| r.action).collect::<Vec<_>>(),
            vec![
                DynamicMenuAction::Merit(MeritAction::Item {
                    item: MERIT3_ITEM_RAISE,
                    enabled: true
                }),
                DynamicMenuAction::Merit(MeritAction::Item {
                    item: MERIT3_ITEM_LOWER,
                    enabled: false
                }),
            ],
            "count 0: Lower is disabled"
        );
    }

    fn open_raise_lower(
        dynamic: &mut DynamicMenu,
        snap: &SceneSnapshot,
        dat: &MeritDat,
        index: u16,
    ) -> MenuStack {
        let mut stack = enter_war(dynamic, snap, dat);
        stack.current_mut().unwrap().cursor = row_of(&stack, dynamic, snap, dat, index);
        select(&mut stack, dynamic, snap, dat);
        stack
    }

    /// Raise: `sortyn` opens on No under the 13/130 title and the 612 help line; Yes opens
    /// `merityn` on No; Yes there sends EditMode raise and closes `merit3`.
    #[test]
    fn raise_flow_opens_each_prompt_on_no_and_yes_sends_edit_raise() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        let snap = snapshot();
        let mut stack = open_raise_lower(&mut dynamic, &snap, &dat, WAR_FIRST);
        select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(
            top(&stack),
            (
                MenuKind::Merit(MeritWindow::SortConfirm {
                    index: WAR_FIRST,
                    raise: true
                }),
                CONFIRM_NO_ROW
            )
        );
        assert_eq!(dynamic.merit.last_chosen, Some(true));
        let cx = MeritContext {
            snap: &snap,
            dat: &dat,
            windows: &dynamic.merit,
        };
        let drawn = panel_windows(&stack, |_| Vec::new(), &cx);
        let title = drawn
            .iter()
            .find(|w| w.name == TITLEWIN_WINDOW)
            .expect("titled");
        assert_eq!(title.texts[0].text, "title");

        stack.current_mut().unwrap().cursor = 0;
        select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(
            top(&stack),
            (
                MenuKind::Merit(MeritWindow::Confirm {
                    index: WAR_FIRST,
                    raise: true
                }),
                CONFIRM_NO_ROW
            ),
            "sortyn answered Yes and closed; merityn opens on No"
        );
        let below = stack.levels[stack.levels.len() - 2].kind;
        assert_eq!(
            below,
            MenuKind::Merit(MeritWindow::RaiseLower { index: WAR_FIRST })
        );

        assert!(cancel_merit_window(&mut stack, &mut dynamic));
        assert_eq!(
            sounds(&mut dynamic),
            vec![UiSound::Buzzer],
            "Esc only buzzes"
        );
        assert!(matches!(
            top(&stack).0,
            MenuKind::Merit(MeritWindow::Confirm { .. })
        ));

        stack.current_mut().unwrap().cursor = 0;
        let outcome = select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(
            outcome.requests,
            vec![MeritRequest::Edit {
                index: WAR_FIRST,
                raise: true
            }]
        );
        assert_eq!(
            top(&stack).0,
            MenuKind::Merit(MeritWindow::Categories {
                entered: Some(WAR_CATEGORY)
            }),
            "merit3 closes with the answer"
        );
    }

    /// No on `sortyn` and on `merityn` both land back on `merit3` and send nothing; Lower uses
    /// the lower prompt.
    #[test]
    fn no_answers_return_to_merit3_and_lower_sends_edit_lower() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        let mut snap = snapshot();
        snap.merit_entries
            .insert(WAR_FIRST, MeritEntry { next: 2, count: 1 });
        let mut stack = open_raise_lower(&mut dynamic, &snap, &dat, WAR_FIRST);
        stack.current_mut().unwrap().cursor = 1;
        select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(
            top(&stack).0,
            MenuKind::Merit(MeritWindow::SortConfirm {
                index: WAR_FIRST,
                raise: false
            })
        );
        assert_eq!(
            select(&mut stack, &mut dynamic, &snap, &dat),
            MeritOutcome::default()
        );
        assert_eq!(
            top(&stack).0,
            MenuKind::Merit(MeritWindow::RaiseLower { index: WAR_FIRST })
        );

        stack.current_mut().unwrap().cursor = 1;
        select(&mut stack, &mut dynamic, &snap, &dat);
        stack.current_mut().unwrap().cursor = 0;
        select(&mut stack, &mut dynamic, &snap, &dat);
        let cx = MeritContext {
            snap: &snap,
            dat: &dat,
            windows: &dynamic.merit,
        };
        assert_eq!(
            merit_prompt(&cx, WAR_FIRST, false),
            ConfirmPrompt {
                entry: LOWER_PROMPT_ENTRY,
                args: vec![PromptArg::Name(format!("m{WAR_FIRST}"))]
            }
        );
        assert_eq!(
            select(&mut stack, &mut dynamic, &snap, &dat),
            MeritOutcome::default(),
            "No on merityn"
        );
        assert_eq!(
            top(&stack).0,
            MenuKind::Merit(MeritWindow::RaiseLower { index: WAR_FIRST })
        );
        stack.current_mut().unwrap().cursor = 1;
        select(&mut stack, &mut dynamic, &snap, &dat);
        stack.current_mut().unwrap().cursor = 0;
        select(&mut stack, &mut dynamic, &snap, &dat);
        stack.current_mut().unwrap().cursor = 0;
        assert_eq!(
            select(&mut stack, &mut dynamic, &snap, &dat).requests,
            vec![MeritRequest::Edit {
                index: WAR_FIRST,
                raise: false
            }]
        );
    }

    #[test]
    fn raise_prompt_takes_the_name_and_next_cost() {
        let dat = dat();
        let dynamic = DynamicMenu::default();
        let snap = snapshot();
        let cx = MeritContext {
            snap: &snap,
            dat: &dat,
            windows: &dynamic.merit,
        };
        let prompt = merit_prompt(&cx, MAX_HP, true);
        assert_eq!(
            prompt,
            ConfirmPrompt {
                entry: RAISE_PROMPT_ENTRY,
                args: vec![PromptArg::Name(format!("m{MAX_HP}")), PromptArg::Points(3)]
            }
        );
    }

    /// `merityn` draws the raise and lower prompts' formatted entries 4 in from its left and
    /// top, wrapped to its width less 8, lines 16 apart, in neutral.
    #[test]
    fn merityn_draws_the_formatted_prompt_at_the_record_place() {
        let dat = dat();
        let dynamic = DynamicMenu::default();
        let snap = snapshot();
        let cx = MeritContext {
            snap: &snap,
            dat: &dat,
            windows: &dynamic.merit,
        };
        let width = f32::from(cx.frame(MERITYN_WINDOW).expect("merityn record").width);
        let name = format!("m{MAX_HP}");
        for (raise, want) in [
            (true, prompt_text(RAISE_PROMPT_ENTRY, &name, Some(3))),
            (false, prompt_text(LOWER_PROMPT_ENTRY, &name, None)),
        ] {
            let mut stack = menu_stack();
            stack.push(MenuKind::Merit(MeritWindow::Confirm {
                index: MAX_HP,
                raise,
            }));
            let drawn = panel_windows(&stack, |_| Vec::new(), &cx);
            let merityn = drawn
                .iter()
                .find(|w| w.name == MERITYN_WINDOW)
                .expect("merityn drawn");
            let prompt = merityn
                .texts
                .iter()
                .find(|t| t.text == want)
                .unwrap_or_else(|| panic!("no {want:?} in {:?}", merityn.texts));
            assert_eq!(
                (prompt.x, prompt.y),
                (
                    kit::layout::MERITYN_TEXT_INSET,
                    kit::layout::MERITYN_TEXT_INSET
                )
            );
            assert_eq!(
                prompt.wrap_width,
                Some(width - kit::layout::MERITYN_WRAP_MARGIN)
            );
            assert_eq!(prompt.line_pitch, Some(kit::layout::MERITYN_LINE_PITCH));
            assert_eq!(prompt.color, ARGB_NEUTRAL);
        }
    }

    /// `merit3` closes with its prompts once neither action is possible, or once the action last
    /// chosen is gone; each refresh re-evaluates both from the rebuilt rows.
    #[test]
    fn merit3_closes_with_its_prompts_when_its_actions_go() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        let mut snap = snapshot();
        snap.merit_entries
            .insert(WAR_FIRST, MeritEntry { next: 2, count: 1 });
        let mut stack = open_raise_lower(&mut dynamic, &snap, &dat, WAR_FIRST);
        let merit3_at = stack.levels.len() - 1;
        select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(raise_lower_close_at(&stack, &dynamic.merit), None);

        snap.merits.as_mut().unwrap().merit_points = 1;
        sync_merit_windows(&mut dynamic, Some(&stack), &snap, &dat);
        assert_eq!(
            raise_lower_close_at(&stack, &dynamic.merit),
            None,
            "a Merits record alone does not rebuild the rows"
        );
        snap.merit_entries
            .insert(MAX_HP, MeritEntry { next: 3, count: 2 });
        sync_merit_windows(&mut dynamic, Some(&stack), &snap, &dat);
        assert_eq!(
            raise_lower_close_at(&stack, &dynamic.merit),
            Some(merit3_at),
            "an s2c 0x08C rebuilds: raise, the chosen item, is gone"
        );

        dynamic.merit.last_chosen = Some(false);
        assert_eq!(
            raise_lower_close_at(&stack, &dynamic.merit),
            None,
            "lower remains"
        );
        snap.merit_entries
            .insert(WAR_FIRST, MeritEntry { next: 2, count: 0 });
        sync_merit_windows(&mut dynamic, Some(&stack), &snap, &dat);
        assert_eq!(
            raise_lower_close_at(&stack, &dynamic.merit),
            Some(merit3_at)
        );
    }

    fn jobs_snapshot() -> SceneSnapshot {
        let mut snap = snapshot();
        let mut job_levels = vec![0u8; 23];
        job_levels[usize::from(WAR)] = MASTER_LEVEL_JOB_CAP;
        job_levels[usize::from(RDM)] = 75;
        snap.jobs = Some(JobList {
            unlocked: 0,
            sub_job_unlocked: false,
            job_levels,
        });
        snap.job_points_received = true;
        let mut record = JobPointsRecord {
            access: 1,
            ..Default::default()
        };
        record.jobs[usize::from(WAR)] = JobPointsJob {
            capacity_points: 25_000,
            current_jp: 12,
            total_jp_spent: 100,
        };
        snap.job_points = Some(record);
        snap.job_point_types
            .insert(WAR_TYPE_A, JobPointType { level: 3, next: 4 });
        snap.job_point_types.insert(
            WAR_TYPE_B,
            JobPointType {
                level: TYPE_MAX,
                next: 0,
            },
        );
        snap
    }

    /// `merit2ca` asks for c2s 0x0C0 only before any s2c 0x08D, lists the jobs with a record
    /// and opens `jbpcat` only for jobs 1..=22 at 99.
    #[test]
    fn job_list_requests_data_once_and_opens_jbpcat_for_level_99_jobs() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        let mut snap = jobs_snapshot();
        snap.job_points_received = false;
        let mut stack = menu_stack();
        assert_eq!(
            open_job_point_jobs(&mut stack, &snap).requests,
            vec![MeritRequest::RequestJobPoints]
        );
        snap.job_points_received = true;
        stack.pop();
        assert!(open_job_point_jobs(&mut stack, &snap).requests.is_empty());
        assert_eq!(top(&stack), (MenuKind::Merit(MeritWindow::Jobs), 0));

        assert_eq!(
            job_rows(&dat.tables, &snap),
            vec![(WAR, ARGB_NEUTRAL), (RDM, ARGB_DIM)]
        );
        stack.current_mut().unwrap().cursor = 1;
        select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(sounds(&mut dynamic), vec![UiSound::Buzzer], "RDM is 75");
        stack.current_mut().unwrap().cursor = 0;
        select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(
            top(&stack),
            (
                MenuKind::Merit(MeritWindow::JobCategories {
                    job: WAR,
                    gifts: false
                }),
                0
            )
        );

        let cx = MeritContext {
            snap: &snap,
            dat: &dat,
            windows: &dynamic.merit,
        };
        let levmeri2 = levmeri2_texts(&cx, Some(WAR));
        assert_eq!(levmeri2[0].text, format!("25000/{CAPACITY_POINT_MAX}"));
        assert_eq!(levmeri2[1].text, format!("12/{JOB_POINT_MAX} P"));
        assert_eq!(levmeri2[1].color, ARGB_NEUTRAL);
    }

    /// Job Points rows skip empty slots and undefined types; colors follow S, data, cap and
    /// affordability. Gift rows list thresholds lit by the job's spent total.
    #[test]
    fn job_page_rows_and_colors() {
        let tables = tables();
        let snap = jobs_snapshot();
        let rows = job_point_rows(WAR, &tables, &snap, true);
        assert_eq!(
            rows,
            vec![
                JobPageRow::Type(JobPointCell {
                    type_id: WAR_TYPE_A,
                    level: 3,
                    max: TYPE_MAX,
                    next: 4,
                    state: UpgradeRowState::Affordable
                }),
                JobPageRow::Type(JobPointCell {
                    type_id: WAR_TYPE_B,
                    level: TYPE_MAX,
                    max: TYPE_MAX,
                    next: 0,
                    state: UpgradeRowState::AtCap
                }),
            ]
        );
        assert!(job_point_rows(WAR, &tables, &snap, false).iter().all(
            |row| matches!(row, JobPageRow::Type(c) if c.state == UpgradeRowState::Unavailable)
        ));
        let stored = JobPointType { level: 3, next: 13 };
        assert_eq!(
            job_point_state(true, stored, TYPE_MAX, Some(12)),
            UpgradeRowState::TooCostly
        );
        assert_eq!(
            job_point_state(true, stored, TYPE_MAX, None),
            UpgradeRowState::TooCostly,
            "no JobPoints record"
        );
        assert_eq!(
            job_point_state(true, JobPointType::default(), TYPE_MAX, Some(99)),
            UpgradeRowState::Unavailable,
            "no stored data"
        );

        assert_eq!(
            gift_rows(WAR, &tables, &snap),
            vec![
                JobPageRow::Gift(GiftCell {
                    gift: 1,
                    threshold: 100,
                    color: ARGB_NEUTRAL
                }),
                JobPageRow::Gift(GiftCell {
                    gift: 3,
                    threshold: 500,
                    color: ARGB_DIM
                }),
            ]
        );
    }

    fn open_war_page(dynamic: &mut DynamicMenu, snap: &SceneSnapshot, dat: &MeritDat) -> MenuStack {
        let mut stack = menu_stack();
        open_job_point_jobs(&mut stack, snap);
        select(&mut stack, dynamic, snap, dat);
        sync_merit_windows(dynamic, Some(&stack), snap, dat);
        stack
    }

    /// A raisable row opens `merityn` on No; Yes sends c2s 0x0BF and starts the lockout, during
    /// which a raisable row plays 0002 and opens nothing until 30 counted passes run out.
    #[test]
    fn spend_opens_on_no_yes_sends_spend_and_the_lockout_holds_for_30_passes() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        let snap = jobs_snapshot();
        let mut stack = open_war_page(&mut dynamic, &snap, &dat);
        assert!(lockout_counts(&stack));
        select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(
            top(&stack),
            (
                MenuKind::Merit(MeritWindow::JobPointConfirm {
                    job: WAR,
                    type_id: WAR_TYPE_A
                }),
                CONFIRM_NO_ROW
            )
        );
        assert!(!lockout_counts(&stack));
        let cx = MeritContext {
            snap: &snap,
            dat: &dat,
            windows: &dynamic.merit,
        };
        assert_eq!(
            spend_prompt(&cx, WAR, WAR_TYPE_A),
            ConfirmPrompt {
                entry: SPEND_PROMPT_ENTRY,
                args: vec![
                    PromptArg::Name(format!("j{WAR_TYPE_A}")),
                    PromptArg::Points(4)
                ]
            }
        );
        let drawn = panel_windows(&stack, |_| Vec::new(), &cx);
        assert!(
            drawn.iter().all(|w| w.name != TITLEWIN_WINDOW),
            "the spend prompt has no title"
        );
        let want = prompt_text(SPEND_PROMPT_ENTRY, &format!("j{WAR_TYPE_A}"), Some(4));
        assert!(
            drawn
                .iter()
                .filter(|w| w.name == MERITYN_WINDOW)
                .flat_map(|w| &w.texts)
                .any(|t| t.text == want),
            "merityn draws {want:?}"
        );

        assert!(cancel_merit_window(&mut stack, &mut dynamic));
        assert_eq!(sounds(&mut dynamic), vec![UiSound::Buzzer]);
        stack.current_mut().unwrap().cursor = 0;
        let outcome = select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(
            outcome.requests,
            vec![MeritRequest::SpendJobPoint {
                type_id: WAR_TYPE_A
            }]
        );
        assert!(dynamic.merit.spend_lockout.is_running());

        select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(sounds(&mut dynamic), vec![UiSound::Confirm]);
        assert!(matches!(
            top(&stack).0,
            MenuKind::Merit(MeritWindow::JobCategories { .. })
        ));
        dynamic.merit.spend_lockout.tick(SPEND_LOCKOUT_PASSES - 1);
        select(&mut stack, &mut dynamic, &snap, &dat);
        assert_eq!(
            sounds(&mut dynamic),
            vec![UiSound::Confirm],
            "one pass left"
        );
        dynamic.merit.spend_lockout.tick(1);
        select(&mut stack, &mut dynamic, &snap, &dat);
        assert!(matches!(
            top(&stack).0,
            MenuKind::Merit(MeritWindow::JobPointConfirm { .. })
        ));

        let frame = 1.0 / crate::scheduler_runtime::RETAIL_FPS;
        let mut by_time = SpendLockout::default();
        by_time.start();
        for _ in 1..SPEND_LOCKOUT_PASSES {
            by_time.advance(frame);
        }
        assert!(by_time.is_running());
        by_time.advance(frame);
        assert!(!by_time.is_running());
    }

    /// Outside S: message 178 and the buzzer. Capped rows and Gifts rows only buzz.
    #[test]
    fn job_page_refusals() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        dynamic.merit.not_in_mog_house_message = Some("not here".into());
        let mut outside = jobs_snapshot();
        outside.login_state = ffxi_proto::decode::ServerLoginMyroom::LOGIN_STATE_GAME;
        let mut stack = open_war_page(&mut dynamic, &outside, &dat);
        let outcome = select(&mut stack, &mut dynamic, &outside, &dat);
        assert_eq!(outcome.chat_lines, vec!["not here".to_string()]);
        assert_eq!(sounds(&mut dynamic), vec![UiSound::Buzzer]);

        let snap = jobs_snapshot();
        let mut stack = open_war_page(&mut dynamic, &snap, &dat);
        stack.current_mut().unwrap().cursor = 1;
        assert_eq!(
            select(&mut stack, &mut dynamic, &snap, &dat),
            MeritOutcome::default()
        );
        assert_eq!(sounds(&mut dynamic), vec![UiSound::Buzzer], "capped");

        assert!(toggle_job_page(&mut stack, &mut dynamic));
        sync_merit_windows(&mut dynamic, Some(&stack), &snap, &dat);
        assert_eq!(
            select(&mut stack, &mut dynamic, &snap, &dat),
            MeritOutcome::default()
        );
        assert_eq!(
            sounds(&mut dynamic),
            vec![UiSound::PageToggle, UiSound::Buzzer],
            "a Gifts row buzzes"
        );
        assert!(!lockout_counts(&stack), "the Gifts page counts no passes");
    }

    /// Left and Right toggle the page with 0001; each page keeps its cursor row.
    #[test]
    fn page_toggle_keeps_each_pages_cursor() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        let snap = jobs_snapshot();
        let mut stack = open_war_page(&mut dynamic, &snap, &dat);
        stack.current_mut().unwrap().cursor = 1;
        assert!(toggle_job_page(&mut stack, &mut dynamic));
        assert_eq!(
            top(&stack),
            (
                MenuKind::Merit(MeritWindow::JobCategories {
                    job: WAR,
                    gifts: true
                }),
                0
            )
        );
        sync_merit_windows(&mut dynamic, Some(&stack), &snap, &dat);
        let rows = merit_rows_for(stack.current().unwrap(), &dat, &dynamic.merit, &snap);
        assert_eq!(
            rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
            vec!["gift 1", "gift 3"]
        );
        stack.current_mut().unwrap().cursor = 1;
        assert!(toggle_job_page(&mut stack, &mut dynamic));
        assert_eq!(top(&stack).1, 1, "the Job Points page's row");
        assert!(toggle_job_page(&mut stack, &mut dynamic));
        assert_eq!(top(&stack).1, 1, "the Gifts page's row");
        assert_eq!(sounds(&mut dynamic), vec![UiSound::PageToggle; 3]);

        let mut merit_stack = menu_stack();
        open_merit_menu(&mut merit_stack, &mut dynamic, &snap);
        assert!(!toggle_job_page(&mut merit_stack, &mut dynamic));
    }

    /// 0x08D or 0x063 JobPoints changes for the open job rebuild the Job Points page; the Gifts
    /// page builds when shown.
    #[test]
    fn job_page_rebuilds_on_its_jobs_data() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        let mut snap = jobs_snapshot();
        let stack = open_war_page(&mut dynamic, &snap, &dat);
        let state = |dynamic: &DynamicMenu| dynamic.merit.job_page.clone().unwrap();
        assert_eq!(state(&dynamic).cell(WAR_TYPE_A).unwrap().level, 3);
        snap.job_point_types
            .insert(WAR_TYPE_A, JobPointType { level: 4, next: 5 });
        sync_merit_windows(&mut dynamic, Some(&stack), &snap, &dat);
        assert_eq!(state(&dynamic).cell(WAR_TYPE_A).unwrap().level, 4);
        snap.job_points.as_mut().unwrap().jobs[usize::from(WAR)].current_jp = 0;
        sync_merit_windows(&mut dynamic, Some(&stack), &snap, &dat);
        assert_eq!(
            state(&dynamic).cell(WAR_TYPE_A).unwrap().state,
            UpgradeRowState::TooCostly
        );
    }

    #[test]
    fn readouts_draw_at_their_offsets() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        let mut snap = snapshot();
        let cx = MeritContext {
            snap: &snap,
            dat: &dat,
            windows: &dynamic.merit,
        };
        let levmerit = levmerit_texts(&cx);
        let right = 150.0 - layout::READOUT_RIGHT_INSET;
        assert_eq!(
            (levmerit[0].text.as_str(), levmerit[0].x, levmerit[0].y),
            ("1234/10000", right, layout::READOUT_FIRST_TOP)
        );
        assert_eq!(levmerit[1].text, "10/30 P");
        assert_eq!(levmerit[1].color, ARGB_NEUTRAL);
        snap.merits = None;
        let cx = MeritContext {
            snap: &snap,
            dat: &dat,
            windows: &dynamic.merit,
        };
        let levmerit = levmerit_texts(&cx);
        assert_eq!(levmerit.len(), 1);
        assert_eq!(
            levmerit[0].text,
            format!("{NO_RECORD_LIMIT_POINTS}/{LIMIT_POINT_MAX}")
        );

        let snap = snapshot();
        let stack = open_categories(&mut dynamic, &snap, &dat);
        sync_merit_windows(&mut dynamic, Some(&stack), &snap, &dat);
        let cx = MeritContext {
            snap: &snap,
            dat: &dat,
            windows: &dynamic.merit,
        };
        let rows = dynamic.merit.categories.as_ref().unwrap().rows.clone();
        let info = meritinf_texts(&cx, None, 0, &rows);
        assert_eq!(info[0].text, "10 P");
        let total = &info[1];
        assert_eq!(
            total.full_text(),
            "9/15",
            "HP/MP: 1 + 8 stored upgrades of 15"
        );
        assert_eq!(total.color, ARGB_NEUTRAL);
        assert_eq!(total.suffix, Some(("/15".to_string(), ARGB_NEUTRAL)));
        let war = meritinf_texts(&cx, None, 1, &rows);
        assert_eq!(war.len(), 3, "a job category shows its second section too");
        assert_eq!(war[2].suffix.as_ref().map(|s| s.1), Some(SECTION_COLORS[1]));
        assert_eq!(war[2].x, 200.0 - layout::MERITINF_SECOND_RIGHT_INSET);
        let at_cap = total_text(15, 15, 0.0, 0.0, ARGB_NEUTRAL);
        assert_eq!(at_cap.color, ARGB_CAPPED);
    }

    /// `meritcat`'s cells sit at the record's columns, the cursor row's name in the cursor color
    /// and a blank cost cell while next is 0.
    #[test]
    fn meritcat_cells_sit_at_the_record_columns() {
        let dat = dat();
        let mut dynamic = DynamicMenu::default();
        let snap = snapshot();
        let mut stack = open_categories(&mut dynamic, &snap, &dat);
        select(&mut stack, &mut dynamic, &snap, &dat);
        sync_merit_windows(&mut dynamic, Some(&stack), &snap, &dat);
        let level = stack.current().unwrap().clone();
        let rows = dynamic.merit.categories.as_ref().unwrap().rows.clone();
        let cx = MeritContext {
            snap: &snap,
            dat: &dat,
            windows: &dynamic.merit,
        };
        let texts = meritcat_texts(&level, true, &rows, &cx);
        let find = |text: &str| texts.iter().find(|t| t.text == text).unwrap().clone();
        let hp = find(&format!("m{MAX_HP}"));
        assert_eq!(
            (hp.x, hp.y, hp.color),
            (
                layout::NAME_X,
                layout::LIST_FIRST_ROW_TOP + layout::NAME_DROP,
                ARGB_FP_CURSOR
            )
        );
        let cost = find("3 P");
        assert_eq!(
            (cost.x, cost.align, cost.y),
            (
                layout::COST_RIGHT,
                kit::TextAlign::Right,
                layout::LIST_FIRST_ROW_TOP + layout::NUMBER_DROP
            )
        );
        let caption = find("cost");
        assert_eq!(caption.x, layout::COST_CAPTION_X);
        let mp_row_y = layout::LIST_FIRST_ROW_TOP + layout::LIST_ROW_PITCH + layout::NUMBER_DROP;
        assert!(
            texts
                .iter()
                .filter(|t| t.y == mp_row_y)
                .all(|t| t.text != "cost"),
            "MP's next is 0: no cost cell"
        );
        let mp_count = texts
            .iter()
            .find(|t| t.y == mp_row_y && t.x == layout::COUNT_RIGHT)
            .unwrap();
        assert_eq!((mp_count.text.as_str(), mp_count.color), ("8", ARGB_CAPPED));
    }

    #[test]
    fn every_kind_names_its_record() {
        use crate::hud::menu::{is_dynamic, record_window_name};
        for (window, name) in [
            (MeritWindow::Menu, MERIT1_WINDOW),
            (MeritWindow::Mode, MERIT2_WINDOW),
            (MeritWindow::Categories { entered: None }, MERITCAT_WINDOW),
            (MeritWindow::RaiseLower { index: 0 }, MERIT3_WINDOW),
            (
                MeritWindow::SortConfirm {
                    index: 0,
                    raise: true,
                },
                SORTYN_WINDOW,
            ),
            (
                MeritWindow::Confirm {
                    index: 0,
                    raise: true,
                },
                MERITYN_WINDOW,
            ),
            (MeritWindow::Jobs, MERIT2CA_WINDOW),
            (
                MeritWindow::JobCategories {
                    job: WAR,
                    gifts: false,
                },
                JBPCAT_WINDOW,
            ),
            (
                MeritWindow::JobPointConfirm {
                    job: WAR,
                    type_id: 0,
                },
                MERITYN_WINDOW,
            ),
        ] {
            assert_eq!(record_window_name(MenuKind::Merit(window)), Some(name));
            assert!(is_dynamic(MenuKind::Merit(window)));
            assert!(MERIT_WINDOWS.contains(&name));
        }
    }

    #[test]
    fn missing_refusal_text_prints_its_id() {
        let windows = MeritWindows::default();
        assert_eq!(
            not_in_mog_house_line(&windows),
            kit::client_message_fallback(GETTEXT_TABLE_CLIENT_MESSAGES, NOT_ALLOWED_MESSAGE_ID)
        );
    }
}
