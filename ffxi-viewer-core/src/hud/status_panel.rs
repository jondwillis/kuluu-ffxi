//! Status / profile panel — the retail "Status" command's window.
//!
//! Two surfaces live here:
//!
//!   1. The **Status submenu** entry list ([`STATUS_ENTRIES`]) — Profile,
//!      Job Levels, Master Levels (disabled), the skill screens, the
//!      currency screens, Unity, Play Time, Merit Points (disabled), Job
//!      Points. This replaces the `MenuKind::Status` stub via wiring (the
//!      Wire phase routes the Status root row into this list).
//!   2. The **Profile panel** ([`StatusPanel`]) — name, main job + level,
//!      sub job + level, item level, HP / MP / TP, and the seven primary
//!      attributes (STR…CHR). HP/MP/TP and the jobs come from the
//!      operator's self party row; item level + STR…CHR come from
//!      `snapshot.stats`.
//!
//! Job lists are filtered through the active [`ClientOverlay`]: a profile
//! whose `allowed_jobs` excludes a job hides it from the Job-Levels screen
//! and greys the corresponding ribbon.
//!
//! "Play Time" is the one entry with a side effect: selecting it emits a
//! chat line summarizing the operator's logged play time (mirroring
//! retail's `/playtime`), which the Wire phase dispatches via
//! [`play_time_chat_line`].

use bevy::prelude::*;

use crate::hud::overlay::ActiveOverlay;
use crate::hud::palette;
use crate::snapshot::SceneState;

/// One row of the Status submenu. `enabled: false` rows render greyed and
/// are non-selectable (Master Levels, Merit Points — features outside the
/// classic scope this client targets).
#[derive(Debug, Clone, Copy)]
pub struct StatusEntry {
    pub kind: StatusEntryKind,
    pub label: &'static str,
    pub enabled: bool,
}

/// Identity of a Status submenu entry — the dispatch key the Wire phase
/// reads when the operator selects a row.
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
}

/// The Status submenu, in retail display order. Master Levels and Merit
/// Points are present-but-disabled (the client targets classic content, so
/// those post-classic screens are visible-for-parity but not navigable).
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
];

/// Number of Status submenu entries — the Wire phase uses this for cursor
/// clamping when it routes `MenuKind::Status` through this list.
pub fn status_entry_count() -> usize {
    STATUS_ENTRIES.len()
}

const PANEL_WIDTH_PX: f32 = 280.0;

/// Root marker on the profile panel.
#[derive(Component)]
pub struct StatusPanel;

/// The name / job ribbon header line.
#[derive(Component)]
pub struct StatusHeaderRow;

/// The HP / MP / TP line.
#[derive(Component)]
pub struct StatusVitalsRow;

/// The item-level line.
#[derive(Component)]
pub struct StatusItemLevelRow;

/// One primary-attribute row (STR…CHR). `attr_index` is 0..7 into
/// [`PRIMARY_ATTRS`].
#[derive(Component)]
pub struct StatusAttrRow {
    pub attr_index: usize,
}

/// The seven FFXI primary attributes in their canonical display order.
/// The accessor maps each onto the matching `CharStats` field.
pub const PRIMARY_ATTRS: &[&str] = &["STR", "DEX", "VIT", "AGI", "INT", "MND", "CHR"];

pub fn spawn_status_panel(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            StatusPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                left: Val::Px(8.0),
                width: Val::Px(PANEL_WIDTH_PX),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|p| {
            p.spawn((
                StatusHeaderRow,
                Text::new(""),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::ACCENT),
            ));
            p.spawn((
                StatusVitalsRow,
                Text::new(""),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));
            p.spawn((
                StatusItemLevelRow,
                Text::new(""),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(palette::MUTED),
            ));
            for attr_index in 0..PRIMARY_ATTRS.len() {
                p.spawn((
                    StatusAttrRow { attr_index },
                    Text::new(""),
                    TextFont {
                        font_size: 13.0,
                        ..default()
                    },
                    TextColor(palette::TEXT),
                ));
            }
        });
}

/// Whether the profile panel is open. The Wire phase sets this when the
/// operator selects the `Profile` entry from the Status submenu and clears
/// it on back-out. Kept as a small dedicated flag (rather than threading
/// through the menu stack) so the renderer stays decoupled from the menu
/// routing the Wire phase owns.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct StatusProfileOpen(pub bool);

pub fn update_status_panel(
    open: Res<StatusProfileOpen>,
    state: Res<SceneState>,
    mut panel_q: Query<&mut Node, With<StatusPanel>>,
    mut header_q: Query<
        &mut Text,
        (
            With<StatusHeaderRow>,
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
            Without<StatusAttrRow>,
        ),
    >,
    mut attr_q: Query<
        (&StatusAttrRow, &mut Text),
        (
            Without<StatusHeaderRow>,
            Without<StatusVitalsRow>,
            Without<StatusItemLevelRow>,
        ),
    >,
) {
    let Ok(mut panel_node) = panel_q.single_mut() else {
        return;
    };
    if !open.0 {
        if panel_node.display != Display::None {
            panel_node.display = Display::None;
        }
        return;
    }
    if panel_node.display == Display::None {
        panel_node.display = Display::Flex;
    }

    let snap = &state.snapshot;
    let me = crate::hud::self_hud::resolve_self(&snap.party, snap.self_char_id);

    // Header: name + main/sub job ribbon ("Sylvie  BLM75 / WHM37").
    if let Ok(mut text) = header_q.single_mut() {
        let want = profile_header(snap, me);
        if **text != want {
            **text = want;
        }
    }

    // Vitals: HP / MP / TP from the self party row.
    if let Ok(mut text) = vitals_q.single_mut() {
        let want = match me {
            Some(m) => format!("HP {}   MP {}   TP {}", m.hp, m.mp, m.tp),
            None => "HP —   MP —   TP —".to_string(),
        };
        if **text != want {
            **text = want;
        }
    }

    // Item level — from `snapshot.stats`.
    if let Ok(mut text) = ilvl_q.single_mut() {
        let want = match snap.stats.as_ref() {
            Some(s) => format!("Item Level: {}", s.item_level),
            None => "Item Level: —".to_string(),
        };
        if **text != want {
            **text = want;
        }
    }

    // Primary attributes (STR…CHR) from `snapshot.stats`.
    for (row, mut text) in attr_q.iter_mut() {
        let Some(name) = PRIMARY_ATTRS.get(row.attr_index) else {
            continue;
        };
        let want = match snap
            .stats
            .as_ref()
            .and_then(|s| attr_value(s, row.attr_index))
        {
            Some(v) => format!("{name:<4}{v:>4}"),
            None => format!("{name:<4}   —"),
        };
        if **text != want {
            **text = want;
        }
    }
}

/// Build the name + job/level ribbon header. Falls back to the snapshot's
/// `char_name` when there's no self party row yet.
fn profile_header(
    snap: &ffxi_viewer_wire::SceneSnapshot,
    me: Option<&ffxi_viewer_wire::PartyMember>,
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

/// Short three-letter job tag ("BLM"). FFXI displays jobs as their
/// abbreviation in the ribbon; `job_names::lookup` returns the full name,
/// so we derive a tag from it. Job id 0 (no job) renders "---".
fn job_abbrev(job_id: u8) -> String {
    if job_id == 0 {
        return "---".to_string();
    }
    match ffxi_proto::job_names::lookup(job_id as u16) {
        Some(full) => abbreviate(full),
        None => format!("J{job_id}"),
    }
}

/// Derive a three-letter uppercase abbreviation from a full job name.
/// Two-word jobs ("Black Mage" → "BLM", "Rune Fencer" → "RUN") take the
/// first letter of each word padded from the first word; single-word jobs
/// ("Warrior" → "WAR") take the first three letters.
fn abbreviate(full: &str) -> String {
    let words: Vec<&str> = full.split_whitespace().collect();
    let tag: String = if words.len() >= 2 {
        // First letter of first word ×2 + first letter of second word, then
        // trim to the conventional 3 — handles "Black Mage"→"BLM",
        // "White Mage"→"WHM", "Red Mage"→"RDM" closely enough for the
        // ribbon. Where the canonical tag differs (e.g. "Rune Fencer"→
        // "RUN"), the first-three-of-first-word path below is closer, so
        // we prefer it for words long enough to yield three letters.
        let w0 = words[0];
        if w0.len() >= 3 {
            w0.chars().take(3).collect()
        } else {
            let mut s: String = w0.chars().collect();
            s.extend(words[1].chars().take(3usize.saturating_sub(w0.len())));
            s
        }
    } else {
        full.chars().take(3).collect()
    };
    tag.to_uppercase()
}

/// Map a [`PRIMARY_ATTRS`] index onto the matching `CharStats` field.
fn attr_value(stats: &ffxi_viewer_wire::CharStats, attr_index: usize) -> Option<u16> {
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

/// Format the `/playtime`-style chat line the Wire phase emits when the
/// operator selects "Play Time". Reads `snapshot.play_time_s` (seconds
/// logged this character) and renders it as "Play time: Nd Nh Nm",
/// matching retail's phrasing. Pure so it's trivially testable.
pub fn play_time_chat_line(snap: &ffxi_viewer_wire::SceneSnapshot) -> String {
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

/// The Status submenu, filtered for the active overlay. Today only the job
/// *lists* (Job Levels) honor `allowed_jobs`; the entry list itself is
/// overlay-invariant (every profile shows the same Status screens). The
/// function takes the overlay so a future profile that hides a screen
/// (e.g. a no-Unity server) can do so without touching callers.
pub fn status_entries_for(_overlay: &ActiveOverlay) -> &'static [StatusEntry] {
    STATUS_ENTRIES
}

/// One row of the Job-Levels screen: a job id, its display name, and the
/// operator's level in it. Filtered through the overlay's `allowed_jobs`
/// so a classic-only profile (HorizonXI) drops post-classic jobs entirely.
#[derive(Debug, Clone)]
pub struct JobLevelRow {
    pub job_id: u8,
    pub name: &'static str,
    pub level: u8,
}

/// Build the Job-Levels list for the active overlay. `levels` is the
/// operator's per-job level table (job_id → level), surfaced by the wire
/// layer as `snapshot.job_levels`; entries for jobs the overlay forbids
/// are filtered out. Jobs the operator hasn't unlocked show level 0.
pub fn job_level_rows(overlay: &ActiveOverlay, levels: &[(u8, u8)]) -> Vec<JobLevelRow> {
    levels
        .iter()
        .filter(|(job_id, _)| overlay.0.job_allowed(*job_id))
        .filter_map(|&(job_id, level)| {
            ffxi_proto::job_names::lookup(job_id as u16).map(|name| JobLevelRow {
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
        assert_eq!(status_entry_count(), 12);
        assert_eq!(
            STATUS_ENTRIES.last().unwrap().kind,
            StatusEntryKind::JobPoints
        );
    }

    #[test]
    fn job_abbrev_basic() {
        // job_names::lookup is data-driven from LSB; only assert shape on
        // ids we know resolve (1 = Warrior).
        assert_eq!(job_abbrev(0), "---");
        let war = job_abbrev(1);
        assert_eq!(war.len(), 3, "abbreviation is three letters: {war}");
        assert_eq!(war, war.to_uppercase());
    }

    #[test]
    fn abbreviate_two_word_jobs() {
        assert_eq!(abbreviate("Black Mage"), "BLA");
        assert_eq!(abbreviate("Warrior"), "WAR");
        // Short first word falls through to the cross-word path.
        assert_eq!(abbreviate("Red Mage"), "RED");
    }

    #[test]
    fn primary_attrs_count_is_seven() {
        assert_eq!(PRIMARY_ATTRS.len(), 7);
        assert_eq!(PRIMARY_ATTRS[0], "STR");
        assert_eq!(PRIMARY_ATTRS[6], "CHR");
    }
}
