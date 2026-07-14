use bevy::prelude::*;

use crate::hud::palette;

pub const EARTH_EPOCH_UNIX: u64 = 1_009_810_800;

pub const EARTH_SECS_PER_VANA_HOUR: u64 = 144;

pub const EARTH_SECS_PER_VANA_DAY: u64 = EARTH_SECS_PER_VANA_HOUR * 24;

const FRAMES_GROUP: &str = "menu    frames  ";
const DAY_ORB_BASE_INDEX: usize = 106;
const ORB_SIZE_PX: f32 = 14.0;

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
        BackgroundColor(palette::BACKGROUND),
        BorderColor::all(palette::BORDER),
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
            TextFont {
                font_size: 12.0.into(),
                ..default()
            },
            TextColor(palette::TEXT),
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
        Self::ORDER[(total_vana_days % 8) as usize]
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
    q_self: Query<&Transform, With<crate::components::IsSelf>>,
    grid: Option<Res<crate::minimap::retail::PlayerMapGrid>>,
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
    let cell = player_grid_cell(grid.as_deref(), q_self.single().ok());
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
