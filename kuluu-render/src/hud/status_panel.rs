use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};

use crate::hud::overlay::ActiveOverlay;
use crate::hud::style::{self, theme};
use crate::snapshot::SceneState;

#[derive(Debug, Clone, Copy)]
pub struct StatusEntry {
    pub kind: StatusEntryKind,
    pub label: &'static str,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusEntryKind {
    Profile,
    JobLevels,
    MasterLevels,
    CombatSkill,
    MagicSkill,
    CraftSkill,
    Currencies,
    Currencies2,
    Unity,
    PlayTime,
    MeritPoints,
    JobPoints,
    /// statcom2 id 13 (`fp_cat` — the record's caption reads "Displays
    /// Categories for which alternate...", vault main_menu.md §59): the Facet
    /// Points category browser. Kuluu has no facet store yet, so its detail
    /// window shows the grounded empty readout.
    FacetCategories,
}

/// statcom2's live rows in DISPLAY order (block y), paired with each row's
/// handler-visible item id: the selector dispatches on these ids (FFXiMain.dll
/// retail-2026-09 select handler RVA 0x148D00 over detail table RVA 0x3716B8)
/// while rows draw where their block y says — so Master Levels (id 12) sits
/// third and Play Time (id 9) tenth, the record read in `ffxi-dat`
/// `menu_windows` pins (vault main_menu.md §59). STATUS_ENTRIES follows this
/// order row for row.
pub const STATCOM2_DISPLAY_ITEM_IDS: [i16; 13] = [1, 2, 12, 3, 4, 5, 6, 10, 11, 9, 7, 8, 13];

/// The focus law (vault main_menu.md §59): display-only windows never receive keys,
/// so while one is open the selector underneath keeps navigating. These are
/// exactly retail's display-only statcom2 companions — profile, joblevel,
/// btlskill, mgcskill, trdskill, masterle (detail table entries behind
/// FFXiMain.dll retail-2026-09 RVA 0x148D00; the close-details helper at
/// RVA 0x148AC0 only ever closes these six).
pub fn status_detail_display_only(kind: StatusEntryKind) -> bool {
    matches!(
        kind,
        StatusEntryKind::Profile
            | StatusEntryKind::JobLevels
            | StatusEntryKind::MasterLevels
            | StatusEntryKind::CombatSkill
            | StatusEntryKind::MagicSkill
            | StatusEntryKind::CraftSkill
    )
}

pub const STATUS_ENTRIES: &[StatusEntry] = &[
    StatusEntry {
        kind: StatusEntryKind::Profile,
        label: "Profile",
        enabled: true,
    },
    StatusEntry {
        kind: StatusEntryKind::JobLevels,
        label: "Job Levels",
        enabled: true,
    },
    StatusEntry {
        kind: StatusEntryKind::MasterLevels,
        label: "Master Levels",
        enabled: false,
    },
    StatusEntry {
        kind: StatusEntryKind::CombatSkill,
        label: "Combat Skill",
        enabled: true,
    },
    StatusEntry {
        kind: StatusEntryKind::MagicSkill,
        label: "Magic Skill",
        enabled: true,
    },
    StatusEntry {
        kind: StatusEntryKind::CraftSkill,
        label: "Craft Skill",
        enabled: true,
    },
    StatusEntry {
        kind: StatusEntryKind::Currencies,
        label: "Currencies",
        enabled: true,
    },
    StatusEntry {
        kind: StatusEntryKind::Currencies2,
        label: "Currencies 2",
        enabled: true,
    },
    StatusEntry {
        kind: StatusEntryKind::Unity,
        label: "Unity",
        // Owner ruling 2026-10-22: Unity stays grey and refused - it is never played with here.
        enabled: false,
    },
    StatusEntry {
        kind: StatusEntryKind::PlayTime,
        label: "Play Time",
        enabled: true,
    },
    StatusEntry {
        kind: StatusEntryKind::MeritPoints,
        label: "Merit Points",
        enabled: false,
    },
    StatusEntry {
        kind: StatusEntryKind::JobPoints,
        label: "Job Points",
        enabled: true,
    },
    StatusEntry {
        kind: StatusEntryKind::FacetCategories,
        // The record's description (catalog table 11 id 1096) reads "Displays
        // Categories for which alter ego points may be spent to increase
        // attributes" — the label follows that wording (vault main_menu.md §59).
        label: "Alter Ego Points",
        enabled: true,
    },
];

pub fn status_entry_count() -> usize {
    STATUS_ENTRIES.len()
}

/// Main-job level 99 — the second half of retail's gate on Master Levels, Job
/// Points and Alter Ego Points (`FFXiMain.dll retail-2026-09 `.text 0x148B20` /
/// `.text 0x148EB1`, both `cmp al,0x63`).
pub const MASTER_LEVEL_JOB_CAP: u8 = 99;

/// Bit 0 of the s2c 0x063 JobPoints record's `access` byte opens the Job Points row.
pub const JOB_POINTS_ACCESS_BIT: u8 = 1;

/// Key item that unlocks the Alter Ego Points row (id 13): retail tests it in the
/// same branch as the level-99 check (`FFXiMain.dll retail-2026-09 RVA 0x148CD2`,
/// `push 0xD21` feeding the key-item test helper `.text 0x97E80`).
pub const ALTER_EGO_POINTS_KEY_ITEM: u16 = 3361;

/// The live enable law of one Status row. Retail opens ids 3/4/5/12/13 disabled
/// (`FFXiMain.dll retail-2026-09 `.text 0x148B50`, five `call 0x118B60(win,id,5)`
/// disables) and re-enables them only when their content exists: each skill family
/// needs a visible word in s2c 0x062 (which is also why those three stay grey before
/// the packet — Shane's A1/A2 reading, corroborated by this cycle's re-read of the
/// disable/enable sites), and Master Levels needs the mastery flag plus a level-99
/// main job. A gated row draws grey, its select plays the error blip, and the detail
/// handler is never called.
pub fn status_row_enabled(kind: StatusEntryKind, snap: &kuluu_snapshot::SceneSnapshot) -> bool {
    use ffxi_proto::decode::CharSkillLevels;
    match kind {
        // Owner ruling 2026-10-22 (ur/147): Unity is greyed out and its select refused,
        // whatever the snapshot says; the unity windows are not used in kuluu.
        StatusEntryKind::Unity => false,
        StatusEntryKind::CombatSkill => visible_in_range(
            snap,
            CharSkillLevels::COMBAT_SKILL_RANGE,
            CharSkillLevels::SKILL_HIDDEN,
        ),
        StatusEntryKind::MagicSkill => visible_in_range(
            snap,
            CharSkillLevels::MAGIC_SKILL_RANGE,
            CharSkillLevels::SKILL_HIDDEN,
        ),
        StatusEntryKind::CraftSkill => visible_in_range(
            snap,
            CharSkillLevels::CRAFT_SKILL_RANGE,
            CharSkillLevels::CRAFT_HIDDEN,
        ),
        StatusEntryKind::MasterLevels => {
            main_job_level(snap) >= MASTER_LEVEL_JOB_CAP
                && snap.stats.is_some_and(|s| s.master_level_unlocked)
        }
        // Merit Points (id 7): enabled exactly when a s2c 0x063 Merits record has been
        // received AND its can-use-merit-mode bit is set — retail's gate body (.text
        // FFXiMain.dll retail-2026-09 `0x148E88`) tests the received flag at .data 0x485824
        // bit 1 (raised only by the dispatcher case arm `.text 0x99290`) and then byte
        // 0x485829 mask 0x20 = bit 13 of the stored flag word (vault main_menu.md §59
        // addendum 36; bead kuluu-o9qt7.43).
        StatusEntryKind::MeritPoints => snap.merits.is_some_and(|m| m.can_use_merit_mode),
        // Job Points (id 8): an s2c 0x063 JobPoints record with bit 0 of `access` set and a
        // main job (s2c 0x061 `mjob_lv`) at level 99
        // (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).
        StatusEntryKind::JobPoints => {
            snap.job_points
                .is_some_and(|record| record.access & JOB_POINTS_ACCESS_BIT != 0)
                && snap.stats.map_or(0, |stats| stats.mjob_lv) >= MASTER_LEVEL_JOB_CAP
        }
        // Alter Ego Points (id 13): the Status data is ready (an s2c 0x119 arrived), level 99
        // and key item [`ALTER_EGO_POINTS_KEY_ITEM`]
        // (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
        StatusEntryKind::FacetCategories => {
            snap.status_data_ready
                && main_job_level(snap) >= MASTER_LEVEL_JOB_CAP
                && snap.key_items.contains(&ALTER_EGO_POINTS_KEY_ITEM)
        }
        _ => true,
    }
}

/// Any word in `range` that is not the family's hidden sentinel.
fn visible_in_range(
    snap: &kuluu_snapshot::SceneSnapshot,
    range: std::ops::RangeInclusive<usize>,
    hidden_word: u16,
) -> bool {
    let Some(block) = snap.char_skills.as_ref() else {
        return false;
    };
    range
        .into_iter()
        .any(|id| block.skills.get(id).copied() != Some(hidden_word))
}

/// The character's main-job level as the server last reported it, 0 when unknown.
fn main_job_level(snap: &kuluu_snapshot::SceneSnapshot) -> u8 {
    crate::snapshot::resolve_self(&snap.party, snap.self_char_id)
        .map(|m| m.main_job_lv)
        .unwrap_or(0)
}

/// Width of kuluu's own persona panel when the install carries no `persona` record —
/// also the column width every Status detail window falls back to (owner ruling
/// 2026-10-18) and the stacked `level` box mirrors until its record places it.
pub const PANEL_WIDTH_PX: f32 = 280.0;
const PERSONA_PAD_X_PX: f32 = 10.0;
const PERSONA_PAD_Y_PX: f32 = 8.0;
/// Air around the caption glyphs inside their mask box — the box (owner ruling
/// 2026-10-19: "the 'status' of the child menu has the line through it") is what
/// hides the frame edge behind the word.
const PERSONA_CAPTION_PAD_PX: f32 = 3.0;
const PERSONA_BORDER_PX: f32 = 1.0;
const PERSONA_ROW_GAP_PX: f32 = 2.0;
const PERSONA_NAME_FONT_PX: f32 = 14.0;
const PERSONA_ROW_FONT_PX: f32 = 13.0;
/// Persona's column widths as percentages of the row (label · right-set value · slash
/// slot · max/Su); the bonus column takes what is left after them.
const PERSONA_LABEL_COL_PCT: f32 = 28.0;
const PERSONA_VALUE_COL_PCT: f32 = 30.0;
const PERSONA_SLASH_COL_PCT: f32 = 5.0;
const PERSONA_MAX_COL_PCT: f32 = 30.0;
/// The air retail leaves between the persona blocks [name/job/ILv] · [HP/MP/TP] ·
/// attributes — screenshot-measured [O].
pub const PERSONA_BLOCK_GAP_PX: f32 = 7.0;
/// ILv and TP rows are in the set for every snapshot, so the count of block gaps is fixed
/// and the fitted pitch can subtract them from the frame's inner height.
const PERSONA_BLOCK_GAPS: usize = 2;

/// Main menu Status code-moves `persona` to (16, 16)
/// (.agents/skills/retail-observe/references/2026-10-15-menu-window-stacking.md).
pub const PERSONA_STATUS_LEFT: f32 = 16.0;
pub const PERSONA_STATUS_TOP: f32 = 16.0;

/// The persona window's DAT record: its width and height are the record frame's.
pub const PERSONA_WINDOW: &str = "persona";

/// Persona opens with Status, after the menu help bar, so it draws over the bar's title box
/// (.agents/skills/retail-observe/references/2026-10-15-menu-window-stacking.md: newest on top).
pub const PERSONA_Z: i32 = style::WINDOW_Z + 1;

/// Name, main job, sub job, ILv/Su, HP, MP and TP, ahead of the attribute rows.
const PERSONA_HEAD_ROWS: usize = 7;
pub const PERSONA_ROW_COUNT: usize = PERSONA_HEAD_ROWS + PRIMARY_ATTRS.len();

pub const LEVEL_LABEL: &str = "Lv";
/// Retail draws this label verbatim from its image (FFXiMain.dll retail-2026-09 drawn raw
/// at `.text 0x14725C`, string literal `ILv`): capital I.
pub const ITEM_LEVEL_LABEL: &str = "ILv";
pub const SUPERIOR_LEVEL_LABEL: &str = "Su";
pub const HP_LABEL: &str = "HP";
pub const MP_LABEL: &str = "MP";
pub const TP_LABEL: &str = "TP";
/// Shown where the server has not reported a value.
pub const UNKNOWN_VALUE: &str = "---";

/// Status code-moves `joblevel` to (130, 48), off its record frame
/// (.agents/skills/retail-observe/references/2026-10-15-menu-window-stacking.md).
pub const JOBLEVEL_WINDOW: &str = "joblevel";
pub const JOBLEVEL_STATUS_LEFT: f32 = 130.0;
pub const JOBLEVEL_STATUS_TOP: f32 = 48.0;

/// The `level` window opens with Status at its own record frame.
pub const STATUS_LEVEL_WINDOW: &str = "level";

/// The air between persona and the stacked `level` box: DAT delta between persona's bottom
/// (16+190) and `level`'s record top (208).
const STATUS_STACK_GAP_PX: f32 = 2.0;

/// Full-screen host for the persona panel and its `level` box: persona sits at its Status
/// spot, and the box stacks under it until the `level` record places it.
#[derive(Component)]
pub struct StatusLeftStack;

#[derive(Component)]
pub struct StatusPanel;

/// The framed `level` box (retail window `menu    level  `): Level / EXP /
/// NEXTLevel, from the s2c 0x061 pair kuluu folds into
/// [`kuluu_snapshot::CharStats::exp_now`] (vendor/server/src/map/packets/s2c/
/// 0x061_clistatus.h CLISTATUS exp_now/exp_next).
#[derive(Component)]
pub struct StatusLevelBox;

/// One text row of the `level` box. Retail stacks five lines — Level heading, EXP label,
/// the EXP value on its own right-set line, NEXTLevel label and its value — above an
/// experience gauge (`update_status_level_box`, owner retail screenshot law 2026-10-18).
#[derive(Component)]
pub struct StatusLevelLine(pub LevelLine);

/// Which `level` box row a [`StatusLevelLine`] carries, in retail display order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LevelLine {
    Heading,
    ExpLabel,
    ExpValue,
    NextLabel,
    NextValue,
}

/// The gauge's salmon fill; its percent width is the wire's `exp_now`/`exp_next` ratio.
#[derive(Component)]
pub struct StatusExpFill;

// Gauge geometry measured off the owner's retail screenshot of `menu    level  `
// (frame x=16 y=208 w=112 h=88): two blue end caps around a grey track that grows to
// every width between them, with a salmon fill over it from the left.
const LEVEL_GAUGE_CAP_WIDTH_PX: f32 = 3.0;
const LEVEL_GAUGE_CAP_HEIGHT_PX: f32 = 8.0;
const LEVEL_GAUGE_TRACK_HEIGHT_PX: f32 = 6.0;
/// Air under the gauge's bottom edge, owned BY THE GAUGE ROW (margin), not by the box.
/// Owner ruling 2026-10-20 (capture ur/24): "the extra edges of XP bar are still not being
/// counted in height properly" - with zero box padding the caps' last pixel row landed ON the
/// frame line. The clearance is part of what the gauge itself measures, so the auto-height box
/// counts it while keeping no bottom padding of its own (the no-padding law stays on every box).
const LEVEL_GAUGE_BOTTOM_CLEARANCE_PX: f32 = 6.0;
/// How far each value row is pulled up toward its label (owner ruling 2026-10-20, capture
/// ur/26: "the row spacing after EXP and NEXTLevel - so numbers closer to their labels"). A
/// negative top margin shortens only the label→value gap; every other flow gap is untouched.
const LEVEL_VALUE_PULL_UP_PX: f32 = 4.0;
/// The EXP label sits further right than NEXTLevel in the retail box.
pub const LEVEL_EXP_LABEL_INDENT_PX: f32 = 14.0;
/// Retail values end short of the column's right edge, over the gauge's tail.
pub const LEVEL_VALUE_INSET_END_PX: f32 = 16.0;

/// Gauge colors sampled from the owner's retail screenshot [O]: cyan-blue caps, salmon
/// fill for the earned EXP, grey track remainder (measured rgb(16,151,248),
/// rgb(237,123,122) and rgb(118,114,125)).
pub const LEVEL_GAUGE_CAP_COLOR: Color = Color::srgb(0.063, 0.612, 0.973);
pub const LEVEL_EXP_FILL_COLOR: Color = Color::srgb(0.929, 0.482, 0.478);
pub const LEVEL_GAUGE_TRACK_COLOR: Color = Color::srgb(0.451, 0.447, 0.494);

/// Only the wide left-side details — Currencies/Currencies 2 (`evitem` (16,48,w366)) and
/// Facet Categories (`fp_cat`, same spot) — replace the stats column. Job Levels, Master
/// Levels and every skill window sit BESIDE it at `(130,48,w252)` with persona, level box,
/// their "Status" caption AND the statcom2 list all still visible (DAT records + owner
/// screenshots 2026-10-19; supersedes the 2026-10-18 all-details rule).
fn stats_panel_covered(mode: &crate::input_mode::InputMode) -> bool {
    let crate::input_mode::InputMode::Menu(stack) = mode else {
        return false;
    };
    stack.current().is_some_and(|l| match l.kind {
        crate::input_mode::MenuKind::StatusDetail { entry } => {
            STATUS_ENTRIES.get(usize::from(entry)).is_some_and(|e| {
                matches!(
                    e.kind,
                    StatusEntryKind::Currencies
                        | StatusEntryKind::Currencies2
                        | StatusEntryKind::FacetCategories
                )
            })
        }
        _ => false,
    })
}

/// True while the stack holds statcom2 or one of its detail windows — retail
/// opens persona + level WITH the list and cascades them closed together, so
/// kuluu shows both panels for exactly as long as a `Status`/detail frame is open.
pub fn status_flow_open(mode: &crate::input_mode::InputMode) -> bool {
    let crate::input_mode::InputMode::Menu(stack) = mode else {
        return false;
    };
    stack.levels.iter().any(|l| {
        matches!(
            l.kind,
            crate::input_mode::MenuKind::Status | crate::input_mode::MenuKind::StatusDetail { .. }
        )
    })
}

/// The Status level that holds the keys while a display-only detail tops the stack: display-only
/// windows never take keys, so the selector under it keeps them
/// (.agents/skills/retail-observe/references/2026-10-15-menu-window-stacking.md).
pub fn status_selector_under_detail(stack: &crate::input_mode::MenuStack) -> Option<usize> {
    use crate::input_mode::MenuKind;
    let n = stack.levels.len();
    if n < 2 {
        return None;
    }
    let MenuKind::StatusDetail { entry } = stack.levels[n - 1].kind else {
        return None;
    };
    let entry_def = STATUS_ENTRIES.get(usize::from(entry))?;
    if !status_detail_display_only(entry_def.kind) {
        return None;
    }
    (stack.levels[n - 2].kind == MenuKind::Status).then_some(n - 2)
}

/// One persona row wrapper by draw position; `index` runs over [`PERSONA_ROW_COUNT`].
/// The row is a flex line of five [`PersonaCell`] columns.
#[derive(Component)]
pub struct PersonaRow {
    pub index: usize,
}

/// One column of the persona table. Retail draws every row across the same columns —
/// label, value right-aligned into one numeric column that meets the shared `/`, the slash
/// slot, then max/`Su <n>`, and the signed adjustment in its own column after that
/// (owner ruling 2026-10-18: "HP, MP and ILv on the /", "the STR to CHR values are
/// right-aligned in one column, and the green +n gets its own column after that"). An
/// empty cell renders nothing.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonaCol {
    Label,
    /// Right-aligned before the slash: HP/MP current, ILv, TP, attribute base.
    V1,
    Slash,
    /// After the slash: HP/MP max, `Su <n>`.
    V2,
    Bonus,
}

/// Draw order across the row. The bonus sits directly after the reserved slash slot —
/// retail writes `+6` right behind the attribute, and every column keeps a fixed edge so
/// a row WITH a bonus cannot nudge any number (owner ruling 2026-10-18).
pub const PERSONA_COLS: [PersonaCol; 5] = [
    PersonaCol::Label,
    PersonaCol::V1,
    PersonaCol::Slash,
    PersonaCol::Bonus,
    PersonaCol::V2,
];

/// One persona table cell: row `index` at column [`PersonaCol`].
#[derive(Component, Debug)]
pub struct PersonaCell {
    pub index: usize,
    pub col: PersonaCol,
}

/// The panel's title, centered on the window's top border (owner ruling 2026-10-18:
/// "Retail centers it on the persona window's top border").
#[derive(Component)]
pub struct StatusCaption;

/// The caption's only mask: one border-row of panel fill behind the glyphs — sized in
/// `update_persona_frame`. Replaces the word-level background box the owner rejected
/// 2026-10-19 x3 ("the small status text now has a box around it... wtf"): that rectangle
/// stuck out beyond the frame and read as a tab. The strip covers just the frame line.
#[derive(Component)]
pub struct StatusCaptionMask;

pub const STATUS_CAPTION_TEXT: &str = "Status";

/// The `level` box gauge track: a grey bar growing to every width between its caps, under
/// the fill (owner ruling 2026-10-18: "a full-width grey track with a red fill and blue
/// end caps").
#[derive(Component)]
pub struct StatusExpTrack;

pub const PRIMARY_ATTRS: &[&str] = &["STR", "DEX", "VIT", "AGI", "INT", "MND", "CHR"];

/// One persona row in retail's column table. Retail aligns numbers in columns — the
/// HP/MP/ILv values meet at a shared `/` column (owner ruling 2026-10-18: "retail lines
/// up HP, MP and ILv on the /"), attribute bases right-align into one column, and the green
/// adjustment sits in its own column after it. Cells render as fixed-percentage columns; an
/// empty cell renders nothing. The bonus is the CLISTATUS `bp_adj` word;
/// vendor/server/src/map/packets/s2c/0x061_clistatus.h.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonaLine {
    pub kind: PersonaRowKind,
    /// Full-text rows (name, job) fill this and leave the value cells empty.
    pub label: String,
    /// Right-aligned before the slash column (HP/MP current, ILv).
    pub v1: String,
    /// The shared `/` glyph — only on lines that carry a ratio.
    pub slash: bool,
    /// Left cell after the slash (max value; for ILv it carries `Su <n>` as retail draws it).
    pub v2: String,
    pub bonus: Option<i16>,
}

/// Which block of the persona table a row belongs to — drives both column layout and the
/// grouped spacing retail shows between blocks (name/job/ILv · HP/MP/TP · attributes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonaRowKind {
    Name,
    Job,
    Ilv,
    Hp,
    Mp,
    Tp,
    Attr(usize),
}

impl PersonaLine {
    fn plain(kind: PersonaRowKind, label: String) -> Self {
        Self {
            kind,
            label,
            v1: String::new(),
            slash: false,
            v2: String::new(),
            bonus: None,
        }
    }

    fn pair(kind: PersonaRowKind, label: String, cur: String, max: String) -> Self {
        Self {
            kind,
            label,
            v1: cur,
            slash: true,
            v2: max,
            bonus: None,
        }
    }

    /// The row joined into one string, for tests and diagnostics.
    pub fn display_text(&self) -> String {
        let mut out = self.label.clone();
        if !self.v1.is_empty() || self.slash {
            out.push(' ');
            out.push_str(&self.v1);
        }
        if self.slash {
            out.push('/');
            out.push_str(&self.v2);
        }
        out
    }
}

/// Retail appends a positive adjustment as " +%d" and a negative one as " %d", the minus
/// coming from the value (FFXiMain.dll retail-2026-09 branch at `.text 0x14751B`, format
/// literals `.rdata 0x3715C0` and `.rdata 0x36C00C`); zero adds no span at all.
pub fn persona_bonus_text(bonus: i16) -> String {
    format!(" {bonus:+}")
}

/// Positive adjustments draw green, negative red (colour pushes at FFXiMain.dll
/// retail-2026-09 `.text 0x147530` and `.text 0x14754C`).
pub fn persona_bonus_color(bonus: i16) -> Color {
    if bonus > 0 {
        theme::ATTR_ADJUST_UP
    } else {
        theme::ATTR_ADJUST_DOWN
    }
}

/// How persona fills its rectangle: the record frame's width and height with every row sized to
/// fit inside it, or kuluu's own panel without an install.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PersonaLayout {
    pub width: f32,
    pub height: Option<f32>,
    pub row_gap: f32,
    pub name_font_px: f32,
    pub row_font_px: f32,
    /// `None` keeps the font's own line height.
    pub line_pitch: Option<f32>,
}

impl PersonaLayout {
    const FALLBACK: Self = Self {
        width: PANEL_WIDTH_PX,
        height: None,
        row_gap: PERSONA_ROW_GAP_PX,
        name_font_px: PERSONA_NAME_FONT_PX,
        row_font_px: PERSONA_ROW_FONT_PX,
        line_pitch: None,
    };

    pub fn font_px(&self, index: usize) -> f32 {
        if index == 0 {
            self.name_font_px
        } else {
            self.row_font_px
        }
    }
}

/// `widest_chars` is the longest row in characters, so the fitted font also keeps it inside the
/// frame's width.
pub fn persona_layout(
    frame: Option<&crate::hud::menu::RecordFrame>,
    widest_chars: usize,
) -> PersonaLayout {
    let Some(frame) = frame else {
        return PersonaLayout::FALLBACK;
    };
    let Some(height) = frame.height else {
        return PersonaLayout {
            width: frame.width,
            ..PersonaLayout::FALLBACK
        };
    };
    // The block gaps spend frame height too, or the fitted pitch plus margins overflows it.
    let inner_height = (height
        - 2.0 * (PERSONA_PAD_Y_PX + PERSONA_BORDER_PX)
        - PERSONA_BLOCK_GAPS as f32 * PERSONA_BLOCK_GAP_PX)
        .max(0.0);
    let inner_width = (frame.width - 2.0 * (PERSONA_PAD_X_PX + PERSONA_BORDER_PX)).max(0.0);
    let pitch = inner_height / PERSONA_ROW_COUNT as f32;
    let height_fit = pitch / crate::ui_font::line_height_px(1.0);
    let char_width = crate::ui_font::text_width_px(UNKNOWN_VALUE, 1.0) / UNKNOWN_VALUE.len() as f32;
    let width_fit = inner_width / (char_width * widest_chars.max(1) as f32);
    let font = height_fit.min(width_fit);
    PersonaLayout {
        width: frame.width,
        height: Some(height),
        row_gap: 0.0,
        name_font_px: font,
        row_font_px: font,
        line_pitch: Some(pitch),
    }
}

/// Persona's rows: name, `Lv <n> <Main Job>`, `Lv <n> <Sub Job>` only with a sub job,
/// `ILv <x>/ Su <y>`, HP and MP as current/max, TP, then each attribute's base with its adjustment.
pub fn persona_lines(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<PersonaLine> {
    let me = crate::snapshot::resolve_self(&snap.party, snap.self_char_id);
    let stats = snap.stats.as_ref();
    let mut lines = vec![PersonaLine::plain(
        PersonaRowKind::Name,
        me.and_then(|m| m.name.clone())
            .or_else(|| snap.char_name.clone())
            .unwrap_or_else(|| UNKNOWN_VALUE.to_string()),
    )];
    match me {
        Some(m) => {
            lines.push(PersonaLine::plain(
                PersonaRowKind::Job,
                job_line(m.main_job, m.main_job_lv),
            ));
            if m.sub_job != 0 {
                lines.push(PersonaLine::plain(
                    PersonaRowKind::Job,
                    job_line(m.sub_job, m.sub_job_lv),
                ));
            }
        }
        None => lines.push(PersonaLine::plain(
            PersonaRowKind::Job,
            format!("{LEVEL_LABEL} {UNKNOWN_VALUE}"),
        )),
    }
    let item_level = stats
        .filter(|s| s.item_level > 0)
        .map_or_else(|| UNKNOWN_VALUE.to_string(), |s| s.item_level.to_string());
    let superior = stats.map_or_else(|| UNKNOWN_VALUE.to_string(), |s| s.su_lv.to_string());
    // Retail draws the ILv ratio on the same slash column as HP/MP, with `Su <n>` after it.
    lines.push(PersonaLine::pair(
        PersonaRowKind::Ilv,
        ITEM_LEVEL_LABEL.to_string(),
        item_level.clone(),
        format!(" {SUPERIOR_LEVEL_LABEL} {superior}"),
    ));
    let current =
        |value: Option<u32>| value.map_or_else(|| UNKNOWN_VALUE.to_string(), |v| v.to_string());
    lines.push(PersonaLine::pair(
        PersonaRowKind::Hp,
        HP_LABEL.to_string(),
        current(me.map(|m| m.hp)),
        current(stats.map(|s| s.hp_max)),
    ));
    lines.push(PersonaLine::pair(
        PersonaRowKind::Mp,
        MP_LABEL.to_string(),
        current(me.map(|m| m.mp)),
        current(stats.map(|s| s.mp_max)),
    ));
    // Retail stacks TP's single number in the same right-set column as every other value.
    let mut tp = PersonaLine::plain(PersonaRowKind::Tp, TP_LABEL.to_string());
    tp.v1 = current(me.map(|m| m.tp));
    lines.push(tp);
    for (attr_index, name) in PRIMARY_ATTRS.iter().enumerate() {
        let line = match stats {
            Some(s) => {
                let base = attr_value(s, attr_index).unwrap_or(0);
                let bonus = s.bonus.get(attr_index).copied().unwrap_or(0);
                let mut l = PersonaLine::plain(PersonaRowKind::Attr(attr_index), name.to_string());
                // Retail right-aligns the attribute base into the shared value column.
                l.v1 = base.to_string();
                l.bonus = (bonus != 0).then_some(bonus);
                l
            }
            None => {
                let mut l = PersonaLine::plain(PersonaRowKind::Attr(attr_index), name.to_string());
                l.v1 = UNKNOWN_VALUE.to_string();
                l
            }
        };
        lines.push(line);
    }
    lines
}

fn job_line(job: u8, level: u8) -> String {
    let name = ffxi_vocab::job_names::lookup(u16::from(job)).unwrap_or(UNKNOWN_VALUE);
    format!("{LEVEL_LABEL} {level} {name}")
}

/// The persona panel. Its caption carries `KeepOwnFont` (it wants the oblique cut of the
/// HUD family, which `update_persona_frame` assigns) so `apply_ui_font`, which stamps the
/// regular face onto every new `TextFont`, passes it by.
pub fn spawn_status_panel(mut commands: Commands) {
    let stack = commands
        .spawn((
            crate::components::InGameEntity,
            StatusLeftStack,
            Node {
                position_type: PositionType::Absolute,
                top: Val::ZERO,
                left: Val::ZERO,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                padding: UiRect {
                    left: Val::Px(PERSONA_STATUS_LEFT),
                    top: Val::Px(PERSONA_STATUS_TOP),
                    ..default()
                },
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(STATUS_STACK_GAP_PX),
                display: Display::None,
                ..default()
            },
            GlobalZIndex(PERSONA_Z),
            bevy::picking::Pickable::IGNORE,
        ))
        .id();
    let panel = commands
        .spawn((
            StatusPanel,
            Node {
                width: Val::Px(PANEL_WIDTH_PX),
                padding: UiRect::axes(Val::Px(PERSONA_PAD_X_PX), Val::Px(PERSONA_PAD_Y_PX)),
                border: UiRect::all(Val::Px(PERSONA_BORDER_PX)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(PERSONA_ROW_GAP_PX),
                flex_shrink: 0.0,
                ..default()
            },
            // The standard window chrome (owner x4, 2026-10-19: "why did you randomly
            // change the background alpha?"). Occlusion of what sits behind comes from
            // draw order and coding stray rows hidden, not from a different fill.
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
        ))
        .id();
    commands.entity(stack).add_child(panel);
    commands.entity(stack).with_children(|s| {
        s.spawn((
            StatusLevelBox,
            Node {
                width: Val::Px(PANEL_WIDTH_PX),
                // Owner rulings 2026-10-20: no padding at the bottom AND none at the top of
                // these boxes ("remove the top padding on all - we dont want wasted space up
                // top"); only side air remains. The gauge still clears the frame - that clearance
                // belongs to the gauge row's own margin (`LEVEL_GAUGE_BOTTOM_CLEARANCE_PX`),
                // never to the box.
                padding: UiRect {
                    left: Val::Px(10.0),
                    right: Val::Px(10.0),
                    top: Val::ZERO,
                    bottom: Val::ZERO,
                },
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(1.0),
                ..default()
            },
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
        ))
        .with_children(|b| {
            // One wrapper row per line with the text as its CHILD — Bevy only applies
            // a node's justify/flex settings to child entities, so value rows need this
            // shape to set them right.
            spawn_level_line(b, LevelLine::Heading, "Level —", Node::default());
            spawn_level_line(
                b,
                LevelLine::ExpLabel,
                "",
                Node {
                    margin: UiRect::left(Val::Px(LEVEL_EXP_LABEL_INDENT_PX)),
                    ..default()
                },
            );
            spawn_level_line(b, LevelLine::ExpValue, "", value_row_node());
            spawn_level_line(b, LevelLine::NextLabel, "", Node::default());
            spawn_level_line(b, LevelLine::NextValue, "", value_row_node());
            // The experience gauge spans the box: blue caps at both ends of a grey
            // track that grows between them, salmon fill over it from the left
            // (owner ruling 2026-10-18; colors measured off the retail box [O]).
            b.spawn((
                LevelGaugeRow,
                Node {
                    width: Val::Percent(100.0),
                    align_items: AlignItems::Center,
                    // The gauge row carries the air under its own bottom edge so the caps' extra
                    // edges count into the box's measured height instead of sitting on the border
                    // (owner ruling 2026-10-20; capture ur/24).
                    margin: UiRect::bottom(Val::Px(LEVEL_GAUGE_BOTTOM_CLEARANCE_PX)),
                    ..default()
                },
            ))
            .with_children(|g| {
                g.spawn((
                    Node {
                        width: Val::Px(LEVEL_GAUGE_CAP_WIDTH_PX),
                        height: Val::Px(LEVEL_GAUGE_CAP_HEIGHT_PX),
                        ..default()
                    },
                    BackgroundColor(LEVEL_GAUGE_CAP_COLOR),
                ));
                g.spawn((
                    StatusExpTrack,
                    Node {
                        flex_grow: 1.0,
                        height: Val::Px(LEVEL_GAUGE_TRACK_HEIGHT_PX),
                        ..default()
                    },
                    BackgroundColor(LEVEL_GAUGE_TRACK_COLOR),
                ))
                .with_child((
                    StatusExpFill,
                    Node {
                        width: Val::Percent(0.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    BackgroundColor(LEVEL_EXP_FILL_COLOR),
                ));
                g.spawn((
                    Node {
                        width: Val::Px(LEVEL_GAUGE_CAP_WIDTH_PX),
                        height: Val::Px(LEVEL_GAUGE_CAP_HEIGHT_PX),
                        ..default()
                    },
                    BackgroundColor(LEVEL_GAUGE_CAP_COLOR),
                ));
            });
        });
    });
    commands.entity(panel).with_children(|p| {
        let layout = PersonaLayout::FALLBACK;
        for index in 0..PERSONA_ROW_COUNT {
            let row_color = if index == 0 {
                theme::TITLE
            } else {
                theme::TEXT
            };
            p.spawn((
                PersonaRow { index },
                Node {
                    width: Val::Percent(100.0),
                    // Rows keep their fitted pitch even when a row's content overflows.
                    flex_shrink: 0.0,
                    ..default()
                },
            ))
            .with_children(|r| {
                for col in PERSONA_COLS {
                    let cell = (
                        PersonaCell { index, col },
                        Text::new(""),
                        TextLayout {
                            linebreak: LineBreak::NoWrap,
                            ..default()
                        },
                        style::text_font(layout.font_px(index)),
                        bevy::text::LineHeight::default(),
                        TextColor(row_color),
                    );
                    match col {
                        // A node never right-aligns its own text — the value column keeps a
                        // FlexEnd wrapper, the same trick the `level` box rows use.
                        PersonaCol::V1 => {
                            r.spawn(Node {
                                width: Val::Percent(PERSONA_VALUE_COL_PCT),
                                min_width: Val::Percent(PERSONA_VALUE_COL_PCT),
                                max_width: Val::Percent(PERSONA_VALUE_COL_PCT),
                                justify_content: JustifyContent::FlexEnd,
                                flex_shrink: 0.0,
                                ..default()
                            })
                            // The implicit Node bevy seeds on a Text spawn carries Taffy's
                            // shrink-1 default — the very thing that squeezed an INT `+5`
                            // row (pinned by persona_columns_are_pinned_against_the_bonus_row).
                            .with_child((persona_cell_node(col), cell));
                        }
                        _ => {
                            r.spawn((persona_cell_node(col), cell));
                        }
                    }
                }
            });
        }
        // The title rides the window's top border; `update_persona_frame` measures and
        // places it — centered across, and lifted to straddle the frame edge. The word
        // itself draws with NO box (owner x3, 2026-10-19): only the thin strip behind it
        // masks the frame line.
        p.spawn((
            StatusCaptionMask,
            Node {
                position_type: PositionType::Absolute,
                // Absolute children measure from the padding edge, so the frame band sits
                // one border-width ABOVE it. Sitting at 0 (as this did) covered a content
                // row instead and left the frame line running through the glyphs.
                top: Val::Px(-PERSONA_BORDER_PX),
                height: Val::Px(PERSONA_BORDER_PX),
                ..default()
            },
            // Matches the panel's own fill exactly — same chrome color, one border row.
            BackgroundColor(theme::FRAME_BG),
        ));
        // Placed by `update_persona_frame` from the measured word width. Stretch plus
        // `Justify::Center` was the old attempt at centering and never worked: bevy draws
        // the implicit text child at its own laid-out width, so the box being wide did not
        // move the word (owner 2026-10-19: "its not center on the top line correctly").
        p.spawn((
            StatusCaption,
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            Text::new(STATUS_CAPTION_TEXT),
            TextLayout {
                justify: bevy::text::Justify::Center,
                linebreak: LineBreak::NoWrap,
            },
            // Italic by face, not by slanting the box (owner 2026-10-19): `update_persona_frame`
            // gives it DejaVu Sans Mono Oblique, drawn with the same advances as every other
            // HUD string.
            crate::ui_font::KeepOwnFont,
            style::text_font(PERSONA_ROW_FONT_PX),
            bevy::text::LineHeight::default(),
            TextColor(theme::TITLE),
        ));
    });
}

/// The fixed column grid for a persona cell that is a direct row child; the right-set
/// V1 lives inside its wrapper and takes the wrapper's share. A bonus row must never move
/// another number's column (owner ruling 2026-10-18), so each sized column pins
/// min=max=width alongside shrink 0.
fn persona_cell_node(col: PersonaCol) -> Node {
    let sized = |pct: f32| Node {
        // min=max=width pins the column so no row's content can move it in any direction.
        width: Val::Percent(pct),
        min_width: Val::Percent(pct),
        max_width: Val::Percent(pct),
        flex_shrink: 0.0,
        ..default()
    };
    match col {
        PersonaCol::Label => sized(PERSONA_LABEL_COL_PCT),
        PersonaCol::Slash => sized(PERSONA_SLASH_COL_PCT),
        PersonaCol::V2 => sized(PERSONA_MAX_COL_PCT),
        PersonaCol::V1 | PersonaCol::Bonus => Node {
            flex_shrink: 0.0,
            ..default()
        },
    }
}

/// The `level` box body as the wire reports it (values from s2c 0x061 — no wire value,
/// no invented number). Retail draws comma-grouped values on their own lines and a
/// gauge filled by the ratio law: screenshot measured ≈27px of salmon over the ≈66px
/// track at exp_now=1,304 / exp_next=3,400.
#[derive(Debug, PartialEq)]
pub struct LevelBoxBody {
    pub heading: String,
    /// Comma-grouped decimal; `None` until the stats block arrives — then retail's
    /// value lines are empty and the gauge sits at zero.
    pub exp: Option<String>,
    pub next_exp: Option<String>,
    /// Percent width of the salmon fill, clamped to 0..=100.
    pub fill_percent: f32,
}

pub fn level_box_body(snap: &kuluu_snapshot::SceneSnapshot) -> LevelBoxBody {
    let me = crate::snapshot::resolve_self(&snap.party, snap.self_char_id);
    let Some(lv) = me.map(|m| m.main_job_lv) else {
        return LevelBoxBody {
            heading: "Level —".to_string(),
            exp: None,
            next_exp: None,
            fill_percent: 0.0,
        };
    };
    let Some(stats) = snap.stats.as_ref().filter(|s| s.exp_next != 0) else {
        return LevelBoxBody {
            heading: format!("Level {lv}"),
            exp: None,
            next_exp: None,
            fill_percent: 0.0,
        };
    };
    // Non-negative wire values widen without loss; the casts are total after clamping.
    let now = stats.exp_now.max(0) as u32;
    let next = stats.exp_next.max(0) as u32;
    LevelBoxBody {
        heading: format!("Level {lv}"),
        exp: Some(comma_groups(now)),
        next_exp: Some(comma_groups(next)),
        fill_percent: if next == 0 {
            0.0
        } else {
            ((now.min(next) as f64 / next as f64) * 100.0) as f32
        },
    }
}

/// `1,304` — the retail box groups thousands; every digit string here is ASCII.
pub fn comma_groups(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.char_indices() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// The wrapper row of one `level` box line. Marked so the squeeze pass can take it off-box
/// with the gauge during a chrome-only frame; the word itself rides inside as its only child.
#[derive(Component)]
pub(crate) struct LevelRowWrap;

/// The gauge row of the `level` box, marked for the same reason (it is pure background
/// furniture, not text).
#[derive(Component)]
pub(crate) struct LevelGaugeRow;

/// One `level` box line: wrapper row + text child (`StatusLevelLine`).
fn spawn_level_line(
    b: &mut bevy::ecs::relationship::RelatedSpawnerCommands<'_, ChildOf>,
    line: LevelLine,
    text: &'static str,
    node: Node,
) {
    b.spawn((LevelRowWrap, node)).with_child((
        StatusLevelLine(line),
        Text::new(text),
        style::text_font(13.0),
        TextColor(theme::TEXT),
    ));
}

fn value_row_node() -> Node {
    Node {
        width: Val::Percent(100.0),
        justify_content: JustifyContent::FlexEnd,
        padding: UiRect::right(Val::Px(LEVEL_VALUE_INSET_END_PX)),
        // Pull each number up against its label (owner ruling 2026-10-20); the following row
        // keeps its normal gap, so only the pair tightens.
        margin: UiRect::top(Val::Px(-LEVEL_VALUE_PULL_UP_PX)),
        ..default()
    }
}

/// The `level` box NEVER leaves the persona column: it stays stacked in flow under the
/// panel and mirrors that panel's live width — one aligned column, same x, same w, exactly
/// where the Status words sit (owner ruling 2026-10-18; matches the DAT record law of
/// x=16 flush at persona's own width, which is why placing it from raw frames independently
/// only ever misaligns it).
pub fn update_status_level_box(
    panel_q: Query<&Node, (With<StatusPanel>, Without<StatusLevelBox>)>,
    mut level_box_q: Query<&mut Node, (With<StatusLevelBox>, Without<StatusPanel>)>,
) {
    let column_width = panel_q.single().map(|panel| panel.width).ok();
    for mut node in level_box_q.iter_mut() {
        let mut want = node.clone();
        want.position_type = PositionType::Relative;
        want.left = Val::Auto;
        want.top = Val::Auto;
        want.right = Val::Auto;
        want.bottom = Val::Auto;
        want.margin = UiRect::ZERO;
        want.width = column_width.unwrap_or(Val::Px(PANEL_WIDTH_PX));
        node.set_if_neq(want);
    }
}

/// Persona takes its whole rectangle from its record frame while the install supplies one, rows
/// sized to fit inside it; without it the panel keeps kuluu's own width and row sizes.
pub fn update_persona_frame(
    config_rows: Res<crate::hud::menu::ConfigDatRows>,
    state: Res<SceneState>,
    italic_font: Res<crate::ui_font::UiFontItalic>,
    mut panel_q: Query<
        &mut Node,
        (
            With<StatusPanel>,
            Without<StatusCaption>,
            Without<StatusCaptionMask>,
        ),
    >,
    mut cell_q: Query<
        (&PersonaCell, &mut TextFont, &mut bevy::text::LineHeight),
        Without<StatusCaption>,
    >,
    mut caption_q: Query<
        (&mut Node, &mut TextFont, &mut bevy::text::LineHeight),
        (
            With<StatusCaption>,
            Without<PersonaCell>,
            Without<StatusCaptionMask>,
        ),
    >,
    mut mask_q: Query<
        &mut Node,
        (
            With<StatusCaptionMask>,
            Without<StatusCaption>,
            Without<PersonaCell>,
        ),
    >,
) {
    let widest = persona_lines(&state.snapshot)
        .iter()
        .map(|line| {
            line.display_text().len() + line.bonus.map_or(0, |b| persona_bonus_text(b).len())
        })
        .max()
        .unwrap_or(0);
    let layout = persona_layout(config_rows.record_frame(PERSONA_WINDOW).as_ref(), widest);
    // The caption's given height (owner ruling 2026-10-20, capture ur/27: "the tiny 'status' on
    // top of window should have given height to prevent word overlapping"): the word box is this
    // tall and straddles the frame line, so half minus a border hangs into the content area.
    // The panel reserves exactly that band as its top inset - rows start at the word's bottom
    // edge by construction, never under it. It is derived from the caption, not chrome padding.
    let pitch_px = layout
        .line_pitch
        .unwrap_or_else(|| crate::ui_font::line_height_px(layout.row_font_px));
    let caption_reserve_px = pitch_px / 2.0 - PERSONA_BORDER_PX / 2.0;
    for mut node in panel_q.iter_mut() {
        let mut want = node.clone();
        want.width = Val::Px(layout.width);
        // Owner ruling 2026-10-20: no padding at the bottom of these boxes. Height is deliberately
        // NOT computed here (an earlier attempt pinned `border + top pad + gaps + pitch x rows`):
        // Taffy lays the rows out a touch taller than that arithmetic assumes, and once the old
        // slack was gone the overflow showed up as CHR crossing the bottom border (owner capture,
        // red arrows). One source of truth - Taffy measures, so flow children are inside by
        // construction; short characters simply end higher. Width stays from the record.
        want.padding.bottom = Val::ZERO;
        // Owner rulings 2026-10-20: chrome top padding gone (ur/26), but the row zone still
        // reserves the caption's hanging half so "Status" can never overlap row 1 (ur/27).
        want.padding.top = Val::Px(caption_reserve_px);
        want.height = Val::Auto;
        want.row_gap = Val::Px(layout.row_gap);
        node.set_if_neq(want);
    }
    let line_height = layout
        .line_pitch
        .map_or_else(bevy::text::LineHeight::default, bevy::text::LineHeight::Px);
    for (cell, mut font, mut pitch) in cell_q.iter_mut() {
        let size = bevy::text::FontSize::Px(layout.font_px(cell.index));
        if font.font_size != size {
            font.font_size = size;
        }
        pitch.set_if_neq(line_height);
    }
    // Half the line height sits above the frame edge, centered on it like retail's title.
    // Absolute children are inset from the padding box, so half the panel's width for
    // centering spans them is its width minus BOTH borders — not the content width inside
    // the padding. Subtracting `PERSONA_PAD_X_PX` as well (as this did) measured against a
    // narrower band and left the word visibly right of centre, while the strip sat on a
    // content row instead of over the frame line (owner 2026-10-19: "the box line is
    // overwriting the text and our status is not centered").
    let caption_band = layout.width - 2.0 * PERSONA_BORDER_PX;
    let caption_width = crate::ui_font::text_width_px(STATUS_CAPTION_TEXT, layout.row_font_px);
    // The word is set in the family's oblique cut: slanted outlines with upright letters,
    // which is what italic means (owner 2026-10-19, after a rotated text box was tried and
    // rejected). Assigned here rather than at spawn so it survives whatever order a
    // front-end schedules spawners against font loading; untouched until the asset exists.
    let caption_face = if italic_font.0 == Handle::default() {
        None
    } else {
        Some(bevy::text::FontSource::Handle(italic_font.0.clone()))
    };
    for (mut node, mut font, mut pitch) in caption_q.iter_mut() {
        let size = bevy::text::FontSize::Px(layout.row_font_px);
        if font.font_size != size {
            font.font_size = size;
        }
        if let Some(face) = &caption_face {
            if font.font != *face {
                font.font = face.clone();
            }
        }
        pitch.set_if_neq(line_height);
        // The word wears its given height, matching the box layout already measures for it
        // (owner ruling 2026-10-20); `caption_reserve_px` above is drawn from this same number.
        let want_height = Val::Px(pitch_px);
        if node.height != want_height {
            node.height = want_height;
        }
        // Half a border further down than the old half-pitch lift: centred on the frame
        // line itself, so no sliver of it shows above or below the glyphs.
        let want_top = Val::Px(-pitch_px / 2.0 - PERSONA_BORDER_PX / 2.0);
        if node.top != want_top {
            node.top = want_top;
        }
        // The word is centered by placement, from its own measured width: the box gets that
        // exact width and sits at the band's middle (monospace advances make this exact).
        let want_left = Val::Px((caption_band - caption_width) / 2.0);
        if node.left != want_left {
            node.left = want_left;
        }
    }
    // The strip behind the word: panel fill, one border row tall, wide enough for the
    // glyphs plus their air — it breaks the frame line where the caption sits (retail's
    // gap in the top border) and nothing else. No box.
    let mask_width = caption_width + 2.0 * PERSONA_CAPTION_PAD_PX;
    for mut mask in mask_q.iter_mut() {
        let want_left = Val::Px((caption_band - mask_width) / 2.0);
        if mask.left != want_left {
            mask.left = want_left;
        }
        let want_width = Val::Px(mask_width);
        if mask.width != want_width {
            mask.width = want_width;
        }
    }
}

/// The squeeze state of both Status column boxes: one leg per box plus the full-size shape
/// each last stood at while at rest - a hidden host lays out nothing, so a grow-in has no
/// memory without it.
#[derive(Default)]
pub(crate) struct StatusColumnFx {
    panel: crate::hud::menu_anim::WindowFx,
    level: crate::hud::menu_anim::WindowFx,
    seen_panel: Option<crate::hud::menu_anim::FullRect>,
    seen_level: Option<crate::hud::menu_anim::FullRect>,
}

#[allow(clippy::type_complexity)]
pub(crate) fn update_status_panel(
    mode: Res<crate::input_mode::InputMode>,
    state: Res<SceneState>,
    mut stack_q: Query<
        &mut Node,
        (
            With<StatusLeftStack>,
            Without<StatusPanel>,
            Without<StatusLevelBox>,
            Without<PersonaRow>,
            Without<StatusExpFill>,
            Without<StatusCaption>,
            Without<StatusCaptionMask>,
            Without<LevelRowWrap>,
            Without<LevelGaugeRow>,
        ),
    >,
    // Pairwise-disjoint per the cycle-57 B0001 law: every &mut Text/&mut Node query here
    // must exclude every other marker in the system.
    mut level_line_q: Query<(&StatusLevelLine, &mut Text), Without<PersonaCell>>,
    mut fill_q: Query<
        &mut Node,
        (
            With<StatusExpFill>,
            Without<PersonaRow>,
            Without<StatusLevelLine>,
            Without<StatusLeftStack>,
            Without<StatusPanel>,
            Without<StatusLevelBox>,
            Without<StatusCaption>,
            Without<StatusCaptionMask>,
            Without<LevelRowWrap>,
            Without<LevelGaugeRow>,
        ),
    >,
    mut row_q: Query<
        (&PersonaRow, &mut Node),
        (
            Without<StatusLevelLine>,
            Without<StatusExpFill>,
            Without<StatusLeftStack>,
            Without<StatusPanel>,
            Without<StatusLevelBox>,
            Without<StatusCaption>,
            Without<StatusCaptionMask>,
            Without<LevelRowWrap>,
            Without<LevelGaugeRow>,
        ),
    >,
    mut cell_q: Query<(&PersonaCell, &mut Text, &mut TextColor), Without<StatusLevelLine>>,
    // The Status column's own squeeze legs (owner window-fx law 2026-10-22): while one runs
    // these two nodes are the only writers of its box geometry. This system is chained after
    // `update_persona_frame`/`update_status_level_box`, so when the fx goes quiet those
    // passes stand the natural shape again on their own.
    mut panel_q: Query<
        (&mut Node, &ComputedNode, &UiGlobalTransform),
        (
            With<StatusPanel>,
            Without<StatusLeftStack>,
            Without<StatusLevelBox>,
            Without<PersonaRow>,
            Without<StatusExpFill>,
            Without<StatusCaption>,
            Without<StatusCaptionMask>,
            Without<LevelRowWrap>,
            Without<LevelGaugeRow>,
        ),
    >,
    mut level_box_q: Query<
        (&mut Node, &ComputedNode, &UiGlobalTransform),
        (
            With<StatusLevelBox>,
            Without<StatusLeftStack>,
            Without<StatusPanel>,
            Without<PersonaRow>,
            Without<StatusExpFill>,
            Without<StatusCaption>,
            Without<StatusCaptionMask>,
            Without<LevelRowWrap>,
            Without<LevelGaugeRow>,
        ),
    >,
    // The body furniture the chrome-only law takes off: caption, its mask, the level-box
    // wrapper rows and the gauge row.
    mut body_q: Query<
        &mut Node,
        (
            Or<(
                With<StatusCaption>,
                With<StatusCaptionMask>,
                With<LevelRowWrap>,
                With<LevelGaugeRow>,
            )>,
            Without<StatusLeftStack>,
            Without<StatusPanel>,
            Without<StatusLevelBox>,
            Without<PersonaRow>,
            Without<StatusExpFill>,
        ),
    >,
    mut fx: Local<StatusColumnFx>,
) {
    use crate::hud::menu_anim::{FullRect, WindowFx};
    let Ok(mut stack_node) = stack_q.single_mut() else {
        return;
    };
    let open = status_flow_open(&mode);
    // A wide detail replaces the panel: retail keeps neither persona nor its level box
    // visible under one.
    let covered = stats_panel_covered(&mode);
    let want_show = open && !covered;
    if !want_show && stack_node.display == Display::None {
        return;
    }
    let was_shown = stack_node.display != Display::None;
    let idle_at_start = matches!(fx.panel, WindowFx::Idle) && matches!(fx.level, WindowFx::Idle);
    if want_show && !idle_at_start {
        // ESC back onto a column mid-collapse stands it full and plain at once: returning to
        // the same place is instant and never grows through the closing leg's geometry.
        fx.panel = WindowFx::Idle;
        fx.level = WindowFx::Idle;
    }
    if was_shown && idle_at_start {
        // Frames at rest keep the remembered shapes current; a squeeze always animates to or
        // from what the boxes last really stood.
        let panel_rect = panel_q
            .single()
            .ok()
            .and_then(|(_, c, t)| FullRect::from_laid_out_box(c, t));
        if let Some(rect) = panel_rect {
            fx.seen_panel = Some(rect);
        }
        let level_rect = level_box_q
            .single()
            .ok()
            .and_then(|(_, c, t)| FullRect::from_laid_out_box(c, t));
        if let Some(rect) = level_rect {
            fx.seen_level = Some(rect);
        }
    }
    if want_show && !was_shown {
        if let (Some(p), Some(l)) = (fx.seen_panel, fx.seen_level) {
            fx.panel = WindowFx::opening(p);
            fx.level = WindowFx::opening(l);
        }
        stack_node.display = Display::Flex;
    }
    let leaving = !want_show && was_shown;
    if leaving && idle_at_start {
        if let (Some(p), Some(l)) = (fx.seen_panel, fx.seen_level) {
            fx.panel = WindowFx::closing(p);
            fx.level = WindowFx::closing(l);
        }
    }
    // One tick each per frame; `WindowFx` owns the shared period law.
    let pf = fx.panel.tick();
    let lf = fx.level.tick();
    let animating = !matches!(fx.panel, WindowFx::Idle) || !matches!(fx.level, WindowFx::Idle);

    let snap = &state.snapshot;
    let lines = persona_lines(snap);
    for (row, mut node) in row_q.iter_mut() {
        let line = lines.get(row.index);
        // Chrome-only fx frames take the words off too ("text ... removed at the start").
        let want_display = if animating || line.is_none() {
            Display::None
        } else {
            Display::Flex
        };
        if node.display != want_display {
            node.display = want_display;
        }
        // Retail groups the table into [name/job/ILv] · [HP/MP/TP] · attributes; the air
        // lands as a margin under the block's last row, whichever slot that is this snapshot.
        let grouped = matches!(
            line.map(|l| l.kind),
            Some(PersonaRowKind::Ilv) | Some(PersonaRowKind::Tp)
        );
        let want_margin_bottom = if grouped {
            Val::Px(PERSONA_BLOCK_GAP_PX)
        } else {
            Val::ZERO
        };
        if node.margin.bottom != want_margin_bottom {
            node.margin.bottom = want_margin_bottom;
        }
    }
    for (cell, mut text, mut color) in cell_q.iter_mut() {
        let line = lines.get(cell.index);
        let bonus_text;
        let want = match cell.col {
            PersonaCol::Label => line.map_or("", |l| l.label.as_str()),
            PersonaCol::V1 => line.map_or("", |l| l.v1.as_str()),
            PersonaCol::Slash => {
                if line.is_some_and(|l| l.slash) {
                    "/"
                } else {
                    ""
                }
            }
            PersonaCol::V2 => line.map_or("", |l| l.v2.as_str()),
            PersonaCol::Bonus => {
                bonus_text = line
                    .and_then(|l| l.bonus)
                    .map_or_else(String::new, persona_bonus_text);
                bonus_text.as_str()
            }
        };
        if text.as_str() != want {
            **text = want.to_string();
        }
        if cell.col == PersonaCol::Bonus {
            if let Some(value) = line.and_then(|l| l.bonus) {
                color.set_if_neq(TextColor(persona_bonus_color(value)));
            }
        }
    }

    let body = level_box_body(snap);
    for (line, mut text) in level_line_q.iter_mut() {
        // Absent values render as empty lines — retail keeps the box's row rhythm.
        let want = match line.0 {
            LevelLine::Heading => body.heading.as_str(),
            LevelLine::ExpLabel => "EXP",
            LevelLine::NextLabel => "NEXTLevel",
            LevelLine::ExpValue => body.exp.as_deref().unwrap_or(""),
            LevelLine::NextValue => body.next_exp.as_deref().unwrap_or(""),
        };
        if text.as_str() != want {
            **text = want.to_string();
        }
    }
    for mut fill in fill_q.iter_mut() {
        let want = Val::Percent(body.fill_percent);
        if fill.width != want {
            fill.width = want;
        }
    }

    // The center-hold legs. The boxes stay in flow and hold their centers by relative offsets:
    // persona pins its own seat (first child of the host), so each axis keeps half its slack
    // as an offset. The `level` box's flow seat follows the animated height persona asserts
    // above it, so its top offset carries that eaten height too - without it the box would
    // slide up the column while the panel collapsed.
    let extra = match (fx.panel.full(), pf) {
        (Some(fp), Some((_, h1))) => fp.height - h1,
        _ => 0.0,
    };
    if let (Some((w, h)), Some(full)) = (pf, fx.panel.full()) {
        if let Ok((mut node, _, _)) = panel_q.single_mut() {
            node.left = Val::Px((full.width - w) / 2.0);
            node.top = Val::Px((full.height - h) / 2.0);
            node.width = Val::Px(w);
            node.height = Val::Px(h);
        }
    }
    if let (Some((w, h)), Some(full)) = (lf, fx.level.full()) {
        if let Ok((mut node, _, _)) = level_box_q.single_mut() {
            node.left = Val::Px((full.width - w) / 2.0);
            node.top = Val::Px((full.height - h) / 2.0 + extra);
            node.width = Val::Px(w);
            node.height = Val::Px(h);
        }
    }
    if pf.is_none() && lf.is_none() {
        // At rest nothing of ours may linger on the nodes. Persona's placement pass owns
        // every other field and re-asserts them each frame; the `level` box's height has no
        // such owner (`update_status_level_box` pins position and width only), so our last
        // squeeze fraction would otherwise stand there forever with its words overflowing
        // an invisible stub chrome.
        if let Ok((mut node, _, _)) = panel_q.single_mut() {
            if !(node.left == Val::Auto && node.top == Val::Auto && node.height == Val::Auto) {
                node.left = Val::Auto;
                node.top = Val::Auto;
                node.height = Val::Auto;
            }
        }
        if let Ok((mut node, _, _)) = level_box_q.single_mut() {
            if !(node.left == Val::Auto && node.top == Val::Auto && node.height == Val::Auto) {
                node.left = Val::Auto;
                node.top = Val::Auto;
                node.height = Val::Auto;
            }
        }
    }

    // Chrome-only law for the body furniture: caption, mask, level rows and the gauge row go
    // off at a squeeze's first frame and land only when full shape arrives. (Persona rows are
    // taken off in their own pass above; no other system owns these nodes' display.)
    let body = if animating {
        Display::None
    } else {
        Display::Flex
    };
    for mut node in body_q.iter_mut() {
        if node.display != body {
            node.display = body;
        }
    }

    // The host comes down when the legs reach zero (or were never armed at all - the plain
    // instant hide, unchanged behavior for shapes that never stood).
    if !want_show && matches!(fx.panel, WindowFx::Idle) && matches!(fx.level, WindowFx::Idle) {
        stack_node.display = Display::None;
    }
}

/// Where the Profile row sits in [`STATUS_ENTRIES`], for callers that match a detail window by
/// selector row rather than repeating the index.
pub fn profile_entry_index() -> usize {
    STATUS_ENTRIES
        .iter()
        .position(|entry| entry.kind == StatusEntryKind::Profile)
        .unwrap_or(0)
}

/// The first section caption of the Status Profile window. Retail draws both captions as their own
/// header plates across the window; kuluu has no plate surface for them yet, so they read as plain
/// rows in the same list (owner capture 2026-10-19).
pub const PROFILE_SECTION_PROFILE: &str = "Profile";

/// The second section caption of the same window.
pub const PROFILE_SECTION_MONSTROSITY: &str = "Monstrosity";

/// The word printed beside a conquest rank and above its progress bar, whose left edge it marks.
pub const PROFILE_RANK_BAR_LABEL: &str = "Rank Points";

/// What the row carrying the rank starts with, so `menu::update_status_detail_window` can find the
/// row again to hang the gauge under it.
pub const PROFILE_RANK_ROW_PREFIX: &str = "Rank:";

/// What an unread field shows, as everywhere else in this window family.
const UNREAD: &str = "\u{2014}";

/// The playable races by entity-look race id: 1 Hume M, 2 Hume F, 3 Elvaan M, 4 Elvaan F,
/// 5 Tarutaru M, 6 Tarutaru F, 7 Mithra, 8 Galka. That is the numbering the item DAT `races` mask
/// uses (its bits are proven one-per-race against retail records in `item_detail::format_races`),
/// and the same byte a player's own record carries.
fn race_name(race: u8) -> Option<&'static str> {
    Some(match race {
        1 | 2 => "Hume",
        3 | 4 => "Elvaan",
        5 | 6 => "Tarutaru",
        7 => "Mithra",
        8 => "Galka",
        _ => return None,
    })
}

/// Our own race, read from our entity's look. That is where FFXiMain.dll retail-2026-09 reads it as
/// well (`mov bl, byte ptr [entity + 0xEF]` at `.text 0x132099`, inside the equip-legality RVA
/// 0x132050), and `None` means that spawn has not arrived yet.
fn self_race(snap: &kuluu_snapshot::SceneSnapshot) -> Option<u8> {
    snap.self_char_id
        .and_then(|id| snap.entities.iter().find(|e| e.id == id))
        .and_then(|e| match e.look.as_ref()? {
            kuluu_snapshot::EntityLook::Equipped { race, .. } => Some(*race),
            _ => None,
        })
}

/// The Status Profile window's rows (owner capture 2026-10-19): allegiance, then Monstrosity, and
/// nothing else. No name/job line, no HP/MP/TP, no item level and no primary attributes: those are
/// the persona panel's, which stays open beside this window in retail (owner ruling 2026-10-19,
/// after comparing the two windows side by side: "the profile of retail and kuluu are not matching
/// items.. at all").
fn profile_lines(
    snap: &kuluu_snapshot::SceneSnapshot,
    zone_names: Option<&crate::hud::zone_flash::ZoneNameResolver>,
) -> Vec<String> {
    let stats = snap.stats.as_ref();
    let mut lines = vec![PROFILE_SECTION_PROFILE.to_string()];

    // `Nation:` is s2c 0x061's nation byte, named through the pinned list the mission tables use.
    lines.push(format!(
        "Nation: {}",
        match stats
            .and_then(|s| crate::hud::menu::MISSION_NATION_NAMES_PINNED.get(usize::from(s.nation)))
        {
            Some(name) => (*name).to_string(),
            None => UNREAD.to_string(),
        }
    ));

    // `Race:` comes from the entity look, not from s2c 0x061, so it stays unread until we spawn.
    lines.push(format!(
        "Race: {}",
        match self_race(snap).and_then(race_name) {
            Some(name) => name.to_string(),
            None => UNREAD.to_string(),
        }
    ));

    // `Home Point:` is s2c 0x061's BindZoneNo; the zone table names it exactly as the entry banner
    // does (same resolver, same underscore law).
    let home_point_zone = stats.and_then(|s| (s.home_point_zone != 0).then_some(s.home_point_zone));
    lines.push(format!(
        "Home Point: {}",
        match home_point_zone.and_then(|zone| zone_names.and_then(|r| (r.0)(zone))) {
            Some(name) => name.replace('_', " "),
            None => UNREAD.to_string(),
        }
    ));

    // `Rank:` prints s2c 0x061's rank and, to its right, the label of the bar that fills in the row
    // under it (the widget lives in the window; this row is where it hangs from).
    let rank = stats.map_or_else(|| UNREAD.to_string(), |s| s.conquest_rank.to_string());
    lines.push(format!("Rank: {rank}   {PROFILE_RANK_BAR_LABEL}"));

    // The bar's own line carries no text; the spacer keeps its vertical slot reserved so the
    // window is as tall as retail's whether or not the report has arrived.
    lines.push(String::new());

    // `Mastery Rank:` and the mentor marker are s2c 0x01B's three tail bytes. Retail draws the
    // mentor as a pennant glyph; these rows take text only, so kuluu prints its rank number there
    // and an unread marker when the byte says no mentor.
    let ranks = snap.job_ranks.as_ref();
    lines.push(format!(
        "Mastery Rank: {}    Mentor: {}",
        match ranks {
            Some(r) => r.mastery_rank.to_string(),
            None => UNREAD.to_string(),
        },
        match ranks.filter(|r| r.mentor_rank != 0) {
            Some(r) => r.mentor_rank.to_string(),
            None => UNREAD.to_string(),
        }
    ));

    lines.push(PROFILE_SECTION_MONSTROSITY.to_string());
    // `Gladiator Rank:` is in retail's window but nowhere else: a search for gladiator across the
    // vendored server turns up mob names, a key item and the pankration enum, never a rank ladder,
    // so there is no field to read here. kuluu shows the label with an unread marker instead of
    // inventing one.
    lines.push(format!("Gladiator Rank: {UNREAD}"));

    // `Prestige:` comes out of the s2c 0x113 report body the Currencies list already keeps.
    let prestige = snap
        .currencies_1
        .as_deref()
        .and_then(crate::hud::currency_law::prestige);
    lines.push(format!(
        "Prestige:   {}",
        prestige.map_or_else(|| UNREAD.to_string(), |p| p.to_string(),)
    ));
    lines
}

/// The caption retail puts on the Profile window's top border: the title currently worn, which is
/// s2c 0x061's `designation` shown through the generated title table.
pub fn profile_caption(snap: &kuluu_snapshot::SceneSnapshot) -> String {
    match snap
        .stats
        .as_ref()
        .and_then(|s| ffxi_vocab::title_names::lookup(s.title_id))
    {
        Some(title) => format!("Title: {title}"),
        None => "Title:".to_string(),
    }
}

pub(crate) fn job_abbrev(job_id: u8) -> String {
    if job_id == 0 {
        return "---".to_string();
    }
    // Canonical 3-letter code from LSB (2 → "MNK"); truncating "Monk" gives "MON".
    match ffxi_vocab::job_names::abbrev(job_id as u16) {
        Some(code) => code.to_string(),
        None => format!("J{job_id}"),
    }
}

fn attr_value(stats: &kuluu_snapshot::CharStats, attr_index: usize) -> Option<u16> {
    Some(match attr_index {
        0 => stats.str_,
        1 => stats.dex,
        2 => stats.vit,
        3 => stats.agi,
        4 => stats.int_,
        5 => stats.mnd,
        6 => stats.chr,
        _ => return None,
    })
}

/// The detail window's rows for one Status selector row (vault main_menu.md §59).
/// Every line is either live snapshot data or a grounded empty readout — the
/// display-only companions in retail show exactly what their source object
/// holds, and kuluu shows nothing more than that from a window it has not
/// built packet readers for yet. Selecting any of these rows is inert
/// ([`crate::hud::menu::DynamicMenuAction::DisplayInfo`]); the windows carry no
/// commands (statcom2's detail table maps ids to OPEN actions only, FFXiMain.dll
/// retail-2026-09 RVA 0x148D00).
/// `currency_labels_1`/`currency_labels_2` are the two Currencies lists' table-12 label
/// columns — slot-aligned with `currency_law::CurrencyMode::rows()`; empty before the DAT
/// catalog loads, in which case each row falls back to its text id like the other loaders do.
/// `zone_names` names the home point of [`kuluu_snapshot::CharStats::home_point_zone`] through the
/// same resolver the entry banner uses; without it that row reads unread rather than a raw id.
pub fn status_detail_lines(
    entry_idx: usize,
    snap: &kuluu_snapshot::SceneSnapshot,
    currency_labels_1: &[Option<String>],
    currency_labels_2: &[Option<String>],
    zone_names: Option<&crate::hud::zone_flash::ZoneNameResolver>,
) -> Vec<String> {
    let Some(entry) = STATUS_ENTRIES.get(entry_idx) else {
        return vec!["(no such status row)".to_string()];
    };
    match entry.kind {
        StatusEntryKind::Profile => profile_lines(snap, zone_names),
        StatusEntryKind::JobLevels => match snap.jobs.as_ref() {
            Some(list) => {
                let mut lines: Vec<String> = list
                    .job_levels
                    .iter()
                    .enumerate()
                    .filter(|(job_id, _)| *job_id != 0)
                    .filter(|(_, &level)| level > 0)
                    .map(|(job_id, &level)| {
                        // One space after the code (owner ruling 2026-10-20, ur/33: the old
                        // `{:<16}` field was the "LARGE gap"), and the level reserves TWO digit
                        // slots always (owner ruling 2026-10-20, ur/39: "lets assume 2 digits
                        // always for space") so single-digit levels keep column two steady.
                        format!("{:<4}{:>2}", job_abbrev(job_id as u8), level)
                    })
                    .collect();
                if lines.is_empty() {
                    lines.push("(no job levels reported yet)".to_string());
                }
                lines
            }
            None => vec!["(job roster not received yet)".to_string()],
        },
        StatusEntryKind::MasterLevels => vec![
            "(master-level roster has no wire source in LSB yet — the window opens empty like retail's with nothing to show)".to_string(),
        ],
        StatusEntryKind::CombatSkill => skill_detail_rows(
                snap,
                SkillFamily::Noncraft(ffxi_proto::decode::CharSkillLevels::COMBAT_SKILL_RANGE),
            )
            .into_iter()
            .map(|row| row.text)
            .collect(),
        StatusEntryKind::MagicSkill => skill_detail_rows(
                snap,
                SkillFamily::Noncraft(ffxi_proto::decode::CharSkillLevels::MAGIC_SKILL_RANGE),
            )
            .into_iter()
            .map(|row| row.text)
            .collect(),
        StatusEntryKind::CraftSkill => skill_detail_rows(snap, SkillFamily::Craft)
            .into_iter()
            .map(|row| row.text)
            .collect(),
        // The two Currencies lists read the raw s2c report through the client's own getter
        // law — one descriptor row each, label from GetText table 12, value from the packet
        // (FFXiMain.dll retail-2026-09 fill `.text 0x2297C0`, draw `.text 0x229F80`; vault §59
        // addendum 27; bead kuluu-o9qt7.35). With no report yet the window is empty, matching
        // retail's evitem opening request-in-flight with nothing drawn until the reply lands.
        StatusEntryKind::Currencies => {
            currency_detail_rows(crate::hud::currency_law::CurrencyMode::One, snap, currency_labels_1)
        }
        StatusEntryKind::Currencies2 => {
            currency_detail_rows(crate::hud::currency_law::CurrencyMode::Two, snap, currency_labels_2)
        }
        // Rows 11, 7, 8 and 13 open the Unity, merit, job point and alter ego point windows
        // instead of a readout.
        StatusEntryKind::Unity
        | StatusEntryKind::MeritPoints
        | StatusEntryKind::JobPoints
        | StatusEntryKind::FacetCategories => Vec::new(),
        StatusEntryKind::PlayTime => vec![play_time_chat_line(snap)],
    }
}

/// One Currencies list, straight from the folded raw report: `(label) value` per descriptor
/// in DISPLAY order (descriptor law + getters in [`crate::hud::currency_law`]). No report yet
/// → no rows at all: retail's evitem opens with an outstanding request and draws nothing until
/// the s2c reply's one-shot callback fills it (`.text 0x99640`/`.text 0x99660`).
pub fn currency_detail_rows(
    mode: crate::hud::currency_law::CurrencyMode,
    snap: &kuluu_snapshot::SceneSnapshot,
    labels: &[Option<String>],
) -> Vec<String> {
    let Some(body) = (match mode {
        crate::hud::currency_law::CurrencyMode::One => snap.currencies_1.as_deref(),
        crate::hud::currency_law::CurrencyMode::Two => snap.currencies_2.as_deref(),
    }) else {
        return Vec::new();
    };
    mode.rows()
        .iter()
        .enumerate()
        .map(|(slot, &(id, _label_id))| {
            // Labels are slot-aligned (the loader walks the same descriptor array), so an
            // unloaded catalog shows em-dashes rather than transcribed strings — DAT-first.
            let label = labels
                .get(slot)
                .and_then(|s| s.as_deref())
                .unwrap_or("\u{2014}")
                .to_string();
            match crate::hud::currency_law::currency_value(mode, id, body) {
                Some(value) => format!("{label} {value}"),
                // The packet stopped short of this field — retail would read past the block,
                // kuluu shows no value rather than invented bytes.
                None => format!("{label} \u{2014}"),
            }
        })
        .collect()
}

/// The blue-paint flag aligned 1:1 with `status_detail_lines`' output for the three
/// skill entries — both walk `skill_detail_rows`, so the indices match by construction.
/// Every other entry yields an empty mask (`.text FFXiMain.dll retail-2026-09 0x148564`).
pub fn status_detail_capped(entry_idx: usize, snap: &kuluu_snapshot::SceneSnapshot) -> Vec<bool> {
    let Some(entry) = STATUS_ENTRIES.get(entry_idx) else {
        return Vec::new();
    };
    use ffxi_proto::decode::CharSkillLevels;
    let family = match entry.kind {
        StatusEntryKind::CombatSkill => SkillFamily::Noncraft(CharSkillLevels::COMBAT_SKILL_RANGE),
        StatusEntryKind::MagicSkill => SkillFamily::Noncraft(CharSkillLevels::MAGIC_SKILL_RANGE),
        StatusEntryKind::CraftSkill => SkillFamily::Craft,
        _ => return Vec::new(),
    };
    skill_detail_rows(snap, family)
        .into_iter()
        .map(|row| row.capped)
        .collect()
}

/// Which family of the 0x062 table a Status skill window shows. Combat and magic
/// share one encoding (level in the low 15 bits, hidden only at exactly 0x8000)
/// and the two-column layout; craft hides on 0xFFFF instead and packs level+rank,
/// drawn in one column.
#[derive(Debug, Clone)]
pub enum SkillFamily {
    Noncraft(std::ops::RangeInclusive<usize>),
    Craft,
}

impl SkillFamily {
    /// One column for craft, two for combat/magic — the row pitch the retail
    /// height law divides by (FFXiMain.dll retail-2026-09 `.text 0x1483B0`:
    /// `height = 22 + 14 * ceil(n/2)`; craft uses `22 + 14*n` at RVA 0x148840).
    pub fn columns(&self) -> usize {
        match self {
            Self::Noncraft(_) => 2,
            Self::Craft => 1,
        }
    }

    pub fn ids(&self) -> Box<dyn Iterator<Item = usize> + '_> {
        match self {
            Self::Noncraft(range) => Box::new(range.clone()),
            Self::Craft => Box::new(ffxi_proto::decode::CharSkillLevels::CRAFT_SKILL_RANGE),
        }
    }
}

/// One row of a skill window: the line as retail formats it, plus whether the raw
/// word says the skill is capped (retail paints those in ARGB 0x802060A0 rather than
/// the ordinary grey — FFXiMain.dll retail-2026-09 `.text 0x14857A..0x14858C`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDetailRow {
    pub text: String,
    pub capped: bool,
}

/// The rows of one skill window straight out of the folded s2c 0x062 block.
/// `None` from the server (no 0x062 yet) is the empty readout — retail has no other
/// source for these three windows, and opens them greyed until a block lands
/// (`.text 0x148B50`).
pub fn skill_detail_rows(
    snap: &kuluu_snapshot::SceneSnapshot,
    family: SkillFamily,
) -> Vec<SkillDetailRow> {
    let Some(block) = snap.char_skills.as_ref() else {
        return Vec::new();
    };
    let mut visible: Vec<(String, bool)> = Vec::new();
    for id in family.ids() {
        let word = block.skills.get(id).copied().unwrap_or(0);
        let label = skill_label(id);
        match family {
            SkillFamily::Noncraft(_) => {
                if let Some(row) = ffxi_proto::decode::CharSkillLevels::noncraft_row(word) {
                    visible.push((format!("{label} {}", row.level), row.capped));
                }
            }
            SkillFamily::Craft => {
                // Retail's craft rows read `name  rank word  (level)` - the middle column is the
                // rank the SERVER put in the packet word's low 5 bits, mapped through LSB's own
                // xi.craftRank ladder below (owner captures ur/44-45: kuluu showed no middle).
                if let Some(row) = ffxi_proto::decode::CharSkillLevels::craft_row(word) {
                    visible.push((
                        format!("{label} {} ({})", craft_rank_word(row.rank), row.level),
                        row.capped,
                    ));
                }
            }
        }
    }

    // One row PER ENTRY (owner ruling 2026-10-20, ur/40): combat/magic get the same two-column
    // treatment Job Levels gets - `update_status_detail_window` splits the roster across packed,
    // fitted columns (first-half-left) rather than the old merged pair-lines, which sprawled off
    // a fitted box. The blue-paint flag stays per entry (retail tints each entry, `.text 0x148564`).
    let _columns = family.columns();
    visible
        .into_iter()
        .map(|(text, capped)| SkillDetailRow { text, capped })
        .collect()
}

/// The craft rank words LSB itself defines for the packet's low-5-bit rank index,
/// in ladder order (vendor/server/scripts/enum/craft_rank.lua `xi.craftRank`). Index
/// out of range renders no word rather than inventing one.
fn craft_rank_word(rank: u8) -> &'static str {
    const CRAFT_RANK_WORDS: [&str; 16] = [
        "Amateur",
        "Recruit",
        "Initiate",
        "Novice",
        "Apprentice",
        "Journeyman",
        "Craftsman",
        "Artisan",
        "Adept",
        "Veteran",
        "Expert",
        "Authority",
        "Luminary",
        "Master",
        "Grandmaster",
        "Legend",
    ];
    CRAFT_RANK_WORDS
        .get(usize::from(rank))
        .copied()
        .unwrap_or("")
}

/// The id→name table is compile-time vocab (ids are stable); an unknown id still
/// gets a row rather than silently vanishing, because the wire word said it exists.
fn skill_label(id: usize) -> String {
    u8::try_from(id)
        .ok()
        .and_then(ffxi_vocab::skill_names::lookup)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Skill {id}"))
}

pub fn play_time_chat_line(snap: &kuluu_snapshot::SceneSnapshot) -> String {
    let total = snap.play_time_s;
    let days = total / 86_400;
    let hours = (total % 86_400) / 3_600;
    let mins = (total % 3_600) / 60;
    if days > 0 {
        format!("Play time: {days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("Play time: {hours}h {mins}m")
    } else {
        format!("Play time: {mins}m")
    }
}

pub fn status_entries_for(_overlay: &ActiveOverlay) -> &'static [StatusEntry] {
    STATUS_ENTRIES
}

#[derive(Debug, Clone)]
pub struct JobLevelRow {
    pub job_id: u8,
    pub name: &'static str,
    pub level: u8,
}

pub fn job_level_rows(overlay: &ActiveOverlay, levels: &[(u8, u8)]) -> Vec<JobLevelRow> {
    levels
        .iter()
        .filter(|(job_id, _)| overlay.0.job_allowed(*job_id))
        .filter_map(|&(job_id, level)| {
            ffxi_vocab::job_names::lookup(job_id as u16).map(|name| JobLevelRow {
                job_id,
                name,
                level,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression pin (release-client panic, 2026-10-14): the real schedule runs
    /// `update_status_panel`, and Bevy's B0001 conflict check fires at system
    /// init — every &mut Text parameter pair must be provably disjoint via
    /// Without<..>. Unit tests never touched the scheduler, so this only ever
    /// exploded in the built game; ticking it once here pins that class.
    #[test]
    fn update_status_panel_runs_in_a_real_schedule_without_query_conflicts() {
        use bevy::app::App;
        let mut app = App::new();
        app.add_systems(bevy::app::Update, update_status_panel);
        app.insert_resource(crate::input_mode::InputMode::default());
        app.insert_resource(SceneState::default());
        app.init_resource::<crate::hud::menu::ConfigDatRows>();
        // No entities: the system returns early, but the queries are already
        // registered — a B0001 conflict panics inside this same call.
        app.update();
    }

    /// Persona opens with Status, after the help bar, so it covers the bar's title box.
    #[test]
    fn persona_draws_over_the_help_bar_title_box() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.run_system_once(spawn_status_panel).unwrap();
        world
            .run_system_once(crate::hud::menu_help_bar::spawn_menu_help_bar)
            .unwrap();
        fn z_of<F: bevy::ecs::query::QueryFilter>(world: &mut World) -> i32 {
            world
                .query_filtered::<&GlobalZIndex, F>()
                .single(world)
                .expect("one root")
                .0
        }
        let persona = z_of::<With<StatusLeftStack>>(&mut world);
        let bar = z_of::<With<crate::hud::menu_help_bar::MenuHelpBar>>(&mut world);
        assert!(
            persona > bar,
            "persona {persona} must draw over the help bar {bar}"
        );
    }

    fn persona_app() -> bevy::app::App {
        let mut app = bevy::app::App::new();
        // Enough of the asset layer for `load_ui_fonts` to hand out handles; no IO wanted.
        app.insert_resource(bevy::asset::Assets::<Font>::default());
        app.init_resource::<crate::hud::menu::ConfigDatRows>();
        app.init_resource::<SceneState>();
        // Mirrors KuluuRenderPlugin: both cuts of the HUD family load, so
        // `update_persona_frame` can set the caption in the oblique.
        app.init_resource::<crate::ui_font::UiFont>();
        app.init_resource::<crate::ui_font::UiFontItalic>();
        app.add_systems(bevy::app::Startup, crate::ui_font::load_ui_fonts);
        app.add_systems(bevy::app::Startup, spawn_status_panel);
        app.add_systems(bevy::app::Update, update_persona_frame);
        app.update();
        app
    }

    fn persona_node(app: &mut bevy::app::App) -> Node {
        app.world_mut()
            .query_filtered::<&Node, With<StatusPanel>>()
            .single(app.world())
            .expect("one persona panel")
            .clone()
    }

    /// Persona takes width from its record frame and fits the pitch to that frame, but lets layout
    /// measure its height (owner 2026-10-20: pinned arithmetic laid CHR across the border once the
    /// bottom padding went); without an install it keeps kuluu's own panel.
    #[test]
    fn persona_takes_its_rectangle_from_its_record_frame() {
        let mut app = persona_app();
        let fallback = persona_node(&mut app);
        assert_eq!(fallback.width, Val::Px(PANEL_WIDTH_PX));
        assert_eq!(fallback.height, Val::Auto);
        assert_eq!(fallback.row_gap, Val::Px(PERSONA_ROW_GAP_PX));

        const RECORD_WIDTH: i16 = 112;
        const RECORD_HEIGHT: i16 = 190;
        {
            let mut rows = app
                .world_mut()
                .resource_mut::<crate::hud::menu::ConfigDatRows>();
            rows.frames.insert(
                PERSONA_WINDOW.into(),
                (
                    PERSONA_STATUS_LEFT as i16,
                    PERSONA_STATUS_TOP as i16,
                    RECORD_WIDTH,
                ),
            );
            rows.frame_extents.insert(
                PERSONA_WINDOW.into(),
                (RECORD_HEIGHT, crate::hud::menu::FRAME_ANCHOR_NONE),
            );
        }
        app.update();
        let placed = persona_node(&mut app);
        // Height law (owner 2026-10-20): Taffy measures the box - never arithmetic pinning, which
        // laid CHR across the border once there was no slack under it. No bottom padding; chrome
        // top padding gone too ("remove the top padding on all", ur/26) - but NOT a hair more:
        // the top inset is exactly the caption's hanging half (its given height, ur/27), which
        // for this record is fitted pitch / 2 - border / 2.
        assert_eq!(placed.padding.bottom, Val::ZERO);
        let inner_full = f32::from(RECORD_HEIGHT)
            - 2.0 * (PERSONA_PAD_Y_PX + PERSONA_BORDER_PX)
            - PERSONA_BLOCK_GAPS as f32 * PERSONA_BLOCK_GAP_PX;
        let pitch = inner_full / PERSONA_ROW_COUNT as f32;
        assert!(
            matches!(placed.padding.top, Val::Px(top) if (top - (pitch / 2.0 - PERSONA_BORDER_PX / 2.0)).abs() < 0.01),
            "the row zone reserves the caption's hanging half, nothing more: {:?}",
            placed.padding.top
        );
        assert_eq!(
            (placed.width, placed.height),
            (Val::Px(f32::from(RECORD_WIDTH)), Val::Auto),
            "width from the record; height measured by layout, never computed"
        );

        let inner = f32::from(RECORD_HEIGHT) - 2.0 * (PERSONA_PAD_Y_PX + PERSONA_BORDER_PX);
        let world = app.world_mut();
        let rows: Vec<(f32, bevy::text::LineHeight)> = world
            .query::<(&PersonaCell, &TextFont, &bevy::text::LineHeight)>()
            .iter(world)
            .map(|(_, font, pitch)| {
                let bevy::text::FontSize::Px(px) = font.font_size else {
                    panic!("persona cells size in px");
                };
                (px, *pitch)
            })
            .collect();
        assert_eq!(rows.len(), PERSONA_ROW_COUNT * PERSONA_COLS.len());
        for (font_px, pitch) in rows {
            let bevy::text::LineHeight::Px(pitch) = pitch else {
                panic!("fitted rows carry an explicit pitch");
            };
            assert!(
                pitch * PERSONA_ROW_COUNT as f32 <= inner + f32::EPSILON,
                "rows overflow the frame"
            );
            assert!(crate::ui_font::line_height_px(font_px) <= pitch + f32::EPSILON);
        }
    }

    /// The fitted font also keeps the widest row inside the record's width.
    #[test]
    fn persona_layout_fits_the_widest_row_into_the_record_width() {
        const NARROW: f32 = 80.0;
        const TALL: f32 = 400.0;
        const WIDEST: usize = 20;
        let frame = crate::hud::menu::RecordFrame {
            x: PERSONA_STATUS_LEFT,
            y: PERSONA_STATUS_TOP,
            width: NARROW,
            height: Some(TALL),
            anchor: crate::hud::menu::FRAME_ANCHOR_NONE,
        };
        let layout = persona_layout(Some(&frame), WIDEST);
        let inner = NARROW - 2.0 * (PERSONA_PAD_X_PX + PERSONA_BORDER_PX);
        let row = "0".repeat(WIDEST);
        assert!(crate::ui_font::text_width_px(&row, layout.row_font_px) <= inner + 1e-3);
        assert_eq!(persona_layout(None, WIDEST), PersonaLayout::FALLBACK);
    }

    fn persona_snapshot(sub_job: u8) -> kuluu_snapshot::SceneSnapshot {
        const MAIN_JOB: u8 = 16;
        const SUB_JOB_LEVEL: u8 = 7;
        let mut snap = kuluu_snapshot::SceneSnapshot {
            self_char_id: Some(7),
            ..Default::default()
        };
        snap.party.push(kuluu_snapshot::PartyMember {
            id: 7,
            act_index: 1,
            name: Some("Cowrevenge".into()),
            hp: 232,
            mp: 98,
            tp: 525,
            hp_pct: 100,
            mp_pct: 100,
            zone_no: 1,
            main_job: MAIN_JOB,
            main_job_lv: 14,
            sub_job,
            sub_job_lv: SUB_JOB_LEVEL,
            is_party_leader: false,
            is_alliance_leader: false,
            in_mog_house: false,
            party_no: 0,
        });
        snap.stats = Some(kuluu_snapshot::CharStats {
            item_level: 0,
            su_lv: 0,
            hp_max: 232,
            mp_max: 98,
            str_: 17,
            dex: 16,
            bonus: [0, 6, 0, 0, 0, 0, -1],
            ..Default::default()
        });
        snap
    }

    /// Two job lines (the sub only with a sub job), one ILv/Su row, HP/MP/TP each on their own
    /// row, and each attribute's base with its nonzero adjustment.
    #[test]
    fn persona_rows_match_retail_layout() {
        const SUB_JOB: u8 = 1;
        let job = |id: u8| ffxi_vocab::job_names::lookup(u16::from(id)).unwrap();
        let snap = persona_snapshot(SUB_JOB);
        let lines = persona_lines(&snap);
        assert_eq!(lines.len(), PERSONA_ROW_COUNT);
        let main = format!("Lv 14 {}", job(16));
        let sub = format!("Lv 7 {}", job(SUB_JOB));
        let want: [&str; PERSONA_HEAD_ROWS] = [
            "Cowrevenge",
            &main,
            &sub,
            "ILv ---/ Su 0",
            "HP 232/232",
            "MP 98/98",
            "TP 525",
        ];
        for (i, want) in want.into_iter().enumerate() {
            assert_eq!(lines[i].display_text(), want, "row {i}");
        }
        assert_eq!(
            lines[PERSONA_HEAD_ROWS].display_text(),
            "STR 17",
            "the column table right-sets the base; joined view keeps one space"
        );
        assert_eq!(lines[PERSONA_HEAD_ROWS].bonus, None);
        assert_eq!(lines[PERSONA_HEAD_ROWS + 1].display_text(), "DEX 16");
        assert_eq!(lines[PERSONA_HEAD_ROWS + 1].bonus, Some(6));
        assert_eq!(lines[PERSONA_ROW_COUNT - 1].bonus, Some(-1));

        let solo = persona_lines(&persona_snapshot(0));
        assert_eq!(solo.len(), PERSONA_ROW_COUNT - 1, "no sub job, no sub line");
        assert_eq!(solo[2].display_text(), "ILv ---/ Su 0");

        let mut geared = persona_snapshot(0);
        if let Some(stats) = geared.stats.as_mut() {
            stats.item_level = 119;
            stats.su_lv = 3;
        }
        assert_eq!(persona_lines(&geared)[2].display_text(), "ILv 119/ Su 3");
    }

    /// Every cell of the column table draws its own part — label, right-set value, shared
    /// slash, max/Su — the bonus column shows only the nonzero adjustment (green above zero,
    /// red below), and rows past the line count hide.
    #[test]
    fn persona_rows_fill_their_retail_cells() {
        use crate::input_mode::{InputMode, MenuKind, MenuLevel, MenuStack};
        let mut app = persona_app();
        app.add_systems(bevy::app::Update, update_status_panel);
        let mut stack = MenuStack::root();
        stack.levels.push(MenuLevel::new(MenuKind::Status, 0));
        app.insert_resource(InputMode::Menu(stack));
        app.world_mut().resource_mut::<SceneState>().snapshot = persona_snapshot(0);
        app.update();

        let world = app.world_mut();
        let mut hidden: Vec<usize> = world
            .query::<(&PersonaRow, &Node)>()
            .iter(world)
            .filter(|(_, node)| node.display == Display::None)
            .map(|(row, _)| row.index)
            .collect();
        hidden.sort_unstable();
        assert_eq!(
            hidden,
            [PERSONA_ROW_COUNT - 1],
            "no sub job leaves one slot free"
        );

        let mut cells: Vec<(usize, PersonaCol, String, Color)> = world
            .query::<(&PersonaCell, &Text, &TextColor)>()
            .iter(world)
            .map(|(cell, text, color)| (cell.index, cell.col, text.as_str().to_string(), color.0))
            .collect();
        cells.sort_by_key(|c| (c.0, PERSONA_COLS.iter().position(|p| *p == c.1).unwrap()));
        let at = |index: usize, col: PersonaCol| -> (String, Color) {
            cells
                .iter()
                .find(|c| c.0 == index && c.1 == col)
                .map(|c| (c.2.clone(), c.3))
                .expect("every row spawns every column")
        };

        // Without the sub line ILv is row 2 and HP row 3.
        assert_eq!(at(0, PersonaCol::Label).0, "Cowrevenge");
        assert_eq!(
            (
                at(2, PersonaCol::Label).0.as_str(),
                at(2, PersonaCol::V1).0.as_str(),
                at(2, PersonaCol::Slash).0.as_str(),
                at(2, PersonaCol::V2).0.as_str()
            ),
            ("ILv", "---", "/", " Su 0")
        );
        assert_eq!(
            (
                at(3, PersonaCol::Label).0.as_str(),
                at(3, PersonaCol::V1).0.as_str(),
                at(3, PersonaCol::Slash).0.as_str(),
                at(3, PersonaCol::V2).0.as_str()
            ),
            ("HP", "232", "/", "232"),
            "the numbers meet the shared slash column"
        );
        assert_eq!(
            (
                at(5, PersonaCol::Label).0.as_str(),
                at(5, PersonaCol::V1).0.as_str(),
                at(5, PersonaCol::Slash).0.as_str()
            ),
            ("TP", "525", ""),
            "TP's lone number stacks in the value column with no slash"
        );

        // Without a sub line the attribute rows start one row early, so STR is 6 and DEX 7.
        assert_eq!(at(6, PersonaCol::V1).0, "17");
        assert_eq!(at(6, PersonaCol::Bonus), (String::new(), theme::TEXT));
        assert_eq!(
            at(7, PersonaCol::Bonus),
            (" +6".to_string(), theme::ATTR_ADJUST_UP)
        );
        // CHR is line 13 of the 13-line snapshot — slot PERSONA_ROW_COUNT-1 stays empty.
        assert_eq!(at(PERSONA_ROW_COUNT - 2, PersonaCol::V1).0, "0");
        assert_eq!(
            at(PERSONA_ROW_COUNT - 2, PersonaCol::Bonus),
            (" -1".to_string(), theme::ATTR_ADJUST_DOWN),
            "CHR carries the negative adjustment"
        );
    }

    /// Retail groups the table [name/job/ILv] · [HP/MP/TP] · attributes; the air is a margin
    /// under whichever slot holds ILv and TP in this snapshot.
    #[test]
    fn persona_blocks_breathe_after_the_ilv_and_tp_rows() {
        use crate::input_mode::{InputMode, MenuKind, MenuLevel, MenuStack};
        let mut app = persona_app();
        app.add_systems(bevy::app::Update, update_status_panel);
        let mut stack = MenuStack::root();
        stack.levels.push(MenuLevel::new(MenuKind::Status, 0));
        app.insert_resource(InputMode::Menu(stack));
        // With a sub job ILv sits at row 3 and TP at row 6.
        app.world_mut().resource_mut::<SceneState>().snapshot = persona_snapshot(1);
        app.update();
        let world = app.world_mut();
        let mut margins: Vec<(usize, Val)> = world
            .query::<(&PersonaRow, &Node)>()
            .iter(world)
            .map(|(row, node)| (row.index, node.margin.bottom))
            .collect();
        margins.sort_by_key(|m| m.0);
        assert_eq!(margins.len(), PERSONA_ROW_COUNT);
        assert_eq!(margins[3].1, Val::Px(PERSONA_BLOCK_GAP_PX), "after ILv");
        assert_eq!(margins[6].1, Val::Px(PERSONA_BLOCK_GAP_PX), "after TP");
        for (index, margin) in margins.iter().take(PERSONA_ROW_COUNT) {
            if *index != 3 && *index != 6 {
                assert_eq!(*margin, Val::ZERO, "row {index} keeps the plain pitch");
            }
        }
    }

    /// The panel title rides the persona panel, absolutely placed and centered, lifted half
    /// its line height to straddle the frame's top edge (owner ruling 2026-10-18: "Retail
    /// centers it on the persona window's top border"). NO box around the word — only a
    /// one-border-row strip masks the frame line behind it (owner x3, 2026-10-19).
    #[test]
    fn status_caption_straddles_the_panel_top_border() {
        let mut app = persona_app();
        let world = app.world_mut();
        // Owned copies per query, each dropped before the next borrow — several live
        // World queries over one test would fight for the same mutable world.
        let (caption, node) = {
            let mut q = world.query_filtered::<(Entity, &Node), With<StatusCaption>>();
            let (c, n) = q.single(world).expect("one caption");
            (c, n.clone())
        };
        {
            let mut t = world.query_filtered::<&Text, With<StatusCaption>>();
            assert_eq!(t.single(world).expect("caption text").as_str(), "Status");
        }
        let caption_bg = world.get::<BackgroundColor>(caption).map(|b| b.0);
        assert!(
            caption_bg.is_none() || caption_bg.unwrap().alpha() == 0.0,
            "the word itself never carries a visible fill — that read as a box (got {caption_bg:?})"
        );
        let mask = {
            let mut mq = world.query_filtered::<&Node, With<StatusCaptionMask>>();
            mq.single(world).expect("one caption strip").clone()
        };
        assert!(
            matches!(mask.height, Val::Px(h) if (h - PERSONA_BORDER_PX).abs() < 0.01),
            "the strip is one frame-border row tall: {:?}",
            mask.height
        );
        assert_eq!(node.position_type, PositionType::Absolute);
        assert!(
            matches!(node.top, Val::Px(top) if top < 0.0),
            "lifted over the frame edge: {:?}",
            node.top
        );
        // Given height (owner ruling 2026-10-20, capture ur/27: "should have give height to
        // prevent word overlapping") - explicit line box, from which the panel's top row
        // reservation is drawn.
        let want_h = crate::ui_font::line_height_px(PERSONA_ROW_FONT_PX);
        assert!(
            matches!(node.height, Val::Px(h) if (h - want_h).abs() < 0.01),
            "the caption wears its given height: {:?}",
            node.height
        );
        // Centering law (owner 2026-10-19): word and strip share one middle — that of the
        // band absolute children measure across, i.e. the panel minus its two borders, NOT
        // the content box inside the padding (the old math was off by `PERSONA_PAD_X_PX`).
        let band = PANEL_WIDTH_PX - 2.0 * PERSONA_BORDER_PX;
        let text_w = crate::ui_font::text_width_px(STATUS_CAPTION_TEXT, PERSONA_ROW_FONT_PX);
        let want_mask_w = text_w + 2.0 * PERSONA_CAPTION_PAD_PX;
        let want_mask_left = (band - want_mask_w) / 2.0;
        let want_caption_left = (band - text_w) / 2.0;
        assert!(
            matches!(node.left, Val::Px(l) if (l - want_caption_left).abs() < 0.5),
            "word centered by placement: left {:?}, want {want_caption_left}",
            node.left
        );
        assert!(
            matches!(mask.left, Val::Px(l) if (l - want_mask_left).abs() < 0.5),
            "strip centered on the same band: left {:?}, want {want_mask_left}",
            mask.left
        );
        let Val::Px(mask_left) = mask.left else {
            unreachable!("the update system places the strip in pixels")
        };
        assert!(
            matches!(mask.width, Val::Px(w) if (w - want_mask_w).abs() < 1.0),
            "strip covers the glyphs plus air: {:?}",
            mask.width
        );
        // The strip is symmetric about the word, which only holds if both were centered on
        // one band.
        let mask_center = mask_left + want_mask_w / 2.0;
        assert!(
            (mask_center - band / 2.0).abs() < 0.5,
            "strip middle {mask_center} sits on the panel middle {}",
            band / 2.0
        );
        // Italic (owner 2026-10-19): the word is set in an italic FACE — DejaVu Sans Mono
        // Oblique — while its box stays square up. Tilting the text node was tried and
        // rejected: "that is NOT italics.. you just slanted the whole word".
        let face = {
            let mut q = world.query_filtered::<&TextFont, With<StatusCaption>>();
            q.single(world).expect("one caption font").font.clone()
        };
        let italic = world.resource::<crate::ui_font::UiFontItalic>().0.clone();
        let regular = world.resource::<crate::ui_font::UiFont>().0.clone();
        assert_ne!(italic, regular, "the test app loaded both cuts");
        assert_eq!(
            face,
            bevy::text::FontSource::Handle(italic),
            "the caption is set in the oblique cut"
        );
        assert!(
            world.get::<crate::ui_font::KeepOwnFont>(caption).is_some(),
            "and `apply_ui_font` must not stamp the regular face back over it"
        );
        // The strip covers the frame band (-border..0 from the padding edge), otherwise the
        // line keeps running through the glyphs.
        assert!(
            matches!(mask.top, Val::Px(t) if (t + PERSONA_BORDER_PX).abs() < 0.01),
            "strip sits on the frame band: top {:?}",
            mask.top
        );
        let child_of = world.get::<ChildOf>(caption).expect("caption is parented");
        assert!(
            world.get::<StatusPanel>(child_of.parent()).is_some(),
            "the caption rides the persona panel, hiding with it"
        );
    }

    /// No row's content may move another number's column: every sized persona column pins
    /// min=max=width, nothing in the table shrinks, and a bonus occupies its own column right
    /// after the slash slot (owner ruling 2026-10-18 on the `+5` shifting INT).
    #[test]
    fn persona_columns_are_pinned_against_the_bonus_row() {
        let mut app = persona_app();
        let world = app.world_mut();
        let mut q = world.query::<(&PersonaCell, &Node)>();
        assert_eq!(q.iter(world).count(), PERSONA_ROW_COUNT * 5);
        let mut pinned = 0;
        for (cell, node) in q.iter(world) {
            assert_eq!(node.flex_shrink, 0.0, "{cell:?} never shrinks");
            let want = match cell.col {
                PersonaCol::Label => Some(PERSONA_LABEL_COL_PCT),
                PersonaCol::Slash => Some(PERSONA_SLASH_COL_PCT),
                PersonaCol::V2 => Some(PERSONA_MAX_COL_PCT),
                PersonaCol::V1 | PersonaCol::Bonus => None,
            };
            if let Some(pct) = want {
                assert_eq!(node.width, Val::Percent(pct));
                assert_eq!(
                    (node.min_width, node.max_width),
                    (Val::Percent(pct), Val::Percent(pct)),
                    "{:?} column pinned",
                    cell.col
                );
                pinned += 1;
            }
        }
        assert_eq!(pinned, PERSONA_ROW_COUNT * 3);
        let rows: Vec<f32> = world
            .query_filtered::<&Node, With<PersonaRow>>()
            .iter(world)
            .map(|node| node.flex_shrink)
            .collect();
        assert_eq!(rows.len(), PERSONA_ROW_COUNT);
        assert!(rows.iter().all(|shrink| *shrink == 0.0));
    }

    /// The `level` box gauge track grows to fill everything between its blue caps and is grey
    /// behind the salmon fill (owner ruling 2026-10-18: fixed-width floated tracks "fall apart").
    #[test]
    fn exp_gauge_track_is_full_width_and_grey() {
        let mut app = persona_app();
        let world = app.world_mut();
        let mut q = world.query_filtered::<(&Node, &BackgroundColor), With<StatusExpTrack>>();
        let (node, bg) = q.single(world).expect("one gauge track");
        assert!(
            node.flex_grow > 0.0,
            "the track takes the whole span between caps"
        );
        assert_eq!(bg.0, LEVEL_GAUGE_TRACK_COLOR);
    }

    /// Each value row is pulled up against its own label (owner ruling 2026-10-20, capture
    /// ur/26: "the row spacing after EXP and NEXTLevel - so numbers closer to their labels").
    /// Only the pair gaps shrink; heading and labels wear no top margin.
    #[test]
    fn level_values_pull_up_to_their_labels() {
        let mut app = persona_app();
        let world = app.world_mut();
        let mut q = world.query::<(&StatusLevelLine, &ChildOf)>();
        let mut pulled = 0;
        for (line, child_of) in q.iter(world) {
            let wrapper = world
                .get::<Node>(child_of.parent())
                .expect("row wrapper node");
            match line.0 {
                LevelLine::ExpValue | LevelLine::NextValue => {
                    pulled += 1;
                    assert_eq!(
                        wrapper.margin.top,
                        Val::Px(-LEVEL_VALUE_PULL_UP_PX),
                        "the number hugs its label"
                    );
                }
                _ => assert_eq!(wrapper.margin.top, Val::ZERO, "only value rows are pulled"),
            }
        }
        assert_eq!(pulled, 2, "both EXP and NEXTLevel values pull up");
    }

    /// The gauge row reserves the air under its own bottom edge (owner ruling 2026-10-20,
    /// capture ur/24: "the extra edges of XP bar are still not being counted in height
    /// properly") - with no padding on the box, the caps' overhang must be part of what layout
    /// measures for the gauge itself, or its last pixel row lands on the frame line.
    #[test]
    fn exp_gauge_row_carries_its_under_edge_footprint() {
        let mut app = persona_app();
        let world = app.world_mut();
        let row_entity = world
            .query_filtered::<&ChildOf, With<StatusExpTrack>>()
            .single(world)
            .expect("one gauge track")
            .parent();
        let row = world.get::<Node>(row_entity).expect("gauge row node");
        assert_eq!(
            row.margin.bottom,
            Val::Px(LEVEL_GAUGE_BOTTOM_CLEARANCE_PX),
            "the gauge row owns the clearance under its caps"
        );
    }

    /// Retail law (DAT records + owner screenshots 2026-10-19): side details — Profile,
    /// Job Levels, Master Levels and every skill window (`(130,48,w252)`) keep the stats
    /// column showing; only the wide left-side `evitem`/`fp_cat` lists (Currencies 1/2, Facet
    /// Categories) replace it. Supersedes the 2026-10-18 all-details rule.
    #[test]
    fn any_status_detail_hides_the_stats_panel() {
        use bevy::app::App;

        use crate::input_mode::{InputMode, MenuKind, MenuLevel, MenuStack};

        let display_with = |entry: Option<u8>| -> Display {
            let mut app = App::new();
            app.init_resource::<SceneState>();
            app.add_systems(bevy::app::Startup, spawn_status_panel);
            app.add_systems(bevy::app::Update, update_status_panel);
            let mut stack = MenuStack::root();
            stack.levels.push(MenuLevel::new(MenuKind::Status, 0));
            if let Some(entry) = entry {
                stack
                    .levels
                    .push(MenuLevel::new(MenuKind::StatusDetail { entry }, 0));
            }
            app.insert_resource(InputMode::Menu(stack));
            for _ in 0..2 {
                app.update();
            }
            let world = app.world_mut();
            world
                .query_filtered::<&Node, With<StatusLeftStack>>()
                .single(world)
                .expect("one Status host")
                .display
        };

        assert_eq!(display_with(None), Display::Flex);
        for entry in [0u8, 1, 2, 3, 4, 5] {
            assert_eq!(
                display_with(Some(entry)),
                Display::Flex,
                "side detail {entry} keeps the stats column (retail: persona + Status stay up)"
            );
        }
        for entry in [6u8, 7] {
            assert_eq!(
                display_with(Some(entry)),
                Display::None,
                "Currencies/Currencies 2 replace the stats column (left-side evitem list)"
            );
        }
    }

    /// The `level` box never leaves the persona column even when the install supplies its
    /// record — placing from raw frames scales independently of the panel and is exactly what
    /// made kuluu's box wider/offset from Status (owner ruling 2026-10-18: pixel-flush under
    /// the panel, same x, same width).
    #[test]
    fn level_box_never_leaves_the_persona_column() {
        use bevy::app::App;
        let mut app = App::new();
        app.init_resource::<crate::hud::menu::ConfigDatRows>();
        app.add_systems(bevy::app::Startup, spawn_status_panel);
        app.add_systems(bevy::app::Update, update_status_level_box);

        {
            // The install's real record: (16,208) w=112 h=88 — persona's own column.
            let mut rows = app
                .world_mut()
                .resource_mut::<crate::hud::menu::ConfigDatRows>();
            rows.frames
                .insert(STATUS_LEVEL_WINDOW.into(), (16, 208, 112));
            rows.frame_extents.insert(
                STATUS_LEVEL_WINDOW.into(),
                (88, crate::hud::menu::FRAME_ANCHOR_NONE),
            );
        }

        let level_box = |app: &mut App| -> Node {
            app.world_mut()
                .query_filtered::<&Node, With<StatusLevelBox>>()
                .single(app.world())
                .expect("one level box")
                .clone()
        };
        app.update();

        let stacked = level_box(&mut app);
        // No chrome padding top or bottom (owner rulings 2026-10-20, capture ur/26): the box
        // wears side air only; the gauge carries its own under-edge clearance.
        assert_eq!(
            (stacked.padding.top, stacked.padding.bottom),
            (Val::ZERO, Val::ZERO)
        );
        assert_eq!(
            stacked.position_type,
            PositionType::Relative,
            "stays in flow under persona"
        );
        assert_eq!(
            (stacked.left, stacked.top),
            (Val::Auto, Val::Auto),
            "never placed independently"
        );
        assert_eq!(
            stacked.width,
            Val::Px(PANEL_WIDTH_PX),
            "mirrors the panel column from the first frame"
        );

        // Narrow the persona column — the box follows it, staying one aligned column.
        {
            let mut panel_q = app
                .world_mut()
                .query_filtered::<&mut Node, With<StatusPanel>>();
            panel_q.single_mut(app.world_mut()).unwrap().width = Val::Px(112.0);
        }
        app.update();
        assert_eq!(
            level_box(&mut app).width,
            Val::Px(112.0),
            "tracks the persona column width"
        );

        let host = app
            .world_mut()
            .query_filtered::<&Node, With<StatusLeftStack>>()
            .single(app.world())
            .expect("one Status host")
            .clone();
        assert_eq!(
            (host.padding.left, host.padding.top),
            (Val::Px(PERSONA_STATUS_LEFT), Val::Px(PERSONA_STATUS_TOP)),
            "persona keeps its Status spot"
        );
        // DAT delta: persona bottom 16+190=206 to the level record top 208.
        assert_eq!(host.row_gap, Val::Px(2.0));
    }

    /// The `level` box under the persona panel answers from s2c 0x061 EXP
    /// (vault main_menu.md §59 addendum 10 + owner retail screenshot law 2026-10-18):
    /// Level heading, comma-grouped EXP/NEXTLevel values on their own lines, gauge
    /// filled by the ratio — no invented values when the stats block is absent.
    #[test]
    fn level_box_answers_from_the_wire_exp_pair() {
        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        let body = level_box_body(&snap);
        assert_eq!(body.heading, "Level —", "no self yet");
        assert_eq!(
            (body.exp.as_deref(), body.next_exp.as_deref()),
            (None, None)
        );
        assert_eq!(body.fill_percent, 0.0);

        snap.self_char_id = Some(7);
        snap.party.push(kuluu_snapshot::PartyMember {
            id: 7,
            act_index: 0x100,
            name: Some("Cowrevenge".into()),
            hp: 98,
            mp: 55,
            tp: 572,
            hp_pct: 100,
            mp_pct: 100,
            zone_no: 104,
            main_job: 22,
            main_job_lv: 6,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: true,
            is_alliance_leader: false,
            in_mog_house: false,
            party_no: 0,
        });
        let body = level_box_body(&snap);
        assert_eq!(body.heading, "Level 6", "no stats block yet");
        assert!(body.exp.is_none() && body.next_exp.is_none());

        snap.stats = Some(kuluu_snapshot::CharStats {
            exp_now: 806,
            exp_next: 1750,
            ..Default::default()
        });
        let body = level_box_body(&snap);
        assert_eq!(
            (body.exp.as_deref(), body.next_exp.as_deref()),
            (Some("806"), Some("1,750")),
            "comma-grouped like the retail box"
        );
        let want_fill = 806.0 / 1750.0 * 100.0;
        assert!(
            (body.fill_percent - want_fill).abs() < 0.01,
            "gauge fills by the wire ratio: {}",
            body.fill_percent
        );
    }

    #[test]
    fn comma_grouping_matches_the_retail_box() {
        assert_eq!(comma_groups(0), "0");
        assert_eq!(comma_groups(999), "999");
        assert_eq!(comma_groups(1304), "1,304");
        assert_eq!(comma_groups(i16::MAX as u32), "32,767");
    }

    /// Gauge law off the retail screenshot: fill ≈ earned/next measured on the pixels
    /// (≈27px of salmon over the ≈66px track at 1,304/3,400), clamped at both ends.
    #[test]
    fn gauge_fill_follows_and_clamps_the_wire_ratio() {
        let mut snap = kuluu_snapshot::SceneSnapshot {
            self_char_id: Some(7),
            ..Default::default()
        };
        snap.party.push(kuluu_snapshot::PartyMember {
            id: 7,
            act_index: 0x100,
            name: Some("Cowrevenge".into()),
            hp: 232,
            mp: 98,
            tp: 525,
            hp_pct: 100,
            mp_pct: 100,
            zone_no: 104,
            main_job: 22,
            main_job_lv: 14,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: true,
            is_alliance_leader: false,
            in_mog_house: false,
            party_no: 0,
        });
        snap.stats = Some(kuluu_snapshot::CharStats {
            exp_now: 1304,
            exp_next: 3400,
            ..Default::default()
        });
        let body = level_box_body(&snap);
        assert_eq!(body.exp.as_deref(), Some("1,304"));
        assert_eq!(body.next_exp.as_deref(), Some("3,400"));
        assert!((body.fill_percent - 38.35).abs() < 0.02); // screenshot bar ≈ this

        snap.stats = Some(kuluu_snapshot::CharStats {
            exp_now: 9000,
            exp_next: 1750,
            ..Default::default()
        });
        assert_eq!(level_box_body(&snap).fill_percent, 100.0, "clamped at full");

        snap.stats = Some(kuluu_snapshot::CharStats {
            exp_now: -3,
            exp_next: 1750,
            ..Default::default()
        });
        assert_eq!(level_box_body(&snap).fill_percent, 0.0, "never negative");
    }

    #[test]
    fn status_entries_have_disabled_post_classic_screens() {
        let ml = STATUS_ENTRIES
            .iter()
            .find(|e| e.kind == StatusEntryKind::MasterLevels)
            .unwrap();
        assert!(!ml.enabled, "Master Levels disabled");
        let mp = STATUS_ENTRIES
            .iter()
            .find(|e| e.kind == StatusEntryKind::MeritPoints)
            .unwrap();
        assert!(!mp.enabled, "Merit Points disabled");
    }

    #[test]
    fn status_entry_order_matches_retail() {
        assert_eq!(STATUS_ENTRIES[0].kind, StatusEntryKind::Profile);
        assert_eq!(STATUS_ENTRIES[1].kind, StatusEntryKind::JobLevels);
        // statcom2 displays 13 live rows (ffxi-dat record pin, vault
        // main_menu.md §59); the engine list follows that display order.
        assert_eq!(status_entry_count(), STATCOM2_DISPLAY_ITEM_IDS.len());
        assert_eq!(
            STATUS_ENTRIES.last().unwrap().kind,
            StatusEntryKind::FacetCategories
        );
    }

    #[test]
    fn statcom_display_only_set_matches_retail_detail_table() {
        // The retail set is profile/joblevel/btlskill/mgcskill/trdskill/
        // masterle: exactly what FFXiMain.dll retail-2026-09 close-details
        // helper RVA 0x148AC0 acts on (vault main_menu.md §59).
        for kind in [
            StatusEntryKind::Profile,
            StatusEntryKind::JobLevels,
            StatusEntryKind::MasterLevels,
            StatusEntryKind::CombatSkill,
            StatusEntryKind::MagicSkill,
            StatusEntryKind::CraftSkill,
        ] {
            assert!(status_detail_display_only(kind), "{kind:?} display-only");
        }
        for kind in [
            StatusEntryKind::Currencies,
            StatusEntryKind::MeritPoints,
            StatusEntryKind::JobPoints,
            StatusEntryKind::Unity,
            StatusEntryKind::FacetCategories,
        ] {
            assert!(!status_detail_display_only(kind), "{kind:?} interactive");
        }
    }

    #[test]
    fn job_abbrev_uses_canonical_codes() {
        assert_eq!(job_abbrev(0), "---");
        assert_eq!(job_abbrev(1), "WAR");
        assert_eq!(job_abbrev(2), "MNK"); // not "MON"
        assert_eq!(job_abbrev(3), "WHM"); // not "WHI"
        assert_eq!(job_abbrev(4), "BLM"); // not "BLA"
    }

    #[test]
    fn primary_attrs_count_is_seven() {
        assert_eq!(PRIMARY_ATTRS.len(), 7);
        assert_eq!(PRIMARY_ATTRS[0], "STR");
        assert_eq!(PRIMARY_ATTRS[6], "CHR");
    }

    /// A snapshot holding one folded s2c 0x062 block. Words left at zero are real
    /// level-0 rows — only the family sentinels hide anything.
    fn snap_with_skills(
        ids: &[(usize, u16)],
        craft_all_hidden: bool,
    ) -> kuluu_snapshot::SceneSnapshot {
        use ffxi_proto::decode::CharSkillLevels;
        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        let mut block = kuluu_snapshot::CharSkills::zeroed();
        for (id, word) in ids {
            block.skills[*id] = *word;
        }
        block.skills[*CharSkillLevels::MAGIC_SKILL_RANGE.start()] = 240;
        if craft_all_hidden {
            for id in CharSkillLevels::CRAFT_SKILL_RANGE {
                block.skills[id] = CharSkillLevels::CRAFT_HIDDEN;
            }
        } else {
            // One packed word at id 50: level 122, rank 6.
            const LEVEL_122: u16 = 122 << 5;
            block.skills[50] = LEVEL_122 | 6;
        }
        snap.char_skills = Some(block);
        snap
    }

    /// Combat and magic share one encoding: a word equal to the hidden sentinel is
    /// not drawn at all, a bit-15 word keeps its level (only its colour changes), and
    /// (owner ruling 2026-10-20, ur/40) each ENTRY is its own row - like Job Levels - so the
    /// window's packed two-column grid can place them; pairing into merged lines sprawled.
    #[test]
    fn noncraft_skill_window_applies_the_word_law_one_row_per_entry() {
        use ffxi_proto::decode::CharSkillLevels;
        let snap = snap_with_skills(
            &[
                (1, 301),
                (2, CharSkillLevels::SKILL_HIDDEN),
                (3, CharSkillLevels::SKILL_CAP_BIT | 125),
                (4, 88),
            ],
            false,
        );

        let rows = skill_detail_rows(&snap, SkillFamily::Noncraft(1..=4));
        // Id 2 is the sentinel and drops out; the three remaining ids each get a row.
        assert_eq!(rows.len(), 3);
        assert!(
            rows[0].text.starts_with("Hand-to-Hand") && rows[0].text.contains("301"),
            "{}",
            rows[0].text
        );
        assert!(
            !rows
                .iter()
                .any(|r| r.text.contains("Hand") && r.text.contains("Club")),
            "no merged pair-lines anymore (ur/40)"
        );

        // The capped word is drawn with its level and flagged for the blue paint.
        let capped = skill_detail_rows(&snap, SkillFamily::Noncraft(3..=4));
        assert_eq!(capped.len(), 2);
        assert!(capped[0].capped, "bit-15 marks its own row");
        assert!(!capped[1].capped, "the plain entry keeps a clear flag");
    }

    /// Retail tints EACH entry: with one row per entry (ur/40) a bit-15 word flags its
    /// own row and only that one.
    #[test]
    fn a_capped_word_flags_only_its_own_entry_row() {
        use ffxi_proto::decode::CharSkillLevels;
        let snap = snap_with_skills(&[(4, 88), (5, CharSkillLevels::SKILL_CAP_BIT | 140)], false);
        let rows = skill_detail_rows(&snap, SkillFamily::Noncraft(4..=5));
        assert_eq!(rows.len(), 2);
        assert!(
            !rows[0].capped && !rows[0].text.contains("301"),
            "range starts at id 4"
        );
        assert!(rows[1].capped, "bit-15 lights its own row only");
    }

    /// The tint mask walks the same rows as the drawn lines — aligned by construction.
    #[test]
    fn detail_capped_mask_aligns_with_detail_lines() {
        use ffxi_proto::decode::CharSkillLevels;
        let snap = snap_with_skills(&[(1, 301), (2, CharSkillLevels::SKILL_CAP_BIT | 75)], false);
        let combat_entry = STATUS_ENTRIES
            .iter()
            .position(|e| e.kind == StatusEntryKind::CombatSkill)
            .unwrap();
        let mask = status_detail_capped(combat_entry, &snap);
        let lines = status_detail_lines(combat_entry, &snap, &[], &[], None);
        assert!(!lines.is_empty());
        assert_eq!(mask.len(), lines.len(), "one flag per drawn line");
        assert!(mask.iter().any(|c| *c), "the capped id is flagged");
    }

    /// Craft hides on its own sentinel, unpacks level+rank from the word, and never
    /// pairs (one column).
    #[test]
    fn craft_skill_window_uses_its_own_sentinel_and_one_column() {
        let hidden = snap_with_skills(&[(1, 0)], true);
        assert!(
            skill_detail_rows(&hidden, SkillFamily::Craft).is_empty(),
            "every word at the sentinel means no rows"
        );

        let shown = snap_with_skills(&[(1, 0)], false);
        let rows = skill_detail_rows(&shown, SkillFamily::Craft);
        // Craft hides only on its sentinel: an untrained craft is a real row at
        // level 0, which is why every id in 48..=62 shows.
        const CRAFT_IDS: usize = *ffxi_proto::decode::CharSkillLevels::CRAFT_SKILL_RANGE.end()
            - *ffxi_proto::decode::CharSkillLevels::CRAFT_SKILL_RANGE.start()
            + 1;
        assert_eq!(rows.len(), CRAFT_IDS);

        // The one packed word unpacks to its level; the others read as level 0.
        let packed = rows.iter().filter(|r| r.text.contains("122")).count();
        assert_eq!(packed, 1, "exactly one row carries the packed word");
        // Retail's craft triple (owner captures ur/44-45): name · rank word from the packet
        // nibble · (level). The packed seed word carries rank 6 = xi.craftRank CRAFTSMAN.
        let smithy = rows
            .iter()
            .find(|r| r.text.contains("122"))
            .expect("the packed craft row");
        assert!(
            smithy.text.contains("Craftsman") && smithy.text.ends_with("(122)"),
            "rank word and parenthesised level: {}",
            smithy.text
        );
        let fished = rows
            .iter()
            .find(|r| r.text.starts_with("Fishing"))
            .expect("id 48 is Fishing");
        assert!(
            fished.text.ends_with("Amateur (0)"),
            "an untrained craft still shows retail's rank column: {}",
            fished.text
        );
        // One column: no line ever holds two entries, so nothing gets the pairing
        // padding of two spaces. (An unknown id falls back to `Skill 60`, so the token
        // count is not what distinguishes one column from two.)
        for row in &rows {
            assert!(
                !row.text.contains("  "),
                "no pairing padding in a one-column window: {}",
                row.text
            );
        }
    }

    /// With no 0x062 yet the three skill windows are empty and grey: retail's
    /// `statcom2` opens them disabled and only content in that table lifts them.
    #[test]
    fn status_skill_rows_stay_disabled_until_the_block_arrives() {
        use ffxi_proto::decode::CharSkillLevels;
        let empty = kuluu_snapshot::SceneSnapshot::default();
        for kind in [
            StatusEntryKind::CombatSkill,
            StatusEntryKind::MagicSkill,
            StatusEntryKind::CraftSkill,
        ] {
            assert!(
                !status_row_enabled(kind, &empty),
                "{kind:?} must be grey with no 0x062"
            );
            assert!(skill_detail_rows(&empty, SkillFamily::Craft).is_empty());
        }

        // A block whose combat words are all the sentinel still leaves Combat grey,
        // while magic (which has content) lights up.
        let mut snap = empty;
        let mut block = kuluu_snapshot::CharSkills::zeroed();
        for id in CharSkillLevels::COMBAT_SKILL_RANGE {
            block.skills[id] = CharSkillLevels::SKILL_HIDDEN;
        }
        for id in CharSkillLevels::CRAFT_SKILL_RANGE {
            block.skills[id] = CharSkillLevels::CRAFT_HIDDEN;
        }
        block.skills[*CharSkillLevels::MAGIC_SKILL_RANGE.start()] = 12;
        snap.char_skills = Some(block);

        assert!(!status_row_enabled(StatusEntryKind::CombatSkill, &snap));
        assert!(!status_row_enabled(StatusEntryKind::CraftSkill, &snap));
        assert!(status_row_enabled(StatusEntryKind::MagicSkill, &snap));
    }

    /// The three level-99 rows follow their own gates: mastery flag for Master
    /// Levels, the Alter Ego key item for Facet Categories, and a JobPoints record for Job
    /// Points (`job_points_row_needs_access_and_level_99`).
    #[test]
    fn status_level_99_rows_gate_on_their_own_conditions() {
        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        assert!(!status_row_enabled(StatusEntryKind::MasterLevels, &snap));
        assert!(!status_row_enabled(StatusEntryKind::JobPoints, &snap));
        assert!(!status_row_enabled(StatusEntryKind::FacetCategories, &snap));

        snap.stats = Some(kuluu_snapshot::CharStats {
            master_level_unlocked: true,
            ..Default::default()
        });
        // Still gated: the mastery flag is only half of Master Levels' condition.
        assert!(!status_row_enabled(StatusEntryKind::MasterLevels, &snap));

        let mut party = snap.party.clone();
        party.push(kuluu_snapshot::PartyMember {
            id: 7,
            act_index: 1,
            name: Some("Sylvie".into()),
            hp: 1,
            mp: 0,
            tp: 0,
            hp_pct: 100,
            mp_pct: 0,
            zone_no: 1,
            main_job: 0,
            main_job_lv: MASTER_LEVEL_JOB_CAP,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: false,
            is_alliance_leader: false,
            party_no: 0,
            in_mog_house: false,
        });
        let mut gated = snap.clone();
        gated.party = party;
        // `resolve_self` needs the id to match, or the level reads as 0.
        gated.self_char_id = Some(7);
        assert_eq!(main_job_level(&gated), MASTER_LEVEL_JOB_CAP);

        // Mastery flag + level 99 lifts Master Levels; the key item is still missing
        // for Alter Ego Points, so that one stays grey.
        assert!(status_row_enabled(StatusEntryKind::MasterLevels, &{
            gated.clone()
        }));
        assert!(
            !status_row_enabled(StatusEntryKind::JobPoints, &gated),
            "no s2c 0x063 JobPoints record"
        );
        assert!(!status_row_enabled(
            StatusEntryKind::FacetCategories,
            &gated
        ));

        gated.key_items.push(ALTER_EGO_POINTS_KEY_ITEM);
        assert!(
            !status_row_enabled(StatusEntryKind::FacetCategories, &gated),
            "no s2c 0x119 yet: the Status data is not ready"
        );
        gated.status_data_ready = true;
        assert!(status_row_enabled(StatusEntryKind::FacetCategories, &gated));
        assert!(
            !status_row_enabled(StatusEntryKind::Unity, &gated),
            "owner ruling ur/147: row 11 stays grey whatever the snapshot says"
        );
    }

    /// Merit Points (id 7): the row lifts exactly when an s2c 0x063 Merits record has arrived
    /// AND its can-use-merit-mode bit is set — retail's two-step gate (.text FFXiMain.dll
    /// retail-2026-09 `0x148E88`: received flag, then stored word bit 13); the detail window
    /// draws live numbers from that record, never a canned line (bead kuluu-o9qt7.43).
    #[test]
    fn merit_points_row_lifts_only_with_a_can_use_merits_record() {
        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        assert!(!status_row_enabled(StatusEntryKind::MeritPoints, &snap));
        // A record that says “not available” keeps the row grey despite being present.
        snap.merits = Some(kuluu_snapshot::MisdataMerits {
            limit_points: 12,
            merit_points: 3,
            blu_bonus: 0,
            can_use_merit_mode: false,
            xp_capped_or_merit_mode: true,
            merit_mode_enabled: false,
            max_merit_points: 98,
        });
        assert!(!status_row_enabled(StatusEntryKind::MeritPoints, &snap));

        snap.merits.as_mut().unwrap().can_use_merit_mode = true;
        assert!(status_row_enabled(StatusEntryKind::MeritPoints, &snap));
    }

    /// Job Points (id 8) lifts once an s2c 0x063 JobPoints record has bit 0 of `access` set and
    /// s2c 0x061 reports a level-99 main job
    /// (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).
    #[test]
    fn job_points_row_needs_access_and_level_99() {
        let mut snap = kuluu_snapshot::SceneSnapshot {
            stats: Some(kuluu_snapshot::CharStats {
                mjob_lv: MASTER_LEVEL_JOB_CAP,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(!status_row_enabled(StatusEntryKind::JobPoints, &snap));
        snap.job_points = Some(kuluu_snapshot::JobPointsRecord::default());
        assert!(
            !status_row_enabled(StatusEntryKind::JobPoints, &snap),
            "access bit 0 clear"
        );
        snap.job_points = Some(kuluu_snapshot::JobPointsRecord {
            access: JOB_POINTS_ACCESS_BIT,
            ..Default::default()
        });
        assert!(status_row_enabled(StatusEntryKind::JobPoints, &snap));
        snap.stats = Some(kuluu_snapshot::CharStats {
            mjob_lv: MASTER_LEVEL_JOB_CAP - 1,
            ..Default::default()
        });
        assert!(!status_row_enabled(StatusEntryKind::JobPoints, &snap));
    }

    /// Rows 7 and 8 open their windows, so they keep no readout of their own.
    #[test]
    fn merit_and_job_point_rows_have_no_readout() {
        let snap = kuluu_snapshot::SceneSnapshot::default();
        for kind in [StatusEntryKind::MeritPoints, StatusEntryKind::JobPoints] {
            let entry = STATUS_ENTRIES.iter().position(|e| e.kind == kind).unwrap();
            assert!(status_detail_lines(entry, &snap, &[], &[], None).is_empty());
        }
    }

    /// Bead kuluu-o9qt7.35: the two Currencies windows draw straight out of the raw s2c block,
    /// one row per descriptor, label slot-aligned from the table-12 loader. Values follow the
    /// client's getter law — first conquest dword (packet +4) is list row 1 in mode 1.
    #[test]
    fn currency_lists_draw_raw_report_rows_in_descriptor_order() {
        use crate::hud::currency_law::CurrencyMode;
        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        assert!(
            currency_detail_rows(CurrencyMode::One, &snap, &[]).is_empty(),
            "no report yet → the list stays empty (retail fills it in its one-shot reply callback)"
        );

        let mut body = vec![0u8; 0xF6];
        // First conquest point dwords at packet +4/+8/+C (mode-1 kind 0).
        body[0x0..0x4].copy_from_slice(&7_777i32.to_le_bytes());
        snap.currencies_1 = Some(body.clone());
        let labels: Vec<Option<String>> = (CurrencyMode::One.rows())
            .iter()
            .map(|_| Some("CP".to_string()))
            .collect();
        let rows = currency_detail_rows(CurrencyMode::One, &snap, &labels);
        assert_eq!(rows.len(), 99, "one row per descriptor");
        assert_eq!(&rows[0], "CP 7777");

        // Unloaded catalog → placeholder labels, never transcribed strings (DAT-first).
        let unlabeled = currency_detail_rows(CurrencyMode::One, &snap, &[]);
        assert_ne!(&unlabeled[0], "CP 7777");
        assert!(unlabeled[0].contains("7777"), "the value still draws");

        // Mode routing: statcom2 display row for item id 6 (Currencies) reads report 1; the
        // id-10 row reads report 2 — different blocks must not cross-contaminate.
        let mut body2 = vec![0u8; 0x9C];
        body2[0..4].copy_from_slice(&5i32.to_le_bytes()); // mode-2 kind 0 dword (packet +4)
        snap.currencies_2 = Some(body2);
        let entry_of =
            |want: StatusEntryKind| STATUS_ENTRIES.iter().position(|e| e.kind == want).unwrap();
        let cur1 = status_detail_lines(
            entry_of(StatusEntryKind::Currencies),
            &snap,
            &labels,
            &[],
            None,
        );
        assert_eq!(cur1.len(), 99);
        assert!(&cur1[0].contains("7777"));
        let cur2 = status_detail_lines(
            entry_of(StatusEntryKind::Currencies2),
            &snap,
            &[],
            &labels,
            None,
        );
        assert_eq!(cur2.len(), 97, "report 2 has its own descriptor count");
        assert!(&cur2[0].contains('5'), "bayld slot reads report 2");
        assert!(!&cur2[0].contains("7777"), "blocks never cross modes");
    }
}
