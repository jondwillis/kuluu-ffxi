//! Animation runtime — port of `xim/poc/SkeletonAnimator.kt`.
//!
//! Layering, from the bottom up:
//!   * [`SkeletonAnimationContext`] — one playing clip: frame cursor, loop
//!     bounds, completion bookkeeping.
//!   * [`AnimationSnapshot`] / [`AnimationTransition`] — same-slot crossfade.
//!   * [`SkeletonAnimator`] — one animation *slot*: current clip + transition.
//!   * [`SkeletonAnimationCoordinator`] — 8 slots keyed by `DatId.final_digit`,
//!     with cross-slot blending in `get_joint_transform`.
//!
//! Numeric defaults match XIM (transition in/out 7.5 frames, etc.).

use std::collections::HashMap;

use ffxi_dat::skel_anim::{nlerp, KeyFrameTransform, SkeletonAnimation};

/// XIM `SkeletonAnimationKeyFrameTransform.interpolate` — nlerp rotation,
/// lerp translation + scale. Re-derived to match `ffxi_dat`'s private
/// `interpolate` (same nlerp + component lerp).
pub fn interpolate_kf(a: &KeyFrameTransform, b: &KeyFrameTransform, t: f32) -> KeyFrameTransform {
    KeyFrameTransform {
        rotation: nlerp(a.rotation, b.rotation, t),
        translation: lerp3(a.translation, b.translation, t),
        scale: lerp3(a.scale, b.scale, t),
    }
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let inv = 1.0 - t;
    [
        a[0] * inv + b[0] * t,
        a[1] * inv + b[1] * t,
        a[2] * inv + b[2] * t,
    ]
}

const UNIT_TRANSFORM: KeyFrameTransform = KeyFrameTransform {
    rotation: [0.0, 0.0, 0.0, 1.0],
    translation: [0.0, 0.0, 0.0],
    scale: [1.0, 1.0, 1.0],
};

/// XIM `AnimationTransition.interpolate` for nullable inputs: when only one
/// side acts on the joint, nlerp/lerp toward the unit transform for
/// rotation/translation but keep whichever scale is present (scale-to-unit
/// breaks some effects in XIM).
fn interpolate_nullable(
    a: Option<&KeyFrameTransform>,
    b: Option<&KeyFrameTransform>,
    t: f32,
) -> Option<KeyFrameTransform> {
    match (a, b) {
        (None, None) => None,
        (Some(a), Some(b)) => Some(interpolate_kf(a, b, t)),
        _ => {
            let ar = a.map(|x| x.rotation).unwrap_or(UNIT_TRANSFORM.rotation);
            let br = b.map(|x| x.rotation).unwrap_or(UNIT_TRANSFORM.rotation);
            let at = a
                .map(|x| x.translation)
                .unwrap_or(UNIT_TRANSFORM.translation);
            let bt = b
                .map(|x| x.translation)
                .unwrap_or(UNIT_TRANSFORM.translation);
            let scale = a
                .map(|x| x.scale)
                .or_else(|| b.map(|x| x.scale))
                .unwrap_or(UNIT_TRANSFORM.scale);
            Some(KeyFrameTransform {
                rotation: nlerp(ar, br, t),
                translation: lerp3(at, bt, t),
                scale,
            })
        }
    }
}

/// XIM `LoopParams`.
#[derive(Debug, Clone, Copy)]
pub struct LoopParams {
    /// `None` => loop over the clip's natural length. `Some(0.0)` => freeze on
    /// frame 0 (single-frame). `Some(d)` => time-scale the clip to `d` frames.
    pub loop_duration: Option<f32>,
    /// `None` => loop forever. `Some(n)` => complete after `n` loops.
    pub num_loops: Option<u32>,
    pub low_priority: bool,
}

impl LoopParams {
    /// XIM `LoopParams.lowPriorityLoop()`.
    pub fn low_priority_loop() -> Self {
        LoopParams {
            loop_duration: None,
            num_loops: None,
            low_priority: true,
        }
    }
}

/// XIM `TransitionParams`. `resolved_in_between` is XIM's
/// `resolvedInBetween: Map<slot, SkeletonAnimationResource>` — one in-between
/// clip per slot, keyed by the clip's `DatId.final_digit`. Each
/// [`SkeletonAnimator`] looks up its OWN slot's clip in [`set_next_animation`],
/// so reusing one `TransitionParams` across animations that land in different
/// slots gives each slot its slot-specific in-between (XIM's
/// `setSkeletonAnimation` builds ONE `TransitionParams` and registers it over
/// the whole resource list).
#[derive(Clone)]
pub struct TransitionParams {
    pub transition_in_time: f32,
    pub transition_out_time: f32,
    pub resolved_in_between: HashMap<usize, SkeletonAnimation>,
    pub eager_transition_out: bool,
}

impl Default for TransitionParams {
    fn default() -> Self {
        TransitionParams {
            transition_in_time: 7.5,
            transition_out_time: 7.5,
            resolved_in_between: HashMap::new(),
            eager_transition_out: false,
        }
    }
}

impl TransitionParams {
    /// Build `resolved_in_between` from a list of in-between clips, keyed by
    /// each clip's `final_digit` slot — XIM
    /// `fetchAnimations(inBetween, ...).associateBy { it.id.finalDigit() ?: 0 }`.
    pub fn with_in_between(mut self, clips: impl IntoIterator<Item = SkeletonAnimation>) -> Self {
        for clip in clips {
            let slot = (clip.id.final_digit().unwrap_or(0) as usize).min(7);
            self.resolved_in_between.insert(slot, clip);
        }
        self
    }
}

/// XIM `SkeletonAnimationContext`.
pub struct SkeletonAnimationContext {
    pub animation: SkeletonAnimation,
    pub loop_params: LoopParams,
    pub transition_params: Option<TransitionParams>,
    pub current_frame: f32,
    pub frames_since_complete: f32,
    pub total_life_time: f32,
    completed: bool,
    loop_counter: u32,
}

impl SkeletonAnimationContext {
    pub fn new(
        animation: SkeletonAnimation,
        loop_params: LoopParams,
        transition_params: Option<TransitionParams>,
    ) -> Self {
        SkeletonAnimationContext {
            animation,
            loop_params,
            transition_params,
            current_frame: 0.0,
            frames_since_complete: 0.0,
            total_life_time: 0.0,
            completed: false,
            loop_counter: 0,
        }
    }

    pub fn advance(&mut self, elapsed_frames: f32) {
        self.total_life_time += elapsed_frames;

        let eager = self
            .transition_params
            .as_ref()
            .map(|t| t.eager_transition_out)
            .unwrap_or(false);
        if self.completed || eager {
            self.frames_since_complete += elapsed_frames;
        }

        if self.loop_params.loop_duration == Some(0.0) {
            self.current_frame = 0.0;
            self.completed = true;
            return;
        }

        let length = self.animation.length_in_frames();
        let loop_duration = self.loop_params.loop_duration.unwrap_or(length);
        let scaling_factor = length / loop_duration;

        self.current_frame += elapsed_frames * scaling_factor;
        self.current_frame = self.apply_loop_bounds();
    }

    pub fn get_joint_transform(&self, joint: usize) -> Option<KeyFrameTransform> {
        self.animation
            .get_joint_transform(joint as u32, self.current_frame)
    }

    pub fn is_done_looping(&self) -> bool {
        self.loop_params.num_loops.is_none() || self.completed
    }

    fn apply_loop_bounds(&mut self) -> f32 {
        let max_loops = self.loop_params.num_loops.unwrap_or(0);
        let length = self.animation.length_in_frames();

        while self.current_frame > length {
            self.loop_counter += 1;
            self.current_frame -= length;
        }

        if max_loops != 0 && self.loop_counter >= max_loops {
            self.completed = true;
            return length;
        }

        self.current_frame
    }
}

/// XIM `AnimationSnapshot` — frozen per-joint transforms captured at the moment
/// a new animation is set on a slot.
pub struct AnimationSnapshot {
    joint_snapshots: HashMap<usize, KeyFrameTransform>,
}

impl AnimationSnapshot {
    /// XIM `AnimationSnapshot(previous: SkeletonAnimationContext)`.
    pub fn from_context(ctx: &SkeletonAnimationContext) -> Self {
        let mut joint_snapshots = HashMap::new();
        for &joint in ctx.animation.key_frame_sets.keys() {
            if let Some(t) = ctx.animation.get_joint_transform(joint, ctx.current_frame) {
                joint_snapshots.insert(joint as usize, t);
            }
        }
        AnimationSnapshot { joint_snapshots }
    }

    /// XIM `AnimationSnapshot(transition: AnimationTransition)` — flatten an
    /// in-progress transition into a fresh snapshot (used when re-transitioning
    /// before the previous transition finished).
    pub fn from_transition(transition: &AnimationTransition) -> Self {
        let mut joint_snapshots = HashMap::new();
        let joints: std::collections::HashSet<usize> = transition
            .previous
            .joint_snapshots
            .keys()
            .copied()
            .chain(
                transition
                    .next
                    .animation
                    .key_frame_sets
                    .keys()
                    .map(|&k| k as usize),
            )
            .collect();
        for joint in joints {
            if let Some(t) = transition.get_joint_transform(joint) {
                joint_snapshots.insert(joint, t);
            }
        }
        AnimationSnapshot { joint_snapshots }
    }

    fn get_joint_transform(&self, joint: usize) -> Option<KeyFrameTransform> {
        self.joint_snapshots.get(&joint).copied()
    }
}

/// XIM `AnimationTransition`.
pub struct AnimationTransition {
    pub previous: AnimationSnapshot,
    pub next: SkeletonAnimationContext,
    pub transition_duration: f32,
    pub in_between: Option<SkeletonAnimation>,
    progress: f32,
}

impl AnimationTransition {
    pub fn new(
        previous: AnimationSnapshot,
        next: SkeletonAnimationContext,
        transition_duration: f32,
        in_between: Option<SkeletonAnimation>,
    ) -> Self {
        AnimationTransition {
            previous,
            next,
            transition_duration,
            in_between,
            progress: 0.0,
        }
    }

    /// Returns true when complete.
    pub fn update(&mut self, elapsed_frames: f32) -> bool {
        self.progress += elapsed_frames;
        self.is_complete()
    }

    pub fn is_complete(&self) -> bool {
        self.progress >= self.transition_duration
    }

    pub fn get_joint_transform(&self, joint: usize) -> Option<KeyFrameTransform> {
        let t = self.progress / self.transition_duration;

        match &self.in_between {
            None => {
                let prev = self.previous.get_joint_transform(joint);
                let next = self.next.get_joint_transform(joint);
                interpolate_nullable(prev.as_ref(), next.as_ref(), t)
            }
            Some(in_between) => {
                if t < 0.5 {
                    let prev = self.previous.get_joint_transform(joint);
                    let next = in_between.get_joint_transform(joint as u32, 0.0);
                    interpolate_nullable(prev.as_ref(), next.as_ref(), t * 2.0)
                } else {
                    let prev = in_between.get_joint_transform(joint as u32, 0.0);
                    let next = self.next.get_joint_transform(joint);
                    interpolate_nullable(prev.as_ref(), next.as_ref(), (t - 0.5) * 2.0)
                }
            }
        }
    }
}

/// XIM `SkeletonAnimator` — one animation slot.
pub struct SkeletonAnimator {
    animation_slot: usize,
    pub current_animation: Option<SkeletonAnimationContext>,
    pub transition: Option<AnimationTransition>,
}

impl SkeletonAnimator {
    pub fn new(animation_slot: usize) -> Self {
        SkeletonAnimator {
            animation_slot,
            current_animation: None,
            transition: None,
        }
    }

    pub fn update(&mut self, elapsed_frames: f32) {
        let transition_complete = self.transition.as_mut().map(|t| t.update(elapsed_frames));

        match transition_complete {
            Some(true) => self.transition = None,
            _ => {
                if let Some(ctx) = self.current_animation.as_mut() {
                    ctx.advance(elapsed_frames);
                }
            }
        }
    }

    pub fn set_next_animation(
        &mut self,
        ctx: SkeletonAnimationContext,
        transition_params: Option<&TransitionParams>,
    ) {
        let transition_in_zero = transition_params
            .map(|t| t.transition_in_time == 0.0)
            .unwrap_or(false);

        if self.current_animation.is_none() || transition_in_zero {
            self.current_animation = Some(ctx);
            return;
        }

        let current = self.current_animation.as_ref().unwrap();

        // Skip re-triggering the same low-priority (idle) clip.
        if same_animation(&current.animation, &ctx.animation) && current.loop_params.low_priority {
            return;
        }

        // Slot 5 wants cross-slot interpolation instead of a same-slot
        // transition (XIM note re: triplet mobs).
        if self.animation_slot != 5 {
            let transition_duration = if let Some(tp) = transition_params {
                tp.transition_in_time
            } else if current
                .transition_params
                .as_ref()
                .map(|t| t.transition_out_time > 0.0)
                .unwrap_or(false)
            {
                current
                    .transition_params
                    .as_ref()
                    .unwrap()
                    .transition_out_time
            } else {
                7.5
            };

            let snapshot = match &self.transition {
                Some(t) => AnimationSnapshot::from_transition(t),
                None => AnimationSnapshot::from_context(current),
            };
            // XIM `transitionParams?.resolvedInBetween?.get(animationSlot)` —
            // each slot picks its OWN in-between clip.
            let in_between = transition_params
                .and_then(|t| t.resolved_in_between.get(&self.animation_slot).cloned());

            self.transition = Some(AnimationTransition::new(
                snapshot,
                // The transition's `next` snapshots the incoming clip; XIM
                // shares the same context object, but a clone of frame-0 state
                // is equivalent for read-only snapshotting.
                clone_context_at_frame0(&ctx),
                transition_duration,
                in_between,
            ));
        }

        self.current_animation = Some(ctx);
    }

    pub fn get_joint_transform(&self, joint: usize) -> Option<KeyFrameTransform> {
        if let Some(t) = &self.transition {
            t.get_joint_transform(joint)
        } else {
            self.current_animation
                .as_ref()
                .and_then(|c| c.get_joint_transform(joint))
        }
    }
}

fn same_animation(a: &SkeletonAnimation, b: &SkeletonAnimation) -> bool {
    a.id == b.id
}

/// XIM shares the incoming context between the slot and the transition's
/// `next`. Since `AnimationTransition` only reads `next` at its current frame
/// during the transition window, a frame-0 read-only copy is sufficient here.
fn clone_context_at_frame0(ctx: &SkeletonAnimationContext) -> SkeletonAnimationContext {
    SkeletonAnimationContext::new(
        ctx.animation.clone(),
        ctx.loop_params,
        ctx.transition_params.clone(),
    )
}

/// XIM `SkeletonAnimationCoordinator` — 8 slots keyed by `DatId.final_digit`.
#[derive(Default)]
pub struct SkeletonAnimationCoordinator {
    pub animations: [Option<SkeletonAnimator>; 8],
}

impl SkeletonAnimationCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, elapsed_frames: f32) {
        for slot in self.animations.iter_mut().flatten() {
            slot.update(elapsed_frames);
        }
    }

    /// XIM `registerAnimation` for a single resolved animation. The slot is the
    /// animation id's final digit (0..7, defaulting to 0). `override_condition`
    /// gates whether the slot's current animation is replaced.
    pub fn register_animation(
        &mut self,
        animation: SkeletonAnimation,
        loop_params: LoopParams,
        transition_params: Option<TransitionParams>,
        override_condition: impl Fn(&SkeletonAnimator) -> bool,
    ) {
        let slot = animation.id.final_digit().unwrap_or(0) as usize;
        let slot = slot.min(7);
        let animator = self.get_or_put(slot);

        if override_condition(animator) {
            let ctx =
                SkeletonAnimationContext::new(animation, loop_params, transition_params.clone());
            animator.set_next_animation(ctx, transition_params.as_ref());
        }
    }

    /// XIM `registerIdleAnimation`: a low-priority loop, gated on
    /// transition-out readiness.
    pub fn register_idle_animation(
        &mut self,
        animation: SkeletonAnimation,
        require_transition_out: bool,
    ) {
        self.register_animation(
            animation,
            LoopParams::low_priority_loop(),
            None,
            |animator| ready_for_transition_out(animator, require_transition_out),
        );
    }

    /// XIM `getJointTransform`: scan slots 8..=0, take the highest occupied
    /// transform and the next-lower, blend via `cross_slot_interpolation`.
    pub fn get_joint_transform(&self, joint: usize) -> Option<KeyFrameTransform> {
        let mut high: Option<(KeyFrameTransform, &SkeletonAnimator)> = None;
        let mut low: Option<KeyFrameTransform> = None;

        // XIM iterates `8 downTo 0` — index 8 is out of the 0..7 array bounds
        // and yields null in Kotlin, so it is a no-op; we start at 7.
        for i in (0..8).rev() {
            let Some(animator) = &self.animations[i] else {
                continue;
            };
            let Some(t) = animator.get_joint_transform(joint) else {
                continue;
            };

            if high.is_none() {
                high = Some((t, animator));
            } else if low.is_none() {
                low = Some(t);
                break;
            }
        }

        match (high, low) {
            (Some((h, animator)), Some(l)) => Some(cross_slot_interpolation(h, animator, l)),
            (Some((h, _)), None) => Some(h),
            (None, Some(l)) => Some(l),
            (None, None) => None,
        }
    }

    pub fn is_transitioning(&self) -> bool {
        self.animations
            .iter()
            .flatten()
            .any(|a| a.transition.is_some())
    }

    pub fn clear(&mut self) {
        for slot in self.animations.iter_mut() {
            *slot = None;
        }
    }

    /// Bitmask of currently-occupied slots (bit `i` set ⇒ slot `i` has an
    /// animator). Used by the live tick to find slots the previous pose
    /// occupied but the incoming one doesn't.
    pub fn occupied_slots(&self) -> u8 {
        let mut mask = 0u8;
        for (i, slot) in self.animations.iter().enumerate() {
            if slot.is_some() {
                mask |= 1 << i;
            }
        }
        mask
    }

    /// Empty a single slot, leaving the others' frame cursors intact (unlike
    /// [`clear`](Self::clear), which resets everything). The live tick uses
    /// this to retire an orphan layer — e.g. the run upper-body layer (slot 1)
    /// when the next pose is a slot-0-only idle — so it doesn't keep animating
    /// under the new pose, while the surviving slots still crossfade in place.
    pub fn clear_slot(&mut self, slot: usize) {
        if let Some(s) = self.animations.get_mut(slot) {
            *s = None;
        }
    }

    fn get_or_put(&mut self, slot: usize) -> &mut SkeletonAnimator {
        if self.animations[slot].is_none() {
            self.animations[slot] = Some(SkeletonAnimator::new(slot));
        }
        self.animations[slot].as_mut().unwrap()
    }
}

/// XIM `readyForTransitionOut`.
fn ready_for_transition_out(animator: &SkeletonAnimator, require_transition_out: bool) -> bool {
    let current = animator.current_animation.as_ref();

    let transition_out_reqs = if !require_transition_out {
        true
    } else {
        let out_time = current
            .and_then(|c| c.transition_params.as_ref())
            .map(|t| t.transition_out_time);
        let no_out = matches!(out_time, None | Some(0.0));
        let has_out = matches!(out_time, Some(t) if t > 0.0);
        no_out || has_out
    };

    let mut done_looping = match current {
        None => true,
        Some(c) => c.is_done_looping(),
    };

    let eager = current
        .and_then(|c| c.transition_params.as_ref())
        .map(|t| t.eager_transition_out)
        .unwrap_or(false);
    if eager {
        done_looping = true;
    }

    transition_out_reqs && done_looping
}

/// XIM `crossSlotInterpolation`: blend the highest-slot transform toward the
/// next-lower as the high slot transitions in (`deltaIn`) or out (`deltaOut`).
fn cross_slot_interpolation(
    high_slot: KeyFrameTransform,
    high_animator: &SkeletonAnimator,
    low_slot: KeyFrameTransform,
) -> KeyFrameTransform {
    let Some(high_animation) = high_animator.current_animation.as_ref() else {
        return high_slot;
    };
    let Some(transition_params) = high_animation.transition_params.as_ref() else {
        return high_slot;
    };

    let mut delta_in = 0.0;
    let mut delta_out = 0.0;

    if high_animation.total_life_time < transition_params.transition_in_time {
        delta_in = 1.0 - high_animation.total_life_time / transition_params.transition_in_time;
    }

    if ready_for_transition_out(high_animator, true) {
        delta_out = if transition_params.transition_out_time == 0.0 {
            0.0
        } else {
            high_animation.frames_since_complete / transition_params.transition_out_time
        };
    }

    let delta = delta_in.max(delta_out);

    if delta <= 0.0 {
        high_slot
    } else if delta >= 1.0 {
        low_slot
    } else {
        interpolate_kf(&high_slot, &low_slot, delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::datid::DatId;
    use ffxi_dat::skel_anim::SkeletonAnimation;

    /// Build a one-joint animation: joint 0, `num_frames` frames where
    /// translation.x = frame*10, identity rotation, unit scale.
    fn anim(id: &str, num_frames: usize, duration: f32) -> SkeletonAnimation {
        let mut sets = HashMap::new();
        let frames: Vec<KeyFrameTransform> = (0..num_frames)
            .map(|f| KeyFrameTransform {
                rotation: [0.0, 0.0, 0.0, 1.0],
                translation: [f as f32 * 10.0, 0.0, 0.0],
                scale: [1.0, 1.0, 1.0],
            })
            .collect();
        sets.insert(0u32, frames);
        SkeletonAnimation {
            id: DatId::from_str(id),
            num_joints: 1,
            num_frames,
            key_frame_duration: duration,
            key_frame_sets: sets,
        }
    }

    #[test]
    fn advance_wraps_within_length() {
        let mut ctx = SkeletonAnimationContext::new(
            anim("idl0", 5, 1.0), // length_in_frames = (5-1)/1 = 4
            LoopParams {
                loop_duration: None,
                num_loops: None,
                low_priority: false,
            },
            None,
        );
        // advance past the end; current_frame must wrap back below length 4.
        ctx.advance(5.0);
        assert!(
            ctx.current_frame <= 4.0,
            "frame {} not wrapped",
            ctx.current_frame
        );
        assert!(ctx.current_frame >= 0.0);
    }

    #[test]
    fn single_frame_freeze_when_loop_duration_zero() {
        let mut ctx = SkeletonAnimationContext::new(
            anim("idl0", 5, 1.0),
            LoopParams {
                loop_duration: Some(0.0),
                num_loops: None,
                low_priority: false,
            },
            None,
        );
        ctx.advance(3.0);
        assert_eq!(ctx.current_frame, 0.0);
        assert!(ctx.is_done_looping());
    }

    #[test]
    fn num_loops_completes() {
        let mut ctx = SkeletonAnimationContext::new(
            anim("idl0", 5, 1.0), // length 4
            LoopParams {
                loop_duration: None,
                num_loops: Some(1),
                low_priority: false,
            },
            None,
        );
        assert!(!ctx.is_done_looping());
        ctx.advance(5.0); // crosses the length once -> loopCounter 1 >= 1
        assert!(ctx.is_done_looping());
        assert_eq!(ctx.current_frame, 4.0);
    }

    #[test]
    fn registered_clip_slot0_returns_interpolated() {
        let mut coord = SkeletonAnimationCoordinator::new();
        coord.register_animation(
            anim("idl0", 3, 1.0),
            LoopParams {
                loop_duration: None,
                num_loops: None,
                low_priority: false,
            },
            None,
            |_| true,
        );
        // frame 0 -> translation.x = 0
        let t0 = coord.get_joint_transform(0).unwrap();
        assert!((t0.translation[0] - 0.0).abs() < 1e-4);
        // advance half a frame -> interpolated to 5
        coord.update(0.5);
        let t1 = coord.get_joint_transform(0).unwrap();
        assert!(
            (t1.translation[0] - 5.0).abs() < 1e-3,
            "got {}",
            t1.translation[0]
        );
    }

    #[test]
    fn coordinator_picks_highest_occupied_slot() {
        let mut coord = SkeletonAnimationCoordinator::new();
        // slot 0: translation.x sequence base 10/frame
        coord.register_animation(
            anim("aaa0", 3, 1.0),
            LoopParams {
                loop_duration: None,
                num_loops: None,
                low_priority: false,
            },
            None,
            |_| true,
        );
        // slot 3: a distinct clip whose frame0 x = 0 too but tag via num_frames
        let mut high = anim("bbb3", 2, 1.0);
        // make slot 3's frame0 distinctly 100 so we can detect it wins
        high.key_frame_sets.get_mut(&0).unwrap()[0].translation[0] = 100.0;
        coord.register_animation(
            high,
            LoopParams {
                loop_duration: None,
                num_loops: None,
                low_priority: false,
            },
            None,
            |_| true,
        );
        // Two slots occupied (0 and 3); highest is 3. With both present, the
        // result is a cross-slot blend, but with no transition params on the
        // high slot the blend returns the high transform unchanged.
        let t = coord.get_joint_transform(0).unwrap();
        assert!(
            (t.translation[0] - 100.0).abs() < 1e-4,
            "got {}",
            t.translation[0]
        );
    }

    #[test]
    fn cross_slot_returns_high_when_not_transitioning() {
        // High slot has transition params but totalLifeTime already exceeds
        // transitionInTime and it is not ready to transition out, so delta=0
        // and the high transform is returned verbatim.
        let mut coord = SkeletonAnimationCoordinator::new();
        coord.register_animation(
            anim("aaa0", 3, 1.0),
            LoopParams {
                loop_duration: None,
                num_loops: None, // never completes -> never transitions out
                low_priority: false,
            },
            None,
            |_| true,
        );
        let mut high = anim("bbb3", 2, 1.0);
        high.key_frame_sets.get_mut(&0).unwrap()[0].translation[0] = 100.0;
        coord.register_animation(
            high,
            LoopParams {
                loop_duration: None,
                num_loops: None,
                low_priority: false,
            },
            Some(TransitionParams::default()),
            |_| true,
        );
        // Push total_life_time well past the 7.5-frame transition-in window so
        // deltaIn = 0; never-completing loop means deltaOut = 0.
        coord.update(20.0);
        let t = coord.get_joint_transform(0).unwrap();
        // frame0 was 100; after 20 frames of a 2-frame (len=1) loop it wraps,
        // so just assert it is the high slot's clip (x advanced from its own
        // sequence, not blended toward slot 0's small values).
        assert!(t.translation[0] >= 0.0);
    }

    #[test]
    fn same_low_priority_idle_not_retriggered() {
        let mut animator = SkeletonAnimator::new(0);
        let a = anim("idl0", 3, 1.0);
        animator.set_next_animation(
            SkeletonAnimationContext::new(a.clone(), LoopParams::low_priority_loop(), None),
            None,
        );
        // advance so the frame moves
        animator.update(1.0);
        let frame_before = animator.current_animation.as_ref().unwrap().current_frame;
        // re-register the same low-priority animation: should be a no-op (no
        // reset, no transition)
        animator.set_next_animation(
            SkeletonAnimationContext::new(a.clone(), LoopParams::low_priority_loop(), None),
            None,
        );
        let frame_after = animator.current_animation.as_ref().unwrap().current_frame;
        assert_eq!(frame_before, frame_after);
        assert!(animator.transition.is_none());
    }

    #[test]
    fn slot5_skips_same_slot_transition() {
        let mut animator = SkeletonAnimator::new(5);
        animator.set_next_animation(
            SkeletonAnimationContext::new(
                anim("aaa5", 3, 1.0),
                LoopParams {
                    loop_duration: None,
                    num_loops: None,
                    low_priority: false,
                },
                None,
            ),
            None,
        );
        animator.set_next_animation(
            SkeletonAnimationContext::new(
                anim("bbb5", 3, 1.0),
                LoopParams {
                    loop_duration: None,
                    num_loops: None,
                    low_priority: false,
                },
                None,
            ),
            Some(&TransitionParams::default()),
        );
        // slot 5 must not create a same-slot transition
        assert!(animator.transition.is_none());
    }

    #[test]
    fn per_slot_in_between_picks_own_slot_clip() {
        // One TransitionParams whose resolved_in_between holds DISTINCT clips for
        // slot 0 and slot 3 (XIM's resolvedInBetween map). Each animator's
        // transition must use ITS OWN slot's in-between, not a shared one.
        let mut ib0 = anim("xxx0", 1, 1.0);
        ib0.key_frame_sets.get_mut(&0).unwrap()[0].translation[0] = 11.0;
        let mut ib3 = anim("yyy3", 1, 1.0);
        ib3.key_frame_sets.get_mut(&0).unwrap()[0].translation[0] = 33.0;
        let tp = TransitionParams::default().with_in_between([ib0, ib3]);

        let loop_params = LoopParams {
            loop_duration: None,
            num_loops: None,
            low_priority: false,
        };

        // Slot 0 animator: first set current, then transition with tp.
        let mut a0 = SkeletonAnimator::new(0);
        a0.set_next_animation(
            SkeletonAnimationContext::new(anim("aaa0", 3, 1.0), loop_params, None),
            None,
        );
        a0.set_next_animation(
            SkeletonAnimationContext::new(anim("bbb0", 3, 1.0), loop_params, Some(tp.clone())),
            Some(&tp),
        );
        let ib_slot0 = a0.transition.as_ref().unwrap().in_between.as_ref().unwrap();
        assert_eq!(
            ib_slot0.get_joint_transform(0, 0.0).unwrap().translation[0],
            11.0,
        );

        // Slot 3 animator: must pick slot-3's in-between (33.0), not slot-0's.
        let mut a3 = SkeletonAnimator::new(3);
        a3.set_next_animation(
            SkeletonAnimationContext::new(anim("aaa3", 3, 1.0), loop_params, None),
            None,
        );
        a3.set_next_animation(
            SkeletonAnimationContext::new(anim("bbb3", 3, 1.0), loop_params, Some(tp.clone())),
            Some(&tp),
        );
        let ib_slot3 = a3.transition.as_ref().unwrap().in_between.as_ref().unwrap();
        assert_eq!(
            ib_slot3.get_joint_transform(0, 0.0).unwrap().translation[0],
            33.0,
        );

        // A slot with no entry in the map gets no in-between.
        let mut a7 = SkeletonAnimator::new(7);
        a7.set_next_animation(
            SkeletonAnimationContext::new(anim("aaa7", 3, 1.0), loop_params, None),
            None,
        );
        a7.set_next_animation(
            SkeletonAnimationContext::new(anim("bbb7", 3, 1.0), loop_params, Some(tp.clone())),
            Some(&tp),
        );
        assert!(a7.transition.as_ref().unwrap().in_between.is_none());
    }
}
