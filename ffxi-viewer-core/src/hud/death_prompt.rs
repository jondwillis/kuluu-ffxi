//! Dead-state Return-to-Home-Point prompt. Auto-appears when the
//! operator's own party row reports `hp_pct == 0`; auto-disappears
//! when raised (HP transitions back > 0) or when the homepoint warp
//! lands (zone change) — both manifest as a non-zero `hp_pct` on the
//! next `update_self_hud` tick.
//!
//! Mirrors the dialog HUD's `Display::None` toggle pattern (see
//! `hud/dialog.rs`) — keeping the panel resident in the scene tree
//! and flipping `display` is cheaper than spawn/despawn at HUD rate.
//!
//! This module renders only. Input handling (Enter → dispatch
//! `AgentCommand::ReturnToHomePoint`) lives in the front-end (the
//! `viewer-core` crate is tokio-free and has no `cmd_tx`); see the
//! native viewer's death-prompt input system.

use bevy::prelude::*;

use crate::hud::palette;
use crate::hud::self_hud::resolve_self;
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct DeathPromptPanel;

const PANEL_WIDTH_PX: f32 = 380.0;

pub fn spawn_death_prompt(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            DeathPromptPanel,
            Node {
                position_type: PositionType::Absolute,
                // Center horizontally, sit a touch above mid-screen so
                // the player's eye lands on it before chat / dialog.
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
            // Red border — matches the retail "k.o." cue.
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
                Text::new("Press [Enter] to return to your home point."),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));
        });
}

pub fn update_death_prompt_system(
    state: Res<SceneState>,
    mut panel_q: Query<&mut Node, With<DeathPromptPanel>>,
) {
    if !state.is_changed() {
        return;
    }
    let Ok(mut panel_node) = panel_q.single_mut() else {
        return;
    };
    let snap = &state.snapshot;
    // Visible iff our resolved party row reports 0% HP. The official
    // FFXI death state implies `hp_pct == 0`, but the converse holds
    // too in practice (no other state sets the bar to exactly 0).
    let dead = resolve_self(&snap.party, snap.self_char_id)
        .map(|m| m.hp_pct == 0)
        .unwrap_or(false);
    let want = if dead { Display::Flex } else { Display::None };
    if panel_node.display != want {
        panel_node.display = want;
    }
}

/// Polled by front-end input systems to decide whether to consume
/// Enter / route it to a `ReturnToHomePoint` dispatch.
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
        // No party row → no self resolution → not dead. Important for
        // the post-zone-in gap before party-attr arrives, so the prompt
        // doesn't flicker in during loading.
        let state = SceneState::default();
        assert!(!is_dead(&state));
    }

    #[test]
    fn is_dead_falls_back_to_first_member_when_self_id_unknown() {
        // Mirror `resolve_self`'s fallback: when `self_char_id` is None
        // we trust the first party row. Soloing puts only-self there
        // anyway, and during the post-zone-in race the bar should still
        // surface a real death state.
        let mut state = SceneState::default();
        state.snapshot.self_char_id = None;
        state.snapshot.party = vec![member(99, 0)];
        assert!(is_dead(&state));
    }
}
