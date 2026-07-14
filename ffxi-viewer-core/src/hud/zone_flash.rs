use bevy::prelude::*;
use ffxi_viewer_wire::MyRoom;

use crate::hud::palette;
use crate::snapshot::SceneState;

#[derive(Resource)]
pub struct ZoneNameResolver(pub Box<dyn Fn(u16) -> Option<&'static str> + Send + Sync>);

impl ZoneNameResolver {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(u16) -> Option<&'static str> + Send + Sync + 'static,
    {
        Self(Box::new(f))
    }
}

#[derive(Resource, Default, Debug)]
pub struct ZoneFlashState {
    pub last_zone_id: Option<u16>,
    pub last_myroom: Option<MyRoom>,
    pub since_change: Option<f32>,
}

const HOLD_SECS: f32 = 1.5;
const FADE_SECS: f32 = 1.0;
const TOTAL_SECS: f32 = HOLD_SECS + FADE_SECS;

// research/xim .../resource/table/ZoneTables.kt:73-81: inside the Mog House the
// retail banner shows the floor name instead of the zone-name table entry.
const MOG_HOUSE_1F_NAME: &str = "Mog House 1F";
const MOG_HOUSE_2F_NAME: &str = "Mog House 2F";

pub fn mog_house_display_name(myroom: MyRoom) -> &'static str {
    if myroom.sub_map == ffxi_proto::decode::ServerLoginMyroom::SUB_MAP_2F {
        MOG_HOUSE_2F_NAME
    } else {
        MOG_HOUSE_1F_NAME
    }
}

#[derive(Component)]
pub struct ZoneFlashBanner;

#[derive(Component)]
pub struct ZoneFlashLabel;

pub fn spawn_zone_flash(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            ZoneFlashBanner,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(20.0),
                left: Val::Percent(50.0),
                margin: UiRect {
                    left: Val::Px(-150.0),
                    ..default()
                },
                width: Val::Px(300.0),
                padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                justify_content: JustifyContent::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|p| {
            p.spawn((
                ZoneFlashLabel,
                Text::new(""),
                TextFont {
                    font_size: 20.0.into(),
                    ..default()
                },
                TextColor(palette::ACCENT),
            ));
        });
}

pub fn update_zone_flash(
    state: Res<SceneState>,
    time: Res<Time>,
    resolver: Option<Res<ZoneNameResolver>>,
    mut flash: ResMut<ZoneFlashState>,
    mut banner_q: Query<(&mut Node, &mut BackgroundColor, &mut BorderColor), With<ZoneFlashBanner>>,
    mut label_q: Query<(&mut Text, &mut TextColor), With<ZoneFlashLabel>>,
) {
    let current_zone = state.snapshot.zone_id;
    let current_myroom = state.snapshot.myroom;

    if current_zone.is_some()
        && (current_zone != flash.last_zone_id || current_myroom != flash.last_myroom)
    {
        flash.last_zone_id = current_zone;
        flash.last_myroom = current_myroom;
        flash.since_change = Some(0.0);
    } else if let Some(t) = flash.since_change.as_mut() {
        *t += time.delta_secs();
    }

    let Ok((mut node, mut bg, mut border)) = banner_q.single_mut() else {
        return;
    };
    let Ok((mut text, mut tc)) = label_q.single_mut() else {
        return;
    };

    let Some(t) = flash.since_change else {
        if node.display != Display::None {
            node.display = Display::None;
        }
        return;
    };

    if t >= TOTAL_SECS {
        if node.display != Display::None {
            node.display = Display::None;
        }
        flash.since_change = None;
        return;
    }

    if node.display != Display::Flex {
        node.display = Display::Flex;
    }

    let zone_id = current_zone.unwrap_or(0);
    let name = match current_myroom {
        Some(myroom) => mog_house_display_name(myroom).to_string(),
        None => resolver
            .as_ref()
            .and_then(|r| (r.0)(zone_id))
            .map(|s| s.replace('_', " "))
            .unwrap_or_else(|| format!("Zone #{zone_id}")),
    };
    if **text != name {
        **text = name;
    }

    let alpha = if t < HOLD_SECS {
        1.0
    } else {
        let fade_t = (t - HOLD_SECS) / FADE_SECS;
        1.0 - fade_t.clamp(0.0, 1.0)
    };
    bg.0 = with_alpha(palette::BACKGROUND, alpha * 0.85);
    *border = BorderColor::all(with_alpha(palette::ACCENT, alpha));
    tc.0 = with_alpha(palette::ACCENT, alpha);
}

fn with_alpha(c: Color, a: f32) -> Color {
    let s = c.to_srgba();
    Color::srgba(s.red, s.green, s.blue, a.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compute_alpha(t: f32) -> f32 {
        if t < HOLD_SECS {
            1.0
        } else if t >= TOTAL_SECS {
            0.0
        } else {
            let fade_t = (t - HOLD_SECS) / FADE_SECS;
            1.0 - fade_t.clamp(0.0, 1.0)
        }
    }

    #[test]
    fn alpha_full_during_hold() {
        assert_eq!(compute_alpha(0.0), 1.0);
        assert_eq!(compute_alpha(HOLD_SECS - 0.001), 1.0);
    }

    #[test]
    fn alpha_starts_fading_after_hold() {
        let mid = HOLD_SECS + FADE_SECS / 2.0;
        let a = compute_alpha(mid);
        assert!(a > 0.4 && a < 0.6, "expected ~0.5, got {a}");
    }

    #[test]
    fn alpha_zero_after_total() {
        assert_eq!(compute_alpha(TOTAL_SECS), 0.0);
        assert_eq!(compute_alpha(TOTAL_SECS + 1.0), 0.0);
    }

    #[test]
    fn mog_house_name_selects_floor_by_sub_map() {
        assert_eq!(
            mog_house_display_name(MyRoom {
                model: 256,
                sub_map: 0
            }),
            "Mog House 1F"
        );
        assert_eq!(
            mog_house_display_name(MyRoom {
                model: 615,
                sub_map: ffxi_proto::decode::ServerLoginMyroom::SUB_MAP_2F
            }),
            "Mog House 2F"
        );
    }
}
