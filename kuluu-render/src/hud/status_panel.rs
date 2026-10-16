use bevy::prelude::*;

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
        enabled: true,
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

const PANEL_WIDTH_PX: f32 = 280.0;
const LEVEL_BOX_WIDTH_PX: f32 = 150.0;

/// Main menu Status code-moves `persona` to (16, 16)
/// (.agents/skills/retail-observe/references/2026-10-15-menu-window-stacking.md).
pub const PERSONA_STATUS_LEFT: f32 = 16.0;
pub const PERSONA_STATUS_TOP: f32 = 16.0;

/// Status code-moves `joblevel` to (130, 48), off its record frame
/// (.agents/skills/retail-observe/references/2026-10-15-menu-window-stacking.md).
pub const JOBLEVEL_WINDOW: &str = "joblevel";
pub const JOBLEVEL_STATUS_LEFT: f32 = 130.0;
pub const JOBLEVEL_STATUS_TOP: f32 = 48.0;

/// The `level` window opens with Status at its own record frame.
pub const STATUS_LEVEL_WINDOW: &str = "level";

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

#[derive(Component)]
pub struct StatusLevelText;

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

#[derive(Component)]
pub struct StatusHeaderRow;

#[derive(Component)]
pub struct StatusVitalsRow;

#[derive(Component)]
pub struct StatusItemLevelRow;

#[derive(Component)]
pub struct StatusAttrRow {
    pub attr_index: usize,
}

pub const PRIMARY_ATTRS: &[&str] = &["STR", "DEX", "VIT", "AGI", "INT", "MND", "CHR"];

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
                row_gap: Val::Px(6.0),
                display: Display::None,
                ..default()
            },
            bevy::picking::Pickable::IGNORE,
        ))
        .id();
    let panel = commands
        .spawn((
            StatusPanel,
            Node {
                width: Val::Px(PANEL_WIDTH_PX),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
        ))
        .id();
    commands.entity(stack).add_child(panel);
    commands.entity(stack).with_children(|s| {
        s.spawn((
            StatusLevelBox,
            Node {
                width: Val::Px(LEVEL_BOX_WIDTH_PX),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
        ))
        .with_children(|b| {
            b.spawn((
                StatusLevelText,
                Text::new(""),
                style::text_font(13.0),
                TextColor(theme::TEXT),
            ));
        });
    });
    commands.entity(panel).with_children(|p| {
        p.spawn((
            StatusHeaderRow,
            Text::new(""),
            style::text_font(14.0),
            TextColor(theme::TITLE),
        ));
        p.spawn((
            StatusVitalsRow,
            Text::new(""),
            style::text_font(13.0),
            TextColor(theme::TEXT),
        ));
        p.spawn((
            StatusItemLevelRow,
            Text::new(""),
            style::text_font(13.0),
            TextColor(theme::MUTED),
        ));
        for attr_index in 0..PRIMARY_ATTRS.len() {
            p.spawn((
                StatusAttrRow { attr_index },
                Text::new(""),
                style::text_font(13.0),
                TextColor(theme::TEXT),
            ));
        }
    });
}

/// The `level` box body: Level / EXP / NEXTLevel (labels follow the retail
/// window's on-screen words captured in the owner's screenshot; values from
/// s2c 0x061 — no wire value, no invented number).
pub fn level_box_lines(snap: &kuluu_snapshot::SceneSnapshot) -> String {
    let me = crate::snapshot::resolve_self(&snap.party, snap.self_char_id);
    let Some(lv) = me.map(|m| m.main_job_lv) else {
        return "Level —".to_string();
    };
    match snap.stats.as_ref() {
        Some(s) if s.exp_next != 0 => {
            format!("Level {lv}\nEXP {}\nNEXTLevel {}", s.exp_now, s.exp_next)
        }
        _ => format!("Level {lv}"),
    }
}

/// The `level` box sits at its own record frame while the install supplies one
/// (.agents/skills/retail-observe/references/2026-10-15-menu-window-stacking.md); without it
/// the box stays stacked under persona.
pub fn update_status_level_box(
    config_rows: Res<crate::hud::menu::ConfigDatRows>,
    mut level_box_q: Query<&mut Node, With<StatusLevelBox>>,
) {
    let frame = config_rows.record_frame(STATUS_LEVEL_WINDOW);
    for mut node in level_box_q.iter_mut() {
        let mut want = node.clone();
        match &frame {
            Some(frame) => crate::hud::menu::place_at_record_frame(&mut want, frame),
            None => {
                want.position_type = PositionType::Relative;
                want.left = Val::Auto;
                want.top = Val::Auto;
                want.right = Val::Auto;
                want.bottom = Val::Auto;
                want.margin = UiRect::ZERO;
                want.width = Val::Px(LEVEL_BOX_WIDTH_PX);
            }
        }
        node.set_if_neq(want);
    }
}

pub fn update_status_panel(
    mode: Res<crate::input_mode::InputMode>,
    state: Res<SceneState>,
    mut stack_q: Query<&mut Node, With<StatusLeftStack>>,
    mut level_text_q: Query<&mut Text, With<StatusLevelText>>,
    // Pairwise-disjoint per the cycle-57 B0001 law: every &mut Text query here
    // must exclude EVERY other marker in the system (StatusLevelText included —
    // schedule-time conflict detection killed the release client without it).
    mut header_q: Query<
        &mut Text,
        (
            With<StatusHeaderRow>,
            Without<StatusLevelText>,
            Without<StatusVitalsRow>,
            Without<StatusItemLevelRow>,
            Without<StatusAttrRow>,
        ),
    >,
    mut vitals_q: Query<
        &mut Text,
        (
            With<StatusVitalsRow>,
            Without<StatusHeaderRow>,
            Without<StatusLevelText>,
            Without<StatusItemLevelRow>,
            Without<StatusAttrRow>,
        ),
    >,
    mut ilvl_q: Query<
        &mut Text,
        (
            With<StatusItemLevelRow>,
            Without<StatusHeaderRow>,
            Without<StatusVitalsRow>,
            Without<StatusLevelText>,
            Without<StatusAttrRow>,
        ),
    >,
    mut attr_q: Query<
        (&StatusAttrRow, &mut Text),
        (
            Without<StatusHeaderRow>,
            Without<StatusVitalsRow>,
            Without<StatusItemLevelRow>,
            Without<StatusLevelText>,
        ),
    >,
) {
    let Ok(mut stack_node) = stack_q.single_mut() else {
        return;
    };
    let open = status_flow_open(&mode);
    if !open && stack_node.display != Display::None {
        stack_node.display = Display::None;
        return;
    }
    if open && stack_node.display == Display::None {
        stack_node.display = Display::Flex;
    }

    let snap = &state.snapshot;
    let me = crate::snapshot::resolve_self(&snap.party, snap.self_char_id);

    if let Ok(mut text) = header_q.single_mut() {
        let want = profile_header(snap, me);
        if **text != want {
            **text = want;
        }
    }

    if let Ok(mut text) = vitals_q.single_mut() {
        let want = match me {
            Some(m) => format!("HP {}   MP {}   TP {}", m.hp, m.mp, m.tp),
            None => "HP —   MP —   TP —".to_string(),
        };
        if **text != want {
            **text = want;
        }
    }

    if let Ok(mut text) = ilvl_q.single_mut() {
        let want = match snap.stats.as_ref().filter(|s| s.item_level > 0) {
            Some(s) => format!("Item Level: {}", s.item_level),
            None => "Item Level: —".to_string(),
        };
        if **text != want {
            **text = want;
        }
    }

    if let Ok(mut text) = level_text_q.single_mut() {
        let want = level_box_lines(snap);
        if **text != want {
            **text = want;
        }
    }

    for (row, mut text) in attr_q.iter_mut() {
        let Some(name) = PRIMARY_ATTRS.get(row.attr_index) else {
            continue;
        };
        let want = match snap.stats.as_ref() {
            Some(s) => {
                let base = attr_value(s, row.attr_index).unwrap_or(0);
                let bonus = s.bonus.get(row.attr_index).copied().unwrap_or(0);
                if bonus != 0 {
                    format!("{name:<4}{base:>4} {bonus:+}")
                } else {
                    format!("{name:<4}{base:>4}")
                }
            }
            None => format!("{name:<4}   —"),
        };
        if **text != want {
            **text = want;
        }
    }
}

fn profile_header(
    snap: &kuluu_snapshot::SceneSnapshot,
    me: Option<&kuluu_snapshot::PartyMember>,
) -> String {
    let name = me
        .and_then(|m| m.name.clone())
        .or_else(|| snap.char_name.clone())
        .unwrap_or_else(|| "—".to_string());
    match me {
        Some(m) => {
            let main = job_abbrev(m.main_job);
            if m.sub_job != 0 {
                let sub = job_abbrev(m.sub_job);
                format!("{name}  {main}{} / {sub}{}", m.main_job_lv, m.sub_job_lv)
            } else {
                format!("{name}  {main}{}", m.main_job_lv)
            }
        }
        None => name,
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
pub fn status_detail_lines(
    entry_idx: usize,
    snap: &kuluu_snapshot::SceneSnapshot,
    currency_labels_1: &[Option<String>],
    currency_labels_2: &[Option<String>],
) -> Vec<String> {
    let Some(entry) = STATUS_ENTRIES.get(entry_idx) else {
        return vec!["(no such status row)".to_string()];
    };
    let me = crate::snapshot::resolve_self(&snap.party, snap.self_char_id);
    match entry.kind {
        StatusEntryKind::Profile => {
            let mut lines = vec![profile_header(snap, me)];
            lines.push(match me {
                Some(m) => format!("HP {}   MP {}   TP {}", m.hp, m.mp, m.tp),
                None => "HP —   MP —   TP —".to_string(),
            });
            lines.push(match snap.stats.as_ref().filter(|s| s.item_level > 0) {
                Some(s) => format!("Item Level: {}", s.item_level),
                _ => "Item Level: —".to_string(),
            });
            for (attr_index, name) in PRIMARY_ATTRS.iter().enumerate() {
                lines.push(match snap.stats.as_ref() {
                    Some(s) => {
                        let base = attr_value(s, attr_index).unwrap_or(0);
                        let bonus = s.bonus.get(attr_index).copied().unwrap_or(0);
                        if bonus != 0 {
                            format!("{name:<4}{base:>4} {bonus:+}")
                        } else {
                            format!("{name:<4}{base:>4}")
                        }
                    }
                    None => format!("{name:<4}   —"),
                });
            }
            lines
        }
        StatusEntryKind::JobLevels => match snap.jobs.as_ref() {
            Some(list) => {
                let mut lines: Vec<String> = list
                    .job_levels
                    .iter()
                    .enumerate()
                    .filter(|(job_id, _)| *job_id != 0)
                    .filter(|(_, &level)| level > 0)
                    .map(|(job_id, &level)| {
                        format!("{:<16} {}", job_abbrev(job_id as u8), level)
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
                // Craft shows the level only: the rank half of the word indexes the
                // client's rank glyphs, which are DAT art rather than text.
                if let Some(row) = ffxi_proto::decode::CharSkillLevels::craft_row(word) {
                    visible.push((format!("{label} {}", row.level), row.capped));
                }
            }
        }
    }

    // Two columns means the entries pair up, left column first (retail walks its
    // array by column: `.text 0x1485FA` adds the 14px pitch only on every other
    // entry, and the height law counts ceil(n/2) rows).
    let columns = family.columns();
    if columns == 1 {
        return visible
            .into_iter()
            .map(|(text, capped)| SkillDetailRow { text, capped })
            .collect();
    }
    let rows = visible.len().div_ceil(columns);
    let left_width = visible
        .iter()
        .take(rows)
        .map(|(t, _)| t.chars().count())
        .max()
        .unwrap_or(0);
    (0..rows)
        .map(|row| {
            let left = &visible[row];
            let right = visible.get(row + rows);
            match right {
                Some((right_text, _)) => SkillDetailRow {
                    text: format!(
                        "{:<left_width$}  {}",
                        left.0,
                        right_text,
                        left_width = left_width + 2
                    ),
                    capped: left.1,
                },
                None => SkillDetailRow {
                    text: left.0.clone(),
                    capped: left.1,
                },
            }
        })
        .collect()
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
        // No entities: the system returns early, but the queries are already
        // registered — a B0001 conflict panics inside this same call.
        app.update();
    }

    /// The `level` box sits at its own record frame once the install supplies one and stays
    /// stacked under persona without it; persona keeps its Status spot either way.
    #[test]
    fn level_box_sits_at_its_record_frame_or_stacks_under_persona() {
        use bevy::app::App;
        let mut app = App::new();
        app.init_resource::<crate::hud::menu::ConfigDatRows>();
        app.add_systems(bevy::app::Startup, spawn_status_panel);
        app.add_systems(bevy::app::Update, update_status_level_box);
        app.update();

        let level_box = |app: &mut App| -> Node {
            app.world_mut()
                .query_filtered::<&Node, With<StatusLevelBox>>()
                .single(app.world())
                .expect("one level box")
                .clone()
        };
        let stacked = level_box(&mut app);
        assert_eq!(
            stacked.position_type,
            PositionType::Relative,
            "in flow under persona"
        );
        assert_eq!(stacked.width, Val::Px(LEVEL_BOX_WIDTH_PX));
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

        {
            let mut rows = app
                .world_mut()
                .resource_mut::<crate::hud::menu::ConfigDatRows>();
            rows.frames
                .insert(STATUS_LEVEL_WINDOW.into(), (16, 210, 112));
            rows.frame_extents.insert(
                STATUS_LEVEL_WINDOW.into(),
                (40, crate::hud::menu::FRAME_ANCHOR_NONE),
            );
        }
        app.update();
        let placed = level_box(&mut app);
        assert_eq!(placed.position_type, PositionType::Absolute);
        assert_eq!(
            (placed.left, placed.top, placed.width),
            (Val::Px(16.0), Val::Px(210.0), Val::Px(112.0)),
            "the record frame places it"
        );
    }

    /// The `level` box under the persona panel answers from s2c 0x061 EXP
    /// (vault main_menu.md §59 addendum 10 screenshot law): Level / EXP /
    /// NEXTLevel, no invented values when the stats block is absent.
    #[test]
    fn level_box_answers_from_the_wire_exp_pair() {
        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        assert_eq!(level_box_lines(&snap), "Level —", "no self yet");

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
        assert_eq!(level_box_lines(&snap), "Level 6", "no stats block yet");

        snap.stats = Some(kuluu_snapshot::CharStats {
            exp_now: 806,
            exp_next: 1750,
            ..Default::default()
        });
        assert_eq!(level_box_lines(&snap), "Level 6\nEXP 806\nNEXTLevel 1750");
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
    /// two columns means `ceil(n/2)` lines.
    #[test]
    fn noncraft_skill_window_applies_the_word_law_and_pairs_columns() {
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
        // Id 2 is the sentinel and drops out first, so three visible ids pair into
        // two lines with ids 3 and 4 landing as the right column.
        assert_eq!(rows.len(), 2);
        let left_of_line1 = rows[0]
            .text
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(left_of_line1, "Hand-to-Hand 301", "{}", rows[0].text);
        assert!(rows[0].text.contains(' '), "two columns on one line");

        // The capped word is drawn with its level and flagged for the blue paint.
        let capped = skill_detail_rows(&snap, SkillFamily::Noncraft(3..=4));
        assert_eq!(capped.len(), 1);
        assert!(capped[0].capped, "bit-15 marks a capped row");
        assert!(
            capped[0].text.contains("125") && capped[0].text.contains("88"),
            "both levels survive on one line: {}",
            capped[0].text
        );
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
        let fished = rows
            .iter()
            .find(|r| r.text.starts_with("Fishing"))
            .expect("id 48 is Fishing");
        assert!(
            fished.text.ends_with('0'),
            "an untrained craft shows its zero level: {}",
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
            status_row_enabled(
                StatusEntryKind::Unity,
                &kuluu_snapshot::SceneSnapshot::default()
            ),
            "row 11 is never disabled"
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
            assert!(status_detail_lines(entry, &snap, &[], &[]).is_empty());
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
        let cur1 = status_detail_lines(entry_of(StatusEntryKind::Currencies), &snap, &labels, &[]);
        assert_eq!(cur1.len(), 99);
        assert!(&cur1[0].contains("7777"));
        let cur2 = status_detail_lines(entry_of(StatusEntryKind::Currencies2), &snap, &[], &labels);
        assert_eq!(cur2.len(), 97, "report 2 has its own descriptor count");
        assert!(&cur2[0].contains('5'), "bayld slot reads report 2");
        assert!(!&cur2[0].contains("7777"), "blocks never cross modes");
    }
}
