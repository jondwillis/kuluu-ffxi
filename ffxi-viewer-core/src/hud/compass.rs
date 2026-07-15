use bevy::prelude::*;

use crate::camera::ChaseCamera;
use crate::hud::style::{self, theme};

#[derive(Component)]
pub struct CompassPanel;

#[derive(Component)]
pub struct CompassLabel;

const PANEL_SIZE_PX: f32 = 32.0;

const OVERLAY_BG: Color = Color::srgba(0.04, 0.04, 0.04, 0.66);

pub fn spawn_compass_overlay_as_child(p: &mut ChildSpawnerCommands) {
    p.spawn((
        CompassPanel,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(3.0),
            left: Val::Px(3.0),
            min_width: Val::Px(20.0),
            padding: UiRect::axes(Val::Px(4.0), Val::Px(1.0)),
            border: UiRect::all(Val::Px(1.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        ZIndex(15),
        BackgroundColor(OVERLAY_BG),
        BorderColor::all(theme::FRAME_EDGE),
    ))
    .with_children(|p| {
        p.spawn((
            CompassLabel,
            Text::new("—"),
            style::text_font(13.0),
            TextColor(theme::TITLE),
        ));
    });
}

pub fn spawn_compass_as_child(p: &mut ChildSpawnerCommands) {
    p.spawn((
        CompassPanel,
        Node {
            flex_shrink: 0.0,
            width: Val::Px(PANEL_SIZE_PX),
            height: Val::Px(PANEL_SIZE_PX),
            padding: UiRect::all(Val::Px(2.0)),
            border: UiRect::all(Val::Px(1.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(theme::FRAME_BG),
        BorderColor::all(theme::FRAME_EDGE),
    ))
    .with_children(|p| {
        p.spawn((
            CompassLabel,
            Text::new("—"),
            style::text_font(14.0),
            TextColor(theme::TITLE),
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

pub fn direction_label(yaw: f32) -> &'static str {
    const LABELS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    let tau = std::f32::consts::TAU;

    let normalized = yaw.rem_euclid(tau);

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
        assert_eq!(direction_label(-std::f32::consts::FRAC_PI_2), "W");
    }

    #[test]
    fn boundary_just_under_half_octant_stays_north() {
        let almost_ne = std::f32::consts::FRAC_PI_4 - 0.001;

        assert_eq!(direction_label(almost_ne), "NE");
    }
}
