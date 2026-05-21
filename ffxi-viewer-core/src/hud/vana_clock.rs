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

/// Spawn the Vana clock as a child of the bottom-left flex stack,
/// docked above the minimap. Retail FFXI clusters its persistent
/// indicators (minimap-compass + clock) in the same screen corner;
/// this matches that layout rather than the previous top-right
/// column.
pub fn spawn_vana_clock_as_child(p: &mut ChildSpawnerCommands) {
    p.spawn((
        VanaClockPanel,
        Node {
            // `flex_shrink: 0` keeps the chip at its content width
            // even when the chat panel expands and squeezes the
            // bottom-left stack.
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

/// Monotonic Vana minutes since the V-epoch (8866-01-01 00:00).
///
/// Use this when you need a single increasing scalar across days —
/// e.g. when a Scheduler-like timeline straddles a Vana-day rollover,
/// or when comparing two Vana timestamps across an arbitrary span.
/// For per-action timelines stay frame-based (see
/// `ffxi_dat::scheduler::TimedStage::frame`); for in-day animations
/// use [`crate::sun_moon::VanaSky::hour`].
pub fn vana_minutes_since_epoch(earth_unix_secs: u64) -> u64 {
    let earth_since_vana = earth_unix_secs.saturating_sub(EARTH_EPOCH_UNIX);
    // 25 Earth seconds = 1 V-hour = 60 V-min, so 1 Earth-second = 60/25 = 2.4 V-min.
    // Stay in integer math by scaling: vana-min = earth-sec * 60 / 25.
    earth_since_vana.saturating_mul(60) / 25
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

    #[test]
    fn vana_minutes_since_epoch_matches_formatter() {
        // One V-hour = 60 V-min = 25 earth-sec.
        assert_eq!(vana_minutes_since_epoch(EARTH_EPOCH_UNIX), 0);
        assert_eq!(vana_minutes_since_epoch(EARTH_EPOCH_UNIX + 25), 60);
        // A full V-day = 24 × 60 = 1440 V-min.
        assert_eq!(
            vana_minutes_since_epoch(EARTH_EPOCH_UNIX + EARTH_SECS_PER_VANA_DAY),
            1440
        );
        // Pre-epoch clamps to 0 like the formatter.
        assert_eq!(vana_minutes_since_epoch(0), 0);
    }
}
