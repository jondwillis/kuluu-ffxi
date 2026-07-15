use bevy::prelude::*;
use ffxi_viewer_wire::PartyMember;

use crate::hud::style::{self, theme};
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct SelfHudPanel;

#[derive(Component)]
pub struct SelfHpRow;

#[derive(Component)]
pub struct SelfMpRow;

#[derive(Component)]
pub struct SelfTpRow;

#[derive(Component)]
pub struct SelfStatusRow;

#[derive(Component)]
pub struct SelfPartyRow;

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
            crate::components::InGameEntity,
            SelfHudPanel,
            Node {
                position_type: PositionType::Absolute,

                bottom: Val::Px(28.0),
                right: Val::Px(8.0),
                width: Val::Px(PANEL_WIDTH_PX),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
        ))
        .with_children(|p| {
            spawn_row(p, SelfHpRow, "HP", "—");
            spawn_row(p, SelfMpRow, "MP", "—");
            spawn_row(p, SelfTpRow, "TP", "—");
            spawn_row(p, SelfStatusRow, "", "");
            spawn_row(p, SelfPartyRow, "", "Solo");
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
            style::text_font(13.0),
            TextColor(theme::MUTED),
        ));
        row.spawn((
            marker,
            Text::new(init.to_string()),
            style::text_font(13.0),
            TextColor(theme::TEXT),
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
                tc.0 = theme::MUTED;
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
                tc.0 = theme::MUTED;
            }
        }
    }
    if let Ok((mut text, mut tc)) = tp_q.single_mut() {
        match me {
            Some(m) => {
                **text = format!("{:>6}", m.tp);
                tc.0 = if m.tp >= 1000 {
                    Color::srgb(1.00, 0.55, 0.10)
                } else {
                    Color::srgb(0.80, 0.55, 0.20)
                };
            }
            None => {
                **text = "—".into();
                tc.0 = theme::MUTED;
            }
        }
    }
}

pub fn update_self_status(
    state: Res<SceneState>,
    time: Res<Time>,
    mut tracker: ResMut<SelfHealTracker>,
    mut row_q: Query<(&mut Text, &mut TextColor), With<SelfStatusRow>>,
) {
    let snap = &state.snapshot;
    let me = resolve_self(&snap.party, snap.self_char_id);

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
        tracker.pulse_remaining_s = (tracker.pulse_remaining_s - time.delta_secs()).max(0.0);
    }

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
        theme::MUTED
    };
    if tc.0 != want_color {
        tc.0 = want_color;
    }
}

pub fn update_self_party_indicator(
    state: Res<SceneState>,
    mut q: Query<&mut Text, With<SelfPartyRow>>,
) {
    if !state.dirty {
        return;
    }
    let Ok(mut text) = q.single_mut() else {
        return;
    };
    let n = state.snapshot.party.len();
    let want = if n <= 1 {
        "Solo".to_string()
    } else {
        format!("Party {n}/6")
    };
    if **text != want {
        **text = want;
    }
}

pub fn resolve_self(party: &[PartyMember], self_char_id: Option<u32>) -> Option<&PartyMember> {
    if let Some(id) = self_char_id {
        if let Some(m) = party.iter().find(|m| m.id == id) {
            return Some(m);
        }
    }
    party.first()
}

pub fn hp_color(pct: u8) -> Color {
    if pct >= 76 {
        theme::GOOD
    } else if pct >= 26 {
        theme::WARN
    } else {
        theme::DANGER
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
        assert_eq!(hp_color(100), theme::GOOD);
        assert_eq!(hp_color(76), theme::GOOD);
        assert_eq!(hp_color(75), theme::WARN);
        assert_eq!(hp_color(26), theme::WARN);
        assert_eq!(hp_color(25), theme::DANGER);
        assert_eq!(hp_color(0), theme::DANGER);
    }
}
