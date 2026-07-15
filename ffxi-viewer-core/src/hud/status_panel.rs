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
];

pub fn status_entry_count() -> usize {
    STATUS_ENTRIES.len()
}

const PANEL_WIDTH_PX: f32 = 280.0;

#[derive(Component)]
pub struct StatusPanel;

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
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
        ))
        .with_children(|p| {
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

pub(crate) fn job_abbrev(job_id: u8) -> String {
    if job_id == 0 {
        return "---".to_string();
    }
    // Canonical 3-letter code from LSB (2 → "MNK"); truncating "Monk" gives "MON".
    match ffxi_proto::job_names::abbrev(job_id as u16) {
        Some(code) => code.to_string(),
        None => format!("J{job_id}"),
    }
}

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
}
