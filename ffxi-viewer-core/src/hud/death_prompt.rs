use bevy::prelude::*;

use crate::hud::palette;
use crate::hud::self_hud::resolve_self;
use crate::input_method::InputMethod;
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct DeathPromptPanel;

#[derive(Component)]
pub struct DeathPromptHint;

#[derive(Component)]
pub struct DeathCountdownText;

fn hint_text(method: InputMethod) -> &'static str {
    match method {
        InputMethod::KeyboardMouse => "Press [Enter] to return to your home point.",
        InputMethod::Gamepad => "Press [A] to return to your home point.",
    }
}

const PANEL_WIDTH_PX: f32 = 380.0;

pub fn spawn_death_prompt(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            DeathPromptPanel,
            Node {
                position_type: PositionType::Absolute,

                top: Val::Percent(35.0),
                left: Val::Percent(50.0),
                margin: UiRect {
                    left: Val::Px(-PANEL_WIDTH_PX / 2.0),
                    ..default()
                },
                width: Val::Px(PANEL_WIDTH_PX),
                padding: UiRect::axes(Val::Px(14.0), Val::Px(10.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::STAGE_BAD),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("You were defeated."),
                TextFont {
                    font_size: 16.0,
                    ..default()
                },
                TextColor(palette::STAGE_BAD),
            ));
            p.spawn((
                DeathPromptHint,
                Text::new(hint_text(InputMethod::default())),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));
            p.spawn((
                DeathCountdownText,
                Text::new(String::new()),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(palette::STAGE_BAD),
            ));
        });
}

fn format_mmss(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The server only re-sends 0x037 char_status on status changes, not every second,
/// so the KO countdown is anchored to the last server value and ticked down locally.
#[derive(Default)]
pub struct DeathCountdownAnchor {
    server_secs: Option<u32>,
    anchor_elapsed: f64,
}

pub fn update_death_prompt_system(
    time: Res<Time>,
    state: Res<SceneState>,
    method: Res<InputMethod>,
    mut anchor: Local<DeathCountdownAnchor>,
    mut panel_q: Query<&mut Node, With<DeathPromptPanel>>,
    mut countdown_q: Query<&mut Text, With<DeathCountdownText>>,
    mut hint_q: Query<&mut Text, (With<DeathPromptHint>, Without<DeathCountdownText>)>,
) {
    let snap = &state.snapshot;

    let dead = resolve_self(&snap.party, snap.self_char_id)
        .map(|m| m.hp_pct == 0)
        .unwrap_or(false);

    if let Ok(mut panel_node) = panel_q.single_mut() {
        let want = if dead { Display::Flex } else { Display::None };
        if panel_node.display != want {
            panel_node.display = want;
        }
    }

    let now = time.elapsed_secs_f64();
    let server = if dead {
        snap.death_homepoint_secs
    } else {
        None
    };
    if anchor.server_secs != server {
        anchor.server_secs = server;
        anchor.anchor_elapsed = now;
    }

    if let Ok(mut text) = countdown_q.single_mut() {
        let label = match anchor.server_secs {
            Some(secs) => {
                let ticked = (now - anchor.anchor_elapsed).max(0.0) as u32;
                format!("Home Point in {}", format_mmss(secs.saturating_sub(ticked)))
            }
            None => String::new(),
        };
        if **text != label {
            **text = label;
        }
    }

    if method.is_changed() {
        if let Ok(mut text) = hint_q.single_mut() {
            let label = hint_text(*method);
            if **text != label {
                **text = label.to_string();
            }
        }
    }
}

pub fn is_dead(state: &SceneState) -> bool {
    let snap = &state.snapshot;
    resolve_self(&snap.party, snap.self_char_id)
        .map(|m| m.hp_pct == 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_wire::PartyMember;

    fn member(id: u32, hp_pct: u8) -> PartyMember {
        PartyMember {
            id,
            act_index: 1,
            name: None,
            hp: 0,
            mp: 0,
            tp: 0,
            hp_pct,
            mp_pct: 100,
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
    fn is_dead_true_when_self_party_row_at_zero() {
        let mut state = SceneState::default();
        state.snapshot.self_char_id = Some(7);
        state.snapshot.party = vec![member(7, 0)];
        assert!(is_dead(&state));
    }

    #[test]
    fn is_dead_false_when_self_alive() {
        let mut state = SceneState::default();
        state.snapshot.self_char_id = Some(7);
        state.snapshot.party = vec![member(7, 50)];
        assert!(!is_dead(&state));
    }

    #[test]
    fn is_dead_false_when_party_empty() {
        let state = SceneState::default();
        assert!(!is_dead(&state));
    }

    #[test]
    fn is_dead_falls_back_to_first_member_when_self_id_unknown() {
        let mut state = SceneState::default();
        state.snapshot.self_char_id = None;
        state.snapshot.party = vec![member(99, 0)];
        assert!(is_dead(&state));
    }
}
