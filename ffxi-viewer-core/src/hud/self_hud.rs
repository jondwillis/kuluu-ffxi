//! Self HP / MP / TP bars — bottom-left, above the chat panel.
//!
//! Sources HP/MP/TP from `snapshot.party`'s entry whose `id ==
//! snapshot.self_char_id`. Falls back to the snapshot's first party
//! member when `self_char_id` is None — that's almost always the
//! local player anyway, since soloing puts only-self in the party.
//!
//! Layout: three stacked bars with right-aligned numeric value, sized
//! to read at a glance from across the screen. HP color follows the
//! retail FFXI palette (green ≥76%, yellow 26–75%, red <26%); MP stays
//! cyan, TP stays orange and shows a chargeable / over-100 highlight.
//!
//! Why party data and not entity data: `Entity` only carries `hp_pct:
//! Option<u8>` (the visible bar everyone in the zone sees). The
//! party-attr update carries the full numeric `hp/mp/tp` only for
//! party members — including self — so it's the cheapest path to a
//! real number to display.

use bevy::prelude::*;
use ffxi_viewer_wire::PartyMember;

use crate::hud::palette;
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct SelfHudPanel;

#[derive(Component)]
pub struct SelfHpRow;

#[derive(Component)]
pub struct SelfMpRow;

#[derive(Component)]
pub struct SelfTpRow;

/// Combined combat-status / healing badge row beneath HP/MP/TP. Renders
/// "ENGAGED" while the player has a non-zero `bt_target_id`, and overlays
/// a brief "+N HP" pulse whenever the player's HP increases between
/// snapshots. Both signals live in one row so the panel doesn't grow
/// vertically when neither condition holds.
#[derive(Component)]
pub struct SelfStatusRow;

/// Tracks the most recently observed self HP and how long the heal pulse
/// has been visible. Reset by `update_self_status` each tick; the badge
/// shows "+N HP" for `HEAL_PULSE_SECS` after an increase, then fades.
#[derive(Resource, Default)]
pub struct SelfHealTracker {
    pub last_hp: Option<u32>,
    pub pulse_amount: u32,
    pub pulse_remaining_s: f32,
}

const HEAL_PULSE_SECS: f32 = 1.5;

const PANEL_WIDTH_PX: f32 = 220.0;

pub fn spawn_self_hud(mut commands: Commands) {
    commands
        .spawn((
            SelfHudPanel,
            Node {
                position_type: PositionType::Absolute,
                // Bottom-right corner, just above the diagnostics strip
                // (28 px tall, anchored at bottom: 0). Retail FFXI puts
                // the player's HP/MP/TP bars in this quadrant; the
                // chat log lives bottom-left, so the two never overlap
                // even at narrow window widths.
                bottom: Val::Px(28.0),
                right: Val::Px(8.0),
                width: Val::Px(PANEL_WIDTH_PX),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::BORDER),
        ))
        .with_children(|p| {
            spawn_row(p, SelfHpRow, "HP", "—");
            spawn_row(p, SelfMpRow, "MP", "—");
            spawn_row(p, SelfTpRow, "TP", "—");
            spawn_row(p, SelfStatusRow, "", "");
        });
}

fn spawn_row<M: Component>(p: &mut ChildSpawnerCommands, marker: M, label: &str, init: &str) {
    p.spawn(Node {
        flex_direction: FlexDirection::Row,
        column_gap: Val::Px(8.0),
        ..default()
    })
    .with_children(|row| {
        row.spawn((
            Text::new(label.to_string()),
            TextFont {
                font_size: 13.0,
                ..default()
            },
            TextColor(palette::MUTED),
        ));
        row.spawn((
            marker,
            Text::new(init.to_string()),
            TextFont {
                font_size: 13.0,
                ..default()
            },
            TextColor(palette::TEXT),
        ));
    });
}

pub fn update_self_hud(
    state: Res<SceneState>,
    mut hp_q: Query<
        (&mut Text, &mut TextColor),
        (With<SelfHpRow>, Without<SelfMpRow>, Without<SelfTpRow>),
    >,
    mut mp_q: Query<
        (&mut Text, &mut TextColor),
        (With<SelfMpRow>, Without<SelfHpRow>, Without<SelfTpRow>),
    >,
    mut tp_q: Query<
        (&mut Text, &mut TextColor),
        (With<SelfTpRow>, Without<SelfHpRow>, Without<SelfMpRow>),
    >,
) {
    if !state.dirty {
        return;
    }
    let snap = &state.snapshot;
    let me = resolve_self(&snap.party, snap.self_char_id);

    if let Ok((mut text, mut tc)) = hp_q.single_mut() {
        match me {
            Some(m) => {
                **text = format!("{:>6}  ({:>3}%)", m.hp, m.hp_pct);
                tc.0 = hp_color(m.hp_pct);
            }
            None => {
                **text = "—".into();
                tc.0 = palette::MUTED;
            }
        }
    }
    if let Ok((mut text, mut tc)) = mp_q.single_mut() {
        match me {
            Some(m) => {
                **text = format!("{:>6}  ({:>3}%)", m.mp, m.mp_pct);
                tc.0 = Color::srgb(0.40, 0.85, 1.00);
            }
            None => {
                **text = "—".into();
                tc.0 = palette::MUTED;
            }
        }
    }
    if let Ok((mut text, mut tc)) = tp_q.single_mut() {
        match me {
            Some(m) => {
                // TP is 0..3000 (FFXI uses 1000 = ready to weaponskill,
                // 3000 = absolute max with merits/merits). Highlight at
                // ≥1000 so the operator sees "I can WS".
                **text = format!("{:>6}", m.tp);
                tc.0 = if m.tp >= 1000 {
                    Color::srgb(1.00, 0.55, 0.10)
                } else {
                    Color::srgb(0.80, 0.55, 0.20)
                };
            }
            None => {
                **text = "—".into();
                tc.0 = palette::MUTED;
            }
        }
    }
}

/// Per-frame: detect HP gain on self, drive the engaged + heal badge.
///
/// Two signals share one text row:
///   - Engaged: self entity has `bt_target_id != 0`. Renders "ENGAGED"
///     in red. Read from the snapshot directly each tick so the badge
///     clears the moment the server disengages us.
///   - Heal: party-row HP increased between snapshots. The delta is
///     latched into the tracker, shown as "+N HP" in green for
///     `HEAL_PULSE_SECS` then cleared. A simultaneous engage + heal
///     concatenates both in the row ("ENGAGED  +50 HP").
///
/// Tracker initialisation: first observation seeds `last_hp` without
/// firing a pulse — otherwise the pulse would mis-fire on zone-in when
/// the prior `last_hp` is `None`.
pub fn update_self_status(
    state: Res<SceneState>,
    time: Res<Time>,
    mut tracker: ResMut<SelfHealTracker>,
    mut row_q: Query<(&mut Text, &mut TextColor), With<SelfStatusRow>>,
) {
    let snap = &state.snapshot;
    let me = resolve_self(&snap.party, snap.self_char_id);

    // HP-delta tracking. Only fire a pulse on a strict increase; equal
    // HP (e.g. attr-only update with no change) is a no-op.
    if let Some(m) = me {
        match tracker.last_hp {
            Some(prev) if m.hp > prev => {
                tracker.pulse_amount = m.hp - prev;
                tracker.pulse_remaining_s = HEAL_PULSE_SECS;
            }
            _ => {}
        }
        tracker.last_hp = Some(m.hp);
    }
    if tracker.pulse_remaining_s > 0.0 {
        tracker.pulse_remaining_s =
            (tracker.pulse_remaining_s - time.delta_secs()).max(0.0);
    }

    // Engaged check: self entity in the entity list (party-row data
    // doesn't carry `bt_target_id`).
    let engaged = match snap.self_char_id {
        Some(id) => snap
            .entities
            .iter()
            .any(|e| e.id == id && e.bt_target_id != 0),
        None => false,
    };

    let Ok((mut text, mut tc)) = row_q.single_mut() else {
        return;
    };

    let pulse_active = tracker.pulse_remaining_s > 0.0;
    let want_text = match (engaged, pulse_active) {
        (true, true) => format!("ENGAGED  +{} HP", tracker.pulse_amount),
        (true, false) => "ENGAGED".to_string(),
        (false, true) => format!("+{} HP", tracker.pulse_amount),
        (false, false) => String::new(),
    };
    if **text != want_text {
        **text = want_text;
    }
    let want_color = if engaged && !pulse_active {
        Color::srgb(1.00, 0.25, 0.30)
    } else if pulse_active {
        Color::srgb(0.30, 1.00, 0.45)
    } else {
        palette::MUTED
    };
    if tc.0 != want_color {
        tc.0 = want_color;
    }
}

/// Resolve the operator's own party row. Prefer `self_char_id` lookup
/// for correctness; fall back to the first member when the id hasn't
/// been resolved yet so the HUD shows *something* during the post-zone
/// race window where party data has arrived but `self_char_id` is
/// still `None`.
pub fn resolve_self<'a>(
    party: &'a [PartyMember],
    self_char_id: Option<u32>,
) -> Option<&'a PartyMember> {
    if let Some(id) = self_char_id {
        if let Some(m) = party.iter().find(|m| m.id == id) {
            return Some(m);
        }
    }
    party.first()
}

/// HP color band matching retail FFXI: green ≥76%, yellow 26–75%, red <26%.
pub fn hp_color(pct: u8) -> Color {
    if pct >= 76 {
        palette::STAGE_GOOD
    } else if pct >= 26 {
        palette::STAGE_TRANSITIONING
    } else {
        palette::STAGE_BAD
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pm(id: u32, hp: u32, hp_pct: u8) -> PartyMember {
        PartyMember {
            id,
            act_index: id as u16,
            name: Some("X".into()),
            hp,
            mp: 0,
            tp: 0,
            hp_pct,
            mp_pct: 0,
            zone_no: 0,
            main_job: 0,
            main_job_lv: 0,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: false,
            is_alliance_leader: false,
            in_mog_house: false,
        }
    }

    #[test]
    fn resolve_self_uses_self_char_id_when_present() {
        let party = vec![pm(1, 100, 100), pm(42, 500, 80)];
        let me = resolve_self(&party, Some(42));
        assert_eq!(me.unwrap().hp, 500);
    }

    #[test]
    fn resolve_self_falls_back_to_first_when_id_unknown() {
        let party = vec![pm(1, 100, 100), pm(42, 500, 80)];
        let me = resolve_self(&party, None);
        assert_eq!(me.unwrap().hp, 100);
    }

    #[test]
    fn resolve_self_falls_back_to_first_when_id_not_in_party() {
        let party = vec![pm(1, 100, 100), pm(42, 500, 80)];
        let me = resolve_self(&party, Some(999));
        assert_eq!(me.unwrap().hp, 100);
    }

    #[test]
    fn resolve_self_returns_none_for_empty_party() {
        let party: Vec<PartyMember> = vec![];
        assert!(resolve_self(&party, Some(42)).is_none());
    }

    #[test]
    fn hp_color_bands() {
        assert_eq!(hp_color(100), palette::STAGE_GOOD);
        assert_eq!(hp_color(76), palette::STAGE_GOOD);
        assert_eq!(hp_color(75), palette::STAGE_TRANSITIONING);
        assert_eq!(hp_color(26), palette::STAGE_TRANSITIONING);
        assert_eq!(hp_color(25), palette::STAGE_BAD);
        assert_eq!(hp_color(0), palette::STAGE_BAD);
    }
}
