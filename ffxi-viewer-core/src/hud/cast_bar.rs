#![cfg(feature = "enhanced-cast-bar")]
//! Enhanced (non-retail) spell cast bar. Retail's classic client shows no
//! filling cast bar; this addon-style widget reproduces one from the reactor's
//! optimistic `SelfCasting` projection (spell name + castTime).

use bevy::prelude::*;
use ffxi_viewer_wire::SelfCasting;

use crate::hud::style::{self, theme};
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct CastBarPanel;

#[derive(Component)]
pub struct CastBarLabel;

#[derive(Component)]
pub struct CastBarTrack;

#[derive(Component)]
pub struct CastBarFill;

const PANEL_WIDTH_PX: f32 = 260.0;

/// Sized so the track fits the panel inside its horizontal padding.
const BAR_WIDTH_PX: f32 = PANEL_WIDTH_PX - 2.0 * 12.0;

const BAR_HEIGHT_PX: f32 = 12.0;

/// What the cast bar should show for a snapshot state. Pure so the mapping is
/// unit-testable without a Bevy app.
#[derive(Debug, Clone, PartialEq)]
pub struct CastBarModel {
    pub visible: bool,
    pub label: String,
    /// Cast progress fill fraction 0..=1.
    pub fill: f32,
    pub interrupted: bool,
}

impl CastBarModel {
    pub const HIDDEN: Self = Self {
        visible: false,
        label: String::new(),
        fill: 0.0,
        interrupted: false,
    };
}

pub fn compute_model(casting: Option<&SelfCasting>) -> CastBarModel {
    let Some(c) = casting else {
        return CastBarModel::HIDDEN;
    };
    let fill = if c.total_ms == 0 {
        0.0
    } else {
        (c.elapsed_ms as f32 / c.total_ms as f32).clamp(0.0, 1.0)
    };
    CastBarModel {
        visible: true,
        label: c.name.clone(),
        fill,
        interrupted: c.interrupted,
    }
}

pub fn spawn_cast_bar(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            CastBarPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(90.0),
                left: Val::Percent(50.0),
                margin: UiRect {
                    left: Val::Px(-PANEL_WIDTH_PX / 2.0),
                    ..default()
                },
                width: Val::Px(PANEL_WIDTH_PX),
                padding: UiRect::axes(Val::Px(12.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(4.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
        ))
        .with_children(|p| {
            p.spawn((
                CastBarLabel,
                Text::new(""),
                style::text_font(15.0),
                TextColor(theme::TITLE),
            ));
            p.spawn((
                CastBarTrack,
                Node {
                    width: Val::Px(BAR_WIDTH_PX),
                    height: Val::Px(BAR_HEIGHT_PX),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(theme::CELL_BG),
                BorderColor::all(theme::CELL_EDGE),
            ))
            .with_children(|track| {
                track.spawn((
                    CastBarFill,
                    Node {
                        width: Val::Percent(0.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    BackgroundColor(theme::CURSOR),
                ));
            });
        });
}

#[allow(clippy::type_complexity)]
pub fn update_cast_bar(
    state: Res<SceneState>,
    mut panel_q: Query<&mut Node, (With<CastBarPanel>, Without<CastBarFill>)>,
    mut label_q: Query<&mut Text, With<CastBarLabel>>,
    mut fill_q: Query<
        (&mut Node, &mut BackgroundColor),
        (With<CastBarFill>, Without<CastBarPanel>),
    >,
) {
    if !state.dirty {
        return;
    }
    let model = compute_model(state.snapshot.self_casting.as_ref());

    if let Ok(mut node) = panel_q.single_mut() {
        node.display = if model.visible {
            Display::Flex
        } else {
            Display::None
        };
    }
    if !model.visible {
        return;
    }
    if let Ok(mut text) = label_q.single_mut() {
        if **text != model.label {
            **text = model.label.clone();
        }
    }
    if let Ok((mut node, mut bg)) = fill_q.single_mut() {
        node.width = Val::Percent(model.fill * 100.0);
        bg.0 = if model.interrupted {
            theme::DANGER
        } else {
            theme::CURSOR
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn casting(name: &str, elapsed_ms: u32, total_ms: u32, interrupted: bool) -> SelfCasting {
        SelfCasting {
            name: name.to_string(),
            elapsed_ms,
            total_ms,
            interrupted,
        }
    }

    #[test]
    fn hidden_when_not_casting() {
        assert_eq!(compute_model(None), CastBarModel::HIDDEN);
    }

    #[test]
    fn fill_tracks_elapsed_over_total() {
        let m = compute_model(Some(&casting("stone", 250, 500, false)));
        assert!(m.visible);
        assert_eq!(m.label, "stone");
        assert_eq!(m.fill, 0.5);
    }

    #[test]
    fn fill_clamps_on_overshoot() {
        let m = compute_model(Some(&casting("cure", 3000, 2000, false)));
        assert_eq!(m.fill, 1.0);
    }

    #[test]
    fn zero_total_reads_empty() {
        let m = compute_model(Some(&casting("x", 100, 0, false)));
        assert_eq!(m.fill, 0.0);
    }

    #[test]
    fn interrupted_flag_passthrough() {
        let m = compute_model(Some(&casting("fire", 100, 500, true)));
        assert!(m.interrupted);
    }
}
