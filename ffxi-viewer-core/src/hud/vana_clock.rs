//! Vana'diel clock HUD.
//!
//! Vana'dielian time is a deterministic transform of Earth time. We
//! match LSB's authoritative ratio (`vendor/server/src/common/vanadiel_clock.h`):
//!
//! - 1 Earth second = 25 Vana'diel seconds.
//! - 1 V-minute  = 2.4 Earth seconds.
//! - 1 V-hour    = 144 Earth seconds (= 2.4 min).
//! - 1 V-day     = 3456 Earth seconds (≈ 57.6 min).
//! - Vana'diel epoch is Earth 2002-01-01 00:00 JST (Unix 1009810800);
//!   see `vendor/server/src/common/earth_time.h:40`.
//!
//! Time can come from one of two sources, in priority order:
//!   1. The `VanaClock` resource (server-authoritative, seeded by the
//!      `GameTime` field of the 0x00A LOGIN packet).
//!   2. System time fallback before any server packet has arrived.
//!
//! Always prefer reading via [`crate::vana_time::current_vana`] rather
//! than `SystemTime::now()` directly — this module only owns the HUD
//! widget and the pure formulas.

use bevy::prelude::*;

use crate::hud::palette;

/// Earth epoch of Vana'diel 0886-01-01 00:00 (LSB's `vanadiel_epoch`),
/// in Unix seconds. 2002-01-01 00:00 JST.
pub const EARTH_EPOCH_UNIX: u64 = 1_009_810_800;
/// Vana'diel epoch year offset.
const VANA_EPOCH_YEAR: u32 = 886;
/// Earth seconds per Vana'diel hour. 1 Earth-sec = 25 Vana-sec ⇒ 1 V-hour = 144 Earth-sec.
pub const EARTH_SECS_PER_VANA_HOUR: u64 = 144;
/// Earth seconds per Vana'diel day.
pub const EARTH_SECS_PER_VANA_DAY: u64 = EARTH_SECS_PER_VANA_HOUR * 24;
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

/// FFXI 8-day week. Indexed by `total_vana_days % 8`; epoch (886/1/1)
/// is Firesday per `vendor/server/settings/default/map.lua:77` and the
/// authoritative ordering is `vendor/server/scripts/commands/time.lua`.
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

    // Vana day rollover → System chat. The format string in the HUD
    // label changes once per V-minute; we want one toast per V-day
    // (every ~10 Earth minutes), so track the day index separately
    // rather than diffing strings. First call seeds without firing
    // so logging in mid-day doesn't immediately announce the current
    // day as if it were a rollover.
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

/// Monotonic Vana minutes since the V-epoch (0886-01-01 00:00).
///
/// Use this when you need a single increasing scalar across days —
/// e.g. when a Scheduler-like timeline straddles a Vana-day rollover,
/// or when comparing two Vana timestamps across an arbitrary span.
/// For per-action timelines stay frame-based (see
/// `ffxi_dat::scheduler::TimedStage::frame`); for in-day animations
/// use [`crate::sun_moon::VanaSky::hour`].
pub fn vana_minutes_since_epoch(earth_unix_secs: u64) -> u64 {
    let earth_since_vana = earth_unix_secs.saturating_sub(EARTH_EPOCH_UNIX);
    // 1 Earth-sec = 25 Vana-sec ⇒ Vana-min = Earth-sec * 25 / 60.
    earth_since_vana.saturating_mul(25) / 60
}

/// Convert `earth_unix_secs` to a `V YYYY-MM-DD HH:MM` string.
pub fn format_vana(earth_unix_secs: u64) -> String {
    let earth_since_vana = earth_unix_secs.saturating_sub(EARTH_EPOCH_UNIX);
    // Express everything in total Vana minutes for clean integer math.
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
        // One V-hour = 60 V-min = 144 Earth-sec.
        assert_eq!(vana_minutes_since_epoch(EARTH_EPOCH_UNIX), 0);
        assert_eq!(
            vana_minutes_since_epoch(EARTH_EPOCH_UNIX + EARTH_SECS_PER_VANA_HOUR),
            60
        );
        // A full V-day = 24 × 60 = 1440 V-min.
        assert_eq!(
            vana_minutes_since_epoch(EARTH_EPOCH_UNIX + EARTH_SECS_PER_VANA_DAY),
            1440
        );
        // Pre-epoch clamps to 0 like the formatter.
        assert_eq!(vana_minutes_since_epoch(0), 0);
    }
}
