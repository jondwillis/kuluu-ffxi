use bevy::prelude::*;
use ffxi_viewer_wire::{FishingArrow, SelfFishing};

use crate::hud::style::{self, theme};
use crate::keybinds::{Action, Bindings};
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct FishingHudPanel;

#[derive(Component)]
pub struct FishingHudStatus;

#[derive(Component)]
pub struct FishingHudBar;

#[derive(Component)]
pub struct FishingHudBarFill;

#[derive(Component)]
pub struct FishingHudArrow;

#[derive(Component)]
pub struct FishingHudHint;

const PANEL_WIDTH_PX: f32 = 240.0;

/// Sized so the bar fits the panel inside its horizontal padding.
const BAR_WIDTH_PX: f32 = PANEL_WIDTH_PX - 2.0 * 12.0;

const BAR_HEIGHT_PX: f32 = 10.0;

/// What the fishing HUD should show for a given snapshot state. Pure so the
/// snapshot → display mapping is unit-testable without a Bevy app.
#[derive(Debug, Clone, PartialEq)]
pub struct FishingHudModel {
    pub visible: bool,
    pub status: &'static str,
    /// Fish stamina fill fraction 0..=1, `None` until a fish is hooked
    /// (denominator is the s2c 0x115 `FishPacket::stamina` carried as
    /// `SelfFishing::fish_max`).
    pub bar: Option<f32>,
    pub arrow: Option<FishingArrow>,
}

impl FishingHudModel {
    pub const HIDDEN: Self = Self {
        visible: false,
        status: "",
        bar: None,
        arrow: None,
    };
}

pub fn compute_model(fishing: Option<&SelfFishing>) -> FishingHudModel {
    let Some(f) = fishing else {
        return FishingHudModel::HIDDEN;
    };
    // fish_max stays 0 through the cast/waiting phases (fsh0/fsh1, the looping
    // clips in `ffxi_actor::fishing_clip`) and turns nonzero once the 0x115
    // fish packet lands.
    if f.fish_max == 0 {
        return FishingHudModel {
            visible: true,
            status: "Waiting for a bite\u{2026}",
            bar: None,
            arrow: f.arrow,
        };
    }
    FishingHudModel {
        visible: true,
        status: "Fish on!",
        bar: Some((f.fish_hp as f32 / f.fish_max as f32).clamp(0.0, 1.0)),
        arrow: f.arrow,
    }
}

pub fn arrow_glyph(arrow: FishingArrow) -> &'static str {
    if arrow.left {
        "\u{25C0}"
    } else {
        "\u{25B6}"
    }
}

/// Golden arrows reuse the cursor gold; regular arrows stay body-text white.
pub fn arrow_color(arrow: FishingArrow) -> Color {
    if arrow.golden {
        theme::CURSOR
    } else {
        theme::TEXT
    }
}

pub fn spawn_fishing_hud(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            FishingHudPanel,
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(120.0),
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
                FishingHudStatus,
                Text::new(""),
                style::text_font(15.0),
                TextColor(theme::TITLE),
            ));
            p.spawn((
                FishingHudArrow,
                Text::new(" "),
                style::text_font(24.0),
                TextColor(theme::TEXT),
            ));
            p.spawn((
                FishingHudBar,
                Node {
                    width: Val::Px(BAR_WIDTH_PX),
                    height: Val::Px(BAR_HEIGHT_PX),
                    border: UiRect::all(Val::Px(1.0)),
                    display: Display::None,
                    ..default()
                },
                BackgroundColor(theme::CELL_BG),
                BorderColor::all(theme::CELL_EDGE),
            ))
            .with_children(|bar| {
                bar.spawn((
                    FishingHudBarFill,
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    BackgroundColor(theme::GOOD),
                ));
            });
            p.spawn((
                FishingHudHint,
                Text::new(""),
                style::text_font(12.0),
                TextColor(theme::MUTED),
            ));
        });
}

fn hint_line(bindings: &Bindings) -> String {
    let key = |a: Action| bindings.key_label(a).unwrap_or("?");
    format!(
        "{}/{} react \u{00B7} {} hook \u{00B7} {} give up",
        key(Action::FishingReelLeft),
        key(Action::FishingReelRight),
        key(Action::FishingHook),
        key(Action::FishingCancel),
    )
}

#[allow(clippy::type_complexity)]
pub fn update_fishing_hud(
    state: Res<SceneState>,
    bindings: Res<Bindings>,
    mut panel_q: Query<
        &mut Node,
        (
            With<FishingHudPanel>,
            Without<FishingHudBar>,
            Without<FishingHudBarFill>,
        ),
    >,
    mut status_q: Query<
        &mut Text,
        (
            With<FishingHudStatus>,
            Without<FishingHudArrow>,
            Without<FishingHudHint>,
        ),
    >,
    mut bar_q: Query<
        &mut Node,
        (
            With<FishingHudBar>,
            Without<FishingHudPanel>,
            Without<FishingHudBarFill>,
        ),
    >,
    mut fill_q: Query<
        &mut Node,
        (
            With<FishingHudBarFill>,
            Without<FishingHudPanel>,
            Without<FishingHudBar>,
        ),
    >,
    mut arrow_q: Query<
        (&mut Text, &mut TextColor),
        (
            With<FishingHudArrow>,
            Without<FishingHudStatus>,
            Without<FishingHudHint>,
        ),
    >,
    mut hint_q: Query<
        &mut Text,
        (
            With<FishingHudHint>,
            Without<FishingHudStatus>,
            Without<FishingHudArrow>,
        ),
    >,
) {
    if !state.dirty {
        return;
    }
    let model = compute_model(state.snapshot.self_fishing.as_ref());

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

    if let Ok(mut text) = status_q.single_mut() {
        **text = model.status.to_string();
    }
    if let Ok(mut node) = bar_q.single_mut() {
        node.display = if model.bar.is_some() {
            Display::Flex
        } else {
            Display::None
        };
    }
    if let (Some(fill), Ok(mut node)) = (model.bar, fill_q.single_mut()) {
        node.width = Val::Percent(fill * 100.0);
    }
    if let Ok((mut text, mut tc)) = arrow_q.single_mut() {
        match model.arrow {
            Some(a) => {
                **text = arrow_glyph(a).to_string();
                tc.0 = arrow_color(a);
            }
            // Non-empty placeholder keeps the row height stable so the panel
            // does not jump when an arrow prompt appears.
            None => {
                **text = " ".to_string();
                tc.0 = theme::MUTED;
            }
        }
    }
    if let Ok(mut text) = hint_q.single_mut() {
        let hint = hint_line(&bindings);
        if **text != hint {
            **text = hint;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fishing(phase: u8, fish_max: u16, fish_hp: u16, arrow: Option<FishingArrow>) -> SelfFishing {
        SelfFishing {
            phase,
            fish_max,
            fish_hp,
            arrow,
        }
    }

    #[test]
    fn hidden_when_not_fishing() {
        assert_eq!(compute_model(None), FishingHudModel::HIDDEN);
        assert!(!compute_model(None).visible);
    }

    #[test]
    fn waiting_before_fish_packet() {
        let m = compute_model(Some(&fishing(1, 0, 0, None)));
        assert!(m.visible);
        assert_eq!(m.bar, None);
        assert_eq!(m.status, "Waiting for a bite\u{2026}");
    }

    #[test]
    fn bar_tracks_fish_stamina() {
        let m = compute_model(Some(&fishing(1, 200, 50, None)));
        assert_eq!(m.status, "Fish on!");
        assert_eq!(m.bar, Some(0.25));
    }

    #[test]
    fn bar_clamps_regen_overshoot() {
        // arrow_regen can push stamina past the starting max; the bar pins at
        // full instead of overflowing the frame.
        let m = compute_model(Some(&fishing(1, 100, 150, None)));
        assert_eq!(m.bar, Some(1.0));
    }

    #[test]
    fn drained_fish_reads_empty() {
        let m = compute_model(Some(&fishing(1, 100, 0, None)));
        assert_eq!(m.bar, Some(0.0));
    }

    #[test]
    fn arrow_passthrough_and_styling() {
        let left = FishingArrow {
            left: true,
            golden: false,
        };
        let golden_right = FishingArrow {
            left: false,
            golden: true,
        };
        let m = compute_model(Some(&fishing(1, 100, 100, Some(left))));
        assert_eq!(m.arrow, Some(left));
        assert_eq!(arrow_glyph(left), "\u{25C0}");
        assert_eq!(arrow_glyph(golden_right), "\u{25B6}");
        assert_eq!(arrow_color(golden_right), theme::CURSOR);
        assert_eq!(arrow_color(left), theme::TEXT);
    }

    #[test]
    fn default_bindings_render_full_hint() {
        let hint = hint_line(&Bindings::default());
        assert!(
            !hint.contains('?'),
            "unbound fishing action in hint: {hint}"
        );
    }
}
