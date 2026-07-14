use bevy::prelude::*;
use ffxi_viewer_wire::Stage;

use crate::hud::palette;
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct StageDot;

#[derive(Component)]
pub struct StageLabel;

#[derive(Component)]
pub struct CharName;

#[derive(Component)]
pub struct ZoneLabel;

const FONT: f32 = 13.0;

pub fn spawn_stage_cluster_as_child(p: &mut ChildSpawnerCommands) {
    p.spawn((
        StageDot,
        Text::new("● "),
        TextFont {
            font_size: FONT.into(),
            ..default()
        },
        TextColor(stage_color(Stage::default())),
    ));

    p.spawn((
        StageLabel,
        Text::new(stage_label(Stage::default()).to_string()),
        TextFont {
            font_size: FONT.into(),
            ..default()
        },
        TextColor(stage_color(Stage::default())),
    ));

    p.spawn((
        Text::new("  ▪  "),
        TextFont {
            font_size: FONT.into(),
            ..default()
        },
        TextColor(palette::MUTED),
    ));
    p.spawn((
        CharName,
        Text::new("(no char)".to_string()),
        TextFont {
            font_size: FONT.into(),
            ..default()
        },
        TextColor(palette::TEXT),
    ));
    p.spawn((
        Text::new("  ▪  "),
        TextFont {
            font_size: FONT.into(),
            ..default()
        },
        TextColor(palette::MUTED),
    ));
    p.spawn((
        ZoneLabel,
        Text::new("—".to_string()),
        TextFont {
            font_size: FONT.into(),
            ..default()
        },
        TextColor(palette::TEXT),
    ));
}

pub fn update_stage_bar(
    state: Res<SceneState>,
    mut q_dot: Query<&mut TextColor, (With<StageDot>, Without<StageLabel>)>,
    mut q_label: Query<(&mut Text, &mut TextColor), (With<StageLabel>, Without<StageDot>)>,
    mut q_char: Query<&mut Text, (With<CharName>, Without<StageLabel>, Without<ZoneLabel>)>,
    mut q_zone: Query<&mut Text, (With<ZoneLabel>, Without<StageLabel>, Without<CharName>)>,
) {
    if !state.dirty {
        return;
    }
    let stage = state.snapshot.stage;
    let color = stage_color(stage);
    let label = stage_label(stage);

    if let Ok(mut tc) = q_dot.single_mut() {
        tc.0 = color;
    }
    if let Ok((mut text, mut tc)) = q_label.single_mut() {
        **text = label.to_string();
        tc.0 = color;
    }
    if let Ok(mut text) = q_char.single_mut() {
        let name = state
            .snapshot
            .char_name
            .as_deref()
            .unwrap_or("(no char)")
            .to_string();
        **text = name;
    }
    if let Ok(mut text) = q_zone.single_mut() {
        let z = match state.snapshot.zone_id {
            Some(z) => format!("zone {z}"),
            None => "—".to_string(),
        };
        **text = z;
    }
}

fn stage_label(stage: Stage) -> &'static str {
    match stage {
        Stage::Idle => "idle",
        Stage::Authenticating => "auth",
        Stage::LobbyHandshake => "lobby",
        Stage::MapBootstrap => "map-bootstrap",
        Stage::Zoning => "zoning",
        Stage::InZone => "in-zone",
        Stage::Disconnected => "disconnected",
    }
}

fn stage_color(stage: Stage) -> Color {
    match stage {
        Stage::Idle => palette::STAGE_IDLE,
        Stage::Authenticating | Stage::LobbyHandshake | Stage::MapBootstrap | Stage::Zoning => {
            palette::STAGE_TRANSITIONING
        }
        Stage::InZone => palette::STAGE_GOOD,
        Stage::Disconnected => palette::STAGE_BAD,
    }
}
