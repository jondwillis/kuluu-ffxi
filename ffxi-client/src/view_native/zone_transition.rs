use bevy::picking::Pickable;
use bevy::prelude::*;

use ffxi_viewer_core::dat_mzb::{LastAutoLoadedZone, LoadMzbInFlight};
use ffxi_viewer_core::SceneState;
use ffxi_viewer_wire::Stage;

use super::AppPhase;

const FADE_OUT_SECS: f32 = 0.2;

const FADE_IN_SECS: f32 = 0.4;

const FADE_HOLD_MIN_SECS: f32 = 0.35;

const MAX_HOLD_SECS: f32 = 15.0;

const LOADING_TEXT: &str = "Downloading data";

const DOT_FRAMES: [&str; 4] = ["   ", ".  ", ".. ", "..."];

const DOT_PERIOD_SECS: f32 = 0.4;

#[derive(Resource, Default, Clone, Copy, PartialEq, Debug)]
enum ZoneOverlayFade {
    #[default]
    Idle,
    FadingOut {
        elapsed: f32,
    },
    Holding {
        elapsed: f32,
    },
    FadingIn {
        elapsed: f32,
    },
}

impl ZoneOverlayFade {
    fn alpha(&self) -> f32 {
        match *self {
            ZoneOverlayFade::Idle => 0.0,
            ZoneOverlayFade::FadingOut { elapsed } => (elapsed / FADE_OUT_SECS).clamp(0.0, 1.0),
            ZoneOverlayFade::Holding { .. } => 1.0,
            ZoneOverlayFade::FadingIn { elapsed } => 1.0 - (elapsed / FADE_IN_SECS).clamp(0.0, 1.0),
        }
    }
}

fn tick(state: ZoneOverlayFade, dt: f32, ready: bool) -> ZoneOverlayFade {
    match state {
        ZoneOverlayFade::Idle => ZoneOverlayFade::Idle,
        ZoneOverlayFade::FadingOut { elapsed } => {
            let next = elapsed + dt;
            if next >= FADE_OUT_SECS {
                ZoneOverlayFade::Holding { elapsed: 0.0 }
            } else {
                ZoneOverlayFade::FadingOut { elapsed: next }
            }
        }
        ZoneOverlayFade::Holding { elapsed } => {
            let next = elapsed + dt;
            if (next >= FADE_HOLD_MIN_SECS && ready) || next >= MAX_HOLD_SECS {
                ZoneOverlayFade::FadingIn { elapsed: 0.0 }
            } else {
                ZoneOverlayFade::Holding { elapsed: next }
            }
        }
        ZoneOverlayFade::FadingIn { elapsed } => {
            let next = elapsed + dt;
            if next >= FADE_IN_SECS {
                ZoneOverlayFade::Idle
            } else {
                ZoneOverlayFade::FadingIn { elapsed: next }
            }
        }
    }
}

#[derive(Resource, Default)]
struct HudVisibilityStash(std::collections::HashMap<Entity, Visibility>);

#[derive(Resource, Default)]
struct LoadingDots {
    elapsed: f32,
    last_frame: usize,
}

#[derive(Component)]
struct ZoneOverlayRoot;

#[derive(Component)]
struct ZoneOverlayLabel;

type HudRootFilter = (With<Node>, Without<ChildOf>, Without<ZoneOverlayRoot>);

pub struct ZoneTransitionOverlayPlugin;

impl Plugin for ZoneTransitionOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneOverlayFade>()
            .init_resource::<HudVisibilityStash>()
            .init_resource::<LoadingDots>()
            .add_systems(OnEnter(AppPhase::InGame), spawn_zone_overlay)
            .add_systems(
                Update,
                (drive_zone_overlay_fade, apply_zone_overlay_alpha)
                    .chain()
                    .run_if(in_state(AppPhase::InGame)),
            );
    }
}

fn spawn_zone_overlay(
    mut commands: Commands,
    mut fade: ResMut<ZoneOverlayFade>,
    mut stash: ResMut<HudVisibilityStash>,
) {
    *fade = ZoneOverlayFade::Holding { elapsed: 0.0 };
    stash.0.clear();

    commands
        .spawn((
            super::InGameEntity,
            ZoneOverlayRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),

                align_items: AlignItems::FlexEnd,
                justify_content: JustifyContent::FlexEnd,
                padding: UiRect {
                    right: Val::Px(40.0),
                    bottom: Val::Px(28.0),
                    ..default()
                },
                ..default()
            },
            BackgroundColor(Color::BLACK.with_alpha(1.0)),
            GlobalZIndex(i32::MAX),
            Pickable::IGNORE,
        ))
        .with_children(|p| {
            p.spawn((
                ZoneOverlayLabel,
                Text::new(format!("{LOADING_TEXT}{}", DOT_FRAMES[0])),
                TextFont {
                    font_size: 20.0,
                    ..default()
                },
                TextColor(Color::WHITE.with_alpha(1.0)),
                Pickable::IGNORE,
            ));
        });
}

fn drive_zone_overlay_fade(
    time: Res<Time>,
    scene: Res<SceneState>,
    mzb_in_flight: Res<LoadMzbInFlight>,
    last_auto: Res<LastAutoLoadedZone>,
    mut fade: ResMut<ZoneOverlayFade>,
    mut stash: ResMut<HudVisibilityStash>,
    mut hud_roots: Query<(Entity, &mut Visibility), HudRootFilter>,
) {
    let stage = scene.snapshot.stage;

    if stage == Stage::Zoning
        && matches!(
            *fade,
            ZoneOverlayFade::Idle | ZoneOverlayFade::FadingIn { .. }
        )
    {
        *fade = ZoneOverlayFade::FadingOut { elapsed: 0.0 };

        if stash.0.is_empty() {
            for (e, mut vis) in hud_roots.iter_mut() {
                stash.0.insert(e, *vis);
                *vis = Visibility::Hidden;
            }
        }
    }

    let ready = stage == Stage::InZone
        && last_auto.zone_id.is_some()
        && last_auto.zone_id == scene.snapshot.zone_id
        && mzb_in_flight.tasks.is_empty();

    let prev = *fade;
    *fade = tick(*fade, time.delta_secs(), ready);

    if !matches!(prev, ZoneOverlayFade::Idle)
        && *fade == ZoneOverlayFade::Idle
        && !stash.0.is_empty()
    {
        for (e, mut vis) in hud_roots.iter_mut() {
            if let Some(prev_vis) = stash.0.get(&e) {
                *vis = *prev_vis;
            }
        }
        stash.0.clear();
    }
}

fn apply_zone_overlay_alpha(
    fade: Res<ZoneOverlayFade>,
    time: Res<Time>,
    mut dots: ResMut<LoadingDots>,
    mut root_q: Query<(&mut BackgroundColor, &mut Node), With<ZoneOverlayRoot>>,
    mut label_q: Query<(&mut Text, &mut TextColor), With<ZoneOverlayLabel>>,
) {
    let alpha = fade.alpha();
    let idle = matches!(*fade, ZoneOverlayFade::Idle);
    let want_display = if idle { Display::None } else { Display::Flex };
    if let Ok((mut bg, mut node)) = root_q.single_mut() {
        if node.display != want_display {
            node.display = want_display;
        }
        if (bg.0.alpha() - alpha).abs() > 0.001 {
            bg.0 = Color::BLACK.with_alpha(alpha);
        }
    }

    if idle {
        dots.elapsed = 0.0;
    } else {
        dots.elapsed += time.delta_secs();
    }
    let frame = ((dots.elapsed / DOT_PERIOD_SECS) as usize) % DOT_FRAMES.len();
    if let Ok((mut text, mut tc)) = label_q.single_mut() {
        if frame != dots.last_frame {
            dots.last_frame = frame;
            **text = format!("{LOADING_TEXT}{}", DOT_FRAMES[frame]);
        }
        if (tc.0.alpha() - alpha).abs() > 0.001 {
            tc.0 = Color::WHITE.with_alpha(alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_edges() {
        assert_eq!(ZoneOverlayFade::Idle.alpha(), 0.0);
        assert_eq!((ZoneOverlayFade::Holding { elapsed: 0.0 }).alpha(), 1.0);

        let half_out = ZoneOverlayFade::FadingOut {
            elapsed: FADE_OUT_SECS / 2.0,
        }
        .alpha();
        assert!((half_out - 0.5).abs() < 0.01, "fade-out mid: {half_out}");
        let half_in = ZoneOverlayFade::FadingIn {
            elapsed: FADE_IN_SECS / 2.0,
        }
        .alpha();
        assert!((half_in - 0.5).abs() < 0.01, "fade-in mid: {half_in}");
    }

    #[test]
    fn fade_out_advances_to_hold() {
        let s = tick(
            ZoneOverlayFade::FadingOut { elapsed: 0.0 },
            FADE_OUT_SECS,
            false,
        );
        assert_eq!(s, ZoneOverlayFade::Holding { elapsed: 0.0 });
    }

    #[test]
    fn hold_waits_for_ready() {
        let s = tick(
            ZoneOverlayFade::Holding {
                elapsed: FADE_HOLD_MIN_SECS,
            },
            0.016,
            false,
        );
        assert!(matches!(s, ZoneOverlayFade::Holding { .. }));

        let s = tick(
            ZoneOverlayFade::Holding {
                elapsed: FADE_HOLD_MIN_SECS,
            },
            0.016,
            true,
        );
        assert_eq!(s, ZoneOverlayFade::FadingIn { elapsed: 0.0 });
    }

    #[test]
    fn hold_does_not_fade_in_before_minimum_even_if_ready() {
        let s = tick(ZoneOverlayFade::Holding { elapsed: 0.0 }, 0.016, true);
        assert!(matches!(s, ZoneOverlayFade::Holding { .. }));
    }

    #[test]
    fn hold_times_out_without_ready() {
        let s = tick(
            ZoneOverlayFade::Holding {
                elapsed: MAX_HOLD_SECS,
            },
            0.016,
            false,
        );
        assert_eq!(s, ZoneOverlayFade::FadingIn { elapsed: 0.0 });
    }

    #[test]
    fn fade_in_completes_to_idle() {
        let s = tick(
            ZoneOverlayFade::FadingIn { elapsed: 0.0 },
            FADE_IN_SECS,
            false,
        );
        assert_eq!(s, ZoneOverlayFade::Idle);
    }
}
