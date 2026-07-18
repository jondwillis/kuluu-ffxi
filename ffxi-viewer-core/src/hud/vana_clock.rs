use bevy::prelude::*;

use crate::hud::style::{self, theme};

pub use ffxi_proto::vana_time::VANA_EPOCH_UNIX as EARTH_EPOCH_UNIX;

pub const EARTH_SECS_PER_VANA_HOUR: u64 = 144;

pub const EARTH_SECS_PER_VANA_DAY: u64 = EARTH_SECS_PER_VANA_HOUR * 24;

// vendor/server/src/common/vanadiel_clock.h:40-42 — week = 8 Vana days,
// month = 30, year = 360; vendor/server/src/common/vana_time.h:129-135 —
// get_year counts years since 886.
pub const VANA_DAYS_PER_WEEK: u64 = 8;
pub const VANA_DAYS_PER_MONTH: u64 = 30;
pub const VANA_DAYS_PER_YEAR: u64 = 360;
pub const VANA_BASE_YEAR: u64 = 886;

// Placeholder cell when no PlayerMapGrid is available: pre-load on native, and
// always on wasm, where crate::minimap (the grid's source) is compiled out
// (kuluu-ehye).
const GRID_CELL_UNKNOWN: &str = "(?-?)";

const FRAMES_GROUP: &str = "menu    frames  ";
const DAY_ORB_BASE_INDEX: usize = 106;
const ORB_SIZE_PX: f32 = 14.0;

#[derive(Resource, Debug, Clone, Copy)]
pub struct VanaClockVisible(pub bool);

impl Default for VanaClockVisible {
    fn default() -> Self {
        Self(true)
    }
}

#[derive(Component)]
pub struct VanaClockPanel;

#[derive(Component)]
pub struct VanaClockLabel;

#[derive(Component)]
pub struct VanaClockOrb;

pub fn spawn_vana_clock_as_child(p: &mut ChildSpawnerCommands) {
    p.spawn((
        VanaClockPanel,
        Node {
            flex_shrink: 0.0,
            align_items: AlignItems::Center,
            padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(theme::FRAME_BG),
        BorderColor::all(theme::FRAME_EDGE),
    ))
    .with_children(|p| {
        p.spawn((
            VanaClockOrb,
            Node {
                width: Val::Px(ORB_SIZE_PX),
                height: Val::Px(ORB_SIZE_PX),
                margin: UiRect::right(Val::Px(4.0)),
                display: Display::None,
                ..default()
            },
            ImageNode::new(Handle::default()),
        ));
        p.spawn((
            VanaClockLabel,
            Text::new("0:00   (?-?)"),
            style::text_font(12.0),
            TextColor(theme::TEXT),
        ));
    });
}

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
    // DAY_ORB_BASE_INDEX + this (research/xim/.../ui/Compass.kt:43-54).
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

pub fn update_vana_clock(
    mut q: Query<&mut Text, With<VanaClockLabel>>,
    mut orb_q: Query<(&mut Node, &mut ImageNode), With<VanaClockOrb>>,
    #[cfg(not(target_arch = "wasm32"))] q_self: Query<&Transform, With<crate::components::IsSelf>>,
    #[cfg(not(target_arch = "wasm32"))] grid: Option<Res<crate::minimap::retail::PlayerMapGrid>>,
    atlas: Option<ResMut<crate::ui_element_atlas::UiElementAtlas>>,
    dat_root: Option<Res<crate::ui_element_atlas::UiElementDatRoot>>,
    mut images: ResMut<Assets<Image>>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    mut prev_vana_day: Local<Option<u64>>,
) {
    let Ok(mut text) = q.single_mut() else {
        return;
    };

    let earth_now = vana_clock.earth_unix_secs_now();
    #[cfg(not(target_arch = "wasm32"))]
    let cell = player_grid_cell(grid.as_deref(), q_self.single().ok());
    #[cfg(target_arch = "wasm32")]
    let cell = GRID_CELL_UNKNOWN;
    let want = format!("{}   {}", format_vana_time(earth_now), cell);
    if **text != want {
        **text = want;
    }

    let earth_since_vana = earth_now.saturating_sub(EARTH_EPOCH_UNIX);
    let total_vana_days = earth_since_vana / EARTH_SECS_PER_VANA_DAY;
    if *prev_vana_day != Some(total_vana_days) {
        if let Some(prev) = *prev_vana_day {
            if prev != total_vana_days {
                let weekday = VanaWeekday::from_vana_day(total_vana_days).name();
                toasts.write(crate::snapshot::ToastEvent::system(format!(
                    "📅 Vana day {} — {}",
                    total_vana_days, weekday,
                )));
            }
        }
        update_day_orb(&mut orb_q, total_vana_days, atlas, dat_root, &mut images);
    }
    *prev_vana_day = Some(total_vana_days);
}

fn update_day_orb(
    orb_q: &mut Query<(&mut Node, &mut ImageNode), With<VanaClockOrb>>,
    total_vana_days: u64,
    atlas: Option<ResMut<crate::ui_element_atlas::UiElementAtlas>>,
    dat_root: Option<Res<crate::ui_element_atlas::UiElementDatRoot>>,
    images: &mut Assets<Image>,
) {
    let Ok((mut node, mut image_node)) = orb_q.single_mut() else {
        return;
    };
    let (Some(mut atlas), Some(dat_root)) = (atlas, dat_root) else {
        return;
    };
    let index = DAY_ORB_BASE_INDEX + VanaWeekday::from_vana_day(total_vana_days).element_index();
    match atlas.ensure(FRAMES_GROUP, index, &dat_root, images) {
        Some(handle) => {
            image_node.image = handle;
            node.display = Display::Flex;
        }
        None => {
            node.display = Display::None;
        }
    }
}

// Also keyed off Added<VanaClockPanel>: the panel is despawned with the
// BottomLeftStack's InGameEntity on zone change, and the respawned entity must
// pick up a hidden state whose resource change tick has already been consumed.
pub fn apply_vana_clock_visibility(
    visible: Res<VanaClockVisible>,
    added: Query<(), Added<VanaClockPanel>>,
    mut q: Query<&mut Node, With<VanaClockPanel>>,
) {
    if !visible.is_changed() && added.is_empty() {
        return;
    }
    let want = if visible.0 {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in q.iter_mut() {
        if node.display != want {
            node.display = want;
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
    // vendor/server/src/common/vana_time.h:106-143 calendar getters over the
    // vendor/server/src/common/vanadiel_clock.h:35-42 ratios. LSB's
    // get_monthday/get_month ceil partial days/months (vana_time.h:118,126),
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

// Line wording is provisional pending a retail capture of the Current Time
// menu output (bead kuluu-y5hq retail_unknowns); correct it here in one place.
pub const VANA_TIME_LINE_PREFIX: &str = "Vana'diel Time: ";
pub const EARTH_TIME_LINE_PREFIX: &str = "Earth Time: ";
const EARTH_TIME_FORMAT: &str = "%Y/%m/%d %H:%M:%S";

pub fn vana_time_chat_line(earth_unix_secs: u64) -> String {
    let date = VanaDate::from_earth_unix(earth_unix_secs);
    format!(
        "{VANA_TIME_LINE_PREFIX}{}, {}, {}/{}/{} C.E.",
        date.weekday.name(),
        format_vana_time(earth_unix_secs),
        date.day,
        date.month,
        date.year,
    )
}

pub fn earth_time_chat_line(earth_unix_secs: u64) -> String {
    format!(
        "{EARTH_TIME_LINE_PREFIX}{}",
        earth_time_text(&chrono::Local, earth_unix_secs)
    )
}

fn earth_time_text<Tz: chrono::TimeZone>(tz: &Tz, earth_unix_secs: u64) -> String
where
    Tz::Offset: std::fmt::Display,
{
    match tz.timestamp_opt(earth_unix_secs as i64, 0).single() {
        Some(dt) => dt.format(EARTH_TIME_FORMAT).to_string(),
        None => format!("unix {earth_unix_secs}"),
    }
}

pub fn current_time_chat_lines(clock: &crate::vana_time::VanaClock) -> [String; 2] {
    let earth = clock.earth_unix_secs_now();
    [vana_time_chat_line(earth), earth_time_chat_line(earth)]
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

#[cfg(not(target_arch = "wasm32"))]
fn player_grid_cell(
    grid: Option<&crate::minimap::retail::PlayerMapGrid>,
    player: Option<&Transform>,
) -> String {
    match (grid.and_then(|g| g.aabb), player) {
        (Some(aabb), Some(tf)) => {
            let (col, row) = aabb.world_to_grid(tf.translation);
            format!("({col}-{row})")
        }
        _ => GRID_CELL_UNKNOWN.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_orb_index_maps_weekday_to_element_sprite() {
        // Firesday->Fire(106), Earthsday->Earth(109), Watersday->Water(111),
        // Windsday->Wind(108), Iceday->Ice(107), Lightningday->Lightning(110),
        // Lightsday->Light(112), Darksday->Dark(113). (Compass.kt:43-54)
        let expected = [106, 109, 111, 108, 107, 110, 112, 113];
        for (day, want) in expected.iter().enumerate() {
            let weekday = VanaWeekday::from_vana_day(day as u64);
            assert_eq!(DAY_ORB_BASE_INDEX + weekday.element_index(), *want);
        }
    }

    #[test]
    fn player_grid_cell_without_grid_matches_wasm_placeholder() {
        // The wasm build of update_vana_clock substitutes GRID_CELL_UNKNOWN
        // directly (crate::minimap is compiled out there, kuluu-ehye); this pins
        // the native no-grid fallback to the same string so the two targets
        // render identically before a map grid loads.
        assert_eq!(player_grid_cell(None, None), GRID_CELL_UNKNOWN);
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
        // vendor/server/scripts/globals/chocobo_raising.lua:52 — one Vana'diel
        // day is 3456 Earth seconds.
        assert_eq!(EARTH_SECS_PER_VANA_DAY, 3456);
        let date = VanaDate::from_earth_unix(EARTH_EPOCH_UNIX + EARTH_SECS_PER_VANA_DAY);
        assert_eq!((date.day, date.month, date.year), (2, 1, VANA_BASE_YEAR));
        assert_eq!(date.weekday, VanaWeekday::Earthsday);
    }

    #[test]
    fn one_vana_week_wraps_the_weekday() {
        // vendor/server/scripts/globals/chocobo_raising.lua:51 — one Vana'diel
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

    #[test]
    fn vana_chat_line_snapshot() {
        let ts = EARTH_EPOCH_UNIX
            + VANA_DAYS_PER_MONTH * EARTH_SECS_PER_VANA_DAY
            + 13 * EARTH_SECS_PER_VANA_HOUR
            + 5 * EARTH_SECS_PER_VANA_HOUR / 60;
        assert_eq!(
            vana_time_chat_line(ts),
            "Vana'diel Time: Lightsday, 13:05, 1/2/886 C.E."
        );
        assert!(vana_time_chat_line(ts).starts_with(VANA_TIME_LINE_PREFIX));
    }

    #[test]
    fn earth_chat_line_formats_a_civil_datetime() {
        // The Vana'diel epoch is 2001-12-31 15:00:00 UTC (2002-01-01 00:00 JST,
        // vendor/server/src/common/earth_time.h:40).
        assert_eq!(
            earth_time_text(&chrono::Utc, EARTH_EPOCH_UNIX),
            "2001/12/31 15:00:00"
        );
        assert!(earth_time_chat_line(EARTH_EPOCH_UNIX).starts_with(EARTH_TIME_LINE_PREFIX));
    }

    #[test]
    fn current_time_lines_use_the_exported_prefixes() {
        let clock = crate::vana_time::VanaClock::anchored_at_hour(12.0);
        let [vana, earth] = current_time_chat_lines(&clock);
        assert!(vana.starts_with(VANA_TIME_LINE_PREFIX), "{vana:?}");
        assert!(earth.starts_with(EARTH_TIME_LINE_PREFIX), "{earth:?}");
    }

    #[test]
    fn visibility_apply_flips_display_and_covers_respawn() {
        let mut app = App::new();
        app.init_resource::<VanaClockVisible>();
        app.add_systems(Update, apply_vana_clock_visibility);
        let panel = app
            .world_mut()
            .spawn((VanaClockPanel, Node::default()))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Node>(panel).unwrap().display,
            Display::Flex
        );

        app.world_mut().resource_mut::<VanaClockVisible>().0 = false;
        app.update();
        assert_eq!(
            app.world().get::<Node>(panel).unwrap().display,
            Display::None
        );

        // A zone-change respawn arrives after the resource change tick was
        // consumed; the Added<VanaClockPanel> key must still hide it.
        let respawned = app
            .world_mut()
            .spawn((VanaClockPanel, Node::default()))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Node>(respawned).unwrap().display,
            Display::None
        );

        app.world_mut().resource_mut::<VanaClockVisible>().0 = true;
        app.update();
        assert_eq!(
            app.world().get::<Node>(panel).unwrap().display,
            Display::Flex
        );
        assert_eq!(
            app.world().get::<Node>(respawned).unwrap().display,
            Display::Flex
        );
    }
}
