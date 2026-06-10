//! In-game zoning loading screen — retail-style "Downloading data" blackout.
//!
//! When the session zones between areas the wire `Stage` goes
//! `InZone → Zoning → InZone` (the intermediate `Zoning` spans the
//! between-map-servers reconnect, during which `SceneState.snapshot.zone_id`
//! is `None` — the "zone 0" transient). This plugin masks that whole window:
//! it hides the HUD, fades the screen to black, shows a bottom-right
//! "Downloading data" label (animated ellipsis, like retail), holds black
//! until the destination zone is both
//! position-seeded (`Stage::InZone` — gated on the server-authoritative
//! spawn seed in `session.rs`) and geometry-loaded, then fades back in and
//! restores the HUD.
//!
//! # Why this also fixes "work on zone 0 being visible"
//!
//! The black hold covers the entire `zone_id == None` transient — entities
//! cleared, old geometry despawned, the self avatar momentarily absent / the
//! camera holding its last transform — so none of that churn is ever on
//! screen. Combined with the `session.rs` seed-gate (the player can no longer
//! be reported to the server at the origin), the visible artifacts of the
//! transition disappear.
//!
//! # Structure
//!
//! Modeled on [`super::launcher_backdrop`]'s fade state machine, but backed
//! by a full-screen Bevy UI `Node` (it must cover the 3D world, carry text,
//! and sit above the HUD) instead of a camera-facing 3D quad. The fade math
//! ([`ZoneOverlayFade::alpha`] / [`tick`]) is pure and unit-tested.

use bevy::picking::Pickable;
use bevy::prelude::*;

use ffxi_viewer_core::dat_mzb::{LastAutoLoadedZone, LoadMzbInFlight};
use ffxi_viewer_core::SceneState;
use ffxi_viewer_wire::Stage;

use super::AppPhase;

/// Fade-to-black duration when a zone change begins (fast — the player
/// already committed to the zoneline; lingering on a half-dimmed scene
/// reads as lag).
const FADE_OUT_SECS: f32 = 0.2;
/// Fade-from-black duration once the destination zone is ready.
const FADE_IN_SECS: f32 = 0.4;
/// Minimum time to stay fully black before we'll even consider fading in.
/// Covers the one-frame race where the auto-loader has stamped
/// [`LastAutoLoadedZone`] for the new zone but [`LoadMzbInFlight`] hasn't
/// been populated yet — without this floor we could read "no tasks in
/// flight" as "geometry ready" on the very frame the load is kicked.
const FADE_HOLD_MIN_SECS: f32 = 0.35;
/// Hard cap on the black hold. A zone whose `Stage` never reaches `InZone`
/// (lost LOGIN) or whose geometry never settles still fades in eventually
/// rather than wedging the player behind a permanent black screen.
const MAX_HOLD_SECS: f32 = 15.0;

/// Loading-screen text, matching retail's bottom-right "Downloading data".
const LOADING_TEXT: &str = "Downloading data";
/// Animated-ellipsis frames. Trailing spaces pad each to a constant width
/// so [`LOADING_TEXT`] holds position as the dots cycle.
const DOT_FRAMES: [&str; 4] = ["   ", ".  ", ".. ", "..."];
/// Seconds each ellipsis frame holds before advancing.
const DOT_PERIOD_SECS: f32 = 0.4;

/// Crossfade state for the zoning blackout. `Idle` is the steady in-zone
/// state (overlay transparent + `Display::None` so it never intercepts
/// clicks). The session's `Stage::Zoning` drives `Idle → FadingOut`; the
/// destination zone becoming ready drives `Holding → FadingIn → Idle`.
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
    /// Overlay opacity in 0..=1.
    fn alpha(&self) -> f32 {
        match *self {
            ZoneOverlayFade::Idle => 0.0,
            ZoneOverlayFade::FadingOut { elapsed } => (elapsed / FADE_OUT_SECS).clamp(0.0, 1.0),
            ZoneOverlayFade::Holding { .. } => 1.0,
            ZoneOverlayFade::FadingIn { elapsed } => 1.0 - (elapsed / FADE_IN_SECS).clamp(0.0, 1.0),
        }
    }
}

/// Pure transition for the fade state machine. `ready` means the
/// destination zone is position-seeded and its geometry has settled (see
/// the caller). Kept free of Bevy types so the timing edges are testable
/// without a world. Note `Idle → FadingOut` is NOT here: that edge is
/// driven by the session `Stage::Zoning`, not by elapsed time.
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

/// Stash of each HUD root's pre-blackout `Visibility`, so we restore the
/// exact prior state on fade-in rather than force-showing panels their own
/// `Display` logic wants hidden. Non-empty == "HUD currently hidden by us",
/// which also makes [`set_hud_hidden`] idempotent across a fade-out that
/// restarts mid-cycle (back-to-back zonelines).
#[derive(Resource, Default)]
struct HudVisibilityStash(std::collections::HashMap<Entity, Visibility>);

/// Cadence accumulator for the loading label's animated ellipsis.
/// `elapsed` resets to 0 while the overlay is idle so each zone-in starts
/// dot-less; `last_frame` gates text rewrites to once per frame change.
#[derive(Resource, Default)]
struct LoadingDots {
    elapsed: f32,
    last_frame: usize,
}

/// Full-screen blackout node.
#[derive(Component)]
struct ZoneOverlayRoot;

/// The bottom-right "Downloading data" label (child of [`ZoneOverlayRoot`]).
#[derive(Component)]
struct ZoneOverlayLabel;

/// Root-level UI nodes we blanket-hide during a zone transition. Excludes
/// our own overlay (`Without<ZoneOverlayRoot>`); the overlay's label child
/// is excluded automatically by `Without<ChildOf>` (only roots match).
/// Gameplay HUD widgets toggle their own visibility via `Display`, not
/// `Visibility`, so overriding `Visibility` here is conflict-free; dev
/// panels do use `Visibility` (driven by `DevHud`), and stash/restore puts
/// those back exactly as found.
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

/// Spawn the blackout node (tagged `InGameEntity` so the shared
/// `OnExit(InGame)` despawn cleans it up) and arm the fade at full black so
/// the *first* zone-in fades the world in rather than flashing a
/// half-constructed scene. Resets the fade + stash for a clean re-login.
fn spawn_zone_overlay(
    mut commands: Commands,
    mut fade: ResMut<ZoneOverlayFade>,
    mut stash: ResMut<HudVisibilityStash>,
) {
    // Start fully black and let the readiness gate fade us in. `stash`
    // begins empty: the HUD spawns this same `OnEnter` (deferred Commands),
    // so there's nothing to hide yet — and it doesn't matter, the opaque
    // hold covers it until the fade-in reveal.
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
                // Anchor the label to the bottom-right corner like retail,
                // inset from the screen edge. Row main-axis = horizontal, so
                // `FlexEnd` on both axes parks the single child bottom-right.
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
            // Above every HUD widget (which use local `ZIndex` up to
            // `i32::MAX`); a `GlobalZIndex` opens a new top stacking context.
            GlobalZIndex(i32::MAX),
            // Never eat clicks meant for the world / HUD — during the
            // transition the HUD is hidden anyway, and at `Idle` the node
            // is `Display::None`, but belt-and-suspenders.
            Pickable::IGNORE,
        ))
        .with_children(|p| {
            p.spawn((
                ZoneOverlayLabel,
                // Start at the dot-less frame (trailing spaces hold width).
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

/// Drive the fade state machine off the session `Stage` + geometry-load
/// signals, and hide/restore the HUD on the fade-out/fade-in edges.
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

    // While the session reports `Zoning`, keep the screen covered: kick a
    // fresh fade-out from `Idle`, or interrupt an in-progress fade-IN (a
    // back-to-back zoneline arriving before the previous reveal finished).
    // An already-running FadingOut/Holding is left alone.
    if stage == Stage::Zoning
        && matches!(
            *fade,
            ZoneOverlayFade::Idle | ZoneOverlayFade::FadingIn { .. }
        )
    {
        *fade = ZoneOverlayFade::FadingOut { elapsed: 0.0 };
        // Stash + hide every root HUD node. No-op when already hidden
        // (stash non-empty) so a fade-out restarting mid-cycle can't
        // clobber the saved state with the now-`Hidden` values.
        if stash.0.is_empty() {
            for (e, mut vis) in hud_roots.iter_mut() {
                stash.0.insert(e, *vis);
                *vis = Visibility::Hidden;
            }
        }
    }

    // Destination zone ready = position-seeded (`InZone` is gated on the
    // server spawn seed in session.rs) AND the auto-loader has fired for the
    // *current* zone AND no MZB parse is still in flight. A no-DAT-mapping
    // zone still stamps `LastAutoLoadedZone` and never populates
    // `tasks`, so it reads ready after the minimum hold (nothing to wait
    // for) — the `MAX_HOLD_SECS` cap covers a genuinely stuck stage.
    let ready = stage == Stage::InZone
        && last_auto.zone_id.is_some()
        && last_auto.zone_id == scene.snapshot.zone_id
        && mzb_in_flight.tasks.is_empty();

    let prev = *fade;
    *fade = tick(*fade, time.delta_secs(), ready);

    // Reveal complete — restore each stashed HUD node to its pre-blackout
    // `Visibility`, handing control back to the HUD's own `Display` logic.
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

/// Push the current fade alpha onto the overlay backdrop + label, animate
/// the loading ellipsis, and park the node at `Display::None` when idle so
/// a transparent full-screen node can't intercept input.
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
    // Advance the ellipsis only while the overlay is up; reset at idle so
    // the next zone-in starts dot-less.
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
        // Halfway through each transition is ~0.5.
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
        // Past the minimum hold but not ready → keep holding.
        let s = tick(
            ZoneOverlayFade::Holding {
                elapsed: FADE_HOLD_MIN_SECS,
            },
            0.016,
            false,
        );
        assert!(matches!(s, ZoneOverlayFade::Holding { .. }));
        // Past the minimum hold AND ready → fade in.
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
        // Guards the same-frame race: ready can be true on the frame the
        // auto-loader fires (tasks not yet kicked) — the minimum hold must
        // still gate the reveal.
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
