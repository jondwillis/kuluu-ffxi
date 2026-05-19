//! Top status bar mirroring `chrome::draw_stage_bar`:
//!
//! ```text
//! ▌ ffxi-client ● <stage_label> ▪ <charname> ▪ <zone>
//! ```
//!
//! - "▌ ffxi-client" in bold cyan
//! - "●" + stage label color-coded:
//!     idle → dark gray
//!     authenticating / lobby_handshake / map_bootstrap / zoning → yellow
//!     in_zone → green
//!     disconnected → red
//! - "▪" separators in default text color
//! - charname falls back to "(no char)", zone to "—"
//! - 1px DarkGray border, very dark background

use bevy::prelude::*;
use ffxi_viewer_wire::Stage;

use crate::hud::palette;
use crate::snapshot::SceneState;

/// Marker component on the root bar node.
#[derive(Component)]
pub struct StageBar;

/// Marker on the `●` glyph — its color reflects current stage.
#[derive(Component)]
pub struct StageDot;

/// Marker on the stage label text — both content and color reflect stage.
#[derive(Component)]
pub struct StageLabel;

/// Marker on the character name text.
#[derive(Component)]
pub struct CharName;

/// Marker on the zone string.
#[derive(Component)]
pub struct ZoneLabel;

/// Spawn the stage bar at the top of the screen.
pub fn spawn_stage_bar(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            StageBar,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                height: Val::Px(28.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(0.0),
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::BORDER),
        ))
        .with_children(|p| {
            // "▌ ffxi-client " — bold cyan brand.
            p.spawn((
                Text::new("▌ ffxi-client "),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::ACCENT),
            ));
            // "● " — status dot, color reflects stage.
            p.spawn((
                StageDot,
                Text::new("● "),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(stage_color(Stage::default())),
            ));
            // Stage label, color-matched to dot.
            p.spawn((
                StageLabel,
                Text::new(stage_label(Stage::default()).to_string()),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(stage_color(Stage::default())),
            ));
            // " ▪ " separator → CharName → " ▪ " → Zone.
            p.spawn((
                Text::new("  ▪  "),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::MUTED),
            ));
            p.spawn((
                CharName,
                Text::new("(no char)".to_string()),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));
            p.spawn((
                Text::new("  ▪  "),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::MUTED),
            ));
            p.spawn((
                ZoneLabel,
                Text::new("—".to_string()),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));
        });
}

/// Refresh stage bar text + colors when the snapshot changes.
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
