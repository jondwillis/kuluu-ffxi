//! Vana'diel clock HUD.
//!
//! Vana'dielian time is a deterministic transform of Earth time. The
//! widely-used reference (Wiki / `vana_time` formulas) is:
//!
//! - 25 Earth seconds = 1 Vana'diel hour.
//! - The Vana'dielian epoch is 8866-01-01 00:00 V.D., which corresponds
//!   to Earth 2002-06-23 15:00 UTC.
//!
//! Equivalently, Vana time advances 24 / (25 * 24) = 1/25 of a real
//! second per real second times 24 V hours per day, etc. The standard
//! approach is to measure seconds since the Earth epoch, multiply by
//! the V-day-rate, and convert back.
//!
//! No server data needed; this is pure formula.

use bevy::prelude::*;

use crate::hud::palette;

/// Earth epoch corresponding to Vana'diel 8866-01-01 00:00, in
/// `(SystemTime - UNIX_EPOCH).as_secs()` form. 2002-06-23 15:00 UTC.
pub const EARTH_EPOCH_UNIX: u64 = 1_024_844_400;
/// Vana'diel epoch year offset.
const VANA_EPOCH_YEAR: u32 = 886;
/// Earth seconds per Vana'diel day. 1 V-hour = 25 Earth seconds, day = 24.
pub const EARTH_SECS_PER_VANA_DAY: u64 = 25 * 24;
/// V-days per V-month and V-months per V-year. FFXI uses 30 / 12.
const VANA_DAYS_PER_MONTH: u32 = 30;
const VANA_MONTHS_PER_YEAR: u32 = 12;
const VANA_DAYS_PER_YEAR: u32 = VANA_DAYS_PER_MONTH * VANA_MONTHS_PER_YEAR;

#[derive(Component)]
pub struct VanaClockPanel;

#[derive(Component)]
pub struct VanaClockLabel;

pub fn spawn_vana_clock(mut commands: Commands) {
    commands
        .spawn((
            VanaClockPanel,
            Node {
                position_type: PositionType::Absolute,
                // Top-right column, slot 2: just below the compass.
                // Compass: top 36..96 (60 px tall). Clock at 104 leaves
                // an 8-px gap. The full top-right column stack is:
                //   compass     top: 36   (60px)
                //   vana_clock  top: 104  (~28px)
                //   llm_badge   top: 140  (~32px)
                //   roster      top: 200  (~variable, expands down)
                top: Val::Px(104.0),
                right: Val::Px(8.0),
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

pub fn update_vana_clock(time: Res<Time>, mut q: Query<&mut Text, With<VanaClockLabel>>) {
    let Ok(mut text) = q.single_mut() else {
        return;
    };
    // Refresh once per real second — V-time advances ~1.4 V-minutes per
    // real second so once per second is plenty of resolution.
    let _ = time;

    let earth_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(EARTH_EPOCH_UNIX);
    let want = format_vana(earth_now);
    if **text != want {
        **text = want;
    }
}

/// Convert `earth_unix_secs` to a `V YYYY-MM-DD HH:MM` string.
pub fn format_vana(earth_unix_secs: u64) -> String {
    let earth_since_vana = earth_unix_secs.saturating_sub(EARTH_EPOCH_UNIX);
    // Vana days that have elapsed since 8866-01-01.
    let total_vana_days = earth_since_vana / EARTH_SECS_PER_VANA_DAY;
    let secs_into_today = earth_since_vana % EARTH_SECS_PER_VANA_DAY;

    // Hour: 25 Earth seconds = 1 V-hour. Minutes: 25/60 Earth seconds per V-min.
    let v_hour = secs_into_today / 25; // 0..23
    let v_minute_secs = secs_into_today % 25;
    let v_minute = (v_minute_secs as f64 * 60.0 / 25.0) as u64; // 0..59

    let v_year = VANA_EPOCH_YEAR as u64 + total_vana_days / VANA_DAYS_PER_YEAR as u64;
    let day_of_year = (total_vana_days % VANA_DAYS_PER_YEAR as u64) as u32;
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
        let s = format_vana(EARTH_EPOCH_UNIX + 25);
        assert_eq!(s, "V 0886-01-01 01:00");
    }

    #[test]
    fn one_vana_day_after_epoch_is_day_two() {
        let s = format_vana(EARTH_EPOCH_UNIX + EARTH_SECS_PER_VANA_DAY);
        assert_eq!(s, "V 0886-01-02 00:00");
    }

    #[test]
    fn earth_time_before_vana_epoch_clamps_to_epoch() {
        // Saturating-sub on `< EARTH_EPOCH_UNIX` returns 0 — should produce
        // the epoch string, not panic or wrap.
        let s = format_vana(0);
        assert_eq!(s, "V 0886-01-01 00:00");
    }

    #[test]
    fn month_rolls_over_at_30_days() {
        // 30 days after epoch = 0886-02-01.
        let s = format_vana(EARTH_EPOCH_UNIX + 30 * EARTH_SECS_PER_VANA_DAY);
        assert_eq!(s, "V 0886-02-01 00:00");
    }

    #[test]
    fn year_rolls_over_at_360_days() {
        // 12 months × 30 days = 360 V-days. After 360 V-days we should be
        // at year 887.
        let s = format_vana(EARTH_EPOCH_UNIX + 360 * EARTH_SECS_PER_VANA_DAY);
        assert_eq!(s, "V 0887-01-01 00:00");
    }
}
