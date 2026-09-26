use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use kuluu_snapshot::ViewerEvent;

pub use ffxi_vocab::vana_time::VANA_EPOCH_UNIX as EARTH_EPOCH_UNIX;

pub const EARTH_SECS_PER_VANA_HOUR: u64 = 144;

pub const EARTH_SECS_PER_VANA_DAY: u64 = EARTH_SECS_PER_VANA_HOUR * 24;

// vendor/server/src/common/vanadiel_clock.h vanadiel_clock week_ratio — week = 8 Vana days,
// month = 30, year = 360; vendor/server/src/common/vana_time.h get_month —
// get_year counts years since 886.
pub const VANA_DAYS_PER_WEEK: u64 = 8;
pub const VANA_DAYS_PER_MONTH: u64 = 30;
pub const VANA_DAYS_PER_YEAR: u64 = 360;
pub const VANA_BASE_YEAR: u64 = 886;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VanaWeekday {
    Firesday,
    Earthsday,
    Watersday,
    Windsday,
    Iceday,
    Lightningday,
    Lightsday,
    Darksday,
}

impl VanaWeekday {
    const ORDER: [VanaWeekday; 8] = [
        Self::Firesday,
        Self::Earthsday,
        Self::Watersday,
        Self::Windsday,
        Self::Iceday,
        Self::Lightningday,
        Self::Lightsday,
        Self::Darksday,
    ];

    pub fn from_vana_day(total_vana_days: u64) -> Self {
        Self::ORDER[(total_vana_days % VANA_DAYS_PER_WEEK) as usize]
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Firesday => "Firesday",
            Self::Earthsday => "Earthsday",
            Self::Watersday => "Watersday",
            Self::Windsday => "Windsday",
            Self::Iceday => "Iceday",
            Self::Lightningday => "Lightningday",
            Self::Lightsday => "Lightsday",
            Self::Darksday => "Darksday",
        }
    }

    // Index of this day's element in the canonical FFXI element order
    // Fire, Ice, Wind, Earth, Lightning, Water, Light, Dark (ffxi-proto
    // decode.rs def_elem). The day-of-week orb sprite is
    // DAY_ORB_BASE_INDEX + this (research/xim/src/jsMain/kotlin/xim/poc/ui/Compass.kt drawClock dayOfWeekIndex).
    pub fn element_index(self) -> usize {
        match self {
            Self::Firesday => 0,
            Self::Iceday => 1,
            Self::Windsday => 2,
            Self::Earthsday => 3,
            Self::Lightningday => 4,
            Self::Watersday => 5,
            Self::Lightsday => 6,
            Self::Darksday => 7,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VanaDate {
    pub year: u64,
    pub month: u64,
    pub day: u64,
    pub weekday: VanaWeekday,
}

impl VanaDate {
    // vendor/server/src/common/vana_time.h get_hour calendar getters over the
    // vendor/server/src/common/vanadiel_clock.h vanadiel_clock millisecond_ratio ratios. LSB's
    // get_monthday/get_month ceil partial days/months (vana_time.h,126),
    // which transiently report the prior day/month during the exact boundary
    // second; floor+1 agrees at every other instant.
    pub fn from_earth_unix(earth_unix_secs: u64) -> Self {
        let total_days = earth_unix_secs.saturating_sub(EARTH_EPOCH_UNIX) / EARTH_SECS_PER_VANA_DAY;
        Self {
            year: VANA_BASE_YEAR + total_days / VANA_DAYS_PER_YEAR,
            month: (total_days % VANA_DAYS_PER_YEAR) / VANA_DAYS_PER_MONTH + 1,
            day: total_days % VANA_DAYS_PER_MONTH + 1,
            weekday: VanaWeekday::from_vana_day(total_days),
        }
    }
}

pub fn vana_minutes_since_epoch(earth_unix_secs: u64) -> u64 {
    let earth_since_vana = earth_unix_secs.saturating_sub(EARTH_EPOCH_UNIX);

    earth_since_vana.saturating_mul(25) / 60
}

// research/xim EnvironmentManager.kt getFullDayInterpolation: the clock-driven
// color tracks (ParticleUpdaters.kt ClockValueUpdater) sample at the fraction
// of the Vana'diel day elapsed, in [0, 1). One Vana day = 1440 Vana minutes.
pub fn full_day_fraction(earth_unix_secs: u64) -> f32 {
    let total_v_min = vana_minutes_since_epoch(earth_unix_secs);
    const VANA_MINUTES_PER_DAY: u64 = 24 * 60;
    (total_v_min % VANA_MINUTES_PER_DAY) as f32 / VANA_MINUTES_PER_DAY as f32
}

// research/XIClient/src/XIClient/source/World/XiDateTime.cpp XiDateTime::ConvertEarthSecondsToVanaHour
// ConvertEarthSecondsToVanaHour — `(seconds % EARTH_SECONDS_PER_GAME_DAY) /
// EARTH_SECONDS_PER_GAME_HOUR`, which is this same floor division.
pub fn vana_hour(earth_unix_secs: u64) -> u64 {
    (vana_minutes_since_epoch(earth_unix_secs) / 60) % 24
}

pub fn format_vana_time(earth_unix_secs: u64) -> String {
    let v_minute = vana_minutes_since_epoch(earth_unix_secs) % 60;
    let v_hour = vana_hour(earth_unix_secs);
    format!("{v_hour}:{v_minute:02}")
}

#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct VanaClock {
    anchor_earth_unix: Option<u64>,

    anchor_instant: Option<Instant>,
    /// Fixed instant every reader sees while a cutscene holds the game clock; cleared by [`Self::thaw`].
    frozen_at_earth_unix: Option<f64>,
}

impl VanaClock {
    pub fn is_synced(&self) -> bool {
        self.anchor_earth_unix.is_some()
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen_at_earth_unix.is_some()
    }

    /// Hold every reader at the current instant until [`Self::thaw`].
    pub fn freeze(&mut self) {
        self.frozen_at_earth_unix = Some(self.earth_unix_now());
    }

    /// Hold every reader at Vana'diel hour `hour`, minute `minute`, on the day
    /// they are in now (research/XiEvents/OpCodes/0x0077.md SetHour/SetMinute).
    pub fn freeze_at_hour_minute(&mut self, hour: u32, minute: u32) {
        let since_epoch = (self.earth_unix_now() - EARTH_EPOCH_UNIX as f64).max(0.0);
        let day_index = (since_epoch / EARTH_SECS_PER_VANA_DAY as f64).floor();
        self.freeze_at_earth_unix(
            EARTH_EPOCH_UNIX as f64
                + day_index * EARTH_SECS_PER_VANA_DAY as f64
                + hour.rem_euclid(24) as f64 * EARTH_SECS_PER_VANA_HOUR as f64
                + minute as f64 * EARTH_SECS_PER_VANA_HOUR as f64 / 60.0,
        );
    }

    /// Hold every reader at Vana'diel day `day_from_epoch` (0-based from the
    /// calendar epoch) at hour `hour`, minute `minute`
    /// (research/XiEvents/OpCodes/0x00A9.md).
    pub fn freeze_at_day_hour_minute(&mut self, day_from_epoch: u32, hour: u32, minute: u32) {
        self.freeze_at_earth_unix(
            EARTH_EPOCH_UNIX as f64
                + day_from_epoch as f64 * EARTH_SECS_PER_VANA_DAY as f64
                + hour.rem_euclid(24) as f64 * EARTH_SECS_PER_VANA_HOUR as f64
                + minute as f64 * EARTH_SECS_PER_VANA_HOUR as f64 / 60.0,
        );
    }

    fn freeze_at_earth_unix(&mut self, earth_unix: f64) {
        self.frozen_at_earth_unix = Some(earth_unix);
    }

    /// Release the hold so readers resume from the live clock (research/XiEvents/OpCodes/0x0078.md EnableGameTimer).
    pub fn thaw(&mut self) {
        self.frozen_at_earth_unix = None;
    }

    pub fn earth_unix_now(&self) -> f64 {
        if let Some(frozen) = self.frozen_at_earth_unix {
            return frozen;
        }
        if let (Some(anchor), Some(instant)) = (self.anchor_earth_unix, self.anchor_instant) {
            anchor as f64 + instant.elapsed().as_secs_f64()
        } else {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(EARTH_EPOCH_UNIX as f64)
        }
    }

    pub fn earth_unix_secs_now(&self) -> u64 {
        self.earth_unix_now() as u64
    }

    fn anchor(&mut self, game_time: u32) {
        self.anchor_earth_unix = Some(EARTH_EPOCH_UNIX + game_time as u64);
        self.anchor_instant = Some(Instant::now());
    }

    // Pin the clock to a fixed Vana'diel hour (headless renders / tests).
    // `vana_sky_from_unix` maps game_time seconds at 25x: hour = game_time * 25 / 3600,
    // so one Vana hour is 144 anchor seconds.
    pub fn anchored_at_hour(hour: f32) -> Self {
        let game_time = (hour.rem_euclid(24.0) * 144.0) as u32;
        let mut clock = Self::default();
        clock.anchor(game_time);
        clock
    }
}

pub fn ingest_vana_time(
    events: Res<crate::snapshot::EventLog>,
    mut clock: ResMut<VanaClock>,
    mut last_seen_len: Local<usize>,
) {
    let len = events.recent.len();
    let start = (*last_seen_len).min(len);
    for ev in events.recent.iter().skip(start) {
        if let ViewerEvent::VanaTimeSynced { game_time } = ev {
            clock.anchor(*game_time);
        }
    }
    *last_seen_len = len;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsynced_clock_falls_back_to_system_time() {
        let clock = VanaClock::default();
        assert!(!clock.is_synced());

        assert!(clock.earth_unix_now() > EARTH_EPOCH_UNIX as f64);
    }

    #[test]
    fn synced_clock_uses_server_anchor() {
        let mut clock = VanaClock::default();
        clock.anchor(12345);
        assert!(clock.is_synced());
        let expected = (EARTH_EPOCH_UNIX + 12345) as f64;

        let now = clock.earth_unix_now();
        assert!(now >= expected);
        assert!(now < expected + 1.0);
    }

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
    fn vana_hour_matches_the_retail_floor_division() {
        // research/XIClient/src/XIClient/source/World/XiDateTime.cpp XiDateTime::ConvertEarthSecondsToVanaHour.
        for game_time in [0u64, 143, 144, 3455, 3456, 100_000, 1_234_567] {
            let retail = (game_time % EARTH_SECS_PER_VANA_DAY) / EARTH_SECS_PER_VANA_HOUR;
            assert_eq!(
                vana_hour(EARTH_EPOCH_UNIX + game_time),
                retail,
                "game_time {game_time}"
            );
        }
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

    #[test]
    fn epoch_is_firesday_first_of_year_886() {
        assert_eq!(
            VanaDate::from_earth_unix(EARTH_EPOCH_UNIX),
            VanaDate {
                year: VANA_BASE_YEAR,
                month: 1,
                day: 1,
                weekday: VanaWeekday::Firesday,
            }
        );
        assert_eq!(format_vana_time(EARTH_EPOCH_UNIX), "0:00");
    }

    #[test]
    fn one_vana_day_advances_the_monthday() {
        // vendor/server/scripts/globals/chocobo_raising.lua xi.chocoboRaising.dayLength — one Vana'diel
        // day is 3456 Earth seconds.
        assert_eq!(EARTH_SECS_PER_VANA_DAY, 3456);
        let date = VanaDate::from_earth_unix(EARTH_EPOCH_UNIX + EARTH_SECS_PER_VANA_DAY);
        assert_eq!((date.day, date.month, date.year), (2, 1, VANA_BASE_YEAR));
        assert_eq!(date.weekday, VanaWeekday::Earthsday);
    }

    #[test]
    fn one_vana_week_wraps_the_weekday() {
        // vendor/server/scripts/globals/chocobo_raising.lua xi.chocoboRaising.dayLength — one Vana'diel
        // week is 27648 Earth seconds (8 days).
        let week_secs = VANA_DAYS_PER_WEEK * EARTH_SECS_PER_VANA_DAY;
        assert_eq!(week_secs, 27_648);
        assert_eq!(
            VanaDate::from_earth_unix(EARTH_EPOCH_UNIX + week_secs).weekday,
            VanaWeekday::Firesday
        );
    }

    #[test]
    fn month_rolls_at_day_30_and_year_at_day_360() {
        let day = EARTH_SECS_PER_VANA_DAY;
        let last_of_month =
            VanaDate::from_earth_unix(EARTH_EPOCH_UNIX + (VANA_DAYS_PER_MONTH - 1) * day);
        assert_eq!((last_of_month.day, last_of_month.month), (30, 1));

        let first_of_next = VanaDate::from_earth_unix(EARTH_EPOCH_UNIX + VANA_DAYS_PER_MONTH * day);
        assert_eq!((first_of_next.day, first_of_next.month), (1, 2));

        let last_of_year =
            VanaDate::from_earth_unix(EARTH_EPOCH_UNIX + (VANA_DAYS_PER_YEAR - 1) * day);
        assert_eq!(
            (last_of_year.day, last_of_year.month, last_of_year.year),
            (30, 12, VANA_BASE_YEAR)
        );

        let new_year = VanaDate::from_earth_unix(EARTH_EPOCH_UNIX + VANA_DAYS_PER_YEAR * day);
        assert_eq!(
            (new_year.day, new_year.month, new_year.year),
            (1, 1, VANA_BASE_YEAR + 1)
        );
    }
}
