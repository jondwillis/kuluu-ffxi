use bevy::prelude::*;

use crate::hud::palette;

pub const EARTH_EPOCH_UNIX: u64 = 1_009_810_800;

pub const EARTH_SECS_PER_VANA_HOUR: u64 = 144;

pub const EARTH_SECS_PER_VANA_DAY: u64 = EARTH_SECS_PER_VANA_HOUR * 24;

#[derive(Component)]
pub struct VanaClockPanel;

#[derive(Component)]
pub struct VanaClockLabel;

pub fn spawn_vana_clock_as_child(p: &mut ChildSpawnerCommands) {
    p.spawn((
        VanaClockPanel,
        Node {
            flex_shrink: 0.0,
            padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(palette::BACKGROUND),
        BorderColor::all(palette::BORDER),
    ))
    .with_children(|p| {
        p.spawn((
            VanaClockLabel,
            Text::new("0:00   (?-?)"),
            TextFont {
                font_size: 12.0,
                ..default()
            },
            TextColor(palette::TEXT),
        ));
    });
}

pub const VANA_WEEKDAYS: [&str; 8] = [
    "Firesday",
    "Earthsday",
    "Watersday",
    "Windsday",
    "Iceday",
    "Lightningday",
    "Lightsday",
    "Darksday",
];

pub fn update_vana_clock(
    mut q: Query<&mut Text, With<VanaClockLabel>>,
    q_self: Query<&Transform, With<crate::components::IsSelf>>,
    grid: Option<Res<crate::minimap::retail::PlayerMapGrid>>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    mut prev_vana_day: Local<Option<u64>>,
) {
    let Ok(mut text) = q.single_mut() else {
        return;
    };

    let earth_now = vana_clock.earth_unix_secs_now();
    let cell = player_grid_cell(grid.as_deref(), q_self.single().ok());
    let want = format!("{}   {}", format_vana_time(earth_now), cell);
    if **text != want {
        **text = want;
    }

    let earth_since_vana = earth_now.saturating_sub(EARTH_EPOCH_UNIX);
    let total_vana_days = earth_since_vana / EARTH_SECS_PER_VANA_DAY;
    if let Some(prev) = *prev_vana_day {
        if prev != total_vana_days {
            let weekday = VANA_WEEKDAYS[(total_vana_days % 8) as usize];
            toasts.write(crate::snapshot::ToastEvent::system(format!(
                "📅 Vana day {} — {}",
                total_vana_days, weekday,
            )));
        }
    }
    *prev_vana_day = Some(total_vana_days);
}

pub fn vana_minutes_since_epoch(earth_unix_secs: u64) -> u64 {
    let earth_since_vana = earth_unix_secs.saturating_sub(EARTH_EPOCH_UNIX);

    earth_since_vana.saturating_mul(25) / 60
}

// research/xim EnvironmentManager.kt:92-94 getFullDayInterpolation: the clock-driven
// color tracks (ParticleUpdaters.kt:172-183 ClockValueUpdater) sample at the fraction
// of the Vana'diel day elapsed, in [0, 1). One Vana day = 1440 Vana minutes.
pub fn full_day_fraction(earth_unix_secs: u64) -> f32 {
    let total_v_min = vana_minutes_since_epoch(earth_unix_secs);
    const VANA_MINUTES_PER_DAY: u64 = 24 * 60;
    (total_v_min % VANA_MINUTES_PER_DAY) as f32 / VANA_MINUTES_PER_DAY as f32
}

pub fn format_vana_time(earth_unix_secs: u64) -> String {
    let total_v_min = vana_minutes_since_epoch(earth_unix_secs);
    let v_minute = total_v_min % 60;
    let v_hour = (total_v_min / 60) % 24;
    format!("{v_hour}:{v_minute:02}")
}

fn player_grid_cell(
    grid: Option<&crate::minimap::retail::PlayerMapGrid>,
    player: Option<&Transform>,
) -> String {
    match (grid.and_then(|g| g.aabb), player) {
        (Some(aabb), Some(tf)) => {
            let (col, row) = aabb.world_to_grid(tf.translation);
            format!("({col}-{row})")
        }
        _ => "(?-?)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vana_epoch_renders_as_midnight() {
        assert_eq!(format_vana_time(EARTH_EPOCH_UNIX), "0:00");
    }

    #[test]
    fn one_vana_hour_after_epoch_is_1_00() {
        assert_eq!(
            format_vana_time(EARTH_EPOCH_UNIX + EARTH_SECS_PER_VANA_HOUR),
            "1:00"
        );
    }

    #[test]
    fn afternoon_hour_has_no_leading_zero_minute_does() {
        let five_vana_minutes = 5 * EARTH_SECS_PER_VANA_HOUR / 60;
        assert_eq!(
            format_vana_time(EARTH_EPOCH_UNIX + 13 * EARTH_SECS_PER_VANA_HOUR + five_vana_minutes),
            "13:05"
        );
    }

    #[test]
    fn hour_wraps_at_a_full_day() {
        assert_eq!(
            format_vana_time(EARTH_EPOCH_UNIX + EARTH_SECS_PER_VANA_DAY),
            "0:00"
        );
    }

    #[test]
    fn earth_time_before_vana_epoch_clamps_to_midnight() {
        assert_eq!(format_vana_time(0), "0:00");
    }

    #[test]
    fn full_day_fraction_spans_the_vana_day() {
        assert!((full_day_fraction(EARTH_EPOCH_UNIX) - 0.0).abs() < 1e-6);
        // 12 Vana hours after epoch == midday == 0.5.
        let half = EARTH_EPOCH_UNIX + 12 * EARTH_SECS_PER_VANA_HOUR;
        assert!((full_day_fraction(half) - 0.5).abs() < 1e-6);
        // A full day later wraps back to 0.0.
        assert!((full_day_fraction(EARTH_EPOCH_UNIX + EARTH_SECS_PER_VANA_DAY) - 0.0).abs() < 1e-6);
        // Always strictly below 1.0.
        let late = EARTH_EPOCH_UNIX + 23 * EARTH_SECS_PER_VANA_HOUR;
        assert!(full_day_fraction(late) < 1.0);
    }

    #[test]
    fn vana_minutes_since_epoch_matches_formatter() {
        assert_eq!(vana_minutes_since_epoch(EARTH_EPOCH_UNIX), 0);
        assert_eq!(
            vana_minutes_since_epoch(EARTH_EPOCH_UNIX + EARTH_SECS_PER_VANA_HOUR),
            60
        );

        assert_eq!(
            vana_minutes_since_epoch(EARTH_EPOCH_UNIX + EARTH_SECS_PER_VANA_DAY),
            1440
        );

        assert_eq!(vana_minutes_since_epoch(0), 0);
    }
}
