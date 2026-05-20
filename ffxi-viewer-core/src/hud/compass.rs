//! Top-right compass HUD.
//!
//! Reads the operator's `ChaseCamera.yaw` and renders an 8-direction
//! compass label (`N`, `NE`, `E`, …). Camera-driven rather than
//! player-heading-driven because the operator's perspective is the
//! camera; in chase mode they're typically watching the world from
//! behind/above and the compass tells them where the camera is looking.
//!
//! 8-direction quantization rather than continuous degrees: matches
//! the classic FFXI map widget aesthetic and avoids needing variable-
//! width text glyphs at the cost of fine-grained accuracy. Boundaries
//! land at every 22.5° offset from a cardinal so each octant covers a
//! 45° arc — `N` spans `[-22.5°, +22.5°]`.
//!
//! Not gated on a snapshot tick — the camera yaw can change every frame
//! via mouse-look. The change-detection write guard (`**text != want`)
//! keeps the per-frame cost a single string comparison when the user
//! hasn't moved.

use bevy::prelude::*;

use crate::camera::ChaseCamera;
use crate::hud::palette;

#[derive(Component)]
pub struct CompassPanel;

#[derive(Component)]
pub struct CompassLabel;

// Vanilla FFXI's compass is a small N-indicator, not the big addon-style
// disc. 32×32 reads as a corner glyph, not a panel.
const PANEL_SIZE_PX: f32 = 32.0;

pub fn spawn_compass(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            CompassPanel,
            Node {
                position_type: PositionType::Absolute,
                // Bottom-left, sitting just above the chat panel's top
                // edge (chat panel: bottom 54..214, so top at 214). 220
                // leaves a small gap. Vanilla retail anchors the compass
                // to the chat-log frame; we approximate that by living
                // in the same bottom-left quadrant. The quick-action
                // modal also opens at bottom: 222, but it's transient
                // and intentionally covers the compass when invoked.
                bottom: Val::Px(220.0),
                left: Val::Px(8.0),
                width: Val::Px(PANEL_SIZE_PX),
                height: Val::Px(PANEL_SIZE_PX),
                padding: UiRect::all(Val::Px(2.0)),
                border: UiRect::all(Val::Px(1.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                // Hidden by default: the minimap (also bottom-left)
                // includes a compass N indicator in retail, and our
                // minimap module now occupies this slot. Kept here
                // (rather than deleted) so a future "minimap off"
                // mode can re-show the text compass without
                // re-introducing spawn logic.
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::BORDER),
        ))
        .with_children(|p| {
            p.spawn((
                CompassLabel,
                Text::new("—"),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::ACCENT),
            ));
        });
}

pub fn update_compass(chase: Res<ChaseCamera>, mut label_q: Query<&mut Text, With<CompassLabel>>) {
    let Ok(mut text) = label_q.single_mut() else {
        return;
    };
    let want = direction_label(chase.yaw);
    if **text != want {
        **text = want.into();
    }
}

/// Quantize a yaw radian value to one of eight cardinal labels.
///
/// `yaw == 0` → camera sits at +Z (looking toward the player from `+Z`),
/// which means the operator is looking *north* (in Bevy's right-handed
/// world the player's "forward" maps to +Z when heading=0). Positive
/// yaw rotates clockwise viewed from above (CCW in math conventions
/// flipped because Y is up). The boundaries are at `(2k+1) * π/8` for
/// `k = 0..8`.
pub fn direction_label(yaw: f32) -> &'static str {
    const LABELS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    let tau = std::f32::consts::TAU;
    // Normalize to [0, 2π). `rem_euclid` keeps negatives positive.
    let normalized = yaw.rem_euclid(tau);
    // Shift by half-octant so the N arc is centered on 0 rad.
    let octant = ((normalized + tau / 16.0) / (tau / 8.0)) as usize;
    LABELS[octant % LABELS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_zero_is_north() {
        assert_eq!(direction_label(0.0), "N");
    }

    #[test]
    fn quarter_turns_are_cardinals() {
        let q = std::f32::consts::FRAC_PI_2;
        assert_eq!(direction_label(q), "E");
        assert_eq!(direction_label(2.0 * q), "S");
        assert_eq!(direction_label(3.0 * q), "W");
    }

    #[test]
    fn eighths_are_diagonals() {
        let e = std::f32::consts::FRAC_PI_4;
        assert_eq!(direction_label(e), "NE");
        assert_eq!(direction_label(3.0 * e), "SE");
        assert_eq!(direction_label(5.0 * e), "SW");
        assert_eq!(direction_label(7.0 * e), "NW");
    }

    #[test]
    fn negative_yaw_normalizes() {
        // -π/2 should be the same as +3π/2 → W
        assert_eq!(direction_label(-std::f32::consts::FRAC_PI_2), "W");
    }

    #[test]
    fn boundary_just_under_half_octant_stays_north() {
        let almost_ne = std::f32::consts::FRAC_PI_4 - 0.001;
        // Just under π/4 — still in the N arc since boundary is at π/8 + π/8 = π/4.
        // Actually our code centers N on 0, so N arc is [-π/8, +π/8]. At π/4-ε
        // we're well past +π/8, so this is NE.
        assert_eq!(direction_label(almost_ne), "NE");
    }
}
