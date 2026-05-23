//! Right-anchored party roster HUD panel.
//!
//! One row per `PartyMember`: name, HP/MP/TP bars, and the main job /
//! sub job number pair (`MainLv/SubLv` — string job names land in a
//! later stage). Source: `SceneState.snapshot.party`.
//!
//! Strategy: rebuild the row tree whenever the party shape changes
//! (length differs, or any id at any position differs). Otherwise just
//! refresh text + bar widths in place. This keeps the steady-state cost
//! flat while handling member joins/leaves correctly.
//!
//! Visual chrome mirrors `chat_panel`: cyan-bordered (`palette::BORDER`)
//! dark-gray panel, 13px text, muted labels. Top-right anchor at 280px
//! wide, auto height.
//!
//! HP/MP/TP bars are 100px wide background tracks with a coloured fill
//! whose width scales by percentage. HP/MP use the snapshot's `_pct`
//! fields directly; TP uses `tp.min(3000) / 3000` (server reports 0-3000
//! with 1000 unlocks WS).
//!
//! `PartyMember.main_job` is a `u8` (LSB JOBTYPE enum). For 0e we just
//! show the number; mapping to "WAR/MNK/THF" can come later.

use bevy::prelude::*;
use ffxi_viewer_wire::PartyMember;

use crate::hud::palette;
use crate::snapshot::SceneState;

/// Marker for the panel root node.
#[derive(Component)]
pub struct RosterPanel;

/// Marker for the per-member row container. Holds the wire `id` so we
/// can detect "shape changed" cheaply (compare to the snapshot list).
#[derive(Component)]
pub struct RosterRow {
    pub member_id: u32,
}

/// Marker on the row's name + job text node so the per-frame update
/// pass can find it without iterating all descendants.
#[derive(Component)]
pub struct RosterRowHeader;

/// Marker on a bar fill rect. Stores which stat it tracks so a single
/// update pass can branch on it.
#[derive(Component)]
pub struct RosterBarFill {
    pub stat: BarStat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarStat {
    Hp,
    Mp,
    Tp,
}

const BAR_TRACK_WIDTH_PX: f32 = 100.0;
const BAR_HEIGHT_PX: f32 = 5.0;

/// HP color = green, MP = blue, TP = pale cyan/yellow split. Matches
/// the LSB convention well enough for an at-a-glance read.
fn bar_color(stat: BarStat) -> Color {
    match stat {
        BarStat::Hp => Color::srgb(0.30, 0.85, 0.30),
        BarStat::Mp => Color::srgb(0.40, 0.55, 1.00),
        BarStat::Tp => Color::srgb(1.00, 0.85, 0.30),
    }
}

/// Spawn the empty panel. Rows land in [`update_roster_panel_system`]
/// when the first snapshot with party data arrives.
pub fn spawn_roster_panel(mut commands: Commands) {
    commands.spawn((
        crate::components::InGameEntity,
        RosterPanel,
        Node {
            position_type: PositionType::Absolute,
            // Top-right column, slot 4: under the LLM badge. Retail
            // FFXI puts the party list mid-right; this top: 200
            // anchor approximates that while keeping the column
            // sequence (compass / clock / llm-badge / roster)
            // collision-free. See `hud/vana_clock.rs` for the
            // canonical column layout.
            top: Val::Px(200.0),
            right: Val::Px(8.0),
            width: Val::Px(280.0),
            padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
            border: UiRect::all(Val::Px(1.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(6.0),
            ..default()
        },
        BackgroundColor(palette::BACKGROUND),
        BorderColor::all(palette::BORDER),
    ));
}

/// Per-frame: rebuild row tree on shape changes, otherwise refresh in place.
pub fn update_roster_panel_system(
    state: Res<SceneState>,
    panel_q: Query<Entity, With<RosterPanel>>,
    rows_q: Query<(Entity, &RosterRow, &Children)>,
    children_q: Query<&Children>,
    header_q: Query<&RosterRowHeader>,
    mut text_q: Query<&mut Text>,
    mut node_q: Query<&mut Node>,
    bar_q: Query<&RosterBarFill>,
    mut commands: Commands,
) {
    let Ok(panel) = panel_q.single() else {
        return;
    };

    let party = &state.snapshot.party;

    // Decide whether shape changed: compare the ordered (id) list with
    // the current row set. RosterRow markers carry their member_id so we
    // can compare without walking children.
    let mut existing_rows: Vec<(Entity, u32)> = rows_q
        .iter()
        .map(|(e, row, _)| (e, row.member_id))
        .collect();

    let shape_changed = {
        if existing_rows.len() != party.len() {
            true
        } else {
            // Order doesn't matter — match by id.
            existing_rows.sort_by_key(|(_, id)| *id);
            let mut want: Vec<u32> = party.iter().map(|m| m.id).collect();
            want.sort();
            existing_rows
                .iter()
                .zip(want.iter())
                .any(|((_, a), b)| a != b)
        }
    };

    if shape_changed {
        // Despawn old rows, spawn fresh ones. Cheap because party size is
        // bounded (alliance cap = 18) and rebuilds are rare.
        for (e, _) in &existing_rows {
            commands.entity(*e).despawn();
        }
        commands.entity(panel).with_children(|p| {
            for member in party {
                spawn_member_row(p, member);
            }
        });
        return;
    }

    // In-place refresh: text + bar widths.
    for (_, row, row_children) in &rows_q {
        let Some(member) = party.iter().find(|m| m.id == row.member_id) else {
            continue;
        };

        for child in row_children.iter() {
            if header_q.get(child).is_ok() {
                if let Ok(mut text) = text_q.get_mut(child) {
                    let new = format_header(member);
                    if **text != new {
                        **text = new;
                    }
                }
                continue;
            }

            // Walk one level deeper for bar tracks → bar fills.
            if let Ok(track_children) = children_q.get(child) {
                for fill_e in track_children.iter() {
                    if let Ok(bar) = bar_q.get(fill_e) {
                        let pct = stat_pct(member, bar.stat);
                        if let Ok(mut node) = node_q.get_mut(fill_e) {
                            node.width = Val::Px(BAR_TRACK_WIDTH_PX * pct);
                        }
                    }
                }
            }
        }
    }
}

fn spawn_member_row(parent: &mut ChildSpawnerCommands, member: &PartyMember) {
    parent
        .spawn((
            RosterRow {
                member_id: member.id,
            },
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            },
        ))
        .with_children(|row| {
            // Header line: "Name  WAR75/MNK37".
            row.spawn((
                RosterRowHeader,
                Text::new(format_header(member)),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));

            spawn_bar(row, member, BarStat::Hp);
            spawn_bar(row, member, BarStat::Mp);
            spawn_bar(row, member, BarStat::Tp);
        });
}

fn spawn_bar(row: &mut ChildSpawnerCommands, member: &PartyMember, stat: BarStat) {
    let pct = stat_pct(member, stat);
    row.spawn((
        Node {
            width: Val::Px(BAR_TRACK_WIDTH_PX),
            height: Val::Px(BAR_HEIGHT_PX),
            ..default()
        },
        BackgroundColor(palette::DARK),
    ))
    .with_children(|track| {
        track.spawn((
            RosterBarFill { stat },
            Node {
                width: Val::Px(BAR_TRACK_WIDTH_PX * pct),
                height: Val::Px(BAR_HEIGHT_PX),
                ..default()
            },
            BackgroundColor(bar_color(stat)),
        ));
    });
}

fn stat_pct(member: &PartyMember, stat: BarStat) -> f32 {
    let pct = match stat {
        BarStat::Hp => member.hp_pct as f32 / 100.0,
        BarStat::Mp => member.mp_pct as f32 / 100.0,
        // TP runs 0..3000; the WS unlock is 1000 but the bar shows the
        // full range so the operator can see TP saving for SC/skillchain.
        BarStat::Tp => (member.tp as f32 / 3000.0).min(1.0),
    };
    pct.clamp(0.0, 1.0)
}

/// LSB `JOBTYPE` enum (`vendor/server/src/map/lua/lualib/luautils.cpp`
/// and `src/common/jobs.h`) maps `u8` → vanilla job abbreviation. Index
/// 0 is unset / sub-not-chosen and renders as the em-dash. The post-30
/// extension jobs (SCH/GEO/RUN) and the 60-cap MoP/RoV adds are
/// included so a level-99 character with any sub renders correctly.
///
/// Out-of-range ids return `"???"` so we don't panic when the server
/// surfaces a value we haven't catalogued yet.
pub fn job_abbr(job_id: u8) -> &'static str {
    match job_id {
        0 => "—",
        1 => "WAR",
        2 => "MNK",
        3 => "WHM",
        4 => "BLM",
        5 => "RDM",
        6 => "THF",
        7 => "PLD",
        8 => "DRK",
        9 => "BST",
        10 => "BRD",
        11 => "RNG",
        12 => "SAM",
        13 => "NIN",
        14 => "DRG",
        15 => "SMN",
        16 => "BLU",
        17 => "COR",
        18 => "PUP",
        19 => "DNC",
        20 => "SCH",
        21 => "GEO",
        22 => "RUN",
        _ => "???",
    }
}

fn format_header(member: &PartyMember) -> String {
    let name = member.name.as_deref().unwrap_or("?");
    // "Name MainAbbrLv/SubAbbrLv" — e.g. "Sylvie SAM75/NIN37". Sub
    // with no job set (sub_job == 0) collapses to the em-dash, matching
    // retail's blank sub display.
    let main = job_abbr(member.main_job);
    let sub_part = if member.sub_job == 0 {
        "—".to_string()
    } else {
        format!("{}{}", job_abbr(member.sub_job), member.sub_job_lv)
    };
    format!("{name}  {}{}/{}", main, member.main_job_lv, sub_part)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pm(id: u32, hp_pct: u8, mp_pct: u8, tp: u32) -> PartyMember {
        PartyMember {
            id,
            act_index: 0,
            name: Some(format!("p{id}")),
            hp: 1000,
            mp: 100,
            tp,
            hp_pct,
            mp_pct,
            zone_no: 0,
            main_job: 1,
            main_job_lv: 75,
            sub_job: 6,
            sub_job_lv: 37,
            is_party_leader: false,
            is_alliance_leader: false,
            in_mog_house: false,
        }
    }

    #[test]
    fn stat_pct_clamps_and_scales() {
        let m = pm(1, 50, 25, 1500);
        assert!((stat_pct(&m, BarStat::Hp) - 0.5).abs() < 1e-4);
        assert!((stat_pct(&m, BarStat::Mp) - 0.25).abs() < 1e-4);
        assert!((stat_pct(&m, BarStat::Tp) - 0.5).abs() < 1e-4);

        // TP > 3000 should clamp.
        let m = pm(1, 0, 0, 4000);
        assert_eq!(stat_pct(&m, BarStat::Tp), 1.0);
    }

    #[test]
    fn header_format_includes_jobs() {
        // pm() sets main_job=1 (WAR) lv 75, sub_job=6 (THF) lv 37.
        let m = pm(1, 100, 100, 0);
        assert_eq!(format_header(&m), "p1  WAR75/THF37");
    }

    #[test]
    fn header_no_sub_when_sub_job_is_zero() {
        let mut m = pm(2, 100, 100, 0);
        m.sub_job = 0;
        m.sub_job_lv = 0;
        assert_eq!(format_header(&m), "p2  WAR75/—");
    }

    #[test]
    fn job_abbr_covers_post_extension_jobs() {
        assert_eq!(job_abbr(20), "SCH");
        assert_eq!(job_abbr(21), "GEO");
        assert_eq!(job_abbr(22), "RUN");
        assert_eq!(job_abbr(0), "—");
        assert_eq!(job_abbr(99), "???");
    }
}
