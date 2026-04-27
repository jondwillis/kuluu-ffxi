use bevy::prelude::*;

use crate::hud::palette;

pub const EARTH_EPOCH_UNIX: u64 = 1_009_810_800;

const VANA_EPOCH_YEAR: u32 = 886;

pub const EARTH_SECS_PER_VANA_HOUR: u64 = 144;

pub const EARTH_SECS_PER_VANA_DAY: u64 = EARTH_SECS_PER_VANA_HOUR * 24;

const VANA_DAYS_PER_MONTH: u32 = 30;
const VANA_MONTHS_PER_YEAR: u32 = 12;
const VANA_DAYS_PER_YEAR: u32 = VANA_DAYS_PER_MONTH * VANA_MONTHS_PER_YEAR;

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
            Text::new("V—"),
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
    time: Res<Time>,
    mut q: Query<&mut Text, With<VanaClockLabel>>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    mut prev_vana_day: Local<Option<u64>>,
) {
    let Ok(mut text) = q.single_mut() else {
        return;
    };
    let _ = time;

    let earth_now = vana_clock.earth_unix_secs_now();
    let want = format_vana(earth_now);
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

pub fn format_vana(earth_unix_secs: u64) -> String {
    let earth_since_vana = earth_unix_secs.saturating_sub(EARTH_EPOCH_UNIX);

    let total_v_min = earth_since_vana.saturating_mul(25) / 60;
    let total_v_hour = total_v_min / 60;
    let total_v_day = total_v_hour / 24;

    let v_minute = total_v_min % 60;
    let v_hour = total_v_hour % 24;

    let v_year = VANA_EPOCH_YEAR as u64 + total_v_day / VANA_DAYS_PER_YEAR as u64;
    let day_of_year = (total_v_day % VANA_DAYS_PER_YEAR as u64) as u32;
    let v_month = day_of_year / VANA_DAYS_PER_MONTH + 1;
    let v_day = day_of_year % VANA_DAYS_PER_MONTH + 1;

    format!("V {v_year:04}-{v_month:02}-{v_day:02} {v_hour:02}:{v_minute:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vana_epoch_renders_as_first_day_midnight() {
        let s = format_vana(EARTH_EPOCH_UNIX);
        assert_eq!(s, "V 0886-01-01 00:00");
    }

    #[test]
    fn one_vana_hour_after_epoch_is_01_00() {
        let s = format_vana(EARTH_EPOCH_UNIX + EARTH_SECS_PER_VANA_HOUR);
        assert_eq!(s, "V 0886-01-01 01:00");
    }

    #[test]
    fn one_vana_day_after_epoch_is_day_two() {
        let s = format_vana(EARTH_EPOCH_UNIX + EARTH_SECS_PER_VANA_DAY);
        assert_eq!(s, "V 0886-01-02 00:00");
    }

    #[test]
    fn earth_time_before_vana_epoch_clamps_to_epoch() {
        let s = format_vana(0);
        assert_eq!(s, "V 0886-01-01 00:00");
    }

    #[test]
    fn month_rolls_over_at_30_days() {
        let s = format_vana(EARTH_EPOCH_UNIX + 30 * EARTH_SECS_PER_VANA_DAY);
        assert_eq!(s, "V 0886-02-01 00:00");
    }

    #[test]
    fn year_rolls_over_at_360_days() {
        let s = format_vana(EARTH_EPOCH_UNIX + 360 * EARTH_SECS_PER_VANA_DAY);
        assert_eq!(s, "V 0887-01-01 00:00");
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
