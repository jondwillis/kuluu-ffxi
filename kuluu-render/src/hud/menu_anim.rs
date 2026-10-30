//! The menu open/close fx law (owner ruling 2026-10-22): a window collapses toward its own
//! center and grows back from it, one callable function for both directions. Per-axis speed is
//! that axis's size divided by the shared period, so both axes reach their target on exactly the
//! same frame ("100x200 .. then the 200 would move twice as fast", "the speed is the distance
//! required to be 0 in 5 frames"). Only the box and its background are rendered while it animates:
//! text comes off at the start of a close and is written again only when the final shape lands, so
//! nothing ever crawls across moving chrome. The top command bar fades instead of squeezing (same
//! period law on alpha; the owner chose fade there because the bar is so wide).

use bevy::prelude::*;

/// Frames a squeeze open or close takes end to end (owner: 5, "needs fine tunes" - tune here only).
pub const MENU_SQUEEZE_FRAMES: u8 = 5;

/// Frames the top command bar fades in over while its text stays off (see [`fade_share`]).
pub const MENU_FADE_FRAMES: u8 = 5;

/// Below this a surface has never stood on screen as a box, so there is no shape to animate.
const ANIM_MIN_SIZE_PX: f32 = 1.0;

/// One frame of the collapse along one axis: `full_px` scaled by the frames still remaining.
/// Exactly 0 once `elapsed_frames` reaches `period_frames`.
pub fn squeeze_size(full_px: f32, elapsed_frames: u8, period_frames: u8) -> f32 {
    let period = period_frames.max(1);
    let elapsed = elapsed_frames.min(period);
    full_px * (f32::from(period - elapsed)) / f32::from(period)
}

/// The open mirror of [`squeeze_size`]: exactly `full_px` once `elapsed_frames` reaches
/// `period_frames`.
pub fn grow_size(full_px: f32, elapsed_frames: u8, period_frames: u8) -> f32 {
    let period = period_frames.max(1);
    full_px * f32::from(elapsed_frames.min(period)) / f32::from(period)
}

/// One frame of the bar fade-in: 0 before the first tick, exactly 1.0 at and past `period_frames`.
pub fn fade_share(elapsed_frames: u8, period_frames: u8) -> f32 {
    let period = period_frames.max(1);
    f32::from(elapsed_frames.min(period)) / f32::from(period)
}

/// The full box an fx animates to or from. Its center stays fixed while both axes scale - the
/// close is "calculated to end in center" by holding this point, whatever the axis speeds differ by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FullRect {
    pub center_x: f32,
    pub center_y: f32,
    pub width: f32,
    pub height: f32,
}

impl FullRect {
    /// The box a window surface's current style stands at; `None` unless both anchors and both
    /// axes are explicit pixels - a percent/right-edge-anchored node has no design-space px seat
    /// to center-hold around, so such windows keep drawing plainly.
    pub fn from_style(node: &Node) -> Option<Self> {
        let (Val::Px(left), Val::Px(top), Val::Px(width), Val::Px(height)) =
            (node.left, node.top, node.width, node.height)
        else {
            return None;
        };
        if width < ANIM_MIN_SIZE_PX || height < ANIM_MIN_SIZE_PX {
            return None;
        }
        Some(Self {
            center_x: left + width / 2.0,
            center_y: top + height / 2.0,
            width,
            height,
        })
    }

    /// Like [`Self::from_style`], but a height still `Auto` (a surface that never pinned) is
    /// captured at its last measured px instead of losing the squeeze outright - without this an
    /// unmeasured window pops out instantly on takeover dismissal while its settled siblings animate.
    pub fn from_style_measuring(node: &Node, measured_height: f32) -> Option<Self> {
        let (Val::Px(left), Val::Px(top), Val::Px(width)) = (node.left, node.top, node.width)
        else {
            return None;
        };
        if width < ANIM_MIN_SIZE_PX || measured_height < ANIM_MIN_SIZE_PX {
            return None;
        }
        Some(Self {
            center_x: left + width / 2.0,
            center_y: top + measured_height / 2.0,
            width,
            height: measured_height,
        })
    }

    /// The box a node last laid out - the selector column's law is right-anchored, so its style
    /// has no `Px` left to read and only the laid-out frame carries its full seat. Same conversion
    /// as [`crate::hud::dock::node_anchor`] (`drawn_height`'s both axes).
    pub fn from_laid_out_box(
        computed: &bevy::ui::ComputedNode,
        transform: &bevy::ui::UiGlobalTransform,
    ) -> Option<Self> {
        let scale = computed.inverse_scale_factor();
        let size = computed.size() * scale;
        if size.x < ANIM_MIN_SIZE_PX || size.y < ANIM_MIN_SIZE_PX {
            return None;
        }
        let center = transform.translation * scale;
        Some(Self {
            center_x: center.x,
            center_y: center.y,
            width: size.x,
            height: size.y,
        })
    }

    /// The four style fields of a `width`-by-`height` box seated on this rect's center - write all
    /// four together and the box holds its center while the axes scale.
    pub fn centered(&self, width: f32, height: f32) -> (Val, Val, Val, Val) {
        (
            Val::Px(self.center_x - width / 2.0),
            Val::Px(self.center_y - height / 2.0),
            Val::Px(width),
            Val::Px(height),
        )
    }
}

/// One pooled window surface's fx state. Idle is the resting case: whatever the draw pass decided
/// (full shape or hidden) stands with no override.
#[derive(Clone, Copy, Debug, Default)]
pub enum WindowFx {
    #[default]
    Idle,
    Opening {
        full: FullRect,
        elapsed_frames: u8,
    },
    Closing {
        full: FullRect,
        elapsed_frames: u8,
    },
}

impl WindowFx {
    /// Arm a grow-in; the first [`Self::tick`] draws frame one.
    pub fn opening(full: FullRect) -> Self {
        Self::Opening {
            full,
            elapsed_frames: 0,
        }
    }

    /// Arm a collapse; the first [`Self::tick`] draws frame one.
    pub fn closing(full: FullRect) -> Self {
        Self::Closing {
            full,
            elapsed_frames: 0,
        }
    }

    /// Advance one fx frame. `Some((width, height))` is this frame's center-held box size (the fx
    /// keeps running); `None` means the fx reached its target on this tick - from here the natural
    /// draw applies again (full shape for an open, hidden for a close).
    pub fn tick(&mut self) -> Option<(f32, f32)> {
        let period = MENU_SQUEEZE_FRAMES;
        match self {
            Self::Idle => None,
            Self::Opening {
                full,
                elapsed_frames,
            } => {
                *elapsed_frames += 1;
                if *elapsed_frames >= period {
                    *self = Self::Idle;
                    return None;
                }
                Some((
                    grow_size(full.width, *elapsed_frames, period),
                    grow_size(full.height, *elapsed_frames, period),
                ))
            }
            Self::Closing {
                full,
                elapsed_frames,
            } => {
                *elapsed_frames += 1;
                if *elapsed_frames >= period {
                    *self = Self::Idle;
                    return None;
                }
                Some((
                    squeeze_size(full.width, *elapsed_frames, period),
                    squeeze_size(full.height, *elapsed_frames, period),
                ))
            }
        }
    }

    /// The full box of a running (or just armed) fx.
    pub fn full(&self) -> Option<FullRect> {
        match self {
            Self::Idle => None,
            Self::Opening { full, .. } | Self::Closing { full, .. } => Some(*full),
        }
    }

    /// True while a grow-in still owns this surface - the draw pass reads it to keep every word
    /// off until the box reaches its final shape.
    pub fn is_opening(&self) -> bool {
        matches!(self, Self::Opening { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squeeze_reaches_zero_on_the_period_frame() {
        // The owner's 100x200 example: each axis covers its own distance in the same frames.
        let width_steps: Vec<f32> = (1..MENU_SQUEEZE_FRAMES)
            .map(|e| squeeze_size(100.0, e, MENU_SQUEEZE_FRAMES))
            .collect();
        assert_eq!(width_steps, vec![80.0, 60.0, 40.0, 20.0]);
        let height_steps: Vec<f32> = (1..MENU_SQUEEZE_FRAMES)
            .map(|e| squeeze_size(200.0, e, MENU_SQUEEZE_FRAMES))
            .collect();
        assert_eq!(height_steps, vec![160.0, 120.0, 80.0, 40.0]);
        for (w_step, h_step) in width_steps.iter().zip(&height_steps) {
            // "the 200 would move twice as fast": per-frame travel scales with the axis size.
            assert!((h_step * 100.0 - w_step * 200.0).abs() < 0.001);
        }
        assert_eq!(
            squeeze_size(200.0, MENU_SQUEEZE_FRAMES, MENU_SQUEEZE_FRAMES),
            0.0
        );
    }

    #[test]
    fn grow_is_the_exact_mirror_of_squeeze() {
        // At any frame of a period the grown share and the collapsed remainder are complementary.
        for elapsed in 0..=MENU_SQUEEZE_FRAMES {
            let sum = grow_size(57.0, elapsed, MENU_SQUEEZE_FRAMES)
                + squeeze_size(57.0, elapsed, MENU_SQUEEZE_FRAMES);
            assert!((sum - 57.0).abs() < 0.001, "frame {elapsed}");
        }
        assert_eq!(
            grow_size(57.0, MENU_SQUEEZE_FRAMES, MENU_SQUEEZE_FRAMES),
            57.0
        );
    }

    #[test]
    fn a_zero_period_clamps_to_one_frame() {
        assert_eq!(squeeze_size(90.0, 1, 0), 0.0);
        assert_eq!(grow_size(90.0, 1, 0), 90.0);
        assert_eq!(fade_share(1, 0), 1.0);
    }

    #[test]
    fn fade_share_runs_0_to_1_over_the_period() {
        assert_eq!(fade_share(0, MENU_FADE_FRAMES), 0.0);
        let shares: Vec<f32> = (1..=MENU_FADE_FRAMES)
            .map(|e| fade_share(e, MENU_FADE_FRAMES))
            .collect();
        for pair in shares.windows(2) {
            assert!(pair[1] > pair[0]);
        }
        assert_eq!(*shares.last().unwrap(), 1.0);
    }

    #[test]
    fn full_rect_needs_explicit_pixels_on_both_axes() {
        let stood = Node {
            left: Val::Px(16.0),
            top: Val::Px(48.0),
            width: Val::Px(366.0),
            height: Val::Px(57.0),
            ..default()
        };
        let full = FullRect::from_style(&stood).expect("a fully-pixelled box");
        assert_eq!(full.center_x, 16.0 + 366.0 / 2.0);
        assert_eq!(full.center_y, 48.0 + 57.0 / 2.0);

        let mut unmeasured = stood.clone();
        unmeasured.height = Val::Auto;
        assert!(
            FullRect::from_style(&unmeasured).is_none(),
            "a surface that never measured its height has no box"
        );
        let mut anchored = stood.clone();
        anchored.left = Val::Percent(100.0);
        assert!(
            FullRect::from_style(&anchored).is_none(),
            "percent-anchored styles have no px seat to center-hold around"
        );
        let mut sliver = stood.clone();
        sliver.width = Val::Px(0.5);
        assert!(
            FullRect::from_style(&sliver).is_none(),
            "sub-box sizes were never seen on screen"
        );
    }

    #[test]
    fn centered_seats_hold_the_center_at_any_size() {
        let full = FullRect {
            center_x: 199.0,
            center_y: 76.5,
            width: 366.0,
            height: 248.0,
        };
        for size in [(366.0f32, 248.0f32), (146.4, 99.2), (73.2, 49.6)] {
            let (left, top, w, h) = full.centered(size.0, size.1);
            let (Val::Px(l), Val::Px(t), Val::Px(wv), Val::Px(hv)) = (left, top, w, h) else {
                panic!("centered seats are px")
            };
            assert!((l + wv / 2.0 - full.center_x).abs() < 0.01);
            assert!((t + hv / 2.0 - full.center_y).abs() < 0.01);
        }
    }

    #[test]
    fn a_surface_ticked_through_its_period_returns_to_idle() {
        let full = FullRect {
            center_x: 0.0,
            center_y: 0.0,
            width: 50.0,
            height: 100.0,
        };
        let mut fx = WindowFx::closing(full);
        let mut frames = Vec::new();
        for _ in 0..MENU_SQUEEZE_FRAMES {
            frames.push(fx.tick());
        }
        assert!(matches!(fx, WindowFx::Idle), "closed out within the period");
        assert_eq!(
            frames.last().unwrap(),
            &None,
            "the target frame hands drawing back"
        );
        let sizes: Vec<(f32, f32)> = frames.iter().flatten().copied().collect();
        assert_eq!(sizes.len(), usize::from(MENU_SQUEEZE_FRAMES - 1));

        let mut fx = WindowFx::opening(full);
        assert!(fx.tick().is_some() && fx.is_opening());
        for _ in 1..MENU_SQUEEZE_FRAMES {
            fx.tick();
        }
        assert!(
            matches!(fx, WindowFx::Idle),
            "grown to full within the period"
        );
    }
}
